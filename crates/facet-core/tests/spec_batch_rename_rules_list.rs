// target: crates/facet-core/tests/spec_batch_rename_rules_list.rs
// AC: #272: 규칙 목록 차례·꺼진 규칙, 적용 대상, 찾아 바꾸기(대소문자·첫 번째만·글자 그대로·정규식 $1/${name}), 틀린 정규식의 규칙 문제, 넣기, 번호, RenameRule::to_rules와 S14a 같음, 겹침 검사, 1만 항목
//! 일괄 이름 바꾸기의 규칙 목록 미리 보기(`batch_rename::preview_rules`, 순수 함수)를 확인한다: 켜진 규칙을 차례로 적용하고
//! 꺼진 규칙은 건너뜀, 적용 대상(이름 부분·확장자·전체), 찾아 바꾸기(대소문자 구분·첫 번째만/모두·글자 그대로·정규식 묶음
//! 치환), 틀린 정규식이 그 규칙만 건너뛰고 규칙 문제로 옴, 넣기와 번호의 자리, S14a 규칙(`RenameRule`)을 규칙 목록으로 바꾼
//! 미리 보기가 S14a 미리 보기와 같음, 규칙 목록의 결과에도 걸리는 S14a 검사. 파일 시스템을 쓰지 않아 모든 환경에서 돈다.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use facet_core::batch_rename::{
    BatchItem, BatchProblem, Edge, EntryStatus, ExistingNames, NumberPosition, Numbering, Place,
    Preview, RenameRule, Rule, RuleContext, RuleError, RuleKind, RuleProblem, Target, preview,
    preview_rules,
};
use facet_core::name_input::NameError;

fn file(name: &str) -> BatchItem {
    BatchItem {
        folder: PathBuf::from("F"),
        name: OsString::from(name),
        is_folder: false,
    }
}

fn dir(name: &str) -> BatchItem {
    BatchItem {
        is_folder: true,
        ..file(name)
    }
}

fn apply(items: &[BatchItem], rules: &[Rule]) -> Preview {
    preview_rules(items, rules, &RuleContext::default(), &ExistingNames::new())
}

fn names(preview: &Preview) -> Vec<&str> {
    preview
        .entries()
        .iter()
        .map(|entry| entry.new_name.as_str())
        .collect()
}

fn statuses(preview: &Preview) -> Vec<EntryStatus> {
    preview.entries().iter().map(|entry| entry.status).collect()
}

/// 항목 하나에 규칙 목록을 쓴 새 이름.
fn renamed(item: BatchItem, rules: &[Rule]) -> String {
    apply(&[item], rules).entries()[0].new_name.clone()
}

fn replace(find: &str, with: &str) -> RuleKind {
    RuleKind::Replace {
        find: find.to_string(),
        replace: with.to_string(),
        regex: false,
        case_sensitive: false,
        all: true,
    }
}

/// 찾아 바꾸기의 모든 선택지를 받는다.
fn replace_with(find: &str, with: &str, regex: bool, case_sensitive: bool, all: bool) -> Rule {
    Rule::new(RuleKind::Replace {
        find: find.to_string(),
        replace: with.to_string(),
        regex,
        case_sensitive,
        all,
    })
}

fn regex(find: &str, with: &str) -> Rule {
    replace_with(find, with, true, false, true)
}

fn insert(text: &str, from: Edge, at: usize) -> Rule {
    Rule::new(RuleKind::Insert {
        text: text.to_string(),
        from,
        at,
    })
}

fn number(start: u64, step: u64, digits: usize, place: Place, separator: &str) -> Rule {
    Rule::new(RuleKind::Number {
        start,
        step,
        digits,
        place,
        separator: separator.to_string(),
    })
}

fn on(kind: RuleKind, target: Target) -> Rule {
    Rule {
        target,
        ..Rule::new(kind)
    }
}

fn off(rule: Rule) -> Rule {
    Rule {
        enabled: false,
        ..rule
    }
}

#[test]
fn a_new_rule_is_enabled_and_targets_the_stem() {
    let rule = Rule::new(replace("a", "b"));
    assert!(rule.enabled);
    assert_eq!(rule.target, Target::Stem);
    assert_eq!(Target::default(), Target::Stem);
}

