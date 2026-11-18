// target: crates/facet-core/tests/spec_preview_kind.rs
// AC: #337: 텍스트·이미지 확장자와 폴더, 확장자 없음·모르는 확장자의 NUL 판단, 거짓 확장자, 처리기 조회, 온라인 전용(파일을 열지 않음), 없는·잠긴 파일, 내용 만들기(텍스트·폴더 항목 수·정보만·도우미·이유)
//! 미리보기 종류 고르기(`facet_core::preview::choose_kind`)와 내용 만들기(`load_preview`)를 임시 폴더의 실제 파일로
//! 확인한다. 처리기 조회는 시험이 넘기는 함수다(셸을 부르지 않음). 잠긴 파일·숨김 속성은 Windows에서만 돈다.

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::SystemTime;

use facet_core::preview::{
    PreviewBody, PreviewKind, PreviewProblem, PreviewRequest, ShellPreview, TEXT_HEAD_LIMIT,
    TextEncoding, choose_kind, load_preview,
};
use facet_core::{Attributes, Item};
use tempfile::TempDir;

fn no_handler(_: &str) -> bool {
    false
}

fn any_handler(_: &str) -> bool {
    true
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, bytes).unwrap();
    path
}

/// 실제 파일·폴더의 목록 항목(속성은 꺼짐).
fn item_of(path: &Path) -> Item {
    let meta = fs::metadata(path).unwrap();
    Item {
        name: path.file_name().unwrap().to_os_string(),
        is_folder: meta.is_dir(),
        size: meta.len(),
        modified: meta.modified().unwrap(),
        attributes: Attributes::default(),
    }
}

/// 온라인 전용 표시가 있는 항목(파일이 없어도 된다).
fn online_item(name: &str) -> Item {
    Item {
        name: name.into(),
        is_folder: false,
        size: 1234,
        modified: SystemTime::UNIX_EPOCH,
        attributes: Attributes {
            online_only: true,
            ..Attributes::default()
        },
    }
}

fn kind(path: &Path, has_handler: &dyn Fn(&str) -> bool) -> PreviewKind {
    choose_kind(path, &item_of(path), has_handler)
}

fn binary() -> Vec<u8> {
    let mut bytes = b"PK\x03\x04\x14\x00\x00\x00\x08\x00".to_vec();
    for i in 0..2048u32 {
        bytes.push(if i % 3 == 0 { 0 } else { (i * 13 % 251) as u8 });
    }
    bytes
}

