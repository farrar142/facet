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
use std::thread;
use std::time::{Duration, Instant};

use facet_index::ATTRIBUTE_DIRECTORY;
use facet_indexer::SERVICE_NAME;
use facet_indexer::follow::Stop;
use facet_indexer::service::index_volumes;
use facet_indexer::session::Cancel;
use facet_indexer::volume::local_volumes;
use facet_indexer::volumes::{Plan, Volumes};
use facet_protocol::{
    Case, Condition, PIPE_CLIENT_ACCESS, PIPE_NAME, PROTOCOL_VERSION, QueryMessage, Request,
    Response, VolumeState, read_message, write_message,
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
        let threads = index_volumes(&volumes, targets, &stop);
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
    // 되풀이한다. 다른 볼륨(C: 등)은 아직 만드는 중이어도 된다.
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
            .unwrap_or_else(|| panic!("상태에 {volume}이 없음: {status:?}"))
            .state
            .clone();
        match state {
            VolumeState::Ready { items } => {
                assert!(items > 0);
                break;
            }
            VolumeState::Building { .. } => {}
            other => panic!("{volume}의 상태가 {other:?}"),
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

    let removed = Command::new(env!("CARGO_BIN_EXE_facet-indexer"))
        .arg("uninstall")
        .status()
        .unwrap();
    assert!(removed.success(), "uninstall 실패: {removed}");
    assert!(!service_exists(), "제거한 뒤에도 서비스가 남아 있음");
    // 서비스가 멈췄으므로 파이프도 없다.
    let out = Command::new(&query).arg("status").output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "서비스가 없다는 종료 코드가 아님"
    );
    drop(guard);
}
