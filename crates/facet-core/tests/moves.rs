//! 이동 작업을 임시 폴더의 실제 파일로 확인한다.
//!
//! 이름 바꾸기·파일 복사는 facet-win에 맡기므로 실제 이동을 보는 테스트는 Windows 전용이다. Windows가 아니면 시작 전
//! 거부만 확인한다. 볼륨을 넘는 이동은 두 번째 볼륨의 쓸 수 있는 폴더를 환경 변수 `FACET_TEST_OTHER_VOLUME_DIR`로
//! 받는다(`cargo xtask ci`는 임시 폴더와 작업 공간이 다른 드라이브면 `target/other-volume`을 넘긴다). 없으면
//! 건너뛰었다고 출력한다.

use facet_core::job::{CopyRejection, JobQueue};
use tempfile::TempDir;

#[test]
fn 폴더를_자기_자신이나_하위로_이동하면_거부한다() {
    let dir = TempDir::new().unwrap();
    let folder = dir.path().join("폴더");
    let inner = folder.join("안");
    std::fs::create_dir_all(&inner).unwrap();
    let mut queue = JobQueue::new();
    assert_eq!(
        queue.move_items(vec![folder.clone()], folder.clone()),
        Err(CopyRejection::IntoItself(folder.clone()))
    );
    assert_eq!(
        queue.move_items(vec![folder.clone()], inner.clone()),
        Err(CopyRejection::TargetInsideSource(folder.clone()))
    );
    assert_eq!(
        queue.move_items(vec![folder.clone()], dir.path().join("없는 폴더")),
        Err(CopyRejection::TargetMissing)
    );
    assert_eq!(
        queue.move_items(Vec::new(), dir.path().to_path_buf()),
        Err(CopyRejection::NoSources)
    );
    assert!(queue.poll().is_empty(), "거부된 작업은 사건이 없다");
    assert!(!queue.has_unfinished());
    assert!(folder.is_dir() && inner.is_dir(), "아무것도 옮기지 않았다");
}

#[cfg(windows)]
mod windows {
    use std::env;
    use std::fs::{self, File, FileTimes, OpenOptions};
    use std::os::windows::fs::{FileTimesExt, MetadataExt, OpenOptionsExt};
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{Duration, Instant, SystemTime};

    use facet_core::job::{
        ConflictAnswer, ErrorAnswer, FileOpErrorKind, JobEvent, JobId, JobQueue, JobState,
        Resolution, Summary,
    };
    use facet_win::file_ops;
    use tempfile::TempDir;

    const TIMEOUT: Duration = Duration::from_secs(120);
    const OTHER_VOLUME_ENV: &str = "FACET_TEST_OTHER_VOLUME_DIR";
    const READONLY: u32 = 0x1;
    const HIDDEN: u32 = 0x2;