#[test]
fn enabled_rules_apply_in_order_and_disabled_rules_are_skipped() {
    let a_to_b = Rule::new(replace("a", "b"));
    let b_to_c = Rule::new(replace("b", "c"));

    assert_eq!(
        renamed(file("ab.txt"), &[a_to_b.clone(), b_to_c.clone()]),
        "cc.txt",
        "앞 규칙의 결과가 다음 규칙의 입력이다"
    );
    assert_eq!(
        renamed(file("ab.txt"), &[b_to_c.clone(), a_to_b.clone()]),
        "bc.txt",
        "차례를 바꾸면 결과가 바뀐다"
    );
    assert_eq!(
        renamed(file("ab.txt"), &[a_to_b.clone(), off(b_to_c.clone())]),
        "bb.txt"
    );
    assert_eq!(
        renamed(file("ab.txt"), &[off(a_to_b), off(b_to_c)]),
        "ab.txt"
    );

    let result = apply(&[file("a.txt"), dir("b")], &[]);
    assert_eq!(names(&result), ["a.txt", "b"]);
    assert_eq!(result.unchanged_count(), 2);
    assert!(!result.can_execute(), "규칙이 없으면 바꿀 것이 없다");
}

#[test]
fn the_target_picks_the_stem_the_extension_or_the_whole_name() {
    let o_to_0 = replace("o", "0");
    let three = |item: fn(&str) -> BatchItem, name: &str| {
        [Target::Stem, Target::Extension, Target::Whole]
            .map(|target| renamed(item(name), &[on(o_to_0.clone(), target)]))
    };
    assert_eq!(
        three(file, "photo.ogg"),
        ["ph0t0.ogg", "photo.0gg", "ph0t0.0gg"]
    );
    assert_eq!(
        three(dir, "foo.org"),
        ["f00.0rg", "foo.org", "f00.0rg"],
        "폴더는 확장자가 없고 이름 부분이 이름 전체다"
    );
    assert_eq!(
        three(file, "boot"),
        ["b00t", "boot", "b00t"],
        "확장자 없는 파일에서 확장자 대상 규칙은 아무것도 하지 않는다"
    );
    assert_eq!(
        three(file, ".gitignore"),
        [".gitign0re", ".gitignore", ".gitign0re"],
        "맨 앞 점만 있는 이름은 확장자가 없다"
    );

    // 확장자 대상은 마지막 점 뒤 글자에만 쓰고 점은 그대로 둔다.
    let at_start = insert("x", Edge::Start, 0);
    assert_eq!(
        renamed(
            file("a.tar.gz"),
            &[Rule {
                target: Target::Extension,
                ..at_start.clone()
            }]
        ),
        "a.tar.xgz"
    );
    assert_eq!(
        renamed(
            file("a.tar.gz"),
            &[Rule {
                target: Target::Whole,
                ..at_start.clone()
            }]
        ),
        "xa.tar.gz"
    );
    assert_eq!(renamed(file("a.tar.gz"), &[at_start]), "xa.tar.gz");
    assert_eq!(
        renamed(
            file("a.tar.gz"),
            &[Rule {
                target: Target::Stem,
                ..insert("x", Edge::End, 0)
            }]
        ),
        "a.tarx.gz"
    );
}

#[test]
fn replace_can_match_case_and_stop_after_the_first() {
    let cases = [
        // (대소문자 구분, 모두, 기대)
        (false, true, "bbb.txt"),
        (false, false, "baA.txt"),
        (true, true, "AbA.txt"),
        (true, false, "AbA.txt"),
    ];
    for (case_sensitive, all, expected) in cases {
        let rule = replace_with("a", "b", false, case_sensitive, all);
        assert_eq!(
            renamed(file("AaA.txt"), &[rule]),
            expected,
            "대소문자 구분 {case_sensitive}, 모두 {all}"
        );
    }
    let first = replace_with("a", "b", false, true, false);
    assert_eq!(renamed(file("aAa.txt"), &[first]), "bAa.txt");

    let empty = Rule::new(replace("", "x"));
    let result = apply(&[file("a.txt")], &[empty]);
    assert_eq!(
        names(&result),
        ["a.txt"],
        "빈 찾을 글자는 아무것도 하지 않는다"
    );
    assert_eq!(statuses(&result), [EntryStatus::Unchanged]);
}

