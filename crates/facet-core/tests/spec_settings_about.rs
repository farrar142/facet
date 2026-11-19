// target: crates/facet-core/tests/spec_settings_about.rs
// AC: #459: 「facet 정보」(about) 명령이 명령 목록·팔레트에 있고 늘 켜지며 기본 단축키가 없음, 실행하면 화면에 정보 화면을 맡김
//! 정보 화면을 여는 명령(`CommandId::About`, `Execution::ShowAbout`)을 앱([`App`])으로 확인한다. 정보 화면의 내용
//! (버전·커밋·라이선스 목록·`AboutSlint`)은 사람이 확인한다. 모든 환경에서 돈다.

use std::time::Duration;

use facet_core::Location;
use facet_core::command::CommandId;
use facet_core::tab::ReadOutcome;
use facet_core::window::{App, Execution};
use tempfile::TempDir;

#[test]
#[ignore = "#459"]
fn the_about_command_has_a_stable_id_and_no_default_shortcut() {
    assert_eq!(CommandId::About.as_str(), "about");
    assert_eq!(CommandId::from_id("about"), Some(CommandId::About));
    assert!(CommandId::ALL.contains(&CommandId::About));
    assert!(CommandId::About.shortcuts().is_empty());
}

#[test]
#[ignore = "#459"]
fn the_about_command_is_always_enabled_and_hands_the_screen_the_about_page() {
    let dir = TempDir::new().unwrap();
    let mut app = App::new(Some(Location::file_system(dir.path())));
    let (_, outcome) = app.wait(Duration::from_secs(10)).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);

    let found = app.search_commands("", |c| c.as_str().to_owned());
    let entry = found
        .iter()
        .find(|e| e.command == CommandId::About)
        .unwrap_or_else(|| panic!("팔레트에 있어야 한다"));
    assert!(entry.enabled);
    assert!(entry.shortcuts.is_empty());
    assert!(app.can_execute(CommandId::About));
    assert_eq!(app.execute(CommandId::About), Execution::ShowAbout);
}
