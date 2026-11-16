//! 인덱서 서비스 연결(`indexer_client`). 같은 프로세스에 합성 인덱스의 인덱서 파이프 서버나, 버전 인사를 거절·
//! 답하지 않음·끊김을 흉내 내는 가짜 파이프 서버를 띄워 본다. 관리자 권한이 필요 없다.

use std::time::Duration;

use facet_core::indexer_client::{CONNECT_TIMEOUT, ClientOptions, RECONNECT_INTERVAL};

#[test]
fn default_options_use_the_documented_times() {
    let options = ClientOptions::default();
    assert_eq!(options.connect_timeout, CONNECT_TIMEOUT);
    assert_eq!(options.reconnect_interval, RECONNECT_INTERVAL);
    assert_eq!(CONNECT_TIMEOUT, Duration::from_millis(200));
    assert!(!options.pipe.is_empty());
}

#[cfg(windows)]
mod windows {
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    use facet_core::indexer_client::{
        CONNECT_TIMEOUT, ClientOptions, Connection, IndexerClient, IndexerEvent, Reply,
        RequestError, RequestId,
    };
    use facet_index::{ATTRIBUTE_DIRECTORY, Index, NewItem};
    use facet_indexer::pipe::PipeServer;
    use facet_indexer::sample::SampleBackend;
    use facet_indexer::volumes::Volumes;
    use facet_protocol::{
        Case, Condition, HitMessage, Order, PROTOCOL_VERSION, PageMessage, QueryMessage, Request,
        Response, ResumeMessage, SortKey, VolumeState, read_message, write_message,
    };
    use facet_win::pipe_client::testing::Listener;
    use facet_win::pipe_client::{PipeReader, PipeWriter};

    const VOLUME: &str = "T:";
    const ROOT: u64 = 5;
    const REPORTS: u64 = 50;

    /// 테스트끼리, 그리고 실제 서비스와 겹치지 않는 파이프 이름.
    fn unique_pipe() -> String {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        format!(
            r"\\.\pipe\facet-core-indexer-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// 인덱스 크레이트로 만든 합성 인덱스: 「문서」 아래 보고서 [`REPORTS`]개와 「사진」 아래 사진 하나.
    fn synthetic_index() -> Index {
        let mut index = Index::new(VOLUME, ROOT);
        let mut put = |number, parent, name: &str, size, attributes| {
            index
                .insert(NewItem {
                    number,
                    parent,
                    name,
                    size,
                    modified: 133_000_000_000_000_000 + number,
                    attributes,
                })
                .unwrap();
        };
        put(100, ROOT, "문서", 0, ATTRIBUTE_DIRECTORY);
        put(101, ROOT, "사진", 0, ATTRIBUTE_DIRECTORY);
        put(102, 101, "여름 바다.jpg", 2_000_000, 0x20);
        for n in 1..=REPORTS {
            put(1000 + n, 100, &format!("보고서 {n:02}.txt"), n * 100, 0x20);
        }
        index
    }

    fn index_server(pipe: &str) -> PipeServer {
        let backend = SampleBackend(Volumes::with_index(VOLUME, synthetic_index()));
        PipeServer::start(pipe, Arc::new(backend)).unwrap()
    }

    fn options(pipe: &str) -> ClientOptions {
        ClientOptions {
            pipe: pipe.to_owned(),
            connect_timeout: CONNECT_TIMEOUT,
            reconnect_interval: Duration::from_millis(100),
        }
    }

    fn name_query(part: &str, limit: Option<u32>) -> QueryMessage {
        QueryMessage {
            condition: Condition::NameContains {
                part: part.into(),
                case: Case::Insensitive,
            },
            sort: Some((SortKey::Name, Order::Ascending)),
            limit,
        }
    }

    /// 부르는 스레드를 막지 않는가: 한 번 부르는 데 이보다 오래 걸리면 기다린 것이다(연결 판정 200ms보다 훨씬 짧다).
    const NON_BLOCKING: Duration = Duration::from_millis(50);

    fn quick<T>(what: &str, f: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        let value = f();
        let took = started.elapsed();
        assert!(took < NON_BLOCKING, "{what}이(가) {took:?} 동안 막았다");
        value
    }

    /// `want` 상태가 될 때까지 사건을 받는다. 그때까지 받은 다른 사건과 걸린 시간을 돌려준다.
    fn wait_for_connection(
        client: &IndexerClient,
        want: &Connection,
        within: Duration,
    ) -> (Vec<IndexerEvent>, Duration) {
        let started = Instant::now();
        let mut others = Vec::new();
        while started.elapsed() < within {
            match client.wait_event(Duration::from_millis(10)) {
                Some(IndexerEvent::Connection(c)) if &c == want => {
                    return (others, started.elapsed());
                }
                Some(other) => others.push(other),
                None => {}
            }
        }
        panic!(
            "{within:?} 안에 {want:?}가 되지 않았다(지금 {:?}, 받은 사건 {others:?})",
            client.connection()
        );
    }

    fn wait_reply(client: &IndexerClient, id: RequestId) -> Result<Reply, RequestError> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some(IndexerEvent::Reply { id: got, result }) =
                client.wait_event(Duration::from_millis(20))
                && got == id
            {
                return result;
            }
        }
        panic!("{id:?}의 답이 오지 않았다");
    }

