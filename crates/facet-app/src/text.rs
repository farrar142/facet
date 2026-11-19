//! 앱의 Rust 코드가 만드는 화면 글자를 모으는 곳(ADR-0007).
//!
//! 코어는 문장을 만들지 않고 종류와 값을 돌려준다. 그것을 화면 글자로 바꾸는 일은 여기서 한다.
//! 번역 방식이 정해지면 이 모듈을 바꾼다.

use facet_core::command::{CommandId, Key, Shortcut};
use facet_core::content_search::Skipped;
use facet_core::details::FileType;
use facet_core::dragdrop::{DropEffect, Refusal};
use facet_core::edit::EditError;
use facet_core::history::{NotUndoable, OperationKind, Record, RevertSkip};
use facet_core::job::{CopyRejection, Summary};
use facet_core::name_input::{MAX_NAME_UNITS, NameError};
use facet_core::view::GridSize;
use facet_core::window::{ClipboardDone, ClipboardFailure};
use facet_win::clipboard::ClipboardError;
use facet_win::drag_source::DragError;
use facet_win::drop_target::DropLabel;
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
        CommandId::BatchRename => "일괄 이름 바꾸기",
        CommandId::NewFolder => "새 폴더",
        CommandId::Undo => "실행취소",
        CommandId::Redo => "다시 실행",
        CommandId::ShowHistory => "작업 기록 보기",
        CommandId::Filter => "필터",
        CommandId::Search => "검색",
        CommandId::ContentSearch => "내용 검색",
        CommandId::Delete => "삭제",
        CommandId::DeletePermanently => "영구 삭제",
        CommandId::Copy => "복사",
        CommandId::Paste => "붙여넣기",
        CommandId::Cut => "잘라내기",
        CommandId::CopyPath => "경로 복사",
        CommandId::SortByLocation => "위치로 정렬",
        CommandId::SortByMatches => "일치로 정렬",
        CommandId::SortByTotalSize => "전체 크기로 정렬",
        CommandId::SortByFreeSpace => "남은 공간으로 정렬",
        CommandId::ShowInFolder => "폴더에서 보기",
        CommandId::StopSearch => "검색 중지",
        CommandId::InstantUnderFolder => "현재 폴더 아래만",
        CommandId::DetailsView => "자세히 보기",
        CommandId::GridView(GridSize::Small) => "그리드 보기: 작게",
        CommandId::GridView(GridSize::Medium) => "그리드 보기: 보통",
        CommandId::GridView(GridSize::Large) => "그리드 보기: 크게",
        CommandId::GridView(GridSize::ExtraLarge) => "그리드 보기: 아주 크게",
        CommandId::ClearThumbnailCache => "썸네일 캐시 비우기",
        CommandId::OpenWith => "연결 프로그램",
        CommandId::Properties => "속성",
        CommandId::OpenThisPc => "내 PC 열기",
        CommandId::OpenRecycleBin => "휴지통 열기",
        CommandId::Restore => "복원",
        CommandId::EmptyRecycleBin => "휴지통 비우기",
        CommandId::TogglePreview => "미리보기 영역",
        CommandId::KeepOnDevice | CommandId::FreeUpSpace => todo!("#455"),
        CommandId::DownloadForPreview => todo!("#456"),
        CommandId::ExtractAllHere | CommandId::ExtractToFolder | CommandId::ExtractTo => {
            todo!("#427")
        }
        CommandId::NewZip => todo!("#429"),
        CommandId::AddToZip => todo!("#430"),
        CommandId::CheckForUpdates => todo!("#475"),
        CommandId::About => todo!("#459"),
        CommandId::OpenSettings | CommandId::OpenSettingsFile => todo!("#462"),
    }
}

