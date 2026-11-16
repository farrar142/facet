//! OLE 드롭 대상의 Windows 구현. `unsafe`는 이 파일 안에 가둔다(규약은 [`crate::drop_target`] 문서).

use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::num::NonZeroIsize;
use std::panic::{AssertUnwindSafe, catch_unwind};

use windows::Win32::Foundation::{
    DRAGDROP_E_ALREADYREGISTERED, DRAGDROP_E_INVALIDHWND, GlobalFree, HGLOBAL, HWND, POINT, POINTL,
    RPC_E_CHANGED_MODE,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM,
    STGMEDIUM_0, TYMED_HGLOBAL,
};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_MOVE, DROPEFFECT_NONE, IDropTarget,
    IDropTarget_Impl, OleInitialize, OleUninitialize, RegisterDragDrop, ReleaseStgMedium,
    RevokeDragDrop,
};
use windows::Win32::System::SystemServices::{
    MK_CONTROL, MK_RBUTTON, MK_SHIFT, MODIFIERKEYS_FLAGS,
};
use windows::Win32::UI::Shell::{
    CFSTR_DROPDESCRIPTION, CFSTR_LOGICALPERFORMEDDROPEFFECT, CFSTR_PERFORMEDDROPEFFECT,
    CLSID_DragDropHelper, DROPIMAGE_COPY, DROPIMAGE_INVALID, DROPIMAGE_MOVE, DROPIMAGE_NONE,
    IDropTargetHelper,
};
use windows::core::{PCWSTR, Ref, implement};

use crate::drop_target::{
    Allowed, DragData, DragKeys, DragState, DropError, DropHandler, DropLabel, Effect,
};

/// 이 스레드의 OLE 초기화 하나. 버리면 짝을 맞춰 `OleUninitialize`한다. `Send`가 아니다.
pub(crate) struct OleThread {
    _not_send: PhantomData<*const ()>,
}

impl OleThread {
    pub(crate) fn initialize() -> Result<OleThread, DropError> {
        // SAFETY: 예약 인수는 비워 둔다. 성공(S_FALSE 포함)하면 같은 스레드의 Drop이 짝을 맞춘다.
        match unsafe { OleInitialize(None) } {
            Ok(()) => Ok(OleThread {
                _not_send: PhantomData,
            }),
            Err(e) if e.code() == RPC_E_CHANGED_MODE => Err(DropError::WrongThread),
            Err(e) => Err(DropError::Other(e.code().0)),
        }
    }
}

impl Drop for OleThread {
    fn drop(&mut self) {
        // SAFETY: `initialize`가 이 스레드에서 성공한 초기화 하나와 짝이다(`Send`가 아니라 같은 스레드).
        unsafe { OleUninitialize() };
    }
}

/// 등록 하나. 버리면 해제하고 OLE 초기화를 놓는다.
pub(crate) struct Registered {
    hwnd: HWND,
    window: NonZeroIsize,
    // 해제 뒤에 OLE를 놓는다(필드는 선언 순서대로 버려지지만 Drop에서 해제를 먼저 한다).
    _ole: OleThread,
}

impl Registered {
    pub(crate) fn window(&self) -> NonZeroIsize {
        self.window
    }
}

impl Drop for Registered {
    fn drop(&mut self) {
        // SAFETY: 이 스레드에서 등록한 창이다. 창이 이미 없어졌으면 실패를 돌려줄 뿐이다.
        let _ = unsafe { RevokeDragDrop(self.hwnd) };
    }
}

pub(crate) fn register(
    window: NonZeroIsize,
    handler: Box<dyn DropHandler>,
) -> Result<Registered, DropError> {
    let ole = OleThread::initialize()?;
    let hwnd = HWND(window.get() as *mut std::ffi::c_void);
    // 끌기 그림 도우미는 없어도 된다(그림만 안 보인다).
    // SAFETY: OLE를 초기화한 스레드에서 클래스 ID와 인터페이스만 넘긴다.
    let helper: Option<IDropTargetHelper> =
        unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER) }.ok();
    let target: IDropTarget = Target::new(handler, Some(hwnd), helper).into();
    // SAFETY: OLE를 초기화한 스레드이고 `target`은 살아 있는 COM 개체다(OLE가 참조를 하나 더 갖는다). 없는 창이면
    // 실패 코드를 돌려줄 뿐이다.
    match unsafe { RegisterDragDrop(hwnd, &target) } {
        Ok(()) => Ok(Registered {
            hwnd,
            window,
            _ole: ole,
        }),
        Err(e) if e.code() == DRAGDROP_E_ALREADYREGISTERED => Err(DropError::AlreadyRegistered),
        Err(e) if e.code() == DRAGDROP_E_INVALIDHWND => Err(DropError::InvalidWindow),
        Err(e) => Err(DropError::Other(e.code().0)),
    }
}

