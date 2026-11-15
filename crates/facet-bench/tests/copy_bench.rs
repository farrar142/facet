//! 복사 벤치마크의 재료와 한도 판정. 실제 복사는 작은 수로 한 번 돈다(복사 엔진은 Windows 전용).

use std::time::Duration;

use facet_bench::Measurement;
use facet_bench::copy::{
    COPY_SMALL_LIMIT, COPY_SMALL_TARGET, COPY_THROUGHPUT_LIMIT, COPY_THROUGHPUT_TARGET,
    CompareResult, Comparison, EXPLORER_THROUGHPUT_RATIO, check_throughput,
};
use facet_bench::screen::SystemSpec;

#[test]
fn 한도는_로컬_기준의_10배이고_처리량은_10분의_1이다() {
    assert_eq!(COPY_SMALL_LIMIT, COPY_SMALL_TARGET * 10);
    assert_eq!(COPY_THROUGHPUT_LIMIT * 10, COPY_THROUGHPUT_TARGET);
}

#[test]
fn 처리량이_하한보다_낮으면_그_수치를_알린다() {
    // 100MB를 1초(중앙값)에: 100MB/s.
    let m = Measurement {
        item_count: 1,
        times: vec![
            Duration::from_millis(900),
            Duration::from_millis(1000),
            Duration::from_millis(2000),
        ],
    };
    assert!(check_throughput(&m, 100_000_000, 100_000_000).is_ok());
    let err = check_throughput(&m, 100_000_000, 101_000_000).unwrap_err();
    assert!(
        err.contains("100.0MB/s") && err.contains("101.0MB/s") && err.contains("50.0MB/s"),
        "{err}"
    );
}

fn result(big: (u64, u64), small: (u64, u64)) -> CompareResult {
    let ms = Duration::from_millis;
    CompareResult {
        source: "C:\\원본".into(),
        target: "D:\\대상".into(),
        big: Comparison {
            bytes: 1 << 30,
            items: 1,
            facet: vec![ms(big.0)],
            explorer: vec![ms(big.1)],
        },
        small: Comparison {
            bytes: 4096 * 10,
            items: 11,
            facet: vec![ms(small.0)],
            explorer: vec![ms(small.1)],
        },
        spec: SystemSpec::default(),
        target_disk: None,
        debug_build: false,
    }
}

#[test]
fn 비교는_큰_파일_90퍼센트와_작은_파일_느리지_않음을_본다() {
    assert_eq!(EXPLORER_THROUGHPUT_RATIO, 0.9);
    // 처리량 90%: 탐색기 900ms면 facet 1000ms까지 통과.
    let v = result((1000, 900), (500, 500)).verdict();
    assert!(v.big_pass && v.small_pass && v.pass());
    let v = result((1001, 900), (500, 500)).verdict();
    assert!(!v.big_pass && !v.pass());
    let v = result((900, 900), (501, 500)).verdict();
    assert!(v.big_pass && !v.small_pass && !v.pass());
}

#[test]
fn 비교_결과는_표와_json에_통과_여부를_담는다() {
    let r = result((1000, 900), (600, 500));
    let table = r.to_table();
    assert!(table.contains("탐색기(IFileOperation)"), "{table}");
    assert!(table.contains("작은 파일 10개"), "{table}");
    assert!(table.contains("전체: 실패"), "{table}");
    let json = r.to_json();
    assert!(json.contains("\"big_file\""), "{json}");
    assert!(json.contains("\"small_files\""), "{json}");
    assert!(json.contains("\"pass\": false"), "{json}");
    assert!(json.trim_end().ends_with('}'), "{json}");
    // 쉼표가 닫는 괄호 바로 앞에 오지 않는다(올바른 JSON 모양).
    let compact: String = json.split_whitespace().collect();
    assert!(!compact.contains(",}") && !compact.contains(",]"), "{json}");
}

#[cfg(windows)]
mod windows {
    use std::fs;

    use facet_bench::copy::{CopyFixture, Engine, measure_copy};
    use tempfile::TempDir;

    #[test]
    fn 작은_수로_큰_파일과_작은_파일들의_복사_벤치마크를_한_번_돈다() {
        let dir = TempDir::new().unwrap();
        let fixture = CopyFixture::create(&dir.path().join("원본"), 3 << 20, 20, 4096).unwrap();
        assert_eq!(fs::metadata(&fixture.big).unwrap().len(), 3 << 20);
        assert_eq!(fs::read_dir(&fixture.small).unwrap().count(), 20);
        let target = dir.path().join("대상");

        for engine in [Engine::Facet, Engine::Explorer] {
            let m = measure_copy(
                engine,
                std::slice::from_ref(&fixture.big),
                &target,
                fixture.big_items(),
                2,
            )
            .unwrap();
            assert_eq!(m.times.len(), 2);
            assert_eq!(m.item_count, 1);
            // 다 재면 대상을 지운다.
            assert!(!target.exists(), "{engine:?}");

            let m = measure_copy(
                engine,
                std::slice::from_ref(&fixture.small),
                &target,
                fixture.small_items(),
                2,
            )
            .unwrap();
            assert_eq!(m.times.len(), 2);
            assert_eq!(m.item_count, 21);
            assert!(!target.exists(), "{engine:?}");
        }

        fixture.remove();
        assert!(!dir.path().join("원본").exists());
    }

    #[test]
    fn 복사한_항목_수가_다르면_실패한다() {
        let dir = TempDir::new().unwrap();
        let fixture = CopyFixture::create(&dir.path().join("원본"), 4096, 3, 4096).unwrap();
        let err = measure_copy(
            Engine::Facet,
            std::slice::from_ref(&fixture.small),
            &dir.path().join("대상"),
            99,
            1,
        )
        .unwrap_err();
        assert!(err.contains("99"), "{err}");
    }
}
