// target: crates/facet-core/tests/spec_sync_folders_user_actions.rs
// AC: #456: 온라인 전용 파일 복사는 「내려받는 중」 진행이 있고 내용이 같으며 내용 요청이 생김, 이 장치에 있는 파일만 복사하면 없음
// AC: #456: 같은 동기화 폴더 안의 이동·이름바꾸기는 내용 요청 0이고 여전히 온라인 전용
// AC: #456: 동기화 폴더의 Delete는 확인 없이 휴지통 삭제 작업이고 in_sync_folder가 참, 일반 폴더는 거짓
// AC: #456: 미리보기가 온라인 전용을 거절하는 동안 「미리 보기 내려받기」가 켜지고 실행하면 내려받은 뒤 글이 보이며 열지 않음, 로컬 파일이면 꺼짐
//! 사용자 동작의 내려받기 표시(`Progress::downloading`, `JobEntry::in_sync_folder`, `CommandId::DownloadForPreview`)와
//! 내려받기가 필요 없는 동작(같은 볼륨 이동·이름바꾸기)을 시험용 동기화 공급자(`facet_win::cloud_files::testing`)로
//! 만든 진짜 온라인 전용 파일로 확인한다. 명령 목록은 모든 플랫폼에서, 나머지는 Windows에서만 돈다. 동기화 루트를
//! 등록할 수 없는 환경이면 건너뛰었다고 출력하고, 환경 변수 `FACET_REQUIRE_SYNC_ROOT`가 있으면 그 경우를 실패로 본다.
//! 휴지통에 넣은 항목은 끝날 때 휴지통에서 지운다.

use facet_core::command::CommandId;

#[test]
#[ignore = "#456"]
fn download_for_preview_is_in_palette_without_shortcut() {
    let command = CommandId::DownloadForPreview;
    assert!(CommandId::ALL.contains(&command));
    assert!(command.shortcuts().is_empty());
    assert_eq!(command.as_str(), "download-for-preview");
    assert_eq!(CommandId::from_id("download-for-preview"), Some(command));
}

#[cfg(windows)]
#[path = "common/bin.rs"]
mod bin;

#[cfg(windows)]
mod real {
    use std::fs;
    use std::os::windows::fs::MetadataExt;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use facet_core::Location;
    use facet_core::command::CommandId;
    use facet_core::job::{self, JobEvent, JobId, JobKind, JobQueue, JobState, Summary};
    use facet_core::preview::{PreviewBody, PreviewContent, PreviewProblem};
    use facet_core::tab::ReadOutcome;
    use facet_core::window::{App, Execution, Update};
    use facet_win::cloud_files::testing::TestSyncRoot;
    use tempfile::TempDir;

    const WAIT: Duration = Duration::from_secs(60);
    const ENV_REQUIRE: &str = "FACET_REQUIRE_SYNC_ROOT";
    const ONLINE_BITS: u32 = 0x1000 | 0x40_0000;

    fn sync_root(dir: &Path, test: &str) -> Option<TestSyncRoot> {
        match TestSyncRoot::register(dir) {
            Ok(root) => Some(root),
            Err(error) => {
                assert!(
                    std::env::var_os(ENV_REQUIRE).is_none(),
                    "{ENV_REQUIRE}가 있는데 동기화 루트를 등록하지 못했다: {error}"
                );
                println!("{test}: 동기화 루트를 등록할 수 없어 건너뜀: {error}");
                None
            }
        }
    }

    fn content() -> Vec<u8> {
        (0..512 * 1024).map(|i| (i % 253) as u8).collect()
    }