/// 가상 폴더(내 PC 등)의 목록을 흘려 받는 동안 목록 위에 보일 글자: 지금까지 읽은 항목 수.
pub fn loading_text(count: usize) -> String {
    format!("불러오는 중 · {count}개")
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

/// 일괄 이름 바꾸기 미리 보기의 문제 줄에 보일 까닭.
pub fn batch_problem_text(problem: facet_core::batch_rename::BatchProblem) -> String {
    use facet_core::batch_rename::BatchProblem;
    match problem {
        BatchProblem::Invalid(error) => name_error_text(error),
        BatchProblem::Duplicate => "겹침: 다른 항목과 바뀔 이름이 같습니다".to_owned(),
        BatchProblem::NameTaken => {
            "같은 이름이 있음: 폴더에 그 이름의 항목이 남아 있습니다".to_owned()
        }
        BatchProblem::NotUnicode => "읽을 수 없는 이름입니다".to_owned(),
    }
}

/// 일괄 이름 바꾸기 창 아래의 수치 줄.
pub fn batch_counts_text(renames: usize, unchanged: usize, problems: usize) -> String {
    format!("바꿈 {renames} · 건너뜀 {unchanged} · 문제 {problems}")
}

/// 「문제 행만 보기」를 켰는데 문제 행이 없을 때의 안내.
pub fn batch_no_problems_text() -> String {
    "문제 없음".to_owned()
}

/// 열 수 없는 규칙 묶음 줄의 까닭.
pub const PRESET_UNOPENABLE: &str = "이 창에서 열 수 없음";

/// 규칙 묶음 조작이 거부된 까닭(창에 보임).
pub fn preset_error_text(error: facet_core::rename_presets::PresetError) -> String {
    use facet_core::rename_presets::PresetError;
    match error {
        PresetError::EmptyName => "묶음 이름을 입력하세요".to_owned(),
        PresetError::NameTaken => "같은 이름의 묶음이 이미 있습니다".to_owned(),
        PresetError::NotFound => "목록에서 묶음을 먼저 고르세요".to_owned(),
    }
}

/// 묶음 파일을 저장하지 못했다는 까닭(창에 보임).
pub fn preset_save_failed_text(error: &str) -> String {
    format!("규칙 묶음을 저장하지 못했습니다: {error}")
}

/// 묶음 파일을 읽을 때 생긴 일(건너뛴 묶음 수, 파일이 깨져 옆에 남긴 파일 이름). 알릴 것이 없으면 빈 글자.
pub fn preset_notice_text(skipped: usize, broken: Option<&Option<String>>) -> String {
    let mut parts = Vec::new();
    match broken {
        Some(Some(kept)) => {
            parts.push(format!("묶음 파일이 깨져 옆에 남겼습니다: {kept}"));
        }
        Some(None) => parts.push("묶음 파일이 깨져 읽지 못했습니다".to_owned()),
        None => {}
    }
    if skipped > 0 {
        parts.push(format!("읽지 못해 건너뛴 묶음 {skipped}개"));
    }
    parts.join(" · ")
}

/// 「바꾸기」가 막힌 까닭(창 아래에 보임).
pub fn batch_error_text(error: facet_core::window::BatchRenameError) -> String {
    match error {
        facet_core::window::BatchRenameError::Unavailable => {
            "지금은 일괄 이름 바꾸기를 할 수 없습니다".to_owned()
        }
        facet_core::window::BatchRenameError::Problems => {
            "문제가 있는 이름을 먼저 고치세요".to_owned()
        }
        facet_core::window::BatchRenameError::NothingToRename => {
            "바뀌는 이름이 없습니다".to_owned()
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
            | FileOpErrorKind::Other
            | FileOpErrorKind::Corrupt => "이름을 바꾸지 못했습니다".to_owned(),
            FileOpErrorKind::SyncProviderNotRunning => todo!("#453"),
        },
    }
}

/// 이름 입력란 곁의 까닭.
pub fn rename_problem_text(problem: crate::rename::Problem) -> String {
    match problem {
        crate::rename::Problem::Name(error) => name_error_text(error),
        crate::rename::Problem::Edit(error) => edit_error_text(error),
        crate::rename::Problem::Batch(error) => batch_error_text(error),
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
            CopyRejection::Unsupported => "휴대폰과는 PC와만 주고받을 수 있습니다".to_owned(),
            CopyRejection::NotEnoughSpace { .. } => todo!("#426"),
            CopyRejection::EncryptedArchive => todo!("#430"),
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
        CopyRejection::Unsupported => "휴대폰과는 PC와만 주고받을 수 있습니다".to_owned(),
        CopyRejection::NotEnoughSpace { .. } => todo!("#426"),
        CopyRejection::EncryptedArchive => todo!("#430"),
    }
}

/// 끄는 동안 커서 옆에 보일 설명(셸의 드롭 설명): 「〈폴더〉로 이동」「〈폴더〉로 복사」, 놓을 수 없으면 까닭.
/// 셸은 `message`의 `%1`을 `insert`(폴더 이름, 강조)로 바꿔 보인다.
pub fn drop_label(
    effect: Option<DropEffect>,
    folder: Option<&str>,
    refusal: Option<Refusal>,
) -> DropLabel {
    match (effect, folder) {
        (Some(effect), Some(folder)) => {
            let verb = match effect {
                DropEffect::Copy => "복사",
                DropEffect::Move => "이동",
            };
            DropLabel {
                message: format!("%1{} {verb}", direction_particle(folder)),
                insert: folder.to_owned(),
            }
        }
        _ => DropLabel {
            message: match refusal {
                Some(Refusal::IntoItself) => "폴더를 그 안으로 놓을 수 없음",
                Some(Refusal::AlreadyThere) => "이미 이 폴더에 있음",
                Some(Refusal::NotFiles) => "파일이 아님",
                Some(Refusal::NotAFolder) => "여기에는 놓을 수 없음",
                Some(Refusal::NoSources | Refusal::NoEffectAllowed) | None => "놓을 수 없음",
            }
            .to_owned(),
            insert: String::new(),
        },
    }
}

/// 방향을 나타내는 조사: 마지막 글자가 받침 없는 한글이거나 ㄹ 받침이면 「로」, 다른 받침이면 「으로」, 한글이 아니면
/// 「(으)로」.
pub fn direction_particle(word: &str) -> &'static str {
    match word.chars().last() {
        Some(c @ '가'..='힣') => {
            let jong = (c as u32 - '가' as u32) % 28;
            if jong == 0 || jong == 8 {
                "로"
            } else {
                "으로"
            }
        }
        _ => "(으)로",
    }
}

