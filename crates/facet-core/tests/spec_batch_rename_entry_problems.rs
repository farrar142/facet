// target: crates/facet-core/tests/spec_batch_rename_entry_problems.rs
// AC: #294: 문제 행 자리는 문제 항목 번호들이고 수는 problem_count와 같음·문제가 없으면 빔, 규칙 목록 미리 보기에서도 같고 규칙 문제는 들지 않음
//! 일괄 이름 바꾸기 창의 「문제 행만 보기」가 쓰는 문제 행 자리(`Preview::problem_rows`)를 순수 미리 보기 함수
//! (`preview`·`preview_rules`)로 확인한다: 문제 항목만 차례대로, 문제가 없으면 빔, 틀린 정규식 같은 규칙 문제는 행이
//! 아님. 파일 시스템을 쓰지 않으므로 모든 환경에서 돈다.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use facet_core::batch_rename::{
    BatchItem, EntryStatus, ExistingNames, RenameRule, Rule, RuleContext, RuleKind, preview,
    preview_rules,
};

const NAMES: [&str; 5] = ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"];

fn folder() -> PathBuf {
    PathBuf::from(r"C:\시험\사진")
}

fn items() -> Vec<BatchItem> {
    NAMES
        .iter()
        .map(|name| BatchItem {
            folder: folder(),
            name: OsString::from(name),
            is_folder: false,
        })
        .collect()
}

/// 폴더의 모든 이름: 바꿀 항목들과 고르지 않은 `x.txt`.
fn existing() -> ExistingNames {
    let mut names = ExistingNames::new();
    let folder = folder();
    for name in NAMES.iter().chain(&["x.txt"]) {
        names.insert(&folder, OsStr::new(name));
    }
    names
}

fn replace(find: &str, with: &str, regex: bool) -> Rule {
    Rule::new(RuleKind::Replace {
        find: find.to_string(),
        replace: with.to_string(),
        regex,
        case_sensitive: false,
        all: true,
    })
}

fn problem_positions(statuses: &[EntryStatus]) -> Vec<usize> {
    statuses
        .iter()
        .enumerate()
        .filter(|(_, status)| matches!(status, EntryStatus::Problem(_)))
        .map(|(i, _)| i)
        .collect()
}

#[test]
fn problem_rows_are_the_problem_entries_in_order() {
    // a → x는 남는 x.txt와, c와 e → q는 서로 겹친다. b·d는 그대로다.
    let rules = [
        replace("a", "x", false),
        replace("c", "q", false),
        replace("e", "q", false),
    ];
    let preview = preview_rules(&items(), &rules, &RuleContext::default(), &existing());
    let statuses: Vec<EntryStatus> = preview.entries().iter().map(|e| e.status).collect();
    assert_eq!(problem_positions(&statuses), [0, 2, 4], "기대한 문제 행");

    let rows = preview.problem_rows();
    assert_eq!(rows, [0, 2, 4]);
    assert_eq!(rows.len(), preview.problem_count());
    assert!(!preview.can_execute());
}

#[test]
fn problem_rows_are_empty_without_problems() {
    let rule = RenameRule {
        prefix: "휴가-".to_string(),
        ..RenameRule::default()
    };
    let preview = preview(&items(), &rule, &existing());
    assert_eq!(preview.problem_count(), 0);
    assert!(preview.problem_rows().is_empty());
    assert!(preview.can_execute());
}

#[test]
fn a_broken_rule_is_not_a_problem_row() {
    // 틀린 정규식 규칙은 건너뛰고(규칙 문제), 나머지 규칙은 쓴다: b → b:는 쓸 수 없는 이름이다.
    let rules = [replace("(", "z", true), replace("b", "b:", false)];
    let preview = preview_rules(&items(), &rules, &RuleContext::default(), &existing());
    assert_eq!(preview.rule_problems().len(), 1, "틀린 정규식 하나");
    assert_eq!(preview.problem_rows(), [1], "행 문제는 b.txt 하나뿐");
    assert_eq!(preview.problem_count(), 1);

    // 행 문제가 없으면 규칙 문제가 있어도 비어 있다.
    let rules = [replace("(", "z", true)];
    let preview = preview_rules(&items(), &rules, &RuleContext::default(), &existing());
    assert_eq!(preview.rule_problems().len(), 1);
    assert!(preview.problem_rows().is_empty());
}