/// OLE가 부르는 드롭 대상 개체. 처리기를 감싸 키·효과를 바꾸고 데이터를 읽는다.
#[implement(IDropTarget)]
struct Target {
    handler: Box<dyn DropHandler>,
    /// 끌기 그림 도우미에 넘길 창. 시험은 없다.
    hwnd: Option<HWND>,
    helper: Option<IDropTargetHelper>,
    /// 이 끌기에서 오른쪽 버튼이 눌린 적이 있는가(놓을 때는 버튼이 이미 떨어져 있다).
    right_button: Cell<bool>,
    /// 들어온 끌기의 데이터 개체(드롭 설명을 적을 곳). 떠나거나 놓으면 버린다.
    object: RefCell<Option<IDataObject>>,
    /// 마지막으로 적은 드롭 설명(같으면 다시 적지 않는다).
    described: RefCell<Option<(Effect, Option<DropLabel>)>>,
}

impl Target {
    fn new(
        handler: Box<dyn DropHandler>,
        hwnd: Option<HWND>,
        helper: Option<IDropTargetHelper>,
    ) -> Target {
        Target {
            handler,
            hwnd,
            helper,
            right_button: Cell::new(false),
            object: RefCell::new(None),
            described: RefCell::new(None),
        }
    }

    /// 처리기가 준 설명을 들어온 데이터 개체에 드롭 설명으로 적는다(바뀌었을 때만).
    fn describe(&self, effect: Effect) {
        let label = catch_unwind(AssertUnwindSafe(|| self.handler.label())).unwrap_or(None);
        let next = (effect, label);
        if self.described.borrow().as_ref() == Some(&next) {
            return;
        }
        if let Some(object) = self.object.borrow().as_ref() {
            let kind = match (&next.1, effect) {
                (None, _) => DROPIMAGE_INVALID,
                (Some(_), Effect::Copy) => DROPIMAGE_COPY,
                (Some(_), Effect::Move) => DROPIMAGE_MOVE,
                (Some(_), Effect::None) => DROPIMAGE_NONE,
            };
            set_hglobal_named(
                object,
                CFSTR_DROPDESCRIPTION,
                &description_bytes(kind.0, next.1.as_ref()),
            );
        }
        *self.described.borrow_mut() = Some(next);
    }

    /// 끌기가 끝났다: 적은 설명을 지우고 데이터 개체를 버린다.
    fn finish(&self) {
        let object = self.object.borrow_mut().take();
        let described = self.described.borrow_mut().take();
        if let (Some(object), Some(_)) = (object, described) {
            set_hglobal_named(
                &object,
                CFSTR_DROPDESCRIPTION,
                &description_bytes(DROPIMAGE_INVALID.0, None),
            );
        }
    }

    fn state(&self, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, allowed: DROPEFFECT) -> DragState {
        if keys.0 & MK_RBUTTON.0 != 0 {
            self.right_button.set(true);
        }
        DragState {
            x: pt.x,
            y: pt.y,
            keys: DragKeys {
                ctrl: keys.0 & MK_CONTROL.0 != 0,
                shift: keys.0 & MK_SHIFT.0 != 0,
                right_button: self.right_button.get(),
            },
            allowed: Allowed {
                copy: allowed.0 & DROPEFFECT_COPY.0 != 0,
                mov: allowed.0 & DROPEFFECT_MOVE.0 != 0,
            },
        }
    }

