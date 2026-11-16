//! 앱의 Rust 코드가 만드는 화면 글자를 모으는 곳(ADR-0007).
//!
//! 코어는 문장을 만들지 않고 종류와 값을 돌려준다. 그것을 화면 글자로 바꾸는 일은 여기서 한다.
//! 번역 방식이 정해지면 이 모듈을 바꾼다.

use facet_core::command::{CommandId, Key, Shortcut};
use facet_core::details::FileType;
use facet_core::edit::EditError;
use facet_core::history::{NotUndoable, OperationKind, Record, RevertSkip};
use facet_core::job::{CopyRejection, Summary};
use facet_core::name_input::{MAX_NAME_UNITS, NameError};
use facet_core::window::{ClipboardDone, ClipboardFailure};
use facet_win::clipboard::ClipboardError;
use facet_win::file_ops::FileOpErrorKind;

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
        CommandId::Undo => "실행취소",
        CommandId::Redo => "다시 실행",
        CommandId::ShowHistory => "작업 기록 보기",
        CommandId::Delete => "삭제",
        CommandId::DeletePermanently => "영구 삭제",
        CommandId::Copy => "복사",
        CommandId::Paste => "붙여넣기",
        CommandId::Cut => "잘라내기",
        CommandId::CopyPath => "경로 복사",
    }
}

/// 새 폴더의 처음 이름(바탕 이름). 있으면 코어가 「새 폴더 (2)」처럼 번호를 붙인다.
pub const NEW_FOLDER_BASE: &str = "새 폴더";

/// 이름 입력란 곁에 보일, 쓸 수 없는 이름의 까닭.
pub fn name_error_text(error: NameError) -> String {
    match error {
        NameError::Empty => "이름을 입력하세요".to_owned(),
        NameError::ForbiddenChar(c) => {
            format!(
                "이름에 쓸 수 없는 글자가 있습니다: {c}  (\\ / : * ? \" < > | 는 쓸 수 없습니다)"
            )
        }
        NameError::ControlChar(c) => {
            format!("이름에 제어 문자를 쓸 수 없습니다: U+{:04X}", u32::from(c))
        }
        NameError::Reserved => {
            "Windows가 예약한 이름입니다(CON, PRN, AUX, NUL, COM1~9, LPT1~9)".to_owned()
        }
        NameError::TrailingDot => "이름이 점(.)으로 끝날 수 없습니다".to_owned(),
        NameError::TrailingSpace => "이름이 공백으로 끝날 수 없습니다".to_owned(),
        NameError::TooLong { units } => {
            format!("이름이 너무 깁니다: {units}자 (최대 {MAX_NAME_UNITS}자)")
        }
    }
}

/// 이름바꾸기·새 폴더가 막히거나 실패한 까닭.
pub fn edit_error_text(error: EditError) -> String {
    match error {
        EditError::Unavailable => "지금 이 위치에서는 할 수 없습니다".to_owned(),
        EditError::Unchanged => "이름이 바뀌지 않았습니다".to_owned(),
        EditError::InvalidName(error) => name_error_text(error),
        EditError::NameTaken => "같은 이름의 항목이 이미 있습니다".to_owned(),
        EditError::File(kind) => match kind {
            FileOpErrorKind::AccessDenied => "권한이 없습니다".to_owned(),
            FileOpErrorKind::SourceMissing => "항목이 사라졌습니다".to_owned(),
            FileOpErrorKind::TargetExists => "같은 이름의 항목이 이미 있습니다".to_owned(),
            FileOpErrorKind::Locked => "다른 프로그램이 쓰고 있습니다".to_owned(),
            FileOpErrorKind::DiskFull => "디스크가 가득 찼습니다".to_owned(),
            FileOpErrorKind::Unsupported => "이 환경에서는 할 수 없습니다".to_owned(),
            FileOpErrorKind::CrossVolume
            | FileOpErrorKind::Cancelled
            | FileOpErrorKind::TimedOut
            | FileOpErrorKind::Other => "이름을 바꾸지 못했습니다".to_owned(),
        },
    }
}

