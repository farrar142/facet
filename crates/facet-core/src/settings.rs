//! #458: 설정의 단일 정의와 설정 파일(`settings.toml`) 읽기·검증·형식 보존 쓰기.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::command::{CommandId, Shortcut};
use crate::keymap::Keymap;

/// #458: 설정 파일 이름.
pub const FILE_NAME: &str = "settings.toml";

/// #458: 설정 폴더 `%APPDATA%\facet`.
pub fn settings_folder() -> Option<PathBuf> {
    todo!("#458")
}

/// #458: 설정 분류.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    General,
    View,
    Shortcuts,
    Search,
    Thumbnails,
    Network,
    Advanced,
}

/// #458: 설정 값.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
    Integer(i64),
    Choice(&'static str),
}

/// #458: 설정의 형과 범위.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bool,
    Integer { min: i64, max: i64 },
    Choice(&'static [&'static str]),
}

/// #458: 설정 항목 하나의 정의.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Definition {
    pub key: &'static str,
    pub category: Category,
    pub kind: Kind,
    pub default: Value,
}

/// #458: 모든 설정 정의.
pub fn definitions() -> &'static [Definition] {
    todo!("#458")
}

/// #458: 키로 정의를 찾는다.
pub fn definition(_key: &str) -> Option<&'static Definition> {
    todo!("#458")
}

/// #458: 키·앱이 준 (표시 이름, 설명)의 대소문자 무시 부분 일치, 정의 순서.
pub fn search<S: AsRef<str>>(
    _query: &str,
    _text: impl Fn(&Definition) -> (S, S),
) -> Vec<&'static Definition> {
    todo!("#458")
}

/// #458: 받지 않는 값의 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueError {
    UnknownKey,
    WrongType,
    OutOfRange { min: i64, max: i64 },
    NotAChoice,
    InvalidShortcut,
}

/// #458: 설정 파일의 문법 오류(줄·열은 1부터).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

/// #458: 설정 파일의 틀린 값.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub key: String,
    pub line: usize,
    pub column: usize,
    pub error: ValueError,
}

/// #458: 파일에 쓰지 못한 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveError {
    FileBroken,
    Write(std::io::ErrorKind),
}

/// #458: 바꾼 값을 파일에 썼는가.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Saved {
    Written,
    KeptInMemory(SaveError),
}

/// #464: 쓸 모양.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}

/// #464: 글꼴 크기(포인트)에 맞춘 행 높이(논리 픽셀).
pub fn auto_row_height(_font_size: u32) -> u32 {
    todo!("#464")
}

/// #458: 적용 중인 설정과 설정 파일.
#[derive(Debug, Clone)]
pub struct Settings {}

impl Settings {
    /// #458: 파일 없는 기본값.
    pub fn defaults() -> Settings {
        todo!("#458")
    }

    /// #458: 설정 폴더의 설정 파일을 읽는다.
    pub fn load(_folder: &Path) -> Settings {
        todo!("#458")
    }

    /// #458: 적용 중인 값.
    pub fn get(&self, _key: &str) -> Option<Value> {
        todo!("#458")
    }

    /// #458: 기본값과 다른가.
    pub fn is_modified(&self, _key: &str) -> bool {
        todo!("#458")
    }

    /// #458: 값을 바꾼다.
    pub fn set(&mut self, _key: &str, _value: Value) -> Result<Saved, ValueError> {
        todo!("#458")
    }

    /// #458: 기본값으로 돌린다.
    pub fn reset(&mut self, _key: &str) -> Result<Saved, ValueError> {
        todo!("#458")
    }

    /// #458: 파일의 문법 오류.
    pub fn syntax_error(&self) -> Option<&SyntaxError> {
        todo!("#458")
    }

    /// #458: 파일의 틀린 값.
    pub fn problems(&self) -> &[Problem] {
        todo!("#458")
    }

    /// #458: 읽은 설정 파일 경로.
    pub fn file_path(&self) -> Option<PathBuf> {
        todo!("#458")
    }

    /// #460: 단축키 모델.
    pub fn keymap(&self) -> &Keymap {
        todo!("#460")
    }

    /// #460: 명령의 단축키를 바꾼다.
    pub fn set_shortcuts(&mut self, _command: CommandId, _shortcuts: Vec<Shortcut>) -> Saved {
        todo!("#460")
    }

    /// #460: 명령의 단축키를 프리셋 기본으로.
    pub fn reset_shortcuts(&mut self, _command: CommandId) -> Saved {
        todo!("#460")
    }

    /// #460: 모든 단축키를 프리셋 기본으로.
    pub fn reset_all_shortcuts(&mut self) -> Saved {
        todo!("#460")
    }

    /// #464: 쓸 모양.
    pub fn appearance(&self, _system_dark: bool) -> Appearance {
        todo!("#464")
    }

    /// #464: 글꼴 크기(포인트).
    pub fn font_size(&self) -> u32 {
        todo!("#464")
    }

    /// #464: 행 높이(논리 픽셀).
    pub fn row_height(&self) -> u32 {
        todo!("#464")
    }

    /// #465: 내용 검색 큰 파일 한도(바이트).
    pub fn content_max_file_size(&self) -> u64 {
        todo!("#465")
    }

    /// #465: 썸네일 디스크 캐시 상한(바이트).
    pub fn thumbnail_disk_limit(&self) -> u64 {
        todo!("#465")
    }

    /// #465: 셸 확장 도우미 응답 대기 시간.
    pub fn shell_reply_timeout(&self) -> Duration {
        todo!("#465")
    }

    /// #465: 셸 확장 도우미 유휴 종료 시간.
    pub fn shell_idle_timeout(&self) -> Duration {
        todo!("#465")
    }
}
