//! 이동 작업: 항목마다 먼저 이름 바꾸기를 해 보고, 다른 볼륨이면 복사 뒤 원본 삭제로 바꾼다.
//!
//! - **같은 볼륨**: 이름 바꾸기 한 번이다. 큰 폴더도 곧바로 끝나고 파일 ID·보안 설명자가 그대로 남는다. 진행은
//!   항목 수로만 센다(폴더 하나를 옮겨도 항목 하나, 바이트는 늘지 않는다).
//! - **다른 볼륨**(이름 바꾸기가 [`FileOpErrorKind::CrossVolume`]): 그 항목을 복사 작업과 같은 훑기·복사로 옮기되,
//!   파일은 복사를 마친 뒤 원본을 지우고, 폴더는 안의 항목을 다 옮긴 뒤 빈 폴더일 때만 지운다. 그래서 실패·건너뛰기·
//!   취소한 항목의 원본과 그 상위 폴더는 남는다. 바이트 진행은 이렇게 복사로 바뀐 항목만 센다.
//! - **이름 충돌**: 같은 볼륨이어도 대상에 같은 이름이 있으면 묻는다. 폴더끼리는 묻지 않고 합친다: 안의 항목을
//!   하나씩 (같은 규칙으로) 옮기고 빈 원본 폴더를 지운다. 합친 폴더 안의 항목 수는 합칠 때 센다.
//! - **같은 폴더로 이동**(원본이 대상 폴더 바로 안에 있음)은 아무것도 하지 않고 처리한 항목으로만 센다.
//!
//! 항목 수는 넘긴 원본 수에서 시작해 합치거나 복사로 바꾼 폴더를 훑으며 늘어나므로, 작업이 끝날 때까지 다 셌다고
//! 하지 않는다(남은 시간은 「계산 중」).

use std::fs;
use std::io;
use std::path::Path;
use std::sync::atomic::Ordering;

use facet_win::file_ops::{self, FileOpErrorKind};

use super::copy::{Copier, CopyJob, ErrorStep, error_kind, free_numbered, temp_path};
use super::{Context, CopyRejection, Resolution, Summary};

/// 넣기 전에 확인을 마친 이동 작업. 시작 전에 막는 경우는 복사와 같다.
pub(super) struct MoveJob {
    plan: CopyJob,
}

/// 항목 하나를 한 번 해 본 결과.
enum Outcome {
    /// 끝났다(이름을 바꿨거나, 충돌로 건너뛰었다). 항목 하나를 한 것으로 센다.
    Done,
    /// 다른 길(복사, 합치기)이 그 항목을 이미 세며 다뤘다(취소로 멈췄으면 `cancelled`가 서 있다).
    Handled,
    /// 취소로 그만뒀다.
    Cancelled,
    /// 실패했다. 항목별 오류로 묻는다.
    Failed(FileOpErrorKind),
}

impl MoveJob {
    pub(super) fn prepare(
        sources: Vec<std::path::PathBuf>,
        target: std::path::PathBuf,
    ) -> Result<Self, CopyRejection> {
        Ok(MoveJob {
            plan: CopyJob::prepare(sources, target)?,
        })
    }

    /// 넘긴 원본 수.
    pub(super) fn root_count(&self) -> u64 {
        self.plan.root_count()
    }

    /// 작업 스레드에서 끝까지(또는 취소될 때까지) 실행하고 결과 요약과 취소로 멈췄는지를 돌려준다.
    pub(super) fn run(self, context: &Context<'_>) -> (Summary, bool) {
        let mut mover = Mover {
            copier: Copier::new(context, true),
        };
        let totals = std::sync::Arc::clone(&mover.copier.reporter.totals);
        totals
            .items
            .store(self.plan.roots.len() as u64, Ordering::Release);
        for root in &self.plan.roots {
            if !mover.proceed() {
                break;
            }
            if root.same_folder {
                // 같은 폴더로 이동: 아무것도 하지 않는다.
                mover.copier.reporter.items_done += 1;
                continue;
            }
            match root.source.file_name() {
                Some(name) => mover.move_item(&root.source, &self.plan.target.join(name)),
                // 이름이 없는 원본(드라이브 루트 등)은 옮길 수 없다.
                None => mover.fail_without_retry(&root.source),
            }
            mover.copier.reporter.maybe_emit();
        }
        if !mover.copier.cancelled {
            totals.counted.store(true, Ordering::Release);
        }
        mover.copier.finish()
    }
}

