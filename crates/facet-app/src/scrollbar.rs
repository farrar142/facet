//! facet이 그리는 스크롤 막대(`ui/scrollbar.slint`의 `ScrollBar`)의 식: 썸 길이·자리, 썸 끌기, 트랙 누르기.
//! Slint는 이 함수들을 전역 `ScrollMath`의 콜백으로 부른다([`install`]). 그래서 화면이 그리는 썸과 단위 테스트가 보는
//! 식이 같다.
//!
//! 길이는 모두 논리 픽셀이다. `track`은 트랙 길이, `visible`은 보이는 길이(한 쪽), `total`은 내용 전체 길이,
//! `offset`은 넘긴 거리(0 이상, 맨 위·맨 왼쪽이 0)다.

/// 썸의 가장 작은 길이. 10만 행에서도 마우스로 잡을 수 있게 한다.
pub const MIN_THUMB: f32 = 20.0;

/// 넘길 수 있는 가장 먼 거리.
pub fn max_offset(visible: f32, total: f32) -> f32 {
    (total - visible).max(0.0)
}

/// 썸 길이: 트랙 길이에 `보이는 길이 / 전체 길이`를 곱하고 [`MIN_THUMB`]보다 작게 하지 않는다(트랙보다 길지도 않다).
/// 내용이 다 보이면 트랙 전체다.
pub fn thumb_length(track: f32, visible: f32, total: f32) -> f32 {
    let track = track.max(0.0);
    if total <= visible || total <= 0.0 {
        return track;
    }
    (track * visible / total).max(MIN_THUMB).min(track)
}

/// 썸의 시작 자리(트랙 기준): 썸이 움직일 수 있는 길이에 `넘긴 거리 / 넘길 수 있는 거리`를 곱한다. 맨 끝이면 썸이
/// 트랙 끝에 닿는다.
pub fn thumb_start(track: f32, visible: f32, total: f32, offset: f32) -> f32 {
    let max = max_offset(visible, total);
    if max <= 0.0 {
        return 0.0;
    }
    let room = (track - thumb_length(track, visible, total)).max(0.0);
    room * (offset / max).clamp(0.0, 1.0)
}

/// 썸 끌기: 누를 때 넘긴 거리가 `pressed_offset`이고 그 뒤 포인터가 트랙을 따라 `delta`만큼 움직였을 때의 넘긴 거리.
pub fn dragged(track: f32, visible: f32, total: f32, pressed_offset: f32, delta: f32) -> f32 {
    let max = max_offset(visible, total);
    let room = track - thumb_length(track, visible, total);
    if max <= 0.0 || room <= 0.0 {
        return pressed_offset.clamp(0.0, max);
    }
    (pressed_offset + delta * max / room).clamp(0.0, max)
}

/// 트랙을 누른 자리 `pointer`(트랙 기준)가 썸의 어느 쪽인가: 앞(위·왼쪽)이면 -1, 뒤면 1, 썸 위면 0.
pub fn page_direction(track: f32, visible: f32, total: f32, offset: f32, pointer: f32) -> i32 {
    let start = thumb_start(track, visible, total, offset);
    let end = start + thumb_length(track, visible, total);
    if pointer < start {
        -1
    } else if pointer >= end {
        1
    } else {
        0
    }
}

/// 트랙 누르기: 누를 때 정한 쪽 `direction`(-1·1, [`page_direction`])으로 한 쪽(보이는 길이)을 넘긴 거리. 넘길
/// 수 있는 거리 안으로 자른다. 썸이 포인터에 닿았거나 지나갔으면(탐색기처럼 누르고 있는 동안 거기서 멈춘다) 그대로다.
pub fn paged(
    track: f32,
    visible: f32,
    total: f32,
    offset: f32,
    pointer: f32,
    direction: i32,
) -> f32 {
    if direction == 0 || page_direction(track, visible, total, offset, pointer) != direction {
        return offset;
    }
    let step = visible.max(0.0) * direction as f32;
    (offset + step).clamp(0.0, max_offset(visible, total))
}

