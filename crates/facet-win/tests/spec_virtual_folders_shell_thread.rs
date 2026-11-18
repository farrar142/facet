// target: crates/facet-win/tests/spec_virtual_folders_shell_thread.rs
// AC: #349: 전용 스레드에서 돌고 같은 스레드를 쓴다, COM STA, 시간 초과는 TimedOut이고 기다리지 않는다, 시간 초과 뒤 새 스레드·늦은 결과 버림, 패닉은 Failed
//! 셸 호출용 전용 STA 스레드(`facet_win::shell_thread::ShellThread`)를 공개 인터페이스로 확인한다: 일이 부른 스레드가
//! 아닌 한 스레드에서 돌고 그 스레드가 COM STA인지, 시간 제한을 넘은 일은 기다리지 않고 `TimedOut`으로 돌아오는지,
//! 그 뒤 요청은 멈춘 일을 기다리지 않고 새 스레드에서 돌며 멈췄던 일의 늦은 결과가 섞이지 않는지, 패닉한 일 뒤에도
//! 쓸 수 있는지. Windows 전용.
#![cfg(windows)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use facet_win::shell_thread::{CallError, ShellThread, apartment_is_sta};

const LONG: Duration = Duration::from_secs(10);

fn thread_id(shell: &ShellThread) -> ThreadId {
    shell
        .call(LONG, || thread::current().id())
        .expect("일이 돈다")
}

#[test]
fn jobs_run_on_one_dedicated_thread() {
    let shell = ShellThread::new();
    assert_eq!(shell.call(LONG, || 6 * 7), Ok(42));

    let first = thread_id(&shell);
    assert_ne!(
        first,
        thread::current().id(),
        "부른 스레드가 아닌 곳에서 돈다"
    );
    assert_eq!(thread_id(&shell), first, "버리기 전까지 같은 스레드다");
}

#[test]
fn the_thread_is_a_com_sta() {
    let shell = ShellThread::new();
    assert_eq!(shell.call(LONG, apartment_is_sta), Ok(true));
}

#[test]
fn a_stalled_job_times_out_without_waiting_for_it() {
    let shell = ShellThread::new();
    let stall = Duration::from_secs(3);
    let timeout = Duration::from_millis(200);

    let started = Instant::now();
    let result = shell.call(timeout, move || {
        thread::sleep(stall);
        1
    });
    let waited = started.elapsed();

    assert_eq!(result, Err(CallError::TimedOut));
    assert!(waited >= timeout, "제한 시간까지는 기다린다: {waited:?}");
    assert!(
        waited < Duration::from_secs(2),
        "멈춘 일을 기다리지 않는다: {waited:?}"
    );
}

#[test]
fn after_a_timeout_the_next_call_runs_at_once_on_a_new_thread() {
    let shell = ShellThread::new();
    let before = thread_id(&shell);
    let stall = Duration::from_secs(3);
    let finished = Arc::new(AtomicBool::new(false));

    let flag = Arc::clone(&finished);
    let result = shell.call(Duration::from_millis(200), move || {
        thread::sleep(stall);
        flag.store(true, Ordering::SeqCst);
        "늦은 결과"
    });
    assert_eq!(result, Err(CallError::TimedOut));

    let started = Instant::now();
    let next = shell.call(LONG, || ("새 결과", thread::current().id()));
    let waited = started.elapsed();
    let (value, after) = next.expect("다음 요청은 돈다");

    assert_eq!(value, "새 결과", "늦은 결과가 섞이지 않는다");
    assert_ne!(after, before, "멈춘 스레드는 버리고 새 스레드에서 돈다");
    assert!(
        !finished.load(Ordering::SeqCst),
        "멈춘 일이 끝나기 전에 다음 요청이 돌았다: {waited:?}"
    );

    // 멈췄던 일이 끝난 뒤에도 요청은 그 결과가 아니라 제 결과를 받는다.
    thread::sleep(stall);
    assert!(finished.load(Ordering::SeqCst));
    assert_eq!(shell.call(LONG, || "그 뒤 결과"), Ok("그 뒤 결과"));
    assert_eq!(thread_id(&shell), after, "새 스레드를 계속 쓴다");
}

#[test]
fn a_panicking_job_fails_and_the_thread_stays_usable() {
    let shell = ShellThread::new();
    let result: Result<u32, CallError> = shell.call(LONG, || panic!("시험용 패닉"));
    assert_eq!(result, Err(CallError::Failed));
    assert_eq!(shell.call(LONG, || 7), Ok(7));
    assert_eq!(shell.call(LONG, apartment_is_sta), Ok(true));
}

#[test]
fn the_thread_can_be_shared_between_threads() {
    let shell = Arc::new(ShellThread::new());
    let handles: Vec<_> = (0..4)
        .map(|n| {
            let shell = Arc::clone(&shell);
            thread::spawn(move || shell.call(LONG, move || n * 10))
        })
        .collect();
    let mut results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap().unwrap())
        .collect();
    results.sort_unstable();
    assert_eq!(results, vec![0, 10, 20, 30]);
}
