// target: crates/facet-core/tests/spec_batch_rename_rules_edit.rs
// AC: #273: 지우기(앞·뒤 기준, 글자 단위, 범위 넘음), 대소문자(네 가지, 유니코드, 한글), 적용 대상에 따른 대소문자, 확장자 바꾸기(바꾸기·지우기·확장자 없음·.gitignore·폴더·대상 무시), S14a 검사
//! 일괄 이름 바꾸기 규칙 목록의 지우기·대소문자·확장자 바꾸기 규칙(`batch_rename::preview_rules`, 순수 함수)을 확인한다:
//! 앞·뒤에서 센 자리부터 안쪽으로 지우기와 범위를 넘는 부분, 네 가지 대소문자와 한글이 섞인 이름, 같은 규칙이 이름 부분·
//! 확장자·전체에서 다른 결과, 확장자 바꾸기·지우기·붙이기와 폴더 건너뛰기, 그 결과에도 걸리는 겹침 검사. 파일 시스템을
//! 쓰지 않아 모든 환경에서 돈다.

use std::ffi::OsString;
use std::path::PathBuf;

use facet_core::batch_rename::{
    BatchItem, BatchProblem, CaseMode, Edge, EntryStatus, ExistingNames, Preview, Rule,
    RuleContext, RuleKind, Target, preview_rules,
};

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

fn renamed(item: BatchItem, rule: Rule) -> String {
    apply(&[item], &[rule]).entries()[0].new_name.clone()
}

fn delete(from: Edge, at: usize, count: usize) -> Rule {
    Rule::new(RuleKind::Delete { from, at, count })
}

fn case(mode: CaseMode) -> Rule {
    Rule::new(RuleKind::Case(mode))
}

fn targeted(rule: Rule, target: Target) -> Rule {
    Rule { target, ..rule }
}

fn extension(new: Option<&str>) -> Rule {
    Rule::new(RuleKind::Extension {
        new: new.map(str::to_string),
    })
}

#[test]
fn delete_removes_chars_inward_from_either_end() {
    let cases = [
        (Edge::Start, 0, 2, "cdef.txt"),
        (Edge::Start, 1, 2, "adef.txt"),
        (Edge::End, 0, 2, "abcd.txt"),
        (Edge::End, 1, 2, "abcf.txt"),
        (Edge::Start, 4, 10, "abcd.txt"),
        (Edge::End, 4, 10, "cdef.txt"),
    ];
    for (from, at, count, expected) in cases {
        assert_eq!(
            renamed(file("abcdef.txt"), delete(from, at, count)),
            expected,
            "{from:?} {at} {count}"
        );
    }
}

#[test]
fn delete_out_of_range_changes_nothing() {
    for rule in [
        delete(Edge::Start, 6, 3),
        delete(Edge::Start, 10, 3),
        delete(Edge::End, 6, 3),
        delete(Edge::End, 10, 3),
        delete(Edge::Start, 0, 0),
    ] {
        let result = apply(&[file("abcdef.txt")], std::slice::from_ref(&rule));
        assert_eq!(result.entries()[0].new_name, "abcdef.txt", "{rule:?}");
        assert_eq!(result.entries()[0].status, EntryStatus::Unchanged);
    }
}

#[test]
fn delete_counts_korean_and_emoji_as_one_char_each() {
    assert_eq!(
        renamed(file("😀사진_2026.jpg"), delete(Edge::Start, 0, 1)),
        "사진_2026.jpg"
    );
    assert_eq!(
        renamed(file("😀사진_2026.jpg"), delete(Edge::End, 0, 5)),
        "😀사진.jpg"
    );
    assert_eq!(
        renamed(file("😀사진_2026.jpg"), delete(Edge::Start, 1, 2)),
        "😀_2026.jpg"
    );
    assert_eq!(
        renamed(dir("접두_폴더"), delete(Edge::Start, 0, 3)),
        "폴더",
        "폴더는 이름 전체가 이름 부분이다"
    );
}

#[test]
fn case_rules_lower_upper_title_and_sentence() {
    assert_eq!(
        renamed(file("HeLLo WORLD.TXT"), case(CaseMode::Lower)),
        "hello world.TXT"
    );
    assert_eq!(
        renamed(file("HeLLo world.txt"), case(CaseMode::Upper)),
        "HELLO WORLD.txt"
    );
    assert_eq!(
        renamed(
            file("hello wORLD_foo-bar.baz qux.txt"),
            case(CaseMode::Title)
        ),
        "Hello World_Foo-Bar.Baz Qux.txt",
        "공백·_·-·. 뒤 첫 글자를 대문자로, 나머지는 소문자로"
    );
    assert_eq!(
        renamed(file("hELLO wORLD-X.txt"), case(CaseMode::Sentence)),
        "Hello world-x.txt"
    );
}

