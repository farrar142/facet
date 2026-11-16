//! 같은 프로세스 안에서 파이프 서버를 띄우고 시험용 클라이언트로 붙어 보는 테스트.
//! 관리자 권한이 필요 없다.
#![cfg(windows)]

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use facet_indexer::pipe::{
    CLIENT_ACCESS, FILE_ALL_ACCESS, FILE_CREATE_PIPE_INSTANCE, INTERACTIVE_SID, NETWORK_SID,
    PipeServer, current_user_sid,
};
use facet_indexer::sample::{SAMPLE_VOLUME, SampleBackend, sample_index};
use facet_indexer::session::{Backend, Caller, Cancel, Cursor, Page};
use facet_indexer::volumes::Volumes;
use facet_protocol::{
    Case, Condition, ErrorKind, ErrorMessage, MAX_FRAME_LEN, MAX_SUPPORTED_VERSION,
    MIN_SUPPORTED_VERSION, Order, PROTOCOL_VERSION, QueryMessage, ReadError, Request, Response,
    ResumeMessage, SortKey, StatusMessage, VolumeState, read_message, write_message,
};

/// 견본의 「보고서 N.txt」 수. 견본 인덱스 항목은 이것과 4개(폴더 둘, 파일 둘)다.
const REPORTS: u64 = 250;

/// 파이프 이름이 테스트끼리, 그리고 실제 서비스와 겹치지 않게 한다.
fn unique_name() -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        r"\\.\pipe\facet-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn sample_server(name: &str) -> PipeServer {
    PipeServer::start(name, Arc::new(SampleBackend::new(REPORTS))).unwrap()
}

/// 일반 클라이언트처럼(읽기와 데이터 쓰기 권한만 청해) 붙는다. 인스턴스가 모두 바쁘면 잠시 다시 한다.
fn connect(name: &str) -> File {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match OpenOptions::new().access_mode(CLIENT_ACCESS).open(name) {
            Ok(f) => return f,
            // ERROR_PIPE_BUSY
            Err(e) if e.raw_os_error() == Some(231) && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(e) => panic!("{name}에 붙지 못함: {e}"),
        }
    }
}

fn ask(pipe: &mut File, request: &Request) -> Response {
    write_message(pipe, request).unwrap();
    read_message(pipe).unwrap()
}

fn greeted(name: &str) -> File {
    let mut pipe = connect(name);
    assert_eq!(
        ask(
            &mut pipe,
            &Request::Hello {
                version: PROTOCOL_VERSION
            }
        ),
        Response::HelloAccepted {
            version: PROTOCOL_VERSION
        }
    );
    pipe
}

fn reports(limit: Option<u32>) -> Request {
    Request::Query(QueryMessage {
        condition: Condition::NameContains {
            part: "보고서".into(),
            case: Case::Insensitive,
        },
        sort: Some((SortKey::Size, Order::Descending)),
        limit,
    })
}

fn error_kind(r: Response) -> ErrorKind {
    match r {
        Response::Error(e) => e.kind,
        other => panic!("오류 응답이 아님: {other:?}"),
    }
}

#[test]
fn hello_then_query_resume_and_status() {
    let name = unique_name();
    let _server = sample_server(&name);
    let mut pipe = greeted(&name);

    let Response::Page(first) = ask(&mut pipe, &reports(Some(100))) else {
        panic!("결과 조각이 아님")
    };
    assert_eq!(first.total, REPORTS);
    assert_eq!(first.hits.len(), 100);
    assert_eq!(first.hits[0].name, format!("보고서 {REPORTS}.txt"));
    assert_eq!(
        first.hits[0].path,
        format!(r"{SAMPLE_VOLUME}\문서\보고서 {REPORTS}.txt")
    );

    let mut seen = first.hits.len() as u64;
    let mut next = first.next;
    while let Some(cursor) = next {
        let Response::Page(page) = ask(
            &mut pipe,
            &Request::Resume(ResumeMessage { cursor, limit: 100 }),
        ) else {
            panic!("결과 조각이 아님")
        };
        assert_eq!(page.total, REPORTS);
        seen += page.hits.len() as u64;
        next = page.next;
    }
    assert_eq!(seen, REPORTS);

    let Response::Status(status) = ask(&mut pipe, &Request::Status) else {
        panic!("상태 응답이 아님")
    };
    assert_eq!(status.volumes.len(), 1);
    assert_eq!(status.volumes[0].volume, SAMPLE_VOLUME);
    assert_eq!(
        status.volumes[0].state,
        VolumeState::Ready { items: REPORTS + 4 }
    );
}

#[test]
fn unsupported_version_is_rejected_with_supported_range_and_disconnected() {
    let name = unique_name();
    let _server = sample_server(&name);
    let mut pipe = connect(&name);
    assert_eq!(
        ask(&mut pipe, &Request::Hello { version: 999 }),
        Response::HelloRejected {
            client_version: 999,
            min_version: MIN_SUPPORTED_VERSION,
            max_version: MAX_SUPPORTED_VERSION,
        }
    );
    // 거절한 뒤에는 연결을 끊는다.
    assert!(matches!(
        read_message::<Response>(&mut pipe),
        Err(ReadError::Closed)
    ));
}

