//! #336: 미리보기(텍스트 읽기·종류 고르기·내용 만들기·요청기·미리보기 영역).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use crate::Item;
use crate::thumbnail::Thumbnail;

/// #336: 텍스트 미리보기가 읽는 앞부분의 한도(256KB).
pub const TEXT_HEAD_LIMIT: usize = 256 * 1024;

/// #336: 추정한 텍스트 인코딩(BOM 유무는 가르지 않음).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    Cp949,
    Windows1252,
}

/// #336: 바이트의 인코딩을 추정한다(`None`이면 텍스트가 아님).
pub fn detect_encoding(_bytes: &[u8]) -> Option<TextEncoding> {
    todo!("#336")
}

/// #336: 바이트를 그 인코딩의 글자로 바꾼다(BOM은 걷고 끝의 반쪽 글자는 버림).
pub fn decode_text(_bytes: &[u8], _encoding: TextEncoding) -> String {
    todo!("#336")
}

/// #336: 텍스트 미리보기(앞부분의 글자, 인코딩, 잘렸는가).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextPreview {
    pub text: String,
    pub encoding: TextEncoding,
    pub truncated: bool,
}

/// #336: 파일 앞부분을 한도까지 읽어 글자로 바꾼다.
pub fn read_text_head(_path: &Path, _limit: usize) -> Result<TextPreview, PreviewProblem> {
    todo!("#336")
}

/// #336: 미리 보지 못한 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewProblem {
    OnlineOnly,
    NotText,
    Missing,
    AccessDenied,
    Locked,
    Corrupt,
    TooLarge,
    Unsupported,
    Failed,
    ShellFailed,
    Io(std::io::ErrorKind),
}

/// #337: 미리보기 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewKind {
    Folder,
    Image,
    Text,
    Shell,
    Info,
    Unavailable(PreviewProblem),
}

/// #337: 항목의 미리보기 종류를 고른다.
pub fn choose_kind(_path: &Path, _item: &Item, _has_handler: &dyn Fn(&str) -> bool) -> PreviewKind {
    todo!("#337")
}

/// #337: 미리보기 내용 요청.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewRequest {
    pub path: PathBuf,
    pub item: Item,
    pub show_hidden: bool,
    pub image_side: u32,
}

/// #337: 도우미 미리보기의 상태.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellPreview {
    Loading,
    Showing,
}

/// #337: 미리보기 본문.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewBody {
    Loading,
    Folder {
        count: Option<u64>,
    },
    Image {
        image: Arc<Thumbnail>,
        placeholder: bool,
    },
    Text(TextPreview),
    Shell(ShellPreview),
    Info,
    Unavailable(PreviewProblem),
}

/// #337: 종류대로 미리보기 내용을 만든다.
pub fn load_preview(
    _request: &PreviewRequest,
    _cancel: &AtomicBool,
    _has_handler: &dyn Fn(&str) -> bool,
) -> PreviewBody {
    todo!("#337")
}

/// #338: 선택이 바뀐 뒤 요청을 내기까지 기다리는 시간(50ms).
pub const PREVIEW_DELAY: Duration = Duration::ZERO; // #338

/// #338: 미리보기 요청 번호.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PreviewId(u64);

/// #338: 끝난 미리보기 요청.
#[derive(Debug, Clone)]
pub struct PreviewDone {
    pub id: PreviewId,
    pub request: PreviewRequest,
    pub body: PreviewBody,
}

/// #338: 미리보기 요청기(기다림·취소·늦은 결과 버림).
pub struct Previewer {
    _private: (),
}

impl Previewer {
    /// #338: 만드는 함수로 요청기를 만든다.
    pub fn new(
        _loader: impl Fn(&PreviewRequest, &AtomicBool) -> PreviewBody + Send + Sync + 'static,
    ) -> Previewer {
        todo!("#338")
    }

    /// #338: 결과가 올 때마다 부를 깨우기.
    pub fn set_waker(&self, _waker: impl Fn() + Send + Sync + 'static) {
        todo!("#338")
    }

    /// #338: 선택이 바뀌었다(`None`은 선택 없음).
    pub fn request(&mut self, _request: Option<PreviewRequest>, _now: Instant) {
        todo!("#338")
    }

    /// #338: 기다리는 요청을 낼 시각.
    pub fn due(&self) -> Option<Instant> {
        todo!("#338")
    }

    /// #338: 기다림이 끝났으면 일꾼에 내고 그 번호.
    pub fn tick(&mut self, _now: Instant) -> Option<PreviewId> {
        todo!("#338")
    }

    /// #338: 마지막으로 낸 요청의 결과.
    pub fn poll(&mut self) -> Vec<PreviewDone> {
        todo!("#338")
    }

    /// #338: 결과 하나를 기다린다.
    pub fn wait_event(&mut self, _timeout: Duration) -> Option<PreviewDone> {
        todo!("#338")
    }
}

/// #339: 미리보기 영역의 기본 폭(논리 픽셀).
pub const PREVIEW_WIDTH_DEFAULT: f32 = 320.0;
/// #339: 미리보기 영역의 가장 좁은 폭.
pub const PREVIEW_WIDTH_MIN: f32 = 160.0;
/// #339: 미리보기 영역의 가장 넓은 폭.
pub const PREVIEW_WIDTH_MAX: f32 = 1600.0;

/// #339: 창의 미리보기 영역(켜짐·폭).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreviewPane {
    pub visible: bool,
    pub width: f32,
}

impl Default for PreviewPane {
    fn default() -> Self {
        PreviewPane {
            visible: false,
            width: PREVIEW_WIDTH_DEFAULT,
        }
    }
}

/// #339: 미리보기 영역의 내용.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewContent {
    Nothing,
    Many {
        count: usize,
        total_size: u64,
    },
    Item {
        path: PathBuf,
        item: Item,
        body: PreviewBody,
    },
}

/// #342: 도우미 미리보기의 대기 시간(3초).
pub const SHELL_PREVIEW_TIMEOUT: Duration = Duration::ZERO; // #342

/// #342: 처리기 창을 붙일 자리(창 핸들 값, 화면 물리 픽셀).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewHost {
    pub parent: u64,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
