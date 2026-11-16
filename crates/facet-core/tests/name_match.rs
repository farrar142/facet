//! 이름 일치 규칙(`name_match`)을 공개 인터페이스로 확인한다: 부분 일치, 대소문자, 한글, 와일드카드.

use std::ffi::OsStr;

use facet_core::name_match::{NameMatcher, name_matches};

fn m(pattern: &str, name: &str) -> bool {
    name_matches(pattern, OsStr::new(name))
}

#[test]
fn 이름_어디에_있든_부분_일치다() {
    assert!(m("port", "Quarterly Report.xlsx"));
    assert!(m("xlsx", "Quarterly Report.xlsx"));
    assert!(m("Quar", "Quarterly Report.xlsx"));
    assert!(!m("xyz", "Quarterly Report.xlsx"));
    assert!(m("", "아무 이름"), "빈 패턴은 모두 맞는다");
}

#[test]
fn 대소문자를_무시한다() {
    assert!(m("REPORT", "quarterly report"));
    assert!(m("report", "QUARTERLY REPORT"));
    assert!(m("ÄÖ", "xäöx"), "유니코드 글자도 접는다");
    assert!(m("k", "\u{212A}elvin"), "켈빈 기호는 k로 접힌다");
}

#[test]
fn 한글은_그대로_글자_단위로_맞춘다() {
    assert!(m("보고", "분기 보고서 001.txt"));
    assert!(m("고서 0", "분기 보고서 001.txt"));
    assert!(!m("보서", "분기 보고서 001.txt"));
    assert!(m("🎉", "photo 🎉.jpg"));
    assert!(m("보고서*.txt", "보고서 001.txt"));
}

#[test]
fn 별표와_물음표는_이름_전체에_대한_와일드카드다() {
    // 별표: 0글자 이상. 이름 전체가 맞아야 한다.
    assert!(m("*.pdf", "계약서.PDF"));
    assert!(!m("*.pdf", "계약서.pdf.bak"), "끝이 .pdf인 이름만");
    assert!(m("a*", "abc"));
    assert!(!m("a*", "bac"), "부분 일치가 아니라 이름 전체");
    assert!(m("a*c", "ac"));
    assert!(m("a*c", "abbbc"));
    assert!(!m("a*c", "abbbcd"));
    assert!(m("*", ""));
    assert!(m("**a**", "xxaxx"));
    // 물음표: 정확히 한 글자.
    assert!(m("a?c", "abc"));
    assert!(!m("a?c", "ac"));
    assert!(!m("a?c", "abbc"));
    assert!(m("???", "한글이"), "글자 단위");
    assert!(!m("??", "한글이"));
    // 둘 다.
    assert!(m("?a*", "xabc"));
    assert!(!m("?a*", "abc"));
    assert!(m("*.t?t", "memo.TXT"));
    assert!(!m("*.t?t", "memo.tt"));
    // 와일드카드 글자가 없는 패턴의 점은 그냥 글자다.
    assert!(m(".", "a.b"));
}

#[test]
fn 컴파일한_규칙은_한_번_맞추는_것과_같다() {
    let matcher = NameMatcher::new("*.RS");
    for name in ["lib.rs", "main.rs.bak", "🦀.rs", "rs"] {
        assert_eq!(
            matcher.is_match(OsStr::new(name)),
            m("*.RS", name),
            "{name}"
        );
    }
}
