//! 질의 문법 해석: 검색창에 친 글자를 질의 프로토콜의 조건 트리([`Condition`])로 바꾼다.
//!
//! 화면·파일 시스템·시계를 부르지 않는 순수 함수이고 화면 문장 없이 까닭([`ParseErrorKind`])만 돌려준다(ADR-0007).
//! 결과는 프로토콜 조건 트리 그대로라 즉시 검색에 그대로 보내고, 강등한 실시간 탐색도 같은 트리로 판정한다.
//!
//! 문법:
//!
//! - 공백은 그리고, `|`는 또는(그리고보다 약하게 묶음: `a b|c`는 `(a b) | c`), `!`는 아님, `<` `>`는 묶기,
//!   큰따옴표는 구절(안의 공백·연산자 글자는 글자).
//! - 낱말에 `*`·`?`가 있으면 이름 전체 와일드카드, 없으면 이름 부분 일치. 모두 대소문자 무시.
//! - 조건(이름은 대소문자 무시): `ext:a;b`(확장자 또는), `size:`, `dm:`, `path:<글자>`(전체 경로 일치, `*`·`?`가
//!   있으면 경로 전체 와일드카드), `regex:<식>`(이름 정규식), `folder:`·`file:`(폴더 속성 유무).
//! - 알 수 없는 `낱말:`(예: `c:`)은 오류가 아니라 글자(이름 부분 일치)다.
//! - 빈 질의(공백뿐 포함)는 [`Parsed::Empty`]다.
//!
//! `size:` 값: `>` `<` `>=` `<=` `=`와 크기, `a..b`(양 끝 포함, 한쪽은 비울 수 있음), 크기 하나(정확히 그 크기),
//! 낱말. 크기는 수(소수 가능)에 단위 `b`·`kb`·`mb`·`gb`(1024 배수, 없으면 바이트)를 붙인다. 낱말 구간은
//! Everything과 같다(아래 `size_word`): `empty`(0), `tiny`(0 초과 10KB 이하), `small`(10KB 초과 100KB 이하),
//! `medium`(100KB 초과 1MB 이하), `large`(1MB 초과 16MB 이하), `huge`(16MB 초과 128MB 이하),
//! `gigantic`(128MB 초과).
//!
//! `dm:`(수정 날짜) 값: 낱말 `today` `yesterday` `thisweek` `lastweek` `thismonth` `thisyear`, 날짜 `2026-11-15`,
//! 달 `2026-11`, 해 `2026`, 범위 `a..b`(양 끝은 낱말·날짜·달·해, 한쪽은 비울 수 있고 범위는 `a`의 처음부터
//! `b`의 끝까지). 한 주는 월요일에 시작한다.
//!
//! 「오늘」은 부르는 쪽이 [`Today`]로 넘긴다: 지역 날짜와 그 지역의 UTC와의 차이(초). 하루의 경계가 지역
//! 자정이어야 하므로 날짜만이 아니라 차이도 받는다. 수정 날짜는 `FILETIME`(1601-01-01 UTC부터 100ns)이다.
//!
//! 오류 위치는 입력 글자의 **UTF-8 바이트 범위**다(글자 입력란의 선택 오프셋 단위와 같음). 값이 틀린 조건은
//! 그 낱말 전체의 범위이고, 틀린 정규식은 같다.

use std::ops::Range;

use chrono::{Datelike, Days, Months, NaiveDate, NaiveTime};
pub use facet_protocol::{Case, Condition};
use regex::Regex;

/// 폴더 속성 비트(Windows `FILE_ATTRIBUTE_DIRECTORY`).
const DIRECTORY: u32 = 0x10;

/// 괄호 묶기를 겹칠 수 있는 깊이. 넘으면 [`ParseErrorKind::TooDeep`].
pub const MAX_DEPTH: usize = 64;

const KB: u64 = 1024;
const MB: u64 = KB * 1024;
const GB: u64 = MB * 1024;

/// 1601-01-01부터 1970-01-01까지의 초.
const FILETIME_UNIX_EPOCH_SECS: i64 = 11_644_473_600;

