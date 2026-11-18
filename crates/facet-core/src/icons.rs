//! #325: 파일 종류별 아이콘: 키 규칙과 메모리 캐시(얻기는 UI 밖 스레드).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::Item;

pub use facet_win::icon::IconImage;

/// #325: 아이콘 캐시의 키.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IconKey {
    Folder,
    Extension(String),
    NoExtension,
    File(PathBuf),
}

/// #325: 파일마다 아이콘이 다른 확장자.
pub const PER_FILE_EXTENSIONS: [&str; 4] = ["exe", "ico", "lnk", "url"];

/// #325: 항목의 아이콘 키.
pub fn icon_key(_folder: &Path, _item: &Item) -> IconKey {
    todo!("#325")
}

/// #325: 아이콘 메모리 캐시.
pub struct IconCache {
    _todo: Vec<u8>, // #325
}

impl IconCache {
    /// #325: 시스템 아이콘을 얻는 캐시.
    pub fn new() -> IconCache {
        todo!("#325")
    }

    /// #325: 얻기 함수를 주입한 캐시.
    pub fn with_loader(
        _loader: impl Fn(&IconKey, u32) -> Option<IconImage> + Send + Sync + 'static,
    ) -> IconCache {
        todo!("#325")
    }

    /// #325: 캐시에 있으면 그림, 없으면 `None`(얻기를 맡김).
    pub fn get(&self, _key: &IconKey, _size: u32) -> Option<Arc<IconImage>> {
        todo!("#325")
    }

    /// #325: 얻기가 끝날 때 부를 깨우기.
    pub fn set_waker(&self, _waker: impl Fn() + Send + Sync + 'static) {
        todo!("#325")
    }

    /// #325: 얻기가 끝난 키·크기들.
    pub fn poll(&self) -> Vec<(IconKey, u32)> {
        todo!("#325")
    }

    /// #325: 얻기가 끝난 키·크기 하나를 기다린다.
    pub fn wait_event(&self, _timeout: Duration) -> Option<(IconKey, u32)> {
        todo!("#325")
    }
}

impl Default for IconCache {
    fn default() -> Self {
        Self::new()
    }
}
