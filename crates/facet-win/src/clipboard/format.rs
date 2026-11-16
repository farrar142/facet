//! 클립보드 형식: 탐색기와 같은 바이트·글자를 만들고 읽는 순수 함수.
//!
//! 실제 클립보드는 부르지 않는다(그건 [`super::Clipboard`]의 STA 스레드 몫이고, 그 스레드가 이 함수들로 바이트를 만들고 읽는다).
//! 코어가 이 크레이트에 기대므로 형식은 여기 두고 코어는 `facet_core::clipboard`로 다시 내보낸다. 화면 문장은 없다(ADR-0007).
//!
//! - 파일 목록 `CF_HDROP`: [`hdrop_bytes`]·[`parse_hdrop`]. 유니코드 `DROPFILES` 머리(20바이트: 경로 시작
//!   오프셋 `pFiles`, 점 `pt` 둘, `fNC`, `fWide`) + 널로 끝나는 UTF-16LE 경로들 + 끝 널.
//! - 잘라내기 표시 `Preferred DropEffect`: [`preferred_drop_effect_bytes`]·[`parse_preferred_drop_effect`].
//! - 이동으로 붙여넣은 뒤 원본 앱에 알리는 `Performed DropEffect`·`Paste Succeeded`:
//!   [`performed_drop_effect_bytes`]·[`paste_succeeded_bytes`].
//! - 경로 복사 글자: [`copy_as_path_text`].
//!
//! 정한 것:
//! - **ANSI 목록은 거절한다**(`fWide`가 0이면 [`HdropError::Ansi`]). 해석에 현재 코드 페이지가 필요해
//!   순수 함수로 안전하게 읽을 수 없고, 요즘 셸은 늘 유니코드 목록을 준다.
//! - 모르는 잘라내기 값은 **복사**로 본다. 이동(원본 삭제)은 값에 MOVE(2) 비트가 있고 COPY(1) 비트가 없을
//!   때만이다. 0·링크(4)·복사+이동(3) 같은 값은 원본을 지우지 않는 복사다. 값이 4바이트보다 짧으면 `None`,
//!   길면 앞 4바이트만 본다.

use std::fmt;
use std::path::{Path, PathBuf};

/// `DROPFILES` 머리 길이(바이트).
pub const DROPFILES_HEADER_LEN: usize = 20;
/// 파일 목록 형식 번호(`CF_HDROP`, 표준 형식이라 이름이 없다).
pub const CF_HDROP: u32 = 15;
/// 잘라내기 표시 형식 이름.
pub const PREFERRED_DROP_EFFECT: &str = "Preferred DropEffect";
/// 붙여넣기를 마친 종류를 알리는 형식 이름.
pub const PERFORMED_DROP_EFFECT: &str = "Performed DropEffect";
/// 붙여넣기가 성공했음을 알리는 형식 이름.
pub const PASTE_SUCCEEDED: &str = "Paste Succeeded";

const DROPEFFECT_COPY: u32 = 1;
const DROPEFFECT_MOVE: u32 = 2;

/// 클립보드에 놓인 파일을 붙여넣을 때의 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardOperation {
    /// 복사.
    Copy,
    /// 잘라내기(붙여넣은 뒤 원본을 옮김).
    Move,
}

impl ClipboardOperation {
    fn effect(self) -> u32 {
        match self {
            Self::Copy => DROPEFFECT_COPY,
            Self::Move => DROPEFFECT_MOVE,
        }
    }
}

/// 파일 목록 바이트를 읽지 못한 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdropError {
    /// 머리(20바이트)보다 짧다.
    TooShort,
    /// 경로 시작 오프셋이 머리 안이거나 데이터 밖이다.
    BadOffset,
    /// ANSI 목록(`fWide`가 0). 거절한다.
    Ansi,
    /// 경로 데이터 길이가 2의 배수가 아니다.
    OddLength,
    /// 끝 널이 없다(잘림).
    Unterminated,
    /// 짝이 맞지 않는 UTF-16 대용 글자가 있어 경로로 읽을 수 없다(Windows 밖에서만).
    InvalidText,
}

impl fmt::Display for HdropError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for HdropError {}

fn to_wide(path: &Path) -> Vec<u16> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str().encode_wide().collect()
    }
    #[cfg(not(windows))]
    {
        path.to_string_lossy().encode_utf16().collect()
    }
}

