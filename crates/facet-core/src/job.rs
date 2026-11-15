//! 작업 대기열과 그 안에서 도는 작업(ADR-0005).
//!
//! **작업 대기열**([`JobQueue`])은 앱 전체에 하나다. 앱이 하나만 만들어 들고 있는다. 작업을 넣으면 일꾼 스레드
//! 하나가 넣은 차례대로 하나씩 실행하고, 그 사이 일어난 일을 **사건**([`JobEvent`])으로 보낸다. 화면은 사건을
//! 받아 그리기만 한다. 사건에는 화면 문장이 없다(ADR-0007): 상태는 [`JobState`], 실패는 종류
//! ([`FileOpErrorKind`])와 경로다.
//!
//! 사건은 탭의 읽기 결과와 나란한 통로로 받는다: [`JobQueue::set_waker`]가 사건을 보낼 때마다 부를 깨우기를
//! 정하고, 깨워진 앱이 [`JobQueue::poll`]로 쌓인 사건을 받는다(테스트는 [`JobQueue::wait_event`]). 탭 결과의
//! 통로([`crate::window::App::poll`])에 섞지 않은 까닭: 작업은 탭·창에 속하지 않고(마지막 창을 닫아도 돈다),
//! 대기열을 `App` 없이 따로 테스트·벤치마크할 수 있어야 하기 때문이다. 깨우기 방식은 같으므로 앱은 두 통로를
//! 한 깨우기에서 차례로 받으면 된다.
//!
//! 지금 있는 작업은 **복사**([`JobQueue::copy`])다. 진행·이름 충돌·오류 처리의 자세한 규칙은 그 문서에 있다.

mod copy;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub use facet_win::file_ops::FileOpErrorKind;

/// 진행 사건 사이의 가장 짧은 간격. 작업마다 진행 사건은 초당 10번 이하다.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// 작업의 번호. 대기열 안에서 바뀌지 않고 다시 쓰이지 않는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JobId(u64);

impl JobId {
    /// 번호 값(기록·디버그용).
    pub fn get(self) -> u64 {
        self.0
    }
}

/// 작업의 상태.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JobState {
    /// 넣었고 차례를 기다린다.
    Queued,
    /// 실행 중이다.
    Running,
    /// 사용자가 멈췄다(뒤 티켓).
    Paused,
    /// 이름 충돌·오류에 대한 사용자의 답을 기다린다(뒤 티켓).
    AwaitingAnswer,
    /// 끝났다. 항목별 실패가 없다(이름 충돌로 건너뛴 항목은 있을 수 있다).
    Finished,
    /// 끝났지만 실패한 항목이 있다.
    Failed,
    /// 사용자가 취소했다(뒤 티켓).
    Cancelled,
}

impl JobState {
    /// 더 바뀌지 않는 마지막 상태인지.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobState::Finished | JobState::Failed | JobState::Cancelled
        )
    }
}

/// 남은 시간.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remaining {
    /// 아직 다 세지 않았거나 속도를 모른다(「계산 중」).
    Calculating,
    /// 지금 속도로 남은 바이트를 옮기는 시간.
    Known(Duration),
}

/// 작업의 진행. 항목은 파일과 폴더(넘긴 원본 자신 포함)이고, 바이트는 파일 내용의 크기다.
///
/// 「한 것」에는 이름 충돌·오류로 건너뛴 항목도 든다. 그래서 작업이 끝나면 한 것이 센 것과 같다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    /// 처리한 항목 수.
    pub items_done: u64,
    /// 지금까지 센 항목 수. 다 세기 전에는 늘어난다.
    pub items_total: u64,
    /// 처리한 바이트(복사 중인 파일의 복사한 만큼 포함).
    pub bytes_done: u64,
    /// 지금까지 센 바이트.
    pub bytes_total: u64,
    /// 원본을 다 셌는지. 다 세기 전에는 `items_total`·`bytes_total`이 전체가 아니다.
    pub counted: bool,
    /// 작업을 시작한 뒤 평균 속도(초당 바이트).
    pub bytes_per_second: u64,
    /// 남은 시간. 다 세기 전에는 늘 [`Remaining::Calculating`].
    pub remaining: Remaining,
    /// 지금 다루는 원본 항목.
    pub current: Option<PathBuf>,
    /// 작업을 시작한 뒤 흐른 시간(이 진행을 만든 때).
    pub elapsed: Duration,
}

