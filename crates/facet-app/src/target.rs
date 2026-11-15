//! 복사·이동 대상 선택 창의 상태: 코어가 준 내용(원본, 반대편 패널 위치, 최근 대상), 자동 완성 후보, 고른 줄.
//!
//! 후보 읽기는 코어가 일꾼 스레드에서 한다([`facet_core::window::App::complete_address`]). 여기서는 받은 결과가
//! 지금 기다리는 요청의 것인지 가리고(늦게 온 앞선 요청의 후보는 버림), 보일 줄을 정하고, ↑↓·Tab으로 줄을
//! 차례로 고른다. 선택 창은 Windows 폴더 선택 대화상자를 쓰지 않고 facet이 그린다(ADR-0006).

use facet_core::Candidate;
use facet_core::tab::CompletionId;
use facet_core::window::TransferChoice;

use crate::address::AddressEdit;

/// 줄의 출처.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// 반대편 패널의 위치(패널이 둘일 때).
    Opposite,
    /// 최근 대상.
    Recent,
    /// 친 글자의 자동 완성 후보(하위 폴더).
    Completion,
}

/// 선택 창의 줄 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub section: Section,
    /// 전체 경로 글자.
    pub path: String,
}

/// 선택 창 하나의 상태. 창을 열 때 새로 만들고 닫을 때 버린다.
#[derive(Debug)]
pub struct TargetChooser {
    choice: TransferChoice,
    completion: AddressEdit,
    /// 글자가 비어 있지 않아 후보를 보이는 중인가(아니면 반대편 위치와 최근 대상을 보인다).
    completing: bool,
    highlight: Option<usize>,
}

impl TargetChooser {
    pub fn new(choice: TransferChoice) -> Self {
        TargetChooser {
            choice,
            completion: AddressEdit::default(),
            completing: false,
            highlight: None,
        }
    }

    pub fn choice(&self) -> &TransferChoice {
        &self.choice
    }

    /// 글자가 비었다: 후보를 버리고 반대편 위치와 최근 대상을 다시 보인다. 늦게 오는 앞선 후보는 받지 않는다.
    pub fn cleared(&mut self) {
        self.completion = AddressEdit::default();
        self.completing = false;
        self.highlight = None;
    }

    /// 글자가 바뀌어 새 자동 완성 요청을 보냈다. 앞선 후보는 지금 글자와 맞지 않으므로 버린다.
    pub fn requested(&mut self, id: CompletionId) {
        self.completion.request(id);
        self.completing = true;
        self.highlight = None;
    }

    /// 자동 완성 결과를 받는다. 기다리는 요청의 결과면 줄을 바꾸고 `true`다. 늦게 온 앞선 요청의 결과면 버리고
    /// `false`다.
    pub fn receive(&mut self, id: CompletionId, candidates: Option<Vec<Candidate>>) -> bool {
        let received = self.completion.receive(id, candidates);
        if received {
            self.highlight = None;
        }
        received
    }

    /// 보일 줄: 글자가 없으면 반대편 패널 위치와 최근 대상, 글자를 치는 중이면 자동 완성 후보.
    pub fn rows(&self) -> Vec<Row> {
        if self.completing {
            return self
                .completion
                .candidates()
                .iter()
                .map(|candidate| Row {
                    section: Section::Completion,
                    path: candidate.location.to_string(),
                })
                .collect();
        }
        let mut rows = Vec::new();
        if let Some(opposite) = &self.choice.opposite {
            rows.push(Row {
                section: Section::Opposite,
                path: opposite.display().to_string(),
            });
        }
        rows.extend(self.choice.recent.iter().map(|path| Row {
            section: Section::Recent,
            path: path.display().to_string(),
        }));
        rows
    }

    pub fn highlight(&self) -> Option<usize> {
        self.highlight
    }

    /// ↓·Tab(`forward`)이나 ↑·Shift+Tab으로 다음·앞 줄을 고르고 그 줄을 돌려준다. 끝에서는 반대쪽 끝으로 돈다.
    /// 처음이면 앞으로는 첫 줄, 뒤로는 마지막 줄이다. 줄이 없으면 `None`이다.
    pub fn step(&mut self, forward: bool) -> Option<Row> {
        let rows = self.rows();
        let len = rows.len();
        if len == 0 {
            return None;
        }
        let next = match (self.highlight, forward) {
            (None, true) => 0,
            (None, false) => len - 1,
            (Some(i), true) => (i + 1) % len,
            (Some(i), false) => (i + len - 1) % len,
        };
        self.highlight = Some(next);
        rows.into_iter().nth(next)
    }

