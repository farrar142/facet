//! **즉시 검색**을 검색 결과 위치에 넣는 코어 흐름: 같은 프로세스의 인덱서 파이프 서버(합성 인덱스)에 입력마다 질의를 보내
//! 결과가 탭에 들어오는지 공개 인터페이스로 확인한다. 관리자 권한이 필요 없고 Windows 전용이다(파이프).
//!
//! 합성 인덱스는 임시 폴더의 실제 경로와 같은 경로를 갖는다(「현재 폴더 아래만」이 그 폴더 조건으로 좁히는 것을 보려고).

#![cfg(windows)]

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use facet_core::command::CommandId;
use facet_core::details::{Column, Direction, Sort};
use facet_core::indexer_client::{CONNECT_TIMEOUT, ClientOptions};
use facet_core::instant_search::{PAGE_SIZE, Unavailable};
use facet_core::menu::MenuKind;
use facet_core::tab::{Navigation, ReadOutcome, Tab};
use facet_core::window::{App, Execution};
use facet_core::{Location, SearchKind};
use facet_index::{ATTRIBUTE_DIRECTORY, Index, NewItem};
use facet_indexer::pipe::PipeServer;
use facet_indexer::sample::SampleBackend;
use facet_indexer::volumes::Volumes;
use facet_protocol::{
    ErrorKind, ErrorMessage, PROTOCOL_VERSION, PageMessage, Request, Response, read_message,
    write_message,
};
use facet_win::pipe_client::testing::Listener;
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(30);
const ROOT: u64 = 5;
/// 시험 폴더 아래 「보고서」 파일 수: 첫 화면 분량([`PAGE_SIZE`])을 넘어 이어 받기를 본다.
const REPORTS: u64 = 250;
/// 시험 폴더 밖(같은 볼륨의 다른 폴더)의 「보고서」 파일 수.
const OUTSIDE: u64 = 3;

fn unique_pipe() -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        r"\\.\pipe\facet-core-instant-test-{}-{}",
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

/// `dir`와 같은 경로를 가진 합성 인덱스: `dir` 아래 `보고서 001.txt`…(크기 = 번호 × 100)와 `dir`이 든 볼륨의 `기타` 폴더 아래
/// `보고서 외부 N.txt`. 볼륨 이름(`C:` 등)도 함께 돌려준다.
fn index_for(dir: &Path) -> (String, Index) {
    let mut components = dir.components();
    let volume = match components.next() {
        Some(Component::Prefix(prefix)) => prefix.as_os_str().to_string_lossy().into_owned(),
        other => panic!("드라이브 경로가 아니다: {other:?}"),
    };
    let mut index = Index::new(&volume, ROOT);
    let mut next = 100u64;
    let mut parent = ROOT;
    for component in components {
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
    for n in 1..=REPORTS {
        index
            .insert(NewItem {
                number: 10_000 + n,
                parent,
                name: &format!("보고서 {n:03}.txt"),
                size: n * 100,
                modified: 133_000_000_000_000_000 + n,
                attributes: 0x20,
            })
            .unwrap();
    }
    index
        .insert(NewItem {
            number: 9_000,
            parent: ROOT,
            name: "기타",
            size: 0,
            modified: 133_000_000_000_000_000,
            attributes: ATTRIBUTE_DIRECTORY,
        })
        .unwrap();
    for n in 1..=OUTSIDE {
        index
            .insert(NewItem {
                number: 9_100 + n,
                parent: 9_000,
                name: &format!("보고서 외부 {n}.txt"),
                size: 1,
                modified: 133_000_000_000_000_000,
                attributes: 0x20,
            })
            .unwrap();
    }
    (volume, index)
}

/// 시험 폴더·합성 인덱스·인덱서 파이프 서버·인덱서에 붙은 앱.
struct Fixture {
    dir: TempDir,
    volume: String,
    backend: Arc<SampleBackend>,
    _server: PipeServer,
    app: App,
}

impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let (volume, index) = index_for(dir.path());
        let backend = Arc::new(SampleBackend(Volumes::with_index(&volume, index)));
        let pipe = unique_pipe();
        let server = PipeServer::start(&pipe, backend.clone()).unwrap();
        let mut app = App::new(Some(Location::file_system(dir.path())));
        let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
        assert_eq!(outcome, ReadOutcome::Committed);
        app.enable_indexer(options(&pipe));
        Self {
            dir,
            volume,
            backend,
            _server: server,
            app,
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }
}

