//! #328: 썸네일 캐시(디스크·메모리).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::thumbnail::{Thumbnail, ThumbnailKey};

/// #328: 디스크 크기 상한(1GB).
pub const DEFAULT_DISK_LIMIT: u64 = 1 << 30;
/// #328: 메모리 크기 상한(256MB).
pub const DEFAULT_MEMORY_LIMIT: u64 = 256 << 20;

/// #328: 캐시 상한.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheLimits {
    pub disk_bytes: u64,
    pub memory_bytes: u64,
}

impl Default for CacheLimits {
    fn default() -> Self {
        CacheLimits {
            disk_bytes: DEFAULT_DISK_LIMIT,
            memory_bytes: DEFAULT_MEMORY_LIMIT,
        }
    }
}

/// #328: 기본 캐시 위치(`%LOCALAPPDATA%\facet\thumbs`).
pub fn default_cache_dir() -> Option<PathBuf> {
    todo!("#328")
}

/// #328: 썸네일 캐시.
pub struct ThumbnailCache {
    _todo: Vec<u8>, // #328
}

impl ThumbnailCache {
    /// #328: 캐시 폴더를 연다.
    pub fn open(_dir: &Path, _limits: CacheLimits) -> std::io::Result<ThumbnailCache> {
        todo!("#328")
    }

    /// #328: 키로 꺼낸다.
    pub fn get(&self, _key: &ThumbnailKey) -> Option<Arc<Thumbnail>> {
        todo!("#328")
    }

    /// #328: 넣는다.
    pub fn put(&self, _key: &ThumbnailKey, _thumbnail: Arc<Thumbnail>) -> std::io::Result<()> {
        todo!("#328")
    }

    /// #328: 디스크 상한 아래가 될 때까지 가장 오래 쓰지 않은 것부터 지운다.
    pub fn trim(&self) -> std::io::Result<()> {
        todo!("#328")
    }

    /// #328: 모두 비운다.
    pub fn clear(&self) -> std::io::Result<()> {
        todo!("#328")
    }

    /// #328: 디스크에 둔 크기.
    pub fn disk_bytes(&self) -> u64 {
        todo!("#328")
    }

    /// #328: 메모리에 둔 크기.
    pub fn memory_bytes(&self) -> u64 {
        todo!("#328")
    }
}
