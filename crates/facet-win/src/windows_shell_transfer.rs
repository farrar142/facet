//! 셸 항목과 PC 사이 파일 전송의 Windows 부분([`crate::shell_transfer`]). `unsafe`는 이 파일 안에 있다. 함수는 COM을
//! STA로 초기화한 스레드에서 불린다.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, IStream};
use windows::Win32::UI::Shell::{
    BHID_Stream, FOF_NO_UI, FileOperation, IFileOperation, IFileOperationProgressSink, IShellItem,
    IShellItem2, SHCreateItemFromParsingName,
};
use windows::core::{GUID, Interface, PCWSTR};

use crate::file_ops::{FileOpError, FileOpErrorKind, FileOpResult};
use crate::shell_folder::{ShellEntry, ShellFolderError};
use crate::shell_transfer::{READ_CHUNK, ShellChild};
use crate::windows_file_ops::{from_io, from_windows};
use crate::windows_impl::to_wide;
use crate::windows_shell_folder::{OwnedId, take_string};

/// `PKEY_FileName`(`System.FileName`): 확장자를 포함한 파일 이름.
const PKEY_FILE_NAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x41CF5AE0_F75A_4806_BD87_59C7D9248EB9),
    pid: 100,
};

/// 셸 폴더 실패를 파일 연산 실패로: 항목이 없으면 「원본 없음」, 그 밖은 셸 코드 그대로.
fn shell_error(error: ShellFolderError) -> FileOpError {
    match error {
        ShellFolderError::NotFound => FileOpError::new(FileOpErrorKind::SourceMissing, None),
        ShellFolderError::Failed { code } => from_windows(&windows::core::Error::from_hresult(
            windows::core::HRESULT(code),
        )),
        ShellFolderError::InvalidId => FileOpError::new(FileOpErrorKind::Other, None),
        ShellFolderError::Unsupported => FileOpError::new(FileOpErrorKind::Unsupported, None),
    }
}

fn item_of(id: &[u8]) -> FileOpResult<IShellItem> {
    if !crate::shell_folder::is_valid_id(id) {
        return Err(FileOpError::new(FileOpErrorKind::Other, None));
    }
    OwnedId::copy(id)
        .and_then(|owned| owned.item())
        .map_err(shell_error)
}

pub(crate) fn file_name(id: &[u8]) -> FileOpResult<String> {
    let item = item_of(id)?;
    if let Ok(item2) = item.cast::<IShellItem2>() {
        // SAFETY: item2는 살아 있는 셸 항목이고 속성 열쇠는 정적 값이다. 돌려받은 글자는 take_string이 푼다.
        if let Ok(text) = unsafe { item2.GetString(&PKEY_FILE_NAME) } {
            let name = take_string(text);
            if !name.is_empty() {
                return Ok(name);
            }
        }
    }
    crate::windows_shell_folder::display_name(id).map_err(shell_error)
}

pub(crate) fn children(id: &[u8]) -> FileOpResult<Vec<ShellChild>> {
    let mut entries: Vec<ShellEntry> = Vec::new();
    crate::windows_shell_folder::enumerate(id, 256, true, &mut |batch| {
        entries.extend(batch);
        true
    })
    .map_err(shell_error)?;
    Ok(entries
        .into_iter()
        .map(|entry| ShellChild {
            name: file_name(&entry.id).unwrap_or(entry.name),
            is_folder: entry.is_folder,
            id: entry.id,
            size: entry.size,
            modified: entry.modified,
        })
        .collect())
}

pub(crate) fn read_to_file(
    id: &[u8],
    target: &Path,
    on_progress: &mut dyn FnMut(u64) -> bool,
) -> FileOpResult<u64> {
    let item = item_of(id)?;
    // SAFETY: item은 살아 있는 셸 항목이고 바인드 문맥은 주지 않는다.
    let stream: IStream =
        unsafe { item.BindToHandler(None, &BHID_Stream) }.map_err(|e| from_windows(&e))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(from_io)?;
    let mut buffer = vec![0u8; READ_CHUNK];
    let mut total = 0u64;
    let result = loop {
        let mut read = 0u32;
        // SAFETY: buffer는 READ_CHUNK바이트의 쓸 수 있는 메모리이고 스트림은 read바이트까지만 채운다.
        let hresult = unsafe {
            stream.Read(
                buffer.as_mut_ptr().cast(),
                READ_CHUNK as u32,
                Some(&mut read),
            )
        };
        if hresult.is_err() {
            break Err(from_windows(&windows::core::Error::from_hresult(hresult)));
        }
        if read == 0 {
            break file.flush().map(|()| total).map_err(from_io);
        }
        if let Err(error) = file.write_all(&buffer[..read as usize]) {
            break Err(from_io(error));
        }
        total += u64::from(read);
        if !on_progress(total) {
            break Err(FileOpError::new(FileOpErrorKind::Cancelled, None));
        }
    };
    drop(file);
    if result.is_err() {
        let _ = fs::remove_file(target);
    }
    result
}

