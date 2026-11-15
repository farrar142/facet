//! 두 번째 실행: 이미 주인이 있으면 facet 실행 파일은 창을 띄우지 않고 인자(경로 목록)와 작업 폴더를 주인에게
//! 넘긴 뒤 종료 코드 0으로 끝난다.
//!
//! 테스트 프로세스가 실제 facet 이름(`InstanceName::facet`)으로 주인이 되어 수신기를 열고, facet 실행 파일을
//! 경로 인자와 함께 띄운다. 이 기계에서 facet이 이미 떠 있어 주인이 될 수 없으면 건너뛴다(그 facet에 창이 뜨면
//! 안 되므로 띄우지 않는다). 받은 쪽이 새 창을 여는 것은 코어 테스트(`facet-core/tests/launch.rs`)가 본다.

#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use facet_win::single_instance::{self, Claim, InstanceName};

const WAIT: Duration = Duration::from_secs(20);

/// 두 테스트가 같은 facet 이름의 주인이 되므로 하나씩 돈다.
static SERIAL: Mutex<()> = Mutex::new(());

#[test]
fn 두_번째_실행은_인자와_작업_폴더를_넘기고_종료_코드_0으로_끝난다() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let name = InstanceName::facet();
    let mut owner = match single_instance::claim(&name).expect("주인 확인") {
        Claim::Owner(owner) => owner,
        Claim::Taken => {
            eprintln!("facet이 이미 떠 있어 건너뛴다");
            return;
        }
    };
    let (sender, receiver) = mpsc::channel();
    owner
        .listen(move |launch| {
            let _ = sender.send(launch);
        })
        .expect("수신기");

    let working_dir = std::env::temp_dir();
    let started = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_facet"))
        .args(["상대 폴더", r"C:\Windows"])
        .current_dir(&working_dir)
        // 측정 모드면 단일 실행에 끼지 않으므로 꺼 둔다.
        .env_remove("FACET_MEASURE_OUT")
        .spawn()
        .expect("facet 실행");
    let status = loop {
        if let Some(status) = child.try_wait().expect("기다리기") {
            break status;
        }
        if started.elapsed() > WAIT {
            let _ = child.kill();
            panic!("두 번째 실행이 스스로 끝나지 않았다(창을 띄웠을 수 있다)");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(0), "종료 코드");

    let launch = receiver.recv_timeout(WAIT).expect("넘긴 인자");
    assert_eq!(
        launch.paths,
        vec![PathBuf::from("상대 폴더"), PathBuf::from(r"C:\Windows")],
        "상대 경로는 풀지 않고 그대로 넘긴다"
    );
    assert_eq!(
        std::fs::canonicalize(&launch.working_dir).unwrap(),
        std::fs::canonicalize(&working_dir).unwrap()
    );
}

#[test]
fn 측정_모드는_단일_실행에_끼지_않는다() {
    // 측정 모드의 앱은 주인을 찾지도 넘기지도 않으므로, 주인이 있어도 수신기에 아무것도 오지 않아야 한다. 창을
    // 띄우므로 끝나기를 기다리지 않고, 넘길 시간을 준 뒤 끝낸다.
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let name = InstanceName::facet();
    let mut owner = match single_instance::claim(&name).expect("주인 확인") {
        Claim::Owner(owner) => owner,
        Claim::Taken => {
            eprintln!("facet이 이미 떠 있어 건너뛴다");
            return;
        }
    };
    let (sender, receiver) = mpsc::channel();
    owner
        .listen(move |launch| {
            let _ = sender.send(launch);
        })
        .expect("수신기");
    let out = std::env::temp_dir().join(format!("facet-measure-test-{}.txt", std::process::id()));
    let mut child = Command::new(env!("CARGO_BIN_EXE_facet"))
        .arg(std::env::temp_dir())
        .env("FACET_MEASURE_OUT", &out)
        .spawn()
        .expect("facet 실행");
    let received = receiver.recv_timeout(Duration::from_secs(3));
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_file(&out);
    assert!(
        received.is_err(),
        "측정 모드가 주인에게 넘겼다: {received:?}"
    );
}
