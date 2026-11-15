//! 화면 위 창 찾기. `unsafe`는 이 파일 안에 가둔다.

use std::num::NonZeroIsize;

use windows::Win32::Foundation::POINT;
use windows::Win32::UI::WindowsAndMessaging::{GA_ROOT, GetAncestor, WindowFromPoint};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 어느_모니터에도_없는_점에는_창이_없다() {
        assert_eq!(top_level_window_at(-1_000_000_000, -1_000_000_000), None);
        assert_eq!(top_level_window_at(1_000_000_000, 1_000_000_000), None);
    }
}
