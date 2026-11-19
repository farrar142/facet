// target: crates/facet-core/tests/spec_settings_tab.rs
// AC: #462: 「설정」 명령(Ctrl+,)이 설정 탭을 활성 탭 뒤에 열기, 같은 창의 설정 탭으로 가기·다른 창은 세지 않음, 주소창 facet:settings와 뒤로, 설정 위치의 목록·상위·경로 조각·꺼진 파일 작업, 세션 왕복, 「설정 파일 열기」 명령과 쉼표 키
//! 설정 탭(`Location::settings`, 명령 `OpenSettings`·`OpenSettingsFile`, `Key::Comma`)을 앱([`App`])과 임시 폴더로
//! 확인한다. 설정 화면 자체(분류·검색·값 바꾸기)는 사람이 확인한다. 「설정 파일 열기」는 기본 편집기를 띄우므로 실행하지
//! 않고 명령의 자리만 본다. 모든 환경에서 돈다.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use facet_core::command::{CommandId, Key, Shortcut, find_by_shortcut};
use facet_core::keymap::{self, Context};
use facet_core::session::Session;
use facet_core::tab::{Navigation, ReadOutcome};
use facet_core::window::{App, Execution, Panel};
use facet_core::{Location, provider_for};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(10);

fn folder() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.txt"), "a").unwrap();
    fs::create_dir(dir.path().join("하위")).unwrap();
    dir
}

