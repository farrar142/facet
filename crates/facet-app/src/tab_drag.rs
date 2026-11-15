//! 탭 끌기: 누른 탭을 조금 넘게 움직이면 끌기가 되고(끌기 시작 문턱), 끄는 점이 어느 창의 어느 패널 탭 줄의 몇
//! 번째 자리(탭 사이 틈) 위인가, 아니면 facet의 어느 창 위도 아닌가를 정한다. 화면에 기대지 않는 계산만 둔다.
//! 놓으면 앱이 코어의 탭 옮기기(`App::move_tab`)를 부른다.
//!
//! 좌표는 둘이다. 창 기준 점([`Point`], 그 창 그리기 영역의 왼쪽 위에서 논리 픽셀)과 화면 점([`ScreenPoint`], 가상
//! 화면 기준 물리 픽셀, 모니터 여럿도 한 좌표계). 창마다 자리와 배율이 달라 창 사이는 화면 점으로 오간다
//! ([`Frame`]). 끄는 점 아래가 facet의 어느 창인지는 앱이 정해 [`Hover`]로 넘긴다.

/// 끌기 시작 문턱(논리 픽셀). 누른 점에서 이만큼 움직이기 전에는 끌기가 아니라 누르기다. Windows의 끌기 문턱
/// (`SM_CXDRAG`, 기본 4픽셀)보다 조금 넉넉히 두어 누르다 손이 떨려도 누르기로 남는다.
pub const DRAG_THRESHOLD: f32 = 6.0;

/// 창 기준 점(논리 픽셀).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// 화면 점(가상 화면 기준 물리 픽셀). 주 모니터 왼쪽·위의 모니터에서는 음수다.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenPoint {
    pub x: f64,
    pub y: f64,
}

/// 창 하나의 화면 자리: 그리기 영역(제목 줄·테두리 안)과 창 바깥 테두리의 왼쪽 위 화면 점, 배율(논리 픽셀 하나의
/// 물리 픽셀 수).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub client: ScreenPoint,
    pub outer: ScreenPoint,
    pub scale: f32,
}

impl Frame {
    /// 이 창 기준 점의 화면 점.
    pub fn to_screen(self, at: Point) -> ScreenPoint {
        let scale = f64::from(self.scale);
        ScreenPoint {
            x: self.client.x + f64::from(at.x) * scale,
            y: self.client.y + f64::from(at.y) * scale,
        }
    }

    /// 화면 점의 이 창 기준 점. 창 밖이면 음수나 창 크기보다 큰 값이다.
    pub fn to_window(self, at: ScreenPoint) -> Point {
        let scale = if self.scale > 0.0 {
            f64::from(self.scale)
        } else {
            1.0
        };
        Point {
            x: ((at.x - self.client.x) / scale) as f32,
            y: ((at.y - self.client.y) / scale) as f32,
        }
    }
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

    /// `index`번째 탭의 왼쪽 위(창 기준). 넘긴 만큼 왼쪽이다.
    pub fn tab_origin(&self, index: usize) -> Point {
        Point {
            x: self.left + index as f32 * self.tab_width - self.scroll,
            y: self.top,
        }
    }
}

/// 끄는 점 아래: facet의 창 `window`(그 창 기준 점 `at`, 패널마다의 탭 줄 `strips[패널]`, 없는 패널은 `None`), 또는
/// facet의 어느 창도 아닌 곳(다른 프로그램의 창, 바탕 화면, 다른 모니터).
#[derive(Debug, Clone, PartialEq)]
pub enum Hover<W> {
    Window {
        window: W,
        at: Point,
        strips: Vec<Option<Strip>>,
    },
    Outside(ScreenPoint),
}

/// 지금 놓으면 탭이 갈 곳.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target<W> {
    /// 창 `window`의 패널 `panel` 탭 줄의 자리 `slot`(탭 사이 틈).
    Strip {
        window: W,
        panel: usize,
        slot: usize,
    },
    /// facet의 어느 창 위도 아니다: 놓은 화면 점에 새 창을 연다.
    NewWindow(ScreenPoint),
}

/// 누른 탭 하나의 끌기 상태. `W`는 창을 가리키는 번호다.
#[derive(Debug, Clone, PartialEq)]
pub struct TabDrag<W> {
    /// 누른 탭의 창, 패널(0 왼쪽), 그 패널 안 번호.
    pub window: W,
    pub panel: usize,
    pub index: usize,
    start: Point,
    grab: Point,
    dragging: bool,
    target: Option<Target<W>>,
}