/// 활성 탭의 이동이 확정될 때까지 기다린다.
fn wait_read(app: &mut App) {
    loop {
        let (tab, outcome) = app.wait(WAIT).expect("읽기 결과");
        if tab == app.active_tab_id() {
            assert_eq!(outcome, ReadOutcome::Committed);
            return;
        }
    }
}

/// 활성 탭이 `done`이 될 때까지 서비스의 응답을 받는다.
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

/// 질의 응답이 들어와 결과가 자리 잡은 탭: 기다리는 요청이 없고 전체 개수나 서비스 없음이 정해졌다.
fn settled(tab: &Tab) -> bool {
    tab.instant()
        .is_some_and(|s| !s.loading && (s.total.is_some() || s.unavailable.is_some()))
}

fn wait_settled(app: &mut App) {
    wait_until(app, "응답이 오지 않았다", settled);
}

/// 보이는 결과의 이름(목록 순서).
fn names(tab: &Tab) -> Vec<String> {
    tab.items()
        .map(|item| item.name.to_string_lossy().into_owned())
        .collect()
}

/// 글자를 넘기고 결과 위치로 간 뒤 응답을 받는다(첫 글자).
fn type_first(app: &mut App, text: &str) {
    app.set_instant_query(text);
    wait_read(app);
    wait_settled(app);
}

/// 제자리에서 글자를 바꾸고 응답을 받는다(뒤 글자).
fn type_more(app: &mut App, text: &str) {
    app.set_instant_query(text);
    wait_settled(app);
}

#[test]
fn 글자를_넘기면_결과_위치가_되고_결과와_전체_개수가_들어오며_기록은_하나다() {
    let mut f = Fixture::new();
    let dir = f.path().to_path_buf();
    type_first(&mut f.app, "보고서");

    let tab = f.app.active_tab();
    assert_eq!(
        tab.location(),
        &Location::instant_search_results(&dir, "보고서", false)
    );
    assert!(tab.location().file_system_path().is_none());
    let status = tab.instant().expect("즉시 검색 상태");
    assert_eq!(status.total, Some(REPORTS + OUTSIDE));
    assert!(!status.total_is_upper_bound);
    assert!(!status.may_be_incomplete());
    assert_eq!(status.unavailable, None);
    assert_eq!(tab.len(), PAGE_SIZE as usize, "첫 질의는 한 화면 분량만");
    assert_eq!(tab.back_history(), [Location::file_system(&dir)]);
    // 항목은 인덱스가 준 값으로 만들고(디스크에 없는 파일이다) 든 폴더가 경로로 남는다.
    let first = tab.get(0).unwrap();
    assert!(!first.is_folder);
    assert_eq!(tab.folder_of(0), Some(dir.as_path()));

    // 글자를 바꿔도 기록 항목은 하나이고 위치가 그 자리에서 바뀐다.
    type_more(&mut f.app, "\"보고서 1\"");
    let tab = f.app.active_tab();
    assert_eq!(
        tab.location(),
        &Location::instant_search_results(&dir, "\"보고서 1\"", false)
    );
    assert_eq!(tab.back_history(), [Location::file_system(&dir)]);
    assert_eq!(tab.instant().unwrap().total, Some(100));
    assert_eq!(tab.len(), 100);
    assert!(names(tab).iter().all(|name| name.starts_with("보고서 1")));

    // 뒤로 가면 검색 전 폴더이고, 앞으로 가면 같은 결과로 돌아온다(다시 질의).
    assert!(f.app.navigate(Navigation::Back));
    wait_read(&mut f.app);
    assert_eq!(f.app.active_tab().location(), &Location::file_system(&dir));
    assert!(f.app.active_tab().instant().is_none());
    assert!(f.app.navigate(Navigation::Forward));
    wait_read(&mut f.app);
    wait_settled(&mut f.app);
    let tab = f.app.active_tab();
    assert_eq!(
        tab.location(),
        &Location::instant_search_results(&dir, "\"보고서 1\"", false)
    );
    assert_eq!(tab.instant().unwrap().total, Some(100));
    assert_eq!(tab.len(), 100);
}

