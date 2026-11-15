//! USN 레코드 적용과 「다시 만들기 필요」 판정을 공개 인터페이스로만 확인한다.
//! 레코드 열은 `USN_RECORD_V2` 바이트로 직접 만든다.

use facet_index::{
    ATTRIBUTE_DIRECTORY, Freshness, Index, JournalInfo, JournalState, NewItem, Query,
    RebuildReason, SecurityChange, USN_REASON_FILE_CREATE, USN_REASON_FILE_DELETE,
    USN_REASON_RENAME_NEW_NAME, USN_REASON_RENAME_OLD_NAME, USN_REASON_SECURITY_CHANGE, UsnError,
};

const ROOT: u64 = 5;
const DIR: u32 = ATTRIBUTE_DIRECTORY;
const FILE: u32 = 0x20;

/// `USN_RECORD_V2` 하나의 바이트.
fn record(number: u64, parent: u64, usn: i64, reason: u32, attributes: u32, name: &str) -> Vec<u8> {
    let units: Vec<u16> = name.encode_utf16().collect();
    let length = (60 + units.len() * 2).next_multiple_of(8);
    let mut b = vec![0u8; length];
    b[0..4].copy_from_slice(&(length as u32).to_le_bytes());
    b[4..6].copy_from_slice(&2u16.to_le_bytes());
    // 위 16비트는 재사용 순번이다. 적용할 때 무시해야 한다.
    b[8..16].copy_from_slice(&(number | 7 << 48).to_le_bytes());
    b[16..24].copy_from_slice(&(parent | 3 << 48).to_le_bytes());
    b[24..32].copy_from_slice(&usn.to_le_bytes());
    b[32..40].copy_from_slice(&1_234u64.to_le_bytes());
    b[40..44].copy_from_slice(&reason.to_le_bytes());
    b[52..56].copy_from_slice(&attributes.to_le_bytes());
    b[56..58].copy_from_slice(&((units.len() * 2) as u16).to_le_bytes());
    b[58..60].copy_from_slice(&60u16.to_le_bytes());
    for (i, u) in units.iter().enumerate() {
        b[60 + i * 2..62 + i * 2].copy_from_slice(&u.to_le_bytes());
    }
    b
}

fn create(number: u64, parent: u64, usn: i64, attributes: u32, name: &str) -> Vec<u8> {
    record(
        number,
        parent,
        usn,
        USN_REASON_FILE_CREATE,
        attributes,
        name,
    )
}

fn delete(number: u64, parent: u64, usn: i64, attributes: u32, name: &str) -> Vec<u8> {
    record(
        number,
        parent,
        usn,
        USN_REASON_FILE_DELETE,
        attributes,
        name,
    )
}

/// 이름바꾸기·이동: 옛 이름 레코드와 새 이름 레코드 한 쌍.
fn rename(
    number: u64,
    (old_parent, old_name): (u64, &str),
    (new_parent, new_name): (u64, &str),
    usn: i64,
    attributes: u32,
) -> Vec<u8> {
    let mut b = record(
        number,
        old_parent,
        usn,
        USN_REASON_RENAME_OLD_NAME,
        attributes,
        old_name,
    );
    b.extend(record(
        number,
        new_parent,
        usn + 100,
        USN_REASON_RENAME_NEW_NAME,
        attributes,
        new_name,
    ));
    b
}

fn apply(index: &mut Index, records: &[Vec<u8>]) {
    index.apply_usn(&records.concat()).unwrap();
}

fn paths(index: &Index, part: &str) -> Vec<String> {
    let mut paths: Vec<String> = index
        .query(&Query::name_contains(part))
        .into_iter()
        .map(|h| h.path)
        .collect();
    paths.sort();
    paths
}

fn base() -> Index {
    let mut index = Index::new(r"C:\", ROOT);
    for (number, parent, name, attributes) in [
        (40, ROOT, "문서", DIR),
        (41, ROOT, "사진", DIR),
        (50, 40, "report.txt", FILE),
    ] {
        index
            .insert(NewItem {
                number,
                parent,
                name,
                size: 10,
                modified: 99,
                attributes,
            })
            .unwrap();
    }
    index
}

#[test]
fn create_adds_files_and_folders_with_paths() {
    let mut index = base();
    apply(
        &mut index,
        &[
            create(60, 41, 1, DIR, "여행"),
            create(61, 60, 2, FILE, "🎉 해변.jpg"),
        ],
    );
    assert_eq!(paths(&index, "해변"), [r"C:\사진\여행\🎉 해변.jpg"]);
    assert_eq!(index.len(), 5);
}

#[test]
fn delete_removes_the_item() {
    let mut index = base();
    apply(&mut index, &[delete(50, 40, 1, FILE, "report.txt")]);
    assert!(paths(&index, "report").is_empty());
    assert_eq!(index.len(), 2);
    // 같은 번호를 다시 만들 수 있다.
    apply(&mut index, &[create(50, 41, 2, FILE, "again.txt")]);
    assert_eq!(paths(&index, "again"), [r"C:\사진\again.txt"]);
}

#[test]
fn rename_in_place_keeps_size_and_date() {
    let mut index = base();
    apply(
        &mut index,
        &[rename(50, (40, "report.txt"), (40, "Final.TXT"), 1, FILE)],
    );
    assert!(paths(&index, "report").is_empty());
    let hits = index.query(&Query::name_contains("final"));
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, r"C:\문서\Final.TXT");
    assert_eq!((hits[0].size, hits[0].modified), (10, 99));
}

