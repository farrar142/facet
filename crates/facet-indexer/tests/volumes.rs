//! 볼륨 여럿의 인덱스([`Volumes`]): 상태 질의, 볼륨을 합친 질의·정렬·이어 받기, 만드는 중 알림,
//! 볼륨 분류. 관리자 권한이 필요 없다.

use facet_index::{ATTRIBUTE_DIRECTORY, Index, NewItem};
use facet_indexer::session::Cancel;
use facet_indexer::volumes::{Plan, Volumes, classify, drive_type};
use facet_protocol::{
    Case, Condition, ErrorKind, NoIndexReason, Order, QueryMessage, SortKey, VolumeState,
    VolumeStatus,
};

const ROOT: u64 = 5;

/// `volume`에 `files`(이름, 크기)를 `폴더` 아래 넣은 인덱스.
fn index(volume: &str, files: &[(&str, u64)]) -> Index {
    let mut index = Index::new(volume, ROOT);
    let mut put = |number, parent, name: &str, size, attributes| {
        index
            .insert(NewItem {
                number,
                parent,
                name,
                size,
                modified: 1000 + size,
                attributes,
            })
            .unwrap();
    };
    put(100, ROOT, "폴더", 0, ATTRIBUTE_DIRECTORY);
    for (n, (name, size)) in files.iter().enumerate() {
        put(200 + n as u64, 100, name, *size, 0x20);
    }
    index
}

fn query(part: &str, sort: Option<(SortKey, Order)>, limit: Option<u32>) -> QueryMessage {
    QueryMessage {
        condition: Condition::NameContains {
            part: part.into(),
            case: Case::Insensitive,
        },
        sort,
        limit,
    }
}

fn paths(hits: &[facet_index::Hit]) -> Vec<String> {
    hits.iter().map(|h| h.path.clone()).collect()
}

/// 두 볼륨: C:에 a(30)·C(10)·e(50), D:에 B(20)·d(40).
fn two_volumes() -> Volumes {
    let volumes = Volumes::new();
    volumes.set_ready("D:", index("D:", &[("B.txt", 20), ("d.txt", 40)]));
    volumes.set_ready(
        "C:",
        index("C:", &[("a.txt", 30), ("C.txt", 10), ("e.txt", 50)]),
    );
    volumes
}

#[test]
fn status_lists_volumes_in_name_order_with_state_and_items() {
    let volumes = two_volumes();
    volumes.set_not_indexed(
        "E:",
        NoIndexReason::NotNtfs {
            filesystem: "exFAT".into(),
        },
    );
    volumes.set_not_indexed("Z:", NoIndexReason::Network);
    volumes.set_building("F:");
    assert_eq!(
        volumes.status().volumes,
        vec![
            VolumeStatus {
                volume: "C:".into(),
                state: VolumeState::Ready { items: 4 }
            },
            VolumeStatus {
                volume: "D:".into(),
                state: VolumeState::Ready { items: 3 }
            },
            VolumeStatus {
                volume: "E:".into(),
                state: VolumeState::NotIndexed(NoIndexReason::NotNtfs {
                    filesystem: "exFAT".into()
                })
            },
            VolumeStatus {
                volume: "F:".into(),
                state: VolumeState::Building { items: None }
            },
            VolumeStatus {
                volume: "Z:".into(),
                state: VolumeState::NotIndexed(NoIndexReason::Network)
            },
        ]
    );
}

#[test]
fn query_spans_volumes_and_counts_all_matches() {
    let volumes = two_volumes();
    let page = volumes
        .search(&query(".txt", None, None), &Cancel::new())
        .unwrap();
    assert_eq!(page.total, 5);
    assert!(page.next.is_none());
    assert!(page.building.is_empty());
    // 정렬이 없으면 볼륨 이름 순서로 잇는다.
    assert_eq!(
        paths(&page.hits),
        [
            r"C:\폴더\a.txt",
            r"C:\폴더\C.txt",
            r"C:\폴더\e.txt",
            r"D:\폴더\B.txt",
            r"D:\폴더\d.txt"
        ]
    );
}

