//! 일괄 이름 바꾸기 창의 상태: 규칙 입력값과 마지막 미리 보기.
//!
//! 입력란의 글자(모두 글자 그대로)를 코어의 규칙([`RenameRule`])으로 바꾸고, 코어가 준 미리 보기([`Preview`])를
//! 화면에 그릴 줄([`Line`])로 바꾼다. 화면 문장은 [`crate::text`]가 만든다(ADR-0007).

use facet_core::batch_rename::{EntryStatus, NumberPosition, Numbering, Preview, RenameRule};
use facet_core::window::BatchPreviewId;

use crate::text;

/// 자릿수 입력의 위쪽 한계(이름 길이 한계와 같은 크기: 더 크면 어차피 쓸 수 없는 이름이고 거대한 글자를 만들지 않게).
const MAX_DIGITS: usize = 255;

/// 입력란의 값들.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inputs {
    pub find: String,
    pub replace: String,
    pub prefix: String,
    pub suffix: String,
    /// 「번호 매기기」 확인란.
    pub numbering: bool,
    /// 시작 번호 글자(수가 아니거나 비면 1).
    pub start: String,
    /// 자릿수 글자(수가 아니거나 비면 0: 채우지 않음).
    pub digits: String,
    /// 번호를 줄기 뒤에 둘지(아니면 앞).
    pub at_end: bool,
    /// 번호와 줄기 사이의 구분 글자.
    pub separator: String,
}

impl Default for Inputs {
    fn default() -> Self {
        Self {
            find: String::new(),
            replace: String::new(),
            prefix: String::new(),
            suffix: String::new(),
            numbering: false,
            start: "1".to_owned(),
            digits: "1".to_owned(),
            at_end: true,
            separator: String::new(),
        }
    }
}

impl Inputs {
    /// 코어의 규칙으로 바꾼다. 번호 설정은 확인란이 켜졌을 때만 쓴다.
    pub fn rule(&self) -> RenameRule {
        RenameRule {
            find: self.find.clone(),
            replace: self.replace.clone(),
            prefix: self.prefix.clone(),
            suffix: self.suffix.clone(),
            numbering: self.numbering.then(|| Numbering {
                start: self.start.trim().parse().unwrap_or(1),
                digits: self
                    .digits
                    .trim()
                    .parse::<usize>()
                    .unwrap_or(0)
                    .min(MAX_DIGITS),
                position: if self.at_end {
                    NumberPosition::End
                } else {
                    NumberPosition::Start
                },
                separator: self.separator.clone(),
            }),
        }
    }
}

/// 줄의 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// 이름이 바뀐다.
    Rename,
    /// 그대로(흐리게).
    Unchanged,
    /// 문제(빨갛게).
    Problem,
}

/// 미리 보기 목록의 줄 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub original: String,
    pub new_name: String,
    pub kind: LineKind,
    /// 문제의 까닭 글자(문제 줄만, 아니면 비어 있음).
    pub problem: String,
}

/// 미리 보기의 줄들(넘긴 항목 차례).
pub fn lines(preview: &Preview) -> Vec<Line> {
    preview
        .entries()
        .iter()
        .map(|entry| {
            let (kind, problem) = match entry.status {
                EntryStatus::Rename => (LineKind::Rename, String::new()),
                EntryStatus::Unchanged => (LineKind::Unchanged, String::new()),
                EntryStatus::Problem(problem) => {
                    (LineKind::Problem, text::batch_problem_text(problem))
                }
            };
            Line {
                original: entry.original.to_string_lossy().into_owned(),
                new_name: entry.new_name.clone(),
                kind,
                problem,
            }
        })
        .collect()
}

/// 보일 줄: `problems_only`면 문제 행만(코어의 `Preview::problem_rows`), 아니면 모두(넘긴 차례).
pub fn visible_lines(preview: &Preview, problems_only: bool) -> Vec<Line> {
    let all = lines(preview);
    if !problems_only {
        return all;
    }
    let mut all: Vec<Option<Line>> = all.into_iter().map(Some).collect();
    preview
        .problem_rows()
        .into_iter()
        .filter_map(|row| all.get_mut(row).and_then(Option::take))
        .collect()
}

/// 창의 상태: 지금 입력값과 마지막으로 받은 미리 보기, 결과를 기다리는 마지막 요청.
#[derive(Debug, Default)]
pub struct BatchWindow {
    inputs: Inputs,
    preview: Option<Preview>,
    /// 일꾼 스레드에 맡긴 마지막 미리 보기 요청(`App::request_batch_rename_preview`). 결과가 오면 비운다.
    pending: Option<BatchPreviewId>,
    /// 「문제 행만 보기」 전환. 새 미리 보기가 와도 그대로다.
    problems_only: bool,
}

