//! 이름 일치 규칙: 필터·(앞으로의) 실시간 탐색·내용 검색의 파일 이름 조건이 함께 쓰는 순수 함수.
//!
//! - 별표(`*`)도 물음표(`?`)도 없으면 이름 **어디에 있든** 부분 일치다.
//! - 있으면 이름 **전체**에 대한 와일드카드다(`*`는 0글자 이상, `?`는 정확히 한 글자). 예: 「*.pdf」는 끝이
//!   「.pdf」인 이름, 「a?」는 두 글자짜리 「a」로 시작하는 이름.
//! - 대소문자를 무시한다. 글자마다 유니코드 단순 대소문자 접기(소문자 하나로 바뀌는 글자만 바꾸고, 여러 글자로
//!   늘어나는 글자는 그대로 둔다)로 맞춘다. 한글은 그대로 글자 단위로 맞춘다.
//! - 빈 패턴은 모든 이름에 맞는다.
//!
//! 화면 문장은 없다(ADR-0007).

use std::cell::RefCell;
use std::ffi::OsStr;

thread_local! {
    /// 이름을 접은 글자로 펼치는 임시 버퍼. 이름마다 할당하지 않는다(10만 개를 거를 때 시간이 크게 준다).
    static FOLDED: RefCell<Vec<char>> = const { RefCell::new(Vec::new()) };
}

/// 대소문자를 무시한 단순 접기: 소문자 한 글자로 바뀌면 그 글자, 아니면 그대로.
fn fold(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(one), None) => one,
        _ => c,
    }
}

fn fold_into(name: &str, out: &mut Vec<char>) {
    out.clear();
    out.extend(name.chars().map(fold));
}

/// 패턴에 와일드카드 글자가 있는가.
pub fn has_wildcard(pattern: &str) -> bool {
    pattern.contains(['*', '?'])
}

/// 컴파일한 이름 일치 규칙. 같은 패턴으로 이름을 많이 맞출 때 쓴다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameMatcher {
    /// 접은 패턴 글자.
    pattern: Vec<char>,
    wildcard: bool,
    /// 패턴이 모두 ASCII면 접은 바이트(이름도 ASCII면 할당 없이 맞춘다).
    ascii: Option<Vec<u8>>,
}

impl NameMatcher {
    /// `pattern`으로 규칙을 만든다.
    pub fn new(pattern: &str) -> Self {
        let folded: Vec<char> = pattern.chars().map(fold).collect();
        let ascii = pattern
            .is_ascii()
            .then(|| pattern.bytes().map(|b| b.to_ascii_lowercase()).collect());
        NameMatcher {
            pattern: folded,
            wildcard: has_wildcard(pattern),
            ascii,
        }
    }

    /// 이름이 규칙에 맞는가.
    pub fn is_match(&self, name: &OsStr) -> bool {
        if self.pattern.is_empty() {
            return true;
        }
        let name = name.to_string_lossy();
        if self.wildcard {
            return FOLDED.with_borrow_mut(|chars| {
                fold_into(&name, chars);
                glob(&self.pattern, chars)
            });
        }
        // 패턴이 ASCII면 이름의 ASCII 아닌 글자는 맞을 수 없으니 바이트로 맞춘다. 예외는 소문자 k로 접히는 켈빈 기호
        // (U+212A, UTF-8 `E2 84 AA`)뿐이라 첫 바이트 E2가 든 이름만 글자로 맞춘다.
        if let (Some(pattern), false) = (&self.ascii, name.as_bytes().contains(&0xE2)) {
            return name
                .as_bytes()
                .windows(pattern.len())
                .any(|window| window.eq_ignore_ascii_case(pattern));
        }
        FOLDED.with_borrow_mut(|chars| {
            fold_into(&name, chars);
            chars
                .windows(self.pattern.len())
                .any(|window| window == self.pattern.as_slice())
        })
    }
}

/// `name`이 `pattern`(이름 일치 규칙)에 맞는가. 한 번만 맞출 때의 편의 함수다.
pub fn name_matches(pattern: &str, name: &OsStr) -> bool {
    NameMatcher::new(pattern).is_match(name)
}

/// 이름 전체에 대한 와일드카드 맞추기(접은 글자끼리). 거슬러 올라가는 되돌림은 마지막 `*` 하나만 기억해 입력
/// 길이에 비례한다.
fn glob(pattern: &[char], name: &[char]) -> bool {
    let (mut p, mut n) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        match pattern.get(p) {
            Some('*') => {
                star = Some((p, n));
                p += 1;
            }
            Some('?') => {
                p += 1;
                n += 1;
            }
            Some(&c) if c == name[n] => {
                p += 1;
                n += 1;
            }
            _ => match star {
                Some((star_p, star_n)) => {
                    p = star_p + 1;
                    n = star_n + 1;
                    star = Some((star_p, star_n + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}
