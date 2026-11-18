//! **영구 삭제** 작업: 휴지통을 거치지 않고 자체 엔진이 지운다(ADR-0005). 되돌릴 수 없다.
//!
//! 복사와 같은 훑기를 쓴다: 훑기 스레드가 원본을 깊이 우선으로 훑어 항목을 세며 통로에 넣고, 작업 스레드가 이미 센
//! 항목부터 지운다. 그래서 큰 폴더도 다 세기를 기다리지 않고 곧바로 지우기 시작한다.
//!
//! - **폴더**는 안의 항목을 먼저 지운 뒤(훑기의 「폴더 끝」에서) 빈 폴더를 지운다. 빈 폴더만 지우는 연산을 쓰므로
//!   그 사이 밖에서 생긴 항목을 모르고 지우지 않는다.
//! - **읽기 전용** 파일·폴더는 읽기 전용을 풀고 지운다(facet-win의 지우기 연산이 한다).
//! - **심볼릭 링크(파일·폴더)·정션**은 따라 들어가지 않고 링크만 지운다. 가리키던 곳은 건드리지 않는다. 훑기가
//!   링크 안으로 들어가지 않으므로 자기 상위를 가리키는 링크가 있어도 끝난다.
//! - **잠긴 파일·접근 거부** 등은 항목별 오류로 묻는다(다시 시도·건너뛰기·나머지 오류도 건너뛰기·작업 취소).
//!   건너뛴 항목이 든 폴더는 비지 않으므로 지우지 않고 남긴다(그 상위 폴더들도). 그것은 오류로 다시 묻지 않는다.
//! - **없는 항목**(넘긴 원본이 이미 없음, 훑은 뒤 밖에서 지워짐)은 이미 지워진 것으로 보고 묻지 않는다.
//! - **취소**하면 이미 지운 것만 지워져 있다(지운 것을 되살리지 않는다).
//! - 260자를 넘는 경로도 지운다(표준 라이브러리와 facet-win이 확장 길이 경로로 바꿔 부른다).
//!
//! 진행은 항목 수(파일·폴더·링크, 넘긴 원본 자신 포함)와 파일 크기의 합으로 센다. 폴더는 지웠거나 남기기로 한 때
//! (폴더 끝) 한 것으로 센다.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use facet_win::file_ops::{self, FileOpErrorKind, FileOpResult};

use super::copy::{Copier, Entry, ErrorStep, count, pipeline, scan_one};
use super::{Context, DeleteRejection, JournalSink, Summary};
use crate::history::Step;

/// 넣기 전에 확인을 마친 영구 삭제 작업.
pub(super) struct DeleteJob {
    /// 지울 원본(넘긴 차례대로).
    sources: Vec<PathBuf>,
    /// 한 일을 적을 곳(작업 기록에 남길 작업만).
    pub(super) journal: Option<JournalSink>,
}

