// target: crates/facet-core/tests/spec_install_update_decide.rs
// AC: #471: 버전 해석과 순서, 목록 해석(초안·버전 아닌 태그 빠짐, 자산·페이지 주소, 깨진 JSON), 있음/없음/prerelease, 건너뜀과 더 새 버전, nightly는 늘 없음
//! 업데이트 판정의 순수 함수(`facet_core::update`)를 고정한 GitHub Releases API 목록 JSON으로 확인한다: 의미 버전
//! 해석과 순서, 릴리스 목록에서 초안·버전이 아닌 태그(`nightly`)를 빼고 MSI·체크섬 자산과 페이지 주소를 담기,
//! prerelease를 뺀 가장 높은 정식 버전으로 「있음/없음/건너뜀」 정하기, nightly 채널은 늘 「없음」. 네트워크를 쓰지 않고
//! 모든 플랫폼에서 돈다.

use facet_core::build::Channel;
use facet_core::update::{Release, UpdateDecision, Version, decide, parse_releases};

fn v(text: &str) -> Version {
    Version::parse(text).unwrap_or_else(|| panic!("버전이어야 한다: {text}"))
}

/// GitHub `GET /repos/{owner}/{repo}/releases`가 주는 모양을 줄인 목록(최근 것부터).
const RELEASES: &str = r#"[
  {
    "tag_name": "nightly",
    "name": "Nightly",
    "html_url": "https://github.com/sandcorporation/facet/releases/tag/nightly",
    "draft": false,
    "prerelease": true,
    "assets": [
      {"name": "facet-nightly-windows-x64.zip", "browser_download_url": "https://example.test/nightly.zip"}
    ]
  },
  {
    "tag_name": "v1.3.0",
    "name": "v1.3.0",
    "html_url": "https://github.com/sandcorporation/facet/releases/tag/v1.3.0",
    "draft": true,
    "prerelease": false,
    "assets": []
  },
  {
    "tag_name": "v1.2.0-rc.1",
    "name": "v1.2.0-rc.1",
    "html_url": "https://github.com/sandcorporation/facet/releases/tag/v1.2.0-rc.1",
    "draft": false,
    "prerelease": true,
    "assets": []
  },
  {
    "tag_name": "v1.1.0",
    "name": "v1.1.0",
    "html_url": "https://github.com/sandcorporation/facet/releases/tag/v1.1.0",
    "draft": false,
    "prerelease": false,
    "body": "바뀐 점",
    "assets": [
      {"name": "facet-v1.1.0-windows-x64.zip", "browser_download_url": "https://example.test/v1.1.0/facet-v1.1.0-windows-x64.zip"},
      {"name": "facet-v1.1.0-windows-x64.zip.sha256", "browser_download_url": "https://example.test/v1.1.0/facet-v1.1.0-windows-x64.zip.sha256"},
      {"name": "facet-v1.1.0-windows-x64.msi", "browser_download_url": "https://example.test/v1.1.0/facet-v1.1.0-windows-x64.msi"},
      {"name": "facet-v1.1.0-windows-x64.msi.sha256", "browser_download_url": "https://example.test/v1.1.0/facet-v1.1.0-windows-x64.msi.sha256"}
    ]
  },
  {
    "tag_name": "v1.0.0",
    "name": "v1.0.0",
    "html_url": "https://github.com/sandcorporation/facet/releases/tag/v1.0.0",
    "draft": false,
    "prerelease": false,
    "assets": []
  }
]"#;

fn releases() -> Vec<Release> {
    parse_releases(RELEASES).expect("목록 해석")
}

fn tags(list: &[Release]) -> Vec<&str> {
    list.iter().map(|r| r.tag.as_str()).collect()
}

#[test]
#[ignore = "#471"]
fn versions_parse_and_compare_semantically() {
    assert_eq!(v("v1.2.3"), v("1.2.3"));
    assert_eq!(v("1.2.3").to_string(), "1.2.3");
    assert_eq!(v("v1.2.0-rc.1").to_string(), "1.2.0-rc.1");
    assert!(v("1.10.0") > v("1.9.9"), "수로 견준다");
    assert!(v("2.0.0") > v("1.99.99"));
    assert!(v("1.2.0-rc.1") < v("1.2.0"), "미리 버전은 정식보다 낮다");
    assert!(v("1.2.0-rc.1") > v("1.1.9"));
    for wrong in ["", "nightly", "1.2", "v", "1.x.0", "1.2.3.4", "-1.2.3"] {
        assert_eq!(Version::parse(wrong), None, "{wrong:?}는 버전이 아니다");
    }
}

