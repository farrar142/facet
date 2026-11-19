// target: crates/facet-core/tests/spec_settings_app.rs
// AC: #461: 설정 없는 앱은 기본값·넘긴 설정을 돌려줌, set_setting·reset_setting이 쓰고 곧바로 보임, 밖의 변경 알림과 적용(Windows), facet이 쓴 변경은 알림 없음(Windows), 밖에서 깨뜨리면 알림·기본값·오류 위치(Windows), 바꾼 단축키·프리셋이 곧바로 해석에 쓰임, 입력란 포커스는 전역 명령만
//! 앱([`App`])이 설정을 들고 쓰는 것(`App::set_settings`·`settings`·`set_setting`·`reset_setting`·`set_shortcuts`·
//! `watch_settings`·`shortcuts`·`find_shortcut`, `Update::Settings`)을 임시 폴더의 실제 `settings.toml`로 확인한다.
//! 설정 폴더 감시를 쓰는 테스트(밖의 변경·facet이 쓴 변경)는 Windows에서만 돌고, 나머지는 모든 환경에서 돈다.

use std::fs;
use std::path::Path;
use std::time::Duration;

use facet_core::Location;
use facet_core::command::{CommandId, Key, Shortcut};
use facet_core::keymap::Context;
use facet_core::settings::{FILE_NAME, Saved, Settings, Value, ValueError};
use facet_core::tab::ReadOutcome;
use facet_core::window::App;
use tempfile::TempDir;

/// CI 러너가 느려도 흔들리지 않게 넉넉히 기다린다. 조건이 맞으면 곧바로 끝난다.
const WAIT: Duration = Duration::from_secs(10);

fn f(n: u8) -> Shortcut {
    Shortcut::new(Key::Function(n))
}

fn ctrl(c: char) -> Shortcut {
    Shortcut::new(Key::Letter(c)).ctrl()
}

/// 볼 폴더(`files`)와 설정 폴더(`config`)를 따로 둔 임시 폴더.
struct Fixture {
    _dir: TempDir,
    files: std::path::PathBuf,
    config: std::path::PathBuf,
}

fn fixture() -> Fixture {
    let dir = TempDir::new().unwrap();
    let files = dir.path().join("files");
    let config = dir.path().join("config");
    fs::create_dir(&files).unwrap();
    fs::create_dir(&config).unwrap();
    fs::write(files.join("a.txt"), "a").unwrap();
    Fixture {
        _dir: dir,
        files,
        config,
    }
}