fn utf16le(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn request(path: &Path, show_hidden: bool) -> PreviewRequest {
    PreviewRequest {
        path: path.to_path_buf(),
        item: item_of(path),
        show_hidden,
        image_side: 256,
    }
}

fn load(request: &PreviewRequest, has_handler: &dyn Fn(&str) -> bool) -> PreviewBody {
    load_preview(request, &AtomicBool::new(false), has_handler)
}

#[test]
#[ignore = "#337"]
fn text_extensions_are_text_in_any_case() {
    let dir = TempDir::new().unwrap();
    for name in [
        "a.txt", "b.log", "c.md", "d.json", "e.csv", "f.xml", "g.ini", "h.rs", "I.TXT", "j.Log",
    ] {
        let path = write(dir.path(), name, "내용 content\n".as_bytes());
        assert_eq!(kind(&path, &no_handler), PreviewKind::Text, "{name}");
    }
}

#[test]
#[ignore = "#337"]
fn image_extensions_are_images_in_any_case() {
    let dir = TempDir::new().unwrap();
    for name in [
        "a.jpg", "b.jpeg", "c.png", "d.gif", "e.webp", "f.bmp", "G.JPG", "h.Png",
    ] {
        let path = write(dir.path(), name, b"not decoded at this step");
        assert_eq!(kind(&path, &any_handler), PreviewKind::Image, "{name}");
    }
}

#[test]
#[ignore = "#337"]
fn a_folder_is_a_folder() {
    let dir = TempDir::new().unwrap();
    let folder = dir.path().join("사진.txt");
    fs::create_dir(&folder).unwrap();
    assert_eq!(kind(&folder, &any_handler), PreviewKind::Folder);
}

#[test]
#[ignore = "#337"]
fn files_without_a_known_extension_are_judged_by_nul_bytes() {
    let dir = TempDir::new().unwrap();
    let readme = write(dir.path(), "README", "설명서\nhello\n".as_bytes());
    assert_eq!(kind(&readme, &no_handler), PreviewKind::Text);
    let blob = write(dir.path(), "blob", &binary());
    assert_eq!(kind(&blob, &no_handler), PreviewKind::Info);
    let conf = write(dir.path(), "app.conf9", b"key=value\n");
    assert_eq!(kind(&conf, &no_handler), PreviewKind::Text);
    let data = write(dir.path(), "data.xyz", &binary());
    assert_eq!(kind(&data, &no_handler), PreviewKind::Info);
    let empty = write(dir.path(), "empty.xyz", b"");
    assert_eq!(kind(&empty, &no_handler), PreviewKind::Text);
}

#[test]
#[ignore = "#337"]
fn utf16_text_without_a_bom_is_not_mistaken_for_binary() {
    let dir = TempDir::new().unwrap();
    let path = write(
        dir.path(),
        "utf16.txt",
        &utf16le(&"line 1: 로그\r\nline 2: ok\r\n".repeat(20)),
    );
    assert_eq!(kind(&path, &no_handler), PreviewKind::Text);
    let unknown = write(
        dir.path(),
        "utf16.xyz",
        &utf16le(&"plain ascii text\r\n".repeat(20)),
    );
    assert_eq!(kind(&unknown, &no_handler), PreviewKind::Text);
}

#[test]
#[ignore = "#337"]
fn a_binary_with_a_text_extension_is_info_only() {
    let dir = TempDir::new().unwrap();
    let path = write(dir.path(), "fake.txt", &binary());
    assert_eq!(kind(&path, &any_handler), PreviewKind::Info);
}

#[test]
#[ignore = "#337"]
fn a_registered_handler_makes_it_a_shell_preview() {
    let dir = TempDir::new().unwrap();
    let pdf = write(dir.path(), "문서.PDF", &binary());
    let asked = RefCell::new(Vec::<String>::new());
    let pdf_only = |ext: &str| {
        asked.borrow_mut().push(ext.to_string());
        ext == "pdf"
    };
    assert_eq!(kind(&pdf, &pdf_only), PreviewKind::Shell);
    assert_eq!(*asked.borrow(), ["pdf"], "소문자·점 없는 확장자로 묻는다");

    let zip = write(dir.path(), "a.zip", &binary());
    assert_eq!(
        kind(&zip, &pdf_only),
        PreviewKind::Info,
        "처리기가 없으면 정보만"
    );
    assert_eq!(kind(&pdf, &no_handler), PreviewKind::Info);

    let text = write(dir.path(), "a.txt", b"hello");
    assert_eq!(
        kind(&text, &any_handler),
        PreviewKind::Text,
        "텍스트는 자체 미리보기"
    );
}

#[test]
#[ignore = "#337"]
fn online_only_items_are_not_previewed_and_not_opened() {
    let dir = TempDir::new().unwrap();
    for name in ["없음.txt", "없음.jpg", "없음.pdf", "없음"] {
        let path = dir.path().join(name);
        assert_eq!(
            choose_kind(&path, &online_item(name), &any_handler),
            PreviewKind::Unavailable(PreviewProblem::OnlineOnly),
            "{name}: 파일이 없어도 읽지 않고 정한다"
        );
    }
}

#[cfg(windows)]
#[test]
#[ignore = "#337"]
fn online_only_items_are_decided_even_when_the_file_is_locked() {
    use std::os::windows::fs::OpenOptionsExt;

    let dir = TempDir::new().unwrap();
    let path = write(dir.path(), "cloud.log", b"remote");
    let _held = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    assert_eq!(
        choose_kind(&path, &online_item("cloud.log"), &no_handler),
        PreviewKind::Unavailable(PreviewProblem::OnlineOnly)
    );
    let req = PreviewRequest {
        path: path.clone(),
        item: online_item("cloud.log"),
        show_hidden: true,
        image_side: 256,
    };
    assert_eq!(
        load(&req, &no_handler),
        PreviewBody::Unavailable(PreviewProblem::OnlineOnly)
    );
}

#[test]
#[ignore = "#337"]
fn a_missing_file_is_missing() {
    let dir = TempDir::new().unwrap();
    let path = write(dir.path(), "gone.xyz", b"x");
    let item = item_of(&path);
    fs::remove_file(&path).unwrap();
    assert_eq!(
        choose_kind(&path, &item, &no_handler),
        PreviewKind::Unavailable(PreviewProblem::Missing)
    );
}

#[cfg(windows)]
#[test]
#[ignore = "#337"]
fn a_locked_file_is_locked() {
    use std::os::windows::fs::OpenOptionsExt;

    let dir = TempDir::new().unwrap();
    let path = write(dir.path(), "locked.xyz", b"x");
    let item = item_of(&path);
    let _held = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    assert_eq!(
        choose_kind(&path, &item, &no_handler),
        PreviewKind::Unavailable(PreviewProblem::Locked)
    );
}

#[test]
#[ignore = "#337"]
fn loading_a_text_file_gives_its_text() {
    let dir = TempDir::new().unwrap();
    let path = write(dir.path(), "메모.txt", "첫 줄\n둘째 줄\n".as_bytes());
    match load(&request(&path, false), &no_handler) {
        PreviewBody::Text(text) => {
            assert_eq!(text.text, "첫 줄\n둘째 줄\n");
            assert_eq!(text.encoding, TextEncoding::Utf8);
            assert!(!text.truncated);
        }
        other => panic!("텍스트여야 한다: {other:?}"),
    }

    let big = write(dir.path(), "big.log", &vec![b'x'; TEXT_HEAD_LIMIT + 10]);
    match load(&request(&big, false), &no_handler) {
        PreviewBody::Text(text) => {
            assert!(text.truncated, "256KB를 넘으면 잘림 표시");
            assert_eq!(text.text.len(), TEXT_HEAD_LIMIT);
        }
        other => panic!("텍스트여야 한다: {other:?}"),
    }
}

#[test]
#[ignore = "#337"]
fn loading_a_folder_counts_its_items() {
    let dir = TempDir::new().unwrap();
    let folder = dir.path().join("폴더");
    fs::create_dir(&folder).unwrap();
    write(&folder, "a.txt", b"a");
    write(&folder, "b.txt", b"b");
    fs::create_dir(folder.join("sub")).unwrap();
    assert_eq!(
        load(&request(&folder, true), &no_handler),
        PreviewBody::Folder { count: Some(3) }
    );
}

#[cfg(windows)]
#[test]
#[ignore = "#337"]
fn loading_a_folder_follows_the_hidden_items_setting() {
    let dir = TempDir::new().unwrap();
    let folder = dir.path().join("폴더");
    fs::create_dir(&folder).unwrap();
    write(&folder, "a.txt", b"a");
    let hidden = write(&folder, "hidden.txt", b"h");
    let status = std::process::Command::new("attrib")
        .arg("+h")
        .arg(&hidden)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(
        load(&request(&folder, false), &no_handler),
        PreviewBody::Folder { count: Some(1) }
    );
    assert_eq!(
        load(&request(&folder, true), &no_handler),
        PreviewBody::Folder { count: Some(2) }
    );
}

#[test]
#[ignore = "#337"]
fn loading_info_shell_and_problems_gives_those_values() {
    let dir = TempDir::new().unwrap();
    let zip = write(dir.path(), "a.zip", &binary());
    assert_eq!(load(&request(&zip, false), &no_handler), PreviewBody::Info);

    let pdf = write(dir.path(), "a.pdf", &binary());
    assert_eq!(
        load(&request(&pdf, false), &any_handler),
        PreviewBody::Shell(ShellPreview::Loading)
    );

    let gone = write(dir.path(), "gone.txt", b"x");
    let req = request(&gone, false);
    fs::remove_file(&gone).unwrap();
    assert_eq!(
        load(&req, &no_handler),
        PreviewBody::Unavailable(PreviewProblem::Missing)
    );

    let online = PreviewRequest {
        path: dir.path().join("cloud.txt"),
        item: online_item("cloud.txt"),
        show_hidden: false,
        image_side: 256,
    };
    assert_eq!(
        load(&online, &no_handler),
        PreviewBody::Unavailable(PreviewProblem::OnlineOnly)
    );
}
