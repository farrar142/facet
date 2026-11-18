// target: crates/facet-bench/tests/spec_thumbnails_bench.rs
// AC: #326: 4000×3000 JPEG 축소 디코딩 벤치마크: 상한은 기준의 10배, 중앙값이 상한을 넘으면 수치를 담아 실패, 한 번 돌린 측정은 회차 수만큼이고 남긴 것이 없음
// AC: #328: 캐시 읽기 1천 건 벤치마크: 상한은 기준의 10배·기본 1천 건, 중앙값이 상한을 넘으면 수치를 담아 실패, 작은 수로 잰 측정은 건수·회차 수가 맞고 남긴 것이 없음
//! 썸네일 벤치마크(`measure_thumbnail_decode`·`measure_thumbnail_cache_read`)의 상수와 한도 판정, 작은 측정을 확인한다.
//! 순수 Rust 디코더와 파일 캐시라 모든 플랫폼에서 돈다.

use std::path::Path;
use std::time::Duration;

use facet_bench::{
    Measurement, THUMBNAIL_CACHE_READ_COUNT, THUMBNAIL_CACHE_READ_LIMIT,
    THUMBNAIL_CACHE_READ_TARGET, THUMBNAIL_DECODE_LIMIT, THUMBNAIL_DECODE_TARGET,
    check_thumbnail_cache_read_limit, check_thumbnail_decode_limit, measure_thumbnail_cache_read,
    measure_thumbnail_decode,
};
use tempfile::TempDir;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn sample() -> Measurement {
    Measurement {
        item_count: 1,
        times: vec![ms(100), ms(200), ms(900)],
    }
}

fn left_in(dir: &Path) -> Vec<std::ffi::OsString> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect()
}

#[test]
fn 디코딩_상한은_기준의_10배다() {
    assert_eq!(THUMBNAIL_DECODE_LIMIT, THUMBNAIL_DECODE_TARGET * 10);
    assert!(THUMBNAIL_DECODE_TARGET > Duration::ZERO);
}

#[test]
fn 디코딩_중앙값이_상한을_넘으면_수치를_담아_실패한다() {
    let m = sample();
    assert!(check_thumbnail_decode_limit(&m, ms(200)).is_ok());
    assert!(check_thumbnail_decode_limit(&m, ms(1000)).is_ok());
    let err = check_thumbnail_decode_limit(&m, ms(150)).unwrap_err();
    assert!(
        err.contains("200") && err.contains("150"),
        "중앙값과 상한이 보여야 한다: {err}"
    );
}

#[test]
fn 디코딩_측정은_회차마다_재고_남긴_것이_없다() {
    let root = TempDir::new().unwrap();
    let m = measure_thumbnail_decode(root.path(), 2).expect("측정한다");
    assert_eq!(m.item_count, 1, "그림 하나");
    assert_eq!(m.times.len(), 2, "회차 수만큼");
    assert!(m.times.iter().all(|t| *t > Duration::ZERO));
    let left = left_in(root.path());
    assert!(left.is_empty(), "만든 것을 지운다: {left:?}");
}

#[test]
#[ignore = "#328"]
fn 캐시_읽기_상한은_기준의_10배이고_1천_건이다() {
    assert_eq!(THUMBNAIL_CACHE_READ_COUNT, 1_000);
    assert_eq!(THUMBNAIL_CACHE_READ_LIMIT, THUMBNAIL_CACHE_READ_TARGET * 10);
    assert!(THUMBNAIL_CACHE_READ_TARGET > Duration::ZERO);
}

#[test]
#[ignore = "#328"]
fn 캐시_읽기_중앙값이_상한을_넘으면_수치를_담아_실패한다() {
    let m = sample();
    assert!(check_thumbnail_cache_read_limit(&m, ms(200)).is_ok());
    let err = check_thumbnail_cache_read_limit(&m, ms(150)).unwrap_err();
    assert!(
        err.contains("200") && err.contains("150"),
        "중앙값과 상한이 보여야 한다: {err}"
    );
}

#[test]
#[ignore = "#328"]
fn 캐시_읽기_측정은_건수와_회차가_맞고_남긴_것이_없다() {
    let root = TempDir::new().unwrap();
    let m = measure_thumbnail_cache_read(root.path(), 20, 2).expect("측정한다");
    assert_eq!(m.item_count, 20, "읽은 건수");
    assert_eq!(m.times.len(), 2, "회차 수만큼");
    let left = left_in(root.path());
    assert!(left.is_empty(), "만든 것을 지운다: {left:?}");
}
