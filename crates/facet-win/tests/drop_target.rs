//! OLE 드롭 대상 래퍼: 가짜 데이터 개체로 파일 목록·「파일 아님」 판정, 키·허용 효과 전달, 이동 드롭의 셸 규약,
//! 창 등록·해제. Windows 전용.
#![cfg(windows)]

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use facet_win::clipboard::format::hdrop_bytes;
use facet_win::drop_target::testing::{FakeData, HiddenWindow, TestTarget};
use facet_win::drop_target::{
    Allowed, DragData, DragKeys, DragState, DropError, DropHandler, Effect, register,
};

#[derive(Debug, Clone, PartialEq)]
enum Call {
    Enter(Option<Vec<PathBuf>>, DragState),
    Over(DragState),
    Leave,
    Drop(Option<Vec<PathBuf>>, DragState),
}

/// 불린 것을 적고 정해 둔 효과를 돌려주는 처리기.
struct Recorder {
    calls: Rc<RefCell<Vec<Call>>>,
    answer: Effect,
}

fn files(data: &DragData) -> Option<Vec<PathBuf>> {
    data.files.clone()
}

impl DropHandler for Recorder {
    fn enter(&self, data: &DragData, state: &DragState) -> Effect {
        self.calls
            .borrow_mut()
            .push(Call::Enter(files(data), *state));
        self.answer
    }
    fn over(&self, state: &DragState) -> Effect {
        self.calls.borrow_mut().push(Call::Over(*state));
        self.answer
    }
    fn leave(&self) {
        self.calls.borrow_mut().push(Call::Leave);
    }
    fn drop(&self, data: &DragData, state: &DragState) -> Effect {
        self.calls
            .borrow_mut()
            .push(Call::Drop(files(data), *state));
        self.answer
    }
}

fn target(answer: Effect) -> (TestTarget, Rc<RefCell<Vec<Call>>>) {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let handler = Recorder {
        calls: Rc::clone(&calls),
        answer,
    };
    (TestTarget::new(Box::new(handler)).expect("OLE"), calls)
}

const ALL: Allowed = Allowed {
    copy: true,
    mov: true,
};

fn paths() -> Vec<PathBuf> {
    vec![
        PathBuf::from(r"C:\자료\보고서.txt"),
        PathBuf::from(r"C:\자료\사진 🎉"),
    ]
}

#[test]
fn 파일_목록을_꺼내_넘기고_키와_허용_효과와_화면_점을_넘긴다() {
    let (target, calls) = target(Effect::Copy);
    let data = FakeData::hdrop(&hdrop_bytes(&paths())).expect("가짜 데이터");
    let keys = DragKeys {
        ctrl: true,
        shift: false,
        right_button: false,
    };
    let allowed = Allowed {
        copy: true,
        mov: false,
    };
    assert_eq!(target.enter(&data, keys, (-10, 20), allowed), Effect::Copy);
    assert_eq!(target.over(keys, (30, 40), allowed), Effect::Copy);
    let calls = calls.borrow();
    let Call::Enter(Some(found), state) = &calls[0] else {
        panic!("{calls:?}");
    };
    assert_eq!(found, &paths());
    assert_eq!((state.x, state.y), (-10, 20));
    assert_eq!(state.keys, keys);
    assert_eq!(state.allowed, allowed);
    let Call::Over(state) = &calls[1] else {
        panic!("{calls:?}");
    };
    assert_eq!((state.x, state.y), (30, 40));
}

#[test]
fn 글자나_가상_파일은_파일_아님이다() {
    for data in [
        FakeData::text("끌어 온 글자").expect("글자"),
        FakeData::file_group_descriptor().expect("가상 파일"),
    ] {
        let (target, calls) = target(Effect::None);
        assert_eq!(
            target.enter(&data, DragKeys::default(), (0, 0), ALL),
            Effect::None
        );
        assert!(matches!(calls.borrow()[0], Call::Enter(None, _)));
    }
}

#[test]
fn 허용하지_않은_효과는_놓을_수_없음으로_바뀐다() {
    let (target, _) = target(Effect::Move);
    let data = FakeData::hdrop(&hdrop_bytes(&paths())).expect("가짜 데이터");
    let copy_only = Allowed {
        copy: true,
        mov: false,
    };
    assert_eq!(
        target.enter(&data, DragKeys::default(), (0, 0), copy_only),
        Effect::None
    );
}

