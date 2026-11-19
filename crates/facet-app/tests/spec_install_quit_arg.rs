// target: crates/facet-app/tests/spec_install_quit_arg.rs
// AC: #472: `facet --request-quit`이 떠 있는 facet 없이 종료 코드 0으로 곧바로 끝난다(창을 띄우지 않음)
//! 설치 프로그램이 부르는 `facet --request-quit`을 실제 실행 파일로 확인한다: 떠 있는 facet이 없으면 창을 띄우지 않고
//! 종료 코드 0으로 곧바로 끝난다. 이 기계에서 facet이 이미 떠 있으면(그 facet을 끝내지 않게) 건너뛴다. 작업이 있는
//! facet의 거절(종료 코드 3)은 손 확인이다. Windows 전용(단일 실행 이름은 Windows에서만 뜻이 있음).

#![cfg(windows)]

use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use facet_win::single_instance::{self, Claim, InstanceName};

/// 창 없이 답하고 끝나야 하는 시간(콜드 스타트보다 넉넉히).
const QUICK: Duration = Duration::from_secs(5);

#[test]
#[ignore = "#472"]
fn request_quit_without_a_running_facet_exits_zero_at_once() {
    match single_instance::claim(&InstanceName::facet()).expect("주인 확인") {
        // 주인 자리를 곧바로 내놓아 떠 있는 facet이 없는 상태로 둔다.
        Claim::Owner(owner) => drop(owner),
        Claim::Taken => {
            eprintln!("facet이 이미 떠 있어 건너뛴다");
            return;
        }
    }

    let started = Instant::now();
    let mut child = Command::new(Path::new(env!("CARGO_BIN_EXE_facet")))
        .arg("--request-quit")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("facet --request-quit 실행");
    let status = loop {
        if let Some(status) = child.try_wait().expect("기다리기") {
            break status;
        }
        if started.elapsed() > QUICK {
            let _ = child.kill();
            let _ = child.wait();
            panic!("--request-quit이 {QUICK:?} 안에 끝나지 않았다(창을 띄웠나)");
        }
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(0), "떠 있는 facet이 없으면 0");
}
