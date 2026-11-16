//! Slint 키 이벤트의 글자를 코어의 키([`Key`])로 바꾼다. 무엇을 실행할지는 코어가 정한다.
//!
//! Slint는 Enter·화살표·기능 키 같은 글자 없는 키를 유니코드 사용자 영역의 글자 하나로 준다
//! ([`slint::platform::Key`]). 글자 있는 키(영문자, 숫자, 공백, 두벌식 자모)는 [`Key::from_char`]가 바꾼다.
//!
//! 백슬래시 키(Ctrl+\\, 패널 나누기/합치기)는 글자가 아니라 키 위치로 묶는다. 한국어 배열에서는 그 키가 「₩」로
//! 보이고, 독일어 배열에서는 그 자리 키가 「#」을 낸다(백슬래시는 AltGr+ß). Slint의 키 이벤트에는 글자만 있고
//! 키 위치가 없으므로, 창의 winit 이벤트 훅에서 키를 누를 때마다 그 키가 US 배열 백슬래시 자리
//! (`KeyCode::Backslash`, 스캔 코드 0x2B)인지 적어 두고([`note_window_event`]) 바로 뒤에 오는 Slint 키 이벤트에서
//! 쓴다([`shortcut_of`]). winit 이벤트를 못 받는 환경이면 글자로만 본다(「\\」·「₩」·「￦」, [`Key::from_char`]).

use std::cell::Cell;

use facet_core::command::{Key, Shortcut};
use slint::platform::Key as SlintKey;
use slint::winit_030::winit::event::{ElementState, WindowEvent};
use slint::winit_030::winit::keyboard::{KeyCode, PhysicalKey};

thread_local! {
    /// 마지막으로 누른 키가 백슬래시 자리였는가. 키 이벤트를 하나도 받지 못했으면 `None`이다.
    static BACKSLASH_POSITION: Cell<Option<bool>> = const { Cell::new(None) };
}

/// 창의 winit 이벤트 하나. 키를 눌렀으면 그 키가 백슬래시 자리인지 적는다. Slint는 이 이벤트를 받은 바로 뒤에
/// 같은 키의 키 이벤트를 화면에 넘긴다.
pub fn note_window_event(event: &WindowEvent) {
    if let WindowEvent::KeyboardInput { event, .. } = event
        && event.state == ElementState::Pressed
    {
        let backslash = event.physical_key == PhysicalKey::Code(KeyCode::Backslash);
        BACKSLASH_POSITION.with(|slot| slot.set(Some(backslash)));
    }
}

/// 글자 없는 키와 코어 키의 짝.
const SPECIAL: [(SlintKey, Key); 15] = [
    (SlintKey::Return, Key::Enter),
    (SlintKey::Backspace, Key::Backspace),
    (SlintKey::Tab, Key::Tab),
    (SlintKey::Escape, Key::Escape),
    (SlintKey::Delete, Key::Delete),
    (SlintKey::Insert, Key::Insert),
    (SlintKey::LeftArrow, Key::Left),
    (SlintKey::RightArrow, Key::Right),
    (SlintKey::UpArrow, Key::Up),
    (SlintKey::DownArrow, Key::Down),
    (SlintKey::Home, Key::Home),
    (SlintKey::End, Key::End),
    (SlintKey::PageUp, Key::PageUp),
    (SlintKey::PageDown, Key::PageDown),
    (SlintKey::Space, Key::Space),
];

/// 기능 키 F1..F24. Slint에서 이어진 글자다.
const FUNCTION: [SlintKey; 24] = [
    SlintKey::F1,
    SlintKey::F2,
    SlintKey::F3,
    SlintKey::F4,
    SlintKey::F5,
    SlintKey::F6,
    SlintKey::F7,
    SlintKey::F8,
    SlintKey::F9,
    SlintKey::F10,
    SlintKey::F11,
    SlintKey::F12,
    SlintKey::F13,
    SlintKey::F14,
    SlintKey::F15,
    SlintKey::F16,
    SlintKey::F17,
    SlintKey::F18,
    SlintKey::F19,
    SlintKey::F20,
    SlintKey::F21,
    SlintKey::F22,
    SlintKey::F23,
    SlintKey::F24,
];

/// 키 이벤트(글자와 수식 키)를 코어의 키 조합으로. 백슬래시 자리는 훅이 적어 둔 키 위치로 정한다
/// ([`shortcut_at`]).
pub fn shortcut_of(text: &str, ctrl: bool, alt: bool, shift: bool) -> Option<Shortcut> {
    let position = BACKSLASH_POSITION.with(Cell::get);
    shortcut_at(text, ctrl, alt, shift, position)
}

