//! 클립보드: 클립보드를 소유하고 OLE를 부르는 전용 STA 스레드([`Clipboard`])와 형식([`format`](mod@format)).
//!
//! UI 스레드는 클립보드를 직접 열지 않는다. [`Clipboard::start`]가 띄운 스레드 하나가 OLE를 초기화하고(STA), 숨은
//! 메시지 전용 창으로 자기 메시지 루프를 돌며 모든 클립보드 호출을 한다. 메서드는 그 스레드에 요청을 넘기고 답을
//! **기다리므로 UI 스레드에서 부르지 않는다**(코어 일꾼에서 부른다). 다른 프로세스가 클립보드를 잡고 있으면
//! [`OPEN_RETRY_INTERVAL`]마다 다시 열어 보고 [`OPEN_TIMEOUT`]이 지나면 [`ClipboardError::Busy`]다.
//!
//! - 올리기: 파일 목록([`Clipboard::set_files`], `CF_HDROP`와 `Preferred DropEffect`를 함께), 글자
//!   ([`Clipboard::set_text`], `CF_UNICODETEXT`).
//! - 읽기: 파일 목록과 복사·잘라내기([`Clipboard::files`]), 글자([`Clipboard::text`]).
//! - 잘라내기 붙여넣기를 마친 뒤의 셸 규약([`Clipboard::finish_cut_paste`]): 원본 데이터 개체에
//!   `Performed DropEffect`·`Paste Succeeded`(이동)를 알리고 클립보드를 비운다. 그 사이 클립보드가 바뀌었으면
//!   아무것도 하지 않는다.
//! - 비우기([`Clipboard::clear`]).
//! - 변경 알림: 시작할 때 한 번, 그 뒤 클립보드가 바뀔 때마다(`AddClipboardFormatListener`) 요약
//!   (`Option<ClipboardFiles>`)을 콜백으로 넘긴다. 그래서 메뉴를 만들 때 클립보드를 부르지 않아도 된다.
//!
//! 정한 것:
//! - **올리기는 OLE 데이터 개체가 아니라 클립보드에 직접 올린다**(`OpenClipboard` + `SetClipboardData`, 미리 만든
//!   메모리). 탐색기·메일·채팅은 OLE로 읽어도 OLE가 이 형식들을 그대로 감싸 주므로 받을 수 있고, 데이터 개체를
//!   구현하지 않아도 되며 이 스레드가 끝나도 내용이 남는다(지연 렌더링이 없음). 그 대신 facet이 원본일 때 다른
//!   앱이 알리는 `Paste Succeeded`는 받지 않는다(잘라낸 원본은 붙여넣는 쪽이 옮긴다).
//! - **읽기도 클립보드 함수로** 한다. 다른 앱이 OLE로 올린 데이터도 `GetClipboardData`로 읽힌다.
//! - `finish_cut_paste`만 OLE(`OleGetClipboard` → `IDataObject::SetData`)를 쓴다. 원본 앱의 데이터 개체가 알림을
//!   받지 않아도(직접 올린 데이터 등) 실패로 치지 않고 클립보드를 비운다.
//! - 파일 목록이 없거나, 비었거나, 읽을 수 없는 목록(ANSI 목록 등, [`format::HdropError`])이면 「파일 없음」(`None`)이다.
//!   글자·그림·`FileGroupDescriptor`(메일 첨부 등 가상 파일)만 있는 것도 그렇다. `Preferred DropEffect`가 없으면 복사다.
//! - 클립보드 차례 번호(`GetClipboardSequenceNumber`)를 [`ClipboardFiles::sequence`]에 담아, 읽은 뒤 클립보드가
//!   바뀌었는지 알 수 있게 한다.
//!
//! 화면 문장은 없다(ADR-0007). Windows가 아니면 [`Clipboard::start`]가 [`ClipboardError::Unsupported`]다.

pub mod format;

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use format::ClipboardOperation;

/// 다른 프로세스가 클립보드를 잡고 있을 때 다시 열어 보는 시간 제한.
pub const OPEN_TIMEOUT: Duration = Duration::from_secs(1);
/// 다시 열어 보는 간격.
pub const OPEN_RETRY_INTERVAL: Duration = Duration::from_millis(10);

/// 클립보드에 놓인 파일 목록.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardFiles {
    /// 파일·폴더 경로(비어 있지 않음, 올린 차례대로).
    pub paths: Vec<PathBuf>,
    /// 복사인지 잘라내기인지(`Preferred DropEffect`, 없으면 복사).
    pub operation: ClipboardOperation,
    /// 읽을 때의 클립보드 차례 번호. 클립보드가 바뀌면 달라진다([`Clipboard::finish_cut_paste`]에 넘긴다).
    pub sequence: u32,
}

