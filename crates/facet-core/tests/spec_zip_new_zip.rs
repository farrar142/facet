// target: crates/facet-core/tests/spec_zip_new_zip.rs
// AC: #429: 팔레트와 꺼짐, 여러 항목을 현재 폴더 이름의 zip으로(내용·빈 폴더·UTF-8 표시·Deflate), 항목 하나의 이름과 번호, take_new_zip과 커서, 수정 시각, 작업 기록과 실행취소, zip64
//! 「새 zip으로 묶기」를 확인한다. 명령 목록·id·팔레트는 모든 플랫폼에서, 임시 폴더의 실제 파일을 묶어 facet의 리더와
//! central directory 바이트로 결과를 다시 읽는 것·이름 규칙·커서·작업 기록·실행취소·zip64는 Windows에서만 돈다
//! (작업과 휴지통이 Windows 전용).

#[path = "spec_zip_support.rs"]
mod zip_support;

#[cfg(windows)]
#[path = "common/bin.rs"]
mod bin;

use facet_core::command::CommandId;
use facet_core::window::App;

#[test]
fn new_zip_is_a_palette_command() {
    assert_eq!(CommandId::NewZip.as_str(), "new-zip");
    assert!(CommandId::ALL.contains(&CommandId::NewZip));
    assert!(
        CommandId::NewZip.shortcuts().is_empty(),
        "기본 단축키가 없다"
    );
    let app = App::new(None);
    assert!(
        app.search_commands("", |command| command.as_str())
            .iter()
            .any(|entry| entry.command == CommandId::NewZip)
    );
}

