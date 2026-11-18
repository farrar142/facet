//! 셸 대화상자(#321): Windows 속성 대화상자와 「연결 프로그램」 대화상자를 띄운다. 셸 확장 도우미(ADR-0006)만 부른다.
//! UI 프로세스는 셸 확장을 싣지 않으므로 이 함수들을 부르지 않는다(포그라운드 권한 넘기기 [`allow_foreground`]만 쓴다).
//!
//! 두 대화상자 함수는 부른 스레드에서 COM을 STA로 초기화하고 **대화상자가 닫힐 때까지** 돌아오지 않는다(부르는 쪽은
//! 대화상자마다 스레드를 하나 띄운다). 속성 대화상자는 셸이 따로 띄우는 비모달 창이라, 이 프로세스의 보이는 최상위
//! 창이 모두 사라질 때까지 메시지를 돌리며 기다린다(도우미는 창을 따로 만들지 않으므로 그 창이 대화상자다).
//!
//! Windows가 아니면 대화상자 함수는 [`io::ErrorKind::Unsupported`]이고 [`allow_foreground`]는 아무것도 하지 않는다.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 대화상자 창이 나타나기를 기다리는 가장 긴 시간.
///
/// 근거: 속성 대화상자는 셸이 따로 띄워 보통 수백 ms 안에 나타나지만, 셸 확장(속성 페이지 처리기)을 처음 싣거나
/// 네트워크 위치면 몇 초 걸릴 수 있다. 끝내 나타나지 않으면(띄우지 못함) 기다림을 그만두는 값이다.
pub const APPEAR_TIMEOUT: Duration = Duration::from_secs(10);

/// 대화상자가 닫혔는지 보는 간격.
const POLL: Duration = Duration::from_millis(50);

