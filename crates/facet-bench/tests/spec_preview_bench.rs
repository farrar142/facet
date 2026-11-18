// target: crates/facet-bench/tests/spec_preview_bench.rs
// AC: #336: 256KB 텍스트 인코딩 추정·디코딩 벤치마크의 상수(로컬 기준 5ms, CI 상한 10배)·판정 함수·측정 함수
//! 미리보기 텍스트 벤치마크(`measure_text_preview`)의 상수와 한도 판정, 작은 회차 수로 한 번 돌린 측정을 확인한다.
//! 모든 환경에서 돈다.

use std::time::Duration;

use facet_bench::{
    Measurement, TEXT_PREVIEW_LIMIT, TEXT_PREVIEW_TARGET, check_text_preview_limit,
    measure_text_preview,
};

#[test]
fn the_limit_is_ten_times_the_five_millisecond_target() {
    assert_eq!(TEXT_PREVIEW_TARGET, Duration::from_millis(5));
    assert_eq!(TEXT_PREVIEW_LIMIT, TEXT_PREVIEW_TARGET * 10);
}

#[test]
fn a_median_over_the_limit_fails_with_the_numbers() {
    let ms = Duration::from_millis;
    let m = Measurement {
        item_count: 1,
        times: vec![ms(2), ms(30), ms(90)],
    };
    assert!(check_text_preview_limit(&m, ms(30)).is_ok());
    assert!(check_text_preview_limit(&m, ms(50)).is_ok());
    let err = check_text_preview_limit(&m, ms(25)).unwrap_err();
    assert!(
        err.contains("30") && err.contains("25"),
        "중앙값과 상한이 보여야 한다: {err}"
    );
}

#[test]
fn a_small_run_measures_each_round() {
    let m = measure_text_preview(3).expect("측정한다");
    assert_eq!(m.times.len(), 3, "회차 수만큼");
    assert_eq!(m.item_count, 1);
    assert!(m.times.iter().all(|t| *t > Duration::ZERO));
}