    /// 처리기를 부르되 패닉은 COM 경계를 넘지 않게 「놓을 수 없음」으로 바꾼다. 허용하지 않은 효과도 막는다.
    fn ask(&self, allowed: Allowed, f: impl FnOnce(&dyn DropHandler) -> Effect) -> Effect {
        catch_unwind(AssertUnwindSafe(|| f(self.handler.as_ref())))
            .unwrap_or(Effect::None)
            .within(allowed)
    }
}

fn effect_value(effect: Effect) -> DROPEFFECT {
    match effect {
        Effect::None => DROPEFFECT_NONE,
        Effect::Copy => DROPEFFECT_COPY,
        Effect::Move => DROPEFFECT_MOVE,
    }
}

/// `pdweffect`에서 원본이 허용한 효과를 읽는다. 널이면 아무것도 허용하지 않은 것으로 본다.
///
/// # Safety
/// `pdweffect`는 널이거나 OLE가 넘긴 읽을 수 있는 `DROPEFFECT`를 가리킨다.
unsafe fn read_effect(pdweffect: *mut DROPEFFECT) -> DROPEFFECT {
    if pdweffect.is_null() {
        DROPEFFECT_NONE
    } else {
        // SAFETY: 부르는 쪽 약속.
        unsafe { *pdweffect }
    }
}

/// # Safety
/// `pdweffect`는 널이거나 OLE가 넘긴 쓸 수 있는 `DROPEFFECT`를 가리킨다.
unsafe fn write_effect(pdweffect: *mut DROPEFFECT, value: DROPEFFECT) {
    if !pdweffect.is_null() {
        // SAFETY: 부르는 쪽 약속.
        unsafe { *pdweffect = value };
    }
}

fn screen_point(pt: &POINTL) -> POINT {
    POINT { x: pt.x, y: pt.y }
}

impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(
        &self,
        pdataobj: Ref<IDataObject>,
        grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        self.right_button.set(false);
        // SAFETY: OLE가 넘긴 효과 자리다.
        let allowed = unsafe { read_effect(pdweffect) };
        let state = self.state(grfkeystate, pt, allowed);
        let data = read_data(pdataobj.as_ref());
        *self.described.borrow_mut() = None;
        *self.object.borrow_mut() = pdataobj.as_ref().cloned();
        let effect = self.ask(state.allowed, |h| h.enter(&data, &state));
        self.describe(effect);
        let value = effect_value(effect);
        if let (Some(helper), Some(hwnd), Some(object)) =
            (&self.helper, self.hwnd, pdataobj.as_ref())
        {
            // SAFETY: 살아 있는 데이터 개체와 지역 점을 넘긴다. 실패해도 그림만 안 보인다.
            let _ = unsafe { helper.DragEnter(hwnd, object, &screen_point(pt), value) };
        }
        // SAFETY: OLE가 넘긴 효과 자리다.
        unsafe { write_effect(pdweffect, value) };
        Ok(())
    }

    fn DragOver(
        &self,
        grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        // SAFETY: OLE가 넘긴 효과 자리다.
        let allowed = unsafe { read_effect(pdweffect) };
        let state = self.state(grfkeystate, pt, allowed);
        let effect = self.ask(state.allowed, |h| h.over(&state));
        self.describe(effect);
        let value = effect_value(effect);
        if let Some(helper) = &self.helper {
            // SAFETY: 지역 점을 넘긴다.
            let _ = unsafe { helper.DragOver(&screen_point(pt), value) };
        }
        // SAFETY: OLE가 넘긴 효과 자리다.
        unsafe { write_effect(pdweffect, value) };
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        let _ = catch_unwind(AssertUnwindSafe(|| self.handler.leave()));
        self.finish();
        self.right_button.set(false);
        if let Some(helper) = &self.helper {
            // SAFETY: 인수 없음.
            let _ = unsafe { helper.DragLeave() };
        }
        Ok(())
    }

    fn Drop(
        &self,
        pdataobj: Ref<IDataObject>,
        grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        // SAFETY: OLE가 넘긴 효과 자리다.
        let allowed = unsafe { read_effect(pdweffect) };
        let state = self.state(grfkeystate, pt, allowed);
        let data = read_data(pdataobj.as_ref());
        let effect = self.ask(state.allowed, |h| h.drop(&data, &state));
        self.finish();
        self.right_button.set(false);
        // 이동은 facet이 직접 한다(최적화된 이동): 원본 앱이 원본을 지우지 않게 결과를 적고 「없음」을 돌려준다.
        let returned = match effect {
            Effect::Move => {
                if let Some(object) = pdataobj.as_ref() {
                    set_dword(object, CFSTR_PERFORMEDDROPEFFECT, DROPEFFECT_NONE.0);
                    set_dword(object, CFSTR_LOGICALPERFORMEDDROPEFFECT, DROPEFFECT_MOVE.0);
                }
                DROPEFFECT_NONE
            }
            other => effect_value(other),
        };
        if let (Some(helper), Some(object)) = (&self.helper, pdataobj.as_ref()) {
            // SAFETY: 살아 있는 데이터 개체와 지역 점을 넘긴다.
            let _ = unsafe { helper.Drop(object, &screen_point(pt), effect_value(effect)) };
        }
        // SAFETY: OLE가 넘긴 효과 자리다.
        unsafe { write_effect(pdweffect, returned) };
        Ok(())
    }
}

