// target: crates/facet-core/tests/spec_install_interception_app.rs
// AC: #480: 설정 전 NotConfigured, portable은 켤 수 없고 레지스트리 그대로, 켜고 끄기, 밖의 변경을 확인하면 Update::Interception과 풀림·다시 켜고 끄면 바뀐 값
//! 앱의 폴더 열기 가로채기 상태(`App::set_interception`·`interception_state`·`enable_interception`·
//! `disable_interception`·`check_interception`)를 실제 레지스트리로 확인한다. 실제 폴더 열기 키는 건드리지 않고 HKCU의
//! `Software\facet-test\...` 아래 시험마다 새 루트 키에서 켜고 끄며, 끝나면 지운다. Windows 전용.

#![cfg(windows)]

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::build::InstallKind;
use facet_core::interception::{
    BACKUP_FILE_NAME, Change, Interception, InterceptionError, InterceptionStatus,
};
use facet_core::tab::ReadOutcome;
use facet_core::window::{App, InterceptionState, Update};
use facet_win::registry::{self, Hive, RegValue};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(10);
const KEY: &str = r"Folder\shell\open\command";

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// 시험마다 새로 만드는 HKCU 아래 루트 키. 버리면 지운다.
struct TestRoot(String);

impl TestRoot {
    fn new() -> TestRoot {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = format!(
            r"Software\facet-test\interception-app-{}-{n}",
            std::process::id()
        );
        registry::delete_tree(Hive::CurrentUser, &root).unwrap();
        TestRoot(root)
    }

    fn command(&self) -> Option<RegValue> {
        registry::read_value(Hive::CurrentUser, &format!(r"{}\{KEY}", self.0), "").unwrap()
    }

    fn set_command(&self, text: &str) {
        registry::write_value(
            Hive::CurrentUser,
            &format!(r"{}\{KEY}", self.0),
            "",
            &RegValue::string(text),
        )
        .unwrap();
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = registry::delete_tree(Hive::CurrentUser, &self.0);
    }
}

const ORIGINAL: &str = r"C:\Windows\explorer.exe";
const FACET: &str = r#""C:\facet\facet.exe" "%1""#;

fn interception(root: &TestRoot, backup: &Path) -> Interception {
    Interception::new(
        Hive::CurrentUser,
        root.0.clone(),
        vec![Change {
            key: KEY.to_owned(),
            name: String::new(),
            value: RegValue::string(FACET),
        }],
        backup,
    )
}

fn app_at(path: &Path) -> App {
    let mut app = App::new(Some(Location::file_system(path)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

/// 원래 값을 둔 루트와 그 루트의 가로채기를 설정한 앱.
fn setup(kind: InstallKind) -> (TempDir, TestRoot, App) {
    let dir = TempDir::new().unwrap();
    let root = TestRoot::new();
    root.set_command(ORIGINAL);
    let mut app = app_at(dir.path());
    app.set_interception(
        interception(&root, &dir.path().join(BACKUP_FILE_NAME)),
        kind,
    );
    (dir, root, app)
}

fn wait_interception(app: &mut App) {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "가로채기 확인 결과가 오지 않았다");
        if let Some((_, Update::Interception)) = app.wait_update(left) {
            return;
        }
    }
}

#[test]
#[ignore = "#480"]
fn 설정하기_전에는_설정_전_상태다() {
    let dir = TempDir::new().unwrap();
    let mut app = app_at(dir.path());
    assert_eq!(app.interception_state(), InterceptionState::NotConfigured);
    assert!(app.enable_interception().is_err());
}

#[test]
#[ignore = "#480"]
fn portable은_켤_수_없고_레지스트리가_그대로다() {
    let (_dir, root, mut app) = setup(InstallKind::Portable);
    assert_eq!(app.interception_state(), InterceptionState::Portable);
    assert_eq!(app.enable_interception(), Err(InterceptionError::Portable));
    assert_eq!(root.command(), Some(RegValue::string(ORIGINAL)));
    assert_eq!(app.interception_state(), InterceptionState::Portable);
}

#[test]
#[ignore = "#480"]
fn 켜고_끄면_상태와_값이_따라간다() {
    let (dir, root, mut app) = setup(InstallKind::Installed);
    assert_eq!(
        app.interception_state(),
        InterceptionState::Status(InterceptionStatus::Off)
    );

    app.enable_interception().expect("켜기");
    assert_eq!(
        app.interception_state(),
        InterceptionState::Status(InterceptionStatus::On)
    );
    assert_eq!(root.command(), Some(RegValue::string(FACET)));
    assert!(dir.path().join(BACKUP_FILE_NAME).is_file());

    app.disable_interception().expect("끄기");
    assert_eq!(
        app.interception_state(),
        InterceptionState::Status(InterceptionStatus::Off)
    );
    assert_eq!(root.command(), Some(RegValue::string(ORIGINAL)));
    assert!(!dir.path().join(BACKUP_FILE_NAME).exists());
}

#[test]
#[ignore = "#480"]
fn 밖에서_풀리면_확인이_풀림을_알리고_다시_켜면_새_원래_값을_남긴다() {
    let (_dir, root, mut app) = setup(InstallKind::Installed);
    app.enable_interception().unwrap();

    // Windows 업데이트가 값을 돌려놓은 경우.
    let reset = r"C:\Windows\explorer.exe /new";
    root.set_command(reset);
    app.check_interception();
    wait_interception(&mut app);
    assert_eq!(
        app.interception_state(),
        InterceptionState::Status(InterceptionStatus::Lost)
    );

    app.enable_interception().expect("다시 켜기");
    assert_eq!(
        app.interception_state(),
        InterceptionState::Status(InterceptionStatus::On)
    );
    app.disable_interception().unwrap();
    assert_eq!(root.command(), Some(RegValue::string(reset)));
}

#[test]
#[ignore = "#480"]
fn 확인은_기다리지_않고_결과로_온다() {
    let (_dir, _root, mut app) = setup(InstallKind::Installed);
    let started = Instant::now();
    app.check_interception();
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "확인은 작업 스레드에서 한다"
    );
    wait_interception(&mut app);
    assert_eq!(
        app.interception_state(),
        InterceptionState::Status(InterceptionStatus::Off)
    );
}
