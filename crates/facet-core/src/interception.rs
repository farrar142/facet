//! #469: 폴더 열기 가로채기의 백업·복구.

use std::path::{Path, PathBuf};

use facet_win::registry::{Hive, RegValue, RegistryError};

/// #469: 백업 파일 이름.
pub const BACKUP_FILE_NAME: &str = "interception-backup.json";

/// #469: 바꿀 값 하나(루트 아래 상대 키, 값 이름, 새 값).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub key: String,
    pub name: String,
    pub value: RegValue,
}

/// #469: 가로채기 상태.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterceptionStatus {
    Off,
    On,
    Lost,
}

/// #469: 가로채기 오류.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterceptionError {
    Backup(std::io::ErrorKind),
    BackupBroken,
    NotEnabled,
    Registry(RegistryError),
    Portable,
}

/// #469: 하이브·루트 키·바꿀 값 목록·백업 파일 경로.
#[derive(Debug, Clone)]
pub struct Interception {}

impl Interception {
    /// #469: 가로채기를 만든다.
    pub fn new(
        _hive: Hive,
        _root: impl Into<String>,
        _changes: Vec<Change>,
        _backup_path: impl Into<PathBuf>,
    ) -> Interception {
        todo!("#469")
    }

    /// #469: 실제로 쓸 HKCU `Software\Classes` 아래 값 목록.
    pub fn for_facet(_exe: &Path, _backup_path: impl Into<PathBuf>) -> Interception {
        todo!("#469")
    }

    /// #469: 하이브.
    pub fn hive(&self) -> Hive {
        todo!("#469")
    }

    /// #469: 루트 키.
    pub fn root(&self) -> &str {
        todo!("#469")
    }

    /// #469: 바꿀 값 목록.
    pub fn changes(&self) -> &[Change] {
        todo!("#469")
    }

    /// #469: 백업 파일 경로.
    pub fn backup_path(&self) -> &Path {
        todo!("#469")
    }

    /// #469: 켜기.
    pub fn enable(&self) -> Result<(), InterceptionError> {
        todo!("#469")
    }

    /// #469: 끄기.
    pub fn disable(&self) -> Result<(), InterceptionError> {
        todo!("#469")
    }

    /// #469: 상태.
    pub fn status(&self) -> Result<InterceptionStatus, InterceptionError> {
        todo!("#469")
    }
}

/// #469: 백업대로 되돌리고 백업 파일을 지운다.
pub fn restore_from_backup(
    _hive: Hive,
    _root: &str,
    _backup_path: &Path,
) -> Result<(), InterceptionError> {
    todo!("#469")
}

/// #479: 되돌릴 사용자 프로필 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub hive: Hive,
    pub root: String,
    pub backup_path: PathBuf,
}

/// #479: 프로필 하나의 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileOutcome {
    Restored,
    NoBackup,
    Failed(InterceptionError),
}

/// #479: 프로필마다 되돌린 결과(넘긴 차례).
pub fn restore_profiles(_profiles: &[Profile]) -> Vec<(String, ProfileOutcome)> {
    todo!("#479")
}