fn hglobal_format(format: u16) -> FORMATETC {
    FORMATETC {
        cfFormat: format,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

/// 데이터 개체에서 파일 목록(`CF_HDROP`) 바이트를 꺼낸다. 없으면 「파일 아님」.
pub(crate) fn read_data(object: Option<&IDataObject>) -> DragData {
    let bytes = object.and_then(|object| read_hglobal(object, CF_HDROP.0));
    DragData::from_hdrop(bytes.as_deref())
}

/// 데이터 개체에서 형식 `format`의 전역 메모리 내용을 복사해 꺼낸다.
fn read_hglobal(object: &IDataObject, format: u16) -> Option<Vec<u8>> {
    let format = hglobal_format(format);
    // SAFETY: 지역 FORMATETC를 넘긴다. 받은 매체는 아래에서 한 번만 놓는다.
    let mut medium = unsafe { object.GetData(&format) }.ok()?;
    let bytes = if medium.tymed == TYMED_HGLOBAL.0 as u32 {
        // SAFETY: tymed가 HGLOBAL이므로 공용체의 그 자리가 유효하다.
        copy_hglobal(unsafe { medium.u.hGlobal })
    } else {
        None
    };
    // SAFETY: GetData가 채운 매체를 한 번 놓는다(pUnkForRelease 규약은 함수가 따른다).
    unsafe { ReleaseStgMedium(&mut medium) };
    bytes
}

/// 전역 메모리 내용을 복사한다.
fn copy_hglobal(global: HGLOBAL) -> Option<Vec<u8>> {
    // SAFETY: 데이터 개체가 넘긴 전역 메모리 핸들이다. 잠근 동안 크기만큼 읽고 곧바로 푼다.
    unsafe {
        let size = GlobalSize(global);
        let pointer = GlobalLock(global) as *const u8;
        if pointer.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(pointer, size).to_vec();
        let _ = GlobalUnlock(global);
        Some(bytes)
    }
}

/// 전역 메모리 하나를 `bytes`로 만든다. 성공하면 넘겨받는 쪽이 놓는다.
fn new_hglobal(bytes: &[u8]) -> Option<HGLOBAL> {
    // SAFETY: 크기만큼 할당하고 잠근 동안 그만큼만 쓴다. 실패하면 놓는다.
    unsafe {
        let global = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).ok()?;
        let pointer = GlobalLock(global) as *mut u8;
        if pointer.is_null() {
            let _ = GlobalFree(Some(global));
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len());
        let _ = GlobalUnlock(global);
        Some(global)
    }
}

/// 데이터 개체에 형식 `format`을 전역 메모리 `bytes`로 넣는다. 데이터 개체가 메모리를 넘겨받는다.
pub(crate) fn set_hglobal(object: &IDataObject, format: u16, bytes: &[u8]) -> bool {
    let Some(global) = new_hglobal(bytes) else {
        return false;
    };
    let format = hglobal_format(format);
    let medium = STGMEDIUM {
        tymed: TYMED_HGLOBAL.0 as u32,
        u: STGMEDIUM_0 { hGlobal: global },
        pUnkForRelease: ManuallyDrop::new(None),
    };
    // SAFETY: 지역 FORMATETC·STGMEDIUM을 넘기고 fRelease=TRUE로 메모리를 넘긴다. 실패하면 넘기지 않았으므로 놓는다.
    match unsafe { object.SetData(&format, &medium, true) } {
        Ok(()) => true,
        Err(_) => {
            // SAFETY: 데이터 개체가 넘겨받지 않은 우리 메모리다.
            let _ = unsafe { GlobalFree(Some(global)) };
            false
        }
    }
}

fn named_format(name: PCWSTR) -> Option<u16> {
    // SAFETY: 상수 글자를 넘긴다.
    let format = unsafe { RegisterClipboardFormatW(name) };
    u16::try_from(format).ok().filter(|&f| f != 0)
}

fn set_dword(object: &IDataObject, name: PCWSTR, value: u32) {
    set_hglobal_named(object, name, &value.to_le_bytes());
}

fn set_hglobal_named(object: &IDataObject, name: PCWSTR, bytes: &[u8]) {
    if let Some(format) = named_format(name) {
        set_hglobal(object, format, bytes);
    }
}

/// `DROPDESCRIPTION`의 글자 칸 하나(UTF-16 260단위, 널로 끝남).
const DESCRIPTION_CHARS: usize = 260;

/// `DROPDESCRIPTION` 구조체 바이트: 종류(`i32`), 설명 글자, 강조 글자. 글자는 259단위에서 자른다(대리 쌍을 가르지
/// 않는다).
fn description_bytes(kind: i32, label: Option<&DropLabel>) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(4 + 4 * DESCRIPTION_CHARS);
    bytes.extend_from_slice(&kind.to_le_bytes());
    for text in [
        label.map_or("", |l| l.message.as_str()),
        label.map_or("", |l| l.insert.as_str()),
    ] {
        let mut units: Vec<u16> = Vec::with_capacity(DESCRIPTION_CHARS);
        for c in text.chars() {
            let mut buf = [0u16; 2];
            let encoded = c.encode_utf16(&mut buf);
            if units.len() + encoded.len() > DESCRIPTION_CHARS - 1 {
                break;
            }
            units.extend_from_slice(encoded);
        }
        units.resize(DESCRIPTION_CHARS, 0);
        bytes.extend(units.iter().flat_map(|u| u.to_le_bytes()));
    }
    bytes
}