#[test]
fn 첫_질의는_정한_수만_받고_이어_받기로_나머지가_붙는다() {
    let mut f = Fixture::new();
    type_first(&mut f.app, "보고서");
    let id = f.app.active_tab_id();
    let total = (REPORTS + OUTSIDE) as usize;

    assert_eq!(f.app.active_tab().len(), 100);
    assert!(f.app.active_tab().has_more_results());
    assert!(f.app.active_tab().instant().unwrap().has_more);
    let mut loaded = 100;
    while loaded < total {
        assert!(f.app.load_more_results(id), "남은 결과를 이어 받는다");
        // 응답을 기다리는 동안 또 청하지 않는다.
        assert!(!f.app.load_more_results(id));
        wait_settled(&mut f.app);
        loaded = (loaded + PAGE_SIZE as usize).min(total);
        assert_eq!(f.app.active_tab().len(), loaded);
    }
    assert!(!f.app.active_tab().has_more_results());
    assert!(!f.app.load_more_results(id));
    // 이어 받은 결과가 앞 결과와 겹치지 않는다.
    let all = names(f.app.active_tab());
    let mut unique = all.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), total);
    assert_eq!(
        f.app.active_tab().instant().unwrap().total,
        Some(total as u64)
    );
}

#[test]
fn 정렬_열을_바꾸면_그_정렬로_다시_받고_모르는_정렬은_받아_둔_것만_정렬한다() {
    let mut f = Fixture::new();
    type_first(&mut f.app, "보고서");
    let by = |column, direction| Sort { column, direction };

    // 크기 내림차순: 서비스가 그 정렬로 다시 받는다(가장 큰 파일이 맨 위, 100개 밖의 것도 온다).
    f.app.set_sort(by(Column::Size, Direction::Descending));
    assert!(
        f.app.active_tab().instant().unwrap().loading,
        "다시 질의했다"
    );
    wait_settled(&mut f.app);
    let tab = f.app.active_tab();
    assert_eq!(tab.len(), 100);
    assert_eq!(
        tab.get(0).unwrap().name.to_string_lossy(),
        format!("보고서 {REPORTS:03}.txt")
    );
    assert_eq!(tab.get(0).unwrap().size, REPORTS * 100);

    // 위치(경로) 열도 서비스가 안다.
    f.app.set_sort(by(Column::Location, Direction::Ascending));
    assert!(f.app.active_tab().instant().unwrap().loading);
    wait_settled(&mut f.app);

    // 유형 열은 서비스가 모른다: 다시 질의하지 않고 받아 둔 결과만 정렬한다.
    let before = f.app.active_tab().len();
    f.app.set_sort(by(Column::Type, Direction::Ascending));
    let tab = f.app.active_tab();
    assert!(!tab.instant().unwrap().loading, "다시 질의하지 않는다");
    assert_eq!(tab.len(), before);
    assert_eq!(tab.sort().column, Column::Type);
    assert_eq!(tab.instant().unwrap().total, Some(REPORTS + OUTSIDE));
}

