//! [`crate::shell_menu`]의 Windows 구현: `IContextMenu`로 셸 메뉴를 만들고 읽고 실행한다. 셸 확장 도우미만 부른다.

use std::collections::HashMap;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_INVALID_DRIVE, ERROR_PATH_NOT_FOUND, HMODULE, HWND, LPARAM,
    LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BLACK_BRUSH, CreateCompatibleDC,
    CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, FillRect, GdiFlush, GetDIBits,
    GetObjectW, GetStockObject, HBITMAP, HBRUSH, HDC, HGDIOBJ, SelectObject, WHITE_BRUSH,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Diagnostics::Debug::{EXCEPTION_POINTERS, SetUnhandledExceptionFilter};
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleFileNameW, GetModuleHandleExW,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CLASSES_ROOT, KEY_READ, RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW, RegGetValueW,
    RegOpenKeyExW,
};
use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODA_DRAWENTIRE, ODT_MENU};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_SFObject, BHID_SFUIObject, CMF_NORMAL, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX,
    IContextMenu, IContextMenu2, IContextMenu3, ILFindLastID, IShellFolder, IShellItem,
    SHBindToParent, SHCreateItemFromIDList, SHCreateShellItemArrayFromIDLists, SHParseDisplayName,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, GetMenuItemCount, GetMenuItemInfoW, HBMMENU_CALLBACK, HMENU,
    MENUITEMINFOW, MFS_DISABLED, MFT_SEPARATOR, MIIM_BITMAP, MIIM_CHECKMARKS, MIIM_DATA,
    MIIM_FTYPE, MIIM_ID, MIIM_STATE, MIIM_STRING, MIIM_SUBMENU, SW_SHOWNORMAL, WM_DRAWITEM,
    WM_INITMENUPOPUP, WM_MEASUREITEM,
};
use windows::core::{HSTRING, Interface, PCSTR, PCWSTR, PWSTR};

use crate::icon::IconImage;
use crate::shell_menu::{Command, Entry, MAX_DEPTH, MenuError, MenuTarget, Submenu};
use crate::windows_icon::unblend;

/// 메뉴 항목 번호의 범위(셸에 넘기는 첫·끝 번호). 항목의 [`Command::id`]는 첫 번호에서 뺀 값이다.
const ID_FIRST: u32 = 1;
const ID_LAST: u32 = 0x7FFF;
/// `CMINVOKECOMMANDINFOEX`의 글자가 UTF-16임.
const CMIC_MASK_UNICODE: u32 = 0x0000_4000;
/// 실행이 끝날 때까지 돌아오지 않게 한다(돌아온 뒤 메뉴 객체를 놓아도 되게).
const CMIC_MASK_NOASYNC: u32 = 0x0000_0100;
/// `HBMMENU_*` 특수 값의 끝(이하 값은 그림 핸들이 아니다).
const HBMMENU_LAST_SPECIAL: isize = 11;
/// 하위 메뉴가 채워지기를 기다리는 가장 긴 시간.
///
/// 근거: 「보내기」는 메뉴를 만들 때 SendTo 폴더를 뒤에서 읽기 시작해 보통 수십~수백 ms 안에 끝난다(이 기계에서 잼). 끝내
/// 채워지지 않는 하위 메뉴는 이만큼만 메뉴 응답을 늦춘다.
const FILL_WAIT: Duration = Duration::from_millis(1500);
/// 하위 메뉴가 채워졌는지 다시 보는 간격.
const FILL_POLL: Duration = Duration::from_millis(10);
/// 아이콘 그림으로 받는 가장 큰 변(이보다 크면 받지 않는다).
const MAX_ICON_SIDE: i32 = 256;

fn failed(error: &windows::core::Error) -> MenuError {
    let code = error.code();
    let missing = [
        ERROR_FILE_NOT_FOUND,
        ERROR_PATH_NOT_FOUND,
        ERROR_INVALID_DRIVE,
    ]
    .iter()
    .any(|e| e.to_hresult() == code);
    if missing {
        MenuError::NotFound(error.message())
    } else {
        MenuError::Failed(format!("{:#010x} {}", code.0, error.message()))
    }
}

/// 셸이 준 절대 PIDL. 버리면 푼다.
struct Pidl(*mut ITEMIDLIST);