/// 「오늘」: 지역 날짜와 그 지역이 UTC보다 앞선 초(한국은 `9 * 3600`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Today {
    pub date: NaiveDate,
    pub utc_offset_seconds: i32,
}

impl Today {
    /// 이 기계의 지금 지역 날짜와 UTC와의 차이.
    pub fn now() -> Self {
        use chrono::Offset;
        let now = chrono::Local::now();
        Self {
            date: now.date_naive(),
            utc_offset_seconds: now.offset().fix().local_minus_utc(),
        }
    }
}

/// 해석 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// 빈 질의(공백뿐 포함).
    Empty,
    /// 조건 트리.
    Query(Condition),
}

/// 질의 글자가 틀린 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseErrorKind {
    /// 닫히지 않은 큰따옴표.
    UnclosedQuote,
    /// 닫히지 않은 `<`.
    UnclosedGroup,
    /// 짝 없는 `>`.
    UnmatchedClose,
    /// 빈 묶음 `<>`.
    EmptyGroup,
    /// 연산자에 피연산자가 없음(`a |`, `| a`, 끝의 `!`, 빈 구절).
    MissingOperand,
    /// 조건에 값이 없음(`size:`).
    MissingValue(Keyword),
    /// 값을 쓰지 않는 조건에 값이 있음(`folder:x`).
    UnexpectedValue(Keyword),
    /// 틀린 `size:` 값.
    InvalidSize,
    /// 틀린 `dm:` 값.
    InvalidDate,
    /// 틀린 정규식(`regex` 크레이트의 설명).
    InvalidRegex(String),
    /// 묶기가 [`MAX_DEPTH`]보다 깊음.
    TooDeep,
}

/// 조건 이름.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyword {
    Ext,
    Size,
    Dm,
    Path,
    Regex,
    Folder,
    File,
}

impl Keyword {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_lowercase().as_str() {
            "ext" => Self::Ext,
            "size" => Self::Size,
            "dm" => Self::Dm,
            "path" => Self::Path,
            "regex" => Self::Regex,
            "folder" => Self::Folder,
            "file" => Self::File,
            _ => return None,
        })
    }
}

/// 틀린 질의: 글자 범위(UTF-8 바이트)와 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub range: Range<usize>,
    pub kind: ParseErrorKind,
}

impl ParseError {
    fn new(range: Range<usize>, kind: ParseErrorKind) -> Self {
        Self { range, kind }
    }
}

/// 질의 글자를 해석한다.
pub fn parse(text: &str, today: Today) -> Result<Parsed, ParseError> {
    let mut p = Parser {
        text,
        pos: 0,
        today,
    };
    p.skip_ws();
    if p.pos >= text.len() {
        return Ok(Parsed::Empty);
    }
    let condition = p.or_expr(0)?;
    Ok(Parsed::Query(condition))
}

struct Word {
    range: Range<usize>,
    /// 큰따옴표를 걷은 글자.
    text: String,
    /// `text` 안 첫 따옴표 밖 `:`의 위치.
    colon: Option<usize>,
}

