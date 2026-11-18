//! 휴대폰(MTP)과 PC 사이의 **복사·이동·삭제** 작업(#357, spec #27의 사용자 이야기 17·18·19·22).
//!
//! 휴대폰 항목은 실제 파일 시스템 경로가 없어 휴대폰 항목 경로([`crate::shell_provider::DEVICE_PATH_ROOT`] 아래의
//! 표시 이름 경로)로 넘어오고, 셸 공급자가 적어 둔 표([`crate::shell_provider::device_item`])로 셸 항목을 찾는다. 셸
//! 호출([`facet_win::shell_transfer`])은 이 작업의 전용 COM STA 스레드에서 한다: 휴대폰 탭의 목록 읽기(장치의 셸
//! 스레드)가 큰 파일 전송 뒤에서 기다리다 「응답 없음」이 되지 않게 따로 둔다.
//!
//! - **가져오기**(휴대폰 → PC): 파일은 스트림으로 덩어리씩 읽어 PC 파일에 쓴다(덩어리마다 진행, 취소하면 만들다 만
//!   파일을 지운다). 수정한 날짜는 장치가 준 값으로 맞춘다. 폴더는 PC에 만들고 그 안을 열거해 차례로 가져온다.
//! - **넣기**(PC → 휴대폰): 파일은 셸의 `IFileOperation`으로 넣고(파일 하나를 넣는 동안은 진행이 멈춰 보인다), 폴더는
//!   휴대폰에 만든 뒤 PC 폴더 안을 차례로 넣는다.
//! - **이동**은 「복사 뒤 원본 삭제」다. 휴대폰 쪽 원본은 휴지통이 없어 영구 삭제다.
//! - **삭제**(휴대폰 항목)는 휴지통을 거치지 않는 영구 삭제다(폴더는 셸 호출 하나로 안까지).
//! - 진행·이름 충돌·항목별 오류(다시 시도·건너뛰기·나머지 오류도 건너뛰기·작업 취소)는 다른 작업과 같다. 일시정지는
//!   **항목 사이**에서만 듣는다(셸 호출 하나는 멈출 수 없다). 취소는 가져오는 파일이면 덩어리 사이에서 듣는다.
//! - 이름 충돌: 휴대폰 쪽 대상은 휴지통이 없어 덮어쓰면 되살릴 수 없다(`overwrite_is_permanent`). 장치가 날짜를 주지
//!   않으면 「더 새것만 덮어쓰기」를 고를 수 없다([`super::Conflict::can_overwrite_if_newer`]).
//! - **다시 꽂고 다시 시도**: 장치가 빠졌다 다시 꽂히면 셸 항목 식별자가 바뀔 수 있다. 다시 시도하면 그 항목이 든 휴대폰
//!   폴더를 처음 다룰 때 적어 둔 해석 이름으로 같은 장치에 다시 이어(셸 공급자의 다시 잇기와 같은 규칙) 그 항목부터 이어 간다.
//! - 작업 기록: 휴대폰에서 PC로 가져온 복사는 만든 PC 항목마다 [`Step::Copied`]로 남아 실행취소(휴지통으로 보내기)가 된다.
//!   휴대폰 쪽을 바꾼 작업(넣기, 휴대폰 원본을 지운 이동, 삭제)은 되돌릴 수 없다고 남는다.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant, SystemTime};

use facet_win::file_ops::{self, FileOpErrorKind};
use facet_win::shell_folder;
use facet_win::shell_thread::{CallError, ShellThread};
use facet_win::shell_transfer::{self, ShellChild};

use super::copy::{
    Copier, ErrorStep, FileOutcome, Replaced, conflict_item, free_numbered, temp_path,
};
use super::{
    Conflict, ConflictItem, Context, CopyRejection, DeleteRejection, ItemKind, JournalSink,
    PROGRESS_INTERVAL, Resolution, Summary,
};
use crate::history::{NotUndoable, Stamp, Step};
use crate::shell_provider::{self, DeviceItem};

/// 셸 호출 하나(파일 하나의 전송·폴더 열거·삭제)의 시간 제한. 느린 휴대폰(MTP는 초당 수십 MB)으로 수 GB 동영상 하나를
/// 옮기는 데도 넉넉하다. 멈춘 장치에서도 사용자는 언제든 취소할 수 있다([`CANCEL_GRACE`]).
pub const DEVICE_CALL_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// 취소한 뒤 셸 호출이 돌아오기를 기다리는 시간. 넘으면 그 호출을 버리고(셸 스레드는 끝나면 스스로 끝난다) 취소로 끝낸다.
const CANCEL_GRACE: Duration = Duration::from_secs(2);

/// 작업 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DeviceOp {
    Copy,
    Move,
    Delete,
}

/// 넘긴 원본 하나.
enum Source {
    /// PC 항목.
    Pc(PathBuf),
    /// 휴대폰 항목: 항목 경로와 셸 항목(표에 없으면 `None`: 「원본 없음」으로 묻는다).
    Device {
        label: PathBuf,
        item: Option<DeviceItem>,
    },
}

/// 대상 폴더.
enum Target {
    Pc(PathBuf),
    Device { label: PathBuf, id: Vec<u8> },
}

/// 넣기 전에 확인을 마친 휴대폰 작업.
pub(super) struct DeviceJob {
    op: DeviceOp,
    sources: Vec<Source>,
    target: Option<Target>,
    /// 한 일을 적을 곳(작업 기록에 남길 작업만).
    pub(super) journal: Option<JournalSink>,
}

/// 이 원본·대상이 휴대폰 작업인가(휴대폰 항목 경로가 하나라도 있다).
pub(super) fn involves_device(sources: &[PathBuf], target: Option<&Path>) -> bool {
    sources
        .iter()
        .any(|path| shell_provider::is_device_path(path))
        || target.is_some_and(shell_provider::is_device_path)
}

