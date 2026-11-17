// target: crates/facet-core/tests/spec_instant_search_address.rs
// AC: 226: 주소창과 탭 제목은 즉시 검색 위치에서 「검색: <질의>」를 보이고, 범위가 현재 폴더 아래면 그 폴더도 보인다(코어 몫: 경로 조각)
//! 즉시 검색 결과 위치의 경로 조각(주소창과 탭 제목이 쓰는 `Provider::segments`)을 확인한다: 범위가 모든 볼륨이면
//! 검색 전 폴더의 조각 없이 검색어 조각 하나뿐이고, 「현재 폴더 아래만」이면 그 폴더의 조각 뒤에 검색어 조각이 온다.
//! 이름·내용 검색 결과 위치의 조각은 그대로다. 화면 문장(「검색: 」)은 앱 몫이라 보지 않는다. 파일 시스템을 읽지 않는
//! 확인이라 모든 플랫폼에서 돈다.

use std::path::PathBuf;

use facet_core::content_search::ContentMode;
use facet_core::{FileSystemProvider, Location, PathSegment, Provider, provider_for};

/// 검색 전 폴더: 상위가 있는 실제 경로(조각이 둘 이상).
fn folder() -> PathBuf {
    std::env::temp_dir().join("facet 즉시 검색 폴더")
}

/// 조각을 견주기 쉬운 (이름, 위치)로 펼친다.
fn pairs(segments: Vec<PathSegment>) -> Vec<(String, Location)> {
    segments
        .into_iter()
        .map(|segment| (segment.label, segment.location))
        .collect()
}

/// 검색 전 폴더 자체의 경로 조각.
fn folder_pairs() -> Vec<(String, Location)> {
    pairs(FileSystemProvider.segments(&Location::file_system(folder())))
}

#[test]
fn 모든_볼륨_즉시_검색_위치의_경로_조각은_검색어_하나뿐이다() {
    let location = Location::instant_search_results(folder(), "보고서 ext:xlsx", false);
    let segments = pairs(provider_for(&location).segments(&location));
    assert_eq!(
        segments,
        vec![("보고서 ext:xlsx".to_owned(), location.clone())],
        "범위가 모든 볼륨이면 검색 전 폴더를 주소창에 보이지 않는다"
    );
}

#[test]
fn 현재_폴더_아래만_즉시_검색_위치는_그_폴더_조각_뒤에_검색어가_온다() {
    let location = Location::instant_search_results(folder(), "보고서", true);
    let mut expected = folder_pairs();
    assert!(expected.len() >= 2, "시험 폴더는 상위가 있다: {expected:?}");
    expected.push(("보고서".to_owned(), location.clone()));
    assert_eq!(pairs(provider_for(&location).segments(&location)), expected);
}

#[test]
fn 이름_검색과_내용_검색_결과_위치의_경로_조각은_그대로다() {
    let name = Location::search_results(folder(), "보고서");
    let mut expected = folder_pairs();
    expected.push(("보고서".to_owned(), name.clone()));
    assert_eq!(pairs(provider_for(&name).segments(&name)), expected);

    let content =
        Location::content_search_results(folder(), "합계", ContentMode::Literal, String::new());
    let mut expected = folder_pairs();
    expected.push(("합계".to_owned(), content.clone()));
    assert_eq!(pairs(provider_for(&content).segments(&content)), expected);
}