impl Drop for Pidl {
    fn drop(&mut self) {
        // SAFETY: SHParseDisplayName이 CoTaskMemAlloc으로 준 PIDL이고 한 번만 푼다.
        unsafe { CoTaskMemFree(Some(self.0 as *const _)) };
    }
}

fn parse(path: &Path) -> Result<Pidl, MenuError> {
    let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
    // SAFETY: 이름은 NUL로 끝나는 글자(HSTRING)이고 pidl은 셸이 채울 칸이다. 성공하면 Pidl이 푼다.
    unsafe { SHParseDisplayName(&HSTRING::from(path.as_os_str()), None, &mut pidl, 0, None) }
        .map_err(|e| failed(&e))?;
    if pidl.is_null() {
        return Err(MenuError::NotFound(path.display().to_string()));
    }
    Ok(Pidl(pidl))
}

/// 만든 셸 메뉴(메뉴 핸들과 메뉴 객체).
pub(crate) struct Menu {
    hmenu: HMENU,
    menu: IContextMenu,
}

impl Drop for Menu {
    fn drop(&mut self) {
        // SAFETY: CreatePopupMenu가 만든 메뉴이고 여기서 한 번만 지운다(하위 메뉴도 함께 지워진다). 메뉴 객체는 이 뒤
        // 필드가 버려질 때 놓인다.
        let _ = unsafe { DestroyMenu(self.hmenu) };
    }
}

impl Menu {
    pub(crate) fn build(target: &crate::shell_menu::MenuTarget) -> Result<Menu, MenuError> {
        note_handlers(target);
        let menu = match target {
            MenuTarget::Items(paths) => items_menu(paths)?,
            MenuTarget::Background(folder) => background_menu(folder)?,
        };
        // SAFETY: 인자 없는 호출이다. 실패하면 null이다.
        let hmenu = unsafe { CreatePopupMenu() }.map_err(|e| failed(&e))?;
        let built = Menu { hmenu, menu };
        // SAFETY: hmenu는 위에서 만든 빈 메뉴이고 번호 범위는 이 메뉴에서만 쓴다.
        unsafe {
            built
                .menu
                .QueryContextMenu(built.hmenu, 0, ID_FIRST, ID_LAST, CMF_NORMAL)
        }
        .ok()
        .map_err(|e| failed(&e))?;
        Ok(built)
    }

    pub(crate) fn entries(&self) -> Vec<Entry> {
        self.read(self.hmenu, 0)
    }

