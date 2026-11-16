//! **실시간 탐색의 조건 트리 판정**: 질의 문법이 만든 조건 트리로 임시 폴더의 실제 파일을 걸러 기대한 집합과 같은지,
//! 같은 항목 목록을 합성 인덱스에 넣어 같은 질의를 했을 때와 같은 항목을 고르는지, 틀린 문법이면 검색을 시작하지
//! 않고 위치·기록이 그대로인지 본다(Windows 전용: 경로 조건이 `\` 구분 경로를 쓴다).
#![cfg(windows)]

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::os::windows::fs::MetadataExt;
use std::path::Path;
use std::time::{Duration, SystemTime};

use chrono::NaiveDate;
use facet_core::Location;
use facet_core::live_search::{LiveSearch, SearchEvent, SearchRequest};
use facet_core::query_syntax::{ParseErrorKind, Parsed, Today, parse};
use facet_core::tab::ReadOutcome;
use facet_core::window::{App, SearchError};
use facet_index::{Index, NewItem};
use facet_protocol::{Case, Condition, QueryMessage};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(30);

fn today() -> Today {
    Today {
        date: NaiveDate::from_ymd_opt(2026, 11, 16).unwrap(),
        utc_offset_seconds: 0,
    }
}

/// UTC 정오의 시각.
fn noon(y: i32, m: u32, d: u32) -> SystemTime {
    let date = NaiveDate::from_ymd_opt(y, m, d).unwrap();
    let days = date
        .signed_duration_since(NaiveDate::from_ymd_opt(1970, 1, 1).unwrap())
        .num_days();
    SystemTime::UNIX_EPOCH + Duration::from_secs(days as u64 * 86_400 + 12 * 3600)
}

fn put(root: &Path, relative: &str, size: u64, modified: SystemTime) {
    let path = root.join(relative);
    let file = File::create(&path).unwrap();
    file.set_len(size).unwrap();
    file.set_modified(modified).unwrap();
}

/// 시험 나무.
fn tree() -> TempDir {
    let dir = TempDir::with_prefix("facet-cond-").unwrap();
    let root = dir.path();
    fs::create_dir(root.join("docs")).unwrap();
    fs::create_dir(root.join("old")).unwrap();
    put(root, "report.txt", 10, noon(2026, 3, 10));
    put(root, "Report Final.docx", 2000, noon(2026, 3, 10));
    put(root, "notes.md", 0, noon(2020, 1, 1));
    put(root, "big.bin", 2 * 1024 * 1024, noon(2020, 1, 1));
    put(root, "photo.JPG", 100, noon(2026, 3, 11));
    put(root, "docs/readme.txt", 1, noon(2026, 3, 10));
    put(root, "docs/my report.txt", 3, noon(2026, 3, 10));
    put(root, "docs/보고서.txt", 4, noon(2026, 3, 11));
    put(root, "old/archive.zip", 50, noon(2019, 6, 1));
    let mut permissions = fs::metadata(root.join("notes.md")).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(root.join("notes.md"), permissions).unwrap();
    dir
}

fn run(root: &Path, condition: Condition) -> BTreeSet<String> {
    let mut search = LiveSearch::start(SearchRequest {
        root: Location::file_system(root),
        condition,
        show_hidden: true,
    })
    .unwrap();
    let mut found = BTreeSet::new();
    loop {
        match search.wait_event(WAIT).expect("탐색이 끝나지 않았다") {
            SearchEvent::Batch { items, .. } => {
                for item in items {
                    assert!(found.insert(item.path().to_string_lossy().into_owned()));
                }
            }
            SearchEvent::Finished(_) => return found,
        }
    }
}

fn parsed(query: &str) -> Condition {
    match parse(query, today()).unwrap_or_else(|e| panic!("{query}: {e:?}")) {
        Parsed::Query(condition) => condition,
        Parsed::Empty => panic!("빈 질의"),
    }
}

fn expect(root: &Path, query: &str, expected: &[&str]) {
    let found = run(root, parsed(query));
    let prefix = format!("{}\\", root.display());
    let mut got: Vec<String> = found
        .iter()
        .map(|p| p.strip_prefix(&prefix).unwrap().replace('\\', "/"))
        .collect();
    got.sort();
    let mut want: Vec<String> = expected.iter().map(|s| (*s).to_owned()).collect();
    want.sort();
    assert_eq!(got, want, "질의: {query}");
}

#[test]
fn 낱말_그리고_또는_아님_따옴표_묶음() {
    let dir = tree();
    let root = dir.path();
    expect(
        root,
        "report",
        &["report.txt", "Report Final.docx", "docs/my report.txt"],
    );
    expect(root, "report txt", &["report.txt", "docs/my report.txt"]);
    expect(root, "readme|notes", &["docs/readme.txt", "notes.md"]);
    expect(root, "report !txt", &["Report Final.docx"]);
    expect(root, "\"my report\"", &["docs/my report.txt"]);
    expect(
        root,
        "file: ext:txt <readme|보고서>",
        &["docs/readme.txt", "docs/보고서.txt"],
    );
    expect(root, "folder:", &["docs", "old"]);
    expect(root, "!file:", &["docs", "old"]);
    expect(root, "folder: !old", &["docs"]);
}