#[test]
fn 이동_드롭은_최적화된_이동_규약대로_원본_앱에_없음을_돌려준다() {
    let (target, calls) = target(Effect::Move);
    let data = FakeData::hdrop(&hdrop_bytes(&paths())).expect("가짜 데이터");
    target.enter(&data, DragKeys::default(), (0, 0), ALL);
    assert_eq!(
        target.drop(&data, DragKeys::default(), (5, 6), ALL),
        Effect::None
    );
    assert_eq!(data.performed_effect(), Some(0));
    assert_eq!(data.logical_performed_effect(), Some(2));
    let calls = calls.borrow();
    assert!(matches!(&calls[1], Call::Drop(Some(found), _) if found == &paths()));
}

#[test]
fn 복사_드롭은_복사를_돌려주고_결과를_적지_않는다() {
    let (target, _) = target(Effect::Copy);
    let data = FakeData::hdrop(&hdrop_bytes(&paths())).expect("가짜 데이터");
    target.enter(&data, DragKeys::default(), (0, 0), ALL);
    assert_eq!(
        target.drop(&data, DragKeys::default(), (0, 0), ALL),
        Effect::Copy
    );
    assert_eq!(data.performed_effect(), None);
}

#[test]
fn 오른쪽_버튼으로_끈_것은_놓을_때도_오른쪽_버튼이다() {
    let (target, calls) = target(Effect::None);
    let data = FakeData::hdrop(&hdrop_bytes(&paths())).expect("가짜 데이터");
    let right = DragKeys {
        right_button: true,
        ..DragKeys::default()
    };
    target.enter(&data, right, (0, 0), ALL);
    target.drop(&data, right, (0, 0), ALL);
    let calls = calls.borrow();
    assert!(matches!(&calls[1], Call::Drop(_, state) if state.keys.right_button));
    // 다음 끌기는 새로 센다.
    drop(calls);
    target.leave();
}

#[test]
fn 떠나면_처리기에_알린다() {
    let (target, calls) = target(Effect::Copy);
    let data = FakeData::hdrop(&hdrop_bytes(&paths())).expect("가짜 데이터");
    target.enter(&data, DragKeys::default(), (0, 0), ALL);
    target.leave();
    assert_eq!(calls.borrow().last(), Some(&Call::Leave));
}

struct Panics;

impl DropHandler for Panics {
    fn enter(&self, _: &DragData, _: &DragState) -> Effect {
        panic!("처리기 패닉");
    }
    fn over(&self, _: &DragState) -> Effect {
        Effect::Copy
    }
    fn leave(&self) {}
    fn drop(&self, _: &DragData, _: &DragState) -> Effect {
        Effect::Copy
    }
}

#[test]
fn 처리기가_패닉해도_놓을_수_없음으로_답한다() {
    let target = TestTarget::new(Box::new(Panics)).expect("OLE");
    let data = FakeData::hdrop(&hdrop_bytes(&paths())).expect("가짜 데이터");
    assert_eq!(
        target.enter(&data, DragKeys::default(), (0, 0), ALL),
        Effect::None
    );
}

#[test]
fn 창에_한_번만_등록되고_해제하면_다시_등록된다() {
    let window = HiddenWindow::new().expect("숨은 창");
    let first = register(window.handle(), Box::new(Panics)).expect("등록");
    assert_eq!(first.window(), window.handle());
    assert_eq!(
        register(window.handle(), Box::new(Panics)).unwrap_err(),
        DropError::AlreadyRegistered
    );
    drop(first);
    let again = register(window.handle(), Box::new(Panics)).expect("해제 뒤 다시 등록");
    drop(again);
}

#[test]
fn 창이_아니면_등록하지_못한다() {
    let bogus = std::num::NonZeroIsize::new(0x7fff_fff0).expect("값");
    assert_eq!(
        register(bogus, Box::new(Panics)).unwrap_err(),
        DropError::InvalidWindow
    );
}

#[test]
fn 읽을_수_없는_목록과_빈_목록은_파일_아님이다() {
    // ANSI 목록(fWide 0), 빈 목록, 잘린 바이트.
    let mut ansi = hdrop_bytes(&paths());
    ansi[16..20].copy_from_slice(&0u32.to_le_bytes());
    let empty = hdrop_bytes::<PathBuf>(&[]);
    let cut = &hdrop_bytes(&paths())[..10];
    for bytes in [&ansi[..], &empty[..], cut] {
        assert_eq!(DragData::from_hdrop(Some(bytes)).files, None);
        let (target, calls) = target(Effect::Copy);
        let data = FakeData::hdrop(bytes).expect("가짜 데이터");
        target.enter(&data, DragKeys::default(), (0, 0), ALL);
        assert!(matches!(calls.borrow()[0], Call::Enter(None, _)));
    }
    assert_eq!(DragData::from_hdrop(None).files, None);
}
