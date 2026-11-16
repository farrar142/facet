//! OLE 드롭 대상: 다른 앱(탐색기 등)에서 끌어 온 것을 창 하나가 받는다(`RegisterDragDrop`).
//!
//! 앱은 창 핸들 값과 [`DropHandler`]를 [`register`]에 넘기고, 돌려받은 [`Registration`]을 창이 사는 동안 갖는다.
//! 끄는 동안 OLE가 그 창을 등록한 스레드(UI 스레드)에서 [`DropHandler`]를 부르고, 처리기가 돌려준 효과([`Effect`])가
//! 커서 모양이 된다. 화면 문장은 없다.
//!
//! ## 스레드와 `unsafe` 경계
//!
//! - 드롭 대상은 OLE를 STA로 초기화한 스레드에서만 등록할 수 있다. [`register`]가 그 스레드에서 `OleInitialize`를
//!   부르고(여러 번 불러도 짝만 맞으면 된다) [`Registration`]을 버릴 때 `RevokeDragDrop` 뒤 `OleUninitialize`로
//!   짝을 맞춘다. 스레드가 이미 MTA로 초기화돼 있으면 [`DropError::WrongThread`]다.
//! - OLE는 등록한 스레드에서만 처리기를 부른다(STA의 메시지 펌프 안). 그래서 처리기는 `Send`가 아니어도 되고
//!   [`Registration`]도 `Send`가 아니다(등록·해제가 같은 스레드에서 일어나게).
//! - 처리기가 패닉하면 COM 경계를 넘지 않게 여기서 잡아 「놓을 수 없음」으로 답한다.
//! - 창 핸들 값은 번호일 뿐이다. 없는 창이면 등록이 실패할 뿐 메모리를 건드리지 않는다. 창이 먼저 없어진 뒤
//!   [`Registration`]을 버려도 해제가 실패할 뿐이다.
//! - 들어온 데이터 개체는 이 모듈 밖으로 나가지 않는다. 파일 목록은 `CF_HDROP` 바이트(`DROPFILES`)를 복사해
//!   클립보드와 같은 형식([`crate::clipboard::format::parse_hdrop`], 코어가 `facet_core::clipboard`로 다시 내보내는
//!   것)으로 읽어 경로로 넘긴다. 그 밖의 형식(글자·그림·`FileGroupDescriptor` 같은 가상 파일)이나 읽을 수 없는 목록
//!   (ANSI·깨진 바이트)·빈 목록은 「파일 아님」([`DragData::files`]가 `None`)이다.
//!
//! ## 이동 드롭(셸의 최적화된 이동)
//!
//! 처리기가 놓기에 [`Effect::Move`]를 돌려주면 facet이 이동을 직접 한다는 뜻이다. 셸 규약대로 데이터 개체에
//! `Performed DropEffect`를 `DROPEFFECT_NONE`, `Logical Performed DropEffect`를 `DROPEFFECT_MOVE`로 적고 원본 앱에는
//! `DROPEFFECT_NONE`을 돌려준다. 원본 앱은 그것을 보고 원본을 지우지 않는다(두 번 지우지 않는다). 복사는
//! `DROPEFFECT_COPY`를 그대로 돌려준다.
//!
//! 셸의 끌기 그림 도우미(`IDropTargetHelper`)가 있으면 끄는 동안 탐색기가 만든 끌기 그림을 이 창 위에도 그린다.
//!
//! 처리기가 [`DropHandler::label`]로 설명을 주면 데이터 개체에 셸의 드롭 설명(`DropDescription`)을 적어 끌기 그림
//! 옆에 「〈폴더〉로 이동」 같은 글자가 보이게 한다. 떠나거나 놓으면 지운다(`DROPIMAGE_INVALID`).
//!
//! Windows가 아니면 [`register`]는 [`DropError::Unsupported`]다.

use std::fmt;
use std::num::NonZeroIsize;
use std::path::PathBuf;

/// 끄는 동안 눌린 키와 버튼.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DragKeys {
    /// Ctrl.
    pub ctrl: bool,
    /// Shift.
    pub shift: bool,
    /// 오른쪽 버튼으로 끌었는가. 놓는 순간에는 버튼이 이미 떨어져 있으므로, 이 끌기에서 한 번이라도 오른쪽 버튼이
    /// 눌려 있었으면 놓기에도 참이다.
    pub right_button: bool,
}

/// 원본 앱이 허용한 효과.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Allowed {
    /// 복사(`DROPEFFECT_COPY`).
    pub copy: bool,
    /// 이동(`DROPEFFECT_MOVE`).
    pub mov: bool,
}

/// 끄는 순간의 상태.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DragState {
    /// 마우스 자리: 화면 점(가상 화면 기준 물리 픽셀). 창 기준 점으로 바꾸는 것은 앱이 한다.
    pub x: i32,
    /// 마우스 자리 세로.
    pub y: i32,
    /// 키와 버튼.
    pub keys: DragKeys,
    /// 원본 앱이 허용한 효과.
    pub allowed: Allowed,
}

/// 끌어 온 데이터에서 꺼낸 것.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DragData {
    /// 파일 목록(`CF_HDROP`의 경로들, 비어 있지 않음). 파일 목록이 아니면 `None`(「파일 아님」).
    pub files: Option<Vec<PathBuf>>,
}

