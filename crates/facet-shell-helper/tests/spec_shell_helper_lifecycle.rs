// target: crates/facet-shell-helper/tests/spec_shell_helper_lifecycle.rs
// AC: #319: 첫 요청 때 별도 프로세스로 뜸, 항목 트리 응답(항목 여럿·폴더 배경), 셸 항목 실행, 시간 초과와 늦은 응답 버림, 메뉴 놓기, 죽으면 끊김과 다시 띄우기, 버전 다름, 기본 대기 시간 2초
//! 셸 확장 도우미 관리(`facet_core::shell_helper::ShellHelper`)의 띄우기·요청과 응답·대기 시간·끊김 뒤 다시 띄우기를
//! 실제 도우미 프로세스(시험용 모드 `--fake`)와 실제 파이프로 확인한다. 셸 확장은 부르지 않는다. 실제 프로세스와 파이프를
//! 쓰므로 Windows에서만 돈다.
#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use facet_core::shell_helper::{
    HelperEvent, HelperOptions, REPLY_TIMEOUT, Reply, RequestError, RequestId, ShellHelper,
};
use facet_shell_protocol::{MenuItem, MenuTarget};
use tempfile::TempDir;

/// CI 러너가 느려도 흔들리지 않게 넉넉히 기다린다. 결과가 오면 곧바로 끝난다.
const WAIT: Duration = Duration::from_secs(20);

fn helper_program() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_facet-shell-helper"))
}

/// 시험용 모드로 띄우는 설정. 죽은 기록은 남기지 않는다(사용자 폴더를 건드리지 않게).
fn fake_options() -> HelperOptions {
    let mut options = HelperOptions::new(helper_program());
    options.args = vec!["--fake".into()];
    options.crash_log = None;
    options
}

fn file(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, b"x").unwrap();
    path
}

/// 그 요청의 결과가 올 때까지 기다린다(다른 요청의 사건은 버린다).
fn reply_for(helper: &ShellHelper, id: RequestId) -> Result<Reply, RequestError> {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "{id:?}의 결과가 오지 않았다");
        if let Some(HelperEvent::Reply { id: got, result }) = helper.wait_event(left)
            && got == id
        {
            return result;
        }
    }
}

/// 여러 요청의 결과를 모두 받는다(받은 차례와 상관없이 넘긴 차례로).
fn replies_for(helper: &ShellHelper, ids: &[RequestId]) -> Vec<Result<Reply, RequestError>> {
    let mut got: Vec<Option<Result<Reply, RequestError>>> = vec![None; ids.len()];
    let deadline = Instant::now() + WAIT;
    while got.iter().any(Option::is_none) {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "결과가 다 오지 않았다: {got:?}");
        if let Some(HelperEvent::Reply { id, result }) = helper.wait_event(left)
            && let Some(slot) = ids.iter().position(|wanted| *wanted == id)
        {
            assert!(got[slot].is_none(), "{id:?}의 결과가 두 번 왔다");
            got[slot] = Some(result);
        }
    }
    got.into_iter().map(Option::unwrap).collect()
}

/// `during` 동안 그 요청의 사건이 오지 않는가.
fn no_event_for(helper: &ShellHelper, id: RequestId, during: Duration) -> bool {
    let deadline = Instant::now() + during;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        if let Some(HelperEvent::Reply { id: got, .. }) = helper.wait_event(left)
            && got == id
        {
            return false;
        }
    }
}

fn menu_items(result: Result<Reply, RequestError>) -> Vec<MenuItem> {
    match result {
        Ok(Reply::Menu(items)) => items,
        other => panic!("항목 트리가 와야 한다: {other:?}"),
    }
}

