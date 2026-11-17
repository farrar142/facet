// target: crates/facet-core/tests/spec_rename_presets_window.rs
// AC: #283: RenameRule::from_rules가 to_rules의 반대(입력란 규칙은 되돌아옴), 입력란으로 나타낼 수 없는 규칙 목록은 None
//! 일괄 이름 바꾸기 창이 규칙 묶음을 입력란에 채울 수 있는지 가르는 `RenameRule::from_rules`를 확인한다: 입력란 규칙
//! (`RenameRule`)을 규칙 목록으로 바꿨다가 되돌리면 같은 규칙이고, 입력란으로 나타낼 수 없는 목록(다른 종류·다른 고정값·
//! 꺼진 규칙·다른 대상·다른 차례·같은 종류 둘)은 `None`이라 창이 「이 창에서 열 수 없음」으로 보인다. 순수 함수라 모든
//! 환경에서 돈다.

use facet_core::batch_rename::{
    CaseMode, Edge, NumberPosition, Numbering, Place, RenameRule, Rule, RuleKind, Target,
};

fn numbering(start: u64, digits: usize, position: NumberPosition, separator: &str) -> Numbering {
    Numbering {
        start,
        digits,
        position,
        separator: separator.to_owned(),
    }
}

/// 입력란이 만들 수 있는 규칙들.
fn window_rules() -> Vec<RenameRule> {
    vec![
        RenameRule::default(),
        RenameRule {
            find: "IMG_".to_owned(),
            ..RenameRule::default()
        },
        RenameRule {
            find: "(1)".to_owned(),
            replace: "$1 사본".to_owned(),
            ..RenameRule::default()
        },
        RenameRule {
            prefix: "사진_".to_owned(),
            ..RenameRule::default()
        },
        RenameRule {
            suffix: " 🎉".to_owned(),
            ..RenameRule::default()
        },
        RenameRule {
            numbering: Some(numbering(1, 3, NumberPosition::Start, "_")),
            ..RenameRule::default()
        },
        RenameRule {
            numbering: Some(numbering(0, 0, NumberPosition::End, "")),
            ..RenameRule::default()
        },
        RenameRule {
            find: "DSC".to_owned(),
            replace: "사진".to_owned(),
            prefix: "앞-".to_owned(),
            suffix: "-뒤".to_owned(),
            numbering: Some(numbering(42, 5, NumberPosition::End, " - ")),
        },
    ]
}

fn literal(find: &str, replace: &str) -> Rule {
    Rule::new(RuleKind::Replace {
        find: find.to_owned(),
        replace: replace.to_owned(),
        regex: false,
        case_sensitive: false,
        all: true,
    })
}

fn number(step: u64, place: Place) -> Rule {
    Rule::new(RuleKind::Number {
        start: 1,
        step,
        digits: 3,
        place,
        separator: "_".to_owned(),
    })
}

fn insert(from: Edge, at: usize) -> Rule {
    Rule::new(RuleKind::Insert {
        text: "x".to_owned(),
        from,
        at,
    })
}

#[test]
#[ignore = "#283"]
fn 입력란_규칙을_규칙_목록으로_바꿨다가_되돌리면_같은_규칙이다() {
    for rule in window_rules() {
        assert_eq!(
            RenameRule::from_rules(&rule.to_rules()),
            Some(rule.clone()),
            "{rule:?}"
        );
    }
    assert_eq!(
        RenameRule::from_rules(&[]),
        Some(RenameRule::default()),
        "빈 목록은 빈 입력란"
    );
}

#[test]
#[ignore = "#283"]
fn 입력란이_만드는_모양의_손으로_만든_목록도_채울_수_있다() {
    let rules = vec![
        literal("a", "b"),
        number(1, Place::End),
        insert(Edge::Start, 0),
        insert(Edge::End, 0),
    ];
    assert_eq!(
        RenameRule::from_rules(&rules),
        Some(RenameRule {
            find: "a".to_owned(),
            replace: "b".to_owned(),
            prefix: "x".to_owned(),
            suffix: "x".to_owned(),
            numbering: Some(numbering(1, 3, NumberPosition::End, "_")),
        })
    );
}

