//! 질의 결과의 폴더 권한 거르기([`Volumes::search_as`], [`AccessCache`])를 가짜 사용자로 확인한다.
//! 실제 폴더 권한으로 확인하는 테스트는 `tests/permission.rs`. 관리자 권한이 필요 없다.

use std::collections::HashSet;
use std::sync::Mutex;

use facet_index::{
    ATTRIBUTE_DIRECTORY, Index, NewItem, USN_REASON_FILE_CREATE, USN_REASON_RENAME_NEW_NAME,
    USN_REASON_RENAME_OLD_NAME, USN_REASON_SECURITY_CHANGE,
};
use facet_indexer::access::{AccessCache, Caller, Everyone, ancestors};
use facet_indexer::session::{Backend, Cancel};
use facet_indexer::volumes::Volumes;
use facet_protocol::{Case, Condition, ErrorKind, Order, QueryMessage, SortKey};

const ROOT: u64 = 5;
const DIR: u32 = ATTRIBUTE_DIRECTORY;
const FILE: u32 = 0x20;

/// 가짜 사용자: 막힌 폴더 목록을 테스트가 바꾸고, 물어본 폴더를 적어 둔다.
struct Fake {
    user: Option<String>,
    denied: Mutex<HashSet<String>>,
    asked: Mutex<Vec<String>>,
}

impl Fake {
    fn new(user: &str, denied: &[&str]) -> Self {
        Self {
            user: Some(user.to_owned()),
            denied: Mutex::new(denied.iter().map(|d| (*d).to_owned()).collect()),
            asked: Mutex::new(Vec::new()),
        }
    }

    fn anonymous(denied: &[&str]) -> Self {
        Self {
            user: None,
            ..Self::new("", denied)
        }
    }

    fn allow(&self, folder: &str) {
        self.denied.lock().unwrap().remove(folder);
    }

    fn deny(&self, folder: &str) {
        self.denied.lock().unwrap().insert(folder.to_owned());
    }

    /// 지금까지 물어본 폴더들을 꺼내고 비운다.
    fn take_asked(&self) -> Vec<String> {
        std::mem::take(&mut *self.asked.lock().unwrap())
    }
}

impl Caller for Fake {
    fn user(&self) -> Option<String> {
        self.user.clone()
    }

    fn can_list(&self, folder: &str) -> bool {
        self.asked.lock().unwrap().push(folder.to_owned());
        !self.denied.lock().unwrap().contains(folder)
    }
}

