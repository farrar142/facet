//! 자세히 보기 정렬과 열 표시 글자.

mod common;

use std::cmp::Ordering;
use std::time::{Duration, SystemTime};

use facet_core::details::{
    Column, DetailsList, Direction, FileType, Sort, compare_names, file_type, modified_text,
    size_text,
};
use facet_core::file_list::format_modified_local;
use facet_core::{Attributes, Item};

use common::names::{ORDERED, SAME_IGNORING_CASE};

fn item(name: &str, is_folder: bool, size: u64, secs: u64) -> Item {
    Item {
        name: name.into(),
        is_folder,
        size,
        modified: SystemTime::UNIX_EPOCH + Duration::from_secs(secs),
        attributes: Attributes::default(),
    }
}

fn file(name: &str, size: u64, secs: u64) -> Item {
    item(name, false, size, secs)
}

fn folder(name: &str, secs: u64) -> Item {
    item(name, true, 0, secs)
}

fn names(list: &DetailsList) -> Vec<String> {
    list.iter()
        .map(|i| i.name.to_string_lossy().into_owned())
        .collect()
}

fn sort(column: Column, direction: Direction) -> Sort {
    Sort { column, direction }
}

// ── 자연 정렬 ──

#[test]
fn 이름_표의_모든_쌍이_표의_순서대로_비교된다() {
    for (i, a) in ORDERED.iter().enumerate() {
        for (j, b) in ORDERED.iter().enumerate() {
            assert_eq!(compare_names(a, b), i.cmp(&j), "{a:?}와 {b:?}");
        }
    }
}

#[test]
fn 대소문자만_다른_이름은_같다() {
    for (a, b) in SAME_IGNORING_CASE {
        assert_eq!(compare_names(a, b), Ordering::Equal, "{a:?}와 {b:?}");
    }
}

#[test]
fn 섞인_이름_표를_자연_정렬한다() {
    let shuffled: Vec<Item> = ORDERED
        .iter()
        .enumerate()
        .map(|(i, n)| (i * 7919 % ORDERED.len(), n))
        .collect::<std::collections::BTreeMap<_, _>>()
        .into_values()
        .map(|n| file(n, 0, 0))
        .collect();
    assert_eq!(shuffled.len(), ORDERED.len());
    let list = DetailsList::new(shuffled, Sort::default());
    assert_eq!(names(&list), ORDERED);
}

#[test]
fn 파일2가_파일10_앞이다() {
    assert_eq!(compare_names("파일2", "파일10"), Ordering::Less);
    assert_eq!(compare_names("파일10", "파일2"), Ordering::Greater);
}

#[test]
fn 큰_숫자_덩어리도_값으로_비교한다() {
    assert_eq!(
        compare_names("99999999999999999999", "100000000000000000000"),
        Ordering::Less
    );
    assert_eq!(
        compare_names("x 0000000000000000000000000001", "x 2"),
        Ordering::Less
    );
}

// ── 열마다 오름·내림, 폴더 먼저 ──

fn sample() -> Vec<Item> {
    vec![
        file("b.txt", 3000, 300),
        folder("폴더10", 100),
        file("a10.md", 10, 200),
        folder("폴더2", 400),
        file("README", 5000, 100),
        file("a2.txt", 3000, 500),
        folder("Alpha", 300),
    ]
}

#[test]
fn 기본_정렬은_이름_오름차순이다() {
    assert_eq!(Sort::default(), sort(Column::Name, Direction::Ascending));
}

#[test]
fn 이름_오름_내림() {
    let mut list = DetailsList::new(sample(), sort(Column::Name, Direction::Ascending));
    assert_eq!(
        names(&list),
        [
            "Alpha", "폴더2", "폴더10", "a2.txt", "a10.md", "b.txt", "README"
        ]
    );
    list.set_sort(sort(Column::Name, Direction::Descending));
    assert_eq!(
        names(&list),
        [
            "폴더10", "폴더2", "Alpha", "README", "b.txt", "a10.md", "a2.txt"
        ]
    );
}