#[test]
fn sorted_queries_merge_volumes_in_sort_order() {
    let volumes = two_volumes();
    let cases: [((SortKey, Order), [&str; 5]); 5] = [
        (
            (SortKey::Name, Order::Ascending),
            ["a.txt", "B.txt", "C.txt", "d.txt", "e.txt"],
        ),
        (
            (SortKey::Name, Order::Descending),
            ["e.txt", "d.txt", "C.txt", "B.txt", "a.txt"],
        ),
        (
            (SortKey::Size, Order::Ascending),
            ["C.txt", "B.txt", "a.txt", "d.txt", "e.txt"],
        ),
        (
            (SortKey::Modified, Order::Descending),
            ["e.txt", "d.txt", "a.txt", "B.txt", "C.txt"],
        ),
        (
            (SortKey::Path, Order::Descending),
            ["d.txt", "B.txt", "e.txt", "C.txt", "a.txt"],
        ),
    ];
    for (sort, want) in cases {
        let page = volumes
            .search(&query(".txt", Some(sort), None), &Cancel::new())
            .unwrap();
        let names: Vec<&str> = page.hits.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, want, "{sort:?}");
    }
}

#[test]
fn limit_applies_to_merged_results_and_resume_continues_in_order() {
    let volumes = two_volumes();
    let sort = Some((SortKey::Size, Order::Descending));
    let first = volumes
        .search(&query(".txt", sort, Some(2)), &Cancel::new())
        .unwrap();
    assert_eq!(first.total, 5);
    let mut sizes: Vec<u64> = first.hits.iter().map(|h| h.size).collect();
    let mut next = first.next;
    while let Some(cursor) = next {
        let page = volumes.resume(cursor, 2).unwrap();
        assert_eq!(page.total, 5);
        assert!(page.hits.len() <= 2);
        sizes.extend(page.hits.iter().map(|h| h.size));
        next = page.next;
    }
    assert_eq!(sizes, [50, 40, 30, 20, 10]);
}

#[test]
fn building_volume_answers_with_previous_index_and_is_reported() {
    let volumes = two_volumes();
    volumes.set_building("D:");
    assert_eq!(
        volumes.status().volumes[1].state,
        VolumeState::Building { items: Some(3) }
    );
    let page = volumes
        .search(&query("d.txt", None, None), &Cancel::new())
        .unwrap();
    assert_eq!(paths(&page.hits), [r"D:\폴더\d.txt"]);
    assert_eq!(page.building, ["D:"]);

    // 처음 만드는 볼륨은 결과 없이 「만드는 중」만 알린다.
    volumes.set_building("G:");
    let page = volumes
        .search(&query("d.txt", None, None), &Cancel::new())
        .unwrap();
    assert_eq!(page.building, ["D:", "G:"]);

    volumes.set_ready("D:", index("D:", &[("d.txt", 1)]));
    let page = volumes
        .search(&query("d.txt", None, None), &Cancel::new())
        .unwrap();
    assert_eq!(page.building, ["G:"]);
    assert_eq!(page.hits[0].size, 1);
}

#[test]
fn replacing_an_index_makes_cursors_stale() {
    let volumes = two_volumes();
    let page = volumes
        .search(&query(".txt", None, Some(1)), &Cancel::new())
        .unwrap();
    // 만드는 중이 되어도 앞선 인덱스로 답하는 동안은 표지가 산다.
    volumes.set_building("D:");
    let page = volumes.resume(page.next.unwrap(), 1).unwrap();
    volumes.set_ready("D:", index("D:", &[("d.txt", 1)]));
    let err = volumes.resume(page.next.unwrap(), 1).unwrap_err();
    assert_eq!(err.kind, ErrorKind::StaleCursor);
}

