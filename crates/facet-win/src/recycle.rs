//! 휴지통으로 보내기와 휴지통에서 복원하기(ADR-0005: 셸에 맡기는 파일 작업은 이 둘뿐).
//!
//! - [`recycle`]: 경로 여러 개를 셸의 `IFileOperation`으로 휴지통에 보낸다. 대화상자·진행 창을 띄우지 않고, 항목마다
//!   결과([`RecycleOutcome`])를 돌려준다. 보낸 항목은 휴지통 안에 생긴 항목([`RecycledItem`])을 함께 돌려준다.
//! - **몰래 영구 삭제하지 않는다**: 셸이 어떤 항목을 휴지통 대신 영구 삭제로 바꾸려 하면(휴지통에 들어가지 않는 큰
//!   항목, 휴지통이 없는 볼륨, 휴지통을 끈 볼륨) 그 항목을 지우지 않고 작업을 멈춘다. 그 항목은
//!   [`RecycleOutcome::NotRecyclable`], 뒤에 남은 항목은 [`RecycleOutcome::NotAttempted`]이고, 부르는 쪽이 사용자에게
//!   묻는다.
//! - [`restore`]: [`recycle`]이 돌려준 휴지통 항목을 원래 자리로 되돌린다. 휴지통 항목이 이미 없으면(휴지통을 비움)
//!   [`FileOpErrorKind::SourceMissing`], 원래 자리에 같은 이름이 있으면 덮어쓰지 않고
//!   [`FileOpErrorKind::TargetExists`]다.
//! - [`has_recycle_bin`]: 경로의 볼륨에 휴지통이 있는지 미리 안다(실행 전 경고용).
//!
//! # 스레드와 시간 제한
//!
//! 셸의 파일 작업 인터페이스는 다른 프로그램이 설치한 복사 훅 같은 셸 확장을 프로세스에 실을 수 있다. ADR-0006의
//! 「예외」에 따라 이 둘은 도우미 프로세스를 거치지 않고 UI 프로세스에서 부르되, **UI 스레드에서는 부르지 않는다**:
//! 부를 때마다 띄우는 전용 스레드에서 COM을 STA로 초기화해 셸을 부르고, 부른 스레드는 결과를 기다린다. 이 함수들
//! 자체도 결과를 기다리므로 앱은 작업 대기열의 일꾼 스레드에서 부른다.
//!
//! 셸 확장이 멈추면 셸 호출이 돌아오지 않는다. 그래서 기다림에 시간 제한([`SHELL_TIMEOUT`])을 둔다. 제한은 셸이
//! 마지막으로 진행을 알린 때부터 센다(항목이 많아 오래 걸리는 정상 작업은 끊지 않고, 아무 진행 없이 멈춘 호출만
//! 끊는다). 넘으면 끝나지 않은 항목을 [`FileOpErrorKind::TimedOut`]으로 돌려주고, 셸 스레드에는 그만두라고 알린다
//! (다음 진행 알림에서 작업을 멈춘다). 멈춘 셸 스레드는 죽일 수 없으므로 그대로 두고 기다리지 않는다. 시간 초과
//! 때 셸이 이미 손댄 항목 하나는 뒤늦게 휴지통에 들어갈 수 있다(영구 삭제는 아니다).
//!
//! 실패는 [`FileOpError`]이고 종류로 이유를 나눈다. 화면 문장은 없다(ADR-0007). 경로는 260자를 넘어도 된다.
//! Windows가 아니면 모두 [`FileOpErrorKind::Unsupported`]이고 [`has_recycle_bin`]은 `false`다.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::{Duration, Instant};

use crate::file_ops::{FileOpError, FileOpErrorKind, FileOpResult};

/// 셸 호출을 기다리는 시간 제한. 셸이 마지막으로 진행을 알린 때부터 센다.
///
/// 근거: 휴지통으로 보내기와 복원은 같은 볼륨 안의 이름 바꾸기라 항목 하나에 보통 수 밀리초다. 느린 디스크·많은
/// 항목에서도 진행 알림은 항목마다 오므로 알림 사이가 몇 초를 넘을 일이 없다. 그 사이가 30초를 넘으면 셸 확장이
/// 멈춘 것으로 보고 끊는다. 너무 짧으면 바이러스 검사기가 붙잡은 큰 폴더를 잘못 끊고, 너무 길면 사용자가 멈춘
/// 작업을 오래 본다.
pub const SHELL_TIMEOUT: Duration = Duration::from_secs(30);

