//! 셸 네임스페이스의 폴더 하나를 열거한다(S12 셸 공급자, ADR-0004). `unsafe`는 `windows_shell_folder.rs`에 있다.
//!
//! 폴더는 셸 항목 식별자(PIDL)를 바이트로 직렬화한 값으로 가리킨다(바이트 그대로, 끝의 0 두 바이트 포함). 함수는 부른
//! 스레드에서 셸을 부르고 기다리므로 COM STA 스레드([`crate::shell_thread::ShellThread`])에서만 부른다. UI 스레드에서
//! 부르지 않는다. 화면 문장은 없다. Windows가 아니면 「지원하지 않음」이다.

use std::path::PathBuf;

/// 셸 폴더 안의 항목 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellEntry {
    /// 셸의 표시 이름(`SIGDN_NORMALDISPLAY`, 예: 「로컬 디스크 (C:)」).
    pub name: String,
    /// 실제 파일 시스템 경로가 있으면 그 경로(`SFGAO_FILESYSTEM`일 때 `SIGDN_FILESYSPATH`, 예: `C:\`).
    pub path: Option<PathBuf>,
    /// 바탕 화면부터의 해석 이름(`SIGDN_DESKTOPABSOLUTEPARSING`). 장치(휴대폰)는 `\?\` 장치 경로를 품는다.
    pub parsing_name: String,
    /// 폴더인가(`SFGAO_FOLDER`).
    pub is_folder: bool,
    /// 이 항목의 절대 PIDL 바이트.
    pub id: Vec<u8>,
    /// 셸 속성의 크기(바이트, `PKEY_Size`). [`enumerate_detailed`]로 읽은 파일만 있고, 장치가 주지 않으면 없다.
    pub size: Option<u64>,
    /// 셸 속성의 수정한 날짜(`FILETIME`, `PKEY_DateModified`). [`enumerate_detailed`]로 읽은 것만 있고, 장치가 주지
    /// 않으면 없다.
    pub modified: Option<u64>,
}

/// 셸 폴더를 읽지 못한 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellFolderError {
    /// 그 식별자의 셸 항목이 없다(장치가 빠짐 등).
    NotFound,
    /// 식별자 바이트가 PIDL 형식이 아니다.
    InvalidId,
    /// 셸이 실패를 알렸다(`code`는 HRESULT).
    Failed { code: i32 },
    /// 이 환경(Windows가 아닌 곳)에서는 셸을 부를 수 없다.
    Unsupported,
}

impl std::fmt::Display for ShellFolderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("셸 항목이 없습니다"),
            Self::InvalidId => f.write_str("셸 항목 식별자가 틀렸습니다"),
            Self::Failed { code } => write!(f, "셸 호출이 실패했습니다 (0x{code:08X})"),
            Self::Unsupported => f.write_str("이 환경에서는 셸을 부를 수 없습니다"),
        }
    }
}

impl std::error::Error for ShellFolderError {}

/// 식별자 바이트가 PIDL 형식인가: 조각마다 길이(2바이트, 리틀 엔디언)가 2 이상이고 끝이 길이 0의 조각(0 두 바이트)이다.
pub fn is_valid_id(id: &[u8]) -> bool {
    let mut rest = id;
    loop {
        let Some((cb, _)) = rest.split_first_chunk::<2>() else {
            return false;
        };
        let cb = usize::from(u16::from_le_bytes(*cb));
        if cb == 0 {
            return rest.len() == 2;
        }
        if cb < 2 || cb > rest.len() {
            return false;
        }
        rest = &rest[cb..];
    }
}

/// 식별자 `id`의 셸 폴더 자신의 표시 이름(`SIGDN_NORMALDISPLAY`). COM STA 스레드에서 부른다.
#[cfg(windows)]
pub fn display_name(id: &[u8]) -> Result<String, ShellFolderError> {
    if !is_valid_id(id) {
        return Err(ShellFolderError::InvalidId);
    }
    crate::windows_shell_folder::display_name(id)
}

/// Windows가 아니면 늘 「지원하지 않음」이다.
#[cfg(not(windows))]
pub fn display_name(_id: &[u8]) -> Result<String, ShellFolderError> {
    Err(ShellFolderError::Unsupported)
}

