// target: crates/facet-core/tests/spec_settings_view.rs
// AC: #464: 설정 값에서 쓸 모양(밝게·어둡게·시스템 따름), 글꼴 크기와 행 높이(기본 9pt·24px, 0이면 글꼴에 맞춤, 맞춘 값보다 작으면 맞춘 값)
//! 테마·글꼴·행 높이 설정을 읽는 코어 값(`Settings::appearance`·`font_size`·`row_height`, `settings::auto_row_height`)을
//! 확인한다. 실제 색·글꼴·Windows 테마 따라가기는 사람이 확인한다. 순수 값이라 모든 환경에서 돈다.

use facet_core::settings::{self, Appearance, Settings, Value};

fn with(pairs: &[(&str, Value)]) -> Settings {
    let mut settings = Settings::defaults();
    for (key, value) in pairs {
        settings.set(key, *value).expect("받아야 한다");
    }
    settings
}

#[test]
#[ignore = "#464"]
fn the_appearance_follows_the_theme_setting() {
    let light = with(&[("view.theme", Value::Choice("light"))]);
    assert_eq!(light.appearance(false), Appearance::Light);
    assert_eq!(light.appearance(true), Appearance::Light);

    let dark = with(&[("view.theme", Value::Choice("dark"))]);
    assert_eq!(dark.appearance(false), Appearance::Dark);
    assert_eq!(dark.appearance(true), Appearance::Dark);

    let system = with(&[("view.theme", Value::Choice("system"))]);
    assert_eq!(system.appearance(false), Appearance::Light);
    assert_eq!(system.appearance(true), Appearance::Dark);

    // 기본은 시스템 따름이다.
    let defaults = Settings::defaults();
    assert_eq!(defaults.appearance(true), Appearance::Dark);
    assert_eq!(defaults.appearance(false), Appearance::Light);
}

#[test]
#[ignore = "#464"]
fn the_default_font_and_row_height_match_todays_screen() {
    let defaults = Settings::defaults();
    assert_eq!(defaults.font_size(), 9);
    assert_eq!(settings::auto_row_height(9), 24);
    assert_eq!(defaults.row_height(), 24);
}

#[test]
#[ignore = "#464"]
fn the_automatic_row_height_grows_with_the_font() {
    let mut last = 0;
    for size in 8..=20 {
        let height = settings::auto_row_height(size);
        assert!(height >= last, "{size}pt: {height} < {last}");
        assert!(
            f64::from(height) > f64::from(size) * 96.0 / 72.0,
            "{size}pt"
        );
        last = height;
    }
    assert!(settings::auto_row_height(20) > settings::auto_row_height(9));

    let big = with(&[("view.font_size", Value::Integer(16))]);
    assert_eq!(big.font_size(), 16);
    assert_eq!(big.row_height(), settings::auto_row_height(16));
}

#[test]
#[ignore = "#464"]
fn an_explicit_row_height_is_used_unless_it_would_clip_the_text() {
    let tall = with(&[("view.row_height", Value::Integer(40))]);
    assert_eq!(tall.row_height(), 40);

    // 글꼴에 맞춘 값보다 작으면 맞춘 값을 쓴다.
    let short = with(&[("view.row_height", Value::Integer(10))]);
    assert_eq!(short.row_height(), 24);

    let both = with(&[
        ("view.font_size", Value::Integer(18)),
        ("view.row_height", Value::Integer(30)),
    ]);
    assert_eq!(
        both.row_height(),
        30u32.max(settings::auto_row_height(18)),
        "{}",
        settings::auto_row_height(18)
    );

    // 0으로 돌리면 다시 맞춤이다.
    let auto = with(&[
        ("view.row_height", Value::Integer(40)),
        ("view.row_height", Value::Integer(0)),
    ]);
    assert_eq!(auto.row_height(), 24);
}
