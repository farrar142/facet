//! 오른쪽 버튼으로 끌어 놓으면 놓은 자리에 띄우는 고르기 메뉴: 「여기에 복사」「여기로 이동」「취소」.
//!
//! 무엇을 할 수 있는지는 코어 결정(`dragdrop::Decision::Menu`)이 정하고, 할 수 없는 항목은 숨기지 않고 꺼서 자리를
//! 고정한다(오른쪽 메뉴와 같다). 그리기는 오른쪽 메뉴와 같은 화면 요소(`ContextMenuView`)와 줄 높이·너비를 쓴다.
//! 이 모듈은 줄 고르기(↑↓, 꺼진 줄·구분선은 건너뛰고 끝에서 돔)·누르기·Enter를 맡는다. 화면에 기대지 않는다.

use crate::context_menu::{PADDING, ROW_HEIGHT, SEPARATOR_HEIGHT};

/// 메뉴 줄.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    CopyHere,
    MoveHere,
    Separator,
    Cancel,
}

/// 메뉴의 고정 순서.
pub const ENTRIES: [Entry; 4] = [
    Entry::CopyHere,
    Entry::MoveHere,
    Entry::Separator,
    Entry::Cancel,
];

/// 고른 것.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Copy,
    Move,
    Cancel,
}

/// 열린 고르기 메뉴.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropMenu {
    copy: bool,
    mov: bool,
    selected: Option<usize>,
}

impl DropMenu {
    /// 복사·이동을 할 수 있는지와 처음 고를 효과(`default_move`면 이동, 아니면 복사, 할 수 없으면 다른 것).
    pub fn new(copy: bool, mov: bool, default_move: bool) -> DropMenu {
        let mut menu = DropMenu {
            copy,
            mov,
            selected: None,
        };
        let first = if default_move { 1 } else { 0 };
        menu.selected = [first, 1 - first, 3].into_iter().find(|i| menu.enabled(*i));
        menu
    }

    /// 줄 `i`를 고를 수 있는가.
    pub fn enabled(&self, i: usize) -> bool {
        match ENTRIES.get(i) {
            Some(Entry::CopyHere) => self.copy,
            Some(Entry::MoveHere) => self.mov,
            Some(Entry::Cancel) => true,
            Some(Entry::Separator) | None => false,
        }
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// 아래(`delta` 1)·위(-1)의 고를 수 있는 줄로. 끝에서 돈다.
    pub fn step(&mut self, delta: i32) {
        let len = ENTRIES.len();
        let start = self
            .selected
            .unwrap_or(if delta >= 0 { len - 1 } else { 0 });
        let next = (1..=len)
            .map(|k| {
                if delta >= 0 {
                    (start + k) % len
                } else {
                    (start + len - k % len) % len
                }
            })
            .find(|i| self.enabled(*i));
        self.selected = next.or(self.selected);
    }

    /// 줄 위로 마우스가 왔다.
    pub fn hover(&mut self, i: usize) {
        if self.enabled(i) {
            self.selected = Some(i);
        }
    }

    /// 줄을 눌렀다. 꺼진 줄·구분선은 `None`.
    pub fn activate(&self, i: usize) -> Option<Choice> {
        if !self.enabled(i) {
            return None;
        }
        match ENTRIES[i] {
            Entry::CopyHere => Some(Choice::Copy),
            Entry::MoveHere => Some(Choice::Move),
            Entry::Cancel => Some(Choice::Cancel),
            Entry::Separator => None,
        }
    }

    /// Enter: 고른 줄을 누른다.
    pub fn enter(&self) -> Option<Choice> {
        self.selected.and_then(|i| self.activate(i))
    }

    /// 메뉴 높이(오른쪽 메뉴와 같은 줄 높이).
    pub fn height() -> f32 {
        2.0 * PADDING
            + ENTRIES
                .iter()
                .map(|e| {
                    if *e == Entry::Separator {
                        SEPARATOR_HEIGHT
                    } else {
                        ROW_HEIGHT
                    }
                })
                .sum::<f32>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 처음에는_기본_효과를_고른다() {
        assert_eq!(DropMenu::new(true, true, true).selected(), Some(1));
        assert_eq!(DropMenu::new(true, true, false).selected(), Some(0));
        assert_eq!(
            DropMenu::new(true, false, true).selected(),
            Some(0),
            "이동을 못 하면 복사"
        );
    }

    #[test]
    fn 꺼진_줄과_구분선은_건너뛴다() {
        let mut menu = DropMenu::new(true, false, false);
        menu.step(1);
        assert_eq!(
            menu.selected(),
            Some(3),
            "이동(꺼짐)과 구분선을 건너뛰어 취소"
        );
        menu.step(1);
        assert_eq!(menu.selected(), Some(0), "끝에서 처음으로 돈다");
        menu.step(-1);
        assert_eq!(menu.selected(), Some(3));
        menu.hover(1);
        assert_eq!(menu.selected(), Some(3), "꺼진 줄은 고르지 않는다");
    }

    #[test]
    fn 누르면_고른_것이_된다() {
        let menu = DropMenu::new(true, true, true);
        assert_eq!(menu.activate(0), Some(Choice::Copy));
        assert_eq!(menu.activate(1), Some(Choice::Move));
        assert_eq!(menu.activate(2), None);
        assert_eq!(menu.activate(3), Some(Choice::Cancel));
        assert_eq!(menu.enter(), Some(Choice::Move));
        let only_copy = DropMenu::new(true, false, true);
        assert_eq!(only_copy.activate(1), None);
    }

    #[test]
    fn 높이는_세_줄과_구분선_하나() {
        assert_eq!(
            DropMenu::height(),
            2.0 * PADDING + 3.0 * ROW_HEIGHT + SEPARATOR_HEIGHT
        );
    }
}
