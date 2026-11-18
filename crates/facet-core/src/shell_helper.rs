//! 셸 확장 도우미 관리(spec #24, ADR-0006): 셸 확장을 UI 프로세스 대신 싣는 도우미 프로세스를 띄우고, 파이프로
//! 요청을 보내 응답을 사건으로 받는다. 화면도 셸도 부르지 않는다(셸 확장을 부르는 코드는 도우미 실행 파일에만 있다).
//!
//! [`ShellHelper::start`]는 곧바로 돌아오고 도우미를 띄우지 않는다: **첫 요청 때** 띄운다(콜드 스타트에 끼지 않게).
//! 띄우기·연결·버전 인사·요청 쓰기는 관리 일꾼 스레드에서, 응답 읽기는 연결마다 읽는 스레드에서 한다. 부르는
//! 쪽(UI 스레드)은 무엇도 기다리지 않는다: 요청은 번호([`RequestId`])만 받고 결과는 사건([`HelperEvent`])으로 온다.
//! 사건은 인덱서 연결(`indexer_client`)과 같은 깨우기([`ShellHelper::set_waker`])와 받기([`ShellHelper::poll`]·
//! [`ShellHelper::wait_event`]) 흐름이다.
//!
//! # 연결
//!
//! 도우미를 띄울 때마다 이 프로세스가 사적인 파이프(`\\.\pipe\facet-shell-helper-<UI 프로세스 번호>-<무작위 값>`,
//! [`facet_win::pipe_client::private`])를 만들고 그 이름을 도우미 명령 줄(`--pipe <이름> --parent <감시할 프로세스
//! 번호>` 뒤에 [`HelperOptions::args`])로 넘긴다. 파이프의 보안 설명자는 현재 사용자만 열게 하고(NETWORK 거부),
//! 인스턴스는 하나뿐이라 도우미 하나만 붙는다. 무작위 값은 이름을 미리 알아 차지하지 못하게 한다. 붙으면 프로토콜
//! 버전 인사를 먼저 하고, 버전이 맞지 않는 도우미는 끝내고 그 요청을 [`RequestError::VersionMismatch`]로 끝낸다.
//!
//! # 요청과 결과
//!
//! - 메뉴([`ShellHelper::menu`])·항목 실행([`ShellHelper::invoke`], 메뉴를 만든 요청 번호와 항목 번호).
//! - 속성 대화상자([`ShellHelper::properties`])·「연결 프로그램」 대화상자([`ShellHelper::open_with`], #321). 도우미는
//!   띄우자마자 답하고, 대화상자가 떠 있는 동안 쉬면 끝내기를 미룬다.
//! - 응답이 쓴 때부터 [`HelperOptions::reply_timeout`](기본 [`REPLY_TIMEOUT`], 2초) 안에 오지 않으면
//!   [`RequestError::TimedOut`]으로 끝나고, 그 뒤 늦게 온 응답은 버린다.
//! - 메뉴를 놓으면([`ShellHelper::release`]) 기다리던 그 메뉴 요청은 아무 사건도 내지 않는다(이미 쌓인 사건도 지운다).
//! - 도우미가 죽으면(연결이 끊기면) 그 도우미에 보낸 기다리던 요청은 모두 [`RequestError::Disconnected`]로 끝나고,
//!   다음 요청 때 새 도우미를 띄운다.
//!
//! # 수명(#320)
//!
//! - **멈춤**: 도우미에 쓴 요청이 [`HelperOptions::stall_timeout`](기본 [`STALL_TIMEOUT`], 10초) 동안 아무 응답 없이
//!   도우미를 붙잡고 있으면 멈춘 것으로 보고 끝낸다. 다음 요청 때 새 도우미를 띄운다. 멈춤은 죽음으로 세지 않는다.
//! - **연달아 죽음**: 도우미가 스스로(종료 코드 0이 아니게) 끝나면 죽음으로 센다. [`HelperOptions::crash_window`] 안에
//!   [`HelperOptions::crash_limit`]을 넘게 죽으면 [`HelperOptions::rest`] 동안 띄우지 않고, 그동안의 요청은 곧바로
//!   [`RequestError::Resting`]으로 끝난다([`ShellHelper::is_resting`]). 쉬기 시작하면 센 죽음은 비운다.
//! - **쉬면 끝내기**: [`HelperOptions::idle_timeout`]을 도우미에 넘긴다(`--idle-timeout-ms`). 도우미는 그동안 요청이
//!   없으면 종료 코드 0으로 스스로 끝나고, 그것은 죽음이 아니다. 끝나는 순간과 새 요청이 엇갈리지 않게, 관리 쪽도 그
//!   시간에 거의 닿은 도우미에는 쓰지 않고 조용히 끝낸 뒤 새로 띄운다.
//! - **부모 감시**: 도우미에 `--parent`(기본은 지금 프로세스)를 넘기고, 도우미는 그 프로세스가 끝나면 끝난다.
//! - **죽게 한 확장**: 띄울 때마다 공유 상태 파일(임시 폴더의 `facet-shell-helper-<번호>-<무작위 값>.state`)을
//!   `--crash-state`로 넘긴다. 도우미는 확장을 부르기 직전에 CLSID·DLL 경로·이름을 한 줄씩 적는다. 죽으면 그것을 읽어
//!   [`HelperOptions::crash_log`]에 `YYYY-MM-DD HH:MM:SS<탭>CLSID<탭>DLL<탭>이름` 한 줄을 덧붙이고(지역 시각, 폴더가
//!   없으면 만듦, 쓰지 못해도 돈다), 같은 확장으로 [`SAME_EXTENSION_CRASHES`]번 죽으면
//!   [`ShellHelper::crashing_extension`]으로 알린다. 세기와 알림은 기록 파일과 상관없다. 확장을 막지는 않는다.
//!
//! 화면 문장은 없다(ADR-0007).

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use facet_shell_protocol::{
    MAX_SUPPORTED_VERSION, MIN_SUPPORTED_VERSION, MenuItem, MenuTarget, PROTOCOL_VERSION, Request,
    Response, read_message, write_message,
};
use facet_win::pipe_client::private::PrivatePipe;
use facet_win::pipe_client::{PipeCloser, PipeReader, PipeWriter};

