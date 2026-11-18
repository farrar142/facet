//! 셸 확장 도우미 프로토콜(spec #24): UI 프로세스와 도우미가 주고받는 메시지, 이진 형식, 길이 접두 프레임,
//! 버전 인사. Windows API를 쓰지 않고 바이트 스트림 위에서 동작한다. 모양은 `facet-protocol`을 따른다.
//! 화면 문장은 없다(항목 글자는 셸이 준 것을 그대로 나른다).

use std::io::{Read, Write};
use std::path::PathBuf;

/// 이 크레이트가 말하는 프로토콜 버전.
pub const PROTOCOL_VERSION: u16 = 1;
/// 받아들이는 가장 낮은 버전.
pub const MIN_SUPPORTED_VERSION: u16 = 1;
/// 받아들이는 가장 높은 버전.
pub const MAX_SUPPORTED_VERSION: u16 = 1;
/// 프레임 본문의 최대 길이(16MiB: 아이콘 그림이 든 큰 메뉴도 들어가고, 깨진 머리가 큰 메모리를 잡게 하지 않는 값).
pub const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;
/// 하위 메뉴 중첩의 최대 깊이(깨진 입력이 스택을 넘치게 하지 않게).
const MAX_DEPTH: usize = 32;

/// 버전 인사에 대한 도우미의 답.
pub fn answer_hello(client_version: u16) -> Response {
    if (MIN_SUPPORTED_VERSION..=MAX_SUPPORTED_VERSION).contains(&client_version) {
        Response::HelloAccepted {
            version: client_version,
        }
    } else {
        Response::HelloRejected {
            min_version: MIN_SUPPORTED_VERSION,
            max_version: MAX_SUPPORTED_VERSION,
        }
    }
}

/// UI가 도우미에 보내는 요청.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Hello {
        version: u16,
    },
    Menu {
        id: u64,
        target: MenuTarget,
    },
    /// 메뉴 항목 실행(`command`는 `ShellCommand::id`).
    Execute {
        id: u64,
        target: MenuTarget,
        command: u32,
    },
    /// 끌어 놓기 메뉴(오른쪽 버튼으로 끌어 `folder`에 놓음)의 항목 요청.
    DropMenu {
        id: u64,
        sources: Vec<PathBuf>,
        folder: PathBuf,
    },
    /// 속성 창 열기.
    Properties {
        id: u64,
        paths: Vec<PathBuf>,
    },
    /// 연결 프로그램 목록 열기.
    OpenWith {
        id: u64,
        path: PathBuf,
    },
}

/// 도우미의 응답.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    HelloAccepted {
        version: u16,
    },
    HelloRejected {
        min_version: u16,
        max_version: u16,
    },
    Menu {
        id: u64,
        items: Vec<MenuItem>,
    },
    /// 실행·속성·연결 프로그램 요청이 끝남.
    Done {
        id: u64,
    },
    Error {
        id: u64,
        kind: ErrorKind,
        message: String,
    },
}

/// 요청을 하지 못한 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// 대상이 없음.
    NotFound,
    /// 모르는 항목 번호.
    UnknownCommand,
    /// 셸이 실패함.
    Failed,
}

/// 셸 메뉴의 대상.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuTarget {
    Items(Vec<PathBuf>),
    Background(PathBuf),
}

/// 셸 메뉴의 한 줄.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuItem {
    Command(ShellCommand),
    Submenu(ShellSubmenu),
    Separator,
}

/// 셸 명령 항목.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommand {
    pub id: u32,
    pub text: String,
    pub icon: Option<Icon>,
    pub enabled: bool,
}

/// 셸 하위 메뉴.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellSubmenu {
    pub text: String,
    pub icon: Option<Icon>,
    pub enabled: bool,
    pub items: Vec<MenuItem>,
}

/// 아이콘 그림(RGBA).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Icon {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// 메시지 본문의 이진 형식.
pub trait Message: Sized {
    /// 메시지 본문(프레임 머리 없이)을 만든다.
    fn to_bytes(&self) -> Vec<u8>;
    /// 메시지 본문을 읽는다.
    fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError>;
}

/// 본문을 읽지 못한 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// 본문이 중간에 끝남.
    Truncated,
    /// 모르는 종류 번호.
    UnknownTag(u8),
    /// 글자가 UTF-8이 아님.
    InvalidText,
    /// 하위 메뉴가 너무 깊음.
    TooDeep,
    /// 아이콘 크기와 바이트 수가 맞지 않음.
    InvalidIcon,
    /// 본문 뒤에 남는 바이트가 있음.
    TrailingBytes,
}

/// 프레임 읽기 오류.
#[derive(Debug)]
pub enum ReadError {
    /// 읽기 실패(머리나 본문이 잘림 포함).
    Io(std::io::Error),
    /// 본문 길이가 `MAX_FRAME_LEN`을 넘음(본문은 읽지 않음).
    TooLarge { len: u32 },
    /// 본문이 깨짐.
    Decode(DecodeError),
}

