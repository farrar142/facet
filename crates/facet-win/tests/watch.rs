//! 폴더 변경 감시를 임시 폴더에서 확인한다(Windows 전용).
#![cfg(windows)]

use std::ffi::OsString;
use std::fs;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use facet_win::{DirectoryChange, DirectoryWatcher};
use tempfile::TempDir;

/// 알림을 기다리는 넉넉한 시간. CI 러너가 느려도 흔들리지 않게 크게 둔다.
const WAIT: Duration = Duration::from_secs(10);

fn watch(dir: &std::path::Path) -> (DirectoryWatcher, Receiver<DirectoryChange>) {
    let (sender, receiver) = mpsc::channel();
    let watcher = DirectoryWatcher::watch(dir, move |change| {
        let _ = sender.send(change);
    })
    .expect("감시를 걸 수 있어야 한다");
    (watcher, receiver)
}

/// `names`가 모두 알려질 때까지 기다린다.
fn expect_names(receiver: &Receiver<DirectoryChange>, names: &[&str]) {
    let deadline = Instant::now() + WAIT;
    let mut missing: Vec<OsString> = names.iter().map(OsString::from).collect();
    while !missing.is_empty() {
        let left = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(left) {
            Ok(DirectoryChange::Changed(name)) => missing.retain(|m| *m != name),
            Ok(other) => panic!("뜻밖의 알림: {other:?}"),
            Err(_) => panic!("알림이 오지 않았다: {missing:?}"),
        }
    }
}

#[test]
fn 만들기_이름바꾸기_지우기를_이름으로_알린다() {
    let dir = TempDir::new().unwrap();
    let (_watcher, receiver) = watch(dir.path());
    fs::write(dir.path().join("새 파일.txt"), b"x").unwrap();
    expect_names(&receiver, &["새 파일.txt"]);
    fs::rename(
        dir.path().join("새 파일.txt"),
        dir.path().join("바꾼 이름.txt"),
    )
    .unwrap();
    expect_names(&receiver, &["새 파일.txt", "바꾼 이름.txt"]);
    fs::create_dir(dir.path().join("폴더")).unwrap();
    fs::remove_file(dir.path().join("바꾼 이름.txt")).unwrap();
    expect_names(&receiver, &["폴더", "바꾼 이름.txt"]);
}

#[test]
fn 하위_폴더_안의_변경은_알리지_않는다() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    let (_watcher, receiver) = watch(dir.path());
    fs::write(dir.path().join("sub").join("inner.txt"), b"x").unwrap();
    fs::write(dir.path().join("outer.txt"), b"x").unwrap();
    // 하위 폴더 자체의 수정 시각 변경은 올 수 있지만 안쪽 이름은 오지 않는다.
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match receiver
            .recv_timeout(left)
            .expect("outer.txt 알림이 와야 한다")
        {
            DirectoryChange::Changed(name) if name == "outer.txt" => break,
            DirectoryChange::Changed(name) => assert_eq!(name, "sub"),
            other => panic!("뜻밖의 알림: {other:?}"),
        }
    }
}

#[test]
fn 버리면_더_알리지_않는다() {
    let dir = TempDir::new().unwrap();
    let (watcher, receiver) = watch(dir.path());
    drop(watcher);
    fs::write(dir.path().join("뒤에 만든 파일"), b"x").unwrap();
    // 콜백(보내는 쪽)도 함께 버려져 채널이 끊긴다.
    assert!(matches!(
        receiver.recv_timeout(WAIT),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
}

#[test]
fn 감시하는_폴더가_지워지면_끝을_알린다() {
    let root = TempDir::new().unwrap();
    let dir = root.path().join("보는 폴더");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("안의 파일"), b"x").unwrap();
    let (_watcher, receiver) = watch(&dir);
    fs::remove_dir_all(&dir).expect("감시 중에도 폴더를 지울 수 있어야 한다");
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(left).expect("끝 알림이 와야 한다") {
            DirectoryChange::Ended(_) => break,
            other => eprintln!("{other:?}"),
        }
    }
}

#[test]
fn 없는_폴더는_감시할_수_없다() {
    let dir = TempDir::new().unwrap();
    let result = DirectoryWatcher::watch(&dir.path().join("없음"), |_| {});
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::NotFound);
}