use crate::menu::MenuKind;

/// 응답 대기 시간(2초).
///
/// 근거: 셸 메뉴는 사람이 오른쪽 클릭을 하고 기다리는 것이라, 보통의 확장이 답하는 시간(수십~수백 ms)보다 넉넉하고
/// 사람이 「멈췄다」고 느끼기 전에 facet 항목만으로라도 메뉴를 쓰게 하는 값이다(spec #24가 정한 값).
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(2);
/// 멈춤 판정 시간(10초, spec #24가 정한 값): 쓴 요청이 이만큼 응답 없이 도우미를 붙잡고 있으면 도우미를 끝낸다.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(10);
/// 쉬면 끝내기 시간(5분, spec #24가 정한 값).
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// 연달아 죽음을 세는 시간(1분, spec #24가 정한 값).
pub const CRASH_WINDOW: Duration = Duration::from_secs(60);
/// 그 시간 안에 이 수를 넘게 죽으면 쉰다(3, spec #24가 정한 값).
pub const CRASH_LIMIT: u32 = 3;
/// 쉬는 시간(1분, spec #24가 정한 값).
pub const REST_DURATION: Duration = Duration::from_secs(60);
/// 같은 확장으로 이만큼 죽으면 알린다(3, spec #24가 정한 값).
pub const SAME_EXTENSION_CRASHES: u32 = 3;

/// 연결이 끊긴 뒤 도우미 프로세스가 끝나기를 기다리는 가장 긴 시간. 넘으면 끝내고 죽음으로 센다.
///
/// 근거: 파이프는 프로세스가 끝나며 닫히므로 보통 곧바로 끝나 있다. 끝나는 중인 프로세스를 넉넉히 기다리되 일꾼을
/// 오래 막지 않는 값이다.
const EXIT_WAIT: Duration = Duration::from_secs(2);
/// 끝나기를 기다리는 동안 보는 간격.
const EXIT_POLL: Duration = Duration::from_millis(10);
/// 쉬면 끝내기 시간에서 이 몫(1/10, 많아야 1초) 안쪽으로 닿은 도우미에는 쓰지 않고 새로 띄운다(엇갈림을 피함).
const IDLE_MARGIN_MAX: Duration = Duration::from_secs(1);
/// 쉬면 끝내기 시간이 지난 뒤 도우미가 스스로 끝나기까지 기다려 주는 시간(#321). 이보다 더 살아 있으면 대화상자 때문에
/// 끝내기를 미룬 것으로 본다.
///
/// 근거: 도우미는 쉬는 시간을 많아야 0.5초마다 확인하므로 그 두 배면 미루지 않은 도우미는 끝나 있다.
const IDLE_EXIT_GRACE: Duration = Duration::from_secs(1);

/// 도우미를 띄워 버전 인사를 마칠 때까지의 가장 긴 시간. 넘으면 그 도우미를 끝내고 요청을
/// [`RequestError::Spawn`]([`std::io::ErrorKind::TimedOut`])으로 끝낸다.
///
/// 근거: 프로세스를 띄워 파이프에 붙고 인사하는 데는 보통 수십 ms다. 느린 기계에서 바이러스 검사가 새 실행 파일을
/// 붙잡아도 견디되, 끝내 붙지 않는 도우미가 일꾼을 오래 막지 않게 멈춤 판정 시간과 같은 10초로 둔다.
const START_TIMEOUT: Duration = Duration::from_secs(10);

/// 파이프 연결을 기다리는 동안 도우미가 먼저 끝났는지 보는 간격.
const ACCEPT_POLL: Duration = Duration::from_millis(50);

/// 도우미 관리 설정.
#[derive(Debug, Clone)]
pub struct HelperOptions {
    /// 도우미 실행 파일.
    pub program: PathBuf,
    /// 도우미 명령 줄 끝에 붙일 인자(시험용 모드 `--fake` 등).
    pub args: Vec<OsString>,
    /// 도우미가 감시할 UI 프로세스 번호. `None`이면 지금 프로세스.
    pub parent: Option<u32>,
    /// 응답 대기 시간.
    pub reply_timeout: Duration,
    /// 멈춤 판정 시간(#320).
    pub stall_timeout: Duration,
    /// 쉬면 끝내기 시간(#320).
    pub idle_timeout: Duration,
    /// 연달아 죽음을 세는 시간(#320).
    pub crash_window: Duration,
    /// 쉬기 전에 견디는 죽음 수(#320).
    pub crash_limit: u32,
    /// 쉬는 시간(#320).
    pub rest: Duration,
    /// 죽은 기록 파일(#320). `None`이면 남기지 않는다.
    pub crash_log: Option<PathBuf>,
}