    fn connected(pipe: &str) -> IndexerClient {
        let client = IndexerClient::start(options(pipe));
        wait_for_connection(
            &client,
            &Connection::Connected {
                version: PROTOCOL_VERSION,
            },
            Duration::from_secs(5),
        );
        client
    }

    fn page(result: Result<Reply, RequestError>) -> PageMessage {
        match result {
            Ok(Reply::Page(page)) => page,
            other => panic!("결과 조각이 아니다: {other:?}"),
        }
    }

    fn names(page: &PageMessage) -> Vec<String> {
        page.hits.iter().map(|h| h.name.clone()).collect()
    }

    #[test]
    fn query_resume_and_status_against_an_indexer_pipe_server() {
        let pipe = unique_pipe();
        let _server = index_server(&pipe);
        let client = connected(&pipe);
        assert_eq!(
            client.connection(),
            Connection::Connected {
                version: PROTOCOL_VERSION
            }
        );

        let id = quick("질의", || client.query(name_query("보고서", Some(10))));
        let first = page(wait_reply(&client, id));
        assert_eq!(first.total, REPORTS);
        assert_eq!(
            names(&first),
            (1..=10)
                .map(|n| format!("보고서 {n:02}.txt"))
                .collect::<Vec<_>>()
        );
        assert_eq!(first.hits[0].path, r"T:\문서\보고서 01.txt");
        let cursor = first.next.expect("남은 결과가 있다");

        let id = quick("이어 받기", || {
            client.resume(ResumeMessage { cursor, limit: 100 })
        });
        let rest = page(wait_reply(&client, id));
        assert_eq!(
            names(&rest),
            (11..=REPORTS)
                .map(|n| format!("보고서 {n:02}.txt"))
                .collect::<Vec<_>>()
        );
        assert_eq!(rest.next, None);

        let id = quick("상태 질의", || client.status());
        match wait_reply(&client, id) {
            Ok(Reply::Status(status)) => {
                assert_eq!(status.volumes.len(), 1);
                assert_eq!(status.volumes[0].volume, VOLUME);
                assert_eq!(
                    status.volumes[0].state,
                    VolumeState::Ready { items: REPORTS + 3 }
                );
            }
            other => panic!("상태가 아니다: {other:?}"),
        }

        // 서비스가 오류로 답한 질의(틀린 정규식)는 그 오류로 끝나고 연결은 그대로다.
        let id = client.query(QueryMessage {
            condition: Condition::NameRegex {
                pattern: "(".into(),
                case: Case::Insensitive,
            },
            sort: None,
            limit: None,
        });
        assert!(matches!(
            wait_reply(&client, id),
            Err(RequestError::Service(_))
        ));
        let id = client.query(name_query("여름", None));
        assert_eq!(names(&page(wait_reply(&client, id))), ["여름 바다.jpg"]);
    }

