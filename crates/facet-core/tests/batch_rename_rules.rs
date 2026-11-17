//! 규칙 목록 미리 보기(`batch_rename::preview_rules`)의 인수 시험 밖 동작: 이름 부분·확장자 경계 유지,
//! 아직 없는 규칙 종류, 틀린 정규식 여럿, 대소문자 구분 글자 바꾸기의 첫 번째만.

use std::ffi::OsString;
use std::path::PathBuf;

use facet_core::batch_rename::{
    BatchItem, CaseMode, Edge, ExistingNames, Place, Preview, Rule, RuleContext, RuleKind, Target,
    preview_rules,
};

fn file(name: &str) -> BatchItem {
    BatchItem {
        folder: PathBuf::from("F"),
        name: OsString::from(name),
        is_folder: false,
    }
}

fn apply(items: &[BatchItem], rules: &[Rule]) -> Preview {
    preview_rules(items, rules, &RuleContext::default(), &ExistingNames::new())
}

fn renamed(name: &str, rules: &[Rule]) -> String {
    apply(&[file(name)], rules).entries()[0].new_name.clone()
}

fn insert(text: &str, from: Edge, at: usize, target: Target) -> Rule {
    Rule {
        target,
        ..Rule::new(RuleKind::Insert {
            text: text.to_string(),
            from,
            at,
        })
    }
}

fn regex(find: &str) -> Rule {
    Rule::new(RuleKind::Replace {
        find: find.to_string(),
        replace: String::new(),
        regex: true,
        case_sensitive: false,
        all: true,
    })
}

#[test]
fn a_dot_put_into_the_stem_does_not_move_the_extension() {
    // 이름 부분에 점을 넣어도 확장자는 처음 이름의 것 그대로다.
    let rules = [
        insert(".", Edge::Start, 1, Target::Stem),
        insert("x", Edge::End, 0, Target::Extension),
    ];
    assert_eq!(renamed("ab.txt", &rules), "a.b.txtx");
}

#[test]
fn a_whole_rule_splits_its_result_again() {
    let rules = [
        insert(".md", Edge::End, 0, Target::Whole),
        insert("x", Edge::Start, 0, Target::Extension),
    ];
    assert_eq!(renamed("readme", &rules), "readme.xmd");
}

#[test]
fn emptying_the_stem_keeps_the_extension() {
    let rules = [
        Rule::new(RuleKind::Replace {
            find: "a".to_string(),
            replace: String::new(),
            regex: false,
            case_sensitive: false,
            all: true,
        }),
        insert("x", Edge::Start, 0, Target::Stem),
    ];
    assert_eq!(renamed("a.txt", &rules), "x.txt");
}

#[test]
fn kinds_not_built_yet_do_nothing() {
    let rules = [
        Rule::new(RuleKind::Delete {
            from: Edge::Start,
            at: 0,
            count: 1,
        }),
        Rule::new(RuleKind::Case(CaseMode::Upper)),
        Rule::new(RuleKind::Extension {
            new: Some("md".to_string()),
        }),
        Rule::new(RuleKind::Date {
            format: "YYYY".to_string(),
            place: Place::Start,
            separator: String::new(),
        }),
    ];
    let result = apply(&[file("a.txt")], &rules);
    assert_eq!(result.entries()[0].new_name, "a.txt");
    assert!(result.rule_problems().is_empty());
}

#[test]
fn each_broken_regex_is_reported_once_in_list_order() {
    let result = apply(
        &[file("a.txt"), file("b.txt"), file("c.txt")],
        &[regex("("), regex("b"), regex("[")],
    );
    let rules: Vec<usize> = result.rule_problems().iter().map(|p| p.rule).collect();
    assert_eq!(rules, [0, 2]);
    let disabled = Rule {
        enabled: false,
        ..regex("(")
    };
    assert!(
        apply(&[file("a.txt")], &[disabled])
            .rule_problems()
            .is_empty(),
        "꺼진 규칙은 확인하지 않는다"
    );
}
