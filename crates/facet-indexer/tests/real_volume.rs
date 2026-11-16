//! 실제 볼륨 테스트: 관리자 권한과 NTFS 시험 볼륨이 필요한 테스트.
//!
//! 표시 방법(작업 공간 전체의 약속):
//! - 테스트 이름을 `real_volume_`으로 시작하고 `#[ignore = "실제 볼륨 테스트 ..."]`를 붙인다.
//!   그래서 기본 `cargo test --workspace`에서 빠진다.
//! - 시험 볼륨의 드라이브 문자는 환경 변수 `FACET_TEST_VOLUME`(예: `T:`)으로 받는다.
//! - `cargo xtask ci`는 관리자일 때 시험 VHD를 만들어 NTFS로 포맷해 붙이고, 이 변수를 넣어
//!   `cargo test --workspace -- --ignored real_volume_`으로 이 테스트들만 돌린 뒤 VHD를 뗀다.
//!   관리자가 아니면 건너뛴다고 출력한다.
//! - 손으로 돌리기: 관리자 터미널에서 시험 볼륨을 붙이고
//!   `FACET_TEST_VOLUME=T: cargo test --workspace -- --ignored real_volume_`(서비스 테스트는
//!   시험용 질의 명령 `facet-query`도 부르므로 작업 공간 전체로 빌드한다).
//! - 서비스 테스트는 인덱서 서비스를 실제로 설치했다가 지운다. 이미 설치돼 있으면 건드리지 않고
//!   건너뛴다.
#![cfg(windows)]

