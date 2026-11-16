//! 인덱서 서비스 연결(질의 클라이언트). 앱 전체에 하나를 두고 즉시 검색이 쓴다(spec #23).
//!
//! [`IndexerClient::start`]는 곧바로 돌아오고, 연결·버전 인사·요청·응답은 모두 연결 일꾼 스레드에서 한다.
//! 부르는 쪽(UI 스레드)은 무엇도 기다리지 않는다: 요청은 번호([`RequestId`])만 받고 결과는 사건
//! ([`IndexerEvent`])으로 온다. 사건은 작업 대기열·검색 피드와 같은 깨우기([`IndexerClient::set_waker`])와 받기
//! ([`IndexerClient::poll`]·[`IndexerClient::wait_event`]) 흐름이다.
//!
//! # 연결 상태
//!
//! [`Connection`]: 판정 전 [`Connection::Connecting`], 연결됨, 서비스 없음, 버전 다름(서비스가 알린 지원 범위).
//!
//! - 연결 판정은 [`ClientOptions::connect_timeout`](기본 [`CONNECT_TIMEOUT`], 200ms) 안에 끝난다. 파이프가 없으면
//!   곧바로, 파이프가 바쁘거나 서비스가 버전 인사에 답하지 않으면 그 시간이 지나면 서비스 없음이다.
//! - 붙지 못했거나 연결이 끊기면 [`ClientOptions::reconnect_interval`](기본 [`RECONNECT_INTERVAL`], 1초)마다 뒤에서
//!   다시 붙어 본다. 그동안 들어온 요청은 기다리게 하지 않고 곧바로 [`RequestError::NotConnected`]로 답한다.
//! - 연결된 뒤 서비스가 멈추거나 파이프가 끊기면, 답을 기다리던 요청은 [`RequestError::Disconnected`]로 끝나고 상태가
//!   서비스 없음으로 바뀐다.
//! - 상태가 바뀔 때마다 [`IndexerEvent::Connection`]이 온다(같은 상태로 다시 판정하면 오지 않는다).
//!
//! # 요청과 응답
//!
//! 질의([`IndexerClient::query`]), 이어 받기([`IndexerClient::resume`]), 상태 질의([`IndexerClient::status`]).
//! 서비스는 요청마다 응답을 하나씩 받은 차례대로 보내므로, 일꾼이 보낸 요청의 차례를 기억해 응답을 번호에 맞춘다.
//!
//! 새 질의는 앞선 질의를 취소한다: 같은 연결로 보내므로 서비스가 아직 답하지 않은 앞선 질의·이어 받기에 취소로
//! 답한다. 클라이언트는 새 질의를 보낼 때 앞선 질의·이어 받기를 「낡음」으로 적어 두고, 그 응답(취소든 결과든 늦게 온
//! 것이든)을 버린다. 그래서 앞선 질의의 [`IndexerEvent::Reply`]는 오지 않는다. 상태 질의는 질의와 상관없다.
//!
//! # 파이프 이름
//!
//! 기본은 프로토콜의 이름 [`facet_protocol::PIPE_NAME`]이고, 환경 변수 [`ENV_PIPE`](`FACET_INDEXER_PIPE`)가 있으면 그
//! 이름이다([`default_pipe_name`]). 시험은 [`ClientOptions::pipe`]로 직접 준다.
//!
//! 파이프는 겹친 입출력으로 연다([`facet_win::pipe_client`], 응답을 기다리며 읽는 동안 새 질의를 써야 하므로). 화면 문장은
//! 없다(ADR-0007).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use facet_protocol::{
    ErrorKind, ErrorMessage, PIPE_CLIENT_ACCESS, PROTOCOL_VERSION, PageMessage, QueryMessage,
    Request, Response, ResumeMessage, StatusMessage, read_message, write_message,
};
use facet_win::pipe_client::{self, PipeCloser, PipeReader, PipeWriter};