/// `DROPDESCRIPTION` 바이트를 읽는다(종류, 설명, 강조). 시험이 쓴다.
fn parse_description(bytes: &[u8]) -> Option<(i32, String, String)> {
    let kind = i32::from_le_bytes(bytes.get(..4)?.try_into().ok()?);
    let text = |start: usize| -> Option<String> {
        let units: Vec<u16> = bytes
            .get(start..start + 2 * DESCRIPTION_CHARS)?
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|u| *u != 0)
            .collect();
        Some(String::from_utf16_lossy(&units))
    };
    Some((kind, text(4)?, text(4 + 2 * DESCRIPTION_CHARS)?))
}

/// 시험 도우미. 앱은 부르지 않는다.
pub mod testing {
    use std::num::NonZeroIsize;

    use windows::Win32::Foundation::{HWND, POINTL};
    use windows::Win32::System::Com::IDataObject;
    use windows::Win32::System::Ole::{
        CF_HDROP, CF_UNICODETEXT, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_MOVE, IDropTarget,
    };
    use windows::Win32::System::SystemServices::{
        MK_CONTROL, MK_LBUTTON, MK_RBUTTON, MK_SHIFT, MODIFIERKEYS_FLAGS,
    };
    use windows::Win32::UI::Shell::{
        CFSTR_DROPDESCRIPTION, CFSTR_FILEDESCRIPTORW, CFSTR_LOGICALPERFORMEDDROPEFFECT,
        CFSTR_PERFORMEDDROPEFFECT, SHCreateDataObject,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WINDOW_STYLE,
    };
    use windows::core::w;

    use super::{OleThread, Target, named_format, read_hglobal, set_hglobal};
    use crate::drop_target::{Allowed, DragKeys, DropError, DropHandler, Effect};