#[test]
#[ignore = "#283"]
fn 찾아_바꾸기가_입력란과_다르면_열_수_없다() {
    let variants = [
        ("정규식", true, false, true),
        ("대소문자 구분", false, true, true),
        ("첫 번째만", false, false, false),
    ];
    for (case, regex, case_sensitive, all) in variants {
        let rules = vec![Rule::new(RuleKind::Replace {
            find: "a".to_owned(),
            replace: "b".to_owned(),
            regex,
            case_sensitive,
            all,
        })];
        assert_eq!(RenameRule::from_rules(&rules), None, "{case}");
    }
}

#[test]
#[ignore = "#283"]
fn 번호와_넣기가_입력란과_다르면_열_수_없다() {
    let cases: Vec<(&str, Vec<Rule>)> = vec![
        ("증가 2", vec![number(2, Place::Start)]),
        (
            "N번째 자리 번호",
            vec![number(
                1,
                Place::At {
                    from: Edge::Start,
                    at: 2,
                },
            )],
        ),
        ("앞 1번째 자리 넣기", vec![insert(Edge::Start, 1)]),
        ("뒤 3번째 자리 넣기", vec![insert(Edge::End, 3)]),
    ];
    for (case, rules) in cases {
        assert_eq!(RenameRule::from_rules(&rules), None, "{case}");
    }
}

#[test]
#[ignore = "#283"]
fn 입력란에_없는_종류는_열_수_없다() {
    let kinds = [
        RuleKind::Delete {
            from: Edge::Start,
            at: 0,
            count: 2,
        },
        RuleKind::Case(CaseMode::Lower),
        RuleKind::Extension {
            new: Some("jpg".to_owned()),
        },
        RuleKind::Extension { new: None },
        RuleKind::Date {
            format: "yyyy-MM-dd".to_owned(),
            place: Place::Start,
            separator: "_".to_owned(),
        },
    ];
    for kind in kinds {
        let rules = vec![literal("a", "b"), Rule::new(kind)];
        assert_eq!(RenameRule::from_rules(&rules), None, "{rules:?}");
    }
}

#[test]
#[ignore = "#283"]
fn 꺼진_규칙이나_이름_부분이_아닌_대상은_열_수_없다() {
    let mut off = literal("a", "b");
    off.enabled = false;
    assert_eq!(RenameRule::from_rules(&[off]), None, "꺼진 규칙");

    for target in [Target::Extension, Target::Whole] {
        let mut rule = insert(Edge::Start, 0);
        rule.target = target;
        assert_eq!(RenameRule::from_rules(&[rule]), None, "{target:?}");
    }
}

#[test]
#[ignore = "#283"]
fn 차례가_다르거나_같은_종류가_둘이면_열_수_없다() {
    let cases: Vec<(&str, Vec<Rule>)> = vec![
        (
            "번호가 찾아 바꾸기 앞",
            vec![number(1, Place::Start), literal("a", "b")],
        ),
        (
            "앞 붙이기가 번호 앞",
            vec![insert(Edge::Start, 0), number(1, Place::Start)],
        ),
        (
            "뒤 붙이기가 앞 붙이기 앞",
            vec![insert(Edge::End, 0), insert(Edge::Start, 0)],
        ),
        ("찾아 바꾸기 둘", vec![literal("a", "b"), literal("c", "d")]),
        (
            "번호 둘",
            vec![number(1, Place::Start), number(1, Place::End)],
        ),
        (
            "앞 붙이기 둘",
            vec![insert(Edge::Start, 0), insert(Edge::Start, 0)],
        ),
    ];
    for (case, rules) in cases {
        assert_eq!(RenameRule::from_rules(&rules), None, "{case}");
    }
}