/// 오른쪽 끌기 고르기 메뉴의 줄 이름.
pub fn drop_menu_name(entry: crate::drop_menu::Entry) -> &'static str {
    match entry {
        crate::drop_menu::Entry::CopyHere => "여기에 복사",
        crate::drop_menu::Entry::MoveHere => "여기로 이동",
        crate::drop_menu::Entry::Cancel => "취소",
        crate::drop_menu::Entry::Separator => "",
    }
}

/// 항목 끌기를 시작하지 못한 까닭(상태 줄).
pub fn drag_failed_text(error: &DragError) -> String {
    format!("항목을 끌 수 없습니다: {error}")
}

/// 검색 결과에서 열려던 항목이 디스크에 없어 결과에서 뺐다는 까닭(상태 줄).
pub fn missing_item_text(path: &std::path::Path) -> String {
    format!("항목이 더 이상 없어 결과에서 뺐습니다: {}", path.display())
}

/// 위치의 되는 일이 막은 명령을 실행했을 때 상태 줄 글자.
pub const NOT_ALLOWED_HERE: &str = "이 위치에서는 할 수 없음";

/// zip 위치의 되는 일이 막은 명령의 까닭(상태 줄·오른쪽 메뉴·명령 팔레트).
pub const NOT_SUPPORTED_IN_ZIP: &str = "zip 안에서는 지원하지 않음";

/// 위치 `location`의 되는 일이 막은 명령의 까닭: zip 안이면 [`NOT_SUPPORTED_IN_ZIP`], 아니면 [`NOT_ALLOWED_HERE`].
pub fn not_allowed_text(location: &facet_core::Location) -> &'static str {
    if location.zip_location().is_some() {
        NOT_SUPPORTED_IN_ZIP
    } else {
        NOT_ALLOWED_HERE
    }
}

/// zip 안의 zip을 열려고 했을 때 상태 줄 글자.
pub const NESTED_ZIP: &str = "zip 안의 zip은 열 수 없음, 먼저 꺼내기";

/// zip 위치의 상태 줄 글자: 같은 이름이 있으면 그 수, 손상됐으면 그 사실. 둘 다 없으면 빈 글자.
pub fn archive_notes_text(notes: facet_core::zip_provider::ArchiveNotes) -> String {
    let mut parts = Vec::new();
    if notes.duplicates > 0 {
        parts.push(format!(
            "같은 이름 {}개는 뒤에 나온 것만 보임",
            notes.duplicates
        ));
    }
    if notes.damaged {
        parts.push("zip이 손상되어 읽을 수 있는 항목만 보임".to_string());
    }
    parts.join(" · ")
}

/// 주소창 › 펼침 메뉴의 「읽는 중」 줄(#306).
pub const CRUMB_LOADING: &str = "읽는 중…";

/// 주소창 › 펼침 메뉴의 빈 목록 줄(#306).
pub const CRUMB_EMPTY: &str = "하위 폴더가 없습니다";