    pub(crate) fn invoke(&self, id: u32) -> Result<(), MenuError> {
        if id > ID_LAST - ID_FIRST {
            return Err(MenuError::UnknownCommand);
        }
        let info = CMINVOKECOMMANDINFOEX {
            cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
            fMask: CMIC_MASK_UNICODE | CMIC_MASK_NOASYNC,
            // 항목 번호는 「자원 번호」(MAKEINTRESOURCE) 꼴로 넘긴다.
            lpVerb: PCSTR(id as usize as *const u8),
            lpVerbW: PCWSTR(id as usize as *const u16),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        // SAFETY: info는 올바른 크기의 실행 정보이고 호출이 끝날 때까지 살아 있다. 확장 구조체는 앞부분이 기본 구조체와
        // 같아 그 주소로 넘기는 것이 셸의 약속이다.
        unsafe {
            self.menu
                .InvokeCommand(&info as *const CMINVOKECOMMANDINFOEX as *const CMINVOKECOMMANDINFO)
        }
        .map_err(|e| failed(&e))
    }

    /// 메뉴 하나를 읽는다.
    fn read(&self, hmenu: HMENU, depth: usize) -> Vec<Entry> {
        // SAFETY: hmenu는 이 메뉴(또는 그 하위 메뉴)의 핸들이다.
        let count = unsafe { GetMenuItemCount(Some(hmenu)) }.max(0) as u32;
        let mut entries = Vec::new();
        for index in 0..count {
            if let Some(entry) = self.read_item(hmenu, index, depth) {
                entries.push(entry);
            }
        }
        tidy_separators(entries)
    }

    fn read_item(&self, hmenu: HMENU, index: u32, depth: usize) -> Option<Entry> {
        let mut info = MENUITEMINFOW {
            cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_FTYPE
                | MIIM_STATE
                | MIIM_ID
                | MIIM_SUBMENU
                | MIIM_STRING
                | MIIM_BITMAP
                | MIIM_CHECKMARKS
                | MIIM_DATA,
            ..Default::default()
        };
        // SAFETY: info는 크기를 적은 지역 구조체이고 글자 칸이 없으므로 글자 길이(cch)만 받는다.
        unsafe { GetMenuItemInfoW(hmenu, index, true, &mut info) }.ok()?;
        if info.fType.0 & MFT_SEPARATOR.0 != 0 {
            return Some(Entry::Separator);
        }
        let text = item_text(hmenu, index, info.cch);
        if text.trim().is_empty() {
            // 글자 없는 항목(직접 그리는 항목 등)은 보일 수 없다.
            return None;
        }
        let enabled = info.fState.0 & MFS_DISABLED.0 == 0;
        if !info.hSubMenu.is_invalid() {
            if depth + 1 >= MAX_DEPTH {
                return None;
            }
            self.init_popup(info.hSubMenu, index);
            let icon = self.icon(&info);
            return Some(Entry::Submenu(Submenu {
                text,
                icon,
                enabled,
                items: self.read(info.hSubMenu, depth + 1),
            }));
        }
        if !(ID_FIRST..=ID_LAST).contains(&info.wID) {
            return None;
        }
        let icon = self.icon(&info);
        Some(Entry::Command(Command {
            id: info.wID - ID_FIRST,
            text,
            icon,
            enabled,
        }))
    }

    /// 탐색기가 하위 메뉴를 펼칠 때처럼 `WM_INITMENUPOPUP`을 메뉴 객체에 알려 채우게 한다.
    ///
    /// 「보내기」처럼 메뉴를 만들 때 뒤에서 목록을 읽기 시작하는 확장은 그 읽기가 끝나기 전에 펼치면 빈 자리 표시만 남긴다.
    /// 그래서 하위 메뉴에 글자 있는 항목이 없으면 메시지를 돌리며 [`FILL_WAIT`]까지 다시 알린다.
    fn init_popup(&self, submenu: HMENU, index: u32) {
        let wparam = WPARAM(submenu.0 as usize);
        let lparam = LPARAM(index as isize);
        let deadline = Instant::now() + FILL_WAIT;
        loop {
            let _ = self.message(WM_INITMENUPOPUP, wparam, lparam);
            if has_text(submenu) || Instant::now() >= deadline {
                return;
            }
            crate::shell_dialog::pump_messages();
            std::thread::sleep(FILL_POLL);
        }
    }

    /// 메뉴 객체에 창 메시지를 넘긴다(`IContextMenu3`, 없으면 `IContextMenu2`). 넘길 곳이 없거나 실패하면 `false`.
    ///
    /// `IContextMenu3`에는 결과 칸을 꼭 넘긴다: 「보내기」 확장은 결과 칸이 없으면 `WM_INITMENUPOPUP`에 성공으로 답하고도
    /// 하위 메뉴를 채우지 않는다.
    fn message(&self, message: u32, wparam: WPARAM, lparam: LPARAM) -> bool {
        if let Ok(menu3) = self.menu.cast::<IContextMenu3>() {
            let mut result = LRESULT(0);
            // SAFETY: 메시지 인자는 그 메시지의 약속대로 만든 값이고 가리키는 구조체와 결과 칸은 호출 동안 살아 있다.
            return unsafe { menu3.HandleMenuMsg2(message, wparam, lparam, Some(&mut result)) }
                .is_ok();
        }
        if let Ok(menu2) = self.menu.cast::<IContextMenu2>() {
            // SAFETY: 위와 같다.
            return unsafe { menu2.HandleMenuMsg(message, wparam, lparam) }.is_ok();
        }
        false
    }

    /// 항목 아이콘: 그림 핸들이면 그 그림, 그려 달라는 표시(`HBMMENU_CALLBACK`)면 메뉴 객체가 그린 그림, 아니면 체크
    /// 표시 그림(옛 확장이 아이콘을 거기 둠).
    fn icon(&self, info: &MENUITEMINFOW) -> Option<IconImage> {
        if info.hbmpItem == HBMMENU_CALLBACK {
            return self.drawn_icon(info);
        }
        bitmap_icon(info.hbmpItem).or_else(|| bitmap_icon(info.hbmpUnchecked))
    }

    /// 메뉴 객체에 크기를 묻고(`WM_MEASUREITEM`) 검은 바탕·흰 바탕에 그리게 해(`WM_DRAWITEM`) 투명도를 되살린다.
    fn drawn_icon(&self, info: &MENUITEMINFOW) -> Option<IconImage> {
        let mut measure = MEASUREITEMSTRUCT {
            CtlType: ODT_MENU,
            CtlID: 0,
            itemID: info.wID,
            itemWidth: 0,
            itemHeight: 0,
            itemData: info.dwItemData,
        };
        let measured = self.message(
            WM_MEASUREITEM,
            WPARAM(0),
            LPARAM(&mut measure as *mut MEASUREITEMSTRUCT as isize),
        );
        let width = measure.itemWidth as i32;
        let height = measure.itemHeight as i32;
        if !measured
            || !(1..=MAX_ICON_SIDE).contains(&width)
            || !(1..=MAX_ICON_SIDE).contains(&height)
        {
            return None;
        }
        let surface = Surface::new(width, height)?;
        let draw = |background: HBRUSH| -> Option<Vec<u8>> {
            surface.fill(background);
            let mut item = DRAWITEMSTRUCT {
                CtlType: ODT_MENU,
                CtlID: 0,
                itemID: info.wID,
                itemAction: ODA_DRAWENTIRE,
                itemState: Default::default(),
                hwndItem: HWND(self.hmenu.0),
                hDC: surface.dc,
                rcItem: RECT {
                    left: 0,
                    top: 0,
                    right: width,
                    bottom: height,
                },
                itemData: info.dwItemData,
            };
            if !self.message(
                WM_DRAWITEM,
                WPARAM(0),
                LPARAM(&mut item as *mut DRAWITEMSTRUCT as isize),
            ) {
                return None;
            }
            Some(surface.pixels())
        };
        // SAFETY: 시스템 붓은 지우지 않아도 되는 공용 객체다.
        let black = HBRUSH(unsafe { GetStockObject(BLACK_BRUSH) }.0);
        // SAFETY: 위와 같다.
        let white = HBRUSH(unsafe { GetStockObject(WHITE_BRUSH) }.0);
        let on_black = draw(black)?;
        let on_white = draw(white)?;
        let rgba = unblend(&on_black, &on_white);
        visible(IconImage {
            width: width as u32,
            height: height as u32,
            rgba,
        })
    }
}

/// 메뉴에 글자 있는 항목(또는 구분선)이 하나라도 있는가.
fn has_text(hmenu: HMENU) -> bool {
    // SAFETY: hmenu는 셸 메뉴의 하위 메뉴 핸들이다.
    let count = unsafe { GetMenuItemCount(Some(hmenu)) }.max(0) as u32;
    (0..count).any(|index| {
        let mut info = MENUITEMINFOW {
            cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_FTYPE | MIIM_STRING,
            ..Default::default()
        };
        // SAFETY: info는 크기를 적은 지역 구조체이고 글자 칸이 없으므로 글자 길이만 받는다.
        let got = unsafe { GetMenuItemInfoW(hmenu, index, true, &mut info) }.is_ok();
        got && (info.cch > 0 || info.fType.0 & MFT_SEPARATOR.0 != 0)
    })
}

/// 아무 화소도 보이지 않는 그림은 아이콘이 아니다.
fn visible(icon: IconImage) -> Option<IconImage> {
    icon.rgba
        .as_chunks::<4>()
        .0
        .iter()
        .any(|px| px[3] != 0)
        .then_some(icon)
}

/// 항목 글자(탭 뒤 단축키 글자는 뺌).
fn item_text(hmenu: HMENU, index: u32, len: u32) -> String {
    if len == 0 {
        return String::new();
    }
    let mut buffer = vec![0u16; len as usize + 1];
    let mut info = MENUITEMINFOW {
        cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_STRING,
        dwTypeData: PWSTR(buffer.as_mut_ptr()),
        cch: buffer.len() as u32,
        ..Default::default()
    };
    // SAFETY: dwTypeData는 cch 글자를 담는 지역 버퍼를 가리키고 호출 동안 살아 있다.
    if unsafe { GetMenuItemInfoW(hmenu, index, true, &mut info) }.is_err() {
        return String::new();
    }
    let end = (info.cch as usize).min(len as usize);
    let text = String::from_utf16_lossy(&buffer[..end]);
    match text.split_once('\t') {
        Some((label, _)) => label.to_owned(),
        None => text,
    }
}

/// 앞뒤 구분선과 잇단 구분선을 걷는다.
fn tidy_separators(entries: Vec<Entry>) -> Vec<Entry> {
    let mut tidy: Vec<Entry> = Vec::with_capacity(entries.len());
    for entry in entries {
        let separator = matches!(entry, Entry::Separator);
        if separator && matches!(tidy.last(), None | Some(Entry::Separator)) {
            continue;
        }
        tidy.push(entry);
    }
    if matches!(tidy.last(), Some(Entry::Separator)) {
        tidy.pop();
    }
    tidy
}

/// 항목들의 메뉴 객체. 모두 같은 폴더면 그 폴더의 `GetUIObjectOf`(탐색기와 같은 길), 아니면 셸 항목 배열의 것.
fn items_menu(paths: &[PathBuf]) -> Result<IContextMenu, MenuError> {
    if paths.is_empty() {
        return Err(MenuError::NotFound("no items".into()));
    }
    let pidls = paths
        .iter()
        .map(|p| parse(p))
        .collect::<Result<Vec<_>, _>>()?;
    let same_folder = paths
        .windows(2)
        .all(|pair| same_path(pair[0].parent(), pair[1].parent()));
    if same_folder {
        // SAFETY: 첫 PIDL은 살아 있는 절대 PIDL이다. 상위 폴더 객체를 받는다(마지막 조각 주소는 받지 않음).
        let folder: IShellFolder =
            unsafe { SHBindToParent(pidls[0].0, None) }.map_err(|e| failed(&e))?;
        let children: Vec<*const ITEMIDLIST> = pidls
            .iter()
            // SAFETY: 살아 있는 절대 PIDL의 마지막 조각(같은 메모리 안을 가리킴)이고 pidls가 살아 있는 동안 쓴다.
            .map(|p| unsafe { ILFindLastID(p.0) } as *const ITEMIDLIST)
            .collect();
        // SAFETY: children은 folder 바로 아래 항목들의 상대 PIDL이고 호출 동안 살아 있다.
        unsafe { folder.GetUIObjectOf::<IContextMenu>(HWND::default(), &children, None) }
            .map_err(|e| failed(&e))
    } else {
        let raw: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| p.0 as *const _).collect();
        // SAFETY: raw의 PIDL들은 pidls가 살아 있는 동안 올바르다.
        let array = unsafe { SHCreateShellItemArrayFromIDLists(&raw) }.map_err(|e| failed(&e))?;
        // SAFETY: array는 살아 있는 셸 항목 배열이다.
        unsafe { array.BindToHandler::<_, IContextMenu>(None, &BHID_SFUIObject) }
            .map_err(|e| failed(&e))
    }
}

