//! 앱의 Rust 코드가 만드는 화면 글자를 모으는 곳(ADR-0007).
//!
//! 코어는 문장을 만들지 않고 종류와 값을 돌려준다. 그것을 화면 글자로 바꾸는 일은 여기서 한다.
//! 번역 방식이 정해지면 이 모듈을 바꾼다.

use facet_core::command::{CommandId, Key, Shortcut};
use facet_core::details::FileType;

/// 명령 이름: 명령 팔레트에 보이고, 팔레트 검색은 이 이름에서 찾는다(코어는 ID만 갖는다).
pub fn command_name(command: CommandId) -> &'static str {
    match command {
        CommandId::Open => "열기",
        CommandId::Back => "뒤로",
        CommandId::Forward => "앞으로",
        CommandId::Up => "위로",
        CommandId::Refresh => "새로 고침",
        CommandId::SelectAll => "모두 선택",
        CommandId::ToggleCursorSelection => "커서 항목 선택 토글",
        CommandId::EditAddress => "주소창 고쳐 쓰기",
        CommandId::ToggleHidden => "숨김 항목 보이기/감추기",
        CommandId::CommandPalette => "명령 팔레트",
        CommandId::SortByName => "이름으로 정렬",
        CommandId::SortByModified => "수정한 날짜로 정렬",
        CommandId::SortByType => "유형으로 정렬",
        CommandId::SortBySize => "크기로 정렬",
        CommandId::NewTab => "새 탭",
        CommandId::CloseTab => "탭 닫기",
        CommandId::NextTab => "다음 탭",
        CommandId::PreviousTab => "이전 탭",
        CommandId::GoToTab(1) => "첫 번째 탭",
        CommandId::GoToTab(2) => "두 번째 탭",
        CommandId::GoToTab(3) => "세 번째 탭",
        CommandId::GoToTab(4) => "네 번째 탭",
        CommandId::GoToTab(5) => "다섯 번째 탭",
        CommandId::GoToTab(6) => "여섯 번째 탭",
        CommandId::GoToTab(7) => "일곱 번째 탭",
        CommandId::GoToTab(8) => "여덟 번째 탭",
        // 명령 목록에는 1..=8만 있다.
        CommandId::GoToTab(_) => "n번째 탭",
        CommandId::LastTab => "마지막 탭",
        CommandId::OpenInNewTab => "새 탭에서 열기",
        CommandId::ReopenClosedTab => "닫은 탭 되살리기",
        CommandId::TogglePanelSplit => "패널 나누기/합치기",
        CommandId::SwitchPanel => "활성 패널 바꾸기",
        CommandId::NewWindow => "새 창",
        CommandId::CopyToOtherPanel => "반대편 패널로 복사",
        CommandId::MoveToOtherPanel => "반대편 패널로 이동",
        CommandId::CopyTo => "복사 위치…",
        CommandId::MoveTo => "이동 위치…",
        CommandId::Rename => "이름 바꾸기",
        CommandId::NewFolder => "새 폴더",
    }
}

/// 새 폴더의 처음 이름(바탕 이름). 있으면 코어가 「새 폴더 (2)」처럼 번호를 붙인다.
pub const NEW_FOLDER_BASE: &str = "새 폴더";

/// 단축키의 글자: `Ctrl+Shift+P`, `Alt+←`. 키 이름은 번역하지 않는다.
pub fn shortcut_text(shortcut: &Shortcut) -> String {
    let key = match shortcut.key {
        Key::Letter(c) => c.to_string(),
        Key::Digit(d) => d.to_string(),
        Key::Function(n) => format!("F{n}"),
        Key::Enter => "Enter".to_owned(),
        Key::Backspace => "Backspace".to_owned(),
        Key::Space => "Space".to_owned(),
        Key::Tab => "Tab".to_owned(),
        Key::Escape => "Esc".to_owned(),
        Key::Delete => "Delete".to_owned(),
        Key::Insert => "Insert".to_owned(),
        Key::Left => "←".to_owned(),
        Key::Right => "→".to_owned(),
        Key::Up => "↑".to_owned(),
        Key::Down => "↓".to_owned(),
        Key::Home => "Home".to_owned(),
        Key::End => "End".to_owned(),
        Key::PageUp => "PageUp".to_owned(),
        Key::PageDown => "PageDown".to_owned(),
        // 한국어 배열에서는 같은 키가 「₩」로 보인다.
        Key::Backslash => "\\".to_owned(),
    };
    let mut text = String::new();
    for (on, name) in [
        (shortcut.ctrl, "Ctrl+"),
        (shortcut.alt, "Alt+"),
        (shortcut.shift, "Shift+"),
    ] {
        if on {
            text.push_str(name);
        }
    }
    text + &key
}

