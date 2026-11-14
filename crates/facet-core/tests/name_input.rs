//! 이름 입력란(F2). 코어 공개 인터페이스만 부르고, 임시 폴더의 실제 항목으로 확인한다.

use std::fs;

use facet_core::list_location;
use facet_core::name_input::NameInput;
use tempfile::TempDir;

fn folder_with(names: &[&str]) -> TempDir {
    let dir = TempDir::new().unwrap();
    for name in names {
        fs::write(dir.path().join(name), b"x").unwrap();
    }
    dir
}

#[test]
fn 선택이_없으면_입력란이_열리지_않는다() {
    let dir = folder_with(&["a.txt"]);
    let items = list_location(dir.path()).unwrap();
    assert!(NameInput::open(&items, None).is_none());
}

#[test]
fn 빈_목록이나_범위_밖_선택이면_열리지_않는다() {
    assert!(NameInput::open(&[], Some(0)).is_none());
    let dir = folder_with(&["a.txt"]);
    let items = list_location(dir.path()).unwrap();
    assert!(NameInput::open(&items, Some(1)).is_none());
}

#[test]
fn 고른_항목의_현재_이름으로_채워진다() {
    let dir = folder_with(&["새 폴더 가나다.txt", "😀 이모지.md", "b.txt"]);
    let items = list_location(dir.path()).unwrap();
    for (index, item) in items.iter().enumerate() {
        let input = NameInput::open(&items, Some(index)).unwrap();
        assert_eq!(input.index(), index);
        assert_eq!(input.original(), item.name.to_str().unwrap());
    }
}

#[test]
fn 확정하면_입력값을_돌려주고_파일_시스템은_그대로다() {
    let dir = folder_with(&["원래 이름.txt", "다른 파일.txt"]);
    let before = list_location(dir.path()).unwrap();
    let index = before
        .iter()
        .position(|i| i.name == "원래 이름.txt")
        .unwrap();

    let input = NameInput::open(&before, Some(index)).unwrap();
    let entered = input.confirm("새 폴더 가나다");
    assert_eq!(entered.original, "원래 이름.txt");
    assert_eq!(entered.entered, "새 폴더 가나다");

    let after = list_location(dir.path()).unwrap();
    assert_eq!(before, after);
    assert!(dir.path().join("원래 이름.txt").exists());
    assert!(!dir.path().join("새 폴더 가나다").exists());
}

#[test]
fn 취소해도_파일_시스템은_그대로다() {
    let dir = folder_with(&["원래 이름.txt"]);
    let before = list_location(dir.path()).unwrap();
    let input = NameInput::open(&before, Some(0)).unwrap();
    input.cancel();
    assert_eq!(list_location(dir.path()).unwrap(), before);
}

#[test]
fn 확정한_값을_알리는_문구는_입력값을_담고_이름을_바꾸지_않았다고_말한다() {
    let dir = folder_with(&["원래 이름.txt"]);
    let items = list_location(dir.path()).unwrap();
    let entered = NameInput::open(&items, Some(0))
        .unwrap()
        .confirm("새 폴더 가나다");
    let message = entered.message();
    assert!(message.contains("「새 폴더 가나다」"), "{message}");
    assert!(message.contains("바꾸지 않았습니다"), "{message}");
}