#[test]
fn requests_before_hello_are_refused() {
    let name = unique_name();
    let _server = sample_server(&name);
    let mut pipe = connect(&name);
    assert_eq!(
        error_kind(ask(&mut pipe, &Request::Status)),
        ErrorKind::NotGreeted
    );
    // 그 뒤에 인사하면 받아들인다.
    assert!(matches!(
        ask(
            &mut pipe,
            &Request::Hello {
                version: PROTOCOL_VERSION
            }
        ),
        Response::HelloAccepted { .. }
    ));
}

#[test]
fn too_large_frame_disconnects_only_that_connection() {
    let name = unique_name();
    let _server = sample_server(&name);
    let mut pipe = greeted(&name);
    // 머리만 보낸다. 서버는 본문을 기다리지 않고 끊어야 한다.
    pipe.write_all(&(MAX_FRAME_LEN + 1).to_le_bytes()).unwrap();
    assert!(matches!(
        read_message::<Response>(&mut pipe),
        Err(ReadError::Closed)
    ));

    // 서버는 계속 다른 연결을 받는다.
    let mut other = greeted(&name);
    assert!(matches!(
        ask(&mut other, &Request::Status),
        Response::Status(_)
    ));
}

/// 「느린」을 찾는 질의는 취소될 때까지 붙잡고 있는 인덱스 쪽. 붙잡기 시작하면 알린다.
struct SlowBackend {
    inner: Volumes,
    entered: Mutex<Sender<()>>,
}

