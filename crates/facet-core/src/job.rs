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
//! 작업은 기본으로 넣은 차례대로 하나씩 돈다. 사용자는 작업을 일시정지·이어하기([`JobQueue::pause`]·
//! [`JobQueue::resume`])·취소([`JobQueue::cancel`])하고, 대기 중인 작업을 「지금 시작」([`JobQueue::start_now`])해
//! 이미 도는 작업과 나란히 돌릴 수 있다. 상태가 바뀔 때마다 [`JobEvent::StateChanged`]를 보낸다. 끝난 작업에
//! 이것들을 부르면 아무 일도 없다.
//!
//! 작업이 사용자의 답을 기다려야 하면(**이름 충돌**, [`JobEvent::Conflict`]) 그 작업만 [`JobState::AwaitingAnswer`]로
//! 멈추고 질문 사건을 보낸다. 답을 기다리는 작업은 **차례를 놓는다**: 차례 실행의 다음 작업이 곧 시작할 수 있다.
//! 답([`JobQueue::answer_conflict`])을 받은 작업은 차례를 다시 기다리지 않고 그 자리에서 이어 돈다(그동안 차례를
//! 잡은 작업과 나란히). 답을 기다리는 작업도 취소할 수 있다. 오류 질문도 이 규칙을 따른다.
//!
//! 지금 있는 작업은 **복사**([`JobQueue::copy`])다. 진행·이름 충돌·오류 처리의 자세한 규칙은 그 문서에 있다.

mod copy;

use std::collections::{HashMap, VecDeque};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, SystemTime};

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
    /// 사용자가 멈췄다. 시작 전에 멈춘 작업은 차례가 와도 시작하지 않는다.
    Paused,
    /// 이름 충돌에 대한 사용자의 답을 기다린다. 차례를 잡고 있지 않다.
    AwaitingAnswer,
    /// 끝났다. 항목별 실패가 없다(이름 충돌로 건너뛴 항목은 있을 수 있다).
    Finished,
    /// 끝났지만 실패한 항목이 있다.
    Failed,
    /// 사용자가 취소했다. 이미 다 복사한 항목은 남는다.
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
    /// 이름 충돌에 건너뛰기(또는 더 새것만 덮어쓰기에서 원본이 새것이 아님)로 답해 건너뛴 항목 수(폴더면 그 폴더
    /// 하나로 센다).
    pub conflicts_skipped: u64,
    /// 이름 충돌에 덮어쓰기로 답해 덮어쓴 파일 수([`Summary::files_copied`]에도 든다).
    pub files_overwritten: u64,
    /// 대상에 같은 이름의 폴더가 있어 묻지 않고 합친 폴더 수([`Summary::folders_copied`]에는 들지 않는다).
    pub folders_merged: u64,
    /// 따라 들어가지 않고 건너뛴 심볼릭 링크·정션 수(파일 링크 포함). 링크 자체를 복사하는 것은 뒤 티켓이다.
    pub links_skipped: u64,
    /// 실패한 항목(폴더면 그 아래는 따로 세지 않는다).
    pub failures: Vec<ItemFailure>,
    /// 처리한 항목 수([`Progress::items_done`]와 같은 셈: 건너뛴 항목도 든다).
    pub items_done: u64,
    /// 하지 못한 항목 수: 센 항목 가운데 처리하지 못한 것. 끝까지 돈 작업은 0이고, 취소한 작업은 남은 만큼이다.
    /// 다 세기 전에 취소했으면([`Summary::all_counted`]가 `false`) 센 데까지의 수라 실제로는 더 많을 수 있다.
    /// 시작하기 전에 취소한 작업은 넘긴 원본 수다.
    pub items_remaining: u64,
    /// 원본을 다 셌는지. `true`면 `items_done + items_remaining`이 원본 전체 항목 수다.
    pub all_counted: bool,
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
    /// 상태가 바뀌었다. 넣으면 [`JobState::Queued`], 시작하면 [`JobState::Running`], 일시정지하면
    /// [`JobState::Paused`], 이어 가면 [`JobState::Running`](시작 전에 멈췄던 작업은 [`JobState::Queued`]),
    /// 끝나면 마지막 상태다.
    StateChanged {
        /// 작업.
        job: JobId,
        /// 새 상태.
        state: JobState,
    },
    /// 이름 충돌: 작업이 답을 기다린다. [`JobState::AwaitingAnswer`]의 [`JobEvent::StateChanged`] 바로 뒤에 온다.
    /// [`JobQueue::answer_conflict`]로 답한다.
    Conflict {
        /// 작업.
        job: JobId,
        /// 부딪힌 두 항목.
        conflict: Conflict,
    },
    /// 작업이 끝났다. 마지막 상태의 [`JobEvent::StateChanged`] 바로 뒤에 온다. 이 작업의 마지막 사건이다.
    Finished {
        /// 작업.
        job: JobId,
        /// 결과 요약.
        summary: Summary,
    },
}

