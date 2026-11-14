//! 실제 볼륨 테스트: 관리자 권한과 NTFS 시험 볼륨이 필요한 테스트.
//!
//! 표시 방법(작업 공간 전체의 약속):
//! - 테스트 이름을 `real_volume_`으로 시작하고 `#[ignore = "실제 볼륨 테스트 ..."]`를 붙인다.
//!   그래서 기본 `cargo test --workspace`에서 빠진다.
//! - 시험 볼륨의 드라이브 문자는 환경 변수 `FACET_TEST_VOLUME`(예: `T:`)으로 받는다.
//! - `cargo xtask ci`는 관리자일 때 시험 VHD를 만들어 NTFS로 포맷해 붙이고, 이 변수를 넣어
//!   `cargo test --workspace -- --ignored real_volume_`으로 이 테스트들만 돌린 뒤 VHD를 뗀다.
//!   관리자가 아니면 건너뛴다고 출력한다.
//! - 손으로 돌리기: 관리자 터미널에서 시험 볼륨을 붙이고
//!   `FACET_TEST_VOLUME=T: cargo test -p facet-indexer -- --ignored real_volume_`.
#![cfg(windows)]

use std::env;
use std::process::Command;

const VOLUME_VAR: &str = "FACET_TEST_VOLUME";

fn test_volume() -> String {
    env::var(VOLUME_VAR).unwrap_or_else(|_| {
        panic!("실제 볼륨 테스트에는 시험 NTFS 볼륨의 드라이브 문자를 담은 FACET_TEST_VOLUME이 필요합니다")
    })
}

#[test]
#[ignore = "실제 볼륨 테스트: 관리자 권한과 FACET_TEST_VOLUME이 필요하다(cargo xtask ci가 관리자일 때 돌린다)"]
fn real_volume_dump_mft_reads_file_records() {
    let volume = test_volume();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("volume.mft");

    let status = Command::new(env!("CARGO_BIN_EXE_facet-indexer"))
        .args(["dump-mft", &volume])
        .arg(&out)
        .status()
        .unwrap();
    assert!(status.success(), "dump-mft 실패: {status}");

    let bytes = std::fs::read(&out).unwrap();
    // 시험 VHD는 레코드 1024바이트로 포맷된다.
    const RECORD: usize = 1024;
    assert_eq!(bytes.len() % RECORD, 0, "레코드 크기의 배수가 아님");
    let records: Vec<&[u8]> = bytes.chunks(RECORD).collect();
    // 레코드 0..16은 NTFS 메타데이터 파일($MFT, $MFTMirr, $LogFile, ..., $Extend 등)이라 늘 쓰인다.
    assert!(records.len() >= 16, "레코드가 {}개뿐임", records.len());
    for (n, record) in records.iter().take(16).enumerate() {
        assert_eq!(&record[0..4], b"FILE", "레코드 {n}의 서명이 FILE이 아님");
    }
}
