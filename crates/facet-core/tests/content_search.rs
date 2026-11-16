//! 내용 검색 엔진(`content_search`)의 시험: 임시 폴더의 실제 파일.

use std::fs;
use std::path::Path;
use std::time::Duration;

use facet_core::Location;
use facet_core::content_search::{
    ContentEnd, ContentError, ContentEvent, ContentFound, ContentMode, ContentRequest,
    ContentSearch, ContentSummary, MAX_LINE_CHARS, Skipped, decode, is_online_only, looks_binary,
};

struct Outcome {
    found: Vec<ContentFound>,
    summary: ContentSummary,
}

impl Outcome {
    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .found
            .iter()
            .map(|f| f.item.name.to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

fn run(request: ContentRequest) -> Outcome {
    let mut search = ContentSearch::start(request).expect("시작해야 한다");
    let mut found = Vec::new();
    loop {
        match search.wait_event(Duration::from_secs(60)) {
            Some(ContentEvent::Batch { items, .. }) => found.extend(items),
            Some(ContentEvent::Finished(summary)) => return Outcome { found, summary },
            None => panic!("검색이 끝나지 않았다"),
        }
    }
}

fn request(dir: &Path, text: &str) -> ContentRequest {
    ContentRequest::new(Location::file_system(dir), text)
}

fn write(dir: &Path, name: &str, bytes: &[u8]) {
    let path = dir.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn utf16(text: &str, little: bool) -> Vec<u8> {
    let mut bytes = if little {
        vec![0xFF, 0xFE]
    } else {
        vec![0xFE, 0xFF]
    };
    for unit in text.encode_utf16() {
        bytes.extend(if little {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        });
    }
    bytes
}

#[test]
fn 글자_일치는_대소문자를_무시하고_하위_폴더도_훑는다() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.txt", b"Hello World\nsecond");
    write(dir.path(), "sub/deep/b.txt", b"say HELLO there");
    write(dir.path(), "c.txt", b"nothing here");
    let outcome = run(request(dir.path(), "hello"));
    assert_eq!(outcome.names(), ["a.txt", "b.txt"]);
    assert_eq!(outcome.summary.end, ContentEnd::Finished);
    let progress = outcome.summary.progress;
    assert_eq!(progress.found, 2);
    assert_eq!(progress.files_read, 3);
    assert_eq!(progress.skipped, Skipped::default());
    assert_eq!(progress.folders_scanned, 3);
}

#[test]
fn 글자는_정규식_기호를_그대로_찾는다() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.txt", b"price (a+b)* done");
    write(dir.path(), "b.txt", b"aab");
    let outcome = run(request(dir.path(), "(a+b)*"));
    assert_eq!(outcome.names(), ["a.txt"]);
}

#[test]
fn 첫_일치_줄과_일치_수를_낸다() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "a.txt",
        b"first line\r\n    the needle is here  \r\nneedle again, NEEDLE\r\n",
    );
    let outcome = run(request(dir.path(), "needle"));
    let [found] = outcome.found.as_slice() else {
        panic!("{:?}", outcome.names());
    };
    assert_eq!(found.first_line, "the needle is here");
    assert_eq!(found.matches, 3);
    assert_eq!(found.item.size, 60);
    assert_eq!(&*found.folder, dir.path());
}

#[test]
fn 긴_줄은_일치가_보이게_상한까지만_자른다() {
    let dir = tempfile::tempdir().unwrap();
    let line = format!("{}NEEDLE{}", "가".repeat(500), "나".repeat(500));
    write(dir.path(), "a.txt", line.as_bytes());
    let outcome = run(request(dir.path(), "needle"));
    let first = &outcome.found[0].first_line;
    assert!(first.contains("NEEDLE"), "{first}");
    assert!(first.starts_with('…') && first.ends_with('…'));
    assert_eq!(first.chars().count(), MAX_LINE_CHARS + 2);
    // 앞쪽 일치는 앞이 잘리지 않는다.
    write(
        dir.path(),
        "b.txt",
        format!("NEEDLE{}", "x".repeat(500)).as_bytes(),
    );
    let outcome = run(request(dir.path(), "needle"));
    let b = outcome
        .found
        .iter()
        .find(|f| f.item.name == "b.txt")
        .unwrap();
    assert!(b.first_line.starts_with("NEEDLE") && b.first_line.ends_with('…'));
}

