// target: crates/facet-bench/tests/spec_zip_bench.rs
// AC: #432: 상수(10만 개, 기준과 상한 10배, Stored 비율 0.9, Deflate 하한), 시험 zip의 항목과 다시 읽기, 목록 열기·폴더 이동 측정과 한도 판정, 꺼내기 측정과 비율·하한 판정
//! zip 벤치마크(`measure_zip_list`·`measure_zip_navigate`·`measure_zip_extract`)의 상수와 한도 판정, 작은 측정을
//! 확인한다. 시험 zip 만들기·목록·폴더 이동은 모든 플랫폼에서, 꺼내기 측정은 작업 대기열의 꺼내기·복사를 쓰므로
//! Windows에서만 돈다.

use std::time::Duration;

use facet_bench::{
    Measurement, ZIP_DEFLATE_THROUGHPUT_LIMIT, ZIP_LIST_COUNT, ZIP_LIST_LIMIT, ZIP_LIST_TARGET,
    ZIP_NAVIGATE_LIMIT, ZIP_NAVIGATE_TARGET, ZIP_STORED_COPY_RATIO, ZipExtractMeasurement,
    check_zip_extract, check_zip_list_limit, check_zip_navigate_limit, make_test_zip,
    measure_zip_list, measure_zip_navigate, test_folder_entry, zip_test_entry_path,
};
use facet_core::zip_archive::Archive;
use tempfile::TempDir;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn sample(times: &[u64]) -> Measurement {
    Measurement {
        item_count: 1,
        times: times.iter().map(|&t| ms(t)).collect(),
    }
}

#[cfg(windows)]
fn left_in(dir: &std::path::Path) -> Vec<std::ffi::OsString> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect()
}

#[test]
#[ignore = "#432"]
fn constants_follow_the_spec() {
    assert_eq!(ZIP_LIST_COUNT, 100_000);
    assert_eq!(ZIP_LIST_TARGET, ms(300));
    assert_eq!(ZIP_LIST_LIMIT, ZIP_LIST_TARGET * 10);
    assert_eq!(ZIP_NAVIGATE_TARGET, ms(10));
    assert_eq!(ZIP_NAVIGATE_LIMIT, ZIP_NAVIGATE_TARGET * 10);
    assert!((ZIP_STORED_COPY_RATIO - 0.9).abs() < f64::EPSILON);
    assert!(ZIP_DEFLATE_THROUGHPUT_LIMIT > 0.0);
}

#[test]
#[ignore = "#432"]
fn test_zip_entries_are_grouped_test_folder_entries() {
    assert_eq!(
        zip_test_entry_path(0),
        format!("묶음 000/{}", test_folder_entry(0).name)
    );
    assert_eq!(
        zip_test_entry_path(1234),
        format!("묶음 001/{}", test_folder_entry(1234).name)
    );
    assert!(test_folder_entry(4).is_folder);
    assert_eq!(
        zip_test_entry_path(4),
        format!("묶음 000/{}/", test_folder_entry(4).name),
        "폴더는 끝에 '/'"
    );

    let dir = TempDir::new().unwrap();
    let zip = dir.path().join("시험.zip");
    make_test_zip(&zip, 2500).expect("시험 zip을 만든다");
    let archive = Archive::open(&zip).expect("facet의 리더로 다시 읽는다");
    let mut root: Vec<String> = archive
        .children("")
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    root.sort();
    assert_eq!(root, ["묶음 000", "묶음 001", "묶음 002"]);
    assert_eq!(archive.children("묶음 001").unwrap().len(), 1000);
    assert_eq!(archive.children("묶음 002").unwrap().len(), 500);
    let entry = test_folder_entry(1001);
    let found = archive
        .children("묶음 001")
        .unwrap()
        .into_iter()
        .find(|e| e.name == entry.name)
        .expect("항목");
    assert_eq!(found.size, entry.size);
    assert_eq!(found.is_folder, entry.is_folder);
}

#[test]
#[ignore = "#432"]
fn list_and_navigate_are_measured_per_run() {
    let dir = TempDir::new().unwrap();
    let zip = dir.path().join("시험.zip");
    make_test_zip(&zip, 3000).unwrap();
    let list = measure_zip_list(&zip, 2).expect("목록 열기를 잰다");
    assert_eq!(list.times.len(), 2, "회차 수만큼");
    assert_eq!(list.item_count, 3, "루트 항목 수");
    let navigate = measure_zip_navigate(&zip, 3).expect("폴더 이동을 잰다");
    assert_eq!(navigate.times.len(), 3);
}

#[test]
#[ignore = "#432"]
fn limits_fail_with_the_numbers_when_the_median_is_over() {
    let m = sample(&[100, 200, 900]);
    assert!(check_zip_list_limit(&m, ms(200)).is_ok());
    let err = check_zip_list_limit(&m, ms(150)).unwrap_err();
    assert!(err.contains("200") && err.contains("150"), "{err}");
    assert!(check_zip_navigate_limit(&m, ms(200)).is_ok());
    let err = check_zip_navigate_limit(&m, ms(150)).unwrap_err();
    assert!(err.contains("200") && err.contains("150"), "{err}");
}

#[test]
#[ignore = "#432"]
fn extract_checks_the_stored_ratio_and_the_deflate_floor() {
    // 1MB를 복사 100ms(10MB/s), Stored 꺼내기 125ms(8MB/s = 0.8배), Deflate 꺼내기 200ms(5MB/s).
    let m = ZipExtractMeasurement {
        bytes: 1_000_000,
        stored: sample(&[125, 125, 125]),
        deflate: sample(&[200, 200, 200]),
        copy: sample(&[100, 100, 100]),
    };
    assert!(check_zip_extract(&m, 0.75, 4_000_000.0).is_ok());
    let err = check_zip_extract(&m, 0.9, 4_000_000.0).unwrap_err();
    assert!(
        err.contains("0.8") && err.contains("0.9"),
        "Stored 비율을 보인다: {err}"
    );
    let err = check_zip_extract(&m, 0.75, 6_000_000.0).unwrap_err();
    assert!(err.contains("5"), "Deflate 처리량을 보인다: {err}");
}

#[cfg(windows)]
#[test]
#[ignore = "#432"]
fn extract_is_measured_per_run_and_leaves_nothing() {
    let dir = TempDir::new().unwrap();
    let m = facet_bench::measure_zip_extract(dir.path(), 1024 * 1024, 2).expect("꺼내기를 잰다");
    assert_eq!(m.bytes, 1024 * 1024);
    assert_eq!(m.stored.times.len(), 2);
    assert_eq!(m.deflate.times.len(), 2);
    assert_eq!(m.copy.times.len(), 2);
    let left = left_in(dir.path());
    assert!(left.is_empty(), "만든 것을 지운다: {left:?}");
}