#[test]
fn 수정한_날짜_오름_내림() {
    let mut list = DetailsList::new(sample(), sort(Column::Modified, Direction::Ascending));
    assert_eq!(
        names(&list),
        [
            "폴더10", "Alpha", "폴더2", "README", "a10.md", "b.txt", "a2.txt"
        ]
    );
    list.set_sort(sort(Column::Modified, Direction::Descending));
    assert_eq!(
        names(&list),
        [
            "폴더2", "Alpha", "폴더10", "a2.txt", "b.txt", "a10.md", "README"
        ]
    );
}

#[test]
fn 유형_오름_내림() {
    // 폴더끼리는 유형이 같아 이름으로 가른다. 확장자 없는 파일은 확장자 있는 파일보다 앞이다.
    let mut list = DetailsList::new(sample(), sort(Column::Type, Direction::Ascending));
    assert_eq!(
        names(&list),
        [
            "Alpha", "폴더2", "폴더10", "README", "a10.md", "a2.txt", "b.txt"
        ]
    );
    list.set_sort(sort(Column::Type, Direction::Descending));
    assert_eq!(
        names(&list),
        [
            "Alpha", "폴더2", "폴더10", "a2.txt", "b.txt", "a10.md", "README"
        ]
    );
}

#[test]
fn 크기_오름_내림() {
    // 폴더는 크기를 보이지 않으므로 크기 정렬에서도 이름으로 가른다.
    let mut list = DetailsList::new(sample(), sort(Column::Size, Direction::Ascending));
    assert_eq!(
        names(&list),
        [
            "Alpha", "폴더2", "폴더10", "a10.md", "a2.txt", "b.txt", "README"
        ]
    );
    list.set_sort(sort(Column::Size, Direction::Descending));
    assert_eq!(
        names(&list),
        [
            "Alpha", "폴더2", "폴더10", "README", "a2.txt", "b.txt", "a10.md"
        ]
    );
}

#[test]
fn 폴더는_어느_열_어느_방향이든_파일보다_앞이다() {
    let mut list = DetailsList::new(sample(), Sort::default());
    for column in [Column::Name, Column::Modified, Column::Type, Column::Size] {
        for direction in [Direction::Ascending, Direction::Descending] {
            list.set_sort(sort(column, direction));
            assert_eq!(list.sort(), sort(column, direction));
            let kinds: Vec<bool> = list.iter().map(|i| i.is_folder).collect();
            assert_eq!(
                kinds,
                [true, true, true, false, false, false, false],
                "{column:?} {direction:?}"
            );
        }
    }
}

#[test]
fn 같은_키는_방향과_상관없이_이름_오름차순으로_가른다() {
    let items = vec![
        file("다.txt", 7, 50),
        file("가.txt", 7, 50),
        file("나.txt", 7, 50),
    ];
    let mut list = DetailsList::new(items, Sort::default());
    for column in [Column::Modified, Column::Type, Column::Size] {
        for direction in [Direction::Ascending, Direction::Descending] {
            list.set_sort(sort(column, direction));
            assert_eq!(
                names(&list),
                ["가.txt", "나.txt", "다.txt"],
                "{column:?} {direction:?}"
            );
        }
    }
}

#[test]
fn 대소문자만_다른_이름도_순서가_정해진다() {
    // 대소문자를 구분하는 파일 시스템에서 올 수 있다. 정렬할 때마다 같은 순서여야 한다.
    let a = DetailsList::new(vec![file("abc", 0, 0), file("ABC", 0, 0)], Sort::default());
    let b = DetailsList::new(vec![file("ABC", 0, 0), file("abc", 0, 0)], Sort::default());
    assert_eq!(names(&a), names(&b));
}

