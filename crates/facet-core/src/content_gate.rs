//! #450: 내용 읽기 관문.

/// #450: 누가 내용을 읽으려 하는가(배경 동작 여섯, 사용자 동작 넷).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reader {
    Thumbnail,
    Preview,
    ContentSearch,
    FolderSize,
    TypeSniff,
    LiveSearchDescend,
    Copy,
    CrossVolumeMove,
    Open,
    PreviewDownload,
}

impl Reader {
    /// #450: 사용자가 명시한 동작인가.
    pub fn is_user_action(self) -> bool {
        todo!("#450")
    }
}

/// #450: 관문의 허락(관문만 만든다).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentPermit {
    reader: Reader,
    downloads: bool,
}

impl ContentPermit {
    /// #450: 읽으면 내려받기가 일어나는가.
    pub fn downloads(&self) -> bool {
        todo!("#450")
    }

    /// #450: 누가 읽는가.
    pub fn reader(&self) -> Reader {
        todo!("#450")
    }
}

/// #450: 관문의 거절 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    OnlineOnly,
    NotExpanded,
}

/// #450: 속성 값과 읽는 쪽으로 허락·거절을 정한다.
pub fn check(_attributes: u32, _reader: Reader) -> Result<ContentPermit, Refusal> {
    todo!("#450")
}
