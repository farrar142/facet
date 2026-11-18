//! 파일 종류별 시스템 아이콘 얻기(확장자·폴더·파일마다). Windows가 아니면 모두 `None`이다.
//!
//! 셸의 시스템 아이콘 목록(`SHGetFileInfoW`·`SHGetImageList`)에서 아이콘을 찾아 요청 크기의 RGBA로 그린다.
//! 확장자·폴더 아이콘은 파일을 열지 않는다(`SHGFI_USEFILEATTRIBUTES`). 파일 하나의 고유 아이콘([`file_icon`])은 그
//! 파일을 읽으므로 온라인 전용 파일에는 부르지 않는다(코어의 아이콘 키 규칙이 막는다). 부른 스레드에서 셸을 부르고
//! 기다리므로 UI 스레드에서 부르지 않는다. 그 스레드에 COM이 없으면 부르는 동안 STA로 초기화한다.

use std::path::Path;

/// 얻은 아이콘 그림(RGBA8, 곧은 알파, 행 우선, 길이 `width * height * 4`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// 확장자(점 없음)의 아이콘을 `size` 화소로 얻는다. 파일을 읽지 않는다. `size`는 1..=256.
pub fn extension_icon(extension: &str, size: u32) -> Option<IconImage> {
    #[cfg(windows)]
    {
        crate::windows_icon::extension_icon(extension, size)
    }
    #[cfg(not(windows))]
    {
        let _ = (extension, size);
        None
    }
}

/// 폴더 아이콘을 `size` 화소로 얻는다. `size`는 1..=256.
pub fn folder_icon(size: u32) -> Option<IconImage> {
    #[cfg(windows)]
    {
        crate::windows_icon::folder_icon(size)
    }
    #[cfg(not(windows))]
    {
        let _ = size;
        None
    }
}

/// 파일 하나의 고유 아이콘(실행 파일·아이콘 파일·바로 가기의 아이콘)을 `size` 화소로 얻는다. `size`는 1..=256.
pub fn file_icon(path: &Path, size: u32) -> Option<IconImage> {
    #[cfg(windows)]
    {
        crate::windows_icon::file_icon(path, size)
    }
    #[cfg(not(windows))]
    {
        let _ = (path, size);
        None
    }
}