/// 식별자 `id`의 셸 폴더 안 항목을 셸이 주는 차례대로 열거해 `batch`개 이하씩 `on_batch`에 넘긴다. `on_batch`가
/// `false`를 돌려주면 그만두고 `Ok`다(부른 쪽이 취소한 것). 숨김 항목도 넣는다. COM STA 스레드에서 부른다.
#[cfg(windows)]
pub fn enumerate(
    id: &[u8],
    batch: usize,
    on_batch: &mut dyn FnMut(Vec<ShellEntry>) -> bool,
) -> Result<(), ShellFolderError> {
    if !is_valid_id(id) {
        return Err(ShellFolderError::InvalidId);
    }
    crate::windows_shell_folder::enumerate(id, batch.max(1), false, on_batch)
}

/// [`enumerate`]와 같되 항목마다 크기·수정한 날짜 셸 속성([`ShellEntry::size`]·[`ShellEntry::modified`])도 읽는다
/// (휴대폰 폴더의 열). COM STA 스레드에서 부른다.
#[cfg(windows)]
pub fn enumerate_detailed(
    id: &[u8],
    batch: usize,
    on_batch: &mut dyn FnMut(Vec<ShellEntry>) -> bool,
) -> Result<(), ShellFolderError> {
    if !is_valid_id(id) {
        return Err(ShellFolderError::InvalidId);
    }
    crate::windows_shell_folder::enumerate(id, batch.max(1), true, on_batch)
}

/// Windows가 아니면 늘 「지원하지 않음」이다.
#[cfg(not(windows))]
pub fn enumerate_detailed(
    _id: &[u8],
    _batch: usize,
    _on_batch: &mut dyn FnMut(Vec<ShellEntry>) -> bool,
) -> Result<(), ShellFolderError> {
    Err(ShellFolderError::Unsupported)
}

/// 식별자 `id`의 셸 항목의 바탕 화면부터의 해석 이름(`SIGDN_DESKTOPABSOLUTEPARSING`, 휴대폰은 `\\?\` 장치 경로를
/// 품는다). COM STA 스레드에서 부른다.
#[cfg(windows)]
pub fn parsing_name(id: &[u8]) -> Result<String, ShellFolderError> {
    if !is_valid_id(id) {
        return Err(ShellFolderError::InvalidId);
    }
    crate::windows_shell_folder::parsing_name(id)
}

/// Windows가 아니면 늘 「지원하지 않음」이다.
#[cfg(not(windows))]
pub fn parsing_name(_id: &[u8]) -> Result<String, ShellFolderError> {
    Err(ShellFolderError::Unsupported)
}

/// 바탕 화면부터의 해석 이름 `name`의 셸 항목 식별자(절대 PIDL 바이트, `SHParseDisplayName`). 그 항목이 없으면
/// 「없음」이다. COM STA 스레드에서 부른다.
#[cfg(windows)]
pub fn id_from_parsing_name(name: &str) -> Result<Vec<u8>, ShellFolderError> {
    crate::windows_shell_folder::id_from_parsing_name(name)
}

/// Windows가 아니면 늘 「지원하지 않음」이다.
#[cfg(not(windows))]
pub fn id_from_parsing_name(_name: &str) -> Result<Vec<u8>, ShellFolderError> {
    Err(ShellFolderError::Unsupported)
}

/// Windows가 아니면 늘 「지원하지 않음」이다.
#[cfg(not(windows))]
pub fn enumerate(
    _id: &[u8],
    _batch: usize,
    _on_batch: &mut dyn FnMut(Vec<ShellEntry>) -> bool,
) -> Result<(), ShellFolderError> {
    Err(ShellFolderError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pidl_형식을_확인한다() {
        assert!(is_valid_id(&[0, 0]));
        assert!(is_valid_id(&[4, 0, 9, 9, 0, 0]));
        assert!(!is_valid_id(&[]));
        assert!(!is_valid_id(&[0]));
        assert!(!is_valid_id(&[4, 0, 9, 9]));
        assert!(!is_valid_id(&[1, 0, 0, 0]));
        assert!(!is_valid_id(&[9, 0, 0, 0]));
        assert!(!is_valid_id(&[0, 0, 0, 0]));
    }
}