impl HelperOptions {
    /// 기본값의 설정(`args` 비어 있음, `parent: None`, `crash_log: default_crash_log()`).
    pub fn new(program: impl Into<PathBuf>) -> HelperOptions {
        HelperOptions {
            program: program.into(),
            args: Vec::new(),
            parent: None,
            reply_timeout: REPLY_TIMEOUT,
            stall_timeout: STALL_TIMEOUT,
            idle_timeout: IDLE_TIMEOUT,
            crash_window: CRASH_WINDOW,
            crash_limit: CRASH_LIMIT,
            rest: REST_DURATION,
            crash_log: default_crash_log(),
        }
    }
}

/// 기본 죽은 기록 파일(`%LOCALAPPDATA%\facet\helper-crashes.txt`). `LOCALAPPDATA`가 없으면 `None`.
pub fn default_crash_log() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty())?;
    Some(PathBuf::from(base).join("facet").join("helper-crashes.txt"))
}

/// 지금 실행 파일 옆의 `facet-shell-helper.exe`. 지금 실행 파일 자리를 모르면 이름만.
pub fn default_program() -> PathBuf {
    const NAME: &str = "facet-shell-helper.exe";
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(NAME)))
        .unwrap_or_else(|| PathBuf::from(NAME))
}

type Waker = Arc<dyn Fn() + Send + Sync>;

/// 기다리는 요청 하나.
struct Pending {
    /// 도우미에 쓴 뒤의 마감. 쓰기 전이면 `None`.
    deadline: Option<Instant>,
    /// 쓴 연결(도우미)의 번호. 쓰기 전이면 0.
    session: u64,
}

/// 부르는 쪽과 일꾼이 함께 보는 상태. 한 잠금 아래에서 기다리는 요청과 쌓인 사건을 함께 바꾸므로, 놓은 메뉴의 결과가
/// 놓은 뒤에 쌓이지 않는다.
#[derive(Default)]
struct State {
    events: VecDeque<HelperEvent>,
    pending: HashMap<u64, Pending>,
    /// 메뉴 요청 번호 → 그 대상(항목 실행 요청에 함께 보낸다). 메뉴를 놓으면 빠진다.
    menus: HashMap<u64, MenuTarget>,
    process_id: Option<u32>,
    /// 쉬는 중이면 끝나는 때.
    resting_until: Option<Instant>,
    /// 같은 확장으로 [`SAME_EXTENSION_CRASHES`]번 넘게 죽게 한 확장(마지막에 그렇게 된 것).
    crashing: Option<CrashedExtension>,
}

struct Shared {
    state: Mutex<State>,
    ready: Condvar,
    waker: Mutex<Option<Waker>>,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 아직 기다리는 요청이면 끝내고 사건을 쌓는다. 놓았거나 이미 끝난 요청이면 아무것도 하지 않는다.
    fn finish(&self, id: u64, result: Result<Reply, RequestError>) {
        {
            let mut state = self.lock();
            if state.pending.remove(&id).is_none() {
                return;
            }
            state.events.push_back(HelperEvent::Reply {
                id: RequestId(id),
                result,
            });
        }
        self.ready.notify_all();
        let waker = self.waker.lock().ok().and_then(|w| w.clone());
        if let Some(waker) = waker {
            waker();
        }
    }

    /// `session` 번째 도우미에 쓴 기다리는 요청을 모두 끊김으로 끝낸다.
    fn fail_session(&self, session: u64) {
        let ids: Vec<u64> = self
            .lock()
            .pending
            .iter()
            .filter(|(_, p)| p.session == session)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.finish(id, Err(RequestError::Disconnected));
        }
    }
}

/// 일꾼 스레드가 받는 것. 부르는 쪽의 요청과 읽는 스레드의 응답이 한 통로로 온다.
enum Inbox {
    Send { id: u64, request: Request },
    Response { session: u64, response: Response },
    ReaderEnded { session: u64 },
    Shutdown,
}

/// 도우미 관리. 앱 전체에 하나를 둔다. 버리면 일꾼이 도우미를 끝낸다(기다리지 않는다).
pub struct ShellHelper {
    inbox: Sender<Inbox>,
    shared: Arc<Shared>,
    next_id: AtomicU64,
}

