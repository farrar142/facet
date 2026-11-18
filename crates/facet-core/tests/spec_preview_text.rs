// target: crates/facet-core/tests/spec_preview_text.rs
// AC: #336: 인코딩 추정(BOM UTF-8·UTF-16LE/BE, BOM 없는 UTF-8 한글·UTF-16LE/BE(ASCII만 담은 UTF-16 포함), CP949, Windows-1252), 빈 파일과 바이너리, 256KB 경계의 반쪽 글자와 잘림 표시·한도와 같은 크기·큰 파일, 없는 파일·잠긴 파일, 읽는 동안 이름바꾸기·지우기
//! 미리보기의 텍스트 읽기(`facet_core::preview`의 `detect_encoding`·`decode_text`·`read_text_head`)를 임시 폴더의 실제
//! 파일로 확인한다: 인코딩 추정 순서(BOM → UTF-8 → UTF-16 → CP949 → Windows-1252)와 글자, 바이너리 거르기, 앞부분만
//! 읽을 때 반쪽 글자를 버리고 잘림을 알리는 것, 이유 값. 잠긴 파일과 「읽는 동안 이름바꾸기·지우기」는 Windows에서만 돈다.

use std::fs;
use std::path::{Path, PathBuf};

use facet_core::preview::{
    PreviewProblem, TEXT_HEAD_LIMIT, TextEncoding, decode_text, detect_encoding, read_text_head,
};
use tempfile::TempDir;

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, bytes).unwrap();
    path
}

fn utf16le(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn utf16be(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_be_bytes).collect()
}

fn cp949(text: &str) -> Vec<u8> {
    let (bytes, _, unmappable) = encoding_rs::EUC_KR.encode(text);
    assert!(!unmappable, "시험 글자는 CP949로 쓸 수 있어야 한다");
    bytes.into_owned()
}

/// 파일로 쓰고 한도까지 읽어, 그 인코딩·글자로 잘리지 않고 나오는지 본다.
fn assert_reads(bytes: &[u8], encoding: TextEncoding, text: &str) {
    let dir = TempDir::new().unwrap();
    let path = write(dir.path(), "a.txt", bytes);
    assert_eq!(detect_encoding(bytes), Some(encoding), "추정한 인코딩");
    assert_eq!(decode_text(bytes, encoding), text, "디코딩한 글자");
    let preview = read_text_head(&path, TEXT_HEAD_LIMIT).expect("텍스트다");
    assert_eq!(preview.encoding, encoding);
    assert_eq!(preview.text, text);
    assert!(!preview.truncated, "한도보다 작은 파일은 잘리지 않는다");
}

/// 한국어와 영어가 섞인 여러 줄 글자.
const KOREAN: &str =
    "첫 줄: 로그 시작 2026-11-18\r\n둘째 줄: 한글 텍스트 파일입니다\r\nthird line: ok\r\n";

#[test]
fn the_head_limit_is_256_kilobytes() {
    assert_eq!(TEXT_HEAD_LIMIT, 256 * 1024);
}

#[test]
fn utf8_with_a_bom_reads_without_the_bom() {
    let text = "BOM 있는 UTF-8 🎉\n두 줄";
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(text.as_bytes());
    assert_reads(&bytes, TextEncoding::Utf8, text);
}

#[test]
fn utf16_with_a_bom_reads_in_both_byte_orders() {
    let text = "UTF-16 한글 텍스트 😀\r\n둘째 줄";
    let mut le = vec![0xFF, 0xFE];
    le.extend(utf16le(text));
    assert_reads(&le, TextEncoding::Utf16Le, text);

    let mut be = vec![0xFE, 0xFF];
    be.extend(utf16be(text));
    assert_reads(&be, TextEncoding::Utf16Be, text);
}

#[test]
fn utf8_korean_without_a_bom_is_utf8() {
    assert_reads(KOREAN.as_bytes(), TextEncoding::Utf8, KOREAN);
}

#[test]
fn utf16_without_a_bom_is_found_from_the_nul_pattern() {
    let text = "line 1: 로그 시작\r\nline 2: ok\r\n".repeat(20);
    assert_reads(&utf16le(&text), TextEncoding::Utf16Le, &text);
    assert_reads(&utf16be(&text), TextEncoding::Utf16Be, &text);
}