#[cfg(windows)]
mod run {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant, SystemTime};

    use facet_core::Location;
    use facet_core::command::CommandId;
    use facet_core::history::OperationKind;
    use facet_core::job::{JobId, JobState, Summary};
    use facet_core::tab::ReadOutcome;
    use facet_core::window::{App, Execution};
    use facet_core::zip_archive::Archive;
    use tempfile::TempDir;

    use super::bin;
    use super::zip_support::{self, DEFLATE, Entry, FLAG_UTF8};

    const WAIT: Duration = Duration::from_secs(300);
    const A_TIME: u64 = 1_600_000_000;

    fn app_at(location: Location) -> App {
        let mut app = App::new(Some(location));
        let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
        assert_eq!(outcome, ReadOutcome::Committed);
        app
    }

    fn index_of(app: &App, name: &str) -> usize {
        let tab = app.active_tab();
        (0..tab.len())
            .find(|&i| tab.get(i).unwrap().name == name)
            .unwrap_or_else(|| panic!("{name}가 목록에 없다"))
    }

    fn cursor_name(app: &App) -> Option<String> {
        let tab = app.active_tab();
        tab.cursor()
            .and_then(|i| tab.get(i))
            .map(|item| item.name.to_string_lossy().into_owned())
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

    /// 「새 zip으로 묶기」를 실행해 넣은 작업이 끝나기를 기다린다.
    fn new_zip(app: &mut App) -> JobState {
        let before = app.work_area().entries().len();
        assert!(app.can_execute(CommandId::NewZip));
        assert_eq!(app.execute(CommandId::NewZip), Execution::Done);
        app.poll_jobs();
        let entries = app.work_area().entries();
        assert_eq!(entries.len(), before + 1, "작업을 하나 넣었다");
        let job = entries.last().unwrap().id;
        wait_job(app, job).0
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

    fn set_modified(path: &Path, secs: u64) {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }

    /// `묶음 폴더`: a.txt, empty.txt, sub/b.txt, 한글.txt, 빈(빈 폴더).
    fn bundle_folder(dir: &TempDir) -> PathBuf {
        let folder = dir.path().join("묶음 폴더");
        fs::create_dir_all(folder.join("sub")).unwrap();
        fs::create_dir_all(folder.join("빈")).unwrap();
        fs::write(folder.join("a.txt"), zip_support::repeated_text(8)).unwrap();
        set_modified(&folder.join("a.txt"), A_TIME);
        fs::write(folder.join("empty.txt"), b"").unwrap();
        fs::write(folder.join("sub").join("b.txt"), "비 내용").unwrap();
        fs::write(folder.join("한글.txt"), "한글 내용").unwrap();
        folder
    }

    #[test]
    fn several_items_become_a_zip_named_after_the_folder() {
        let dir = TempDir::new().unwrap();
        let folder = bundle_folder(&dir);
        let mut app = app_at(Location::file_system(&folder));
        app.active_tab_mut().select_all();
        assert_eq!(new_zip(&mut app), JobState::Finished);

        let zip = folder.join("묶음 폴더.zip");
        assert!(zip.is_file(), "현재 폴더 이름의 zip");
        let archive = Archive::open(&zip).expect("다시 읽는다");
        assert_eq!(
            names(&archive, ""),
            ["a.txt", "empty.txt", "sub", "빈", "한글.txt"]
        );
        assert_eq!(names(&archive, "sub"), ["b.txt"]);
        assert!(
            archive.children("빈").unwrap().is_empty(),
            "빈 폴더도 담는다"
        );
        assert_eq!(
            archive.read_entry("a.txt", None),
            Ok(zip_support::repeated_text(8))
        );
        assert_eq!(
            archive.read_entry("sub/b.txt", None),
            Ok("비 내용".as_bytes().to_vec())
        );
        assert_eq!(
            archive.read_entry("한글.txt", None),
            Ok("한글 내용".as_bytes().to_vec())
        );
        assert_eq!(archive.read_entry("empty.txt", None), Ok(Vec::new()));

        let a = archive
            .children("")
            .unwrap()
            .into_iter()
            .find(|e| e.name == "a.txt")
            .unwrap();
        let written = a.modified.expect("수정 시각");
        let expected = SystemTime::UNIX_EPOCH + Duration::from_secs(A_TIME);
        let gap = written
            .duration_since(expected)
            .unwrap_or_else(|e| e.duration());
        assert!(gap <= Duration::from_secs(2), "수정 시각을 남긴다: {gap:?}");

        let raw = fs::read(&zip).unwrap();
        for (name, flags, method) in zip_support::central_entries(&raw) {
            let name = String::from_utf8(name).expect("이름은 UTF-8");
            assert!(flags & FLAG_UTF8 != 0, "{name}에 UTF-8 표시");
            if ["a.txt", "sub/b.txt", "한글.txt"].contains(&name.as_str()) {
                assert_eq!(method, DEFLATE, "{name}은 Deflate");
            }
        }

        assert_eq!(
            app.take_new_zip(),
            Some(zip.clone()),
            "만든 zip을 한 번 준다"
        );
        assert_eq!(app.take_new_zip(), None);
        let deadline = Instant::now() + WAIT;
        while cursor_name(&app).as_deref() != Some("묶음 폴더.zip") {
            assert!(Instant::now() < deadline, "만든 zip이 커서가 되지 않았다");
            let _ = app.wait_update(Duration::from_millis(50));
            app.poll();
        }
    }

    #[test]
    fn a_single_item_names_the_zip_and_numbers_avoid_clashes() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("보고서.txt"), "보고서").unwrap();
        fs::create_dir(dir.path().join("사진")).unwrap();
        fs::write(dir.path().join("사진").join("1.jpg"), "jpg").unwrap();
        let mut app = app_at(Location::file_system(dir.path()));

        let index = index_of(&app, "보고서.txt");
        app.active_tab_mut().select_only(index);
        assert_eq!(new_zip(&mut app), JobState::Finished);
        assert!(
            dir.path().join("보고서.zip").is_file(),
            "파일은 확장자를 뺀 이름"
        );

        let index = index_of(&app, "사진");
        app.active_tab_mut().select_only(index);
        assert_eq!(new_zip(&mut app), JobState::Finished);
        let photos = Archive::open(&dir.path().join("사진.zip")).expect("폴더 이름의 zip");
        assert_eq!(names(&photos, ""), ["사진"]);
        assert_eq!(names(&photos, "사진"), ["1.jpg"]);

        let index = index_of(&app, "보고서.txt");
        app.active_tab_mut().select_only(index);
        assert_eq!(new_zip(&mut app), JobState::Finished);
        assert!(
            dir.path().join("보고서 (2).zip").is_file(),
            "이름이 겹치면 번호를 붙인다"
        );
    }

    #[test]
    fn new_zip_is_off_without_items_or_outside_real_folders() {
        let dir = TempDir::new().unwrap();
        let empty = dir.path().join("빈");
        fs::create_dir(&empty).unwrap();
        let app = app_at(Location::file_system(&empty));
        assert!(!app.can_execute(CommandId::NewZip), "원본이 없다");

        let zip = dir.path().join("자료.zip");
        zip_support::write_zip(&zip, &[Entry::stored("a.txt", b"a")]);
        let mut app = app_at(Location::zip(&zip, ""));
        let index = index_of(&app, "a.txt");
        app.active_tab_mut().select_only(index);
        assert!(!app.can_execute(CommandId::NewZip), "zip 안에서는 꺼진다");
    }

    #[test]
    fn new_zip_is_recorded_and_undone() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("보고서.txt"), "보고서").unwrap();
        let mut app = app_at(Location::file_system(dir.path()));
        let index = index_of(&app, "보고서.txt");
        app.active_tab_mut().select_only(index);
        assert_eq!(new_zip(&mut app), JobState::Finished);
        let zip = dir.path().join("보고서.zip");
        assert!(zip.is_file());
        let record = app.history().records().next_back().expect("작업 기록");
        assert_eq!(record.kind(), OperationKind::NewZip);

        let undo = app.undo().expect("실행취소");
        assert_eq!(wait_job(&mut app, undo).0, JobState::Finished);
        assert!(!zip.exists(), "만든 zip이 사라졌다");
        assert!(dir.path().join("보고서.txt").is_file(), "원본은 그대로다");
    }

    #[test]
    fn more_than_65535_items_are_written_as_zip64() {
        const COUNT: usize = 65_600;
        let dir = TempDir::new().unwrap();
        let many = dir.path().join("많음");
        fs::create_dir(&many).unwrap();
        for i in 0..COUNT {
            fs::write(many.join(format!("{i:05}.txt")), b"").unwrap();
        }
        let mut app = app_at(Location::file_system(dir.path()));
        let index = index_of(&app, "많음");
        app.active_tab_mut().select_only(index);
        assert_eq!(new_zip(&mut app), JobState::Finished);

        let zip = dir.path().join("많음.zip");
        let archive = Archive::open(&zip).expect("zip64를 다시 읽는다");
        assert_eq!(archive.children("많음").unwrap().len(), COUNT);
        let raw = fs::read(&zip).unwrap();
        let files = zip_support::central_entries(&raw)
            .into_iter()
            .filter(|(name, _, _)| !name.ends_with(b"/"))
            .count();
        assert_eq!(files, COUNT, "zip64 끝 기록으로 모든 항목을 찾는다");
    }
}
