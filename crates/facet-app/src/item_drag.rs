//! 항목 끌기의 시작: 항목 위(아이콘·이름 글자)를 누른 뒤 끌기 시작 문턱([`DRAG_THRESHOLD`], 탭 끌기·사각형 선택과
//! 같음)을 넘게 움직이면 OLE 끌기를 시작한다(`facet_win::drag_source`). 넘기 전에 놓으면 누르기다.
//!
//! 탐색기처럼 고르기를 미루는 경우가 있다: 이미 선택된 항목을 Shift 없이 누르면 누를 때는 선택을 그대로 두고(여럿을
//! 골라 둔 채 끌 수 있게), 끌지 않고 놓을 때 그 클릭을 한다(그 항목 하나만 고르기, Ctrl이면 빼기). 선택에 없는
//! 항목은 누를 때 곧바로 고른다. 그래서 끄는 항목은 늘 누른 항목이 든 선택이다.
//!
//! 오른쪽 버튼은 누를 때 선택만 정하고(선택에 없으면 그 항목 하나) 메뉴는 끌지 않고 놓을 때 연다. 끌면 오른쪽 끌기다.
//! 화면에 기대지 않는 계산만 둔다.

use crate::tab_drag::{DRAG_THRESHOLD, Point};

/// 누른 버튼.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
}

/// 왼쪽 버튼으로 항목을 누를 때 클릭(고르기)을 놓을 때로 미루는가: 이미 선택된 항목을 Shift 없이 누를 때.
pub fn defers_click(selected: bool, shift: bool) -> bool {
    selected && !shift
}

/// 항목 위 누르기 하나. 누르면 생기고 놓거나 끌기를 시작하거나 끊기면 없어진다. `T`는 누른 탭의 번호다.
#[derive(Debug, Clone)]
pub struct ItemPress<T> {
    /// 누른 패널.
    pub panel: usize,
    /// 누른 패널의 보이던 탭.
    pub tab: T,
    /// 누른 항목 번호.
    pub row: usize,
    pub button: Button,
    /// 놓을 때 할 클릭(Ctrl·Shift). 누를 때 이미 골랐으면 `None`.
    pub deferred: Option<(bool, bool)>,
    /// 누른 창 기준 점.
    pub pressed: Point,
    /// 문턱을 넘게 끌었지만 위치가 끌기 원본을 막는다(휴지통): 놓을 때까지 금지 커서를 보이고 놓으면 아무것도 하지 않는다.
    pub refused: bool,
}

impl<T> ItemPress<T> {
    /// 마우스가 `at`으로 왔다: 누른 점에서 문턱을 넘었으면 `true`(끌기를 시작할 때).
    pub fn crossed(&self, at: Point) -> bool {
        let (dx, dy) = (at.x - self.pressed.x, at.y - self.pressed.y);
        dx.hypot(dy) > DRAG_THRESHOLD
    }

    /// 버튼 `button`을 놓은 것이 이 누르기를 끝내는가.
    pub fn released_by(&self, button: Button) -> bool {
        self.button == button
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(button: Button) -> ItemPress<u32> {
        ItemPress {
            panel: 0,
            tab: 7,
            row: 3,
            button,
            deferred: None,
            pressed: Point { x: 100.0, y: 50.0 },
            refused: false,
        }
    }

    #[test]
    fn 문턱을_넘어야_끌기다() {
        let p = press(Button::Left);
        assert!(!p.crossed(Point { x: 103.0, y: 54.0 }), "5픽셀은 누르기");
        assert!(
            !p.crossed(Point { x: 106.0, y: 50.0 }),
            "문턱과 같으면 아직 누르기"
        );
        assert!(p.crossed(Point { x: 100.0, y: 57.0 }));
        assert!(p.crossed(Point { x: 90.0, y: 40.0 }), "어느 쪽으로든");
    }

    #[test]
    fn 이미_선택된_항목은_shift_없이_누르면_고르기를_미룬다() {
        assert!(defers_click(true, false));
        assert!(!defers_click(true, true), "Shift는 범위 고르기라 곧바로");
        assert!(!defers_click(false, false), "선택에 없으면 곧바로 고른다");
        assert!(!defers_click(false, true));
    }

    #[test]
    fn 누른_버튼을_놓아야_끝난다() {
        assert!(press(Button::Left).released_by(Button::Left));
        assert!(!press(Button::Left).released_by(Button::Right));
        assert!(press(Button::Right).released_by(Button::Right));
    }
}