#[test]
fn 빠르게_여러_글자를_넘기면_마지막_글자의_결과만_남는다() {
    let mut f = Fixture::new();
    let dir = f.path().to_path_buf();
    for text in [
        "보",
        "보고",
        "보고서",
        "보고서 ",
        "보고서 2",
        "\"보고서 24\"",
    ] {
        f.app.set_instant_query(text);
    }
    wait_read(&mut f.app);
    wait_settled(&mut f.app);
    // 늦게 온 앞선 응답이 있어도 마지막 글자의 결과가 남는다.
    let end = Instant::now() + Duration::from_millis(300);
    while Instant::now() < end {
        f.app.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    let tab = f.app.active_tab();
    assert_eq!(
        tab.location(),
        &Location::instant_search_results(&dir, "\"보고서 24\"", false)
    );
    assert_eq!(tab.back_history(), [Location::file_system(&dir)]);
    assert_eq!(tab.instant().unwrap().total, Some(10));
    assert_eq!(tab.len(), 10);
    assert!(names(tab).iter().all(|name| name.starts_with("보고서 24")));

    // 결과를 받은 뒤의 빠른 입력도 같다.
    for text in ["\"보고서 1\"", "\"보고서 12\"", "\"보고서 120\""] {
        f.app.set_instant_query(text);
    }
    wait_settled(&mut f.app);
    assert_eq!(f.app.active_tab().len(), 1);
    assert_eq!(names(f.app.active_tab()), ["보고서 120.txt"]);
}

#[test]
fn 틀린_문법이면_앞_결과가_그대로이고_까닭과_위치가_온다() {
    let mut f = Fixture::new();
    let dir = f.path().to_path_buf();
    type_first(&mut f.app, "\"보고서 1\"");
    let before = names(f.app.active_tab());
    assert_eq!(before.len(), 100);

    f.app.set_instant_query("보고서 <1");
    let tab = f.app.active_tab();
    let error = tab.instant_error().expect("틀린 글자의 까닭");
    assert!(!error.range.is_empty());
    assert_eq!(names(tab), before, "앞 결과를 지우지 않는다");
    assert_eq!(
        tab.location(),
        &Location::instant_search_results(&dir, "\"보고서 1\"", false)
    );
    assert_eq!(tab.back_history().len(), 1);

    // 맞는 글자를 넘기면 까닭이 지워진다.
    type_more(&mut f.app, "\"보고서 12\"");
    assert!(f.app.active_tab().instant_error().is_none());
    assert_eq!(f.app.active_tab().len(), 10);

    // 첫 글자부터 틀리면 위치도 바뀌지 않는다.
    f.app.navigate(Navigation::Back);
    wait_read(&mut f.app);
    f.app.set_instant_query("<");
    assert!(f.app.active_tab().instant_error().is_some());
    assert_eq!(f.app.active_tab().location(), &Location::file_system(&dir));
}

#[test]
fn 빈_글자나_검색_끝내기면_검색_전_위치로_돌아간다() {
    let mut f = Fixture::new();
    let dir = f.path().to_path_buf();
    type_first(&mut f.app, "보고서");
    type_more(&mut f.app, "\"보고서 1\"");

    // 빈 글자(공백뿐 포함).
    f.app.set_instant_query("   ");
    wait_read(&mut f.app);
    assert_eq!(f.app.active_tab().location(), &Location::file_system(&dir));
    assert!(f.app.active_tab().instant().is_none());

    // 검색 끝내기: 앞으로 가면 같은 검색이 돌아온다.
    type_first(&mut f.app, "보고서");
    f.app.set_instant_query("<보고서");
    assert!(f.app.active_tab().instant_error().is_some());
    f.app.end_instant_search();
    assert!(f.app.active_tab().instant_error().is_none());
    wait_read(&mut f.app);
    assert_eq!(f.app.active_tab().location(), &Location::file_system(&dir));
    assert!(f.app.navigate(Navigation::Forward));
    wait_read(&mut f.app);
    assert!(f.app.active_tab().is_instant_search());

    // 즉시 검색 중이 아니면 아무 일도 없다.
    f.app.navigate(Navigation::Back);
    wait_read(&mut f.app);
    f.app.end_instant_search();
    assert_eq!(f.app.active_tab().location(), &Location::file_system(&dir));

    // 결과 위치로 가는 이동이 끝나기 전에 끝내면 그 이동을 버린다.
    f.app.set_instant_query("보고서");
    f.app.end_instant_search();
    let end = Instant::now() + Duration::from_millis(300);
    while Instant::now() < end {
        f.app.poll();
        let _ = f.app.wait_update(Duration::from_millis(10));
    }
    assert_eq!(f.app.active_tab().location(), &Location::file_system(&dir));
}

#[test]
fn 현재_폴더_아래만은_결과를_좁히고_탭에_남으며_팔레트에_나온다() {
    let mut f = Fixture::new();
    let dir = f.path().to_path_buf();
    let command = CommandId::InstantUnderFolder;
    assert!(f.app.can_execute(command));
    let found = f.app.search_commands("instant-under", |c| c.as_str());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].command, command);
    assert!(found[0].enabled);
    assert!(!command.is_checked(f.app.active_tab()));

    // 모든 볼륨(기본): 시험 폴더 밖의 것도 나온다.
    type_first(&mut f.app, "보고서");
    assert_eq!(
        f.app.active_tab().instant().unwrap().total,
        Some(REPORTS + OUTSIDE)
    );

    // 결과 위치에서 켜면 같은 질의가 그 폴더 아래로 좁혀지고 기록은 늘지 않는다.
    assert_eq!(f.app.execute(command), Execution::Done);
    assert!(command.is_checked(f.app.active_tab()));
    wait_settled(&mut f.app);
    let tab = f.app.active_tab();
    assert_eq!(
        tab.location(),
        &Location::instant_search_results(&dir, "보고서", true)
    );
    assert_eq!(tab.instant().unwrap().total, Some(REPORTS));
    assert_eq!(tab.back_history().len(), 1);

    // 범위는 탭에 남는다: 검색을 끝내고 다시 시작해도 폴더 아래만이다.
    f.app.end_instant_search();
    wait_read(&mut f.app);
    assert!(f.app.active_tab().instant_under());
    type_first(&mut f.app, "\"보고서 외부\"");
    assert_eq!(f.app.active_tab().instant().unwrap().total, Some(0));
    assert_eq!(f.app.active_tab().len(), 0);

    // 끄면 모든 볼륨으로 돌아온다.
    assert_eq!(f.app.execute(command), Execution::Done);
    wait_settled(&mut f.app);
    assert!(!command.is_checked(f.app.active_tab()));
    assert_eq!(f.app.active_tab().instant().unwrap().total, Some(OUTSIDE));
    assert_eq!(f.app.active_tab().len(), OUTSIDE as usize);
}