impl ShellHelper {
    /// 관리 일꾼을 띄우고 곧바로 돌아온다. 도우미는 첫 요청 때 뜬다.
    pub fn start(options: HelperOptions) -> ShellHelper {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            ready: Condvar::new(),
            waker: Mutex::new(None),
        });
        let (inbox, inbox_rx) = mpsc::channel();
        let worker = Worker {
            options,
            inbox: inbox_rx,
            inbox_tx: inbox.clone(),
            shared: Arc::clone(&shared),
            session: 0,
            helper: None,
            crashes: VecDeque::new(),
            extensions: HashMap::new(),
        };
        // 일꾼을 띄우지 못하면 요청은 보낼 곳이 없다. 보내기가 실패하면 그 요청을 곧바로 끝낸다(`send`).
        let _ = thread::Builder::new()
            .name("facet-shell-helper".into())
            .spawn(move || worker.run());
        ShellHelper {
            inbox,
            shared,
            next_id: AtomicU64::new(0),
        }
    }

    /// 사건을 보낼 때마다 부를 깨우기(이벤트 루프 깨우기). 일꾼 스레드에서 불린다.
    pub fn set_waker(&self, waker: impl Fn() + Send + Sync + 'static) {
        if let Ok(mut slot) = self.shared.waker.lock() {
            *slot = Some(Arc::new(waker));
        }
    }

    fn next(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn send(&self, id: u64, request: Request) {
        if self.inbox.send(Inbox::Send { id, request }).is_err() {
            self.shared
                .finish(id, Err(RequestError::Failed("관리 일꾼이 없습니다".into())));
        }
    }

    /// 셸 메뉴 요청. 결과는 [`Reply::Menu`]로 온다.
    pub fn menu(&self, target: MenuTarget) -> RequestId {
        let id = self.next();
        {
            let mut state = self.shared.lock();
            state.pending.insert(
                id,
                Pending {
                    deadline: None,
                    session: 0,
                },
            );
            state.menus.insert(id, target.clone());
        }
        self.send(id, Request::Menu { id, target });
        RequestId(id)
    }

    /// 셸 항목 실행 요청: `menu`가 만든 메뉴의 `item`번 항목. 결과는 [`Reply::Done`]으로 온다. 놓았거나 모르는
    /// 메뉴면 [`RequestError::Failed`]로 끝난다.
    pub fn invoke(&self, menu: RequestId, item: u32) -> RequestId {
        let id = self.next();
        let target = {
            let mut state = self.shared.lock();
            state.pending.insert(
                id,
                Pending {
                    deadline: None,
                    session: 0,
                },
            );
            state.menus.get(&menu.0).cloned()
        };
        match target {
            Some(target) => self.send(
                id,
                Request::Execute {
                    id,
                    target,
                    command: item,
                },
            ),
            None => self
                .shared
                .finish(id, Err(RequestError::Failed("모르는 메뉴입니다".into()))),
        }
        RequestId(id)
    }

    /// 메뉴를 놓는다. 기다리던 그 메뉴 요청은 아무 사건도 내지 않고, 그 메뉴의 항목은 더 실행할 수 없다.
    pub fn release(&self, menu: RequestId) {
        let mut state = self.shared.lock();
        state.pending.remove(&menu.0);
        state.menus.remove(&menu.0);
        state
            .events
            .retain(|HelperEvent::Reply { id, .. }| *id != menu);
    }

    /// 마지막으로 띄운 도우미의 프로세스 번호(띄운 적이 없으면 `None`).
    pub fn process_id(&self) -> Option<u32> {
        self.shared.lock().process_id
    }

    /// 쌓인 사건을 모두 받는다. 기다리지 않는다.
    pub fn poll(&self) -> Vec<HelperEvent> {
        self.shared.lock().events.drain(..).collect()
    }

    /// 사건 하나를 `timeout`까지 기다려 받는다.
    pub fn wait_event(&self, timeout: Duration) -> Option<HelperEvent> {
        let deadline = Instant::now() + timeout;
        let mut state = self.shared.lock();
        loop {
            if let Some(event) = state.events.pop_front() {
                return Some(event);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            state = self
                .shared
                .ready
                .wait_timeout(state, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// 연달아 죽어 쉬는 중인가(쉬는 동안의 요청은 곧바로 [`RequestError::Resting`]).
    pub fn is_resting(&self) -> bool {
        self.shared
            .lock()
            .resting_until
            .is_some_and(|until| Instant::now() < until)
    }

    /// 같은 확장으로 [`SAME_EXTENSION_CRASHES`]번 이상 죽었으면 그 확장(그 전에는 `None`).
    pub fn crashing_extension(&self) -> Option<CrashedExtension> {
        self.shared.lock().crashing.clone()
    }

    /// 속성 대화상자 요청(#321): 도우미가 `paths`(목록 순서, 여럿이면 여러 항목의 속성)의 Windows 속성 대화상자를
    /// 띄운다. 도우미는 띄우자마자 [`Reply::Done`]으로 답하고, 대화상자가 떠 있는 동안은 쉬면 끝내기를 미룬다.
    pub fn properties(&self, paths: Vec<PathBuf>) -> RequestId {
        self.request(|id| Request::Properties { id, paths })
    }

    /// 「연결 프로그램」 대화상자 요청(#321): 도우미가 파일 `path`의 프로그램 고르기 대화상자를 띄운다. 띄우자마자
    /// [`Reply::Done`]으로 답한다.
    pub fn open_with(&self, path: PathBuf) -> RequestId {
        self.request(|id| Request::OpenWith { id, path })
    }

    /// 메뉴와 상관없는 요청 하나를 기다리는 요청으로 적고 보낸다.
    fn request(&self, make: impl FnOnce(u64) -> Request) -> RequestId {
        let id = self.next();
        self.shared.lock().pending.insert(
            id,
            Pending {
                deadline: None,
                session: 0,
            },
        );
        self.send(id, make(id));
        RequestId(id)
    }
}

impl Drop for ShellHelper {
    fn drop(&mut self) {
        let _ = self.inbox.send(Inbox::Shutdown);
    }
}

/// 떠 있는 도우미 하나와 그 연결.
struct Helper {
    session: u64,
    child: Child,
    writer: PipeWriter,
    closer: PipeCloser,
    /// 공유 상태 파일(마지막으로 부른 확장).
    state_file: PathBuf,
    /// 쓰고 아직 답을 받지 못한 요청 수(놓거나 시간이 지난 요청도 도우미는 아직 붙잡고 있을 수 있다).
    outstanding: usize,
    /// 답을 기다리는 요청이 있으면 마지막으로 나아간 때(쓰기나 응답). 멈춤 판정의 기준.
    busy_since: Option<Instant>,
    /// 마지막 쓰기나 응답의 때. 쉬면 끝내기의 엇갈림을 피하는 데 쓴다.
    last_activity: Instant,
}

impl Helper {
    /// 연결을 닫고 프로세스를 끝내 거둔다(이미 끝났으면 거두기만).
    fn end(mut self) {
        self.closer.close();
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.state_file);
    }

    /// 연결이 끊긴 도우미가 끝나기를 기다려 거둔다. 스스로 깨끗이(종료 코드 0) 끝났는지와 공유 상태에 적힌 확장.
    fn reap(mut self) -> (bool, Option<ExtensionInfo>) {
        self.closer.close();
        let deadline = Instant::now() + EXIT_WAIT;
        let status = loop {
            match self.child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => thread::sleep(EXIT_POLL),
                _ => break None,
            }
        };
        let clean = match status {
            Some(status) => status.success(),
            None => {
                let _ = self.child.kill();
                let _ = self.child.wait();
                false
            }
        };
        let extension = std::fs::read_to_string(&self.state_file)
            .ok()
            .and_then(|text| ExtensionInfo::parse(&text));
        let _ = std::fs::remove_file(&self.state_file);
        (clean, extension)
    }

    /// 요청 하나를 썼다.
    fn wrote(&mut self, now: Instant) {
        self.outstanding += 1;
        self.busy_since.get_or_insert(now);
        self.last_activity = now;
    }

    /// 요청 하나의 응답을 받았다.
    fn answered(&mut self, now: Instant) {
        self.outstanding = self.outstanding.saturating_sub(1);
        self.busy_since = (self.outstanding > 0).then_some(now);
        self.last_activity = now;
    }
}

/// 공유 상태 파일에 적힌 확장(CLSID·DLL 경로·이름 한 줄씩).
#[derive(Debug, Clone, PartialEq, Eq)]
struct ExtensionInfo {
    clsid: String,
    dll: PathBuf,
    name: Option<String>,
}

impl ExtensionInfo {
    fn parse(text: &str) -> Option<ExtensionInfo> {
        let mut lines = text.lines().map(str::trim);
        let clsid = lines.next().filter(|l| !l.is_empty())?.to_owned();
        let dll = PathBuf::from(lines.next().filter(|l| !l.is_empty())?);
        let name = lines.next().filter(|l| !l.is_empty()).map(str::to_owned);
        Some(ExtensionInfo { clsid, dll, name })
    }
}

/// 기록 파일의 한 줄: 지역 시각, CLSID, DLL 경로, 이름(모르면 비움)을 탭으로 가른다.
fn crash_line(time: chrono::DateTime<chrono::Local>, extension: &ExtensionInfo) -> String {
    format!(
        "{}\t{}\t{}\t{}\n",
        time.format("%Y-%m-%d %H:%M:%S"),
        extension.clsid,
        extension.dll.display(),
        extension.name.as_deref().unwrap_or("")
    )
}

/// 기록 파일에 한 줄을 덧붙인다(폴더가 없으면 만든다). 실패해도 관리는 돈다.
fn append_crash_line(log: &std::path::Path, line: &str) {
    use std::io::Write;
    if let Some(parent) = log.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// 도우미를 끝내는 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ending {
    /// 관리 쪽이 끝낸다(맞지 않는 응답, 멈춤, 쉬면 끝내기 직전). 죽음으로 세지 않는다.
    Ended,
    /// 연결이 끊겼다(도우미가 스스로 끝남). 종료 코드가 0이 아니면 죽음으로 센다.
    Lost,
}

struct Worker {
    options: HelperOptions,
    inbox: Receiver<Inbox>,
    inbox_tx: Sender<Inbox>,
    shared: Arc<Shared>,
    /// 마지막으로 띄운 도우미의 번호(1부터).
    session: u64,
    helper: Option<Helper>,
    /// 센 죽음의 때(`crash_window` 안의 것만).
    crashes: VecDeque<Instant>,
    /// 확장(CLSID, DLL)마다 (이름, 죽은 수).
    extensions: HashMap<(String, PathBuf), (Option<String>, u32)>,
}

impl Worker {
    fn run(mut self) {
        loop {
            let message = match self.next_deadline() {
                Some(deadline) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    match self.inbox.recv_timeout(left) {
                        Ok(m) => Some(m),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => Some(Inbox::Shutdown),
                    }
                }
                None => Some(self.inbox.recv().unwrap_or(Inbox::Shutdown)),
            };
            match message {
                Some(Inbox::Send { id, request }) => self.send(id, request),
                Some(Inbox::Response { session, response }) => {
                    if self.helper.as_ref().is_some_and(|h| h.session == session) {
                        self.receive(response);
                    }
                }
                Some(Inbox::ReaderEnded { session }) => {
                    if self.helper.as_ref().is_some_and(|h| h.session == session) {
                        self.end_helper(Ending::Lost);
                    }
                }
                Some(Inbox::Shutdown) => {
                    if let Some(helper) = self.helper.take() {
                        helper.end();
                    }
                    return;
                }
                None => {}
            }
            self.expire();
            self.check_stall();
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        let reply = self
            .shared
            .lock()
            .pending
            .values()
            .filter_map(|p| p.deadline)
            .min();
        let stall = self.stall_deadline();
        match (reply, stall) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn stall_deadline(&self) -> Option<Instant> {
        self.helper
            .as_ref()
            .and_then(|h| h.busy_since)
            .map(|since| since + self.options.stall_timeout)
    }

    /// 쓴 요청이 멈춤 판정 시간 동안 나아가지 않았으면 그 도우미를 끝낸다.
    fn check_stall(&mut self) {
        if self.stall_deadline().is_some_and(|d| d <= Instant::now()) {
            self.end_helper(Ending::Ended);
        }
    }

    /// 마감이 지난 요청을 시간 초과로 끝낸다.
    fn expire(&self) {
        let now = Instant::now();
        let expired: Vec<u64> = self
            .shared
            .lock()
            .pending
            .iter()
            .filter(|(_, p)| p.deadline.is_some_and(|d| d <= now))
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            self.shared.finish(id, Err(RequestError::TimedOut));
        }
    }

    /// 떠 있는 도우미를 끝내고 그 도우미에 보낸 기다리는 요청을 끊김으로 끝낸다. 연결이 끊겨 끝난 것이면 죽음인지
    /// 보고 세고 기록한 뒤에 요청을 끝낸다(요청의 끊김을 받은 쪽이 곧바로 쉬는지 볼 수 있게).
    fn end_helper(&mut self, ending: Ending) {
        let Some(helper) = self.helper.take() else {
            return;
        };
        let session = helper.session;
        match ending {
            Ending::Ended => helper.end(),
            Ending::Lost => {
                let (clean, extension) = helper.reap();
                if !clean {
                    self.record_crash(Instant::now(), extension);
                }
            }
        }
        self.shared.fail_session(session);
    }

    /// 죽음 하나를 센다: 연달아 죽음이면 쉬기 시작하고, 확장을 알면 기록하고 세어 알린다.
    fn record_crash(&mut self, now: Instant, extension: Option<ExtensionInfo>) {
        let window = self.options.crash_window;
        self.crashes.push_back(now);
        while self
            .crashes
            .front()
            .is_some_and(|t| now.duration_since(*t) > window)
        {
            self.crashes.pop_front();
        }
        let crashed = u32::try_from(self.crashes.len()).unwrap_or(u32::MAX);
        let mut state = self.shared.lock();
        if crashed > self.options.crash_limit {
            state.resting_until = Some(now + self.options.rest);
            self.crashes.clear();
        }
        let Some(extension) = extension else {
            return;
        };
        if let Some(log) = &self.options.crash_log {
            append_crash_line(log, &crash_line(chrono::Local::now(), &extension));
        }
        let entry = self
            .extensions
            .entry((extension.clsid.clone(), extension.dll.clone()))
            .or_insert((None, 0));
        if extension.name.is_some() {
            entry.0 = extension.name.clone();
        }
        entry.1 = entry.1.saturating_add(1);
        if entry.1 >= SAME_EXTENSION_CRASHES {
            state.crashing = Some(CrashedExtension {
                clsid: extension.clsid,
                dll: extension.dll,
                name: entry.0.clone(),
                crashes: entry.1,
            });
        }
    }

    /// 쉬는 중인가. 쉬는 시간이 지났으면 쉬기를 푼다.
    fn resting(&self) -> bool {
        let mut state = self.shared.lock();
        match state.resting_until {
            Some(until) if Instant::now() < until => true,
            Some(_) => {
                state.resting_until = None;
                false
            }
            None => false,
        }
    }

    /// 떠 있는 도우미가 쉬면 끝내기 시간에 거의 닿았으면(엇갈림을 피해) 조용히 끝낸다.
    ///
    /// 쉬는 시간을 넉넉히(그 몫만큼 더) 넘기고도 살아 있는 도우미는 대화상자가 떠 있어 끝내기를 미룬 것이라(#321) 그대로
    /// 쓴다: 끝내면 사용자가 보던 대화상자가 닫힌다. 이미 끝났으면 거둔다.
    fn retire_idle_helper(&mut self) {
        let idle = self.options.idle_timeout;
        let margin = (idle / 10).min(IDLE_MARGIN_MAX);
        let Some(helper) = self.helper.as_mut() else {
            return;
        };
        if helper.outstanding > 0 {
            return;
        }
        let elapsed = helper.last_activity.elapsed();
        let retire = if elapsed < idle.saturating_sub(margin) {
            false
        } else if elapsed <= idle + margin + IDLE_EXIT_GRACE {
            true
        } else {
            !matches!(helper.child.try_wait(), Ok(None))
        };
        if retire {
            self.end_helper(Ending::Ended);
        }
    }

    fn send(&mut self, id: u64, request: Request) {
        if !self.shared.lock().pending.contains_key(&id) {
            return; // 그 사이 놓았다.
        }
        self.retire_idle_helper();
        if self.helper.is_none() {
            if self.resting() {
                self.shared.finish(id, Err(RequestError::Resting));
                return;
            }
            match self.launch() {
                Ok(helper) => self.helper = Some(helper),
                Err(error) => {
                    self.shared.finish(id, Err(error));
                    return;
                }
            }
        }
        let Some(helper) = self.helper.as_mut() else {
            return;
        };
        {
            let mut state = self.shared.lock();
            match state.pending.get_mut(&id) {
                Some(pending) => {
                    pending.deadline = Some(Instant::now() + self.options.reply_timeout);
                    pending.session = helper.session;
                }
                None => return,
            }
        }
        if matches!(
            request,
            Request::Properties { .. } | Request::OpenWith { .. }
        ) {
            // 도우미가 띄울 대화상자가 facet 창 앞에 나오게 포그라운드 권한을 넘긴다.
            facet_win::shell_dialog::allow_foreground(helper.child.id());
        }
        if write_message(&mut helper.writer, &request).is_err() {
            self.end_helper(Ending::Lost);
            return;
        }
        helper.wrote(Instant::now());
    }

    fn receive(&mut self, response: Response) {
        if let Some(helper) = self.helper.as_mut() {
            helper.answered(Instant::now());
        }
        match response {
            Response::Menu { id, items } => self.shared.finish(id, Ok(Reply::Menu(items))),
            Response::Done { id } => self.shared.finish(id, Ok(Reply::Done)),
            Response::Error { id, message, .. } => {
                self.shared.finish(id, Err(RequestError::Failed(message)))
            }
            // 인사는 띄울 때 끝났다. 그 뒤의 인사 응답은 맞지 않는 응답이라 그 도우미를 버린다.
            Response::HelloAccepted { .. } | Response::HelloRejected { .. } => {
                self.end_helper(Ending::Ended)
            }
        }
    }

    /// 새 도우미를 띄워 붙고 버전 인사를 한다.
    fn launch(&mut self) -> Result<Helper, RequestError> {
        self.session += 1;
        let session = self.session;
        let deadline = Instant::now() + START_TIMEOUT;
        let name = pipe_name(session);
        let pipe = PrivatePipe::create(&name).map_err(|e| RequestError::Spawn(e.kind()))?;
        let state_file = state_file_path(session);
        let _ = std::fs::remove_file(&state_file);
        let idle_ms = u64::try_from(self.options.idle_timeout.as_millis()).unwrap_or(u64::MAX);

        let mut command = Command::new(&self.options.program);
        command
            .arg("--pipe")
            .arg(&name)
            .arg("--parent")
            .arg(
                self.options
                    .parent
                    .unwrap_or_else(std::process::id)
                    .to_string(),
            )
            .arg("--idle-timeout-ms")
            .arg(idle_ms.to_string())
            .arg("--crash-state")
            .arg(&state_file)
            .args(&self.options.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            /// 콘솔 창을 띄우지 않는다.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().map_err(|e| RequestError::Spawn(e.kind()))?;
        self.shared.lock().process_id = Some(child.id());

        let abandon = |mut child: Child, error: RequestError| {
            let _ = child.kill();
            let _ = child.wait();
            Err(error)
        };

        let client = loop {
            match pipe.accept(ACCEPT_POLL) {
                Ok(Some(client)) => break client,
                Ok(None) => {}
                Err(e) => return abandon(child, RequestError::Spawn(e.kind())),
            }
            if matches!(child.try_wait(), Ok(Some(_))) {
                return abandon(child, RequestError::Disconnected);
            }
            if Instant::now() >= deadline {
                return abandon(child, RequestError::Spawn(std::io::ErrorKind::TimedOut));
            }
        };
        let (mut reader, mut writer, closer) = match client.split() {
            Ok(parts) => parts,
            Err(e) => return abandon(child, RequestError::Spawn(e.kind())),
        };

        let hello = Request::Hello {
            version: PROTOCOL_VERSION,
        };
        if write_message(&mut writer, &hello).is_err() {
            return abandon(child, RequestError::Disconnected);
        }
        reader.set_deadline(Some(deadline));
        match read_message::<Response>(&mut reader) {
            Ok(Response::HelloAccepted { version })
                if (MIN_SUPPORTED_VERSION..=MAX_SUPPORTED_VERSION).contains(&version) => {}
            Ok(Response::HelloAccepted { .. } | Response::HelloRejected { .. }) => {
                closer.close();
                return abandon(child, RequestError::VersionMismatch);
            }
            Ok(_) | Err(_) => {
                closer.close();
                return abandon(child, RequestError::Disconnected);
            }
        }
        reader.set_deadline(None);

        let inbox = self.inbox_tx.clone();
        let spawned = thread::Builder::new()
            .name("facet-shell-helper-reader".into())
            .spawn(move || read_loop(session, reader, inbox));
        if spawned.is_err() {
            closer.close();
            return abandon(child, RequestError::Spawn(std::io::ErrorKind::Other));
        }
        Ok(Helper {
            session,
            child,
            writer,
            closer,
            state_file,
            outstanding: 0,
            busy_since: None,
            last_activity: Instant::now(),
        })
    }
}

/// 연결 하나의 응답을 읽어 일꾼에게 넘긴다. 끊기거나 깨진 응답이면 끝났다고 알리고 끝난다.
fn read_loop(session: u64, mut reader: PipeReader, inbox: Sender<Inbox>) {
    loop {
        match read_message::<Response>(&mut reader) {
            Ok(response) => {
                if inbox.send(Inbox::Response { session, response }).is_err() {
                    return;
                }
            }
            Err(_) => {
                let _ = inbox.send(Inbox::ReaderEnded { session });
                return;
            }
        }
    }
}

/// 도우미 파이프 이름: UI 프로세스 번호와 무작위 값(이름을 미리 알아 차지하지 못하게).
fn pipe_name(session: u64) -> String {
    format!(
        r"\\.\pipe\facet-shell-helper-{}-{:016x}",
        std::process::id(),
        random_value(session)
    )
}

/// 공유 상태 파일 자리(임시 폴더, 프로세스 번호와 무작위 값).
fn state_file_path(session: u64) -> PathBuf {
    std::env::temp_dir().join(format!(
        "facet-shell-helper-{}-{:016x}.state",
        std::process::id(),
        random_value(session)
    ))
}

/// 표준 라이브러리 해시의 무작위 열쇠(프로세스마다 운영체제 난수로 정함)에 시각과 번호를 섞은 값.
fn random_value(session: u64) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(session);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    hasher.write_u128(nanos);
    hasher.finish()
}

/// 요청 번호. 관리 하나 안에서 늘어나기만 한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(u64);

/// 도우미 관리의 사건.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperEvent {
    /// 요청 하나가 끝났다. 놓은 메뉴의 요청은 오지 않는다.
    Reply {
        id: RequestId,
        result: Result<Reply, RequestError>,
    },
}

/// 요청의 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// 메뉴 요청의 항목 트리(도우미가 준 그대로).
    Menu(Vec<MenuItem>),
    /// 항목 실행이 끝났다.
    Done,
}