    /// 가짜 데이터 개체(셸의 `SHCreateDataObject`에 형식을 넣은 것).
    pub struct FakeData {
        object: IDataObject,
        _ole: OleThread,
    }

    impl FakeData {
        fn empty() -> Result<FakeData, DropError> {
            let ole = OleThread::initialize()?;
            // SAFETY: 빈 데이터 개체를 만든다(항목 목록 없음).
            let object: IDataObject = unsafe { SHCreateDataObject(None, None, None) }
                .map_err(|e| DropError::Other(e.code().0))?;
            Ok(FakeData { object, _ole: ole })
        }

        fn with(format: u16, bytes: &[u8]) -> Result<FakeData, DropError> {
            let data = FakeData::empty()?;
            if !set_hglobal(&data.object, format, bytes) {
                return Err(DropError::Other(0));
            }
            Ok(data)
        }

        /// 파일 목록(`CF_HDROP` 바이트, `DROPFILES`)을 담은 데이터. 바이트는 코어의 `hdrop_bytes`로 만든다.
        pub fn hdrop(bytes: &[u8]) -> Result<FakeData, DropError> {
            FakeData::with(CF_HDROP.0, bytes)
        }

        /// 글자만 담은 데이터(브라우저에서 끈 글자처럼).
        pub fn text(text: &str) -> Result<FakeData, DropError> {
            let mut bytes: Vec<u8> = text
                .encode_utf16()
                .chain(std::iter::once(0))
                .flat_map(u16::to_le_bytes)
                .collect();
            bytes.extend_from_slice(&[0, 0]);
            FakeData::with(CF_UNICODETEXT.0, &bytes)
        }

        /// 가상 파일(`FileGroupDescriptorW`)만 담은 데이터(브라우저에서 끈 그림처럼). 내용은 비어 있다.
        pub fn file_group_descriptor() -> Result<FakeData, DropError> {
            let format = named_format(CFSTR_FILEDESCRIPTORW).ok_or(DropError::Other(0))?;
            // 항목 수 0인 FILEGROUPDESCRIPTORW 머리.
            FakeData::with(format, &0u32.to_le_bytes())
        }

        /// 드롭 대상이 적은 드롭 설명(종류 값, 설명, 강조). 없으면 `None`. 종류 -1은 지운 것(`DROPIMAGE_INVALID`).
        pub fn drop_description(&self) -> Option<(i32, String, String)> {
            let bytes = read_hglobal(&self.object, named_format(CFSTR_DROPDESCRIPTION)?)?;
            super::parse_description(&bytes)
        }

        /// 이미 있는 데이터 개체(끌기 원본이 만든 것)를 감싼다.
        pub(crate) fn wrap(object: IDataObject) -> Result<FakeData, DropError> {
            Ok(FakeData {
                object,
                _ole: OleThread::initialize()?,
            })
        }

        /// 놓은 뒤 드롭 대상이 적은 `Performed DropEffect`.
        pub fn performed_effect(&self) -> Option<u32> {
            self.dword(CFSTR_PERFORMEDDROPEFFECT)
        }

        /// 놓은 뒤 드롭 대상이 적은 `Logical Performed DropEffect`.
        pub fn logical_performed_effect(&self) -> Option<u32> {
            self.dword(CFSTR_LOGICALPERFORMEDDROPEFFECT)
        }