/// 연결 판정에 쓰는 가장 긴 시간(파이프 열기와 버전 인사까지).
///
/// 근거: 같은 기계의 파이프 열기와 버전 인사 한 번은 1ms 안팎이다. 서비스가 없으면 파이프 열기가 곧바로 실패하므로
/// 이 시간은 서비스가 있는데 바쁘거나 멈춘 경우에만 다 쓴다. 즉시 검색의 첫 화면 기준이 50ms이고, 그보다 넉넉하되
/// 사람이 「멈췄다」고 느끼기 전(수백 ms)에 강등하도록 200ms로 둔다(spec #23이 정한 값).
pub const CONNECT_TIMEOUT: Duration = Duration::from_millis(200);

/// 붙지 못했거나 끊긴 뒤 다시 붙어 보는 간격.
///
/// 근거: 서비스가 없을 때 한 번 붙어 보는 비용은 파일 열기 한 번(마이크로초)이라 1초마다 해도 부담이 없고, 서비스를 다시
/// 띄운 뒤 1초 안에 즉시 검색으로 돌아온다(사람이 서비스를 켜고 검색창으로 돌아오는 시간보다 짧다).
pub const RECONNECT_INTERVAL: Duration = Duration::from_secs(1);

/// 파이프 이름을 바꾸는 환경 변수(시험·측정용). 없으면 [`facet_protocol::PIPE_NAME`].
pub const ENV_PIPE: &str = "FACET_INDEXER_PIPE";

/// 쓸 파이프 이름: 환경 변수 [`ENV_PIPE`]가 있고 비어 있지 않으면 그것, 아니면 [`facet_protocol::PIPE_NAME`].
pub fn default_pipe_name() -> String {
    match std::env::var(ENV_PIPE) {
        Ok(name) if !name.trim().is_empty() => name,
        _ => facet_protocol::PIPE_NAME.to_owned(),
    }
}

/// 연결 설정.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientOptions {
    /// 파이프 이름.
    pub pipe: String,
    /// 연결 판정에 쓰는 가장 긴 시간.
    pub connect_timeout: Duration,
    /// 다시 붙어 보는 간격.
    pub reconnect_interval: Duration,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            pipe: default_pipe_name(),
            connect_timeout: CONNECT_TIMEOUT,
            reconnect_interval: RECONNECT_INTERVAL,
        }
    }
}

/// 인덱서 서비스와의 연결 상태.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connection {
    /// 처음 판정하는 중이다(시작 뒤 길게는 연결 판정 시간).
    Connecting,
    /// 연결됐다. 버전 인사로 정한 프로토콜 버전.
    Connected { version: u16 },
    /// 서비스가 없거나 닿지 않는다(파이프 없음, 바쁨, 답 없음, 끊김).
    NoService,
    /// 서비스가 이 클라이언트의 버전을 받지 않는다. 서비스가 알린 지원 범위.
    VersionMismatch {
        client_version: u16,
        min_version: u16,
        max_version: u16,
    },
}

/// 요청 번호. 클라이언트 하나 안에서 늘어나기만 한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId(u64);

/// 요청의 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// 질의·이어 받기의 결과 조각.
    Page(PageMessage),
    /// 상태 질의의 응답.
    Status(StatusMessage),
}

/// 요청이 결과 없이 끝난 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    /// 연결돼 있지 않아 보내지 않았다. 그때의 상태.
    NotConnected(Connection),
    /// 답을 받기 전에 연결이 끊겼다.
    Disconnected,
    /// 서비스가 오류로 답했다(틀린 질의, 낡은 이어 받기 표지 등).
    Service(ErrorMessage),
    /// 서비스가 요청과 맞지 않는 응답을 보냈다(그 연결은 끊는다).
    Unexpected,
}

/// 연결이 보내는 사건.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexerEvent {
    /// 연결 상태가 바뀌었다.
    Connection(Connection),
    /// 요청 하나가 끝났다. 새 질의로 낡은 앞선 질의·이어 받기는 오지 않는다.
    Reply {
        id: RequestId,
        result: Result<Reply, RequestError>,
    },
}

type Waker = Arc<dyn Fn() + Send + Sync>;

/// 일꾼이 사건을 보내는 곳. 보낸 뒤 깨우기가 있으면 부른다.
#[derive(Clone)]
struct Outbox {
    sender: Sender<IndexerEvent>,
    waker: Arc<Mutex<Option<Waker>>>,
    state: Arc<Mutex<Connection>>,
}

