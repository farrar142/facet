//! 위치의 항목 목록 읽기.
//!
//! 지금은 실제 폴더만 읽는다. S1에서 파일 시스템 공급자(ADR-0004)로 자란다.

use std::ffi::OsString;
use std::fmt;
use std::fs::{self, DirEntry, Metadata};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 위치 안의 항목 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// 항목 이름. 파일 시스템이 준 그대로다(UTF-16으로 짝이 맞지 않는 이름도 잃지 않는다).
    pub name: OsString,
    /// 폴더인가. 폴더를 가리키는 심볼릭 링크·정션도 폴더로 본다.
    pub is_folder: bool,
    /// 바이트 단위 크기. 폴더는 파일 시스템이 주는 값(보통 0)이다.
    pub size: u64,
    /// 마지막 수정 시각.
    pub modified: SystemTime,
}

/// 목록 읽기가 실패한 까닭.
#[derive(Debug)]
pub enum ListError {
    /// 경로가 없다.
    NotFound { path: PathBuf },
    /// 읽을 권한이 없다.
    PermissionDenied { path: PathBuf },
    /// 경로가 폴더가 아니다.
    NotAFolder { path: PathBuf },
    /// 그 밖의 입출력 오류.
    Io { path: PathBuf, source: io::Error },
}

impl ListError {
    fn from_io(path: &Path, source: io::Error) -> Self {
        let path = path.to_path_buf();
        match source.kind() {
            io::ErrorKind::NotFound => Self::NotFound { path },
            io::ErrorKind::PermissionDenied => Self::PermissionDenied { path },
            io::ErrorKind::NotADirectory => Self::NotAFolder { path },
            _ => Self::Io { path, source },
        }
    }

    /// 실패한 경로.
    pub fn path(&self) -> &Path {
        match self {
            Self::NotFound { path }
            | Self::PermissionDenied { path }
            | Self::NotAFolder { path }
            | Self::Io { path, .. } => path,
        }
    }
}

impl fmt::Display for ListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { path } => write!(f, "위치가 없습니다: {}", path.display()),
            Self::PermissionDenied { path } => {
                write!(f, "위치를 읽을 권한이 없습니다: {}", path.display())
            }
            Self::NotAFolder { path } => write!(f, "폴더가 아닙니다: {}", path.display()),
            Self::Io { path, source } => {
                write!(f, "위치를 읽지 못했습니다: {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for ListError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// 실제 폴더 `path`의 항목 목록을 읽는다.
///
/// 숨김 항목도 포함한다. 순서는 파일 시스템이 돌려준 순서이며 정렬하지 않는다.
/// 항목마다 시스템 호출을 더 하지 않는다: Windows에서는 디렉터리 열거 결과에
/// 크기·시각·속성이 이미 들어 있어 [`DirEntry::metadata`]가 그것을 재사용한다.
pub fn list_location(path: &Path) -> Result<Vec<Item>, ListError> {
    let entries = fs::read_dir(path).map_err(|e| ListError::from_io(path, e))?;
    let mut items = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| ListError::from_io(path, e))?;
        let metadata = entry
            .metadata()
            .map_err(|e| ListError::from_io(&entry.path(), e))?;
        items.push(Item {
            name: entry.file_name(),
            is_folder: is_folder(&entry, &metadata),
            size: metadata.len(),
            modified: metadata
                .modified()
                .map_err(|e| ListError::from_io(&entry.path(), e))?,
        });
    }
    Ok(items)
}

#[cfg(windows)]
fn is_folder(_entry: &DirEntry, metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    // 폴더 심볼릭 링크와 정션도 디렉터리 속성을 가진다.
    metadata.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0
}

#[cfg(not(windows))]
fn is_folder(entry: &DirEntry, metadata: &Metadata) -> bool {
    if metadata.is_symlink() {
        // 링크일 때만 대상을 한 번 더 본다.
        fs::metadata(entry.path()).is_ok_and(|m| m.is_dir())
    } else {
        metadata.is_dir()
    }
}
