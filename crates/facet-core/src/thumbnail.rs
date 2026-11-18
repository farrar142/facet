//! #326: 썸네일 만들기(자체 디코더: JPEG·PNG·GIF·WebP·BMP).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::SystemTime;

use crate::Item;

/// #326: 썸네일 그림(RGBA8, 행 우선, 길이 `width * height * 4`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// #326: 썸네일을 만들지 못한 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailError {
    Unsupported,
    NotFound,
    AccessDenied,
    TooLarge,
    Corrupt,
    Io(std::io::ErrorKind),
    Cancelled,
    Panicked,
}

/// #326: 디코더 화소 수 상한.
pub const DEFAULT_MAX_PIXELS: u64 = 100_000_000;
/// #326: 디코더에 넣을 파일 크기 상한.
pub const DEFAULT_MAX_FILE_SIZE: u64 = 0; // #326

/// #326: 디코더 상한.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeLimits {
    pub max_pixels: u64,
    pub max_file_size: u64,
}

impl Default for DecodeLimits {
    fn default() -> Self {
        DecodeLimits {
            max_pixels: DEFAULT_MAX_PIXELS,
            max_file_size: DEFAULT_MAX_FILE_SIZE,
        }
    }
}

/// #326: 썸네일 키(전체 경로, 한 변, 수정 시각, 파일 크기).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ThumbnailKey {
    pub path: PathBuf,
    pub side: u32,
    pub modified: SystemTime,
    pub file_size: u64,
}

impl ThumbnailKey {
    /// #326: 파일의 수정 시각·크기를 읽어 키를 만든다.
    pub fn for_file(_path: &Path, _side: u32) -> std::io::Result<ThumbnailKey> {
        todo!("#326")
    }
}

/// #326: 자체 디코더 형식인가(확장자, 대소문자 무시).
pub fn is_decodable(_name: &OsStr) -> bool {
    todo!("#326")
}

/// #326: 기본 상한으로 썸네일을 만든다.
pub fn make_thumbnail(_path: &Path, _side: u32) -> Result<Thumbnail, ThumbnailError> {
    todo!("#326")
}

/// #326: 상한과 취소 표지를 받아 썸네일을 만든다.
pub fn make_thumbnail_with(
    _path: &Path,
    _side: u32,
    _limits: &DecodeLimits,
    _cancel: &AtomicBool,
) -> Result<Thumbnail, ThumbnailError> {
    todo!("#326")
}

/// #330: 항목의 썸네일 계획.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailPlan {
    Decode,
    CacheOnly,
    IconOnly,
}

/// #330: 항목마다 계획을 정한다.
pub fn plan(_item: &Item) -> ThumbnailPlan {
    todo!("#330")
}
