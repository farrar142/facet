// target: crates/facet-win/tests/spec_sync_folders_provider.rs
// AC: #449: 동기화 폴더 판정(등록한 루트와 그 안 항목은 참, 보통 폴더·버린 뒤는 거짓)
// AC: #449: 온라인 전용 파일의 속성·크기와 목록 읽기의 내용 요청 0, 읽으면 내용 요청과 같은 내용
// AC: #449: 펼쳐지지 않은 온라인 폴더의 RECALL_ON_OPEN과 목록 요청, 고정 상태 바꾸기(항상 유지·공간 확보·재귀·동기화 폴더 밖 실패)
// AC: #449: 내려받기 실패·멈춤 모드, 연결을 끊은 뒤 복사의 SyncProviderNotRunning
//! 안전 래퍼의 클라우드 파일 기능(`cloud_files`: 동기화 폴더 판정·고정 상태 바꾸기)과 시험용 동기화 공급자
//! (`cloud_files::testing::TestSyncRoot`)를 임시 폴더로 확인한다. 속성은 상위 폴더의 열거 결과로 읽어 파일을 열지
//! 않는다. Windows에서만 돈다. 동기화 루트를 등록할 수 없는 환경(클라우드 파일 필터 드라이버 없음)이면 테스트마다
//! 건너뛰었다고 출력하고, 환경 변수 `FACET_REQUIRE_SYNC_ROOT`가 있으면 그 경우를 실패로 본다.
#![cfg(windows)]

use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use facet_win::cloud_files::testing::TestSyncRoot;
use facet_win::cloud_files::{PinState, is_in_sync_root, set_pin_state};
use facet_win::file_ops::{self, CopyStep, FileOpErrorKind};
use tempfile::TempDir;

/// CI 러너가 느려도 흔들리지 않게 넉넉히 기다린다.
const WAIT: Duration = Duration::from_secs(30);

const OFFLINE: u32 = 0x1000;
const RECALL_ON_OPEN: u32 = 0x4_0000;
const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
const PINNED: u32 = 0x8_0000;
const UNPINNED: u32 = 0x10_0000;

const ENV_REQUIRE: &str = "FACET_REQUIRE_SYNC_ROOT";

/// 임시 폴더를 동기화 루트로 등록한다. 등록할 수 없는 환경이면 건너뛴다고 출력하고 `None`.
fn sync_root(dir: &Path, test: &str) -> Option<TestSyncRoot> {
    match TestSyncRoot::register(dir) {
        Ok(root) => Some(root),
        Err(error) => {
            assert!(
                std::env::var_os(ENV_REQUIRE).is_none(),
                "{ENV_REQUIRE}가 있는데 동기화 루트를 등록하지 못했다: {error}"
            );
            println!("{test}: 동기화 루트를 등록할 수 없어 건너뜀: {error}");
            None
        }
    }
}

/// 상위 폴더의 열거 결과로 읽은 속성 값과 크기(파일을 열지 않는다).
fn entry_of(path: &Path) -> (u32, u64) {
    let parent = path.parent().unwrap();
    let name = path.file_name().unwrap();
    let entry = fs::read_dir(parent)
        .unwrap()
        .map(|e| e.unwrap())
        .find(|e| e.file_name() == name)
        .unwrap_or_else(|| panic!("{}이 목록에 없다", path.display()));
    let metadata = entry.metadata().unwrap();
    (metadata.file_attributes(), metadata.len())
}

fn attributes_of(path: &Path) -> u32 {
    entry_of(path).0
}

