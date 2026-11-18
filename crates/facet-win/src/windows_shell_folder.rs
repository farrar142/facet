//! 셸 폴더 열거의 Windows 부분([`crate::shell_folder`]). `unsafe`는 이 파일 안에 있다. 함수는 COM을 STA로 초기화한
//! 스레드에서 불린다.

use std::path::PathBuf;

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, PROPERTYKEY};
use windows::Win32::System::Com::{CoTaskMemAlloc, CoTaskMemFree};
use windows::Win32::System::SystemServices::{SFGAO_FILESYSTEM, SFGAO_FOLDER};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_EnumItems, IEnumShellItems, ILGetSize, IShellItem, IShellItem2, SHCreateItemFromIDList,
    SHGetIDListFromObject, SHParseDisplayName, SIGDN, SIGDN_DESKTOPABSOLUTEPARSING,
    SIGDN_FILESYSPATH, SIGDN_NORMALDISPLAY,
};
use windows::core::{GUID, HSTRING, Interface, PWSTR};

/// 셸 속성 묶음 `FMTID_Storage`(크기·수정한 날짜가 든 묶음).
const FMTID_STORAGE: GUID = GUID::from_u128(0xB725F130_47EF_101A_A5F1_02608C9EEBAC);
/// `PKEY_Size`: 항목 크기(바이트).
const PKEY_SIZE: PROPERTYKEY = PROPERTYKEY {
    fmtid: FMTID_STORAGE,
    pid: 12,
};
/// `PKEY_DateModified`: 수정한 날짜(`FILETIME`).
const PKEY_DATE_MODIFIED: PROPERTYKEY = PROPERTYKEY {
    fmtid: FMTID_STORAGE,
    pid: 14,
};

use crate::shell_folder::{ShellEntry, ShellFolderError};

fn failed(error: &windows::core::Error) -> ShellFolderError {
    let code = error.code();
    if code == ERROR_FILE_NOT_FOUND.to_hresult() || code == ERROR_PATH_NOT_FOUND.to_hresult() {
        ShellFolderError::NotFound
    } else {
        ShellFolderError::Failed { code: code.0 }
    }
}

/// 셸이 준 글자(`CoTaskMemAlloc`)를 읽고 푼다.
fn take_string(text: PWSTR) -> String {
    // SAFETY: text는 셸이 돌려준 NUL로 끝나는 UTF-16 글자다. 읽은 뒤 한 번만 푼다.
    let value = unsafe { text.to_string() }.unwrap_or_default();
    // SAFETY: 셸이 CoTaskMemAlloc으로 준 메모리이고 여기서 한 번만 푼다.
    unsafe { CoTaskMemFree(Some(text.0 as *const _)) };
    value
}

fn name_of(item: &IShellItem, kind: SIGDN) -> Option<String> {
    // SAFETY: item은 살아 있는 셸 항목이다. 돌려받은 글자는 take_string이 푼다.
    let text = unsafe { item.GetDisplayName(kind) }.ok()?;
    Some(take_string(text))
}

/// 바이트를 정렬된 COM 메모리에 옮긴 PIDL. 버리면 푼다.
struct OwnedId(*mut ITEMIDLIST);

impl OwnedId {
    fn copy(id: &[u8]) -> Result<OwnedId, ShellFolderError> {
        // SAFETY: id.len()바이트를 할당한다. 실패하면 널이다.
        let memory = unsafe { CoTaskMemAlloc(id.len()) }.cast::<u8>();
        if memory.is_null() {
            return Err(ShellFolderError::Failed { code: 0 });
        }
        // SAFETY: memory는 방금 할당한 id.len()바이트이고 id와 겹치지 않는다.
        unsafe { std::ptr::copy_nonoverlapping(id.as_ptr(), memory, id.len()) };
        Ok(OwnedId(memory.cast()))
    }

    fn item(&self) -> Result<IShellItem, ShellFolderError> {
        // SAFETY: self.0은 형식을 확인한(is_valid_id) PIDL이고 호출 동안 살아 있다.
        unsafe { SHCreateItemFromIDList::<IShellItem>(self.0) }.map_err(|e| failed(&e))
    }
}

impl Drop for OwnedId {
    fn drop(&mut self) {
        // SAFETY: CoTaskMemAlloc으로 만든 메모리이고 한 번만 푼다.
        unsafe { CoTaskMemFree(Some(self.0 as *const _)) };
    }
}

/// 셸 항목의 절대 PIDL 바이트.
fn id_of(item: &IShellItem) -> Option<Vec<u8>> {
    // SAFETY: item은 살아 있는 셸 항목이다. 돌려받은 PIDL은 아래에서 한 번 푼다.
    let pidl = unsafe { SHGetIDListFromObject(item) }.ok()?;
    if pidl.is_null() {
        return None;
    }
    // SAFETY: pidl은 셸이 준 올바른 PIDL이다(끝의 0 두 바이트까지 크기에 든다).
    let size = unsafe { ILGetSize(Some(pidl)) } as usize;
    // SAFETY: pidl에서 size바이트를 읽는다. ILGetSize가 그만큼이 PIDL임을 알렸다.
    let bytes = unsafe { std::slice::from_raw_parts(pidl.cast::<u8>(), size) }.to_vec();
    // SAFETY: 셸이 CoTaskMemAlloc으로 준 PIDL이고 한 번만 푼다.
    unsafe { CoTaskMemFree(Some(pidl as *const _)) };
    Some(bytes)
}

