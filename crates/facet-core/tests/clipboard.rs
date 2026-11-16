use facet_core::clipboard::*;
use std::path::PathBuf;

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

#[test]
fn hdrop_round_trips() {
    let long = format!(r"C:\{}파일.txt", r"아주긴폴더이름\".repeat(40));
    assert!(long.chars().count() > 260);
    for paths in [
        vec![p(r"C:\a\b.txt")],
        vec![p(r"C:\한글\문서.txt"), p(r"D:\😀\x.png"), p(&long)],
        vec![],
    ] {
        let bytes = hdrop_bytes(&paths);
        assert_eq!(parse_hdrop(&bytes).unwrap(), paths);
    }
}

#[test]
fn hdrop_header_layout() {
    let bytes = hdrop_bytes(&[p("a")]);
    assert_eq!(&bytes[..4], &20u32.to_le_bytes());
    assert_eq!(&bytes[16..20], &1u32.to_le_bytes());
    assert_eq!(&bytes[20..], &[b'a', 0, 0, 0, 0, 0]);
}

#[test]
fn broken_bytes_are_errors_not_panics() {
    let good = hdrop_bytes(&[p(r"C:\a.txt"), p(r"C:\b.txt")]);
    for cut in 0..good.len() {
        // 어떤 길이로 잘라도 패닉하지 않는다.
        let _ = parse_hdrop(&good[..cut]);
    }
    assert_eq!(parse_hdrop(&good[..10]), Err(HdropError::TooShort));
    assert_eq!(
        parse_hdrop(&good[..good.len() - 2]),
        Err(HdropError::Unterminated)
    );
    assert_eq!(
        parse_hdrop(&good[..good.len() - 1]),
        Err(HdropError::OddLength)
    );
    let mut bad = good.clone();
    bad[0..4].copy_from_slice(&9999u32.to_le_bytes());
    assert_eq!(parse_hdrop(&bad), Err(HdropError::BadOffset));
    bad[0..4].copy_from_slice(&4u32.to_le_bytes());
    assert_eq!(parse_hdrop(&bad), Err(HdropError::BadOffset));
    let mut ansi = good;
    ansi[16..20].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(parse_hdrop(&ansi), Err(HdropError::Ansi));
}

#[test]
fn drop_effect_round_trips_and_unknown_is_copy() {
    for op in [ClipboardOperation::Copy, ClipboardOperation::Move] {
        assert_eq!(
            parse_preferred_drop_effect(&preferred_drop_effect_bytes(op)),
            Some(op)
        );
    }
    assert_eq!(
        preferred_drop_effect_bytes(ClipboardOperation::Copy),
        [1, 0, 0, 0]
    );
    assert_eq!(
        preferred_drop_effect_bytes(ClipboardOperation::Move),
        [2, 0, 0, 0]
    );
    assert_eq!(
        performed_drop_effect_bytes(ClipboardOperation::Move),
        [2, 0, 0, 0]
    );
    assert_eq!(
        paste_succeeded_bytes(ClipboardOperation::Move),
        [2, 0, 0, 0]
    );
    for v in [0u32, 3, 4, 0xFFFF_FFFF] {
        assert_eq!(
            parse_preferred_drop_effect(&v.to_le_bytes()),
            Some(ClipboardOperation::Copy)
        );
    }
    assert_eq!(parse_preferred_drop_effect(&[2, 0, 0]), None);
    assert_eq!(parse_preferred_drop_effect(&[]), None);
}

#[test]
fn copy_as_path_quotes_each_line() {
    assert_eq!(
        copy_as_path_text(&[p(r"C:\a b\c.txt")]),
        "\"C:\\a b\\c.txt\""
    );
    assert_eq!(
        copy_as_path_text(&[p(r"C:\a"), p(r"C:\한글")]),
        "\"C:\\a\"\r\n\"C:\\한글\""
    );
    assert_eq!(copy_as_path_text::<PathBuf>(&[]), "");
}