/// `paths`(한 개 이상)의 Windows 속성 대화상자를 띄우고 닫힐 때까지 기다린다. 여러 항목이면 여러 항목의 속성이다.
/// 경로를 셸 항목으로 바꾸지 못하거나 셸이 실패하면 `Err`.
pub fn show_properties(paths: &[PathBuf]) -> io::Result<()> {
    if paths.is_empty() {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    #[cfg(windows)]
    {
        imp::show_properties(paths)
    }
    #[cfg(not(windows))]
    {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

/// 파일 `path`의 「연결 프로그램」 대화상자를 띄우고 닫힐 때까지 기다린다. 고른 프로그램으로 그 파일을 연다. 취소해도
/// `Ok`다.
pub fn show_open_with(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        imp::show_open_with(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

/// 프로세스 `pid`가 창을 앞으로 가져올 수 있게 포그라운드 권한을 넘긴다(`AllowSetForegroundWindow`). 이 프로세스가
/// 포그라운드가 아니면 아무 일도 없다. 실패해도 조용하다.
pub fn allow_foreground(pid: u32) {
    #[cfg(windows)]
    imp::allow_foreground(pid);
    #[cfg(not(windows))]
    let _ = pid;
}

/// 이 스레드의 메시지를 돌리며 이 프로세스의 보이는 창이 `appear` 안에 나타나면 모두 사라질 때까지 기다린다(셸 메뉴
/// 항목 실행 뒤, [`crate::shell_menu::pump_while_windows`]).
#[cfg(windows)]
pub(crate) fn wait_for_windows(appear: Duration) {
    imp::wait_for_windows(appear);
}

/// 이 스레드에 쌓인 창 메시지를 모두 돌린다.
#[cfg(windows)]
pub(crate) fn pump_messages() {
    imp::pump_messages();
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    use windows::Win32::Foundation::{ERROR_CANCELLED, HWND, LPARAM};
    use windows::Win32::System::Com::{CoTaskMemFree, IDataObject};
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        BHID_DataObject, OAIF_ALLOW_REGISTRATION, OAIF_EXEC, OAIF_REGISTER_EXT, OPENASINFO,
        SHCreateShellItemArrayFromIDLists, SHMultiFileProperties, SHOpenWithDialog,
        SHParseDisplayName,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, DispatchMessageW, EnumWindows, GetWindowThreadProcessId,
        IsWindowVisible, MSG, PM_REMOVE, PeekMessageW, SetForegroundWindow, TranslateMessage,
    };
    use windows::core::{BOOL, HSTRING, PCWSTR};

    use super::{APPEAR_TIMEOUT, POLL};
    use crate::windows_impl::ComApartment;

    fn os_error(error: &windows::core::Error) -> io::Error {
        io::Error::other(error.to_string())
    }

    /// 셸이 준 절대 PIDL. 버리면 푼다.
    struct Pidl(*mut ITEMIDLIST);

    impl Drop for Pidl {
        fn drop(&mut self) {
            // SAFETY: SHParseDisplayName이 CoTaskMemAlloc으로 준 PIDL이고 한 번만 푼다.
            unsafe { CoTaskMemFree(Some(self.0 as *const _)) };
        }
    }

    fn parse(path: &Path) -> io::Result<Pidl> {
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        // SAFETY: 이름은 NUL로 끝나는 글자(HSTRING)이고 pidl은 셸이 채울 칸이다. 성공하면 Pidl이 푼다.
        unsafe { SHParseDisplayName(&HSTRING::from(path.as_os_str()), None, &mut pidl, 0, None) }
            .map_err(|e| os_error(&e))?;
        if pidl.is_null() {
            return Err(io::Error::from(io::ErrorKind::NotFound));
        }
        Ok(Pidl(pidl))
    }

    pub fn show_properties(paths: &[PathBuf]) -> io::Result<()> {
        let _com = ComApartment::enter()?;
        let pidls = paths
            .iter()
            .map(|p| parse(p))
            .collect::<io::Result<Vec<_>>>()?;
        let raw: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| p.0 as *const _).collect();
        // SAFETY: raw의 PIDL들은 pidls가 살아 있는 동안 올바르다.
        let array = unsafe { SHCreateShellItemArrayFromIDLists(&raw) }.map_err(|e| os_error(&e))?;
        // SAFETY: array는 살아 있는 셸 항목 배열이다.
        let data: IDataObject =
            unsafe { array.BindToHandler(None, &BHID_DataObject) }.map_err(|e| os_error(&e))?;
        // SAFETY: data는 살아 있는 데이터 개체다. 셸이 대화상자를 따로 띄우고 곧바로 돌아온다.
        unsafe { SHMultiFileProperties(&data, 0) }.map_err(|e| os_error(&e))?;
        wait_for_windows(APPEAR_TIMEOUT);
        drop(data);
        Ok(())
    }

    pub fn show_open_with(path: &Path) -> io::Result<()> {
        let _com = ComApartment::enter()?;
        let file = HSTRING::from(path.as_os_str());
        let info = OPENASINFO {
            pcszFile: PCWSTR(file.as_ptr()),
            pcszClass: PCWSTR::null(),
            oaifInFlags: OAIF_ALLOW_REGISTRATION | OAIF_REGISTER_EXT | OAIF_EXEC,
        };
        // 모달 대화상자는 이 스레드를 막으므로 앞으로 가져오기는 다른 스레드가 한다.
        let _ = std::thread::Builder::new()
            .name("facet-shell-dialog-front".into())
            .spawn(|| {
                let deadline = Instant::now() + APPEAR_TIMEOUT;
                while Instant::now() < deadline {
                    if let Some(window) = own_visible_window() {
                        // SAFETY: window는 방금 찾은 이 프로세스의 창 핸들이다(사라졌으면 실패할 뿐).
                        let _ = unsafe { SetForegroundWindow(window) };
                        return;
                    }
                    std::thread::sleep(super::POLL);
                }
            });
        // SAFETY: info와 그 글자(file)는 호출이 끝날 때까지 살아 있다. 대화상자가 닫히면 돌아온다.
        match unsafe { SHOpenWithDialog(None, &info) } {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ERROR_CANCELLED.to_hresult() => Ok(()),
            Err(e) => Err(os_error(&e)),
        }
    }

    pub fn allow_foreground(pid: u32) {
        // SAFETY: 값으로 받는 인자만 넘긴다.
        let _ = unsafe { AllowSetForegroundWindow(pid) };
    }

    /// 이 스레드의 메시지를 돌리며 이 프로세스의 보이는 창이 나타났다 모두 사라질 때까지 기다린다. 처음 나타난 창은
    /// 앞으로 가져온다. `appear` 안에 나타나지 않으면 그만둔다.
    pub(crate) fn wait_for_windows(appear: std::time::Duration) {
        let deadline = Instant::now() + appear;
        let mut seen = false;
        loop {
            pump_messages();
            match own_visible_window() {
                Some(window) if !seen => {
                    seen = true;
                    // SAFETY: window는 방금 찾은 이 프로세스의 창 핸들이다.
                    let _ = unsafe { SetForegroundWindow(window) };
                }
                Some(_) => {}
                None if seen || Instant::now() >= deadline => return,
                None => {}
            }
            std::thread::sleep(POLL);
        }
    }

    pub(crate) fn pump_messages() {
        let mut msg = MSG::default();
        // SAFETY: msg는 이 함수의 지역 칸이고, 이 스레드의 메시지만 꺼내 돌린다.
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            // SAFETY: 방금 꺼낸 메시지다.
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    /// 이 프로세스의 보이는 최상위 창 하나.
    fn own_visible_window() -> Option<HWND> {
        struct Search {
            pid: u32,
            found: Option<HWND>,
        }
        unsafe extern "system" fn visit(window: HWND, data: LPARAM) -> BOOL {
            // SAFETY: data는 아래 EnumWindows가 넘긴 Search의 주소이고 열거 동안 살아 있다.
            let search = unsafe { &mut *(data.0 as *mut Search) };
            let mut pid = 0u32;
            // SAFETY: window는 열거가 준 창 핸들이고 pid는 지역 칸이다.
            unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
            // SAFETY: window는 열거가 준 창 핸들이다.
            if pid == search.pid && unsafe { IsWindowVisible(window) }.as_bool() {
                search.found = Some(window);
                return BOOL(0);
            }
            BOOL(1)
        }
        let mut search = Search {
            // SAFETY: 인자 없는 호출이다.
            pid: unsafe { GetCurrentProcessId() },
            found: None,
        };
        // SAFETY: visit는 위 서명의 콜백이고, search는 열거가 끝날 때까지 살아 있다. 찾으면 멈추느라 실패로 돌아온다.
        let _ = unsafe { EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize)) };
        search.found
    }
}
