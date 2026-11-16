//! 즉시 검색의 **강등**: 서비스를 쓸 수 없으면 같은 조건 트리로 실시간 탐색을 띄워 같은 검색 결과 위치에 결과를 넣고
//! 까닭을 탭이 드러낸다. 임시 폴더의 실제 파일과 같은 프로세스의 인덱서 파이프 서버(합성 인덱스)·가짜 서버를 쓴다.
//! 관리자 권한이 필요 없고 Windows 전용이다(파이프).

#![cfg(windows)]

use std::path::{Component, Path};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::command::CommandId;
use facet_core::indexer_client::{CONNECT_TIMEOUT, ClientOptions, Connection};
use facet_core::instant_search::{DEMOTED_SEARCH_DELAY, DemotionReason};
use facet_core::tab::{ReadOutcome, Tab};
use facet_core::window::App;
use facet_index::{ATTRIBUTE_DIRECTORY, Index, NewItem};
use facet_indexer::pipe::PipeServer;
use facet_indexer::sample::SampleBackend;
use facet_indexer::volumes::Volumes;
use facet_protocol::{
    NoIndexReason, PROTOCOL_VERSION, Request, Response, StatusMessage, VolumeState, VolumeStatus,
    read_message, write_message,
};
use facet_win::pipe_client::testing::Listener;
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(30);
const ROOT: u64 = 5;

fn unique_pipe() -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        r"\\.\pipe\facet-core-demotion-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn options(pipe: &str) -> ClientOptions {
    ClientOptions {
        pipe: pipe.to_owned(),
        connect_timeout: CONNECT_TIMEOUT,
        reconnect_interval: Duration::from_millis(100),
    }
}

fn volume_of(dir: &Path) -> String {
    match dir.components().next() {
        Some(Component::Prefix(prefix)) => prefix.as_os_str().to_string_lossy().into_owned(),
        other => panic!("드라이브 경로가 아니다: {other:?}"),
    }
}

/// `dir`와 같은 경로를 가진 합성 인덱스: `dir` 아래 `보고서 인덱스 N.txt` 세 개.
fn index_for(dir: &Path) -> (String, Index) {
    let volume = volume_of(dir);
    let mut index = Index::new(&volume, ROOT);
    let mut next = 100u64;
    let mut parent = ROOT;
    for component in dir.components().skip(1) {
        if let Component::Normal(name) = component {
            let number = next;
            next += 1;
            index
                .insert(NewItem {
                    number,
                    parent,
                    name: &name.to_string_lossy(),
                    size: 0,
                    modified: 133_000_000_000_000_000,
                    attributes: ATTRIBUTE_DIRECTORY,
                })
                .unwrap();
            parent = number;
        }
    }
    for n in 1..=3u64 {
        index
            .insert(NewItem {
                number: 10_000 + n,
                parent,
                name: &format!("보고서 인덱스 {n}.txt"),
                size: 1,
                modified: 133_000_000_000_000_000,
                attributes: 0x20,
            })
            .unwrap();
    }
    (volume, index)
}

/// 실제 파일이 든 시험 폴더: 폴더 안과 하위 폴더 안에 「보고서」 파일, 맞지 않는 파일.
fn make_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("보고서 a.txt"), "a").unwrap();
    std::fs::write(dir.path().join("보고서 b.md"), "b").unwrap();
    std::fs::write(dir.path().join("other.txt"), "o").unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub").join("보고서 c.txt"), "c").unwrap();
    dir
}

