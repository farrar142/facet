// target: crates/facet-core/tests/spec_install_msi.rs
// AC: #477: 조용한 설치의 파일·버전 줄, 서비스 자동(지연 시작)·실행·인덱스 폴더·파이프 답, 시작 메뉴 바로 가기, 조용한 제거가 모두 지우고 설정은 남김, 업그레이드가 새 버전 하나, 내려가기 거절
// AC: #479: 설치한 facet으로 켠 지금 계정의 가로채기가 조용한 제거 뒤 원래대로이고 백업이 사라짐
//! MSI 설치 프로그램을 이 기계에 실제로 설치·제거해 확인한다. 관리자 권한과 환경 변수 `FACET_TEST_MSI`(지금 MSI),
//! 업그레이드·내려가기는 `FACET_TEST_MSI_OLDER`(더 낮은 제품 버전 MSI, `cargo xtask dist <label> --msi-version 0.0.1`)가
//! 있을 때만 돌고, 없으면 건너뛰었다고 출력한다(평소 `cargo test`는 기계를 바꾸지 않는다). 태그·nightly 빌드가 이 변수를
//! 주고 돌린다. 시험끼리 같은 설치를 다루므로 하나씩 돈다. 가로채기 시험은 지금 계정의 HKCU를 바꾸므로 이미 facet
//! 가로채기 백업이 있는 계정이면 건너뛴다. Windows 전용.

#![cfg(windows)]