/// NTFS는 이름바꾸기에 옛 이름, 새 이름, 그리고 닫을 때 새 이름(이유가 쌓인 것)을 한 번 더 낸다.
#[test]
fn repeated_new_name_record_keeps_size_and_date() {
    let mut index = base();
    let mut records = rename(50, (40, "report.txt"), (41, "moved.txt"), 1, FILE);
    records.extend(record(
        50,
        41,
        300,
        USN_REASON_RENAME_OLD_NAME | USN_REASON_RENAME_NEW_NAME,
        FILE,
        "moved.txt",
    ));
    index.apply_usn(&records).unwrap();
    let hits = index.query(&Query::name_contains("moved"));
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, r"C:\사진\moved.txt");
    assert_eq!((hits[0].size, hits[0].modified), (10, 99));
    assert!(paths(&index, "report").is_empty());
    assert_eq!(index.len(), 3);
}

#[test]
fn rename_split_across_calls_still_works() {
    let mut index = base();
    let both = rename(50, (40, "report.txt"), (41, "moved.txt"), 1, FILE);
    let (old, new) = both.split_at(both.len() / 2);
    index.apply_usn(old).unwrap();
    index.apply_usn(new).unwrap();
    assert_eq!(paths(&index, "moved"), [r"C:\사진\moved.txt"]);
    assert!(paths(&index, "report").is_empty());
}

#[test]
fn file_move_changes_the_path() {
    let mut index = base();
    apply(
        &mut index,
        &[rename(50, (40, "report.txt"), (41, "report.txt"), 1, FILE)],
    );
    assert_eq!(paths(&index, "report"), [r"C:\사진\report.txt"]);
}

#[test]
fn folder_move_changes_paths_below_it() {
    let mut index = base();
    apply(
        &mut index,
        &[
            create(60, 40, 1, DIR, "sub"),
            create(61, 60, 2, FILE, "deep.txt"),
            rename(40, (ROOT, "문서"), (41, "문서 옮김"), 3, DIR),
        ],
    );
    assert_eq!(paths(&index, "deep"), [r"C:\사진\문서 옮김\sub\deep.txt"]);
    assert_eq!(paths(&index, "report"), [r"C:\사진\문서 옮김\report.txt"]);
    assert_eq!(paths(&index, "문서"), [r"C:\사진\문서 옮김"]);
}

#[test]
fn applying_the_same_records_twice_changes_nothing() {
    let mut index = base();
    let records = [
        create(60, 41, 1, FILE, "a.txt"),
        delete(50, 40, 2, FILE, "report.txt"),
    ];
    apply(&mut index, &records);
    apply(&mut index, &records);
    assert_eq!(paths(&index, "a.txt"), [r"C:\사진\a.txt"]);
    assert_eq!(index.len(), 3);
}

#[test]
fn journal_id_change_needs_rebuild() {
    let mut index = base();
    index.set_journal(JournalState {
        journal_id: 7,
        usn: 500,
    });
    let info = |journal_id, first_usn| JournalInfo {
        journal_id,
        first_usn,
    };
    assert_eq!(index.check_journal(info(7, 100)), Freshness::Current);
    assert_eq!(index.check_journal(info(7, 500)), Freshness::Current);
    assert_eq!(
        index.check_journal(info(8, 100)),
        Freshness::RebuildNeeded(RebuildReason::JournalIdChanged)
    );
}

#[test]
fn overflow_needs_rebuild() {
    let mut index = base();
    index.set_journal(JournalState {
        journal_id: 7,
        usn: 500,
    });
    assert_eq!(
        index.check_journal(JournalInfo {
            journal_id: 7,
            first_usn: 501
        }),
        Freshness::RebuildNeeded(RebuildReason::Overflowed)
    );
}

#[test]
fn index_without_journal_state_needs_rebuild() {
    let index = base();
    assert_eq!(index.journal(), None);
    assert_eq!(
        index.check_journal(JournalInfo {
            journal_id: 1,
            first_usn: 0
        }),
        Freshness::RebuildNeeded(RebuildReason::NoJournalState)
    );
}

