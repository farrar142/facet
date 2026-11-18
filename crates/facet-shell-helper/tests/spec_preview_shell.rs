// target: crates/facet-shell-helper/tests/spec_preview_shell.rs
// AC: #342: 도우미 종류 항목의 Shell(Showing), 처리기 없음과 도우미 관리가 꺼짐의 정보만, 3초 시간 초과(앱은 멀쩡), 도우미가 죽으면 실패와 다음 선택에 다시 뜸, 늦은 앞선 응답 무시, 도우미가 잡은 파일의 이름바꾸기·영구 삭제(내리고 한 번 다시 시도), 기본 대기 시간 3초
//! 앱(`App`)의 미리보기 영역이 도우미 종류 항목을 셸 확장 도우미에 맡기는 흐름(`set_preview_host`·
//! `set_preview_handler_lookup`·`App::preview`)을 시험용 모드(`--fake`)의 실제 도우미 프로세스와 임시 폴더의 실제 파일로
//! 확인한다. 시험용 모드는 미리보기 요청을 파일 이름의 줄기(`hang`·`crash`·`late-<ms>`·`nohandler`·`lock-…`)로 고른다
//! (#342의 약속). 실제 처리기와 창 붙이기는 사람이 확인한다. 실제 프로세스를 쓰므로 Windows에서만 돈다.
#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::command::CommandId;
use facet_core::edit::{EditError, Edited};
use facet_core::preview::{
    PreviewBody, PreviewContent, PreviewHost, PreviewProblem, SHELL_PREVIEW_TIMEOUT, ShellPreview,
};
use facet_core::shell_helper::HelperOptions;
use facet_core::tab::ReadOutcome;
use facet_core::window::{App, Execution, Update};
use tempfile::TempDir;

/// CI 러너가 느려도 흔들리지 않게 넉넉히 기다린다. 결과가 오면 곧바로 끝난다.
const WAIT: Duration = Duration::from_secs(20);

const HOST: PreviewHost = PreviewHost {
    parent: 0,
    x: 100,
    y: 100,
    width: 400,
    height: 300,
};

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
        fs::write(dir.path().join(name), b"preview me").unwrap();
    }
    dir
}

/// 그 폴더를 열고 미리보기 영역을 켠 앱. `helper`면 도우미 관리를 켜고, 처리기 조회는 늘 참이다.
fn app_at(path: &Path, helper: bool) -> App {
    let mut app = App::new(Some(Location::file_system(path)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    if helper {
        app.enable_shell_helper(fake_options());
    }
    app.set_preview_handler_lookup(|_| true);
    assert_eq!(app.execute(CommandId::TogglePreview), Execution::Done);
    let window = app.active_window_id();
    app.set_preview_host(window, Some(HOST));
    app
}

fn select(app: &mut App, name: &str) {
    let index = app
        .active_tab()
        .items()
        .position(|item| item.name == name)
        .unwrap_or_else(|| panic!("{name}이 목록에 없다"));
    app.active_tab_mut().select_only(index);
    app.sync_preview();
}

fn body_of<'a>(content: &'a PreviewContent, name: &str) -> Option<&'a PreviewBody> {
    match content {
        PreviewContent::Item { item, body, .. } if item.name == name => Some(body),
        _ => None,
    }
}

/// 그 항목의 본문이 조건에 맞을 때까지 받는다.
fn wait_body(app: &mut App, name: &str, wanted: impl Fn(&PreviewBody) -> bool) -> PreviewBody {
    let window = app.active_window_id();
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(body) = app.preview(window).and_then(|c| body_of(c, name))
            && wanted(body)
        {
            return body.clone();
        }
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(
            !left.is_zero(),
            "{name}의 미리보기가 오지 않았다: {:?}",
            app.preview(window)
        );
        app.wait_update(left.min(Duration::from_millis(200)));
    }
}

fn showing(body: &PreviewBody) -> bool {
    *body == PreviewBody::Shell(ShellPreview::Showing)
}

fn shell_failed(body: &PreviewBody) -> bool {
    *body == PreviewBody::Unavailable(PreviewProblem::ShellFailed)
}

fn helper_pid(app: &App) -> Option<u32> {
    app.shell_helper().and_then(|helper| helper.process_id())
}

#[test]
fn the_shell_preview_timeout_is_three_seconds() {
    assert_eq!(SHELL_PREVIEW_TIMEOUT, Duration::from_secs(3));
}

#[test]
fn a_shell_kind_item_is_shown_by_the_helper() {
    let dir = folder_with(&["doc.fakeprev"]);
    let mut app = app_at(dir.path(), true);
    select(&mut app, "doc.fakeprev");
    wait_body(&mut app, "doc.fakeprev", showing);
    let pid = helper_pid(&app).expect("도우미가 떴다");
    assert_ne!(pid, std::process::id(), "처리기는 별도 프로세스에서 그린다");
}

#[test]
fn no_handler_shows_info_only() {
    let dir = folder_with(&["nohandler.fakeprev"]);
    let mut app = app_at(dir.path(), true);
    select(&mut app, "nohandler.fakeprev");
    wait_body(&mut app, "nohandler.fakeprev", |b| *b == PreviewBody::Info);
}

#[test]
fn without_the_helper_shell_kind_items_show_info_only() {
    let dir = folder_with(&["doc.fakeprev"]);
    let mut app = app_at(dir.path(), false);
    select(&mut app, "doc.fakeprev");
    wait_body(&mut app, "doc.fakeprev", |b| *b == PreviewBody::Info);
    assert!(app.shell_helper().is_none());
}

