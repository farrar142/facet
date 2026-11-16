//! 질의 문법 해석 시험(순수 함수, 오늘은 인자로 고정).

use chrono::NaiveDate;
use facet_core::query_syntax::{Keyword, ParseErrorKind, Parsed, Today, parse};
use facet_protocol::{Case, Condition, QueryMessage, Request, read_message, write_message};

const KB: u64 = 1024;
const MB: u64 = KB * 1024;

fn today() -> Today {
    // 2026-11-18 수요일, 한국 시간.
    Today {
        date: NaiveDate::from_ymd_opt(2026, 11, 18).unwrap(),
        utc_offset_seconds: 9 * 3600,
    }
}

fn q(text: &str) -> Condition {
    match parse(text, today()) {
        Ok(Parsed::Query(c)) => c,
        other => panic!("{text:?}: {other:?}"),
    }
}

fn err(text: &str) -> (std::ops::Range<usize>, ParseErrorKind) {
    let e = parse(text, today()).expect_err(text);
    (e.range, e.kind)
}

fn contains(s: &str) -> Condition {
    Condition::NameContains {
        part: s.into(),
        case: Case::Insensitive,
    }
}

fn filetime(y: i32, m: u32, d: u32) -> u64 {
    let date = NaiveDate::from_ymd_opt(y, m, d).unwrap();
    let unix = date.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp() - 9 * 3600;
    (unix + 11_644_473_600) as u64 * 10_000_000
}

#[test]
fn words_and_or_not_groups() {
    assert_eq!(q("a"), contains("a"));
    assert_eq!(q("a b"), Condition::And(vec![contains("a"), contains("b")]));
    assert_eq!(q("a|b"), Condition::Or(vec![contains("a"), contains("b")]));
    // 그리고가 또는보다 강하다.
    assert_eq!(
        q("a b|c"),
        Condition::Or(vec![
            Condition::And(vec![contains("a"), contains("b")]),
            contains("c")
        ])
    );
    assert_eq!(
        q("a <b|c>"),
        Condition::And(vec![
            contains("a"),
            Condition::Or(vec![contains("b"), contains("c")])
        ])
    );
    assert_eq!(q("!a"), Condition::Not(Box::new(contains("a"))));
    assert_eq!(
        q("!<a b>"),
        Condition::Not(Box::new(Condition::And(vec![contains("a"), contains("b")])))
    );
    assert_eq!(q("  a   b  "), q("a b"));
}

#[test]
fn wildcards_and_quotes() {
    assert_eq!(
        q("*.TXT"),
        Condition::NameWildcard {
            pattern: "*.TXT".into(),
            case: Case::Insensitive
        }
    );
    assert!(matches!(q("a?c"), Condition::NameWildcard { .. }));
    assert_eq!(q("\"a b\""), contains("a b"));
    assert_eq!(q("\"a|b <c> !d\""), contains("a|b <c> !d"));
    assert_eq!(
        q("\"a b\" c"),
        Condition::And(vec![contains("a b"), contains("c")])
    );
    assert_eq!(
        q("path:\"my docs\""),
        Condition::PathContains {
            part: "my docs".into(),
            case: Case::Insensitive
        }
    );
}

#[test]
fn korean_and_emoji() {
    assert_eq!(q("보고서"), contains("보고서"));
    assert_eq!(
        q("🎉 파티"),
        Condition::And(vec![contains("🎉"), contains("파티")])
    );
    // 오류 범위는 UTF-8 바이트다.
    assert_eq!(err("가나 \"다"), (7..8, ParseErrorKind::UnclosedQuote));
}

#[test]
fn unknown_prefix_is_text() {
    assert_eq!(q("c:"), contains("c:"));
    assert_eq!(q("c:\\temp"), contains("c:\\temp"));
    assert_eq!(q("foo:bar"), contains("foo:bar"));
    assert_eq!(q(":x"), contains(":x"));
}

#[test]
fn empty_query() {
    assert_eq!(parse("", today()), Ok(Parsed::Empty));
    assert_eq!(parse("  \t ", today()), Ok(Parsed::Empty));
}

