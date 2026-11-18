// target: crates/facet-shell-helper/tests/spec_shell_helper_app_menu.rs
// AC: #322: 「불러오는 중」 뒤 셸 항목·자체 항목 그대로, 선택 전체, 폴더 배경, 검색 결과의 제 폴더 경로, 시간 초과의 「불러오지 못함」과 늦은 응답 버림, 메뉴를 닫으면 결과 없음, 도우미가 죽으면 「불러오지 못함」과 다음 메뉴 정상, 쉬는 중·죽게 한 확장 이름, 셸 항목 실행과 메뉴 닫힘·작업 기록 없음, 꺼진·아직 없는 항목은 실행하지 않음
//! 앱(`App`)의 오른쪽 메뉴가 자체 항목 아래 셸 항목 자리를 두고 셸 확장 도우미의 응답으로 채우는 흐름(`open_shell_menu`·
//! `shell_menu`·`close_shell_menu`·`invoke_shell_item`·`Update::ShellMenu`)을 시험용 모드(`--fake`)의 실제 도우미
//! 프로세스와 임시 폴더의 실제 파일로 확인한다. 화면 그리기는 사람이 확인한다. 실제 프로세스를 쓰므로 Windows에서만 돈다.
#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::menu::MenuKind;
use facet_core::shell_helper::{HelperOptions, ShellMenu, ShellMenuId, ShellSection};
use facet_core::tab::ReadOutcome;
use facet_core::window::{App, Update};
use facet_shell_protocol::MenuItem;
use tempfile::TempDir;

/// CI 러너가 느려도 흔들리지 않게 넉넉히 기다린다. 결과가 오면 곧바로 끝난다.
const WAIT: Duration = Duration::from_secs(20);

fn fake_options() -> HelperOptions {
    let mut options = HelperOptions::new(PathBuf::from(env!("CARGO_BIN_EXE_facet-shell-helper")));
    options.args = vec!["--fake".into()];
    options.crash_log = None;
    options
}