impl BatchWindow {
    /// 「문제 행만 보기」를 바꾼다(수치 줄·「바꾸기」 켜짐은 전체 미리 보기로 정하므로 그대로).
    pub fn set_problems_only(&mut self, on: bool) {
        self.problems_only = on;
    }

    /// 보일 줄.
    pub fn visible_lines(&self) -> Vec<Line> {
        self.preview
            .as_ref()
            .map(|p| visible_lines(p, self.problems_only))
            .unwrap_or_default()
    }

    /// 문제 행만 보는데 문제 행이 없으면 그 안내, 아니면 비어 있다.
    pub fn empty_text(&self) -> String {
        match &self.preview {
            Some(p) if self.problems_only && p.problem_count() == 0 => {
                text::batch_no_problems_text()
            }
            _ => String::new(),
        }
    }

    /// 입력값과 그 미리 보기를 곧바로 둔다(시험에서 동기 미리 보기를 넣을 때). 화면은 [`BatchWindow::request`]·
    /// [`BatchWindow::receive`]로 일꾼 스레드의 결과를 받는다.
    #[cfg(test)]
    pub fn update(&mut self, inputs: Inputs, preview: Option<Preview>) {
        self.inputs = inputs;
        self.preview = preview;
    }

    /// 입력이 바뀌어 미리 보기를 요청했다. 결과가 오기 전에는 앞 미리 보기를 그대로 둔다.
    pub fn request(&mut self, inputs: Inputs, id: BatchPreviewId) {
        self.inputs = inputs;
        self.pending = Some(id);
    }

    /// 미리 보기 결과를 받는다. 기다리던 마지막 요청의 결과(미리 보기가 있음)면 두고 `true`, 앞선 요청이나 다른 창의
    /// 요청이면 아무것도 바꾸지 않고 `false`다.
    pub fn receive(&mut self, id: BatchPreviewId, preview: Option<&Preview>) -> bool {
        if self.pending != Some(id) {
            return false;
        }
        let Some(preview) = preview else {
            return false;
        };
        self.pending = None;
        self.preview = Some(preview.clone());
        true
    }

    /// 마지막 미리 보기.
    #[cfg(test)]
    pub fn preview(&self) -> Option<&Preview> {
        self.preview.as_ref()
    }

    /// 「바꾸기」를 누를 수 있는가(문제가 없고 바꿀 항목이 있음).
    pub fn can_execute(&self) -> bool {
        self.preview.as_ref().is_some_and(Preview::can_execute)
    }

    /// 아래 줄의 수치 글자.
    pub fn counts_text(&self) -> String {
        match &self.preview {
            Some(preview) => text::batch_counts_text(
                preview.rename_count(),
                preview.unchanged_count(),
                preview.problem_count(),
            ),
            None => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use facet_core::batch_rename::{BatchItem, BatchProblem, ExistingNames, preview};

    use super::*;

    fn items(names: &[&str]) -> Vec<BatchItem> {
        names
            .iter()
            .map(|name| BatchItem {
                folder: PathBuf::from(r"C:\a"),
                name: OsString::from(name),
                is_folder: false,
            })
            .collect()
    }

    #[test]
    fn 번호는_확인란이_켜졌을_때만_규칙에_든다() {
        let mut inputs = Inputs::default();
        assert!(inputs.rule().numbering.is_none());
        inputs.numbering = true;
        inputs.start = "5".into();
        inputs.digits = "3".into();
        inputs.separator = "_".into();
        let numbering = inputs.rule().numbering.unwrap();
        assert_eq!(numbering.start, 5);
        assert_eq!(numbering.digits, 3);
        assert_eq!(numbering.position, NumberPosition::End);
        assert_eq!(numbering.separator, "_");
    }

    #[test]
    fn 수가_아닌_번호_입력은_기본값으로_본다() {
        let inputs = Inputs {
            numbering: true,
            start: "가".into(),
            digits: "-2".into(),
            at_end: false,
            ..Inputs::default()
        };
        let numbering = inputs.rule().numbering.unwrap();
        assert_eq!(numbering.start, 1);
        assert_eq!(numbering.digits, 0);
        assert_eq!(numbering.position, NumberPosition::Start);
    }

    #[test]
    fn 자릿수는_한계로_자른다() {
        let inputs = Inputs {
            numbering: true,
            digits: "99999999".into(),
            ..Inputs::default()
        };
        assert_eq!(inputs.rule().numbering.unwrap().digits, MAX_DIGITS);
    }

    #[test]
    fn 문제_행만_보기는_문제_줄만_거르고_수치는_그대로다() {
        let items = items(&["a.txt", "b.txt"]);
        let rule = RenameRule {
            find: "a".into(),
            replace: ":".into(),
            ..RenameRule::default()
        };
        let preview = preview(&items, &rule, &ExistingNames::new());
        let mut window = BatchWindow::default();
        window.update(Inputs::default(), Some(preview));
        assert_eq!(window.visible_lines().len(), 2);
        let counts = window.counts_text();
        window.set_problems_only(true);
        let shown = window.visible_lines();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].kind, LineKind::Problem);
        assert_eq!(window.counts_text(), counts);
        assert!(window.empty_text().is_empty());
    }

