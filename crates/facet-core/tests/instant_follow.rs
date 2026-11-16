//! 즉시 검색 결과가 **인덱스 변경을 따라간다**: 같은 프로세스의 인덱서 파이프 서버(합성 인덱스)에서 결과가 보이는 동안
//! 인덱스에 항목을 더하거나 빼면 같은 질의를 조용히 다시 받아 결과가 바뀌고(이어 받은 만큼 다시 받음), 남은 항목의 선택이
//! 유지되며, 숨은 탭은 다시 질의하지 않는다. 결과에서 연 항목이 디스크에 없으면(합성 인덱스의 항목은 디스크에 없다) 까닭이
//! 오고 결과에서 빠진다. 관리자 권한이 필요 없고 Windows 전용이다(파이프).

#![cfg(windows)]

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::command::CommandId;
use facet_core::indexer_client::{CONNECT_TIMEOUT, ClientOptions};
use facet_core::instant_search::{FOLLOW_INTERVAL, PAGE_SIZE};
use facet_core::tab::{ReadOutcome, Tab};
use facet_core::window::{App, Execution, TabId};
use facet_index::{ATTRIBUTE_DIRECTORY, Index, NewItem};
use facet_indexer::pipe::PipeServer;
use facet_indexer::sample::SampleBackend;
use facet_indexer::volumes::Volumes;
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(30);
const ROOT: u64 = 5;
/// 시험 폴더 아래 「보고서」 파일 수: 첫 화면 분량([`PAGE_SIZE`])을 넘어 이어 받기를 본다.
const REPORTS: u64 = 250;

