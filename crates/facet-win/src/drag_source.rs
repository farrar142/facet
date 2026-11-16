//! OLE 끌기 원본: facet 항목을 끌어 facet 창이나 다른 앱(탐색기·바탕 화면·메일 앱)에 놓는다(`DoDragDrop`).
//!
//! 앱은 끌 파일 목록과 허용 효과로 [`DragObject`]를 만들고 [`run`]으로 끌기 루프를 돈다. 루프는 끌기를 시작한 UI
//! 스레드에서 돌고, 놓거나 그만둘 때까지 돌아오지 않는다. 그동안 이 모듈은 파일 시스템을 부르지 않고 이미 만든 파일
//! 목록(`CF_HDROP` 바이트)만 건넨다. facet 창 위에 놓으면 그 창의 드롭 대상([`crate::drop_target`])이 다른 앱에서 온
//! 끌기와 똑같이 받는다(같은 길).
//!
//! ## 데이터 개체
//!
//! 셸의 빈 데이터 개체(`SHCreateDataObject`)에 파일 목록을 클립보드와 같은 형식(`CF_HDROP`,
//! [`crate::clipboard::format::hdrop_bytes`])으로 넣는다. 셸 데이터 개체는 드롭 대상이 적는 형식(드롭 설명,
//! `Performed DropEffect` 등)과 끌기 그림을 그대로 받아 둔다. 허용 효과는 데이터 개체가 아니라 `DoDragDrop`에 넘긴다
//! (`Preferred DropEffect`는 붙여넣기용 힌트라 끌기에는 넣지 않는다: 탐색기가 그것을 기본 효과로 써 같은 볼륨 이동이
//! 복사로 바뀔 수 있다).
//!
//! ## 끌기 그림과 설명
//!
//! 셸의 끌기 그림 도우미(`IDragSourceHelper2`)로 데이터 개체의 항목에서 기본 끌기 그림을 만들고 드롭 설명 글자를
//! 허용한다(`DSH_ALLOWDROPDESCRIPTIONTEXT`). 그래서 드롭 대상이 적은 「〈폴더〉로 이동」 같은 설명이 커서 옆에 보인다.
//! 그림 창은 셸이 이 스레드에 만들고 `DoDragDrop`의 메시지 루프가 그리므로 facet 창이 다시 그려지는지와 상관없다.
//! 도우미를 만들지 못하면 그림과 설명만 안 보인다.
//!
//! ## 이동 결과
//!
//! 놓은 쪽이 이동으로 받으면 놓은 쪽이 옮기는 것으로 본다: 셸 규약의 최적화된 이동(드롭 대상이 직접 옮기고
//! `DROPEFFECT_NONE`을 돌려줌, facet의 드롭 대상과 탐색기가 그렇게 한다)이면 원본에 할 일이 없다. 최적화하지 않은
//! 이동(`DROPEFFECT_MOVE`가 돌아옴)에서도 facet은 **원본을 지우지 않는다**([`DragOutcome::Dropped`]의 효과만
//! 알린다): 받은 쪽이 다 썼는지 facet이 확인할 수 없고, 남은 원본은 사람이 지울 수 있지만 잘못 지운 원본은 되돌릴 수
//! 없기 때문이다.
//!
//! ## 마우스 상태
//!
//! `DoDragDrop`이 마우스를 붙잡고 버튼 놓기 메시지를 먹으므로, 창(winit)은 버튼이 아직 눌린 줄 안다. 그래서 끌기가
//! 끝나면 그 창에 버튼 놓기 메시지(`WM_LBUTTONUP`·`WM_RBUTTONUP`)를 지금 커서 자리로 보내 창이 붙잡기를 풀게 한다.
//!
//! ## 언제 부르나
//!
//! 창 이벤트 처리 안에서 곧바로 `DoDragDrop`을 돌면 winit이 그동안 들어온 자기 창 이벤트를 처리가 끝날 때까지 쌓아
//! 두어 facet 창이 다시 그려지지 않는다. [`run_soon`]은 할 일을 스레드 타이머로 미뤄 winit의 메시지 루프가 그 타이머를
//! 이벤트 처리 밖에서 부르게 한다(`DispatchMessage`가 타이머 함수를 부른다). 그 안에서 [`run`]을 돌면 끄는 동안
//! winit이 창 이벤트를 곧바로 처리해 facet 창(놓일 폴더 행 강조)이 다시 그려진다.
//!
//! Windows가 아니면 모두 [`DragError::Unsupported`]다.

use std::fmt;
use std::num::NonZeroIsize;
use std::path::PathBuf;

use crate::drop_target::{Allowed, Effect};