impl<W: Copy + PartialEq> TabDrag<W> {
    /// 창 `window` 패널 `panel`의 `index`번째 탭을 `start`(창 기준)에서 눌렀다. `grab`은 누른 점이 그 탭의 왼쪽
    /// 위에서 떨어진 거리다(새 창을 띄울 때 탭이 같은 자리로 손 아래 오게 한다). 아직 끌기가 아니다.
    pub fn new(window: W, panel: usize, index: usize, start: Point, grab: Point) -> Self {
        TabDrag {
            window,
            panel,
            index,
            start,
            grab,
            dragging: false,
            target: None,
        }
    }

    /// 누른 채 `at`(누른 창 기준)으로 움직였다. 문턱을 넘으면 끌기가 되고(한번 되면 돌아오지 않는다), 끄는 중이면
    /// 그 점 아래(`hover`, 끄는 중에만 부른다)에서 놓일 곳을 찾는다. facet 창 위지만 탭 줄 밖이면 놓일 곳이 없다
    /// (놓아도 그대로 둔다).
    pub fn moved(&mut self, at: Point, hover: impl FnOnce() -> Hover<W>) {
        if !self.dragging {
            let (dx, dy) = (at.x - self.start.x, at.y - self.start.y);
            self.dragging = dx.hypot(dy) >= DRAG_THRESHOLD;
        }
        if !self.dragging {
            return;
        }
        self.target = match hover() {
            Hover::Window { window, at, strips } => {
                strips.iter().enumerate().find_map(|(panel, strip)| {
                    strip
                        .as_ref()
                        .and_then(|strip| strip.slot_at(at))
                        .map(|slot| Target::Strip {
                            window,
                            panel,
                            slot,
                        })
                })
            }
            Hover::Outside(at) => Some(Target::NewWindow(at)),
        };
    }

    /// 끄는 중인가(문턱을 넘었다). 아니면 놓았을 때 누르기다.
    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// 지금 놓으면 갈 곳. facet 창 위의 탭 줄 밖이면 `None`이다.
    pub fn target(&self) -> Option<Target<W>> {
        self.target
    }

    /// 창 `window`의 패널 `panel` 탭 줄에서 놓일 자리. 그 줄 위가 아니면 `None`이다.
    pub fn slot_in(&self, window: W, panel: usize) -> Option<usize> {
        match self.target {
            Some(Target::Strip {
                window: w,
                panel: p,
                slot,
            }) if w == window && p == panel => Some(slot),
            _ => None,
        }
    }