/// 요청이 결과 없이 끝난 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    /// 대기 시간 안에 응답이 오지 않았다.
    TimedOut,
    /// 답을 받기 전에 도우미가 끝났다(연결이 끊겼다).
    Disconnected,
    /// 도우미의 프로토콜 버전이 맞지 않아 쓰지 않았다.
    VersionMismatch,
    /// 연달아 죽어 쉬는 중이다(#320).
    Resting,
    /// 도우미를 띄우지 못했다.
    Spawn(std::io::ErrorKind),
    /// 도우미가 오류로 답했다(그 설명, 화면 문장 아님).
    Failed(String),
}

/// 도우미를 되풀이해 죽게 한 확장([`ShellHelper::crashing_extension`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrashedExtension {
    /// 확장의 CLSID(`{...}` 글자).
    pub clsid: String,
    /// 확장 DLL 경로.
    pub dll: PathBuf,
    /// 확장 이름(도우미가 알았으면).
    pub name: Option<String>,
    /// 그 확장으로 죽은 수.
    pub crashes: u32,
}

/// #322: 열린 셸 메뉴의 번호.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShellMenuId(pub(crate) u64);

/// #322: 열린 셸 메뉴.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellMenu {
    pub id: ShellMenuId,
    pub kind: MenuKind,
    pub section: ShellSection,
    pub crashing_extension: Option<CrashedExtension>,
}