#[test]
fn plain_replace_takes_regex_symbols_and_dollars_literally() {
    assert_eq!(
        renamed(file("a(1).txt"), &[Rule::new(replace("(1)", ""))]),
        "a.txt"
    );
    assert_eq!(
        renamed(file("x.y.z.txt"), &[Rule::new(replace(".", "_"))]),
        "x_y_z.txt",
        "점은 아무 글자가 아니라 점이다"
    );
    assert_eq!(
        renamed(file("a+b.txt"), &[Rule::new(replace("A+B", "c"))]),
        "c.txt"
    );
    assert_eq!(
        renamed(file("a.txt"), &[Rule::new(replace("a", "$1"))]),
        "$1.txt",
        "글자 모드의 바꿀 글자는 치환하지 않는다"
    );
    assert_eq!(
        renamed(file("사진_사진.jpg"), &[Rule::new(replace("사진", "휴가"))]),
        "휴가_휴가.jpg"
    );
}

#[test]
fn regex_replace_uses_numbered_and_named_groups() {
    assert_eq!(
        renamed(file("123-abc.txt"), &[regex(r"(\d+)-(\w+)", "$2-$1")]),
        "abc-123.txt"
    );
    assert_eq!(
        renamed(
            file("20261115 사진.jpg"),
            &[regex(
                r"(?P<y>\d{4})(?P<m>\d{2})(?P<d>\d{2})",
                "${y}-${m}-${d}"
            )]
        ),
        "2026-11-15 사진.jpg"
    );
    assert_eq!(
        renamed(file("IMG_1.jpg"), &[regex("img", "pic")]),
        "pic_1.jpg",
        "대소문자 무시가 기본"
    );
    let result = apply(
        &[file("IMG_1.jpg")],
        &[replace_with("img", "pic", true, true, true)],
    );
    assert_eq!(names(&result), ["IMG_1.jpg"]);
    assert_eq!(statuses(&result), [EntryStatus::Unchanged]);

    assert_eq!(
        renamed(
            file("a1b2.txt"),
            &[replace_with(r"\d", "#", true, false, false)]
        ),
        "a#b2.txt"
    );
    assert_eq!(
        renamed(
            file("a1b2.txt"),
            &[replace_with(r"\d", "#", true, false, true)]
        ),
        "a#b#.txt"
    );
    assert_eq!(
        renamed(file("x.y.txt"), &[regex(".", "_")]),
        "___.txt",
        "정규식의 점은 아무 글자다(이름 부분에만)"
    );
}

#[test]
fn a_broken_regex_is_reported_and_only_that_rule_is_skipped() {
    let items = [file("a.txt"), file("b.txt")];
    let result = apply(&items, &[regex("(", "x"), insert("x_", Edge::Start, 0)]);
    assert_eq!(names(&result), ["x_a.txt", "x_b.txt"]);
    assert_eq!(result.rename_count(), 2);
    match result.rule_problems() {
        [
            RuleProblem {
                rule: 0,
                error: RuleError::Regex(reason),
            },
        ] => assert!(!reason.is_empty(), "regex 크레이트의 설명을 담는다"),
        other => panic!("첫 규칙의 정규식 문제 하나여야 한다: {other:?}"),
    }

    let result = apply(
        &items,
        &[
            insert("x_", Edge::Start, 0),
            regex("[a-", "y"),
            Rule::new(replace("x", "z")),
        ],
    );
    assert_eq!(
        names(&result),
        ["z_a.txt", "z_b.txt"],
        "틀린 규칙 뒤의 규칙도 그대로 적용한다"
    );
    let rules: Vec<usize> = result
        .rule_problems()
        .iter()
        .map(|problem| problem.rule)
        .collect();
    assert_eq!(rules, [1], "규칙 번호는 목록의 0부터 센 자리");

    let fine = apply(&items, &[regex(r"(\w)", "$1$1")]);
    assert!(fine.rule_problems().is_empty());
    assert_eq!(names(&fine), ["aa.txt", "bb.txt"]);
}