/// 주소창 맨 앞 › 메뉴의 사용자 홈 폴더 줄: 홈임을 알 수 있게 이름 뒤에 붙인다(#308).
pub fn home_row_text(name: &str) -> String {
    format!("{name} (홈 폴더)")
}

/// 주소창 › 펼침 메뉴에서 하위 폴더를 읽지 못한 까닭 줄(#306).
pub fn crumb_error_text(error: facet_core::window::SubfoldersError) -> String {
    use facet_core::window::SubfoldersError;
    match error {
        SubfoldersError::NotFound => "폴더를 찾을 수 없습니다".to_owned(),
        SubfoldersError::PermissionDenied => "권한이 없어 읽을 수 없습니다".to_owned(),
        SubfoldersError::NotAFolder => "폴더가 아닙니다".to_owned(),
        SubfoldersError::Io(kind) => format!("읽지 못했습니다: {kind}"),
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
        // 일괄 이름 바꾸기는 항목이 하나여도 수로 보인다(작업 이름이 한 줄로 같게).
        (OperationKind::BatchRename, _) => {
            format!("일괄 이름 바꾸기({}개)", record.steps().len())
        }
        (OperationKind::Extract, Some(name)) => format!("「{name}」 꺼내기"),
        (OperationKind::Extract, None) => format!("항목 {n}개 꺼내기"),
        (OperationKind::NewZip, _) => todo!("#429"),
        (OperationKind::AddToZip, _) => todo!("#430"),
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
        NotUndoable::NotUndoableThere => "되돌릴 수 없음: 휴대폰 등 되돌릴 수 없는 위치".to_owned(),
    }
}