#[test]
fn updating_an_index_shows_changes_and_makes_cursors_stale() {
    let volumes = two_volumes();
    let page = volumes
        .search(&query(".txt", None, Some(1)), &Cancel::new())
        .unwrap();
    let cursor = page.next.unwrap();

    let updated = volumes.update("D:", |index| {
        index
            .insert(NewItem {
                number: 300,
                parent: 100,
                name: "new.txt",
                size: 7,
                modified: 1,
                attributes: 0x20,
            })
            .unwrap();
        index.len()
    });
    assert_eq!(updated, Some(4));
    let page = volumes
        .search(&query("new.txt", None, None), &Cancel::new())
        .unwrap();
    assert_eq!(paths(&page.hits), [r"D:\폴더\new.txt"]);
    assert_eq!(
        volumes.status().volumes[1].state,
        VolumeState::Ready { items: 4 }
    );

    // 바뀐 볼륨의 결과를 들고 있던 표지는 무효다.
    let err = volumes.resume(cursor, 10).unwrap_err();
    assert_eq!(err.kind, ErrorKind::StaleCursor);
}

#[test]
fn only_ready_volumes_are_updated() {
    let volumes = two_volumes();
    volumes.set_building("D:");
    assert_eq!(volumes.update("D:", |_| ()), None);
    assert_eq!(volumes.update("Q:", |_| ()), None);
    volumes.set_not_indexed("C:", NoIndexReason::Network);
    assert_eq!(volumes.update("C:", |_| ()), None);
}

#[test]
fn not_indexed_volume_has_no_results() {
    let volumes = two_volumes();
    volumes.set_not_indexed(
        "D:",
        NoIndexReason::Failed {
            message: "읽기 실패".into(),
        },
    );
    let page = volumes
        .search(&query(".txt", None, None), &Cancel::new())
        .unwrap();
    assert_eq!(page.total, 3);
    assert!(page.hits.iter().all(|h| h.path.starts_with("C:")));
}

#[test]
fn bad_regex_is_invalid_query() {
    let volumes = two_volumes();
    let q = QueryMessage {
        condition: Condition::NameRegex {
            pattern: "(".into(),
            case: Case::Insensitive,
        },
        sort: None,
        limit: None,
    };
    assert_eq!(
        volumes.search(&q, &Cancel::new()).unwrap_err().kind,
        ErrorKind::InvalidQuery
    );
}

#[test]
fn classify_indexes_only_local_ntfs() {
    const FIXED: u32 = 3;
    const REMOVABLE: u32 = 2;
    const CDROM: u32 = 5;
    assert_eq!(classify(FIXED, Some("NTFS")), Some(Plan::Index));
    assert_eq!(classify(REMOVABLE, Some("NTFS")), Some(Plan::Index));
    for fs in ["FAT32", "exFAT", "ReFS", "FAT"] {
        assert_eq!(
            classify(FIXED, Some(fs)),
            Some(Plan::NotIndexed(NoIndexReason::NotNtfs {
                filesystem: fs.into()
            })),
            "{fs}"
        );
    }
    assert_eq!(
        classify(CDROM, Some("UDF")),
        Some(Plan::NotIndexed(NoIndexReason::NotNtfs {
            filesystem: "UDF".into()
        }))
    );
    // 네트워크 드라이브는 파일 시스템이 NTFS로 보여도 인덱스를 만들지 않는다.
    assert_eq!(
        classify(drive_type::REMOTE, Some("NTFS")),
        Some(Plan::NotIndexed(NoIndexReason::Network))
    );
    assert_eq!(
        classify(drive_type::REMOTE, None),
        Some(Plan::NotIndexed(NoIndexReason::Network))
    );
    // 매체가 없거나 볼륨이 붙어 있지 않은 드라이브는 목록에서 뺀다.
    assert_eq!(classify(REMOVABLE, None), None);
    assert_eq!(classify(drive_type::NO_ROOT_DIR, Some("NTFS")), None);
    assert_eq!(classify(drive_type::UNKNOWN, Some("NTFS")), None);
}
