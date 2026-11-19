// target: crates/facet-core/tests/spec_zip_extract.rs
// AC: #425: read_entry(Stored·Deflate·Corrupt·UnsupportedMethod·NotAFile·NotFound·PasswordNeeded), zip 항목 복사가 꺼내기(내용·시각·중간 폴더), extract_all, 이름 충돌 네 가지와 나머지에도 적용, 일시정지·취소, 지원하지 않는 방식 건너뛰기, CRC·크기 오류 질문, 반대편 패널로 꺼내기와 작업 기록·실행취소
//! zip 항목 꺼내기를 임시 폴더의 실제 zip 파일로 확인한다. 리더의 `Archive::read_entry`(풀기와 오류 값)는 모든
//! 플랫폼에서, 작업 대기열의 꺼내기 작업(zip 항목 경로를 원본으로 한 `JobQueue::copy`와 `JobQueue::extract_all`)과
//! 앱의 반대편 패널로 복사·작업 기록·실행취소는 Windows에서만 돈다(파일 연산·휴지통이 Windows 전용). 덮어쓰기와
//! 실행취소가 휴지통에 넣은 항목은 끝날 때 휴지통에서 지운다.

#[path = "spec_zip_support.rs"]
mod zip_support;

#[cfg(windows)]
#[path = "common/bin.rs"]
mod bin;

use std::path::PathBuf;

use facet_core::zip_archive::{Archive, EntryError};
use tempfile::TempDir;
use zip_support::{BZIP2, Entry};

fn archive_of(entries: &[Entry]) -> (TempDir, PathBuf, Archive) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("자료.zip");
    zip_support::write_zip(&path, entries);
    let archive = Archive::open(&path).expect("zip을 연다");
    (dir, path, archive)
}

#[test]
#[ignore = "#425"]
fn read_entry_unpacks_stored_and_deflate() {
    let text = zip_support::repeated_text(30);
    let (_dir, _path, archive) = archive_of(&[
        Entry::stored("문서/저장.txt", "그대로 저장".as_bytes()),
        Entry::deflated("문서/압축.txt", &text),
        Entry::stored("빈.txt", b""),
    ]);
    assert_eq!(
        archive.read_entry("문서/저장.txt", None),
        Ok("그대로 저장".as_bytes().to_vec())
    );
    assert_eq!(archive.read_entry("문서/압축.txt", None), Ok(text));
    assert_eq!(archive.read_entry("빈.txt", None), Ok(Vec::new()));
}

#[test]
#[ignore = "#425"]
fn read_entry_reports_errors_as_values() {
    let bad_crc = Entry {
        crc: 0xDEAD_BEEF,
        ..Entry::stored("crc.txt", b"checksum does not match")
    };
    let too_long = Entry {
        size: 10,
        ..Entry::stored("long.txt", &[b'x'; 100])
    };
    let bzip = Entry {
        method: BZIP2,
        ..Entry::stored("bzip.bin", b"pretend bzip2 data")
    };
    let broken_deflate = Entry {
        data: vec![0xFF; 32],
        ..Entry::deflated("broken.txt", b"some text that was deflated")
    };
    let (_dir, _path, archive) = archive_of(&[
        bad_crc,
        too_long,
        bzip,
        broken_deflate,
        Entry::folder("폴더"),
        Entry::stored("폴더/안.txt", b"in"),
        Entry::stored("암호.txt", b"secret").zipcrypto("pw"),
    ]);
    assert_eq!(
        archive.read_entry("crc.txt", None),
        Err(EntryError::Corrupt)
    );
    assert_eq!(
        archive.read_entry("long.txt", None),
        Err(EntryError::Corrupt),
        "실제로 풀린 크기가 선언 크기를 넘는다"
    );
    assert_eq!(
        archive.read_entry("bzip.bin", None),
        Err(EntryError::UnsupportedMethod(BZIP2))
    );
    assert_eq!(
        archive.read_entry("broken.txt", None),
        Err(EntryError::Corrupt),
        "깨진 압축 데이터"
    );
    assert_eq!(archive.read_entry("폴더", None), Err(EntryError::NotAFile));
    assert_eq!(
        archive.read_entry("없음.txt", None),
        Err(EntryError::NotFound)
    );
    assert_eq!(
        archive.read_entry("암호.txt", None),
        Err(EntryError::PasswordNeeded)
    );
    assert_eq!(archive.read_entry("폴더/안.txt", None), Ok(b"in".to_vec()));
}