/// 실패한 항목 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemFailure {
    /// 원본 경로.
    pub source: PathBuf,
    /// 실패 종류.
    pub kind: FileOpErrorKind,
}

/// 끝난 작업의 결과 요약.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    /// 복사한 파일 수.
    pub files_copied: u64,
    /// 만든 폴더 수.
    pub folders_copied: u64,
    /// 복사한 바이트.
    pub bytes_copied: u64,
    /// 대상에 같은 이름이 있어 건너뛴 항목 수(폴더면 그 폴더 하나로 센다).
    pub conflicts_skipped: u64,
    /// 따라 들어가지 않고 건너뛴 심볼릭 링크·정션 수(파일 링크 포함). 링크 자체를 복사하는 것은 뒤 티켓이다.
    pub links_skipped: u64,
    /// 실패한 항목(폴더면 그 아래는 따로 세지 않는다).
    pub failures: Vec<ItemFailure>,
}

/// 대기열이 보내는 사건.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobEvent {
    /// 진행. 작업마다 [`PROGRESS_INTERVAL`]보다 자주 오지 않는다.
    Progress {
        /// 작업.
        job: JobId,
        /// 진행.
        progress: Progress,
    },
    /// 상태가 바뀌었다. 넣으면 [`JobState::Queued`], 시작하면 [`JobState::Running`], 끝나면 마지막 상태다.
    StateChanged {
        /// 작업.
        job: JobId,
        /// 새 상태.
        state: JobState,
    },
    /// 작업이 끝났다. 마지막 상태의 [`JobEvent::StateChanged`] 바로 뒤에 온다. 이 작업의 마지막 사건이다.
    Finished {
        /// 작업.
        job: JobId,
        /// 결과 요약.
        summary: Summary,
    },
}

/// 복사를 시작하기 전에 막은 까닭. 이때는 작업을 만들지 않는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyRejection {
    /// 원본이 하나도 없다.
    NoSources,
    /// 대상 위치가 없다.
    TargetMissing,
    /// 대상 위치가 폴더가 아니다.
    TargetNotFolder,
    /// 폴더를 그 폴더 자신으로 복사하려 했다(값은 그 원본).
    IntoItself(PathBuf),
    /// 대상 위치가 원본 폴더 안에 있다(값은 그 원본).
    TargetInsideSource(PathBuf),
}

/// 일꾼에게 넘기는 작업.
enum Job {
    Copy(copy::CopyJob),
}

/// 깨우기.
type Waker = Arc<dyn Fn() + Send + Sync>;

/// 일꾼 스레드가 사건을 보내는 곳. 보낸 뒤 깨우기가 있으면 부른다.
#[derive(Clone)]
struct Outbox {
    sender: Sender<JobEvent>,
    waker: Arc<Mutex<Option<Waker>>>,
}

impl Outbox {
    fn send(&self, event: JobEvent) {
        // 대기열이 이미 없어졌으면 사건을 받을 곳이 없다(작업은 끝까지 돈다).
        if self.sender.send(event).is_err() {
            return;
        }
        let waker = self.waker.lock().ok().and_then(|w| w.clone());
        if let Some(waker) = waker {
            waker();
        }
    }
}