impl Backend for SlowBackend {
    fn search(
        &self,
        query: &QueryMessage,
        _: &dyn Caller,
        cancel: &Cancel,
    ) -> Result<Page, ErrorMessage> {
        if matches!(&query.condition, Condition::NameContains { part, .. } if part == "느린") {
            self.entered.lock().unwrap().send(()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            while !cancel.is_cancelled() {
                assert!(Instant::now() < deadline, "취소 표시가 서지 않았다");
                thread::sleep(Duration::from_millis(1));
            }
        }
        self.inner.search(query, cancel)
    }

    fn resume(&self, cursor: Cursor, limit: usize, _: &dyn Caller) -> Result<Page, ErrorMessage> {
        self.inner.resume(cursor, limit)
    }

    fn status(&self) -> StatusMessage {
        self.inner.status()
    }
}

fn slow_server(name: &str) -> (PipeServer, Receiver<()>) {
    let (tx, rx) = mpsc::channel();
    let backend = SlowBackend {
        inner: Volumes::with_index(SAMPLE_VOLUME, sample_index(REPORTS)),
        entered: Mutex::new(tx),
    };
    (PipeServer::start(name, Arc::new(backend)).unwrap(), rx)
}

#[test]
fn new_query_cancels_the_running_query_on_the_same_connection() {
    let name = unique_name();
    let (_server, entered) = slow_server(&name);
    let mut pipe = greeted(&name);

    let slow = Request::Query(QueryMessage {
        condition: Condition::NameContains {
            part: "느린".into(),
            case: Case::Insensitive,
        },
        sort: None,
        limit: None,
    });
    write_message(&mut pipe, &slow).unwrap();
    entered
        .recv_timeout(Duration::from_secs(10))
        .expect("느린 질의가 시작되지 않음");

    // 앞선 질의가 도는 중에 새 질의를 보낸다.
    write_message(&mut pipe, &reports(Some(10))).unwrap();

    // 응답은 요청 순서대로 하나씩: 앞선 질의는 취소, 새 질의는 결과.
    assert_eq!(
        error_kind(read_message(&mut pipe).unwrap()),
        ErrorKind::Cancelled
    );
    let Response::Page(page) = read_message(&mut pipe).unwrap() else {
        panic!("결과 조각이 아님")
    };
    assert_eq!(page.total, REPORTS);
    assert_eq!(page.hits.len(), 10);
}

#[test]
fn new_query_invalidates_cursors_of_earlier_queries() {
    let name = unique_name();
    let _server = sample_server(&name);
    let mut pipe = greeted(&name);

    let Response::Page(first) = ask(&mut pipe, &reports(Some(10))) else {
        panic!("결과 조각이 아님")
    };
    let cursor = first.next.expect("남은 결과가 있어야 함");
    assert!(matches!(
        ask(&mut pipe, &reports(Some(10))),
        Response::Page(_)
    ));
    assert_eq!(
        error_kind(ask(
            &mut pipe,
            &Request::Resume(ResumeMessage { cursor, limit: 10 })
        )),
        ErrorKind::StaleCursor
    );
}

#[test]
fn connections_do_not_cancel_each_other() {
    let name = unique_name();
    let (_server, entered) = slow_server(&name);
    let mut a = greeted(&name);
    let mut b = greeted(&name);

    write_message(&mut a, &reports(Some(5))).unwrap();
    // b의 질의는 a의 앞선 질의를 취소하지 않는다.
    let Response::Page(page) = ask(&mut b, &reports(Some(5))) else {
        panic!("결과 조각이 아님")
    };
    assert_eq!(page.hits.len(), 5);
    let Response::Page(page) = read_message(&mut a).unwrap() else {
        panic!("결과 조각이 아님")
    };
    assert_eq!(page.hits.len(), 5);
    assert!(entered.try_recv().is_err());
}

#[test]
fn invalid_query_is_an_error_and_the_connection_stays_open() {
    let name = unique_name();
    let _server = sample_server(&name);
    let mut pipe = greeted(&name);
    let bad = Request::Query(QueryMessage {
        condition: Condition::NameRegex {
            pattern: "(".into(),
            case: Case::Insensitive,
        },
        sort: None,
        limit: None,
    });
    assert_eq!(error_kind(ask(&mut pipe, &bad)), ErrorKind::InvalidQuery);
    assert!(matches!(
        ask(&mut pipe, &Request::Status),
        Response::Status(_)
    ));
}

#[test]
fn second_server_with_the_same_name_is_refused() {
    let name = unique_name();
    let _first = sample_server(&name);
    let second = PipeServer::start(&name, Arc::new(SampleBackend::new(1)));
    let err = second.err().expect("같은 이름의 둘째 서버가 시작됨");
    assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
}

#[test]
fn name_is_free_again_after_the_server_stops() {
    let name = unique_name();
    drop(sample_server(&name));
    let _again = sample_server(&name);
}

#[test]
fn pipe_security_opens_only_to_interactive_users_and_the_server_account() {
    let name = unique_name();
    let server = sample_server(&name);
    let security = server.security();
    let me = current_user_sid().unwrap();

    assert!(security.protected, "상속받는 ACE가 없어야 함");

    let allowed: Vec<_> = security.aces.iter().filter(|a| a.allow).collect();
    assert_eq!(allowed.len(), 2, "{:?}", security.aces);
    let interactive = allowed
        .iter()
        .find(|a| a.sid == INTERACTIVE_SID)
        .expect("대화형 로그온 그룹 허용");
    assert_eq!(interactive.mask, CLIENT_ACCESS);
    // 읽기와 데이터 쓰기는 되지만 인스턴스를 더 만들어 가로채지는 못한다.
    assert_eq!(interactive.mask & 0x1, 0x1, "FILE_READ_DATA");
    assert_eq!(interactive.mask & 0x2, 0x2, "FILE_WRITE_DATA");
    assert_eq!(interactive.mask & FILE_CREATE_PIPE_INSTANCE, 0);
    assert_eq!(interactive.mask & 0x000C_0000, 0, "WRITE_DAC·WRITE_OWNER");
    let owner = allowed
        .iter()
        .find(|a| a.sid == me)
        .expect("서버 계정 허용");
    assert_eq!(owner.mask, FILE_ALL_ACCESS);

    // 원격(NETWORK)은 거부한다.
    assert!(
        security
            .aces
            .iter()
            .any(|a| !a.allow && a.sid == NETWORK_SID && a.mask == FILE_ALL_ACCESS),
        "{:?}",
        security.aces
    );
    // 모두(Everyone)·인증된 사용자·익명에게는 열지 않는다.
    for sid in ["S-1-1-0", "S-1-5-11", "S-1-5-7"] {
        assert!(
            !allowed.iter().any(|a| a.sid == sid),
            "{sid}: {:?}",
            security.aces
        );
    }
}

#[test]
fn path_condition_query_returns_results() {
    let name = unique_name();
    let _server = sample_server(&name);
    let mut pipe = greeted(&name);
    let query = |condition| {
        Request::Query(QueryMessage {
            condition,
            sort: Some((SortKey::Path, Order::Ascending)),
            limit: Some(100),
        })
    };

    // 폴더 이름 조각: `사진` 폴더 아래 두 파일.
    let Response::Page(page) = ask(
        &mut pipe,
        &query(Condition::PathContains {
            part: r"사진\".into(),
            case: Case::Insensitive,
        }),
    ) else {
        panic!("결과 조각이 아님")
    };
    let paths: Vec<_> = page.hits.iter().map(|h| h.path.clone()).collect();
    assert_eq!(
        paths,
        [
            format!(r"{SAMPLE_VOLUME}\사진\README.md"),
            format!(r"{SAMPLE_VOLUME}\사진\여름 바다.jpg"),
        ]
    );

    // 경로 와일드카드와 이름 조건 섞기.
    let Response::Page(page) = ask(
        &mut pipe,
        &query(Condition::And(vec![
            Condition::PathWildcard {
                pattern: format!(r"{SAMPLE_VOLUME}\문서\*.txt"),
                case: Case::Insensitive,
            },
            Condition::NameContains {
                part: "보고서 1".into(),
                case: Case::Insensitive,
            },
        ])),
    ) else {
        panic!("결과 조각이 아님")
    };
    assert!(page.total >= 1);
    assert!(page.hits.iter().all(|h| h.name.starts_with("보고서 1")));
}