/// 마지막 작업이 되돌릴 수 없어 실행취소하지 않았다(상태 줄). 그 앞 작업도 되돌리지 않았다.
pub fn undo_refused_text(record: &Record, reason: NotUndoable) -> String {
    let why = match reason {
        NotUndoable::PermanentDelete => "영구 삭제한 항목은 되살릴 수 없습니다",
        NotUndoable::Overwritten => "덮어쓴 파일은 되살릴 수 없습니다",
        NotUndoable::NotUndoableThere => "휴대폰 등 그 위치에 넣은 작업은 되돌릴 수 없습니다",
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
        FileOpErrorKind::SyncProviderNotRunning => todo!("#453"),
        FileOpErrorKind::Corrupt => "데이터가 손상되었습니다",
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

/// 오른쪽 메뉴 셸 항목 자리의 안내 줄 글자(#322).
pub fn shell_notice_text(notice: &crate::context_menu::Notice) -> String {
    use crate::context_menu::Notice;
    match notice {
        Notice::Loading => "셸 항목 불러오는 중…".to_owned(),
        Notice::Failed => "셸 항목 불러오지 못함".to_owned(),
        Notice::Resting => "셸 확장 도우미가 연달아 멈춰 잠시 쉬는 중".to_owned(),
        Notice::Crashing(name) => format!("셸 확장 「{name}」이(가) 도우미를 자꾸 멈추게 함"),
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
        Key::Comma => todo!("#462"),
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

/// 검색 결과 위치의 탭 제목·경로 조각: 검색어 앞에 「검색:」을 붙여 폴더와 구분한다.
pub fn search_title(query: &str) -> String {
    format!("검색: {query}")
}

/// 걸린 시간 글자: 1분 미만은 소수 한 자리 초(`1.5초`), 그 이상은 `분:초`.
pub fn elapsed_text(elapsed: std::time::Duration) -> String {
    if elapsed.as_secs() < 60 {
        format!("{:.1}초", elapsed.as_secs_f64())
    } else {
        remaining_text(elapsed)
    }
}

/// 탐색 상태 줄 글자(목록 위 띠): 탐색 중이면 찾은 수·훑은 폴더 수, 끝나면 찾은 수·건너뛴 폴더 수·걸린 시간.
pub fn search_line_text(line: crate::search_bar::Line) -> String {
    use crate::search_bar::Line;
    let tail = |found: usize, skipped: usize, elapsed, files: Option<Skipped>, online: usize| {
        format!(
            "{found}개 찾음 · 건너뛴 폴더 {skipped}개 · {}{}{}",
            elapsed_text(elapsed),
            online_folders_text(online),
            skipped_files_text(files)
        )
    };
    match line {
        Line::Running {
            found,
            folders,
            files_skipped,
            online_folders,
        } => format!(
            "탐색 중… {found}개 찾음 · 폴더 {folders}개 훑음{}{}",
            online_folders_text(online_folders),
            skipped_files_text(files_skipped)
        ),
        Line::Finished {
            found,
            skipped,
            elapsed,
            files_skipped,
            online_folders,
        } => format!(
            "탐색 끝 · {}",
            tail(found, skipped, elapsed, files_skipped, online_folders)
        ),
        Line::Stopped {
            found,
            skipped,
            elapsed,
            files_skipped,
            online_folders,
        } => format!(
            "중지함 · {}",
            tail(found, skipped, elapsed, files_skipped, online_folders)
        ),
    }
}

/// 실시간 탐색이 내려가지 않은 펼쳐지지 않은 온라인 폴더 수(상태 줄 끝에 붙는다). 없으면 비어 있다.
pub fn online_folders_text(count: usize) -> String {
    if count == 0 {
        String::new()
    } else {
        format!(" · 온라인 폴더 {count}개는 열지 않음")
    }
}

/// 내용 검색이 건너뛴 파일의 종류별 수(상태 줄 끝에 붙는다). 건너뛴 것이 없거나 내용 검색이 아니면 비어 있다.
pub fn skipped_files_text(skipped: Option<Skipped>) -> String {
    let Some(skipped) = skipped.filter(|skipped| skipped.total() > 0) else {
        return String::new();
    };
    let kinds = [
        ("바이너리", skipped.binary),
        ("온라인 전용", skipped.online_only),
        ("큰 파일", skipped.too_large),
        ("읽지 못함", skipped.unreadable),
    ];
    let parts: Vec<String> = kinds
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .map(|(name, count)| format!("{name} {count}"))
        .collect();
    format!(" · 건너뛴 파일: {}", parts.join(", "))
}

/// 내용 검색 결과 항목의 「일치」 열 글자: 일치 수와 첫 일치 줄.
pub fn match_text(hit: &facet_core::details::Hit) -> String {
    format!("{}회 · {}", hit.matches, hit.first_line)
}

/// 내 PC 드라이브의 「전체 크기」「남은 공간」 열 글자: 크기(`465.1 GB`), 모르면(끊긴 네트워크 드라이브·빈 카드 리더·
/// 아직 얻는 중) 「—」.
pub fn drive_space_text(bytes: Option<u64>) -> String {
    bytes.map_or_else(|| "—".to_owned(), facet_core::file_list::format_size)
}

/// 내용 검색을 시작하지 못한 까닭(검색창 곁에 보인다).
pub fn content_search_error_text(error: &facet_core::window::ContentSearchError) -> String {
    use facet_core::window::ContentSearchError;
    match error {
        ContentSearchError::EmptyText => "찾을 글자를 입력하세요".to_owned(),
        ContentSearchError::Regex(reason) => {
            // regex 크레이트의 설명은 여러 줄이라 마지막 줄(오류 요지)만 보인다(한 줄 자리).
            let last = reason.lines().rev().find(|line| !line.trim().is_empty());
            let last = last.unwrap_or("").trim();
            let last = last.strip_prefix("error: ").unwrap_or(last);
            format!("정규식이 올바르지 않습니다: {last}")
        }
        ContentSearchError::NoFolder => "여기서는 검색할 수 없습니다".to_owned(),
    }
}

/// 이름 검색을 시작하지 못한 까닭(검색창 아래에 보인다). 문법이 틀리면 틀린 곳을 글자 순서(1부터)로 알린다.
pub fn search_error_text(query: &str, error: &facet_core::window::SearchError) -> String {
    use facet_core::query_syntax::{Keyword, ParseErrorKind};
    use facet_core::window::SearchError;
    match error {
        SearchError::Empty => "검색어를 입력하세요".to_owned(),
        SearchError::NoFolder => "여기서는 검색할 수 없습니다".to_owned(),
        SearchError::Syntax(error) => {
            let keyword = |k: &Keyword| match k {
                Keyword::Ext => "ext:",
                Keyword::Size => "size:",
                Keyword::Dm => "dm:",
                Keyword::Path => "path:",
                Keyword::Regex => "regex:",
                Keyword::Folder => "folder:",
                Keyword::File => "file:",
            };
            let reason = match &error.kind {
                ParseErrorKind::UnclosedQuote => "닫히지 않은 큰따옴표입니다".to_owned(),
                ParseErrorKind::UnclosedGroup => "닫히지 않은 <입니다".to_owned(),
                ParseErrorKind::UnmatchedClose => "짝이 없는 >입니다".to_owned(),
                ParseErrorKind::EmptyGroup => "비어 있는 <>입니다".to_owned(),
                ParseErrorKind::MissingOperand => "연산자에 조건이 없습니다".to_owned(),
                ParseErrorKind::MissingValue(k) => format!("{}에 값이 없습니다", keyword(k)),
                ParseErrorKind::UnexpectedValue(k) => {
                    format!("{}에는 값을 쓰지 않습니다", keyword(k))
                }
                ParseErrorKind::InvalidSize => "크기 값이 올바르지 않습니다".to_owned(),
                ParseErrorKind::InvalidDate => "날짜 값이 올바르지 않습니다".to_owned(),
                ParseErrorKind::InvalidRegex(reason) => {
                    let last = reason.lines().rev().find(|line| !line.trim().is_empty());
                    let last = last.unwrap_or("").trim();
                    let last = last.strip_prefix("error: ").unwrap_or(last);
                    format!("정규식이 올바르지 않습니다: {last}")
                }
                ParseErrorKind::TooDeep => "<>를 너무 깊게 겹쳤습니다".to_owned(),
            };
            // 바이트 오프셋을 글자 순서로 바꾼다(범위가 글자 경계가 아니어도 넘어지지 않게 자른다).
            let at = error.range.start.min(query.len());
            let chars = query.char_indices().take_while(|(i, _)| *i < at).count();
            format!(
                "검색어가 올바르지 않습니다 ({}번째 글자): {reason}",
                chars + 1
            )
        }
    }
}

/// 즉시 검색 상태 줄 글자: 전체 개수(상한이면 그렇게)·「덜 찼을 수 있음」, 강등이면 「인덱스 없음 · 실시간 탐색」과 까닭.
pub fn instant_status_text(line: &crate::search_bar::InstantLine) -> String {
    use crate::search_bar::{InstantCount, Waiting};
    use facet_core::instant_search::DemotionReason;
    let mut parts: Vec<String> = Vec::new();
    if let Some(demotion) = line.demotion {
        parts.push("인덱스 없음 · 실시간 탐색".to_owned());
        parts.push(
            match demotion.reason {
                DemotionReason::NoService => "서비스 없음",
                DemotionReason::VersionMismatch => "서비스 버전이 다름",
                DemotionReason::VolumeNotIndexed => "이 볼륨은 인덱스 대상 아님",
            }
            .to_owned(),
        );
        if demotion.narrowed_to_folder {
            parts.push("현재 폴더로 좁힘".to_owned());
        }
    } else if let Some(waiting) = line.waiting {
        parts.push(
            match waiting {
                Waiting::Connecting => "서비스에 연결하는 중…",
                Waiting::NoAnswer => "서비스가 답하지 않음",
            }
            .to_owned(),
        );
    }
    match line.count {
        InstantCount::Unknown => {}
        InstantCount::Total { total, upper_bound } => parts.push(if upper_bound {
            format!("최대 {total}개 (접근 권한으로 줄 수 있음)")
        } else {
            format!("{total}개")
        }),
        InstantCount::Found(found) => parts.push(format!("{found}개 찾음")),
    }
    if !line.building.is_empty() {
        parts.push(format!(
            "덜 찼을 수 있음 (인덱스 만드는 중: {})",
            line.building.join(", ")
        ));
    }
    parts.join(" · ")
}

/// 즉시 검색 문법 오류 글자: 틀린 곳(글자 순서)과 까닭, 틀린 글자 조각.
pub fn instant_problem_text(query: &str, error: &facet_core::query_syntax::ParseError) -> String {
    let base = search_error_text(
        query,
        &facet_core::window::SearchError::Syntax(error.clone()),
    );
    match crate::search_bar::wrong_part(query, &error.range) {
        Some(part) => format!("{base} — 「{part}」"),
        None => base,
    }
}

/// 운영 체제가 준 오류 문장과 코드(HRESULT)를 한 글자로: `메시지 (0x80070005)`.
/// 문장은 운영 체제의 언어로 오므로 번역하지 않는다.
pub fn os_error_detail(message: &str, code: i32) -> String {
    format!("{message} (0x{code:08X})")
}

/// 미리보기 영역: 선택이 없을 때의 안내.
pub fn preview_nothing_text() -> String {
    "미리 볼 항목을 고르세요".to_owned()
}

/// 미리보기 영역: 정보 줄의 이름표.
pub fn preview_label_type() -> &'static str {
    "유형"
}

/// 미리보기 영역: 정보 줄의 이름표.
pub fn preview_label_size() -> &'static str {
    "크기"
}

/// 미리보기 영역: 정보 줄의 이름표.
pub fn preview_label_modified() -> &'static str {
    "수정한 날짜"
}

/// 미리보기 영역: 정보 줄의 이름표.
pub fn preview_label_items() -> &'static str {
    "항목"
}