/// 끌기를 시작한 마우스 버튼.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragButton {
    /// 왼쪽 버튼: 놓으면 놓은 쪽의 기본 효과.
    Left,
    /// 오른쪽 버튼: 놓은 쪽이 고르기 메뉴를 띄운다.
    Right,
}

/// 끌기가 끝난 결과.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragOutcome {
    /// 놓았다. 받은 쪽이 알린 효과(최적화된 이동이나 거절이면 [`Effect::None`]).
    Dropped(Effect),
    /// 놓지 않고 그만뒀다(Esc, 다른 버튼, 놓을 수 없는 곳).
    Cancelled,
}

/// 끌기를 시작하지 못한 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragError {
    /// 끌 파일이 없다.
    NoFiles,
    /// 허용한 효과가 없다.
    NoEffect,
    /// 이 스레드는 OLE(STA)를 쓸 수 없다.
    WrongThread,
    /// Windows가 아님.
    Unsupported,
    /// 그 밖의 실패(원래 코드).
    Other(i32),
}

impl fmt::Display for DragError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DragError::NoFiles => write!(f, "끌 파일 없음"),
            DragError::NoEffect => write!(f, "허용한 효과 없음"),
            DragError::WrongThread => write!(f, "OLE를 쓸 수 없는 스레드"),
            DragError::Unsupported => write!(f, "지원하지 않음"),
            DragError::Other(code) => write!(f, "끌기 실패 0x{code:08X}"),
        }
    }
}

impl std::error::Error for DragError {}

/// 끌 데이터: 파일 목록을 담은 데이터 개체와 허용 효과. 만든 스레드에서만 쓴다(`Send`가 아님).
pub struct DragObject {
    #[cfg(windows)]
    inner: crate::windows_drag_source::Object,
    allowed: Allowed,
    #[cfg(not(windows))]
    _not_send: std::marker::PhantomData<*const ()>,
}

impl fmt::Debug for DragObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DragObject")
            .field("allowed", &self.allowed)
            .finish_non_exhaustive()
    }
}

impl DragObject {
    /// 파일 `files`(비어 있지 않음)와 허용 효과로 데이터 개체를 만든다. 파일 시스템은 부르지 않는다.
    pub fn new(files: &[PathBuf], allowed: Allowed) -> Result<DragObject, DragError> {
        if files.is_empty() {
            return Err(DragError::NoFiles);
        }
        if !allowed.copy && !allowed.mov {
            return Err(DragError::NoEffect);
        }
        #[cfg(windows)]
        {
            Ok(DragObject {
                inner: crate::windows_drag_source::Object::new(files)?,
                allowed,
            })
        }
        #[cfg(not(windows))]
        {
            Err(DragError::Unsupported)
        }
    }

    /// 데이터 개체에서 파일 목록을 다시 읽는다(드롭 대상이 읽는 것과 같은 길). 읽지 못하면 `None`.
    pub fn files(&self) -> Option<Vec<PathBuf>> {
        #[cfg(windows)]
        {
            self.inner.files()
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    /// 허용 효과.
    pub fn allowed(&self) -> Allowed {
        self.allowed
    }

    /// 시험용: 이 데이터 개체를 드롭 대상 시험 도우미가 받는 가짜 데이터로 감싼다(같은 개체).
    #[cfg(windows)]
    pub fn as_test_data(
        &self,
    ) -> Result<crate::drop_target::testing::FakeData, crate::drop_target::DropError> {
        self.inner.as_test_data()
    }
}

/// 창 `window`(끌기를 시작한 창 핸들 값)에서 `object`를 `button`으로 끄는 루프를 돈다. 놓거나 그만둘 때까지 돌아오지
/// 않는다. 그 창을 만든 UI 스레드에서 불러야 한다. 창 이벤트 처리 안이 아니라 [`run_soon`]에서 부르는 것이 좋다.
pub fn run(
    window: NonZeroIsize,
    object: &DragObject,
    button: DragButton,
) -> Result<DragOutcome, DragError> {
    #[cfg(windows)]
    {
        crate::windows_drag_source::run(window, &object.inner, object.allowed, button)
    }
    #[cfg(not(windows))]
    {
        let _ = (window, object, button);
        Err(DragError::Unsupported)
    }
}

/// `task`를 이 스레드의 메시지 루프가 다음에 돌 때(창 이벤트 처리 밖에서) 부른다. 미루지 못하면 곧바로 부른다.
/// Windows가 아니면 곧바로 부른다.
pub fn run_soon(task: Box<dyn FnOnce()>) {
    #[cfg(windows)]
    {
        crate::windows_drag_source::run_soon(task);
    }
    #[cfg(not(windows))]
    {
        task();
    }
}
