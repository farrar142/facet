// target: crates/facet-core/tests/spec_virtual_folders_this_pc.rs
// AC: #350: 내 PC 열기·주소창 이름, 드라이브 열기와 뒤로·앞으로, 드라이브 루트에서 위로, 경로 조각, 세션, 다 읽으면 읽는 중 아님, 열기만 되는 일
//! 셸 공급자의 「내 PC」 위치를 실제 셸로 공개 인터페이스(`App`·`Tab`·공급자)에서 확인한다: 명령과 주소창 글자로 가고
//! 시스템 드라이브가 실제 경로를 가진 폴더 항목으로 들어오는지, 드라이브를 열면 파일 시스템 공급자의 드라이브 루트로
//! 가고 뒤로·앞으로가 되는지, 드라이브 루트에서 위로 가면 내 PC인지, 경로 조각·세션·되는 일. Windows 전용.
#![cfg(windows)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use facet_core::command::CommandId;
use facet_core::session::Session;
use facet_core::tab::{Navigation, ReadOutcome};
use facet_core::window::{App, Execution};
use facet_core::{Capabilities, Location, VirtualFolder, provider_for};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(30);

/// 시스템 드라이브의 루트(`C:\` 등).
fn system_root() -> PathBuf {
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_owned());
    PathBuf::from(format!("{drive}\\"))
}

fn app_at(location: Location) -> App {
    let mut app = App::new(Some(location));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

/// 활성 탭이 `expected`에 닿아 목록을 다 읽을 때까지 결과를 받는다.
fn settle(app: &mut App, expected: &Location) {
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll();
        let tab = app.active_tab();
        if tab.pending_location().is_none() {
            if let Some(error) = tab.error() {
                panic!("읽기가 실패했다: {error}");
            }
            if tab.location() == expected && tab.loading().is_none() {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "{expected}에 닿지 못했다: 지금 {}, 읽는 중 {:?}",
            tab.location(),
            tab.loading()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// 활성 탭에서 시스템 드라이브 항목의 번호.
fn system_drive_index(app: &App) -> usize {
    let tab = app.active_tab();
    let root = system_root();
    (0..tab.len())
        .find(|&i| tab.item_path(i).as_deref() == Some(root.as_path()))
        .unwrap_or_else(|| {
            let names: Vec<_> = tab.items().map(|item| item.name.clone()).collect();
            panic!("내 PC에 {} 항목이 없다: {names:?}", root.display())
        })
}

fn open_this_pc(dir: &TempDir) -> App {
    let mut app = app_at(Location::file_system(dir.path()));
    assert_eq!(app.execute(CommandId::OpenThisPc), Execution::Done);
    settle(&mut app, &Location::this_pc());
    app
}

#[test]
fn this_pc_location_is_made_without_the_shell() {
    let this_pc = Location::this_pc();
    assert_eq!(this_pc, Location::this_pc());
    assert_eq!(this_pc.virtual_folder(), Some(VirtualFolder::ThisPc));
    assert_eq!(this_pc.file_system_path(), None);
    assert!(!this_pc.is_search_results());
    assert_eq!(Location::file_system(system_root()).virtual_folder(), None);
}

#[test]
fn the_command_opens_this_pc_with_the_system_drive() {
    let dir = TempDir::new().unwrap();
    let app = open_this_pc(&dir);
    let tab = app.active_tab();
    assert_eq!(tab.location().virtual_folder(), Some(VirtualFolder::ThisPc));

    let index = system_drive_index(&app);
    let drive = tab.get(index).unwrap();
    assert!(drive.is_folder, "드라이브는 폴더 항목이다: {drive:?}");
    assert!(!drive.name.is_empty());
    assert_eq!(tab.back_history(), [Location::file_system(dir.path())]);
}

#[test]
fn address_names_go_to_this_pc() {
    let dir = TempDir::new().unwrap();
    for text in ["내 PC", "shell:MyComputerFolder"] {
        let mut app = app_at(Location::file_system(dir.path()));
        assert!(app.go_to_text(text), "{text}");
        settle(&mut app, &Location::this_pc());
        system_drive_index(&app);
    }
}

#[test]
fn opening_a_drive_goes_to_the_file_system_root() {
    let dir = TempDir::new().unwrap();
    let mut app = open_this_pc(&dir);
    let index = system_drive_index(&app);
    app.active_tab_mut().select_only(index);

    assert_eq!(app.execute(CommandId::Open), Execution::Done);
    let root = Location::file_system(system_root());
    settle(&mut app, &root);
    assert_eq!(app.active_tab().location().virtual_folder(), None);
    assert!(
        !app.active_tab().is_empty(),
        "드라이브 루트를 파일 시스템으로 읽었다"
    );

    assert!(app.navigate(Navigation::Back));
    settle(&mut app, &Location::this_pc());
    assert!(app.navigate(Navigation::Forward));
    settle(&mut app, &root);
}

#[test]
fn up_from_a_drive_root_is_this_pc() {
    let root = Location::file_system(system_root());
    let mut app = app_at(root.clone());
    assert!(
        app.active_tab().can_go_up(),
        "드라이브 루트에서 위로가 켜진다"
    );
    assert!(CommandId::Up.is_enabled(app.active_tab()));

    assert_eq!(app.execute(CommandId::Up), Execution::Done);
    settle(&mut app, &Location::this_pc());
    assert_eq!(app.active_tab().back_history(), [root]);

    assert!(!app.active_tab().can_go_up(), "내 PC에는 상위가 없다");
    assert!(!CommandId::Up.is_enabled(app.active_tab()));
}

#[test]
fn this_pc_has_one_path_segment() {
    let this_pc = Location::this_pc();
    let segments = provider_for(&this_pc).segments(&this_pc);
    assert_eq!(segments.len(), 1, "{segments:?}");
    assert_eq!(segments[0].location, this_pc);
    assert!(!segments[0].label.trim().is_empty());
}

#[test]
fn this_pc_can_only_open() {
    let dir = TempDir::new().unwrap();
    let app = open_this_pc(&dir);
    let expected = Capabilities {
        open: true,
        ..Capabilities::default()
    };
    assert_eq!(app.active_tab().capabilities(), expected);
    let this_pc = Location::this_pc();
    assert_eq!(provider_for(&this_pc).capabilities(&this_pc), expected);
}

#[test]
fn a_this_pc_tab_comes_back_from_the_session() {
    let dir = TempDir::new().unwrap();
    let app = open_this_pc(&dir);
    let json = app.session().expect("창이 있다").to_json();
    drop(app);

    let session = Session::from_json(&json).expect("세션을 읽는다");
    let mut restored = App::restore(Some(session), Vec::new());
    settle(&mut restored, &Location::this_pc());
    system_drive_index(&restored);
}
