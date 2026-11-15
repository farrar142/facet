//! 작업 기록 보기: 작업 기록([`History`])을 창 위에 겹쳐 그릴 읽기 전용 목록의 줄로 바꾼다.
//!
//! 끝난 작업을 최근 것부터, 줄마다 작업 이름·결과(끝낸·건너뛴 항목 수)·되돌릴 수 있는지를 보인다. 골라 되돌리기는
//! 없다. 줄은 기록의 종류와 상관없이 같은 모양이라 작업 종류가 늘어도 이 화면은 그대로이고, 이름과 까닭 글자만
//! 문장 모듈([`crate::text`])이 만든다.

use facet_core::history::History;

use crate::text;

/// 작업 기록 보기의 줄 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// 작업 이름(「새.txt」 이름 바꾸기 등).
    pub name: String,
    /// 끝낸 항목 수.
    pub completed: u64,
    /// 건너뛴 항목 수.
    pub skipped: u64,
    /// 되돌릴 수 없으면 그 까닭의 글자. 되돌릴 수 있으면 `None`.
    pub not_undoable: Option<String>,
}

/// 작업 기록의 줄들, 최근 것부터. 도는 실행취소·다시 실행의 대상과 다시 실행 목록은 들지 않는다(끝난 작업만).
pub fn lines(history: &History) -> Vec<Line> {
    history
        .records()
        .rev()
        .map(|record| Line {
            name: text::operation_name(record),
            completed: record.items_completed(),
            skipped: record.items_skipped(),
            not_undoable: record.not_undoable_reason().map(text::not_undoable_text),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::SystemTime;

    use facet_core::history::{NotUndoable, OperationKind, Record, Stamp, Step};

    use super::*;

    fn folder(name: &str) -> Step {
        Step::CreatedFolder {
            path: PathBuf::from(format!(r"C:\a\{name}")),
            stamp: Stamp {
                is_folder: true,
                size: 0,
                modified: SystemTime::UNIX_EPOCH,
            },
        }
    }

    #[test]
    fn 최근_것부터_이름_결과_되돌릴_수_있는지() {
        let mut history = History::new();
        assert!(lines(&history).is_empty());
        history.record(Record::new(OperationKind::NewFolder, vec![folder("첫")]));
        history.record(
            Record::new(OperationKind::NewFolder, vec![folder("1"), folder("2")]).with_skipped(1),
        );
        history.record(Record::not_undoable(
            OperationKind::Rename,
            vec![folder("셋")],
            NotUndoable::PermanentDelete,
        ));
        assert_eq!(
            lines(&history),
            vec![
                Line {
                    name: "「셋」 이름 바꾸기".to_owned(),
                    completed: 1,
                    skipped: 0,
                    not_undoable: Some("되돌릴 수 없음: 영구 삭제".to_owned()),
                },
                Line {
                    name: "새 폴더 2개 만들기".to_owned(),
                    completed: 2,
                    skipped: 1,
                    not_undoable: None,
                },
                Line {
                    name: "새 폴더 「첫」 만들기".to_owned(),
                    completed: 1,
                    skipped: 0,
                    not_undoable: None,
                },
            ]
        );
    }
}
