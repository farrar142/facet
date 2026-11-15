//! [`crate::file_ops`]의 Windows 구현. `unsafe`는 이 파일 안에 가둔다.
//!
//! 경로를 직접 Windows API에 넘기는 곳(`CopyFile2`, `MoveFileExW`, `SetFileAttributesW`)은 [`extended`]로 확장 길이
//! 경로(`\\?\`)를 만든다. std로 여는 곳은 std가 긴 경로를 스스로 처리한다.

use std::any::Any;
use std::ffi::c_void;
use std::fs::{self, File, FileTimes, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{FileTimesExt, MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    COPY_FILE_FAIL_IF_EXISTS, COPY_FILE_NO_BUFFERING, COPYFILE2_CALLBACK_CHUNK_FINISHED,
    COPYFILE2_EXTENDED_PARAMETERS, COPYFILE2_MESSAGE, COPYFILE2_MESSAGE_ACTION,
    COPYFILE2_PROGRESS_CANCEL, COPYFILE2_PROGRESS_CONTINUE, CopyFile2, CreateSymbolicLinkW,
    FILE_ATTRIBUTE_NORMAL, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_FLAGS_AND_ATTRIBUTES, FILE_ID_INFO, FILE_WRITE_ATTRIBUTES, FileIdInfo,
    GetFileInformationByHandleEx, MOVE_FILE_FLAGS, MOVEFILE_REPLACE_EXISTING, MoveFileExW,
    SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE, SYMBOLIC_LINK_FLAG_DIRECTORY,
    SYMBOLIC_LINK_FLAGS, SetFileAttributesW,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::core::PCWSTR;

use crate::file_ops::{
    CopyProgress, CopyStep, FileId, FileOpError, FileOpErrorKind, FileOpResult, Link, LinkKind,
    MAX_REPARSE_BUFFER, UNBUFFERED_COPY_THRESHOLD, junction_buffer, nt_from_win32, parse_reparse,
    win32_from_hresult,
};

/// 재분석 지점 읽기(`FSCTL_GET_REPARSE_POINT`).
const FSCTL_GET_REPARSE_POINT: u32 = 0x0009_00A8;
/// 재분석 지점 걸기(`FSCTL_SET_REPARSE_POINT`).
const FSCTL_SET_REPARSE_POINT: u32 = 0x0009_00A4;

const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x20;
const FILE_ATTRIBUTE_NOT_CONTENT_INDEXED: u32 = 0x2000;

/// 폴더에 맞출 수 있는 속성(`SetFileAttributesW`가 받는 것 가운데 폴더에 뜻이 있는 것).
const FOLDER_ATTRIBUTES: u32 = FILE_ATTRIBUTE_READONLY
    | FILE_ATTRIBUTE_HIDDEN
    | FILE_ATTRIBUTE_SYSTEM
    | FILE_ATTRIBUTE_ARCHIVE
    | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED;

/// 경로를 확장 길이 경로(`\\?\C:\...`, `\\?\UNC\server\share\...`)의 NUL로 끝나는 UTF-16으로 바꾼다.
///
/// 확장 길이 경로는 Windows가 정리하지 않으므로 먼저 절대 경로로 정리한다(`.`·`..`·`/`). 이미 `\\?\`·`\\.\`로
/// 시작하면 그대로 둔다.
pub(crate) fn extended(path: &Path) -> FileOpResult<Vec<u16>> {
    let absolute = std::path::absolute(path).map_err(from_io)?;
    let wide: Vec<u16> = absolute.as_os_str().encode_wide().collect();
    let starts = |prefix: &str| {
        let prefix: Vec<u16> = prefix.encode_utf16().collect();
        wide.starts_with(&prefix)
    };
    let mut out: Vec<u16> = if starts(r"\\?\") || starts(r"\\.\") {
        wide
    } else if starts(r"\\") {
        r"\\?\UNC\"
            .encode_utf16()
            .chain(wide[2..].iter().copied())
            .collect()
    } else {
        r"\\?\".encode_utf16().chain(wide).collect()
    };
    out.push(0);
    Ok(out)
}

pub(crate) fn from_io(error: io::Error) -> FileOpError {
    match error.raw_os_error() {
        Some(code) => FileOpError::from_win32(code as u32),
        None => FileOpError::new(FileOpErrorKind::Other, None),
    }
}

pub(crate) fn from_windows(error: &windows::core::Error) -> FileOpError {
    let hresult = error.code().0;
    match win32_from_hresult(hresult) {
        Some(code) => FileOpError::from_win32(code),
        None => FileOpError::new(FileOpErrorKind::Other, Some(hresult)),
    }
}

/// 「찾을 수 없음」인데 원본이 실제로 있으면 대상 쪽(대상 폴더 없음 등) 문제이므로 「그 밖」으로 바꾼다.
fn refine_missing(error: FileOpError, source: &Path) -> FileOpError {
    if error.kind() == FileOpErrorKind::SourceMissing && fs::symlink_metadata(source).is_ok() {
        FileOpError::new(FileOpErrorKind::Other, error.code())
    } else {
        error
    }
}

/// 원본의 메타데이터. 없으면 「원본 없음」.
fn source_metadata(source: &Path) -> FileOpResult<fs::Metadata> {
    fs::metadata(source).map_err(from_io)
}

/// 진행 콜백에 넘기는 문맥.
struct Context<'a> {
    progress: &'a mut dyn FnMut(CopyProgress) -> CopyStep,
    cancelled: bool,
    panic: Option<Box<dyn Any + Send>>,
}

unsafe extern "system" fn on_progress(
    message: *const COPYFILE2_MESSAGE,
    context: *const c_void,
) -> COPYFILE2_MESSAGE_ACTION {
    // SAFETY: CopyFile2는 `copy_file`이 넘긴 문맥 포인터를 그대로 돌려주고, 이 콜백은 CopyFile2가 돌아오기 전에
    // 같은 스레드에서만 불린다. 그동안 `copy_file`은 문맥을 다른 곳에서 빌리지 않는다.
    let context = unsafe { &mut *(context as *mut Context<'_>) };
    // SAFETY: CopyFile2가 넘긴 메시지는 이 호출 동안 유효하다.
    let message = unsafe { &*message };
    if message.Type != COPYFILE2_CALLBACK_CHUNK_FINISHED || context.cancelled {
        return if context.cancelled {
            COPYFILE2_PROGRESS_CANCEL
        } else {
            COPYFILE2_PROGRESS_CONTINUE
        };
    }
    // SAFETY: 메시지 종류가 CHUNK_FINISHED이므로 공용체의 ChunkFinished가 채워져 있다.
    let info = unsafe { message.Info.ChunkFinished };
    let progress = CopyProgress {
        copied: info.uliTotalBytesTransferred,
        total: info.uliTotalFileSize,
    };
    // 패닉이 FFI 경계를 넘으면 프로세스가 끝나므로 여기서 받아 두었다가 CopyFile2가 돌아온 뒤 다시 일으킨다.
    match panic::catch_unwind(AssertUnwindSafe(|| (context.progress)(progress))) {
        Ok(CopyStep::Continue) => COPYFILE2_PROGRESS_CONTINUE,
        Ok(CopyStep::Cancel) => {
            context.cancelled = true;
            COPYFILE2_PROGRESS_CANCEL
        }
        Err(payload) => {
            context.panic = Some(payload);
            context.cancelled = true;
            COPYFILE2_PROGRESS_CANCEL
        }
    }
}

pub(crate) fn copy_file(
    source: &Path,
    target: &Path,
    progress: &mut dyn FnMut(CopyProgress) -> CopyStep,
) -> FileOpResult<()> {
    let metadata = source_metadata(source)?;
    let source_wide = extended(source)?;
    let target_wide = extended(target)?;
    let mut flags = COPY_FILE_FAIL_IF_EXISTS;
    if metadata.len() >= UNBUFFERED_COPY_THRESHOLD {
        flags |= COPY_FILE_NO_BUFFERING;
    }
    let mut context = Context {
        progress,
        cancelled: false,
        panic: None,
    };
    let parameters = COPYFILE2_EXTENDED_PARAMETERS {
        dwSize: std::mem::size_of::<COPYFILE2_EXTENDED_PARAMETERS>() as u32,
        dwCopyFlags: flags,
        pfCancel: std::ptr::null_mut(),
        pProgressRoutine: Some(on_progress),
        pvCallbackContext: &mut context as *mut Context<'_> as *mut c_void,
    };
    // SAFETY: 두 경로는 NUL로 끝나는 UTF-16이고 호출 동안 살아 있다. 매개변수 구조체는 dwSize를 채웠고, 문맥
    // 포인터는 호출 동안 살아 있는 `context`를 가리키며 콜백만 그것을 쓴다.
    let result = unsafe {
        CopyFile2(
            PCWSTR(source_wide.as_ptr()),
            PCWSTR(target_wide.as_ptr()),
            Some(&parameters),
        )
    };
    let Context {
        cancelled, panic, ..
    } = context;
    if let Some(payload) = panic {
        remove_partial(target);
        panic::resume_unwind(payload);
    }
    if let Err(error) = result {
        let error = if cancelled {
            FileOpError::new(FileOpErrorKind::Cancelled, None)
        } else {
            refine_missing(from_windows(&error), source)
        };
        // 대상이 이미 있었으면 그것은 남의 파일이다. 그 밖에는 이 복사가 만든 것이므로 남기지 않는다.
        if error.kind() != FileOpErrorKind::TargetExists {
            remove_partial(target);
        }
        return Err(error);
    }
    // CopyFile2는 수정 시각·속성·대체 데이터 스트림을 옮기지만 만든 시각은 지금으로 둔다. 원본 것으로 맞춘다.
    if let Err(error) = set_times(target, &metadata, false) {
        remove_partial(target);
        return Err(error);
    }
    Ok(())
}

/// 만들다 만 대상을 지운다. 실패해도 더 할 수 있는 것이 없다.
fn remove_partial(target: &Path) {
    let _ = remove_file(target);
}

/// `target`의 만든 시각·수정 시각을 `metadata`의 것으로 맞춘다.
fn set_times(target: &Path, metadata: &fs::Metadata, folder: bool) -> FileOpResult<()> {
    let mut options = OpenOptions::new();
    options.access_mode(FILE_WRITE_ATTRIBUTES.0);
    if folder {
        options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0);
    }
    let file = options.open(target).map_err(from_io)?;
    let mut times = FileTimes::new();
    if let Ok(created) = metadata.created() {
        times = times.set_created(created);
    }
    if let Ok(modified) = metadata.modified() {
        times = times.set_modified(modified);
    }
    file.set_times(times).map_err(from_io)
}

fn set_attributes(path: &Path, attributes: u32) -> FileOpResult<()> {
    let wide = extended(path)?;
    let attributes = if attributes == 0 {
        FILE_ATTRIBUTE_NORMAL
    } else {
        FILE_FLAGS_AND_ATTRIBUTES(attributes)
    };
    // SAFETY: 경로는 NUL로 끝나는 UTF-16이고 호출 동안 살아 있다.
    unsafe { SetFileAttributesW(PCWSTR(wide.as_ptr()), attributes) }
        .map_err(|error| from_windows(&error))
}

pub(crate) fn create_folder(path: &Path) -> FileOpResult<()> {
    fs::create_dir(path).map_err(|error| {
        let error = from_io(error);
        // 만들 폴더에는 원본이 없다. 찾을 수 없음은 부모 폴더가 없다는 뜻이다.
        if error.kind() == FileOpErrorKind::SourceMissing {
            FileOpError::new(FileOpErrorKind::Other, error.code())
        } else {
            error
        }
    })
}

pub(crate) fn copy_folder_metadata(source: &Path, target: &Path) -> FileOpResult<()> {
    let metadata = source_metadata(source)?;
    let missing_target = |error: FileOpError| refine_missing(error, source);
    apply_folder_metadata(&metadata, target).map_err(missing_target)
}

pub(crate) fn apply_folder_metadata(metadata: &fs::Metadata, target: &Path) -> FileOpResult<()> {
    // 읽기 전용 속성은 시각 맞추기를 막지 않지만, 순서를 시각 → 속성으로 두어 걱정할 것을 없앤다.
    set_times(target, metadata, true)?;
    set_attributes(target, metadata.file_attributes() & FOLDER_ATTRIBUTES)
}

fn move_file(source: &Path, target: &Path, flags: MOVE_FILE_FLAGS) -> FileOpResult<()> {
    let source_wide = extended(source)?;
    let target_wide = extended(target)?;
    // SAFETY: 두 경로는 NUL로 끝나는 UTF-16이고 호출 동안 살아 있다. MOVEFILE_COPY_ALLOWED를 주지 않으므로 다른
    // 볼륨이면 복사하지 않고 실패한다.
    unsafe {
        MoveFileExW(
            PCWSTR(source_wide.as_ptr()),
            PCWSTR(target_wide.as_ptr()),
            flags,
        )
    }
    .map_err(|error| refine_missing(from_windows(&error), source))
}

pub(crate) fn rename(source: &Path, target: &Path) -> FileOpResult<()> {
    move_file(source, target, MOVE_FILE_FLAGS(0))
}

pub(crate) fn replace(replacement: &Path, target: &Path) -> FileOpResult<()> {
    // 링크 자체도 바꿔 넣으므로 따라가지 않고 있는지만 본다(가리키는 곳이 없는 링크도).
    fs::symlink_metadata(replacement).map_err(from_io)?;
    let readonly = fs::symlink_metadata(target)
        .ok()
        .map(|metadata| metadata.file_attributes())
        .filter(|attributes| attributes & FILE_ATTRIBUTE_READONLY != 0);
    if let Some(attributes) = readonly {
        set_attributes(target, attributes & !FILE_ATTRIBUTE_READONLY & 0xFFFF)?;
    }
    let result = move_file(replacement, target, MOVEFILE_REPLACE_EXISTING);
    if result.is_err()
        && let Some(attributes) = readonly
    {
        let _ = set_attributes(target, attributes & 0xFFFF);
    }
    result
}

/// `remove`가 접근 거부로 실패하고 대상이 읽기 전용이면 읽기 전용을 풀고 한 번 더 한다. 그래도 안 되면 되돌린다.
fn remove_with(path: &Path, remove: impl Fn(&Path) -> io::Result<()>) -> FileOpResult<()> {
    let error = match remove(path) {
        Ok(()) => return Ok(()),
        Err(error) => from_io(error),
    };
    let attributes = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata.file_attributes(),
        Err(_) => return Err(error),
    };
    if error.kind() != FileOpErrorKind::AccessDenied || attributes & FILE_ATTRIBUTE_READONLY == 0 {
        return Err(error);
    }
    set_attributes(path, attributes & !FILE_ATTRIBUTE_READONLY & 0xFFFF)?;
    remove(path).map_err(|error| {
        let _ = set_attributes(path, attributes & 0xFFFF);
        from_io(error)
    })
}

pub(crate) fn remove_file(path: &Path) -> FileOpResult<()> {
    remove_with(path, |path| fs::remove_file(path))
}

pub(crate) fn remove_empty_folder(path: &Path) -> FileOpResult<()> {
    remove_with(path, |path| fs::remove_dir(path))
}

pub(crate) fn file_id(path: &Path) -> FileOpResult<FileId> {
    let file: File = OpenOptions::new()
        .access_mode(0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
        .open(path)
        .map_err(from_io)?;
    let mut info = FILE_ID_INFO::default();
    // SAFETY: 핸들은 살아 있는 `file`의 것이고, 버퍼는 크기를 정확히 넘긴 지역 FILE_ID_INFO다.
    unsafe {
        GetFileInformationByHandleEx(
            HANDLE(file.as_raw_handle()),
            FileIdInfo,
            &mut info as *mut FILE_ID_INFO as *mut c_void,
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    }
    .map_err(|error| from_windows(&error))?;
    Ok(FileId {
        volume_serial: info.VolumeSerialNumber,
        file_id: info.FileId.Identifier,
    })
}

/// 재분석 지점 자체를 여는 옵션(따라가지 않는다, 폴더도 연다).
fn reparse_point_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0);
    options
}

pub(crate) fn read_link(path: &Path) -> FileOpResult<Link> {
    let metadata = fs::symlink_metadata(path).map_err(from_io)?;
    let file = reparse_point_options()
        .access_mode(0)
        .open(path)
        .map_err(from_io)?;
    let mut buffer = vec![0u8; MAX_REPARSE_BUFFER];
    let mut returned = 0u32;
    // SAFETY: 핸들은 살아 있는 `file`의 것이고, 출력 버퍼는 넘긴 길이만큼 쓸 수 있는 지역 Vec이다. 겹친 입출력이
    // 아니므로 받은 바이트 수를 받을 `returned`를 넘기고, 호출이 끝나면 버퍼를 더 쓰지 않는다.
    unsafe {
        DeviceIoControl(
            HANDLE(file.as_raw_handle()),
            FSCTL_GET_REPARSE_POINT,
            None,
            0,
            Some(buffer.as_mut_ptr() as *mut c_void),
            buffer.len() as u32,
            Some(&mut returned),
            None,
        )
    }
    .map_err(|error| from_windows(&error))?;
    buffer.truncate(returned as usize);
    // std의 `is_dir`은 링크면 거짓이라 디렉터리 속성을 직접 본다.
    let is_folder = metadata.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0;
    parse_reparse(&buffer, is_folder).ok_or_else(|| FileOpError::new(FileOpErrorKind::Other, None))
}

pub(crate) fn create_link(path: &Path, link: &Link) -> FileOpResult<()> {
    match link.kind {
        LinkKind::Junction => create_junction(path, &link.target),
        LinkKind::FileSymlink => create_symlink(path, link, false),
        LinkKind::FolderSymlink => create_symlink(path, link, true),
    }
}

/// 만들 링크의 「찾을 수 없음」은 부모 폴더가 없다는 뜻이다(원본이 아니다).
fn creation_error(error: FileOpError) -> FileOpError {
    if error.kind() == FileOpErrorKind::SourceMissing {
        FileOpError::new(FileOpErrorKind::Other, error.code())
    } else {
        error
    }
}

/// `ERROR_INVALID_PARAMETER`: 개발자 모드 플래그를 모르는 옛 Windows.
const ERROR_INVALID_PARAMETER: i32 = 87;

fn create_symlink(path: &Path, link: &Link, folder: bool) -> FileOpResult<()> {
    let path_wide = extended(path)?;
    // 상대 경로는 적힌 그대로 둔다. 절대 경로는 260자를 넘을 때만 확장 길이 경로로 넘긴다(아니면 탐색기가 보이는
    // 표시 이름에 `\\?\`가 붙는다).
    let target_wide: Vec<u16> =
        if link.relative || link.target.as_os_str().encode_wide().count() < 260 {
            link.target
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect()
        } else {
            extended(&link.target)?
        };
    let base = if folder {
        SYMBOLIC_LINK_FLAG_DIRECTORY
    } else {
        SYMBOLIC_LINK_FLAGS(0)
    };
    let create = |flags: SYMBOLIC_LINK_FLAGS| {
        // SAFETY: 두 경로는 NUL로 끝나는 UTF-16이고 호출 동안 살아 있다. 실패 코드는 곧바로 같은 스레드에서 읽는다.
        let created = unsafe {
            CreateSymbolicLinkW(
                PCWSTR(path_wide.as_ptr()),
                PCWSTR(target_wide.as_ptr()),
                flags,
            )
        };
        if created {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    };
    // 개발자 모드면 관리자가 아니어도 만들 수 있게 청한다. 그 플래그를 모르는 Windows는 빼고 다시 한다.
    let result = match create(base | SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE) {
        Err(error) if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER) => create(base),
        result => result,
    };
    result.map_err(|error| creation_error(from_io(error)))
}

/// 정션은 만드는 API가 없어 빈 폴더를 만든 뒤 마운트 지점 재분석 데이터를 건다. 실패하면 그 폴더를 지운다.
fn create_junction(path: &Path, target: &Path) -> FileOpResult<()> {
    let invalid = || FileOpError::new(FileOpErrorKind::Other, None);
    let print = target.to_str().ok_or_else(invalid)?;
    let substitute = nt_from_win32(print).ok_or_else(invalid)?;
    let buffer = junction_buffer(&substitute, print).ok_or_else(invalid)?;
    fs::create_dir(path).map_err(|error| creation_error(from_io(error)))?;
    let result = (|| {
        let file = reparse_point_options()
            .write(true)
            .open(path)
            .map_err(from_io)?;
        let mut returned = 0u32;
        // SAFETY: 핸들은 살아 있는 `file`의 것이고, 입력 버퍼는 넘긴 길이만큼 읽을 수 있는 지역 Vec이다. 겹친
        // 입출력이 아니므로 받은 바이트 수를 받을 `returned`를 넘긴다.
        unsafe {
            DeviceIoControl(
                HANDLE(file.as_raw_handle()),
                FSCTL_SET_REPARSE_POINT,
                Some(buffer.as_ptr() as *const c_void),
                buffer.len() as u32,
                None,
                0,
                Some(&mut returned),
                None,
            )
        }
        .map_err(|error| from_windows(&error))
    })();
    if result.is_err() {
        let _ = fs::remove_dir(path);
    }
    result
}

pub(crate) fn remove_link(path: &Path) -> FileOpResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(from_io)?;
    // 폴더 링크·정션은 디렉터리 항목이라 폴더로 지운다(가리키는 폴더의 내용은 건드리지 않는다).
    if metadata.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0 {
        remove_with(path, |path| fs::remove_dir(path))
    } else {
        remove_with(path, |path| fs::remove_file(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(wide: &[u16]) -> String {
        String::from_utf16(&wide[..wide.len() - 1]).unwrap()
    }

    #[test]
    fn 확장_길이_경로를_만든다() {
        assert_eq!(
            text(&extended(Path::new(r"C:\가\..\나/다.txt")).unwrap()),
            r"\\?\C:\나\다.txt"
        );
        assert_eq!(
            text(&extended(Path::new(r"\\server\share\a")).unwrap()),
            r"\\?\UNC\server\share\a"
        );
        assert_eq!(
            text(&extended(Path::new(r"\\?\C:\a")).unwrap()),
            r"\\?\C:\a"
        );
    }
}
