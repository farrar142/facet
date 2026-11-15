//! 질의 메시지의 모든 조건·정렬·제한·이어 받기가 인덱스 질의로 바뀌어 인덱스에 직접 건
//! 질의와 같은 결과를 내는지 공개 인터페이스로만 확인한다.

use std::cell::RefCell;
use std::collections::HashMap;

use facet_index::{self as index, ATTRIBUTE_DIRECTORY, ATTRIBUTE_HIDDEN, Filter, Index, NewItem};
use facet_protocol::{
    Case, Condition, ErrorKind, ErrorMessage, HitMessage, Message, Order, PageMessage,
    QueryMessage, Request, ResumeMessage, SortKey,
};

const ROOT: u64 = 5;

fn sample() -> Index {
    let mut index = Index::new(r"C:\", ROOT);
    let items: [(u64, u64, &str, u64, u64, u32); 8] = [
        (10, ROOT, "문서", 0, 100, ATTRIBUTE_DIRECTORY),
        (11, 10, "Report.TXT", 300, 200, 0),
        (12, 10, "2024-보고서.txt", 50, 300, ATTRIBUTE_HIDDEN),
        (13, ROOT, "photo.jpg", 9_000, 400, 0),
        (14, ROOT, "Notes.md", 10, 500, ATTRIBUTE_HIDDEN),
        (15, 10, "sub", 0, 600, ATTRIBUTE_DIRECTORY),
        (16, 15, "deep.rs", 70, 700, 0),
        (17, ROOT, "readme", 1, 800, 0),
    ];
    for (number, parent, name, size, modified, attributes) in items {
        index
            .insert(NewItem {
                number,
                parent,
                name,
                size,
                modified,
                attributes,
            })
            .unwrap();
    }
    index
}

fn paths(hits: &[index::Hit]) -> Vec<String> {
    hits.iter().map(|h| h.path.clone()).collect()
}

/// 메시지를 이진 형식으로 왕복시킨 뒤 인덱스 질의로 바꿔 실행한다.
fn run(index: &Index, message: QueryMessage) -> index::Page {
    let bytes = Request::Query(message).to_bytes();
    let Request::Query(message) = Request::from_bytes(&bytes).unwrap() else {
        unreachable!()
    };
    index.search(&message.to_query().unwrap())
}

fn condition_only(condition: Condition) -> QueryMessage {
    QueryMessage {
        condition,
        sort: None,
        limit: None,
    }
}

#[test]
fn every_condition_becomes_the_same_index_filter() {
    let index = sample();
    let cases: Vec<(Condition, Filter)> = vec![
        (
            Condition::NameContains {
                part: "report".into(),
                case: Case::Insensitive,
            },
            Filter::name_contains("report", index::Case::Insensitive),
        ),
        (
            Condition::NameContains {
                part: "report".into(),
                case: Case::Sensitive,
            },
            Filter::name_contains("report", index::Case::Sensitive),
        ),
        (
            Condition::NameWildcard {
                pattern: "*.t?t".into(),
                case: Case::Insensitive,
            },
            Filter::name_wildcard("*.t?t", index::Case::Insensitive).unwrap(),
        ),
        (
            Condition::NameWildcard {
                pattern: "*.t?t".into(),
                case: Case::Sensitive,
            },
            Filter::name_wildcard("*.t?t", index::Case::Sensitive).unwrap(),
        ),
        (
            Condition::NameRegex {
                pattern: r"^\d{4}-".into(),
                case: Case::Insensitive,
            },
            Filter::name_regex(r"^\d{4}-", index::Case::Insensitive).unwrap(),
        ),
        (
            Condition::Under {
                folder: r"C:\문서".into(),
            },
            Filter::under(r"C:\문서"),
        ),
        (
            Condition::Size {
                min: Some(50),
                max: Some(300),
            },
            Filter::size(Some(50), Some(300)),
        ),
        (
            Condition::Modified {
                min: Some(300),
                max: None,
            },
            Filter::modified(Some(300), None),
        ),
        (
            Condition::Attributes {
                mask: ATTRIBUTE_HIDDEN,
            },
            Filter::attributes(ATTRIBUTE_HIDDEN),
        ),
        (
            Condition::Extension {
                extension: "TXT".into(),
            },
            Filter::extension("TXT"),
        ),
        (
            Condition::And(vec![
                Condition::Under {
                    folder: r"C:\문서".into(),
                },
                Condition::Not(Box::new(Condition::Attributes {
                    mask: ATTRIBUTE_DIRECTORY,
                })),
            ]),
            Filter::and([
                Filter::under(r"C:\문서"),
                Filter::not(Filter::attributes(ATTRIBUTE_DIRECTORY)),
            ]),
        ),
        (
            Condition::Or(vec![
                Condition::Extension {
                    extension: "md".into(),
                },
                Condition::Extension {
                    extension: "rs".into(),
                },
            ]),
            Filter::or([Filter::extension("md"), Filter::extension("rs")]),
        ),
        (Condition::And(vec![]), Filter::and([])),
        (Condition::Or(vec![]), Filter::or([])),
    ];
    for (condition, filter) in cases {
        let expected = paths(&index.query(&index::Query::filter(filter)));
        let got = paths(&run(&index, condition_only(condition.clone())).hits);
        assert_eq!(got, expected, "{condition:?}");
    }
    // 조건마다 결과가 실제로 갈리는지(모두 빈 결과로 같아지는 것이 아닌지) 몇 개만 본다.
    let got = run(
        &index,
        condition_only(Condition::Under {
            folder: r"C:\문서".into(),
        }),
    );
    assert_eq!(got.total, 4);
}

#[test]
fn sort_and_limit_become_index_sort_and_limit() {
    let index = sample();
    for key in [
        SortKey::Name,
        SortKey::Path,
        SortKey::Size,
        SortKey::Modified,
    ] {
        for order in [Order::Ascending, Order::Descending] {
            let expected = index.search(
                &index::Query::filter(Filter::and([]))
                    .sort(key.into(), order.into())
                    .limit(3),
            );
            let got = run(
                &index,
                QueryMessage {
                    condition: Condition::And(vec![]),
                    sort: Some((key, order)),
                    limit: Some(3),
                },
            );
            assert_eq!(paths(&got.hits), paths(&expected.hits), "{key:?} {order:?}");
            assert_eq!(got.hits.len(), 3);
            assert_eq!(got.total, 8);
        }
    }
}

#[test]
fn page_and_resume_go_through_messages() {
    let index = sample();
    // 서비스가 들고 있을 이어 받기 표지 표.
    let cursors: RefCell<HashMap<u64, index::Cursor>> = RefCell::default();
    let keep = |cursor| {
        let mut cursors = cursors.borrow_mut();
        let id = cursors.len() as u64 + 1000;
        cursors.insert(id, cursor);
        id
    };

    let page = run(
        &index,
        QueryMessage {
            condition: Condition::And(vec![]),
            sort: Some((SortKey::Size, Order::Descending)),
            limit: Some(3),
        },
    );
    let first = PageMessage::from_page(page, keep);
    assert_eq!(first.total, 8);
    assert_eq!(first.hits.len(), 3);
    assert_eq!(first.hits[0].name, "photo.jpg");
    let cursor = first.next.expect("남은 결과가 있다");

    let mut seen: Vec<HitMessage> = first.hits;
    let mut token = Some(cursor);
    while let Some(t) = token {
        let request = Request::Resume(ResumeMessage {
            cursor: t,
            limit: 2,
        });
        let Request::Resume(resume) = Request::from_bytes(&request.to_bytes()).unwrap() else {
            unreachable!()
        };
        let stored = cursors.borrow_mut().remove(&resume.cursor).unwrap();
        let page = index.resume(stored, resume.limit()).unwrap();
        let message = PageMessage::from_page(page, keep);
        assert_eq!(message.total, 8);
        assert!(message.hits.len() <= 2);
        seen.extend(message.hits);
        token = message.next;
    }
    let sizes: Vec<u64> = seen.iter().map(|h| h.size).collect();
    assert_eq!(sizes, [9_000, 300, 70, 50, 10, 1, 0, 0]);

    // 결과는 인덱스의 결과와 같다.
    let direct: Vec<HitMessage> = index
        .query(
            &index::Query::filter(Filter::and([]))
                .sort(index::SortKey::Size, index::Order::Descending),
        )
        .into_iter()
        .map(HitMessage::from)
        .collect();
    assert_eq!(seen, direct);
}

#[test]
fn index_errors_become_error_messages() {
    let bad = QueryMessage {
        condition: Condition::NameRegex {
            pattern: "(".into(),
            case: Case::Insensitive,
        },
        sort: None,
        limit: None,
    };
    let err = ErrorMessage::from(bad.to_query().unwrap_err());
    assert_eq!(err.kind, ErrorKind::InvalidQuery);

    let nested_bad = condition_only(Condition::Not(Box::new(Condition::And(vec![
        Condition::NameWildcard {
            pattern: "*".into(),
            case: Case::Sensitive,
        },
        Condition::NameRegex {
            pattern: "[".into(),
            case: Case::Sensitive,
        },
    ]))));
    assert!(nested_bad.to_query().is_err());

    let mut index = sample();
    let page = index.search(&index::Query::filter(Filter::and([])).limit(1));
    index
        .insert(NewItem {
            number: 99,
            parent: ROOT,
            name: "new",
            size: 0,
            modified: 0,
            attributes: 0,
        })
        .unwrap();
    let err = ErrorMessage::from(index.resume(page.next.unwrap(), 1).unwrap_err());
    assert_eq!(err.kind, ErrorKind::StaleCursor);

    let err = ErrorMessage::from(Request::from_bytes(&[0xee]).unwrap_err());
    assert_eq!(err.kind, ErrorKind::BadMessage);
}

#[test]
fn enums_convert_both_ways() {
    for c in [Case::Insensitive, Case::Sensitive] {
        assert_eq!(Case::from(index::Case::from(c)), c);
    }
    for k in [
        SortKey::Name,
        SortKey::Path,
        SortKey::Size,
        SortKey::Modified,
    ] {
        assert_eq!(SortKey::from(index::SortKey::from(k)), k);
    }
    for o in [Order::Ascending, Order::Descending] {
        assert_eq!(Order::from(index::Order::from(o)), o);
    }
    let hit = HitMessage {
        path: r"C:\a".into(),
        name: "a".into(),
        size: 1,
        modified: 2,
        attributes: 3,
    };
    assert_eq!(HitMessage::from(index::Hit::from(hit.clone())), hit);
}