#[test]
fn ascii_only_utf16_without_a_bom_is_utf16_not_utf8_with_nuls() {
    // ASCII만 담은 UTF-16은 NUL이 섞인 올바른 UTF-8 바이트이기도 하다. NUL 모양이 UTF-8 검사보다 먼저다.
    let text = "plain ascii text\r\nsecond line\r\n".repeat(20);
    assert!(std::str::from_utf8(&utf16le(&text)).is_ok());
    assert_reads(&utf16le(&text), TextEncoding::Utf16Le, &text);
    assert_reads(&utf16be(&text), TextEncoding::Utf16Be, &text);
}

#[test]
fn cp949_korean_is_cp949() {
    let bytes = cp949(KOREAN);
    assert!(
        std::str::from_utf8(&bytes).is_err(),
        "UTF-8로는 맞지 않는 바이트"
    );
    assert_reads(&bytes, TextEncoding::Cp949, KOREAN);
}

#[test]
fn windows_1252_is_the_last_resort() {
    let text = "Café crème brûlée, naïve façade\r\n¿Qué? Olá!\r\n";
    let (bytes, _, unmappable) = encoding_rs::WINDOWS_1252.encode(text);
    assert!(!unmappable);
    assert!(
        encoding_rs::EUC_KR
            .decode_without_bom_handling_and_without_replacement(&bytes)
            .is_none(),
        "CP949로도 맞지 않는 바이트여야 한다"
    );
    assert_reads(&bytes, TextEncoding::Windows1252, text);
}

#[test]
fn an_empty_file_is_empty_utf8_text() {
    assert_eq!(detect_encoding(b""), Some(TextEncoding::Utf8));
    assert_reads(b"", TextEncoding::Utf8, "");
}

#[test]
fn binary_with_scattered_nul_bytes_is_not_text() {
    let mut bytes = b"\x7fELF\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00".to_vec();
    for i in 0..4096u32 {
        // NUL이 짝·홀 자리에 고루 흩어진다.
        bytes.push(if i % 3 == 0 { 0 } else { (i * 7 % 251) as u8 });
    }
    assert_eq!(detect_encoding(&bytes), None);
    let dir = TempDir::new().unwrap();
    let path = write(dir.path(), "a.bin", &bytes);
    assert_eq!(
        read_text_head(&path, TEXT_HEAD_LIMIT),
        Err(PreviewProblem::NotText)
    );
}

#[test]
fn a_three_byte_character_cut_at_the_limit_is_dropped() {
    let dir = TempDir::new().unwrap();
    let path = write(dir.path(), "big.txt", "가".repeat(100_000).as_bytes());
    let preview = read_text_head(&path, TEXT_HEAD_LIMIT).unwrap();
    assert_eq!(preview.encoding, TextEncoding::Utf8);
    assert!(preview.truncated, "한도를 넘었다");
    assert!(
        !preview.text.contains('\u{FFFD}'),
        "반쪽 글자가 깨진 글자로 나오면 안 된다"
    );
    assert!(preview.text.chars().all(|c| c == '가'));
    assert_eq!(preview.text.chars().count(), TEXT_HEAD_LIMIT / 3);
}

#[test]
fn a_surrogate_pair_cut_at_the_limit_is_dropped() {
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(utf16le(&"😀".repeat(80_000)));
    let dir = TempDir::new().unwrap();
    let path = write(dir.path(), "emoji.txt", &bytes);
    let preview = read_text_head(&path, TEXT_HEAD_LIMIT).unwrap();
    assert_eq!(preview.encoding, TextEncoding::Utf16Le);
    assert!(preview.truncated);
    assert!(!preview.text.contains('\u{FFFD}'));
    assert!(preview.text.chars().all(|c| c == '😀'));
    assert_eq!(preview.text.chars().count(), (TEXT_HEAD_LIMIT - 2) / 4);
}