/// 프레임 쓰기 오류.
#[derive(Debug)]
pub enum WriteError {
    Io(std::io::Error),
    /// 본문이 `MAX_FRAME_LEN`을 넘음.
    TooLarge {
        len: usize,
    },
}

/// 메시지 하나를 프레임(길이 u32 little-endian 4바이트 + 본문)으로 쓴다.
pub fn write_message(w: &mut impl Write, message: &impl Message) -> Result<(), WriteError> {
    let body = message.to_bytes();
    if body.len() > MAX_FRAME_LEN as usize {
        return Err(WriteError::TooLarge { len: body.len() });
    }
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    w.write_all(&frame).map_err(WriteError::Io)?;
    w.flush().map_err(WriteError::Io)
}

/// 프레임 하나를 읽어 메시지로 바꾼다.
pub fn read_message<M: Message>(r: &mut impl Read) -> Result<M, ReadError> {
    let mut head = [0u8; 4];
    r.read_exact(&mut head).map_err(ReadError::Io)?;
    let len = u32::from_le_bytes(head);
    if len > MAX_FRAME_LEN {
        return Err(ReadError::TooLarge { len });
    }
    // 머리가 거짓이어도 실제 들어온 만큼만 잡도록 take로 읽는다.
    let mut body = Vec::new();
    r.take(u64::from(len))
        .read_to_end(&mut body)
        .map_err(ReadError::Io)?;
    if body.len() != len as usize {
        return Err(ReadError::Io(std::io::ErrorKind::UnexpectedEof.into()));
    }
    M::from_bytes(&body).map_err(ReadError::Decode)
}

// ---- 이진 형식 ----

struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, v: &[u8]) {
        self.u32(v.len() as u32);
        self.0.extend_from_slice(v);
    }
    fn text(&mut self, v: &str) {
        self.bytes(v.as_bytes());
    }
    fn path(&mut self, p: &std::path::Path) {
        let units = path_units(p);
        self.u32(units.len() as u32);
        for unit in units {
            self.u16(unit);
        }
    }
    fn paths(&mut self, ps: &[PathBuf]) {
        self.u32(ps.len() as u32);
        for p in ps {
            self.path(p);
        }
    }
    fn target(&mut self, t: &MenuTarget) {
        match t {
            MenuTarget::Items(ps) => {
                self.u8(0);
                self.paths(ps);
            }
            MenuTarget::Background(p) => {
                self.u8(1);
                self.path(p);
            }
        }
    }
    fn icon(&mut self, icon: &Option<Icon>) {
        match icon {
            None => self.u8(0),
            Some(i) => {
                self.u8(1);
                self.u32(i.width);
                self.u32(i.height);
                self.bytes(&i.rgba);
            }
        }
    }
    fn items(&mut self, items: &[MenuItem]) {
        self.u32(items.len() as u32);
        for item in items {
            match item {
                MenuItem::Command(c) => {
                    self.u8(0);
                    self.u32(c.id);
                    self.text(&c.text);
                    self.icon(&c.icon);
                    self.u8(c.enabled as u8);
                }
                MenuItem::Submenu(s) => {
                    self.u8(1);
                    self.text(&s.text);
                    self.icon(&s.icon);
                    self.u8(s.enabled as u8);
                    self.items(&s.items);
                }
                MenuItem::Separator => self.u8(2),
            }
        }
    }
}

#[cfg(windows)]
fn path_units(p: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().collect()
}

#[cfg(not(windows))]
fn path_units(p: &std::path::Path) -> Vec<u16> {
    p.to_string_lossy().encode_utf16().collect()
}

#[cfg(windows)]
fn path_from_units(units: &[u16]) -> PathBuf {
    use std::os::windows::ffi::OsStringExt;
    PathBuf::from(std::ffi::OsString::from_wide(units))
}

