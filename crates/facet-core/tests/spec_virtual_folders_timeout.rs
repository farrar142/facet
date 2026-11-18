// target: crates/facet-core/tests/spec_virtual_folders_timeout.rs
// AC: #355: 기본 시간 제한 10초, 열거가 멈추면 원래 위치에 남고 「응답 없음」, 늦은 결과 무시, 다시 시도는 새 스레드에서 곧바로, 다른 탭의 실제 폴더 이동은 곧바로 끝남
//! 셸 공급자 호출의 시간 제한을 공개 인터페이스(`App`·`Tab`)와 시험용 주입(`shell_provider::fault::stall_listing`)으로
//! 확인한다: 내 PC 열거가 응답하지 않으면 제한 시간 언저리에 탭이 원래 위치에 남고 까닭이 「응답 없음」인지, 멈춘 열거가
//! 뒤늦게 끝나도 탭이 옮겨 가지 않는지, 「다시 시도」가 멈춘 열거를 기다리지 않고 새 스레드에서 읽는지, 그동안 다른 탭의
//! 실제 폴더 읽기가 곧바로 끝나는지. 시간 제한은 프로세스 전체 값이라 이 파일의 테스트는 차례로 돌고 끝나면 기본값으로
//! 되돌린다. Windows 전용.
#![cfg(windows)]

use std::fs;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use facet_core::command::CommandId;
use facet_core::shell_provider::{self, CALL_TIMEOUT, fault};
use facet_core::tab::ReadOutcome;
use facet_core::window::{App, Execution};
use facet_core::{ListError, Location};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(30);

/// 시험에서 쓰는 짧은 시간 제한.
const SHORT_TIMEOUT: Duration = Duration::from_millis(300);

/// 이 파일의 테스트를 차례로 돌린다(시간 제한이 프로세스 전체 값이다).
static SERIAL: Mutex<()> = Mutex::new(());

/// 시간 제한을 짧게 바꾸고, 버릴 때 기본값으로 되돌린다.
struct ShortTimeout {
    _serial: MutexGuard<'static, ()>,
}

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl ShortTimeout {
    fn set() -> Self {
        let serial = serial();
        shell_provider::set_call_timeout(SHORT_TIMEOUT);
        ShortTimeout { _serial: serial }
    }
}

impl Drop for ShortTimeout {
    fn drop(&mut self) {
        shell_provider::set_call_timeout(CALL_TIMEOUT);
    }
}

