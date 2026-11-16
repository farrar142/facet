//! OLE 끌기 원본의 Windows 구현. `unsafe`는 이 파일 안에 가둔다(규약은 [`crate::drag_source`] 문서).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::num::NonZeroIsize;
use std::path::PathBuf;

use windows::Win32::Foundation::{
    DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, HWND, LPARAM, POINT, S_OK,
    WPARAM,
};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IDataObject};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_MOVE, DROPEFFECT_NONE, DoDragDrop,
    IDropSource, IDropSource_Impl,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MK_RBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::{
    CLSID_DragDropHelper, DSH_ALLOWDROPDESCRIPTIONTEXT, IDragSourceHelper2, SHCreateDataObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, KillTimer, PostMessageW, SetTimer, WM_LBUTTONUP, WM_RBUTTONUP,
};
use windows::core::{BOOL, HRESULT, implement};

use crate::clipboard::format::hdrop_bytes;
use crate::drag_source::{DragButton, DragError, DragOutcome};
use crate::drop_target::{Allowed, DropError, Effect};
use crate::windows_drop_target::{OleThread, read_data, set_hglobal};

/// 파일 목록을 담은 셸 데이터 개체.
pub(crate) struct Object {
    object: IDataObject,
    _ole: OleThread,
}

fn drop_error(error: DropError) -> DragError {
    match error {
        DropError::WrongThread => DragError::WrongThread,
        DropError::Unsupported => DragError::Unsupported,
        DropError::Other(code) => DragError::Other(code),
        DropError::AlreadyRegistered | DropError::InvalidWindow => DragError::Other(0),
    }
}

impl Object {
    pub(crate) fn new(files: &[PathBuf]) -> Result<Object, DragError> {
        let ole = OleThread::initialize().map_err(drop_error)?;
        // SAFETY: OLE를 초기화한 스레드에서 항목 목록 없는 빈 데이터 개체를 만든다.
        let object: IDataObject = unsafe { SHCreateDataObject(None, None, None) }
            .map_err(|e| DragError::Other(e.code().0))?;
        if !set_hglobal(&object, CF_HDROP.0, &hdrop_bytes(files)) {
            return Err(DragError::Other(0));
        }
        Ok(Object { object, _ole: ole })
    }

    pub(crate) fn files(&self) -> Option<Vec<PathBuf>> {
        read_data(Some(&self.object)).files
    }

    pub(crate) fn as_test_data(&self) -> Result<crate::drop_target::testing::FakeData, DropError> {
        crate::windows_drop_target::testing::FakeData::wrap(self.object.clone())
    }
}

fn allowed_value(allowed: Allowed) -> DROPEFFECT {
    let mut value = DROPEFFECT_NONE.0;
    if allowed.copy {
        value |= DROPEFFECT_COPY.0;
    }
    if allowed.mov {
        value |= DROPEFFECT_MOVE.0;
    }
    DROPEFFECT(value)
}

/// OLE가 부르는 끌기 원본: 끈 버튼이 떨어지면 놓고, Esc나 다른 버튼이면 그만둔다. 커서는 기본 커서다.
#[implement(IDropSource)]
struct Source {
    button: DragButton,
}

