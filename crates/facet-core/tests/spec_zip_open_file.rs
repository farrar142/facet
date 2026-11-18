// target: crates/facet-core/tests/spec_zip_open_file.rs
// AC: #431: prepare가 임시 폴더에 같은 이름·내용·시각으로 읽기 전용 꺼내기, 두 번 열기와 오류(폴더·없음·zip 밖), clean이 하루 지난 것만 지움, 기본 임시 폴더
//! zip 안 파일 열기가 쓰는 임시 꺼내기(`zip_open::prepare`)와 정리(`zip_open::clean`)를 임시 폴더의 실제 zip 파일로
//! 확인한다. 기본 프로그램을 실제로 띄우는 것은 사람이 확인한다. 읽기 전용 속성과 파일 연산을 보므로 Windows에서만 돈다.

#![cfg(windows)]

#[path = "spec_zip_support.rs"]
mod zip_support;

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use facet_core::zip_archive::EntryError;
use facet_core::zip_open::{self, CLEAN_AGE, ZipOpenError};
use tempfile::TempDir;
use zip_support::Entry;

const MEMO_TIME: u32 = 1_650_000_000;

struct Fixture {
    dir: TempDir,
    zip: PathBuf,
    root: PathBuf,
}

fn fixture() -> Fixture {
    let dir = TempDir::new().unwrap();
    let zip = dir.path().join("자료.zip");
    zip_support::write_zip(
        &zip,
        &[
            Entry::deflated("문서/메모.txt", &zip_support::repeated_text(3))
                .modified_unix(MEMO_TIME),
            Entry::stored("top.txt", b"top"),
        ],
    );
    fs::write(dir.path().join("옆.txt"), "next").unwrap();
    let root = dir.path().join("zip-open");
    Fixture { dir, zip, root }
}

#[test]
#[ignore = "#431"]
fn prepare_extracts_a_read_only_copy() {
    let f = fixture();
    let path = zip_open::prepare(&f.root, &f.zip.join("문서").join("메모.txt"))
        .expect("임시 폴더에 꺼낸다");
    assert!(
        path.starts_with(&f.root),
        "임시 폴더 아래: {}",
        path.display()
    );
    assert_eq!(path.file_name().unwrap(), "메모.txt");
    assert_eq!(fs::read(&path).unwrap(), zip_support::repeated_text(3));
    let meta = fs::metadata(&path).unwrap();
    assert!(meta.permissions().readonly(), "읽기 전용 속성");
    assert_eq!(
        meta.modified().unwrap(),
        SystemTime::UNIX_EPOCH + Duration::from_secs(u64::from(MEMO_TIME))
    );
    assert!(f.dir.path().join("옆.txt").is_file());
}

#[test]
#[ignore = "#431"]
fn opening_the_same_item_twice_works() {
    let f = fixture();
    let item = f.zip.join("top.txt");
    let first = zip_open::prepare(&f.root, &item).expect("처음");
    let second = zip_open::prepare(&f.root, &item).expect("두 번째(앞 사본이 읽기 전용이어도)");
    assert_eq!(fs::read(&first).unwrap(), b"top");
    assert_eq!(fs::read(&second).unwrap(), b"top");
}

#[test]
#[ignore = "#431"]
fn folders_missing_items_and_paths_outside_a_zip_are_errors() {
    let f = fixture();
    assert!(matches!(
        zip_open::prepare(&f.root, &f.zip.join("문서")),
        Err(ZipOpenError::Entry(EntryError::NotAFile))
    ));
    assert!(matches!(
        zip_open::prepare(&f.root, &f.zip.join("없음.txt")),
        Err(ZipOpenError::Entry(EntryError::NotFound))
    ));
    assert!(matches!(
        zip_open::prepare(&f.root, &f.dir.path().join("옆.txt")),
        Err(ZipOpenError::NotInArchive)
    ));
}

#[test]
#[ignore = "#431"]
fn clean_removes_only_folders_older_than_a_day() {
    assert_eq!(CLEAN_AGE, Duration::from_secs(24 * 60 * 60));
    let f = fixture();
    let path = zip_open::prepare(&f.root, &f.zip.join("top.txt")).unwrap();
    let now = SystemTime::now();

    assert_eq!(zip_open::clean(&f.root, now), 0, "방금 만든 것은 남긴다");
    assert!(path.is_file());

    let later = now + CLEAN_AGE + Duration::from_secs(60 * 60);
    assert!(
        zip_open::clean(&f.root, later) >= 1,
        "하루 지난 것을 지운다"
    );
    assert!(!path.exists(), "읽기 전용 파일도 지운다");
}

#[test]
#[ignore = "#431"]
fn the_default_root_is_under_the_temp_folder() {
    let root = zip_open::default_root();
    assert!(root.starts_with(std::env::temp_dir()), "{}", root.display());
    assert!(
        root.ends_with(PathBuf::from("facet").join("zip-open")),
        "{}",
        root.display()
    );
}
