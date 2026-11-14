//! 인덱스 저장·읽기를 공개 인터페이스로만 확인한다.

use std::fs;

use facet_index::{
    ATTRIBUTE_DIRECTORY, FORMAT_VERSION, Index, JournalState, NewItem, Order, Query, SortKey,
    StoreError,
};

fn sample() -> Index {
    let mut index = Index::new(r"C:\", 5);
    let items = [
        (40, 5, "문서", ATTRIBUTE_DIRECTORY, 0),
        (41, 40, "Report.TXT", 0, 10),
        (42, 40, "메모.md", 0, 20),
        (43, 5, "Photos", ATTRIBUTE_DIRECTORY, 0),
        (44, 43, "a.jpg", 0, 30),
        (45, 43, "gone.tmp", 0, 1),
    ];
    for (number, parent, name, attributes, size) in items {
        index
            .insert(NewItem {
                number,
                parent,
                name,
                size,
                modified: number * 100,
                attributes,
            })
            .unwrap();
    }
    index.set_journal(JournalState {
        journal_id: 0xABCD,
        usn: -7,
    });
    index
}

fn saved(index: &Index) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("C.idx");
    index.save(&path).unwrap();
    (dir, path)
}

fn all(index: &Index, sort: SortKey) -> Vec<(String, u64, u64)> {
    index
        .query(&Query::name_contains("").sort(sort, Order::Ascending))
        .into_iter()
        .map(|h| (h.path, h.size, h.modified))
        .collect()
}

#[test]
fn round_trip_gives_same_results_and_journal() {
    let index = sample();
    let (_dir, path) = saved(&index);
    let loaded = Index::load(&path).unwrap();
    assert_eq!(loaded.len(), index.len());
    assert_eq!(loaded.journal(), index.journal());
    for sort in [
        SortKey::Name,
        SortKey::Path,
        SortKey::Size,
        SortKey::Modified,
    ] {
        assert_eq!(all(&loaded, sort), all(&index, sort));
    }
    let q = Query::name_contains("report");
    assert_eq!(loaded.query(&q), index.query(&q));
    assert_eq!(loaded.query(&q)[0].name, "Report.TXT");
}

#[test]
fn round_trip_without_journal_and_empty() {
    let (_dir, path) = saved(&Index::new("D:", 5));
    let loaded = Index::load(&path).unwrap();
    assert!(loaded.is_empty());
    assert_eq!(loaded.journal(), None);
}

#[test]
fn loaded_index_stays_usable() {
    let (_dir, path) = saved(&sample());
    let mut loaded = Index::load(&path).unwrap();
    loaded
        .insert(NewItem {
            number: 50,
            parent: 43,
            name: "new.png",
            size: 1,
            modified: 1,
            attributes: 0,
        })
        .unwrap();
    assert_eq!(
        loaded.query(&Query::name_contains("new"))[0].path,
        r"C:\Photos\new.png"
    );
}

#[test]
fn save_overwrites_previous_file() {
    let index = sample();
    let (_dir, path) = saved(&Index::new(r"C:\", 5));
    index.save(&path).unwrap();
    assert_eq!(Index::load(&path).unwrap().len(), index.len());
}

#[test]
fn different_version_is_rejected() {
    let (_dir, path) = saved(&sample());
    let mut bytes = fs::read(&path).unwrap();
    bytes[8..12].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
    fs::write(&path, bytes).unwrap();
    assert_eq!(
        Index::load(&path).unwrap_err(),
        StoreError::UnsupportedVersion {
            found: FORMAT_VERSION + 1
        }
    );
}

#[test]
fn wrong_checksum_is_rejected() {
    let (_dir, path) = saved(&sample());
    let mut bytes = fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    fs::write(&path, &bytes).unwrap();
    assert_eq!(
        Index::load(&path).unwrap_err(),
        StoreError::ChecksumMismatch
    );
}

#[test]
fn truncated_files_are_rejected_at_every_length() {
    let (_dir, path) = saved(&sample());
    let bytes = fs::read(&path).unwrap();
    for len in 0..bytes.len() {
        fs::write(&path, &bytes[..len]).unwrap();
        let err = Index::load(&path).unwrap_err();
        if len < 24 {
            assert_eq!(err, StoreError::Truncated, "길이 {len}");
        } else {
            assert_eq!(err, StoreError::ChecksumMismatch, "길이 {len}");
        }
    }
}

#[test]
fn foreign_and_missing_files_are_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.idx");
    fs::write(&path, vec![b'x'; 100]).unwrap();
    assert_eq!(Index::load(&path).unwrap_err(), StoreError::BadMagic);
    assert!(matches!(
        Index::load(&dir.path().join("none.idx")).unwrap_err(),
        StoreError::Io(std::io::ErrorKind::NotFound, _)
    ));
}

#[test]
fn failed_save_keeps_previous_file() {
    let index = sample();
    let (dir, path) = saved(&index);
    let before = fs::read(&path).unwrap();
    // 임시 파일 자리를 폴더로 막아 쓰기를 실패시킨다.
    fs::create_dir(dir.path().join("C.idx.tmp")).unwrap();
    let mut bigger = sample();
    bigger
        .insert(NewItem {
            number: 60,
            parent: 5,
            name: "extra",
            size: 0,
            modified: 0,
            attributes: 0,
        })
        .unwrap();
    assert!(matches!(bigger.save(&path), Err(StoreError::Io(..))));
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(Index::load(&path).unwrap().len(), index.len());
}
