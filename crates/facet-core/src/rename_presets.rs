//! #282: 일괄 이름 바꾸기의 규칙 묶음 저장소(`rename-presets.toml`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::batch_rename::Rule;

/// #282: 파일 형식 버전.
pub const FORMAT_VERSION: u64 = 1;

/// #282: 이름 붙인 규칙 묶음 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    pub name: String,
    pub rules: Vec<Rule>,
}

/// #282: 규칙 묶음 모음(묶음들·마지막 규칙·건너뛴 묶음).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Presets {}

impl Presets {
    /// #282: 빈 모음.
    pub fn new() -> Presets {
        todo!("#282")
    }

    /// #282: 묶음들(파일·저장 차례).
    pub fn presets(&self) -> &[Preset] {
        todo!("#282")
    }

    /// #282: 이름으로 찾기(앞뒤 공백 걷고 대소문자 무시).
    pub fn find(&self, _name: &str) -> Option<&Preset> {
        todo!("#282")
    }

    /// #282: 이름 붙여 저장(같은 이름이면 규칙을 바꿈).
    pub fn save_preset(&mut self, _name: &str, _rules: Vec<Rule>) -> Result<(), PresetError> {
        todo!("#282")
    }

    /// #282: 묶음 이름 바꾸기.
    pub fn rename_preset(&mut self, _from: &str, _to: &str) -> Result<(), PresetError> {
        todo!("#282")
    }

    /// #282: 묶음 지우기.
    pub fn delete_preset(&mut self, _name: &str) -> Result<(), PresetError> {
        todo!("#282")
    }

    /// #282: 마지막에 쓴 규칙.
    pub fn last(&self) -> Option<&[Rule]> {
        todo!("#282")
    }

    /// #282: 마지막에 쓴 규칙 정하기.
    pub fn set_last(&mut self, _rules: Vec<Rule>) {
        todo!("#282")
    }

    /// #282: 읽을 때 건너뛴 묶음(파일 차례).
    pub fn skipped(&self) -> &[SkippedPreset] {
        todo!("#282")
    }

    /// #282: TOML 글자로.
    pub fn to_toml(&self) -> String {
        todo!("#282")
    }

    /// #282: TOML 글자에서.
    pub fn from_toml(_text: &str) -> Result<Presets, PresetsError> {
        todo!("#282")
    }
}

/// #282: 묶음 조작의 거부 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetError {
    EmptyName,
    NameTaken,
    NotFound,
}

/// #282: 건너뛴 묶음(이름이 있으면 이름, 해석기 설명).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedPreset {
    pub name: Option<String>,
    pub error: String,
}

/// #282: 파일 전체를 읽지 못한 까닭.
#[derive(Debug)]
pub enum PresetsError {
    Io(std::io::Error),
    Invalid(String),
    MissingVersion,
    UnknownVersion(String),
}

/// #282: 파일을 읽은 결과.
#[derive(Debug)]
pub enum Loaded {
    Missing,
    Read(Presets),
    Broken {
        error: PresetsError,
        kept: Option<PathBuf>,
    },
}

/// #282: `%APPDATA%\facet\rename-presets.toml`.
pub fn presets_path() -> Option<PathBuf> {
    todo!("#282")
}

/// #282: 환경 변수를 읽는 함수로 정한 파일 자리.
pub fn presets_path_with(_var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    todo!("#282")
}

/// #282: 옆 임시 파일 자리(이름 뒤에 `.tmp`).
pub fn temp_path(_path: &Path) -> PathBuf {
    todo!("#282")
}

/// #282: 임시 파일에 쓰고 바꿔치기.
pub fn save(_path: &Path, _presets: &Presets) -> std::io::Result<()> {
    todo!("#282")
}

/// #282: 파일 읽기.
pub fn load(_path: &Path) -> Loaded {
    todo!("#282")
}