fn first_text(items: &[MenuItem]) -> &str {
    match items.first() {
        Some(MenuItem::Command(command)) => &command.text,
        other => panic!("첫 줄은 명령 항목이어야 한다: {other:?}"),
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

fn wait_until_gone(pid: u32) -> bool {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if !is_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

#[test]
fn the_helper_is_not_started_until_the_first_request() {
    let dir = TempDir::new().unwrap();
    let a = file(dir.path(), "a.txt");
    let helper = ShellHelper::start(fake_options());
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(helper.process_id(), None, "시작만으로는 띄우지 않는다");
    assert!(helper.poll().is_empty());

    let id = helper.menu(MenuTarget::Items(vec![a]));
    let items = menu_items(reply_for(&helper, id));
    assert!(!items.is_empty());
    let pid = helper.process_id().expect("첫 요청 때 띄웠다");
    assert_ne!(pid, std::process::id(), "도우미는 별도 프로세스다");
    assert!(is_alive(pid));
}

#[test]
fn menu_replies_carry_the_helpers_item_tree() {
    let dir = TempDir::new().unwrap();
    let a = file(dir.path(), "a.txt");
    let b = file(dir.path(), "b.txt");
    let helper = ShellHelper::start(fake_options());

    let id = helper.menu(MenuTarget::Items(vec![a, b]));
    let items = menu_items(reply_for(&helper, id));
    let [
        MenuItem::Command(first),
        MenuItem::Separator,
        MenuItem::Submenu(sub),
        MenuItem::Command(off),
    ] = &items[..]
    else {
        panic!("시험용 트리의 모양이 아니다: {items:?}");
    };
    assert_eq!(first.id, 1);
    assert_eq!(first.text, "fake: a.txt|b.txt", "선택 전체가 목록 순서로");
    assert!(first.enabled);
    assert!(first.icon.is_some(), "아이콘 그림이 온다");
    assert_eq!(sub.text, "fake-submenu");
    assert!(sub.enabled);
    let [MenuItem::Command(child)] = &sub.items[..] else {
        panic!("하위 메뉴에 항목 하나: {:?}", sub.items);
    };
    assert_eq!((child.id, child.text.as_str()), (2, "fake-child"));
    assert!(child.enabled);
    assert_eq!((off.id, off.text.as_str()), (3, "fake-disabled"));
    assert!(!off.enabled, "꺼진 항목은 꺼진 채 온다");

    let sub_dir = dir.path().join("sub");
    fs::create_dir(&sub_dir).unwrap();
    let id = helper.menu(MenuTarget::Background(sub_dir));
    let items = menu_items(reply_for(&helper, id));
    assert_eq!(first_text(&items), "fake-background: sub");
}

#[test]
fn invoking_an_item_runs_it_in_the_helper() {
    let dir = TempDir::new().unwrap();
    let a = file(dir.path(), "a.txt");
    let b = file(dir.path(), "b.txt");
    let helper = ShellHelper::start(fake_options());

    let menu = helper.menu(MenuTarget::Items(vec![a, b]));
    menu_items(reply_for(&helper, menu));
    let invoke = helper.invoke(menu, 1);
    assert_eq!(reply_for(&helper, invoke), Ok(Reply::Done));
    let marker = fs::read_to_string(dir.path().join("facet-fake-invoked.txt")).unwrap();
    assert_eq!(marker.lines().collect::<Vec<_>>(), ["1", "a.txt", "b.txt"]);

    let sub_dir = dir.path().join("sub");
    fs::create_dir(&sub_dir).unwrap();
    let menu = helper.menu(MenuTarget::Background(sub_dir.clone()));
    menu_items(reply_for(&helper, menu));
    let invoke = helper.invoke(menu, 2);
    assert_eq!(reply_for(&helper, invoke), Ok(Reply::Done));
    let marker = fs::read_to_string(sub_dir.join("facet-fake-invoked.txt")).unwrap();
    assert_eq!(marker.lines().collect::<Vec<_>>(), ["2", "sub"]);
}

#[test]
fn a_reply_slower_than_the_timeout_fails_and_the_late_reply_is_dropped() {
    let dir = TempDir::new().unwrap();
    let late = file(dir.path(), "late-1500");
    let a = file(dir.path(), "a.txt");
    let mut options = fake_options();
    options.reply_timeout = Duration::from_millis(300);
    let helper = ShellHelper::start(options);
    // 도우미를 먼저 띄워 두어 띄우는 시간이 대기 시간에 섞이지 않게 한다.
    let warm = helper.menu(MenuTarget::Items(vec![a.clone()]));
    menu_items(reply_for(&helper, warm));

    let started = Instant::now();
    let id = helper.menu(MenuTarget::Items(vec![late]));
    assert_eq!(reply_for(&helper, id), Err(RequestError::TimedOut));
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(250) && elapsed < Duration::from_millis(1400),
        "대기 시간에 끝난다: {elapsed:?}"
    );
    assert!(
        no_event_for(&helper, id, Duration::from_millis(2500)),
        "늦게 온 응답은 버린다"
    );

    let next = helper.menu(MenuTarget::Items(vec![a]));
    let items = menu_items(reply_for(&helper, next));
    assert_eq!(
        first_text(&items),
        "fake: a.txt",
        "다음 요청은 제 응답을 받는다"
    );
}

#[test]
fn releasing_a_menu_drops_its_pending_request() {
    let dir = TempDir::new().unwrap();
    let late = file(dir.path(), "late-800");
    let a = file(dir.path(), "a.txt");
    let helper = ShellHelper::start(fake_options());

    let id = helper.menu(MenuTarget::Items(vec![late]));
    helper.release(id);
    assert!(
        no_event_for(&helper, id, Duration::from_secs(2)),
        "놓은 메뉴의 결과는 오지 않는다"
    );

    let next = helper.menu(MenuTarget::Items(vec![a]));
    let items = menu_items(reply_for(&helper, next));
    assert_eq!(first_text(&items), "fake: a.txt");
}

#[test]
fn a_crash_fails_every_pending_request_and_the_next_request_starts_a_new_helper() {
    let dir = TempDir::new().unwrap();
    let a = file(dir.path(), "a.txt");
    let crash = file(dir.path(), "crash");
    let mut options = fake_options();
    // 끊김이 대기 시간보다 먼저 알려지는지 보려고 대기 시간을 길게 둔다.
    options.reply_timeout = Duration::from_secs(15);
    let helper = ShellHelper::start(options);

    let first = helper.menu(MenuTarget::Items(vec![a.clone()]));
    menu_items(reply_for(&helper, first));
    let old = helper.process_id().unwrap();

    let started = Instant::now();
    let crashing = helper.menu(MenuTarget::Items(vec![crash]));
    let behind = helper.menu(MenuTarget::Items(vec![a.clone()]));
    let results = replies_for(&helper, &[crashing, behind]);
    assert_eq!(
        results,
        [
            Err(RequestError::Disconnected),
            Err(RequestError::Disconnected)
        ],
        "기다리던 요청이 모두 끊김으로 끝난다"
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "대기 시간을 기다리지 않는다: {:?}",
        started.elapsed()
    );
    assert!(wait_until_gone(old), "죽은 도우미는 남지 않는다");

    let next = helper.menu(MenuTarget::Items(vec![a]));
    let items = menu_items(reply_for(&helper, next));
    assert_eq!(first_text(&items), "fake: a.txt");
    let new = helper.process_id().unwrap();
    assert_ne!(new, old, "새 도우미가 떴다");
    assert!(is_alive(new));
}

#[test]
fn a_helper_with_another_protocol_version_is_not_used() {
    let dir = TempDir::new().unwrap();
    let a = file(dir.path(), "a.txt");
    let mut options = fake_options();
    options.args = vec!["--fake".into(), "--fake-version".into(), "999".into()];
    let helper = ShellHelper::start(options);

    let id = helper.menu(MenuTarget::Items(vec![a]));
    assert_eq!(reply_for(&helper, id), Err(RequestError::VersionMismatch));
}

#[test]
fn the_default_reply_timeout_is_two_seconds() {
    assert_eq!(REPLY_TIMEOUT, Duration::from_secs(2));
    let options = HelperOptions::new(helper_program());
    assert_eq!(options.reply_timeout, REPLY_TIMEOUT);
    assert_eq!(options.program, helper_program());
    assert!(options.args.is_empty());
    assert_eq!(options.parent, None);
}
