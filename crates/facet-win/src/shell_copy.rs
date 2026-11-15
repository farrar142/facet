//! 셸의 `IFileOperation`으로 복사하기: **측정 전용**.
//!
//! facet의 복사는 자체 엔진이 한다(ADR-0005). 이 모듈은 탐색기 비교 측정(`facet-bench compare-copy`)이 같은
//! 기계에서 같은 복사를 탐색기의 복사 엔진(`IFileOperation`)으로도 해 기준으로 삼으려고 둔다. 셸 복사는 셸
//! 확장(복사 훅 등)을 프로세스에 실을 수 있으므로 앱(UI 프로세스)은 부르지 않는다(ADR-0006).
//!
//! COM은 부를 때마다 띄우는 전용 스레드에서 STA로 초기화하고, 진행 창·확인 대화·오류 대화가 뜨지 않게
//! `FOF_NO_UI`를 준다. Windows가 아니면 [`ShellCopyError::Unsupported`]다.

use std::fmt;
use std::path::{Path, PathBuf};

/// 셸 복사가 실패한 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellCopyError {
    /// 이 환경(Windows가 아닌 곳)에서는 셸 복사를 할 수 없다.
    Unsupported,
    /// 셸이 작업 일부를 그만두었다(`GetAnyOperationsAborted`).
    Aborted,
    /// 그 밖의 실패. `code`는 Windows가 돌려준 HRESULT다.
    Failed { code: i32, message: String },
}

impl fmt::Display for ShellCopyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShellCopyError::Unsupported => f.write_str("이 환경에서는 셸 복사를 할 수 없습니다"),
            ShellCopyError::Aborted => f.write_str("셸이 복사 일부를 그만두었습니다"),
            ShellCopyError::Failed { code, message } => write!(f, "{message} (0x{code:08X})"),
        }
    }
}

impl std::error::Error for ShellCopyError {}

/// `sources`(파일·폴더)를 폴더 `target` 안으로 셸의 `IFileOperation`으로 복사하고 끝날 때까지 기다린다.
///
/// 전용 스레드를 띄워 COM을 초기화하고 그 스레드에서 복사한다. 대상에 같은 이름이 있으면 셸이 묻지 않고
/// 덮어쓴다(`FOF_NOCONFIRMATION`). 측정 전용이며 앱은 부르지 않는다.
pub fn copy_items(sources: &[PathBuf], target: &Path) -> Result<(), ShellCopyError> {
    let sources = sources.to_vec();
    let target = target.to_path_buf();
    std::thread::Builder::new()
        .name("facet-shell-copy".into())
        .spawn(move || run(&sources, &target))
        .map_err(|e| ShellCopyError::Failed {
            code: 0,
            message: e.to_string(),
        })?
        .join()
        .unwrap_or_else(|_| {
            Err(ShellCopyError::Failed {
                code: 0,
                message: "셸 복사 스레드가 패닉했습니다".into(),
            })
        })
}

#[cfg(windows)]
fn run(sources: &[PathBuf], target: &Path) -> Result<(), ShellCopyError> {
    crate::windows_shell_copy::copy_items(sources, target)
}

#[cfg(not(windows))]
fn run(_sources: &[PathBuf], _target: &Path) -> Result<(), ShellCopyError> {
    Err(ShellCopyError::Unsupported)
}