pub(crate) fn display_name(id: &[u8]) -> Result<String, ShellFolderError> {
    let item = OwnedId::copy(id)?.item()?;
    name_of(&item, SIGDN_NORMALDISPLAY).ok_or(ShellFolderError::Failed { code: 0 })
}

pub(crate) fn parsing_name(id: &[u8]) -> Result<String, ShellFolderError> {
    let item = OwnedId::copy(id)?.item()?;
    name_of(&item, SIGDN_DESKTOPABSOLUTEPARSING).ok_or(ShellFolderError::Failed { code: 0 })
}

pub(crate) fn id_from_parsing_name(name: &str) -> Result<Vec<u8>, ShellFolderError> {
    let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
    // SAFETY: name은 NUL로 끝나는 글자(HSTRING)이고 pidl은 셸이 채울 칸이다. 성공하면 아래에서 한 번 푼다.
    unsafe { SHParseDisplayName(&HSTRING::from(name), None, &mut pidl, 0, None) }
        .map_err(|e| failed(&e))?;
    if pidl.is_null() {
        return Err(ShellFolderError::NotFound);
    }
    // SAFETY: pidl은 셸이 준 올바른 PIDL이다(끝의 0 두 바이트까지 크기에 든다).
    let size = unsafe { ILGetSize(Some(pidl)) } as usize;
    // SAFETY: pidl에서 size바이트를 읽는다. ILGetSize가 그만큼이 PIDL임을 알렸다.
    let bytes = unsafe { std::slice::from_raw_parts(pidl.cast::<u8>(), size) }.to_vec();
    // SAFETY: 셸이 CoTaskMemAlloc으로 준 PIDL이고 한 번만 푼다.
    unsafe { CoTaskMemFree(Some(pidl as *const _)) };
    Ok(bytes)
}

/// 항목의 크기와 수정한 날짜(`FILETIME`): 셸 속성에 있을 때만.
fn size_and_modified(item: &IShellItem, is_folder: bool) -> (Option<u64>, Option<u64>) {
    let Ok(item) = item.cast::<IShellItem2>() else {
        return (None, None);
    };
    let size = if is_folder {
        None
    } else {
        // SAFETY: item은 살아 있는 셸 항목이고 속성 열쇠는 정적 값이다.
        unsafe { item.GetUInt64(&PKEY_SIZE) }.ok()
    };
    // SAFETY: item은 살아 있는 셸 항목이고 속성 열쇠는 정적 값이다.
    let modified = unsafe { item.GetFileTime(&PKEY_DATE_MODIFIED) }
        .ok()
        .map(|time| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime))
        .filter(|&time| time != 0);
    (size, modified)
}

pub(crate) fn enumerate(
    id: &[u8],
    batch: usize,
    with_details: bool,
    on_batch: &mut dyn FnMut(Vec<ShellEntry>) -> bool,
) -> Result<(), ShellFolderError> {
    let folder = OwnedId::copy(id)?.item()?;
    // SAFETY: folder는 살아 있는 셸 항목이고 바인드 문맥은 주지 않는다.
    let items: IEnumShellItems =
        unsafe { folder.BindToHandler(None, &BHID_EnumItems) }.map_err(|e| failed(&e))?;
    let mut slots: Vec<Option<IShellItem>> = vec![None; batch.min(256)];
    let mut pending = Vec::with_capacity(batch);
    loop {
        let mut fetched = 0u32;
        // SAFETY: slots는 비어 있는 칸들이고 셸이 fetched개를 채운다(S_FALSE는 끝 가까이).
        unsafe { items.Next(&mut slots, Some(&mut fetched)) }.map_err(|e| failed(&e))?;
        if fetched == 0 {
            break;
        }
        for slot in slots.iter_mut().take(fetched as usize) {
            let Some(item) = slot.take() else {
                continue;
            };
            // SAFETY: item은 살아 있는 셸 항목이다.
            let attributes =
                unsafe { item.GetAttributes(SFGAO_FOLDER | SFGAO_FILESYSTEM) }.unwrap_or_default();
            let Some(id) = id_of(&item) else {
                continue;
            };
            let path = (attributes & SFGAO_FILESYSTEM == SFGAO_FILESYSTEM)
                .then(|| name_of(&item, SIGDN_FILESYSPATH))
                .flatten()
                .map(PathBuf::from);
            let is_folder = attributes & SFGAO_FOLDER == SFGAO_FOLDER;
            let (size, modified) = if with_details {
                size_and_modified(&item, is_folder)
            } else {
                (None, None)
            };
            pending.push(ShellEntry {
                name: name_of(&item, SIGDN_NORMALDISPLAY).unwrap_or_default(),
                path,
                parsing_name: name_of(&item, SIGDN_DESKTOPABSOLUTEPARSING).unwrap_or_default(),
                is_folder,
                id,
                size,
                modified,
            });
            if pending.len() >= batch && !on_batch(std::mem::take(&mut pending)) {
                return Ok(());
            }
        }
    }
    if !pending.is_empty() {
        on_batch(pending);
    }
    Ok(())
}