impl DeviceJob {
    /// 복사·이동: 원본과 대상 가운데 한쪽만 휴대폰이어야 한다(휴대폰 안·휴대폰끼리는 [`CopyRejection::Unsupported`]).
    pub(super) fn prepare_transfer(
        op: DeviceOp,
        sources: Vec<PathBuf>,
        target: PathBuf,
    ) -> Result<Self, CopyRejection> {
        if sources.is_empty() {
            return Err(CopyRejection::NoSources);
        }
        let target = if shell_provider::is_device_path(&target) {
            let item = shell_provider::device_item(&target).ok_or(CopyRejection::TargetMissing)?;
            if !item.is_folder {
                return Err(CopyRejection::TargetNotFolder);
            }
            Target::Device {
                label: target,
                id: item.id,
            }
        } else {
            let metadata = fs::metadata(&target).map_err(|_| CopyRejection::TargetMissing)?;
            if !metadata.is_dir() {
                return Err(CopyRejection::TargetNotFolder);
            }
            Target::Pc(target)
        };
        let to_device = matches!(target, Target::Device { .. });
        let sources: Vec<Source> = sources.into_iter().map(source_of).collect();
        let mixed = sources
            .iter()
            .any(|source| matches!(source, Source::Device { .. }) == to_device);
        if mixed {
            return Err(CopyRejection::Unsupported);
        }
        Ok(DeviceJob {
            op,
            sources,
            target: Some(target),
            journal: None,
        })
    }

    /// 삭제: 휴대폰 항목은 영구 삭제하고, 섞인 PC 항목은 영구 삭제 엔진으로 지운다.
    pub(super) fn prepare_delete(sources: Vec<PathBuf>) -> Result<Self, DeleteRejection> {
        if sources.is_empty() {
            return Err(DeleteRejection::NoSources);
        }
        Ok(DeviceJob {
            op: DeviceOp::Delete,
            sources: sources.into_iter().map(source_of).collect(),
            target: None,
            journal: None,
        })
    }

    /// 넘긴 원본 수.
    pub(super) fn root_count(&self) -> u64 {
        self.sources.len() as u64
    }

    /// 작업 스레드에서 끝까지(또는 취소될 때까지) 실행한다.
    pub(super) fn run(self, context: &Context<'_>) -> (Summary, bool) {
        let mut engine = Copier::new(context, false);
        // 셸 호출이 하나라도 되돌릴 수 없는 일을 하므로(휴대폰 쪽) 엔진의 기록은 가져온 PC 항목만 적는다.
        let from_device = matches!(self.target, Some(Target::Pc(_)));
        if from_device {
            engine.journal = self.journal.clone();
        }
        let mut runner = Runner {
            engine,
            shell: Arc::new(ShellThread::new()),
            journal: self.journal,
            moving: self.op == DeviceOp::Move,
        };
        runner.count_roots(&self.sources);
        match (self.op, self.target) {
            (DeviceOp::Delete, _) => runner.delete_all(self.sources),
            (_, Some(Target::Pc(target))) => runner.pull_all(self.sources, &target),
            (_, Some(Target::Device { label, id })) => runner.push_all(self.sources, label, id),
            (_, None) => {}
        }
        runner.finish()
    }
}

fn source_of(path: PathBuf) -> Source {
    if shell_provider::is_device_path(&path) {
        let item = shell_provider::device_item(&path);
        Source::Device { label: path, item }
    } else {
        Source::Pc(path)
    }
}

/// 휴대폰 폴더 하나: 항목 경로, 지금 식별자, 다시 꽂힌 장치에 다시 잇는 해석 이름, 읽어 둔 항목들.
struct DeviceFolder {
    label: PathBuf,
    id: Vec<u8>,
    parsing: Option<String>,
    listing: Option<Vec<ShellChild>>,
}

impl DeviceFolder {
    fn new(label: PathBuf, id: Vec<u8>) -> Self {
        DeviceFolder {
            label,
            id,
            parsing: None,
            listing: None,
        }
    }
}

/// 셸 호출 결과.
type Shell<T> = Result<T, FileOpErrorKind>;

/// 한 파일을 해 본 결과.
enum Done {
    /// 했다: 실제 이름(PC면 경로)과 덮어썼는지.
    Copied {
        at: PathBuf,
        replaced: Replaced,
    },
    Skipped,
    Cancelled,
    Failed(FileOpErrorKind),
}

struct Runner<'a> {
    engine: Copier<'a>,
    shell: Arc<ShellThread>,
    journal: Option<JournalSink>,
    moving: bool,
}

impl<'a> Runner<'a> {
    /// 넘긴 원본을 센다. 원본이 모두 파일이면 처음부터 다 센 것이다(폴더는 들어갈 때 안을 더 센다).
    fn count_roots(&mut self, sources: &[Source]) {
        let totals = &self.engine.reporter.totals;
        let mut bytes = 0;
        let mut all_files = true;
        for source in sources {
            match source {
                Source::Pc(path) => match fs::symlink_metadata(path) {
                    Ok(metadata) if metadata.is_dir() => all_files = false,
                    Ok(metadata) => bytes += metadata.len(),
                    Err(_) => {}
                },
                Source::Device { item, .. } => match item {
                    Some(item) if item.is_folder => all_files = false,
                    Some(item) => bytes += item.size.unwrap_or(0),
                    None => {}
                },
            }
        }
        totals
            .items
            .fetch_add(sources.len() as u64, Ordering::AcqRel);
        totals.bytes.fetch_add(bytes, Ordering::AcqRel);
        if all_files {
            totals.counted.store(true, Ordering::Release);
        }
    }

    fn finish(self) -> (Summary, bool) {
        let engine = self.engine;
        if !engine.cancelled {
            engine
                .reporter
                .totals
                .counted
                .store(true, Ordering::Release);
        }
        engine.finish()
    }