fn app_in(dir: &Path) -> App {
    let mut app = App::new(Some(Location::file_system(dir)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

fn wait_read(app: &mut App) {
    loop {
        let (tab, outcome) = app.wait(WAIT).expect("읽기 결과");
        if tab == app.active_tab_id() {
            assert_eq!(outcome, ReadOutcome::Committed);
            return;
        }
    }
}

fn wait_until(app: &mut App, what: &str, done: impl Fn(&Tab) -> bool) {
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll();
        if done(app.active_tab()) {
            return;
        }
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// 강등한 실시간 탐색이 끝났다.
fn live_finished(tab: &Tab) -> bool {
    tab.instant().is_some_and(|s| s.demotion.is_some())
        && tab.search().is_some_and(|s| s.summary.is_some())
}

/// 즉시 검색 결과가 자리 잡았다(강등하지 않고 전체 개수가 옴).
fn instant_settled(tab: &Tab) -> bool {
    tab.instant()
        .is_some_and(|s| s.demotion.is_none() && !s.loading && s.total.is_some())
}

fn names(tab: &Tab) -> Vec<String> {
    let mut names: Vec<String> = tab
        .items()
        .map(|item| item.name.to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// 첫 글자를 넘기고 결과 위치로 간다.
fn type_first(app: &mut App, text: &str) {
    app.set_instant_query(text);
    wait_read(app);
}

#[test]
fn 서버가_없으면_실시간_탐색으로_같은_조건의_항목이_들어오고_범위는_폴더_아래로_좁아진다() {
    let dir = make_dir();
    // 연결을 띄우지 않은 앱과, 없는 파이프에 붙은 앱 둘 다.
    for connect in [false, true] {
        let mut app = app_in(dir.path());
        if connect {
            app.enable_indexer(options(&unique_pipe()));
        }
        type_first(&mut app, "보고서 ext:txt");
        wait_until(&mut app, "강등한 탐색이 끝나지 않았다", live_finished);
        let tab = app.active_tab();
        // 같은 조건 트리(이름 조각 그리고 확장자)로 하위 폴더 안까지 훑는다.
        assert_eq!(names(tab), ["보고서 a.txt", "보고서 c.txt"]);
        let demotion = tab.instant().unwrap().demotion.unwrap();
        assert_eq!(demotion.reason, DemotionReason::NoService);
        assert!(
            demotion.narrowed_to_folder,
            "모든 볼륨 범위는 폴더 아래로 좁아진다"
        );
        assert!(tab.instant().unwrap().total.is_none());
        // 같은 검색 결과 위치다(기록 하나).
        assert!(tab.location().is_search_results());
        assert_eq!(tab.back_history(), [Location::file_system(dir.path())]);
    }
}

#[test]
fn 현재_폴더_아래만이면_좁힘을_드러내지_않는다() {
    let dir = make_dir();
    let mut app = app_in(dir.path());
    app.execute(CommandId::InstantUnderFolder);
    type_first(&mut app, "보고서");
    wait_until(&mut app, "강등한 탐색이 끝나지 않았다", live_finished);
    let demotion = app.active_tab().instant().unwrap().demotion.unwrap();
    assert_eq!(demotion.reason, DemotionReason::NoService);
    assert!(!demotion.narrowed_to_folder);
    assert_eq!(names(app.active_tab()).len(), 3);
}

#[test]
fn 강등_중_이어_치면_앞선_탐색은_멈추고_새_탐색은_잠깐_멈춘_뒤_시작한다() {
    let dir = make_dir();
    let mut app = app_in(dir.path());
    type_first(&mut app, "보고서");
    wait_until(&mut app, "첫 탐색이 끝나지 않았다", live_finished);
    assert_eq!(names(app.active_tab()).len(), 3);

    // 이어 친다: 앞선 탐색은 곧바로 멈추고, 새 탐색은 정한 시간 뒤에 시작한다(그동안 앞 결과가 보인다).
    let typed = Instant::now();
    app.set_instant_query("보고서 a");
    let tab = app.active_tab();
    let due = tab.demoted_search_due().expect("시작을 미뤘다");
    assert!(due >= typed + DEMOTED_SEARCH_DELAY, "{:?}", due - typed);
    assert!(tab.search().unwrap().running);
    assert!(tab.search().unwrap().summary.is_none());
    assert_eq!(
        names(tab).len(),
        3,
        "새 탐색이 시작할 때까지 앞 결과가 남는다"
    );

    // 또 치면 시작할 때가 다시 미뤄진다(앞의 입력으로는 훑지 않는다).
    std::thread::sleep(Duration::from_millis(20));
    app.set_instant_query("보고서 ab");
    assert!(app.active_tab().demoted_search_due().unwrap() > due);

    wait_until(&mut app, "새 탐색이 끝나지 않았다", live_finished);
    assert!(typed.elapsed() >= DEMOTED_SEARCH_DELAY);
    assert!(names(app.active_tab()).is_empty(), "마지막 입력의 조건이다");
}

#[test]
fn 지원하지_않는_버전으로_답하는_서버면_버전_다름으로_강등한다() {
    let dir = make_dir();
    let pipe = unique_pipe();
    let listener = Arc::new(Listener::bind(&pipe).unwrap());
    let server = {
        let listener = Arc::clone(&listener);
        std::thread::spawn(move || {
            while let Ok(Some(client)) = listener.accept(Duration::from_secs(3)) {
                let (mut reader, mut writer, _closer) = client.split().unwrap();
                let Ok(Request::Hello { version }) = read_message::<Request>(&mut reader) else {
                    continue;
                };
                let _ = write_message(
                    &mut writer,
                    &Response::HelloRejected {
                        client_version: version,
                        min_version: PROTOCOL_VERSION + 1,
                        max_version: PROTOCOL_VERSION + 3,
                    },
                );
            }
        })
    };
    let mut app = app_in(dir.path());
    app.enable_indexer(options(&pipe));
    type_first(&mut app, "보고서");
    wait_until(&mut app, "버전 다름으로 강등하지 않았다", live_finished);
    let tab = app.active_tab();
    let demotion = tab.instant().unwrap().demotion.unwrap();
    assert_eq!(demotion.reason, DemotionReason::VersionMismatch);
    assert!(demotion.narrowed_to_folder);
    assert_eq!(names(tab).len(), 3);
    assert!(matches!(
        app.indexer_connection(),
        Some(Connection::VersionMismatch { .. })
    ));
    drop(app);
    drop(listener);
    let _ = server.join();
}

#[test]
fn 범위_폴더의_볼륨에_인덱스가_없으면_그_까닭으로_강등한다() {
    for missing in [false, true] {
        let dir = make_dir();
        let (volume, index) = index_for(dir.path());
        let volumes = if missing {
            // 상태에 그 볼륨이 없다.
            Volumes::new()
        } else {
            let volumes = Volumes::with_index(&volume, index);
            assert!(volumes.set_not_indexed(
                &volume,
                NoIndexReason::NotNtfs {
                    filesystem: "exFAT".into()
                }
            ));
            volumes
        };
        let pipe = unique_pipe();
        let _server = PipeServer::start(&pipe, Arc::new(SampleBackend(volumes))).unwrap();
        let mut app = app_in(dir.path());
        app.enable_indexer(options(&pipe));
        type_first(&mut app, "보고서");
        wait_until(&mut app, "인덱스 없음으로 강등하지 않았다", live_finished);
        let tab = app.active_tab();
        let demotion = tab.instant().unwrap().demotion.unwrap();
        assert_eq!(demotion.reason, DemotionReason::VolumeNotIndexed);
        // 서비스는 살아 있고 인덱스만 없으므로 실제 파일에서 찾는다(인덱스의 「보고서 인덱스」 항목은 나오지 않는다).
        assert_eq!(
            names(tab),
            ["보고서 a.txt", "보고서 b.md", "보고서 c.txt"],
            "missing = {missing}"
        );
    }
}

#[test]
fn 검색_중_서버가_멈추면_강등하고_서버가_다시_뜨면_다음_입력이_즉시_검색이다() {
    let dir = make_dir();
    let (volume, index) = index_for(dir.path());
    let pipe = unique_pipe();
    // 질의를 받고 답하지 않은 채 끊는 서버.
    let listener = Listener::bind(&pipe).unwrap();
    let ready_volume = volume.clone();
    let server = std::thread::spawn(move || {
        let client = listener
            .accept(Duration::from_secs(10))
            .unwrap()
            .expect("클라이언트가 붙지 않았다");
        let (mut reader, mut writer, _closer) = client.split().unwrap();
        assert!(matches!(
            read_message::<Request>(&mut reader).unwrap(),
            Request::Hello { .. }
        ));
        write_message(
            &mut writer,
            &Response::HelloAccepted {
                version: PROTOCOL_VERSION,
            },
        )
        .unwrap();
        // 상태 질의에는 인덱스가 있다고 답하고, 그 뒤 질의에는 답하지 않고 끊는다.
        let Request::Status = read_message::<Request>(&mut reader).unwrap() else {
            panic!("상태 질의가 먼저다");
        };
        write_message(
            &mut writer,
            &Response::Status(StatusMessage {
                volumes: vec![VolumeStatus {
                    volume: ready_volume,
                    state: VolumeState::Ready { items: 1 },
                }],
            }),
        )
        .unwrap();
        let Request::Query(_) = read_message::<Request>(&mut reader).unwrap() else {
            panic!("질의가 아니다");
        };
        // 서버를 멈춘다(연결과 이름을 놓는다).
    });
    let mut app = app_in(dir.path());
    app.enable_indexer(options(&pipe));
    type_first(&mut app, "보고서");
    // 질의에 답하지 않고 서버가 멈추면 탭이 멈추지 않고 그 질의를 강등한다.
    wait_until(&mut app, "서버가 멈춘 뒤 강등하지 않았다", live_finished);
    server.join().unwrap();
    let tab = app.active_tab();
    assert_eq!(
        tab.instant().unwrap().demotion.unwrap().reason,
        DemotionReason::NoService
    );
    assert_eq!(names(tab), ["보고서 a.txt", "보고서 b.md", "보고서 c.txt"]);

    // 서버를 다시 띄운다.
    let backend = Arc::new(SampleBackend(Volumes::with_index(&volume, index)));
    let _server = PipeServer::start(&pipe, backend).unwrap();
    wait_until_connected(&mut app);
    // 강등 중이던 탭은 서버가 돌아와도 스스로 바꾸지 않는다: 다음 입력부터 즉시 검색이다.
    assert!(app.active_tab().instant().unwrap().demotion.is_some());
    app.set_instant_query("보고서 인덱스");
    wait_until(
        &mut app,
        "다음 입력이 즉시 검색이 되지 않았다",
        instant_settled,
    );
    let tab = app.active_tab();
    assert_eq!(tab.instant().unwrap().total, Some(3));
    assert_eq!(
        names(tab),
        [
            "보고서 인덱스 1.txt",
            "보고서 인덱스 2.txt",
            "보고서 인덱스 3.txt"
        ]
    );
}

fn wait_until_connected(app: &mut App) {
    let deadline = Instant::now() + WAIT;
    while !matches!(app.indexer_connection(), Some(Connection::Connected { .. })) {
        app.poll();
        assert!(Instant::now() < deadline, "서버에 다시 붙지 못했다");
        std::thread::sleep(Duration::from_millis(10));
    }
}
