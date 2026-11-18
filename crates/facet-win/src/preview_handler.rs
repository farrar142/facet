//! 셸 미리보기 처리기 싣기(#343, ADR-0006): 파일 하나의 미리보기 처리기(`IPreviewHandler`)를 싣고, 다른 창(다른
//! 프로세스의 창이어도 된다)의 한 자리에 자식 창으로 붙여 그리게 한다. **셸 확장 도우미만 부른다**: 처리기(DLL)가 이
//! 프로세스에 실리므로 UI 프로세스(`facet`)는 이 모듈을 부르지 않는다(부르지 않는 함수는 링크되지 않는다).
//!
//! [`PreviewHandler`]는 COM을 STA로 초기화하고 메시지를 돌리는 스레드(예: [`crate::shell_thread::ShellThread`])에서
//! 만들고 그 스레드에서만 쓴다(`Send`가 아님). 그 스레드가 메시지를 돌려야 처리기 창이 그려진다.
//!
//! - 처리기는 확장자의 연결([`crate::preview::has_preview_handler`]와 같은 조회)에서 찾고, 이 프로세스에 싣는 것을 먼저
//!   해 보고 안 되면(32비트만 등록된 처리기) 셸의 미리보기 대리 프로세스(로컬 서버)로 띄운다.
//! - 초기화는 스트림(`IInitializeWithStream`)을 먼저 쓰고, 처리기가 받지 않으면 경로(`IInitializeWithFile`), 그다음 셸
//!   항목(`IInitializeWithItem`)이다. 스트림은 읽기·쓰기 공유로 연다(삭제 공유는 셸 스트림이 주지 않으므로 잡힌 파일은
//!   작업 엔진이 미리보기를 내린 뒤 다시 시도한다, #342).
//! - 처리기 창은 이 모듈이 만든 그릇 창(`WS_CHILD`) 안에 붙고, 그릇 창을 붙일 창의 자식으로 만든다. 자리는 화면
//!   물리 픽셀로 받아 붙일 창의 안쪽 좌표로 바꾼다: 그래서 이 스레드를 모니터마다 DPI를 아는 문맥(Per-Monitor v2)으로
//!   둔다.
//! - 버리면 처리기를 내리고(`Unload`) 그릇 창을 없앤다(처리기가 잡고 있던 파일을 놓는다).
//!
//! Windows가 아니면 싣지 못한다([`LoadError::Failed`]).

use std::fmt;
use std::path::Path;

/// 처리기 창을 붙일 자리: 붙일 창의 핸들 값과 화면 자리(가상 화면 기준 물리 픽셀).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostRect {
    pub parent: u64,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// 처리기를 싣지 못한 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// 그 파일 형식에 미리보기 처리기가 없다.
    NoHandler,
    /// 처리기를 싣거나 초기화하거나 그리지 못했다(파일 없음·권한·처리기 실패, 붙일 창이 없음 등).
    Failed(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::NoHandler => f.write_str("no preview handler"),
            LoadError::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for LoadError {}

/// 실은 미리보기 처리기 하나. 만든 스레드(COM STA)에서만 쓴다. 버리면 처리기를 내리고 창을 없앤다.
pub struct PreviewHandler {
    #[cfg(windows)]
    inner: crate::windows_preview_handler::Loaded,
    #[cfg(not(windows))]
    _not_send: std::marker::PhantomData<*const ()>,
}

impl PreviewHandler {
    /// `path`의 미리보기 처리기를 싣고 `host` 자리에 붙여 그리기 시작한다. COM STA로 초기화한 스레드에서 부른다.
    pub fn load(path: &Path, host: HostRect) -> Result<PreviewHandler, LoadError> {
        #[cfg(windows)]
        {
            crate::windows_preview_handler::Loaded::load(path, host)
                .map(|inner| PreviewHandler { inner })
        }
        #[cfg(not(windows))]
        {
            let _ = (path, host);
            Err(LoadError::Failed("unsupported".into()))
        }
    }

    /// 처리기 창을 `host` 자리로 옮긴다(크기도 바꾼다). 붙일 창이 바뀌었으면 그 창으로 옮겨 붙인다.
    pub fn move_to(&mut self, host: HostRect) {
        #[cfg(windows)]
        self.inner.move_to(host);
        #[cfg(not(windows))]
        let _ = host;
    }
}
