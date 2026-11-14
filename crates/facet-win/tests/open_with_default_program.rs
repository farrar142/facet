//! 기본 프로그램으로 열기. 실제 프로그램이 뜨는 경로는 자동 테스트하지 않는다.
#![cfg(windows)]

use std::path::PathBuf;
use std::time::Duration;

use facet_win::{OpenError, ShellWorker};

const WAIT: Duration = Duration::from_secs(60);

/// 어떤 프로그램과도 연결되지 않았을 확장자.
const UNASSOCIATED_EXTENSION: &str = "facetnoassoc7f3a";

#[test]
fn 연결된_프로그램이_없는_파일은_연결된_프로그램_없음_오류다() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(format!("문서.{UNASSOCIATED_EXTENSION}"));
    std::fs::write(&file, b"facet").unwrap();

    let worker = ShellWorker::spawn().unwrap();
    let result = worker
        .open_with_default_program(file)
        .recv_timeout(WAIT)
        .unwrap();

    assert_eq!(result, Err(OpenError::NoAssociation));
}

#[test]
fn 없는_파일을_열면_그_밖의_오류다() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("없음.txt");

    let worker = ShellWorker::spawn().unwrap();
    let result = worker
        .open_with_default_program(missing)
        .recv_timeout(WAIT)
        .unwrap();

    assert!(matches!(result, Err(OpenError::Other { .. })), "{result:?}");
}

fn long_dir(root: &std::path::Path) -> PathBuf {
    let mut deep = root.to_path_buf();
    while deep.as_os_str().len() < 300 {
        deep.push("깊은폴더이름0123456789");
    }
    std::fs::create_dir_all(&deep).unwrap();
    deep
}

#[test]
fn 긴_경로의_연결된_프로그램이_없는_파일도_연결된_프로그램_없음_오류다() {
    let dir = tempfile::tempdir().unwrap();
    let file = long_dir(dir.path()).join(format!("문서.{UNASSOCIATED_EXTENSION}"));
    std::fs::write(&file, b"facet").unwrap();

    let worker = ShellWorker::spawn().unwrap();
    let result = worker
        .open_with_default_program(file)
        .recv_timeout(WAIT)
        .unwrap();

    assert_eq!(result, Err(OpenError::NoAssociation));
}

#[test]
fn 없는_긴_경로는_경로가_너무_김이_아니라_그_밖의_오류다() {
    let dir = tempfile::tempdir().unwrap();
    let missing = long_dir(dir.path()).join("없음.txt");

    let worker = ShellWorker::spawn().unwrap();
    let result = worker
        .open_with_default_program(missing)
        .recv_timeout(WAIT)
        .unwrap();

    assert!(matches!(result, Err(OpenError::Other { .. })), "{result:?}");
}

#[test]
fn 부르는_스레드를_막지_않고_요청을_차례로_처리한다() {
    let dir = tempfile::tempdir().unwrap();
    let files: Vec<PathBuf> = (0..3)
        .map(|i| {
            let file = dir.path().join(format!("{i}.{UNASSOCIATED_EXTENSION}"));
            std::fs::write(&file, b"facet").unwrap();
            file
        })
        .collect();

    let worker = ShellWorker::spawn().unwrap();
    // 요청은 결과를 기다리지 않고 바로 돌아온다. 결과는 일꾼 스레드가 채널로 보낸다.
    let pending: Vec<_> = files
        .into_iter()
        .map(|file| worker.open_with_default_program(file))
        .collect();
    for receiver in pending {
        assert_eq!(
            receiver.recv_timeout(WAIT).unwrap(),
            Err(OpenError::NoAssociation)
        );
    }
}

#[test]
fn 일꾼을_버리면_스레드가_끝난다() {
    let worker = ShellWorker::spawn().unwrap();
    drop(worker);
}