    /// 상위 폴더의 열거 결과로 읽은 속성 값(파일을 열지 않는다).
    fn attributes_of(path: &Path) -> u32 {
        let name = path.file_name().unwrap();
        fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap())
            .find(|e| e.file_name() == name)
            .expect("목록에 있다")
            .metadata()
            .unwrap()
            .file_attributes()
    }

    fn app_at(path: &Path) -> App {
        let mut app = App::new(Some(Location::file_system(path)));
        let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
        assert_eq!(outcome, ReadOutcome::Committed);
        app
    }

    fn select(app: &mut App, name: &str) {
        let index = app
            .active_tab()
            .items()
            .position(|item| item.name == name)
            .unwrap_or_else(|| panic!("{name}이 목록에 없다"));
        app.active_tab_mut().select_only(index);
    }

    /// 작업이 끝날 때까지 사건을 모아 (진행에 「내려받는 중」이 켜진 것이 있었는가, 결과)를 돌려준다.
    fn run_job(queue: &mut JobQueue, job: JobId) -> (bool, Summary) {
        let deadline = Instant::now() + WAIT;
        let mut downloading = false;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match queue.wait_event(left).expect("작업이 시간 안에 끝난다") {
                JobEvent::Progress { job: id, progress } if id == job => {
                    downloading |= progress.downloading;
                }
                JobEvent::Finished { job: id, summary } if id == job => {
                    return (downloading, summary);
                }
                JobEvent::ItemError { job: id, error } if id == job => {
                    panic!("묻지 않아야 한다: {error:?}");
                }
                _ => {}
            }
        }
    }

    #[test]
    #[ignore = "#456"]
    fn copying_online_only_file_shows_downloading() {
        let dir = TempDir::new().unwrap();
        let target = TempDir::new().unwrap();
        let Some(root) = sync_root(dir.path(), "복사의 내려받는 중") else {
            return;
        };
        let online = root.add_online_file("온라인.bin", &content());
        let local = dir.path().join("로컬.bin");
        fs::write(&local, content()).unwrap();
        let mut queue = JobQueue::new();

        let job = queue
            .copy(vec![online], target.path().to_path_buf())
            .expect("복사 작업");
        let (downloading, summary) = run_job(&mut queue, job);
        assert!(downloading, "온라인 전용 원본이면 「내려받는 중」이 켜진다");
        assert!(summary.failures.is_empty(), "{summary:?}");
        assert_eq!(
            fs::read(target.path().join("온라인.bin")).unwrap(),
            content()
        );
        assert!(root.fetch_data_count() >= 1);

        let job = queue
            .copy(vec![local], target.path().to_path_buf())
            .expect("복사 작업");
        let (downloading, _) = run_job(&mut queue, job);
        assert!(!downloading, "이 장치에 있는 파일이면 켜지지 않는다");
    }

    #[test]
    #[ignore = "#456"]
    fn move_and_rename_inside_sync_folder_do_not_fetch() {
        let dir = TempDir::new().unwrap();
        let Some(root) = sync_root(dir.path(), "같은 폴더 이동") else {
            return;
        };
        let moving = root.add_online_file("옮길.bin", &content());
        root.add_online_file("바꿀.bin", &content());
        let sub = dir.path().join("하위");
        fs::create_dir(&sub).unwrap();

        let mut queue = JobQueue::new();
        let job = queue
            .move_items(vec![moving], sub.clone())
            .expect("이동 작업");
        let (_, summary) = run_job(&mut queue, job);
        assert!(summary.failures.is_empty(), "{summary:?}");
        let moved = sub.join("옮길.bin");
        assert_ne!(attributes_of(&moved) & ONLINE_BITS, 0, "여전히 온라인 전용");

        let mut app = app_at(dir.path());
        app.rename("바꿀.bin".as_ref(), "바꾼.bin").unwrap();
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match app.wait_update(left).expect("이름바꾸기 결과") {
                (_, Update::Edited(result)) => {
                    result.expect("이름을 바꾼다");
                    break;
                }
                _ => continue,
            }
        }
        let renamed: PathBuf = dir.path().join("바꾼.bin");
        assert_ne!(
            attributes_of(&renamed) & ONLINE_BITS,
            0,
            "여전히 온라인 전용"
        );
        assert_eq!(root.fetch_data_count(), 0, "내용을 읽지 않는다");
    }

    /// Delete를 실행해 새 휴지통 삭제 작업의 `in_sync_folder`를 돌려준다. 끝날 때까지 기다려 휴지통 항목을 치운다.
    fn delete_and_notice(app: &mut App, name: &str) -> bool {
        select(app, name);
        let before: Vec<JobId> = app.work_area().entries().iter().map(|e| e.id).collect();
        assert_eq!(
            app.execute(CommandId::Delete),
            Execution::Done,
            "묻지 않는다"
        );
        let deadline = Instant::now() + WAIT;
        loop {
            app.poll_jobs();
            app.poll();
            if let Some(entry) = app
                .work_area()
                .entries()
                .iter()
                .find(|e| !before.contains(&e.id))
            {
                assert_eq!(entry.kind, JobKind::Recycle);
                let notice = entry.in_sync_folder;
                if let Some(summary) = &entry.summary {
                    crate::bin::purge(&summary.recycled);
                    assert_eq!(entry.state, JobState::Finished);
                    return notice;
                }
            }
            assert!(Instant::now() < deadline, "휴지통 삭제가 끝나지 않았다");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    #[ignore = "#456"]
    fn recycle_in_sync_folder_does_not_ask_and_carries_notice() {
        let dir = TempDir::new().unwrap();
        let plain = TempDir::new().unwrap();
        if !job::has_recycle_bin(plain.path()) {
            println!("휴지통 삭제 알림: 휴지통이 없는 볼륨이라 건너뜀");
            return;
        }
        let Some(_root) = sync_root(dir.path(), "휴지통 삭제 알림") else {
            return;
        };
        fs::write(dir.path().join("지울.txt"), b"x").unwrap();
        fs::write(plain.path().join("지울.txt"), b"x").unwrap();

        let mut app = app_at(dir.path());
        assert!(delete_and_notice(&mut app, "지울.txt"));
        let mut app = app_at(plain.path());
        assert!(!delete_and_notice(&mut app, "지울.txt"));
    }

    #[test]
    #[ignore = "#456"]
    fn download_for_preview_shows_text_without_opening() {
        let dir = TempDir::new().unwrap();
        let Some(root) = sync_root(dir.path(), "미리 보기 내려받기") else {
            return;
        };
        root.add_online_file("온라인.txt", "내려받은 미리보기 글".as_bytes());
        fs::write(dir.path().join("로컬.txt"), "로컬 글").unwrap();
        let mut app = app_at(dir.path());
        if !app.active_window().preview_pane().visible {
            assert_eq!(app.execute(CommandId::TogglePreview), Execution::Done);
        }
        let window = app.active_window_id();

        select(&mut app, "로컬.txt");
        app.sync_preview();
        assert!(!app.can_execute(CommandId::DownloadForPreview));

        select(&mut app, "온라인.txt");
        app.sync_preview();
        let deadline = Instant::now() + WAIT;
        loop {
            app.poll();
            if let Some(PreviewContent::Item { path, body, .. }) = app.preview(window)
                && path.file_name().is_some_and(|n| n == "온라인.txt")
                && *body == PreviewBody::Unavailable(PreviewProblem::OnlineOnly)
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "온라인 전용 거절이 보이지 않았다"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(app.can_execute(CommandId::DownloadForPreview));
        assert_eq!(app.execute(CommandId::DownloadForPreview), Execution::Done);

        let mut opened = false;
        let mut downloaded = false;
        let deadline = Instant::now() + WAIT;
        loop {
            app.poll_jobs();
            for (_, update) in app.poll() {
                opened |= matches!(update, Update::Opened(_));
            }
            downloaded |= app
                .work_area()
                .entries()
                .iter()
                .any(|e| e.kind == JobKind::Download && e.state == JobState::Finished);
            if let Some(PreviewContent::Item {
                body: PreviewBody::Text(text),
                ..
            }) = app.preview(window)
            {
                assert_eq!(text.text, "내려받은 미리보기 글");
                break;
            }
            assert!(
                Instant::now() < deadline,
                "내려받은 뒤 미리보기 글이 보이지 않았다"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(downloaded, "내려받기 작업이 끝났다");
        assert!(root.fetch_data_count() >= 1);
        assert!(!opened, "열지는 않는다");
    }
}
