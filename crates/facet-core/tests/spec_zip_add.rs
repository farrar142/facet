// target: crates/facet-core/tests/spec_zip_add.rs
// AC: #430: zip(안 폴더)을 대상으로 한 복사가 넣기(옛 항목 그대로·새 항목), 이름 충돌과 폴더 합치기, 실패·취소에 원래 zip 그대로·임시 파일 없음, 원래 zip은 휴지통, 암호 zip·이동 거부, 반대편 패널로 복사가 넣기·이동은 꺼짐, 작업 기록과 실행취소, 「zip에 넣기…」
//! 기존 zip에 넣기를 임시 폴더의 실제 파일로 확인한다. 명령 목록·id·팔레트는 모든 플랫폼에서, 작업 대기열의 넣기
//! 작업(대상이 zip 파일 경로나 zip 항목 경로인 `JobQueue::copy`)과 앱의 반대편 패널로 복사·「zip에 넣기…」·작업
//! 기록·실행취소는 Windows에서만 돈다(작업과 휴지통이 Windows 전용). 결과 zip은 facet의 리더로 다시 읽고, 원래
//! zip이 그대로인지는 바이트로 견준다. 휴지통에 넣은 항목은 끝날 때 휴지통에서 지운다.

#[path = "spec_zip_support.rs"]
mod zip_support;

#[cfg(windows)]
#[path = "common/bin.rs"]
mod bin;

use facet_core::command::CommandId;
use facet_core::window::App;

#[test]
#[ignore = "#430"]
fn add_to_zip_is_a_palette_command() {
    assert_eq!(CommandId::AddToZip.as_str(), "add-to-zip");
    assert!(CommandId::ALL.contains(&CommandId::AddToZip));
    assert!(
        CommandId::AddToZip.shortcuts().is_empty(),
        "기본 단축키가 없다"
    );
    let app = App::new(None);
    assert!(
        app.search_commands("", |command| command.as_str())
            .iter()
            .any(|entry| entry.command == CommandId::AddToZip)
    );
}

#[cfg(windows)]
mod run {
    use std::collections::BTreeSet;
    use std::fs;
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use facet_core::Location;
    use facet_core::command::CommandId;
    use facet_core::history::OperationKind;
    use facet_core::job::{
        Conflict, ConflictAnswer, CopyRejection, ErrorAnswer, ItemFailure, JobEvent, JobId,
        JobKind, JobQueue, JobState, Resolution, Summary,
    };
    use facet_core::tab::{Navigation, ReadOutcome};
    use facet_core::window::{App, Execution};
    use facet_core::zip_archive::Archive;
    use tempfile::TempDir;

    use super::bin;
    use super::zip_support::{self, DEFLATE, Entry};

    const WAIT: Duration = Duration::from_secs(300);

    struct Fixture {
        dir: TempDir,
        zip: PathBuf,
        src: PathBuf,
    }

    fn old_text() -> Vec<u8> {
        zip_support::repeated_text(8)
    }

    /// `zips/자료.zip`(old.txt(Deflate), docs/readme.txt)과 넣을 원본 폴더 `src`(new.txt, readme.txt,
    /// newdir/x.txt, docs/new2.txt).
    fn fixture() -> Fixture {
        let dir = TempDir::new().unwrap();
        let zip = dir.path().join("zips").join("자료.zip");
        zip_support::write_zip(
            &zip,
            &[
                Entry::deflated("old.txt", &old_text()),
                Entry::stored("docs/readme.txt", b"old readme"),
            ],
        );
        let src = dir.path().join("src");
        fs::create_dir_all(src.join("newdir")).unwrap();
        fs::create_dir_all(src.join("docs")).unwrap();
        fs::write(src.join("new.txt"), "new").unwrap();
        fs::write(src.join("readme.txt"), "new readme").unwrap();
        fs::write(src.join("newdir").join("x.txt"), "x").unwrap();
        fs::write(src.join("docs").join("new2.txt"), "new2").unwrap();
        Fixture { dir, zip, src }
    }

    fn names(archive: &Archive, folder: &str) -> Vec<String> {
        let mut names: Vec<String> = archive
            .children(folder)
            .unwrap_or_else(|| panic!("{folder:?} 폴더"))
            .into_iter()
            .map(|e| e.name)
            .collect();
        names.sort();
        names
    }