impl Outbox {
    fn send(&self, event: IndexerEvent) {
        if self.sender.send(event).is_ok() {
            let waker = self.waker.lock().ok().and_then(|w| w.clone());
            if let Some(waker) = waker {
                waker();
            }
        }
    }

    fn state(&self) -> Connection {
        self.state
            .lock()
            .map(|s| s.clone())
            .unwrap_or(Connection::NoService)
    }

    /// 상태를 바꾸고, 바뀌었으면 사건을 보낸다.
    fn set_state(&self, next: Connection) {
        let changed = match self.state.lock() {
            Ok(mut s) if *s != next => {
                *s = next.clone();
                true
            }
            _ => false,
        };
        if changed {
            self.send(IndexerEvent::Connection(next));
        }
    }

    fn reply(&self, id: RequestId, result: Result<Reply, RequestError>) {
        self.send(IndexerEvent::Reply { id, result });
    }
}

/// 일꾼 스레드가 받는 것. 부르는 쪽의 요청과 읽는 스레드의 응답이 한 통로로 온다.
enum Inbox {
    Send {
        id: RequestId,
        request: Request,
    },
    /// 읽는 스레드(`session` 번째 연결)가 응답 하나를 읽었다.
    Response {
        session: u64,
        response: Response,
    },
    /// 읽는 스레드(`session` 번째 연결)가 끝났다(끊김·읽기 실패).
    ReaderEnded {
        session: u64,
    },
    Shutdown,
}

/// 앱 전체에 하나인 인덱서 서비스 연결. 버리면 연결 일꾼이 연결을 닫고 끝난다(기다리지 않는다).
pub struct IndexerClient {
    inbox: Sender<Inbox>,
    events: Receiver<IndexerEvent>,
    outbox: Outbox,
    next_id: AtomicU64,
}

impl IndexerClient {
    /// 연결 일꾼을 띄우고 곧바로 돌아온다. 상태는 [`Connection::Connecting`]에서 시작한다.
    pub fn start(options: ClientOptions) -> Self {
        let (event_tx, events) = mpsc::channel();
        let outbox = Outbox {
            sender: event_tx,
            waker: Arc::new(Mutex::new(None)),
            state: Arc::new(Mutex::new(Connection::Connecting)),
        };
        let (inbox, inbox_rx) = mpsc::channel();
        let worker = Worker {
            options,
            inbox: inbox_rx,
            inbox_tx: inbox.clone(),
            outbox: outbox.clone(),
            session: 0,
        };
        let spawned = thread::Builder::new()
            .name("facet-indexer-client".into())
            .spawn(move || worker.run());
        if spawned.is_err() {
            // 스레드를 띄우지 못하면 연결할 수 없다. 요청은 보낼 곳이 없어 NotConnected로 답한다.
            outbox.set_state(Connection::NoService);
        }
        Self {
            inbox,
            events,
            outbox,
            next_id: AtomicU64::new(0),
        }
    }

    /// 지금 연결 상태. 기다리지 않는다.
    pub fn connection(&self) -> Connection {
        self.outbox.state()
    }

    /// 사건을 보낼 때마다 부를 깨우기(이벤트 루프 깨우기). 일꾼 스레드에서 불린다.
    pub fn set_waker(&self, waker: impl Fn() + Send + Sync + 'static) {
        if let Ok(mut slot) = self.outbox.waker.lock() {
            *slot = Some(Arc::new(waker));
        }
    }

    /// 질의를 보낸다. 앞선 질의와 그 이어 받기는 취소되고 그 결과는 오지 않는다.
    pub fn query(&self, query: QueryMessage) -> RequestId {
        self.send(Request::Query(query))
    }

    /// 앞선 질의의 다음 결과 조각을 청한다.
    pub fn resume(&self, resume: ResumeMessage) -> RequestId {
        self.send(Request::Resume(resume))
    }

    /// 볼륨마다의 인덱스 상태를 묻는다.
    pub fn status(&self) -> RequestId {
        self.send(Request::Status)
    }