#[test]
fn a_helper_that_does_not_start_in_time_fails_and_the_app_stays_responsive() {
    let dir = folder_with(&["hang.fakeprev", "other.fakeprev"]);
    let mut app = app_at(dir.path(), true);
    let started = Instant::now();
    select(&mut app, "hang.fakeprev");

    // 도우미가 붙잡혀 있는 동안에도 앱의 부르기는 곧바로 돌아온다.
    std::thread::sleep(Duration::from_millis(500));
    let call = Instant::now();
    app.poll();
    app.sync_preview();
    app.active_tab_mut().select_all();
    app.active_tab_mut().clear_selection();
    select(&mut app, "hang.fakeprev");
    assert!(
        call.elapsed() < Duration::from_millis(200),
        "UI 스레드는 도우미를 기다리지 않는다: {:?}",
        call.elapsed()
    );

    wait_body(&mut app, "hang.fakeprev", shell_failed);
    assert!(
        started.elapsed() >= SHELL_PREVIEW_TIMEOUT,
        "3초보다 일찍 포기하지 않는다: {:?}",
        started.elapsed()
    );
}

#[test]
fn a_crashed_helper_fails_the_preview_and_comes_back_on_the_next_selection() {
    let dir = folder_with(&["crash.fakeprev", "doc.fakeprev"]);
    let mut app = app_at(dir.path(), true);
    select(&mut app, "crash.fakeprev");
    wait_body(&mut app, "crash.fakeprev", shell_failed);
    let crashed = helper_pid(&app).expect("도우미를 띄웠었다");

    select(&mut app, "doc.fakeprev");
    wait_body(&mut app, "doc.fakeprev", showing);
    let again = helper_pid(&app).expect("다시 띄웠다");
    assert_ne!(crashed, again, "새 도우미 프로세스");
}

#[test]
fn a_late_reply_for_an_earlier_item_does_not_change_the_current_one() {
    let dir = folder_with(&["late-1500.fakeprev", "doc.fakeprev"]);
    let mut app = app_at(dir.path(), true);
    select(&mut app, "late-1500.fakeprev");
    std::thread::sleep(Duration::from_millis(300));
    select(&mut app, "doc.fakeprev");
    wait_body(&mut app, "doc.fakeprev", showing);

    let window = app.active_window_id();
    let deadline = Instant::now() + Duration::from_millis(2500);
    while Instant::now() < deadline {
        app.wait_update(Duration::from_millis(100));
        let content = app.preview(window).expect("영역이 켜져 있다");
        assert_eq!(
            body_of(content, "doc.fakeprev"),
            Some(&PreviewBody::Shell(ShellPreview::Showing)),
            "늦게 온 앞선 응답이 지금 항목을 바꾸면 안 된다: {content:?}"
        );
    }
}

fn wait_edited(app: &mut App) -> Result<Edited, EditError> {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "이름바꾸기 결과가 오지 않았다");
        if let Some((_, Update::Edited(result))) = app.wait_update(left) {
            return result;
        }
    }
}

#[test]
fn renaming_a_file_the_helper_holds_releases_the_preview_and_retries() {
    let dir = folder_with(&["lock-a.fakeprev"]);
    let held = dir.path().join("lock-a.fakeprev");
    let mut app = app_at(dir.path(), true);
    select(&mut app, "lock-a.fakeprev");
    wait_body(&mut app, "lock-a.fakeprev", showing);
    assert!(
        fs::rename(&held, dir.path().join("probe.fakeprev")).is_err(),
        "시험용 도우미가 파일을 잡고 있어야 한다"
    );

    app.rename(std::ffi::OsStr::new("lock-a.fakeprev"), "renamed.fakeprev")
        .expect("곧바로 막히지 않는다");
    let edited = wait_edited(&mut app).expect("미리보기를 내리고 다시 시도해 바꾼다");
    assert_eq!(edited.path, dir.path().join("renamed.fakeprev"));
    assert!(!held.exists());
    assert!(dir.path().join("renamed.fakeprev").exists());
}

#[test]
fn deleting_a_file_the_helper_holds_releases_the_preview_and_retries() {
    let dir = folder_with(&["lock-b.fakeprev"]);
    let held = dir.path().join("lock-b.fakeprev");
    let mut app = app_at(dir.path(), true);
    select(&mut app, "lock-b.fakeprev");
    wait_body(&mut app, "lock-b.fakeprev", showing);
    assert!(
        fs::remove_file(&held).is_err(),
        "시험용 도우미가 파일을 잡고 있어야 한다"
    );

    let job = app
        .delete_permanently(vec![held.clone()])
        .expect("작업을 넣는다");
    let deadline = Instant::now() + WAIT;
    let summary = loop {
        app.poll_jobs();
        app.poll();
        let entry = app.work_area().entry(job).expect("작업 영역의 작업");
        assert!(
            entry.question.is_none(),
            "묻지 않고 미리보기를 내린 뒤 다시 시도해야 한다: {:?}",
            entry.question
        );
        if let Some(summary) = &entry.summary {
            break summary.clone();
        }
        assert!(Instant::now() < deadline, "작업이 시간 안에 끝나지 않았다");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(summary.failures.is_empty(), "{:?}", summary.failures);
    assert_eq!(summary.items_deleted, 1);
    assert!(!held.exists());
}
