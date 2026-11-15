//! 질문 대화(이름 충돌·항목별 오류)에 넘길 값: 코어의 질문([`JobQuestion`])을 화면의 값으로 바꾸고, 화면의 답을
//! 코어의 답으로 바꾼다. 문장은 화면(`@tr()`)이 만든다(ADR-0007).

use std::time::SystemTime;

use facet_core::file_list;
use facet_core::job::{
    Conflict, ConflictAnswer, ConflictItem, ErrorAnswer, FileOpErrorKind, ItemKind, JobQuestion,
    Resolution,
};

use crate::{Compare, ConflictChoice, ConflictSide, ErrorChoice, ErrorReason, JobAsk};

/// 화면의 질문 값. 질문이 없으면 [`QuestionView::default`](모두 비어 있음).
#[derive(Debug, Clone, PartialEq)]
pub struct QuestionView {
    pub ask: JobAsk,
    pub source: ConflictSide,
    pub target: ConflictSide,
    pub newer: Compare,
    pub larger: Compare,
    pub can_overwrite: bool,
    pub overwrite_permanent: bool,
    pub error_path: String,
    pub error_reason: ErrorReason,
}

impl Default for QuestionView {
    fn default() -> Self {
        QuestionView {
            ask: JobAsk::None,
            source: ConflictSide::default(),
            target: ConflictSide::default(),
            newer: Compare::Unknown,
            larger: Compare::Unknown,
            can_overwrite: false,
            overwrite_permanent: false,
            error_path: String::new(),
            error_reason: ErrorReason::Other,
        }
    }
}

/// 작업 영역의 질문을 화면 값으로. 크기·수정 시각은 자세히 보기와 같은 서식이다.
pub fn view(question: Option<&JobQuestion>) -> QuestionView {
    match question {
        None => QuestionView::default(),
        Some(JobQuestion::Conflict(conflict)) => conflict_view(conflict),
        Some(JobQuestion::Error(failure)) => QuestionView {
            ask: JobAsk::Error,
            error_path: failure.source.display().to_string(),
            error_reason: error_reason(failure.kind),
            ..QuestionView::default()
        },
    }
}

fn conflict_view(conflict: &Conflict) -> QuestionView {
    QuestionView {
        ask: JobAsk::Conflict,
        source: side(&conflict.source),
        target: side(&conflict.target),
        newer: newer(conflict.source.modified, conflict.target.modified),
        larger: larger(&conflict.source, &conflict.target),
        can_overwrite: conflict.can_overwrite,
        overwrite_permanent: conflict.overwrite_is_permanent,
        ..QuestionView::default()
    }
}

fn side(item: &ConflictItem) -> ConflictSide {
    let folder = item.kind == ItemKind::Folder;
    ConflictSide {
        path: item.path.display().to_string().into(),
        folder,
        size: match item.size {
            Some(size) if !folder => file_list::format_size(size).into(),
            _ => Default::default(),
        },
        modified: item
            .modified
            .map(file_list::format_modified_local)
            .unwrap_or_default()
            .into(),
    }
}

/// 어느 쪽이 더 새것인가(수정 시각). 한쪽이라도 모르면 [`Compare::Unknown`].
pub fn newer(source: Option<SystemTime>, target: Option<SystemTime>) -> Compare {
    match (source, target) {
        (Some(s), Some(t)) => order(s.cmp(&t)),
        _ => Compare::Unknown,
    }
}

/// 어느 쪽이 더 큰가. 둘 다 파일이고 크기를 알 때만 비교한다.
pub fn larger(source: &ConflictItem, target: &ConflictItem) -> Compare {
    if source.kind != ItemKind::File || target.kind != ItemKind::File {
        return Compare::Unknown;
    }
    match (source.size, target.size) {
        (Some(s), Some(t)) => order(s.cmp(&t)),
        _ => Compare::Unknown,
    }
}

fn order(ordering: std::cmp::Ordering) -> Compare {
    match ordering {
        std::cmp::Ordering::Greater => Compare::Source,
        std::cmp::Ordering::Less => Compare::Target,
        std::cmp::Ordering::Equal => Compare::Same,
    }
}

/// 항목별 오류의 까닭. 대화에 따로 문장이 없는 종류는 「그 밖」이다.
pub fn error_reason(kind: FileOpErrorKind) -> ErrorReason {
    match kind {
        FileOpErrorKind::Locked => ErrorReason::Locked,
        FileOpErrorKind::AccessDenied => ErrorReason::AccessDenied,
        FileOpErrorKind::DiskFull => ErrorReason::DiskFull,
        FileOpErrorKind::SourceMissing => ErrorReason::SourceMissing,
        _ => ErrorReason::Other,
    }
}