/// 폴더 배경의 메뉴 객체(`IShellFolder::CreateViewObject`).
fn background_menu(folder: &Path) -> Result<IContextMenu, MenuError> {
    let pidl = parse(folder)?;
    // SAFETY: pidl은 살아 있는 절대 PIDL이다.
    let item: IShellItem = unsafe { SHCreateItemFromIDList(pidl.0) }.map_err(|e| failed(&e))?;
    // SAFETY: item은 살아 있는 셸 항목이다.
    let shell_folder: IShellFolder =
        unsafe { item.BindToHandler(None, &BHID_SFObject) }.map_err(|e| failed(&e))?;
    // SAFETY: shell_folder는 살아 있는 폴더 객체이고 소유 창은 없다.
    unsafe { shell_folder.CreateViewObject::<IContextMenu>(HWND::default()) }
        .map_err(|e| failed(&e))
}

/// 대소문자를 가리지 않고 같은 경로인가.
fn same_path(a: Option<&Path>, b: Option<&Path>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => {
            a.as_os_str().to_string_lossy().to_lowercase()
                == b.as_os_str().to_string_lossy().to_lowercase()
        }
        (None, None) => true,
        _ => false,
    }
}

/// 그림 핸들을 RGBA로 읽는다. 특수 값·빈 핸들·너무 큰 그림이면 `None`.
fn bitmap_icon(bitmap: HBITMAP) -> Option<IconImage> {
    let raw = bitmap.0 as isize;
    if (-1..=HBMMENU_LAST_SPECIAL).contains(&raw) {
        return None;
    }
    let mut info = BITMAP::default();
    // SAFETY: bitmap은 셸 메뉴가 가진 그림 핸들이고 info는 그 크기의 지역 칸이다.
    let got = unsafe {
        GetObjectW(
            HGDIOBJ(bitmap.0),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut info as *mut BITMAP as *mut _),
        )
    };
    let (width, height) = (info.bmWidth, info.bmHeight.abs());
    if got == 0 || !(1..=MAX_ICON_SIDE).contains(&width) || !(1..=MAX_ICON_SIDE).contains(&height) {
        return None;
    }
    let mut header = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bgra = vec![0u8; (width * height * 4) as usize];
    // SAFETY: 화면과 호환되는 메모리 DC를 만든다. 아래에서 지운다.
    let dc = unsafe { CreateCompatibleDC(None) };
    if dc.is_invalid() {
        return None;
    }
    // SAFETY: bgra는 height행 32비트 화소를 담는 버퍼이고 header는 그 모양을 적은 머리다.
    let lines = unsafe {
        GetDIBits(
            dc,
            bitmap,
            0,
            height as u32,
            Some(bgra.as_mut_ptr().cast()),
            &mut header,
            DIB_RGB_COLORS,
        )
    };
    // SAFETY: 위에서 만든 DC를 한 번만 지운다.
    let _ = unsafe { DeleteDC(dc) };
    if lines != height {
        return None;
    }
    let has_alpha = info.bmBitsPixel == 32 && bgra.as_chunks::<4>().0.iter().any(|px| px[3] != 0);
    let mut rgba = Vec::with_capacity(bgra.len());
    for px in bgra.as_chunks::<4>().0 {
        let alpha = if has_alpha { px[3] } else { 255 };
        // 32비트 메뉴 그림은 미리 곱한 알파다: 곧은 알파로 되돌린다.
        let channel = |c: u8| -> u8 {
            if !has_alpha {
                c
            } else if alpha == 0 {
                0
            } else {
                ((u32::from(c) * 255 + u32::from(alpha) / 2) / u32::from(alpha)).min(255) as u8
            }
        };
        rgba.extend_from_slice(&[channel(px[2]), channel(px[1]), channel(px[0]), alpha]);
    }
    visible(IconImage {
        width: width as u32,
        height: height as u32,
        rgba,
    })
}