    /// `index`번 줄.
    pub fn row(&self, index: usize) -> Option<Row> {
        self.rows().into_iter().nth(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use facet_core::Location;
    use facet_core::details::Sort;
    use facet_core::job::JobKind;
    use facet_core::tab::Tab;
    use std::path::PathBuf;

    fn choice(opposite: Option<&str>, recent: &[&str]) -> TransferChoice {
        TransferChoice {
            kind: JobKind::Copy,
            sources: vec![PathBuf::from(r"C:\원본\a.txt")],
            opposite: opposite.map(PathBuf::from),
            recent: recent.iter().map(PathBuf::from).collect(),
        }
    }

    fn candidates(names: &[&str]) -> Vec<Candidate> {
        names
            .iter()
            .map(|name| Candidate {
                name: (*name).into(),
                location: Location::file_system(format!(r"C:\{name}")),
            })
            .collect()
    }

    /// 요청 번호는 코어의 탭만 만든다.
    fn ids(n: usize) -> Vec<CompletionId> {
        let mut tab = Tab::new(Location::file_system("."), Sort::default(), false);
        (0..n).map(|_| tab.complete("x").id()).collect()
    }

    fn paths(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|row| row.path.as_str()).collect()
    }

    #[test]
    fn 글자가_없으면_반대편_위치와_최근_대상을_보인다() {
        let chooser = TargetChooser::new(choice(Some(r"D:\오른쪽"), &[r"E:\x", r"E:\y"]));
        let rows = chooser.rows();
        assert_eq!(paths(&rows), [r"D:\오른쪽", r"E:\x", r"E:\y"]);
        assert_eq!(rows[0].section, Section::Opposite);
        assert_eq!(rows[1].section, Section::Recent);
        // 패널이 하나면 반대편 줄이 없다.
        let single = TargetChooser::new(choice(None, &[r"E:\x"]));
        assert_eq!(paths(&single.rows()), [r"E:\x"]);
    }

    #[test]
    fn 글자를_치면_후보로_바뀌고_늦게_온_앞선_후보는_버린다() {
        let id = ids(3);
        let mut chooser = TargetChooser::new(choice(None, &[r"E:\x"]));
        chooser.requested(id[0]);
        assert!(chooser.rows().is_empty(), "후보가 오기 전");
        chooser.requested(id[1]);
        assert!(
            !chooser.receive(id[0], Some(candidates(&["a"]))),
            "앞선 요청"
        );
        assert!(chooser.receive(id[1], Some(candidates(&["b", "c"]))));
        assert_eq!(paths(&chooser.rows()), [r"C:\b", r"C:\c"]);
        assert!(
            chooser
                .rows()
                .iter()
                .all(|r| r.section == Section::Completion)
        );

        // 글자를 지우면 다시 최근 대상이고, 그 뒤 늦게 온 후보는 받지 않는다.
        chooser.requested(id[2]);
        chooser.cleared();
        assert!(!chooser.receive(id[2], Some(candidates(&["z"]))));
        assert_eq!(paths(&chooser.rows()), [r"E:\x"]);
    }

    #[test]
    fn 줄을_tab과_화살표로_돌며_고른다() {
        let mut chooser = TargetChooser::new(choice(Some(r"D:\r"), &[r"E:\x"]));
        assert_eq!(chooser.step(true).unwrap().path, r"D:\r");
        assert_eq!(chooser.step(true).unwrap().path, r"E:\x");
        assert_eq!(chooser.step(true).unwrap().path, r"D:\r", "끝에서 돈다");
        assert_eq!(chooser.step(false).unwrap().path, r"E:\x", "뒤로도 돈다");
        assert_eq!(chooser.highlight(), Some(1));
        assert_eq!(chooser.row(0).unwrap().path, r"D:\r");

        let mut chooser = TargetChooser::new(choice(Some(r"D:\r"), &[r"E:\x"]));
        assert_eq!(
            chooser.step(false).unwrap().path,
            r"E:\x",
            "처음에 ↑면 마지막 줄"
        );

        let mut empty = TargetChooser::new(choice(None, &[]));
        assert_eq!(empty.step(true), None);
    }

    #[test]
    fn 글자가_바뀌면_고른_줄을_버린다() {
        let id = ids(1);
        let mut chooser = TargetChooser::new(choice(None, &[r"E:\x"]));
        chooser.step(true);
        assert_eq!(chooser.highlight(), Some(0));
        chooser.requested(id[0]);
        assert_eq!(chooser.highlight(), None);
    }
}
