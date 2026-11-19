//! #422: zip 파일의 central directory를 읽는 자체 리더(ADR-0004의 압축 파일 공급자).

use std::path::Path;
use std::time::SystemTime;

/// #422: 연 zip 파일의 항목과 폴더 나무.
pub struct Archive;

impl Archive {
    /// #422: zip 파일을 열어 central directory를 읽는다.
    pub fn open(_path: &Path) -> Result<Archive, ArchiveError> {
        todo!("#422")
    }

    /// #422: 읽을 수 있었던 central directory 항목(차례 그대로).
    pub fn entries(&self) -> &[ArchiveEntry] {
        todo!("#422")
    }

    /// #422: zip 안 폴더 `folder`의 바로 아래 항목. 없으면 `None`.
    pub fn children(&self, _folder: &str) -> Option<Vec<ArchiveEntry>> {
        todo!("#422")
    }

    /// #422: UTF-8 표시 없는 이름에 고른 인코딩.
    pub fn encoding(&self) -> NameEncoding {
        todo!("#422")
    }

    /// #422: 같은 경로가 앞에 또 있어 가려진 기록 수.
    pub fn duplicates(&self) -> usize {
        todo!("#422")
    }

    /// #422: central directory 일부를 읽지 못했는가.
    pub fn damaged(&self) -> bool {
        todo!("#422")
    }

    /// #422: 암호화된 항목이 하나라도 있는가.
    pub fn encrypted(&self) -> bool {
        todo!("#422")
    }

    /// #425: zip 안 파일 `path`를 풀어 내용을 돌려준다.
    pub fn read_entry(&self, _path: &str, _password: Option<&str>) -> Result<Vec<u8>, EntryError> {
        todo!("#425")
    }
}

/// #422: central directory 항목 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    /// #422
    pub path: String,
    /// #422
    pub name: String,
    /// #422
    pub is_folder: bool,
    /// #422
    pub size: u64,
    /// #422
    pub compressed_size: u64,
    /// #422
    pub modified: Option<SystemTime>,
    /// #422
    pub method: u16,
    /// #422
    pub encrypted: bool,
    /// #422
    pub symlink: bool,
}

/// #422: 이름 인코딩.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameEncoding {
    /// #422
    Utf8,
    /// #422
    Cp949,
    /// #422
    Cp437,
}

/// #422: zip을 열지 못한 까닭.
#[derive(Debug)]
pub enum ArchiveError {
    /// #422
    NotZip,
    /// #422
    Damaged,
    /// #422
    Io(std::io::Error),
}

/// #425: 항목을 풀지 못한 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryError {
    /// #425
    NotFound,
    /// #425
    NotAFile,
    /// #425
    PasswordNeeded,
    /// #425
    WrongPassword,
    /// #425
    UnsupportedMethod(u16),
    /// #425
    Corrupt,
    /// #425
    Io(std::io::ErrorKind),
}