    /// 놓은 화면 점 `drop`에 띄울 새 창의 바깥 테두리 왼쪽 위. 새 창의 첫 탭이 손 아래(누른 탭을 잡은 자리)에 오도록
    /// 끈 창의 테두리 두께(`source`)와 첫 탭의 자리(`first_tab`, 끈 창 기준. 새 창도 같은 배치다)만큼 당긴다.
    pub fn new_window_origin(
        &self,
        drop: ScreenPoint,
        source: &Frame,
        first_tab: Point,
    ) -> ScreenPoint {
        let scale = f64::from(source.scale);
        ScreenPoint {
            x: drop.x
                - (source.client.x - source.outer.x)
                - f64::from(first_tab.x + self.grab.x) * scale,
            y: drop.y
                - (source.client.y - source.outer.y)
                - f64::from(first_tab.y + self.grab.y) * scale,
        }
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

    fn screen(x: f64, y: f64) -> ScreenPoint {
        ScreenPoint { x, y }
    }

    /// 창 1 안의 점: 탭 줄 `strips`.
    fn over(window: u32, point: Point, strips: &[Option<Strip>]) -> impl FnOnce() -> Hover<u32> {
        let strips = strips.to_vec();
        move || Hover::Window {
            window,
            at: point,
            strips,
        }
    }

    fn drag_from(window: u32, panel: usize, index: usize, start: Point) -> TabDrag<u32> {
        TabDrag::new(window, panel, index, start, at(10.0, 5.0))
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
        assert_eq!(s.tab_origin(3), at(50.0, 10.0));
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
        let mut drag = drag_from(1, 0, 1, at(150.0, 20.0));
        drag.moved(at(153.0, 23.0), || panic!("끌기 전에는 아래를 보지 않는다"));
        assert!(!drag.is_dragging());
        assert_eq!(drag.target(), None);
        drag.moved(at(156.0, 20.0), over(1, at(156.0, 20.0), &strips));
        assert!(drag.is_dragging());
        assert_eq!(
            drag.target(),
            Some(Target::Strip {
                window: 1,
                panel: 0,
                slot: 2
            })
        );
        // 한번 끌기가 되면 누른 자리로 돌아와도 끌기다.
        drag.moved(at(150.0, 20.0), over(1, at(150.0, 20.0), &strips));
        assert!(drag.is_dragging());
        assert_eq!(drag.slot_in(1, 0), Some(2));
    }

    #[test]
    fn 반대편_패널의_탭_줄_위면_그_패널이_목적지이고_줄_밖이면_없다() {
        let strips = [Some(strip(0.0, 3)), Some(strip(500.0, 2))];
        let mut drag = drag_from(1, 0, 0, at(10.0, 20.0));
        drag.moved(at(540.0, 20.0), over(1, at(540.0, 20.0), &strips));
        assert_eq!(drag.slot_in(1, 1), Some(0));
        assert_eq!(drag.slot_in(1, 0), None);
        drag.moved(at(700.0, 20.0), over(1, at(700.0, 20.0), &strips));
        assert_eq!(drag.slot_in(1, 1), Some(2));
        drag.moved(at(700.0, 300.0), over(1, at(700.0, 300.0), &strips));
        assert_eq!(drag.target(), None);
    }

    #[test]
    fn 다른_창의_탭_줄_위면_그_창의_자리가_목적지다() {
        let mut drag = drag_from(1, 0, 0, at(10.0, 20.0));
        // 누른 창 기준으로는 창 밖(오른쪽)이지만, 그 점은 창 2의 탭 줄 위다.
        drag.moved(
            at(1200.0, 20.0),
            over(2, at(160.0, 20.0), &[Some(strip(0.0, 4))]),
        );
        assert_eq!(
            drag.target(),
            Some(Target::Strip {
                window: 2,
                panel: 0,
                slot: 2
            })
        );
        assert_eq!(drag.slot_in(1, 0), None);
        assert_eq!(drag.slot_in(2, 0), Some(2));
        // 창 2 위지만 탭 줄 밖이면 놓아도 그대로 둔다.
        drag.moved(
            at(1200.0, 300.0),
            over(2, at(160.0, 300.0), &[Some(strip(0.0, 4))]),
        );
        assert_eq!(drag.target(), None);
    }

    #[test]
    fn facet_창_밖이면_놓은_화면_점에_새_창이다() {
        let mut drag = drag_from(1, 0, 0, at(10.0, 20.0));
        drag.moved(at(-500.0, 900.0), || {
            Hover::Outside(screen(-2000.0, 1500.0))
        });
        assert_eq!(
            drag.target(),
            Some(Target::NewWindow(screen(-2000.0, 1500.0)))
        );
        assert_eq!(drag.slot_in(1, 0), None);
    }

    #[test]
    fn 창_기준_점과_화면_점은_창_자리와_배율로_오간다() {
        let frame = Frame {
            client: screen(-1912.0, 131.0),
            outer: screen(-1920.0, 100.0),
            scale: 1.5,
        };
        assert_eq!(frame.to_screen(at(100.0, 20.0)), screen(-1762.0, 161.0));
        assert_eq!(frame.to_window(screen(-1762.0, 161.0)), at(100.0, 20.0));
        // 창 밖은 음수나 창보다 큰 값이다.
        assert_eq!(frame.to_window(screen(-1942.0, 101.0)), at(-20.0, -20.0));
    }

    #[test]
    fn 새_창은_첫_탭이_잡은_자리로_손_아래_오게_놓는다() {
        let source = Frame {
            client: screen(108.0, 131.0),
            outer: screen(100.0, 100.0),
            scale: 2.0,
        };
        // 탭의 (10, 5) 자리를 잡았고, 첫 탭은 창 기준 (0, 4)에 있다.
        let drag = drag_from(1, 0, 3, at(310.0, 9.0));
        let origin = drag.new_window_origin(screen(3000.0, 500.0), &source, at(0.0, 4.0));
        // 테두리(8, 31)와 (첫 탭 + 잡은 자리) × 배율 (20, 18)만큼 당긴다.
        assert_eq!(origin, screen(2972.0, 451.0));
    }
}