fn unique_pipe() -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        r"\\.\pipe\facet-core-follow-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// `dir`와 같은 경로를 가진 합성 인덱스: `dir` 아래 `보고서 NNN.txt`(`keep`이 참인 번호만)와 `extra`의 이름들.
fn index_for(dir: &Path, keep: impl Fn(u64) -> bool, extra: &[&str]) -> (String, Index) {
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
    for n in (1..=REPORTS).filter(|&n| keep(n)) {
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
    for (i, name) in extra.iter().enumerate() {
        index
            .insert(NewItem {
                number: 50_000 + i as u64,
                parent,
                name,
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
        let (volume, index) = index_for(dir.path(), |_| true, &[]);
        let backend = Arc::new(SampleBackend(Volumes::with_index(&volume, index)));
        let pipe = unique_pipe();
        let server = PipeServer::start(&pipe, backend.clone()).unwrap();
        let mut app = App::new(Some(Location::file_system(dir.path())));
        let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
        assert_eq!(outcome, ReadOutcome::Committed);
        app.enable_indexer(ClientOptions {
            pipe,
            connect_timeout: CONNECT_TIMEOUT,
            reconnect_interval: Duration::from_millis(100),
        });
        Self {
            dir,
            volume,
            backend,
            _server: server,
            app,
        }
    }

    fn path(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }

    /// 인덱스에 시험 폴더 아래 항목 `name`을 더한다(USN 변경 적용과 같이 그 볼륨의 이어 받기 표지가 무효가 된다).
    fn add(&self, name: &str) {
        let dir = self.path();
        let folders = dir
            .components()
            .filter(|c| matches!(c, Component::Normal(_)))
            .count() as u64;
        let parent = 100 + folders - 1;
        self.backend
            .0
            .update(&self.volume, |index| {
                index
                    .insert(NewItem {
                        number: 60_000,
                        parent,
                        name,
                        size: 1,
                        modified: 133_000_000_000_000_000,
                        attributes: 0x20,
                    })
                    .unwrap();
            })
            .expect("준비된 볼륨");
    }

    /// 인덱스를 `keep`이 참인 보고서만 든 인덱스로 바꿔 끼운다.
    fn replace(&self, keep: impl Fn(u64) -> bool) {
        let (_, index) = index_for(&self.path(), keep, &[]);
        assert!(self.backend.0.set_ready(&self.volume, index));
    }
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

/// 탭 `id`가 `done`이 될 때까지 서비스의 응답을 받는다.
fn wait_tab(app: &mut App, id: TabId, what: &str, done: impl Fn(&Tab) -> bool) {
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll();
        if done(app.tab(id).expect("탭")) {
            return;
        }
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn settled(tab: &Tab) -> bool {
    tab.instant()
        .is_some_and(|s| !s.loading && (s.total.is_some() || s.unavailable.is_some()))
}

fn names(tab: &Tab) -> Vec<String> {
    tab.items()
        .map(|item| item.name.to_string_lossy().into_owned())
        .collect()
}

fn index_of(tab: &Tab, name: &str) -> usize {
    names(tab)
        .iter()
        .position(|n| n == name)
        .unwrap_or_else(|| panic!("결과에 없다: {name}"))
}

/// 글자를 넘기고 결과 위치로 간 뒤 첫 응답을 받는다.
fn type_first(app: &mut App, text: &str) -> TabId {
    app.set_instant_query(text);
    wait_read(app);
    let id = app.active_tab_id();
    wait_tab(app, id, "응답이 오지 않았다", settled);
    id
}

#[test]
fn 인덱스에_항목을_더하거나_빼면_결과가_바뀌고_이어_받은_만큼_다시_받으며_남은_선택이_유지된다() {
    let mut f = Fixture::new();
    let dir = f.path();
    let id = type_first(&mut f.app, "보고서");
    assert_eq!(f.app.active_tab().len(), PAGE_SIZE as usize);
    // 스크롤로 한 번 더 받는다.
    assert!(f.app.load_more_results(id));
    wait_tab(&mut f.app, id, "이어 받기가 오지 않았다", |tab| {
        tab.len() == 2 * PAGE_SIZE as usize && settled(tab)
    });
    let selected = dir.join("보고서 005.txt");
    let index = index_of(f.app.active_tab(), "보고서 005.txt");
    f.app.active_tab_mut().select_only(index);

    // 더하기: 이름 순서로 맨 앞에 오는 항목.
    f.add("보고서 000.txt");
    wait_tab(
        &mut f.app,
        id,
        "더한 항목이 결과에 나오지 않았다",
        |tab| names(tab).first().map(String::as_str) == Some("보고서 000.txt"),
    );
    let tab = f.app.active_tab();
    assert_eq!(tab.instant().unwrap().total, Some(REPORTS + 1));
    assert_eq!(
        tab.len(),
        2 * PAGE_SIZE as usize,
        "이어 받은 만큼 다시 받는다"
    );
    assert!(!tab.instant().unwrap().loading, "조용히 다시 받는다");
    assert_eq!(tab.selected_paths(), std::slice::from_ref(&selected));
    assert_eq!(tab.cursor_path(), Some(selected.clone()));
    assert_eq!(
        tab.back_history(),
        [Location::file_system(&dir)],
        "기록은 그대로"
    );

    // 빼기: 더한 항목과 3번이 사라진 인덱스로 바꿔 끼운다.
    f.replace(|n| n != 3);
    wait_tab(
        &mut f.app,
        id,
        "뺀 항목이 결과에서 빠지지 않았다",
        |tab| {
            !names(tab)
                .iter()
                .any(|n| n == "보고서 000.txt" || n == "보고서 003.txt")
        },
    );
    let tab = f.app.active_tab();
    assert_eq!(tab.instant().unwrap().total, Some(REPORTS - 1));
    assert_eq!(tab.len(), 2 * PAGE_SIZE as usize);
    assert_eq!(tab.selected_paths(), std::slice::from_ref(&selected));
    assert_eq!(tab.cursor_path(), Some(selected));
    // 다시 받은 뒤에도 이어 받을 수 있다.
    assert!(tab.has_more_results());

    // 선택한 항목이 사라지면 선택에서도 빠진다.
    f.replace(|n| n != 3 && n != 5);
    wait_tab(
        &mut f.app,
        id,
        "선택한 항목이 빠지지 않았다",
        |tab| !names(tab).iter().any(|n| n == "보고서 005.txt"),
    );
    assert!(f.app.active_tab().selected_paths().is_empty());
}

#[test]
fn 결과를_다_받은_탭도_인덱스_변경을_따라간다() {
    let mut f = Fixture::new();
    let id = type_first(&mut f.app, "\"보고서 01\"");
    assert_eq!(f.app.active_tab().len(), 10);
    assert!(!f.app.active_tab().has_more_results());
    f.add("보고서 01 새것.txt");
    wait_tab(
        &mut f.app,
        id,
        "더한 항목이 결과에 나오지 않았다",
        |tab| names(tab).iter().any(|n| n == "보고서 01 새것.txt"),
    );
    assert_eq!(f.app.active_tab().instant().unwrap().total, Some(11));
}

#[test]
fn 숨은_탭은_다시_질의하지_않고_다시_보이면_따라간다() {
    let mut f = Fixture::new();
    let dir = f.path();
    let id = type_first(&mut f.app, "보고서");
    // 새 탭을 열어 결과 탭을 숨긴다.
    f.app.open_tab(Location::file_system(&dir), true);
    wait_read(&mut f.app);
    assert_ne!(f.app.active_tab_id(), id);
    assert!(!f.app.tab(id).unwrap().is_shown());

    f.add("보고서 000.txt");
    let until = Instant::now() + FOLLOW_INTERVAL * 3;
    while Instant::now() < until {
        f.app.poll();
        std::thread::sleep(Duration::from_millis(20));
    }
    let tab = f.app.tab(id).unwrap();
    assert!(!names(tab).iter().any(|n| n == "보고서 000.txt"));
    assert_eq!(
        tab.instant().unwrap().total,
        Some(REPORTS),
        "다시 질의하지 않았다"
    );
    assert_eq!(tab.instant_in_flight(), None);

    // 다시 보이면 따라간다.
    assert!(f.app.activate_tab(id));
    wait_tab(
        &mut f.app,
        id,
        "다시 보인 탭이 따라가지 않았다",
        |tab| names(tab).first().map(String::as_str) == Some("보고서 000.txt"),
    );
}

#[test]
fn 디스크에_없는_결과를_열면_까닭이_오고_결과에서_빠진다() {
    let mut f = Fixture::new();
    let dir = f.path();
    type_first(&mut f.app, "\"보고서 00\"");
    let total = f.app.active_tab().len();
    // 합성 인덱스의 항목은 디스크에 없다.
    let index = index_of(f.app.active_tab(), "보고서 002.txt");
    f.app.active_tab_mut().select_only(index);
    assert_eq!(f.app.execute(CommandId::Open), Execution::ItemMissing);
    let tab = f.app.active_tab();
    let missing = dir.join("보고서 002.txt");
    assert_eq!(tab.missing_item(), Some(missing.as_path()));
    assert_eq!(tab.len(), total - 1);
    assert!(!names(tab).iter().any(|n| n == "보고서 002.txt"));
    assert!(tab.is_instant_search(), "위치는 그대로");

    let index = index_of(f.app.active_tab(), "보고서 003.txt");
    f.app.active_tab_mut().select_only(index);
    assert_eq!(
        f.app.execute(CommandId::ShowInFolder),
        Execution::ItemMissing
    );
    assert!(f.app.active_tab().is_instant_search());
    assert_eq!(f.app.active_tab().len(), total - 2);
}
