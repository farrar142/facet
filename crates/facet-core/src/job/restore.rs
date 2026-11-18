//! **휴지통 복원** 작업: 휴지통 항목을 셸의 복원 경로([`facet_win::recycle::restore`])로 원래 자리에 되돌린다.
//! 작업 스레드에서 부르므로 UI 스레드는 셸을 기다리지 않는다. 원래 폴더가 없으면 셸이 만든다.
//!
//! - 원래 자리에 같은 이름이 있으면 이름 충돌로 묻는다(원본 쪽은 휴지통 항목, 대상 쪽은 원래 자리의 항목).
//!   건너뛰기는 항목을 휴지통에 남기고 원래 자리를 그대로 둔다. 둘 다 유지는 「이름 (2)」로 복원한다. 덮어쓰기는 원래
//!   자리의 항목을 휴지통으로 보낸 뒤 복원한다(영구히 지우지 않는다: 휴지통에 넣을 수 없으면 항목별 오류로 묻고,
//!   복원이 실패하면 휴지통으로 보낸 것을 되돌린다). 더 새것만 덮어쓰기는 휴지통 항목의 수정 시각이 더 늦을 때만
//!   덮어쓴다. 「나머지에도 적용」은 다른 복사·이동 작업과 같다.
//! - 휴지통 항목이 이미 없으면(비움·다른 곳에서 복원) 항목별 오류로 묻는다.

use std::fs;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use facet_win::file_ops::FileOpErrorKind;
use facet_win::recycle::{self, RecycleOutcome, RecycledItem};

use super::copy::{Copier, ErrorStep, conflict_item, free_numbered};
use super::{Context, DeleteRejection, ItemKind, Resolution, Summary};

/// 넣기 전에 확인을 마친 복원 작업.
pub(super) struct RestoreJob {
    /// 복원할 휴지통 항목(넘긴 차례대로).
    items: Vec<RecycledItem>,
}

impl RestoreJob {
    /// 시작 전에 막을 경우를 확인한다.
    pub(super) fn prepare(items: Vec<RecycledItem>) -> Result<Self, DeleteRejection> {
        if items.is_empty() {
            return Err(DeleteRejection::NoSources);
        }
        Ok(RestoreJob { items })
    }

    /// 넘긴 항목 수.
    pub(super) fn root_count(&self) -> u64 {
        self.items.len() as u64
    }

    /// 작업 스레드에서 끝까지(또는 취소될 때까지) 실행하고 결과 요약과 취소로 멈췄는지를 돌려준다.
    pub(super) fn run(self, context: &Context<'_>) -> (Summary, bool) {
        let mut engine = Copier::new(context, false);
        let totals = Arc::clone(&engine.reporter.totals);
        totals
            .items
            .store(self.items.len() as u64, Ordering::Release);
        totals.counted.store(true, Ordering::Release);
        for item in self.items {
            if !context.control.proceed() {
                engine.cancelled = true;
                break;
            }
            engine.reporter.current = Some(item.recycled().to_path_buf());
            restore_one(&mut engine, item);
            if engine.cancelled {
                break;
            }
            engine.reporter.maybe_emit(context.control);
        }
        engine.finish()
    }
}

/// 항목 하나를 복원한다(충돌·오류 질문 포함).
fn restore_one(engine: &mut Copier<'_>, item: RecycledItem) {
    loop {
        match attempt(engine, &item) {
            Ok(()) => {
                engine.reporter.items_done += 1;
                return;
            }
            Err(Stop::Skipped) => {
                engine.reporter.items_done += 1;
                return;
            }
            Err(Stop::Cancelled) => return,
            Err(Stop::Failed(kind)) => match engine.on_error(item.recycled(), kind) {
                ErrorStep::Retry => {}
                ErrorStep::Skip => {
                    engine.reporter.items_done += 1;
                    return;
                }
                ErrorStep::Cancel => return,
            },
        }
    }
}

enum Stop {
    /// 충돌에서 건너뛰었다(휴지통에 남는다).
    Skipped,
    /// 질문에서 작업을 취소했다.
    Cancelled,
    Failed(FileOpErrorKind),
}

fn attempt(engine: &mut Copier<'_>, item: &RecycledItem) -> Result<(), Stop> {
    if fs::symlink_metadata(item.recycled()).is_err() {
        return Err(Stop::Failed(FileOpErrorKind::SourceMissing));
    }
    let original = item.original();
    let mut destination = item.clone();
    if fs::symlink_metadata(original).is_ok() {
        let resolution = engine
            .resolve(item.recycled(), original, true)
            .ok_or_else(|| {
                engine.cancelled = true;
                Stop::Cancelled
            })?;
        match resolution {
            Resolution::Skip => return Err(Stop::Skipped),
            Resolution::KeepBoth => {
                let is_file = conflict_item(original).kind == ItemKind::File;
                destination = RecycledItem::new(
                    free_numbered(original, is_file),
                    item.recycled().to_path_buf(),
                );
            }
            Resolution::OverwriteIfNewer if !is_newer(item, original) => {
                return Err(Stop::Skipped);
            }
            Resolution::Overwrite | Resolution::OverwriteIfNewer => {
                return overwrite(item);
            }
        }
    }
    recycle::restore(&destination)
        .map(|_| ())
        .map_err(|error| Stop::Failed(error.kind()))
}

/// 휴지통 항목이 원래 자리의 항목보다 새것인가.
fn is_newer(item: &RecycledItem, original: &std::path::Path) -> bool {
    let modified = |path: &std::path::Path| fs::metadata(path).and_then(|m| m.modified()).ok();
    match (modified(item.recycled()), modified(original)) {
        (Some(bin), Some(now)) => bin > now,
        _ => false,
    }
}

/// 원래 자리의 항목을 휴지통으로 보낸 뒤 복원한다. 복원이 실패하면 보낸 항목을 되돌린다(영구히 지우지 않는다).
fn overwrite(item: &RecycledItem) -> Result<(), Stop> {
    let original = item.original().to_path_buf();
    let moved = match crate::shell_provider::recycle(std::slice::from_ref(&original)).pop() {
        Some(RecycleOutcome::Recycled(moved)) => moved,
        Some(RecycleOutcome::Failed(error)) => return Err(Stop::Failed(error.kind())),
        Some(RecycleOutcome::NotRecyclable | RecycleOutcome::NotAttempted) | None => {
            return Err(Stop::Failed(FileOpErrorKind::Other));
        }
    };
    match recycle::restore(item) {
        Ok(_) => Ok(()),
        Err(error) => {
            let _ = recycle::restore(&moved);
            Err(Stop::Failed(error.kind()))
        }
    }
}