/// 미리보기 영역: 폴더의 항목 수. `None`이면 아직 세는 중이다.
pub fn preview_folder_count_text(count: Option<u64>) -> String {
    match count {
        Some(count) => format!("{count}개"),
        None => "세는 중".to_owned(),
    }
}

/// 미리보기 영역: 여러 항목을 골랐을 때의 제목.
pub fn preview_many_text(count: usize) -> String {
    format!("{count}개 선택")
}

/// 미리보기 영역: 여러 항목의 크기 합.
pub fn preview_total_size_text(total: u64) -> String {
    format!("크기 합 {}", facet_core::file_list::format_size(total))
}

/// 미리보기 영역: 텍스트가 앞부분만 보일 때(바이트 한도를 KB로).
pub fn preview_truncated_text(limit_bytes: usize) -> String {
    format!("앞 {}KB만 보임", limit_bytes / 1024)
}

/// 미리보기 영역: 내용을 읽는 중.
pub fn preview_loading_text() -> String {
    "불러오는 중".to_owned()
}

/// 미리보기 영역: 보지 않은 이유. 정보만 보이는 까닭(`NotText`·`Unsupported`)은 `None`이다.
pub fn preview_problem_text(problem: facet_core::preview::PreviewProblem) -> Option<String> {
    use facet_core::preview::PreviewProblem as P;
    let text = match problem {
        P::OnlineOnly => "온라인 전용 항목이라 내려받지 않음",
        P::Missing => "항목이 사라짐",
        P::Locked => "다른 프로그램이 쓰고 있어 볼 수 없음",
        P::AccessDenied => "권한이 없어 볼 수 없음",
        P::Corrupt => "파일이 깨져 볼 수 없음",
        P::TooLarge => "너무 커서 보지 않음",
        P::NotText | P::Unsupported => return None,
        P::ShellFailed => "미리보기를 불러오지 못함",
        P::Failed | P::Io(_) => "미리 보지 못함",
    };
    Some(text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 드라이브_용량_글자는_크기이고_모르면_줄표다() {
        assert_eq!(drive_space_text(None), "—");
        assert_eq!(drive_space_text(Some(500 * 1024 * 1024 * 1024)), "500.0 GB");
    }

    #[test]
    fn 즉시_검색_상태_글자() {
        use crate::search_bar::{InstantCount, InstantLine};
        use facet_core::instant_search::{Demotion, DemotionReason};
        let base = InstantLine {
            count: InstantCount::Total {
                total: 12,
                upper_bound: false,
            },
            building: Vec::new(),
            waiting: None,
            demotion: None,
        };
        assert_eq!(instant_status_text(&base), "12개");
        let upper = InstantLine {
            count: InstantCount::Total {
                total: 9,
                upper_bound: true,
            },
            building: vec!["C:".to_owned()],
            ..base.clone()
        };
        let text = instant_status_text(&upper);
        assert!(
            text.contains("최대 9개") && text.contains("덜 찼을 수 있음"),
            "{text}"
        );
        let demoted = InstantLine {
            count: InstantCount::Found(3),
            demotion: Some(Demotion {
                reason: DemotionReason::VolumeNotIndexed,
                narrowed_to_folder: true,
            }),
            ..base
        };
        let text = instant_status_text(&demoted);
        assert!(text.starts_with("인덱스 없음 · 실시간 탐색"), "{text}");
        assert!(
            text.contains("인덱스 대상 아님") && text.contains("현재 폴더로 좁힘"),
            "{text}"
        );
        assert!(text.contains("3개 찾음"), "{text}");
    }

    #[test]
    fn 탐색_상태_줄_글자() {
        use crate::search_bar::Line;
        use std::time::Duration;
        assert_eq!(
            search_line_text(Line::Running {
                found: 3,
                folders: 9,
                files_skipped: None,
                online_folders: 0
            }),
            "탐색 중… 3개 찾음 · 폴더 9개 훑음"
        );
        assert_eq!(
            search_line_text(Line::Finished {
                found: 7,
                skipped: 2,
                elapsed: Duration::from_millis(1500),
                files_skipped: None,
                online_folders: 0
            }),
            "탐색 끝 · 7개 찾음 · 건너뛴 폴더 2개 · 1.5초"
        );
        assert_eq!(
            search_line_text(Line::Finished {
                found: 7,
                skipped: 0,
                elapsed: Duration::from_millis(1500),
                files_skipped: None,
                online_folders: 1
            }),
            "탐색 끝 · 7개 찾음 · 건너뛴 폴더 0개 · 1.5초 · 온라인 폴더 1개는 열지 않음"
        );
        let files = Skipped {
            binary: 2,
            online_only: 1,
            too_large: 0,
            unreadable: 3,
        };
        assert_eq!(
            search_line_text(Line::Finished {
                found: 1,
                skipped: 0,
                elapsed: Duration::from_millis(1500),
                files_skipped: Some(files),
                online_folders: 0
            }),
            "탐색 끝 · 1개 찾음 · 건너뛴 폴더 0개 · 1.5초 · 건너뛴 파일: 바이너리 2, 온라인 전용 1, 읽지 못함 3"
        );
        assert_eq!(skipped_files_text(Some(Skipped::default())), "");
        assert_eq!(skipped_files_text(None), "");
        assert_eq!(elapsed_text(Duration::from_secs(125)), "2:05");
        assert_eq!(search_title("보고서"), "검색: 보고서");
    }

    #[test]
    fn 끄는_동안의_설명은_폴더_이름과_알맞은_조사() {
        assert_eq!(direction_particle("사진"), "으로");
        assert_eq!(direction_particle("문서"), "로");
        assert_eq!(direction_particle("서울"), "로", "ㄹ 받침은 「로」");
        assert_eq!(direction_particle("Downloads"), "(으)로");
        assert_eq!(direction_particle(""), "(으)로");
        let label = drop_label(Some(DropEffect::Move), Some("사진"), None);
        assert_eq!(label.message, "%1으로 이동");
        assert_eq!(label.insert, "사진");
        let label = drop_label(Some(DropEffect::Copy), Some("C:"), None);
        assert_eq!(label.message, "%1(으)로 복사");
        let label = drop_label(None, Some("사진"), Some(Refusal::IntoItself));
        assert_eq!(label.insert, "");
        assert!(!label.message.contains("%1"));
    }

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
    fn 일괄_이름_바꾸기는_바꾼_항목_수로_부른다() {
        let batch = Record::batch_rename(
            vec![
                renamed(r"C:\a\사진 1.jpg", r"C:\a\휴가 1.jpg"),
                renamed(r"C:\b\사진 2.jpg", r"C:\b\휴가 2.jpg"),
                renamed(r"C:\a\사진 3.jpg", r"C:\a\휴가 3.jpg"),
            ],
            4,
        );
        assert_eq!(operation_name(&batch), "일괄 이름 바꾸기(3개)");
        assert_eq!(undo_notice(&batch), "일괄 이름 바꾸기(3개) 실행취소");
        assert_eq!(redo_notice(&batch), "일괄 이름 바꾸기(3개) 다시 실행");
        // 항목이 하나여도 이름이 아니라 수다.
        let one = Record::batch_rename(vec![renamed(r"C:\a\x", r"C:\a\y")], 0);
        assert_eq!(operation_name(&one), "일괄 이름 바꾸기(1개)");
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
        let extract = Record::new(
            OperationKind::Extract,
            vec![copied(r"C:\b\문서"), copied(r"C:\b\문서\readme.txt")],
        );
        assert_eq!(operation_name(&extract), "「문서」 꺼내기");
        let extract = Record::new(
            OperationKind::Extract,
            vec![copied(r"C:\b\1.txt"), copied(r"C:\b\2.txt")],
        );
        assert_eq!(operation_name(&extract), "항목 2개 꺼내기");
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

#[cfg(test)]
mod preset_tests {
    use super::*;

    #[test]
    fn 묶음_파일_알림_글자() {
        assert_eq!(preset_notice_text(0, None), "");
        assert!(preset_notice_text(2, None).contains('2'));
        let kept = Some("x.broken.toml".to_owned());
        let text = preset_notice_text(0, Some(&kept));
        assert!(text.contains("x.broken.toml"));
    }
}