    fn send(&self, request: Request) -> RequestId {
        let id = RequestId(self.next_id.fetch_add(1, Ordering::Relaxed));
        if self.inbox.send(Inbox::Send { id, request }).is_err() {
            // 일꾼이 없다(띄우지 못함).
            self.outbox
                .reply(id, Err(RequestError::NotConnected(self.connection())));
        }
        id
    }

    /// 쌓인 사건을 모두 받는다. 기다리지 않는다.
    pub fn poll(&self) -> Vec<IndexerEvent> {
        self.events.try_iter().collect()
    }

    /// 사건 하나를 `timeout`까지 기다려 받는다.
    pub fn wait_event(&self, timeout: Duration) -> Option<IndexerEvent> {
        self.events.recv_timeout(timeout).ok()
    }
}

impl Drop for IndexerClient {
    fn drop(&mut self) {
        let _ = self.inbox.send(Inbox::Shutdown);
    }
}

/// 보낸 요청의 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Query,
    Resume,
    Status,
}

impl Kind {
    fn of(request: &Request) -> Self {
        match request {
            Request::Query(_) => Kind::Query,
            Request::Resume(_) => Kind::Resume,
            _ => Kind::Status,
        }
    }
}

/// 답을 기다리는 요청 하나.
struct Pending {
    id: RequestId,
    kind: Kind,
    /// 뒤에 새 질의를 보내 이 응답은 버린다.
    stale: bool,
}

/// 연결 하나를 끝낸 까닭.
enum Ended {
    Disconnected,
    Shutdown,
}

/// 붙은 연결의 쓰는 쪽과 닫개.
struct Session {
    writer: PipeWriter,
    closer: PipeCloser,
    version: u16,
}

struct Worker {
    options: ClientOptions,
    inbox: Receiver<Inbox>,
    inbox_tx: Sender<Inbox>,
    outbox: Outbox,
    /// 연결 번호. 앞선 연결의 읽는 스레드가 늦게 보낸 것을 가린다.
    session: u64,
}

impl Worker {
    fn run(mut self) {
        loop {
            match self.connect() {
                Ok(session) => {
                    self.outbox.set_state(Connection::Connected {
                        version: session.version,
                    });
                    match self.serve(session) {
                        Ended::Shutdown => return,
                        Ended::Disconnected => self.outbox.set_state(Connection::NoService),
                    }
                }
                Err(state) => self.outbox.set_state(state),
            }
            if !self.wait_to_retry() {
                return;
            }
        }
    }

    /// 파이프에 붙어 버전 인사를 한다. 모두 연결 판정 시간 안에 끝난다.
    fn connect(&mut self) -> Result<Session, Connection> {
        let deadline = Instant::now() + self.options.connect_timeout;
        let client = pipe_client::connect(
            &self.options.pipe,
            PIPE_CLIENT_ACCESS,
            self.options.connect_timeout,
        )
        .map_err(|_| Connection::NoService)?;
        let (mut reader, mut writer, closer) = client.split().map_err(|_| Connection::NoService)?;
        reader.set_deadline(Some(deadline));
        let greeted = write_message(
            &mut writer,
            &Request::Hello {
                version: PROTOCOL_VERSION,
            },
        )
        .ok()
        .and_then(|()| read_message::<Response>(&mut reader).ok());
        let version = match greeted {
            Some(Response::HelloAccepted { version }) => version,
            Some(Response::HelloRejected {
                client_version,
                min_version,
                max_version,
            }) => {
                closer.close();
                return Err(Connection::VersionMismatch {
                    client_version,
                    min_version,
                    max_version,
                });
            }
            _ => {
                closer.close();
                return Err(Connection::NoService);
            }
        };
        reader.set_deadline(None);
        self.session += 1;
        let session = self.session;
        let inbox = self.inbox_tx.clone();
        let spawned = thread::Builder::new()
            .name("facet-indexer-reader".into())
            .spawn(move || read_loop(reader, session, &inbox));
        if spawned.is_err() {
            closer.close();
            return Err(Connection::NoService);
        }
        Ok(Session {
            writer,
            closer,
            version,
        })
    }