/// 클립보드 호출이 실패한 까닭. 화면 문장은 없다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardError {
    /// 다른 프로세스가 클립보드를 [`OPEN_TIMEOUT`] 넘게 잡고 있다.
    Busy,
    /// 클립보드 스레드가 끝났다(시작하지 못했거나 멈춤).
    Stopped,
    /// Windows가 아니다.
    Unsupported,
    /// 그 밖의 Windows 오류(`HRESULT`).
    Failed {
        /// 원래 오류 코드.
        code: i32,
    },
}

impl fmt::Display for ClipboardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => write!(f, "clipboard is held by another process"),
            Self::Stopped => write!(f, "clipboard thread has stopped"),
            Self::Unsupported => write!(f, "clipboard is not supported on this platform"),
            Self::Failed { code } => write!(f, "clipboard call failed: 0x{:08X}", *code as u32),
        }
    }
}

impl std::error::Error for ClipboardError {}

/// 변경 알림 콜백. 클립보드 스레드에서 불리므로 오래 막지 않는다(값을 넘기고 곧바로 돌아온다).
pub type ChangeCallback = Box<dyn Fn(Option<ClipboardFiles>) + Send + 'static>;

/// 클립보드를 소유하는 전용 STA 스레드. 버리면 스레드를 멈추고 기다린다(올린 내용은 클립보드에 남는다).
///
/// 메서드는 그 스레드의 답을 기다리므로 UI 스레드에서 부르지 않는다. 여러 스레드에서 함께 불러도 된다(`Sync`).
pub struct Clipboard {
    #[cfg(windows)]
    inner: crate::windows_clipboard::Worker,
    #[cfg(not(windows))]
    never: std::convert::Infallible,
}

impl Clipboard {
    /// 스레드를 띄우고 변경 알림을 건다. `on_change`는 시작할 때 한 번, 그 뒤 클립보드가 바뀔 때마다 그 스레드에서
    /// 불린다(파일이 없으면 `None`). 창·알림을 만들지 못하면 오류이고, Windows가 아니면 `Unsupported`.
    pub fn start(
        on_change: impl Fn(Option<ClipboardFiles>) + Send + 'static,
    ) -> Result<Self, ClipboardError> {
        #[cfg(windows)]
        {
            crate::windows_clipboard::Worker::start(Box::new(on_change)).map(|inner| Self { inner })
        }
        #[cfg(not(windows))]
        {
            let _ = on_change;
            Err(ClipboardError::Unsupported)
        }
    }

    /// 파일 목록을 올린다: `CF_HDROP`(유니코드 목록)와 `Preferred DropEffect`(복사·잘라내기). 원래 내용은 지운다.
    pub fn set_files(
        &self,
        paths: &[PathBuf],
        operation: ClipboardOperation,
    ) -> Result<(), ClipboardError> {
        #[cfg(windows)]
        {
            self.inner.set_files(paths, operation)
        }
        #[cfg(not(windows))]
        {
            let _ = (paths, operation);
            match self.never {}
        }
    }

    /// 글자를 올린다(`CF_UNICODETEXT`, 경로 복사용). 원래 내용은 지운다.
    pub fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        #[cfg(windows)]
        {
            self.inner.set_text(text)
        }
        #[cfg(not(windows))]
        {
            let _ = text;
            match self.never {}
        }
    }

    /// 지금 클립보드의 파일 목록과 복사·잘라내기. 파일이 없으면 `None`.
    pub fn files(&self) -> Result<Option<ClipboardFiles>, ClipboardError> {
        #[cfg(windows)]
        {
            self.inner.files()
        }
        #[cfg(not(windows))]
        {
            match self.never {}
        }
    }

    /// 지금 클립보드의 글자(`CF_UNICODETEXT`). 없으면 `None`.
    pub fn text(&self) -> Result<Option<String>, ClipboardError> {
        #[cfg(windows)]
        {
            self.inner.text()
        }
        #[cfg(not(windows))]
        {
            match self.never {}
        }
    }

    /// 잘라낸 파일을 옮겨 붙여넣은 뒤 부른다: 클립보드 차례 번호가 `sequence`(읽은 [`ClipboardFiles::sequence`])와
    /// 같으면 원본 데이터 개체에 `Performed DropEffect`·`Paste Succeeded`(이동)를 알리고 클립보드를 비워 `true`.
    /// 그 사이 클립보드가 바뀌었으면 아무것도 하지 않고 `false`.
    pub fn finish_cut_paste(&self, sequence: u32) -> Result<bool, ClipboardError> {
        #[cfg(windows)]
        {
            self.inner.finish_cut_paste(sequence)
        }
        #[cfg(not(windows))]
        {
            let _ = sequence;
            match self.never {}
        }
    }

    /// 클립보드를 비운다.
    pub fn clear(&self) -> Result<(), ClipboardError> {
        #[cfg(windows)]
        {
            self.inner.clear()
        }
        #[cfg(not(windows))]
        {
            match self.never {}
        }
    }
}

impl fmt::Debug for Clipboard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Clipboard").finish_non_exhaustive()
    }
}