#[test]
fn 정렬_바꾸기는_항목을_그대로_두고_순서만_바꾼다() {
    let mut list = DetailsList::new(sample(), Sort::default());
    let mut before: Vec<Item> = list.iter().cloned().collect();
    list.set_sort(sort(Column::Size, Direction::Descending));
    let mut after: Vec<Item> = list.iter().cloned().collect();
    assert_eq!(after.len(), 7);
    before.sort_by(|a, b| a.name.cmp(&b.name));
    after.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(before, after);
    assert_eq!(list.len(), 7);
    assert!(!list.is_empty());
    assert_eq!(list.into_items().len(), 7);
}

#[test]
fn 빈_목록도_정렬한다() {
    let mut list = DetailsList::new(Vec::new(), Sort::default());
    list.set_sort(sort(Column::Size, Direction::Descending));
    assert!(list.is_empty());
}

#[test]
fn 정렬은_다른_스레드에서_부를_수_있다() {
    fn assert_send<T: Send + 'static>(_: &T) {}
    let list = DetailsList::new(sample(), Sort::default());
    assert_send(&list);
    let list = std::thread::spawn(move || {
        let mut list = list;
        list.set_sort(sort(Column::Name, Direction::Descending));
        list
    })
    .join()
    .unwrap();
    assert_eq!(names(&list)[0], "폴더10");
}

// ── 열 표시 글자 ──

#[test]
fn 유형은_폴더_또는_대문자_확장자다() {
    assert_eq!(file_type(&folder("사진.txt", 0)), FileType::Folder);
    assert_eq!(
        file_type(&file("a.txt", 0, 0)),
        FileType::File {
            extension: Some("TXT".into())
        }
    );
    assert_eq!(
        file_type(&file("archive.tar.Gz", 0, 0)),
        FileType::File {
            extension: Some("GZ".into())
        }
    );
    assert_eq!(
        file_type(&file("사진.jpeg", 0, 0)),
        FileType::File {
            extension: Some("JPEG".into())
        }
    );
    assert_eq!(
        file_type(&file(".gitignore", 0, 0)),
        FileType::File {
            extension: Some("GITIGNORE".into())
        }
    );
    assert_eq!(
        file_type(&file("README", 0, 0)),
        FileType::File { extension: None }
    );
    assert_eq!(
        file_type(&file("끝에 점.", 0, 0)),
        FileType::File { extension: None }
    );
}

#[test]
fn 크기는_kb_단위_올림이고_폴더는_비운다() {
    assert_eq!(size_text(&folder("폴더", 0)), "");
    assert_eq!(size_text(&file("a", 0, 0)), "0 KB");
    assert_eq!(size_text(&file("a", 1, 0)), "1 KB");
    assert_eq!(size_text(&file("a", 1024, 0)), "1 KB");
    assert_eq!(size_text(&file("a", 1025, 0)), "2 KB");
    assert_eq!(size_text(&file("a", 999 * 1024, 0)), "999 KB");
    assert_eq!(size_text(&file("a", 1000 * 1024, 0)), "1,000 KB");
    assert_eq!(size_text(&file("a", 1_234_567, 0)), "1,206 KB");
    assert_eq!(
        size_text(&file("a", u64::MAX, 0)),
        "18,014,398,509,481,984 KB"
    );
}

#[test]
fn 날짜는_로컬_시각_yyyy_mm_dd_hh_mm이다() {
    let item = file("a", 0, 1_709_618_820);
    let text = modified_text(&item);
    assert_eq!(text, format_modified_local(item.modified));
    let bytes = text.as_bytes();
    assert_eq!(bytes.len(), 16, "{text}");
    for (i, b) in bytes.iter().enumerate() {
        match i {
            4 | 7 => assert_eq!(*b, b'-', "{text}"),
            10 => assert_eq!(*b, b' ', "{text}"),
            13 => assert_eq!(*b, b':', "{text}"),
            _ => assert!(b.is_ascii_digit(), "{text}"),
        }
    }
}