impl DragData {
    /// `CF_HDROP` 바이트에서 만든다. 바이트가 없거나 읽을 수 없거나 빈 목록이면 「파일 아님」.
    pub fn from_hdrop(bytes: Option<&[u8]>) -> DragData {
        DragData {
            files: bytes
                .and_then(|bytes| crate::clipboard::format::parse_hdrop(bytes).ok())
                .filter(|files| !files.is_empty()),
        }
    }
}

/// 처리기가 돌려주는 효과(커서 모양과 놓기의 결과).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Effect {
    /// 놓을 수 없음(금지 커서). 놓기에서는 아무 일도 하지 않았음.
    #[default]
    None,
    /// 복사.
    Copy,
    /// 이동. 놓기에서는 facet이 이동을 직접 한다는 뜻이다(셸의 최적화된 이동, 모듈 문서).
    Move,
}

impl Effect {
    /// 원본 앱이 허용하지 않은 효과는 「놓을 수 없음」으로 바꾼다.
    pub fn within(self, allowed: Allowed) -> Effect {
        match self {
            Effect::Copy if allowed.copy => Effect::Copy,
            Effect::Move if allowed.mov => Effect::Move,
            _ => Effect::None,
        }
    }
}

/// 커서 옆에 보일 설명(셸의 드롭 설명 `DROPDESCRIPTION`). 글자는 앱이 준다(ADR-0007).
///
/// 셸은 `message`의 `%1`을 `insert`로 바꿔 보이고 `insert`를 강조한다. 원본 앱이 끌기 그림을 셸 끌기 그림
/// 도우미로 만들고 설명 글자를 허용했을 때만 보인다(facet과 탐색기는 그렇게 한다). 효과(복사·이동·놓을 수 없음)의
/// 아이콘은 처리기가 돌려준 [`Effect`]를 따른다.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DropLabel {
    /// 설명 글자(`%1` 자리에 `insert`가 들어간다). 259글자(UTF-16)를 넘으면 자른다.
    pub message: String,
    /// 강조할 글자(보통 폴더 이름). 259글자를 넘으면 자른다.
    pub insert: String,
}

/// 드롭 대상의 처리기. 창을 등록한 스레드에서만 불린다.
pub trait DropHandler {
    /// 바로 앞 [`DropHandler::enter`]·[`DropHandler::over`]에 맞는 커서 옆 설명. 없으면 셸의 기본 설명이다.
    /// 처리기가 효과를 돌려준 직후 불린다.
    fn label(&self) -> Option<DropLabel> {
        None
    }
    /// 끌기가 창에 들어왔다. 데이터는 이때 한 번 읽어 준다.
    fn enter(&self, data: &DragData, state: &DragState) -> Effect;
    /// 창 위에서 움직이거나 키가 바뀌었다.
    fn over(&self, state: &DragState) -> Effect;
    /// 놓지 않고 창을 떠났다(취소 포함).
    fn leave(&self);
    /// 놓았다. 데이터는 다시 읽어 준다. 처리기는 할 일을 곧바로 맡기고 돌아와야 한다(원본 앱이 기다린다).
    fn drop(&self, data: &DragData, state: &DragState) -> Effect;
}

/// 등록하지 못한 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropError {
    /// 이 스레드는 OLE(STA)를 쓸 수 없다(이미 MTA로 초기화됨).
    WrongThread,
    /// 그 창에는 이미 드롭 대상이 있다.
    AlreadyRegistered,
    /// 창 핸들이 창이 아니다.
    InvalidWindow,
    /// Windows가 아님.
    Unsupported,
    /// 그 밖의 실패(원래 코드).
    Other(i32),
}

impl fmt::Display for DropError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DropError::WrongThread => write!(f, "OLE를 쓸 수 없는 스레드"),
            DropError::AlreadyRegistered => write!(f, "이미 드롭 대상이 있는 창"),
            DropError::InvalidWindow => write!(f, "창이 아님"),
            DropError::Unsupported => write!(f, "지원하지 않음"),
            DropError::Other(code) => write!(f, "드롭 대상 등록 실패 0x{code:08X}"),
        }
    }
}

impl std::error::Error for DropError {}

/// 창 하나의 드롭 대상 등록. 버리면 해제한다(같은 스레드에서, `Send`가 아님).
pub struct Registration {
    #[cfg(windows)]
    inner: crate::windows_drop_target::Registered,
    #[cfg(not(windows))]
    _not_send: std::marker::PhantomData<*const ()>,
}

impl fmt::Debug for Registration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Registration").finish_non_exhaustive()
    }
}

/// 창 `window`(창 핸들 값)를 드롭 대상으로 등록한다. 지금 스레드가 그 창을 만든 UI 스레드여야 한다.
pub fn register(
    window: NonZeroIsize,
    handler: Box<dyn DropHandler>,
) -> Result<Registration, DropError> {
    #[cfg(windows)]
    {
        crate::windows_drop_target::register(window, handler).map(|inner| Registration { inner })
    }
    #[cfg(not(windows))]
    {
        let _ = (window, handler);
        Err(DropError::Unsupported)
    }
}

impl Registration {
    /// 등록한 창 핸들 값.
    pub fn window(&self) -> NonZeroIsize {
        #[cfg(windows)]
        {
            self.inner.window()
        }
        #[cfg(not(windows))]
        {
            unreachable!("Windows가 아니면 등록이 없다")
        }
    }
}

/// 시험용: 등록 없이 드롭 대상을 직접 부르고, 가짜 데이터 개체를 만든다. 앱은 부르지 않는다.
#[cfg(windows)]
pub mod testing {
    pub use crate::windows_drop_target::testing::{FakeData, HiddenWindow, TestTarget};
}
