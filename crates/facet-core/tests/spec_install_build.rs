// target: crates/facet-core/tests/spec_install_build.rs
// AC: #468: 버전 상수, FACET_CHANNEL 없이 빌드하면 nightly, 설치 표지로 설치 종류, 버전 줄 형식
//! 빌드 정보(`facet_core::build`)를 확인한다: 버전 상수가 패키지 버전이고, `FACET_CHANNEL` 없이 빌드한 테스트는 nightly
//! 채널이며, 실행 파일 폴더의 설치 표지 파일로 설치한 것·portable을 가르고(폴더가 없어도 portable), 버전 줄이
//! `facet <버전> (<채널>, <설치 종류>)`이다. 임시 폴더만 쓰고 모든 플랫폼에서 돈다.

use std::fs;

use facet_core::build::{
    self, Channel, INSTALL_MARKER, InstallKind, VERSION, install_kind, version_line,
};
use tempfile::TempDir;

#[test]
fn build_without_channel_variable_is_nightly() {
    match option_env!("FACET_CHANNEL") {
        Some("stable") => assert_eq!(build::channel(), Channel::Stable),
        _ => assert_eq!(build::channel(), Channel::Nightly),
    }
    assert_eq!(Channel::Stable.name(), "stable");
    assert_eq!(Channel::Nightly.name(), "nightly");
}

#[test]
fn install_marker_means_installed_otherwise_portable() {
    let dir = TempDir::new().unwrap();
    assert_eq!(install_kind(dir.path()), InstallKind::Portable);

    fs::write(dir.path().join(INSTALL_MARKER), b"").unwrap();
    assert_eq!(install_kind(dir.path()), InstallKind::Installed);

    fs::remove_file(dir.path().join(INSTALL_MARKER)).unwrap();
    assert_eq!(install_kind(dir.path()), InstallKind::Portable);

    // 다른 이름의 파일은 표지가 아니다.
    fs::write(dir.path().join(format!("{INSTALL_MARKER}.bak")), b"").unwrap();
    assert_eq!(install_kind(dir.path()), InstallKind::Portable);

    // 폴더가 없어도 portable이다.
    assert_eq!(
        install_kind(&dir.path().join("없는 폴더")),
        InstallKind::Portable
    );
    assert_eq!(InstallKind::Installed.name(), "installed");
    assert_eq!(InstallKind::Portable.name(), "portable");
}

#[test]
fn version_line_has_version_channel_and_install_kind() {
    // 버전 상수는 패키지 버전이다.
    assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
    assert_eq!(
        version_line(Channel::Stable, InstallKind::Installed),
        format!("facet {VERSION} (stable, installed)")
    );
    assert_eq!(
        version_line(Channel::Nightly, InstallKind::Portable),
        format!("facet {VERSION} (nightly, portable)")
    );
}
