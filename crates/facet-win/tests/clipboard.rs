//! 실제 클립보드 왕복(Windows 전용). 클립보드는 프로세스 밖까지 하나라 시험을 차례로 돌린다. 클립보드를 열 수
//! 없는 환경(대화형 세션이 없는 러너 등)이면 건너뛰었다고 출력한다. 시험 전 클립보드 글자는 끝나면 되돌린다.
#![cfg(windows)]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use facet_win::clipboard::format::ClipboardOperation;
use facet_win::clipboard::{Clipboard, ClipboardFiles};

static SERIAL: Mutex<()> = Mutex::new(());

/// 차례를 잡고 클립보드 스레드를 띄운다. 쓸 수 없으면 건너뛴다고 출력하고 `None`.
struct Session {
    clipboard: Clipboard,
    saved_text: Option<String>,
    _serial: MutexGuard<'static, ()>,
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = match &self.saved_text {
            Some(text) => self.clipboard.set_text(text),
            None => self.clipboard.clear(),
        };
    }
}

fn session(
    name: &str,
    on_change: impl Fn(Option<ClipboardFiles>) + Send + 'static,
) -> Option<Session> {
    let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let clipboard = match Clipboard::start(on_change) {
        Ok(clipboard) => clipboard,
        Err(error) => {
            println!("{name}: 클립보드 스레드를 띄울 수 없어 건너뜀({error})");
            return None;
        }
    };
    let saved_text = match clipboard.text() {
        Ok(text) => text,
        Err(error) => {
            println!("{name}: 클립보드를 열 수 없어 건너뜀({error})");
            return None;
        }
    };
    Some(Session {
        clipboard,
        saved_text,
        _serial: serial,
    })
}

fn sample_paths() -> Vec<PathBuf> {
    let long = format!(r"C:\facet-clipboard\{}\끝.txt", "긴폴더이름".repeat(60));
    assert!(long.encode_utf16().count() > 260);
    vec![
        PathBuf::from(r"C:\facet-clipboard\한글 폴더"),
        PathBuf::from(r"C:\facet-clipboard\이모지 😀.txt"),
        PathBuf::from(long),
    ]
}

#[test]
fn files_round_trip_with_cut_and_copy() {
    let Some(s) = session("files_round_trip_with_cut_and_copy", |_| {}) else {
        return;
    };
    let paths = sample_paths();
    for operation in [ClipboardOperation::Move, ClipboardOperation::Copy] {
        s.clipboard.set_files(&paths, operation).unwrap();
        let files = s.clipboard.files().unwrap().expect("파일 목록");
        assert_eq!(files.paths, paths);
        assert_eq!(files.operation, operation);
        // 파일 목록만 올렸으니 글자는 없다.
        assert_eq!(s.clipboard.text().unwrap(), None);
    }
}

#[test]
fn text_round_trip_has_no_files() {
    let Some(s) = session("text_round_trip_has_no_files", |_| {}) else {
        return;
    };
    let text = "C:\\한글\\이모지 😀.txt\r\nD:\\b";
    s.clipboard.set_text(text).unwrap();
    assert_eq!(s.clipboard.text().unwrap().as_deref(), Some(text));
    assert_eq!(s.clipboard.files().unwrap(), None);
}

#[test]
fn clear_empties_the_clipboard() {
    let Some(s) = session("clear_empties_the_clipboard", |_| {}) else {
        return;
    };
    s.clipboard
        .set_files(&sample_paths(), ClipboardOperation::Copy)
        .unwrap();
    s.clipboard.clear().unwrap();
    assert_eq!(s.clipboard.files().unwrap(), None);
    assert_eq!(s.clipboard.text().unwrap(), None);
}

#[test]
fn finish_cut_paste_clears_only_the_same_clipboard() {
    let Some(s) = session("finish_cut_paste_clears_only_the_same_clipboard", |_| {}) else {
        return;
    };
    let paths = sample_paths();
    s.clipboard
        .set_files(&paths, ClipboardOperation::Move)
        .unwrap();
    let stale = s.clipboard.files().unwrap().expect("파일 목록").sequence;
    // 그 사이 클립보드가 바뀌면 아무것도 하지 않는다.
    s.clipboard
        .set_files(&paths, ClipboardOperation::Move)
        .unwrap();
    assert!(!s.clipboard.finish_cut_paste(stale).unwrap());
    let files = s.clipboard.files().unwrap().expect("그대로 남음");
    assert!(s.clipboard.finish_cut_paste(files.sequence).unwrap());
    assert_eq!(s.clipboard.files().unwrap(), None);
}

/// `events`에서 `want`를 만족하는 요약이 올 때까지 기다린다.
fn wait_for(
    events: &mpsc::Receiver<Option<ClipboardFiles>>,
    want: impl Fn(&Option<ClipboardFiles>) -> bool,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match events.recv_timeout(left) {
            Ok(summary) if want(&summary) => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    false
}

#[test]
fn change_notification_reports_new_summary() {
    let (tx, events) = mpsc::channel();
    let Some(s) = session("change_notification_reports_new_summary", move |summary| {
        let _ = tx.send(summary);
    }) else {
        return;
    };
    // 시작할 때 한 번 알린다.
    assert!(wait_for(&events, |_| true), "시작 알림이 없음");

    // 다른 스레드(다른 창)가 파일을 올리면 새 요약을 받는다.
    let other = Clipboard::start(|_| {}).unwrap();
    let paths = sample_paths();
    other.set_files(&paths, ClipboardOperation::Move).unwrap();
    assert!(
        wait_for(&events, |summary| summary.as_ref().is_some_and(|files| {
            files.paths == paths && files.operation == ClipboardOperation::Move
        })),
        "파일 목록 알림이 없음"
    );
    drop(other);

    // 다른 프로세스(clip.exe)가 글자를 올리면 「파일 없음」을 받는다.
    let child = Command::new("clip.exe").stdin(Stdio::piped()).spawn();
    let Ok(mut child) = child else {
        println!(
            "change_notification_reports_new_summary: clip.exe가 없어 다른 프로세스 확인은 건너뜀"
        );
        return;
    };
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(b"facet").unwrap();
    }
    assert!(child.wait().unwrap().success());
    assert!(wait_for(&events, Option::is_none), "글자 알림이 없음");
    // 클립보드는 프로세스 밖까지 하나라 알림 직후 한 번 읽으면 그 사이(clip.exe가 비운 직후 등)를 볼 수 있다.
    // 글자가 보일 때까지 잠깐 다시 읽는다.
    let deadline = Instant::now() + Duration::from_secs(5);
    let text = loop {
        let text = s.clipboard.text().unwrap();
        if text.as_deref().map(str::trim) == Some("facet") || Instant::now() >= deadline {
            break text;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(text.as_deref().map(str::trim), Some("facet"));
}