    /// 다음 항목으로 가도 되는가(일시정지면 기다린다). 취소면 `false`.
    fn proceed(&mut self) -> bool {
        if self.engine.cancelled || !self.engine.context.control.proceed() {
            self.engine.cancelled = true;
            return false;
        }
        true
    }

    fn note(&self, step: Step, not_undoable: NotUndoable) {
        if let Some(journal) = &self.journal {
            let mut journal = super::lock(journal);
            journal.steps.push(step);
            journal.not_undoable.get_or_insert(not_undoable);
        }
    }

    fn mark_not_undoable(&self, reason: NotUndoable) {
        if let Some(journal) = &self.journal {
            super::lock(journal).not_undoable.get_or_insert(reason);
        }
    }

    /// 셸 호출 하나를 전송 스레드에서 하고, 기다리는 동안 진행을 알리고 취소를 전한다(가져오기의 덩어리 콜백이
    /// `cancel`을 본다). 지금까지 옮긴 바이트를 `bytes`에 적으면 진행의 바이트가 `base`에 그만큼 더해진다.
    fn call<T: Send + 'static>(
        &mut self,
        job: impl FnOnce(&AtomicBool, &AtomicU64) -> Shell<T> + Send + 'static,
    ) -> Shell<T> {
        let cancel = Arc::new(AtomicBool::new(false));
        let bytes = Arc::new(AtomicU64::new(0));
        let (sender, receiver) = mpsc::channel();
        let shell = Arc::clone(&self.shell);
        let (job_cancel, job_bytes) = (Arc::clone(&cancel), Arc::clone(&bytes));
        let spawned = std::thread::Builder::new()
            .name("facet-device-call".into())
            .spawn(move || {
                let result = shell.call(DEVICE_CALL_TIMEOUT, move || job(&job_cancel, &job_bytes));
                let _ = sender.send(result);
            });
        if spawned.is_err() {
            return Err(FileOpErrorKind::Other);
        }
        let base = self.engine.reporter.bytes_done;
        let mut cancelled_at: Option<Instant> = None;
        loop {
            match receiver.recv_timeout(PROGRESS_INTERVAL) {
                Ok(result) => {
                    self.engine.reporter.bytes_done = base;
                    return match result {
                        Ok(result) => result,
                        Err(CallError::TimedOut) => Err(FileOpErrorKind::TimedOut),
                        Err(CallError::Failed) => Err(FileOpErrorKind::Other),
                    };
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.engine.reporter.bytes_done = base + bytes.load(Ordering::Relaxed);
                    self.engine.reporter.maybe_emit(self.engine.context.control);
                    if self.engine.context.control.is_cancelled() {
                        cancel.store(true, Ordering::Relaxed);
                        let at = *cancelled_at.get_or_insert_with(Instant::now);
                        if at.elapsed() >= CANCEL_GRACE {
                            self.engine.reporter.bytes_done = base;
                            return Err(FileOpErrorKind::Cancelled);
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.engine.reporter.bytes_done = base;
                    return Err(FileOpErrorKind::Other);
                }
            }
        }
    }

    /// 휴대폰 항목의 해석 이름(다시 꽂힌 장치에 다시 잇는 데 쓴다). 얻지 못하면 `None`.
    fn parsing_name(&mut self, id: &[u8]) -> Option<String> {
        let id = id.to_vec();
        self.call(move |_, _| shell_folder::parsing_name(&id).map_err(|_| FileOpErrorKind::Other))
            .ok()
    }

    /// 휴대폰 폴더를 다시 꽂힌 같은 장치에 다시 잇는다(식별자가 바뀌었으면 바꾸고 읽어 둔 항목을 버린다).
    fn relink(&mut self, folder: &mut DeviceFolder) {
        folder.listing = None;
        if let Some(parsing) = &folder.parsing
            && let Some(id) = shell_provider::relink_device_item(parsing)
        {
            folder.id = id;
        }
    }

    /// 휴대폰 폴더의 항목들(읽어 둔 것이 있으면 그것).
    fn listing(&mut self, folder: &mut DeviceFolder) -> Shell<Vec<ShellChild>> {
        if let Some(listing) = &folder.listing {
            return Ok(listing.clone());
        }
        if folder.parsing.is_none() {
            folder.parsing = self.parsing_name(&folder.id);
        }
        let id = folder.id.clone();
        let listing = self.call(move |_, _| shell_transfer::children(&id).map_err(|e| e.kind()))?;
        folder.listing = Some(listing.clone());
        Ok(listing)
    }

    /// 항목별 오류를 묻고 답을 따른다(엔진과 같다).
    fn on_error(&mut self, label: &Path, kind: FileOpErrorKind) -> ErrorStep {
        self.engine.on_error(label, kind)
    }

    fn add_totals(&mut self, items: u64, bytes: u64) {
        let totals = &self.engine.reporter.totals;
        totals.items.fetch_add(items, Ordering::AcqRel);
        totals.bytes.fetch_add(bytes, Ordering::AcqRel);
    }

    fn item_done(&mut self) {
        self.engine.reporter.items_done += 1;
        self.engine.reporter.maybe_emit(self.engine.context.control);
    }

    // --- 삭제 ---

    fn delete_all(&mut self, sources: Vec<Source>) {
        for source in sources {
            if !self.proceed() {
                return;
            }
            match source {
                Source::Pc(path) => {
                    super::delete::delete_one(&mut self.engine, self.journal.clone(), path);
                }
                Source::Device { label, item } => self.delete_device(label, item),
            }
        }
    }

    fn delete_device(&mut self, label: PathBuf, item: Option<DeviceItem>) {
        self.engine.reporter.current = Some(label.clone());
        let Some(item) = item else {
            // 표에 없는 항목: 이미 없는 것으로 본다(삭제는 없는 항목을 묻지 않는다).
            self.item_done();
            return;
        };
        let mut folder = DeviceFolder::new(label.clone(), item.id.clone());
        folder.parsing = self.parsing_name(&item.id);
        loop {
            let id = folder.id.clone();
            match self.call(move |_, _| shell_transfer::delete(&id).map_err(|e| e.kind())) {
                Ok(()) => {
                    self.engine.summary.items_deleted += 1;
                    self.note(
                        Step::Deleted {
                            path: label.clone(),
                        },
                        NotUndoable::PermanentDelete,
                    );
                    break;
                }
                Err(FileOpErrorKind::SourceMissing) => break,
                Err(FileOpErrorKind::Cancelled) => {
                    self.engine.cancelled = true;
                    return;
                }
                Err(kind) => match self.on_error(&label, kind) {
                    ErrorStep::Retry => self.relink(&mut folder),
                    ErrorStep::Skip => break,
                    ErrorStep::Cancel => return,
                },
            }
        }
        self.item_done();
    }

    // --- 가져오기: 휴대폰 → PC ---

    fn pull_all(&mut self, sources: Vec<Source>, target: &Path) {
        if self.moving {
            // 휴대폰 쪽 원본을 지우므로(휴지통 없음) 되돌릴 수 없다.
            self.mark_not_undoable(NotUndoable::PermanentDelete);
        }
        for source in sources {
            if !self.proceed() {
                return;
            }
            let Source::Device { label, item } = source else {
                continue;
            };
            let Some(item) = item else {
                self.missing(&label);
                continue;
            };
            let mut own = DeviceFolder::new(label.clone(), item.id.clone());
            own.parsing = self.parsing_name(&item.id);
            let name = {
                let id = item.id.clone();
                self.call(move |_, _| shell_transfer::file_name(&id).map_err(|e| e.kind()))
                    .ok()
                    .or_else(|| label.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .unwrap_or_default()
            };
            let destination = target.join(pc_name(&name));
            if item.is_folder {
                self.pull_folder(own, destination);
            } else {
                self.pull_file(&label, item, &mut own, None, destination);
            }
        }
    }

    /// 원본 항목을 찾을 수 없다(휴대폰 폴더를 읽은 표에 없음): 「원본 없음」으로 묻는다. 다시 시도는 다시 묻는다(그
    /// 사이 휴대폰 폴더를 다시 읽어도 이 작업의 원본은 바뀌지 않는다).
    fn missing(&mut self, label: &Path) {
        loop {
            match self.on_error(label, FileOpErrorKind::SourceMissing) {
                ErrorStep::Retry => {}
                ErrorStep::Skip => {
                    self.item_done();
                    return;
                }
                ErrorStep::Cancel => return,
            }
        }
    }

    /// 휴대폰 파일 하나를 PC `target`으로 가져온다. `own`은 항목 자신(맨 위 원본)이거나, `parent`가 있으면 그 폴더의
    /// 항목이다(다시 시도할 때 폴더를 다시 이어 이름으로 다시 찾는다).
    fn pull_file(
        &mut self,
        label: &Path,
        item: DeviceItem,
        own: &mut DeviceFolder,
        mut parent: Option<&mut DeviceFolder>,
        target: PathBuf,
    ) {
        self.engine.reporter.current = Some(label.to_path_buf());
        let size = item.size.unwrap_or(0);
        let base = self.engine.reporter.bytes_done;
        let mut id = item.id.clone();
        loop {
            let done = self.pull_file_once(label, &id, &item, &target);
            self.engine.reporter.bytes_done = base;
            match done {
                Done::Copied { at, replaced } => {
                    let summary = &mut self.engine.summary;
                    summary.files_copied += 1;
                    summary.bytes_copied += size;
                    if replaced.overwrote() {
                        summary.files_overwritten += 1;
                    }
                    self.engine.reporter.bytes_done = base + size;
                    self.engine.note_item(label, &at, replaced);
                    if self.moving {
                        self.remove_device(label, &id);
                    }
                    break;
                }
                Done::Skipped => {
                    self.engine.summary.conflicts_skipped += 1;
                    self.engine.reporter.bytes_done = base + size;
                    break;
                }
                Done::Cancelled => {
                    self.engine.cancelled = true;
                    return;
                }
                Done::Failed(kind) => match self.on_error(label, kind) {
                    ErrorStep::Retry => {
                        // 다시 꽂힌 장치에 다시 잇고 그 항목부터 이어 간다.
                        match parent.as_deref_mut() {
                            Some(folder) => {
                                self.relink(folder);
                                let name = label.file_name().unwrap_or_default().to_os_string();
                                if let Ok(listing) = self.listing(folder)
                                    && let Some(child) = find_by_display(&listing, &item, &name)
                                {
                                    id = child.id.clone();
                                }
                            }
                            None => {
                                self.relink(own);
                                id = own.id.clone();
                            }
                        }
                    }
                    ErrorStep::Skip => {
                        self.engine.reporter.bytes_done = base + size;
                        break;
                    }
                    ErrorStep::Cancel => return,
                },
            }
        }
        self.item_done();
    }

    fn pull_file_once(
        &mut self,
        label: &Path,
        id: &[u8],
        item: &DeviceItem,
        target: &Path,
    ) -> Done {
        if fs::symlink_metadata(target).is_err() {
            return self.read_into(id, item, target, target);
        }
        let target_is_folder = fs::metadata(target).is_ok_and(|m| m.is_dir());
        let can_overwrite = !target_is_folder;
        let conflict = Conflict {
            source: ConflictItem {
                path: label.to_path_buf(),
                kind: ItemKind::File,
                size: item.size,
                modified: item.modified,
            },
            target: conflict_item(target),
            can_overwrite,
            overwrite_is_permanent: can_overwrite && !super::has_recycle_bin(target),
        };
        let target_modified = conflict.target.modified;
        let Some(resolution) = self.engine.answer_conflict(conflict) else {
            return Done::Cancelled;
        };
        match resolution {
            Resolution::Skip => Done::Skipped,
            Resolution::KeepBoth => {
                let renamed = free_numbered(target, true);
                self.read_into(id, item, &renamed, &renamed)
            }
            Resolution::Overwrite => self.overwrite_pc(id, item, target),
            Resolution::OverwriteIfNewer => match (item.modified, target_modified) {
                (Some(source), Some(existing)) if source > existing => {
                    self.overwrite_pc(id, item, target)
                }
                _ => Done::Skipped,
            },
        }
    }

    /// 덮어쓰기: 임시 이름에 다 가져온 뒤 원래 대상을 휴지통으로 보내고 놓는다(복사의 덮어쓰기와 같다).
    fn overwrite_pc(&mut self, id: &[u8], item: &DeviceItem, target: &Path) -> Done {
        let temp = temp_path(target);
        match self.read_into(id, item, &temp, target) {
            Done::Copied { .. } => match self.engine.place(&temp, target) {
                FileOutcome::Copied { replaced } => Done::Copied {
                    at: target.to_path_buf(),
                    replaced,
                },
                outcome => {
                    let _ = file_ops::remove_file(&temp);
                    match outcome {
                        FileOutcome::Cancelled => Done::Cancelled,
                        FileOutcome::Failed(kind) => Done::Failed(kind),
                        _ => Done::Failed(FileOpErrorKind::Other),
                    }
                }
            },
            done => done,
        }
    }

    /// 휴대폰 파일 `id`를 `write`(없는 이름)에 가져온다. 다 되면 결과의 자리는 `at`이다.
    fn read_into(&mut self, id: &[u8], item: &DeviceItem, write: &Path, at: &Path) -> Done {
        let (id, path) = (id.to_vec(), write.to_path_buf());
        let result = self.call(move |cancel, bytes| {
            shell_transfer::read_to_file(&id, &path, &mut |done| {
                bytes.store(done, Ordering::Relaxed);
                !cancel.load(Ordering::Relaxed)
            })
            .map_err(|e| e.kind())
        });
        match result {
            Ok(_) => {
                if let Some(modified) = item.modified {
                    let _ = fs::File::options()
                        .write(true)
                        .open(write)
                        .and_then(|file| file.set_modified(modified));
                }
                Done::Copied {
                    at: at.to_path_buf(),
                    replaced: Replaced::Nothing,
                }
            }
            Err(FileOpErrorKind::Cancelled) => {
                let _ = fs::remove_file(write);
                Done::Cancelled
            }
            Err(FileOpErrorKind::TargetExists) => Done::Failed(FileOpErrorKind::TargetExists),
            Err(kind) => Done::Failed(kind),
        }
    }

    /// 이동: 다 가져온 휴대폰 원본을 지운다(영구 삭제). 지우지 못하면 묻는다(건너뛰면 둘 다 남는다).
    fn remove_device(&mut self, label: &Path, id: &[u8]) {
        loop {
            let id = id.to_vec();
            match self.call(move |_, _| shell_transfer::delete(&id).map_err(|e| e.kind())) {
                Ok(()) | Err(FileOpErrorKind::SourceMissing) => return,
                Err(FileOpErrorKind::Cancelled) => {
                    self.engine.cancelled = true;
                    return;
                }
                Err(kind) => match self.on_error(label, kind) {
                    ErrorStep::Retry => {}
                    ErrorStep::Skip | ErrorStep::Cancel => return,
                },
            }
        }
    }

    /// 휴대폰 폴더를 PC `target`으로 가져온다: PC에 폴더를 만들고(같은 이름 폴더면 합친다) 안을 차례로 가져온다.
    fn pull_folder(&mut self, mut folder: DeviceFolder, target: PathBuf) {
        self.engine.reporter.current = Some(folder.label.clone());
        let Some(actual) = self.start_pc_folder(&folder.label, target) else {
            self.item_done();
            return;
        };
        self.item_done();
        let listing = loop {
            match self.listing(&mut folder) {
                Ok(listing) => break listing,
                Err(FileOpErrorKind::Cancelled) => {
                    self.engine.cancelled = true;
                    return;
                }
                Err(kind) => match self.on_error(&folder.label.clone(), kind) {
                    ErrorStep::Retry => self.relink(&mut folder),
                    ErrorStep::Skip | ErrorStep::Cancel => return,
                },
            }
        };
        let bytes = listing.iter().filter_map(|child| child.size).sum();
        self.add_totals(listing.len() as u64, bytes);
        for child in listing {
            if !self.proceed() {
                return;
            }
            let label = folder.label.join(&child.name);
            let item = device_item_of(&child);
            let destination = actual.join(pc_name(&child.name));
            if child.is_folder {
                let mut sub = DeviceFolder::new(label, child.id.clone());
                sub.parsing = self.parsing_name(&child.id);
                self.pull_folder(sub, destination);
            } else {
                let mut own = DeviceFolder::new(label.clone(), child.id.clone());
                self.pull_file(&label, item, &mut own, Some(&mut folder), destination);
            }
        }
        if self.moving && !self.engine.cancelled {
            folder.listing = None;
            if self.listing(&mut folder).is_ok_and(|rest| rest.is_empty()) {
                let id = folder.id.clone();
                let _ = self.call(move |_, _| shell_transfer::delete(&id).map_err(|e| e.kind()));
            }
        }
    }

    /// PC에 폴더를 만들거나(같은 이름 폴더면 합치고, 파일과 부딪히면 묻고) 그 실제 경로를 돌려준다. 건너뛰거나 취소면 `None`.
    fn start_pc_folder(&mut self, label: &Path, target: PathBuf) -> Option<PathBuf> {
        loop {
            let outcome = match fs::symlink_metadata(&target) {
                Ok(metadata) if metadata.is_dir() => {
                    self.engine.summary.folders_merged += 1;
                    return Some(target);
                }
                Ok(_) => {
                    let conflict = Conflict {
                        source: ConflictItem {
                            path: label.to_path_buf(),
                            kind: ItemKind::Folder,
                            size: None,
                            modified: None,
                        },
                        target: conflict_item(&target),
                        can_overwrite: false,
                        overwrite_is_permanent: false,
                    };
                    match self.engine.answer_conflict(conflict) {
                        None => {
                            self.engine.cancelled = true;
                            return None;
                        }
                        Some(Resolution::KeepBoth) => {
                            let renamed = free_numbered(&target, false);
                            file_ops::create_folder(&renamed).map(|()| renamed)
                        }
                        Some(_) => {
                            self.engine.summary.conflicts_skipped += 1;
                            return None;
                        }
                    }
                }
                Err(_) => file_ops::create_folder(&target).map(|()| target.clone()),
            };
            match outcome {
                Ok(created) => {
                    self.engine.summary.folders_copied += 1;
                    self.engine.note_folder(label, &created, true);
                    return Some(created);
                }
                Err(error) => match self.on_error(label, error.kind()) {
                    ErrorStep::Retry => {}
                    ErrorStep::Skip | ErrorStep::Cancel => return None,
                },
            }
        }
    }

    // --- 넣기: PC → 휴대폰 ---

    fn push_all(&mut self, sources: Vec<Source>, label: PathBuf, id: Vec<u8>) {
        // 휴대폰 공급자는 「실행취소 가능」을 알리지 않는다(되돌리려면 휴대폰 쪽을 영구 삭제해야 한다).
        let location = crate::Location::shell(crate::VirtualFolder::Device, id.clone());
        if !crate::provider_for(&location)
            .capabilities(&location)
            .undoable
        {
            self.mark_not_undoable(NotUndoable::NotUndoableThere);
        }
        let mut folder = DeviceFolder::new(label, id);
        for source in sources {
            if !self.proceed() {
                return;
            }
            let Source::Pc(path) = source else {
                continue;
            };
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() => self.push_folder(&path, &mut folder),
                Ok(_) => self.push_file(&path, &mut folder),
                Err(error) => {
                    let kind = super::copy::error_kind(&error);
                    self.push_failed(&path, kind, &mut folder);
                }
            }
        }
    }

    /// 넣을 원본을 읽지 못했다: 묻고, 다시 시도면 다시 해 본다.
    fn push_failed(&mut self, path: &Path, kind: FileOpErrorKind, folder: &mut DeviceFolder) {
        match self.on_error(path, kind) {
            ErrorStep::Retry => match fs::symlink_metadata(path) {
                Ok(metadata) if metadata.is_dir() => self.push_folder(path, folder),
                Ok(_) => self.push_file(path, folder),
                Err(error) => self.push_failed(path, super::copy::error_kind(&error), folder),
            },
            ErrorStep::Skip => self.item_done(),
            ErrorStep::Cancel => {}
        }
    }

    fn push_file(&mut self, source: &Path, folder: &mut DeviceFolder) {
        self.engine.reporter.current = Some(source.to_path_buf());
        let metadata = fs::metadata(source).ok();
        let size = metadata.as_ref().map_or(0, fs::Metadata::len);
        let modified = metadata.as_ref().and_then(|m| m.modified().ok());
        let base = self.engine.reporter.bytes_done;
        loop {
            let done = self.push_file_once(source, folder);
            self.engine.reporter.bytes_done = base;
            match done {
                Done::Copied { at, replaced } => {
                    let summary = &mut self.engine.summary;
                    summary.files_copied += 1;
                    summary.bytes_copied += size;
                    if replaced.overwrote() {
                        summary.files_overwritten += 1;
                    }
                    self.engine.reporter.bytes_done = base + size;
                    folder.listing = None;
                    let stamp = Stamp {
                        is_folder: false,
                        size,
                        modified: modified.unwrap_or_else(SystemTime::now),
                    };
                    let step = if self.moving {
                        Step::Moved {
                            from: source.to_path_buf(),
                            to: at,
                            stamp,
                        }
                    } else {
                        Step::Copied {
                            source: source.to_path_buf(),
                            path: at,
                            stamp,
                        }
                    };
                    self.note(step, NotUndoable::NotUndoableThere);
                    if self.moving {
                        self.remove_pc(source);
                    }
                    break;
                }
                Done::Skipped => {
                    self.engine.summary.conflicts_skipped += 1;
                    self.engine.reporter.bytes_done = base + size;
                    break;
                }
                Done::Cancelled => {
                    self.engine.cancelled = true;
                    return;
                }
                Done::Failed(kind) => match self.on_error(source, kind) {
                    // 다시 꽂힌 장치에 폴더를 다시 잇고 그 파일부터 이어 간다.
                    ErrorStep::Retry => self.relink(folder),
                    ErrorStep::Skip => {
                        self.engine.reporter.bytes_done = base + size;
                        break;
                    }
                    ErrorStep::Cancel => return,
                },
            }
        }
        self.item_done();
    }

    fn push_file_once(&mut self, source: &Path, folder: &mut DeviceFolder) -> Done {
        let Some(name) = source.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            return Done::Failed(FileOpErrorKind::Other);
        };
        if fs::symlink_metadata(source).is_err() {
            return Done::Failed(FileOpErrorKind::SourceMissing);
        }
        let listing = match self.listing(folder) {
            Ok(listing) => listing,
            Err(kind) => return Done::Failed(kind),
        };
        let Some(existing) = find_name(&listing, &name) else {
            return self.copy_to_device(source, folder, &name, None, false);
        };
        let can_overwrite = !existing.is_folder;
        let conflict = Conflict {
            source: conflict_item(source),
            target: ConflictItem {
                path: folder.label.join(&existing.name),
                kind: if existing.is_folder {
                    ItemKind::Folder
                } else {
                    ItemKind::File
                },
                size: existing.size.filter(|_| !existing.is_folder),
                modified: existing
                    .modified
                    .map(shell_provider::filetime_to_system_time),
            },
            can_overwrite,
            // 휴대폰에는 휴지통이 없다.
            overwrite_is_permanent: can_overwrite,
        };
        let (source_modified, target_modified) =
            (conflict.source.modified, conflict.target.modified);
        let Some(resolution) = self.engine.answer_conflict(conflict) else {
            return Done::Cancelled;
        };
        let existing_name = existing.name.clone();
        match resolution {
            Resolution::Skip => Done::Skipped,
            Resolution::KeepBoth => {
                let free = free_device_name(&listing, &name, true);
                self.copy_to_device(source, folder, &free, Some(&free), false)
            }
            Resolution::Overwrite => {
                self.copy_to_device(source, folder, &existing_name, Some(&existing_name), true)
            }
            Resolution::OverwriteIfNewer => match (source_modified, target_modified) {
                (Some(a), Some(b)) if a > b => {
                    self.copy_to_device(source, folder, &existing_name, Some(&existing_name), true)
                }
                _ => Done::Skipped,
            },
        }
    }

    fn copy_to_device(
        &mut self,
        source: &Path,
        folder: &DeviceFolder,
        name: &str,
        new_name: Option<&str>,
        overwrite: bool,
    ) -> Done {
        let (source_path, id, new_name) = (
            source.to_path_buf(),
            folder.id.clone(),
            new_name.map(str::to_owned),
        );
        match self.call(move |_, _| {
            shell_transfer::copy_into(&source_path, &id, new_name.as_deref()).map_err(|e| e.kind())
        }) {
            Ok(()) => Done::Copied {
                at: folder.label.join(name),
                replaced: if overwrite {
                    Replaced::Permanently
                } else {
                    Replaced::Nothing
                },
            },
            Err(FileOpErrorKind::Cancelled) => Done::Cancelled,
            Err(kind) => Done::Failed(kind),
        }
    }

    /// 이동: 휴대폰에 다 넣은 PC 원본을 지운다. 지우지 못하면 묻는다(건너뛰면 둘 다 남는다).
    fn remove_pc(&mut self, source: &Path) {
        while let Err(error) = file_ops::remove_file(source) {
            if error.kind() == FileOpErrorKind::SourceMissing {
                return;
            }
            match self.on_error(source, error.kind()) {
                ErrorStep::Retry => {}
                ErrorStep::Skip | ErrorStep::Cancel => return,
            }
        }
    }

    fn push_folder(&mut self, source: &Path, folder: &mut DeviceFolder) {
        self.engine.reporter.current = Some(source.to_path_buf());
        let Some(mut sub) = self.start_device_folder(source, folder) else {
            self.item_done();
            return;
        };
        self.item_done();
        let entries = loop {
            match fs::read_dir(source).and_then(|entries| entries.collect::<Result<Vec<_>, _>>()) {
                Ok(entries) => break entries,
                Err(error) => match self.on_error(source, super::copy::error_kind(&error)) {
                    ErrorStep::Retry => {}
                    ErrorStep::Skip | ErrorStep::Cancel => return,
                },
            }
        };
        let mut paths: Vec<(PathBuf, Option<fs::Metadata>)> = entries
            .into_iter()
            .map(|entry| (entry.path(), entry.path().symlink_metadata().ok()))
            .collect();
        paths.sort_by(|a, b| a.0.cmp(&b.0));
        let bytes = paths
            .iter()
            .filter_map(|(_, m)| m.as_ref().filter(|m| m.is_file()).map(fs::Metadata::len))
            .sum();
        self.add_totals(paths.len() as u64, bytes);
        for (path, metadata) in paths {
            if !self.proceed() {
                return;
            }
            match metadata {
                // 폴더 링크·정션은 따라 들어가지 않는다(순환을 피함): 묻는다.
                Some(m)
                    if m.file_type().is_symlink()
                        && fs::metadata(&path).is_ok_and(|t| t.is_dir()) =>
                {
                    match self.on_error(&path, FileOpErrorKind::Unsupported) {
                        ErrorStep::Cancel => return,
                        _ => self.item_done(),
                    }
                }
                Some(m) if m.is_dir() => self.push_folder(&path, &mut sub),
                Some(_) => self.push_file(&path, &mut sub),
                None => self.push_failed(&path, FileOpErrorKind::SourceMissing, &mut sub),
            }
        }
        if self.moving && !self.engine.cancelled {
            let _ = file_ops::remove_empty_folder(source);
        }
    }

    /// 휴대폰에 폴더를 만들거나(같은 이름 폴더면 합치고, 파일과 부딪히면 묻고) 그 폴더를 돌려준다. 건너뛰거나 취소면 `None`.
    fn start_device_folder(
        &mut self,
        source: &Path,
        folder: &mut DeviceFolder,
    ) -> Option<DeviceFolder> {
        let name = source.file_name()?.to_string_lossy().into_owned();
        loop {
            let listing = match self.listing(folder) {
                Ok(listing) => listing,
                Err(FileOpErrorKind::Cancelled) => {
                    self.engine.cancelled = true;
                    return None;
                }
                Err(kind) => match self.on_error(source, kind) {
                    ErrorStep::Retry => {
                        self.relink(folder);
                        continue;
                    }
                    ErrorStep::Skip | ErrorStep::Cancel => return None,
                },
            };
            let make = match find_name(&listing, &name) {
                Some(child) if child.is_folder => {
                    self.engine.summary.folders_merged += 1;
                    return Some(DeviceFolder::new(
                        folder.label.join(&child.name),
                        child.id.clone(),
                    ));
                }
                Some(child) => {
                    let conflict = Conflict {
                        source: conflict_item(source),
                        target: ConflictItem {
                            path: folder.label.join(&child.name),
                            kind: ItemKind::File,
                            size: child.size,
                            modified: child.modified.map(shell_provider::filetime_to_system_time),
                        },
                        can_overwrite: false,
                        overwrite_is_permanent: false,
                    };
                    match self.engine.answer_conflict(conflict) {
                        None => {
                            self.engine.cancelled = true;
                            return None;
                        }
                        Some(Resolution::KeepBoth) => free_device_name(&listing, &name, false),
                        Some(_) => {
                            self.engine.summary.conflicts_skipped += 1;
                            return None;
                        }
                    }
                }
                None => name.clone(),
            };
            let (id, made_name) = (folder.id.clone(), make.clone());
            let made = self.call(move |_, _| {
                shell_transfer::new_folder(&id, &made_name).map_err(|e| e.kind())
            });
            folder.listing = None;
            let created = made.and_then(|()| {
                let listing = self.listing(folder)?;
                find_name(&listing, &make)
                    .filter(|child| child.is_folder)
                    .map(|child| child.id.clone())
                    .ok_or(FileOpErrorKind::Other)
            });
            match created {
                Ok(id) => {
                    self.engine.summary.folders_copied += 1;
                    let label = folder.label.join(&make);
                    self.note(
                        Step::Copied {
                            source: source.to_path_buf(),
                            path: label.clone(),
                            stamp: Stamp {
                                is_folder: true,
                                size: 0,
                                modified: SystemTime::now(),
                            },
                        },
                        NotUndoable::NotUndoableThere,
                    );
                    return Some(DeviceFolder::new(label, id));
                }
                Err(FileOpErrorKind::Cancelled) => {
                    self.engine.cancelled = true;
                    return None;
                }
                Err(kind) => match self.on_error(source, kind) {
                    ErrorStep::Retry => self.relink(folder),
                    ErrorStep::Skip | ErrorStep::Cancel => return None,
                },
            }
        }
    }
}

