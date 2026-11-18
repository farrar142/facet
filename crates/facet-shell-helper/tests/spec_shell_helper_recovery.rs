// target: crates/facet-shell-helper/tests/spec_shell_helper_recovery.rs
// AC: #320: 멈춤 판정 뒤 끝내고 다시 띄우기, 세 번까지는 쉬지 않고 네 번째에 쉬기와 쉬는 시간 뒤 다시 답하기, 쉬면 끝내기(죽음으로 세지 않음), 부모가 끝나면 끝나기, 죽을 때마다 기록 줄, 같은 확장으로 세 번이면 알림, 기본값
//! 셸 확장 도우미의 수명 관리(멈춤·연달아 죽음·쉬면 끝내기·부모 감시·죽게 한 확장 기록)를 실제 도우미 프로세스
//! (시험용 모드 `--fake`의 `hang`·`crash`와 가짜 확장)와 실제 파이프로 확인한다. 시간은 설정으로 줄여 잰다. 기본값
//! 검사도 이 파일에 있다. 실제 프로세스를 쓰므로 Windows에서만 돈다.
#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use facet_core::shell_helper::{
    CRASH_LIMIT, CRASH_WINDOW, CrashedExtension, HelperEvent, HelperOptions, IDLE_TIMEOUT,
    REST_DURATION, Reply, RequestError, RequestId, SAME_EXTENSION_CRASHES, STALL_TIMEOUT,
    ShellHelper, default_crash_log,
};
use facet_shell_protocol::MenuTarget;
use tempfile::TempDir;

/// CI 러너가 느려도 흔들리지 않게 넉넉히 기다린다. 결과가 오면 곧바로 끝난다.
const WAIT: Duration = Duration::from_secs(20);

const FAKE_CLSID: &str = "{FACE0000-0000-0000-0000-000000000001}";
const FAKE_DLL: &str = r"C:\fake\fake-extension.dll";
const FAKE_NAME: &str = "Fake Extension";

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

fn assert_menu(helper: &ShellHelper, target: &Path) {
    let id = helper.menu(MenuTarget::Items(vec![target.to_path_buf()]));
    let result = reply_for(helper, id);
    assert!(
        matches!(result, Ok(Reply::Menu(ref items)) if !items.is_empty()),
        "항목 트리가 와야 한다: {result:?}"
    );
}