/// 작업 대기열. 앱 전체에 하나다.
///
/// 일꾼 스레드는 처음 작업을 넣을 때 띄운다. 대기열을 버려도 이미 넣은 작업은 끝까지 돈다.
pub struct JobQueue {
    next_id: u64,
    worker: Option<Sender<(JobId, Job)>>,
    outbox: Outbox,
    receiver: Receiver<JobEvent>,
    unfinished: Arc<AtomicUsize>,
}

impl Default for JobQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl JobQueue {
    /// 빈 대기열.
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        JobQueue {
            next_id: 1,
            worker: None,
            outbox: Outbox {
                sender,
                waker: Arc::default(),
            },
            receiver,
            unfinished: Arc::default(),
        }
    }

    /// 사건을 보낼 때마다 부를 깨우기를 정한다(일꾼 스레드에서 불린다).
    pub fn set_waker(&mut self, waker: impl Fn() + Send + Sync + 'static) {
        if let Ok(mut slot) = self.outbox.waker.lock() {
            *slot = Some(Arc::new(waker));
        }
    }

    /// 쌓인 사건을 모두 받는다. 기다리지 않는다.
    pub fn poll(&mut self) -> Vec<JobEvent> {
        self.receiver.try_iter().collect()
    }

    /// 사건 하나를 `timeout`까지 기다려 받는다.
    pub fn wait_event(&mut self, timeout: Duration) -> Option<JobEvent> {
        self.receiver.recv_timeout(timeout).ok()
    }

    /// 끝나지 않은(기다리거나 도는) 작업이 있는지. 마지막 작업의 [`JobEvent::Finished`]를 보낸 뒤 `false`가 된다.
    pub fn has_unfinished(&self) -> bool {
        self.unfinished.load(Ordering::Acquire) > 0
    }

    /// 원본 항목들(파일·폴더)을 `target` 폴더 안으로 복사하는 작업을 넣는다.
    ///
    /// 막을 경우([`CopyRejection`])에는 작업을 만들지 않고 까닭을 돌려준다: 원본이 없음, 대상 위치가 없거나
    /// 폴더가 아님, 폴더를 자기 자신이나 그 하위로 복사.
    ///
    /// 작업이 시작되면 원본을 훑어 전체 항목 수와 바이트를 세는 동안 이미 센 항목부터 복사한다. 파일 내용과
    /// 수정 시각·만든 시각·속성·대체 데이터 스트림을 보존하고, 폴더 시각·속성은 안의 항목을 다 쓴 뒤 맞춘다.
    /// 심볼릭 링크·정션(폴더·파일)은 따라 들어가지 않고 건너뛴다([`Summary::links_skipped`], 링크 자체 복사는
    /// 뒤 티켓).
    ///
    /// 원본이 대상과 같은 폴더에 있으면 묻지 않고 「이름 - 복사본」(있으면 「이름 - 복사본 (2)」, 「(3)」 …)으로
    /// 만든다. 파일은 확장자를 이름 뒤에 그대로 둔다.
    ///
    /// **임시 동작**(질문으로 바꾸는 것은 뒤 티켓): 대상에 같은 이름이 이미 있으면 덮어쓰지 않고 그 항목(폴더면
    /// 그 아래 전부)을 건너뛰어 [`Summary::conflicts_skipped`]에 센다. 항목별 오류도 묻지 않고 그 항목을 건너뛰어
    /// [`Summary::failures`]에 담는다. 실패한 항목이 있으면 작업은 [`JobState::Failed`]로 끝난다.
    pub fn copy(&mut self, sources: Vec<PathBuf>, target: PathBuf) -> Result<JobId, CopyRejection> {
        let job = copy::CopyJob::prepare(sources, target)?;
        Ok(self.enqueue(Job::Copy(job)))
    }

    fn enqueue(&mut self, job: Job) -> JobId {
        let id = JobId(self.next_id);
        self.next_id += 1;
        self.unfinished.fetch_add(1, Ordering::AcqRel);
        // 대기 상태를 일꾼에게 넘기기 전에 보내 이 작업의 다른 사건보다 먼저 온다.
        self.outbox.send(JobEvent::StateChanged {
            job: id,
            state: JobState::Queued,
        });
        let sender = self
            .worker
            .get_or_insert_with(|| spawn_worker(self.outbox.clone(), Arc::clone(&self.unfinished)));
        if let Err(mpsc::SendError((id, job))) = sender.send((id, job)) {
            // 일꾼이 죽었으면(패닉) 새로 띄운다.
            let sender = spawn_worker(self.outbox.clone(), Arc::clone(&self.unfinished));
            let _ = sender.send((id, job));
            self.worker = Some(sender);
        }
        id
    }
}