/// 휴대폰 폴더 항목을 휴대폰 항목 정보로.
fn device_item_of(child: &ShellChild) -> DeviceItem {
    DeviceItem {
        id: child.id.clone(),
        is_folder: child.is_folder,
        size: child.size,
        modified: child.modified.map(shell_provider::filetime_to_system_time),
    }
}

/// 이름이 같은 항목(대소문자 무시: Windows와 같은 규칙으로 겹침을 본다).
fn find_name<'l>(listing: &'l [ShellChild], name: &str) -> Option<&'l ShellChild> {
    let lower = name.to_lowercase();
    listing
        .iter()
        .find(|child| child.name.to_lowercase() == lower)
}

/// 다시 이은 폴더에서 같은 항목을 찾는다: 이름(파일 이름이나 항목 경로의 마지막 조각)이 같은 것.
fn find_by_display<'l>(
    listing: &'l [ShellChild],
    item: &DeviceItem,
    name: &OsStr,
) -> Option<&'l ShellChild> {
    let name = name.to_string_lossy();
    find_name(listing, &name).filter(|child| child.is_folder == item.is_folder)
}

/// 휴대폰 폴더에서 아직 없는 「이름 (2)」, 「이름 (3)」 ….
fn free_device_name(listing: &[ShellChild], name: &str, is_file: bool) -> String {
    (2..)
        .map(|n| numbered(name, is_file, n))
        .find(|candidate| find_name(listing, candidate).is_none())
        .expect("빈 이름은 언젠가 나온다")
}