#[test]
fn errors_with_positions() {
    assert_eq!(err("a \"b"), (2..3, ParseErrorKind::UnclosedQuote));
    assert_eq!(err("a <b"), (2..3, ParseErrorKind::UnclosedGroup));
    assert_eq!(err("a b>"), (3..4, ParseErrorKind::UnmatchedClose));
    assert_eq!(err("a <>"), (2..4, ParseErrorKind::EmptyGroup));
    assert_eq!(err("a < >"), (2..5, ParseErrorKind::EmptyGroup));
    assert_eq!(err("|"), (0..1, ParseErrorKind::MissingOperand));
    assert_eq!(err("a |"), (3..3, ParseErrorKind::MissingOperand));
    assert_eq!(err("a | | b"), (4..5, ParseErrorKind::MissingOperand));
    assert_eq!(err("a !"), (2..3, ParseErrorKind::MissingOperand));
    assert_eq!(err("!"), (0..1, ParseErrorKind::MissingOperand));
    assert_eq!(err("\"\""), (0..2, ParseErrorKind::MissingOperand));
    assert_eq!(err("<a|>"), (3..4, ParseErrorKind::MissingOperand));
    assert_eq!(
        err("size:"),
        (0..5, ParseErrorKind::MissingValue(Keyword::Size))
    );
    assert_eq!(err("a size:x"), (2..8, ParseErrorKind::InvalidSize));
    assert_eq!(err("dm:foo"), (0..6, ParseErrorKind::InvalidDate));
    assert_eq!(err("dm:2026-13"), (0..10, ParseErrorKind::InvalidDate));
    assert_eq!(
        err("folder:x"),
        (0..8, ParseErrorKind::UnexpectedValue(Keyword::Folder))
    );
    let deep = "<".repeat(100) + "a" + &">".repeat(100);
    assert_eq!(err(&deep).1, ParseErrorKind::TooDeep);
}

#[test]
fn ext_condition() {
    assert_eq!(
        q("ext:txt"),
        Condition::Extension {
            extension: "txt".into()
        }
    );
    assert_eq!(
        q("EXT:.txt"),
        Condition::Extension {
            extension: "txt".into()
        }
    );
    assert_eq!(
        q("ext:jpg;png;"),
        Condition::Or(vec![
            Condition::Extension {
                extension: "jpg".into()
            },
            Condition::Extension {
                extension: "png".into()
            }
        ])
    );
    assert_eq!(err("ext:;").1, ParseErrorKind::MissingValue(Keyword::Ext));
}

#[test]
fn folder_file_path_regex() {
    let dir = Condition::Attributes { mask: 0x10 };
    assert_eq!(q("folder:"), dir);
    assert_eq!(q("file:"), Condition::Not(Box::new(dir.clone())));
    // 값 없는 조건 바로 뒤의 `>`는 묶음을 닫는다.
    assert_eq!(q("<folder:>"), dir);
    assert_eq!(q("folder: a"), Condition::And(vec![dir, contains("a")]));
    assert_eq!(
        q("path:src"),
        Condition::PathContains {
            part: "src".into(),
            case: Case::Insensitive
        }
    );
    assert_eq!(
        q("path:*\\src\\*"),
        Condition::PathWildcard {
            pattern: "*\\src\\*".into(),
            case: Case::Insensitive
        }
    );
    assert_eq!(
        q("regex:^a.*z$"),
        Condition::NameRegex {
            pattern: "^a.*z$".into(),
            case: Case::Insensitive
        }
    );
    let (range, kind) = err("x regex:(a");
    assert_eq!(range, 2..10);
    assert!(matches!(kind, ParseErrorKind::InvalidRegex(_)));
}

fn size(min: Option<u64>, max: Option<u64>) -> Condition {
    Condition::Size { min, max }
}

#[test]
fn size_conditions() {
    assert_eq!(q("size:>1mb"), size(Some(MB + 1), None));
    assert_eq!(q("size:>=1mb"), size(Some(MB), None));
    assert_eq!(q("size:<1kb"), size(None, Some(KB - 1)));
    assert_eq!(q("size:<=1KB"), size(None, Some(KB)));
    assert_eq!(q("size:<0"), size(Some(1), Some(0)));
    assert_eq!(q("size:1mb..2mb"), size(Some(MB), Some(2 * MB)));
    assert_eq!(q("size:..10"), size(None, Some(10)));
    assert_eq!(q("size:1gb.."), size(Some(1024 * MB), None));
    assert_eq!(q("size:100"), size(Some(100), Some(100)));
    assert_eq!(q("size:1.5kb"), size(Some(1536), Some(1536)));
    assert_eq!(
        q("SIZE:>1mb a"),
        Condition::And(vec![size(Some(MB + 1), None), contains("a")])
    );
    // 묶음 안에서도 마지막 `>`는 묶음을 닫는다.
    assert_eq!(q("<size:>1mb>"), size(Some(MB + 1), None));
    assert_eq!(
        q("<size:>=1mb|a>"),
        Condition::Or(vec![size(Some(MB), None), contains("a")])
    );
    assert_eq!(err("size:..").1, ParseErrorKind::InvalidSize);
    assert_eq!(err("size:1tb").1, ParseErrorKind::InvalidSize);
    assert_eq!(err("size:>").1, ParseErrorKind::InvalidSize);
}

