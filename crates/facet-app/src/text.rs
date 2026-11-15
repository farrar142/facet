//! 앱의 Rust 코드가 만드는 화면 글자를 모으는 곳(ADR-0007).
//!
//! 코어는 문장을 만들지 않고 종류와 값을 돌려준다. 그것을 화면 글자로 바꾸는 일은 여기서 한다.
//! 번역 방식이 정해지면 이 모듈을 바꾼다.

use facet_core::command::CommandId;
use facet_core::details::FileType;

/// 명령 이름: 명령 팔레트에 보이고, 팔레트 검색은 이 이름에서 찾는다(코어는 ID만 갖는다).
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "명령 팔레트를 화면에 그리는 티켓(#60)이 부른다. 그때 이 expect를 지운다"
    )
)]
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
    }
}

/// 자세히 보기 유형 열의 글자: 폴더 「파일 폴더」, 파일 「TXT 파일」, 확장자가 없으면 「파일」.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "자세히 보기 열을 화면에 그리는 티켓이 부른다. 그때 이 expect를 지운다"
    )
)]
pub fn file_type_text(file_type: &FileType) -> String {
    match file_type {
        FileType::Folder => "파일 폴더".to_owned(),
        FileType::File {
            extension: Some(extension),
        } => format!("{extension} 파일"),
        FileType::File { extension: None } => "파일".to_owned(),
    }
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