fn app_at(path: &Path) -> App {
    let mut app = App::new(Some(Location::file_system(path)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

/// 활성 탭이 조건을 만족할 때까지 결과를 받는다.
fn wait_active(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + WAIT;
    while !done(app) {
        assert!(Instant::now() < deadline, "{what}");
        app.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn locations(panel: &Panel) -> Vec<Location> {
    panel
        .tabs()
        .map(|(_, tab)| tab.location().clone())
        .collect()
}

fn settings_tab_count(app: &App) -> usize {
    app.active_window()
        .panels()
        .iter()
        .map(|panel| locations(panel).iter().filter(|l| l.is_settings()).count())
        .sum()
}

#[test]
#[ignore = "#462"]
fn the_settings_location_is_not_a_real_folder() {
    let settings = Location::settings();
    assert!(settings.is_settings());
    assert_eq!(settings.file_system_path(), None);
    assert!(!Location::file_system("C:\\").is_settings());
    let provider = provider_for(&settings);
    let segments = provider.segments(&settings);
    assert_eq!(segments.len(), 1, "{segments:?}");
    assert_eq!(segments[0].location, settings);
}

#[test]
#[ignore = "#462"]
fn the_open_settings_command_has_ctrl_comma_and_is_global() {
    assert_eq!(Key::from_char(','), Some(Key::Comma));
    let ctrl_comma = Shortcut::new(Key::Comma).ctrl();
    assert_eq!(CommandId::OpenSettings.shortcuts(), &[ctrl_comma]);
    assert_eq!(find_by_shortcut(ctrl_comma), Some(CommandId::OpenSettings));
    assert_eq!(keymap::context_of(CommandId::OpenSettings), Context::Global);
    assert_eq!(CommandId::OpenSettings.as_str(), "open-settings");
    assert_eq!(
        CommandId::from_id("open-settings"),
        Some(CommandId::OpenSettings)
    );
    assert!(CommandId::ALL.contains(&CommandId::OpenSettings));

    assert_eq!(CommandId::OpenSettingsFile.as_str(), "open-settings-file");
    assert_eq!(
        CommandId::from_id("open-settings-file"),
        Some(CommandId::OpenSettingsFile)
    );
    assert!(CommandId::ALL.contains(&CommandId::OpenSettingsFile));
    assert!(CommandId::OpenSettingsFile.shortcuts().is_empty());
}

#[test]
#[ignore = "#462"]
fn both_commands_are_always_enabled_and_in_the_palette() {
    let dir = folder();
    let app = app_at(dir.path());
    assert!(app.can_execute(CommandId::OpenSettings));
    assert!(app.can_execute(CommandId::OpenSettingsFile));
    let found = app.search_commands("", |c| c.as_str().to_owned());
    for command in [CommandId::OpenSettings, CommandId::OpenSettingsFile] {
        let entry = found
            .iter()
            .find(|e| e.command == command)
            .unwrap_or_else(|| panic!("{command:?}가 팔레트에 없다"));
        assert!(entry.enabled);
    }
}

#[test]
#[ignore = "#462"]
fn open_settings_opens_a_settings_tab_right_after_the_active_tab() {
    let dir = folder();
    let mut app = app_at(dir.path());
    let first = app.active_tab_id();
    app.open_tab(Location::file_system(dir.path().join("하위")), false);
    assert_eq!(app.active_tab_id(), first);

    assert_eq!(app.execute(CommandId::OpenSettings), Execution::Done);
    wait_active(&mut app, "설정 탭이 활성이 되지 않았다", |app| {
        app.active_tab().location().is_settings()
    });
    let panel = app.active_window().active_panel();
    assert_eq!(panel.len(), 3);
    // 활성 탭 바로 뒤(둘째 자리)에 들어간다.
    assert_eq!(panel.active_index(), 1);
    assert_ne!(app.active_tab_id(), first);
    let tab = app.active_tab();
    assert_eq!(tab.len(), 0, "설정 위치에는 목록 항목이 없다");
    assert!(!tab.can_go_up());
    // 파일 작업 명령은 꺼진다.
    for command in [
        CommandId::NewFolder,
        CommandId::Paste,
        CommandId::Copy,
        CommandId::Cut,
        CommandId::Delete,
        CommandId::Rename,
        CommandId::CopyPath,
    ] {
        assert!(!app.can_execute(command), "{command:?}가 켜져 있다");
    }
}

#[test]
#[ignore = "#462"]
fn open_settings_goes_to_the_existing_settings_tab_of_the_same_window() {
    let dir = folder();
    let mut app = app_at(dir.path());
    let start = app.active_tab_id();
    app.execute(CommandId::OpenSettings);
    wait_active(&mut app, "설정 탭", |app| {
        app.active_tab().location().is_settings()
    });
    let settings = app.active_tab_id();

    assert!(app.activate_tab(start));
    assert_eq!(app.execute(CommandId::OpenSettings), Execution::Done);
    assert_eq!(app.active_tab_id(), settings);
    assert_eq!(settings_tab_count(&app), 1);
    assert_eq!(app.active_window().active_panel().len(), 2);

    // 설정 탭이 반대편 패널에 있어도 그 탭으로 간다.
    assert!(app.activate_tab(start));
    assert!(app.split_panel().is_some());
    assert!(app.active_window().is_split());
    assert_eq!(app.active_window().active_panel_index(), 1);
    assert_eq!(app.execute(CommandId::OpenSettings), Execution::Done);
    assert_eq!(app.active_tab_id(), settings);
    assert_eq!(app.active_window().active_panel_index(), 0);
    assert_eq!(settings_tab_count(&app), 1);
}

#[test]
#[ignore = "#462"]
fn a_settings_tab_in_another_window_does_not_count() {
    let dir = folder();
    let mut app = app_at(dir.path());
    app.execute(CommandId::OpenSettings);
    wait_active(&mut app, "설정 탭", |app| {
        app.active_tab().location().is_settings()
    });
    let first_window = app.active_window_id();
    let first_settings = app.active_tab_id();

    let second = app.open_window(Location::file_system(dir.path()));
    assert_eq!(app.active_window_id(), second);
    assert_eq!(app.execute(CommandId::OpenSettings), Execution::Done);
    wait_active(&mut app, "둘째 창의 설정 탭", |app| {
        app.active_tab().location().is_settings()
    });
    assert_eq!(app.active_window_id(), second);
    assert_ne!(app.active_tab_id(), first_settings);
    assert_eq!(settings_tab_count(&app), 1);
    assert!(
        app.window(first_window)
            .unwrap()
            .panels()
            .iter()
            .any(|p| p.tabs().any(|(id, _)| id == first_settings))
    );
}

#[test]
#[ignore = "#462"]
fn the_address_bar_goes_to_facet_settings_and_back() {
    let dir = folder();
    let mut app = app_at(dir.path());
    let tab = app.active_tab_id();
    assert!(app.go_to_text("facet:settings"));
    wait_active(
        &mut app,
        "주소창으로 설정 위치에 가지 않았다",
        |app| app.active_tab().location().is_settings(),
    );
    // 새 탭이 아니라 그 탭이 간다.
    assert_eq!(app.active_tab_id(), tab);
    assert!(app.active_tab().can_go_back());

    assert!(app.navigate(Navigation::Back));
    wait_active(&mut app, "뒤로 가지 않았다", |app| {
        app.active_tab().location() == &Location::file_system(dir.path())
    });

    // 대소문자를 가리지 않는다.
    assert!(app.go_to_text("FACET:Settings"));
    wait_active(&mut app, "대문자 주소", |app| {
        app.active_tab().location().is_settings()
    });
}

#[test]
#[ignore = "#462"]
fn the_settings_tab_survives_the_session() {
    let dir = folder();
    let mut app = app_at(dir.path());
    app.execute(CommandId::OpenSettings);
    wait_active(&mut app, "설정 탭", |app| {
        app.active_tab().location().is_settings()
    });
    let json = app.session().expect("세션").to_json();
    let session = Session::from_json(&json).expect("다시 읽기");

    let restored = App::restore(Some(session), vec![]);
    let window = restored.active_window();
    let all: Vec<Location> = window.panels().iter().flat_map(locations).collect();
    assert_eq!(all.len(), 2, "{all:?}");
    assert_eq!(all[0], Location::file_system(dir.path()));
    assert!(all[1].is_settings(), "{all:?}");
    assert!(restored.active_tab().location().is_settings());
}
