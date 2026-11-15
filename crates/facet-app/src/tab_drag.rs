//! 탭 끌기: 누른 탭을 조금 넘게 움직이면 끌기가 되고(끌기 시작 문턱), 끄는 점이 어느 패널 탭 줄의 몇 번째 자리
//! (탭 사이 틈) 위인가를 정한다. 화면에 기대지 않는 계산만 둔다. 놓으면 앱이 코어의 탭 옮기기
//! (`App::move_tab`)를 부른다.

/// 끌기 시작 문턱(논리 픽셀). 누른 점에서 이만큼 움직이기 전에는 끌기가 아니라 누르기다. Windows의 끌기 문턱
/// (`SM_CXDRAG`, 기본 4픽셀)보다 조금 넉넉히 두어 누르다 손이 떨려도 누르기로 남는다.
pub const DRAG_THRESHOLD: f32 = 6.0;

/// 창 기준 점(논리 픽셀).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// 패널 하나의 탭 줄: 창 기준 자리와 크기, 탭 너비, 넘긴 거리(0 이상), 탭 수.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Strip {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
    pub tab_width: f32,
    pub scroll: f32,
    pub tabs: usize,
}

impl Strip {
    /// 점 `at`이 이 탭 줄 위면 탭이 놓일 자리(0은 첫 탭 앞, 탭 수면 끝)다. 끌다 보면 손이 위아래로 흔들리므로 줄
    /// 높이만큼 위아래로 넉넉히 본다. 가로로는 줄 안이어야 한다.
    pub fn slot_at(&self, at: Point) -> Option<usize> {
        let inside_x = at.x >= self.left && at.x <= self.left + self.width;
        let inside_y = at.y >= self.top - self.height && at.y <= self.top + 2.0 * self.height;
        if !inside_x || !inside_y || self.tab_width <= 0.0 {
            return None;
        }
        let along = (at.x - self.left + self.scroll) / self.tab_width;
        // 탭 가운데를 넘으면 그 탭 뒤 자리다.
        let slot = along.round().max(0.0) as usize;
        Some(slot.min(self.tabs))
    }
}

/// 누른 탭 하나의 끌기 상태.
#[derive(Debug, Clone, PartialEq)]
pub struct TabDrag {
    /// 누른 탭의 패널(0 왼쪽)과 그 패널 안 번호.
    pub panel: usize,
    pub index: usize,
    start: Point,
    dragging: bool,
    target: Option<(usize, usize)>,
}

impl TabDrag {
    /// 패널 `panel`의 `index`번째 탭을 `start`에서 눌렀다. 아직 끌기가 아니다.
    pub fn new(panel: usize, index: usize, start: Point) -> Self {
        TabDrag {
            panel,
            index,
            start,
            dragging: false,
            target: None,
        }
    }

    /// 누른 채 `at`으로 움직였다. 문턱을 넘으면 끌기가 되고(한번 되면 돌아오지 않는다), 끄는 중이면 놓일 곳을
    /// 패널마다의 탭 줄(`strips[패널]`, 없는 패널은 `None`)에서 찾는다.
    pub fn moved(&mut self, at: Point, strips: &[Option<Strip>]) {
        if !self.dragging {
            let (dx, dy) = (at.x - self.start.x, at.y - self.start.y);
            self.dragging = dx.hypot(dy) >= DRAG_THRESHOLD;
        }
        if !self.dragging {
            return;
        }
        self.target = strips.iter().enumerate().find_map(|(panel, strip)| {
            strip
                .as_ref()
                .and_then(|strip| strip.slot_at(at))
                .map(|slot| (panel, slot))
        });
    }

    /// 끄는 중인가(문턱을 넘었다). 아니면 놓았을 때 누르기다.
    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// 지금 놓으면 갈 곳: 패널 번호와 그 패널의 자리. 탭 줄 밖이면 `None`이다.
    pub fn target(&self) -> Option<(usize, usize)> {
        self.target
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(left: f32, tabs: usize) -> Strip {
        Strip {
            left,
            top: 10.0,
            width: 400.0,
            height: 30.0,
            tab_width: 100.0,
            scroll: 0.0,
            tabs,
        }
    }

    fn at(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    #[test]
    fn 자리는_탭_가운데를_기준으로_정하고_끝을_넘지_않는다() {
        let s = strip(0.0, 3);
        assert_eq!(s.slot_at(at(0.0, 20.0)), Some(0));
        assert_eq!(s.slot_at(at(49.0, 20.0)), Some(0));
        assert_eq!(s.slot_at(at(51.0, 20.0)), Some(1));
        assert_eq!(s.slot_at(at(149.0, 20.0)), Some(1));
        assert_eq!(s.slot_at(at(260.0, 20.0)), Some(3));
        // 마지막 탭 뒤 빈 곳도 끝이다.
        assert_eq!(s.slot_at(at(390.0, 20.0)), Some(3));
    }

    #[test]
    fn 넘긴_탭_줄은_넘긴_만큼_뒤의_자리다() {
        let s = Strip {
            scroll: 250.0,
            ..strip(0.0, 8)
        };
        assert_eq!(s.slot_at(at(0.0, 20.0)), Some(3));
        assert_eq!(s.slot_at(at(100.0, 20.0)), Some(4));
    }

    #[test]
    fn 줄_높이만큼_위아래는_줄_위로_보고_그보다_멀거나_가로로_밖이면_아니다() {
        let s = strip(100.0, 2);
        assert_eq!(s.slot_at(at(150.0, -19.0)), Some(1));
        assert_eq!(s.slot_at(at(150.0, 69.0)), Some(1));
        assert_eq!(s.slot_at(at(150.0, -21.0)), None);
        assert_eq!(s.slot_at(at(150.0, 71.0)), None);
        assert_eq!(s.slot_at(at(99.0, 20.0)), None);
        assert_eq!(s.slot_at(at(501.0, 20.0)), None);
    }

    #[test]
    fn 문턱보다_적게_움직이면_누르기이고_넘으면_끌기가_된다() {
        let strips = [Some(strip(0.0, 3)), None];
        let mut drag = TabDrag::new(0, 1, at(150.0, 20.0));
        drag.moved(at(153.0, 23.0), &strips);
        assert!(!drag.is_dragging());
        assert_eq!(drag.target(), None);
        drag.moved(at(156.0, 20.0), &strips);
        assert!(drag.is_dragging());
        assert_eq!(drag.target(), Some((0, 2)));
        // 한번 끌기가 되면 누른 자리로 돌아와도 끌기다.
        drag.moved(at(150.0, 20.0), &strips);
        assert!(drag.is_dragging());
        assert_eq!(drag.target(), Some((0, 2)));
    }

    #[test]
    fn 반대편_패널의_탭_줄_위면_그_패널이_목적지이고_줄_밖이면_없다() {
        let strips = [Some(strip(0.0, 3)), Some(strip(500.0, 2))];
        let mut drag = TabDrag::new(0, 0, at(10.0, 20.0));
        drag.moved(at(540.0, 20.0), &strips);
        assert_eq!(drag.target(), Some((1, 0)));
        drag.moved(at(700.0, 20.0), &strips);
        assert_eq!(drag.target(), Some((1, 2)));
        drag.moved(at(700.0, 300.0), &strips);
        assert_eq!(drag.target(), None);
    }
}