/// 이동을 하는 쪽. 복사로 바꾼 항목과 질문·진행은 [`Copier`](이동 모드)가 한다.
struct Mover<'a> {
    copier: Copier<'a>,
}

impl Mover<'_> {
    /// 일시정지면 기다린다. 계속해도 되면 `true`, 취소됐으면 `cancelled`를 세우고 `false`.
    fn proceed(&mut self) -> bool {
        if !self.copier.context.control.proceed() {
            self.copier.cancelled = true;
        }
        !self.copier.cancelled
    }

    /// 다시 해 볼 수 없는 항목: 묻되 다시 시도도 건너뛰기로 친다.
    fn fail_without_retry(&mut self, source: &Path) {
        match self.copier.on_error(source, FileOpErrorKind::Other) {
            ErrorStep::Retry | ErrorStep::Skip => self.copier.reporter.items_done += 1,
            ErrorStep::Cancel => {}
        }
    }

    /// 항목 하나(폴더면 그 아래 전부)를 `target`으로 옮긴다. 실패하면 항목별 오류로 묻고 답대로 한다.
    fn move_item(&mut self, source: &Path, target: &Path) {
        loop {
            if !self.proceed() {
                return;
            }
            self.copier.reporter.current = Some(source.to_path_buf());
            let kind = match self.try_move(source, target) {
                Outcome::Done => {
                    self.copier.reporter.items_done += 1;
                    return;
                }
                Outcome::Handled => return,
                Outcome::Cancelled => {
                    self.copier.cancelled = true;
                    return;
                }
                Outcome::Failed(kind) => kind,
            };
            match self.copier.on_error(source, kind) {
                ErrorStep::Retry => {}
                ErrorStep::Skip => {
                    self.copier.reporter.items_done += 1;
                    return;
                }
                ErrorStep::Cancel => return,
            }
        }
    }

    fn try_move(&mut self, source: &Path, target: &Path) -> Outcome {
        let Ok(target_metadata) = fs::symlink_metadata(target) else {
            return self.rename_or_copy(source, target);
        };
        let source_metadata = match fs::symlink_metadata(source) {
            Ok(metadata) => metadata,
            Err(error) => return Outcome::Failed(error_kind(&error)),
        };
        let real_folder = |m: &fs::Metadata| m.is_dir() && !m.file_type().is_symlink();
        if real_folder(&source_metadata) && real_folder(&target_metadata) {
            return self.merge(source, target);
        }
        // 링크는 가리키는 쪽의 종류로 본다(복사의 이름 충돌과 같다).
        let is_folder = |path: &Path, m: &fs::Metadata| {
            m.is_dir() || fs::metadata(path).is_ok_and(|m| m.is_dir())
        };
        let source_is_folder = is_folder(source, &source_metadata);
        let target_is_folder = is_folder(target, &target_metadata);
        let can_overwrite = !source_is_folder && !target_is_folder;
        let Some(resolution) = self.copier.resolve(source, target, can_overwrite) else {
            return Outcome::Cancelled;
        };
        match resolution {
            Resolution::Skip => self.skip_conflict(),
            Resolution::KeepBoth => {
                let renamed = free_numbered(target, !source_is_folder);
                self.rename_or_copy(source, &renamed)
            }
            Resolution::Overwrite => self.overwrite(source, target),
            Resolution::OverwriteIfNewer => {
                let modified = |path: &Path| fs::metadata(path).and_then(|m| m.modified()).ok();
                match (modified(source), modified(target)) {
                    (Some(source_time), Some(target_time)) if source_time > target_time => {
                        self.overwrite(source, target)
                    }
                    _ => self.skip_conflict(),
                }
            }
        }
    }

    fn skip_conflict(&mut self) -> Outcome {
        self.copier.summary.conflicts_skipped += 1;
        Outcome::Done
    }

    /// 없는 이름 `target`으로 옮긴다: 이름 바꾸기, 다른 볼륨이면 복사 뒤 원본 삭제.
    fn rename_or_copy(&mut self, source: &Path, target: &Path) -> Outcome {
        match file_ops::rename(source, target) {
            Ok(()) => {
                self.copier.summary.items_renamed += 1;
                Outcome::Done
            }
            Err(error) if error.kind() == FileOpErrorKind::CrossVolume => {
                self.copy_across(source, target, None)
            }
            Err(error) => Outcome::Failed(error.kind()),
        }
    }

    /// 덮어쓰기: 같은 볼륨이면 원본을 대상 폴더 안 임시 이름으로 옮긴 뒤 바꿔치기한다(실패하면 원본 자리로
    /// 되돌린다). 다른 볼륨이면 복사의 덮어쓰기(임시 이름에 다 쓴 뒤 바꿔치기) 뒤 원본을 지운다.
    fn overwrite(&mut self, source: &Path, target: &Path) -> Outcome {
        let temp = temp_path(target);
        match file_ops::rename(source, &temp) {
            Ok(()) => match file_ops::replace(&temp, target) {
                Ok(()) => {
                    self.copier.summary.items_renamed += 1;
                    self.copier.summary.files_overwritten += 1;
                    Outcome::Done
                }
                Err(error) => {
                    // 원본을 제자리로 돌려 놓는다. 못 돌리면 임시 이름으로라도 남는다(지우지 않는다).
                    let _ = file_ops::rename(&temp, source);
                    Outcome::Failed(error.kind())
                }
            },
            Err(error) if error.kind() == FileOpErrorKind::CrossVolume => {
                self.copy_across(source, target, Some(Resolution::Overwrite))
            }
            Err(error) => Outcome::Failed(error.kind()),
        }
    }

    /// 다른 볼륨으로: 복사 작업과 같은 훑기·복사로 옮기고 다 옮긴 원본을 지운다. `answer`는 이미 받은 이름 충돌의
    /// 답(대상에 같은 이름이 있을 때 다시 묻지 않게).
    fn copy_across(&mut self, source: &Path, target: &Path, answer: Option<Resolution>) -> Outcome {
        let metadata = match fs::symlink_metadata(source) {
            Ok(metadata) => metadata,
            Err(error) => return Outcome::Failed(error_kind(&error)),
        };
        let totals = std::sync::Arc::clone(&self.copier.reporter.totals);
        if metadata.is_file() {
            totals.bytes.fetch_add(metadata.len(), Ordering::AcqRel);
            self.copier.preset = answer;
            let done = self.copier.copy_file(source, target, metadata.len());
            self.copier.preset = None;
            if done {
                self.copier.reporter.items_done += 1;
            }
        } else {
            // 폴더(와 따라 들어가지 않는 링크)는 훑기가 그 항목부터 다시 센다.
            totals.items.fetch_sub(1, Ordering::AcqRel);
            self.copier.preset = answer;
            self.copier
                .run_pipeline(vec![(source.to_path_buf(), Some(target.to_path_buf()))]);
            self.copier.preset = None;
        }
        Outcome::Handled
    }

    /// 같은 이름의 폴더끼리 합친다: 안의 항목을 하나씩 옮기고, 다 옮겨 빈 원본 폴더를 지운다(건너뛰거나 실패한
    /// 항목이 남았으면 그대로 둔다). 합친 폴더의 시각·속성은 대상 것 그대로다.
    fn merge(&mut self, source: &Path, target: &Path) -> Outcome {
        let children = match fs::read_dir(source).and_then(|listing| {
            listing
                .map(|entry| entry.map(|e| e.file_name()))
                .collect::<io::Result<Vec<_>>>()
        }) {
            Ok(children) => children,
            Err(error) => return Outcome::Failed(error_kind(&error)),
        };
        self.copier.summary.folders_merged += 1;
        self.copier.reporter.items_done += 1;
        self.copier
            .reporter
            .totals
            .items
            .fetch_add(children.len() as u64, Ordering::AcqRel);
        for name in children {
            self.move_item(&source.join(&name), &target.join(&name));
            if self.copier.cancelled {
                return Outcome::Handled;
            }
            self.copier.reporter.maybe_emit();
        }
        let _ = file_ops::remove_empty_folder(source);
        Outcome::Handled
    }
}
