// target: crates/facet-shell-helper/tests/spec_sync_folders_helper.rs
// AC: #454: 동기화 폴더에서 보이는 행을 요청하면 Update::SyncStates가 오고 syncing…은 동기화 중, error…는 오류, 나머지는 속성 배지 그대로
// AC: #454: 동기화 폴더가 아니면 Update::SyncStates가 오지 않고 배지 없음, 도우미를 켜지 않아도 막히지 않고 속성 배지 그대로, 요청은 50ms 안에 돌아옴
//! 동기화 중·오류 배지(`App::request_sync_states` → 셸 확장 도우미 → `Update::SyncStates`, `Tab::sync_state`)를
//! 시험용 모드(`--fake`: 이름이 `syncing`으로 시작하면 동기화 중, `error`로 시작하면 오류로 답함)의 실제 도우미
//! 프로세스와 시험용 동기화 공급자(`facet_win::cloud_files::testing`)의 폴더로 확인한다. 실제 프로세스를 쓰므로
//! Windows에서만 돈다. 동기화 루트를 등록할 수 없는 환경이면 건너뛰었다고 출력하고, 환경 변수
//! `FACET_REQUIRE_SYNC_ROOT`가 있으면 그 경우를 실패로 본다.
#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::shell_helper::HelperOptions;
use facet_core::sync_status::SyncState;
use facet_core::tab::ReadOutcome;
use facet_core::window::{App, Update};
use facet_win::cloud_files::testing::TestSyncRoot;
use tempfile::TempDir;

/// CI 러너가 느려도 흔들리지 않게 넉넉히 기다린다. 결과가 오면 곧바로 끝난다.
const WAIT: Duration = Duration::from_secs(20);
/// 「오지 않음」을 볼 때 기다리는 시간.
const QUIET: Duration = Duration::from_secs(1);
/// 요청이 돌아와야 하는 시간.
const QUICK: Duration = Duration::from_millis(50);
const ENV_REQUIRE: &str = "FACET_REQUIRE_SYNC_ROOT";

fn sync_root(dir: &Path, test: &str) -> Option<TestSyncRoot> {
    match TestSyncRoot::register(dir) {
        Ok(root) => Some(root),
        Err(error) => {
            assert!(
                std::env::var_os(ENV_REQUIRE).is_none(),
                "{ENV_REQUIRE}가 있는데 동기화 루트를 등록하지 못했다: {error}"
            );
            println!("{test}: 동기화 루트를 등록할 수 없어 건너뜀: {error}");
            None
        }
    }
}

fn fake_options() -> HelperOptions {
    let mut options = HelperOptions::new(PathBuf::from(env!("CARGO_BIN_EXE_facet-shell-helper")));
    options.args = vec!["--fake".into()];
    options.crash_log = None;
    options
}

/// 이 장치에 있는 파일 셋(동기화 중·오류·보통 이름)과 온라인 전용 파일 하나.
fn populate(root: &TestSyncRoot) {
    for name in ["syncing-a.txt", "error-b.txt", "보통.txt"] {
        fs::write(root.root().join(name), b"x").unwrap();
    }
    root.add_online_file("온라인.txt", b"online");
}

fn app_at(path: &Path, options: Option<HelperOptions>) -> App {
    let mut app = App::new(Some(Location::file_system(path)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    if let Some(options) = options {
        app.enable_shell_helper(options);
    }
    app
}

fn state_of(app: &App, name: &str) -> Option<SyncState> {
    let tab = app.active_tab();
    let index = tab
        .items()
        .position(|item| item.name == name)
        .unwrap_or_else(|| panic!("{name}이 목록에 없다"));
    tab.sync_state(index)
}

/// 활성 탭의 모든 행을 요청한다. 걸린 시간을 돌려준다.
fn request_all(app: &mut App) -> Duration {
    let len = app.active_tab().len();
    let tab = app.active_tab_id();
    let started = Instant::now();
    app.request_sync_states(tab, 0..len);
    started.elapsed()
}

/// `during` 동안 받은 `Update::SyncStates` 수.
fn sync_updates_for(app: &mut App, during: Duration) -> usize {
    let until = Instant::now() + during;
    let mut count = 0;
    while Instant::now() < until {
        count += app
            .poll()
            .into_iter()
            .filter(|(_, update)| matches!(update, Update::SyncStates))
            .count();
        std::thread::sleep(Duration::from_millis(10));
    }
    count
}

#[test]
#[ignore = "#454"]
fn helper_reported_syncing_and_error_reach_badges() {
    let dir = TempDir::new().unwrap();
    let Some(root) = sync_root(dir.path(), "동기화 중·오류 배지") else {
        return;
    };
    populate(&root);
    let mut app = app_at(dir.path(), Some(fake_options()));
    assert_eq!(state_of(&app, "syncing-a.txt"), Some(SyncState::OnDevice));
    let took = request_all(&mut app);
    assert!(took < QUICK, "요청은 곧바로 돌아온다: {took:?}");

    let deadline = Instant::now() + WAIT;
    let tab = app.active_tab_id();
    let mut arrived = false;
    loop {
        arrived |= app
            .poll()
            .into_iter()
            .any(|(id, update)| id == tab && matches!(update, Update::SyncStates));
        if arrived && state_of(&app, "error-b.txt") == Some(SyncState::Error) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "셸 동기화 상태가 오지 않았다: {:?}",
            state_of(&app, "error-b.txt")
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(state_of(&app, "syncing-a.txt"), Some(SyncState::Syncing));
    assert_eq!(state_of(&app, "error-b.txt"), Some(SyncState::Error));
    assert_eq!(state_of(&app, "보통.txt"), Some(SyncState::OnDevice));
    assert_eq!(state_of(&app, "온라인.txt"), Some(SyncState::OnlineOnly));
    assert_eq!(root.fetch_data_count(), 0);
}

#[test]
#[ignore = "#454"]
fn no_request_outside_sync_folders() {
    let dir = TempDir::new().unwrap();
    for name in ["syncing-a.txt", "error-b.txt"] {
        fs::write(dir.path().join(name), b"x").unwrap();
    }
    let mut app = app_at(dir.path(), Some(fake_options()));
    let took = request_all(&mut app);
    assert!(took < QUICK, "요청은 곧바로 돌아온다: {took:?}");
    assert_eq!(sync_updates_for(&mut app, QUIET), 0);
    assert_eq!(state_of(&app, "syncing-a.txt"), None);
    assert_eq!(state_of(&app, "error-b.txt"), None);
}

#[test]
#[ignore = "#454"]
fn without_helper_attribute_badges_stay() {
    let dir = TempDir::new().unwrap();
    let Some(root) = sync_root(dir.path(), "도우미 없음") else {
        return;
    };
    populate(&root);
    let mut app = app_at(dir.path(), None);
    let took = request_all(&mut app);
    assert!(took < QUICK, "요청은 곧바로 돌아온다: {took:?}");
    sync_updates_for(&mut app, QUIET);
    assert_eq!(state_of(&app, "syncing-a.txt"), Some(SyncState::OnDevice));
    assert_eq!(state_of(&app, "error-b.txt"), Some(SyncState::OnDevice));
    assert_eq!(state_of(&app, "온라인.txt"), Some(SyncState::OnlineOnly));
}