/// 키 이벤트를 코어의 키 조합으로. Shift+Tab은 Slint가 따로 「Backtab」 글자로 주므로 Tab 키에 Shift를 더한
/// 것으로 바꾼다. 코어가 모르는 키면 `None`이다.
///
/// `backslash_position`은 그 키가 백슬래시 자리인가(모르면 `None`). 알면 글자 대신 위치로 정한다: 그 자리면
/// 무슨 글자든(「#」 등) 백슬래시 키이고, 다른 자리 키가 「\\」를 내면 백슬래시 키가 아니다. 모르면 글자로 본다.
pub fn shortcut_at(
    text: &str,
    ctrl: bool,
    alt: bool,
    shift: bool,
    backslash_position: Option<bool>,
) -> Option<Shortcut> {
    let backtab = text.chars().eq([char::from(SlintKey::Backtab)]);
    let key = match (backtab, backslash_position) {
        (true, _) => Key::Tab,
        (false, Some(true)) => Key::Backslash,
        (false, Some(false)) => key_of(text).filter(|key| *key != Key::Backslash)?,
        (false, None) => key_of(text)?,
    };
    Some(Shortcut {
        key,
        ctrl,
        alt,
        shift: shift || backtab,
    })
}

/// 오른쪽 메뉴를 여는 키인가: 메뉴 키(Apps 키)이거나 Shift+F10이다(Ctrl·Alt가 함께 눌렸으면 아니다).
pub fn is_context_menu_key(text: &str, ctrl: bool, alt: bool, shift: bool) -> bool {
    if ctrl || alt {
        return false;
    }
    let mut chars = text.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return false;
    };
    c == char::from(SlintKey::Menu) || (shift && c == char::from(SlintKey::F10))
}