#[test]
fn insert_counts_chars_from_either_end_and_clamps() {
    let cases = [
        (Edge::Start, 0, "-abcd.txt"),
        (Edge::Start, 2, "ab-cd.txt"),
        (Edge::End, 0, "abcd-.txt"),
        (Edge::End, 1, "abc-d.txt"),
        (Edge::Start, 10, "abcd-.txt"),
        (Edge::End, 10, "-abcd.txt"),
    ];
    for (from, at, expected) in cases {
        assert_eq!(
            renamed(file("abcd.txt"), &[insert("-", from, at)]),
            expected,
            "{from:?} {at}"
        );
    }
    assert_eq!(
        renamed(file("한글😀이름.txt"), &[insert("_", Edge::Start, 3)]),
        "한글😀_이름.txt",
        "한글·이모지 한 글자가 한 자리"
    );
    assert_eq!(
        renamed(file("한글😀이름.txt"), &[insert("_", Edge::End, 3)]),
        "한글_😀이름.txt"
    );
}

#[test]
fn numbers_follow_the_item_order_with_step_padding_place_and_separator() {
    let items = [file("a.jpg"), file("b.jpg"), dir("c")];
    let result = apply(&items, &[number(1, 2, 3, Place::End, "_")]);
    assert_eq!(names(&result), ["a_001.jpg", "b_003.jpg", "c_005"]);

    let result = apply(&items, &[number(0, 10, 2, Place::Start, "-")]);
    assert_eq!(names(&result), ["00-a.jpg", "10-b.jpg", "20-c"]);

    let result = apply(&items[..2], &[number(99, 1, 2, Place::End, "")]);
    assert_eq!(
        names(&result),
        ["a99.jpg", "b100.jpg"],
        "자릿수를 넘으면 그대로 길어진다"
    );

    let result = apply(&items[..2], &[number(7, 1, 0, Place::Start, " ")]);
    assert_eq!(names(&result), ["7 a.jpg", "8 b.jpg"]);

    let at = Place::At {
        from: Edge::Start,
        at: 1,
    };
    let result = apply(
        &[file("abc.txt"), file("xyz.txt")],
        &[number(1, 1, 2, at, "#")],
    );
    assert_eq!(
        names(&result),
        ["a01#bc.txt", "x02#yz.txt"],
        "N번째 자리면 번호 뒤에 구분 글자"
    );
    let at_end = Place::At {
        from: Edge::End,
        at: 1,
    };
    let result = apply(&[file("사진가.txt")], &[number(5, 1, 1, at_end, "_")]);
    assert_eq!(names(&result), ["사진5_가.txt"]);
}

/// S14a 시험이 쓴 것과 같은 꼴의 규칙들.
fn s14a_rules() -> Vec<RenameRule> {
    let numbering = |start, digits, position, separator: &str| Numbering {
        start,
        digits,
        position,
        separator: separator.to_string(),
    };
    vec![
        RenameRule::default(),
        RenameRule {
            find: "img".to_string(),
            replace: "사진".to_string(),
            ..RenameRule::default()
        },
        RenameRule {
            find: " (1)".to_string(),
            ..RenameRule::default()
        },
        RenameRule {
            find: "a".to_string(),
            replace: "$1".to_string(),
            ..RenameRule::default()
        },
        RenameRule {
            find: ".".to_string(),
            replace: "_".to_string(),
            ..RenameRule::default()
        },
        RenameRule {
            prefix: "x_".to_string(),
            suffix: "_y".to_string(),
            ..RenameRule::default()
        },
        RenameRule {
            numbering: Some(numbering(1, 3, NumberPosition::End, "_")),
            ..RenameRule::default()
        },
        RenameRule {
            prefix: "휴가 ".to_string(),
            numbering: Some(numbering(9, 2, NumberPosition::Start, "-")),
            ..RenameRule::default()
        },
        RenameRule {
            find: "A".to_string(),
            replace: "a".to_string(),
            prefix: "a".to_string(),
            suffix: "!".to_string(),
            numbering: Some(numbering(99, 2, NumberPosition::End, "")),
        },
        RenameRule {
            find: "readme".to_string(),
            replace: "README".to_string(),
            ..RenameRule::default()
        },
        RenameRule {
            prefix: "a:".to_string(),
            ..RenameRule::default()
        },
    ]
}

