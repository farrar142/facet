//! 화면 위 창 찾기. `unsafe`는 이 파일 안에 가둔다.

use std::num::NonZeroIsize;

use windows::Win32::Foundation::{LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows::core::BOOL;

use crate::MonitorInfo;
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GetAncestor, MONITORINFOF_PRIMARY, WindowFromPoint,
};

/// 화면 점 `(x, y)`(가상 화면 기준 물리 픽셀, 다른 모니터도 같은 좌표계) 아래 보이는 최상위 창의 핸들 값.
///
/// 그 점에서 맨 위에 보이는 창을 찾고(겹친 창은 위의 것, 숨은 창은 건너뜀), 자식 창이면 그 최상위 창으로 올라간다.
/// 어느 모니터에도 없는 점이거나 창이 없으면 `None`이다. 마우스를 붙잡은 창(끄는 중)과 상관없이 점 아래 창을 본다.
/// 값은 창을 가리키는 번호일 뿐이라 비교에만 쓴다.
pub fn top_level_window_at(x: i32, y: i32) -> Option<NonZeroIsize> {
    // SAFETY: 값으로 받는 POINT 하나만 넘기고, 돌려받는 핸들은 역참조하지 않는다.
    let hit = unsafe { WindowFromPoint(POINT { x, y }) };
    if hit.is_invalid() {
        return None;
    }
    // SAFETY: `hit`은 방금 받은 창 핸들이다. 그 사이 창이 사라졌으면 GetAncestor가 NULL을 돌려줄 뿐이다.
    let root = unsafe { GetAncestor(hit, GA_ROOT) };
    let root = if root.is_invalid() { hit } else { root };
    NonZeroIsize::new(root.0 as isize)
}

/// 프로세스 `pid`가 창을 앞으로 가져올 수 있게 한다. 이 프로세스가 앞에 있지 않으면 Windows가 거절하고 그대로 둔다.
pub fn allow_set_foreground_window(pid: u32) {
    // SAFETY: 값으로 받는 프로세스 번호 하나만 넘긴다.
    let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(pid) };
}

/// 붙어 있는 모니터마다 작업 영역과 주 모니터 여부. 정보를 얻지 못한 모니터는 뺀다.
pub fn monitors() -> Vec<MonitorInfo> {
    let mut found: Vec<MonitorInfo> = Vec::new();
    // SAFETY: 콜백은 이 호출 안에서만 불리고, LPARAM은 그동안 살아 있는 `found`를 가리킨다. 다른 곳에서 `found`를
    // 빌리지 않는다.
    let _ = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(add_monitor),
            LPARAM(&mut found as *mut Vec<MonitorInfo> as isize),
        )
    };
    found
}

unsafe extern "system" fn add_monitor(
    monitor: HMONITOR,
    _: HDC,
    _: *mut RECT,
    data: LPARAM,
) -> BOOL {
    // SAFETY: `monitors`가 넘긴 LPARAM이고, EnumDisplayMonitors가 돌아오기 전에만 불린다.
    let found = unsafe { &mut *(data.0 as *mut Vec<MonitorInfo>) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `monitor`는 Windows가 넘긴 핸들이고 `info`는 cbSize를 채운 지역 값이다.
    if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        let work = info.rcWork;
        found.push(MonitorInfo {
            left: work.left,
            top: work.top,
            right: work.right,
            bottom: work.bottom,
            primary: info.dwFlags & MONITORINFOF_PRIMARY != 0,
        });
    }
    true.into()
}

/// 지금 있는 드라이브 문자들(대문자, 문자 순서). `GetLogicalDrives`의 비트 마스크에서 읽는다.
pub fn drive_letters() -> Vec<char> {
    // SAFETY: 인자가 없고 비트 마스크 값만 돌려준다.
    let mask = unsafe { windows::Win32::Storage::FileSystem::GetLogicalDrives() };
    crate::letters_from_mask(mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 어느_모니터에도_없는_점에는_창이_없다() {
        assert_eq!(top_level_window_at(-1_000_000_000, -1_000_000_000), None);
        assert_eq!(top_level_window_at(1_000_000_000, 1_000_000_000), None);
    }

    #[test]
    fn 모니터의_작업_영역은_비지_않고_주_모니터는_많아야_하나다() {
        // CI 러너처럼 화면이 없는 세션에서도 가상 모니터 하나가 있지만, 없더라도 실패하지 않는다.
        let monitors = monitors();
        for monitor in &monitors {
            assert!(monitor.left < monitor.right, "{monitor:?}");
            assert!(monitor.top < monitor.bottom, "{monitor:?}");
        }
        assert!(
            monitors.iter().filter(|m| m.primary).count() <= 1,
            "{monitors:?}"
        );
    }
}
