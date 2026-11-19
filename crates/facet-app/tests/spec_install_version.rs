// target: crates/facet-app/tests/spec_install_version.rs
// AC: #468: facet --version이 버전 줄을 출력하고 종료 코드 0으로 곧바로 끝나며 facet 주인이 있어도 인자를 넘기지 않는다
//! `facet --version`을 실제 실행 파일로 확인한다: 표준 출력 첫 줄이 `facet_core::build::version_line`(테스트 빌드의
//! 채널, 실행 파일 폴더에 설치 표지가 없으니 portable)이고 종료 코드 0으로 곧바로 끝난다. 테스트 프로세스가 실제 facet
//! 이름의 주인이 되어 수신기를 열고, 그래도 `--version`이 인자를 넘기지 않는지 본다. 이 기계에서 facet이 이미 떠 있어
//! 주인이 될 수 없으면 주인 확인만 건너뛴다. Windows 전용(단일 실행 이름은 Windows에서만 뜻이 있음).

#![cfg(windows)]

use std::io::Read;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use facet_core::build::{self, InstallKind};
use facet_win::single_instance::{self, Claim, InstanceName};

/// 창 없이 버전만 쓰고 끝나야 하는 시간(콜드 스타트보다 넉넉히).
const QUICK: Duration = Duration::from_secs(5);

fn facet_exe() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_facet"))
}

/// `facet --version`을 띄워 끝나기를 `QUICK`까지 기다린다(창을 띄우고 끝나지 않으면 끝내고 실패). 종료 상태, 표준 출력,
/// 걸린 시간.
fn run_version() -> (ExitStatus, String, Duration) {
    let started = Instant::now();
    let mut child = Command::new(facet_exe())
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("facet --version 실행");
    loop {
        if let Some(status) = child.try_wait().expect("기다리기") {
            let mut stdout = String::new();
            if let Some(mut pipe) = child.stdout.take() {
                let _ = pipe.read_to_string(&mut stdout);
            }
            return (status, stdout, started.elapsed());
        }
        if started.elapsed() > QUICK {
            let _ = child.kill();
            let _ = child.wait();
            panic!("--version이 {QUICK:?} 안에 끝나지 않았다(창을 띄웠나)");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn version_flag_prints_the_version_line_and_exits_at_once() {
    let exe_dir = facet_exe().parent().expect("실행 파일 폴더");
    assert_eq!(
        build::install_kind(exe_dir),
        InstallKind::Portable,
        "빌드 폴더에는 설치 표지가 없다"
    );

    let (status, stdout, took) = run_version();

    assert!(status.success(), "종료 코드: {status:?}");
    assert_eq!(
        stdout.lines().next().unwrap_or(""),
        build::version_line(build::channel(), InstallKind::Portable)
    );
    assert!(took < QUICK, "--version이 {took:?} 걸렸다");
}

#[test]
fn version_flag_does_not_forward_to_a_running_facet() {
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

    let (status, stdout, _) = run_version();
    assert!(status.success(), "종료 코드: {status:?}");
    assert!(stdout.starts_with("facet "), "버전 줄을 써야 한다");
    assert!(
        receiver.recv_timeout(Duration::from_secs(1)).is_err(),
        "--version은 주인에게 인자를 넘기지 않는다"
    );
}