    /// 연결 하나로 요청을 보내고 응답을 맞춘다. 끊기거나 그만두라고 하면 돌아온다.
    fn serve(&mut self, mut session: Session) -> Ended {
        let mut pending: VecDeque<Pending> = VecDeque::new();
        let ended = loop {
            let Ok(message) = self.inbox.recv() else {
                break Ended::Shutdown;
            };
            match message {
                Inbox::Shutdown => break Ended::Shutdown,
                Inbox::Send { id, request } => {
                    let kind = Kind::of(&request);
                    if kind == Kind::Query {
                        for p in pending.iter_mut() {
                            if matches!(p.kind, Kind::Query | Kind::Resume) {
                                p.stale = true;
                            }
                        }
                    }
                    pending.push_back(Pending {
                        id,
                        kind,
                        stale: false,
                    });
                    if write_message(&mut session.writer, &request).is_err() {
                        break Ended::Disconnected;
                    }
                }
                Inbox::Response {
                    session: s,
                    response,
                } if s == self.session => {
                    let Some(front) = pending.pop_front() else {
                        // 묻지 않은 응답: 차례가 어긋났으니 이 연결을 믿을 수 없다.
                        break Ended::Disconnected;
                    };
                    match settle(&front, response) {
                        Settled::Drop => {}
                        Settled::Reply(result) => self.outbox.reply(front.id, result),
                        Settled::Broken => {
                            self.outbox.reply(front.id, Err(RequestError::Unexpected));
                            break Ended::Disconnected;
                        }
                    }
                }
                Inbox::ReaderEnded { session: s } if s == self.session => {
                    break Ended::Disconnected;
                }
                // 앞선 연결의 읽는 스레드가 늦게 보낸 것.
                Inbox::Response { .. } | Inbox::ReaderEnded { .. } => {}
            }
        };
        session.closer.close();
        if let Ended::Disconnected = ended {
            for p in pending.into_iter().filter(|p| !p.stale) {
                self.outbox.reply(p.id, Err(RequestError::Disconnected));
            }
        }
        ended
    }

    /// 다시 붙어 보기 전까지 기다린다. 그동안 온 요청은 곧바로 「연결 안 됨」으로 답한다. 그만두라고 하면 `false`.
    fn wait_to_retry(&mut self) -> bool {
        let until = Instant::now() + self.options.reconnect_interval;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match self.inbox.recv_timeout(left) {
                Ok(Inbox::Send { id, .. }) => {
                    let state = self.outbox.state();
                    self.outbox
                        .reply(id, Err(RequestError::NotConnected(state)));
                }
                Ok(Inbox::Shutdown) | Err(RecvTimeoutError::Disconnected) => return false,
                Ok(Inbox::Response { .. } | Inbox::ReaderEnded { .. }) => {}
                Err(RecvTimeoutError::Timeout) => return true,
            }
        }
    }
}

enum Settled {
    /// 버린다(낡은 요청, 취소 응답).
    Drop,
    Reply(Result<Reply, RequestError>),
    /// 요청과 맞지 않는 응답. 연결을 끊는다.
    Broken,
}

/// 기다리던 요청 하나와 받은 응답을 맞춘다.
fn settle(pending: &Pending, response: Response) -> Settled {
    if pending.stale {
        return Settled::Drop;
    }
    match (pending.kind, response) {
        (Kind::Query | Kind::Resume, Response::Page(page)) => Settled::Reply(Ok(Reply::Page(page))),
        (Kind::Status, Response::Status(status)) => Settled::Reply(Ok(Reply::Status(status))),
        // 새 질의로 취소된 응답이다. 그 새 질의는 이 클라이언트가 보냈으므로 낡음으로 적혀 있어야 하지만, 다른
        // 까닭(서비스 쪽 사정)으로 오더라도 부르는 쪽에 보일 결과가 아니다.
        (_, Response::Error(e)) if e.kind == ErrorKind::Cancelled => Settled::Drop,
        (_, Response::Error(e)) => Settled::Reply(Err(RequestError::Service(e))),
        _ => Settled::Broken,
    }
}

/// 읽는 스레드: 응답을 읽는 대로 일꾼에게 넘긴다. 끊기거나 닫으면 끝났다고 알리고 끝난다.
fn read_loop(mut reader: PipeReader, session: u64, inbox: &Sender<Inbox>) {
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
