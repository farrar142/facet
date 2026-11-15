//! 실행취소·다시 실행 작업: 작업 기록의 기록 하나를 항목마다 되돌리거나 다시 한다.
//!
//! - **이름바꾸기**: 실행취소는 되바꾸고, 다시 실행은 다시 바꾼다.
//! - **새 폴더**: 실행취소는 비어 있으면 지우고, 다시 실행은 다시 만든다.
//!
//! 하기 전에 밖의 변경을 기록해 둔 모습([`Stamp`])으로 본다: 다룰 항목이 없거나 바뀌었거나, 되바꿀(만들) 자리에
//! 같은 이름이 있거나, 지울 새 폴더가 비어 있지 않으면 그 항목을 건너뛰고 까닭을 [`Summary::not_reverted`]에
//! 담는다. 아무것도 덮어쓰거나 지우지 않는다(이름 바꾸기는 대상이 있으면 실패하고, 폴더는 빈 것만 지운다). 파일
//! 연산 자체의 실패(잠김·접근 거부 등)는 다른 작업과 같은 항목별 오류 질문으로 묻는다.

use std::path::Path;
use std::sync::atomic::Ordering;

use facet_win::file_ops::{self, FileOpErrorKind};

use super::copy::{Copier, ErrorStep};
use super::{Context, Summary};
use crate::history::{NotReverted, Outcomes, Reversal, RevertSkip, Stamp, Step};

/// 넣기 전에 정한 실행취소·다시 실행 작업.
pub(crate) struct RevertJob {
    reversal: Reversal,
    /// 할 차례대로의 항목.
    steps: Vec<Step>,
    /// 항목마다(같은 차례) 결과를 적는 곳.
    outcomes: Outcomes,
}

/// 항목 하나를 한 번 해 본 결과.
enum Attempt {
    /// 했다. 반대 방향에 쓸 새 모습의 항목.
    Done(Step),
    /// 밖의 변경으로 건너뛴다.
    Skip(RevertSkip),
    /// 파일 연산이 실패했다. 항목별 오류로 묻는다.
    Failed(FileOpErrorKind),
}

impl RevertJob {
    pub(crate) fn new(reversal: Reversal, steps: Vec<Step>, outcomes: Outcomes) -> Self {
        if let Ok(mut slots) = outcomes.lock() {
            *slots = vec![None; steps.len()];
        }
        RevertJob {
            reversal,
            steps,
            outcomes,
        }
    }

    /// 항목 수.
    pub(super) fn root_count(&self) -> u64 {
        self.steps.len() as u64
    }

    /// 작업 스레드에서 끝까지(또는 취소될 때까지) 실행한다.
    pub(super) fn run(self, context: &Context<'_>) -> (Summary, bool) {
        let mut copier = Copier::new(context, false);
        let totals = std::sync::Arc::clone(&copier.reporter.totals);
        totals
            .items
            .store(self.steps.len() as u64, Ordering::Release);
        totals.counted.store(true, Ordering::Release);
        for (index, step) in self.steps.iter().enumerate() {
            if !context.control.proceed() {
                copier.cancelled = true;
                break;
            }
            let path = current_path(step, self.reversal);
            copier.reporter.current = Some(path.to_path_buf());
            loop {
                match attempt(step, self.reversal) {
                    Attempt::Done(changed) => {
                        if let Ok(mut slots) = self.outcomes.lock()
                            && let Some(slot) = slots.get_mut(index)
                        {
                            *slot = Some(changed);
                        }
                        break;
                    }
                    Attempt::Skip(reason) => {
                        copier.summary.not_reverted.push(NotReverted {
                            path: path.to_path_buf(),
                            reason,
                        });
                        break;
                    }
                    Attempt::Failed(kind) => match copier.on_error(path, kind) {
                        ErrorStep::Retry => {}
                        ErrorStep::Skip | ErrorStep::Cancel => break,
                    },
                }
            }
            if copier.cancelled {
                break;
            }
            copier.reporter.items_done += 1;
            copier.reporter.maybe_emit(context.control);
        }
        copier.finish()
    }
}

/// 그 방향에서 다룰 항목이 지금 있어야 하는 경로.
fn current_path(step: &Step, reversal: Reversal) -> &Path {
    match (step, reversal) {
        (Step::Renamed { to, .. }, Reversal::Undo) => to,
        (Step::Renamed { from, .. }, Reversal::Redo) => from,
        (Step::CreatedFolder { path, .. }, _) => path,
    }
}

