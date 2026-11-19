//! **내려받기** 작업(#453): 온라인 전용 파일을 끝까지 읽어 동기화 프로그램이 내용을 이 장치에 내려받게 한다. 열기가
//! 온라인 전용 파일을 열 때 먼저 넣고, 끝나면 앱이 기본 프로그램으로 연다([`crate::window::App::execute`]).
//!
//! 실제 내려받기는 동기화 프로그램(OneDrive 등)의 몫이다. 파일을 읽으면 클라우드 파일 필터가 동기화 프로그램에 내용을
//! 청하고 읽기는 내용이 올 때까지 기다린다. 그 기다림은 작업 스레드에서 끊을 수 없으므로 읽기는 따로 띄운 **읽는
//! 스레드**가 하고, 작업 스레드는 읽은 덩어리를 받으며 취소·일시정지와 **무응답 시간 제한**을 본다:
//!
//! - 덩어리가 오지 않은 채 시간 제한([`super::DOWNLOAD_STALL_TIMEOUT`], 대기열마다
//!   [`super::JobQueue::set_stall_timeout`])이 지나면 [`FileOpErrorKind::TimedOut`] 항목별 오류로 묻는다.
//! - 동기화 프로그램이 꺼져 있으면 읽기가 곧바로 실패해 [`FileOpErrorKind::SyncProviderNotRunning`]으로 묻는다.
//! - 다시 시도는 처음부터 다시 읽는다(이미 내려받은 부분은 동기화 프로그램이 다시 보내지 않는다).
//! - 취소·시간 제한으로 버린 읽는 스레드는 기다리지 않는다: 멈춘 읽기가 풀리면 받을 곳이 없어 스스로 끝난다.
//!
//! 진행은 항목 하나와 읽은 바이트이고 [`super::Progress::downloading`]이 늘 켜져 있다. 아무것도 바꾸지 않으므로
//! 작업 기록에 남지 않는다.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use facet_win::file_ops::{FileOpError, FileOpErrorKind};

use super::{
    Context, ErrorAnswer, ItemFailure, JobEvent, PROGRESS_INTERVAL, Progress, Remaining, Summary,
};

/// 읽는 스레드가 한 번에 읽는 크기.
const CHUNK: usize = 1024 * 1024;

/// 작업 스레드가 덩어리를 기다리다 취소·일시정지를 보러 깨어나는 간격.
const TICK: Duration = Duration::from_millis(50);

/// 넣기 전에 정한 내려받기 작업.
pub(super) struct DownloadJob {
    /// 내려받을 파일.
    source: PathBuf,
    /// 무응답 시간 제한.
    stall_timeout: Duration,
}

/// 읽는 스레드가 보내는 소식.
enum Fetched {
    /// 덩어리 하나를 읽었다(바이트 수).
    Chunk(u64),
    /// 끝까지 읽었다.
    Done,
    /// 읽지 못했다.
    Failed(FileOpError),
}

/// 한 번 읽어 본 결과.
enum Attempt {
    Done,
    Failed(FileOpErrorKind),
    Cancelled,
}

impl DownloadJob {
    pub(super) fn new(source: PathBuf, stall_timeout: Duration) -> Self {
        DownloadJob {
            source,
            stall_timeout,
        }
    }

    /// 넘긴 원본 수(늘 하나).
    pub(super) fn root_count(&self) -> u64 {
        1
    }

    /// 작업 스레드에서 끝까지(또는 취소될 때까지) 실행하고 결과 요약과 취소로 멈췄는지를 돌려준다.
    pub(super) fn run(self, context: &Context<'_>) -> (Summary, bool) {
        // 크기는 속성만 읽는다(내용을 읽지 않아 내려받기가 일어나지 않는다).
        let total = std::fs::metadata(&self.source).map_or(0, |m| m.len());
        let mut reporter = Reporter::new(context, &self.source, total);
        reporter.emit();
        let mut summary = Summary {
            all_counted: true,
            ..Summary::default()
        };
        loop {
            if !context.control.proceed() {
                summary.items_remaining = 1;
                return (summary, true);
            }
            reporter.bytes_done = 0;
            match self.attempt(context, &mut reporter) {
                Attempt::Done => {
                    reporter.items_done = 1;
                    reporter.bytes_done = reporter.bytes_done.max(total);
                    reporter.emit();
                    summary.items_done = 1;
                    summary.bytes_copied = reporter.bytes_done;
                    return (summary, false);
                }
                Attempt::Cancelled => {
                    summary.items_remaining = 1;
                    return (summary, true);
                }
                Attempt::Failed(kind) => {
                    let failure = ItemFailure {
                        source: self.source.clone(),
                        kind,
                    };
                    match context.ask_error(failure.clone()) {
                        Some(ErrorAnswer::Retry) => continue,
                        Some(_) => {
                            reporter.items_done = 1;
                            reporter.emit();
                            summary.items_done = 1;
                            summary.failures.push(failure);
                            return (summary, false);
                        }
                        None => {
                            summary.items_remaining = 1;
                            return (summary, true);
                        }
                    }
                }
            }
        }
    }