/// 이름 충돌의 한 쪽 항목 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemKind {
    /// 파일(파일 링크 포함).
    File,
    /// 폴더(폴더 링크·정션 포함).
    Folder,
}

/// 이름 충돌의 한 쪽 항목: 화면이 두 쪽을 비교해 보일 정보.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictItem {
    /// 경로(이름은 마지막 조각).
    pub path: PathBuf,
    /// 종류.
    pub kind: ItemKind,
    /// 파일 크기(바이트). 폴더는 `None`(안을 세지 않는다).
    pub size: Option<u64>,
    /// 수정 시각. 읽지 못하면 `None`.
    pub modified: Option<SystemTime>,
}

/// **이름 충돌**: 복사할 항목과 같은 이름이 대상에 이미 있다. 폴더끼리는 묻지 않고 합치므로 질문이 오지 않는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// 복사할 원본 항목.
    pub source: ConflictItem,
    /// 대상에 이미 있는 항목.
    pub target: ConflictItem,
    /// 덮어쓰기·더 새것만 덮어쓰기를 고를 수 있는지. 파일과 폴더가 부딪히면 `false`이고 그 답은 거부된다.
    pub can_overwrite: bool,
    /// 덮어쓰면 대상 파일을 되살릴 수 없는지(영구 삭제). 이 spec의 덮어쓰기는 덮어쓸 수 있으면 늘 `true`다. 화면이
    /// 덮어쓰기 선택지에 그 사실을 보이는 데 쓴다(문장은 화면 몫).
    pub overwrite_is_permanent: bool,
}

/// 이름 충돌에 대한 답의 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Resolution {
    /// 대상 파일을 원본으로 덮어쓴다(대상 폴더 안 임시 이름에 다 쓴 뒤 바꿔치기, 읽기 전용 대상도).
    Overwrite,
    /// 이 항목(폴더면 그 아래 전부)을 하지 않는다.
    Skip,
    /// 둘 다 남긴다: 원본을 「이름 (2)」(있으면 (3) …, 파일은 확장자를 뒤에)로 복사한다.
    KeepBoth,
    /// 원본의 수정 시각이 대상보다 늦을 때만 덮어쓰고, 아니면 건너뛴다.
    OverwriteIfNewer,
}

impl Resolution {
    /// 대상을 덮어쓸 수 있는 답인지(파일과 폴더가 부딪히면 고를 수 없다).
    pub fn overwrites(self) -> bool {
        matches!(self, Resolution::Overwrite | Resolution::OverwriteIfNewer)
    }
}

/// 이름 충돌에 대한 답.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConflictAnswer {
    /// 답.
    pub resolution: Resolution,
    /// 「나머지에도 적용」: 그 작업의 남은 이름 충돌에 같은 답을 쓰고 더 묻지 않는다. 다만 그 답을 쓸 수 없는
    /// 충돌(덮어쓰기 답인데 파일과 폴더가 부딪힘)은 다시 묻는다.
    pub apply_to_rest: bool,
}

impl ConflictAnswer {
    /// 이 충돌에만 쓰는 답.
    pub fn once(resolution: Resolution) -> Self {
        ConflictAnswer {
            resolution,
            apply_to_rest: false,
        }
    }

    /// 남은 충돌에도 쓰는 답(「나머지에도 적용」).
    pub fn for_rest(resolution: Resolution) -> Self {
        ConflictAnswer {
            resolution,
            apply_to_rest: true,
        }
    }
}