#[test]
fn case_rules_use_unicode_and_leave_korean_as_is() {
    assert_eq!(
        renamed(file("ÉCOLE Ñandú.txt"), case(CaseMode::Lower)),
        "école ñandú.txt"
    );
    assert_eq!(
        renamed(file("école ñandú.txt"), case(CaseMode::Upper)),
        "ÉCOLE ÑANDÚ.txt"
    );
    assert_eq!(
        renamed(file("사진 abc_DEF.jpg"), case(CaseMode::Title)),
        "사진 Abc_Def.jpg"
    );
    assert_eq!(
        renamed(file("사진 abc_def.jpg"), case(CaseMode::Upper)),
        "사진 ABC_DEF.jpg"
    );
    assert_eq!(
        renamed(file("사진 ABC.jpg"), case(CaseMode::Sentence)),
        "사진 abc.jpg"
    );
    let result = apply(&[file("사진.jpg")], &[case(CaseMode::Upper)]);
    assert_eq!(result.entries()[0].status, EntryStatus::Unchanged);
}

#[test]
fn the_same_case_rule_differs_by_target() {
    let upper = |target| targeted(case(CaseMode::Upper), target);
    assert_eq!(renamed(file("photo.jpg"), upper(Target::Stem)), "PHOTO.jpg");
    assert_eq!(
        renamed(file("photo.jpg"), upper(Target::Extension)),
        "photo.JPG"
    );
    assert_eq!(
        renamed(file("photo.jpg"), upper(Target::Whole)),
        "PHOTO.JPG"
    );
    assert_eq!(
        renamed(dir("my.dir"), upper(Target::Extension)),
        "my.dir",
        "폴더는 확장자 대상 규칙을 건너뛴다"
    );
    assert_eq!(renamed(file("readme"), upper(Target::Extension)), "readme");

    let title = |target| targeted(case(CaseMode::Title), target);
    assert_eq!(
        renamed(file("my photo.jpg"), title(Target::Stem)),
        "My Photo.jpg"
    );
    assert_eq!(
        renamed(file("my photo.jpg"), title(Target::Whole)),
        "My Photo.Jpg"
    );
}

#[test]
fn extension_rule_replaces_or_removes_the_extension() {
    assert_eq!(renamed(file("a.jpeg"), extension(Some("jpg"))), "a.jpg");
    assert_eq!(
        renamed(file("a.jpeg"), extension(Some(".png"))),
        "a.png",
        "앞의 점은 걷는다"
    );
    assert_eq!(renamed(file("a.jpeg"), extension(None)), "a");
    assert_eq!(renamed(file("a.jpeg"), extension(Some(""))), "a");
    assert_eq!(renamed(file("a.tar.gz"), extension(None)), "a.tar");
    assert_eq!(
        renamed(file("a.tar.gz"), extension(Some("tgz"))),
        "a.tar.tgz"
    );
}

#[test]
fn extension_rule_adds_to_names_without_one_and_skips_folders() {
    assert_eq!(renamed(file("README"), extension(Some("md"))), "README.md");
    assert_eq!(
        renamed(file(".gitignore"), extension(Some("bak"))),
        ".gitignore.bak",
        "맨 앞 점만 있는 이름은 확장자가 없다"
    );
    let result = apply(&[file("README")], &[extension(None)]);
    assert_eq!(result.entries()[0].new_name, "README");
    assert_eq!(result.entries()[0].status, EntryStatus::Unchanged);

    for rule in [extension(Some("x")), extension(None)] {
        let result = apply(&[dir("my.dir")], std::slice::from_ref(&rule));
        assert_eq!(result.entries()[0].new_name, "my.dir", "{rule:?}");
        assert_eq!(result.entries()[0].status, EntryStatus::Unchanged);
    }
}

#[test]
fn extension_rule_ignores_its_target() {
    for target in [Target::Stem, Target::Extension, Target::Whole] {
        assert_eq!(
            renamed(file("사진.JPEG"), targeted(extension(Some("jpg")), target)),
            "사진.jpg",
            "{target:?}"
        );
    }
}

#[test]
fn rules_combine_in_order_and_keep_the_s14a_checks() {
    let rules = [
        delete(Edge::Start, 0, 4),
        targeted(case(CaseMode::Lower), Target::Whole),
        extension(Some("jpg")),
    ];
    let result = apply(&[file("IMG_Photo.JPEG"), file("IMG_photo.jpg")], &rules);
    let names: Vec<&str> = result
        .entries()
        .iter()
        .map(|entry| entry.new_name.as_str())
        .collect();
    assert_eq!(names, ["photo.jpg", "photo.jpg"]);
    assert_eq!(
        result
            .entries()
            .iter()
            .map(|entry| entry.status)
            .collect::<Vec<_>>(),
        [
            EntryStatus::Problem(BatchProblem::Duplicate),
            EntryStatus::Problem(BatchProblem::Duplicate)
        ]
    );
    assert!(!result.can_execute());

    // 바뀌지 않아 남는 항목의 이름과 겹친다.
    let result = apply(&[file("a.jpeg"), file("a.jpg")], &[extension(Some("jpg"))]);
    assert_eq!(
        result
            .entries()
            .iter()
            .map(|entry| entry.status)
            .collect::<Vec<_>>(),
        [
            EntryStatus::Problem(BatchProblem::NameTaken),
            EntryStatus::Unchanged
        ]
    );
}
