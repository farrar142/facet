// target: crates/facet-bench/tests/spec_sync_folders_bench.rs
// AC: #450: 관문 벤치마크의 상수(10만 항목, 로컬 기준 1ms, CI 상한 10배)·판정 함수·측정 함수
//! 내용 읽기 관문 벤치마크(`measure_content_gate`)의 상수와 한도 판정, 작은 회차 수로 한 번 돌린 측정을 확인한다.
//! 모든 환경에서 돈다.

use std::time::Duration;

use facet_bench::{
    CONTENT_GATE_COUNT, CONTENT_GATE_LIMIT, CONTENT_GATE_TARGET, Measurement,
    check_content_gate_limit, measure_content_gate,
};

#[test]
fn gate_bench_is_100k_items_1ms_target_and_10x_limit() {
    assert_eq!(CONTENT_GATE_COUNT, 100_000);
    assert_eq!(CONTENT_GATE_TARGET, Duration::from_millis(1));
    assert_eq!(CONTENT_GATE_LIMIT, CONTENT_GATE_TARGET * 10);
}

#[test]
fn gate_limit_fails_with_median_and_limit_when_exceeded() {
    let ms = Duration::from_millis;
    let m = Measurement {
        item_count: CONTENT_GATE_COUNT,
        times: vec![ms(1), ms(6), ms(40)],
    };
    assert!(check_content_gate_limit(&m, ms(6)).is_ok());
    assert!(check_content_gate_limit(&m, ms(10)).is_ok());
    let err = check_content_gate_limit(&m, ms(5)).unwrap_err();
    assert!(
        err.contains('6') && err.contains('5'),
        "중앙값과 상한이 보여야 한다: {err}"
    );
}

#[test]
fn gate_measure_has_a_time_per_run() {
    let m = measure_content_gate(3).expect("측정한다");
    assert_eq!(m.times.len(), 3, "회차 수만큼");
    assert_eq!(m.item_count, CONTENT_GATE_COUNT);
}