/// #322: 셸 항목 자리.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellSection {
    Loading,
    Items(Vec<MenuItem>),
    Failed,
    Resting,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 파이프_이름에_프로세스_번호와_무작위_값이_있다() {
        let a = pipe_name(1);
        let b = pipe_name(1);
        let prefix = format!(r"\\.\pipe\facet-shell-helper-{}-", std::process::id());
        assert!(a.starts_with(&prefix), "{a}");
        let random = &a[prefix.len()..];
        assert_eq!(random.len(), 16);
        assert!(random.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b, "띄울 때마다 다른 무작위 값");
    }

    #[test]
    fn 기본값() {
        let options = HelperOptions::new("x.exe");
        assert_eq!(options.stall_timeout, Duration::from_secs(10));
        assert_eq!(options.idle_timeout, Duration::from_secs(300));
        assert_eq!(options.crash_window, Duration::from_secs(60));
        assert_eq!(options.crash_limit, 3);
        assert_eq!(options.rest, Duration::from_secs(60));
        assert!(default_program().ends_with("facet-shell-helper.exe"));
    }

    #[test]
    fn 공유_상태와_기록_줄() {
        let info = ExtensionInfo::parse("{A}\r\nC:\\x.dll\r\nName\r\n").unwrap();
        assert_eq!(info.clsid, "{A}");
        assert_eq!(info.dll, PathBuf::from(r"C:\x.dll"));
        assert_eq!(info.name.as_deref(), Some("Name"));
        assert_eq!(ExtensionInfo::parse("{A}\nC:\\x.dll\n").unwrap().name, None);
        assert_eq!(ExtensionInfo::parse(""), None);
        assert_eq!(ExtensionInfo::parse("{A}\n"), None);

        use chrono::TimeZone;
        let time = chrono::Local
            .with_ymd_and_hms(2026, 11, 18, 9, 5, 7)
            .single()
            .unwrap();
        assert_eq!(
            crash_line(time, &info),
            "2026-11-18 09:05:07\t{A}\tC:\\x.dll\tName\n"
        );
    }

    #[test]
    fn 놓은_메뉴나_모르는_메뉴의_항목_실행은_실패한다() {
        let helper = ShellHelper::start(HelperOptions::new("does-not-exist.exe"));
        let menu = RequestId(12345);
        let id = helper.invoke(menu, 1);
        assert_eq!(
            helper.wait_event(Duration::from_secs(5)),
            Some(HelperEvent::Reply {
                id,
                result: Err(RequestError::Failed("모르는 메뉴입니다".into()))
            })
        );
    }
}