    /// 읽는 스레드를 띄워 끝까지 읽어 본다.
    fn attempt(&self, context: &Context<'_>, reporter: &mut Reporter<'_>) -> Attempt {
        let receiver = spawn_reader(self.source.clone());
        let mut last_activity = Instant::now();
        loop {
            match receiver.recv_timeout(TICK) {
                Ok(Fetched::Chunk(n)) => {
                    reporter.bytes_done += n;
                    reporter.maybe_emit();
                    last_activity = Instant::now();
                }
                Ok(Fetched::Done) => return Attempt::Done,
                Ok(Fetched::Failed(error)) => return Attempt::Failed(error.kind()),
                Err(RecvTimeoutError::Disconnected) => {
                    return Attempt::Failed(FileOpErrorKind::Other);
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            if context.control.is_cancelled() {
                return Attempt::Cancelled;
            }
            if context.control.is_paused() {
                // 멈춘 동안은 시간 제한을 세지 않는다(읽는 스레드는 받을 곳이 비기를 기다린다).
                if !context.control.proceed() {
                    return Attempt::Cancelled;
                }
                last_activity = Instant::now();
            }
            if last_activity.elapsed() >= self.stall_timeout {
                return Attempt::Failed(FileOpErrorKind::TimedOut);
            }
        }
    }
}

/// 파일을 끝까지 읽는 스레드를 띄운다. 덩어리마다 소식을 보내고, 받는 쪽이 사라지면 그만둔다. 받는 쪽이 받지 않는
/// 동안(일시정지)은 다음 덩어리를 읽지 않는다(통로에 하나만 둔다).
fn spawn_reader(source: PathBuf) -> Receiver<Fetched> {
    let (sender, receiver) = mpsc::sync_channel(1);
    // 스레드를 띄우지 못하면 받는 쪽은 끊긴 통로로 실패를 안다.
    let _ = thread::Builder::new()
        .name("facet-download".into())
        .spawn(move || read_all(&source, &sender));
    receiver
}

fn read_all(source: &Path, sender: &SyncSender<Fetched>) {
    let mut file = match File::open(source) {
        Ok(file) => file,
        Err(error) => {
            let _ = sender.send(Fetched::Failed(FileOpError::from_io(&error)));
            return;
        }
    };
    let mut buffer = vec![0u8; CHUNK];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => {
                let _ = sender.send(Fetched::Done);
                return;
            }
            Ok(n) => {
                if sender.send(Fetched::Chunk(n as u64)).is_err() {
                    return;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => {
                let _ = sender.send(Fetched::Failed(FileOpError::from_io(&error)));
                return;
            }
        }
    }
}

/// 진행 사건을 만든다(작업마다 [`PROGRESS_INTERVAL`]보다 자주 보내지 않는다).
struct Reporter<'a> {
    context: &'a Context<'a>,
    source: &'a Path,
    started: Instant,
    last: Option<Instant>,
    items_done: u64,
    bytes_done: u64,
    bytes_total: u64,
}

impl<'a> Reporter<'a> {
    fn new(context: &'a Context<'a>, source: &'a Path, bytes_total: u64) -> Self {
        Reporter {
            context,
            source,
            started: Instant::now(),
            last: None,
            items_done: 0,
            bytes_done: 0,
            bytes_total,
        }
    }

    fn maybe_emit(&mut self) {
        if self
            .last
            .is_some_and(|last| last.elapsed() < PROGRESS_INTERVAL)
        {
            return;
        }
        self.emit();
    }

    fn emit(&mut self) {
        let elapsed = self.started.elapsed();
        let seconds = elapsed.as_secs_f64();
        let bytes_per_second = if seconds > 0.0 {
            (self.bytes_done as f64 / seconds) as u64
        } else {
            0
        };
        let bytes_total = self.bytes_total.max(self.bytes_done);
        let remaining = if self.bytes_done >= bytes_total {
            Remaining::Known(Duration::ZERO)
        } else if bytes_per_second == 0 {
            Remaining::Calculating
        } else {
            Remaining::Known(Duration::from_secs_f64(
                (bytes_total - self.bytes_done) as f64 / bytes_per_second as f64,
            ))
        };
        self.last = Some(Instant::now());
        let progress = Progress {
            items_done: self.items_done,
            items_total: 1,
            bytes_done: self.bytes_done,
            bytes_total,
            counted: true,
            bytes_per_second,
            remaining,
            current: Some(self.source.to_path_buf()),
            elapsed,
            downloading: true,
        };
        let context = self.context;
        context.control.unless_paused(|| {
            context.outbox.send(JobEvent::Progress {
                job: context.id,
                progress,
            });
        });
    }
}
