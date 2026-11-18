//! 썸네일 요청 대기열(#329): 화면이 주인(탭)마다 「지금 원하는 것」(보이는 칸, 그 위아래 근처)을 통째로 넘기면
//! 작업자 스레드들이 보이는 칸부터 만든다.
//!
//! - 차례: 모든 주인의 보이는 요청이 근처 요청보다 먼저이고, 같은 무리 안에서는 넘긴 차례다(주인끼리는 처음
//!   원한 차례).
//! - 원하는 목록에서 빠진 요청은 시작 전이면 만들지 않고, 만드는 중이면 취소 표지를 세운다. 표지가 선 요청의
//!   결과는 버린다(만드는 함수가 표지를 못 보고 끝냈어도).
//! - 계속 원하는 동안 결과를 낸 요청은 다시 만들지도 내지도 않는다. 목록에서 빠지면 그 기억도 지운다.
//! - 실패(취소 말고)는 키로 대기열이 사는 동안 기억해 다시 원하면 만들지 않고 같은 실패를 곧바로 낸다.
//! - 만드는 함수의 패닉은 작업자에서 잡아 `ThumbnailError::Panicked` 실패로 바꾼다.
//! - 결과는 작업 대기열과 같은 깨우기·받기 흐름(`set_waker`·`poll`·`wait_event`)으로 온다.
//!
//! `want`·`forget`은 잠금을 잠깐 잡을 뿐 만드는 함수를 기다리지 않는다(작업자는 만드는 동안 잠금을 놓는다).

use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use crate::thumbnail::{Thumbnail, ThumbnailError, ThumbnailKey};

/// 작업자 수 상한. 디코딩은 CPU와 디스크를 쓰므로 많아도 이득이 적고 메모리(디코딩 중 그림)만 는다.
pub const MAX_WORKERS: usize = 8;

/// 코어 수에 맞춘 작업자 수: 코어 하나는 UI 스레드 몫으로 남겨 코어 수 - 1, 최소 1, 최대 `MAX_WORKERS`.
pub fn worker_count(cores: usize) -> usize {
    cores.saturating_sub(1).clamp(1, MAX_WORKERS)
}

/// 요청 하나의 결과.
#[derive(Debug, Clone)]
pub struct ThumbnailDone {
    pub owner: u64,
    pub key: ThumbnailKey,
    pub result: Result<Arc<Thumbnail>, ThumbnailError>,
}

type Maker = dyn Fn(&ThumbnailKey, &AtomicBool) -> Result<Thumbnail, ThumbnailError> + Send + Sync;
type Waker = Arc<dyn Fn() + Send + Sync>;

/// 주인 하나가 지금 원하는 것.
struct Want {
    owner: u64,
    visible: Vec<ThumbnailKey>,
    nearby: Vec<ThumbnailKey>,
    /// `visible`과 `nearby`의 합.
    all: HashSet<ThumbnailKey>,
    /// 계속 원하는 동안 결과를 낸 키.
    delivered: HashSet<ThumbnailKey>,
}

/// 만드는 중인 요청.
struct Running {
    owner: u64,
    key: ThumbnailKey,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct State {
    /// 처음 원한 차례.
    wants: Vec<Want>,
    running: Vec<Running>,
    failed: HashMap<ThumbnailKey, ThumbnailError>,
    results: VecDeque<ThumbnailDone>,
    waker: Option<Waker>,
    shutdown: bool,
}

impl State {
    fn is_running(&self, owner: u64, key: &ThumbnailKey) -> bool {
        self.running
            .iter()
            .any(|r| r.owner == owner && &r.key == key)
    }

    /// 다음에 만들 요청: 모든 주인의 보이는 요청, 그다음 근처 요청.
    fn pick(&self) -> Option<(u64, ThumbnailKey)> {
        let ready = |want: &Want, key: &ThumbnailKey| {
            !want.delivered.contains(key)
                && !self.failed.contains_key(key)
                && !self.is_running(want.owner, key)
        };
        for nearby in [false, true] {
            for want in &self.wants {
                let keys = if nearby { &want.nearby } else { &want.visible };
                if let Some(key) = keys.iter().find(|k| ready(want, k)) {
                    return Some((want.owner, key.clone()));
                }
            }
        }
        None
    }

    fn cancel_unwanted(&self, owner: u64, wanted: Option<&HashSet<ThumbnailKey>>) {
        for r in self.running.iter().filter(|r| r.owner == owner) {
            if wanted.is_none_or(|w| !w.contains(&r.key)) {
                r.cancel.store(true, Ordering::SeqCst);
            }
        }
    }
}

struct Shared {
    state: Mutex<State>,
    /// 작업자를 깨운다(새 요청, 끝내기).
    work: Condvar,
    /// `wait_event`를 깨운다.
    event: Condvar,
    maker: Box<Maker>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// 썸네일 요청 대기열. 버리면 작업자에게 끝내라고 알리고 만드는 중인 요청에 취소 표지를 세운다(기다리지 않음).
pub struct ThumbnailQueue {
    shared: Arc<Shared>,
}

impl ThumbnailQueue {
    /// 작업자 `workers`개(0이면 1)와 만드는 함수로 대기열을 띄운다. 만드는 함수는 두 번째 인자인 취소 표지가
    /// 서면 되도록 빨리 그만두는 것이 좋다(결과는 어차피 버린다).
    pub fn new(
        workers: usize,
        maker: impl Fn(&ThumbnailKey, &AtomicBool) -> Result<Thumbnail, ThumbnailError>
        + Send
        + Sync
        + 'static,
    ) -> ThumbnailQueue {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            work: Condvar::new(),
            event: Condvar::new(),
            maker: Box::new(maker),
        });
        for i in 0..workers.max(1) {
            let shared = Arc::clone(&shared);
            thread::Builder::new()
                .name(format!("facet-thumbnail-{i}"))
                .spawn(move || work(&shared))
                .expect("썸네일 작업자 스레드를 띄우지 못했다");
        }
        ThumbnailQueue { shared }
    }