#[test]
fn 정규식은_줄마다_맞고_대소문자를_구분한다() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.txt", b"x\nid=42\nid=7\n");
    write(dir.path(), "b.txt", b"ID=1");
    let mut req = request(dir.path(), r"^id=\d+$");
    req.mode = ContentMode::Regex;
    let outcome = run(req);
    assert_eq!(outcome.names(), ["a.txt"]);
    assert_eq!(outcome.found[0].matches, 2);
    assert_eq!(outcome.found[0].first_line, "id=42");
}

#[test]
fn 파일_이름_조건은_필터와_같은_규칙이고_비우면_모든_파일이다() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["a.txt", "b.md", "note.txt"] {
        write(dir.path(), name, b"needle");
    }
    let mut req = request(dir.path(), "needle");
    req.name_pattern = "*.txt".to_owned();
    assert_eq!(run(req).names(), ["a.txt", "note.txt"]);
    let mut req = request(dir.path(), "needle");
    req.name_pattern = "NOTE".to_owned();
    assert_eq!(run(req).names(), ["note.txt"]);
    assert_eq!(
        run(request(dir.path(), "needle")).names(),
        ["a.txt", "b.md", "note.txt"]
    );
}

#[test]
fn 폴더_이름은_파일_이름_조건에_들지_않고_폴더는_결과가_아니다() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "needle/inner.txt", b"needle");
    let mut req = request(dir.path(), "needle");
    req.name_pattern = "needle".to_owned();
    assert!(run(req).found.is_empty());
}

#[test]
fn 인코딩을_가려_한글_본문을_찾는다() {
    let dir = tempfile::tempdir().unwrap();
    let text = "첫 줄\n안녕하세요 세계\n끝";
    write(dir.path(), "utf8.txt", text.as_bytes());
    let mut bom = vec![0xEF, 0xBB, 0xBF];
    bom.extend(text.as_bytes());
    write(dir.path(), "utf8bom.txt", &bom);
    write(dir.path(), "utf16le.txt", &utf16(text, true));
    write(dir.path(), "utf16be.txt", &utf16(text, false));
    let (cp949, _, bad) = encoding_rs::EUC_KR.encode(text);
    assert!(!bad);
    assert!(std::str::from_utf8(&cp949).is_err());
    write(dir.path(), "cp949.txt", &cp949);
    let outcome = run(request(dir.path(), "세계"));
    assert_eq!(
        outcome.names(),
        [
            "cp949.txt",
            "utf16be.txt",
            "utf16le.txt",
            "utf8.txt",
            "utf8bom.txt"
        ]
    );
    for found in &outcome.found {
        assert_eq!(found.first_line, "안녕하세요 세계", "{:?}", found.item.name);
        assert_eq!(found.matches, 1);
    }
    assert_eq!(outcome.summary.progress.skipped, Skipped::default());
}

#[test]
fn 디코딩은_bom을_걷고_없으면_utf8_다음_cp949다() {
    assert_eq!(decode(&[0xEF, 0xBB, 0xBF, b'a']), "a");
    assert_eq!(decode(&[0xFF, 0xFE, b'a', 0]), "a");
    assert_eq!(decode(&[0xFE, 0xFF, 0, b'a']), "a");
    assert_eq!(decode("가".as_bytes()), "가");
    assert_eq!(decode(&[0xB0, 0xA1]), "가");
}