/// 답을 받지 않은 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnswerError {
    /// 그 작업은 이름 충돌의 답을 기다리지 않는다(없는·끝난·취소 중인 작업 포함).
    NotAsking,
    /// 파일과 폴더가 부딪힌 충돌에 덮어쓰기 답을 했다. 작업은 계속 답을 기다린다.
    OverwriteNotAllowed,
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

impl Job {
    /// 시작하지 않은 채 취소할 때 남은 항목 수로 알릴 값: 넘긴 원본 수(그 아래는 세지 않았다).
    fn root_count(&self) -> u64 {
        match self {
            Job::Copy(job) => job.root_count(),
        }
    }

    /// 작업 스레드에서 끝까지(또는 취소될 때까지) 실행한다. 취소로 멈췄으면 `true`도 돌려준다.
    fn run(self, context: &Context<'_>) -> (Summary, bool) {
        match self {
            Job::Copy(job) => job.run(context),
        }
    }
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

/// 도는 작업 하나의 일시정지·취소 표지. 작업 스레드(복사의 진행 콜백, 항목 사이)와 훑기 스레드가 본다.
#[derive(Default)]
struct Control {
    flags: Mutex<Flags>,
    changed: Condvar,
}

#[derive(Default)]
struct Flags {
    paused: bool,
    cancelled: bool,
    /// 받은 이름 충돌의 답(작업 스레드가 가져간다).
    answer: Option<ConflictAnswer>,
}

impl Control {
    /// 일시정지면 풀리거나 취소될 때까지 기다린다. 계속해도 되면 `true`, 취소됐으면 `false`.
    fn proceed(&self) -> bool {
        let mut flags = lock(&self.flags);
        while flags.paused && !flags.cancelled {
            flags = self
                .changed
                .wait(flags)
                .unwrap_or_else(PoisonError::into_inner);
        }
        !flags.cancelled
    }

    fn set_paused(&self, paused: bool) {
        lock(&self.flags).paused = paused;
        self.changed.notify_all();
    }

    fn cancel(&self) {
        lock(&self.flags).cancelled = true;
        self.changed.notify_all();
    }

    /// 답이 오거나 취소될 때까지 기다린다. 취소됐으면 `None`.
    fn wait_answer(&self) -> Option<ConflictAnswer> {
        let mut flags = lock(&self.flags);
        loop {
            if flags.cancelled {
                return None;
            }
            if let Some(answer) = flags.answer.take() {
                return Some(answer);
            }
            flags = self
                .changed
                .wait(flags)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn set_answer(&self, answer: ConflictAnswer) {
        lock(&self.flags).answer = Some(answer);
        self.changed.notify_all();
    }
}

/// 도는 작업이 대기열과 주고받는 곳: 번호, 사건을 보낼 곳, 일시정지·취소 표지, 판(질문할 때 상태를 바꾼다).
struct Context<'a> {
    id: JobId,
    outbox: &'a Outbox,
    control: &'a Control,
    shared: &'a Shared,
}

impl Context<'_> {
    /// 이름 충돌을 묻고 답을 기다린다. 기다리는 동안 작업은 [`JobState::AwaitingAnswer`]이고 차례를 놓는다. 답이
    /// 오면 [`JobState::Running`]으로 이어 가고(상태는 답하는 쪽이 바꾼다), 취소되면 `None`.
    ///
    /// 일시정지 중이면 이어 갈 때까지 기다린 뒤 묻는다(일시정지와 답 기다림이 겹치지 않게).
    fn ask(&self, conflict: Conflict) -> Option<ConflictAnswer> {
        loop {
            let mut board = self.shared.board();
            let slot = board.jobs.get_mut(&self.id)?;
            if slot.cancelling {
                return None;
            }
            if slot.state == JobState::Paused {
                drop(board);
                if !self.control.proceed() {
                    return None;
                }
                continue;
            }
            slot.state = JobState::AwaitingAnswer;
            slot.can_overwrite = conflict.can_overwrite;
            self.outbox.send(JobEvent::StateChanged {
                job: self.id,
                state: JobState::AwaitingAnswer,
            });
            self.outbox.send(JobEvent::Conflict {
                job: self.id,
                conflict,
            });
            if board.turn == Some(self.id) {
                board.turn = None;
            }
            drop(board);
            self.shared.changed.notify_all();
            return self.control.wait_answer();
        }
    }
}

/// 잠금이 독(다른 스레드의 패닉)에 걸려도 값을 쓴다. 값은 한 번에 바뀌는 단위로만 고치므로 그대로 쓸 수 있다.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 대기열의 판: 끝나지 않은 작업과 차례. 작업 상태는 이 판에서만 바꾸고, 바꾼 그 잠금 안에서 상태 사건을 보내
/// 상태 사건의 차례가 판의 차례와 같다.
#[derive(Default)]
struct Board {
    /// 아직 시작하지 않은 작업의 차례(대기·시작 전 일시정지).
    pending: VecDeque<JobId>,
    /// 끝나지 않은 작업.
    jobs: HashMap<JobId, Slot>,
    /// 차례를 잡고 도는 작업. 차례 일꾼은 이것이 비어야 다음 작업을 시작한다. 답을 기다리는 작업은 차례를 놓는다.
    turn: Option<JobId>,
    /// 대기열을 버렸다. 차례 일꾼은 더 시작할 작업이 없으면 끝난다.
    closed: bool,
}

struct Slot {
    state: JobState,
    /// 시작하기 전이면 작업, 시작했으면 `None`.
    job: Option<Job>,
    control: Arc<Control>,
    /// 취소를 청했다(작업 스레드가 멈추면 취소로 끝난다).
    cancelling: bool,
    /// 답을 기다리는 이름 충돌에 덮어쓰기 답을 받을 수 있는지.
    can_overwrite: bool,
}

/// 시작한 작업: 번호, 작업, 그 표지.
type Started = (JobId, Job, Arc<Control>);

impl Board {
    /// 차례에서 다음에 시작할 작업(맨 앞의 대기 상태, 일시정지는 건너뜀)을 꺼내 실행 중으로 한다.
    fn take_next(&mut self, outbox: &Outbox) -> Option<Started> {
        let index = self.pending.iter().position(|id| {
            self.jobs
                .get(id)
                .is_some_and(|slot| slot.state == JobState::Queued)
        })?;
        let id = self.pending.remove(index)?;
        self.start(id, outbox)
    }

