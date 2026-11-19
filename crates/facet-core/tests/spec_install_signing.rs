// target: crates/facet-core/tests/spec_install_signing.rs
// AC: #481: 비밀값이 없으면 unsigned와 서명 없는 MSI, 비밀값이 있으면 signed와 게시자 이름이 같은 MSI 서명, 서명한 MSI의 체크섬이 맞음
//! `cargo xtask dist`가 만든 산출물 폴더의 서명 상태를 확인한다. 산출물 폴더 `FACET_TEST_DIST_DIR`이 있을 때만 돌고(서명한
//! 빌드면 `FACET_TEST_EXPECT_SIGNED=1`도), 없으면 건너뛰었다고 출력한다. 태그·nightly 빌드가 이 변수를 주고 돌린다.
//! Windows 전용.

#![cfg(windows)]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use facet_core::update::{self, verify_download};
use facet_win::authenticode::{self, SignatureError};
use tempfile::TempDir;

/// 산출물 폴더와 서명을 기대하는지. 변수가 없으면 `None`.
fn dist() -> Option<(PathBuf, bool)> {
    let Some(dir) = env::var_os("FACET_TEST_DIST_DIR").map(PathBuf::from) else {
        eprintln!("FACET_TEST_DIST_DIR이 없어 서명 시험을 건너뛴다");
        return None;
    };
    let signed = env::var("FACET_TEST_EXPECT_SIGNED").is_ok_and(|v| v == "1");
    Some((dir, signed))
}

fn msis(dir: &Path) -> Vec<PathBuf> {
    let found: Vec<PathBuf> = fs::read_dir(dir)
        .expect("산출물 폴더")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "msi"))
        .collect();
    assert!(!found.is_empty(), "산출물 폴더에 MSI가 없다");
    found
}

#[test]
#[ignore = "#481"]
fn 서명_표시가_산출물의_서명과_맞는다() {
    let Some((dir, signed)) = dist() else {
        return;
    };
    let marker = fs::read_to_string(dir.join("signing.txt")).expect("signing.txt");
    assert_eq!(
        marker.trim(),
        if signed { "signed" } else { "unsigned" },
        "서명 표시"
    );

    for msi in msis(&dir) {
        let signer = authenticode::signer(&msi);
        if signed {
            let publisher = update::publisher().expect("서명한 빌드는 게시자 이름을 갖는다");
            assert_eq!(signer.as_deref(), Ok(publisher), "{}", msi.display());
        } else {
            assert_eq!(signer, Err(SignatureError::NotSigned), "{}", msi.display());
        }
    }
}

#[test]
#[ignore = "#481"]
fn 서명한_MSI는_업데이트_확인을_통과한다() {
    let Some((dir, signed)) = dist() else {
        return;
    };
    if !signed {
        eprintln!("서명하지 않은 빌드라 건너뛴다");
        return;
    }
    let publisher = update::publisher().expect("게시자 이름");
    for msi in msis(&dir) {
        // 확인에 실패하면 파일을 지우므로 사본으로 확인한다.
        let copy = TempDir::new().unwrap();
        let name = msi.file_name().unwrap();
        let file = copy.path().join(name);
        let checksum = copy
            .path()
            .join(format!("{}.sha256", name.to_string_lossy()));
        fs::copy(&msi, &file).unwrap();
        fs::copy(
            dir.join(format!("{}.sha256", name.to_string_lossy())),
            &checksum,
        )
        .expect("MSI 체크섬 파일");
        assert_eq!(
            verify_download(&file, &checksum, publisher),
            Ok(()),
            "{}",
            msi.display()
        );
    }
}