#[test]
fn s14a_rules_turned_into_a_rule_list_give_the_same_preview() {
    let items = [
        file("IMG_img.img"),
        file("보고서 (1).docx"),
        file("보고서.docx"),
        file("a.tar.gz"),
        file("README"),
        file("readme.md"),
        file(".gitignore"),
        dir("my.dir"),
        file("Aa.txt"),
        file("x.y.txt"),
    ];
    let mut existing = ExistingNames::new();
    for item in &items {
        existing.insert(&item.folder, &item.name);
    }
    existing.insert(Path::new("F"), OsStr::new("x_README_y"));

    for rule in s14a_rules() {
        let rules = rule.to_rules();
        for existing in [&existing, &ExistingNames::new()] {
            let old = preview(&items, &rule, existing);
            let new = preview_rules(&items, &rules, &RuleContext::default(), existing);
            assert_eq!(new.entries(), old.entries(), "{rule:?}");
            assert_eq!(
                (
                    new.rename_count(),
                    new.unchanged_count(),
                    new.problem_count(),
                    new.can_execute()
                ),
                (
                    old.rename_count(),
                    old.unchanged_count(),
                    old.problem_count(),
                    old.can_execute()
                ),
                "{rule:?}"
            );
            assert!(new.rule_problems().is_empty());
        }
    }
}

#[test]
fn the_s14a_checks_hold_for_rule_list_results() {
    // 같은 폴더의 같은 새 이름(대소문자 무시)은 겹친다.
    let result = apply(&[file("a1.txt"), file("A2.TXT")], &[regex(r"\d+", "")]);
    assert_eq!(names(&result), ["a.txt", "A.TXT"]);
    assert_eq!(
        statuses(&result),
        [
            EntryStatus::Problem(BatchProblem::Duplicate),
            EntryStatus::Problem(BatchProblem::Duplicate)
        ]
    );
    assert!(!result.can_execute());

    // 남는 이름.
    let mut existing = ExistingNames::new();
    existing.insert(Path::new("F"), OsStr::new("b.txt"));
    let result = preview_rules(
        &[file("a.txt")],
        &[Rule::new(replace("a", "B"))],
        &RuleContext::default(),
        &existing,
    );
    assert_eq!(
        statuses(&result),
        [EntryStatus::Problem(BatchProblem::NameTaken)]
    );

    // 맞바꾸기는 겹침이 아니다.
    let swap = [
        replace_with("a", "tmp", false, true, true),
        replace_with("b", "a", false, true, true),
        replace_with("tmp", "b", false, true, true),
    ];
    let result = apply(&[file("a.txt"), file("b.txt")], &swap);
    assert_eq!(names(&result), ["b.txt", "a.txt"]);
    assert_eq!(
        statuses(&result),
        [EntryStatus::Rename, EntryStatus::Rename]
    );
    assert!(result.can_execute());

    // 대소문자만 바꾸기는 같은 항목이다.
    let mut existing = ExistingNames::new();
    existing.insert(Path::new("F"), OsStr::new("readme.md"));
    let result = preview_rules(
        &[file("readme.md")],
        &[replace_with("readme", "README", false, true, true)],
        &RuleContext::default(),
        &existing,
    );
    assert_eq!(statuses(&result), [EntryStatus::Rename]);

    // 쓸 수 없는 이름.
    let result = apply(&[file("f.txt")], &[insert(":", Edge::Start, 0)]);
    assert_eq!(
        statuses(&result),
        [EntryStatus::Problem(BatchProblem::Invalid(
            NameError::ForbiddenChar(':')
        ))]
    );
}

#[test]
fn ten_thousand_items_preview_a_rule_list_quickly() {
    let items: Vec<BatchItem> = (0..10_000)
        .map(|n| file(&format!("사진 {n}.jpg")))
        .collect();
    let mut existing = ExistingNames::new();
    for item in &items {
        existing.insert(&item.folder, &item.name);
    }
    let rules = [
        regex(r"사진 (\d+)", "휴가 $1"),
        insert("2026 ", Edge::Start, 0),
        number(1, 1, 5, Place::End, "_"),
    ];
    let start = Instant::now();
    let result = preview_rules(&items, &rules, &RuleContext::default(), &existing);
    // 넉넉한 상한(디버그 빌드에서도). 속도 예산은 벤치마크가 지킨다.
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(result.rename_count(), 10_000);
    assert_eq!(result.entries()[0].new_name, "2026 휴가 0_00001.jpg");
    assert!(result.can_execute());
}
