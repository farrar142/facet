//! [`crate::preview_handler`]의 Windows 구현: `IPreviewHandler`를 싣고 그릇 창을 붙일 창의 자식으로 만들어 그 안에
//! 그리게 한다. 셸 확장 도우미만 부른다. `unsafe`는 이 파일 안에 있다.

use std::path::Path;
use std::sync::OnceLock;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{COLOR_WINDOW, HBRUSH, ScreenToClient};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, CLSIDFromString, CoCreateInstance, STGM_READ,
    STGM_SHARE_DENY_NONE,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::Shell::PropertiesSystem::{IInitializeWithFile, IInitializeWithStream};
use windows::Win32::UI::Shell::{
    IInitializeWithItem, IPreviewHandler, IShellItem, SHCreateItemFromParsingName,
    SHCreateStreamOnFileEx,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, HWND_TOP, IsWindow, RegisterClassW,
    SWP_NOACTIVATE, SWP_SHOWWINDOW, SetParent, SetWindowPos, WINDOW_EX_STYLE, WNDCLASSW, WS_CHILD,
    WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_NOPARENTNOTIFY, WS_VISIBLE,
};
use windows::core::{HSTRING, Interface, PCWSTR, w};

use crate::preview_handler::{HostRect, LoadError};

/// 그릇 창의 창 클래스 이름.
const CLASS_NAME: PCWSTR = w!("FacetPreviewHost");

fn failed(error: &windows::core::Error) -> LoadError {
    LoadError::Failed(format!("{:#010x} {}", error.code().0, error.message()))
}

/// 실은 처리기와 그 그릇 창.
pub(crate) struct Loaded {
    handler: IPreviewHandler,
    container: HWND,
    parent: HWND,
}

impl Loaded {
    pub(crate) fn load(path: &Path, host: HostRect) -> Result<Loaded, LoadError> {
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .ok_or(LoadError::NoHandler)?;
        if extension.is_empty() || extension.contains(['.', '\\', '/', '\0', '*']) {
            return Err(LoadError::NoHandler);
        }
        let clsid = crate::preview::imp::handler_clsid(extension).ok_or(LoadError::NoHandler)?;
        use_physical_pixels();
        let parent = parent_window(host)?;

        let clsid_text = HSTRING::from(clsid.as_str());
        // SAFETY: 널로 끝나는 UTF-16 글자를 넘기고 GUID를 값으로 받는다.
        let clsid = unsafe { CLSIDFromString(&clsid_text) }.map_err(|e| failed(&e))?;
        // SAFETY: GUID는 지역 변수이고 바깥 개체 없이 만든다. 이 스레드는 COM STA로 초기화돼 있다(부르는 쪽의 약속).
        let handler: IPreviewHandler = unsafe {
            CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER)
                .or_else(|_| CoCreateInstance(&clsid, None, CLSCTX_LOCAL_SERVER))
        }
        .map_err(|e| failed(&e))?;
        initialize(&handler, path)?;

        let container = create_container(parent, host)?;
        let loaded = Loaded {
            handler,
            container,
            parent,
        };
        let rect = inner_rect(host);
        // SAFETY: 그릇 창은 이 스레드가 방금 만든 살아 있는 창이고 `rect`는 지역 변수다.
        unsafe { loaded.handler.SetWindow(loaded.container, &rect) }.map_err(|e| failed(&e))?;
        // SAFETY: 초기화와 창 지정을 마친 처리기다. 실패하면 `loaded`를 버리며 내린다.
        unsafe { loaded.handler.DoPreview() }.map_err(|e| failed(&e))?;
        Ok(loaded)
    }

    pub(crate) fn move_to(&mut self, host: HostRect) {
        let Ok(parent) = parent_window(host) else {
            return;
        };
        if parent != self.parent {
            // SAFETY: 두 창 모두 살아 있는 창 핸들이다(그릇 창은 이 스레드가 만든 것). 실패하면 그대로 둔다.
            if unsafe { SetParent(self.container, Some(parent)) }.is_ok() {
                self.parent = parent;
            }
        }
        place(self.container, self.parent, host);
        let rect = inner_rect(host);
        // SAFETY: 실어 둔 처리기이고 `rect`는 지역 변수다.
        let _ = unsafe { self.handler.SetRect(&rect) };
    }
}

impl Drop for Loaded {
    fn drop(&mut self) {
        // SAFETY: 이 스레드에서 실은 처리기다. 내리면 처리기가 잡고 있던 파일·창을 놓는다.
        let _ = unsafe { self.handler.Unload() };
        // SAFETY: 이 스레드가 만든 그릇 창이다. 붙인 창이 먼저 사라졌으면 이미 없어진 창이라 실패할 뿐이다.
        unsafe {
            if IsWindow(Some(self.container)).as_bool() {
                let _ = DestroyWindow(self.container);
            }
        }
    }
}

