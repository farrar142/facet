// target: crates/facet-core/tests/spec_batch_rename_rules_date.rs
// AC: #274: 수정 날짜 넣기의 형식 낱말·그 밖 글자, 지역 차이로 다른 날, 앞·뒤·N번째 자리와 적용 대상, 시각 모름이면 건너뜀
//! 일괄 이름 바꾸기 규칙 목록의 수정 날짜 넣기 규칙(`RuleKind::Date`, `batch_rename::preview_rules`, 순수 함수)을 확인한다:
//! 형식 낱말 `yyyy` `yy` `MM` `dd` `HH` `mm` `ss`와 그대로 남는 글자, 부르는 쪽이 넘긴 UTC 차이로 같은 시각이 다른 날·해가
//! 됨, 넣을 자리와 구분 글자, 수정 시각을 모르는 항목은 이 규칙만 건너뜀. 시계·시간대·파일 시스템을 부르지 않아 모든
//! 환경에서 돈다.

use std::ffi::OsString;
use std::path::PathBuf;

use facet_core::batch_rename::{
    BatchItem, Edge, EntryStatus, ExistingNames, Place, Preview, Rule, RuleContext, RuleKind,
    Target, preview_rules,
};

/// 1601-01-01(`FILETIME`의 0)과 1970-01-01 사이의 초.
const EPOCH_GAP_SECONDS: u64 = 11_644_473_600;

/// 유닉스 시각(초)을 `FILETIME`(100ns 단위)으로.
const fn filetime(unix_seconds: u64) -> u64 {
    (unix_seconds + EPOCH_GAP_SECONDS) * 10_000_000
}

/// 2026-11-15 23:30:45 UTC.
const NOV_15: u64 = filetime(1_794_785_445);
/// 2026-12-31 20:00:00 UTC.
const DEC_31: u64 = filetime(1_798_747_200);

const HOUR: i64 = 3600;

fn file(name: &str) -> BatchItem {
    BatchItem {
        folder: PathBuf::from("F"),
        name: OsString::from(name),
        is_folder: false,
    }
}

fn date(format: &str, place: Place, separator: &str) -> Rule {
    Rule::new(RuleKind::Date {
        format: format.to_string(),
        place,
        separator: separator.to_string(),
    })
}

fn apply(items: &[BatchItem], rules: &[Rule], modified: &[Option<u64>], offset: i64) -> Preview {
    let context = RuleContext {
        modified: modified.to_vec(),
        utc_offset_seconds: offset,
    };
    preview_rules(items, rules, &context, &ExistingNames::new())
}

/// 항목 하나에 날짜 규칙 하나를 쓴 새 이름.
fn dated(name: &str, rule: Rule, time: u64, offset: i64) -> String {
    apply(&[file(name)], &[rule], &[Some(time)], offset).entries()[0]
        .new_name
        .clone()
}

#[test]
fn format_words_become_the_date_and_other_text_stays() {
    assert_eq!(
        dated(
            "사진.jpg",
            date("yyyy-MM-dd HH.mm.ss", Place::End, " "),
            NOV_15,
            0
        ),
        "사진 2026-11-15 23.30.45.jpg"
    );
    assert_eq!(
        dated("사진.jpg", date("yyMMdd", Place::Start, "_"), NOV_15, 0),
        "261115_사진.jpg"
    );
    assert_eq!(
        dated(
            "사진.jpg",
            date("yyyy년 MM월 dd일", Place::Start, " "),
            NOV_15,
            0
        ),
        "2026년 11월 15일 사진.jpg"
    );
    assert_eq!(
        dated("a.txt", date("[HHmm]", Place::End, ""), NOV_15, 0),
        "a[2330].txt"
    );
}

#[test]
fn the_utc_offset_can_move_the_date_to_another_day_or_year() {
    let rule = || date("yyyy-MM-dd_HH", Place::Start, " ");
    assert_eq!(dated("a.txt", rule(), NOV_15, 0), "2026-11-15_23 a.txt");
    assert_eq!(
        dated("a.txt", rule(), NOV_15, 9 * HOUR),
        "2026-11-16_08 a.txt",
        "UTC+9에서는 다음 날"
    );
    assert_eq!(
        dated("a.txt", rule(), NOV_15, -10 * HOUR),
        "2026-11-15_13 a.txt"
    );
    assert_eq!(
        dated("a.txt", rule(), NOV_15, -24 * HOUR),
        "2026-11-14_23 a.txt"
    );
    assert_eq!(
        dated(
            "a.txt",
            date("yy-MM-dd", Place::Start, " "),
            DEC_31,
            9 * HOUR
        ),
        "27-01-01 a.txt",
        "해가 넘어간다"
    );
    assert_eq!(
        dated("a.txt", date("yyyy-MM-dd", Place::Start, " "), DEC_31, 0),
        "2026-12-31 a.txt"
    );
}

#[test]
fn the_date_goes_where_the_place_says_with_the_separator() {
    let format = "yyyy-MM-dd";
    assert_eq!(
        dated("사진abc.jpg", date(format, Place::Start, "_"), NOV_15, 0),
        "2026-11-15_사진abc.jpg"
    );
    assert_eq!(
        dated("사진abc.jpg", date(format, Place::End, "_"), NOV_15, 0),
        "사진abc_2026-11-15.jpg"
    );
    let at = Place::At {
        from: Edge::Start,
        at: 2,
    };
    assert_eq!(
        dated("사진abc.jpg", date(format, at, "_"), NOV_15, 0),
        "사진2026-11-15_abc.jpg",
        "N번째 자리면 날짜 뒤에 구분 글자"
    );
    let whole = Rule {
        target: Target::Whole,
        ..date(format, Place::End, "_")
    };
    assert_eq!(dated("a.txt", whole, NOV_15, 0), "a.txt_2026-11-15");
}

#[test]
fn items_without_a_known_time_skip_only_the_date_rule() {
    let items = [file("a.jpg"), file("b.jpg"), file("c.jpg")];
    let rule = date("yyyy-MM-dd", Place::End, "_");

    let result = apply(
        &items,
        std::slice::from_ref(&rule),
        &[None, Some(NOV_15)],
        0,
    );
    let names: Vec<&str> = result
        .entries()
        .iter()
        .map(|entry| entry.new_name.as_str())
        .collect();
    assert_eq!(
        names,
        ["a.jpg", "b_2026-11-15.jpg", "c.jpg"],
        "모르는 시각과 목록이 짧아 없는 시각은 건너뛴다"
    );
    let statuses: Vec<EntryStatus> = result.entries().iter().map(|entry| entry.status).collect();
    assert_eq!(
        statuses,
        [
            EntryStatus::Unchanged,
            EntryStatus::Rename,
            EntryStatus::Unchanged
        ]
    );

    let insert = Rule::new(RuleKind::Insert {
        text: "x".to_string(),
        from: Edge::Start,
        at: 0,
    });
    let result = apply(&items[..1], &[rule, insert], &[None], 0);
    assert_eq!(result.entries()[0].new_name, "xa.jpg", "다른 규칙은 그대로");
}