/// 메모리 DC와 거기 고른 32비트 DIB(위에서 아래 행). 버리면 풀어 준다.
struct Surface {
    dc: HDC,
    bitmap: HGDIOBJ,
    previous: HGDIOBJ,
    bits: *mut u8,
    width: i32,
    height: i32,
}

impl Surface {
    fn new(width: i32, height: i32) -> Option<Surface> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: 화면과 호환되는 메모리 DC를 새로 만든다. 실패하면 null이다.
        let dc = unsafe { CreateCompatibleDC(None) };
        if dc.is_invalid() {
            return None;
        }
        let mut bits = std::ptr::null_mut();
        // SAFETY: info는 32비트 위에서 아래 DIB를 그리는 올바른 머리이고 bits는 그림 메모리 주소를 받는다.
        let bitmap = match unsafe {
            CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
        } {
            Ok(bitmap) if !bits.is_null() => HGDIOBJ(bitmap.0),
            _ => {
                // SAFETY: 위에서 만든 DC를 한 번만 지운다.
                let _ = unsafe { DeleteDC(dc) };
                return None;
            }
        };
        // SAFETY: dc와 bitmap은 위에서 만든 살아 있는 객체다. 앞서 골라져 있던 객체는 Drop에서 되돌린다.
        let previous = unsafe { SelectObject(dc, bitmap) };
        Some(Surface {
            dc,
            bitmap,
            previous,
            bits: bits.cast(),
            width,
            height,
        })
    }

    fn fill(&self, brush: HBRUSH) {
        let rect = RECT {
            left: 0,
            top: 0,
            right: self.width,
            bottom: self.height,
        };
        // SAFETY: dc에는 이 크기의 DIB가 골라져 있고 붓은 시스템 붓이다.
        unsafe { FillRect(self.dc, &rect, brush) };
    }

    /// 그린 BGRA 화소.
    fn pixels(&self) -> Vec<u8> {
        // SAFETY: GDI가 미뤄 둔 그리기를 DIB 메모리에 반영한다.
        let _ = unsafe { GdiFlush() };
        let len = (self.width * self.height * 4) as usize;
        // SAFETY: bits는 CreateDIBSection이 준 width*height 32비트 화소 메모리이고 Surface가 살아 있는 동안 유효하다.
        unsafe { std::slice::from_raw_parts(self.bits, len) }.to_vec()
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: 만들 때 골라 둔 앞 객체를 되돌리고, 만든 DIB와 DC를 한 번씩 지운다.
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap);
            let _ = DeleteDC(self.dc);
        }
    }
}

