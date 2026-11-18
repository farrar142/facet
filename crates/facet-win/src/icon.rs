//! #325: 파일 종류별 시스템 아이콘 얻기(확장자·폴더·파일마다). Windows가 아니면 모두 `None`이다.

use std::path::Path;

/// #325: 얻은 아이콘 그림(RGBA8, 행 우선, 길이 `width * height * 4`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// #325: 확장자(점 없음)의 아이콘을 `size` 화소로 얻는다.
pub fn extension_icon(_extension: &str, _size: u32) -> Option<IconImage> {
    todo!("#325")
}

/// #325: 폴더 아이콘을 `size` 화소로 얻는다.
pub fn folder_icon(_size: u32) -> Option<IconImage> {
    todo!("#325")
}

/// #325: 파일 하나의 고유 아이콘을 `size` 화소로 얻는다.
pub fn file_icon(_path: &Path, _size: u32) -> Option<IconImage> {
    todo!("#325")
}