#[test]
fn 검색을_시작한_위치가_실제_폴더가_아니면_현재_폴더_아래만이_꺼지고_모든_볼륨으로_찾는다() {
    let mut f = Fixture::new();
    let dir = f.path().to_path_buf();
    fs_touch(&dir);
    assert_eq!(
        f.app.execute(CommandId::InstantUnderFolder),
        Execution::Done
    );
    assert!(f.app.active_tab().instant_under());

    // 이름 검색 결과 위치는 실제 폴더가 아니다.
    f.app.start_search("보고서").unwrap();
    wait_read(&mut f.app);
    assert!(f.app.active_tab().location().file_system_path().is_none());
    assert!(!f.app.can_execute(CommandId::InstantUnderFolder));
    assert_eq!(
        f.app.execute(CommandId::InstantUnderFolder),
        Execution::Disabled
    );
    let found = f.app.search_commands("instant-under", |c| c.as_str());
    assert!(!found[0].enabled, "팔레트에는 나오되 꺼져 있다");

    // 탭에 범위가 남아 있어도 그 위치에서 시작하면 모든 볼륨이다.
    f.app.set_instant_query("보고서");
    wait_read(&mut f.app);
    wait_settled(&mut f.app);
    let tab = f.app.active_tab();
    assert_eq!(
        tab.location(),
        &Location::instant_search_results(&dir, "보고서", false)
    );
    assert_eq!(tab.instant().unwrap().total, Some(REPORTS + OUTSIDE));
    // 즉시 검색 결과 위치는 검색 전 폴더가 있어 전환이 켜진다.
    assert!(f.app.can_execute(CommandId::InstantUnderFolder));
}