// ---- 죽게 한 확장 알기 ----

/// 등록된 오른쪽 메뉴 확장 하나.
#[derive(Debug, Clone)]
struct Handler {
    clsid: String,
    name: String,
}

/// 공유 상태 파일 자리.
static CRASH_STATE: OnceLock<PathBuf> = OnceLock::new();
/// 소문자 DLL 경로 → 확장. 메뉴를 만들 때마다 대상의 확장을 더한다.
static HANDLERS: Mutex<Option<HashMap<String, Handler>>> = Mutex::new(None);

pub(crate) fn record_crashes(state: &Path) {
    if CRASH_STATE.set(state.to_path_buf()).is_err() {
        return;
    }
    // SAFETY: 이 프로세스의 처리되지 않은 예외 거르개를 아래 함수로 바꾼다. 앞 거르개는 쓰지 않는다.
    unsafe { SetUnhandledExceptionFilter(Some(on_crash)) };
}

/// 처리되지 않은 예외: 예외 주소가 든 DLL과 맞는 확장을 공유 상태에 적고, 기본 처리(프로세스 끝내기)로 넘긴다.
unsafe extern "system" fn on_crash(pointers: *const EXCEPTION_POINTERS) -> i32 {
    const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
    let Some(state) = CRASH_STATE.get() else {
        return EXCEPTION_CONTINUE_SEARCH;
    };
    // SAFETY: 시스템이 넘긴 예외 정보이고 이 호출 동안 살아 있다. 빈 포인터는 건너뛴다.
    let address = unsafe {
        pointers
            .as_ref()
            .and_then(|p| p.ExceptionRecord.as_ref())
            .map(|r| r.ExceptionAddress)
    };
    let Some(address) = address else {
        return EXCEPTION_CONTINUE_SEARCH;
    };
    let Some(dll) = module_of(address) else {
        return EXCEPTION_CONTINUE_SEARCH;
    };
    // 죽는 중이라 잠금을 기다리지 않는다.
    let handler = HANDLERS
        .try_lock()
        .ok()
        .and_then(|map| map.as_ref()?.get(&key(&dll)).cloned());
    let (clsid, name) = match handler {
        Some(h) => (h.clsid, h.name),
        None => ("unknown".to_owned(), String::new()),
    };
    let _ = std::fs::write(state, format!("{clsid}\n{}\n{name}\n", dll.display()));
    EXCEPTION_CONTINUE_SEARCH
}