struct Parser<'a> {
    text: &'a str,
    pos: usize,
    today: Today,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if !c.is_whitespace() {
                break;
            }
            self.pos += c.len_utf8();
        }
    }

    fn or_expr(&mut self, depth: usize) -> Result<Condition, ParseError> {
        let mut parts = vec![self.and_expr(depth)?];
        loop {
            self.skip_ws();
            if self.peek() == Some('|') {
                self.pos += 1;
                parts.push(self.and_expr(depth)?);
            } else {
                break;
            }
        }
        Ok(if parts.len() == 1 {
            parts.pop().unwrap()
        } else {
            Condition::Or(parts)
        })
    }

    fn and_expr(&mut self, depth: usize) -> Result<Condition, ParseError> {
        let mut parts = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                None | Some('|') => break,
                Some('>') => {
                    if depth == 0 {
                        return Err(ParseError::new(
                            self.pos..self.pos + 1,
                            ParseErrorKind::UnmatchedClose,
                        ));
                    }
                    break;
                }
                Some(_) => parts.push(self.unary(depth)?),
            }
        }
        match parts.len() {
            0 => {
                let end = self.peek().map_or(0, char::len_utf8);
                Err(ParseError::new(
                    self.pos..self.pos + end,
                    ParseErrorKind::MissingOperand,
                ))
            }
            1 => Ok(parts.pop().unwrap()),
            _ => Ok(Condition::And(parts)),
        }
    }

    fn unary(&mut self, depth: usize) -> Result<Condition, ParseError> {
        match self.peek() {
            Some('!') => {
                let at = self.pos;
                self.pos += 1;
                self.skip_ws();
                if matches!(self.peek(), None | Some('|') | Some('>')) {
                    return Err(ParseError::new(at..at + 1, ParseErrorKind::MissingOperand));
                }
                Ok(Condition::Not(Box::new(self.unary(depth)?)))
            }
            Some('<') => {
                let open = self.pos;
                if depth + 1 > MAX_DEPTH {
                    return Err(ParseError::new(open..open + 1, ParseErrorKind::TooDeep));
                }
                self.pos += 1;
                self.skip_ws();
                if self.peek() == Some('>') {
                    let close = self.pos;
                    return Err(ParseError::new(open..close + 1, ParseErrorKind::EmptyGroup));
                }
                let inner = self.or_expr(depth + 1)?;
                self.skip_ws();
                if self.peek() == Some('>') {
                    self.pos += 1;
                    Ok(inner)
                } else {
                    Err(ParseError::new(
                        open..open + 1,
                        ParseErrorKind::UnclosedGroup,
                    ))
                }
            }
            _ => {
                let word = self.word()?;
                self.term(word)
            }
        }
    }

    /// 낱말 하나를 읽는다. 공백·`|`에서 끝나고, `>`에서도 끝나되 조건 값 맨 앞의 비교 글자(`size:>1mb`)는 값이다.
    fn word(&mut self) -> Result<Word, ParseError> {
        let start = self.pos;
        let mut text = String::new();
        let mut colon = None;
        while let Some(c) = self.peek() {
            match c {
                _ if c.is_whitespace() || c == '|' => break,
                '>' => {
                    // 비교 글자를 값으로 받는 조건은 `size:`뿐이다(`<folder:>`의 `>`는 묶음을 닫는다).
                    let in_op_position = colon.is_some_and(|i| {
                        text[..i].eq_ignore_ascii_case("size")
                            && text[i + 1..].chars().all(|c| matches!(c, '<' | '>' | '='))
                    });
                    if !in_op_position {
                        break;
                    }
                    text.push(c);
                    self.pos += 1;
                }
                '"' => {
                    let rest = &self.text[self.pos + 1..];
                    let Some(len) = rest.find('"') else {
                        return Err(ParseError::new(
                            self.pos..self.pos + 1,
                            ParseErrorKind::UnclosedQuote,
                        ));
                    };
                    text.push_str(&rest[..len]);
                    self.pos += len + 2;
                }
                ':' => {
                    if colon.is_none() {
                        colon = Some(text.len());
                    }
                    text.push(c);
                    self.pos += 1;
                }
                _ => {
                    text.push(c);
                    self.pos += c.len_utf8();
                }
            }
        }
        Ok(Word {
            range: start..self.pos,
            text,
            colon,
        })
    }

    fn term(&self, word: Word) -> Result<Condition, ParseError> {
        if let Some(i) = word.colon
            && let Some(keyword) = Keyword::from_name(&word.text[..i])
        {
            return self.condition(keyword, &word.text[i + 1..], &word.range);
        }
        if word.text.is_empty() {
            return Err(ParseError::new(word.range, ParseErrorKind::MissingOperand));
        }
        Ok(name_condition(&word.text))
    }

    fn condition(
        &self,
        keyword: Keyword,
        value: &str,
        range: &Range<usize>,
    ) -> Result<Condition, ParseError> {
        let err = |kind| ParseError::new(range.clone(), kind);
        match keyword {
            Keyword::Folder | Keyword::File => {
                if !value.is_empty() {
                    return Err(err(ParseErrorKind::UnexpectedValue(keyword)));
                }
                let dir = Condition::Attributes { mask: DIRECTORY };
                Ok(if keyword == Keyword::Folder {
                    dir
                } else {
                    Condition::Not(Box::new(dir))
                })
            }
            _ if value.is_empty() => Err(err(ParseErrorKind::MissingValue(keyword))),
            Keyword::Ext => {
                let mut all: Vec<Condition> = value
                    .split(';')
                    .map(|e| e.trim().trim_start_matches('.'))
                    .filter(|e| !e.is_empty())
                    .map(|e| Condition::Extension {
                        extension: e.to_owned(),
                    })
                    .collect();
                match all.len() {
                    0 => Err(err(ParseErrorKind::MissingValue(keyword))),
                    1 => Ok(all.pop().unwrap()),
                    _ => Ok(Condition::Or(all)),
                }
            }
            Keyword::Size => parse_size(value)
                .map(|(min, max)| Condition::Size { min, max })
                .ok_or_else(|| err(ParseErrorKind::InvalidSize)),
            Keyword::Dm => parse_dm(value, self.today)
                .map(|(min, max)| Condition::Modified { min, max })
                .ok_or_else(|| err(ParseErrorKind::InvalidDate)),
            Keyword::Path => Ok(if has_wildcard(value) {
                Condition::PathWildcard {
                    pattern: value.to_owned(),
                    case: Case::Insensitive,
                }
            } else {
                Condition::PathContains {
                    part: value.to_owned(),
                    case: Case::Insensitive,
                }
            }),
            Keyword::Regex => match Regex::new(value) {
                Ok(_) => Ok(Condition::NameRegex {
                    pattern: value.to_owned(),
                    case: Case::Insensitive,
                }),
                Err(e) => Err(err(ParseErrorKind::InvalidRegex(e.to_string()))),
            },
        }
    }
}