    /// 시작하지 않은 작업을 실행 중으로 하고 꺼낸다(차례에서는 부르는 쪽이 뺀다).
    fn start(&mut self, id: JobId, outbox: &Outbox) -> Option<Started> {
        let slot = self.jobs.get_mut(&id)?;
        let job = slot.job.take()?;
        slot.state = JobState::Running;
        outbox.send(JobEvent::StateChanged {
            job: id,
            state: JobState::Running,
        });
        Some((id, job, Arc::clone(&slot.control)))
    }
}

/// 판과 그 바뀜 알림. 차례 일꾼이 시작할 작업이 생기기를 기다린다.
#[derive(Default)]
struct Shared {
    board: Mutex<Board>,
    changed: Condvar,
}

impl Shared {
    fn board(&self) -> MutexGuard<'_, Board> {
        lock(&self.board)
    }
}

/// 작업 대기열. 앱 전체에 하나다.
///
/// 작업은 넣은 차례대로 하나씩 실행한다(차례 일꾼 스레드 하나, 처음 작업을 넣을 때 띄움). 사용자가 대기 중인
/// 작업을 [`JobQueue::start_now`]하면 그 작업만 따로 스레드를 띄워 차례를 기다리지 않고 나란히 돈다.
///
/// 대기열을 버려도 이미 시작한 작업과 대기 중인 작업은 끝까지 돈다(일시정지한 작업은 이을 쪽이 없으니 남는다).
pub struct JobQueue {
    next_id: u64,
    shared: Arc<Shared>,
    runner: bool,
    outbox: Outbox,
    receiver: Receiver<JobEvent>,
}

impl Default for JobQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for JobQueue {
    fn drop(&mut self) {
        self.shared.board().closed = true;
        self.shared.changed.notify_all();
    }
}

