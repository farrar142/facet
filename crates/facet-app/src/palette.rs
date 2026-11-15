//! 명령 팔레트의 상태: 코어가 찾아 준 줄과 고른 줄. 검색은 코어(`command::search`)가 한다.

use facet_core::command::{CommandId, PaletteEntry};

#[derive(Default)]
pub struct Palette {
    entries: Vec<PaletteEntry>,
    highlight: Option<usize>,
}

impl Palette {
    /// 새 검색 결과를 넣는다.
    pub fn set(&mut self, entries: Vec<PaletteEntry>, highlight: Option<usize>) {
        self.entries = entries;
        self.highlight = highlight;
    }

    pub fn highlight(&self) -> Option<usize> {
        self.highlight
    }

    /// `index`번 줄의 명령. 꺼진 명령은 실행하지 않으므로 `None`이다.
    pub fn command_at(&self, index: usize) -> Option<CommandId> {
        self.entries
            .get(index)
            .filter(|entry| entry.enabled)
            .map(|entry| entry.command)
    }

    /// 다음(`forward`)이나 앞 줄을 고른다. 끝에서 멈춘다.
    pub fn step(&mut self, forward: bool) {
        if self.entries.is_empty() {
            return;
        }
        let last = self.entries.len() - 1;
        self.highlight = Some(match (self.highlight, forward) {
            (None, true) => 0,
            (None, false) => last,
            (Some(i), true) => (i + 1).min(last),
            (Some(i), false) => i.saturating_sub(1),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(command: CommandId, enabled: bool) -> PaletteEntry {
        PaletteEntry {
            command,
            shortcuts: &[],
            enabled,
        }
    }

    #[test]
    fn 화살표는_끝에서_멈춘다() {
        let mut p = Palette::default();
        p.set(
            vec![entry(CommandId::Back, true), entry(CommandId::Up, true)],
            Some(0),
        );
        p.step(false);
        assert_eq!(p.highlight(), Some(0));
        p.step(true);
        p.step(true);
        assert_eq!(p.highlight(), Some(1));
    }

    #[test]
    fn 꺼진_명령은_실행하지_않는다() {
        let mut p = Palette::default();
        p.set(
            vec![entry(CommandId::Back, false), entry(CommandId::Up, true)],
            Some(0),
        );
        assert_eq!(p.command_at(0), None);
        assert_eq!(p.command_at(1), Some(CommandId::Up));
    }
}