    #[test]
    fn 줄은_바뀔_이름과_문제_까닭을_담는다() {
        let items = items(&["a.txt", "b.txt"]);
        let rule = RenameRule {
            find: "a".into(),
            replace: ":".into(),
            ..RenameRule::default()
        };
        let preview = preview(&items, &rule, &ExistingNames::new());
        let lines = lines(&preview);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].kind, LineKind::Problem);
        assert!(lines[0].problem.contains(':'));
        assert_eq!(lines[1].kind, LineKind::Unchanged);
        assert!(lines[1].problem.is_empty());
    }

    #[test]
    fn 문제가_있거나_바꿀_것이_없으면_바꾸기가_꺼진다() {
        let items = items(&["a.txt", "b.txt"]);
        let existing = ExistingNames::new();
        let mut window = BatchWindow::default();

        let empty = RenameRule::default();
        window.update(Inputs::default(), Some(preview(&items, &empty, &existing)));
        assert!(!window.can_execute());
        assert_eq!(window.counts_text(), "바꿈 0 · 건너뜀 2 · 문제 0");

        let prefix = RenameRule {
            prefix: "x".into(),
            ..RenameRule::default()
        };
        window.update(Inputs::default(), Some(preview(&items, &prefix, &existing)));
        assert!(window.can_execute());
        assert_eq!(window.counts_text(), "바꿈 2 · 건너뜀 0 · 문제 0");

        let same = RenameRule {
            find: "a".into(),
            replace: "b".into(),
            ..RenameRule::default()
        };
        window.update(Inputs::default(), Some(preview(&items, &same, &existing)));
        assert!(!window.can_execute());
        assert!(window.counts_text().ends_with("문제 1"));

        window.update(Inputs::default(), None);
        assert!(!window.can_execute());
    }

    #[test]
    fn 겹침과_남는_이름의_까닭_글자() {
        assert!(text::batch_problem_text(BatchProblem::Duplicate).contains("겹침"));
        assert!(text::batch_problem_text(BatchProblem::NameTaken).contains("같은 이름이 있음"));
    }

    /// 창이 부르는 규칙 목록 입구(`Inputs::rule().to_rules()`)의 미리 보기가 S14a 입구(`Inputs::rule()`)의 미리
    /// 보기와 같다: 항목·바뀔 이름·상태·수치 모두.
    #[test]
    fn 규칙_목록_입구의_미리_보기는_s14a_입구와_같다() {
        use std::time::Duration;

        use facet_core::Location;
        use facet_core::tab::ReadOutcome;
        use facet_core::window::App;

        let dir = tempfile::TempDir::new().unwrap();
        for name in [
            "사진 a.jpg",
            "사진 b.jpg",
            "메모.txt",
            "목록.txt",
            "남는 이름.txt",
        ] {
            std::fs::write(dir.path().join(name), name).unwrap();
        }
        std::fs::create_dir(dir.path().join("폴더")).unwrap();
        let mut app = App::new(Some(Location::file_system(dir.path())));
        let (_, outcome) = app.wait(Duration::from_secs(30)).expect("첫 읽기");
        assert_eq!(outcome, ReadOutcome::Committed);
        let tab = app.active_tab_mut();
        let picked: Vec<usize> = (0..tab.len())
            .filter(|&i| tab.get(i).is_some_and(|item| item.name != "남는 이름.txt"))
            .collect();
        tab.select_only(picked[0]);
        for &i in &picked[1..] {
            tab.toggle_select(i);
        }

        let mut cases = Vec::new();
        cases.push(Inputs::default());
        cases.push(Inputs {
            find: "사진".to_owned(),
            replace: "photo".to_owned(),
            ..Inputs::default()
        });
        cases.push(Inputs {
            prefix: "앞_".to_owned(),
            suffix: "_뒤".to_owned(),
            ..Inputs::default()
        });
        cases.push(Inputs {
            numbering: true,
            start: "7".to_owned(),
            digits: "3".to_owned(),
            at_end: false,
            separator: "-".to_owned(),
            ..Inputs::default()
        });
        cases.push(Inputs {
            find: "목록".to_owned(),
            replace: "남는 이름".to_owned(),
            prefix: "x".to_owned(),
            numbering: true,
            separator: " ".to_owned(),
            ..Inputs::default()
        });
        cases.push(Inputs {
            find: "메모".to_owned(),
            replace: "남는 이름".to_owned(),
            ..Inputs::default()
        });
        cases.push(Inputs {
            find: ".".to_owned(),
            replace: "*".to_owned(),
            ..Inputs::default()
        });
        for inputs in cases {
            let rule = inputs.rule();
            let s14a = app.batch_rename_preview(&rule).expect("S14a 미리 보기");
            for offset in [0, 9 * 3600, -10 * 3600] {
                let rules = app
                    .batch_rename_preview_rules(&rule.to_rules(), offset)
                    .expect("규칙 목록 미리 보기");
                assert_eq!(rules.entries(), s14a.entries(), "{inputs:?}");
                assert_eq!(rules.rename_count(), s14a.rename_count(), "{inputs:?}");
                assert_eq!(
                    rules.unchanged_count(),
                    s14a.unchanged_count(),
                    "{inputs:?}"
                );
                assert_eq!(rules.problem_count(), s14a.problem_count(), "{inputs:?}");
                let mut window = BatchWindow::default();
                window.update(inputs.clone(), Some(rules.clone()));
                let mut old = BatchWindow::default();
                old.update(inputs.clone(), Some(s14a.clone()));
                assert_eq!(window.can_execute(), old.can_execute());
                assert_eq!(window.counts_text(), old.counts_text());
                assert_eq!(lines(&rules), lines(&s14a));
            }
        }
    }
    #[test]
    fn 마지막_요청의_결과만_받고_그_전에는_앞_미리_보기를_둔다() {
        use std::time::Duration;

        use facet_core::Location;
        use facet_core::tab::ReadOutcome;
        use facet_core::window::{App, Update};

        let dir = tempfile::TempDir::new().unwrap();
        for name in ["a.txt", "b.txt"] {
            std::fs::write(dir.path().join(name), name).unwrap();
        }
        let mut app = App::new(Some(Location::file_system(dir.path())));
        let (_, outcome) = app.wait(Duration::from_secs(30)).expect("첫 읽기");
        assert_eq!(outcome, ReadOutcome::Committed);
        app.active_tab_mut().select_all();

        let empty = RenameRule::default().to_rules();
        let prefix = RenameRule {
            prefix: "x".into(),
            ..RenameRule::default()
        }
        .to_rules();
        let first_preview = app.batch_rename_preview_rules(&empty, 0).unwrap();
        let second_preview = app.batch_rename_preview_rules(&prefix, 0).unwrap();

        let mut window = BatchWindow::default();
        window.update(Inputs::default(), Some(first_preview.clone()));
        let first = app.request_batch_rename_preview(&empty, 0).unwrap();
        let second = app.request_batch_rename_preview(&prefix, 0).unwrap();
        window.request(Inputs::default(), second);
        assert_eq!(
            window.preview(),
            Some(&first_preview),
            "결과 전에는 앞 미리 보기"
        );

        // 앞선 요청의 결과는 미리 보기가 있어도 받지 않는다.
        assert!(!window.receive(first, Some(&second_preview)));
        assert!(!window.receive(second, None));
        assert_eq!(window.preview(), Some(&first_preview));

        let mut got = None;
        for _ in 0..100 {
            if let Some((_, Update::BatchPreview { id, preview })) =
                app.wait_update(Duration::from_millis(300))
                && id == second
            {
                got = Some(preview);
                break;
            }
        }
        let preview = got.expect("마지막 요청의 결과");
        assert!(window.receive(second, preview.as_ref()));
        assert_eq!(window.preview(), Some(&second_preview));
        assert!(window.can_execute());
        assert!(!window.receive(second, preview.as_ref()), "한 번만 받는다");
    }
}
