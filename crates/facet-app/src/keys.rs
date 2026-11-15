//! Slint 키 이벤트의 글자를 코어의 키([`Key`])로 바꾼다. 무엇을 실행할지는 코어가 정한다.
//!
//! Slint는 Enter·화살표·기능 키 같은 글자 없는 키를 유니코드 사용자 영역의 글자 하나로 준다
//! ([`slint::platform::Key`]). 글자 있는 키(영문자, 숫자, 공백, 두벌식 자모)는 [`Key::from_char`]가 바꾼다.

use facet_core::command::Key;
use slint::platform::Key as SlintKey;

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
}