/// 휴지통 안에 생긴 항목: 원래 경로와 휴지통 안의 경로. [`restore`]가 받는다.
///
/// 작업 기록에 남겨 두었다가 실행취소할 때 쓸 수 있게 두 경로만 담는다.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RecycledItem {
    original: PathBuf,
    recycled: PathBuf,
}

impl RecycledItem {
    /// 원래 경로와 휴지통 안의 경로로 만든다(작업 기록에서 되살릴 때).
    pub fn new(original: PathBuf, recycled: PathBuf) -> Self {
        RecycledItem { original, recycled }
    }

    /// 휴지통에 보내기 전의 경로. 복원하면 여기로 돌아온다.
    pub fn original(&self) -> &Path {
        &self.original
    }

    /// 휴지통 안의 경로(`<드라이브>\$Recycle.Bin\<사용자 SID>\$R...`).
    pub fn recycled(&self) -> &Path {
        &self.recycled
    }
}

/// 휴지통으로 보내기의 항목 하나 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecycleOutcome {
    /// 휴지통에 들어갔다.
    Recycled(RecycledItem),
    /// 셸이 휴지통 대신 영구 삭제하려 해서 지우지 않고 멈췄다(휴지통보다 큰 항목, 휴지통이 없는 볼륨 등).
    /// 항목은 그대로 있다.
    NotRecyclable,
    /// 앞 항목에서 멈춰 손대지 않았다. 항목은 그대로 있다.
    NotAttempted,
    /// 실패했다(원본 없음, 잠김, 접근 거부, 시간 초과 등). 항목은 그대로 있다.
    Failed(FileOpError),
}

/// `paths`(파일·폴더)를 휴지통으로 보내고 항목마다 결과를 `paths`와 같은 순서로 돌려준다. 시간 제한은
/// [`SHELL_TIMEOUT`].
///
/// 전용 STA 스레드에서 셸을 부르고 끝날 때까지 기다린다. UI 스레드에서 부르지 않는다.
pub fn recycle(paths: &[PathBuf]) -> Vec<RecycleOutcome> {
    recycle_with_timeout(paths, SHELL_TIMEOUT)
}

/// [`recycle`]을 시간 제한 `timeout`으로 한다(셸이 마지막으로 진행을 알린 때부터 센다).
pub fn recycle_with_timeout(paths: &[PathBuf], timeout: Duration) -> Vec<RecycleOutcome> {
    let state = Arc::new(RecycleState {
        watch: Watch::new(),
        paths: paths.to_vec(),
        outcomes: Mutex::new(vec![None; paths.len()]),
    });
    let worker = Arc::clone(&state);
    let finished = on_shell_thread("facet-recycle", timeout, &state.watch, move || {
        recycle_on_thread(&worker)
    });
    let fallback = match finished {
        Ok(Ok(())) => RecycleOutcome::NotAttempted,
        Ok(Err(error)) | Err(error) => RecycleOutcome::Failed(error),
    };
    state
        .outcomes()
        .iter()
        .map(|outcome| outcome.clone().unwrap_or_else(|| fallback.clone()))
        .collect()
}

/// 휴지통 항목 `item`을 원래 자리로 되돌리고, 되돌린 경로(원래 경로)를 돌려준다. 시간 제한은 [`SHELL_TIMEOUT`].
///
/// 내용·수정 시각·속성은 그대로 돌아온다(같은 볼륨 안의 이름 바꾸기). 원래 폴더가 없어졌으면 다시 만든다. 미리 본
/// 뒤 셸이 옮기기 직전에 같은 이름이 생기는 아주 드문 경우에도 덮어쓰지 않고 셸이 붙인 다른 이름(「이름 (2)」)으로
/// 되돌리며, 그때 돌려주는 경로가 원래 경로와 다르다.
/// 휴지통 항목이 이미 없으면 [`FileOpErrorKind::SourceMissing`], 원래 자리에 같은 이름이 있으면 덮어쓰지 않고
/// [`FileOpErrorKind::TargetExists`]다.
pub fn restore(item: &RecycledItem) -> FileOpResult<PathBuf> {
    restore_with_timeout(item, SHELL_TIMEOUT)
}