/// 이름 입력란 곁의 까닭.
pub fn rename_problem_text(problem: crate::rename::Problem) -> String {
    match problem {
        crate::rename::Problem::Name(error) => name_error_text(error),
        crate::rename::Problem::Edit(error) => edit_error_text(error),
    }
}

/// 복사·경로 복사를 마쳤을 때(상태 줄). 붙여넣기는 작업 영역이 보이므로 글자가 없다.
pub fn clipboard_done_text(done: &ClipboardDone) -> Option<String> {
    match done {
        ClipboardDone::Copied { items } => Some(format!("항목 {items}개를 복사했습니다")),
        ClipboardDone::Cut { items } => Some(format!("항목 {items}개를 잘라냈습니다")),
        ClipboardDone::PathsCopied { items: 1 } => Some("경로를 복사했습니다".to_owned()),
        ClipboardDone::PathsCopied { items } => Some(format!("경로 {items}개를 복사했습니다")),
        ClipboardDone::Pasted { .. } => None,
    }
}

/// 클립보드 명령이 실패한 까닭(상태 줄).
pub fn clipboard_failure_text(failure: &ClipboardFailure) -> String {
    match failure {
        ClipboardFailure::Clipboard(error) => clipboard_error_text(error),
        ClipboardFailure::NothingToPaste => "클립보드에 붙여넣을 파일이 없습니다".to_owned(),
        ClipboardFailure::Rejected(rejection) => match rejection {
            CopyRejection::NoSources => "붙여넣을 항목이 없습니다".to_owned(),
            CopyRejection::TargetMissing => "붙여넣을 폴더를 찾을 수 없습니다".to_owned(),
            CopyRejection::TargetNotFolder => "붙여넣을 위치가 폴더가 아닙니다".to_owned(),
            CopyRejection::IntoItself(source) | CopyRejection::TargetInsideSource(source) => {
                format!(
                    "폴더를 그 안으로 붙여넣을 수 없습니다: {}",
                    source.display()
                )
            }
        },
    }
}

/// 다른 앱에서 끌어 온 파일을 놓았는데 작업을 시작하기 전에 막힌 까닭(상태 줄).
pub fn drop_rejected_text(rejection: &CopyRejection) -> String {
    match rejection {
        CopyRejection::NoSources => "놓은 항목을 찾을 수 없습니다".to_owned(),
        CopyRejection::TargetMissing => "놓을 폴더를 찾을 수 없습니다".to_owned(),
        CopyRejection::TargetNotFolder => "놓을 위치가 폴더가 아닙니다".to_owned(),
        CopyRejection::IntoItself(source) | CopyRejection::TargetInsideSource(source) => {
            format!("폴더를 그 안으로 놓을 수 없습니다: {}", source.display())
        }
    }
}

/// 클립보드를 쓰지 못한 까닭(상태 줄).
pub fn clipboard_error_text(error: &ClipboardError) -> String {
    match error {
        ClipboardError::Busy => "다른 프로그램이 클립보드를 쓰고 있어 열 수 없습니다".to_owned(),
        ClipboardError::Stopped => {
            "클립보드를 쓸 수 없습니다(클립보드 스레드가 없습니다)".to_owned()
        }
        ClipboardError::Unsupported => "이 환경에서는 클립보드를 쓸 수 없습니다".to_owned(),
        ClipboardError::Failed { code } => {
            format!("클립보드 오류입니다(0x{:08X})", *code as u32)
        }
    }
}

/// 새 폴더를 만들지 못한 까닭(상태 줄).
pub fn new_folder_error_text(error: EditError) -> String {
    format!("새 폴더를 만들지 못했습니다: {}", edit_error_text(error))
}

