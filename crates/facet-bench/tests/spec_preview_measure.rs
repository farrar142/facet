// target: crates/facet-bench/tests/spec_preview_measure.rs
// AC: #344: 표시 기준(이미지 150ms·텍스트 50ms, 중앙값이 기준보다 작을 때만, 값 없으면 실패), 화살표 이동 판정(켠 때 중앙값 ≤ 끈 때 × 1.1, 켠 때 최악값 ≤ 프레임 시간 × 2, 값 없으면 실패), 전체 통과
//! 미리보기 화면 수치 측정(`cargo xtask measure-preview`)의 판정(`facet_bench::preview_screen`)을 확인한다. 실제 프레임은
//! 사람이 데스크톱에서 잰다. 모든 환경에서 돈다.

use std::time::Duration;

use facet_bench::preview_screen::{
    ARROW_SLOWDOWN_LIMIT, IMAGE_SHOW_LIMIT, PreviewScreenResult, TEXT_SHOW_LIMIT,
};
use facet_bench::screen::FRAME_WORST_FACTOR;

fn ms(n: f64) -> Duration {
    Duration::from_secs_f64(n / 1000.0)
}

fn all(values: &[f64]) -> Vec<Duration> {
    values.iter().map(|n| ms(*n)).collect()
}

/// 모두 통과하는 결과(60Hz).
fn passing() -> PreviewScreenResult {
    PreviewScreenResult {
        arrow_off: all(&[16.0, 16.5, 17.0]),
        arrow_on: all(&[16.0, 16.8, 20.0]),
        image_shows: all(&[90.0, 120.0, 140.0]),
        text_shows: all(&[20.0, 30.0, 45.0]),
        refresh_hz: Some(60.0),
    }
}

#[test]
#[ignore = "#344"]
fn the_limits_are_150_and_50_milliseconds_and_ten_percent() {
    assert_eq!(IMAGE_SHOW_LIMIT, Duration::from_millis(150));
    assert_eq!(TEXT_SHOW_LIMIT, Duration::from_millis(50));
    assert_eq!(ARROW_SLOWDOWN_LIMIT, 1.1);
    assert_eq!(FRAME_WORST_FACTOR, 2.0);
}

#[test]
#[ignore = "#344"]
fn a_result_within_every_limit_passes() {
    let v = passing().verdict();
    assert!(v.arrow_pass && v.image_pass && v.text_pass);
    assert!(v.pass());
}

#[test]
#[ignore = "#344"]
fn the_image_show_median_must_be_under_150ms() {
    let mut r = passing();
    r.image_shows = all(&[100.0, 140.0, 900.0]);
    assert!(r.verdict().image_pass, "최악값 하나는 중앙값에 들지 않는다");
    r.image_shows = all(&[150.0]);
    assert!(!r.verdict().image_pass, "기준과 같으면 실패");
    r.image_shows = all(&[160.0, 170.0, 20.0]);
    assert!(!r.verdict().image_pass);
    r.image_shows = Vec::new();
    assert!(!r.verdict().image_pass, "잰 값이 없으면 실패");
    assert!(!r.verdict().pass(), "전체 통과에도 든다");
}

#[test]
#[ignore = "#344"]
fn the_text_show_median_must_be_under_50ms() {
    let mut r = passing();
    r.text_shows = all(&[20.0, 49.0, 60.0]);
    assert!(r.verdict().text_pass);
    r.text_shows = all(&[50.0]);
    assert!(!r.verdict().text_pass, "기준과 같으면 실패");
    r.text_shows = Vec::new();
    assert!(!r.verdict().text_pass);
    assert!(!r.verdict().pass());
}

#[test]
#[ignore = "#344"]
fn arrow_frames_with_the_pane_on_must_match_those_with_it_off() {
    let mut r = passing();
    r.arrow_off = all(&[10.0, 10.0, 10.0]);
    r.arrow_on = all(&[10.0, 10.5, 12.0]);
    assert!(r.verdict().arrow_pass, "중앙값 10.5ms ≤ 10ms × 1.1");

    r.arrow_on = all(&[11.5, 12.0, 12.0]);
    assert!(!r.verdict().arrow_pass, "중앙값 12ms > 10ms × 1.1");
    assert!(!r.verdict().pass());
}

#[test]
#[ignore = "#344"]
fn the_worst_arrow_frame_with_the_pane_on_is_at_most_two_frames() {
    let mut r = passing();
    r.arrow_off = all(&[16.0, 16.0, 16.0]);
    r.arrow_on = all(&[16.0, 16.0, 40.0]);
    assert!(!r.verdict().arrow_pass, "60Hz의 두 프레임은 약 33.3ms");

    r.refresh_hz = Some(30.0);
    assert!(r.verdict().arrow_pass, "30Hz의 두 프레임은 약 66.7ms");

    r.refresh_hz = None;
    assert!(!r.verdict().arrow_pass, "주사율을 모르면 60Hz로 본다");
}

#[test]
#[ignore = "#344"]
fn missing_arrow_measurements_fail() {
    let mut r = passing();
    r.arrow_on = Vec::new();
    assert!(!r.verdict().arrow_pass);
    let mut r = passing();
    r.arrow_off = Vec::new();
    assert!(!r.verdict().arrow_pass);
}