/// 주소가 든 모듈의 파일 경로.
fn module_of(address: *mut core::ffi::c_void) -> Option<PathBuf> {
    let mut module = HMODULE::default();
    // SAFETY: 주소로 모듈을 찾고 참조 수는 바꾸지 않는다. module은 지역 칸이다.
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(address as *const u16),
            &mut module,
        )
    }
    .ok()?;
    let mut buffer = [0u16; 1024];
    // SAFETY: module은 위에서 찾은 실린 모듈이고 buffer는 지역 칸이다.
    let len = unsafe { GetModuleFileNameW(Some(module), &mut buffer) } as usize;
    (len > 0 && len < buffer.len()).then(|| PathBuf::from(OsString::from_wide(&buffer[..len])))
}

fn key(dll: &Path) -> String {
    dll.to_string_lossy().to_lowercase()
}

/// 대상의 오른쪽 메뉴 확장을 레지스트리에서 모아 둔다(기록을 켰을 때만).
fn note_handlers(target: &MenuTarget) {
    if CRASH_STATE.get().is_none() {
        return;
    }
    let mut found = HashMap::new();
    for class in classes(target) {
        for (name, clsid) in context_menu_handlers(&class) {
            if let Some(dll) = inproc_server(&clsid) {
                found.insert(key(&dll), Handler { clsid, name });
            }
        }
    }
    if let Ok(mut map) = HANDLERS.lock() {
        map.get_or_insert_with(HashMap::new).extend(found);
    }
}