fn has_wildcard(s: &str) -> bool {
    s.contains(['*', '?'])
}

fn name_condition(text: &str) -> Condition {
    if has_wildcard(text) {
        Condition::NameWildcard {
            pattern: text.to_owned(),
            case: Case::Insensitive,
        }
    } else {
        Condition::NameContains {
            part: text.to_owned(),
            case: Case::Insensitive,
        }
    }
}

// ---------------------------------------------------------------- size

type Bounds = (Option<u64>, Option<u64>);

/// 낱말 구간(Everything과 같음): 아래는 제외, 위는 포함.
fn size_word(word: &str) -> Option<Bounds> {
    let (lo, hi): (Option<u64>, Option<u64>) = match word {
        "empty" => return Some((Some(0), Some(0))),
        "tiny" => (Some(0), Some(10 * KB)),
        "small" => (Some(10 * KB), Some(100 * KB)),
        "medium" => (Some(100 * KB), Some(MB)),
        "large" => (Some(MB), Some(16 * MB)),
        "huge" => (Some(16 * MB), Some(128 * MB)),
        "gigantic" => (Some(128 * MB), None),
        _ => return None,
    };
    Some((lo.map(|v| v + 1), hi))
}

fn parse_bytes(s: &str) -> Option<u64> {
    let s = s.trim().to_lowercase();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (number, unit) = s.split_at(split);
    let mult = match unit.trim() {
        "" | "b" => 1,
        "kb" => KB,
        "mb" => MB,
        "gb" => GB,
        _ => return None,
    };
    let (int, frac) = match number.split_once('.') {
        Some((i, f)) => (i, f),
        None => (number, ""),
    };
    if int.is_empty() && frac.is_empty() || !frac.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let int: u64 = if int.is_empty() { 0 } else { int.parse().ok()? };
    let mut bytes = int.checked_mul(mult)?;
    if !frac.is_empty() {
        let f: f64 = format!("0.{frac}").parse().ok()?;
        bytes = bytes.checked_add((f * mult as f64).round() as u64)?;
    }
    Some(bytes)
}

