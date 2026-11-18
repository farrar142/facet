// target: crates/facet-bench/tests/spec_shell_helper_bench.rs
// AC: #324: 로컬 기준 5ms·CI 상한 10배, 중앙값이 상한을 넘으면 수치를 담아 실패, 작은 회차로 재면 회차 수·응답 줄 수·도우미가 남지 않음(Windows)
//! 셸 확장 도우미 요청 왕복 벤치마크(`measure_shell_helper`)의 상수와 한도 판정, 작은 회차 수로 한 번 돌린 측정을
//! 확인한다. 상수와 판정은 모든 환경, 실제 측정(시험용 모드 도우미 프로세스)은 Windows에서만 돈다. 도우미 실행 파일은
//! 이 테스트 실행 파일과 같은 빌드 폴더(`target/<프로필>/facet-shell-helper.exe`)에서 찾고, 없으면 건너뛰었다고 출력한다.

use std::time::Duration;

use facet_bench::{Measurement, SHELL_HELPER_LIMIT, SHELL_HELPER_TARGET, check_shell_helper_limit};

#[test]
fn the_limit_is_ten_times_the_five_millisecond_target() {
    assert_eq!(SHELL_HELPER_TARGET, Duration::from_millis(5));
    assert_eq!(SHELL_HELPER_LIMIT, SHELL_HELPER_TARGET * 10);
}

#[test]
fn a_median_over_the_limit_fails_with_the_numbers() {
    let ms = Duration::from_millis;
    let m = Measurement {
        item_count: 4,
        times: vec![ms(1), ms(20), ms(90)],
    };
    assert!(check_shell_helper_limit(&m, ms(20)).is_ok());
    assert!(check_shell_helper_limit(&m, ms(50)).is_ok());
    let err = check_shell_helper_limit(&m, ms(15)).unwrap_err();
    assert!(
        err.contains("20") && err.contains("15"),
        "중앙값과 상한이 보여야 한다: {err}"
    );
}

#[cfg(windows)]
#[test]
fn a_small_run_measures_each_round_trip_and_leaves_no_helper() {
    let exe = std::env::current_exe().unwrap();
    // target/<프로필>/deps/이 테스트.exe → target/<프로필>/facet-shell-helper.exe
    let helper = exe
        .parent()
        .and_then(|deps| deps.parent())
        .map(|profile| profile.join("facet-shell-helper.exe"))
        .unwrap();
    if !helper.exists() {
        println!(
            "도우미 실행 파일이 없어 건너뜀: {} (cargo test --workspace가 함께 빌드한다)",
            helper.display()
        );
        return;
    }
    let before = helper_processes();

    let m = facet_bench::measure_shell_helper(&helper, 3).expect("측정한다");
    assert_eq!(m.times.len(), 3, "회차 수만큼");
    assert_eq!(m.item_count, 4, "시험용 트리의 맨 위 줄 수");
    assert!(m.times.iter().all(|t| *t > Duration::ZERO));

    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while helper_processes() > before {
        assert!(
            std::time::Instant::now() < deadline,
            "측정이 띄운 도우미가 남았다"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 떠 있는 `facet-shell-helper.exe` 프로세스 수.
#[cfg(windows)]
fn helper_processes() -> usize {
    let output = std::process::Command::new("tasklist")
        .args([
            "/FI",
            "IMAGENAME eq facet-shell-helper.exe",
            "/NH",
            "/FO",
            "CSV",
        ])
        .output()
        .expect("tasklist");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.starts_with("\"facet-shell-helper.exe\""))
        .count()
}