    /// `wanted`인 사건이 올 때까지 사건을 `events`에 모은다.
    fn wait_for(
        queue: &mut JobQueue,
        events: &mut Vec<JobEvent>,
        wanted: impl Fn(&JobEvent) -> bool,
    ) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let event = queue
                .wait_event(left)
                .unwrap_or_else(|| panic!("기다린 사건이 오지 않았다: {events:?}"));
            let found = wanted(&event);
            events.push(event);
            if found {
                return;
            }
        }
    }

    /// `job`이 끝날 때까지 기다려 결과를 돌려준다. 그 사이 질문이 오면 실패한다.
    fn wait_finished(queue: &mut JobQueue, events: &mut Vec<JobEvent>, job: JobId) -> Summary {
        let start = events.len();
        wait_for(
            queue,
            events,
            |e| matches!(e, JobEvent::Finished { job: id, .. } if *id == job),
        );
        assert!(
            !events[start..].iter().any(|e| matches!(
                e,
                JobEvent::ItemError { job: id, .. } | JobEvent::Conflict { job: id, .. } if *id == job
            )),
            "묻지 않아야 한다: {events:?}"
        );
        match events.last().unwrap() {
            JobEvent::Finished { summary, .. } => summary.clone(),
            _ => unreachable!(),
        }
    }

    /// `job`의 다음 질문(이름 충돌이나 항목별 오류)을 기다린다.
    fn wait_question(queue: &mut JobQueue, events: &mut Vec<JobEvent>, job: JobId) -> JobEvent {
        wait_for(queue, events, |e| match e {
            JobEvent::ItemError { job: id, .. }
            | JobEvent::Conflict { job: id, .. }
            | JobEvent::Finished { job: id, .. } => *id == job,
            _ => false,
        });
        let event = events.last().unwrap().clone();
        assert!(
            !matches!(event, JobEvent::Finished { .. }),
            "질문 전에 작업이 끝났다: {events:?}"
        );
        event
    }

    fn move_and_wait(queue: &mut JobQueue, sources: Vec<PathBuf>, target: &Path) -> Summary {
        let id = queue.move_items(sources, target.to_path_buf()).unwrap();
        let mut events = Vec::new();
        let summary = wait_finished(queue, &mut events, id);
        assert!(
            events.contains(&JobEvent::StateChanged {
                job: id,
                state: JobState::Finished
            }),
            "{events:?}"
        );
        summary
    }

    fn names(folder: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(folder)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn attrib(path: &Path, flags: &[&str]) {
        let status = Command::new("attrib")
            .args(flags)
            .arg(path)
            .status()
            .unwrap();
        assert!(status.success(), "attrib {flags:?} {}", path.display());
    }

    fn set_old_times(path: &Path, folder: bool, offset: u64) {
        let created = SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000 + offset);
        let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000 + offset);
        let mut options = OpenOptions::new();
        options.access_mode(0x100); // FILE_WRITE_ATTRIBUTES
        if folder {
            options.custom_flags(0x0200_0000); // FILE_FLAG_BACKUP_SEMANTICS
        }
        let file = options.open(path).unwrap();
        file.set_times(FileTimes::new().set_created(created).set_modified(modified))
            .unwrap();
    }

    fn times(path: &Path) -> (SystemTime, SystemTime) {
        let metadata = fs::metadata(path).unwrap();
        (metadata.created().unwrap(), metadata.modified().unwrap())
    }

    fn attributes(path: &Path) -> u32 {
        fs::metadata(path).unwrap().file_attributes()
    }

    /// 다른 프로그램이 공유 없이 열어 둔 것처럼 잠근다.
    fn lock(path: &Path) -> File {
        OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(path)
            .unwrap()
    }

    /// 두 번째 볼륨의 빈 시험 폴더. 환경 변수가 없으면 건너뛴다고 출력하고 `None`.
    fn other_volume(test: &str) -> Option<TempDir> {
        let Some(other) = env::var_os(OTHER_VOLUME_ENV) else {
            println!(
                "건너뜀: {OTHER_VOLUME_ENV}가 없어 볼륨을 넘는 이동({test})을 확인하지 않았다"
            );
            return None;
        };
        let dir = TempDir::new_in(PathBuf::from(other)).unwrap();
        let here = file_ops::file_id(&env::temp_dir()).unwrap();
        let there = file_ops::file_id(dir.path()).unwrap();
        assert_ne!(
            here.volume_serial, there.volume_serial,
            "{OTHER_VOLUME_ENV}가 임시 폴더와 같은 볼륨이다"
        );
        Some(dir)
    }

    #[test]
    fn 같은_볼륨_이동은_이름_바꾸기라_파일_id가_그대로다() {
        let dir = TempDir::new().unwrap();
        let source_dir = dir.path().join("원본");
        let target = dir.path().join("대상");
        let folder = source_dir.join("폴더 📁");
        fs::create_dir_all(folder.join("안")).unwrap();
        fs::create_dir_all(&target).unwrap();
        let file = source_dir.join("파일.txt");
        fs::write(&file, "내용").unwrap();
        fs::write(folder.join("안").join("깊은.txt"), "깊다").unwrap();
        let file_id = file_ops::file_id(&file).unwrap();
        let folder_id = file_ops::file_id(&folder).unwrap();
        let deep_id = file_ops::file_id(&folder.join("안").join("깊은.txt")).unwrap();

        let mut queue = JobQueue::new();
        let id = queue
            .move_items(vec![file.clone(), folder.clone()], target.clone())
            .unwrap();
        let mut events = Vec::new();
        let summary = wait_finished(&mut queue, &mut events, id);

        assert!(!file.exists() && !folder.exists(), "원본이 사라졌다");
        assert_eq!(names(&source_dir), Vec::<String>::new());
        assert_eq!(
            file_ops::file_id(&target.join("파일.txt")).unwrap(),
            file_id
        );
        assert_eq!(
            file_ops::file_id(&target.join("폴더 📁")).unwrap(),
            folder_id
        );
        assert_eq!(
            file_ops::file_id(&target.join("폴더 📁").join("안").join("깊은.txt")).unwrap(),
            deep_id
        );
        assert_eq!(summary.items_renamed, 2, "{summary:?}");
        assert_eq!(summary.files_copied, 0, "복사하지 않았다: {summary:?}");
        assert_eq!(summary.items_done, 2, "{summary:?}");
        assert_eq!(summary.items_remaining, 0);
        assert!(summary.all_counted);
        // 마지막 진행: 항목 수로 다 했고, 바이트는 이름 바꾸기라 세지 않는다.
        let last = events
            .iter()
            .rev()
            .find_map(|e| match e {
                JobEvent::Progress { progress, .. } => Some(progress.clone()),
                _ => None,
            })
            .expect("진행 사건");
        assert_eq!((last.items_done, last.items_total), (2, 2), "{last:?}");
        assert_eq!((last.bytes_done, last.bytes_total), (0, 0), "{last:?}");
        assert!(last.counted);
    }

    #[test]
    fn 같은_폴더로_이동하면_아무것도_바꾸지_않는다() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("파일.txt");
        let folder = dir.path().join("폴더");
        fs::write(&file, "내용").unwrap();
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("안.txt"), "안").unwrap();
        let before = names(dir.path());
        let file_id = file_ops::file_id(&file).unwrap();

        let mut queue = JobQueue::new();
        let summary = move_and_wait(&mut queue, vec![file.clone(), folder.clone()], dir.path());
        assert_eq!(names(dir.path()), before);
        assert_eq!(names(&folder), ["안.txt"]);
        assert_eq!(file_ops::file_id(&file).unwrap(), file_id);
        assert_eq!(summary.items_renamed, 0, "{summary:?}");
        assert_eq!(summary.items_done, 2, "{summary:?}");
        assert_eq!(summary.items_remaining, 0);
    }

    #[test]
    fn 같은_이름_폴더로_이동하면_합쳐지고_빈_원본_폴더는_지워진다() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("원본").join("폴더");
        let target = dir.path().join("대상");
        fs::create_dir_all(source.join("안")).unwrap();
        fs::create_dir_all(target.join("폴더").join("안")).unwrap();
        fs::write(source.join("가.txt"), "가").unwrap();
        fs::write(source.join("안").join("나.txt"), "나").unwrap();
        fs::create_dir(source.join("새 폴더")).unwrap();
        fs::write(target.join("폴더").join("다.txt"), "다").unwrap();
        fs::write(target.join("폴더").join("안").join("라.txt"), "라").unwrap();
        let moved_id = file_ops::file_id(&source.join("가.txt")).unwrap();

        let mut queue = JobQueue::new();
        let summary = move_and_wait(&mut queue, vec![source.clone()], &target);

        assert!(!source.exists(), "빈 원본 폴더는 지웠다");
        assert_eq!(names(&dir.path().join("원본")), Vec::<String>::new());
        let merged = target.join("폴더");
        assert_eq!(names(&merged), ["가.txt", "다.txt", "새 폴더", "안"]);
        assert_eq!(names(&merged.join("안")), ["나.txt", "라.txt"]);
        assert_eq!(
            file_ops::file_id(&merged.join("가.txt")).unwrap(),
            moved_id,
            "합친 폴더 안 항목도 이름 바꾸기로 옮긴다"
        );
        assert_eq!(summary.folders_merged, 2, "{summary:?}");
        assert_eq!(summary.items_renamed, 3, "{summary:?}");
        // 원본 폴더, 가, 안(합침), 나, 새 폴더.
        assert_eq!(summary.items_done, 5, "{summary:?}");
        assert_eq!(summary.items_remaining, 0);
    }

    #[test]
    fn 같은_볼륨이어도_같은_이름_파일은_묻고_답대로_한다() {
        let dir = TempDir::new().unwrap();
        let source_dir = dir.path().join("원본");
        let target = dir.path().join("대상");
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&target).unwrap();
        let cases = [
            ("건너뛰기.txt", Resolution::Skip),
            ("둘 다.txt", Resolution::KeepBoth),
            ("덮어쓰기.txt", Resolution::Overwrite),
        ];
        let mut sources = Vec::new();
        for (name, _) in cases {
            fs::write(source_dir.join(name), format!("새 {name}")).unwrap();
            fs::write(target.join(name), format!("옛 {name}")).unwrap();
            sources.push(source_dir.join(name));
        }
        // 읽기 전용 대상도 덮어쓴다.
        attrib(&target.join("덮어쓰기.txt"), &["+R"]);

        let mut queue = JobQueue::new();
        let id = queue.move_items(sources, target.clone()).unwrap();
        let mut events = Vec::new();
        for (name, resolution) in cases {
            match wait_question(&mut queue, &mut events, id) {
                JobEvent::Conflict { conflict, .. } => {
                    assert_eq!(conflict.source.path, source_dir.join(name));
                    assert_eq!(conflict.target.path, target.join(name));
                    assert!(conflict.can_overwrite);
                }
                other => panic!("이름 충돌 질문이 아니다: {other:?}"),
            }
            queue
                .answer_conflict(id, ConflictAnswer::once(resolution))
                .unwrap();
        }
        let summary = wait_finished(&mut queue, &mut events, id);

        assert_eq!(names(&source_dir), ["건너뛰기.txt"], "건너뛴 원본만 남는다");
        assert_eq!(
            names(&target),
            ["건너뛰기.txt", "덮어쓰기.txt", "둘 다 (2).txt", "둘 다.txt"]
        );
        let read = |name: &str| fs::read_to_string(target.join(name)).unwrap();
        assert_eq!(read("건너뛰기.txt"), "옛 건너뛰기.txt");
        assert_eq!(read("둘 다.txt"), "옛 둘 다.txt");
        assert_eq!(read("둘 다 (2).txt"), "새 둘 다.txt");
        assert_eq!(read("덮어쓰기.txt"), "새 덮어쓰기.txt");
        assert_eq!(summary.conflicts_skipped, 1, "{summary:?}");
        assert_eq!(summary.files_overwritten, 1, "{summary:?}");
        assert_eq!(summary.items_renamed, 2, "{summary:?}");
        assert_eq!(summary.items_done, 3);
    }

    #[test]
    fn 같은_볼륨에서_잠긴_파일을_건너뛰면_그_원본만_남는다() {
        let dir = TempDir::new().unwrap();
        let source_dir = dir.path().join("원본");
        let target = dir.path().join("대상");
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&target).unwrap();
        let files = ["1 앞.txt", "2 잠김.txt", "3 뒤.txt"].map(|name| {
            let path = source_dir.join(name);
            fs::write(&path, name).unwrap();
            path
        });
        let locked = lock(&files[1]);

        let mut queue = JobQueue::new();
        let id = queue.move_items(files.to_vec(), target.clone()).unwrap();
        let mut events = Vec::new();
        match wait_question(&mut queue, &mut events, id) {
            JobEvent::ItemError { error, .. } => {
                assert_eq!(error.source, files[1]);
                assert_eq!(error.kind, FileOpErrorKind::Locked);
            }
            other => panic!("항목별 오류 질문이 아니다: {other:?}"),
        }
        queue.answer_error(id, ErrorAnswer::Skip).unwrap();
        let summary = wait_finished(&mut queue, &mut events, id);
        drop(locked);

        assert!(events.contains(&JobEvent::StateChanged {
            job: id,
            state: JobState::Failed
        }));
        assert_eq!(names(&source_dir), ["2 잠김.txt"]);
        assert_eq!(names(&target), ["1 앞.txt", "3 뒤.txt"]);
        assert_eq!(summary.errors_skipped(), 1, "{summary:?}");
        assert_eq!(summary.items_renamed, 2, "{summary:?}");
    }

    #[test]
    fn 볼륨을_넘는_이동은_복사_뒤_원본을_지운다() {
        let Some(other) = other_volume("내용·시각·속성") else {
            return;
        };
        let dir = TempDir::new().unwrap();
        let source_dir = dir.path().join("원본");
        let folder = source_dir.join("폴더");
        fs::create_dir_all(folder.join("안")).unwrap();
        let file = source_dir.join("파일.txt");
        fs::write(&file, "파일 내용").unwrap();
        fs::write(folder.join("안").join("깊은.bin"), vec![7u8; 300_000]).unwrap();
        fs::write(folder.join("숨김.txt"), "숨김").unwrap();
        set_old_times(&file, false, 0);
        set_old_times(&folder.join("안").join("깊은.bin"), false, 1);
        set_old_times(&folder.join("숨김.txt"), false, 2);
        attrib(&folder.join("숨김.txt"), &["+H"]);
        attrib(&file, &["+R"]);
        set_old_times(&folder.join("안"), true, 3);
        set_old_times(&folder, true, 4);
        let expected = [
            (file.clone(), PathBuf::from("파일.txt")),
            (
                folder.join("안").join("깊은.bin"),
                PathBuf::from(r"폴더\안\깊은.bin"),
            ),
            (folder.join("숨김.txt"), PathBuf::from(r"폴더\숨김.txt")),
            (folder.join("안"), PathBuf::from(r"폴더\안")),
            (folder.clone(), PathBuf::from("폴더")),
        ]
        .map(|(path, relative)| {
            let contents = fs::metadata(&path)
                .unwrap()
                .is_file()
                .then(|| fs::read(&path).unwrap());
            (relative, contents, times(&path), attributes(&path))
        });

        let target = other.path().to_path_buf();
        let mut queue = JobQueue::new();
        let id = queue
            .move_items(vec![file.clone(), folder.clone()], target.clone())
            .unwrap();
        let mut events = Vec::new();
        let summary = wait_finished(&mut queue, &mut events, id);

        assert!(!file.exists(), "원본 파일이 사라졌다");
        assert!(!folder.exists(), "원본 폴더가 사라졌다");
        assert_eq!(names(&source_dir), Vec::<String>::new());
        for (relative, contents, times_before, attributes_before) in expected {
            let moved = target.join(&relative);
            if let Some(contents) = contents {
                assert_eq!(
                    fs::read(&moved).unwrap(),
                    contents,
                    "{}",
                    relative.display()
                );
            }
            assert_eq!(times(&moved), times_before, "{}", relative.display());
            assert_eq!(
                attributes(&moved) & (READONLY | HIDDEN),
                attributes_before & (READONLY | HIDDEN),
                "{}",
                relative.display()
            );
        }
        assert_eq!(summary.items_renamed, 0, "{summary:?}");
        assert_eq!(summary.files_copied, 3, "{summary:?}");
        assert_eq!(summary.folders_copied, 2, "{summary:?}");
        assert_eq!(summary.items_done, 5, "{summary:?}");
        assert_eq!(summary.items_remaining, 0, "{summary:?}");
        let last = events
            .iter()
            .rev()
            .find_map(|e| match e {
                JobEvent::Progress { progress, .. } => Some(progress.clone()),
                _ => None,
            })
            .expect("진행 사건");
        assert_eq!((last.items_done, last.items_total), (5, 5), "{last:?}");
        let bytes = 300_000 + "파일 내용".len() as u64 + "숨김".len() as u64;
        assert_eq!(
            (last.bytes_done, last.bytes_total),
            (bytes, bytes),
            "{last:?}"
        );
        // 읽기 전용을 풀어 임시 폴더를 지울 수 있게 한다.
        attrib(&target.join("파일.txt"), &["-R"]);
    }

    #[test]
    fn 볼륨을_넘는_이동에서_건너뛴_항목의_원본과_상위_폴더는_남는다() {
        let Some(other) = other_volume("건너뛰기") else {
            return;
        };
        let dir = TempDir::new().unwrap();
        let folder = dir.path().join("폴더");
        fs::create_dir_all(&folder).unwrap();
        let files = ["1 앞.txt", "2 잠김.txt", "3 뒤.txt"].map(|name| {
            let path = folder.join(name);
            fs::write(&path, name).unwrap();
            path
        });
        let locked = lock(&files[1]);

        let target = other.path().to_path_buf();
        let mut queue = JobQueue::new();
        let id = queue
            .move_items(vec![folder.clone()], target.clone())
            .unwrap();
        let mut events = Vec::new();
        match wait_question(&mut queue, &mut events, id) {
            JobEvent::ItemError { error, .. } => {
                assert_eq!(error.source, files[1]);
                assert_eq!(error.kind, FileOpErrorKind::Locked);
            }
            other => panic!("항목별 오류 질문이 아니다: {other:?}"),
        }
        queue.answer_error(id, ErrorAnswer::Skip).unwrap();
        let summary = wait_finished(&mut queue, &mut events, id);
        drop(locked);

        assert_eq!(
            names(&folder),
            ["2 잠김.txt"],
            "복사하지 못한 원본과 그 폴더만 남는다"
        );
        assert_eq!(names(&target.join("폴더")), ["1 앞.txt", "3 뒤.txt"]);
        assert_eq!(summary.errors_skipped(), 1, "{summary:?}");
        assert_eq!(summary.files_copied, 2, "{summary:?}");
    }

    #[test]
    fn 볼륨을_넘는_이동을_취소하면_끝난_항목의_원본만_지워진다() {
        let Some(other) = other_volume("취소") else {
            return;
        };
        let dir = TempDir::new().unwrap();
        let files = ["1 앞.txt", "2 잠김.txt", "3 뒤.txt"].map(|name| {
            let path = dir.path().join(name);
            fs::write(&path, name).unwrap();
            path
        });
        let locked = lock(&files[1]);

        let target = other.path().to_path_buf();
        let mut queue = JobQueue::new();
        let id = queue.move_items(files.to_vec(), target.clone()).unwrap();
        let mut events = Vec::new();
        assert!(matches!(
            wait_question(&mut queue, &mut events, id),
            JobEvent::ItemError { .. }
        ));
        queue.answer_error(id, ErrorAnswer::CancelJob).unwrap();
        let summary = wait_finished(&mut queue, &mut events, id);
        drop(locked);

        assert!(events.contains(&JobEvent::StateChanged {
            job: id,
            state: JobState::Cancelled
        }));
        assert_eq!(names(dir.path()), ["2 잠김.txt", "3 뒤.txt"]);
        assert_eq!(names(&target), ["1 앞.txt"]);
        assert_eq!(
            fs::read_to_string(target.join("1 앞.txt")).unwrap(),
            "1 앞.txt"
        );
        assert_eq!(summary.items_done, 1, "{summary:?}");
        assert_eq!(summary.items_remaining, 2, "{summary:?}");
    }
}