use std::collections::BTreeSet;
use std::env;
use std::fs::{self, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use facet_index::ATTRIBUTE_DIRECTORY;
use facet_indexer::SERVICE_NAME;
use facet_indexer::follow::{self, Stop};
use facet_indexer::index_dir;
use facet_indexer::service::index_volumes;
use facet_indexer::session::Cancel;
use facet_indexer::volume::local_volumes;
use facet_indexer::volumes::{Plan, Volumes};
use facet_protocol::{
    Case, Condition, NoIndexReason, PIPE_CLIENT_ACCESS, PIPE_NAME, PROTOCOL_VERSION, QueryMessage,
    Request, Response, VolumeState, VolumeStatus, read_message, write_message,
};

const VOLUME_VAR: &str = "FACET_TEST_VOLUME";

fn test_volume() -> String {
    env::var(VOLUME_VAR).unwrap_or_else(|_| {
        panic!("실제 볼륨 테스트에는 시험 NTFS 볼륨의 드라이브 문자를 담은 FACET_TEST_VOLUME이 필요합니다")
    })
}

#[test]
#[ignore = "실제 볼륨 테스트: 관리자 권한과 FACET_TEST_VOLUME이 필요하다(cargo xtask ci가 관리자일 때 돌린다)"]
fn real_volume_dump_mft_reads_file_records() {
    let volume = test_volume();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("volume.mft");

    let status = Command::new(env!("CARGO_BIN_EXE_facet-indexer"))
        .args(["dump-mft", &volume])
        .arg(&out)
        .status()
        .unwrap();
    assert!(status.success(), "dump-mft 실패: {status}");

    let bytes = std::fs::read(&out).unwrap();
    // 시험 VHD는 레코드 1024바이트로 포맷된다.
    const RECORD: usize = 1024;
    assert_eq!(bytes.len() % RECORD, 0, "레코드 크기의 배수가 아님");
    let records: Vec<&[u8]> = bytes.chunks(RECORD).collect();
    // 레코드 0..16은 NTFS 메타데이터 파일($MFT, $MFTMirr, $LogFile, ..., $Extend 등)이라 늘 쓰인다.
    assert!(records.len() >= 16, "레코드가 {}개뿐임", records.len());
    for (n, record) in records.iter().take(16).enumerate() {
        assert_eq!(&record[0..4], b"FILE", "레코드 {n}의 서명이 FILE이 아님");
    }
}

/// `FACET_TEST_VOLUME`을 `T:` 꼴로.
fn volume_name() -> String {
    test_volume().trim_end_matches(['\\', '/']).to_uppercase()
}

/// 시험 볼륨 루트 아래 이 테스트만 쓰는 새 폴더. 버릴 때 지운다.
struct Scratch(PathBuf);

impl Scratch {
    fn new(volume: &str, tag: &str) -> Self {
        let dir = PathBuf::from(format!(r"{volume}\facet-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn file(&self, relative: &str, size: usize) -> PathBuf {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, vec![b'x'; size]).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
#[ignore = "실제 볼륨 테스트: 관리자 권한과 FACET_TEST_VOLUME이 필요하다(cargo xtask ci가 관리자일 때 돌린다)"]
fn real_volume_first_index_matches_volume_contents() {
    let volume = volume_name();
    let scratch = Scratch::new(&volume, "first-index");
    let files = [
        (r"보고서 1.txt", 100),
        (r"사진\여름 바다.jpg", 2000),
        (r"사진\깊은\이모지 😀🎉.txt", 0),
        (r"큰 파일.bin", 100_000),
    ];
    for (name, size) in files {
        scratch.file(name, size);
    }

    // 시험 볼륨은 로컬 NTFS 볼륨으로 목록에 든다.
    let targets: Vec<(String, Plan)> = local_volumes()
        .into_iter()
        .filter(|(v, _)| *v == volume)
        .collect();
    assert_eq!(targets, [(volume.clone(), Plan::Index)]);

    let following = Following::start(targets);
    following.wait_ready(&volume);
    let volumes = &following.volumes;
    let status = volumes.status();
    let [state] = &status.volumes[..] else {
        panic!("볼륨 상태가 하나가 아님: {status:?}")
    };
    assert!(
        matches!(state.state, VolumeState::Ready { items } if items as usize >= files.len()),
        "{state:?}"
    );

    let page = volumes
        .search(
            &QueryMessage {
                condition: Condition::Under {
                    folder: scratch.0.display().to_string(),
                },
                sort: None,
                limit: None,
            },
            &Cancel::new(),
        )
        .unwrap();
    // (경로, 파일이면 크기)
    let got: BTreeSet<(String, Option<u64>)> = page
        .hits
        .iter()
        .map(|h| {
            let size = (h.attributes & ATTRIBUTE_DIRECTORY == 0).then_some(h.size);
            (h.path.clone(), size)
        })
        .collect();
    let mut want: BTreeSet<(String, Option<u64>)> = files
        .iter()
        .map(|(name, size)| {
            let path = scratch.0.join(name).display().to_string();
            (path, Some(*size as u64))
        })
        .collect();
    for folder in [r"사진", r"사진\깊은"] {
        want.insert((scratch.0.join(folder).display().to_string(), None));
    }
    assert_eq!(got, want);
    following.stop();
}

/// 서비스처럼 볼륨들의 인덱스를 만들고 따라가는 스레드들. 버릴 때 멈추라고 알린다.
struct Following {
    volumes: Arc<Volumes>,
    stop: Stop,
    threads: Vec<thread::JoinHandle<()>>,
}

impl Following {
    fn start(targets: Vec<(String, Plan)>) -> Self {
        let volumes = Arc::new(Volumes::new());
        let stop = Stop::new();
        let threads = index_volumes(&volumes, targets, &stop, None);
        Self {
            volumes,
            stop,
            threads,
        }
    }

    /// `volume`이 「준비됨」이 될 때까지 기다린다.
    fn wait_ready(&self, volume: &str) {
        let deadline = Instant::now() + Duration::from_secs(300);
        loop {
            let status = self.volumes.status();
            let state = status.volumes.iter().find(|v| v.volume == volume);
            match state.map(|v| &v.state) {
                Some(VolumeState::Ready { .. }) => return,
                Some(VolumeState::Building { .. }) | None => {}
                Some(other) => panic!("{volume}의 상태가 {other:?}"),
            }
            assert!(
                Instant::now() < deadline,
                "{volume}의 인덱스가 300초 안에 준비되지 않음"
            );
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// 이름에 `part`가 든 항목의 경로들(정렬).
    fn paths(&self, part: &str) -> Vec<String> {
        let page = self
            .volumes
            .search(
                &QueryMessage {
                    condition: Condition::NameContains {
                        part: part.to_owned(),
                        case: Case::Insensitive,
                    },
                    sort: None,
                    limit: None,
                },
                &Cancel::new(),
            )
            .unwrap();
        let mut paths: Vec<String> = page.hits.into_iter().map(|h| h.path).collect();
        paths.sort();
        paths
    }

    /// 방금 한 변경이 1초 안에 질의 결과에 반영되는지 본다: 이름에 `part`가 든 항목의 경로들이
    /// `want`(정렬)가 되어야 한다.
    fn expect_within_a_second(&self, what: &str, part: &str, want: &[&Path]) {
        let changed = Instant::now();
        let mut want: Vec<String> = want.iter().map(|p| p.display().to_string()).collect();
        want.sort();
        loop {
            let got = self.paths(part);
            // 질의 자체의 시간도 넣어 잰다(반영을 본 시각이 기준 안이어야 한다).
            let elapsed = changed.elapsed();
            if got == want && elapsed < LIVE_UPDATE {
                eprintln!("{what}: {}ms 만에 반영", elapsed.as_millis());
                return;
            }
            assert!(
                elapsed < LIVE_UPDATE,
                "{what}: {}ms 안에 반영되지 않음(지금 결과 {got:?}, 기대 {want:?})",
                LIVE_UPDATE.as_millis()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn stop(mut self) {
        self.stop.stop();
        for t in self.threads.drain(..) {
            t.join().unwrap();
        }
    }
}

impl Drop for Following {
    fn drop(&mut self) {
        self.stop.stop();
    }
}

/// 파일 변경이 질의 결과에 반영되어야 하는 시간(spec #22).
const LIVE_UPDATE: Duration = Duration::from_secs(1);

#[test]
#[ignore = "실제 볼륨 테스트: 관리자 권한과 FACET_TEST_VOLUME이 필요하다(cargo xtask ci가 관리자일 때 돌린다)"]
fn real_volume_usn_changes_show_up_within_a_second() {
    let volume = volume_name();
    let scratch = Scratch::new(&volume, "usn");
    let tag = format!("usn{}", std::process::id());
    let targets: Vec<(String, Plan)> = local_volumes()
        .into_iter()
        .filter(|(v, _)| *v == volume)
        .collect();
    let following = Following::start(targets);
    following.wait_ready(&volume);

    // 만들기
    let created = scratch.0.join(format!("새 파일 {tag}.txt"));
    fs::write(&created, b"hello").unwrap();
    following.expect_within_a_second("만들기", &tag, &[&created]);

    // 이름바꾸기
    let renamed = scratch.0.join(format!("바꾼 이름 {tag}.txt"));
    fs::rename(&created, &renamed).unwrap();
    following.expect_within_a_second("이름바꾸기", &tag, &[&renamed]);

    // 지우기
    fs::remove_file(&renamed).unwrap();
    following.expect_within_a_second("지우기", &tag, &[]);

    // 폴더 이동: 옮긴 폴더 아래 항목의 경로도 바뀐다.
    let into = scratch.0.join("받는 폴더");
    fs::create_dir_all(&into).unwrap();
    let from = scratch.0.join(format!("옮길 폴더 {tag}"));
    fs::create_dir_all(&from).unwrap();
    let deep = from.join(format!("깊은 파일 {tag}.txt"));
    fs::write(&deep, b"x").unwrap();
    following.expect_within_a_second("폴더와 그 안의 파일 만들기", &tag, &[&from, &deep]);
    let moved = into.join(format!("옮길 폴더 {tag}"));
    fs::rename(&from, &moved).unwrap();
    let moved_deep = moved.join(format!("깊은 파일 {tag}.txt"));
    following.expect_within_a_second("폴더 이동", &tag, &[&moved, &moved_deep]);

    following.stop();
}

/// 설치한 서비스를 테스트가 끝날 때(실패해도) 지운다.
struct Installed;

impl Drop for Installed {
    fn drop(&mut self) {
        let _ = Command::new(env!("CARGO_BIN_EXE_facet-indexer"))
            .arg("uninstall")
            .status();
    }
}

fn service_exists() -> bool {
    Command::new("sc.exe")
        .args(["query", SERVICE_NAME])
        .output()
        .is_ok_and(|o| o.status.success())
}

fn sc(args: &[&str]) {
    let out = Command::new("sc.exe").args(args).output().unwrap();
    assert!(
        out.status.success(),
        "sc {args:?} 실패: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// 서비스의 상태 이름(`RUNNING`, `STOPPED` 등).
fn sc_state() -> Option<String> {
    let out = Command::new("sc.exe")
        .args(["query", SERVICE_NAME])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    ["RUNNING", "STOPPED", "START_PENDING", "STOP_PENDING"]
        .into_iter()
        .find(|s| text.contains(s))
        .map(str::to_owned)
}

fn wait_until(what: &str, patience: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + patience;
    while !done() {
        assert!(
            Instant::now() < deadline,
            "{what}: {}초 안에 되지 않음",
            patience.as_secs()
        );
        thread::sleep(Duration::from_millis(100));
    }
}

fn search_output(query: &Path, part: &str) -> String {
    let out = Command::new(query).args(["search", part]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// 서비스 파이프에 붙어 `volume`이 「준비됨」이 될 때까지 상태 질의를 되풀이한다. 항목 수를 돌려준다.
fn wait_ready_via_pipe(volume: &str) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(300);
    let mut pipe = loop {
        match OpenOptions::new()
            .access_mode(PIPE_CLIENT_ACCESS)
            .open(PIPE_NAME)
        {
            Ok(p) => break p,
            Err(e) if Instant::now() < deadline => {
                eprintln!("파이프를 기다림: {e}");
                thread::sleep(Duration::from_millis(200));
            }
            Err(e) => panic!("서비스 파이프에 붙지 못함: {e}"),
        }
    };
    assert!(matches!(
        ask(
            &mut pipe,
            &Request::Hello {
                version: PROTOCOL_VERSION
            }
        ),
        Response::HelloAccepted { .. }
    ));
    loop {
        let Response::Status(status) = ask(&mut pipe, &Request::Status) else {
            panic!("상태 응답이 아님")
        };
        let state = status
            .volumes
            .iter()
            .find(|v| v.volume == volume)
            .map(|v| v.state.clone());
        match state {
            Some(VolumeState::Ready { items }) => return items,
            Some(VolumeState::Building { .. }) | None => {}
            Some(other) => panic!("{volume}의 상태가 {other:?}"),
        }
        assert!(
            Instant::now() < deadline,
            "{volume}의 인덱스가 300초 안에 준비되지 않음"
        );
        thread::sleep(Duration::from_millis(200));
    }
}

/// 모든 권한(`FILE_ALL_ACCESS`).
const FILE_ALL_ACCESS: u32 = 0x001F_01FF;

/// `path`의 DACL이 SYSTEM과 Administrators에게만 모든 권한을 준다(허용 ACE만, 그 둘만).
fn assert_only_system_and_administrators(path: &Path) {
    let security = index_dir::read_security(path).unwrap();
    let sids: BTreeSet<&str> = security.aces.iter().map(|a| a.sid.as_str()).collect();
    assert_eq!(
        sids,
        BTreeSet::from([index_dir::SYSTEM_SID, index_dir::ADMINISTRATORS_SID]),
        "{}: {security:?}",
        path.display()
    );
    for ace in &security.aces {
        assert!(
            ace.allow && ace.mask == FILE_ALL_ACCESS,
            "{}: {security:?}",
            path.display()
        );
    }
}

/// `path`가 SYSTEM과 Administrators만 쓰는 폴더다: 소유자 Administrators, 상속을 막은 DACL에 그 둘만.
fn assert_locked_down(path: &Path) {
    let security = index_dir::read_security(path).unwrap();
    assert!(path.is_dir(), "{}가 폴더가 아님", path.display());
    assert!(
        !fs::symlink_metadata(path).unwrap().file_type().is_symlink(),
        "{}가 링크임",
        path.display()
    );
    assert_eq!(
        security.owner,
        index_dir::ADMINISTRATORS_SID,
        "{}",
        path.display()
    );
    assert!(
        security.protected,
        "{}: 상속을 막지 않음 {security:?}",
        path.display()
    );
    assert_only_system_and_administrators(path);
}

#[test]
#[ignore = "실제 볼륨 테스트: 관리자 권한이 필요하다(소유자를 바꾼다. cargo xtask ci가 관리자일 때 돌린다)"]
fn real_volume_index_dir_is_locked_down_even_if_someone_made_it_first() {
    // 새로 만들 때.
    let base = tempfile::tempdir().unwrap();
    let dir = base.path().join("facet").join("index");
    index_dir::prepare(&dir).unwrap();
    assert_locked_down(dir.parent().unwrap());
    assert_locked_down(&dir);
    // 다시 해도 된다.
    index_dir::prepare(&dir).unwrap();
    assert_locked_down(&dir);

    // 다른 사람이 먼저 만들어 자기 소유로 두고 모두에게 권한을 준 폴더와 그 안에 넣어 둔 파일.
    let base = tempfile::tempdir().unwrap();
    let dir = base.path().join("facet").join("index");
    fs::create_dir_all(&dir).unwrap();
    let planted = dir.join("C.facetidx");
    fs::write(&planted, b"planted").unwrap();
    let open = "O:WDD:(A;OICI;FA;;;WD)";
    index_dir::apply_sddl(dir.parent().unwrap(), open).unwrap();
    index_dir::apply_sddl(&dir, open).unwrap();
    index_dir::apply_sddl(&planted, "O:WDD:P(A;;FA;;;WD)").unwrap();
    assert_eq!(index_dir::read_security(&dir).unwrap().owner, "S-1-1-0");
    index_dir::prepare(&dir).unwrap();
    assert_locked_down(dir.parent().unwrap());
    assert_locked_down(&dir);
    assert!(!planted.exists(), "남이 넣어 둔 파일이 남아 있음");

    // 인덱스 폴더 자리에 다른 곳을 가리키는 접합(junction)이 있으면 따라가지 않는다.
    let base = tempfile::tempdir().unwrap();
    let elsewhere = base.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("keep.txt"), b"keep").unwrap();
    let dir = base.path().join("facet").join("index");
    fs::create_dir_all(dir.parent().unwrap()).unwrap();
    let made = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&dir)
        .arg(&elsewhere)
        .output()
        .unwrap();
    assert!(made.status.success(), "접합을 만들지 못함");
    index_dir::prepare(&dir).unwrap();
    assert_locked_down(&dir);
    assert!(
        elsewhere.join("keep.txt").exists(),
        "접합이 가리키던 곳을 건드림"
    );
    let elsewhere_security = index_dir::read_security(&elsewhere).unwrap();
    assert!(
        !elsewhere_security.protected,
        "접합이 가리키던 곳의 권한을 바꿈"
    );

    // 제거하면 인덱스 폴더와 빈 부모 폴더가 사라진다.
    index_dir::remove(&dir).unwrap();
    assert!(!dir.exists());
    assert!(!dir.parent().unwrap().exists());
    index_dir::remove(&dir).unwrap();
}

/// 서비스 파이프로 상태 질의를 한 번 한다.
fn status_via_pipe() -> Vec<VolumeStatus> {
    let mut pipe = OpenOptions::new()
        .access_mode(PIPE_CLIENT_ACCESS)
        .open(PIPE_NAME)
        .expect("서비스 파이프에 붙지 못함");
    assert!(matches!(
        ask(
            &mut pipe,
            &Request::Hello {
                version: PROTOCOL_VERSION
            }
        ),
        Response::HelloAccepted { .. }
    ));
    let Response::Status(status) = ask(&mut pipe, &Request::Status) else {
        panic!("상태 응답이 아님")
    };
    status.volumes
}

fn state_via_pipe(volume: &str) -> Option<VolumeState> {
    status_via_pipe()
        .into_iter()
        .find(|v| v.volume == volume)
        .map(|v| v.state)
}

/// 서비스가 도는 동안 시험 VHD를 붙였다 뗀다(spec #22, #55). NTFS 볼륨은 인덱스가 만들어져 질의
/// 결과에 나오고, NTFS가 아닌 볼륨은 「인덱스 없음(NTFS가 아님)」으로 보이고, 떼면 둘 다 상태 질의와
/// 질의 결과에서 빠지고 서비스는 계속 답한다.
fn attach_and_detach_volumes_while_the_service_runs(query: &Path, test_volume: &str) {
    let ntfs = Vhd::create("ntfs");
    let fat = Vhd::create("fat32");
    let attached = format!("{}:", ntfs.letter);
    let fat_volume = format!("{}:", fat.letter);
    let unique = format!("붙인 볼륨 {}.txt", std::process::id());
    let path = format!(r"{attached}\{unique}");
    fs::write(&path, b"usb").unwrap();

    // 붙으면 상태 질의에 나오고, NTFS면 인덱스를 만들어 질의 결과에 나온다.
    wait_until(
        "붙인 NTFS 볼륨 준비됨",
        Duration::from_secs(60),
        || matches!(state_via_pipe(&attached), Some(VolumeState::Ready { .. })),
    );
    wait_until(
        "붙인 볼륨의 파일 검색",
        Duration::from_secs(10),
        || search_output(query, &unique).contains(&path),
    );
    wait_until(
        "붙인 FAT32 볼륨 보임",
        Duration::from_secs(30),
        || state_via_pipe(&fat_volume).is_some(),
    );
    assert_eq!(
        state_via_pipe(&fat_volume),
        Some(VolumeState::NotIndexed(NoIndexReason::NotNtfs {
            filesystem: "FAT32".into()
        }))
    );
    let out = Command::new(query).arg("status").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(&attached), "{stdout}");
    assert!(stdout.contains(&fat_volume), "{stdout}");

    // 떼면 상태 질의와 질의 결과에서 빠지고 서비스는 계속 돈다.
    ntfs.detach();
    fat.detach();
    wait_until(
        "뗀 볼륨이 상태에서 빠짐",
        Duration::from_secs(30),
        || {
            let status = status_via_pipe();
            !status
                .iter()
                .any(|v| v.volume == attached || v.volume == fat_volume)
        },
    );
    assert!(
        !search_output(query, &unique).contains(&path),
        "뗀 볼륨의 파일이 검색됨"
    );
    assert_eq!(sc_state().as_deref(), Some("RUNNING"));
    assert!(
        matches!(state_via_pipe(test_volume), Some(VolumeState::Ready { .. })),
        "남은 시험 볼륨이 준비됨이 아님"
    );
}

/// 이 테스트가 붙였다 떼는 작은 VHD(64MB). 버릴 때 아직 붙어 있으면 떼고, 파일을 지운다.
struct Vhd {
    file: PathBuf,
    letter: char,
    attached: bool,
}

impl Vhd {
    /// VHD를 만들어 `fs`(`ntfs`, `fat32` 등)로 빠르게 포맷하고 빈 드라이브 문자에 붙인다.
    fn create(fs_name: &str) -> Self {
        let dir = env::temp_dir().join("facet-test-vhd");
        fs::create_dir_all(&dir).unwrap();
        // diskpart는 붙일 때와 뗄 때 같은 표기의 경로를 써야 한다(8.3 짧은 이름을 푼다).
        let dir = fs::canonicalize(&dir).unwrap();
        let dir = dir
            .display()
            .to_string()
            .trim_start_matches(r"\\?\")
            .to_owned();
        let file = PathBuf::from(dir).join(format!("attach-{fs_name}-{}.vhdx", std::process::id()));
        let _ = fs::remove_file(&file);
        let mut vhd = Self {
            file,
            letter: '?',
            attached: true,
        };
        diskpart(&format!(
            "create vdisk file=\"{f}\" maximum=64 type=expandable
             select vdisk file=\"{f}\"
             attach vdisk
             create partition primary
             format fs={fs_name} quick label=facet-{fs_name}
",
            f = vhd.file.display()
        ))
        .unwrap();
        let mut refused = Vec::new();
        for letter in ('G'..='Y')
            .rev()
            .filter(|c| !Path::new(&format!("{c}:\\")).exists())
        {
            let assign = format!(
                "select vdisk file=\"{f}\"
select partition 1
assign letter={letter}
",
                f = vhd.file.display()
            );
            match diskpart(&assign) {
                Ok(()) => {
                    vhd.letter = letter;
                    wait_until("붙인 VHD가 보임", Duration::from_secs(30), || {
                        Path::new(&format!("{letter}:\\")).exists()
                    });
                    return vhd;
                }
                Err(e) => refused.push(format!("{letter}: {e}")),
            }
        }
        panic!(
            "VHD에 드라이브 문자를 붙이지 못함:
{}",
            refused.join(
                "
"
            )
        );
    }

    fn detach(mut self) {
        let result = self.try_detach();
        self.attached = false;
        result.unwrap();
    }

    fn try_detach(&self) -> Result<(), String> {
        diskpart(&format!(
            "select vdisk file=\"{f}\"
detach vdisk
",
            f = self.file.display()
        ))
    }
}

impl Drop for Vhd {
    fn drop(&mut self) {
        if self.attached {
            let _ = self.try_detach();
        }
        let _ = fs::remove_file(&self.file);
    }
}

/// `diskpart /s`로 스크립트를 돌린다. 실패하면 출력을 담은 오류.
fn diskpart(script: &str) -> Result<(), String> {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = env::temp_dir().join(format!(
        "facet-attach-diskpart-{}-{n}.txt",
        std::process::id()
    ));
    fs::write(&path, script).unwrap();
    let output = Command::new("diskpart").arg("/s").arg(&path).output();
    let _ = fs::remove_file(&path);
    let output = output.map_err(|e| format!("diskpart를 실행하지 못함: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "diskpart 실패({}):
{}
--- 스크립트 ---
{script}",
            output.status,
            String::from_utf8_lossy(&output.stdout)
        ))
    }
}

fn ask(pipe: &mut fs::File, request: &Request) -> Response {
    write_message(pipe, request).unwrap();
    read_message(pipe).unwrap()
}

/// 시험용 질의 명령(`facet-query`). `cargo test --workspace`가 이 테스트가 부르는 `facet-indexer`와
/// 같은 폴더에 빌드한다.
fn facet_query() -> PathBuf {
    let exe = Path::new(env!("CARGO_BIN_EXE_facet-indexer")).with_file_name("facet-query.exe");
    assert!(
        exe.exists(),
        "{}가 없습니다. cargo test --workspace로 돌리거나 cargo build -p facet-query를 먼저 하십시오",
        exe.display()
    );
    exe
}

#[test]
#[ignore = "실제 볼륨 테스트: 관리자 권한과 FACET_TEST_VOLUME이 필요하다(cargo xtask ci가 관리자일 때 돌린다)"]
fn real_volume_service_install_index_query_uninstall() {
    let volume = volume_name();
    if service_exists() {
        eprintln!("서비스 {SERVICE_NAME}가 이미 설치돼 있어 이 테스트를 건너뜁니다(지우지 않음)");
        return;
    }
    let query = facet_query();
    let scratch = Scratch::new(&volume, "service");
    let unique = format!("서비스 찾기 {}.txt", std::process::id());
    let file = scratch.file(&unique, 123);
    let file_path = file.display().to_string();

    let installed = Command::new(env!("CARGO_BIN_EXE_facet-indexer"))
        .arg("install")
        .status()
        .unwrap();
    let guard = Installed;
    assert!(installed.success(), "install 실패: {installed}");
    assert!(service_exists());

    // 일반 클라이언트처럼(읽기와 데이터 쓰기만 청해) 붙고, 시험 볼륨이 준비될 때까지 상태 질의를
    // 되풀이한다. 다른 볼륨(C: 등)은 아직 만드는 중이어도 된다. 서비스는 볼륨을 고르기 전에
    // 파이프부터 열므로 처음 상태에는 시험 볼륨이 아직 없을 수 있다(빈 목록, run 43929796209).
    let deadline = Instant::now() + Duration::from_secs(300);
    let mut pipe = loop {
        match OpenOptions::new()
            .access_mode(PIPE_CLIENT_ACCESS)
            .open(PIPE_NAME)
        {
            Ok(p) => break p,
            Err(e) if Instant::now() < deadline => {
                eprintln!("파이프를 기다림: {e}");
                thread::sleep(Duration::from_millis(200));
            }
            Err(e) => panic!("서비스 파이프에 붙지 못함: {e}"),
        }
    };
    assert!(matches!(
        ask(
            &mut pipe,
            &Request::Hello {
                version: PROTOCOL_VERSION
            }
        ),
        Response::HelloAccepted { .. }
    ));
    loop {
        let Response::Status(status) = ask(&mut pipe, &Request::Status) else {
            panic!("상태 응답이 아님")
        };
        let state = status
            .volumes
            .iter()
            .find(|v| v.volume == volume)
            .map(|v| v.state.clone());
        match state {
            Some(VolumeState::Ready { items }) => {
                assert!(items > 0);
                break;
            }
            Some(VolumeState::Building { .. }) | None => {}
            Some(other) => panic!("{volume}의 상태가 {other:?}"),
        }
        assert!(
            Instant::now() < deadline,
            "{volume}의 인덱스가 300초 안에 준비되지 않음"
        );
        thread::sleep(Duration::from_millis(200));
    }

    let Response::Page(page) = ask(
        &mut pipe,
        &Request::Query(QueryMessage {
            condition: Condition::NameContains {
                part: unique.clone(),
                case: Case::Insensitive,
            },
            sort: None,
            limit: Some(10),
        }),
    ) else {
        panic!("결과 조각이 아님")
    };
    let paths: Vec<&str> = page.hits.iter().map(|h| h.path.as_str()).collect();
    assert_eq!(paths, [file_path.as_str()]);
    assert_eq!(page.hits[0].size, 123);
    drop(pipe);

    // 시험용 질의 명령으로도 찾는다.
    let out = Command::new(&query)
        .args(["search", &unique])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "facet-query 실패: {stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains(&format!("{file_path}\t123")),
        "facet-query 출력에 결과가 없음:\n{stdout}"
    );
    let out = Command::new(&query).arg("status").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(stdout.contains(&format!("{volume}\t준비됨")), "{stdout}");

    // 서비스가 USN 저널을 따라간다: 새로 만든 파일이 1초 안에 시험용 질의 명령의 결과에 나온다.
    let fresh = format!("서비스 새 파일 {}.txt", std::process::id());
    let fresh_path = scratch.file(&fresh, 7).display().to_string();
    let changed = Instant::now();
    loop {
        let out = Command::new(&query)
            .args(["search", &fresh])
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let elapsed = changed.elapsed();
        if stdout.contains(&fresh_path) && elapsed < LIVE_UPDATE {
            eprintln!("서비스 반영: {}ms", elapsed.as_millis());
            break;
        }
        assert!(
            elapsed < LIVE_UPDATE,
            "새 파일이 1초 안에 facet-query 결과에 나오지 않음:\n{stdout}"
        );
        thread::sleep(Duration::from_millis(20));
    }

    // 시험 볼륨의 인덱스가 인덱스 폴더에 저장되고, 폴더는 SYSTEM과 Administrators만 쓴다.
    let dir = index_dir::default_dir();
    let saved = dir.join(follow::file_name(&volume));
    wait_until("인덱스 파일 저장", Duration::from_secs(60), || {
        saved.exists()
    });
    assert_locked_down(&dir);
    assert_locked_down(dir.parent().unwrap());
    assert_only_system_and_administrators(&saved);

    // 서비스 중지 → 파일 바꾸기 → 시작 → 꺼져 있던 동안의 변경이 반영된다.
    sc(&["stop", SERVICE_NAME]);
    wait_until("서비스 멈춤", Duration::from_secs(60), || {
        sc_state().as_deref() == Some("STOPPED")
    });
    let stopped_bytes = fs::read(&saved).unwrap();
    let offline = format!("꺼진 동안 {}.txt", std::process::id());
    let offline_path = scratch.file(&offline, 9).display().to_string();
    fs::remove_file(&file).unwrap();
    sc(&["start", SERVICE_NAME]);
    let items = wait_ready_via_pipe(&volume);
    assert!(items > 0);
    wait_until(
        "꺼져 있던 동안의 변경 반영",
        Duration::from_secs(30),
        || {
            let made = search_output(&query, &offline);
            let gone = search_output(&query, &unique);
            made.contains(&offline_path) && !gone.contains(&file_path)
        },
    );
    // 저장한 인덱스를 읽어 따라잡았다: 다시 만들었다면 만든 즉시 새 인덱스로 파일을 바꿨을 것이다.
    assert_eq!(
        fs::read(&saved).unwrap(),
        stopped_bytes,
        "다시 뜬 서비스가 저장한 인덱스를 쓰지 않고 다시 만듦"
    );

    attach_and_detach_volumes_while_the_service_runs(&query, &volume);

    let removed = Command::new(env!("CARGO_BIN_EXE_facet-indexer"))
        .arg("uninstall")
        .status()
        .unwrap();
    assert!(removed.success(), "uninstall 실패: {removed}");
    assert!(!service_exists(), "제거한 뒤에도 서비스가 남아 있음");
    assert!(!dir.exists(), "제거한 뒤에도 인덱스 폴더가 남아 있음");
    // 서비스가 멈췄으므로 파이프도 없다.
    let out = Command::new(&query).arg("status").output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "서비스가 없다는 종료 코드가 아님"
    );
    drop(guard);
}
