//! 파이프 클라이언트(`pipe_client`): 같은 프로세스의 시험용 받는 쪽에 붙어 본다. Windows 전용.
#![cfg(windows)]

use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use facet_win::pipe_client::testing::Listener;
use facet_win::pipe_client::{ConnectError, connect};

/// 읽기와 데이터 쓰기 권한(인덱서 프로토콜의 `PIPE_CLIENT_ACCESS`와 같음).
const ACCESS: u32 = 0x0012_0089 | 0x0002;

fn unique_pipe() -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        r"\\.\pipe\facet-win-pipe-client-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

#[test]
fn missing_pipe_is_not_found_at_once() {
    let started = Instant::now();
    let result = connect(&unique_pipe(), ACCESS, Duration::from_secs(5));
    assert!(matches!(result, Err(ConnectError::NotFound)));
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn write_while_another_thread_waits_to_read() {
    let pipe = unique_pipe();
    let listener = Listener::bind(&pipe).unwrap();
    let client = connect(&pipe, ACCESS, Duration::from_secs(5)).unwrap();
    let server = listener.accept(Duration::from_secs(5)).unwrap().unwrap();
    let (mut c_reader, mut c_writer, _c_close) = client.split().unwrap();
    let (mut s_reader, mut s_writer, _s_close) = server.split().unwrap();

    // 클라이언트가 응답을 기다리며 읽는 동안 같은 연결로 쓴다(동기 핸들이면 여기서 막힌다).
    let reading = thread::spawn(move || {
        let mut got = [0u8; 5];
        c_reader.read_exact(&mut got).unwrap();
        got
    });
    thread::sleep(Duration::from_millis(50));
    c_writer.write_all(b"query").unwrap();
    let mut got = [0u8; 5];
    s_reader.read_exact(&mut got).unwrap();
    assert_eq!(&got, b"query");
    s_writer.write_all(b"reply").unwrap();
    assert_eq!(&reading.join().unwrap(), b"reply");
}

#[test]
fn read_past_the_deadline_times_out_and_close_wakes_a_waiting_reader() {
    let pipe = unique_pipe();
    let listener = Listener::bind(&pipe).unwrap();
    let client = connect(&pipe, ACCESS, Duration::from_secs(5)).unwrap();
    let server = listener.accept(Duration::from_secs(5)).unwrap().unwrap();
    let (mut reader, _writer, closer) = client.split().unwrap();

    reader.set_deadline(Some(Instant::now() + Duration::from_millis(100)));
    let started = Instant::now();
    let error = reader.read(&mut [0u8; 4]).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::TimedOut);
    assert!(started.elapsed() >= Duration::from_millis(90));

    reader.set_deadline(None);
    let waiting = thread::spawn(move || reader.read(&mut [0u8; 4]).unwrap_err().kind());
    thread::sleep(Duration::from_millis(50));
    closer.close();
    assert_eq!(waiting.join().unwrap(), ErrorKind::ConnectionAborted);
    drop(server);
}

#[test]
fn server_closing_is_end_of_file() {
    let pipe = unique_pipe();
    let listener = Listener::bind(&pipe).unwrap();
    let client = connect(&pipe, ACCESS, Duration::from_secs(5)).unwrap();
    let server = listener.accept(Duration::from_secs(5)).unwrap().unwrap();
    let (mut reader, _writer, _closer) = client.split().unwrap();
    drop(server);
    assert_eq!(reader.read(&mut [0u8; 4]).unwrap(), 0);
}