#[test]
fn 와일드카드_확장자_정규식() {
    let dir = tree();
    let root = dir.path();
    expect(
        root,
        "*.txt",
        &[
            "report.txt",
            "docs/readme.txt",
            "docs/my report.txt",
            "docs/보고서.txt",
        ],
    );
    expect(root, "ext:jpg;md", &["photo.JPG", "notes.md"]);
    expect(root, "regex:^r.*t\\.txt$", &["report.txt"]);
}

#[test]
fn 크기_날짜_경로() {
    let dir = tree();
    let root = dir.path();
    expect(root, "size:>1mb", &["big.bin"]);
    expect(root, "size:empty file:", &["notes.md"]);
    expect(
        root,
        "file: dm:2026-03-10",
        &[
            "report.txt",
            "Report Final.docx",
            "docs/readme.txt",
            "docs/my report.txt",
        ],
    );
    expect(root, "file: dm:2019", &["old/archive.zip"]);
    expect(
        root,
        "file: dm:2026-03-11..2026-03-11",
        &["photo.JPG", "docs/보고서.txt"],
    );
    expect(
        root,
        "path:docs\\",
        &["docs/readme.txt", "docs/my report.txt", "docs/보고서.txt"],
    );
    expect(root, "path:*\\old\\*.zip", &["old/archive.zip"]);
}

/// 인덱스에 같은 항목 목록을 넣어 같은 조건으로 질의한 결과(전체 경로들).
fn index_answer(root: &Path, condition: &Condition) -> BTreeSet<String> {
    fn walk(index: &mut Index, folder: &Path, parent: u64, next: &mut u64) {
        let mut entries: Vec<_> = fs::read_dir(folder).unwrap().map(|e| e.unwrap()).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let metadata = entry.metadata().unwrap();
            let number = *next;
            *next += 1;
            let name = entry.file_name().to_string_lossy().into_owned();
            index
                .insert(NewItem {
                    number,
                    parent,
                    name: &name,
                    size: metadata.len(),
                    modified: metadata.last_write_time(),
                    attributes: metadata.file_attributes(),
                })
                .unwrap();
            if metadata.is_dir() {
                walk(index, &entry.path(), number, next);
            }
        }
    }
    let mut index = Index::new(&root.display().to_string(), 1);
    let mut next = 2u64;
    walk(&mut index, root, 1, &mut next);
    let message = QueryMessage {
        condition: condition.clone(),
        sort: None,
        limit: None,
    };
    index
        .search(&message.to_query().unwrap())
        .hits
        .into_iter()
        .map(|hit| hit.path)
        .collect()
}

#[test]
fn 조건_트리_판정이_인덱스의_같은_질의와_같은_항목을_고른다() {
    let dir = tree();
    let root = dir.path();
    let folder = root.join("docs").display().to_string();
    let mut conditions: Vec<(String, Condition)> = [
        "report",
        "report txt",
        "readme|notes",
        "report !txt",
        "\"my report\"",
        "*.txt",
        "?eport*",
        "ext:jpg;md",
        "ext:.TXT",
        "size:>1mb",
        "size:tiny",
        "size:10..2000",
        "size:empty file:",
        "file: dm:2026-03-10",
        "file: dm:2020",
        "path:docs\\",
        "path:*\\old\\*.zip",
        "regex:^r.*t\\.txt$",
        "regex:REPORT",
        "folder:",
        "file:",
        "!file:",
        "file: !<ext:txt|ext:md>",
        "보고서",
    ]
    .iter()
    .map(|q| ((*q).to_owned(), parsed(q)))
    .collect();
    // 문법에 없는 조건(폴더 아래, 속성, 대소문자 구분)도 같은 뜻이다.
    for (name, condition) in [
        ("under", Condition::Under { folder }),
        ("readonly", Condition::Attributes { mask: 0x1 }),
        ("directory", Condition::Attributes { mask: 0x10 }),
        (
            "case",
            Condition::NameContains {
                part: "Report".to_owned(),
                case: Case::Sensitive,
            },
        ),
        (
            "path-case",
            Condition::PathContains {
                part: "DOCS".to_owned(),
                case: Case::Insensitive,
            },
        ),
    ] {
        conditions.push((name.to_owned(), condition));
    }
    for (label, condition) in conditions {
        let live = run(root, condition.clone());
        let index = index_answer(root, &condition);
        assert_eq!(live, index, "질의: {label}");
        assert!(
            !live.is_empty(),
            "빈 결과로 견주면 시험이 되지 않는다: {label}"
        );
    }
}

#[test]
fn 틀린_문법이면_검색이_시작되지_않고_위치와_기록이_그대로이며_까닭이_온다() {
    let dir = tree();
    let mut app = App::new(Some(Location::file_system(dir.path())));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    let location = app.active_tab().location().clone();
    let can_back = app.active_tab().can_go_back();

    match app.start_search("report <txt") {
        Err(SearchError::Syntax(error)) => {
            assert_eq!(error.kind, ParseErrorKind::UnclosedGroup);
            assert_eq!(error.range, 7..8);
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        app.start_search("size:abc"),
        Err(SearchError::Syntax(_))
    ));
    assert!(matches!(
        app.start_search("regex:("),
        Err(SearchError::Syntax(e)) if matches!(e.kind, ParseErrorKind::InvalidRegex(_))
    ));
    assert_eq!(app.start_search("  "), Err(SearchError::Empty));
    assert_eq!(app.active_tab().location(), &location);
    assert_eq!(app.active_tab().can_go_back(), can_back);
    assert!(app.active_tab().search().is_none());
}