        fn dword(&self, name: windows::core::PCWSTR) -> Option<u32> {
            let bytes = read_hglobal(&self.object, named_format(name)?)?;
            Some(u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?))
        }
    }

    /// 등록 없이 직접 부르는 드롭 대상(OLE가 부르는 것과 같은 COM 인터페이스로 부른다).
    pub struct TestTarget {
        target: IDropTarget,
        _ole: OleThread,
    }

    fn keys_value(keys: DragKeys) -> MODIFIERKEYS_FLAGS {
        let mut value = if keys.right_button {
            MK_RBUTTON.0
        } else {
            MK_LBUTTON.0
        };
        if keys.ctrl {
            value |= MK_CONTROL.0;
        }
        if keys.shift {
            value |= MK_SHIFT.0;
        }
        MODIFIERKEYS_FLAGS(value)
    }

    fn allowed_value(allowed: Allowed) -> DROPEFFECT {
        let mut value = 0;
        if allowed.copy {
            value |= DROPEFFECT_COPY.0;
        }
        if allowed.mov {
            value |= DROPEFFECT_MOVE.0;
        }
        DROPEFFECT(value)
    }

    fn effect_of(value: DROPEFFECT) -> Effect {
        if value.0 & DROPEFFECT_MOVE.0 != 0 {
            Effect::Move
        } else if value.0 & DROPEFFECT_COPY.0 != 0 {
            Effect::Copy
        } else {
            Effect::None
        }
    }

    impl TestTarget {
        /// 처리기를 감싼 드롭 대상(끌기 그림 도우미 없음).
        pub fn new(handler: Box<dyn DropHandler>) -> Result<TestTarget, DropError> {
            let ole = OleThread::initialize()?;
            let target: IDropTarget = Target::new(handler, None, None).into();
            Ok(TestTarget { target, _ole: ole })
        }

        /// 끌기가 들어옴. 원본 앱에 돌아간 효과.
        pub fn enter(
            &self,
            data: &FakeData,
            keys: DragKeys,
            at: (i32, i32),
            allowed: Allowed,
        ) -> Effect {
            let mut effect = allowed_value(allowed);
            // SAFETY: 살아 있는 데이터 개체와 지역 값을 넘긴다.
            let _ = unsafe {
                self.target.DragEnter(
                    &data.object,
                    keys_value(keys),
                    POINTL { x: at.0, y: at.1 },
                    &mut effect,
                )
            };
            effect_of(effect)
        }

        /// 움직임.
        pub fn over(&self, keys: DragKeys, at: (i32, i32), allowed: Allowed) -> Effect {
            let mut effect = allowed_value(allowed);
            // SAFETY: 지역 값을 넘긴다.
            let _ = unsafe {
                self.target
                    .DragOver(keys_value(keys), POINTL { x: at.0, y: at.1 }, &mut effect)
            };
            effect_of(effect)
        }

        /// 떠남.
        pub fn leave(&self) {
            // SAFETY: 인수 없음.
            let _ = unsafe { self.target.DragLeave() };
        }

        /// 놓기. 놓는 순간에는 마우스 버튼이 떨어져 있으므로 버튼 비트는 넣지 않는다. 원본 앱에 돌아간 효과.
        pub fn drop(
            &self,
            data: &FakeData,
            keys: DragKeys,
            at: (i32, i32),
            allowed: Allowed,
        ) -> Effect {
            let mut effect = allowed_value(allowed);
            let mut flags = keys_value(DragKeys {
                right_button: false,
                ..keys
            });
            flags.0 &= !MK_LBUTTON.0;
            // SAFETY: 살아 있는 데이터 개체와 지역 값을 넘긴다.
            let _ = unsafe {
                self.target.Drop(
                    &data.object,
                    flags,
                    POINTL { x: at.0, y: at.1 },
                    &mut effect,
                )
            };
            effect_of(effect)
        }
    }

    /// 등록 시험용 숨은 창. 버리면 없앤다.
    pub struct HiddenWindow {
        hwnd: HWND,
    }

    impl HiddenWindow {
        /// 보이지 않는 창 하나를 만든다(시스템 클래스 `STATIC`).
        pub fn new() -> Option<HiddenWindow> {
            // SAFETY: 시스템 클래스와 상수 글자로 보이지 않는 최상위 창을 만든다.
            let hwnd = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("STATIC"),
                    w!("facet drop target test"),
                    WINDOW_STYLE(0),
                    0,
                    0,
                    10,
                    10,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .ok()?;
            Some(HiddenWindow { hwnd })
        }

        /// 창 핸들 값.
        pub fn handle(&self) -> NonZeroIsize {
            NonZeroIsize::new(self.hwnd.0 as isize).expect("만든 창 핸들")
        }
    }

    impl Drop for HiddenWindow {
        fn drop(&mut self) {
            // SAFETY: 이 스레드에서 만든 창이다.
            let _ = unsafe { DestroyWindow(self.hwnd) };
        }
    }
}