    /// 그 주인의 원하는 목록을 바꾼다. 빠진 요청은 시작 전이면 빼고 만드는 중이면 취소 표지를 세운다. 기억한
    /// 실패인 키는 만들지 않고 곧바로 그 실패를 낸다.
    pub fn want(&self, owner: u64, visible: Vec<ThumbnailKey>, nearby: Vec<ThumbnailKey>) {
        let mut all: HashSet<ThumbnailKey> = HashSet::with_capacity(visible.len() + nearby.len());
        all.extend(visible.iter().cloned());
        all.extend(nearby.iter().cloned());

        let mut state = self.shared.lock();
        state.cancel_unwanted(owner, Some(&all));
        let index = match state.wants.iter().position(|w| w.owner == owner) {
            Some(i) => i,
            None => {
                state.wants.push(Want {
                    owner,
                    visible: Vec::new(),
                    nearby: Vec::new(),
                    all: HashSet::new(),
                    delivered: HashSet::new(),
                });
                state.wants.len() - 1
            }
        };
        let State { wants, failed, .. } = &mut *state;
        let want = &mut wants[index];
        want.delivered.retain(|k| all.contains(k));
        want.visible = visible;
        want.nearby = nearby;
        want.all = all;

        // 기억한 실패는 곧바로 낸다(넘긴 차례로).
        let mut done = Vec::new();
        for key in want.visible.iter().chain(&want.nearby) {
            if let Some(error) = failed.get(key)
                && want.delivered.insert(key.clone())
            {
                done.push(ThumbnailDone {
                    owner,
                    key: key.clone(),
                    result: Err(*error),
                });
            }
        }
        let waker = if done.is_empty() {
            None
        } else {
            state.results.extend(done);
            state.waker.clone()
        };
        drop(state);
        self.shared.work.notify_all();
        if let Some(waker) = waker {
            self.shared.event.notify_all();
            waker();
        }
    }

    /// 그 주인의 요청을 모두 취소한다(폴더를 떠남·탭을 닫음).
    pub fn forget(&self, owner: u64) {
        let mut state = self.shared.lock();
        state.cancel_unwanted(owner, None);
        state.wants.retain(|w| w.owner != owner);
    }

    /// 결과가 올 때 부를 깨우기. 작업자 스레드에서 잠금 밖에서 부른다.
    pub fn set_waker(&self, waker: impl Fn() + Send + Sync + 'static) {
        self.shared.lock().waker = Some(Arc::new(waker));
    }

    /// 지금까지 온 결과들(온 차례).
    pub fn poll(&self) -> Vec<ThumbnailDone> {
        self.shared.lock().results.drain(..).collect()
    }

    /// 결과 하나를 `timeout`까지 기다린다.
    pub fn wait_event(&self, timeout: Duration) -> Option<ThumbnailDone> {
        let deadline = Instant::now() + timeout;
        let mut state = self.shared.lock();
        loop {
            if let Some(done) = state.results.pop_front() {
                return Some(done);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            state = self
                .shared
                .event
                .wait_timeout(state, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

impl Drop for ThumbnailQueue {
    fn drop(&mut self) {
        let mut state = self.shared.lock();
        state.shutdown = true;
        for r in &state.running {
            r.cancel.store(true, Ordering::SeqCst);
        }
        drop(state);
        self.shared.work.notify_all();
    }
}

/// 작업자 하나: 요청을 골라 잠금 밖에서 만들고 결과를 넣는다.
fn work(shared: &Shared) {
    let mut state = shared.lock();
    loop {
        if state.shutdown {
            return;
        }
        let Some((owner, key)) = state.pick() else {
            state = shared.work.wait(state).unwrap_or_else(|e| e.into_inner());
            continue;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        state.running.push(Running {
            owner,
            key: key.clone(),
            cancel: Arc::clone(&cancel),
        });
        drop(state);

        let result = catch_unwind(AssertUnwindSafe(|| (shared.maker)(&key, &cancel)))
            .unwrap_or(Err(ThumbnailError::Panicked));

        state = shared.lock();
        if let Some(i) = state
            .running
            .iter()
            .position(|r| Arc::ptr_eq(&r.cancel, &cancel))
        {
            state.running.swap_remove(i);
        }
        if cancel.load(Ordering::SeqCst) {
            continue;
        }
        let Some(want) = state
            .wants
            .iter_mut()
            .find(|w| w.owner == owner && w.all.contains(&key))
        else {
            continue;
        };
        want.delivered.insert(key.clone());
        if let Err(error) = result
            && error != ThumbnailError::Cancelled
        {
            state.failed.insert(key.clone(), error);
        }
        state.results.push_back(ThumbnailDone {
            owner,
            key,
            result: result.map(Arc::new),
        });
        let waker = state.waker.clone();
        drop(state);
        shared.event.notify_all();
        if let Some(waker) = waker {
            waker();
        }
        state = shared.lock();
    }
}