fn parse_size(value: &str) -> Option<Bounds> {
    let v = value.trim().to_lowercase();
    if let Some((a, b)) = v.split_once("..") {
        let min = if a.is_empty() {
            None
        } else {
            Some(parse_bytes(a)?)
        };
        let max = if b.is_empty() {
            None
        } else {
            Some(parse_bytes(b)?)
        };
        if min.is_none() && max.is_none() {
            return None;
        }
        return Some((min, max));
    }
    if let Some(r) = v.strip_prefix(">=") {
        return Some((Some(parse_bytes(r)?), None));
    }
    if let Some(r) = v.strip_prefix("<=") {
        return Some((None, Some(parse_bytes(r)?)));
    }
    if let Some(r) = v.strip_prefix('>') {
        return Some((Some(parse_bytes(r)?.checked_add(1)?), None));
    }
    if let Some(r) = v.strip_prefix('<') {
        let n = parse_bytes(r)?;
        // 0보다 작은 크기는 없다: 아무것도 맞지 않는 구간.
        return Some(match n.checked_sub(1) {
            Some(max) => (None, Some(max)),
            None => (Some(1), Some(0)),
        });
    }
    let v = v.strip_prefix('=').unwrap_or(&v);
    if let Some(b) = size_word(v) {
        return Some(b);
    }
    let n = parse_bytes(v)?;
    Some((Some(n), Some(n)))
}

// ---------------------------------------------------------------- date

/// 지역 날짜 하나의 자정을 `FILETIME`으로.
fn filetime(date: NaiveDate, offset: i32) -> u64 {
    let unix = date.and_time(NaiveTime::MIN).and_utc().timestamp() - i64::from(offset);
    ((unix + FILETIME_UNIX_EPOCH_SECS).max(0) as u64) * 10_000_000
}

/// 낱말·해·달·날짜 하나가 덮는 날짜 범위(`[처음, 끝)`).
fn span(token: &str, today: NaiveDate) -> Option<(NaiveDate, NaiveDate)> {
    let t = token.trim().to_lowercase();
    let day = |d: NaiveDate| Some((d, d.checked_add_days(Days::new(1))?));
    match t.as_str() {
        "today" => return day(today),
        "yesterday" => return day(today.checked_sub_days(Days::new(1))?),
        "thisweek" | "lastweek" => {
            let monday = today
                .checked_sub_days(Days::new(u64::from(today.weekday().num_days_from_monday())))?;
            let start = if t == "lastweek" {
                monday.checked_sub_days(Days::new(7))?
            } else {
                monday
            };
            return Some((start, start.checked_add_days(Days::new(7))?));
        }
        "thismonth" => {
            let start = today.with_day(1)?;
            return Some((start, start.checked_add_months(Months::new(1))?));
        }
        "thisyear" => {
            let start = NaiveDate::from_ymd_opt(today.year(), 1, 1)?;
            return Some((start, start.checked_add_months(Months::new(12))?));
        }
        _ => {}
    }
    let parts: Vec<&str> = t.split('-').collect();
    let num = |s: &str, max_len: usize| -> Option<u32> {
        if s.is_empty() || s.len() > max_len || !s.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        s.parse().ok()
    };
    match parts.as_slice() {
        [y] if y.len() == 4 => {
            let start = NaiveDate::from_ymd_opt(num(y, 4)? as i32, 1, 1)?;
            Some((start, start.checked_add_months(Months::new(12))?))
        }
        [y, m] if y.len() == 4 => {
            let start = NaiveDate::from_ymd_opt(num(y, 4)? as i32, num(m, 2)?, 1)?;
            Some((start, start.checked_add_months(Months::new(1))?))
        }
        [y, m, d] if y.len() == 4 => day(NaiveDate::from_ymd_opt(
            num(y, 4)? as i32,
            num(m, 2)?,
            num(d, 2)?,
        )?),
        _ => None,
    }
}

fn parse_dm(value: &str, today: Today) -> Option<Bounds> {
    let off = today.utc_offset_seconds;
    let v = value.trim();
    if let Some((a, b)) = v.split_once("..") {
        let min = if a.trim().is_empty() {
            None
        } else {
            Some(filetime(span(a, today.date)?.0, off))
        };
        let max = if b.trim().is_empty() {
            None
        } else {
            Some(filetime(span(b, today.date)?.1, off).saturating_sub(1))
        };
        if min.is_none() && max.is_none() {
            return None;
        }
        return Some((min, max));
    }
    let (start, end) = span(v, today.date)?;
    Some((
        Some(filetime(start, off)),
        Some(filetime(end, off).saturating_sub(1)),
    ))
}