/// `crash` 대상으로 도우미 하나를 죽게 한다. 죽은 도우미의 프로세스 번호.
fn crash_once(helper: &ShellHelper, crash: &Path) -> u32 {
    let id = helper.menu(MenuTarget::Items(vec![crash.to_path_buf()]));
    assert_eq!(reply_for(helper, id), Err(RequestError::Disconnected));
    let pid = helper.process_id().expect("띄웠던 도우미");
    assert!(wait_until_gone(pid), "죽은 도우미는 남지 않는다");
    pid
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

/// 기록 파일에 줄이 `count`개 생길 때까지 기다려 그 줄들을 돌려준다.
fn wait_for_log_lines(log: &Path, count: usize) -> Vec<String> {
    let deadline = Instant::now() + WAIT;
    loop {
        let lines: Vec<String> = fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::to_owned)
            .collect();
        if lines.len() >= count {
            return lines;
        }
        assert!(
            Instant::now() < deadline,
            "기록 줄이 {count}개 생기지 않았다: {lines:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 줄이 지역 시각 `YYYY-MM-DD HH:MM:SS`로 시작하는가.
fn starts_with_local_time(line: &str) -> bool {
    let pattern = "dddd-dd-dd dd:dd:dd";
    line.len() >= pattern.len()
        && pattern
            .bytes()
            .zip(line.bytes())
            .all(|(want, got)| match want {
                b'd' => got.is_ascii_digit(),
                other => other == got,
            })
}

#[test]
fn a_stalled_helper_is_ended_and_the_next_request_gets_a_new_one() {
    let dir = TempDir::new().unwrap();
    let a = file(dir.path(), "a.txt");
    let hang = file(dir.path(), "hang");
    let mut options = fake_options();
    options.reply_timeout = Duration::from_millis(300);
    options.stall_timeout = Duration::from_millis(1500);
    let helper = ShellHelper::start(options);
    assert_menu(&helper, &a);
    let stalled = helper.process_id().unwrap();

    let id = helper.menu(MenuTarget::Items(vec![hang]));
    assert_eq!(reply_for(&helper, id), Err(RequestError::TimedOut));
    assert!(
        wait_until_gone(stalled),
        "멈춘 도우미는 멈춤 판정 시간이 지나면 끝난다"
    );

    assert_menu(&helper, &a);
    let fresh = helper.process_id().unwrap();
    assert_ne!(fresh, stalled, "새 도우미가 답한다");
    assert!(is_alive(fresh));
}

#[test]
fn three_crashes_in_a_minute_do_not_rest_the_helper() {
    let dir = TempDir::new().unwrap();
    let a = file(dir.path(), "a.txt");
    let crash = file(dir.path(), "crash");
    let helper = ShellHelper::start(fake_options());

    for _ in 0..3 {
        crash_once(&helper, &crash);
    }
    assert!(!helper.is_resting(), "세 번까지는 쉬지 않는다");
    assert_menu(&helper, &a);
}

#[test]
fn a_fourth_crash_in_a_minute_rests_the_helper_until_the_rest_is_over() {
    let dir = TempDir::new().unwrap();
    let a = file(dir.path(), "a.txt");
    let crash = file(dir.path(), "crash");
    let mut options = fake_options();
    options.rest = Duration::from_secs(3);
    let helper = ShellHelper::start(options);

    let mut last = 0;
    for _ in 0..4 {
        last = crash_once(&helper, &crash);
    }
    let rest_began = Instant::now();
    assert!(helper.is_resting(), "1분 안에 네 번 죽으면 쉰다");

    let started = Instant::now();
    let id = helper.menu(MenuTarget::Items(vec![a.clone()]));
    assert_eq!(reply_for(&helper, id), Err(RequestError::Resting));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "쉬는 동안의 요청은 곧바로 끝난다: {:?}",
        started.elapsed()
    );
    assert_eq!(helper.process_id(), Some(last), "쉬는 동안 띄우지 않는다");

    std::thread::sleep(Duration::from_secs(3).saturating_sub(rest_began.elapsed()));
    let deadline = Instant::now() + WAIT;
    loop {
        let id = helper.menu(MenuTarget::Items(vec![a.clone()]));
        match reply_for(&helper, id) {
            Ok(Reply::Menu(items)) => {
                assert!(!items.is_empty());
                break;
            }
            Err(RequestError::Resting) => {
                assert!(Instant::now() < deadline, "쉬는 시간이 끝나지 않았다");
                std::thread::sleep(Duration::from_millis(100));
            }
            other => panic!("쉬는 시간 뒤에는 다시 답해야 한다: {other:?}"),
        }
    }
    assert!(!helper.is_resting());
    assert_ne!(helper.process_id(), Some(last), "쉬는 시간 뒤 새로 띄웠다");
}

#[test]
fn an_idle_helper_ends_by_itself_and_comes_back_on_demand() {
    let dir = TempDir::new().unwrap();
    let a = file(dir.path(), "a.txt");
    let mut options = fake_options();
    options.idle_timeout = Duration::from_millis(500);
    // 죽음을 한 번이라도 세면 곧바로 쉬게 해, 쉬어서 끝난 것을 죽음으로 세지 않는지 본다.
    options.crash_limit = 0;
    let helper = ShellHelper::start(options);

    assert_menu(&helper, &a);
    let idle = helper.process_id().unwrap();
    assert!(wait_until_gone(idle), "쉬면 스스로 끝난다");
    assert!(!helper.is_resting(), "쉬어서 끝난 것은 죽음이 아니다");

    assert_menu(&helper, &a);
    let fresh = helper.process_id().unwrap();
    assert_ne!(fresh, idle, "다음 요청 때 다시 떴다");
}

#[test]
fn the_helper_ends_when_its_parent_ends() {
    let dir = TempDir::new().unwrap();
    let a = file(dir.path(), "a.txt");
    let mut parent = Command::new("cmd")
        .args(["/C", "ping -n 120 127.0.0.1 > NUL"])
        .spawn()
        .expect("부모 대신 오래 도는 프로세스");
    let mut options = fake_options();
    options.parent = Some(parent.id());
    let helper = ShellHelper::start(options);

    assert_menu(&helper, &a);
    let pid = helper.process_id().unwrap();
    assert!(is_alive(pid));

    parent.kill().unwrap();
    parent.wait().unwrap();
    assert!(wait_until_gone(pid), "부모가 끝나면 도우미도 끝난다");
}

#[test]
fn each_crash_is_logged_and_three_crashes_of_one_extension_are_reported() {
    let dir = TempDir::new().unwrap();
    let crash = file(dir.path(), "crash");
    // 없는 폴더 안의 기록 파일: 처음 쓸 때 폴더를 만든다.
    let log = dir.path().join("logs").join("helper-crashes.txt");
    let mut options = fake_options();
    options.crash_log = Some(log.clone());
    let helper = ShellHelper::start(options);

    for count in 1..=3 {
        assert_eq!(
            helper.crashing_extension(),
            None,
            "{}번 죽은 것으로는 알리지 않는다",
            count - 1
        );
        crash_once(&helper, &crash);
        let lines = wait_for_log_lines(&log, count);
        assert_eq!(lines.len(), count, "죽을 때마다 한 줄: {lines:?}");
        let line = &lines[count - 1];
        assert!(starts_with_local_time(line), "시각으로 시작한다: {line}");
        assert!(line.contains(FAKE_CLSID), "CLSID: {line}");
        assert!(line.contains(FAKE_DLL), "DLL 경로: {line}");
    }

    let deadline = Instant::now() + WAIT;
    let reported = loop {
        if let Some(extension) = helper.crashing_extension() {
            break extension;
        }
        assert!(
            Instant::now() < deadline,
            "세 번 죽게 한 확장을 알리지 않았다"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(
        reported,
        CrashedExtension {
            clsid: FAKE_CLSID.into(),
            dll: PathBuf::from(FAKE_DLL),
            name: Some(FAKE_NAME.into()),
            crashes: 3,
        }
    );
}

#[test]
fn the_lifetime_defaults_are_the_specs() {
    assert_eq!(STALL_TIMEOUT, Duration::from_secs(10));
    assert_eq!(IDLE_TIMEOUT, Duration::from_secs(5 * 60));
    assert_eq!(CRASH_WINDOW, Duration::from_secs(60));
    assert_eq!(CRASH_LIMIT, 3);
    assert_eq!(REST_DURATION, Duration::from_secs(60));
    assert_eq!(SAME_EXTENSION_CRASHES, 3);

    let options = HelperOptions::new(helper_program());
    assert_eq!(options.stall_timeout, STALL_TIMEOUT);
    assert_eq!(options.idle_timeout, IDLE_TIMEOUT);
    assert_eq!(options.crash_window, CRASH_WINDOW);
    assert_eq!(options.crash_limit, CRASH_LIMIT);
    assert_eq!(options.rest, REST_DURATION);
    assert_eq!(options.crash_log, default_crash_log());

    let local = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA");
    assert_eq!(
        default_crash_log(),
        Some(
            PathBuf::from(local)
                .join("facet")
                .join("helper-crashes.txt")
        )
    );
}
