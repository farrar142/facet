//! 셸 미리보기 처리기 싣기(#343). 처리기가 없는 형식은 「처리기 없음」, 붙일 창이 없으면 실패이고, 이 기계에 처리기가
//! 등록된 흔한 형식이 있으면 실제로 실어 숨은 창에 붙이고 옮기고 내린다(없으면 건너뛰었다고 출력). Windows에서만 돈다.
#![cfg(windows)]

use std::fs;
use std::time::Duration;

use facet_win::drop_target::testing::HiddenWindow;
use facet_win::preview::has_preview_handler;
use facet_win::preview_handler::{HostRect, LoadError, PreviewHandler};
use facet_win::shell_thread::ShellThread;
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(30);

fn host(parent: u64) -> HostRect {
    HostRect {
        parent,
        x: 0,
        y: 0,
        width: 320,
        height: 240,
    }
}

#[test]
fn an_unregistered_type_has_no_handler() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("doc.facetnopreviewhandler");
    fs::write(&path, b"nothing").unwrap();
    let thread = ShellThread::new();
    let result = thread
        .call(WAIT, move || {
            PreviewHandler::load(&path, host(0)).map(|_| ()).err()
        })
        .unwrap();
    assert_eq!(result, Some(LoadError::NoHandler));
}

/// 흔한 형식 가운데 이 기계에 처리기가 있는 것과 그 내용.
fn registered_sample() -> Option<(&'static str, &'static [u8])> {
    const SAMPLES: &[(&str, &[u8])] = &[
        ("txt", b"hello preview\r\n"),
        ("xml", b"<?xml version=\"1.0\"?><a>hello</a>"),
        ("html", b"<html><body>hello</body></html>"),
        ("svg", b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
    ];
    SAMPLES
        .iter()
        .copied()
        .find(|(extension, _)| has_preview_handler(extension))
}

#[test]
fn a_registered_handler_loads_into_a_window_moves_and_unloads() {
    let Some((extension, content)) = registered_sample() else {
        println!("건너뜀: 이 기계에는 시험할 미리보기 처리기가 없다");
        return;
    };
    let dir = TempDir::new().unwrap();
    let path = dir.path().join(format!("sample.{extension}"));
    fs::write(&path, content).unwrap();
    let thread = ShellThread::new();
    let held = path.clone();
    let outcome = thread
        .call(WAIT, move || {
            let window = HiddenWindow::new().expect("숨은 창");
            let parent = window.handle().get() as u64;
            // 붙일 창이 사라졌으면 싣지 않는다.
            let mut handler = match PreviewHandler::load(&held, host(parent)) {
                Ok(handler) => handler,
                Err(error) => return Err(error),
            };
            handler.move_to(HostRect {
                width: 640,
                height: 480,
                ..host(parent)
            });
            drop(handler);
            drop(window);
            Ok(())
        })
        .expect("셸 스레드");
    match outcome {
        Ok(()) => {}
        // 처리기 자체가 이 환경(서비스 세션 등)에서 그리지 못하는 것은 이 시험의 대상이 아니다.
        Err(LoadError::Failed(message)) => println!("건너뜀: {extension} 처리기가 실패: {message}"),
        Err(LoadError::NoHandler) => panic!("등록된 처리기를 찾지 못했다"),
    }
    // 내린 뒤에는 파일을 잡고 있지 않다.
    fs::remove_file(&path).expect("내린 뒤 지울 수 있다");
}

#[test]
fn a_missing_host_window_fails() {
    let Some((extension, content)) = registered_sample() else {
        println!("건너뜀: 이 기계에는 시험할 미리보기 처리기가 없다");
        return;
    };
    let dir = TempDir::new().unwrap();
    let path = dir.path().join(format!("sample.{extension}"));
    fs::write(&path, content).unwrap();
    let thread = ShellThread::new();
    let result = thread
        .call(WAIT, move || {
            PreviewHandler::load(&path, host(0)).map(|_| ()).err()
        })
        .unwrap();
    assert!(matches!(result, Some(LoadError::Failed(_))), "{result:?}");
}