#[cfg(windows)]
mod jobs {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant, SystemTime};

    use facet_core::Location;
    use facet_core::history::OperationKind;
    use facet_core::job::{
        Conflict, ConflictAnswer, ErrorAnswer, FileOpErrorKind, ItemFailure, JobEvent, JobId,
        JobKind, JobQueue, JobState, Resolution, Summary, ZipSkipReason, ZipSkipped,
    };
    use facet_core::tab::{Navigation, ReadOutcome};
    use facet_core::window::App;
    use tempfile::TempDir;

    use super::bin;
    use super::zip_support::{self, BZIP2, Entry};

    const WAIT: Duration = Duration::from_secs(120);
    const TOP_TIME: u32 = 1_600_000_000;
    const README_TIME: u32 = 1_650_000_000;
    const REPORT_TIME: u32 = 1_700_000_000;

    fn at(secs: u32) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(u64::from(secs))
    }

    fn modified(path: &Path) -> SystemTime {
        fs::metadata(path).unwrap().modified().unwrap()
    }

    /// 임시 폴더: `자료.zip`(top.txt, 문서/readme.txt(Deflate), 문서/2024/보고서.txt, 폴더 기록 없음)과 빈 `대상` 폴더.
    struct Fixture {
        dir: TempDir,
        zip: PathBuf,
        target: PathBuf,
    }

    fn readme_text() -> Vec<u8> {
        zip_support::repeated_text(20)
    }

    fn fixture() -> Fixture {
        let dir = TempDir::new().unwrap();
        let zip = dir.path().join("자료.zip");
        zip_support::write_zip(
            &zip,
            &[
                Entry::stored("top.txt", b"top body").modified_unix(TOP_TIME),
                Entry::deflated("문서/readme.txt", &readme_text()).modified_unix(README_TIME),
                Entry::stored("문서/2024/보고서.txt", "보고서 내용".as_bytes())
                    .modified_unix(REPORT_TIME),
            ],
        );
        let target = dir.path().join("대상");
        fs::create_dir(&target).unwrap();
        Fixture { dir, zip, target }
    }

    /// 작업의 다음 질문이나 끝남.
    enum Next {
        Conflict(Conflict),
        Error(ItemFailure),
        Finished(Summary),
    }

    fn next(queue: &mut JobQueue, job: JobId) -> Next {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match queue.wait_event(left) {
                Some(JobEvent::Conflict { job: id, conflict }) if id == job => {
                    return Next::Conflict(conflict);
                }
                Some(JobEvent::ItemError { job: id, error }) if id == job => {
                    return Next::Error(error);
                }
                Some(JobEvent::Finished { job: id, summary }) if id == job => {
                    bin::purge(&summary.recycled);
                    return Next::Finished(summary);
                }
                Some(_) => {}
                None => panic!("작업이 시간 안에 끝나지 않았다"),
            }
        }
    }

    /// 묻는 것 없이 끝나기를 기다린다.
    fn finish(queue: &mut JobQueue, job: JobId) -> (JobState, Summary) {
        match next(queue, job) {
            Next::Finished(summary) => (queue.state(job).expect("끝난 작업"), summary),
            Next::Conflict(conflict) => panic!("묻지 않아야 한다: {conflict:?}"),
            Next::Error(error) => panic!("오류가 나지 않아야 한다: {error:?}"),
        }
    }

    fn conflict(queue: &mut JobQueue, job: JobId) -> Conflict {
        match next(queue, job) {
            Next::Conflict(conflict) => conflict,
            Next::Error(error) => panic!("이름 충돌이어야 한다: {error:?}"),
            Next::Finished(_) => panic!("이름 충돌을 묻기 전에 끝났다"),
        }
    }

    #[test]
    #[ignore = "#425"]
    fn copying_zip_items_extracts_files_and_folders_with_their_times() {
        let f = fixture();
        let mut queue = JobQueue::new();
        let job = queue
            .copy(
                vec![f.zip.join("top.txt"), f.zip.join("문서")],
                f.target.clone(),
            )
            .expect("꺼내기 작업을 만든다");
        let (state, summary) = finish(&mut queue, job);
        assert_eq!(state, JobState::Finished, "{summary:?}");
        assert!(summary.zip_skipped.is_empty());

        assert_eq!(fs::read(f.target.join("top.txt")).unwrap(), b"top body");
        assert_eq!(modified(&f.target.join("top.txt")), at(TOP_TIME));
        let readme = f.target.join("문서").join("readme.txt");
        assert_eq!(fs::read(&readme).unwrap(), readme_text());
        assert_eq!(modified(&readme), at(README_TIME));
        let report = f.target.join("문서").join("2024").join("보고서.txt");
        assert_eq!(fs::read_to_string(&report).unwrap(), "보고서 내용");
        assert_eq!(modified(&report), at(REPORT_TIME));
        assert!(f.zip.exists(), "zip은 그대로다");
    }

    #[test]
    #[ignore = "#425"]
    fn extract_all_extracts_the_whole_zip() {
        let f = fixture();
        let mut queue = JobQueue::new();
        let job = queue
            .extract_all(f.zip.clone(), f.target.clone())
            .expect("꺼내기 작업을 만든다");
        let (state, _) = finish(&mut queue, job);
        assert_eq!(state, JobState::Finished);
        let mut names: Vec<String> = fs::read_dir(&f.target)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["top.txt", "문서"]);
        assert!(
            f.target
                .join("문서")
                .join("2024")
                .join("보고서.txt")
                .is_file()
        );
    }

    /// 충돌 시험: zip에 a.txt·b.txt(2020년), 대상에 a.txt(2000년, 더 오래됨)·b.txt(2100년, 더 새것).
    fn conflict_fixture() -> Fixture {
        let dir = TempDir::new().unwrap();
        let zip = dir.path().join("충돌.zip");
        zip_support::write_zip(
            &zip,
            &[
                Entry::stored("a.txt", b"zip a").modified_unix(TOP_TIME),
                Entry::stored("b.txt", b"zip b").modified_unix(TOP_TIME),
            ],
        );
        let target = dir.path().join("대상");
        fs::create_dir(&target).unwrap();
        for (name, body, secs) in [
            ("a.txt", "old a", 946_684_800u64),
            ("b.txt", "new b", 4_102_444_800),
        ] {
            let path = target.join(name);
            fs::write(&path, body).unwrap();
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
                .unwrap();
        }
        Fixture { dir, zip, target }
    }

    fn answer(queue: &mut JobQueue, job: JobId, answer: ConflictAnswer) {
        queue.answer_conflict(job, answer).expect("답을 받는다");
    }

    #[test]
    #[ignore = "#425"]
    fn conflicts_offer_the_four_answers() {
        // 덮어쓰기
        let f = conflict_fixture();
        let mut queue = JobQueue::new();
        let job = queue
            .copy(vec![f.zip.join("a.txt")], f.target.clone())
            .unwrap();
        let asked = conflict(&mut queue, job);
        assert_eq!(asked.target.path, f.target.join("a.txt"));
        assert!(asked.can_overwrite);
        answer(&mut queue, job, ConflictAnswer::once(Resolution::Overwrite));
        assert_eq!(finish(&mut queue, job).0, JobState::Finished);
        assert_eq!(fs::read_to_string(f.target.join("a.txt")).unwrap(), "zip a");

        // 건너뛰기
        let f = conflict_fixture();
        let job = queue
            .copy(vec![f.zip.join("a.txt")], f.target.clone())
            .unwrap();
        let _ = conflict(&mut queue, job);
        answer(&mut queue, job, ConflictAnswer::once(Resolution::Skip));
        let (_, summary) = finish(&mut queue, job);
        assert_eq!(summary.conflicts_skipped, 1);
        assert_eq!(fs::read_to_string(f.target.join("a.txt")).unwrap(), "old a");

        // 둘 다 유지
        let f = conflict_fixture();
        let job = queue
            .copy(vec![f.zip.join("a.txt")], f.target.clone())
            .unwrap();
        let _ = conflict(&mut queue, job);
        answer(&mut queue, job, ConflictAnswer::once(Resolution::KeepBoth));
        finish(&mut queue, job);
        assert_eq!(fs::read_to_string(f.target.join("a.txt")).unwrap(), "old a");
        assert_eq!(
            fs::read_to_string(f.target.join("a (2).txt")).unwrap(),
            "zip a"
        );

        // 더 새것만 덮어쓰기 + 나머지에도 적용: a는 zip이 더 새것, b는 대상이 더 새것.
        let f = conflict_fixture();
        let job = queue
            .copy(
                vec![f.zip.join("a.txt"), f.zip.join("b.txt")],
                f.target.clone(),
            )
            .unwrap();
        let _ = conflict(&mut queue, job);
        answer(
            &mut queue,
            job,
            ConflictAnswer::for_rest(Resolution::OverwriteIfNewer),
        );
        let (state, _) = finish(&mut queue, job);
        assert_eq!(state, JobState::Finished);
        assert_eq!(fs::read_to_string(f.target.join("a.txt")).unwrap(), "zip a");
        assert_eq!(fs::read_to_string(f.target.join("b.txt")).unwrap(), "new b");
        assert!(f.dir.path().exists());
    }

    fn big_entries() -> Vec<(String, Vec<u8>)> {
        (0..4u8)
            .map(|i| {
                let body: Vec<u8> = (0..16 * 1024 * 1024u32)
                    .map(|n| (n as u8).wrapping_mul(31).wrapping_add(i))
                    .collect();
                (format!("큰 파일 {i}.bin"), body)
            })
            .collect()
    }

    #[test]
    #[ignore = "#425"]
    fn pause_resume_and_cancel_leave_no_partial_files() {
        let dir = TempDir::new().unwrap();
        let zip = dir.path().join("큰.zip");
        let bodies = big_entries();
        let entries: Vec<Entry> = bodies
            .iter()
            .map(|(name, body)| Entry::stored(name, body))
            .collect();
        zip_support::write_zip(&zip, &entries);
        drop(entries);
        let target = dir.path().join("대상");
        fs::create_dir(&target).unwrap();

        // 일시정지·이어하기
        let mut queue = JobQueue::new();
        let job = queue.extract_all(zip.clone(), target.clone()).unwrap();
        assert!(queue.pause(job), "멈춘다");
        assert_eq!(queue.state(job), Some(JobState::Paused));
        assert!(queue.resume(job), "이어 간다");
        let (state, _) = finish(&mut queue, job);
        assert_eq!(state, JobState::Finished);
        for (name, body) in &bodies {
            assert_eq!(&fs::read(target.join(name)).unwrap(), body);
        }

        // 취소: 첫 진행이 오면 취소한다.
        let target = dir.path().join("취소 대상");
        fs::create_dir(&target).unwrap();
        let job = queue.extract_all(zip.clone(), target.clone()).unwrap();
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match queue.wait_event(left) {
                Some(JobEvent::Progress { job: id, .. }) if id == job => break,
                Some(JobEvent::Finished { job: id, .. }) if id == job => {
                    panic!("진행이 오기 전에 끝났다")
                }
                Some(_) => {}
                None => panic!("진행이 오지 않았다"),
            }
        }
        assert!(queue.cancel(job));
        let (state, _) = finish(&mut queue, job);
        assert!(
            matches!(state, JobState::Cancelled | JobState::Finished),
            "{state:?}"
        );
        for entry in fs::read_dir(&target).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            let body = bodies
                .iter()
                .find(|(n, _)| *n == name)
                .unwrap_or_else(|| panic!("zip에 없는 파일(쓰다 만 임시 파일?)이 남았다: {name}"));
            assert_eq!(
                fs::read(entry.path()).unwrap(),
                body.1,
                "남은 파일은 다 쓴 파일이다: {name}"
            );
        }
    }

    #[test]
    #[ignore = "#425"]
    fn unsupported_methods_are_skipped_and_listed() {
        let dir = TempDir::new().unwrap();
        let zip = dir.path().join("섞임.zip");
        zip_support::write_zip(
            &zip,
            &[
                Entry::stored("좋음.txt", b"good"),
                Entry {
                    method: BZIP2,
                    ..Entry::stored("폴더/bzip.bin", b"pretend bzip2")
                },
            ],
        );
        let target = dir.path().join("대상");
        fs::create_dir(&target).unwrap();
        let mut queue = JobQueue::new();
        let job = queue.extract_all(zip, target.clone()).unwrap();
        let (state, summary) = finish(&mut queue, job);
        assert_eq!(
            state,
            JobState::Failed,
            "건너뛴 항목이 있으면 실패로 끝난다"
        );
        assert_eq!(
            summary.zip_skipped,
            [ZipSkipped {
                entry: "폴더/bzip.bin".to_string(),
                reason: ZipSkipReason::UnsupportedMethod(BZIP2),
            }]
        );
        assert!(summary.items_skipped() >= 1);
        assert_eq!(fs::read(target.join("좋음.txt")).unwrap(), b"good");
        assert!(!target.join("폴더").join("bzip.bin").exists());
    }

    #[test]
    #[ignore = "#425"]
    fn corrupt_entries_are_asked_as_item_errors() {
        let dir = TempDir::new().unwrap();
        let zip = dir.path().join("상함.zip");
        zip_support::write_zip(
            &zip,
            &[
                Entry {
                    crc: 0x1234_5678,
                    ..Entry::stored("crc.txt", b"checksum mismatch")
                },
                Entry {
                    size: 10,
                    ..Entry::stored("long.txt", &[b'x'; 4096])
                },
                Entry::stored("좋음.txt", b"good"),
            ],
        );
        let target = dir.path().join("대상");
        fs::create_dir(&target).unwrap();
        let mut queue = JobQueue::new();
        let job = queue.extract_all(zip.clone(), target.clone()).unwrap();

        let mut asked = Vec::new();
        let summary = loop {
            match next(&mut queue, job) {
                Next::Error(error) => {
                    assert_eq!(error.kind, FileOpErrorKind::Corrupt, "{error:?}");
                    asked.push(error.source.clone());
                    queue.answer_error(job, ErrorAnswer::Skip).unwrap();
                }
                Next::Conflict(conflict) => panic!("충돌이 아니다: {conflict:?}"),
                Next::Finished(summary) => break summary,
            }
        };
        asked.sort();
        let mut expected = vec![zip.join("crc.txt"), zip.join("long.txt")];
        expected.sort();
        assert_eq!(asked, expected, "CRC 불일치와 선언 크기 초과를 묻는다");
        assert_eq!(queue.state(job), Some(JobState::Failed));
        assert_eq!(summary.failures.len(), 2);
        assert!(!target.join("crc.txt").exists(), "쓰다 만 파일을 지운다");
        assert!(!target.join("long.txt").exists(), "쓰다 만 파일을 지운다");
        assert_eq!(fs::read(target.join("좋음.txt")).unwrap(), b"good");
    }

    fn app_at(location: Location) -> App {
        let mut app = App::new(Some(location));
        let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
        assert_eq!(outcome, ReadOutcome::Committed);
        app
    }

    fn wait_job(app: &mut App, job: JobId) -> (JobState, Summary) {
        let deadline = Instant::now() + WAIT;
        loop {
            app.poll_jobs();
            app.poll();
            if let Some(entry) = app.work_area().entry(job)
                && let Some(summary) = &entry.summary
            {
                bin::purge(&summary.recycled);
                return (entry.state, summary.clone());
            }
            assert!(Instant::now() < deadline, "작업이 시간 안에 끝나지 않았다");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    #[ignore = "#425"]
    fn copying_to_the_other_panel_extracts_and_undo_removes_it() {
        let f = fixture();
        let mut app = app_at(Location::zip(&f.zip, "문서"));
        app.split_panel().expect("패널을 나눈다");
        let _ = app.wait(WAIT);
        assert!(app.navigate(Navigation::Go(Location::file_system(&f.target))));
        let (_, outcome) = app.wait(WAIT).expect("대상 폴더 읽기");
        assert_eq!(outcome, ReadOutcome::Committed);
        assert!(app.activate_panel(0));

        let tab = app.active_tab();
        let readme = (0..tab.len())
            .find(|&i| tab.get(i).unwrap().name == "readme.txt")
            .expect("readme.txt");
        app.active_tab_mut().select_only(readme);
        let plan = app.transfer_plan(JobKind::Copy).expect("꺼낼 수 있다");
        assert_eq!(plan.target, f.target);
        assert_eq!(plan.sources, [f.zip.join("문서").join("readme.txt")]);

        let job = app.start_transfer(&plan).expect("작업을 넣는다");
        let (state, _) = wait_job(&mut app, job);
        assert_eq!(state, JobState::Finished);
        let extracted = f.target.join("readme.txt");
        assert_eq!(fs::read(&extracted).unwrap(), readme_text());
        let record = app.history().records().next_back().expect("작업 기록");
        assert_eq!(record.kind(), OperationKind::Extract);

        let undo = app.undo().expect("실행취소를 시작한다");
        let (state, _) = wait_job(&mut app, undo);
        assert_eq!(state, JobState::Finished);
        assert!(!extracted.exists(), "꺼낸 파일이 사라졌다");
        assert!(f.zip.exists(), "zip은 그대로다");
        assert!(f.dir.path().exists());
    }
}
