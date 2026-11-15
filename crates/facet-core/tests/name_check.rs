//! 이름 확인·확장자 바뀜·처음 선택 범위. 코어 공개 인터페이스만 부른다.

use std::time::{Duration, Instant};

use facet_core::name_input::{
    MAX_NAME_UNITS, NameError, extension_changed, initial_selection, validate_name,
};

#[test]
fn forbidden_chars_each() {
    for c in ['\\', '/', ':', '*', '?', '"', '<', '>', '|'] {
        let n = format!("a{c}b");
        assert_eq!(validate_name(&n), Err(NameError::ForbiddenChar(c)));
    }
}

#[test]
fn control_chars() {
    for u in [0u32, 1, 9, 10, 0x1f] {
        let c = char::from_u32(u).unwrap();
        assert_eq!(
            validate_name(&format!("a{c}")),
            Err(NameError::ControlChar(c))
        );
    }
}

#[test]
fn reserved_names() {
    for n in [
        "CON",
        "con",
        "Prn",
        "aUx",
        "NUL",
        "COM1",
        "com9",
        "LPT1",
        "lpt9",
        "CON.txt",
        "nul.tar.gz",
        "Com3.TXT",
        "CON .txt",
    ] {
        assert_eq!(validate_name(n), Err(NameError::Reserved), "{n}");
    }
    for n in [
        "COM0", "LPT0", "CONSOLE", "COM10", "xCON", "CON1", "COM", "a.CON",
    ] {
        assert_eq!(validate_name(n), Ok(()), "{n}");
    }
}

#[test]
fn trailing_dot_space_and_empty() {
    assert_eq!(validate_name(""), Err(NameError::Empty));
    assert_eq!(validate_name("a."), Err(NameError::TrailingDot));
    assert_eq!(validate_name(".."), Err(NameError::TrailingDot));
    assert_eq!(validate_name("a "), Err(NameError::TrailingSpace));
    assert_eq!(validate_name(" a"), Ok(()));
    assert_eq!(validate_name(".gitignore"), Ok(()));
}

#[test]
fn length_boundary_in_utf16_units() {
    assert_eq!(MAX_NAME_UNITS, 255);
    assert_eq!(validate_name(&"a".repeat(255)), Ok(()));
    assert_eq!(
        validate_name(&"a".repeat(256)),
        Err(NameError::TooLong { units: 256 })
    );
    // 이모지는 두 단위.
    assert_eq!(validate_name(&format!("{}😀", "a".repeat(253))), Ok(()));
    assert_eq!(
        validate_name(&format!("{}😀", "a".repeat(254))),
        Err(NameError::TooLong { units: 256 })
    );
    assert_eq!(validate_name(&"가".repeat(255)), Ok(()));
    assert_eq!(
        validate_name(&"가".repeat(256)),
        Err(NameError::TooLong { units: 256 })
    );
}

#[test]
fn korean_and_emoji_allowed() {
    assert_eq!(validate_name("한글 파일.txt"), Ok(()));
    assert_eq!(validate_name("😀 모음.png"), Ok(()));
}

#[test]
fn extension_change() {
    assert!(!extension_changed("a.txt", "b.txt", false));
    assert!(extension_changed("a.txt", "a.md", false));
    assert!(extension_changed("a.txt", "a", false));
    assert!(extension_changed("a", "a.txt", false));
    assert!(extension_changed("a.tar.gz", "a.tar.bz2", false));
    assert!(!extension_changed("a.tar.gz", "b.tar.gz", false));
    assert!(
        !extension_changed("a.txt", "a.TXT", false),
        "대소문자만 다르면 같은 확장자"
    );
    assert!(!extension_changed(".gitignore", ".gitattributes", false));
    assert!(!extension_changed("폴더.d", "폴더.e", true));
    assert!(!extension_changed("한글.텍스트", "이모지😀.텍스트", false));
    assert!(extension_changed("한글.텍스트", "한글.문서", false));
}

#[test]
fn initial_selection_ranges() {
    assert_eq!(initial_selection("report.txt", false), (0, 6));
    assert_eq!(initial_selection("a.tar.gz", false), (0, 5));
    assert_eq!(initial_selection("README", false), (0, 6));
    assert_eq!(initial_selection(".gitignore", false), (0, 10));
    assert_eq!(initial_selection("a.", false), (0, 2));
    assert_eq!(initial_selection("v1.2", true), (0, 4));
    assert_eq!(initial_selection("보고서.txt", false), (0, 9));
    assert_eq!(initial_selection("😀.png", false), (0, 4));
    assert_eq!(initial_selection("", false), (0, 0));
}

#[test]
fn validating_a_255_unit_name_takes_under_1ms() {
    let name = "가".repeat(254) + "a";
    assert_eq!(validate_name(&name), Ok(()));
    let mut worst = Duration::ZERO;
    for _ in 0..20 {
        let t = Instant::now();
        assert_eq!(validate_name(std::hint::black_box(&name)), Ok(()));
        worst = worst.max(t.elapsed());
    }
    assert!(worst < Duration::from_millis(1), "{worst:?}");
}