/// [`restore`]를 시간 제한 `timeout`으로 한다(셸이 마지막으로 진행을 알린 때부터 센다).
pub fn restore_with_timeout(item: &RecycledItem, timeout: Duration) -> FileOpResult<PathBuf> {
    let watch = Arc::new(Watch::new());
    let worker = Arc::clone(&watch);
    let item = item.clone();
    on_shell_thread("facet-restore", timeout, &watch, move || {
        restore_on_thread(&item, &worker)
    })?
}

/// `path`가 있는 볼륨에 휴지통이 있는가. 경로가 아직 없어도 된다.
///
/// 로컬 고정 디스크(내장 디스크, 붙인 VHD)는 있다. 이동식 디스크(USB 메모리·SD 카드), 네트워크 위치, CD·램 디스크,
/// 볼륨을 알 수 없는 경로는 없다. 미리 경고하려는 판단일 뿐이고, 사용자가 볼륨의 휴지통을 꺼 두었으면 있다고 나와도
/// [`recycle`]이 그 항목을 [`RecycleOutcome::NotRecyclable`]로 돌려준다.
pub fn has_recycle_bin(path: &Path) -> bool {
    #[cfg(windows)]
    {
        crate::windows_recycle::has_recycle_bin(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

/// [`recycle`]의 셸 스레드와 부른 스레드가 나누는 상태.
pub(crate) struct RecycleState {
    pub(crate) watch: Watch,
    pub(crate) paths: Vec<PathBuf>,
    outcomes: Mutex<Vec<Option<RecycleOutcome>>>,
}

impl RecycleState {
    pub(crate) fn outcomes(&self) -> MutexGuard<'_, Vec<Option<RecycleOutcome>>> {
        self.outcomes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 항목 `index`의 결과를 아직 없을 때만 적는다.
    pub(crate) fn settle(&self, index: usize, outcome: RecycleOutcome) {
        if let Some(slot) = self.outcomes().get_mut(index)
            && slot.is_none()
        {
            *slot = Some(outcome);
        }
    }
}

/// 셸 스레드의 진행 감시: 마지막 진행 시각과 「그만두라」 신호.
pub(crate) struct Watch {
    last_progress: Mutex<Instant>,
    cancelled: AtomicBool,
}

impl Watch {
    fn new() -> Self {
        Watch {
            last_progress: Mutex::new(Instant::now()),
            cancelled: AtomicBool::new(false),
        }
    }

    /// 셸이 진행을 알렸다: 시간 제한을 처음부터 다시 센다.
    pub(crate) fn touch(&self) {
        *self.last() = Instant::now();
    }

    /// 부른 쪽이 시간 초과로 기다림을 그만두었다. 셸 스레드는 다음 진행 알림에서 작업을 멈춘다.
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    fn last(&self) -> MutexGuard<'_, Instant> {
        self.last_progress
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// `work`를 새 전용 스레드에서 돌리고, 셸이 `timeout` 넘게 진행을 알리지 않으면 [`FileOpErrorKind::TimedOut`]으로
/// 기다림을 그만둔다(스레드는 버려 두고 `watch`로 그만두라고 알린다).
fn on_shell_thread<T: Send + 'static>(
    name: &str,
    timeout: Duration,
    watch: &Watch,
    work: impl FnOnce() -> T + Send + 'static,
) -> FileOpResult<T> {
    let (sender, receiver) = mpsc::channel();
    watch.touch();
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let _ = sender.send(work());
        })
        .map_err(|_| FileOpError::new(FileOpErrorKind::Other, None))?;
    loop {
        let deadline = *watch.last() + timeout;
        let wait = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(wait) {
            Ok(value) => return Ok(value),
            // 셸 스레드가 패닉했다.
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(FileOpError::new(FileOpErrorKind::Other, None));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if Instant::now() >= *watch.last() + timeout {
                    watch.cancelled.store(true, Ordering::SeqCst);
                    return Err(FileOpError::new(FileOpErrorKind::TimedOut, None));
                }
            }
        }
    }
}

