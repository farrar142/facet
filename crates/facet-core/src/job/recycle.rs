//! **휴지통 삭제** 작업: 넘긴 원본을 셸의 휴지통 보내기([`facet_win::recycle::recycle`])로 휴지통에 보낸다(ADR-0005에서 셸에
//! 맡기는 파일 작업). 작업 스레드에서 부르므로 UI 스레드는 셸을 기다리지 않는다.
//!
//! - 원본을 [`RECYCLE_CHUNK`]개씩 한 번의 셸 호출로 보낸다. **일시정지·취소는 셸 호출 사이**에서만 듣는다: 셸의
//!   파일 작업은 한 번 부르면 중간에 멈출 수 없다(멈추면 시간 초과처럼 셸 스레드를 버려야 한다). 휴지통으로 보내기는
//!   같은 볼륨 안 이름 바꾸기라 묶음 하나는 금방 끝나므로, 묶음을 작게 두어 일시정지·취소가 곧 닿게 한다.
//! - 셸이 휴지통 대신 **영구 삭제하려 한 항목**(휴지통보다 큼, 휴지통이 없는·끈 볼륨)은 셸이 지우지 않고 멈춘다
//!   ([`RecycleOutcome::NotRecyclable`]). 그 항목을 [`JobEvent::NotRecyclable`](super::JobEvent::NotRecyclable)로
//!   묻는다: 영구 삭제로 진행하면 영구 삭제 작업의 엔진([`super::delete::delete_one`])으로 지우고 작업 기록에
//!   [`Step::Deleted`](되돌릴 수 없음)로 적는다. 건너뛰면 남긴다([`Summary::not_recycled`]). 그 뒤 항목은 셸이 손대지
//!   않았으므로 다음 호출로 다시 보낸다.
//! - 셸이 실패를 알린 항목(잠김·접근 거부 등)은 항목별 오류로 묻는다(다시 시도·건너뛰기·나머지 오류도 건너뛰기·작업
//!   취소). 없는 항목은 이미 지워진 것으로 보고 묻지 않는다.
//! - 휴지통에 보낸 항목은 작업 기록에 [`Step::Recycled`](원래 경로·휴지통 항목·보내기 전 모습)로 적는다.

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::SystemTime;

use facet_win::file_ops::FileOpErrorKind;
use facet_win::recycle::RecycleOutcome;

use super::copy::{Copier, ErrorStep, conflict_item};
use super::{Context, DeleteRejection, JournalSink, NotRecyclableAnswer, Summary};
use crate::history::{Stamp, Step};

/// 휴지통 삭제가 셸 한 번에 보내는 원본 수. 일시정지·취소·진행은 이 묶음 사이에서 닿는다.
///
/// 근거: 셸 호출 한 번은 전용 스레드·COM 초기화·셸 작업 객체를 만드는 데 수십 밀리초가 들고, 항목 하나는 같은 볼륨 안
/// 이름 바꾸기라 수 밀리초다. 64개면 호출 비용이 항목 비용과 비슷해지고 묶음 하나가 1초 안팎에 끝나 일시정지·취소가
/// 곧 닿는다.
pub const RECYCLE_CHUNK: usize = 64;

/// 넣기 전에 확인을 마친 휴지통 삭제 작업.
pub(super) struct RecycleJob {
    /// 보낼 원본(넘긴 차례대로).
    sources: Vec<PathBuf>,
    /// 한 일을 적을 곳(작업 기록에 남길 작업만).
    pub(super) journal: Option<JournalSink>,
}

impl RecycleJob {
    /// 시작 전에 막을 경우를 확인한다.
    pub(super) fn prepare(sources: Vec<PathBuf>) -> Result<Self, DeleteRejection> {
        if sources.is_empty() {
            return Err(DeleteRejection::NoSources);
        }
        Ok(RecycleJob {
            sources,
            journal: None,
        })
    }

    /// 넘긴 원본 수.
    pub(super) fn root_count(&self) -> u64 {
        self.sources.len() as u64
    }

    /// 작업 스레드에서 끝까지(또는 취소될 때까지) 실행하고 결과 요약과 취소로 멈췄는지를 돌려준다.
    pub(super) fn run(self, context: &Context<'_>) -> (Summary, bool) {
        // 질문·진행·결과 요약은 복사 엔진의 것을 쓴다.
        let mut engine = Copier::new(context, false);
        let totals = Arc::clone(&engine.reporter.totals);
        totals
            .items
            .store(self.sources.len() as u64, Ordering::Release);
        totals.counted.store(true, Ordering::Release);
        Recycler {
            engine: &mut engine,
            journal: self.journal,
            remembered: None,
            released: HashSet::new(),
        }
        .run(self.sources);
        engine.finish()
    }
}

/// 원본을 휴지통으로 보낸다.
struct Recycler<'e, 'a> {
    engine: &'e mut Copier<'a>,
    journal: Option<JournalSink>,
    /// 「나머지에도 적용」으로 받은 휴지통에 들어가지 않는 항목의 답.
    remembered: Option<NotRecyclableAnswer>,
    /// 도우미의 미리보기를 내리고 다시 보낸 항목(#342, 한 번만).
    released: HashSet<PathBuf>,
}

