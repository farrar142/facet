//! 위치와 그 안의 항목, 목록 읽기 오류.
//!
//! 위치를 읽는 방법은 공급자([`crate::Provider`])가 맡는다(ADR-0004).

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 탐색해 들어갈 수 있는 곳.
///
/// 실제 폴더 경로에 묶이지 않는다. 공급자마다 변형 하나를 갖고, 압축 파일 내부(S13)나
/// 가상 폴더(S12) 공급자가 생기면 그 변형이 더해진다. 그래서 밖에서 `match`할 때는
/// 모르는 변형을 다루어야 한다.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Location {
    /// 파일 시스템 공급자가 읽는 실제 폴더. 로컬, 네트워크(UNC), 긴 경로(`\\?\`) 모두 여기다.
    FileSystem(PathBuf),
}

impl Location {
    /// 실제 폴더 `path`의 위치.
    pub fn file_system(path: impl Into<PathBuf>) -> Self {
        Self::FileSystem(path.into())
    }

    /// 실제 폴더 위치면 그 경로. 다른 공급자의 위치면 `None`이다.
    pub fn file_system_path(&self) -> Option<&Path> {
        match self {
            Self::FileSystem(path) => Some(path),
        }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileSystem(path) => write!(f, "{}", path.display()),
        }
    }
}

/// 위치 안의 항목 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// 항목 이름. 공급자가 준 그대로다(UTF-16으로 짝이 맞지 않는 이름도 잃지 않는다).
    pub name: OsString,
    /// 폴더인가. 폴더를 가리키는 심볼릭 링크·정션도 폴더로 본다.
    pub is_folder: bool,
    /// 바이트 단위 크기. 폴더는 파일 시스템이 주는 값(보통 0)이다.
    pub size: u64,
    /// 마지막 수정 시각.
    pub modified: SystemTime,
    /// 숨김·시스템·읽기 전용 속성.
    pub attributes: Attributes,
}

/// 항목의 속성. 기본값은 모두 꺼짐이다.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Attributes {
    /// 숨김 속성.
    pub hidden: bool,
    /// 시스템 속성.
    pub system: bool,
    /// 읽기 전용 속성.
    pub read_only: bool,
}

/// 목록 읽기가 실패한 까닭.
#[derive(Debug)]
pub enum ListError {
    /// 위치가 없다.
    NotFound { location: Location },
    /// 읽을 권한이 없다.
    PermissionDenied { location: Location },
    /// 위치가 폴더가 아니다.
    NotAFolder { location: Location },
    /// 그 밖의 입출력 오류.
    Io {
        location: Location,
        source: io::Error,
    },
}

impl ListError {
    pub(crate) fn from_io(location: Location, source: io::Error) -> Self {
        match source.kind() {
            io::ErrorKind::NotFound => Self::NotFound { location },
            io::ErrorKind::PermissionDenied => Self::PermissionDenied { location },
            io::ErrorKind::NotADirectory => Self::NotAFolder { location },
            _ => Self::Io { location, source },
        }
    }

    /// 실패한 위치.
    pub fn location(&self) -> &Location {
        match self {
            Self::NotFound { location }
            | Self::PermissionDenied { location }
            | Self::NotAFolder { location }
            | Self::Io { location, .. } => location,
        }
    }
}

impl fmt::Display for ListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { location } => write!(f, "위치가 없습니다: {location}"),
            Self::PermissionDenied { location } => {
                write!(f, "위치를 읽을 권한이 없습니다: {location}")
            }
            Self::NotAFolder { location } => write!(f, "폴더가 아닙니다: {location}"),
            Self::Io { location, source } => {
                write!(f, "위치를 읽지 못했습니다: {location}: {source}")
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