#[test]
fn size_words_follow_everything() {
    assert_eq!(q("size:empty"), size(Some(0), Some(0)));
    assert_eq!(q("size:tiny"), size(Some(1), Some(10 * KB)));
    assert_eq!(q("size:small"), size(Some(10 * KB + 1), Some(100 * KB)));
    assert_eq!(q("size:medium"), size(Some(100 * KB + 1), Some(MB)));
    assert_eq!(q("size:large"), size(Some(MB + 1), Some(16 * MB)));
    assert_eq!(q("size:huge"), size(Some(16 * MB + 1), Some(128 * MB)));
    assert_eq!(q("size:gigantic"), size(Some(128 * MB + 1), None));
}

fn modified(min: u64, max: u64) -> Condition {
    Condition::Modified {
        min: Some(min),
        max: Some(max),
    }
}

#[test]
fn date_conditions() {
    assert_eq!(
        q("dm:today"),
        modified(filetime(2026, 11, 18), filetime(2026, 11, 19) - 1)
    );
    assert_eq!(
        q("dm:yesterday"),
        modified(filetime(2026, 11, 17), filetime(2026, 11, 18) - 1)
    );
    // 이번 주는 월요일(11-16)부터.
    assert_eq!(
        q("dm:thisweek"),
        modified(filetime(2026, 11, 16), filetime(2026, 11, 23) - 1)
    );
    assert_eq!(
        q("dm:lastweek"),
        modified(filetime(2026, 11, 9), filetime(2026, 11, 16) - 1)
    );
    assert_eq!(
        q("dm:thismonth"),
        modified(filetime(2026, 11, 1), filetime(2026, 12, 1) - 1)
    );
    assert_eq!(
        q("dm:thisyear"),
        modified(filetime(2026, 1, 1), filetime(2027, 1, 1) - 1)
    );
    assert_eq!(
        q("dm:2026-11-15"),
        modified(filetime(2026, 11, 15), filetime(2026, 11, 16) - 1)
    );
    assert_eq!(
        q("dm:2026-02"),
        modified(filetime(2026, 2, 1), filetime(2026, 3, 1) - 1)
    );
    assert_eq!(
        q("dm:2025"),
        modified(filetime(2025, 1, 1), filetime(2026, 1, 1) - 1)
    );
    assert_eq!(
        q("dm:2026-11-01..2026-11-15"),
        modified(filetime(2026, 11, 1), filetime(2026, 11, 16) - 1)
    );
    assert_eq!(
        q("dm:2026-01..2026-03"),
        modified(filetime(2026, 1, 1), filetime(2026, 4, 1) - 1)
    );
    assert_eq!(
        q("dm:2026-11-15.."),
        Condition::Modified {
            min: Some(filetime(2026, 11, 15)),
            max: None
        }
    );
    assert_eq!(
        q("dm:..2026-11-15"),
        Condition::Modified {
            min: None,
            max: Some(filetime(2026, 11, 16) - 1)
        }
    );
    assert_eq!(q("DM:Today"), q("dm:today"));
}

#[test]
fn utc_offset_moves_day_boundary() {
    let utc = Today {
        utc_offset_seconds: 0,
        ..today()
    };
    let Ok(Parsed::Query(Condition::Modified { min: Some(min), .. })) = parse("dm:today", utc)
    else {
        panic!()
    };
    // 한국 자정은 UTC로 전날 15시: UTC 자정보다 9시간 이르다.
    assert_eq!(min, filetime(2026, 11, 18) + 9 * 3600 * 10_000_000);
}

#[test]
fn result_is_a_protocol_condition_ready_to_send() {
    let condition = q("보고서 ext:pdf;docx size:>1mb dm:thisyear|folder: !temp");
    let message = Request::Query(QueryMessage {
        condition,
        sort: None,
        limit: Some(100),
    });
    let mut bytes = Vec::new();
    write_message(&mut bytes, &message).unwrap();
    let back: Request = read_message(&mut bytes.as_slice()).unwrap();
    assert_eq!(back, message);
}