#[cfg(windows)]
fn recycle_on_thread(state: &Arc<RecycleState>) -> FileOpResult<()> {
    crate::windows_recycle::recycle(state)
}

#[cfg(not(windows))]
fn recycle_on_thread(_state: &Arc<RecycleState>) -> FileOpResult<()> {
    Err(FileOpError::unsupported())
}

#[cfg(windows)]
fn restore_on_thread(item: &RecycledItem, watch: &Arc<Watch>) -> FileOpResult<PathBuf> {
    crate::windows_recycle::restore(item, watch)
}

#[cfg(not(windows))]
fn restore_on_thread(_item: &RecycledItem, _watch: &Arc<Watch>) -> FileOpResult<PathBuf> {
    Err(FileOpError::unsupported())
}

/// 시험용 주입: 실제로 만들기 어려운 경우를 흉내 낸다. 앱은 부르지 않는다.
///
/// - [`refuse_recycle_under`](fault::refuse_recycle_under): 셸이 휴지통 대신 영구 삭제하려는 경우. 휴지통이 없는
///   볼륨이나 휴지통보다 큰 항목을 CI 러너에서 만들 수 없어서, 그 폴더 아래 항목에 셸이 영구 삭제를 알린 것처럼
///   판정한다. 셸의 진행 알림(`PreDeleteItem`) 안에서 판정하므로 셸 작업을 멈추는 길은 실제와 같다.
/// - [`stall_shell_under`](fault::stall_shell_under): 셸 확장이 멈춘 경우. 그 폴더 아래 항목의 진행 알림에서 정한
///   시간 동안 돌아오지 않는다(시간 초과를 확인하려고).
///
/// 폴더로 범위를 정하므로 나란히 도는 다른 테스트에 번지지 않는다. 돌려준 값을 버리면 풀린다.
#[doc(hidden)]
pub mod fault {
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard};
    use std::time::Duration;

    #[derive(Clone)]
    enum Fault {
        Refuse,
        Stall(Duration),
    }

    static FAULTS: Mutex<Vec<(PathBuf, Fault)>> = Mutex::new(Vec::new());

    fn lock() -> MutexGuard<'static, Vec<(PathBuf, Fault)>> {
        FAULTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn add(folder: &Path, fault: Fault) -> Injected {
        let folder = folder.to_path_buf();
        lock().push((folder.clone(), fault));
        Injected(folder)
    }

    /// 돌려준 값이 살아 있는 동안 `folder` 아래 항목은 셸이 휴지통 대신 영구 삭제하려는 것으로 본다.
    #[must_use = "버리면 곧바로 주입이 풀린다"]
    pub fn refuse_recycle_under(folder: &Path) -> Injected {
        add(folder, Fault::Refuse)
    }

    /// 돌려준 값이 살아 있는 동안 `folder` 아래 항목의 진행 알림이 `stall`만큼 돌아오지 않는다.
    #[must_use = "버리면 곧바로 주입이 풀린다"]
    pub fn stall_shell_under(folder: &Path, stall: Duration) -> Injected {
        add(folder, Fault::Stall(stall))
    }

    /// 주입 하나. 버리면 풀린다.
    pub struct Injected(PathBuf);

    impl Drop for Injected {
        fn drop(&mut self) {
            let mut faults = lock();
            if let Some(index) = faults.iter().position(|(f, _)| *f == self.0) {
                faults.remove(index);
            }
        }
    }

    fn find(path: &Path, pick: impl Fn(&Fault) -> bool) -> Option<Fault> {
        lock()
            .iter()
            .find(|(folder, fault)| path.starts_with(folder) && pick(fault))
            .map(|(_, fault)| fault.clone())
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn refuses(path: &Path) -> bool {
        find(path, |fault| matches!(fault, Fault::Refuse)).is_some()
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn stall(path: &Path) -> Option<Duration> {
        match find(path, |fault| matches!(fault, Fault::Stall(_))) {
            Some(Fault::Stall(stall)) => Some(stall),
            _ => None,
        }
    }
}