impl DeleteJob {
    /// 시작 전에 막을 경우를 확인한다.
    pub(super) fn prepare(sources: Vec<PathBuf>) -> Result<Self, DeleteRejection> {
        if sources.is_empty() {
            return Err(DeleteRejection::NoSources);
        }
        Ok(DeleteJob {
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
        let scanned = Deleter::new(&mut engine, self.journal).run(self.sources);
        if scanned {
            totals.counted.store(true, Ordering::Release);
        }
        engine.finish()
    }
}

/// 항목 하나(폴더면 그 아래 전부)를 영구 삭제한다: 휴지통 삭제가 휴지통에 들어가지 않는 항목을 「영구 삭제로 진행」하는
/// 길이다. 규칙은 영구 삭제 작업과 같고 질문·진행·결과 요약은 `engine`의 것이다. 그 항목에서 지운 것이 있으면
/// `journal`에 [`Step::Deleted`]를 적는다. 진행의 항목 수는 이 항목 하나로 친다(안의 항목을 세어도 되돌린다).
/// 취소로 멈췄으면 `engine.cancelled`가 선다.
pub(super) fn delete_one(engine: &mut Copier<'_>, journal: Option<JournalSink>, source: PathBuf) {
    let totals = Arc::clone(&engine.reporter.totals);
    let items_total = totals.items.load(Ordering::Acquire);
    let items_done = engine.reporter.items_done;
    Deleter::new(engine, journal).run(vec![source]);
    totals.items.store(items_total, Ordering::Release);
    engine.reporter.items_done = items_done + 1;
}

/// 넘긴 원본 하나를 다루는 동안의 상태.
struct Root {
    path: PathBuf,
    /// 이 원본(이나 그 안)에서 지운 것이 있다.
    deleted: bool,
}

/// 훑은 항목을 받아 지운다.
struct Deleter<'e, 'a> {
    engine: &'e mut Copier<'a>,
    /// 들어가 있는 폴더(바깥부터): 그 안에 남은 항목이 있는지(건너뛴 항목, 남긴 하위 폴더). 남은 항목이 있는 폴더는
    /// 지우려 하지 않는다.
    open: Vec<bool>,
    /// 지금 다루는 넘긴 원본.
    root: Option<Root>,
    /// 한 일을 적을 곳.
    journal: Option<JournalSink>,
}

impl<'e, 'a> Deleter<'e, 'a> {
    fn new(engine: &'e mut Copier<'a>, journal: Option<JournalSink>) -> Self {
        Deleter {
            engine,
            open: Vec::new(),
            root: None,
            journal,
        }
    }

    /// 원본들을 훑는 동안 이미 센 항목부터 지운다. 다 훑었으면 `true`(취소로 멈췄으면 `false`이고 `cancelled`가
    /// 선다).
    fn run(&mut self, sources: Vec<PathBuf>) -> bool {
        // 훑기는 원본마다 대상 경로를 함께 나르지만 삭제는 대상이 없다. 이름이 없는 원본(드라이브 루트 등)은
        // 지울 수 없어 항목별 오류로 묻는다.
        let plans = sources
            .into_iter()
            .map(|source| {
                let target = source.file_name().map(|_| source.clone());
                (source, target)
            })
            .collect();
        let control = self.engine.context.control;
        let totals = Arc::clone(&self.engine.reporter.totals);
        let scanned = pipeline(plans, &totals, control, |entry| {
            if !control.proceed() {
                self.engine.cancelled = true;
            }
            if self.engine.cancelled {
                return false;
            }
            self.handle(entry);
            true
        });
        if !scanned {
            self.engine.cancelled = true;
        }
        // 취소로 멈춘 원본도 그 안에서 지운 것이 있으면 적는다.
        self.close_root();
        scanned
    }

    fn handle(&mut self, entry: Entry) {
        if self.open.is_empty() && self.root.is_none() {
            let (Entry::File { source, .. }
            | Entry::FolderStart { source, .. }
            | Entry::FolderEnd { source, .. }
            | Entry::Link { source, .. }
            | Entry::Failed { source, .. }) = &entry;
            self.root = Some(Root {
                path: source.clone(),
                deleted: false,
            });
        }
        match entry {
            Entry::File { source, size, .. } => {
                self.engine.reporter.current = Some(source.clone());
                if self.remove(&source, file_ops::remove_file) {
                    self.engine.reporter.bytes_done += size;
                }
            }
            Entry::Link { source, .. } => {
                self.engine.reporter.current = Some(source.clone());
                self.remove(&source, file_ops::remove_link);
            }
            Entry::FolderStart { source, .. } => {
                self.engine.reporter.current = Some(source);
                self.open.push(false);
            }
            Entry::FolderEnd { source, .. } => {
                if self.open.pop().unwrap_or(false) {
                    // 건너뛴 항목이 남아 비지 않은 폴더: 지우지 않고 남기며, 그 상위 폴더도 남는다.
                    self.engine.reporter.items_done += 1;
                    self.keep();
                } else {
                    self.remove(&source, file_ops::remove_empty_folder);
                }
            }
            Entry::Failed {
                source,
                target,
                kind,
            } => self.scan_failed(source, target.is_some(), kind),
        }
        if self.open.is_empty() {
            self.close_root();
        }
        self.engine.reporter.maybe_emit(self.engine.context.control);
    }

    /// 항목 하나를 `remove`로 지운다. 실패하면 항목별 오류로 묻고 답대로 한다. 그 항목을 한 것으로 셌으면(지웠거나
    /// 이미 없거나 건너뜀) `true`, 취소로 그만뒀으면 `false`.
    fn remove(&mut self, path: &Path, remove: fn(&Path) -> FileOpResult<()>) -> bool {
        loop {
            // 도우미의 미리보기 처리기가 잡고 있으면 내리고 한 번 다시 시도한다(#342).
            let kind = match crate::shell_helper::retry_held(path, || remove(path)) {
                Ok(()) => {
                    self.engine.summary.items_deleted += 1;
                    if let Some(root) = &mut self.root {
                        root.deleted = true;
                    }
                    break;
                }
                // 그 사이 밖에서 지워졌다: 이미 지워진 것이다.
                Err(error) if error.kind() == FileOpErrorKind::SourceMissing => break,
                Err(error) => error.kind(),
            };
            match self.engine.on_error(path, kind) {
                ErrorStep::Retry => {}
                ErrorStep::Skip => {
                    self.keep();
                    break;
                }
                ErrorStep::Cancel => return false,
            }
        }
        self.engine.reporter.items_done += 1;
        true
    }

    /// 지금 들어가 있는 폴더에 남는 항목이 있다고 적는다.
    fn keep(&mut self) {
        if let Some(kept) = self.open.last_mut() {
            *kept = true;
        }
    }

    /// 훑지 못한 항목: 없는 항목이면 이미 지워진 것으로 보고, 아니면 묻는다. 다시 시도면 그 항목을 지금 다시 훑어
    /// 지운다. 다시 훑을 수 없는 것(`rescannable`이 아님: 이름이 없는 원본, 목록을 읽다 만 폴더)은 다시 묻는다.
    fn scan_failed(&mut self, source: PathBuf, rescannable: bool, kind: FileOpErrorKind) {
        if rescannable && kind == FileOpErrorKind::SourceMissing {
            self.engine.reporter.items_done += 1;
            return;
        }
        loop {
            match self.engine.on_error(&source, kind) {
                ErrorStep::Skip => {
                    self.engine.reporter.items_done += 1;
                    self.keep();
                    return;
                }
                ErrorStep::Cancel => return,
                ErrorStep::Retry if rescannable => {
                    self.rescan(source);
                    return;
                }
                ErrorStep::Retry => {}
            }
        }
    }

    /// 훑지 못했던 항목 하나를 다시 훑어 다룬다. 앞서 그 항목을 하나로 셌으므로 빼고 다시 센다.
    fn rescan(&mut self, source: PathBuf) {
        let totals = Arc::clone(&self.engine.reporter.totals);
        totals.items.fetch_sub(1, Ordering::AcqRel);
        let control = self.engine.context.control;
        let entries = RefCell::new(Vec::new());
        let send = |entry: Entry| {
            count(&totals, &entry);
            entries.borrow_mut().push(entry);
        };
        let finished = scan_one(source.clone(), Some(source), &send, control);
        for entry in entries.into_inner() {
            if !control.proceed() {
                self.engine.cancelled = true;
            }
            if self.engine.cancelled {
                return;
            }
            self.handle(entry);
        }
        if !finished {
            self.engine.cancelled = true;
        }
    }

    /// 넘긴 원본 하나를 마쳤다(또는 취소로 멈췄다): 그 원본에서 지운 것이 있으면 작업 기록에 적는다. 폴더 안의
    /// 항목은 따로 적지 않는다(되돌릴 수 없는 작업이라 항목별 모습이 필요 없다).
    fn close_root(&mut self) {
        let Some(root) = self.root.take() else {
            return;
        };
        if root.deleted
            && let Some(journal) = &self.journal
        {
            super::lock(journal)
                .steps
                .push(Step::Deleted { path: root.path });
        }
    }
}
