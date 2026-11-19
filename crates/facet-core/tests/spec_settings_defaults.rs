// target: crates/facet-core/tests/spec_settings_defaults.rs
// AC: #465: 설정 정의의 기본값이 앞 spec의 상수와 같음, general.show_hidden이 처음 숨김 항목 보이기이고 바꾸면 곧바로 적용(Ctrl+H는 설정을 바꾸지 않음), 내용 검색이 설정 한도로 큰 파일을 건너뜀, 썸네일·도우미에 넘길 값이 설정에서 나옴
//! 앞 spec들이 박아 둔 기본값을 설정으로 끌어낸 것(`Settings::content_max_file_size`·`thumbnail_disk_limit`·
//! `shell_reply_timeout`·`shell_idle_timeout`, 앱의 숨김 항목 보이기와 내용 검색 한도)을 임시 폴더의 실제 파일로
//! 확인한다. 모든 환경에서 돈다.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::command::CommandId;
use facet_core::content_search::{self, ContentMode};
use facet_core::settings::{FILE_NAME, Settings, Value};
use facet_core::shell_helper;
use facet_core::tab::ReadOutcome;
use facet_core::thumbnail_cache;
use facet_core::window::{App, Execution};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(30);

fn app_at(path: &Path) -> App {
    let mut app = App::new(Some(Location::file_system(path)));
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

fn wait_search(app: &mut App) {
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll();
        let status = app.active_tab().search().expect("검색 결과 위치");
        if status.summary.is_some() && !status.running {
            return;
        }
        assert!(Instant::now() < deadline, "검색이 끝나지 않았다");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "#465"]
fn the_defaults_are_the_constants_of_earlier_specs() {
    let defaults = Settings::defaults();
    assert_eq!(
        defaults.content_max_file_size(),
        content_search::DEFAULT_MAX_FILE_SIZE
    );
    assert_eq!(
        defaults.thumbnail_disk_limit(),
        thumbnail_cache::DEFAULT_DISK_LIMIT
    );
    assert_eq!(defaults.shell_reply_timeout(), shell_helper::REPLY_TIMEOUT);
    assert_eq!(defaults.shell_idle_timeout(), shell_helper::IDLE_TIMEOUT);
    assert_eq!(
        defaults.get("general.show_hidden"),
        Some(Value::Bool(false))
    );
}

#[test]
#[ignore = "#465"]
fn the_values_come_from_the_settings_in_their_units() {
    let mut settings = Settings::defaults();
    settings
        .set("search.content_max_file_size_mb", Value::Integer(120))
        .unwrap();
    settings
        .set("thumbnails.cache_limit_mb", Value::Integer(2048))
        .unwrap();
    settings
        .set("advanced.shell_item_timeout_ms", Value::Integer(5000))
        .unwrap();
    settings
        .set("advanced.shell_helper_idle_minutes", Value::Integer(30))
        .unwrap();
    assert_eq!(settings.content_max_file_size(), 120_000_000);
    assert_eq!(settings.thumbnail_disk_limit(), 2048 << 20);
    assert_eq!(settings.shell_reply_timeout(), Duration::from_secs(5));
    assert_eq!(settings.shell_idle_timeout(), Duration::from_secs(30 * 60));
}

#[test]
#[ignore = "#465"]
fn show_hidden_starts_from_the_setting_and_changes_at_once() {
    let dir = TempDir::new().unwrap();
    let config = dir.path().join("config");
    let files = dir.path().join("files");
    fs::create_dir(&config).unwrap();
    fs::create_dir(&files).unwrap();
    fs::write(files.join("a.txt"), "a").unwrap();
    fs::write(config.join(FILE_NAME), "[general]\nshow_hidden = true\n").unwrap();

    let mut app = app_at(&files);
    assert!(!app.show_hidden());
    app.set_settings(Settings::load(&config));
    assert!(app.show_hidden());
    assert!(app.active_tab().show_hidden());

    // 설정을 바꾸면 곧바로 모든 탭에 적용된다.
    let other = app.open_tab(Location::file_system(files.as_path()), false);
    app.set_setting("general.show_hidden", Value::Bool(false))
        .unwrap();
    assert!(!app.show_hidden());
    assert!(!app.active_tab().show_hidden());
    assert!(!app.tab(other).unwrap().show_hidden());

    // Ctrl+H는 이번 실행의 보이기만 바꾸고 설정은 그대로다.
    assert_eq!(app.execute(CommandId::ToggleHidden), Execution::Done);
    assert!(app.show_hidden());
    assert_eq!(
        app.settings().get("general.show_hidden"),
        Some(Value::Bool(false))
    );
    assert_eq!(
        Settings::load(&config).get("general.show_hidden"),
        Some(Value::Bool(false))
    );
}

/// 앞에 찾는 글자가 있는 1.5MB 글 파일 하나와 작은 파일 하나.
fn big_and_small(dir: &Path) {
    let mut big = String::from("찾는말\n");
    while big.len() < 1_500_000 {
        big.push_str("가나다라마바사 abcdefghij 0123456789\n");
    }
    fs::write(dir.join("big.txt"), big).unwrap();
    fs::write(dir.join("small.txt"), "찾는말").unwrap();
}

fn found_names(app: &App) -> Vec<String> {
    let tab = app.active_tab();
    let mut names: Vec<String> = (0..tab.len())
        .map(|i| tab.get(i).unwrap().name.to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
#[ignore = "#465"]
fn content_search_skips_files_above_the_setting() {
    let dir = TempDir::new().unwrap();
    big_and_small(dir.path());
    let mut app = app_at(dir.path());
    app.set_setting("search.content_max_file_size_mb", Value::Integer(1))
        .unwrap();
    app.start_content_search("찾는말", ContentMode::Literal, "")
        .unwrap();
    wait_read(&mut app);
    wait_search(&mut app);
    assert_eq!(found_names(&app), vec!["small.txt".to_owned()]);
    let content = app.active_tab().search().unwrap().content.unwrap();
    assert_eq!(content.skipped.too_large, 1);
}

#[test]
#[ignore = "#465"]
fn content_search_reads_files_below_the_setting() {
    let dir = TempDir::new().unwrap();
    big_and_small(dir.path());
    let mut app = app_at(dir.path());
    app.set_setting("search.content_max_file_size_mb", Value::Integer(2))
        .unwrap();
    app.start_content_search("찾는말", ContentMode::Literal, "")
        .unwrap();
    wait_read(&mut app);
    wait_search(&mut app);
    assert_eq!(
        found_names(&app),
        vec!["big.txt".to_owned(), "small.txt".to_owned()]
    );
    let content = app.active_tab().search().unwrap().content.unwrap();
    assert_eq!(content.skipped.too_large, 0);
}