impl IDropSource_Impl for Source_Impl {
    fn QueryContinueDrag(&self, fescapepressed: BOOL, grfkeystate: MODIFIERKEYS_FLAGS) -> HRESULT {
        let (held, other) = match self.button {
            DragButton::Left => (MK_LBUTTON.0, MK_RBUTTON.0),
            DragButton::Right => (MK_RBUTTON.0, MK_LBUTTON.0),
        };
        if fescapepressed.as_bool() || grfkeystate.0 & other != 0 {
            DRAGDROP_S_CANCEL
        } else if grfkeystate.0 & held == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _dweffect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

pub(crate) fn run(
    window: NonZeroIsize,
    object: &Object,
    allowed: Allowed,
    button: DragButton,
) -> Result<DragOutcome, DragError> {
    let _ole = OleThread::initialize().map_err(drop_error)?;
    let hwnd = HWND(window.get() as *mut std::ffi::c_void);
    // 끌기 그림과 드롭 설명: 없어도 끌기는 된다.
    // SAFETY: OLE를 초기화한 스레드에서 클래스 ID와 인터페이스만 넘긴다.
    if let Ok(helper) = unsafe {
        CoCreateInstance::<_, IDragSourceHelper2>(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER)
    } {
        // SAFETY: 살아 있는 도우미와 데이터 개체를 넘긴다. 창은 넘기지 않아(창 이벤트 처리로 다시 들어가지 않게)
        // 도우미가 데이터 개체의 항목으로 기본 그림을 만든다. 실패해도 그림만 안 보인다.
        unsafe {
            let _ = helper.SetFlags(DSH_ALLOWDROPDESCRIPTIONTEXT.0 as u32);
            let _ = helper.InitializeFromWindow(None, None, &object.object);
        }
    }
    let source: IDropSource = Source { button }.into();
    let mut effect = DROPEFFECT_NONE;
    // SAFETY: OLE를 초기화한 UI 스레드에서 살아 있는 데이터 개체·원본과 지역 효과 자리를 넘긴다. 돌아올 때까지 이
    // 스레드의 메시지를 OLE가 펌프한다.
    let result =
        unsafe { DoDragDrop(&object.object, &source, allowed_value(allowed), &mut effect) };
    release_button(hwnd, button);
    if result == DRAGDROP_S_DROP {
        let effect = if effect.0 & DROPEFFECT_MOVE.0 != 0 {
            Effect::Move
        } else if effect.0 & DROPEFFECT_COPY.0 != 0 {
            Effect::Copy
        } else {
            Effect::None
        };
        Ok(DragOutcome::Dropped(effect))
    } else if result == DRAGDROP_S_CANCEL {
        Ok(DragOutcome::Cancelled)
    } else {
        Err(DragError::Other(result.0))
    }
}

/// 끌기 루프가 먹은 버튼 놓기를 창에 지금 커서 자리로 보낸다(창이 마우스 붙잡기를 풀게).
fn release_button(hwnd: HWND, button: DragButton) {
    let mut point = POINT::default();
    // SAFETY: 지역 점에 커서 자리를 받고 창 기준으로 바꾼다. 실패하면 (0, 0)으로 보낸다.
    unsafe {
        if GetCursorPos(&mut point).is_ok() {
            let _ = ScreenToClient(hwnd, &mut point);
        }
    }
    let lparam = ((point.y as u32 & 0xFFFF) << 16) | (point.x as u32 & 0xFFFF);
    let message = match button {
        DragButton::Left => WM_LBUTTONUP,
        DragButton::Right => WM_RBUTTONUP,
    };
    // SAFETY: 창 핸들 값과 상수 메시지를 보낸다. 창이 없으면 실패할 뿐이다.
    let _ = unsafe { PostMessageW(Some(hwnd), message, WPARAM(0), LPARAM(lparam as isize)) };
}

thread_local! {
    /// 메시지 루프가 다음에 부를 일들.
    static PENDING: RefCell<VecDeque<Box<dyn FnOnce()>>> = RefCell::new(VecDeque::new());
}

/// 스레드 타이머 함수: 타이머를 끄고 쌓인 일을 차례로 부른다.
unsafe extern "system" fn run_pending(_hwnd: HWND, _message: u32, id: usize, _time: u32) {
    // SAFETY: 이 스레드가 만든 스레드 타이머(창 없음)를 끈다.
    let _ = unsafe { KillTimer(None, id) };
    while let Some(task) = PENDING.with(|pending| pending.borrow_mut().pop_front()) {
        // 패닉이 시스템 콜백 경계를 넘지 않게 잡는다.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task));
    }
}

pub(crate) fn run_soon(task: Box<dyn FnOnce()>) {
    PENDING.with(|pending| pending.borrow_mut().push_back(task));
    // SAFETY: 창 없는 스레드 타이머를 이 스레드에 건다. 함수는 이 파일의 `extern "system"` 함수다.
    let id = unsafe { SetTimer(None, 0, 0, Some(run_pending)) };
    if id == 0 {
        // 타이머를 걸지 못하면 곧바로 부른다.
        while let Some(task) = PENDING.with(|pending| pending.borrow_mut().pop_front()) {
            task();
        }
    }
}
