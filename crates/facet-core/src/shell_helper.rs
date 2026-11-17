//! #319: 셸 확장 도우미 관리(띄우기·연결·요청과 응답·대기 시간·다시 띄우기·쉬면 끝내기).

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use facet_shell_protocol::{MenuItem, MenuTarget};

use crate::menu::MenuKind;

/// #319: 응답 대기 시간(2초).
pub const REPLY_TIMEOUT: Duration = Duration::ZERO; // #319
/// #319: 멈춤 판정 시간(10초).
pub const STALL_TIMEOUT: Duration = Duration::ZERO; // #319
/// #319: 쉬면 끝내기 시간(5분).
pub const IDLE_TIMEOUT: Duration = Duration::ZERO; // #319
/// #319: 연달아 죽음을 세는 시간(1분).
pub const CRASH_WINDOW: Duration = Duration::ZERO; // #319
/// #319: 그 시간 안에 이 수를 넘게 죽으면 쉰다(3).
pub const CRASH_LIMIT: u32 = 0; // #319
/// #319: 쉬는 시간(1분).
pub const REST_DURATION: Duration = Duration::ZERO; // #319
/// #320: 같은 확장으로 이만큼 죽으면 알린다(3).
pub const SAME_EXTENSION_CRASHES: u32 = 0; // #320

/// #319: 도우미 관리 설정.
#[derive(Debug, Clone)]
pub struct HelperOptions {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub parent: Option<u32>,
    pub reply_timeout: Duration,
    pub stall_timeout: Duration,
    pub idle_timeout: Duration,
    pub crash_window: Duration,
    pub crash_limit: u32,
    pub rest: Duration,
    pub crash_log: Option<PathBuf>,
}

impl HelperOptions {
    /// #319: 기본값의 설정.
    pub fn new(_program: impl Into<PathBuf>) -> HelperOptions {
        todo!("#319")
    }
}

/// #319: 기본 죽은 기록 파일(`%LOCALAPPDATA%\facet\helper-crashes.txt`).
pub fn default_crash_log() -> Option<PathBuf> {
    todo!("#319")
}

/// #319: 지금 실행 파일 옆의 `facet-shell-helper.exe`.
pub fn default_program() -> PathBuf {
    todo!("#319")
}

/// #319: 도우미 관리.
pub struct ShellHelper {
    _private: (),
}

impl ShellHelper {
    /// #319: 관리를 시작한다(도우미는 첫 요청 때 뜬다).
    pub fn start(_options: HelperOptions) -> ShellHelper {
        todo!("#319")
    }

    /// #319: 사건을 보낼 때마다 부를 깨우기.
    pub fn set_waker(&self, _waker: impl Fn() + Send + Sync + 'static) {
        todo!("#319")
    }

    /// #319: 셸 메뉴 요청.
    pub fn menu(&self, _target: MenuTarget) -> RequestId {
        todo!("#319")
    }

    /// #319: 셸 항목 실행 요청.
    pub fn invoke(&self, _menu: RequestId, _item: u32) -> RequestId {
        todo!("#319")
    }

    /// #319: 메뉴를 놓는다.
    pub fn release(&self, _menu: RequestId) {
        todo!("#319")
    }

    /// #319: 마지막으로 띄운 도우미의 프로세스 번호.
    pub fn process_id(&self) -> Option<u32> {
        todo!("#319")
    }

    /// #319: 쌓인 사건.
    pub fn poll(&self) -> Vec<HelperEvent> {
        todo!("#319")
    }

    /// #319: 사건 하나를 기다린다.
    pub fn wait_event(&self, _timeout: Duration) -> Option<HelperEvent> {
        todo!("#319")
    }

    /// #320: 연달아 죽어 쉬는 중인가.
    pub fn is_resting(&self) -> bool {
        todo!("#320")
    }

    /// #320: 같은 확장으로 세 번 죽었으면 그 확장.
    pub fn crashing_extension(&self) -> Option<CrashedExtension> {
        todo!("#320")
    }

    /// #321: 속성 대화상자 요청.
    pub fn properties(&self, _paths: Vec<PathBuf>) -> RequestId {
        todo!("#321")
    }

    /// #321: 「연결 프로그램」 대화상자 요청.
    pub fn open_with(&self, _path: PathBuf) -> RequestId {
        todo!("#321")
    }
}

/// #319: 요청 번호.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(u64);

/// #319: 도우미 관리의 사건.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperEvent {
    Reply {
        id: RequestId,
        result: Result<Reply, RequestError>,
    },
}

/// #319: 요청의 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Menu(Vec<MenuItem>),
    Done,
}

/// #319: 요청이 실패한 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    TimedOut,
    Disconnected,
    VersionMismatch,
    Resting,
    Spawn(std::io::ErrorKind),
    Failed(String),
}

/// #320: 도우미를 죽게 한 확장.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrashedExtension {
    pub clsid: String,
    pub dll: PathBuf,
    pub name: Option<String>,
    pub crashes: u32,
}

/// #322: 열린 셸 메뉴의 번호.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShellMenuId(u64);

/// #322: 열린 셸 메뉴.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellMenu {
    pub id: ShellMenuId,
    pub kind: MenuKind,
    pub section: ShellSection,
    pub crashing_extension: Option<CrashedExtension>,
}

/// #322: 셸 항목 자리.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellSection {
    Loading,
    Items(Vec<MenuItem>),
    Failed,
    Resting,
}