/// 창 `ui`의 전역 `ScrollMath` 콜백을 이 모듈의 함수로 잇는다. 창을 만들 때마다 부른다.
pub fn install(ui: &crate::AppWindow) {
    use slint::ComponentHandle;
    let math = ui.global::<crate::ScrollMath>();
    math.on_thumb_length(thumb_length);
    math.on_thumb_start(thumb_start);
    math.on_dragged(dragged);
    math.on_page_direction(page_direction);
    math.on_paged(paged);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10만 행 × 행 높이 24px, 보이는 높이 30행(720px), 트랙 720px.
    const ROW: f32 = 24.0;
    const TOTAL: f32 = 100_000.0 * ROW;
    const VISIBLE: f32 = 30.0 * ROW;
    const TRACK: f32 = 720.0;

    /// 큰 넘긴 거리(수백만 px)는 f32 반올림이 있어 상대 오차도 받는다.
    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 0.01 + b.abs() * 1e-6
    }

    #[test]
    fn large_list_thumb_is_minimum_length() {
        // 비례 길이는 720 × 720 / 2,400,000 = 0.216px라 최소 길이가 된다.
        assert_eq!(thumb_length(TRACK, VISIBLE, TOTAL), MIN_THUMB);
    }

    #[test]
    fn thumb_length_is_proportional_when_large_enough() {
        // 보이는 길이가 전체의 1/4이면 썸도 트랙의 1/4이다.
        assert!(close(thumb_length(400.0, 100.0, 400.0), 100.0));
        // 다 보이면 트랙 전체.
        assert!(close(thumb_length(400.0, 500.0, 300.0), 400.0));
        // 트랙이 최소 길이보다 짧으면 트랙 길이.
        assert!(close(thumb_length(10.0, 1.0, 1000.0), 10.0));
    }

    #[test]
    fn thumb_at_top_middle_and_end_of_large_list() {
        let max = max_offset(VISIBLE, TOTAL);
        assert!(close(thumb_start(TRACK, VISIBLE, TOTAL, 0.0), 0.0));
        // 가운데: 움직일 수 있는 700px의 반.
        assert!(close(thumb_start(TRACK, VISIBLE, TOTAL, max / 2.0), 350.0));
        // 맨 끝: 썸의 끝이 트랙 끝에 닿는다.
        let end = thumb_start(TRACK, VISIBLE, TOTAL, max) + thumb_length(TRACK, VISIBLE, TOTAL);
        assert!(close(end, TRACK));
        // 범위 밖 넘긴 거리는 트랙 안으로 자른다.
        assert!(close(
            thumb_start(TRACK, VISIBLE, TOTAL, max * 2.0),
            TRACK - MIN_THUMB
        ));
        assert!(close(thumb_start(TRACK, VISIBLE, TOTAL, -10.0), 0.0));
    }

    #[test]
    fn thumb_at_start_when_everything_is_visible() {
        assert_eq!(thumb_start(TRACK, 800.0, 500.0, 0.0), 0.0);
    }

    #[test]
    fn dragging_minimum_thumb_across_track_reaches_end() {
        let max = max_offset(VISIBLE, TOTAL);
        // 썸이 움직일 수 있는 길이(700px)를 다 끌면 맨 끝, 반이면 가운데.
        assert!(close(dragged(TRACK, VISIBLE, TOTAL, 0.0, 700.0), max));
        assert!((dragged(TRACK, VISIBLE, TOTAL, 0.0, 350.0) - max / 2.0).abs() < 1.0);
        // 넘치면 자른다.
        assert!(close(dragged(TRACK, VISIBLE, TOTAL, 0.0, 5000.0), max));
        assert!(close(dragged(TRACK, VISIBLE, TOTAL, max, -5000.0), 0.0));
        // 끌어 간 자리의 썸은 포인터를 따라간다.
        let offset = dragged(TRACK, VISIBLE, TOTAL, 0.0, 123.0);
        assert!((thumb_start(TRACK, VISIBLE, TOTAL, offset) - 123.0).abs() < 0.01);
    }

    #[test]
    fn track_press_pages_by_visible_length() {
        // 맨 위에서 트랙 아래쪽을 누르면 한 쪽(720px) 넘어간다.
        let direction = page_direction(TRACK, VISIBLE, TOTAL, 0.0, 600.0);
        assert_eq!(direction, 1);
        assert!(close(
            paged(TRACK, VISIBLE, TOTAL, 0.0, 600.0, direction),
            VISIBLE
        ));
        // 위쪽을 누르면 한 쪽 거슬러 간다.
        let max = max_offset(VISIBLE, TOTAL);
        let offset = max / 2.0;
        let direction = page_direction(TRACK, VISIBLE, TOTAL, offset, 10.0);
        assert_eq!(direction, -1);
        assert!(close(
            paged(TRACK, VISIBLE, TOTAL, offset, 10.0, direction),
            offset - VISIBLE
        ));
    }

    #[test]
    fn track_press_clamps_at_ends() {
        // 보이는 길이 100, 전체 250: 넘길 수 있는 거리 150.
        assert_eq!(paged(400.0, 100.0, 250.0, 100.0, 399.0, 1), 150.0);
        assert_eq!(paged(400.0, 100.0, 250.0, 50.0, 0.0, -1), 0.0);
    }

    #[test]
    fn track_press_stops_when_thumb_reaches_pointer() {
        // 썸 위를 누르면 넘기지 않는다.
        assert_eq!(page_direction(TRACK, VISIBLE, TOTAL, 0.0, 5.0), 0);
        assert_eq!(paged(TRACK, VISIBLE, TOTAL, 0.0, 5.0, 0), 0.0);
        // 아래로 넘기다 썸이 포인터에 닿으면 그 자리에 멈추고, 지나쳐도 거꾸로 가지 않는다.
        let pointer = 100.0;
        let mut offset = 0.0;
        for _ in 0..1_000_000 {
            let next = paged(TRACK, VISIBLE, TOTAL, offset, pointer, 1);
            if next == offset {
                break;
            }
            offset = next;
        }
        let start = thumb_start(TRACK, VISIBLE, TOTAL, offset);
        assert!(start <= pointer && pointer < start + MIN_THUMB + 1.0);
        assert_eq!(paged(TRACK, VISIBLE, TOTAL, offset, pointer, 1), offset);
        assert_eq!(page_direction(TRACK, VISIBLE, TOTAL, offset, pointer), 0);
    }
}
