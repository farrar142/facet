//! 셸 호출용 전용 STA 스레드와 시간 제한(S12, ADR-0002·ADR-0006 「예외」).
//!
//! [`ShellThread`]는 COM을 STA로 초기화하고 메시지 루프를 돌리는 스레드 하나에 일을 보내고 결과를 시간 제한 안에
//! 받는다. 스레드는 처음 요청할 때 만들고(콜드 스타트에 들지 않음), 버리기 전까지 모든 요청이 그 스레드에서 차례로
//! 돈다. 시간 안에 답이 없으면 그 스레드를 버린다: 강제로 끝내지 않고(셸 호출 도중에 스레드를 죽이면 셸의 잠금이
//! 풀리지 않음) 일이 끝나면 스스로 끝나게 두며, 다음 요청은 새 스레드에서 한다. 요청마다 제 결과 통로를 가지므로
//! 버린 스레드가 늦게 낸 결과는 아무 데도 가지 않는다(받을 쪽이 이미 없음). 일이 패닉하면 그 요청만 `Failed`이고
//! 스레드는 계속 쓴다.
//!
//! 뒤의 셸 공급자(내 PC·휴지통)와 휴대폰 장치마다의 스레드가 이것을 쓴다. Windows가 아니면 COM 없이 보통 스레드에서
//! 돌고 [`apartment_is_sta`]는 늘 거짓이다.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

#[cfg(windows)]
use crate::windows_shell_thread as platform;

/// 전용 스레드에 보내는 일 하나.
type Job = Box<dyn FnOnce() + Send + 'static>;

/// 셸 호출을 받아 도는 전용 COM STA 스레드.
///
/// 여러 스레드가 함께 써도 되고(`Send + Sync`), 요청은 같은 스레드에서 넣은 차례대로 하나씩 돈다.
pub struct ShellThread {
    worker: Mutex<Option<Worker>>,
}

/// 지금 쓰는 스레드. 버리면 일 통로가 닫혀, 스레드는 하던 일을 마친 뒤 끝난다.
struct Worker {
    /// 이 스레드의 번호. 시간 초과한 요청이 그 사이 이미 새로 만든 스레드를 버리지 않게 견준다.
    generation: u64,
    jobs: Sender<Job>,
    signal: Arc<Signal>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        // 통로를 닫은 것을 스레드가 알아채게 깨운다(통로는 이 뒤에 버려지므로 먼저 닫는다).
        let (closed, _) = mpsc::channel();
        drop(std::mem::replace(&mut self.jobs, closed));
        self.signal.set();
    }
}

impl ShellThread {
    /// 스레드는 아직 만들지 않는다(처음 요청할 때 만든다).
    #[allow(clippy::new_without_default)]
    pub fn new() -> ShellThread {
        ShellThread {
            worker: Mutex::new(None),
        }
    }

    /// `job`을 전용 스레드에서 돌려 `timeout` 안에 결과를 받는다.
    ///
    /// 시간 안에 끝나지 않으면 기다리지 않고 [`CallError::TimedOut`]을 돌려주고 그 스레드를 버린다(다음 요청은 새
    /// 스레드). 일이 패닉했거나 스레드를 만들지 못하면 [`CallError::Failed`]다. 시간 제한은 앞서 넣은 일을 기다리는
    /// 시간도 포함한다.
    pub fn call<T: Send + 'static>(
        &self,
        timeout: Duration,
        job: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, CallError> {
        let (reply, result) = mpsc::sync_channel(1);
        let job: Job = Box::new(move || {
            let outcome = catch_unwind(AssertUnwindSafe(job)).map_err(|_| CallError::Failed);
            // 부른 쪽이 시간 초과로 떠났으면 받을 쪽이 없다: 늦은 결과는 버린다.
            let _ = reply.send(outcome);
        });

        let generation = {
            let mut slot = self.lock();
            let worker = match slot.take() {
                Some(worker) => worker,
                None => spawn_worker().ok_or(CallError::Failed)?,
            };
            let generation = worker.generation;
            let sent = worker.jobs.send(job);
            if sent.is_err() {
                // 스레드가 이미 끝났다(일어나지 않아야 함): 버리고 실패로 답한다.
                return Err(CallError::Failed);
            }
            worker.signal.set();
            *slot = Some(worker);
            generation
        };

        match result.recv_timeout(timeout) {
            Ok(outcome) => outcome,
            Err(RecvTimeoutError::Timeout) => {
                let mut slot = self.lock();
                if slot
                    .as_ref()
                    .is_some_and(|worker| worker.generation == generation)
                {
                    // 강제로 끝내지 않는다: 통로만 닫아 일이 끝나면 스스로 끝나게 둔다.
                    *slot = None;
                }
                Err(CallError::TimedOut)
            }
            // 일이 결과를 보내지 못한 채 사라졌다(스레드가 끝남).
            Err(RecvTimeoutError::Disconnected) => Err(CallError::Failed),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Worker>> {
        // 잠금 안에서는 패닉할 일이 없지만, 독이 들어도 상태(스레드 손잡이)는 그대로 쓸 수 있다.
        self.worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// 셸 호출이 결과를 내지 못한 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallError {
    /// 시간 제한 안에 끝나지 않았다. 그 스레드는 버렸다.
    TimedOut,
    /// 일이 패닉했거나 스레드를 만들지 못했다.
    Failed,
}

/// 부른 스레드가 COM STA로 초기화돼 있는가. Windows가 아니면 늘 거짓이다.
pub fn apartment_is_sta() -> bool {
    #[cfg(windows)]
    {
        platform::apartment_is_sta()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// 새 스레드를 띄운다. 띄우지 못하면 `None`.
fn spawn_worker() -> Option<Worker> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);

    let signal = Arc::new(Signal::new()?);
    let (jobs, inbox) = mpsc::channel::<Job>();
    let wake = Arc::clone(&signal);
    thread::Builder::new()
        .name("facet-shell-thread".to_owned())
        .spawn(move || run(&wake, inbox))
        .ok()?;
    Some(Worker {
        generation: NEXT.fetch_add(1, Ordering::Relaxed),
        jobs,
        signal,
    })
}

/// 스레드 본체: 통로가 닫힐 때까지 일을 돌린다.
fn run(signal: &Signal, inbox: mpsc::Receiver<Job>) {
    #[cfg(windows)]
    {
        platform::run(&signal.0, || drain(&inbox));
    }
    #[cfg(not(windows))]
    {
        let _ = signal;
        for job in inbox {
            job();
        }
    }
}

/// 쌓인 일을 모두 돌린다. 통로가 닫혔으면 `false`(스레드를 끝냄).
#[cfg_attr(not(windows), allow(dead_code))]
fn drain(inbox: &mpsc::Receiver<Job>) -> bool {
    loop {
        match inbox.try_recv() {
            Ok(job) => job(),
            Err(mpsc::TryRecvError::Empty) => return true,
            Err(mpsc::TryRecvError::Disconnected) => return false,
        }
    }
}

/// 스레드를 깨우는 표지. Windows에서는 메시지 루프가 메시지와 함께 기다리는 이벤트다.
struct Signal(#[cfg(windows)] platform::Event);

impl Signal {
    fn new() -> Option<Signal> {
        #[cfg(windows)]
        {
            platform::Event::new().map(Signal)
        }
        #[cfg(not(windows))]
        {
            Some(Signal())
        }
    }

    fn set(&self) {
        #[cfg(windows)]
        self.0.set();
    }
}