/// 대화 없는(`FOF_NO_UI`) 셸 파일 작업 하나를 만들고 `add`로 넣은 작업을 실행한다.
fn perform(add: impl FnOnce(&IFileOperation) -> windows::core::Result<()>) -> FileOpResult<()> {
    // SAFETY: 이 스레드는 COM STA로 초기화돼 있다(ShellThread). FileOperation은 IFileOperation을 구현하는 셸 클래스다.
    let operation: IFileOperation = unsafe { CoCreateInstance(&FileOperation, None, CLSCTX_ALL) }
        .map_err(|e| from_windows(&e))?;
    // SAFETY: operation은 살아 있는 COM 객체이고 플래그는 상수다. FOF_NO_UI: 진행 창·확인·오류 대화를 띄우지 않는다
    // (같은 이름은 바꿔치기, 삭제는 FOF_ALLOWUNDO가 없어 휴지통을 거치지 않는다).
    unsafe { operation.SetOperationFlags(FOF_NO_UI) }.map_err(|e| from_windows(&e))?;
    add(&operation).map_err(|e| from_windows(&e))?;
    // SAFETY: operation은 살아 있는 COM 객체이고, 넣은 작업을 이 스레드에서 끝까지 실행한다.
    unsafe { operation.PerformOperations() }.map_err(|e| from_windows(&e))?;
    // SAFETY: operation은 살아 있는 COM 객체이고 작업을 다 실행했다.
    let aborted = unsafe { operation.GetAnyOperationsAborted() }.map_err(|e| from_windows(&e))?;
    if aborted.as_bool() {
        return Err(FileOpError::new(FileOpErrorKind::Other, None));
    }
    Ok(())
}

pub(crate) fn copy_into(source: &Path, folder: &[u8], name: Option<&str>) -> FileOpResult<()> {
    let destination = item_of(folder)?;
    let wide = to_wide(source.as_os_str());
    // SAFETY: wide는 NUL로 끝나는 UTF-16 경로이고 호출이 끝날 때까지 살아 있다. 바인드 문맥은 주지 않는다.
    let item: IShellItem = unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None) }
        .map_err(|e| from_windows(&e))?;
    let name = name.map(|name| to_wide(std::ffi::OsStr::new(name)));
    perform(|operation| {
        let new_name = name
            .as_ref()
            .map_or(PCWSTR::null(), |name| PCWSTR(name.as_ptr()));
        // SAFETY: 두 셸 항목과 operation은 살아 있는 COM 객체이고, 새 이름은 NUL로 끝나는 글자(또는 null: 원래 이름)다.
        unsafe {
            operation.CopyItem(
                &item,
                &destination,
                new_name,
                None::<&IFileOperationProgressSink>,
            )
        }
    })
}

pub(crate) fn new_folder(folder: &[u8], name: &str) -> FileOpResult<()> {
    let destination = item_of(folder)?;
    let wide = to_wide(std::ffi::OsStr::new(name));
    perform(|operation| {
        // SAFETY: 셸 항목과 operation은 살아 있는 COM 객체이고, 이름은 NUL로 끝나는 글자다. 서식 파일은 주지 않는다.
        unsafe {
            operation.NewItem(
                &destination,
                FILE_ATTRIBUTE_DIRECTORY.0,
                PCWSTR(wide.as_ptr()),
                PCWSTR::null(),
                None::<&IFileOperationProgressSink>,
            )
        }
    })
}

pub(crate) fn delete(id: &[u8]) -> FileOpResult<()> {
    let item = item_of(id)?;
    perform(|operation| {
        // SAFETY: 셸 항목과 operation은 살아 있는 COM 객체다.
        unsafe { operation.DeleteItem(&item, None::<&IFileOperationProgressSink>) }
    })
}