#[test]
fn 바이너리는_건너뛰어_센다() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.bin", b"needle\0\x01\x02");
    // NUL이 앞 8KB 밖에 있으면 바이너리로 보지 않는다.
    let mut late = b"needle".to_vec();
    late.extend(vec![b'x'; 9000]);
    late.push(0);
    write(dir.path(), "late.txt", &late);
    let outcome = run(request(dir.path(), "needle"));
    assert_eq!(outcome.names(), ["late.txt"]);
    assert_eq!(outcome.summary.progress.skipped.binary, 1);
    assert_eq!(outcome.summary.progress.skipped.total(), 1);
}

#[test]
fn 바이너리_판정은_utf16_bom이_있으면_예외다() {
    assert!(looks_binary(b"ab\0cd"));
    assert!(!looks_binary(b"abcd"));
    assert!(!looks_binary(&[0xFF, 0xFE, b'a', 0]));
    assert!(!looks_binary(&[0xFE, 0xFF, 0, b'a']));
}

#[test]
fn 한도를_넘는_파일은_열지_않고_건너뛴다() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "small.txt", b"needle");
    write(dir.path(), "big.txt", b"needle and more bytes");
    let mut req = request(dir.path(), "needle");
    req.max_file_size = 10;
    let outcome = run(req);
    assert_eq!(outcome.names(), ["small.txt"]);
    assert_eq!(outcome.summary.progress.skipped.too_large, 1);
    assert_eq!(outcome.summary.progress.files_read, 1);
    assert_eq!(outcome.summary.progress.bytes_read, 6);
    // 한도와 같은 크기는 읽는다.
    let mut req = request(dir.path(), "needle");
    req.max_file_size = 21;
    assert_eq!(run(req).names(), ["big.txt", "small.txt"]);
}

#[test]
fn 온라인_전용_속성은_파일을_열지_않고_속성_값만_본다() {
    assert!(is_online_only(0x1000)); // OFFLINE
    assert!(is_online_only(0x4_0000)); // RECALL_ON_OPEN
    assert!(is_online_only(0x40_0000)); // RECALL_ON_DATA_ACCESS
    assert!(is_online_only(0x40_0000 | 0x20));
    assert!(!is_online_only(0x20)); // ARCHIVE
    assert!(!is_online_only(0x2 | 0x4 | 0x1)); // 숨김·시스템·읽기 전용
    assert!(!is_online_only(0));
}

#[test]
fn 숨김_파일은_보이기를_켜야_읽는다() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "visible.txt", b"needle");
    write(dir.path(), "hidden.txt", b"needle");
    #[cfg(windows)]
    {
        let status = std::process::Command::new("attrib")
            .arg("+h")
            .arg(dir.path().join("hidden.txt"))
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(run(request(dir.path(), "needle")).names(), ["visible.txt"]);
        let mut req = request(dir.path(), "needle");
        req.show_hidden = true;
        assert_eq!(run(req).names(), ["hidden.txt", "visible.txt"]);
    }
}

#[test]
fn 틀린_정규식은_시작_전에_까닭이_담긴_오류다() {
    let dir = tempfile::tempdir().unwrap();
    let mut req = request(dir.path(), "(unclosed");
    req.mode = ContentMode::Regex;
    let Err(ContentError::Regex(reason)) = ContentSearch::start(req) else {
        panic!("시작하지 말아야 한다");
    };
    assert!(!reason.is_empty());
    // 같은 글자도 글자 일치로는 시작한다.
    assert!(ContentSearch::start(request(dir.path(), "(unclosed")).is_ok());
}

#[test]
fn 빈_글자와_폴더가_아닌_시작점은_시작하지_않는다() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        ContentSearch::start(request(dir.path(), "")),
        Err(ContentError::EmptyText)
    ));
    write(dir.path(), "f.txt", b"x");
    assert!(matches!(
        ContentSearch::start(request(&dir.path().join("f.txt"), "x")),
        Err(ContentError::Location(_))
    ));
    assert!(matches!(
        ContentSearch::start(request(&dir.path().join("없음"), "x")),
        Err(ContentError::Location(_))
    ));
}

