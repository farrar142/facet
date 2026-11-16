//! OLE 끌기 원본 래퍼: 데이터 개체를 만들고 다시 읽으면 파일 목록·허용 효과가 같은지, 드롭 대상(같은 길)이 그
//! 데이터 개체에서 같은 파일 목록을 읽는지, 드롭 대상이 처리기의 설명을 드롭 설명으로 적고 끝나면 지우는지. Windows
//! 전용. 실제 끌기 루프(`run`)는 마우스가 필요해 사람이 확인한다(`docs/dragdrop-checklist.md`).
#![cfg(windows)]

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use facet_win::drag_source::{DragError, DragObject};
use facet_win::drop_target::testing::TestTarget;
use facet_win::drop_target::{
    Allowed, DragData, DragKeys, DragState, DropHandler, DropLabel, Effect,
};

fn sample_files() -> Vec<PathBuf> {
    let long = format!(r"C:\{}\끝.txt", "긴폴더이름".repeat(60));
    vec![
        PathBuf::from(r"C:\자료\보고서.txt"),
        PathBuf::from(r"D:\사진\🎉 파티"),
        PathBuf::from(long),
        PathBuf::from(r"\\server\share\a b.txt"),
    ]
}

const BOTH: Allowed = Allowed {
    copy: true,
    mov: true,
};

#[test]
fn 데이터_개체를_다시_읽으면_파일_목록과_허용_효과가_같다() {
    let files = sample_files();
    for allowed in [
        BOTH,
        Allowed {
            copy: true,
            mov: false,
        },
        Allowed {
            copy: false,
            mov: true,
        },
    ] {
        let object = DragObject::new(&files, allowed).expect("데이터 개체");
        assert_eq!(object.files().as_deref(), Some(files.as_slice()));
        assert_eq!(object.allowed(), allowed);
    }
}

#[test]
fn 파일이나_허용_효과가_없으면_만들지_않는다() {
    assert_eq!(DragObject::new(&[], BOTH).unwrap_err(), DragError::NoFiles);
    assert_eq!(
        DragObject::new(&sample_files(), Allowed::default()).unwrap_err(),
        DragError::NoEffect
    );
}

/// 받은 파일을 적고, 정한 효과와 설명을 돌려주는 처리기.
struct Labeled {
    seen: Rc<RefCell<Vec<Option<Vec<PathBuf>>>>>,
    effect: Effect,
    label: Option<DropLabel>,
}

impl DropHandler for Labeled {
    fn label(&self) -> Option<DropLabel> {
        self.label.clone()
    }
    fn enter(&self, data: &DragData, _: &DragState) -> Effect {
        self.seen.borrow_mut().push(data.files.clone());
        self.effect
    }
    fn over(&self, _: &DragState) -> Effect {
        self.effect
    }
    fn leave(&self) {}
    fn drop(&self, data: &DragData, _: &DragState) -> Effect {
        self.seen.borrow_mut().push(data.files.clone());
        self.effect
    }
}

fn labeled(
    effect: Effect,
    label: Option<DropLabel>,
) -> (TestTarget, Rc<RefCell<Vec<Option<Vec<PathBuf>>>>>) {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let target = TestTarget::new(Box::new(Labeled {
        seen: Rc::clone(&seen),
        effect,
        label,
    }))
    .expect("드롭 대상");
    (target, seen)
}

#[test]
fn 드롭_대상이_끌기_원본의_데이터에서_같은_파일_목록을_읽는다() {
    let files = sample_files();
    let object = DragObject::new(&files, BOTH).unwrap();
    let data = object.as_test_data().unwrap();
    let (target, seen) = labeled(Effect::Copy, None);
    let keys = DragKeys::default();
    assert_eq!(
        target.enter(&data, keys, (10, 10), object.allowed()),
        Effect::Copy
    );
    assert_eq!(
        target.drop(&data, keys, (10, 10), object.allowed()),
        Effect::Copy
    );
    assert_eq!(
        *seen.borrow(),
        vec![Some(files.clone()), Some(files.clone())]
    );
}

#[test]
fn 처리기의_설명을_드롭_설명으로_적고_떠나면_지운다() {
    let object = DragObject::new(&sample_files(), BOTH).unwrap();
    let data = object.as_test_data().unwrap();
    let label = DropLabel {
        message: "%1(으)로 이동".to_owned(),
        insert: "사진".to_owned(),
    };
    let (target, _) = labeled(Effect::Move, Some(label));
    let keys = DragKeys::default();
    target.enter(&data, keys, (1, 1), BOTH);
    // DROPIMAGE_MOVE = 2
    assert_eq!(
        data.drop_description(),
        Some((2, "%1(으)로 이동".to_owned(), "사진".to_owned()))
    );
    target.over(keys, (2, 2), BOTH);
    target.leave();
    // DROPIMAGE_INVALID = -1: 셸이 기본 설명으로 돌아간다.
    assert_eq!(
        data.drop_description(),
        Some((-1, String::new(), String::new()))
    );
}

#[test]
fn 놓을_수_없음의_설명과_놓은_뒤_지우기() {
    let object = DragObject::new(&sample_files(), BOTH).unwrap();
    let data = object.as_test_data().unwrap();
    let long_name = "가".repeat(400);
    let (target, _) = labeled(
        Effect::None,
        Some(DropLabel {
            message: "놓을 수 없음".to_owned(),
            insert: long_name,
        }),
    );
    let keys = DragKeys::default();
    target.enter(&data, keys, (1, 1), BOTH);
    let (kind, message, insert) = data.drop_description().unwrap();
    assert_eq!(kind, 0, "DROPIMAGE_NONE");
    assert_eq!(message, "놓을 수 없음");
    assert_eq!(insert.chars().count(), 259, "259단위에서 자른다");
    target.drop(&data, keys, (1, 1), BOTH);
    assert_eq!(data.drop_description().map(|d| d.0), Some(-1));
}

#[test]
fn 설명이_없는_처리기는_드롭_설명을_적지_않는다() {
    let object = DragObject::new(&sample_files(), BOTH).unwrap();
    let data = object.as_test_data().unwrap();
    let (target, _) = labeled(Effect::Copy, None);
    target.enter(&data, DragKeys::default(), (1, 1), BOTH);
    // 처음 설명이 없으면 「기본」(-1)을 적거나 아무것도 적지 않는다. 어느 쪽이든 글자는 없다.
    if let Some((kind, message, insert)) = data.drop_description() {
        assert_eq!((kind, message.as_str(), insert.as_str()), (-1, "", ""));
    }
}