fn names_in(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
#[ignore = "#449"]
fn registered_root_is_sync_folder_plain_folder_and_dropped_root_are_not() {
    let dir = TempDir::new().unwrap();
    let other = TempDir::new().unwrap();
    fs::write(other.path().join("보통.txt"), b"x").unwrap();
    let Some(root) = sync_root(dir.path(), "동기화 폴더 판정") else {
        return;
    };
    let file = root.add_online_file("문서.txt", b"hello");
    assert_eq!(root.root(), dir.path());
    assert!(is_in_sync_root(dir.path()));
    assert!(is_in_sync_root(&file));
    assert!(!is_in_sync_root(other.path()));
    assert!(!is_in_sync_root(&other.path().join("보통.txt")));
    drop(root);
    assert!(
        !is_in_sync_root(dir.path()),
        "공급자를 버리면 등록이 지워진다"
    );
}

#[test]
#[ignore = "#449"]
fn online_only_file_has_attributes_and_size_and_listing_does_not_fetch() {
    let dir = TempDir::new().unwrap();
    let Some(root) = sync_root(dir.path(), "온라인 전용 파일 속성") else {
        return;
    };
    let file = root.add_online_file("사진.jpg", b"0123456789");
    let (bits, size) = entry_of(&file);
    assert_ne!(
        bits & (RECALL_ON_DATA_ACCESS | OFFLINE),
        0,
        "온라인 전용 속성이 있다: {bits:#x}"
    );
    assert_eq!(size, 10, "크기는 준 내용의 길이");
    assert_eq!(names_in(dir.path()), vec!["사진.jpg".to_string()]);
    assert_eq!(
        root.fetch_data_count(),
        0,
        "목록 읽기는 내용을 요청하지 않는다"
    );
}

#[test]
#[ignore = "#449"]
fn reading_online_only_file_fetches_given_content() {
    let dir = TempDir::new().unwrap();
    let Some(root) = sync_root(dir.path(), "온라인 전용 파일 읽기") else {
        return;
    };
    let content = "온라인 전용 본문".repeat(1000);
    let file = root.add_online_file("본문.txt", content.as_bytes());
    assert_eq!(fs::read(&file).unwrap(), content.as_bytes());
    assert!(root.fetch_data_count() >= 1);
}

#[test]
#[ignore = "#449"]
fn unexpanded_online_folder_fetches_placeholders_when_listed() {
    let dir = TempDir::new().unwrap();
    let Some(root) = sync_root(dir.path(), "펼쳐지지 않은 온라인 폴더") else {
        return;
    };
    let folder = root.add_online_folder("폴더", &[("x.txt", b"x"), ("y.txt", b"yy")]);
    assert_ne!(attributes_of(&folder) & RECALL_ON_OPEN, 0);
    assert_eq!(root.fetch_placeholders_count(), 0, "상위 목록만 읽었다");
    assert_eq!(
        names_in(&folder),
        vec!["x.txt".to_string(), "y.txt".to_string()]
    );
    assert!(root.fetch_placeholders_count() >= 1);
    assert_ne!(
        attributes_of(&folder.join("y.txt")) & (RECALL_ON_DATA_ACCESS | OFFLINE),
        0,
        "안의 파일도 온라인 전용"
    );
    assert_eq!(root.fetch_data_count(), 0);
}

#[test]
#[ignore = "#449"]
fn set_pin_state_pins_and_unpins() {
    let dir = TempDir::new().unwrap();
    let Some(root) = sync_root(dir.path(), "고정 상태 바꾸기") else {
        return;
    };
    let file = root.add_online_file("a.txt", b"a");
    set_pin_state(&file, PinState::Pinned, false).unwrap();
    let bits = attributes_of(&file);
    assert_ne!(bits & PINNED, 0, "{bits:#x}");
    assert_eq!(bits & UNPINNED, 0, "{bits:#x}");
    set_pin_state(&file, PinState::Unpinned, false).unwrap();
    let bits = attributes_of(&file);
    assert_ne!(bits & UNPINNED, 0, "{bits:#x}");
    assert_eq!(bits & PINNED, 0, "{bits:#x}");
}

#[test]
#[ignore = "#449"]
fn recursive_pin_applies_inside_folder() {
    let dir = TempDir::new().unwrap();
    let Some(root) = sync_root(dir.path(), "재귀 고정") else {
        return;
    };
    let folder = root.add_online_folder("모음", &[("1.txt", b"1"), ("2.txt", b"2")]);
    // 펼친 뒤 고정한다.
    assert_eq!(names_in(&folder).len(), 2);
    set_pin_state(&folder, PinState::Pinned, true).unwrap();
    assert_ne!(attributes_of(&folder) & PINNED, 0);
    for name in ["1.txt", "2.txt"] {
        assert_ne!(attributes_of(&folder.join(name)) & PINNED, 0, "{name}");
    }
}

#[test]
#[ignore = "#449"]
fn set_pin_state_fails_outside_sync_root() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("보통.txt");
    fs::write(&file, b"x").unwrap();
    assert!(set_pin_state(&file, PinState::Pinned, false).is_err());
    assert_eq!(attributes_of(&file) & PINNED, 0);
}

#[test]
#[ignore = "#449"]
fn fail_mode_makes_reads_fail() {
    let dir = TempDir::new().unwrap();
    let Some(root) = sync_root(dir.path(), "내려받기 실패 모드") else {
        return;
    };
    let file = root.add_online_file("끊김.txt", b"never");
    root.set_fail(true);
    assert!(fs::read(&file).is_err());
    root.set_fail(false);
    assert_eq!(fs::read(&file).unwrap(), b"never");
}

#[test]
#[ignore = "#449"]
fn stall_mode_blocks_reads_until_released() {
    let dir = TempDir::new().unwrap();
    let Some(root) = sync_root(dir.path(), "멈춤 모드") else {
        return;
    };
    let file: PathBuf = root.add_online_file("느림.txt", b"slow");
    root.set_stall(true);
    let (tx, rx) = mpsc::channel();
    let reading = file.clone();
    let reader = std::thread::spawn(move || {
        let _ = tx.send(fs::read(&reading).ok());
    });
    assert!(
        rx.recv_timeout(Duration::from_millis(500)).is_err(),
        "멈춘 동안 읽기가 끝나지 않는다"
    );
    root.set_stall(false);
    let got = rx.recv_timeout(WAIT).expect("멈춤을 풀면 끝난다");
    assert_eq!(got.as_deref(), Some(&b"slow"[..]));
    reader.join().unwrap();
}

#[test]
#[ignore = "#449"]
fn disconnected_provider_fails_copy_quickly_with_not_running() {
    let dir = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    let Some(root) = sync_root(dir.path(), "동기화 프로그램 꺼짐") else {
        return;
    };
    let file = root.add_online_file("꺼짐.txt", b"offline");
    root.disconnect();
    let started = Instant::now();
    let result = file_ops::copy_file(&file, &target.path().join("꺼짐.txt"), &mut |_| {
        CopyStep::Continue
    });
    let error = result.expect_err("공급자가 없으면 읽지 못한다");
    assert_eq!(error.kind(), FileOpErrorKind::SyncProviderNotRunning);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "기다리지 않고 실패한다: {:?}",
        started.elapsed()
    );
    assert!(!target.path().join("꺼짐.txt").exists());
}