/// 이름바꾸기가 실패했는데 입력란을 다시 열 수 없을 때(항목이 사라짐, 그 사이 입력란을 닫음, 상태 줄).
pub fn rename_error_text(error: EditError) -> String {
    format!("이름을 바꾸지 못했습니다: {}", edit_error_text(error))
}

/// 기록한 작업의 이름: 상태 줄의 「〈작업 이름〉 실행취소」와 작업 기록 보기의 줄에 쓴다. 항목이 하나면 그
/// 이름(작업 뒤 이름), 여럿이면 수를 넣는다. 복사·이동은 폴더 안의 항목을 세지 않고 맨 위 항목으로 센다.
pub fn operation_name(record: &Record) -> String {
    let steps = record.top_steps();
    let single = match steps.as_slice() {
        [step] => step
            .path()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned()),
        _ => None,
    };
    let n = steps.len();
    match (record.kind(), single) {
        (OperationKind::Rename, Some(name)) => format!("「{name}」 이름 바꾸기"),
        (OperationKind::Rename, None) => format!("항목 {n}개 이름 바꾸기"),
        (OperationKind::NewFolder, Some(name)) => format!("새 폴더 「{name}」 만들기"),
        (OperationKind::NewFolder, None) => format!("새 폴더 {n}개 만들기"),
        (OperationKind::Copy, Some(name)) => format!("「{name}」 복사"),
        (OperationKind::Copy, None) => format!("항목 {n}개 복사"),
        (OperationKind::Move, Some(name)) => format!("「{name}」 이동"),
        (OperationKind::Move, None) => format!("항목 {n}개 이동"),
        (OperationKind::Delete, Some(name)) => format!("「{name}」 영구 삭제"),
        (OperationKind::Delete, None) => format!("항목 {n}개 영구 삭제"),
        (OperationKind::Recycle, Some(name)) => format!("「{name}」 삭제"),
        (OperationKind::Recycle, None) => format!("항목 {n}개 삭제"),
    }
}

/// 실행취소를 시작하기 전에 상태 줄에 보일 글자.
pub fn undo_notice(record: &Record) -> String {
    format!("{} 실행취소", operation_name(record))
}

/// 다시 실행을 시작하기 전에 상태 줄에 보일 글자.
pub fn redo_notice(record: &Record) -> String {
    format!("{} 다시 실행", operation_name(record))
}

/// 되돌릴 수 없는 까닭(작업 기록 보기의 줄).
pub fn not_undoable_text(reason: NotUndoable) -> String {
    match reason {
        NotUndoable::PermanentDelete => "되돌릴 수 없음: 영구 삭제".to_owned(),
        NotUndoable::Overwritten => "되돌릴 수 없음: 덮어씀".to_owned(),
    }
}

/// 마지막 작업이 되돌릴 수 없어 실행취소하지 않았다(상태 줄). 그 앞 작업도 되돌리지 않았다.
pub fn undo_refused_text(record: &Record, reason: NotUndoable) -> String {
    let why = match reason {
        NotUndoable::PermanentDelete => "영구 삭제한 항목은 되살릴 수 없습니다",
        NotUndoable::Overwritten => "덮어쓴 파일은 되살릴 수 없습니다",
    };
    format!("{}: 실행취소할 수 없습니다. {why}", operation_name(record))
}

/// 밖의 변경으로 되돌리지 못한 까닭.
pub fn revert_skip_text(reason: RevertSkip) -> &'static str {
    match reason {
        RevertSkip::Missing => "항목이 없습니다",
        RevertSkip::Changed => "그 뒤 바뀌었습니다",
        RevertSkip::NameTaken => "그 자리에 같은 이름의 항목이 있습니다",
        RevertSkip::NotEmpty => "폴더가 비어 있지 않습니다",
        RevertSkip::NotRecyclable => "휴지통에 넣을 수 없어 지우지 않았습니다",
        RevertSkip::RecycledMissing => "휴지통에 없습니다",
        RevertSkip::PermanentlyDeleted => "영구 삭제해 되살릴 수 없습니다",
    }
}