/// `C:` 볼륨: `공개`(10)와 `비밀`(11), `비밀\안쪽`(12). 파일은 `공개`와 `비밀`에 번갈아 넣어
/// 넣은 순서(정렬 없음)로 걸러지면 앞쪽이 듬성듬성 빈다.
fn index(volume: &str, files: usize) -> Index {
    let mut index = Index::new(volume, ROOT);
    let mut put = |number, parent, name: &str, size, attributes| {
        index
            .insert(NewItem {
                number,
                parent,
                name,
                size,
                modified: 99,
                attributes,
            })
            .unwrap();
    };
    put(10, ROOT, "공개", 0, DIR);
    put(11, ROOT, "비밀", 0, DIR);
    put(12, 11, "안쪽", 0, DIR);
    for i in 0..files {
        let parent = match i % 3 {
            0 => 10,
            1 => 11,
            _ => 12,
        };
        put(
            100 + i as u64,
            parent,
            &format!("파일 {i:03}.txt"),
            (files - i) as u64,
            FILE,
        );
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

/// 표지가 다할 때까지 `step`개씩 이어 받은 결과 전체(처음 조각 포함).
fn all_pages(
    volumes: &Volumes,
    caller: &dyn Caller,
    q: &QueryMessage,
    step: usize,
) -> (Vec<String>, Vec<usize>) {
    let mut page = volumes.search_as(q, caller, &Cancel::new()).unwrap();
    let mut got = paths(&page.hits);
    let mut sizes = vec![page.hits.len()];
    while let Some(cursor) = page.next {
        page = volumes.resume_as(cursor, step, caller).unwrap();
        assert!(page.total_is_upper_bound);
        got.extend(paths(&page.hits));
        sizes.push(page.hits.len());
    }
    (got, sizes)
}

/// 거르지 않은 결과에서 `비밀` 아래를 뺀 것.
fn expected_visible(volumes: &Volumes, q: &QueryMessage) -> Vec<String> {
    let all = volumes
        .search(
            &QueryMessage {
                limit: None,
                ..q.clone()
            },
            &Cancel::new(),
        )
        .unwrap();
    paths(&all.hits)
        .into_iter()
        .filter(|p| !p.contains(r"\비밀\"))
        .collect()
}

#[test]
fn ancestors_are_the_volume_root_and_every_folder_above_the_item() {
    assert_eq!(
        ancestors(r"C:\a\b\c.txt").collect::<Vec<_>>(),
        [r"C:\", r"C:\a", r"C:\a\b"]
    );
    assert_eq!(ancestors(r"C:\top.txt").collect::<Vec<_>>(), [r"C:\"]);
}

#[test]
fn items_under_a_folder_the_user_cannot_list_are_not_returned() {
    let volumes = Volumes::with_index("C:", index("C:", 30));
    let caller = Fake::new("S-1-5-21-1", &[r"C:\비밀"]);
    let q = query(".txt", None, None);
    // 파이프로 받은 질의는 늘 질의한 사용자로 거른다(Backend).
    let page = Backend::search(&volumes, &q, &caller, &Cancel::new()).unwrap();
    let got = paths(&page.hits);
    assert_eq!(got.len(), 10);
    assert!(got.iter().all(|p| p.starts_with(r"C:\공개\")), "{got:?}");
    // 폴더 자신의 이름은 그 폴더를 담은 폴더의 목록이므로 보인다.
    let folders = volumes
        .search_as(&query("비밀", None, None), &caller, &Cancel::new())
        .unwrap();
    assert_eq!(paths(&folders.hits), [r"C:\비밀"]);
}

#[test]
fn filtered_results_are_refilled_from_later_candidates_up_to_the_limit() {
    let volumes = Volumes::with_index("C:", index("C:", 60));
    let caller = Fake::new("S-1-5-21-1", &[r"C:\비밀"]);
    let q = query(".txt", None, Some(4));
    let page = volumes.search_as(&q, &caller, &Cancel::new()).unwrap();
    assert_eq!(page.hits.len(), 4, "걸러진 만큼 채우지 않음");
    // 전체 개수는 거르기 전의 개수이고 그렇다고 표시한다.
    assert_eq!(page.total, 60);
    assert!(page.total_is_upper_bound);
    let unfiltered = volumes.search(&q, &Cancel::new()).unwrap();
    assert_eq!(unfiltered.total, 60);
    assert!(!unfiltered.total_is_upper_bound);

    // 이어 받아도 걸러지고, 빠지거나 겹치는 결과 없이 거르지 않은 순서를 지킨다.
    let (got, sizes) = all_pages(&volumes, &caller, &q, 3);
    assert_eq!(got, expected_visible(&volumes, &q));
    assert_eq!(got.len(), 20);
    // 마지막 조각 말고는 모두 꽉 찬다.
    assert!(
        sizes[..sizes.len() - 1].iter().skip(1).all(|&n| n == 3),
        "{sizes:?}"
    );
}

#[test]
fn sorted_queries_across_volumes_are_filtered_in_sort_order() {
    let volumes = Volumes::new();
    volumes.set_ready("C:", index("C:", 20));
    volumes.set_ready("D:", index("D:", 20));
    let caller = Fake::new("S-1-5-21-1", &[r"C:\비밀", r"D:\비밀"]);
    for sort in [
        (SortKey::Name, Order::Ascending),
        (SortKey::Size, Order::Descending),
        (SortKey::Path, Order::Descending),
    ] {
        let q = query(".txt", Some(sort), Some(5));
        let (got, _) = all_pages(&volumes, &caller, &q, 4);
        assert_eq!(got, expected_visible(&volumes, &q), "{sort:?}");
    }
}

#[test]
fn everything_hidden_gives_no_hits_and_no_cursor() {
    let volumes = Volumes::with_index("C:", index("C:", 30));
    let caller = Fake::new("S-1-5-21-1", &[r"C:\"]);
    let page = volumes
        .search_as(&query(".txt", None, Some(5)), &caller, &Cancel::new())
        .unwrap();
    assert!(page.hits.is_empty());
    assert!(page.next.is_none());
    assert_eq!(page.total, 30);
}

#[test]
fn a_cursor_resumes_only_the_way_it_was_made() {
    let volumes = Volumes::with_index("C:", index("C:", 30));
    let caller = Fake::new("S-1-5-21-1", &[r"C:\비밀"]);
    let page = volumes
        .search(&query(".txt", None, Some(1)), &Cancel::new())
        .unwrap();
    let err = volumes
        .resume_as(page.next.unwrap(), 5, &caller)
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::StaleCursor);
}

#[test]
fn folder_answers_are_cached_per_user() {
    let volumes = Volumes::with_index("C:", index("C:", 30));
    let caller = Fake::new("S-1-5-21-1", &[r"C:\비밀"]);
    let q = query(".txt", None, None);
    volumes.search_as(&q, &caller, &Cancel::new()).unwrap();
    let asked = caller.take_asked();
    let unique: HashSet<&String> = asked.iter().collect();
    assert_eq!(
        asked.len(),
        unique.len(),
        "같은 폴더를 두 번 물음: {asked:?}"
    );
    assert!(!volumes.access().is_empty());

    // 같은 사용자의 다음 질의는 기억으로 답한다.
    volumes.search_as(&q, &caller, &Cancel::new()).unwrap();
    assert!(caller.take_asked().is_empty());

    // 다른 사용자는 따로 묻는다.
    let other = Fake::new("S-1-5-21-2", &[]);
    let page = volumes.search_as(&q, &other, &Cancel::new()).unwrap();
    assert_eq!(page.hits.len(), 30);
    assert!(!other.take_asked().is_empty());

    // 사용자를 모르면 기억하지 않고 매번 묻는다.
    let anonymous = Fake::anonymous(&[r"C:\비밀"]);
    volumes.search_as(&q, &anonymous, &Cancel::new()).unwrap();
    let first = anonymous.take_asked().len();
    volumes.search_as(&q, &anonymous, &Cancel::new()).unwrap();
    assert_eq!(anonymous.take_asked().len(), first);
    assert!(first > 0);
}

/// `USN_RECORD_V2` 하나의 바이트.
fn record(number: u64, parent: u64, usn: i64, reason: u32, attributes: u32, name: &str) -> Vec<u8> {
    let units: Vec<u16> = name.encode_utf16().collect();
    let length = (60 + units.len() * 2).next_multiple_of(8);
    let mut b = vec![0u8; length];
    b[0..4].copy_from_slice(&(length as u32).to_le_bytes());
    b[4..6].copy_from_slice(&2u16.to_le_bytes());
    b[8..16].copy_from_slice(&number.to_le_bytes());
    b[16..24].copy_from_slice(&parent.to_le_bytes());
    b[24..32].copy_from_slice(&usn.to_le_bytes());
    b[40..44].copy_from_slice(&reason.to_le_bytes());
    b[52..56].copy_from_slice(&attributes.to_le_bytes());
    b[56..58].copy_from_slice(&((units.len() * 2) as u16).to_le_bytes());
    b[58..60].copy_from_slice(&60u16.to_le_bytes());
    for (i, u) in units.iter().enumerate() {
        b[60 + i * 2..62 + i * 2].copy_from_slice(&u.to_le_bytes());
    }
    b
}

/// USN 레코드들을 적용하고 그 결과로 권한 기억을 버린다(인덱서 서비스의 따라가기와 같다).
fn apply(volumes: &Volumes, records: &[Vec<u8>]) {
    let outcome = volumes
        .update("C:", |index| index.apply_usn(&records.concat()))
        .unwrap()
        .unwrap();
    volumes.forget_access(&outcome);
}

fn visible(volumes: &Volumes, caller: &Fake, part: &str) -> Vec<String> {
    let page = volumes
        .search_as(&query(part, None, None), caller, &Cancel::new())
        .unwrap();
    let mut got = paths(&page.hits);
    got.sort();
    got
}

#[test]
fn a_security_change_drops_the_cached_answers_under_that_folder() {
    let volumes = Volumes::with_index("C:", index("C:", 3));
    let caller = Fake::new("S-1-5-21-1", &[r"C:\비밀"]);
    assert_eq!(
        visible(&volumes, &caller, ".txt"),
        [r"C:\공개\파일 000.txt"]
    );

    // 권한이 바뀌어도 기억으로 답하다가...
    caller.allow(r"C:\비밀");
    assert_eq!(visible(&volumes, &caller, ".txt").len(), 1);
    caller.take_asked();

    // ...보안 변경 레코드가 오면 그 폴더 아래 기억만 버려 바뀐 권한이 반영된다.
    apply(
        &volumes,
        &[record(11, ROOT, 1, USN_REASON_SECURITY_CHANGE, DIR, "비밀")],
    );
    assert_eq!(
        visible(&volumes, &caller, ".txt"),
        [
            r"C:\공개\파일 000.txt",
            r"C:\비밀\안쪽\파일 002.txt",
            r"C:\비밀\파일 001.txt"
        ]
    );
    let asked = caller.take_asked();
    assert!(asked.contains(&r"C:\비밀".to_owned()), "{asked:?}");
    assert!(!asked.contains(&r"C:\공개".to_owned()), "{asked:?}");

    // 반대로 막혀도 반영된다. 안쪽 폴더의 보안만 바뀌면 그 아래만 다시 묻는다.
    caller.deny(r"C:\비밀\안쪽");
    apply(
        &volumes,
        &[record(12, 11, 2, USN_REASON_SECURITY_CHANGE, DIR, "안쪽")],
    );
    assert_eq!(
        visible(&volumes, &caller, ".txt"),
        [r"C:\공개\파일 000.txt", r"C:\비밀\파일 001.txt"]
    );
    assert_eq!(caller.take_asked(), [r"C:\비밀\안쪽"]);
}

#[test]
fn a_different_folder_placed_at_a_cached_path_is_checked_again() {
    let volumes = Volumes::with_index("C:", index("C:", 3));
    let caller = Fake::new("S-1-5-21-1", &[r"C:\비밀"]);
    assert_eq!(visible(&volumes, &caller, ".txt").len(), 1);

    // 「비밀」을 다른 이름으로 옮기고 같은 이름의 새 폴더를 만든다. 새 폴더는 읽을 수 있다.
    caller.allow(r"C:\비밀");
    caller.deny(r"C:\옛 비밀");
    apply(
        &volumes,
        &[
            record(11, ROOT, 1, USN_REASON_RENAME_OLD_NAME, DIR, "비밀"),
            record(11, ROOT, 2, USN_REASON_RENAME_NEW_NAME, DIR, "옛 비밀"),
            record(13, ROOT, 3, USN_REASON_FILE_CREATE, DIR, "비밀"),
            record(300, 13, 4, USN_REASON_FILE_CREATE, FILE, "새 파일.txt"),
        ],
    );
    assert_eq!(
        visible(&volumes, &caller, ".txt"),
        [r"C:\공개\파일 000.txt", r"C:\비밀\새 파일.txt"]
    );
}

#[test]
fn rebuilding_an_index_forgets_every_answer() {
    let volumes = Volumes::with_index("C:", index("C:", 3));
    let caller = Fake::new("S-1-5-21-1", &[]);
    visible(&volumes, &caller, ".txt");
    assert!(!volumes.access().is_empty());
    volumes.set_ready("C:", index("C:", 3));
    assert!(volumes.access().is_empty());
}

#[test]
fn forgetting_matches_paths_without_regard_to_case_and_only_below_the_folder() {
    let cache = AccessCache::new();
    let caller = Fake::new("S-1-5-21-1", &[]);
    for path in [r"C:\Docs\a.txt", r"C:\Docs\Inner\b.txt", r"C:\Docs2\c.txt"] {
        assert!(cache.visible(&caller, path));
    }
    caller.take_asked();
    cache.forget_under(r"c:\DOCS");
    for path in [r"C:\Docs\a.txt", r"C:\Docs\Inner\b.txt", r"C:\Docs2\c.txt"] {
        assert!(cache.visible(&caller, path));
    }
    assert_eq!(caller.take_asked(), [r"C:\Docs", r"C:\Docs\Inner"]);

    // 볼륨 루트를 버리면 그 볼륨 전체다.
    cache.forget_under(r"C:\");
    assert!(cache.is_empty());
}

#[test]
fn everyone_sees_everything() {
    let volumes = Volumes::with_index("C:", index("C:", 9));
    let page = volumes
        .search_as(&query(".txt", None, None), &Everyone, &Cancel::new())
        .unwrap();
    assert_eq!(page.hits.len(), 9);
}
