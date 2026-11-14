//! 질의 결과의 정렬, 결과 수 제한과 전체 개수, 이어 받기를 합성 인덱스에 대한
//! 공개 인터페이스로만 확인한다.

use std::cmp::Ordering;

use facet_index::{
    ATTRIBUTE_DIRECTORY, Case, Filter, Hit, Index, NewItem, Order, Page, Query, ResumeError,
    SortKey,
};

const ROOT: u64 = 5;

/// 폴더 몇 개 아래 파일 `files`개가 있는 합성 인덱스. 크기·날짜·이름이 겹치는 항목이 있고,
/// 부모를 찾을 수 없는 항목도 하나 있다.
fn synthetic(files: u64) -> Index {
    let mut index = Index::new(r"C:\", ROOT);
    let folders = ["가 폴더", "Alpha", "alpha b", "Ä 폴더", "z"];
    for (n, name) in folders.iter().enumerate() {
        index
            .insert(NewItem {
                number: 10 + n as u64,
                parent: ROOT,
                name,
                size: 0,
                modified: 1_000,
                attributes: ATTRIBUTE_DIRECTORY,
            })
            .unwrap();
    }
    // 폴더 안의 폴더 하나: C:\Alpha\Sub
    index
        .insert(NewItem {
            number: 20,
            parent: 11,
            name: "Sub",
            size: 0,
            modified: 2_000,
            attributes: ATTRIBUTE_DIRECTORY,
        })
        .unwrap();
    for i in 0..files {
        let parent = [10, 11, 12, 13, 14, 20][(i % 6) as usize];
        // 이름은 17가지만 돌아가며 쓰므로 같은 이름이 여러 폴더에 있다.
        let name = match i % 17 {
            0 => "보고서.txt".to_owned(),
            1 => "README.md".to_owned(),
            2 => "readme.md".to_owned(),
            3 => "Ärger.txt".to_owned(),
            4 => "🎉 파티.jpg".to_owned(),
            k => format!("file {k}.txt"),
        };
        index
            .insert(NewItem {
                number: 100 + i,
                parent,
                name: &name,
                size: (i * 37) % 11,
                modified: 5_000 + (i * 13) % 7,
                attributes: 0x20,
            })
            .unwrap();
    }
    // 부모를 찾을 수 없는 항목은 결과에도 전체 개수에도 들지 않는다.
    index
        .insert(NewItem {
            number: 9_999,
            parent: 12_345,
            name: "고아.txt",
            size: 1,
            modified: 1,
            attributes: 0x20,
        })
        .unwrap();
    index
}

fn lower(s: &str) -> String {
    s.chars().flat_map(char::to_lowercase).collect()
}

/// 정렬 기준에서 두 결과의 순서(같은 값끼리의 순서는 따지지 않는다).
fn key_order(key: SortKey, a: &Hit, b: &Hit) -> Ordering {
    match key {
        SortKey::Name => lower(&a.name).cmp(&lower(&b.name)),
        SortKey::Path => {
            let parts = |h: &Hit| -> Vec<String> { h.path.split('\\').map(lower).collect() };
            parts(a).cmp(&parts(b))
        }
        SortKey::Size => a.size.cmp(&b.size),
        SortKey::Modified => a.modified.cmp(&b.modified),
    }
}

const KEYS: [SortKey; 4] = [
    SortKey::Name,
    SortKey::Path,
    SortKey::Size,
    SortKey::Modified,
];
const ORDERS: [Order; 2] = [Order::Ascending, Order::Descending];

fn sorted_paths(hits: &[Hit]) -> Vec<String> {
    let mut v: Vec<String> = hits.iter().map(|h| h.path.clone()).collect();
    v.sort();
    v
}

/// 이어 받기 표지를 따라 끝까지 받아 이은 결과.
fn all_pages(index: &Index, first: Page, size: usize) -> Vec<Hit> {
    let total = first.total;
    let mut hits = first.hits;
    let mut next = first.next;
    while let Some(cursor) = next {
        let page = index
            .resume(cursor, size)
            .expect("인덱스가 그대로면 이어 받는다");
        assert!(page.hits.len() <= size);
        assert_eq!(page.total, total, "이어 받아도 전체 개수는 같다");
        hits.extend(page.hits);
        next = page.next;
    }
    hits
}

#[test]
fn 이름_경로_크기_날짜로_오름_내림_정렬한다() {
    let index = synthetic(200);
    let unsorted = index.search(&Query::name_contains(""));
    for key in KEYS {
        for order in ORDERS {
            let hits = index
                .search(&Query::name_contains("").sort(key, order))
                .hits;
            assert_eq!(
                sorted_paths(&hits),
                sorted_paths(&unsorted.hits),
                "{key:?} {order:?}: 같은 항목들이다"
            );
            for pair in hits.windows(2) {
                let o = key_order(key, &pair[0], &pair[1]);
                let wrong = match order {
                    Order::Ascending => Ordering::Greater,
                    Order::Descending => Ordering::Less,
                };
                assert_ne!(
                    o, wrong,
                    "{key:?} {order:?}: {:?} 다음에 {:?}",
                    pair[0], pair[1]
                );
            }
        }
    }
}

#[test]
fn 이름_정렬은_대소문자를_무시한다() {
    let index = synthetic(0);
    let names = |order| -> Vec<String> {
        index
            .search(&Query::name_contains("").sort(SortKey::Name, order))
            .hits
            .into_iter()
            .map(|h| h.name)
            .collect()
    };
    assert_eq!(
        names(Order::Ascending),
        vec!["Alpha", "alpha b", "Sub", "z", "Ä 폴더", "가 폴더"]
    );
    assert_eq!(
        names(Order::Descending),
        vec!["가 폴더", "Ä 폴더", "z", "Sub", "alpha b", "Alpha"]
    );
}

#[test]
fn 경로_정렬은_폴더_안의_항목을_폴더_바로_뒤에_모은다() {
    let index = synthetic(0);
    let paths: Vec<String> = index
        .search(&Query::name_contains("").sort(SortKey::Path, Order::Ascending))
        .hits
        .into_iter()
        .map(|h| h.path)
        .collect();
    // 「C:\alpha b」가 「C:\Alpha\Sub」보다 글자로는 앞이지만(' ' < '\'), 경로 조각으로 비교한다.
    assert_eq!(
        paths,
        vec![
            r"C:\Alpha",
            r"C:\Alpha\Sub",
            r"C:\alpha b",
            r"C:\z",
            r"C:\Ä 폴더",
            r"C:\가 폴더"
        ]
    );
}

#[test]
fn 크기와_날짜가_같은_항목도_빠짐없이_한_번씩_나온다() {
    let index = synthetic(100);
    for key in [SortKey::Size, SortKey::Modified] {
        let hits = index
            .search(&Query::name_contains("").sort(key, Order::Descending))
            .hits;
        assert_eq!(hits.len(), 106);
        let paths = sorted_paths(&hits);
        let mut unique = paths.clone();
        unique.dedup();
        assert_eq!(paths, unique);
    }
}

#[test]
fn 결과_수를_제한해도_전체_개수는_정확하다() {
    let index = synthetic(300);
    // 폴더 6개 + 파일 300개. 고아는 세지 않는다.
    let all = index.search(&Query::name_contains(""));
    assert_eq!(all.total, 306);
    assert_eq!(all.hits.len(), 306);
    assert!(all.next.is_none(), "제한이 없으면 이어 받을 것이 없다");

    let readme = index.search(&Query::name_contains("readme").limit(5));
    // 300개 중 i % 17 이 1 또는 2인 것.
    let expected = (0..300).filter(|i| i % 17 == 1 || i % 17 == 2).count();
    assert_eq!(readme.total, expected);
    assert_eq!(readme.hits.len(), 5);
    assert!(readme.next.is_some());

    for key in KEYS {
        let page = index.search(
            &Query::name_contains("txt")
                .sort(key, Order::Ascending)
                .limit(10),
        );
        let full = index.search(&Query::name_contains("txt"));
        assert_eq!(page.total, full.hits.len(), "{key:?}");
        assert_eq!(page.hits.len(), 10);
    }

    let none = index.search(&Query::name_contains("없는 이름").limit(10));
    assert_eq!(none.total, 0);
    assert!(none.hits.is_empty());
    assert!(none.next.is_none());

    // 제한보다 결과가 적으면 다 오고 이어 받을 것이 없다.
    let few = index.search(&Query::name_contains("sub").limit(10));
    assert_eq!((few.total, few.hits.len()), (1, 1));
    assert!(few.next.is_none());

    // 조건 트리 질의도 같다.
    let filtered = index.search(
        &Query::filter(Filter::and([
            Filter::name_contains("file", Case::Insensitive),
            Filter::size(Some(5), None),
        ]))
        .limit(3),
    );
    let filtered_all = index.search(&Query::filter(Filter::and([
        Filter::name_contains("file", Case::Insensitive),
        Filter::size(Some(5), None),
    ])));
    assert_eq!(filtered.total, filtered_all.hits.len());
    assert_eq!(filtered.hits.len(), 3);
}

#[test]
fn 이어_받은_조각을_이으면_한_번에_받은_정렬_결과와_같다() {
    let index = synthetic(250);
    let queries = || {
        [
            Query::name_contains(""),
            Query::name_contains("e"),
            Query::filter(Filter::not(Filter::extension("txt"))),
        ]
    };
    for make in 0..3 {
        let base = || queries().into_iter().nth(make).unwrap();
        let mut sorts: Vec<Option<(SortKey, Order)>> = vec![None];
        for key in KEYS {
            for order in ORDERS {
                sorts.push(Some((key, order)));
            }
        }
        for sort in sorts {
            let q = || match sort {
                Some((key, order)) => base().sort(key, order),
                None => base(),
            };
            let whole = index.search(&q());
            assert!(whole.next.is_none());
            for size in [1, 3, 7, 50, 1000] {
                let first = index.search(&q().limit(size));
                assert_eq!(first.total, whole.total);
                let joined = all_pages(&index, first, size);
                assert_eq!(joined, whole.hits, "질의 {make}, {sort:?}, 조각 {size}개");
            }
        }
    }
}

#[test]
fn 이어_받을_때_조각_크기를_바꿀_수_있다() {
    let index = synthetic(100);
    let q = || Query::name_contains("").sort(SortKey::Name, Order::Ascending);
    let whole = index.search(&q()).hits;
    let first = index.search(&q().limit(10));
    let mut hits = first.hits;
    let second = index.resume(first.next.unwrap(), 25).unwrap();
    assert_eq!(second.hits.len(), 25);
    hits.extend(second.hits);
    let rest = index.resume(second.next.unwrap(), usize::MAX).unwrap();
    assert!(rest.next.is_none());
    hits.extend(rest.hits);
    assert_eq!(hits, whole);
}

#[test]
fn 결과_수_제한이_0이면_결과_없이_전체_개수와_이어_받기_표지만_온다() {
    let index = synthetic(20);
    let page = index.search(&Query::name_contains("").limit(0));
    assert!(page.hits.is_empty());
    assert_eq!(page.total, 26);
    let rest = all_pages(&index, page, 4);
    assert_eq!(rest, index.search(&Query::name_contains("")).hits);
}

#[test]
fn 인덱스가_바뀐_뒤의_이어_받기_표지는_무효_오류다() {
    let mut index = synthetic(50);
    let page = index.search(
        &Query::name_contains("")
            .sort(SortKey::Size, Order::Ascending)
            .limit(5),
    );
    let cursor = page.next.unwrap();
    index
        .insert(NewItem {
            number: 50_000,
            parent: ROOT,
            name: "새 파일.txt",
            size: 0,
            modified: 0,
            attributes: 0x20,
        })
        .unwrap();
    assert_eq!(index.resume(cursor, 5).unwrap_err(), ResumeError::Stale);
    assert!(!ResumeError::Stale.to_string().is_empty());

    // 다시 질의하면 새 표지로 이어 받는다.
    let again = index.search(&Query::name_contains("").limit(5));
    assert_eq!(again.total, 57);
    assert!(index.resume(again.next.unwrap(), 5).is_ok());
}

#[test]
fn 넣지_못한_항목은_인덱스를_바꾸지_않는다() {
    let mut index = synthetic(10);
    let page = index.search(&Query::name_contains("").limit(2));
    assert!(
        index
            .insert(NewItem {
                number: 1,
                parent: ROOT,
                name: "",
                size: 0,
                modified: 0,
                attributes: 0,
            })
            .is_err()
    );
    assert!(index.resume(page.next.unwrap(), 2).is_ok());
}

#[test]
fn 다른_인덱스의_이어_받기_표지는_무효_오류다() {
    let a = synthetic(30);
    let b = synthetic(30);
    let cursor = a.search(&Query::name_contains("").limit(3)).next.unwrap();
    assert_eq!(b.resume(cursor, 3).unwrap_err(), ResumeError::Stale);
}

#[test]
fn 정렬과_제한을_준_질의도_query는_결과만_돌려준다() {
    let index = synthetic(40);
    let q = Query::name_contains("")
        .sort(SortKey::Modified, Order::Descending)
        .limit(7);
    assert_eq!(index.query(&q), index.search(&q).hits);
}