use std::env;
use std::fs::{self, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use facet_core::build::{INSTALL_MARKER, VERSION};
use facet_core::interception::{BACKUP_FILE_NAME, Interception, InterceptionStatus};
use facet_protocol::{
    PIPE_CLIENT_ACCESS, PIPE_NAME, PROTOCOL_VERSION, Request, Response, read_message, write_message,
};
use facet_win::registry::{self, Hive};

/// 같은 기계의 설치를 다루므로 하나씩 돈다.
static SERIAL: Mutex<()> = Mutex::new(());

const SERVICE: &str = "facet-indexer";
const UNINSTALL_KEY: &str = r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";

fn is_admin() -> bool {
    Command::new("net")
        .arg("session")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// 관리자이고 그 환경 변수의 MSI가 있으면 경로. 아니면 까닭을 출력하고 `None`.
fn msi_from(var: &str) -> Option<PathBuf> {
    let Some(path) = env::var_os(var).map(PathBuf::from) else {
        eprintln!("{var}가 없어 설치 시험을 건너뛴다");
        return None;
    };
    if !is_admin() {
        eprintln!("관리자가 아니라 설치 시험을 건너뛴다");
        return None;
    }
    assert!(path.is_file(), "{var}={}가 없다", path.display());
    Some(path)
}

fn msiexec(action: &str, msi: &Path) -> i32 {
    let log = env::temp_dir().join(format!(
        "facet-msi-{}-{}.log",
        action.trim_start_matches('/'),
        std::process::id()
    ));
    let status = Command::new("msiexec")
        .arg(action)
        .arg(msi)
        .args(["/qn", "/norestart", "/l*v"])
        .arg(&log)
        .status()
        .expect("msiexec 실행");
    status.code().unwrap_or(-1)
}

fn install(msi: &Path) {
    let code = msiexec("/i", msi);
    assert!(code == 0 || code == 3010, "설치 실패: 종료 코드 {code}");
}

fn uninstall(msi: &Path) {
    let code = msiexec("/x", msi);
    assert!(code == 0 || code == 3010, "제거 실패: 종료 코드 {code}");
}

/// 앞 시험이 남긴 설치를 걷는다(설치돼 있지 않으면 아무 일도 없음).
fn clean(msis: &[&Path]) {
    for msi in msis {
        let _ = msiexec("/x", msi);
    }
}

fn program_files() -> PathBuf {
    PathBuf::from(env::var_os("ProgramFiles").expect("ProgramFiles"))
}

fn program_data() -> PathBuf {
    PathBuf::from(env::var_os("ProgramData").expect("ProgramData"))
}

fn install_dir() -> PathBuf {
    program_files().join("facet")
}

fn index_dir() -> PathBuf {
    program_data().join("facet").join("index")
}

fn shortcut() -> PathBuf {
    program_data()
        .join(r"Microsoft\Windows\Start Menu\Programs")
        .join("facet.lnk")
}

fn sc(args: &[&str]) -> (bool, String) {
    let output = Command::new("sc").args(args).output().expect("sc 실행");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

fn service_exists() -> bool {
    sc(&["query", SERVICE]).0
}

fn wait_service_running() {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let (ok, out) = sc(&["query", SERVICE]);
        if ok && out.contains("RUNNING") {
            return;
        }
        assert!(Instant::now() < deadline, "서비스가 돌지 않는다: {out}");
        thread::sleep(Duration::from_millis(500));
    }
}

/// `reg query`로 「앱 및 기능」의 facet 항목들의 DisplayVersion.
fn installed_versions() -> Vec<String> {
    let output = Command::new("reg")
        .args(["query", UNINSTALL_KEY, "/s", "/f", "facet", "/d", "/e"])
        .output()
        .expect("reg query");
    let listing = String::from_utf8_lossy(&output.stdout).into_owned();
    listing
        .lines()
        .filter(|line| line.starts_with("HKEY_"))
        .filter_map(|key| {
            let output = Command::new("reg")
                .args(["query", key, "/v", "DisplayVersion"])
                .output()
                .ok()?;
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .find(|l| l.trim_start().starts_with("DisplayVersion"))
                .and_then(|l| l.split_whitespace().last())
                .map(str::to_owned)
        })
        .collect()
}

fn installed_version_line() -> String {
    let output = Command::new(install_dir().join("facet.exe"))
        .arg("--version")
        .output()
        .expect("설치한 facet --version");
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .to_owned()
}

/// 질의 파이프에 붙어 버전 인사와 상태 질의를 한다. 서비스가 막 떴을 수 있어 잠깐 다시 해 본다.
fn query_pipe() {
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut pipe = loop {
        match OpenOptions::new()
            .read(true)
            .write(true)
            .access_mode(PIPE_CLIENT_ACCESS)
            .open(PIPE_NAME)
        {
            Ok(pipe) => break pipe,
            Err(e) => {
                assert!(Instant::now() < deadline, "질의 파이프에 붙지 못했다: {e}");
                thread::sleep(Duration::from_millis(500));
            }
        }
    };
    write_message(
        &mut pipe,
        &Request::Hello {
            version: PROTOCOL_VERSION,
        },
    )
    .expect("버전 인사 보내기");
    let hello: Response = read_message(&mut pipe).expect("버전 인사 답");
    assert!(
        matches!(hello, Response::HelloAccepted { .. }),
        "버전 인사: {hello:?}"
    );
    write_message(&mut pipe, &Request::Status).expect("상태 질의 보내기");
    let status: Response = read_message(&mut pipe).expect("상태 답");
    assert!(matches!(status, Response::Status(_)), "상태 답: {status:?}");
}

/// 지금 계정의 사용자 설정 파일. 없으면 만들고, 만들었으면 `true`.
fn ensure_user_settings() -> (PathBuf, bool) {
    let folder = PathBuf::from(env::var_os("APPDATA").expect("APPDATA")).join("facet");
    let file = folder.join("settings.toml");
    if file.exists() {
        return (file, false);
    }
    fs::create_dir_all(&folder).unwrap();
    fs::write(&file, "# facet MSI 시험이 만든 파일\n").unwrap();
    (file, true)
}

#[test]
#[ignore = "#477"]
fn 조용한_설치는_파일_서비스_인덱스_바로_가기를_놓고_제거는_모두_걷되_설정은_남긴다() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(msi) = msi_from("FACET_TEST_MSI") else {
        return;
    };
    clean(&[&msi]);

    install(&msi);
    let dir = install_dir();
    for name in [
        "facet.exe",
        "facet-indexer.exe",
        "facet-shell-helper.exe",
        "LICENSE",
        "NOTICE.txt",
        INSTALL_MARKER,
    ] {
        assert!(dir.join(name).is_file(), "설치 폴더에 {name}이 없다");
    }
    let line = installed_version_line();
    assert!(
        line.starts_with(&format!("facet {VERSION} (")) && line.ends_with(", installed)"),
        "설치한 facet의 버전 줄: {line}"
    );

    wait_service_running();
    let (_, config) = sc(&["qc", SERVICE]);
    assert!(config.contains("AUTO_START"), "자동 시작: {config}");
    assert!(config.contains("DELAYED"), "지연 시작: {config}");
    assert!(config.contains("LocalSystem"), "LocalSystem: {config}");
    let deadline = Instant::now() + Duration::from_secs(120);
    while !index_dir().is_dir() {
        assert!(Instant::now() < deadline, "인덱스 폴더가 생기지 않았다");
        thread::sleep(Duration::from_millis(500));
    }
    query_pipe();
    assert!(shortcut().is_file(), "시작 메뉴 바로 가기가 없다");
    assert_eq!(installed_versions().len(), 1, "「앱 및 기능」 항목은 하나");

    let (settings, created) = ensure_user_settings();
    uninstall(&msi);
    assert!(!service_exists(), "서비스가 남았다");
    assert!(!dir.exists(), "설치 폴더가 남았다");
    assert!(!index_dir().exists(), "인덱스 폴더가 남았다");
    assert!(!shortcut().exists(), "바로 가기가 남았다");
    assert!(
        installed_versions().is_empty(),
        "「앱 및 기능」 항목이 남았다"
    );
    assert!(settings.is_file(), "조용한 제거는 사용자 설정을 남긴다");
    if created {
        let _ = fs::remove_file(&settings);
    }
}

#[test]
#[ignore = "#477"]
fn 옛_버전_위에_설치하면_새_버전_하나가_되고_서비스가_돈다() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (Some(msi), Some(older)) = (msi_from("FACET_TEST_MSI"), msi_from("FACET_TEST_MSI_OLDER"))
    else {
        return;
    };
    clean(&[&msi, &older]);

    install(&msi);
    let current = installed_versions();
    assert_eq!(current.len(), 1);
    uninstall(&msi);

    install(&older);
    let old = installed_versions();
    assert_eq!(old.len(), 1);
    assert_ne!(old, current, "옛 MSI는 제품 버전이 낮아야 한다");
    wait_service_running();

    install(&msi);
    assert_eq!(installed_versions(), current, "새 버전 하나만 남는다");
    wait_service_running();
    query_pipe();
    assert!(install_dir().join("facet.exe").is_file());

    uninstall(&msi);
    assert!(installed_versions().is_empty());
}