/// 이름 충돌 질문 대화의 답.
pub fn conflict_answer(choice: ConflictChoice, apply_to_rest: bool) -> ConflictAnswer {
    ConflictAnswer {
        resolution: match choice {
            ConflictChoice::Overwrite => Resolution::Overwrite,
            ConflictChoice::OverwriteIfNewer => Resolution::OverwriteIfNewer,
            ConflictChoice::KeepBoth => Resolution::KeepBoth,
            ConflictChoice::Skip => Resolution::Skip,
        },
        apply_to_rest,
    }
}

/// 항목별 오류 질문 대화의 답.
pub fn error_answer(choice: ErrorChoice) -> ErrorAnswer {
    match choice {
        ErrorChoice::Retry => ErrorAnswer::Retry,
        ErrorChoice::Skip => ErrorAnswer::Skip,
        ErrorChoice::SkipAll => ErrorAnswer::SkipAll,
        ErrorChoice::CancelJob => ErrorAnswer::CancelJob,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use facet_core::job::ItemFailure;

    use super::*;

    fn file(size: u64, modified: Option<SystemTime>) -> ConflictItem {
        ConflictItem {
            path: PathBuf::from(r"C:\곳\메모.txt"),
            kind: ItemKind::File,
            size: Some(size),
            modified,
        }
    }

    #[test]
    fn 더_새것과_더_큰_쪽() {
        let early = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let late = early + Duration::from_secs(60);
        assert_eq!(newer(Some(late), Some(early)), Compare::Source);
        assert_eq!(newer(Some(early), Some(late)), Compare::Target);
        assert_eq!(newer(Some(early), Some(early)), Compare::Same);
        assert_eq!(newer(None, Some(early)), Compare::Unknown);
        assert_eq!(larger(&file(10, None), &file(3, None)), Compare::Source);
        assert_eq!(larger(&file(3, None), &file(10, None)), Compare::Target);
        assert_eq!(larger(&file(3, None), &file(3, None)), Compare::Same);
        let folder = ConflictItem {
            kind: ItemKind::Folder,
            size: None,
            ..file(0, None)
        };
        assert_eq!(larger(&file(3, None), &folder), Compare::Unknown);
    }

    #[test]
    fn 이름_충돌_질문의_화면_값() {
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let conflict = Conflict {
            source: file(2048, Some(time + Duration::from_secs(60))),
            target: ConflictItem {
                kind: ItemKind::Folder,
                size: None,
                ..file(0, Some(time))
            },
            can_overwrite: false,
            overwrite_is_permanent: false,
        };
        let view = view(Some(&JobQuestion::Conflict(conflict)));
        assert_eq!(view.ask, JobAsk::Conflict);
        assert_eq!(view.source.size, "2.0 KB");
        assert!(!view.source.folder);
        assert_eq!(
            view.source.modified,
            file_list::format_modified_local(time + Duration::from_secs(60))
        );
        assert!(view.target.folder);
        assert_eq!(view.target.size, "");
        assert_eq!(view.newer, Compare::Source);
        assert_eq!(view.larger, Compare::Unknown);
        assert!(!view.can_overwrite);
    }

    #[test]
    fn 오류_질문의_화면_값() {
        let view = view(Some(&JobQuestion::Error(ItemFailure {
            source: PathBuf::from(r"C:\곳\잠긴.txt"),
            kind: FileOpErrorKind::DiskFull,
        })));
        assert_eq!(view.ask, JobAsk::Error);
        assert_eq!(view.error_path, r"C:\곳\잠긴.txt");
        assert_eq!(view.error_reason, ErrorReason::DiskFull);
        assert_eq!(error_reason(FileOpErrorKind::Locked), ErrorReason::Locked);
        assert_eq!(
            error_reason(FileOpErrorKind::TargetExists),
            ErrorReason::Other
        );
        assert_eq!(super::view(None), QuestionView::default());
    }

    #[test]
    fn 화면의_답을_코어의_답으로() {
        assert_eq!(
            conflict_answer(ConflictChoice::OverwriteIfNewer, true),
            ConflictAnswer::for_rest(Resolution::OverwriteIfNewer)
        );
        assert_eq!(
            conflict_answer(ConflictChoice::KeepBoth, false),
            ConflictAnswer::once(Resolution::KeepBoth)
        );
        assert_eq!(error_answer(ErrorChoice::SkipAll), ErrorAnswer::SkipAll);
        assert_eq!(error_answer(ErrorChoice::CancelJob), ErrorAnswer::CancelJob);
    }
}