/// 시험 폴더가 사라지지 않게 쥐고 있을 뿐인 자리(임시 폴더는 `Fixture`가 쥔다).
fn fs_touch(dir: &Path) {
    assert!(dir.is_dir());
}

#[test]
fn 만드는_중인_볼륨이_있으면_덜_찼을_수_있음이_드러난다() {
    let mut f = Fixture::new();
    type_first(&mut f.app, "보고서");
    assert!(!f.app.active_tab().instant().unwrap().may_be_incomplete());

    assert!(f.backend.0.set_building(&f.volume));
    type_more(&mut f.app, "보고서 2");
    let status = f.app.active_tab().instant().unwrap();
    assert!(status.may_be_incomplete());
    assert_eq!(status.building, [f.volume.clone()]);
    // 앞선 인덱스로 답하므로 결과는 있다.
    assert!(!f.app.active_tab().is_empty());
}

#[test]
fn 서비스에_닿지_못하면_서비스_없음이_드러나고_닿으면_질의를_다시_보낸다() {
    let dir = TempDir::new().unwrap();
    let mut app = App::new(Some(Location::file_system(dir.path())));
    app.wait(WAIT).expect("첫 읽기");

    // 연결을 띄우지 않았다.
    app.set_instant_query("보고서");
    wait_read(&mut app);
    let status = app.active_tab().instant().unwrap();
    assert!(matches!(
        status.unavailable,
        Some(Unavailable::Connection(_))
    ));
    assert_eq!(app.active_tab().len(), 0);
    app.end_instant_search();
    wait_read(&mut app);

    // 서비스가 없는 파이프에 붙는다.
    let pipe = unique_pipe();
    app.enable_indexer(options(&pipe));
    app.set_instant_query("보고서");
    wait_read(&mut app);
    wait_settled(&mut app);
    assert!(matches!(
        app.active_tab().instant().unwrap().unavailable,
        Some(Unavailable::Connection(_))
    ));

    // 서비스가 뜨면 연결이 맺어지고 멈춰 있던 질의를 다시 보낸다.
    let (volume, index) = index_for(dir.path());
    let backend = Arc::new(SampleBackend(Volumes::with_index(&volume, index)));
    let _server = PipeServer::start(&pipe, backend).unwrap();
    wait_until(
        &mut app,
        "서비스가 뜬 뒤 결과가 오지 않았다",
        |tab| {
            tab.instant()
                .is_some_and(|s| s.unavailable.is_none() && s.total.is_some() && !s.loading)
        },
    );
    assert_eq!(
        app.active_tab().instant().unwrap().total,
        Some(REPORTS + OUTSIDE)
    );
}