    fn read(zip: &Path, path: &str) -> Vec<u8> {
        Archive::open(zip)
            .expect("zip을 다시 읽는다")
            .read_entry(path, None)
            .unwrap_or_else(|e| panic!("{path}: {e:?}"))
    }

    fn listing(dir: &Path) -> BTreeSet<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }

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

    fn finish(queue: &mut JobQueue, job: JobId) -> (JobState, Summary) {
        match next(queue, job) {
            Next::Finished(summary) => (queue.state(job).unwrap(), summary),
            Next::Conflict(conflict) => panic!("묻지 않아야 한다: {conflict:?}"),
            Next::Error(error) => panic!("오류가 나지 않아야 한다: {error:?}"),
        }
    }

    fn conflict(queue: &mut JobQueue, job: JobId) -> Conflict {
        match next(queue, job) {
            Next::Conflict(conflict) => conflict,
            Next::Error(error) => panic!("이름 충돌이어야 한다: {error:?}"),
            Next::Finished(_) => panic!("묻기 전에 끝났다"),
        }
    }

    #[test]
    #[ignore = "#430"]
    fn copying_into_a_zip_adds_entries_and_keeps_the_old_ones() {
        let f = fixture();
        let before = Archive::open(&f.zip).unwrap();
        let old_compressed = before
            .children("")
            .unwrap()
            .into_iter()
            .find(|e| e.name == "old.txt")
            .unwrap()
            .compressed_size;
        drop(before);

        let mut queue = JobQueue::new();
        let job = queue
            .copy(
                vec![f.src.join("new.txt"), f.src.join("newdir")],
                f.zip.clone(),
            )
            .expect("넣기 작업을 만든다");
        let (state, summary) = finish(&mut queue, job);
        assert_eq!(state, JobState::Finished);
        assert!(
            summary.recycled.iter().any(|item| item.original() == f.zip),
            "원래 zip은 휴지통으로 갔다: {:?}",
            summary.recycled
        );

        let archive = Archive::open(&f.zip).expect("결과 zip을 읽는다");
        assert_eq!(
            names(&archive, ""),
            ["docs", "new.txt", "newdir", "old.txt"]
        );
        assert_eq!(names(&archive, "newdir"), ["x.txt"]);
        let old = archive
            .children("")
            .unwrap()
            .into_iter()
            .find(|e| e.name == "old.txt")
            .unwrap();
        assert_eq!(old.method, DEFLATE);
        assert_eq!(
            old.compressed_size, old_compressed,
            "옛 항목은 다시 압축하지 않고 그대로 옮긴다"
        );
        assert_eq!(archive.read_entry("old.txt", None), Ok(old_text()));
        assert_eq!(
            archive.read_entry("docs/readme.txt", None),
            Ok(b"old readme".to_vec())
        );
        assert_eq!(archive.read_entry("new.txt", None), Ok(b"new".to_vec()));
        assert_eq!(archive.read_entry("newdir/x.txt", None), Ok(b"x".to_vec()));
        assert_eq!(
            listing(f.zip.parent().unwrap()),
            BTreeSet::from(["자료.zip".to_string()]),
            "임시 파일이 남지 않는다"
        );
    }

    #[test]
    #[ignore = "#430"]
    fn copying_into_a_folder_inside_the_zip_adds_there() {
        let f = fixture();
        let mut queue = JobQueue::new();
        let job = queue
            .copy(vec![f.src.join("new.txt")], f.zip.join("docs"))
            .unwrap();
        assert_eq!(finish(&mut queue, job).0, JobState::Finished);
        assert_eq!(read(&f.zip, "docs/new.txt"), b"new");
        assert_eq!(read(&f.zip, "docs/readme.txt"), b"old readme");
    }

    #[test]
    #[ignore = "#430"]
    fn conflicts_inside_the_zip_and_merged_folders() {
        let mut queue = JobQueue::new();

        // 덮어쓰기: 옛 항목을 빼고 새 항목을 쓴다.
        let f = fixture();
        let job = queue
            .copy(vec![f.src.join("readme.txt")], f.zip.join("docs"))
            .unwrap();
        let asked = conflict(&mut queue, job);
        assert!(asked.can_overwrite);
        queue
            .answer_conflict(job, ConflictAnswer::once(Resolution::Overwrite))
            .unwrap();
        assert_eq!(finish(&mut queue, job).0, JobState::Finished);
        let archive = Archive::open(&f.zip).unwrap();
        assert_eq!(archive.duplicates(), 0, "같은 경로가 둘 남지 않는다");
        assert_eq!(names(&archive, "docs"), ["readme.txt"]);
        assert_eq!(read(&f.zip, "docs/readme.txt"), b"new readme");

        // 건너뛰기
        let f = fixture();
        let job = queue
            .copy(vec![f.src.join("readme.txt")], f.zip.join("docs"))
            .unwrap();
        let _ = conflict(&mut queue, job);
        queue
            .answer_conflict(job, ConflictAnswer::once(Resolution::Skip))
            .unwrap();
        finish(&mut queue, job);
        assert_eq!(read(&f.zip, "docs/readme.txt"), b"old readme");

        // 둘 다 유지
        let f = fixture();
        let job = queue
            .copy(vec![f.src.join("readme.txt")], f.zip.join("docs"))
            .unwrap();
        let _ = conflict(&mut queue, job);
        queue
            .answer_conflict(job, ConflictAnswer::once(Resolution::KeepBoth))
            .unwrap();
        finish(&mut queue, job);
        assert_eq!(read(&f.zip, "docs/readme.txt"), b"old readme");
        assert_eq!(read(&f.zip, "docs/readme (2).txt"), b"new readme");

        // 같은 이름 폴더는 묻지 않고 합친다.
        let f = fixture();
        let job = queue.copy(vec![f.src.join("docs")], f.zip.clone()).unwrap();
        assert_eq!(finish(&mut queue, job).0, JobState::Finished);
        let archive = Archive::open(&f.zip).unwrap();
        assert_eq!(names(&archive, "docs"), ["new2.txt", "readme.txt"]);
        assert_eq!(read(&f.zip, "docs/readme.txt"), b"old readme");
        assert!(f.dir.path().exists());
    }

    #[test]
    #[ignore = "#430"]
    fn a_failed_or_cancelled_add_leaves_the_zip_untouched() {
        let f = fixture();
        let original = fs::read(&f.zip).unwrap();
        let folder = f.zip.parent().unwrap().to_path_buf();
        let mut queue = JobQueue::new();

        // 잠긴 원본에서 작업 취소.
        let locked = f.src.join("잠김.txt");
        fs::write(&locked, "locked").unwrap();
        let handle = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(&locked)
            .unwrap();
        let job = queue
            .copy(vec![f.src.join("new.txt"), locked.clone()], f.zip.clone())
            .unwrap();
        let error = match next(&mut queue, job) {
            Next::Error(error) => error,
            Next::Conflict(conflict) => panic!("충돌이 아니다: {conflict:?}"),
            Next::Finished(summary) => panic!("잠긴 원본을 묻지 않고 끝났다: {summary:?}"),
        };
        assert_eq!(error.source, locked);
        queue.answer_error(job, ErrorAnswer::CancelJob).unwrap();
        let (state, _) = finish(&mut queue, job);
        assert_eq!(state, JobState::Cancelled);
        drop(handle);
        assert_eq!(
            fs::read(&f.zip).unwrap(),
            original,
            "원래 zip이 바이트 단위로 그대로다"
        );
        assert_eq!(
            listing(&folder),
            BTreeSet::from(["자료.zip".to_string()]),
            "임시 파일이 남지 않는다"
        );

        // 쓰는 도중 취소.
        let big = f.src.join("큰.bin");
        let body: Vec<u8> = (0..64 * 1024 * 1024u32)
            .map(|n| (n.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        fs::write(&big, &body).unwrap();
        let job = queue.copy(vec![big], f.zip.clone()).unwrap();
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
        assert_eq!(state, JobState::Cancelled);
        assert_eq!(
            fs::read(&f.zip).unwrap(),
            original,
            "원래 zip이 바이트 단위로 그대로다"
        );
        assert_eq!(listing(&folder), BTreeSet::from(["자료.zip".to_string()]));
    }

    #[test]
    #[ignore = "#430"]
    fn encrypted_zips_and_moves_are_refused() {
        let f = fixture();
        let secret = f.dir.path().join("zips").join("암호.zip");
        zip_support::write_zip(
            &secret,
            &[
                Entry::stored("비밀.txt", b"secret").zipcrypto("pw"),
                Entry::stored("공개.txt", b"open"),
            ],
        );
        let original = fs::read(&secret).unwrap();
        let mut queue = JobQueue::new();
        assert_eq!(
            queue.copy(vec![f.src.join("new.txt")], secret.clone()),
            Err(CopyRejection::EncryptedArchive)
        );
        assert_eq!(fs::read(&secret).unwrap(), original);

        let zip_bytes = fs::read(&f.zip).unwrap();
        assert_eq!(
            queue.move_items(vec![f.src.join("new.txt")], f.zip.clone()),
            Err(CopyRejection::Unsupported),
            "zip으로 이동은 하지 않는다"
        );
        assert_eq!(fs::read(&f.zip).unwrap(), zip_bytes);
        assert!(f.src.join("new.txt").is_file());
        assert!(!queue.has_unfinished());
    }

    fn app_at(location: Location) -> App {
        let mut app = App::new(Some(location));
        let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
        assert_eq!(outcome, ReadOutcome::Committed);
        app
    }

    fn select(app: &mut App, name: &str) {
        let tab = app.active_tab();
        let index = (0..tab.len())
            .find(|&i| tab.get(i).unwrap().name == name)
            .unwrap_or_else(|| panic!("{name}가 목록에 없다"));
        app.active_tab_mut().select_only(index);
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
    #[ignore = "#430"]
    fn copying_to_a_zip_in_the_other_panel_adds_and_undo_restores() {
        let f = fixture();
        let original = fs::read(&f.zip).unwrap();
        let mut app = app_at(Location::file_system(&f.src));
        app.split_panel().expect("패널을 나눈다");
        let _ = app.wait(WAIT);
        assert!(app.navigate(Navigation::Go(Location::zip(&f.zip, "docs"))));
        let (_, outcome) = app.wait(WAIT).expect("zip 안 읽기");
        assert_eq!(outcome, ReadOutcome::Committed);
        assert!(app.activate_panel(0));

        select(&mut app, "new.txt");
        assert!(
            !app.can_execute(CommandId::MoveToOtherPanel),
            "zip으로 이동은 꺼진다"
        );
        let plan = app.transfer_plan(JobKind::Copy).expect("넣을 수 있다");
        assert_eq!(plan.target, f.zip.join("docs"));
        assert_eq!(plan.sources, [f.src.join("new.txt")]);
        let job = app.start_transfer(&plan).expect("작업을 넣는다");
        assert_eq!(wait_job(&mut app, job).0, JobState::Finished);
        assert_eq!(read(&f.zip, "docs/new.txt"), b"new");
        let record = app.history().records().next_back().expect("작업 기록");
        assert_eq!(record.kind(), OperationKind::AddToZip);

        let undo = app.undo().expect("실행취소");
        assert_eq!(wait_job(&mut app, undo).0, JobState::Finished);
        assert_eq!(
            fs::read(&f.zip).unwrap(),
            original,
            "실행취소하면 넣기 전 zip과 바이트 단위로 같다"
        );
    }

    #[test]
    #[ignore = "#430"]
    fn add_to_zip_chooses_a_zip_and_adds() {
        let f = fixture();
        let mut app = app_at(Location::file_system(&f.src));
        select(&mut app, "new.txt");
        assert_eq!(app.execute(CommandId::AddToZip), Execution::ChooseZipTarget);

        assert!(
            app.add_to_zip_plan(&f.src.join("readme.txt").to_string_lossy())
                .is_err(),
            "zip이 아니면 오류"
        );
        let plan = app
            .add_to_zip_plan(&f.zip.to_string_lossy())
            .expect("zip 경로를 해석한다");
        assert_eq!(plan.kind, JobKind::Copy);
        assert_eq!(plan.target, f.zip);
        assert_eq!(plan.sources, [f.src.join("new.txt")]);
        let job = app.start_transfer(&plan).unwrap();
        assert_eq!(wait_job(&mut app, job).0, JobState::Finished);
        assert_eq!(read(&f.zip, "new.txt"), b"new");
    }
}