/// 자세히 보기 유형 열의 글자: 폴더 「파일 폴더」, 파일 「TXT 파일」, 확장자가 없으면 「파일」.
pub fn file_type_text(file_type: &FileType) -> String {
    match file_type {
        FileType::Folder => "파일 폴더".to_owned(),
        FileType::File {
            extension: Some(extension),
        } => format!("{extension} 파일"),
        FileType::File { extension: None } => "파일".to_owned(),
    }
}

/// 진행 표시의 남은 시간 글자: 한 시간 미만은 `분:초`(`2:05`), 그 이상은 `시:분:초`(`1:02:03`). 숫자와 쌍점뿐이라
/// 번역하지 않는다. 1초 미만은 1초로 올린다(끝나기 직전에 0:00으로 멈춘 것처럼 보이지 않게).
pub fn remaining_text(left: std::time::Duration) -> String {
    let seconds = left.as_secs() + u64::from(left.subsec_nanos() > 0);
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// 운영 체제가 준 오류 문장과 코드(HRESULT)를 한 글자로: `메시지 (0x80070005)`.
/// 문장은 운영 체제의 언어로 오므로 번역하지 않는다.
pub fn os_error_detail(message: &str, code: i32) -> String {
    format!("{message} (0x{code:08X})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 명령마다_이름이_있고_겹치지_않는다() {
        let mut names: Vec<_> = CommandId::ALL.into_iter().map(command_name).collect();
        assert!(names.iter().all(|n| !n.is_empty()));
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), CommandId::ALL.len());
    }

    #[test]
    fn 팔레트는_앱의_이름에서_찾는다() {
        let tab = facet_core::tab::Tab::new(
            facet_core::Location::file_system("."),
            Default::default(),
            false,
        );
        let found = facet_core::command::search("숨김", command_name, &tab);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].command, CommandId::ToggleHidden);
    }

    #[test]
    fn 단축키_글자() {
        assert_eq!(
            shortcut_text(&Shortcut::new(Key::Letter('P')).ctrl().shift()),
            "Ctrl+Shift+P"
        );
        assert_eq!(shortcut_text(&Shortcut::new(Key::Left).alt()), "Alt+←");
        assert_eq!(shortcut_text(&Shortcut::new(Key::Function(5))), "F5");
        assert_eq!(
            shortcut_text(&Shortcut::new(Key::Backslash).ctrl()),
            "Ctrl+\\"
        );
    }

    #[test]
    fn 남은_시간_글자() {
        use std::time::Duration;
        assert_eq!(remaining_text(Duration::from_secs(125)), "2:05");
        assert_eq!(remaining_text(Duration::from_secs(3723)), "1:02:03");
        assert_eq!(remaining_text(Duration::from_millis(300)), "0:01");
        assert_eq!(remaining_text(Duration::ZERO), "0:00");
    }

    #[test]
    fn 운영_체제_오류_글자() {
        assert_eq!(
            os_error_detail("액세스가 거부되었습니다.", 0x8007_0005_u32 as i32),
            "액세스가 거부되었습니다. (0x80070005)"
        );
    }

    #[test]
    fn 유형_글자() {
        assert_eq!(file_type_text(&FileType::Folder), "파일 폴더");
        assert_eq!(
            file_type_text(&FileType::File {
                extension: Some("TXT".into())
            }),
            "TXT 파일"
        );
        assert_eq!(file_type_text(&FileType::File { extension: None }), "파일");
    }

    #[test]
    fn 항목에서_유형_글자까지() {
        use facet_core::details::file_type;
        use facet_core::{Attributes, Item};
        let item = |name: &str, is_folder| Item {
            name: name.into(),
            is_folder,
            size: 0,
            modified: std::time::SystemTime::UNIX_EPOCH,
            attributes: Attributes::default(),
        };
        assert_eq!(
            file_type_text(&file_type(&item("메모.txt", false))),
            "TXT 파일"
        );
        assert_eq!(file_type_text(&file_type(&item("README", false))), "파일");
        assert_eq!(
            file_type_text(&file_type(&item("사진.zip", true))),
            "파일 폴더"
        );
    }
}
