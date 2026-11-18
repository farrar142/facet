//! 셸 대화상자 띄우기: Windows 속성 대화상자와 「연결 프로그램」 대화상자(spec #24, ADR-0006).
//!
//! 셸 확장(속성 페이지 처리기 등)을 프로세스에 실으므로 **셸 확장 도우미만** 부르고 앱(UI 프로세스)은 부르지 않는다.
//! 두 함수 모두 부를 때마다 전용 스레드를 띄워 그 스레드에서 COM을 STA로 초기화하고 대화상자를 띄운 뒤 곧바로
//! 돌아온다(대화상자를 닫을 때까지 기다리지 않음). 대화상자가 떠 있는 동안은 [`dialogs_open`]이 참이다. 화면 문장은
//! 없다. Windows가 아니면 [`DialogError::Unsupported`]다.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 대화상자를 띄우지 못한 까닭(화면 문장 아님).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogError {
    /// 이 환경(Windows가 아닌 곳)에서는 띄울 수 없다.
    Unsupported,
    /// 대상이 없다(빈 목록).
    NoTarget,
    /// 셸이 실패했다. `code`는 HRESULT(모르면 0), `message`는 Windows의 설명.
    Failed { code: i32, message: String },
}

/// 대화상자 스레드가 도는 수.
static OPEN: AtomicUsize = AtomicUsize::new(0);

/// 이 모듈이 띄운 대화상자 가운데 아직 떠 있는 것이 있는가.
pub fn dialogs_open() -> bool {
    OPEN.load(Ordering::SeqCst) > 0
}

/// 대화상자 스레드 하나가 도는 동안 세는 표지.
struct Open;

impl Open {
    fn new() -> Open {
        OPEN.fetch_add(1, Ordering::SeqCst);
        Open
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        OPEN.fetch_sub(1, Ordering::SeqCst);
    }
}

/// `paths`의 Windows 속성 대화상자를 띄운다: 하나면 그 항목(파일·폴더·드라이브), 여럿이면 여러 항목의 속성.
/// 띄우면 곧바로 돌아오고, 띄우지 못하면 그 까닭이다.
pub fn properties(paths: Vec<PathBuf>) -> Result<(), DialogError> {
    if paths.is_empty() {
        return Err(DialogError::NoTarget);
    }
    run(move |started| imp::properties(&paths, started))
}

/// 파일 `path`로 Windows 「연결 프로그램」 대화상자를 띄운다(고른 프로그램으로 연다). 띄우면 곧바로 돌아온다.
pub fn open_with(path: PathBuf) -> Result<(), DialogError> {
    run(move |started| imp::open_with(&path, started))
}

/// 띄운 결과를 한 번 알리는 곳. 대화상자를 띄우는 쪽은 띄운 뒤(또는 실패하면) 한 번 부르고, 대화상자가 닫힐 때까지
/// 그 스레드에서 돈다.
pub(crate) type Started<'a> = &'a mut dyn FnMut(Result<(), DialogError>);

/// `show`를 전용 스레드에서 돌리고 띄운 결과를 기다린다.
fn run(show: impl FnOnce(Started<'_>) + Send + 'static) -> Result<(), DialogError> {
    let (tx, rx) = std::sync::mpsc::channel();
    let open = Open::new();
    std::thread::Builder::new()
        .name("facet-shell-dialog".into())
        .spawn(move || {
            let _open = open;
            let mut started = |result| {
                let _ = tx.send(result);
            };
            show(&mut started);
        })
        .map_err(|e| DialogError::Failed {
            code: 0,
            message: e.to_string(),
        })?;
    rx.recv().unwrap_or_else(|_| {
        Err(DialogError::Failed {
            code: 0,
            message: "대화상자 스레드가 끝났습니다".into(),
        })
    })
}

#[cfg(windows)]
use crate::windows_shell_dialog as imp;

#[cfg(not(windows))]
mod imp {
    use super::{DialogError, Started};
    use std::path::{Path, PathBuf};

    pub(crate) fn properties(_paths: &[PathBuf], started: Started<'_>) {
        started(Err(DialogError::Unsupported));
    }

    pub(crate) fn open_with(_path: &Path, started: Started<'_>) {
        started(Err(DialogError::Unsupported));
    }
}