/// 키 이벤트의 글자(`event.text`)를 키로. 글자가 하나가 아니거나 코어가 모르는 키면 `None`이다.
pub fn key_of(text: &str) -> Option<Key> {
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    if let Some((_, key)) = SPECIAL.iter().find(|(k, _)| char::from(*k) == c) {
        return Some(*key);
    }
    if let Some(n) = FUNCTION.iter().position(|k| char::from(*k) == c) {
        return u8::try_from(n + 1).ok().map(Key::Function);
    }
    Key::from_char(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_key_and_shift_f10_open_the_context_menu() {
        let menu = char::from(SlintKey::Menu).to_string();
        let f10 = char::from(SlintKey::F10).to_string();
        assert!(is_context_menu_key(&menu, false, false, false));
        assert!(is_context_menu_key(&f10, false, false, true));
        assert!(!is_context_menu_key(&f10, false, false, false));
        assert!(!is_context_menu_key(&f10, true, false, true));
        assert!(!is_context_menu_key("a", false, false, true));
    }

    use facet_core::command::{CommandId, Shortcut, find_by_shortcut};

    fn text(key: SlintKey) -> String {
        char::from(key).to_string()
    }

    #[test]
    fn 글자_없는_키() {
        assert_eq!(key_of(&text(SlintKey::Return)), Some(Key::Enter));
        assert_eq!(key_of(&text(SlintKey::Backspace)), Some(Key::Backspace));
        assert_eq!(key_of(&text(SlintKey::UpArrow)), Some(Key::Up));
        assert_eq!(key_of(&text(SlintKey::PageDown)), Some(Key::PageDown));
        assert_eq!(key_of(&text(SlintKey::F5)), Some(Key::Function(5)));
        assert_eq!(key_of(&text(SlintKey::F24)), Some(Key::Function(24)));
    }

    #[test]
    fn 글자_있는_키() {
        assert_eq!(key_of("a"), Some(Key::Letter('A')));
        assert_eq!(key_of("H"), Some(Key::Letter('H')));
        assert_eq!(key_of("7"), Some(Key::Digit(7)));
        assert_eq!(key_of(" "), Some(Key::Space));
        // 한글 모드의 H 키.
        assert_eq!(key_of("ㅗ"), Some(Key::Letter('H')));
        assert_eq!(key_of(""), None);
        assert_eq!(key_of("가나"), None);
        assert_eq!(key_of("가"), None);
    }

    #[test]
    fn 키_이벤트에서_명령까지() {
        let shortcut = |text: &str, ctrl, alt, shift| {
            find_by_shortcut(Shortcut {
                key: key_of(text).unwrap(),
                ctrl,
                alt,
                shift,
            })
        };
        assert_eq!(
            shortcut(&text(SlintKey::Return), false, false, false),
            Some(CommandId::Open)
        );
        assert_eq!(
            shortcut(&text(SlintKey::Backspace), false, false, false),
            Some(CommandId::Back)
        );
        assert_eq!(
            shortcut(&text(SlintKey::UpArrow), false, true, false),
            Some(CommandId::Up)
        );
        assert_eq!(
            shortcut("a", true, false, false),
            Some(CommandId::SelectAll)
        );
        assert_eq!(
            shortcut("ㅁ", true, false, false),
            Some(CommandId::SelectAll)
        );
        assert_eq!(
            shortcut(&text(SlintKey::F5), false, false, false),
            Some(CommandId::Refresh)
        );
        assert_eq!(shortcut("a", false, false, false), None);
    }

    #[test]
    fn 복사_붙여넣기_경로_복사_키는_한글_모드에서도_같다() {
        let command = |text: &str, ctrl, shift| {
            shortcut_of(text, ctrl, false, shift).and_then(find_by_shortcut)
        };
        assert_eq!(command("c", true, false), Some(CommandId::Copy));
        assert_eq!(command("ㅊ", true, false), Some(CommandId::Copy));
        assert_eq!(command("v", true, false), Some(CommandId::Paste));
        assert_eq!(command("ㅍ", true, false), Some(CommandId::Paste));
        assert_eq!(command("C", true, true), Some(CommandId::CopyPath));
        assert_eq!(command("ㅊ", true, true), Some(CommandId::CopyPath));
    }

    #[test]
    fn 탭_명령_키() {
        let command = |text: &str, ctrl, shift| {
            shortcut_of(text, ctrl, false, shift).and_then(find_by_shortcut)
        };
        assert_eq!(command("t", true, false), Some(CommandId::NewTab));
        assert_eq!(command("ㅅ", true, false), Some(CommandId::NewTab));
        assert_eq!(command("ㅈ", true, false), Some(CommandId::CloseTab));
        assert_eq!(
            command(&text(SlintKey::Tab), true, false),
            Some(CommandId::NextTab)
        );
        // Shift+Tab은 Tab 글자로 오기도 하고 Backtab 글자로 오기도 한다.
        assert_eq!(
            command(&text(SlintKey::Tab), true, true),
            Some(CommandId::PreviousTab)
        );
        assert_eq!(
            command(&text(SlintKey::Backtab), true, true),
            Some(CommandId::PreviousTab)
        );
        assert_eq!(
            command(&text(SlintKey::Backtab), true, false),
            Some(CommandId::PreviousTab)
        );
        assert_eq!(command("1", true, false), Some(CommandId::GoToTab(1)));
        assert_eq!(command("8", true, false), Some(CommandId::GoToTab(8)));
        assert_eq!(command("9", true, false), Some(CommandId::LastTab));
        // 수식 키 없는 Tab은 활성 패널 바꾸기다.
        assert_eq!(
            command(&text(SlintKey::Tab), false, false),
            Some(CommandId::SwitchPanel)
        );
    }

    #[test]
    fn ctrl_백슬래시는_키_위치로_정한다() {
        let command = |text: &str, ctrl, alt, position| {
            shortcut_at(text, ctrl, alt, false, position).and_then(find_by_shortcut)
        };
        let split = Some(CommandId::TogglePanelSplit);
        // US·한국어 배열: 그 자리 키가 「\」를 낸다(한국어 글꼴에서는 「₩」로 보인다).
        assert_eq!(command("\\", true, false, Some(true)), split);
        // 한국어 IME가 「₩」(U+20A9)나 전각 「￦」를 내도 같은 자리다.
        assert_eq!(command("₩", true, false, Some(true)), split);
        assert_eq!(command("￦", true, false, Some(true)), split);
        // 독일어 배열: 같은 자리 키는 「#」을 낸다.
        assert_eq!(command("#", true, false, Some(true)), split);
        // Ctrl과 함께 오는 제어 글자여도 자리로 정한다.
        assert_eq!(command("\u{1c}", true, false, Some(true)), split);
        // 독일어 배열의 AltGr+ß(「\」)는 다른 자리 키이고 Ctrl+Alt라 패널 명령이 아니다.
        assert_eq!(command("\\", true, true, Some(false)), None);
        // 위치를 알면 다른 자리 키가 낸 「\」는 백슬래시 키가 아니다.
        assert_eq!(command("\\", true, false, Some(false)), None);
        // 위치를 모르면 글자로 본다.
        assert_eq!(command("\\", true, false, None), split);
        assert_eq!(command("₩", true, false, None), split);
        assert_eq!(command("#", true, false, None), None);
        // 수식 키 없는 백슬래시 자리는 명령이 아니다(글자로 이동에 남긴다).
        assert_eq!(command("\\", false, false, Some(true)), None);
        // 다른 키는 위치를 알아도 글자로 정한다.
        assert_eq!(
            command("t", true, false, Some(false)),
            Some(CommandId::NewTab)
        );
        assert_eq!(
            shortcut_at(&text(SlintKey::Backtab), true, false, false, Some(false))
                .and_then(find_by_shortcut),
            Some(CommandId::PreviousTab)
        );
    }
}