/// 항목별 오류로 건너뛴 까닭.
pub fn item_error_text(kind: FileOpErrorKind) -> &'static str {
    match kind {
        FileOpErrorKind::AccessDenied => "권한이 없습니다",
        FileOpErrorKind::SourceMissing => "항목이 없습니다",
        FileOpErrorKind::TargetExists => "같은 이름의 항목이 이미 있습니다",
        FileOpErrorKind::Locked => "다른 프로그램이 쓰고 있습니다",
        FileOpErrorKind::DiskFull => "디스크가 가득 찼습니다",
        FileOpErrorKind::Unsupported => "이 환경에서는 할 수 없습니다",
        FileOpErrorKind::CrossVolume
        | FileOpErrorKind::Cancelled
        | FileOpErrorKind::TimedOut
        | FileOpErrorKind::Other => "오류가 났습니다",
    }
}

/// 끝난 작업이 하지 못한 항목들(밖의 변경으로 되돌리지 못함, 항목별 오류로 건너뜀): `경로 (까닭) · 경로 (까닭)`.
/// 없으면 빈 글자. 경로는 번역하지 않는다.
pub fn not_done_text(summary: &Summary) -> String {
    let skipped = summary
        .not_reverted
        .iter()
        .map(|item| (&item.path, revert_skip_text(item.reason)));
    let failed = summary
        .failures
        .iter()
        .map(|item| (&item.source, item_error_text(item.kind)));
    skipped
        .chain(failed)
        .map(|(path, why)| format!("{} ({why})", path.display()))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// 단축키의 글자: `Ctrl+Shift+P`, `Alt+←`. 키 이름은 번역하지 않는다.
/// 오른쪽 메뉴의 하위 메뉴 이름.
pub fn submenu_name(id: facet_core::menu::SubmenuId) -> &'static str {
    match id {
        facet_core::menu::SubmenuId::SortBy => "정렬 기준",
    }
}