#[test]
#[ignore = "#477"]
fn 새_버전_위에_옛_버전을_설치하면_거절하고_그대로다() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (Some(msi), Some(older)) = (msi_from("FACET_TEST_MSI"), msi_from("FACET_TEST_MSI_OLDER"))
    else {
        return;
    };
    clean(&[&msi, &older]);

    install(&msi);
    let current = installed_versions();
    let code = msiexec("/i", &older);
    assert!(code != 0 && code != 3010, "내려가는 설치는 실패해야 한다");
    assert_eq!(installed_versions(), current, "새 버전이 그대로다");
    assert!(install_dir().join("facet.exe").is_file());
    wait_service_running();

    uninstall(&msi);
}

#[test]
#[ignore = "#479"]
fn 제거하면_지금_계정의_가로채기가_원래대로_돌아온다() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(msi) = msi_from("FACET_TEST_MSI") else {
        return;
    };
    let backup = PathBuf::from(env::var_os("APPDATA").expect("APPDATA"))
        .join("facet")
        .join(BACKUP_FILE_NAME);
    if backup.exists() {
        eprintln!("이 계정에 이미 가로채기 백업이 있어 건너뛴다");
        return;
    }
    clean(&[&msi]);
    install(&msi);

    let interception = Interception::for_facet(&install_dir().join("facet.exe"), &backup);
    let full = |key: &str| format!("{}\\{key}", interception.root());
    let before: Vec<_> = interception
        .changes()
        .iter()
        .map(|c| {
            (
                registry::read_value(Hive::CurrentUser, &full(&c.key), &c.name).unwrap(),
                registry::key_exists(Hive::CurrentUser, &full(&c.key)).unwrap(),
            )
        })
        .collect();
    interception.enable().expect("가로채기 켜기");
    assert_eq!(interception.status(), Ok(InterceptionStatus::On));

    uninstall(&msi);
    for (change, (value, existed)) in interception.changes().iter().zip(&before) {
        assert_eq!(
            &registry::read_value(Hive::CurrentUser, &full(&change.key), &change.name).unwrap(),
            value,
            "{}\\{}가 원래대로가 아니다",
            change.key,
            change.name
        );
        if !existed {
            assert!(
                !registry::key_exists(Hive::CurrentUser, &full(&change.key)).unwrap(),
                "facet이 만든 키 {}가 남았다",
                change.key
            );
        }
    }
    assert!(!backup.exists(), "백업 파일이 남았다");
}
