//! 길이 접두 프레임: 본문 길이(4바이트, 리틀 엔디언) 뒤에 본문.
//!
//! 읽는 쪽은 머리의 길이가 [`MAX_FRAME_LEN`]을 넘으면 본문을 읽기 전에 거절하고, 본문은
//! 실제로 온 만큼만 조금씩 버퍼를 늘려 받는다. 그래서 머리만 큰 값을 보내도 메모리를 크게
//! 잡지 않는다.

use std::fmt;
use std::io::{self, ErrorKind, Read, Write};

use crate::wire::DecodeError;

/// 프레임 본문의 최대 바이트 수(8MiB). 결과 조각 수천 개(경로 260자 기준)가 넉넉히 들어간다.
pub const MAX_FRAME_LEN: u32 = 8 * 1024 * 1024;

/// 본문을 한 번에 늘려 받는 크기.
const CHUNK: usize = 64 * 1024;

/// 프레임이나 메시지를 읽지 못한 까닭.
#[derive(Debug)]
pub enum ReadError {
    /// 프레임 경계에서 스트림이 끝났다(상대가 연결을 닫았다).
    Closed,
    /// 머리의 본문 길이가 [`MAX_FRAME_LEN`]을 넘는다. 본문은 읽지 않았다.
    TooLarge { len: u32 },
    /// 머리나 본문 중간에서 스트림이 끝났다.
    Truncated,
    /// 본문이 메시지 형식에 맞지 않는다.
    Decode(DecodeError),
    /// 스트림 읽기 오류.
    Io(io::Error),
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => f.write_str("연결이 닫혔습니다"),
            Self::TooLarge { len } => write!(
                f,
                "프레임이 너무 큽니다: {len}바이트(최대 {MAX_FRAME_LEN}바이트)"
            ),
            Self::Truncated => f.write_str("프레임이 중간에서 끊겼습니다"),
            Self::Decode(e) => write!(f, "메시지를 읽지 못했습니다: {e}"),
            Self::Io(e) => write!(f, "읽기 오류: {e}"),
        }
    }
}

impl std::error::Error for ReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decode(e) => Some(e),
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<DecodeError> for ReadError {
    fn from(e: DecodeError) -> Self {
        Self::Decode(e)
    }
}

/// 프레임이나 메시지를 쓰지 못한 까닭.
#[derive(Debug)]
pub enum WriteError {
    /// 본문이 [`MAX_FRAME_LEN`]을 넘는다. 아무것도 쓰지 않았다.
    TooLarge { len: usize },
    /// 스트림 쓰기 오류.
    Io(io::Error),
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { len } => write!(
                f,
                "프레임이 너무 큽니다: {len}바이트(최대 {MAX_FRAME_LEN}바이트)"
            ),
            Self::Io(e) => write!(f, "쓰기 오류: {e}"),
        }
    }
}

impl std::error::Error for WriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::TooLarge { .. } => None,
        }
    }
}

impl From<io::Error> for WriteError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// 본문 하나를 프레임으로 쓴다. 본문이 최대 크기를 넘으면 쓰지 않고 오류다.
pub fn write_frame(w: &mut impl Write, body: &[u8]) -> Result<(), WriteError> {
    let len = u32::try_from(body.len())
        .ok()
        .filter(|&n| n <= MAX_FRAME_LEN)
        .ok_or(WriteError::TooLarge { len: body.len() })?;
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(body);
    w.write_all(&frame)?;
    w.flush()?;
    Ok(())
}

/// 프레임 하나를 읽어 본문을 돌려준다.
pub fn read_frame(r: &mut impl Read) -> Result<Vec<u8>, ReadError> {
    let mut head = [0u8; 4];
    let mut got = 0;
    while got < head.len() {
        match r.read(&mut head[got..]) {
            Ok(0) if got == 0 => return Err(ReadError::Closed),
            Ok(0) => return Err(ReadError::Truncated),
            Ok(n) => got += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(ReadError::Io(e)),
        }
    }
    let len = u32::from_le_bytes(head);
    if len > MAX_FRAME_LEN {
        return Err(ReadError::TooLarge { len });
    }

    let len = len as usize;
    let mut body = Vec::new();
    while body.len() < len {
        let start = body.len();
        let end = start + CHUNK.min(len - start);
        body.resize(end, 0);
        let mut filled = start;
        while filled < end {
            match r.read(&mut body[filled..end]) {
                Ok(0) => return Err(ReadError::Truncated),
                Ok(n) => filled += n,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(ReadError::Io(e)),
            }
        }
    }
    Ok(body)
}