fn attempt(step: &Step, reversal: Reversal) -> Attempt {
    match (step, reversal) {
        (Step::Renamed { from, to, stamp }, Reversal::Undo) => rename_back(to, from, stamp)
            .map_or_else(Into::into, |stamp| {
                Attempt::Done(Step::Renamed {
                    from: from.clone(),
                    to: to.clone(),
                    stamp,
                })
            }),
        (Step::Renamed { from, to, stamp }, Reversal::Redo) => rename_back(from, to, stamp)
            .map_or_else(Into::into, |stamp| {
                Attempt::Done(Step::Renamed {
                    from: from.clone(),
                    to: to.clone(),
                    stamp,
                })
            }),
        (Step::CreatedFolder { path, stamp }, Reversal::Undo) => remove_new_folder(path)
            .map_or_else(Into::into, |()| {
                Attempt::Done(Step::CreatedFolder {
                    path: path.clone(),
                    stamp: *stamp,
                })
            }),
        (Step::CreatedFolder { path, .. }, Reversal::Redo) => {
            create_again(path).map_or_else(Into::into, |stamp| {
                Attempt::Done(Step::CreatedFolder {
                    path: path.clone(),
                    stamp,
                })
            })
        }
    }
}

/// 하지 못한 까닭: 밖의 변경(건너뜀)이나 파일 연산의 실패(물음).
enum Refusal {
    Skip(RevertSkip),
    Failed(FileOpErrorKind),
}

impl From<Refusal> for Attempt {
    fn from(refusal: Refusal) -> Self {
        match refusal {
            Refusal::Skip(reason) => Attempt::Skip(reason),
            Refusal::Failed(kind) => Attempt::Failed(kind),
        }
    }
}

/// 같은 경로인가(Windows는 대소문자를 가리지 않는다): 대소문자만 바꾼 이름바꾸기의 두 경로는 같은 항목이다.
fn same_entry(a: &Path, b: &Path) -> bool {
    super::same_path(a, b)
}

/// `source`(기록한 모습 `stamp`)의 이름을 `target`으로 바꾼다. 바꾼 뒤의 모습을 돌려준다.
fn rename_back(source: &Path, target: &Path, stamp: &Stamp) -> Result<Stamp, Refusal> {
    let now = Stamp::read(source).ok_or(Refusal::Skip(RevertSkip::Missing))?;
    if !stamp.matches(&now) {
        return Err(Refusal::Skip(RevertSkip::Changed));
    }
    if !same_entry(source, target) && std::fs::symlink_metadata(target).is_ok() {
        return Err(Refusal::Skip(RevertSkip::NameTaken));
    }
    match file_ops::rename(source, target) {
        Ok(()) => Ok(Stamp::read(target).unwrap_or(now)),
        Err(error) => Err(match error.kind() {
            FileOpErrorKind::TargetExists => Refusal::Skip(RevertSkip::NameTaken),
            FileOpErrorKind::SourceMissing => Refusal::Skip(RevertSkip::Missing),
            kind => Refusal::Failed(kind),
        }),
    }
}

/// 만든 폴더를 비어 있으면 지운다.
fn remove_new_folder(path: &Path) -> Result<(), Refusal> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| Refusal::Skip(RevertSkip::Missing))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(Refusal::Skip(RevertSkip::Changed));
    }
    let mut listing = std::fs::read_dir(path)
        .map_err(|error| Refusal::Failed(super::copy::error_kind(&error)))?;
    if listing.next().is_some() {
        return Err(Refusal::Skip(RevertSkip::NotEmpty));
    }
    drop(listing);
    match file_ops::remove_empty_folder(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == FileOpErrorKind::SourceMissing => {
            Err(Refusal::Skip(RevertSkip::Missing))
        }
        // 그 사이 안에 무언가 생겼으면 지우지 못한다(빈 폴더만 지운다).
        Err(_) if std::fs::read_dir(path).is_ok_and(|mut l| l.next().is_some()) => {
            Err(Refusal::Skip(RevertSkip::NotEmpty))
        }
        Err(error) => Err(Refusal::Failed(error.kind())),
    }
}

/// 지웠던 새 폴더를 다시 만든다. 그 자리에 무엇이 있으면 건너뛴다.
fn create_again(path: &Path) -> Result<Stamp, Refusal> {
    if std::fs::symlink_metadata(path).is_ok() {
        return Err(Refusal::Skip(RevertSkip::NameTaken));
    }
    match file_ops::create_folder(path) {
        Ok(()) => Stamp::read(path).ok_or(Refusal::Skip(RevertSkip::Missing)),
        Err(error) if error.kind() == FileOpErrorKind::TargetExists => {
            Err(Refusal::Skip(RevertSkip::NameTaken))
        }
        Err(error) => Err(Refusal::Failed(error.kind())),
    }
}
