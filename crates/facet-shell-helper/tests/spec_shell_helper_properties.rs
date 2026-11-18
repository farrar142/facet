// target: crates/facet-shell-helper/tests/spec_shell_helper_properties.rs
// AC: #321: 도우미 관리를 켜도 도우미가 뜨지 않음, 고른 항목의 「속성」과 고른 것 없는(빈 곳) 「속성」이 도우미로 감, 「연결 프로그램」이 커서 파일로 도우미에 감(폴더면 꺼짐), 대화상자가 떠 있는 동안 쉬면 끝내기를 미룸
//! 앱(`App`)에서 「속성」·「연결 프로그램」 명령을 실행하면 셸 확장 도우미가 그 대상으로 대화상자를 띄우는지 시험용 모드
//! (`--fake`: 대화상자 대신 표지 파일을 쓰고 속성 「대화상자」는 1.5초 떠 있는 것으로 침)의 실제 도우미 프로세스와 임시
//! 폴더의 실제 파일로 확인한다. 실제 프로세스를 쓰므로 Windows에서만 돈다.
#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::command::CommandId;
use facet_core::menu::MenuKind;
use facet_core::shell_helper::HelperOptions;
use facet_core::tab::ReadOutcome;
use facet_core::window::{App, Execution};
use tempfile::TempDir;

/// CI 러너가 느려도 흔들리지 않게 넉넉히 기다린다. 결과가 오면 곧바로 끝난다.
const WAIT: Duration = Duration::from_secs(20);

fn fake_options() -> HelperOptions {
    let mut options = HelperOptions::new(PathBuf::from(env!("CARGO_BIN_EXE_facet-shell-helper")));
    options.args = vec!["--fake".into()];
    options.crash_log = None;
    options
}

/// `a.txt`·`b.txt`와 폴더 `f`, 그 안의 `c.txt`가 든 임시 폴더.
fn tree() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.txt"), b"a").unwrap();
    fs::write(dir.path().join("b.txt"), b"b").unwrap();
    fs::create_dir(dir.path().join("f")).unwrap();
    fs::write(dir.path().join("f").join("c.txt"), b"c").unwrap();
    dir
}

fn app_at(path: &Path, options: HelperOptions) -> App {
    let mut app = App::new(Some(Location::file_system(path)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app.enable_shell_helper(options);
    app
}

fn index_of(app: &App, name: &str) -> usize {
    app.active_tab()
        .items()
        .position(|item| item.name == name)
        .unwrap_or_else(|| panic!("{name}이 목록에 없다"))
}

/// 도우미가 쓴 표지 파일의 줄들. 생길 때까지 기다린다(앱의 결과도 받아 둔다).
fn wait_for_marker(app: &mut App, marker: &Path) -> Vec<String> {
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll();
        if let Ok(text) = fs::read_to_string(marker)
            && !text.is_empty()
        {
            return text.lines().map(str::to_owned).collect();
        }
        assert!(
            Instant::now() < deadline,
            "도우미가 {}를 쓰지 않았다",
            marker.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 그 번호의 프로세스가 살아 있는가(`tasklist`로 본다).
fn is_alive(pid: u32) -> bool {
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .expect("tasklist");
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

#[test]
#[ignore = "#321"]
fn enabling_the_helper_does_not_start_it() {
    let dir = tree();
    let app = app_at(dir.path(), fake_options());
    std::thread::sleep(Duration::from_millis(300));
    let helper = app.shell_helper().expect("도우미 관리를 켰다");
    assert_eq!(helper.process_id(), None, "첫 요청 전에는 뜨지 않는다");
}

#[test]
#[ignore = "#321"]
fn properties_of_the_selection_open_through_the_helper() {
    let dir = tree();
    let mut app = app_at(dir.path(), fake_options());
    let a = index_of(&app, "a.txt");
    let b = index_of(&app, "b.txt");
    app.active_tab_mut().select_only(a);
    app.active_tab_mut().toggle_select(b);

    assert_eq!(app.execute(CommandId::Properties), Execution::Done);
    let names = wait_for_marker(&mut app, &dir.path().join("facet-fake-properties.txt"));
    assert_eq!(names, ["a.txt", "b.txt"], "고른 항목 전체를 목록 순서로");
    let pid = app
        .shell_helper()
        .unwrap()
        .process_id()
        .expect("도우미가 떴다");
    assert_ne!(
        pid,
        std::process::id(),
        "대화상자는 도우미 프로세스가 띄운다"
    );
}

#[test]
#[ignore = "#321"]
fn background_properties_are_the_current_folders() {
    let dir = tree();
    let mut app = app_at(&dir.path().join("f"), fake_options());
    assert_eq!(app.context_click(0, None), Some(MenuKind::Background));
    let menu = app.context_menu(MenuKind::Background);
    let row = menu
        .commands()
        .find(|c| c.id == CommandId::Properties)
        .expect("빈 곳 메뉴의 「속성」");
    assert!(row.enabled);

    assert_eq!(app.execute(row.id), Execution::Done);
    let names = wait_for_marker(&mut app, &dir.path().join("facet-fake-properties.txt"));
    assert_eq!(names, ["f"], "현재 폴더의 속성");
}

#[test]
#[ignore = "#321"]
fn open_with_goes_to_the_helper_for_the_cursor_file_only() {
    let dir = tree();
    let mut app = app_at(dir.path(), fake_options());

    let folder = index_of(&app, "f");
    app.active_tab_mut().select_only(folder);
    assert!(!app.can_execute(CommandId::OpenWith), "폴더에는 꺼진다");
    assert_eq!(app.execute(CommandId::OpenWith), Execution::Disabled);

    let a = index_of(&app, "a.txt");
    app.active_tab_mut().select_only(a);
    assert!(app.can_execute(CommandId::OpenWith));
    assert_eq!(app.execute(CommandId::OpenWith), Execution::Done);
    let names = wait_for_marker(&mut app, &dir.path().join("facet-fake-open-with.txt"));
    assert_eq!(names, ["a.txt"]);
    assert!(
        !dir.path()
            .join("f")
            .join("facet-fake-open-with.txt")
            .exists(),
        "폴더로는 보내지 않았다"
    );
}

#[test]
#[ignore = "#321"]
fn the_helper_does_not_end_idle_while_a_dialog_is_open() {
    let dir = tree();
    let mut options = fake_options();
    options.idle_timeout = Duration::from_millis(200);
    let mut app = app_at(dir.path(), options);
    let a = index_of(&app, "a.txt");
    app.active_tab_mut().select_only(a);

    assert_eq!(app.execute(CommandId::Properties), Execution::Done);
    wait_for_marker(&mut app, &dir.path().join("facet-fake-properties.txt"));
    let shown = Instant::now();
    let pid = app.shell_helper().unwrap().process_id().unwrap();

    std::thread::sleep(Duration::from_millis(700));
    assert!(
        is_alive(pid),
        "대화상자가 떠 있는 동안은 쉬면 끝내기 시간이 지나도 끝나지 않는다"
    );

    let deadline = shown + WAIT;
    while is_alive(pid) {
        assert!(
            Instant::now() < deadline,
            "대화상자가 닫힌 뒤에는 쉬면 끝난다"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}