fn app_at(path: &Path) -> App {
    let mut app = App::new(Some(Location::file_system(path)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

/// 설정 폴더의 설정을 넘긴 앱.
fn app_with_settings(fixture: &Fixture) -> App {
    let mut app = app_at(&fixture.files);
    app.set_settings(Settings::load(&fixture.config));
    app
}

fn write_settings(folder: &Path, text: &str) {
    fs::write(folder.join(FILE_NAME), text).unwrap();
}

#[test]
#[ignore = "#461"]
fn an_app_without_settings_uses_the_defaults_and_writes_nothing() {
    let fixture = fixture();
    let mut app = app_at(&fixture.files);
    assert_eq!(app.settings().file_path(), None);
    assert_eq!(
        app.settings().get("view.theme"),
        Some(Value::Choice("system"))
    );
    assert_eq!(
        app.find_shortcut(f(5), Context::List),
        Some(CommandId::Refresh)
    );
    assert_eq!(app.shortcuts(CommandId::Refresh), vec![f(5)]);
    app.set_setting("view.font_size", Value::Integer(12))
        .expect("받아야 한다");
    assert_eq!(
        app.settings().get("view.font_size"),
        Some(Value::Integer(12))
    );
    assert_eq!(fs::read_dir(fixture.config.as_path()).unwrap().count(), 0);
}

#[test]
#[ignore = "#461"]
fn the_app_keeps_the_settings_it_was_given() {
    let fixture = fixture();
    write_settings(&fixture.config, "[view]\nfont_size = 12\n");
    let app = app_with_settings(&fixture);
    assert_eq!(
        app.settings().file_path(),
        Some(fixture.config.join(FILE_NAME))
    );
    assert_eq!(
        app.settings().get("view.font_size"),
        Some(Value::Integer(12))
    );
}

#[test]
#[ignore = "#461"]
fn set_and_reset_write_the_file_and_show_at_once() {
    let fixture = fixture();
    let mut app = app_with_settings(&fixture);
    assert_eq!(
        app.set_setting("thumbnails.cache_limit_mb", Value::Integer(4096)),
        Ok(Saved::Written)
    );
    assert_eq!(
        app.settings().get("thumbnails.cache_limit_mb"),
        Some(Value::Integer(4096))
    );
    assert!(app.settings().is_modified("thumbnails.cache_limit_mb"));
    assert_eq!(
        Settings::load(&fixture.config).get("thumbnails.cache_limit_mb"),
        Some(Value::Integer(4096))
    );

    assert_eq!(
        app.set_setting("thumbnails.cache_limit_mb", Value::Integer(1)),
        Err(ValueError::OutOfRange {
            min: 64,
            max: 102400
        })
    );
    assert_eq!(
        app.settings().get("thumbnails.cache_limit_mb"),
        Some(Value::Integer(4096))
    );

    assert_eq!(
        app.reset_setting("thumbnails.cache_limit_mb"),
        Ok(Saved::Written)
    );
    assert_eq!(
        app.settings().get("thumbnails.cache_limit_mb"),
        Some(Value::Integer(1024))
    );
    assert_eq!(
        Settings::load(&fixture.config).get("thumbnails.cache_limit_mb"),
        Some(Value::Integer(1024))
    );
}

#[test]
#[ignore = "#461"]
fn changed_shortcuts_are_used_for_the_next_key_at_once() {
    let fixture = fixture();
    let mut app = app_with_settings(&fixture);
    assert_eq!(
        app.set_shortcuts(CommandId::Refresh, vec![f(9), ctrl('R')]),
        Saved::Written
    );
    assert_eq!(
        app.find_shortcut(f(9), Context::List),
        Some(CommandId::Refresh)
    );
    assert_eq!(
        app.find_shortcut(ctrl('R'), Context::List),
        Some(CommandId::Refresh)
    );
    assert_eq!(app.find_shortcut(f(5), Context::List), None);
    // 팔레트·메뉴가 보일 단축키(첫 것이 대표).
    assert_eq!(app.shortcuts(CommandId::Refresh), vec![f(9), ctrl('R')]);
    assert_eq!(
        Settings::load(&fixture.config)
            .keymap()
            .shortcuts(CommandId::Refresh),
        vec![f(9), ctrl('R')]
    );

    assert_eq!(app.reset_shortcuts(CommandId::Refresh), Saved::Written);
    assert_eq!(
        app.find_shortcut(f(5), Context::List),
        Some(CommandId::Refresh)
    );

    app.set_shortcuts(CommandId::Delete, vec![]);
    assert_eq!(
        app.find_shortcut(Shortcut::new(Key::Delete), Context::List),
        None
    );
    assert_eq!(app.reset_all_shortcuts(), Saved::Written);
    assert_eq!(
        app.find_shortcut(Shortcut::new(Key::Delete), Context::List),
        Some(CommandId::Delete)
    );
}

#[test]
#[ignore = "#461"]
fn changing_the_preset_changes_how_keys_resolve() {
    let fixture = fixture();
    let mut app = app_with_settings(&fixture);
    app.set_shortcuts(CommandId::Undo, vec![f(11)]);
    assert_eq!(
        app.set_setting("shortcuts.preset", Value::Choice("total-commander")),
        Ok(Saved::Written)
    );
    assert_eq!(
        app.find_shortcut(f(5), Context::List),
        Some(CommandId::CopyToOtherPanel)
    );
    assert_eq!(
        app.find_shortcut(f(7), Context::List),
        Some(CommandId::NewFolder)
    );
    assert_eq!(app.shortcuts(CommandId::Refresh), vec![ctrl('R')]);
    // 사용자 변경은 남는다.
    assert_eq!(
        app.find_shortcut(f(11), Context::List),
        Some(CommandId::Undo)
    );
}

#[test]
#[ignore = "#461"]
fn an_input_focus_only_hears_global_commands() {
    let fixture = fixture();
    let app = app_with_settings(&fixture);
    assert_eq!(
        app.find_shortcut(ctrl('T'), Context::Input),
        Some(CommandId::NewTab)
    );
    assert_eq!(app.find_shortcut(ctrl('Z'), Context::Input), None);
    assert_eq!(
        app.find_shortcut(ctrl('Z'), Context::List),
        Some(CommandId::Undo)
    );
}

#[test]
#[ignore = "#461"]
fn watching_a_missing_folder_does_nothing() {
    let dir = TempDir::new().unwrap();
    let mut app = app_at(dir.path());
    app.set_settings(Settings::load(&dir.path().join("없는 폴더")));
    app.watch_settings();
    app.watch_settings();
    // 설정을 넘기지 않은 앱도 그대로다.
    let mut plain = app_at(dir.path());
    plain.watch_settings();
    assert!(!dir.path().join("없는 폴더").exists());
}

/// `Update::Settings`가 올 때까지 받는다. `limit` 안에 오지 않으면 `false`.
#[cfg(windows)]
fn settings_update_within(app: &mut App, limit: Duration) -> bool {
    use facet_core::window::Update;
    use std::time::Instant;
    let deadline = Instant::now() + limit;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        if let Some((_, Update::Settings)) = app.wait_update(left) {
            return true;
        }
    }
}

#[cfg(windows)]
#[test]
#[ignore = "#461"]
fn an_outside_edit_is_reloaded_and_announced() {
    let fixture = fixture();
    write_settings(&fixture.config, "[view]\nfont_size = 11\n");
    let mut app = app_with_settings(&fixture);
    app.watch_settings();
    write_settings(
        &fixture.config,
        "[view]\nfont_size = 14\n\n[shortcuts.commands]\nrefresh = [\"F9\"]\n",
    );
    assert!(
        settings_update_within(&mut app, WAIT),
        "밖의 변경 알림이 오지 않았다"
    );
    assert_eq!(
        app.settings().get("view.font_size"),
        Some(Value::Integer(14))
    );
    assert_eq!(
        app.find_shortcut(f(9), Context::List),
        Some(CommandId::Refresh)
    );
    assert_eq!(app.find_shortcut(f(5), Context::List), None);
}

#[cfg(windows)]
#[test]
#[ignore = "#461"]
fn a_file_created_outside_is_picked_up() {
    let fixture = fixture();
    let mut app = app_with_settings(&fixture);
    app.watch_settings();
    write_settings(&fixture.config, "[general]\nshow_hidden = true\n");
    assert!(settings_update_within(&mut app, WAIT));
    assert_eq!(
        app.settings().get("general.show_hidden"),
        Some(Value::Bool(true))
    );
}

#[cfg(windows)]
#[test]
#[ignore = "#461"]
fn facets_own_writes_are_not_announced() {
    let fixture = fixture();
    let mut app = app_with_settings(&fixture);
    app.watch_settings();
    app.set_setting("view.font_size", Value::Integer(13))
        .unwrap();
    app.set_shortcuts(CommandId::Refresh, vec![f(9)]);
    app.reset_setting("view.font_size").unwrap();
    // 묶는 시간(100ms)을 넉넉히 넘겨 기다린다.
    assert!(
        !settings_update_within(&mut app, Duration::from_millis(1500)),
        "facet이 쓴 변경을 알렸다"
    );
    assert_eq!(
        app.find_shortcut(f(9), Context::List),
        Some(CommandId::Refresh)
    );

    // 그 뒤의 밖의 변경은 여전히 알린다.
    write_settings(&fixture.config, "[view]\nfont_size = 15\n");
    assert!(settings_update_within(&mut app, WAIT));
    assert_eq!(
        app.settings().get("view.font_size"),
        Some(Value::Integer(15))
    );
}

#[cfg(windows)]
#[test]
#[ignore = "#461"]
fn breaking_the_file_outside_falls_back_to_defaults_with_the_position() {
    let fixture = fixture();
    write_settings(&fixture.config, "[view]\nfont_size = 12\n");
    let mut app = app_with_settings(&fixture);
    app.watch_settings();
    let broken = "[view]\nfont_size = 12\n[general\n";
    write_settings(&fixture.config, broken);
    assert!(settings_update_within(&mut app, WAIT));
    let error = app.settings().syntax_error().expect("문법 오류");
    assert_eq!(error.line, 3, "{error:?}");
    assert_eq!(
        app.settings().get("view.font_size"),
        Some(Value::Integer(9))
    );
    // 깨진 파일은 쓰지 않는다.
    assert_eq!(
        app.set_setting("view.font_size", Value::Integer(16)),
        Ok(Saved::KeptInMemory(
            facet_core::settings::SaveError::FileBroken
        ))
    );
    assert_eq!(
        fs::read_to_string(fixture.config.join(FILE_NAME)).unwrap(),
        broken
    );
}
