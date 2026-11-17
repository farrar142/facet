// target: crates/facet-bench/tests/spec_batch_rename_entry_bench.rs
// AC: #293: 상한은 기준의 10배·기본 1만 개, 작은 수로 재면 회차 수·항목 수·남은 것 없음(Windows), 중앙값이 상한을 넘으면 수치를 담아 실패
//! 일괄 이름 바꾸기 1만 개 실행 벤치마크(`measure_batch_rename_job`)의 상수와 한도 판정, 작은 수로 한 번 돌린 측정을
//! 확인한다. 상수와 판정은 모든 환경, 실제 측정(작업 대기열의 일괄 이름 바꾸기 작업)은 Windows에서만 돈다.

use std::time::Duration;

use facet_bench::{
    BATCH_RENAME_JOB_COUNT, BATCH_RENAME_JOB_LIMIT, BATCH_RENAME_JOB_TARGET, Measurement,
    check_batch_rename_job_limit,
};

#[test]
fn the_limit_is_ten_times_the_target_for_ten_thousand_items() {
    assert_eq!(BATCH_RENAME_JOB_COUNT, 10_000);
    assert_eq!(BATCH_RENAME_JOB_LIMIT, BATCH_RENAME_JOB_TARGET * 10);
    assert!(BATCH_RENAME_JOB_TARGET > Duration::ZERO);
}

#[test]
fn a_median_over_the_limit_fails_with_the_numbers() {
    let ms = Duration::from_millis;
    let m = Measurement {
        item_count: 10_000,
        times: vec![ms(100), ms(200), ms(900)],
    };
    assert!(check_batch_rename_job_limit(&m, ms(200)).is_ok());
    assert!(check_batch_rename_job_limit(&m, ms(1000)).is_ok());
    let err = check_batch_rename_job_limit(&m, ms(150)).unwrap_err();
    assert!(
        err.contains("200") && err.contains("150"),
        "중앙값과 상한이 보여야 한다: {err}"
    );
}

#[cfg(windows)]
#[test]
fn a_small_run_measures_each_round_and_leaves_nothing_behind() {
    let root = tempfile::TempDir::new().unwrap();
    let m = facet_bench::measure_batch_rename_job(root.path(), 20, 2).expect("측정한다");
    assert_eq!(m.item_count, 20, "바꾼 항목 수");
    assert_eq!(m.times.len(), 2, "회차 수만큼");
    let left: Vec<_> = std::fs::read_dir(root.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(left.is_empty(), "만든 것을 지운다: {left:?}");
}
