//! #282: i64를 넘는 수는 십진 글자로 쓰고 읽을 때 정수와 글자를 모두 받는다.

use facet_core::batch_rename::{Place, Rule, RuleKind};
use facet_core::rename_presets::Presets;

#[test]
fn 아주_큰_수는_글자로_쓰고_그대로_읽는다() {
    let rules = vec![Rule::new(RuleKind::Number {
        start: u64::MAX,
        step: 1,
        digits: usize::MAX,
        place: Place::Start,
        separator: String::new(),
    })];
    let mut presets = Presets::new();
    presets.save_preset("큰 수", rules.clone()).unwrap();
    let text = presets.to_toml();
    assert!(text.contains("\"18446744073709551615\""), "{text}");
    assert_eq!(
        Presets::from_toml(&text)
            .unwrap()
            .find("큰 수")
            .unwrap()
            .rules,
        rules
    );
}