#[test]
fn 전체_개수가_상한이면_구분하고_이어_받기가_무효면_다시_질의한다() {
    let pipe = unique_pipe();
    let listener = Arc::new(Listener::bind(&pipe).unwrap());
    let dir = TempDir::new().unwrap();
    let server = {
        let listener = Arc::clone(&listener);
        std::thread::spawn(move || {
            let client = listener
                .accept(Duration::from_secs(10))
                .unwrap()
                .expect("클라이언트가 붙지 않았다");
            let (mut reader, mut writer, _closer) = client.split().unwrap();
            assert_eq!(
                read_message::<Request>(&mut reader).unwrap(),
                Request::Hello {
                    version: PROTOCOL_VERSION
                }
            );
            write_message(
                &mut writer,
                &Response::HelloAccepted {
                    version: PROTOCOL_VERSION,
                },
            )
            .unwrap();
            let hit = |name: &str| facet_protocol::HitMessage {
                path: format!(r"T:\{name}"),
                name: name.into(),
                size: 1,
                modified: 1,
                attributes: 0x20,
            };
            // 첫 질의: 권한 거르기 전 상한 7개, 이어 받을 것이 있다.
            let Request::Query(_) = read_message::<Request>(&mut reader).unwrap() else {
                panic!("질의가 아니다");
            };
            write_message(
                &mut writer,
                &Response::Page(PageMessage {
                    hits: vec![hit("a.txt")],
                    total: 7,
                    total_is_upper_bound: true,
                    next: Some(42),
                    building: Vec::new(),
                }),
            )
            .unwrap();
            // 이어 받기는 무효(인덱스가 바뀜): 클라이언트가 처음부터 다시 질의한다.
            let Request::Resume(_) = read_message::<Request>(&mut reader).unwrap() else {
                panic!("이어 받기가 아니다");
            };
            write_message(
                &mut writer,
                &Response::Error(ErrorMessage {
                    kind: ErrorKind::StaleCursor,
                    message: "낡음".into(),
                }),
            )
            .unwrap();
            let Request::Query(_) = read_message::<Request>(&mut reader).unwrap() else {
                panic!("다시 보낸 질의가 아니다");
            };
            write_message(
                &mut writer,
                &Response::Page(PageMessage {
                    hits: vec![hit("b.txt"), hit("c.txt")],
                    total: 2,
                    total_is_upper_bound: false,
                    next: None,
                    building: Vec::new(),
                }),
            )
            .unwrap();
        })
    };
    let mut app = App::new(Some(Location::file_system(dir.path())));
    app.wait(WAIT).expect("첫 읽기");
    app.enable_indexer(options(&pipe));
    type_first(&mut app, "a");
    let status = app.active_tab().instant().unwrap();
    assert_eq!(status.total, Some(7));
    assert!(status.total_is_upper_bound, "상한임을 구분한다");
    assert_eq!(app.active_tab().len(), 1);

    let id = app.active_tab_id();
    assert!(app.load_more_results(id));
    wait_until(
        &mut app,
        "다시 질의한 결과가 오지 않았다",
        |tab| {
            tab.instant()
                .is_some_and(|s| !s.loading && s.total == Some(2))
        },
    );
    let tab = app.active_tab();
    assert!(!tab.instant().unwrap().total_is_upper_bound);
    assert_eq!(names(tab), ["b.txt", "c.txt"]);
    server.join().unwrap();
}

#[test]
fn 결과_항목에서_기존_검색_결과_위치의_명령_켜짐과_폴더에서_보기가_그대로다() {
    let mut f = Fixture::new();
    let dir = f.path().to_path_buf();
    type_first(&mut f.app, "\"보고서 001\"");
    assert_eq!(names(f.app.active_tab()), ["보고서 001.txt"]);
    f.app.active_tab_mut().select_only(0);

    // 결과 위치의 명령 켜짐(실시간 탐색 결과와 같다).
    for command in [
        CommandId::Copy,
        CommandId::Cut,
        CommandId::CopyPath,
        CommandId::Delete,
        CommandId::ShowInFolder,
        CommandId::SortByLocation,
        CommandId::CopyTo,
    ] {
        assert!(f.app.can_execute(command), "{command:?}");
    }
    for command in [
        CommandId::NewFolder,
        CommandId::Paste,
        CommandId::SortByMatches,
    ] {
        assert!(!f.app.can_execute(command), "{command:?}");
    }
    assert!(f.app.menu_context(MenuKind::Item).location_is_results);
    assert_eq!(
        f.app.active_tab().selected_paths(),
        [dir.join("보고서 001.txt")]
    );

    // 「폴더에서 보기」: 항목의 상위 폴더를 같은 탭에서 연다(기록에 남음).
    assert_eq!(f.app.execute(CommandId::ShowInFolder), Execution::Done);
    wait_read(&mut f.app);
    let tab = f.app.active_tab();
    assert_eq!(tab.location(), &Location::file_system(&dir));
    assert_eq!(tab.back_history().len(), 2);
    assert!(matches!(
        tab.back_history()[1].search_scope().map(|s| &s.kind),
        Some(SearchKind::Instant { under: false })
    ));
    // 세션에는 검색 전 폴더로 남는다.
    assert_eq!(
        Location::instant_search_results(&dir, "보고서", false).for_session(),
        Location::file_system(PathBuf::from(&dir))
    );
}