#[cfg(not(windows))]
fn path_from_units(units: &[u16]) -> PathBuf {
    PathBuf::from(String::from_utf16_lossy(units))
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], DecodeError> {
        if self.0.len() < n {
            return Err(DecodeError::Truncated);
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }
    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn bytes(&mut self) -> Result<Vec<u8>, DecodeError> {
        let n = self.u32()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    fn text(&mut self) -> Result<String, DecodeError> {
        String::from_utf8(self.bytes()?).map_err(|_| DecodeError::InvalidText)
    }
    fn path(&mut self) -> Result<PathBuf, DecodeError> {
        let n = self.u32()? as usize;
        let raw = self.take(n.checked_mul(2).ok_or(DecodeError::Truncated)?)?;
        let units: Vec<u16> = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        Ok(path_from_units(&units))
    }
    fn paths(&mut self) -> Result<Vec<PathBuf>, DecodeError> {
        let n = self.u32()?;
        let mut out = Vec::new();
        for _ in 0..n {
            out.push(self.path()?);
        }
        Ok(out)
    }
    fn target(&mut self) -> Result<MenuTarget, DecodeError> {
        match self.u8()? {
            0 => Ok(MenuTarget::Items(self.paths()?)),
            1 => Ok(MenuTarget::Background(self.path()?)),
            t => Err(DecodeError::UnknownTag(t)),
        }
    }
    fn icon(&mut self) -> Result<Option<Icon>, DecodeError> {
        match self.u8()? {
            0 => Ok(None),
            1 => {
                let width = self.u32()?;
                let height = self.u32()?;
                let rgba = self.bytes()?;
                if u64::from(width) * u64::from(height) * 4 != rgba.len() as u64 {
                    return Err(DecodeError::InvalidIcon);
                }
                Ok(Some(Icon {
                    width,
                    height,
                    rgba,
                }))
            }
            t => Err(DecodeError::UnknownTag(t)),
        }
    }
    fn items(&mut self, depth: usize) -> Result<Vec<MenuItem>, DecodeError> {
        if depth > MAX_DEPTH {
            return Err(DecodeError::TooDeep);
        }
        let n = self.u32()?;
        let mut out = Vec::new();
        for _ in 0..n {
            out.push(match self.u8()? {
                0 => MenuItem::Command(ShellCommand {
                    id: self.u32()?,
                    text: self.text()?,
                    icon: self.icon()?,
                    enabled: self.u8()? != 0,
                }),
                1 => MenuItem::Submenu(ShellSubmenu {
                    text: self.text()?,
                    icon: self.icon()?,
                    enabled: self.u8()? != 0,
                    items: self.items(depth + 1)?,
                }),
                2 => MenuItem::Separator,
                t => return Err(DecodeError::UnknownTag(t)),
            });
        }
        Ok(out)
    }
    fn finish(self) -> Result<(), DecodeError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(DecodeError::TrailingBytes)
        }
    }
}

impl Message for Request {
    fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer(Vec::new());
        match self {
            Request::Hello { version } => {
                w.u8(0);
                w.u16(*version);
            }
            Request::Menu { id, target } => {
                w.u8(1);
                w.u64(*id);
                w.target(target);
            }
            Request::Execute {
                id,
                target,
                command,
            } => {
                w.u8(2);
                w.u64(*id);
                w.target(target);
                w.u32(*command);
            }
            Request::DropMenu {
                id,
                sources,
                folder,
            } => {
                w.u8(3);
                w.u64(*id);
                w.paths(sources);
                w.path(folder);
            }
            Request::Properties { id, paths } => {
                w.u8(4);
                w.u64(*id);
                w.paths(paths);
            }
            Request::OpenWith { id, path } => {
                w.u8(5);
                w.u64(*id);
                w.path(path);
            }
        }
        w.0
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader(bytes);
        let request = match r.u8()? {
            0 => Request::Hello { version: r.u16()? },
            1 => Request::Menu {
                id: r.u64()?,
                target: r.target()?,
            },
            2 => Request::Execute {
                id: r.u64()?,
                target: r.target()?,
                command: r.u32()?,
            },
            3 => Request::DropMenu {
                id: r.u64()?,
                sources: r.paths()?,
                folder: r.path()?,
            },
            4 => Request::Properties {
                id: r.u64()?,
                paths: r.paths()?,
            },
            5 => Request::OpenWith {
                id: r.u64()?,
                path: r.path()?,
            },
            t => return Err(DecodeError::UnknownTag(t)),
        };
        r.finish()?;
        Ok(request)
    }
}

impl Message for Response {
    fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer(Vec::new());
        match self {
            Response::HelloAccepted { version } => {
                w.u8(0);
                w.u16(*version);
            }
            Response::HelloRejected {
                min_version,
                max_version,
            } => {
                w.u8(1);
                w.u16(*min_version);
                w.u16(*max_version);
            }
            Response::Menu { id, items } => {
                w.u8(2);
                w.u64(*id);
                w.items(items);
            }
            Response::Done { id } => {
                w.u8(3);
                w.u64(*id);
            }
            Response::Error { id, kind, message } => {
                w.u8(4);
                w.u64(*id);
                w.u8(match kind {
                    ErrorKind::NotFound => 0,
                    ErrorKind::UnknownCommand => 1,
                    ErrorKind::Failed => 2,
                });
                w.text(message);
            }
        }
        w.0
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader(bytes);
        let response = match r.u8()? {
            0 => Response::HelloAccepted { version: r.u16()? },
            1 => Response::HelloRejected {
                min_version: r.u16()?,
                max_version: r.u16()?,
            },
            2 => Response::Menu {
                id: r.u64()?,
                items: r.items(0)?,
            },
            3 => Response::Done { id: r.u64()? },
            4 => Response::Error {
                id: r.u64()?,
                kind: match r.u8()? {
                    0 => ErrorKind::NotFound,
                    1 => ErrorKind::UnknownCommand,
                    2 => ErrorKind::Failed,
                    t => return Err(DecodeError::UnknownTag(t)),
                },
                message: r.text()?,
            },
            t => return Err(DecodeError::UnknownTag(t)),
        };
        r.finish()?;
        Ok(response)
    }
}