/// 오른쪽 메뉴 줄의 표시 글자: 켜진 상태는 「✓」, 지금 정렬은 방향 화살표다. 글자가 아니라 기호라 번역하지 않는다.
pub fn menu_mark_text(mark: facet_core::menu::Mark) -> &'static str {
    use facet_core::details::Direction;
    use facet_core::menu::Mark;
    match mark {
        Mark::None => "",
        Mark::Checked => "✓",
        Mark::Sorted(Direction::Ascending) => "▲",
        Mark::Sorted(Direction::Descending) => "▼",
    }
}

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
    fn 이름_까닭_글자() {
        assert!(name_error_text(NameError::ForbiddenChar('?')).contains(": ?"));
        assert!(name_error_text(NameError::TooLong { units: 300 }).contains("300"));
        assert_eq!(
            edit_error_text(EditError::NameTaken),
            "같은 이름의 항목이 이미 있습니다"
        );
        assert_eq!(
            edit_error_text(EditError::File(FileOpErrorKind::AccessDenied)),
            "권한이 없습니다"
        );
    }

    fn stamp() -> facet_core::history::Stamp {
        facet_core::history::Stamp {
            is_folder: false,
            size: 0,
            modified: std::time::SystemTime::UNIX_EPOCH,
        }
    }

    fn renamed(from: &str, to: &str) -> facet_core::history::Step {
        facet_core::history::Step::Renamed {
            from: std::path::PathBuf::from(from),
            to: std::path::PathBuf::from(to),
            stamp: stamp(),
        }
    }

    #[test]
    fn 작업_이름은_항목이_하나면_이름_여럿이면_수다() {
        let one = Record::new(
            OperationKind::Rename,
            vec![renamed(r"C:\a\옛.txt", r"C:\a\새.txt")],
        );
        assert_eq!(operation_name(&one), "「새.txt」 이름 바꾸기");
        assert_eq!(undo_notice(&one), "「새.txt」 이름 바꾸기 실행취소");
        assert_eq!(redo_notice(&one), "「새.txt」 이름 바꾸기 다시 실행");
        let many = Record::new(
            OperationKind::Rename,
            vec![renamed(r"C:\a\1", r"C:\a\2"), renamed(r"C:\a\3", r"C:\a\4")],
        );
        assert_eq!(operation_name(&many), "항목 2개 이름 바꾸기");
        let folder = Record::new(
            OperationKind::NewFolder,
            vec![facet_core::history::Step::CreatedFolder {
                path: std::path::PathBuf::from(r"C:\a\사진"),
                stamp: stamp(),
            }],
        );
        assert_eq!(operation_name(&folder), "새 폴더 「사진」 만들기");
        // 복사·이동은 폴더 안의 항목을 세지 않고 맨 위 항목으로 센다.
        let copied = |path: &str| facet_core::history::Step::Copied {
            source: std::path::PathBuf::from(r"C:\원본"),
            path: std::path::PathBuf::from(path),
            stamp: stamp(),
        };
        let copy = Record::new(
            OperationKind::Copy,
            vec![
                copied(r"C:\b\사진"),
                copied(r"C:\b\사진\1.jpg"),
                copied(r"C:\b\사진\2.jpg"),
            ],
        );
        assert_eq!(operation_name(&copy), "「사진」 복사");
        let copy = Record::new(
            OperationKind::Copy,
            vec![copied(r"C:\b\1.jpg"), copied(r"C:\b\2.jpg")],
        );
        assert_eq!(operation_name(&copy), "항목 2개 복사");
        let moved = Record::not_undoable(
            OperationKind::Move,
            vec![facet_core::history::Step::Moved {
                from: std::path::PathBuf::from(r"C:\a\옛.txt"),
                to: std::path::PathBuf::from(r"C:\b\옛.txt"),
                stamp: stamp(),
            }],
            NotUndoable::Overwritten,
        );
        assert_eq!(operation_name(&moved), "「옛.txt」 이동");
        let deleted = |path: &str| facet_core::history::Step::Deleted {
            path: std::path::PathBuf::from(path),
        };
        let delete = Record::not_undoable(
            OperationKind::Delete,
            vec![deleted(r"C:\a\폴더")],
            NotUndoable::PermanentDelete,
        );
        assert_eq!(operation_name(&delete), "「폴더」 영구 삭제");
        let deletes = Record::not_undoable(
            OperationKind::Delete,
            vec![deleted(r"C:\a\1.txt"), deleted(r"C:\a\2.txt")],
            NotUndoable::PermanentDelete,
        );
        assert_eq!(operation_name(&deletes), "항목 2개 영구 삭제");
        assert_eq!(
            undo_refused_text(&moved, NotUndoable::Overwritten),
            "「옛.txt」 이동: 실행취소할 수 없습니다. 덮어쓴 파일은 되살릴 수 없습니다"
        );
        assert_eq!(
            undo_refused_text(&one, NotUndoable::PermanentDelete),
            "「새.txt」 이름 바꾸기: 실행취소할 수 없습니다. 영구 삭제한 항목은 되살릴 수 없습니다"
        );
    }

    #[test]
    fn 못_한_항목은_경로와_까닭이다() {
        use facet_core::history::NotReverted;
        use facet_core::job::ItemFailure;
        let mut summary = Summary::default();
        assert_eq!(not_done_text(&summary), "");
        summary.not_reverted.push(NotReverted {
            path: std::path::PathBuf::from(r"C:\a\새.txt"),
            reason: RevertSkip::NameTaken,
        });
        summary.failures.push(ItemFailure {
            source: std::path::PathBuf::from(r"C:\a\b"),
            kind: FileOpErrorKind::Locked,
        });
        assert_eq!(
            not_done_text(&summary),
            r"C:\a\새.txt (그 자리에 같은 이름의 항목이 있습니다) · C:\a\b (다른 프로그램이 쓰고 있습니다)"
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