/// 이 스레드를 모니터마다 DPI를 아는 문맥으로 둔다(화면 자리를 물리 픽셀 그대로 붙일 창 안쪽 좌표로 바꾸게).
fn use_physical_pixels() {
    // SAFETY: 정해진 문맥 값을 넘긴다. 실패하면(옛 Windows) 앞 문맥이 그대로일 뿐이다.
    let _ = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

/// 붙일 창. 창이 아니면 실패다.
fn parent_window(host: HostRect) -> Result<HWND, LoadError> {
    let parent = HWND(host.parent as isize as *mut _);
    // SAFETY: 핸들 값을 견주기만 한다(다른 프로세스의 창이어도 된다).
    if host.parent == 0 || !unsafe { IsWindow(Some(parent)) }.as_bool() {
        return Err(LoadError::Failed("host window is gone".into()));
    }
    Ok(parent)
}

/// 처리기를 스트림 → 경로 → 셸 항목 차례로 초기화한다.
fn initialize(handler: &IPreviewHandler, path: &Path) -> Result<(), LoadError> {
    let wide = HSTRING::from(path.as_os_str());
    if let Ok(init) = handler.cast::<IInitializeWithStream>() {
        // SAFETY: 널로 끝나는 경로를 넘기고 읽기 전용·공유 거부 없음으로 연다. 받은 스트림은 처리기가 참조를 잡는다.
        let stream = unsafe {
            SHCreateStreamOnFileEx(&wide, (STGM_READ | STGM_SHARE_DENY_NONE).0, 0, false, None)
        }
        .map_err(|e| failed(&e))?;
        // SAFETY: 방금 연 스트림을 넘긴다.
        return unsafe { init.Initialize(&stream, STGM_READ.0) }.map_err(|e| failed(&e));
    }
    if let Ok(init) = handler.cast::<IInitializeWithFile>() {
        // SAFETY: 널로 끝나는 경로를 넘긴다.
        return unsafe { init.Initialize(&wide, STGM_READ.0) }.map_err(|e| failed(&e));
    }
    if let Ok(init) = handler.cast::<IInitializeWithItem>() {
        // SAFETY: 널로 끝나는 경로를 넘기고 묶음 문맥은 없다.
        let item: IShellItem =
            unsafe { SHCreateItemFromParsingName(&wide, None) }.map_err(|e| failed(&e))?;
        // SAFETY: 방금 만든 셸 항목을 넘긴다.
        return unsafe { init.Initialize(&item, STGM_READ.0) }.map_err(|e| failed(&e));
    }
    Err(LoadError::Failed(
        "handler takes no known initializer".into(),
    ))
}

/// 처리기 창 안쪽 자리(그릇 창 기준).
fn inner_rect(host: HostRect) -> RECT {
    RECT {
        left: 0,
        top: 0,
        right: i32::try_from(host.width).unwrap_or(i32::MAX),
        bottom: i32::try_from(host.height).unwrap_or(i32::MAX),
    }
}

/// 화면 자리를 붙일 창 안쪽 좌표로 바꾼다.
fn client_origin(parent: HWND, host: HostRect) -> POINT {
    let mut point = POINT {
        x: host.x,
        y: host.y,
    };
    // SAFETY: 살아 있는 창 핸들과 지역 변수를 넘긴다. 실패하면 화면 좌표 그대로다(창이 사라진 경우).
    let _ = unsafe { ScreenToClient(parent, &mut point) };
    point
}

fn register_class() -> Option<HINSTANCE> {
    // 모듈 핸들 값(스레드 사이에 나눌 수 있게 값으로 둔다).
    static REGISTERED: OnceLock<Option<isize>> = OnceLock::new();
    let value = (*REGISTERED.get_or_init(|| {
        // SAFETY: 이 실행 파일의 모듈 핸들을 묻는다.
        let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.ok()?.into();
        let class = WNDCLASSW {
            lpfnWndProc: Some(container_proc),
            hInstance: instance,
            lpszClassName: CLASS_NAME,
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as isize as *mut _),
            ..Default::default()
        };
        // SAFETY: 클래스 정보는 이 호출 동안 살아 있고 이름·창 처리 함수는 정적이다.
        let atom = unsafe { RegisterClassW(&class) };
        (atom != 0).then_some(instance.0 as isize)
    }))?;
    Some(HINSTANCE(value as *mut _))
}

unsafe extern "system" fn container_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: 창 관리자가 넘긴 인자를 그대로 기본 처리에 넘긴다.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

/// 붙일 창의 자식으로 그릇 창을 만든다.
fn create_container(parent: HWND, host: HostRect) -> Result<HWND, LoadError> {
    let instance =
        register_class().ok_or_else(|| LoadError::Failed("register class failed".into()))?;
    let origin = client_origin(parent, host);
    // SAFETY: 등록한 클래스 이름과 살아 있는 붙일 창을 넘긴다. 다른 프로세스의 창의 자식으로 만드는 것은 창 관리자가
    // 허락한다(셸의 미리보기 대리 프로세스와 같은 방식). 만든 창은 `Loaded`가 버릴 때 없앤다.
    let container = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_NOPARENTNOTIFY.0),
            CLASS_NAME,
            PCWSTR::null(),
            WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            origin.x,
            origin.y,
            i32::try_from(host.width).unwrap_or(i32::MAX),
            i32::try_from(host.height).unwrap_or(i32::MAX),
            Some(parent),
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|e| failed(&e))?;
    place(container, parent, host);
    Ok(container)
}

/// 그릇 창을 붙일 창 안의 `host` 자리로 옮기고 맨 위에 둔다(활성화하지 않는다).
fn place(container: HWND, parent: HWND, host: HostRect) {
    let origin = client_origin(parent, host);
    // SAFETY: 이 스레드가 만든 그릇 창이다. 붙일 창이 사라졌으면 실패할 뿐이다.
    unsafe {
        let _ = SetWindowPos(
            container,
            Some(HWND_TOP),
            origin.x,
            origin.y,
            i32::try_from(host.width).unwrap_or(i32::MAX),
            i32::try_from(host.height).unwrap_or(i32::MAX),
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}
