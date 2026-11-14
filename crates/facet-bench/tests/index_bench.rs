//! 합성 인덱스와 인덱스 벤치마크 판정을 공개 인터페이스로만 확인한다.

use std::time::Duration;

use facet_bench::{
    BENCH_QUERY, BENCH_RESULT_LIMIT, INDEX_COUNT, INDEX_MEMORY_LIMIT, Measurement, check_memory,
    check_query_limit, measure_query, synthetic_index, test_folder_entry,
};
use facet_index::{ATTRIBUTE_DIRECTORY, Query};

#[test]
fn 합성_인덱스는_시험_폴더와_같은_이름을_모두_경로와_함께_담는다() {
    let index = synthetic_index(1000);
    assert_eq!(index.len(), 1000);
    let hits = index.query(&Query::name_contains(""));
    // 모든 항목이 볼륨 루트까지 이어진다.
    assert_eq!(hits.len(), 1000);
    for (i, hit) in hits.iter().enumerate() {
        let entry = test_folder_entry(i);
        assert_eq!(hit.name, entry.name);
        assert!(hit.path.starts_with(r"X:\"), "{}", hit.path);
        assert!(
            hit.path.ends_with(&format!(r"\{}", entry.name)),
            "{}",
            hit.path
        );
        assert_eq!(hit.attributes & ATTRIBUTE_DIRECTORY != 0, entry.is_folder);
        assert_eq!(hit.size, entry.size);
    }
    // 첫 묶음 말고는 폴더 안에 있다.
    assert!(hits.iter().any(|h| h.path.matches('\\').count() >= 3));
}

#[test]
fn 합성_인덱스는_같은_수면_같다() {
    let a = synthetic_index(500).query(&Query::name_contains("0"));
    let b = synthetic_index(500).query(&Query::name_contains("0"));
    assert_eq!(a, b);
}

#[test]
fn 합성_인덱스_100만_개는_150mb_미만이고_벤치마크_질의는_끝쪽_하나만_맞는다() {
    let index = synthetic_index(INDEX_COUNT);
    assert!(
        check_memory(index.memory_bytes(), INDEX_MEMORY_LIMIT).is_ok(),
        "{}",
        index.memory_bytes()
    );
    let hits = index.query(&Query::name_contains(BENCH_QUERY).limit(BENCH_RESULT_LIMIT));
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name, test_folder_entry(999_997).name);
}

#[test]
fn 질의를_데운_뒤_지정한_횟수만큼_잰다() {
    let index = synthetic_index(200);
    let m = measure_query(&index, &Query::name_contains("보고서").limit(5), 3);
    assert_eq!(m.item_count, 5);
    assert_eq!(m.times.len(), 3);
}

fn measurement(ms: &[u64]) -> Measurement {
    Measurement {
        item_count: 1,
        times: ms.iter().map(|&t| Duration::from_millis(t)).collect(),
    }
}

#[test]
fn 질의_상한_안이면_통과하고_넘으면_넘은_수치를_알린다() {
    let limit = Duration::from_millis(300);
    assert!(check_query_limit(&measurement(&[20, 900, 30]), limit).is_ok());
    let msg = check_query_limit(&measurement(&[500, 400, 450]), limit).unwrap_err();
    assert!(msg.contains("450"), "{msg}");
    assert!(msg.contains("300"), "{msg}");
}

#[test]
fn 메모리_상한_이상이면_넘은_수치를_알린다() {
    assert!(check_memory(149_999_999, 150_000_000).is_ok());
    let msg = check_memory(151_200_000, 150_000_000).unwrap_err();
    assert!(msg.contains("151.2"), "{msg}");
    assert!(msg.contains("150.0"), "{msg}");
}