impl JobQueue {
    /// 빈 대기열.
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        JobQueue {
            next_id: 1,
            shared: Arc::default(),
            runner: false,
            outbox: Outbox {
                sender,
                waker: Arc::default(),
            },
            receiver,
        }
    }

    /// 사건을 보낼 때마다 부를 깨우기를 정한다. 일꾼 스레드에서, 또 상태를 바꾸는 이 대기열의 메서드 안에서
    /// 대기열의 잠금을 쥔 채 불린다. 그러니 깨우기 안에서 대기열을 다시 부르지 않는다(앱은 이벤트 루프만 깨운다).
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
        !self.shared.board().jobs.is_empty()
    }

    /// 끝나지 않은 작업의 지금 상태. 끝난(또는 없는) 작업이면 `None`.
    pub fn state(&self, job: JobId) -> Option<JobState> {
        self.shared.board().jobs.get(&job).map(|slot| slot.state)
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
    /// **이름 충돌**(대상에 같은 이름이 이미 있음)은 사용자에게 묻는다: 작업이 [`JobState::AwaitingAnswer`]가 되고
    /// [`JobEvent::Conflict`]를 보낸 뒤 [`JobQueue::answer_conflict`]의 답([`Resolution`])대로 푼다. 「나머지에도
    /// 적용」이면 그 작업의 남은 충돌에 같은 답을 쓴다. 같은 이름의 폴더끼리는 묻지 않고 합친다(안의 충돌은 다시
    /// 묻는다, 합친 폴더의 시각·속성은 대상 것을 그대로 둔다). 파일과 폴더가 부딪히면 덮어쓰기를 고를 수 없다.
    /// 덮어쓰기는 대상 폴더 안 임시 이름에 다 쓴 뒤 바꿔치기하므로, 복사가 실패하거나 취소되면 원래 파일이 그대로
    /// 남고 임시 파일은 남지 않는다. 덮어쓴 파일은 되살릴 수 없다(영구 삭제).
    ///
    /// **임시 동작**(질문으로 바꾸는 것은 뒤 티켓): 항목별 오류는 묻지 않고 그 항목을 건너뛰어
    /// [`Summary::failures`]에 담는다. 실패한 항목이 있으면 작업은 [`JobState::Failed`]로 끝난다.
    ///
    /// 일시정지하면 파일 경계를 기다리지 않고 파일 복사의 진행 콜백(덩어리 사이)에서 멈추고 훑기도 멈춘다. 취소하면
    /// 복사하다 만 파일은 지우고, 이미 다 복사한 항목은 남긴다.
    pub fn copy(&mut self, sources: Vec<PathBuf>, target: PathBuf) -> Result<JobId, CopyRejection> {
        let job = copy::CopyJob::prepare(sources, target)?;
        Ok(self.enqueue(Job::Copy(job)))
    }

    fn enqueue(&mut self, job: Job) -> JobId {
        let id = JobId(self.next_id);
        self.next_id += 1;
        {
            let mut board = self.shared.board();
            board.jobs.insert(
                id,
                Slot {
                    state: JobState::Queued,
                    job: Some(job),
                    control: Arc::default(),
                    cancelling: false,
                    can_overwrite: false,
                },
            );
            board.pending.push_back(id);
            // 대기 상태를 판에 올린 잠금 안에서 보내 이 작업의 다른 사건보다 먼저 온다.
            self.outbox.send(JobEvent::StateChanged {
                job: id,
                state: JobState::Queued,
            });
        }
        if !self.runner {
            self.runner = true;
            let shared = Arc::clone(&self.shared);
            let outbox = self.outbox.clone();
            thread::Builder::new()
                .name("facet-jobs".into())
                .spawn(move || run_in_turn(&shared, &outbox))
                .expect("작업 일꾼 스레드를 띄우지 못했다");
        }
        self.shared.changed.notify_all();
        id
    }

    /// 작업을 일시정지한다. 실행 중이면 파일 경계를 기다리지 않고 멈추고(훑기도 멈춘다), 대기 중이면 차례가 와도
    /// 시작하지 않고 뒤 작업이 먼저 돈다. 상태는 [`JobState::Paused`]가 되고 그 사건을 보낸다.
    ///
    /// 실행 중에 멈춘 작업은 차례를 쥔 채 멈춘다: 뒤에서 기다리는 작업은 시작하지 않는다(디스크를 잠시 다른 일에
    /// 쓰려고 멈추는 것이므로). 뒤 작업을 돌리려면 그 작업을 [`JobQueue::start_now`]한다.
    ///
    /// 대기·실행 중이 아니면(이미 일시정지, 취소 중, 끝남, 없는 작업) 아무 일도 하지 않고 `false`.
    pub fn pause(&mut self, job: JobId) -> bool {
        let mut board = self.shared.board();
        let Some(slot) = board.jobs.get_mut(&job) else {
            return false;
        };
        if slot.cancelling {
            return false;
        }
        match slot.state {
            JobState::Queued => {}
            JobState::Running => slot.control.set_paused(true),
            _ => return false,
        }
        slot.state = JobState::Paused;
        self.outbox.send(JobEvent::StateChanged {
            job,
            state: JobState::Paused,
        });
        true
    }

    /// 일시정지한 작업을 잇는다. 실행 중에 멈췄으면 멈춘 자리에서 이어 [`JobState::Running`], 시작 전에 멈췄으면
    /// 차례의 제자리로 돌아가 [`JobState::Queued`]가 된다(앞에 도는 작업이 없으면 곧 시작한다).
    ///
    /// 일시정지가 아니면 아무 일도 하지 않고 `false`.
    pub fn resume(&mut self, job: JobId) -> bool {
        let mut board = self.shared.board();
        let Some(slot) = board.jobs.get_mut(&job) else {
            return false;
        };
        if slot.cancelling || slot.state != JobState::Paused {
            return false;
        }
        let state = if slot.job.is_some() {
            JobState::Queued
        } else {
            slot.control.set_paused(false);
            JobState::Running
        };
        slot.state = state;
        self.outbox.send(JobEvent::StateChanged { job, state });
        drop(board);
        self.shared.changed.notify_all();
        true
    }

    /// 작업을 취소한다(대기·실행 중·일시정지).
    ///
    /// 시작하지 않은 작업은 곧바로 [`JobState::Cancelled`]와 끝남 사건을 보낸다(남은 항목 수는 넘긴 원본 수,
    /// [`Summary::all_counted`]는 `false`). 시작한 작업은 복사하던 파일을 그 자리에서 그만두고 만들다 만 파일을
    /// 지운 뒤 취소로 끝난다. 이미 다 복사한 항목은 되돌리지 않는다. 끝남 사건의 결과에 끝낸 항목 수와 남은 항목
    /// 수가 있다. 차례는 다음 작업으로 넘어간다.
    ///
    /// 거의 끝난 작업은 취소가 닿기 전에 마저 끝나 [`JobState::Finished`]로 끝날 수 있다. 끝났거나 이미 취소 중이면
    /// 아무 일도 하지 않고 `false`.
    pub fn cancel(&mut self, job: JobId) -> bool {
        let mut board = self.shared.board();
        let Some(slot) = board.jobs.get_mut(&job) else {
            return false;
        };
        if slot.cancelling {
            return false;
        }
        if slot.job.is_none() {
            slot.cancelling = true;
            slot.control.cancel();
            return true;
        }
        let slot = board.jobs.remove(&job).expect("방금 본 작업");
        board.pending.retain(|id| *id != job);
        let summary = Summary {
            items_remaining: slot.job.as_ref().map_or(0, Job::root_count),
            ..Summary::default()
        };
        self.outbox.send(JobEvent::StateChanged {
            job,
            state: JobState::Cancelled,
        });
        self.outbox.send(JobEvent::Finished { job, summary });
        true
    }

    /// 이름 충돌의 답을 기다리는 작업에 답한다. 작업은 [`JobState::Running`]이 되어(그 상태 사건을 보낸다) 차례를
    /// 다시 기다리지 않고 그 자리에서 이어 돈다.
    ///
    /// 답을 기다리지 않는 작업이면 [`AnswerError::NotAsking`]. 덮어쓸 수 없는 충돌(파일과 폴더)에 덮어쓰기·더
    /// 새것만 덮어쓰기로 답하면 [`AnswerError::OverwriteNotAllowed`]이고 작업은 계속 답을 기다린다.
    pub fn answer_conflict(
        &mut self,
        job: JobId,
        answer: ConflictAnswer,
    ) -> Result<(), AnswerError> {
        let mut board = self.shared.board();
        let Some(slot) = board.jobs.get_mut(&job) else {
            return Err(AnswerError::NotAsking);
        };
        if slot.cancelling || slot.state != JobState::AwaitingAnswer {
            return Err(AnswerError::NotAsking);
        }
        if answer.resolution.overwrites() && !slot.can_overwrite {
            return Err(AnswerError::OverwriteNotAllowed);
        }
        slot.state = JobState::Running;
        self.outbox.send(JobEvent::StateChanged {
            job,
            state: JobState::Running,
        });
        slot.control.set_answer(answer);
        Ok(())
    }

    /// 대기 중인 작업을 차례를 기다리지 않고 바로 시작한다. 이미 도는 작업과 나란히 돌고, 차례 실행은 그대로
    /// 이어진다(이 작업이 끝나도 차례의 다음 작업은 앞 작업이 끝나야 시작한다).
    ///
    /// 시작 전에 일시정지한 작업도 바로 시작한다(이어하기를 겸한다). 시작한 작업·끝난 작업이면 아무 일도 하지
    /// 않고 `false`.
    pub fn start_now(&mut self, job: JobId) -> bool {
        let mut board = self.shared.board();
        if !board.jobs.get(&job).is_some_and(|slot| slot.job.is_some()) {
            return false;
        }
        board.pending.retain(|id| *id != job);
        let Some((id, job, control)) = board.start(job, &self.outbox) else {
            return false;
        };
        drop(board);
        let shared = Arc::clone(&self.shared);
        let outbox = self.outbox.clone();
        thread::Builder::new()
            .name("facet-job-now".into())
            .spawn(move || run_job(id, job, &control, &shared, &outbox))
            .expect("작업 스레드를 띄우지 못했다");
        true
    }
}

