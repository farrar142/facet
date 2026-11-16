//! 조건 트리 판정: 질의 프로토콜의 조건 트리([`Condition`])로 실제 파일 시스템의 항목 하나가 맞는지 가린다.
//!
//! 강등한 실시간 탐색([`crate::live_search`])이 쓴다. 결과의 뜻이 인덱스와 같아야 하므로 각 조건은
//! `facet_index::Filter`와 같은 뜻이다(테스트 `tests/live_search_condition.rs`가 같은 항목 목록을 합성 인덱스에도
//! 넣어 견준다): 이름 부분 일치, 이름 전체 와일드카드, 이름 정규식, 전체 경로 부분 일치·와일드카드(`\` 구분,
//! `*`는 `\`도 넘음), 폴더 아래(폴더 자신은 아님), 크기, 수정 날짜(`FILETIME`), 속성 비트, 확장자(이름의 마지막
//! `.` 뒤), 그리고·또는·아님. 대소문자 무시는 글자마다 소문자로 바꿔 견준다.
//!
//! 열거가 준 항목 정보([`Item`])만 쓰고 시스템 호출을 더 하지 않는다. 경로는 그 조건이 있을 때만 만든다.
//! 속성 비트는 [`Item`]이 아는 것(읽기 전용 `0x1`·숨김 `0x2`·시스템 `0x4`·폴더 `0x10`)만 켜지므로 그 밖의 비트(보관
//! 등)를 요구하는 조건은 맞지 않는다.

use std::path::Path;
use std::time::SystemTime;

use facet_protocol::{Case, Condition};
use regex::{Regex, RegexBuilder};

use crate::Item;

const SEPARATOR: char = '\\';
const READ_ONLY: u32 = 0x1;
const HIDDEN: u32 = 0x2;
const SYSTEM: u32 = 0x4;
const DIRECTORY: u32 = 0x10;

/// 1601-01-01부터 1970-01-01까지의 초.
const FILETIME_UNIX_EPOCH_SECS: u64 = 11_644_473_600;

/// 컴파일해 둔 조건 트리. 항목마다 [`ConditionMatcher::is_match`]로 맞춘다.
#[derive(Debug)]
pub struct ConditionMatcher {
    node: Node,
}

#[derive(Debug)]
enum Node {
    /// 이름 부분 일치. 대소문자 무시면 `part`는 접은 것이다.
    Contains {
        part: String,
        case: Case,
    },
    /// 이름이 정규식(와일드카드를 바꾼 것 포함)과 맞는다.
    NamePattern(Regex),
    PathContains {
        part: String,
        case: Case,
    },
    PathPattern(Regex),
    /// 접은 폴더 경로(`\`로 끝남).
    Under(String),
    Size(Option<u64>, Option<u64>),
    Modified(Option<u64>, Option<u64>),
    Attributes(u32),
    /// 접은 확장자.
    Extension(String),
    All(Vec<Node>),
    Any(Vec<Node>),
    Not(Box<Node>),
}

impl ConditionMatcher {
    /// 조건 트리를 컴파일한다. 틀린 정규식은 정규식 라이브러리의 설명을 오류로 돌려준다.
    pub fn new(condition: &Condition) -> Result<Self, String> {
        Ok(Self {
            node: compile(condition)?,
        })
    }

    /// `folder` 안의 `item`이 조건과 맞는가.
    pub fn is_match(&self, folder: &Path, item: &Item) -> bool {
        let name = item.name.to_string_lossy();
        eval(&self.node, folder, item, &name)
    }
}

fn compile(condition: &Condition) -> Result<Node, String> {
    let regex = |pattern: &str, case: Case| {
        RegexBuilder::new(pattern)
            .case_insensitive(case == Case::Insensitive)
            .dot_matches_new_line(true)
            .build()
            .map_err(|e| e.to_string())
    };
    let part = |part: &str, case: Case| match case {
        Case::Insensitive => fold(part),
        Case::Sensitive => part.to_owned(),
    };
    Ok(match condition {
        Condition::NameContains { part: p, case } => Node::Contains {
            part: part(p, *case),
            case: *case,
        },
        Condition::NameWildcard { pattern, case } => {
            Node::NamePattern(regex(&wildcard_regex(pattern), *case)?)
        }
        Condition::NameRegex { pattern, case } => Node::NamePattern(regex(pattern, *case)?),
        Condition::PathContains { part: p, case } => Node::PathContains {
            part: part(p, *case),
            case: *case,
        },
        Condition::PathWildcard { pattern, case } => {
            Node::PathPattern(regex(&wildcard_regex(pattern), *case)?)
        }
        Condition::Under { folder } => {
            let mut path = fold(folder.trim_end_matches(SEPARATOR));
            path.push(SEPARATOR);
            Node::Under(path)
        }
        Condition::Size { min, max } => Node::Size(*min, *max),
        Condition::Modified { min, max } => Node::Modified(*min, *max),
        Condition::Attributes { mask } => Node::Attributes(*mask),
        Condition::Extension { extension } => {
            Node::Extension(fold(extension.strip_prefix('.').unwrap_or(extension)))
        }
        Condition::And(all) => Node::All(all.iter().map(compile).collect::<Result<_, _>>()?),
        Condition::Or(any) => Node::Any(any.iter().map(compile).collect::<Result<_, _>>()?),
        Condition::Not(inner) => Node::Not(Box::new(compile(inner)?)),
    })
}

