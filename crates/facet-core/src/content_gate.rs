//! 내용 읽기 관문(#450, GLOSSARY의 **내용 읽기 관문**).
//!
//! 파일 내용을 읽기 전에 항목의 속성 값(열거 결과 그대로)과 「누가 읽으려 하는가」([`Reader`])로 허락·거절을
//! 정하는 순수 함수 [`check`]다. 배경 동작은 온라인 전용 항목과 펼쳐지지 않은 온라인 폴더를 읽지 않고(읽으면
//! 내려받기가 일어남), 사용자가 명시한 동작은 늘 허락하되 내려받기가 일어나는지 허락 값([`ContentPermit`])이
//! 알린다. 허락 값은 이 모듈만 만들 수 있어, 내용을 여는 함수가 이 값을 인자로 받으면 관문을 잊은 코드는
//! 컴파일되지 않는다. 판정은 비트 연산 몇 개라 항목당 상수 시간이다. 화면 문장 없음(ADR-0007).

/// `FILE_ATTRIBUTE_OFFLINE`.
pub const ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
/// `FILE_ATTRIBUTE_RECALL_ON_OPEN`: 아직 펼쳐지지 않은 온라인 폴더.
pub const ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
/// `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`: 내용이 이 장치에 없는 파일.
pub const ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;

const ONLINE_ONLY: u32 = ATTRIBUTE_OFFLINE | ATTRIBUTE_RECALL_ON_DATA_ACCESS;

/// 누가 내용을 읽으려 하는가(배경 동작 여섯, 사용자 동작 넷).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reader {
    /// 배경: 썸네일 만들기.
    Thumbnail,
    /// 배경: 미리보기 자동 갱신.
    Preview,
    /// 배경: 내용 검색.
    ContentSearch,
    /// 배경: 폴더 크기 계산.
    FolderSize,
    /// 배경: 파일 종류 판정을 위한 앞부분 읽기.
    TypeSniff,
    /// 배경: 실시간 탐색의 하위 폴더 내려가기.
    LiveSearchDescend,
    /// 사용자: 복사.
    Copy,
    /// 사용자: 볼륨을 넘는 이동.
    CrossVolumeMove,
    /// 사용자: 열기.
    Open,
    /// 사용자: 「미리 보기 내려받기」.
    PreviewDownload,
}

impl Reader {
    /// 사용자가 명시한 동작인가.
    pub fn is_user_action(self) -> bool {
        matches!(
            self,
            Reader::Copy | Reader::CrossVolumeMove | Reader::Open | Reader::PreviewDownload
        )
    }
}

/// 관문의 허락. [`check`]만 만든다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentPermit {
    reader: Reader,
    downloads: bool,
}

impl ContentPermit {
    /// 읽으면 내려받기가 일어나는가(사용자 동작이 온라인 항목을 읽을 때만 참).
    pub fn downloads(&self) -> bool {
        self.downloads
    }

    /// 누가 읽는가.
    pub fn reader(&self) -> Reader {
        self.reader
    }
}

/// 관문의 거절 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// 온라인 전용 항목(`RECALL_ON_DATA_ACCESS`·`OFFLINE`).
    OnlineOnly,
    /// 아직 펼쳐지지 않은 온라인 폴더(`RECALL_ON_OPEN`만).
    NotExpanded,
}

/// 속성 값과 읽는 쪽으로 허락·거절을 정한다. 둘 다 있으면 온라인 전용이 앞선다.
pub fn check(attributes: u32, reader: Reader) -> Result<ContentPermit, Refusal> {
    let refusal = if attributes & ONLINE_ONLY != 0 {
        Some(Refusal::OnlineOnly)
    } else if attributes & ATTRIBUTE_RECALL_ON_OPEN != 0 {
        Some(Refusal::NotExpanded)
    } else {
        None
    };
    match refusal {
        Some(refusal) if !reader.is_user_action() => Err(refusal),
        _ => Ok(ContentPermit {
            reader,
            downloads: refusal.is_some(),
        }),
    }
}

/// 목록 항목의 속성으로 관문에 넘길 속성 값: 원래 값(`bits`)에, `online_only` 표시가 있으면
/// `RECALL_ON_DATA_ACCESS`를 더한다(원래 값을 모르는 항목도 온라인 전용으로 거절되게).
pub fn item_bits(attributes: &crate::Attributes) -> u32 {
    if attributes.online_only {
        attributes.bits | ATTRIBUTE_RECALL_ON_DATA_ACCESS
    } else {
        attributes.bits
    }
}

/// 목록 항목을 관문에 넘긴다([`item_bits`]와 [`check`]).
pub fn check_item(
    attributes: &crate::Attributes,
    reader: Reader,
) -> Result<ContentPermit, Refusal> {
    check(item_bits(attributes), reader)
}
