// target: crates/facet-core/tests/spec_install_update_install.rs
// AC: #478: 알릴 버전 없음, portable·게시자 없는 빌드는 릴리스 페이지, 작업 중이면 JobsRunning, 체크섬 불일치·서명 없음은 Verify 실패와 파일 없음, 404는 Download 실패, 받는 중 Busy, 신뢰하는 서명은 통과, 설치 명령
//! 새 버전 설치(`App::install_update` → `Update::UpdateInstall` → `App::update_install`)를 같은 프로세스에 띄운 작은
//! HTTP 서버(릴리스 목록 JSON, MSI, `.msi.sha256`)로 확인한다: 받지 않고 거절하는 경우(알릴 버전 없음, 끝나지 않은 작업,
//! 받는 중), 받지 않고 릴리스 페이지를 여는 경우(portable, 게시자 이름 없는 빌드), 받은 뒤 확인에 실패하면 내려받기 폴더에
//! 파일이 남지 않는 것, 신뢰하는 인증서로 서명한 파일은 통과하는 것, `msiexec` 명령. 서명 시험은 테스트용 자체 서명
//! 인증서를 로컬 컴퓨터 신뢰 루트에 잠깐 넣으므로 관리자일 때만 돌고 아니면 건너뛰었다고 출력한다. 설치(msiexec 실행)는
//! 하지 않는다. Windows 전용.

#![cfg(windows)]

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::build::{Channel, InstallKind};
use facet_core::command::CommandId;
use facet_core::job::JobKind;
use facet_core::tab::ReadOutcome;
use facet_core::update::{
    InstallError, InstallFailure, InstallStart, UpdateCheckError, UpdateOptions, VerifyError,
    Version, installer_command,
};
use facet_core::window::{App, Update};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(30);
const PAGE: &str = "https://github.com/farrar142/facet/releases/tag/v9.0.0";

#[derive(Clone)]
enum Route {
    Body(Vec<u8>),
    Hang,
}

type Routes = Arc<Mutex<HashMap<String, Route>>>;

/// 경로마다 정한 답을 주는 작은 HTTP/1.1 서버. 없는 경로는 404. 받은 요청 경로를 모은다.
struct Server {
    addr: SocketAddr,
    routes: Routes,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn start() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let routes: Routes = Arc::default();
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let (shared, log) = (Arc::clone(&routes), Arc::clone(&seen));
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let (routes, log) = (Arc::clone(&shared), Arc::clone(&log));
                thread::spawn(move || answer(stream, &routes, &log));
            }
        });
        Server { addr, routes, seen }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    fn route(&self, path: &str, route: Route) {
        self.routes.lock().unwrap().insert(path.to_owned(), route);
    }

    fn requested(&self, path: &str) -> bool {
        self.seen.lock().unwrap().iter().any(|p| p == path)
    }

    /// v9.0.0 하나가 든 릴리스 목록. MSI와 체크섬 자산은 이 서버의 `/dl/` 아래를 가리킨다.
    fn publish_release(&self) {
        let json = format!(
            r#"[{{"tag_name":"v9.0.0","html_url":"{PAGE}","draft":false,"prerelease":false,"assets":[
                {{"name":"facet-v9.0.0-windows-x64.msi","browser_download_url":"{msi}"}},
                {{"name":"facet-v9.0.0-windows-x64.msi.sha256","browser_download_url":"{sum}"}}]}}]"#,
            msi = self.url("/dl/facet-v9.0.0-windows-x64.msi"),
            sum = self.url("/dl/facet-v9.0.0-windows-x64.msi.sha256"),
        );
        self.route("/releases", Route::Body(json.into_bytes()));
    }

    fn serve_download(&self, msi: Route, checksum: &str) {
        self.route("/dl/facet-v9.0.0-windows-x64.msi", msi);
        self.route(
            "/dl/facet-v9.0.0-windows-x64.msi.sha256",
            Route::Body(checksum.as_bytes().to_vec()),
        );
    }
}

fn answer(mut stream: TcpStream, routes: &Routes, seen: &Mutex<Vec<String>>) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => head.push(byte[0]),
            _ => return,
        }
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    let path = head.split(' ').nth(1).unwrap_or("").to_owned();
    seen.lock().unwrap().push(path.clone());
    let route = routes.lock().unwrap().get(&path).cloned();
    match route {
        Some(Route::Body(body)) => {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(&body);
        }
        Some(Route::Hang) => thread::sleep(Duration::from_secs(30)),
        None => {
            let _ = write!(
                stream,
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
        }
    }
}

