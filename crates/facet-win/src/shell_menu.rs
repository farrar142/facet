//! 셸 메뉴(#323, ADR-0006): 대상(항목들 또는 폴더 배경)의 셸 오른쪽 메뉴(`IContextMenu`)를 만들어 항목 트리로 읽고, 고른
//! 항목을 그 메뉴 객체로 실행한다. **셸 확장 도우미만 부른다**: 메뉴를 만들면 등록된 셸 확장(DLL)이 이 프로세스에 실리므로
//! UI 프로세스(`facet`)는 이 모듈을 부르지 않는다(부르지 않는 함수는 링크되지 않는다).
//!
//! [`ShellMenu`]는 COM을 STA로 초기화하고 메시지를 돌리는 스레드(예: [`crate::shell_thread::ShellThread`])에서 만들고
//! 그 스레드에서만 쓴다(`Send`가 아님). 버리면 메뉴와 메뉴 객체를 놓는다.
//!
//! 죽게 한 확장 알기([`record_crashes`]): 처리되지 않은 예외로 프로세스가 죽을 때 그 주소가 든 DLL을 찾아, 메뉴를 만들 때
//! 레지스트리에서 모아 둔 오른쪽 메뉴 확장(CLSID·DLL·이름)과 맞춰 공유 상태 파일에 적는다(알 수 있는 만큼).
//!
//! Windows가 아니면 메뉴를 만들 수 없다([`MenuError::Failed`]).

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub use crate::icon::IconImage;

/// 셸 메뉴의 대상.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuTarget {
    /// 항목들(한 개 이상). 같은 폴더가 아니어도 된다.
    Items(Vec<PathBuf>),
    /// 폴더 배경(빈 곳).
    Background(PathBuf),
}

/// 메뉴의 한 줄.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Command(Command),
    Submenu(Submenu),
    Separator,
}

/// 명령 항목. `id`를 [`ShellMenu::invoke`]에 넘긴다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub id: u32,
    /// 셸이 준 글자 그대로(가속 글자 `&` 포함, 탭 뒤 단축키 글자는 뺌).
    pub text: String,
    pub icon: Option<IconImage>,
    pub enabled: bool,
}

/// 하위 메뉴.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submenu {
    pub text: String,
    pub icon: Option<IconImage>,
    pub enabled: bool,
    pub items: Vec<Entry>,
}

/// 메뉴를 만들거나 실행하지 못한 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuError {
    /// 대상이 없다(지워졌거나 경로가 틀림).
    NotFound(String),
    /// 메뉴에 없는 항목 번호.
    UnknownCommand,
    /// 셸이 실패했다(권한 없음 등).
    Failed(String),
}

impl fmt::Display for MenuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MenuError::NotFound(message) => write!(f, "not found: {message}"),
            MenuError::UnknownCommand => f.write_str("unknown command"),
            MenuError::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for MenuError {}

/// 하위 메뉴를 읽는 가장 깊은 단계(깨진 확장이 끝없이 하위 메뉴를 만들어도 멈춘다).
pub const MAX_DEPTH: usize = 8;

/// 대상의 셸 메뉴 하나. 만든 스레드(COM STA)에서만 쓴다. 버리면 메뉴와 메뉴 객체를 놓는다.
pub struct ShellMenu {
    #[cfg(windows)]
    inner: crate::windows_shell_menu::Menu,
    #[cfg(not(windows))]
    _not_send: std::marker::PhantomData<*const ()>,
}

impl ShellMenu {
    /// `target`의 셸 메뉴를 만든다(셸이 오른쪽 메뉴 확장을 싣고 메뉴를 채운다). COM STA로 초기화한 스레드에서 부른다.
    /// 대상이 없으면 [`MenuError::NotFound`], 셸이 실패하면 [`MenuError::Failed`]다.
    pub fn build(target: &MenuTarget) -> Result<ShellMenu, MenuError> {
        #[cfg(windows)]
        {
            crate::windows_shell_menu::Menu::build(target).map(|inner| ShellMenu { inner })
        }
        #[cfg(not(windows))]
        {
            let _ = target;
            Err(MenuError::Failed("unsupported".into()))
        }
    }

    /// 메뉴의 항목 트리. 하위 메뉴는 셸이 펼칠 때처럼 채운 뒤 읽는다(「보내기」처럼 펼칠 때 채우는 메뉴). 아이콘은
    /// 확장이 준 그림 크기 그대로다. 앞뒤와 겹친 구분선은 걷는다.
    pub fn entries(&self) -> Vec<Entry> {
        #[cfg(windows)]
        {
            self.inner.entries()
        }
        #[cfg(not(windows))]
        {
            Vec::new()
        }
    }

    /// `id`번 항목을 실행하고 끝날 때까지 기다린다(실행이 띄운 모달 창이 닫힐 때까지일 수 있다).
    pub fn invoke(&self, id: u32) -> Result<(), MenuError> {
        #[cfg(windows)]
        {
            self.inner.invoke(id)
        }
        #[cfg(not(windows))]
        {
            let _ = id;
            Err(MenuError::Failed("unsupported".into()))
        }
    }
}

/// 이 스레드의 메시지를 돌리며, 이 프로세스의 보이는 창이 `appear` 안에 나타나면 그 창들이 모두 닫힐 때까지 기다린다
/// (처음 나타난 창은 앞으로 가져온다). 셸 항목 실행이 띄운 비모달 창이 메시지를 받게 한다.
pub fn pump_while_windows(appear: Duration) {
    #[cfg(windows)]
    crate::shell_dialog::wait_for_windows(appear);
    #[cfg(not(windows))]
    let _ = appear;
}

/// 처리되지 않은 예외로 이 프로세스가 죽을 때 그 예외 주소가 든 DLL을 `state` 파일에 「CLSID·DLL 경로·이름」 한 줄씩
/// 적게 한다(셸 확장 도우미의 「마지막으로 부른 확장」 공유 상태, #320). CLSID·이름은 [`ShellMenu::build`]가 대상의
/// 오른쪽 메뉴 확장을 레지스트리에서 모아 둔 것과 DLL 경로로 맞추고, 맞는 것이 없으면 CLSID 자리에 `unknown`을 적는다.
/// 이 프로세스에서 한 번 부른다.
pub fn record_crashes(state: &Path) {
    #[cfg(windows)]
    crate::windows_shell_menu::record_crashes(state);
    #[cfg(not(windows))]
    let _ = state;
}