fn eval(node: &Node, folder: &Path, item: &Item, name: &str) -> bool {
    match node {
        Node::Contains { part, case } => match case {
            Case::Insensitive if part.is_ascii() && name.is_ascii() => {
                // 이름마다 접은 글자를 만들지 않는다(대부분의 이름이 이 길이다).
                let (part, name) = (part.as_bytes(), name.as_bytes());
                part.is_empty()
                    || name
                        .windows(part.len())
                        .any(|w| w.eq_ignore_ascii_case(part))
            }
            Case::Insensitive => fold(name).contains(part.as_str()),
            Case::Sensitive => name.contains(part.as_str()),
        },
        Node::NamePattern(re) => re.is_match(name),
        Node::PathContains { part, case } => {
            let path = full_path(folder, name);
            match case {
                Case::Insensitive => fold(&path).contains(part.as_str()),
                Case::Sensitive => path.contains(part.as_str()),
            }
        }
        Node::PathPattern(re) => re.is_match(&full_path(folder, name)),
        Node::Under(prefix) => {
            let path = fold(&full_path(folder, name));
            path.len() > prefix.len() && path.starts_with(prefix.as_str())
        }
        Node::Size(min, max) => in_range(item.size, *min, *max),
        Node::Modified(min, max) => in_range(filetime(item.modified), *min, *max),
        Node::Attributes(mask) => attribute_bits(item) & mask == *mask,
        Node::Extension(ext) => {
            let name = fold(name);
            name.rfind('.').is_some_and(|dot| &name[dot + 1..] == ext)
        }
        Node::All(all) => all.iter().all(|n| eval(n, folder, item, name)),
        Node::Any(any) => any.iter().any(|n| eval(n, folder, item, name)),
        Node::Not(inner) => !eval(inner, folder, item, name),
    }
}

fn full_path(folder: &Path, name: &str) -> String {
    folder.join(name).to_string_lossy().into_owned()
}

fn attribute_bits(item: &Item) -> u32 {
    let mut bits = 0;
    if item.attributes.read_only {
        bits |= READ_ONLY;
    }
    if item.attributes.hidden {
        bits |= HIDDEN;
    }
    if item.attributes.system {
        bits |= SYSTEM;
    }
    if item.is_folder {
        bits |= DIRECTORY;
    }
    bits
}

/// 수정 시각을 `FILETIME`(1601-01-01 UTC부터 100ns)으로. 1601년 전은 0.
fn filetime(time: SystemTime) -> u64 {
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(since) => {
            (since.as_secs() + FILETIME_UNIX_EPOCH_SECS) * 10_000_000
                + u64::from(since.subsec_nanos() / 100)
        }
        Err(before) => {
            let before = before.duration();
            let total =
                (before.as_secs() as i128) * 10_000_000 + (before.subsec_nanos() / 100) as i128;
            let epoch = i128::from(FILETIME_UNIX_EPOCH_SECS) * 10_000_000;
            u64::try_from((epoch - total).max(0)).unwrap_or(0)
        }
    }
}

fn in_range(value: u64, min: Option<u64>, max: Option<u64>) -> bool {
    min.is_none_or(|m| value >= m) && max.is_none_or(|m| value <= m)
}

fn fold(s: &str) -> String {
    s.chars().flat_map(char::to_lowercase).collect()
}

/// 와일드카드(`*`, `?`)를 처음부터 끝까지 맞는 정규식 글자로 바꾼다(`facet_index`와 같다).
fn wildcard_regex(pattern: &str) -> String {
    let mut re = String::from("^");
    let mut literal = String::new();
    for c in pattern.chars() {
        if matches!(c, '*' | '?') {
            re.push_str(&regex::escape(&literal));
            literal.clear();
            re.push_str(if c == '*' { ".*" } else { "." });
        } else {
            literal.push(c);
        }
    }
    re.push_str(&regex::escape(&literal));
    re.push('$');
    re
}