fn powershell(script: &str) -> Option<String> {
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        eprintln!(
            "PowerShell 실패: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// `cargo xtask dist`와 같은 체크섬 줄.
fn checksum_line(path: &Path) -> String {
    let hash = powershell(&format!(
        "(Get-FileHash -Algorithm SHA256 -LiteralPath '{}').Hash.ToLower()",
        path.display()
    ))
    .expect("Get-FileHash");
    format!("{hash}  facet-v9.0.0-windows-x64.msi\n")
}

/// 서명 없는 PE 파일(이 테스트 실행 파일의 사본).
fn unsigned_pe(dir: &Path) -> PathBuf {
    let path = dir.join("payload.exe");
    fs::copy(std::env::current_exe().unwrap(), &path).unwrap();
    path
}

/// 로컬 컴퓨터가 잠깐 신뢰하는 테스트용 코드 서명 인증서. 버리면 지운다.
struct TestCert {
    thumbprint: String,
}

impl TestCert {
    fn create(subject: &str) -> Option<TestCert> {
        let script = format!(
            "$ErrorActionPreference = 'Stop'
             $c = New-SelfSignedCertificate -Type CodeSigningCert -Subject 'CN={subject}' -CertStoreLocation Cert:\\CurrentUser\\My -NotAfter (Get-Date).AddDays(2)
             try {{
               $p = Join-Path ([IO.Path]::GetTempPath()) ($c.Thumbprint + '.cer')
               Export-Certificate -Cert $c -FilePath $p | Out-Null
               Import-Certificate -FilePath $p -CertStoreLocation Cert:\\LocalMachine\\Root | Out-Null
               Import-Certificate -FilePath $p -CertStoreLocation Cert:\\LocalMachine\\TrustedPublisher | Out-Null
               Remove-Item $p
             }} catch {{
               Remove-Item ('Cert:\\CurrentUser\\My\\' + $c.Thumbprint) -ErrorAction SilentlyContinue
               throw
             }}
             Write-Output $c.Thumbprint"
        );
        let out = powershell(&script)?;
        Some(TestCert {
            thumbprint: out.lines().last()?.trim().to_owned(),
        })
    }

    fn sign(&self, file: &Path) {
        let script = format!(
            "$ErrorActionPreference = 'Stop'
             $c = Get-Item 'Cert:\\CurrentUser\\My\\{}'
             $r = Set-AuthenticodeSignature -LiteralPath '{}' -Certificate $c -HashAlgorithm SHA256
             if ($r.Status -ne 'Valid') {{ throw $r.StatusMessage }}",
            self.thumbprint,
            file.display()
        );
        powershell(&script).expect("테스트 인증서로 서명");
    }
}

impl Drop for TestCert {
    fn drop(&mut self) {
        let t = &self.thumbprint;
        let _ = powershell(&format!(
            "Remove-Item 'Cert:\\LocalMachine\\Root\\{t}','Cert:\\LocalMachine\\TrustedPublisher\\{t}','Cert:\\CurrentUser\\My\\{t}' -ErrorAction SilentlyContinue"
        ));
    }
}

fn app_at(path: &Path) -> App {
    let mut app = App::new(Some(Location::file_system(path)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

fn options(
    server: &Server,
    dir: &Path,
    kind: InstallKind,
    publisher: Option<&str>,
) -> UpdateOptions {
    let mut options = UpdateOptions::new(dir.join("state"));
    options.releases_url = server.url("/releases");
    options.current = Version::parse("0.1.0").unwrap();
    options.channel = Channel::Stable;
    options.install_kind = kind;
    options.publisher = publisher.map(str::to_owned);
    options.first_delay = Duration::from_secs(3600);
    options.interval = None;
    options.timeout = Duration::from_secs(5);
    options.download_folder = dir.join("downloads");
    options
}

/// 업데이트를 켜고 바로 확인해 새 버전 알림이 생길 때까지 기다린다.
fn app_with_notice(dir: &Path, options: UpdateOptions) -> App {
    let mut app = app_at(dir);
    app.enable_updates(options);
    app.execute(CommandId::CheckForUpdates);
    let deadline = Instant::now() + WAIT;
    while app.update_notice().is_none() {
        assert!(Instant::now() < deadline, "새 버전 알림이 오지 않았다");
        app.wait_update(Duration::from_millis(50));
    }
    app
}

/// 받기·확인이 끝날 때까지(`Update::UpdateInstall`) 기다리고 결과를 돌려준다.
fn wait_install(app: &mut App) -> Result<PathBuf, InstallFailure> {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "설치 파일 받기 결과가 오지 않았다");
        if let Some((_, Update::UpdateInstall)) = app.wait_update(left) {
            return app.update_install().cloned().expect("받기 결과");
        }
    }
}

/// 내려받기 폴더에 남은 파일들(폴더가 없으면 없음).
fn leftovers(folder: &Path) -> Vec<PathBuf> {
    match fs::read_dir(folder) {
        Ok(entries) => entries.map(|e| e.unwrap().path()).collect(),
        Err(_) => Vec::new(),
    }
}

#[test]
#[ignore = "#478"]
fn install_without_a_notice_downloads_nothing() {
    let dir = TempDir::new().unwrap();
    let mut app = app_at(dir.path());
    assert_eq!(app.install_update(), Err(InstallError::NoUpdate));
    assert!(app.update_install().is_none());
}

#[test]
#[ignore = "#478"]
fn portable_opens_the_release_page_instead_of_downloading() {
    let dir = TempDir::new().unwrap();
    let server = Server::start();
    server.publish_release();
    let mut app = app_with_notice(
        dir.path(),
        options(&server, dir.path(), InstallKind::Portable, Some("facet")),
    );
    assert_eq!(
        app.install_update(),
        Ok(InstallStart::OpenReleasePage(PAGE.to_owned()))
    );
    thread::sleep(Duration::from_millis(300));
    assert!(!server.requested("/dl/facet-v9.0.0-windows-x64.msi"));
    assert!(leftovers(&dir.path().join("downloads")).is_empty());
}

#[test]
#[ignore = "#478"]
fn build_without_publisher_opens_the_release_page() {
    let dir = TempDir::new().unwrap();
    let server = Server::start();
    server.publish_release();
    let mut app = app_with_notice(
        dir.path(),
        options(&server, dir.path(), InstallKind::Installed, None),
    );
    assert_eq!(
        app.install_update(),
        Ok(InstallStart::OpenReleasePage(PAGE.to_owned()))
    );
}

#[test]
#[ignore = "#478"]
fn unfinished_jobs_block_the_download() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    fs::create_dir(&src).unwrap();
    fs::create_dir(&dst).unwrap();
    fs::write(src.join("파일.txt"), b"new").unwrap();
    fs::write(dst.join("파일.txt"), b"old").unwrap();

    let server = Server::start();
    server.publish_release();
    let mut app = app_with_notice(
        &src,
        options(&server, dir.path(), InstallKind::Installed, Some("facet")),
    );
    // 이름 충돌로 답을 기다리는 복사 작업(답할 때까지 끝나지 않는다).
    let file = app
        .active_tab()
        .items()
        .position(|item| item.name == "파일.txt")
        .expect("원본 파일");
    app.active_tab_mut().select_only(file);
    let plan = app
        .transfer_plan_to(JobKind::Copy, &dst.to_string_lossy())
        .expect("대상 해석");
    app.start_transfer(&plan).expect("작업을 넣는다");
    assert!(app.jobs().has_unfinished());

    assert_eq!(app.install_update(), Err(InstallError::JobsRunning));
    thread::sleep(Duration::from_millis(300));
    assert!(!server.requested("/dl/facet-v9.0.0-windows-x64.msi"));
}

#[test]
#[ignore = "#478"]
fn checksum_mismatch_is_refused_and_leaves_no_files() {
    let dir = TempDir::new().unwrap();
    let server = Server::start();
    server.publish_release();
    server.serve_download(
        Route::Body(b"not really an msi".to_vec()),
        &format!("{}  facet-v9.0.0-windows-x64.msi\n", "0".repeat(64)),
    );
    let mut app = app_with_notice(
        dir.path(),
        options(&server, dir.path(), InstallKind::Installed, Some("facet")),
    );
    assert_eq!(app.install_update(), Ok(InstallStart::Downloading));
    assert_eq!(
        wait_install(&mut app),
        Err(InstallFailure::Verify(VerifyError::ChecksumMismatch))
    );
    assert!(server.requested("/dl/facet-v9.0.0-windows-x64.msi"));
    assert!(leftovers(&dir.path().join("downloads")).is_empty());
}

#[test]
#[ignore = "#478"]
fn unsigned_installer_is_refused_and_leaves_no_files() {
    let dir = TempDir::new().unwrap();
    let payload = unsigned_pe(dir.path());
    let server = Server::start();
    server.publish_release();
    server.serve_download(
        Route::Body(fs::read(&payload).unwrap()),
        &checksum_line(&payload),
    );
    let mut app = app_with_notice(
        dir.path(),
        options(&server, dir.path(), InstallKind::Installed, Some("facet")),
    );
    assert_eq!(app.install_update(), Ok(InstallStart::Downloading));
    assert_eq!(
        wait_install(&mut app),
        Err(InstallFailure::Verify(VerifyError::NotSigned))
    );
    assert!(leftovers(&dir.path().join("downloads")).is_empty());
}

#[test]
#[ignore = "#478"]
fn missing_installer_is_a_download_failure() {
    let dir = TempDir::new().unwrap();
    let server = Server::start();
    server.publish_release(); // MSI 경로는 404
    let mut app = app_with_notice(
        dir.path(),
        options(&server, dir.path(), InstallKind::Installed, Some("facet")),
    );
    assert_eq!(app.install_update(), Ok(InstallStart::Downloading));
    assert_eq!(
        wait_install(&mut app),
        Err(InstallFailure::Download(UpdateCheckError::Http(404)))
    );
    assert!(leftovers(&dir.path().join("downloads")).is_empty());
}

#[test]
#[ignore = "#478"]
fn busy_while_downloading_and_timeout_is_a_download_failure() {
    let dir = TempDir::new().unwrap();
    let server = Server::start();
    server.publish_release();
    server.serve_download(Route::Hang, &format!("{}  x\n", "0".repeat(64)));
    let mut opts = options(&server, dir.path(), InstallKind::Installed, Some("facet"));
    opts.timeout = Duration::from_millis(500);
    let mut app = app_with_notice(dir.path(), opts);

    let started = Instant::now();
    assert_eq!(app.install_update(), Ok(InstallStart::Downloading));
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "설치 시작은 곧바로 돌아온다"
    );
    assert_eq!(app.install_update(), Err(InstallError::Busy));
    assert_eq!(
        wait_install(&mut app),
        Err(InstallFailure::Download(UpdateCheckError::TimedOut))
    );
    assert!(leftovers(&dir.path().join("downloads")).is_empty());
}

#[test]
#[ignore = "#478"]
fn trusted_signature_and_same_publisher_give_the_verified_path() {
    let subject = format!("facet test update {}", std::process::id());
    let Some(cert) = TestCert::create(&subject) else {
        eprintln!("테스트 인증서를 신뢰 루트에 넣지 못해(관리자 아님?) 건너뛴다");
        return;
    };
    let dir = TempDir::new().unwrap();
    let payload = unsigned_pe(dir.path());
    cert.sign(&payload);
    let bytes = fs::read(&payload).unwrap();

    let server = Server::start();
    server.publish_release();
    server.serve_download(Route::Body(bytes.clone()), &checksum_line(&payload));
    let mut app = app_with_notice(
        dir.path(),
        options(
            &server,
            dir.path(),
            InstallKind::Installed,
            Some(subject.as_str()),
        ),
    );
    assert_eq!(app.install_update(), Ok(InstallStart::Downloading));
    let msi = wait_install(&mut app).expect("확인을 통과한다");
    assert!(msi.starts_with(dir.path().join("downloads")));
    assert_eq!(fs::read(&msi).unwrap(), bytes);
}

#[test]
#[ignore = "#478"]
fn installer_command_runs_msiexec_with_relaunch() {
    let msi = Path::new(r"C:\temp\facet-v9.0.0-windows-x64.msi");
    let command = installer_command(msi);
    let program = Path::new(command.get_program());
    assert_eq!(
        program
            .file_stem()
            .map(|s| s.to_string_lossy().to_ascii_lowercase()),
        Some("msiexec".to_owned())
    );
    let args: Vec<&OsStr> = command.get_args().collect();
    assert_eq!(
        args,
        [
            OsStr::new("/i"),
            msi.as_os_str(),
            OsStr::new("FACET_RELAUNCH=1")
        ]
    );
}