fn spawn_worker(outbox: Outbox, unfinished: Arc<AtomicUsize>) -> Sender<(JobId, Job)> {
    let (sender, receiver) = mpsc::channel::<(JobId, Job)>();
    thread::Builder::new()
        .name("facet-jobs".into())
        .spawn(move || {
            for (id, job) in receiver {
                outbox.send(JobEvent::StateChanged {
                    job: id,
                    state: JobState::Running,
                });
                let summary = match job {
                    Job::Copy(job) => job.run(id, &outbox),
                };
                let state = if summary.failures.is_empty() {
                    JobState::Finished
                } else {
                    JobState::Failed
                };
                outbox.send(JobEvent::StateChanged { job: id, state });
                // 끝남을 보내기 전에 줄여, 끝남을 받은 쪽이 보면 이미 끝난 것으로 보인다.
                unfinished.fetch_sub(1, Ordering::AcqRel);
                outbox.send(JobEvent::Finished { job: id, summary });
            }
        })
        .expect("작업 일꾼 스레드를 띄우지 못했다");
    sender
}

/// 두 경로가 같은지(Windows는 대소문자를 가리지 않는다). 둘 다 정규화한 경로라야 한다.
fn same_path(a: &Path, b: &Path) -> bool {
    let a: Vec<_> = a.components().collect();
    let b: Vec<_> = b.components().collect();
    a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| same_component(x, y))
}

/// `path`가 `base`이거나 그 아래인지. 둘 다 정규화한 경로라야 한다.
fn is_within(path: &Path, base: &Path) -> bool {
    let path: Vec<_> = path.components().collect();
    let base: Vec<_> = base.components().collect();
    path.len() >= base.len() && base.iter().zip(&path).all(|(x, y)| same_component(x, y))
}

fn same_component(a: &std::path::Component<'_>, b: &std::path::Component<'_>) -> bool {
    if cfg!(windows) {
        a.as_os_str().to_string_lossy().to_lowercase()
            == b.as_os_str().to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 하위_경로를_가린다() {
        let base = Path::new("/a/b");
        assert!(is_within(Path::new("/a/b"), base));
        assert!(is_within(Path::new("/a/b/c"), base));
        assert!(!is_within(Path::new("/a/bc"), base));
        assert!(!is_within(Path::new("/a"), base));
        assert!(same_path(Path::new("/a/b"), base));
        assert!(!same_path(Path::new("/a/b/c"), base));
    }

    #[cfg(windows)]
    #[test]
    fn windows는_대소문자를_가리지_않는다() {
        assert!(is_within(Path::new(r"C:\Foo\Bar"), Path::new(r"c:\foo")));
        assert!(same_path(Path::new(r"C:\Foo"), Path::new(r"c:\FOO")));
    }

    #[test]
    fn 마지막_상태() {
        assert!(JobState::Finished.is_terminal());
        assert!(JobState::Failed.is_terminal());
        assert!(JobState::Cancelled.is_terminal());
        assert!(!JobState::Queued.is_terminal());
        assert!(!JobState::Running.is_terminal());
        assert!(!JobState::Paused.is_terminal());
        assert!(!JobState::AwaitingAnswer.is_terminal());
    }
}
