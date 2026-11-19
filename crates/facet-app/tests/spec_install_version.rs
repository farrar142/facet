// target: crates/facet-app/tests/spec_install_version.rs
// AC: #468: facet --version이 버전 줄을 출력하고 종료 코드 0으로 곧바로 끝나며 facet 주인이 있어도 인자를 넘기지 않는다
//! `facet --version`을 실제 실행 파일로 확인한다: 표준 출력 첫 줄이 `facet_core::build::version_line`(테스트 빌드의
//! 채널, 실행 파일 폴더에 설치 표지가 없으니 portable)이고 종료 코드 0으로 곧바로 끝난다. 테스트 프로세스가 실제 facet
//! 이름의 주인이 되어 수신기를 열고, 그래도 `--version`이 인자를 넘기지 않는지 본다. 이 기계에서 facet이 이미 떠 있어
//! 주인이 될 수 없으면 주인 확인만 건너뛴다. Windows 전용(단일 실행 이름은 Windows에서만 뜻이 있음).

#![cfg(windows)]

use std::path::Path;
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use facet_core::build::{self, InstallKind};
use facet_win::single_instance::{self, Claim, InstanceName};

/// 창 없이 버전만 쓰고 끝나야 하는 시간(콜드 스타트보다 넉넉히).
const QUICK: Duration = Duration::from_secs(5);

fn facet_exe() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_facet"))
}

#[test]
#[ignore = "#468"]
fn 버전_인자는_버전_줄을_쓰고_곧바로_끝난다() {
    let exe = facet_exe();
    let exe_dir = exe.parent().expect("실행 파일 폴더");
    assert_eq!(
        build::install_kind(exe_dir),
        InstallKind::Portable,
        "빌드 폴더에는 설치 표지가 없다"
    );

    let started = Instant::now();
    let output = Command::new(exe)
        .arg("--version")
        .output()
        .expect("facet --version 실행");
    let took = started.elapsed();

    assert!(output.status.success(), "종료 코드: {:?}", output.status);
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 출력");
    let first = stdout.lines().next().expect("출력 한 줄");
    assert_eq!(
        first,
        build::version_line(build::channel(), InstallKind::Portable)
    );
    assert!(took < QUICK, "--version이 {took:?} 걸렸다");
}

#[test]
#[ignore = "#468"]
fn 버전_인자는_떠_있는_facet에_넘기지_않는다() {
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

    let output = Command::new(facet_exe())
        .arg("--version")
        .output()
        .expect("facet --version 실행");
    assert!(output.status.success(), "종료 코드: {:?}", output.status);
    assert!(
        String::from_utf8_lossy(&output.stdout).starts_with("facet "),
        "버전 줄을 써야 한다"
    );
    assert!(
        receiver.recv_timeout(Duration::from_secs(1)).is_err(),
        "--version은 주인에게 인자를 넘기지 않는다"
    );
}
