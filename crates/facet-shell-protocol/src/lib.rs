//! #318: 셸 확장 도우미 프로토콜(UI 프로세스와 도우미가 주고받는 메시지, 이진 형식, 길이 접두 프레임, 버전 인사).

use std::io::{Read, Write};
use std::path::PathBuf;

/// #318: 이 크레이트가 말하는 프로토콜 버전.
pub const PROTOCOL_VERSION: u16 = 0; // #318
/// #318: 받아들이는 가장 낮은 버전.
pub const MIN_SUPPORTED_VERSION: u16 = 0; // #318
/// #318: 받아들이는 가장 높은 버전.
pub const MAX_SUPPORTED_VERSION: u16 = 0; // #318
/// #318: 프레임 본문의 최대 길이.
pub const MAX_FRAME_LEN: u32 = 0; // #318

/// #318: 버전 인사에 대한 도우미의 답.
pub fn answer_hello(_client_version: u16) -> Response {
    todo!("#318")
}

/// #318: UI가 도우미에 보내는 요청.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Hello { version: u16 },
    Menu { id: u64, target: MenuTarget },
}

/// #318: 도우미의 응답.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    HelloAccepted { version: u16 },
    HelloRejected { min_version: u16, max_version: u16 },
    Menu { id: u64, items: Vec<MenuItem> },
}

/// #318: 셸 메뉴의 대상.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuTarget {
    Items(Vec<PathBuf>),
    Background(PathBuf),
}

/// #318: 셸 메뉴의 한 줄.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuItem {
    Command(ShellCommand),
    Submenu(ShellSubmenu),
    Separator,
}

/// #318: 셸 명령 항목.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommand {
    pub id: u32,
    pub text: String,
    pub icon: Option<Icon>,
    pub enabled: bool,
}

/// #318: 셸 하위 메뉴.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellSubmenu {
    pub text: String,
    pub icon: Option<Icon>,
    pub enabled: bool,
    pub items: Vec<MenuItem>,
}

/// #318: 아이콘 그림(RGBA).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Icon {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// #318: 메시지 본문의 이진 형식.
pub trait Message: Sized {
    /// #318: 메시지 본문(프레임 머리 없이)을 만든다.
    fn to_bytes(&self) -> Vec<u8>;
    /// #318: 메시지 본문을 읽는다.
    fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError>;
}

impl Message for Request {
    fn to_bytes(&self) -> Vec<u8> {
        todo!("#318")
    }

    fn from_bytes(_bytes: &[u8]) -> Result<Self, DecodeError> {
        todo!("#318")
    }
}

impl Message for Response {
    fn to_bytes(&self) -> Vec<u8> {
        todo!("#318")
    }

    fn from_bytes(_bytes: &[u8]) -> Result<Self, DecodeError> {
        todo!("#318")
    }
}

/// #318: 본문을 읽지 못한 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {}

/// #318: 프레임 읽기 오류.
#[derive(Debug)]
pub enum ReadError {
    TooLarge { len: u32 },
}

/// #318: 프레임 쓰기 오류.
#[derive(Debug)]
pub enum WriteError {}

/// #318: 메시지 하나를 프레임으로 쓴다.
pub fn write_message(_w: &mut impl Write, _message: &impl Message) -> Result<(), WriteError> {
    todo!("#318")
}

/// #318: 프레임 하나를 읽어 메시지로 바꾼다.
pub fn read_message<M: Message>(_r: &mut impl Read) -> Result<M, ReadError> {
    todo!("#318")
}
