//! #431: zip 안 파일을 임시 폴더에 꺼내 기본 프로그램으로 열기.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// #431: 이보다 오래된 임시 하위 폴더를 지운다(24시간).
pub const CLEAN_AGE: Duration = Duration::from_secs(0); // #431

/// #431: 시스템 임시 폴더 아래 `facet\zip-open`.
pub fn default_root() -> PathBuf {
    todo!("#431")
}

/// #431: zip 항목 경로의 파일을 `root` 아래 새 하위 폴더에 읽기 전용으로 꺼낸다.
pub fn prepare(_root: &Path, _item: &Path) -> Result<PathBuf, ZipOpenError> {
    todo!("#431")
}

/// #431: `root` 아래 `CLEAN_AGE`보다 오래된 하위 폴더를 지우고 지운 수를 준다.
pub fn clean(_root: &Path, _now: SystemTime) -> usize {
    todo!("#431")
}

/// #431: 꺼내지 못한 까닭.
#[derive(Debug)]
pub enum ZipOpenError {
    /// #431
    NotInArchive,
    /// #431
    Entry(crate::zip_archive::EntryError),
    /// #431
    Io(std::io::Error),
}