#[test]
#[ignore = "#471"]
fn release_list_drops_drafts_and_non_version_tags_and_keeps_urls() {
    let list = releases();
    assert_eq!(tags(&list), ["v1.2.0-rc.1", "v1.1.0", "v1.0.0"]);

    let stable = list.iter().find(|r| r.tag == "v1.1.0").unwrap();
    assert_eq!(stable.version, v("1.1.0"));
    assert!(!stable.prerelease);
    assert_eq!(
        stable.page_url,
        "https://github.com/sandcorporation/facet/releases/tag/v1.1.0"
    );
    assert_eq!(
        stable.msi_url.as_deref(),
        Some("https://example.test/v1.1.0/facet-v1.1.0-windows-x64.msi")
    );
    assert_eq!(
        stable.checksum_url.as_deref(),
        Some("https://example.test/v1.1.0/facet-v1.1.0-windows-x64.msi.sha256")
    );

    let rc = list.iter().find(|r| r.tag == "v1.2.0-rc.1").unwrap();
    assert!(rc.prerelease);
    let old = list.iter().find(|r| r.tag == "v1.0.0").unwrap();
    assert_eq!(old.msi_url, None, "MSI가 없는 릴리스");
    assert_eq!(old.checksum_url, None);
}

#[test]
#[ignore = "#471"]
fn broken_release_list_is_an_error() {
    assert!(parse_releases("{ 깨짐").is_err());
    assert!(parse_releases("{\"message\": \"Not Found\"}").is_err());
    assert_eq!(parse_releases("[]"), Ok(Vec::new()));
}

#[test]
#[ignore = "#471"]
fn higher_stable_version_is_available() {
    let list = releases();
    match decide(&list, &v("1.0.0"), Channel::Stable, None) {
        UpdateDecision::Available(release) => assert_eq!(release.tag, "v1.1.0"),
        other => panic!("새 버전이 있어야 한다: {other:?}"),
    }
}

#[test]
#[ignore = "#471"]
fn up_to_date_when_current_is_highest_or_only_prereleases_are_higher() {
    let list = releases();
    assert_eq!(
        decide(&list, &v("1.1.0"), Channel::Stable, None),
        UpdateDecision::UpToDate,
        "prerelease v1.2.0-rc.1과 초안 v1.3.0은 보지 않는다"
    );
    assert_eq!(
        decide(&list, &v("5.0.0"), Channel::Stable, None),
        UpdateDecision::UpToDate
    );
    assert_eq!(
        decide(&[], &v("0.1.0"), Channel::Stable, None),
        UpdateDecision::UpToDate
    );
}

#[test]
#[ignore = "#471"]
fn skipped_version_is_skipped_and_newer_is_available_again() {
    let list = releases();
    match decide(&list, &v("1.0.0"), Channel::Stable, Some(&v("1.1.0"))) {
        UpdateDecision::Skipped(release) => assert_eq!(release.tag, "v1.1.0"),
        other => panic!("건너뛴 버전이어야 한다: {other:?}"),
    }

    // 1.0.5를 건너뛰었어도 1.1.0은 알린다.
    match decide(&list, &v("1.0.0"), Channel::Stable, Some(&v("1.0.5"))) {
        UpdateDecision::Available(release) => assert_eq!(release.tag, "v1.1.0"),
        other => panic!("더 새 버전은 알린다: {other:?}"),
    }
}

#[test]
#[ignore = "#471"]
fn nightly_channel_is_always_up_to_date() {
    let list = releases();
    assert_eq!(
        decide(&list, &v("0.1.0"), Channel::Nightly, None),
        UpdateDecision::UpToDate
    );
    assert_eq!(
        decide(&list, &v("0.1.0"), Channel::Nightly, Some(&v("1.1.0"))),
        UpdateDecision::UpToDate
    );
}