#[test]
fn a_cp949_character_cut_at_the_limit_is_dropped() {
    // 앞에 한 바이트를 두어 두 바이트 글자가 한도에서 반으로 잘린다.
    let text = format!("a{}", "한".repeat(200_000));
    let dir = TempDir::new().unwrap();
    let path = write(dir.path(), "cp949.txt", &cp949(&text));
    let preview = read_text_head(&path, TEXT_HEAD_LIMIT).unwrap();
    assert_eq!(preview.encoding, TextEncoding::Cp949);
    assert!(preview.truncated);
    assert!(!preview.text.contains('\u{FFFD}'));
    let expected = format!("a{}", "한".repeat((TEXT_HEAD_LIMIT - 1) / 2));
    assert_eq!(preview.text, expected);
}

#[test]
fn a_file_exactly_at_the_limit_is_not_truncated() {
    let dir = TempDir::new().unwrap();
    let path = write(dir.path(), "exact.txt", &vec![b'a'; TEXT_HEAD_LIMIT]);
    let preview = read_text_head(&path, TEXT_HEAD_LIMIT).unwrap();
    assert!(!preview.truncated);
    assert_eq!(preview.text.len(), TEXT_HEAD_LIMIT);
}

#[test]
fn the_limit_parameter_bounds_the_text() {
    let dir = TempDir::new().unwrap();
    let path = write(dir.path(), "short.txt", b"0123456789abcdef");
    let preview = read_text_head(&path, 10).unwrap();
    assert_eq!(preview.text, "0123456789");
    assert!(preview.truncated);
}

#[test]
fn a_large_file_is_read_only_up_to_the_limit() {
    let line = "2026-11-18 12:00:00 INFO 서비스가 응답했다 id=12345\n";
    let dir = TempDir::new().unwrap();
    let path = write(dir.path(), "huge.log", line.repeat(200_000).as_bytes());
    assert!(fs::metadata(&path).unwrap().len() > 8 * 1024 * 1024);
    let preview = read_text_head(&path, TEXT_HEAD_LIMIT).unwrap();
    assert!(preview.truncated);
    assert!(preview.text.len() <= TEXT_HEAD_LIMIT);
    assert!(preview.text.starts_with(line));
}

#[test]
fn a_missing_file_is_missing() {
    let dir = TempDir::new().unwrap();
    assert_eq!(
        read_text_head(&dir.path().join("없음.txt"), TEXT_HEAD_LIMIT),
        Err(PreviewProblem::Missing)
    );
}

#[cfg(windows)]
#[test]
fn a_file_locked_without_sharing_is_locked() {
    use std::os::windows::fs::OpenOptionsExt;

    let dir = TempDir::new().unwrap();
    let path = write(dir.path(), "locked.txt", b"locked");
    let _held = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    assert_eq!(
        read_text_head(&path, TEXT_HEAD_LIMIT),
        Err(PreviewProblem::Locked)
    );
}

#[cfg(windows)]
#[test]
fn renaming_and_deleting_work_while_the_head_is_being_read() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let dir = TempDir::new().unwrap();
    let a = write(
        dir.path(),
        "a.txt",
        "읽는 중인 파일\n".repeat(50_000).as_bytes(),
    );
    let b = dir.path().join("b.txt");
    let stop = Arc::new(AtomicBool::new(false));
    let reader = {
        let stop = Arc::clone(&stop);
        let (a, b) = (a.clone(), b.clone());
        std::thread::spawn(move || {
            let mut reads = 0u32;
            while !stop.load(Ordering::SeqCst) {
                // 이름이 바뀌는 사이 없는 파일이면 실패해도 된다. 읽는 쪽이 막지 않는지만 본다.
                if read_text_head(&a, TEXT_HEAD_LIMIT).is_ok()
                    || read_text_head(&b, TEXT_HEAD_LIMIT).is_ok()
                {
                    reads += 1;
                }
            }
            reads
        })
    };
    std::thread::sleep(std::time::Duration::from_millis(50));
    for _ in 0..50 {
        fs::rename(&a, &b).expect("읽는 동안에도 이름을 바꾼다");
        fs::rename(&b, &a).expect("읽는 동안에도 이름을 바꾼다");
    }
    fs::remove_file(&a).expect("읽는 동안에도 지운다");
    stop.store(true, Ordering::SeqCst);
    let reads = reader.join().unwrap();
    assert!(reads > 0, "읽는 스레드가 실제로 읽었다");
    assert!(!a.exists() && !b.exists());
}
