//! 속성·「연결 프로그램」 대화상자의 Windows API 호출([`crate::shell_dialog`]). `unsafe`는 이 파일 안에 가둔다.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_DataObject, ILFree, OAIF_ALLOW_REGISTRATION, OAIF_EXEC, OAIF_REGISTER_EXT, OPENASINFO,
    SHCreateShellItemArrayFromIDLists, SHMultiFileProperties, SHOP_FILEPATH, SHObjectProperties,
    SHOpenWithDialog, SHParseDisplayName,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EnumWindows, GetWindowThreadProcessId, IsWindowVisible, MSG, PM_REMOVE,
    PeekMessageW, TranslateMessage,
};
use windows::core::{BOOL, PCWSTR};

use crate::shell_dialog::{DialogError, Started};
use crate::windows_impl::{ComApartment, to_wide};

/// 셸이 대화상자 창을 띄우기까지 기다리는 가장 긴 시간. 그 안에 창이 보이지 않으면 기다리기를 그만둔다.
///
/// 근거: 속성 페이지 처리기(셸 확장)를 싣는 데 보통 수백 ms가 걸리고 느린 확장도 몇 초면 끝난다. 끝내 창이 뜨지 않은
/// 경우에 도우미가 「대화상자가 떠 있음」으로 오래 남지 않게 한다.
const APPEAR_TIMEOUT: Duration = Duration::from_secs(15);
/// 창이 닫혔는지 보며 메시지를 처리하는 간격.
const PUMP_INTERVAL: Duration = Duration::from_millis(50);

fn failed(error: windows::core::Error) -> DialogError {
    DialogError::Failed {
        code: error.code().0,
        message: error.message(),
    }
}

fn enter() -> Result<ComApartment, DialogError> {
    ComApartment::enter().map_err(|e| DialogError::Failed {
        code: 0,
        message: e.to_string(),
    })
}

pub(crate) fn properties(paths: &[PathBuf], started: Started<'_>) {
    let _com = match enter() {
        Ok(com) => com,
        Err(e) => return started(Err(e)),
    };
    // COM 객체는 _com보다 먼저 버려지도록 안쪽 함수에서 다룬다.
    let shown = show_properties(paths);
    let ok = shown.is_ok();
    started(shown);
    if ok {
        // 속성 시트는 셸이 띄운 스레드에서 돌고 이 스레드의 개체(데이터 개체)를 부를 수 있으므로, 창이 닫힐 때까지 이
        // 스레드에서 메시지를 처리한다.
        pump_until_closed();
    }
}

fn show_properties(paths: &[PathBuf]) -> Result<(), DialogError> {
    if let [path] = paths {
        let wide = to_wide(path.as_os_str());
        // SAFETY: wide는 NUL로 끝나는 UTF-16 경로이고 호출이 끝날 때까지 살아 있다. 창·페이지 이름은 주지 않는다.
        let shown = unsafe {
            SHObjectProperties(None, SHOP_FILEPATH, PCWSTR(wide.as_ptr()), PCWSTR::null())
        };
        return if shown.as_bool() {
            Ok(())
        } else {
            Err(failed(windows::core::Error::from_thread()))
        };
    }
    let pidls = Pidls::parse(paths)?;
    // SAFETY: pidls의 포인터는 모두 SHParseDisplayName이 만든 살아 있는 절대 항목 ID 목록이다.
    let array = unsafe { SHCreateShellItemArrayFromIDLists(&pidls.0) }.map_err(failed)?;
    // SAFETY: array는 살아 있는 COM 개체이고 바인드 문맥은 주지 않는다.
    let data: IDataObject =
        unsafe { array.BindToHandler(None, &BHID_DataObject) }.map_err(failed)?;
    // SAFETY: data는 살아 있는 데이터 개체이고 플래그는 0이다.
    unsafe { SHMultiFileProperties(&data, 0) }.map_err(failed)
}

/// 경로들의 항목 ID 목록. 버리면 놓는다.
struct Pidls(Vec<*const ITEMIDLIST>);