impl Recycler<'_, '_> {
    fn run(&mut self, sources: Vec<PathBuf>) {
        let control = self.engine.context.control;
        let mut pending: VecDeque<PathBuf> = sources.into();
        while !pending.is_empty() {
            if !control.proceed() {
                self.engine.cancelled = true;
                return;
            }
            let chunk: Vec<PathBuf> = pending.drain(..pending.len().min(RECYCLE_CHUNK)).collect();
            // 보내기 전의 모습: 다시 실행이 「그 뒤 바뀌지 않았는지」 보는 데 쓴다.
            let stamps: Vec<Option<Stamp>> = chunk.iter().map(|path| Stamp::read(path)).collect();
            self.engine.reporter.current = chunk.first().cloned();
            let results = crate::shell_provider::recycle(&chunk);
            let mut again = Vec::new();
            let mut settled = false;
            for (position, (path, stamp)) in chunk.into_iter().zip(stamps).enumerate() {
                let outcome = results.get(position);
                // 앞 항목의 질문에서 취소했어도 셸이 이미 휴지통에 보낸 뒤 항목은 적는다(실행취소로 되살리게).
                if self.engine.cancelled && !matches!(outcome, Some(RecycleOutcome::Recycled(_))) {
                    continue;
                }
                match outcome {
                    Some(RecycleOutcome::Recycled(item)) => {
                        settled = true;
                        let stamp =
                            stamp
                                .or_else(|| Stamp::read(item.recycled()))
                                .unwrap_or(Stamp {
                                    is_folder: false,
                                    size: 0,
                                    modified: SystemTime::UNIX_EPOCH,
                                });
                        self.note(Step::Recycled {
                            path,
                            item: item.clone(),
                            stamp,
                        });
                        self.engine.summary.recycled.push(item.clone());
                        self.engine.reporter.items_done += 1;
                    }
                    Some(RecycleOutcome::NotRecyclable) => {
                        settled = true;
                        self.not_recyclable(path);
                    }
                    // 이미 없다: 지워진 것이다.
                    Some(RecycleOutcome::Failed(error))
                        if error.kind() == FileOpErrorKind::SourceMissing =>
                    {
                        settled = true;
                        self.engine.reporter.items_done += 1;
                    }
                    // 도우미의 미리보기 처리기가 잡고 있다: 내리고 한 번 다시 보낸다(#342).
                    Some(RecycleOutcome::Failed(error))
                        if error.kind() == FileOpErrorKind::Locked
                            && !self.released.contains(&path)
                            && crate::shell_helper::release_held_preview(&path) =>
                    {
                        settled = true;
                        self.released.insert(path.clone());
                        again.push(path);
                    }
                    Some(RecycleOutcome::Failed(error)) => {
                        settled = true;
                        match self.engine.on_error(&path, error.kind()) {
                            ErrorStep::Retry => again.push(path),
                            ErrorStep::Skip => self.engine.reporter.items_done += 1,
                            ErrorStep::Cancel => {}
                        }
                    }
                    // 앞 항목에서 셸이 멈춰 손대지 않았다: 다음 호출로 다시 보낸다.
                    Some(RecycleOutcome::NotAttempted) | None => again.push(path),
                }
            }
            if self.engine.cancelled {
                return;
            }
            if !settled && let Some(first) = again.first().cloned() {
                // 셸이 아무 항목도 다루지 않았다: 같은 호출을 되풀이하지 않게 첫 항목을 오류로 묻는다.
                match self.engine.on_error(&first, FileOpErrorKind::Other) {
                    ErrorStep::Retry => {}
                    ErrorStep::Skip => {
                        again.remove(0);
                        self.engine.reporter.items_done += 1;
                    }
                    ErrorStep::Cancel => return,
                }
            }
            for path in again.into_iter().rev() {
                pending.push_front(path);
            }
            self.engine.reporter.maybe_emit(control);
        }
    }

    /// 휴지통에 들어가지 않는 항목: 묻고(「나머지에도 적용」 뒤면 묻지 않고) 답대로 영구 삭제하거나 남긴다.
    fn not_recyclable(&mut self, path: PathBuf) {
        let answer = match self.remembered {
            Some(answer) => answer,
            None => match self.engine.context.ask_not_recyclable(conflict_item(&path)) {
                Some(answer) => answer,
                None => {
                    self.engine.cancelled = true;
                    return;
                }
            },
        };
        match answer {
            NotRecyclableAnswer::DeletePermanently | NotRecyclableAnswer::DeleteAllPermanently => {
                if answer == NotRecyclableAnswer::DeleteAllPermanently {
                    self.remembered = Some(answer);
                }
                self.engine.reporter.current = Some(path.clone());
                super::delete::delete_one(self.engine, self.journal.clone(), path);
            }
            NotRecyclableAnswer::Skip | NotRecyclableAnswer::SkipAll => {
                if answer == NotRecyclableAnswer::SkipAll {
                    self.remembered = Some(answer);
                }
                self.engine.summary.not_recycled.push(path);
                self.engine.reporter.items_done += 1;
            }
            NotRecyclableAnswer::CancelJob => self.engine.cancelled = true,
        }
    }

    /// 작업 기록에 적는다.
    fn note(&self, step: Step) {
        if let Some(journal) = &self.journal {
            super::lock(journal).steps.push(step);
        }
    }
}