    #[test]
    fn successive_queries_deliver_only_the_last_result() {
        let pipe = unique_pipe();
        let _server = index_server(&pipe);
        let client = connected(&pipe);

        // 앞선 질의들은 서비스가 취소로 답하거나 이미 답했어도 버려진다.
        let mut ids = Vec::new();
        for n in 0..20 {
            let part = if n % 2 == 0 { "보고서" } else { ".txt" };
            ids.push(quick("질의", || client.query(name_query(part, None))));
        }
        let last = client.query(name_query("여름", None));
        // 상태 질의는 질의와 상관없이 답을 받는다.
        let status = client.status();

        let mut replied = HashSet::new();
        let mut last_page = None;
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if let Some(IndexerEvent::Reply { id, result }) =
                client.wait_event(Duration::from_millis(20))
            {
                assert!(replied.insert(id), "{id:?}의 답이 두 번 왔다");
                if id == last {
                    last_page = Some(page(result));
                }
            }
        }
        assert_eq!(replied, HashSet::from([last, status]));
        assert_eq!(names(&last_page.unwrap()), ["여름 바다.jpg"]);
        for id in ids {
            assert!(!replied.contains(&id));
        }
    }

    /// 가짜 서버: 연결 하나를 받아 `handle`에 넘긴다. 연결은 `handle`이 돌아오면 닫힌다.
    fn fake_server(
        listener: Arc<Listener>,
        handle: impl FnOnce(&mut PipeReader, &mut PipeWriter) + Send + 'static,
    ) -> JoinHandle<()> {
        thread::spawn(move || {
            let client = listener
                .accept(Duration::from_secs(10))
                .unwrap()
                .expect("클라이언트가 붙지 않았다");
            let (mut reader, mut writer, _closer) = client.split().unwrap();
            handle(&mut reader, &mut writer);
        })
    }

    fn greet(reader: &mut PipeReader, writer: &mut PipeWriter) {
        assert_eq!(
            read_message::<Request>(reader).unwrap(),
            Request::Hello {
                version: PROTOCOL_VERSION
            }
        );
        write_message(
            writer,
            &Response::HelloAccepted {
                version: PROTOCOL_VERSION,
            },
        )
        .unwrap();
    }

    fn one_hit_page(name: &str) -> Response {
        Response::Page(PageMessage {
            hits: vec![HitMessage {
                path: format!(r"T:\{name}"),
                name: name.into(),
                size: 1,
                modified: 1,
                attributes: 0x20,
            }],
            total: 1,
            total_is_upper_bound: false,
            next: None,
            building: Vec::new(),
        })
    }

    #[test]
    fn late_response_of_an_earlier_query_is_dropped() {
        let pipe = unique_pipe();
        let listener = Arc::new(Listener::bind(&pipe).unwrap());
        let (got_both, both) = std::sync::mpsc::channel();
        let (finish, finished) = std::sync::mpsc::channel::<()>();
        let server = fake_server(Arc::clone(&listener), move |reader, writer| {
            greet(reader, writer);
            // 두 질의를 다 받은 뒤 앞선 질의에도 (취소가 아닌) 결과로 늦게 답한다.
            let first = read_message::<Request>(reader).unwrap();
            let second = read_message::<Request>(reader).unwrap();
            got_both.send((first, second)).unwrap();
            write_message(writer, &one_hit_page("앞선 결과")).unwrap();
            write_message(writer, &one_hit_page("마지막 결과")).unwrap();
            let _ = finished.recv_timeout(Duration::from_secs(10));
        });
        let client = connected(&pipe);
        let first = client.query(name_query("앞선", None));
        let second = client.query(name_query("마지막", None));
        let (a, b) = both.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(a, Request::Query(name_query("앞선", None)));
        assert_eq!(b, Request::Query(name_query("마지막", None)));

        let mut replies = Vec::new();
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            if let Some(IndexerEvent::Reply { id, result }) =
                client.wait_event(Duration::from_millis(20))
            {
                replies.push((id, result));
            }
        }
        assert_eq!(replies.len(), 1, "{replies:?}");
        assert_eq!(replies[0].0, second);
        assert_ne!(replies[0].0, first);
        assert_eq!(names(&page(replies[0].1.clone())), ["마지막 결과"]);
        finish.send(()).unwrap();
        server.join().unwrap();
    }

    #[test]
    fn no_server_is_judged_no_service_at_once_and_requests_fail_without_waiting() {
        let pipe = unique_pipe();
        let client = quick("시작", || IndexerClient::start(options(&pipe)));
        let (_, took) =
            wait_for_connection(&client, &Connection::NoService, Duration::from_secs(5));
        assert!(
            took < CONNECT_TIMEOUT,
            "파이프가 없으면 판정 시간({CONNECT_TIMEOUT:?}) 안에 알아야 하는데 {took:?}"
        );
        let id = quick("질의", || client.query(name_query("보고서", None)));
        assert_eq!(
            wait_reply(&client, id),
            Err(RequestError::NotConnected(Connection::NoService))
        );
        let id = quick("상태 질의", || client.status());
        assert_eq!(
            wait_reply(&client, id),
            Err(RequestError::NotConnected(Connection::NoService))
        );
    }

    #[test]
    fn silent_server_is_judged_no_service_within_the_timeout_without_blocking_the_caller() {
        let pipe = unique_pipe();
        let listener = Arc::new(Listener::bind(&pipe).unwrap());
        let (finish, finished) = std::sync::mpsc::channel::<()>();
        let server = fake_server(Arc::clone(&listener), move |reader, _| {
            // 버전 인사를 읽기만 하고 답하지 않는다.
            let _ = read_message::<Request>(reader);
            let _ = finished.recv_timeout(Duration::from_secs(10));
        });
        let started = Instant::now();
        let client = quick("시작", || IndexerClient::start(options(&pipe)));
        // 판정하는 동안에도 요청은 막지 않는다. 판정이 끝나면 「연결 안 됨」으로 답한다.
        let id = quick("판정 중 질의", || {
            client.query(name_query("보고서", None))
        });
        assert_eq!(
            quick("상태 읽기", || client.connection()),
            Connection::Connecting
        );
        let (_, _) = wait_for_connection(&client, &Connection::NoService, Duration::from_secs(5));
        let took = started.elapsed();
        assert!(
            took >= CONNECT_TIMEOUT - Duration::from_millis(20),
            "{took:?}"
        );
        assert!(
            took < CONNECT_TIMEOUT + Duration::from_millis(500),
            "판정이 {took:?} 걸렸다"
        );
        assert_eq!(
            wait_reply(&client, id),
            Err(RequestError::NotConnected(Connection::NoService))
        );
        finish.send(()).unwrap();
        server.join().unwrap();
    }

    #[test]
    fn server_rejecting_the_version_is_a_version_mismatch() {
        let pipe = unique_pipe();
        let listener = Arc::new(Listener::bind(&pipe).unwrap());
        let server = fake_server(Arc::clone(&listener), |reader, writer| {
            let Request::Hello { version } = read_message::<Request>(reader).unwrap() else {
                panic!("버전 인사가 먼저 와야 한다");
            };
            write_message(
                writer,
                &Response::HelloRejected {
                    client_version: version,
                    min_version: PROTOCOL_VERSION + 1,
                    max_version: PROTOCOL_VERSION + 3,
                },
            )
            .unwrap();
        });
        let client = IndexerClient::start(options(&pipe));
        let want = Connection::VersionMismatch {
            client_version: PROTOCOL_VERSION,
            min_version: PROTOCOL_VERSION + 1,
            max_version: PROTOCOL_VERSION + 3,
        };
        let (_, took) = wait_for_connection(&client, &want, Duration::from_secs(5));
        assert!(
            took < CONNECT_TIMEOUT + Duration::from_millis(500),
            "{took:?}"
        );
        let id = client.query(name_query("보고서", None));
        assert_eq!(
            wait_reply(&client, id),
            Err(RequestError::NotConnected(want))
        );
        server.join().unwrap();
    }

    #[test]
    fn stopped_server_disconnects_and_a_restarted_server_reconnects() {
        let pipe = unique_pipe();
        let listener = Arc::new(Listener::bind(&pipe).unwrap());
        let (asked, got_query) = std::sync::mpsc::channel();
        let (stop, stopping) = std::sync::mpsc::channel::<()>();
        let server = fake_server(Arc::clone(&listener), move |reader, writer| {
            greet(reader, writer);
            asked
                .send(read_message::<Request>(reader).unwrap())
                .unwrap();
            // 답하지 않고 멈춘다(연결이 닫힌다).
            stopping.recv().unwrap();
        });
        let client = connected(&pipe);
        let id = client.query(name_query("보고서", None));
        assert!(matches!(
            got_query.recv_timeout(Duration::from_secs(5)).unwrap(),
            Request::Query(_)
        ));

        // 서비스가 멈춤: 파이프 이름도 사라지고 받은 연결도 끊긴다.
        drop(listener);
        let stopped_at = Instant::now();
        stop.send(()).unwrap();
        server.join().unwrap();
        assert_eq!(wait_reply(&client, id), Err(RequestError::Disconnected));
        wait_for_connection(&client, &Connection::NoService, Duration::from_secs(5));
        assert!(stopped_at.elapsed() < Duration::from_secs(2));
        // 끊겨 있는 동안에도 요청은 막지 않는다.
        let id = quick("끊긴 동안 질의", || {
            client.query(name_query("보고서", None))
        });
        assert_eq!(
            wait_reply(&client, id),
            Err(RequestError::NotConnected(Connection::NoService))
        );

        // 다시 띄우면 뒤에서 다시 붙는다.
        let _server = index_server(&pipe);
        wait_for_connection(
            &client,
            &Connection::Connected {
                version: PROTOCOL_VERSION,
            },
            Duration::from_secs(5),
        );
        let id = client.query(name_query("여름", None));
        assert_eq!(names(&page(wait_reply(&client, id))), ["여름 바다.jpg"]);
    }

    #[test]
    fn dropping_the_client_does_not_wait() {
        let pipe = unique_pipe();
        let _server = index_server(&pipe);
        let client = connected(&pipe);
        let _ = client.query(name_query("보고서", None));
        quick("버리기", || drop(client));
        // 끊긴 클라이언트를 기다리지 않는 상태로도 같은 이름의 서버에 새 클라이언트가 붙는다.
        let again = connected(&pipe);
        let id = again.status();
        assert!(matches!(wait_reply(&again, id), Ok(Reply::Status(_))));
    }
}