impl Pidls {
    fn parse(paths: &[PathBuf]) -> Result<Pidls, DialogError> {
        let mut pidls = Pidls(Vec::with_capacity(paths.len()));
        for path in paths {
            let wide = to_wide(path.as_os_str());
            let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            // SAFETY: wide는 NUL로 끝나는 UTF-16 경로이고 pidl은 결과를 받을 자리다. 성공하면 pidl을 Pidls가 맡아 한 번만
            // 놓는다.
            unsafe { SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None) }
                .map_err(failed)?;
            pidls.0.push(pidl);
        }
        Ok(pidls)
    }
}

impl Drop for Pidls {
    fn drop(&mut self) {
        for pidl in &self.0 {
            // SAFETY: SHParseDisplayName이 만든 목록이고 여기서 한 번만 놓는다.
            unsafe { ILFree(Some(*pidl)) };
        }
    }
}

pub(crate) fn open_with(path: &Path, started: Started<'_>) {
    let _com = match enter() {
        Ok(com) => com,
        Err(e) => return started(Err(e)),
    };
    let wide = to_wide(path.as_os_str());
    let info = OPENASINFO {
        pcszFile: PCWSTR(wide.as_ptr()),
        pcszClass: PCWSTR::null(),
        oaifInFlags: OAIF_ALLOW_REGISTRATION | OAIF_REGISTER_EXT | OAIF_EXEC,
    };
    // 대화상자는 닫힐 때까지 돌아오지 않으므로(제 메시지 루프를 돈다) 띄우기 직전에 알린다.
    started(Ok(()));
    // SAFETY: info와 그것이 가리키는 wide는 호출이 끝날 때까지 살아 있다. 부모 창은 주지 않는다.
    let _ = unsafe { SHOpenWithDialog(None, &info) };
}

/// 이 프로세스에 보이는 최상위 창이 있는 동안 메시지를 처리한다. 처음 창이 보이기까지는 [`APPEAR_TIMEOUT`]까지 기다린다.
fn pump_until_closed() {
    let start = Instant::now();
    let mut seen = false;
    loop {
        pump();
        let visible = process_has_visible_window();
        seen |= visible;
        if (seen && !visible) || (!seen && start.elapsed() >= APPEAR_TIMEOUT) {
            return;
        }
        std::thread::sleep(PUMP_INTERVAL);
    }
}

/// 이 스레드에 쌓인 메시지를 모두 처리한다.
fn pump() {
    let mut message = MSG::default();
    // SAFETY: message는 결과를 받을 자리이고, 모든 창의 메시지를 지우며 꺼낸다.
    while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
        // SAFETY: 방금 꺼낸 메시지를 그대로 넘긴다.
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

/// 이 프로세스가 가진 보이는 최상위 창이 있는가.
fn process_has_visible_window() -> bool {
    struct Search {
        pid: u32,
        found: bool,
    }
    unsafe extern "system" fn visit(window: HWND, data: LPARAM) -> BOOL {
        // SAFETY: data는 아래 EnumWindows 호출 동안 살아 있는 Search를 가리킨다.
        let search = unsafe { &mut *(data.0 as *mut Search) };
        let mut pid = 0u32;
        // SAFETY: window는 EnumWindows가 넘긴 창이고 pid는 결과를 받을 자리다.
        unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
        // SAFETY: window는 EnumWindows가 넘긴 창이다.
        if pid == search.pid && unsafe { IsWindowVisible(window) }.as_bool() {
            search.found = true;
            return BOOL(0); // 찾았으니 그만 훑는다.
        }
        BOOL(1)
    }
    let mut search = Search {
        // SAFETY: 인자 없는 호출이다.
        pid: unsafe { GetCurrentProcessId() },
        found: false,
    };
    // SAFETY: 콜백은 이 호출 안에서만 불리고 LPARAM은 그동안 살아 있는 search를 가리킨다. 콜백이 FALSE로 멈추면
    // 실패를 돌려주지만 결과는 search에 있다.
    let _ = unsafe { EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize)) };
    search.found
}