/// 「이름 (n)」(파일은 확장자를 뒤에).
fn numbered(name: &str, is_file: bool, n: u32) -> String {
    let path = Path::new(name);
    match (is_file, path.file_stem(), path.extension()) {
        (true, Some(stem), Some(extension)) => format!(
            "{} ({n}).{}",
            stem.to_string_lossy(),
            extension.to_string_lossy()
        ),
        _ => format!("{name} ({n})"),
    }
}

/// 휴대폰 항목 이름을 PC에서 쓸 수 있는 이름으로: Windows가 받지 않는 글자는 `_`로, 끝의 점·공백은 뗀다.
fn pc_name(name: &str) -> String {
    let mapped: String = name
        .chars()
        .map(|c| {
            if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
                || (c as u32) < 0x20
            {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = mapped.trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        "_".to_owned()
    } else {
        trimmed.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 휴대폰_이름을_pc_이름으로_바꾼다() {
        assert_eq!(pc_name("IMG_0001.jpg"), "IMG_0001.jpg");
        assert_eq!(pc_name("a:b?.txt"), "a_b_.txt");
        assert_eq!(pc_name("끝에 점. "), "끝에 점");
        assert_eq!(pc_name("..."), "_");
    }

    #[test]
    fn 번호_이름() {
        assert_eq!(numbered("사진.jpg", true, 2), "사진 (2).jpg");
        assert_eq!(numbered("앨범.2024", false, 3), "앨범.2024 (3)");
    }

    fn child(name: &str) -> ShellChild {
        ShellChild {
            name: name.into(),
            is_folder: false,
            id: vec![0, 0],
            size: None,
            modified: None,
        }
    }

    #[test]
    fn 휴대폰_폴더에서_빈_번호_이름을_고른다() {
        let listing = [child("a.jpg"), child("A (2).JPG")];
        assert_eq!(free_device_name(&listing, "a.jpg", true), "a (3).jpg");
        assert!(find_name(&listing, "A.JPG").is_some());
    }
}