fn from_wide(units: &[u16]) -> Result<PathBuf, HdropError> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        Ok(PathBuf::from(std::ffi::OsString::from_wide(units)))
    }
    #[cfg(not(windows))]
    {
        String::from_utf16(units)
            .map(PathBuf::from)
            .map_err(|_| HdropError::InvalidText)
    }
}

/// 경로 목록을 `CF_HDROP` 바이트(유니코드 `DROPFILES`)로 만든다. 경로 길이 제한은 없다(260자 넘어도 됨).
/// 경로 안의 널 글자는 그 경로를 거기서 자른다(Windows 경로에는 없다).
pub fn hdrop_bytes<P: AsRef<Path>>(paths: &[P]) -> Vec<u8> {
    let mut wide: Vec<u16> = Vec::new();
    for path in paths {
        let units = to_wide(path.as_ref());
        let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
        wide.extend_from_slice(&units[..end]);
        wide.push(0);
    }
    wide.push(0);
    let mut out = Vec::with_capacity(DROPFILES_HEADER_LEN + wide.len() * 2);
    out.extend_from_slice(&(DROPFILES_HEADER_LEN as u32).to_le_bytes()); // pFiles
    out.extend_from_slice(&[0; 8]); // pt
    out.extend_from_slice(&0u32.to_le_bytes()); // fNC
    out.extend_from_slice(&1u32.to_le_bytes()); // fWide
    for unit in wide {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// `CF_HDROP` 바이트에서 경로 목록을 읽는다. 잘리거나 깨진 바이트는 패닉 없이 [`HdropError`]다.
/// 빈 경로(널이 연달아 나옴)가 목록의 끝이고 그 뒤 바이트는 무시한다.
pub fn parse_hdrop(bytes: &[u8]) -> Result<Vec<PathBuf>, HdropError> {
    if bytes.len() < DROPFILES_HEADER_LEN {
        return Err(HdropError::TooShort);
    }
    let offset = u32_at(bytes, 0) as usize;
    if offset < DROPFILES_HEADER_LEN || offset > bytes.len() {
        return Err(HdropError::BadOffset);
    }
    if u32_at(bytes, 16) == 0 {
        return Err(HdropError::Ansi);
    }
    let data = &bytes[offset..];
    if !data.len().is_multiple_of(2) {
        return Err(HdropError::OddLength);
    }
    let units: Vec<u16> = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    let mut paths = Vec::new();
    let mut rest = &units[..];
    loop {
        let Some(end) = rest.iter().position(|&u| u == 0) else {
            return Err(HdropError::Unterminated);
        };
        if end == 0 {
            return Ok(paths);
        }
        paths.push(from_wide(&rest[..end])?);
        rest = &rest[end + 1..];
    }
}

/// `Preferred DropEffect` 값(4바이트, 리틀 엔디언 DROPEFFECT).
pub fn preferred_drop_effect_bytes(operation: ClipboardOperation) -> [u8; 4] {
    operation.effect().to_le_bytes()
}

/// `Performed DropEffect` 값: 붙여넣기를 이동으로 마친 쪽이 원본 앱에 알린다.
pub fn performed_drop_effect_bytes(operation: ClipboardOperation) -> [u8; 4] {
    operation.effect().to_le_bytes()
}

/// `Paste Succeeded` 값: 붙여넣기가 성공한 종류(같은 DROPEFFECT 4바이트).
pub fn paste_succeeded_bytes(operation: ClipboardOperation) -> [u8; 4] {
    operation.effect().to_le_bytes()
}

/// `Preferred DropEffect`(또는 같은 모양의 값)를 읽는다. 모르는 값은 복사다(모듈 문서). 4바이트보다 짧으면 `None`.
pub fn parse_preferred_drop_effect(bytes: &[u8]) -> Option<ClipboardOperation> {
    if bytes.len() < 4 {
        return None;
    }
    let value = u32_at(bytes, 0);
    Some(
        if value & DROPEFFECT_MOVE != 0 && value & DROPEFFECT_COPY == 0 {
            ClipboardOperation::Move
        } else {
            ClipboardOperation::Copy
        },
    )
}

/// 경로 복사 글자: 경로마다 큰따옴표로 감싸 줄마다 하나(`\r\n`으로 잇고 끝에 줄바꿈 없음, 탐색기 「경로로 복사」와 같음).
pub fn copy_as_path_text<P: AsRef<Path>>(paths: &[P]) -> String {
    paths
        .iter()
        .map(|p| format!("\"{}\"", p.as_ref().to_string_lossy()))
        .collect::<Vec<_>>()
        .join("\r\n")
}
