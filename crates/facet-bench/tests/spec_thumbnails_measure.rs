// target: crates/facet-bench/tests/spec_thumbnails_measure.rs
// AC: #331: 측정 상수(사진 1만 장, 한 화면 채우기 상한 100ms), 판정(채우기 중앙값 < 상한이고 프레임 최악값 ≤ 한 프레임의 두 배, 잰 값이 없으면 실패)
// AC: #331: 앱 보고에 한 화면 채우기 시간 줄이 더해지고 왕복(앞선 보고도 읽힘), 측정 환경 변수는 FACET_MEASURE_로 시작하고 다른 변수와 다름
//! 그리드 화면 수치 측정(`cargo xtask measure-grid`)의 측정 도구 몫을 확인한다: 상수, 판정(`grid_screen::judge`), 앱과
//! 주고받는 보고(`ScreenReport`)의 한 화면 채우기 줄. 화면 프레임 자체는 사람이 데스크톱에서 잰다. 모든 플랫폼에서 돈다.

use std::time::Duration;

use facet_bench::grid_screen::{GRID_FILL_LIMIT, GRID_IMAGE_COUNT, judge};
use facet_core::measure::{
    ENV_GRID, ENV_INSTANT, ENV_MENU, ENV_OPEN, ENV_RENAME, ENV_SEARCH, ScreenReport,
};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

#[test]
#[ignore = "#331"]
fn 사진_1만_장과_한_화면_채우기_상한_100ms다() {
    assert_eq!(GRID_IMAGE_COUNT, 10_000);
    assert_eq!(GRID_FILL_LIMIT, ms(100));
}

#[test]
#[ignore = "#331"]
fn 채우기_중앙값과_프레임_최악값이_모두_맞아야_통과한다() {
    // 60Hz: 한 프레임 16.7ms, 두 배 33.3ms.
    let ok = judge(&[ms(40), ms(60), ms(500)], &[ms(5), ms(16), ms(30)], 60.0);
    assert!(ok.fill_pass && ok.frame_pass && ok.pass());
    assert_eq!(ok.fill_median, ms(60));
    assert_eq!(ok.frame_worst, ms(30));

    let slow_fill = judge(&[ms(90), ms(120), ms(130)], &[ms(5)], 60.0);
    assert!(!slow_fill.fill_pass && slow_fill.frame_pass && !slow_fill.pass());

    let at_limit = judge(&[ms(100)], &[ms(5)], 60.0);
    assert!(!at_limit.fill_pass, "상한과 같으면 실패");

    let janky = judge(&[ms(40)], &[ms(5), ms(40)], 60.0);
    assert!(janky.fill_pass && !janky.frame_pass && !janky.pass());

    let fast_screen = judge(&[ms(40)], &[ms(20)], 144.0);
    assert!(
        !fast_screen.frame_pass,
        "144Hz의 두 프레임(13.9ms)을 넘는다"
    );
}

#[test]
#[ignore = "#331"]
fn 잰_값이_없으면_실패한다() {
    assert!(!judge(&[], &[ms(5)], 60.0).pass(), "채우기 없음");
    assert!(!judge(&[ms(40)], &[], 60.0).pass(), "프레임 없음");
}

#[test]
#[ignore = "#331"]
fn 앱_보고의_한_화면_채우기_줄을_그대로_읽는다() {
    assert!(ENV_GRID.starts_with("FACET_MEASURE_"));
    for other in [ENV_OPEN, ENV_RENAME, ENV_MENU, ENV_SEARCH, ENV_INSTANT] {
        assert_ne!(ENV_GRID, other);
    }

    let report = ScreenReport {
        rows: 10_000,
        grid_fill: Some(ms(42)),
        ..ScreenReport::default()
    };
    assert_eq!(ScreenReport::parse(&report.to_text()), Ok(report));

    let plain = ScreenReport::parse("rows 3\n").unwrap();
    assert_eq!(plain.grid_fill, None, "앞선 보고도 읽는다");
}