#[test]
fn 취소하면_끝남은_취소이고_그_뒤_묶음은_오지_않는다() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..200 {
        write(dir.path(), &format!("d{}/f{i}.txt", i % 20), b"needle");
    }
    let mut search = ContentSearch::start(request(dir.path(), "needle")).unwrap();
    search.cancel();
    let mut events = Vec::new();
    while let Some(event) = search.wait_event(Duration::from_secs(10)) {
        let done = matches!(event, ContentEvent::Finished(_));
        events.push(event);
        if done {
            break;
        }
    }
    assert!(
        events
            .iter()
            .all(|e| matches!(e, ContentEvent::Finished(_)))
    );
    let Some(ContentEvent::Finished(summary)) = events.pop() else {
        panic!("끝남이 없다");
    };
    assert_eq!(summary.end, ContentEnd::Cancelled);
    assert!(search.poll().is_empty());
}

#[test]
fn 일치하는_파일이_없으면_빈_결과로_끝난다() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.txt", b"abc");
    let outcome = run(request(dir.path(), "zzz"));
    assert!(outcome.found.is_empty());
    assert_eq!(outcome.summary.end, ContentEnd::Finished);
    assert_eq!(outcome.summary.progress.files_read, 1);
}

#[cfg(windows)]
mod windows {
    use super::*;
    use facet_core::content_search;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::windows::fs::OpenOptionsExt;

    #[test]
    fn 다른_핸들이_배타로_연_파일은_읽지_못한_파일로_센다() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "ok.txt", b"needle");
        write(dir.path(), "locked.txt", b"needle");
        let _lock = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(dir.path().join("locked.txt"))
            .unwrap();
        let outcome = run(request(dir.path(), "needle"));
        assert_eq!(outcome.names(), ["ok.txt"]);
        assert_eq!(outcome.summary.progress.skipped.unreadable, 1);
    }

    /// 검색이 파일을 여는 방식(`open_shared`)으로 열어 둔 동안 다른 핸들로 쓰고 지울 수 있어야 한다.
    #[test]
    fn 검색이_연_방식의_핸들이_열려_있어도_쓰고_지울_수_있다() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        fs::write(&path, b"needle").unwrap();
        let _held = content_search::open_shared(&path).unwrap();
        let mut writer = OpenOptions::new().append(true).open(&path).unwrap();
        writer.write_all(b" more").unwrap();
        drop(writer);
        fs::remove_file(&path).unwrap();
        assert!(!path.exists());
    }

    /// 검색이 큰 파일을 읽는 동안에도 같은 파일에 쓰고 다른 파일을 지울 수 있다.
    #[test]
    fn 검색이_도는_동안에도_쓰고_지울_수_있다() {
        let dir = tempfile::tempdir().unwrap();
        let big = vec![b'x'; 20_000_000];
        for i in 0..8 {
            write(dir.path(), &format!("big{i}.txt"), &big);
        }
        let target = dir.path().join("big0.txt");
        let mut search = ContentSearch::start(request(dir.path(), "needle")).unwrap();
        let mut appended = false;
        let mut removed = false;
        let mut finished = false;
        for _ in 0..2000 {
            if !appended {
                if let Ok(mut f) = OpenOptions::new().append(true).open(&target) {
                    f.write_all(b"needle").unwrap();
                    appended = true;
                }
            } else if !removed && fs::remove_file(dir.path().join("big7.txt")).is_ok() {
                removed = true;
            }
            if let Some(ContentEvent::Finished(_)) = search.wait_event(Duration::from_millis(5)) {
                finished = true;
                break;
            }
        }
        assert!(appended && removed, "쓰기·지우기가 거부되었다");
        assert!(finished);
    }
}