/// 이름마다 작은 파일을 만든 임시 폴더.
fn folder_with(names: &[&str]) -> TempDir {
    let dir = TempDir::new().unwrap();
    for name in names {
        fs::write(dir.path().join(name), b"x").unwrap();
    }
    dir
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

fn index_of(app: &App, name: &str) -> usize {
    app.active_tab()
        .items()
        .position(|item| item.name == name)
        .unwrap_or_else(|| panic!("{name}이 목록에 없다"))
}

/// 그 항목을 오른쪽 클릭하고 셸 메뉴를 연다.
fn open_on(app: &mut App, name: &str) -> ShellMenuId {
    let index = index_of(app, name);
    assert_eq!(app.context_click(0, Some(index)), Some(MenuKind::Item));
    app.open_shell_menu(MenuKind::Item)
        .expect("도우미 관리가 켜져 있고 대상이 있다")
}

/// 열린 셸 메뉴의 셸 자리가 「불러오는 중」을 벗어날 때까지 받는다. 열 때 「불러오는 중」이었으면 그 사이
/// `Update::ShellMenu(id)`가 와야 한다(쉬는 중처럼 열 때 이미 정해진 자리는 바뀐 것이 아니다).
fn wait_settled(app: &mut App, id: ShellMenuId) -> ShellMenu {
    let deadline = Instant::now() + WAIT;
    let mut announced = app.shell_menu().expect("열린 셸 메뉴").section != ShellSection::Loading;
    loop {
        for (_, update) in app.poll() {
            if matches!(update, Update::ShellMenu(got) if got == id) {
                announced = true;
            }
        }
        let menu = app.shell_menu().expect("열린 셸 메뉴").clone();
        assert_eq!(menu.id, id, "다른 셸 메뉴가 열려 있다");
        if menu.section != ShellSection::Loading {
            assert!(announced, "셸 자리가 바뀌면 Update::ShellMenu가 온다");
            return menu;
        }
        assert!(
            Instant::now() < deadline,
            "셸 자리가 「불러오는 중」에 머문다"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// `during` 동안 그 셸 메뉴의 `Update::ShellMenu`가 오지 않는가.
fn no_update_for(app: &mut App, id: ShellMenuId, during: Duration) -> bool {
    let deadline = Instant::now() + during;
    while Instant::now() < deadline {
        for (_, update) in app.poll() {
            if matches!(update, Update::ShellMenu(got) if got == id) {
                return false;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

fn items(menu: &ShellMenu) -> &[MenuItem] {
    match &menu.section {
        ShellSection::Items(items) => items,
        other => panic!("셸 항목이 와야 한다: {other:?}"),
    }
}

fn first_text(menu: &ShellMenu) -> &str {
    match items(menu).first() {
        Some(MenuItem::Command(command)) => &command.text,
        other => panic!("첫 줄은 명령 항목이어야 한다: {other:?}"),
    }
}

/// 도우미가 쓴 실행 표지의 줄들. 생길 때까지 기다린다.
fn wait_for_invoked(app: &mut App, folder: &Path) -> Vec<String> {
    let marker = folder.join("facet-fake-invoked.txt");
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll();
        if let Ok(text) = fs::read_to_string(&marker)
            && !text.is_empty()
        {
            return text.lines().map(str::to_owned).collect();
        }
        assert!(Instant::now() < deadline, "셸 항목이 실행되지 않았다");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// `crash` 파일의 셸 메뉴를 열어 도우미를 죽게 하고 「불러오지 못함」을 확인한다.
fn crash_once(app: &mut App) {
    let id = open_on(app, "crash");
    let menu = wait_settled(app, id);
    assert_eq!(menu.section, ShellSection::Failed);
}

#[test]
fn own_items_show_at_once_and_shell_items_follow_below() {
    let dir = folder_with(&["a.txt"]);
    let mut app = app_at(dir.path(), Some(fake_options()));
    let index = index_of(&app, "a.txt");
    assert_eq!(app.context_click(0, Some(index)), Some(MenuKind::Item));
    let own = app.context_menu(MenuKind::Item);

    let id = app.open_shell_menu(MenuKind::Item).expect("셸 메뉴를 연다");
    let opened = app.shell_menu().expect("열린 셸 메뉴");
    assert_eq!(opened.id, id);
    assert_eq!(opened.kind, MenuKind::Item);
    assert_eq!(
        opened.section,
        ShellSection::Loading,
        "곧바로 「불러오는 중」이다"
    );
    assert_eq!(opened.crashing_extension, None);

    let menu = wait_settled(&mut app, id);
    assert_eq!(first_text(&menu), "fake: a.txt");
    assert_eq!(items(&menu).len(), 4, "도우미가 준 트리 그대로");
    assert_eq!(
        app.context_menu(MenuKind::Item),
        own,
        "셸 항목이 붙어도 자체 항목은 그대로다"
    );
}

#[test]
fn the_shell_menu_covers_the_whole_selection() {
    let dir = folder_with(&["a.txt", "b.txt", "c.txt"]);
    let mut app = app_at(dir.path(), Some(fake_options()));
    let a = index_of(&app, "a.txt");
    let c = index_of(&app, "c.txt");
    app.active_tab_mut().select_only(a);
    app.active_tab_mut().toggle_select(c);

    // 선택 안의 항목을 오른쪽 클릭하면 선택은 그대로다.
    let id = open_on(&mut app, "c.txt");
    let menu = wait_settled(&mut app, id);
    assert_eq!(first_text(&menu), "fake: a.txt|c.txt");
}

#[test]
fn the_background_menu_is_for_the_current_folder() {
    let dir = TempDir::new().unwrap();
    let folder = dir.path().join("f");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("a.txt"), b"x").unwrap();
    let mut app = app_at(&folder, Some(fake_options()));
    assert_eq!(app.context_click(0, None), Some(MenuKind::Background));

    let id = app.open_shell_menu(MenuKind::Background).unwrap();
    let menu = wait_settled(&mut app, id);
    assert_eq!(menu.kind, MenuKind::Background);
    assert_eq!(first_text(&menu), "fake-background: f");
}

#[test]
fn search_result_items_use_their_own_folders() {
    let dir = TempDir::new().unwrap();
    let deep = dir.path().join("f");
    fs::create_dir(&deep).unwrap();
    fs::write(deep.join("needle.txt"), b"x").unwrap();
    fs::write(dir.path().join("other.txt"), b"x").unwrap();
    let mut app = app_at(dir.path(), Some(fake_options()));

    app.start_search("needle").unwrap();
    app.wait(WAIT).expect("결과 위치로 옮김");
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll();
        let status = app.active_tab().search().expect("검색 결과 위치");
        if status.summary.is_some() && !status.running {
            break;
        }
        assert!(Instant::now() < deadline, "검색이 끝나지 않았다");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(app.active_tab().len(), 1);

    let id = open_on(&mut app, "needle.txt");
    let menu = wait_settled(&mut app, id);
    assert_eq!(first_text(&menu), "fake: needle.txt");
    assert!(app.invoke_shell_item(1));
    assert_eq!(
        wait_for_invoked(&mut app, &deep),
        ["1", "needle.txt"],
        "결과 항목은 제 폴더 경로로 간다"
    );
    assert!(!dir.path().join("facet-fake-invoked.txt").exists());
}

#[test]
fn a_slow_helper_fails_the_shell_section_and_its_late_reply_is_dropped() {
    let dir = folder_with(&["late-2000"]);
    let mut options = fake_options();
    options.reply_timeout = Duration::from_millis(300);
    let mut app = app_at(dir.path(), Some(options));

    let id = open_on(&mut app, "late-2000");
    let menu = wait_settled(&mut app, id);
    assert_eq!(
        menu.section,
        ShellSection::Failed,
        "「셸 항목 불러오지 못함」"
    );

    assert!(
        no_update_for(&mut app, id, Duration::from_secs(3)),
        "늦게 온 응답은 버린다"
    );
    assert_eq!(app.shell_menu().unwrap().section, ShellSection::Failed);
}

#[test]
fn closing_the_menu_drops_its_pending_request() {
    let dir = folder_with(&["late-800"]);
    let mut app = app_at(dir.path(), Some(fake_options()));

    let id = open_on(&mut app, "late-800");
    app.close_shell_menu();
    assert!(app.shell_menu().is_none());
    assert!(
        no_update_for(&mut app, id, Duration::from_secs(2)),
        "닫은 메뉴의 결과는 오지 않는다"
    );
    assert!(app.shell_menu().is_none());
}

#[test]
fn a_crashing_helper_fails_the_open_menu_and_the_next_menu_is_fine() {
    let dir = folder_with(&["a.txt", "crash"]);
    let mut app = app_at(dir.path(), Some(fake_options()));

    crash_once(&mut app);

    let id = open_on(&mut app, "a.txt");
    let menu = wait_settled(&mut app, id);
    assert_eq!(
        first_text(&menu),
        "fake: a.txt",
        "다음 메뉴는 정상으로 뜬다"
    );
}

#[test]
fn a_resting_helper_shows_that_it_is_resting() {
    let dir = folder_with(&["a.txt", "crash"]);
    let mut options = fake_options();
    options.crash_limit = 0;
    options.rest = Duration::from_secs(60);
    let mut app = app_at(dir.path(), Some(options));

    crash_once(&mut app);

    let id = open_on(&mut app, "a.txt");
    let menu = wait_settled(&mut app, id);
    assert_eq!(menu.section, ShellSection::Resting);
}

#[test]
fn three_crashes_of_one_extension_show_its_name() {
    let dir = folder_with(&["a.txt", "crash"]);
    let mut options = fake_options();
    options.crash_limit = 10;
    let mut app = app_at(dir.path(), Some(options));

    for _ in 0..3 {
        crash_once(&mut app);
    }

    let id = open_on(&mut app, "a.txt");
    let menu = wait_settled(&mut app, id);
    assert_eq!(first_text(&menu), "fake: a.txt");
    let extension = menu
        .crashing_extension
        .expect("세 번 죽게 한 확장을 셸 자리에 알린다");
    assert_eq!(extension.name.as_deref(), Some("Fake Extension"));
    assert_eq!(extension.crashes, 3);
}

#[test]
fn invoking_a_shell_item_runs_it_closes_the_menu_and_records_nothing() {
    let dir = folder_with(&["a.txt"]);
    let mut app = app_at(dir.path(), Some(fake_options()));
    let records = app.history().records().len();

    let id = open_on(&mut app, "a.txt");
    wait_settled(&mut app, id);
    assert!(app.invoke_shell_item(1));
    assert!(app.shell_menu().is_none(), "실행하면 메뉴가 닫힌다");
    assert_eq!(wait_for_invoked(&mut app, dir.path()), ["1", "a.txt"]);

    std::thread::sleep(Duration::from_millis(300));
    app.poll();
    assert_eq!(
        app.history().records().len(),
        records,
        "셸 항목이 한 일은 작업 기록에 들지 않는다"
    );
}

#[test]
fn disabled_or_not_yet_loaded_items_are_not_invoked() {
    let dir = folder_with(&["late-1500"]);
    let mut options = fake_options();
    options.reply_timeout = Duration::from_secs(10);
    let mut app = app_at(dir.path(), Some(options));

    let id = open_on(&mut app, "late-1500");
    assert!(!app.invoke_shell_item(1), "아직 오지 않은 메뉴");
    assert!(app.shell_menu().is_some(), "메뉴는 그대로 열려 있다");

    wait_settled(&mut app, id);
    assert!(!app.invoke_shell_item(3), "꺼진 항목");
    assert!(!app.invoke_shell_item(99), "없는 항목");
    assert!(app.shell_menu().is_some());
    std::thread::sleep(Duration::from_millis(500));
    assert!(!dir.path().join("facet-fake-invoked.txt").exists());

    assert!(app.invoke_shell_item(2), "하위 메뉴 안의 켜진 항목");
    assert_eq!(wait_for_invoked(&mut app, dir.path()), ["2", "late-1500"]);
}

#[test]
fn without_the_helper_no_shell_menu_opens() {
    let dir = folder_with(&["a.txt"]);
    let mut app = app_at(dir.path(), None);
    let index = index_of(&app, "a.txt");
    assert_eq!(app.context_click(0, Some(index)), Some(MenuKind::Item));
    assert_eq!(app.open_shell_menu(MenuKind::Item), None);
    assert!(app.shell_menu().is_none());
}