/// 차례 일꾼: 차례를 잡은 작업이 없으면 차례의 맨 앞 대기 작업을 시작해 차례를 준다. 작업은 따로 스레드에서
/// 돌고, 끝나거나 답을 기다리느라 차례를 놓으면 다음 작업을 시작한다. 대기열을 버렸는데 시작할 작업이 없으면
/// 끝난다.
fn run_in_turn(shared: &Arc<Shared>, outbox: &Outbox) {
    loop {
        let (id, job, control) = {
            let mut board = shared.board();
            loop {
                if board.turn.is_none()
                    && let Some(next) = board.take_next(outbox)
                {
                    board.turn = Some(next.0);
                    break next;
                }
                let startable = board.pending.iter().any(|id| {
                    board
                        .jobs
                        .get(id)
                        .is_some_and(|slot| slot.state == JobState::Queued)
                });
                if board.closed && !startable {
                    return;
                }
                board = shared
                    .changed
                    .wait(board)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        let shared = Arc::clone(shared);
        let outbox = outbox.clone();
        thread::Builder::new()
            .name("facet-job".into())
            .spawn(move || run_job(id, job, &control, &shared, &outbox))
            .expect("작업 스레드를 띄우지 못했다");
    }
}

/// 작업 하나를 이 스레드에서 끝까지 실행하고 마지막 상태와 끝남 사건을 보낸다. 차례를 잡고 있었으면 놓는다.
fn run_job(id: JobId, job: Job, control: &Control, shared: &Shared, outbox: &Outbox) {
    let context = Context {
        id,
        outbox,
        control,
        shared,
    };
    let result = panic::catch_unwind(AssertUnwindSafe(|| job.run(&context)));
    let (summary, state) = match result {
        Ok((summary, true)) => (summary, JobState::Cancelled),
        Ok((summary, false)) if summary.failures.is_empty() => (summary, JobState::Finished),
        Ok((summary, false)) => (summary, JobState::Failed),
        // 작업 코드의 버그: 대기열은 계속 돌게 하고 그 작업만 실패로 끝낸다.
        Err(_) => (Summary::default(), JobState::Failed),
    };
    let mut board = shared.board();
    // 끝남을 보내기 전에 판에서 빼, 끝남을 받은 쪽이 보면 이미 끝난 것으로 보인다.
    board.jobs.remove(&id);
    if board.turn == Some(id) {
        board.turn = None;
    }
    outbox.send(JobEvent::StateChanged { job: id, state });
    outbox.send(JobEvent::Finished { job: id, summary });
    drop(board);
    shared.changed.notify_all();
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