/// 대상의 오른쪽 메뉴 확장이 등록되는 클래스 키들.
fn classes(target: &MenuTarget) -> Vec<String> {
    let mut classes = Vec::new();
    match target {
        MenuTarget::Background(_) => {
            classes.push(r"Directory\Background".to_owned());
            classes.push("DesktopBackground".to_owned());
        }
        MenuTarget::Items(paths) => {
            classes.push("*".to_owned());
            classes.push("AllFilesystemObjects".to_owned());
            classes.push("Directory".to_owned());
            classes.push("Folder".to_owned());
            classes.push("Drive".to_owned());
            for path in paths {
                let Some(ext) = path.extension() else {
                    continue;
                };
                let ext = format!(".{}", ext.to_string_lossy());
                if let Some(prog_id) = reg_string(&ext, None) {
                    classes.push(prog_id);
                }
                classes.push(format!(r"SystemFileAssociations\{ext}"));
                classes.push(ext);
            }
        }
    }
    classes.sort();
    classes.dedup();
    classes
}

/// `<class>\shellex\ContextMenuHandlers` 아래 (이름, CLSID).
fn context_menu_handlers(class: &str) -> Vec<(String, String)> {
    let path = format!(r"{class}\shellex\ContextMenuHandlers");
    let wide = HSTRING::from(path.as_str());
    let mut key = HKEY::default();
    // SAFETY: 이름은 NUL로 끝나는 글자이고 key는 열린 키를 받는 지역 칸이다. 연 키는 아래에서 닫는다.
    if unsafe { RegOpenKeyExW(HKEY_CLASSES_ROOT, &wide, None, KEY_READ, &mut key) }.is_err() {
        return Vec::new();
    }
    let mut handlers = Vec::new();
    for index in 0.. {
        let mut name = [0u16; 256];
        let mut len = name.len() as u32;
        // SAFETY: key는 열린 키이고 name·len은 지역 칸이다.
        let status = unsafe {
            RegEnumKeyExW(
                key,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut len,
                None,
                None,
                None,
                None,
            )
        };
        if status.is_err() {
            break;
        }
        let name = String::from_utf16_lossy(&name[..len as usize]);
        let clsid = reg_string(&format!(r"{path}\{name}"), None)
            .filter(|v| v.starts_with('{'))
            .unwrap_or_else(|| name.clone());
        if clsid.starts_with('{') {
            handlers.push((name, clsid));
        }
    }
    // SAFETY: 위에서 연 키를 한 번만 닫는다.
    let _ = unsafe { RegCloseKey(key) };
    handlers
}

/// CLSID의 DLL 경로(`CLSID\{...}\InprocServer32`의 기본 값, 환경 변수를 펼침).
fn inproc_server(clsid: &str) -> Option<PathBuf> {
    reg_string(&format!(r"CLSID\{clsid}\InprocServer32"), None).map(PathBuf::from)
}

/// `HKEY_CLASSES_ROOT\<subkey>`의 글자 값(`value`가 없으면 기본 값).
fn reg_string(subkey: &str, value: Option<&str>) -> Option<String> {
    let subkey = HSTRING::from(subkey);
    let value = value.map(HSTRING::from);
    let value_name = value
        .as_ref()
        .map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr()));
    let mut buffer = vec![0u16; 1024];
    let mut bytes = (buffer.len() * 2) as u32;
    // SAFETY: 이름들은 NUL로 끝나는 글자이고 buffer·bytes는 지역 칸이다. 펼친 글자를 받는다.
    let status = unsafe {
        RegGetValueW(
            HKEY_CLASSES_ROOT,
            &subkey,
            value_name,
            // 펼칠 글자(REG_EXPAND_SZ)도 펼쳐 REG_SZ로 받는다.
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if status.is_err() {
        return None;
    }
    let len = (bytes as usize / 2).min(buffer.len());
    let text = String::from_utf16_lossy(&buffer[..len]);
    let text = text.trim_end_matches('\0').trim().to_owned();
    (!text.is_empty()).then_some(text)
}