#[test]
fn applying_records_advances_the_journal_usn() {
    let mut index = base();
    index.set_journal(JournalState {
        journal_id: 7,
        usn: 100,
    });
    apply(
        &mut index,
        &[
            create(60, 41, 150, FILE, "a"),
            create(61, 41, 220, FILE, "b"),
        ],
    );
    assert_eq!(
        index.journal(),
        Some(JournalState {
            journal_id: 7,
            usn: 220
        })
    );
    // 저장한 상태를 따라가면 넘침이 아니다.
    assert_eq!(
        index.check_journal(JournalInfo {
            journal_id: 7,
            first_usn: 200
        }),
        Freshness::Current
    );
}

#[test]
fn security_change_reports_the_folder() {
    let mut index = base();
    let outcome = index
        .apply_usn(
            &[
                record(40, ROOT, 1, USN_REASON_SECURITY_CHANGE, DIR, "문서"),
                // 파일의 보안이 바뀌면 그 파일이 든 폴더를 알린다. 같은 폴더는 한 번만.
                record(50, 40, 2, USN_REASON_SECURITY_CHANGE, FILE, "report.txt"),
                record(50, 41, 3, USN_REASON_SECURITY_CHANGE, FILE, "elsewhere"),
            ]
            .concat(),
        )
        .unwrap();
    assert_eq!(
        outcome.security_changes,
        [
            SecurityChange {
                folder: 40,
                path: Some(r"C:\문서".to_owned())
            },
            SecurityChange {
                folder: 41,
                path: Some(r"C:\사진".to_owned())
            },
        ]
    );
    assert_eq!(outcome.records, 3);
}

#[test]
fn records_without_security_change_report_nothing() {
    let mut index = base();
    let outcome = index.apply_usn(&create(60, 41, 1, FILE, "a.txt")).unwrap();
    assert!(outcome.security_changes.is_empty());
}

#[test]
fn broken_bytes_are_errors_and_change_nothing() {
    let mut index = base();
    let good = create(60, 41, 1, FILE, "a.txt");

    let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();
    cases.push(("truncated", good[..good.len() - 8].to_vec()));
    cases.push(("tiny", vec![1, 2, 3]));
    cases.push(("zero length", vec![0; 64]));
    let mut long = good.clone();
    long[0..4].copy_from_slice(&u32::MAX.to_le_bytes());
    cases.push(("length overflow", long));
    let mut odd = good.clone();
    odd[0..4].copy_from_slice(&61u32.to_le_bytes());
    cases.push(("unaligned length", odd));
    let mut version = good.clone();
    version[4..6].copy_from_slice(&3u16.to_le_bytes());
    cases.push(("version", version));
    let mut name_out = good.clone();
    name_out[58..60].copy_from_slice(&u16::MAX.to_le_bytes());
    cases.push(("name offset", name_out));
    let mut name_odd = good.clone();
    name_odd[56..58].copy_from_slice(&3u16.to_le_bytes());
    cases.push(("odd name length", name_odd));
    let mut no_name = good.clone();
    no_name[56..58].copy_from_slice(&0u16.to_le_bytes());
    cases.push(("empty name", no_name));
    cases.push(("slash in name", create(60, 41, 1, FILE, r"a\b")));
    // 좋은 레코드 뒤의 깨진 레코드: 앞 레코드도 적용하지 않는다.
    cases.push(("good then bad", [good.clone(), vec![9; 5]].concat()));

    for (what, bytes) in cases {
        assert!(index.apply_usn(&bytes).is_err(), "{what}");
    }
    assert_eq!(paths(&index, "a.txt"), Vec::<String>::new());
    assert_eq!(index.len(), 3);

    assert_eq!(
        index.apply_usn(&[1, 2, 3]).unwrap_err(),
        UsnError::Truncated { offset: 0 }
    );
    assert!(index.apply_usn(&[]).is_ok());
}

#[test]
fn arbitrary_garbage_never_panics() {
    let mut index = base();
    let mut seed = 0x1234_5678_9abc_def0u64;
    for _ in 0..2000 {
        let len = {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 33) as usize % 200
        };
        let mut bytes: Vec<u8> = (0..len)
            .map(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                (seed >> 40) as u8
            })
            .collect();
        // 헤더처럼 보이게 일부를 맞춘다.
        if bytes.len() >= 8 && seed.is_multiple_of(2) {
            let whole = (bytes.len() as u32 & !7).to_le_bytes();
            bytes[0..4].copy_from_slice(&whole);
            bytes[4..6].copy_from_slice(&2u16.to_le_bytes());
        }
        let _ = index.apply_usn(&bytes);
    }
}