fn app_at(location: Location) -> App {
    let mut app = App::new(Some(location));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

/// 활성 탭의 읽기가 실패해 끝날 때까지 받는다.
fn wait_failure(app: &mut App) {
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll();
        let tab = app.active_tab();
        if tab.pending_location().is_none() && tab.error().is_some() {
            return;
        }
        assert!(
            tab.location() != &Location::this_pc(),
            "멈춘 열거로 내 PC에 가면 안 된다"
        );
        assert!(Instant::now() < deadline, "읽기가 실패로 끝나지 않았다");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// 활성 탭이 `expected`에 닿아 목록을 다 읽을 때까지 받는다.
fn settle(app: &mut App, expected: &Location) {
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll();
        let tab = app.active_tab();
        if tab.pending_location().is_none()
            && tab.location() == expected
            && tab.loading().is_none()
            && tab.error().is_none()
        {
            return;
        }
        assert!(Instant::now() < deadline, "{expected}에 닿지 못했다");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn assert_not_responding(app: &App) {
    match app.active_tab().error() {
        Some(ListError::NotResponding { location }) => {
            assert_eq!(location, &Location::this_pc());
        }
        other => panic!("「응답 없음」이어야 한다: {other:?}"),
    }
    assert_eq!(
        app.active_tab().error().unwrap().location(),
        &Location::this_pc()
    );
}

#[test]
#[ignore = "#355"]
fn the_default_timeout_is_ten_seconds() {
    assert_eq!(CALL_TIMEOUT, Duration::from_secs(10));
}

#[test]
#[ignore = "#355"]
fn a_stalled_listing_times_out_and_its_late_result_is_ignored() {
    let _timeout = ShortTimeout::set();
    let dir = TempDir::new().unwrap();
    let start = Location::file_system(dir.path());
    let mut app = app_at(start.clone());
    let stall = Duration::from_secs(3);
    let _stall = fault::stall_listing(&Location::this_pc(), stall);

    let started = Instant::now();
    assert_eq!(app.execute(CommandId::OpenThisPc), Execution::Done);
    wait_failure(&mut app);
    let waited = started.elapsed();

    assert!(
        waited >= SHORT_TIMEOUT,
        "제한 시간까지는 기다린다: {waited:?}"
    );
    assert!(
        waited < Duration::from_secs(2),
        "멈춘 열거를 기다리지 않는다: {waited:?}"
    );
    assert_not_responding(&app);
    assert_eq!(app.active_tab().location(), &start, "원래 위치에 남는다");

    // 멈춘 열거가 끝난 뒤에도 그 결과는 탭에 들어가지 않는다.
    std::thread::sleep(stall + Duration::from_secs(1));
    for _ in 0..20 {
        app.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(app.active_tab().location(), &start, "늦은 결과는 버린다");
    assert_not_responding(&app);
}

#[test]
#[ignore = "#355"]
fn retry_reads_again_at_once_on_a_new_thread() {
    let _timeout = ShortTimeout::set();
    let dir = TempDir::new().unwrap();
    let mut app = app_at(Location::file_system(dir.path()));
    let tab = app.active_tab_id();
    let stall = Duration::from_secs(6);
    let injected = fault::stall_listing(&Location::this_pc(), stall);

    let started = Instant::now();
    assert_eq!(app.execute(CommandId::OpenThisPc), Execution::Done);
    wait_failure(&mut app);
    assert_not_responding(&app);
    drop(injected);

    assert!(app.retry(tab), "실패한 읽기를 다시 한다");
    settle(&mut app, &Location::this_pc());
    assert!(
        started.elapsed() < stall,
        "멈춘 열거가 끝나기를 기다리지 않았다: {:?}",
        started.elapsed()
    );
    assert!(!app.active_tab().is_empty(), "드라이브가 들어왔다");
    assert!(!app.retry(tab), "다시 할 읽기가 없다");
}

#[test]
#[ignore = "#355"]
fn other_tabs_keep_reading_real_folders_while_the_shell_is_stalled() {
    // 기본 시간 제한(10초) 그대로: 멈춘 셸 호출이 도는 동안을 본다.
    let _serial = serial();
    let dir = TempDir::new().unwrap();
    let other = dir.path().join("다른 폴더");
    fs::create_dir(&other).unwrap();
    fs::write(other.join("파일.txt"), "a").unwrap();
    let mut app = app_at(Location::file_system(dir.path()));
    let stalled_tab = app.active_tab_id();
    let stall = Duration::from_secs(5);
    let _stall = fault::stall_listing(&Location::this_pc(), stall);

    assert_eq!(app.execute(CommandId::OpenThisPc), Execution::Done);
    let started = Instant::now();
    let opened = app.open_tab(Location::file_system(&other), true);
    loop {
        let (tab, outcome) = app.wait(WAIT).expect("읽기 결과");
        if tab == opened {
            assert_eq!(outcome, ReadOutcome::Committed);
            break;
        }
        assert_eq!(tab, stalled_tab, "다른 탭의 결과는 멈춘 탭뿐이다");
    }
    let waited = started.elapsed();
    assert!(
        waited < Duration::from_secs(1),
        "셸이 멈춘 동안에도 실제 폴더는 곧바로 읽힌다: {waited:?}"
    );
    assert_eq!(app.active_tab().location(), &Location::file_system(other));
    assert_eq!(app.active_tab().len(), 1);
}
