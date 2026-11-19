//! 작업 대기열과 복사 작업을 임시 폴더의 실제 파일로 확인한다.
//!
//! 파일 하나의 복사는 facet-win(`CopyFile2`)에 맡기므로 실제 복사를 보는 테스트는 Windows 전용이다. Windows가
//! 아니면 시작 전 거부만 확인한다.

use std::path::{Path, PathBuf};
use std::time::Duration;

use facet_core::job::{CopyRejection, JobEvent, JobId, JobQueue, JobState, Remaining, Summary};
use tempfile::TempDir;

/// 작업 하나가 끝날 때까지 기다리는 넉넉한 한도.
const JOB_TIMEOUT: Duration = Duration::from_secs(120);

/// `jobs`가 모두 끝날 때까지 사건을 모은다.
fn collect_until_finished(queue: &mut JobQueue, jobs: &[JobId]) -> Vec<JobEvent> {
    let mut events = Vec::new();
    let mut remaining: Vec<JobId> = jobs.to_vec();
    let deadline = std::time::Instant::now() + JOB_TIMEOUT;
    while !remaining.is_empty() {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let event = queue
            .wait_event(left)
            .unwrap_or_else(|| panic!("작업이 끝나지 않았다: {events:?}"));
        if let JobEvent::Finished { job, .. } = &event {
            remaining.retain(|id| id != job);
        }
        events.push(event);
    }
    events
}

fn summary_of(events: &[JobEvent], id: JobId) -> Summary {
    events
        .iter()
        .find_map(|event| match event {
            JobEvent::Finished { job, summary } if *job == id => Some(summary.clone()),
            _ => None,
        })
        .expect("끝남 사건")
}

fn copy_and_wait(queue: &mut JobQueue, sources: Vec<PathBuf>, target: &Path) -> Vec<JobEvent> {
    let id = queue.copy(sources, target.to_path_buf()).unwrap();
    collect_until_finished(queue, &[id])
}

#[test]
fn 없는_대상_위치는_작업을_만들지_않는다() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("원본.txt");
    std::fs::write(&source, "x").unwrap();
    let mut queue = JobQueue::new();
    let missing = dir.path().join("없는 폴더");
    assert_eq!(
        queue.copy(vec![source.clone()], missing),
        Err(CopyRejection::TargetMissing)
    );
    assert_eq!(
        queue.copy(vec![source.clone()], source.clone()),
        Err(CopyRejection::TargetNotFolder)
    );
    assert_eq!(
        queue.copy(Vec::new(), dir.path().to_path_buf()),
        Err(CopyRejection::NoSources)
    );
    assert!(queue.poll().is_empty(), "거부된 작업은 사건이 없다");
    assert!(!queue.has_unfinished());
}

#[test]
fn 폴더를_자기_자신이나_하위로_복사하면_거부한다() {
    let dir = TempDir::new().unwrap();
    let folder = dir.path().join("폴더");
    let inner = folder.join("안").join("더 안");
    std::fs::create_dir_all(&inner).unwrap();
    let mut queue = JobQueue::new();
    assert_eq!(
        queue.copy(vec![folder.clone()], folder.clone()),
        Err(CopyRejection::IntoItself(folder.clone()))
    );
    assert_eq!(
        queue.copy(vec![folder.clone()], inner.clone()),
        Err(CopyRejection::TargetInsideSource(folder.clone()))
    );
    // 대소문자만 다른 경로도 같은 폴더다(Windows).
    #[cfg(windows)]
    {
        let upper = PathBuf::from(inner.to_string_lossy().to_uppercase());
        assert_eq!(
            queue.copy(vec![folder.clone()], upper),
            Err(CopyRejection::TargetInsideSource(folder.clone()))
        );
    }
    assert!(queue.poll().is_empty());
}

#[cfg(windows)]
mod windows {
    use super::*;

    use std::fs::{self, FileTimes, OpenOptions};
    use std::os::windows::fs::{FileTimesExt, MetadataExt, OpenOptionsExt};
    use std::process::Command;
    use std::time::SystemTime;

    const READONLY: u32 = 0x1;
    const HIDDEN: u32 = 0x2;

    fn attrib(path: &Path, flags: &[&str]) {
        let status = Command::new("attrib")
            .args(flags)
            .arg(path)
            .status()
            .unwrap();
        assert!(status.success(), "attrib {flags:?} {}", path.display());
    }

    fn attributes(path: &Path) -> u32 {
        fs::metadata(path).unwrap().file_attributes()
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

    fn ads(path: &Path, stream: &str) -> PathBuf {
        let mut text = path.as_os_str().to_owned();
        text.push(":");
        text.push(stream);
        PathBuf::from(text)
    }

    fn pattern(len: usize, seed: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 + seed) as u8).collect()
    }

    /// 원본과 대상 트리가 이름·내용·시각·속성까지 같은지 본다.
    fn assert_same_tree(source: &Path, target: &Path) {
        let source_meta = fs::symlink_metadata(source).unwrap();
        let target_meta = fs::symlink_metadata(target).unwrap();
        assert_eq!(source_meta.is_dir(), target_meta.is_dir(), "{target:?}");
        assert_eq!(times(source), times(target), "시각 {target:?}");
        assert_eq!(attributes(source), attributes(target), "속성 {target:?}");
        if source_meta.is_dir() {
            let mut names: Vec<_> = fs::read_dir(source)
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            let mut target_names: Vec<_> = fs::read_dir(target)
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            names.sort();
            target_names.sort();
            assert_eq!(names, target_names, "{target:?}");
            for name in names {
                assert_same_tree(&source.join(&name), &target.join(&name));
            }
        } else {
            assert_eq!(fs::read(source).unwrap(), fs::read(target).unwrap());
        }
    }

    /// 읽기 전용을 풀어 임시 폴더를 지울 수 있게 한다.
    fn clear_readonly(path: &Path) {
        let status = Command::new("attrib")
            .args(["-r", "-h", "/s", "/d"])
            .arg(path.join("*"))
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn 파일_폴더_중첩_폴더를_복사하면_내용_시각_속성_대체_데이터_스트림이_같다() {
        let dir = TempDir::new().unwrap();
        let source_dir = dir.path().join("원본");
        let target_dir = dir.path().join("대상");
        fs::create_dir_all(&target_dir).unwrap();

        let file = source_dir.join("파일 하나.txt");
        let folder = source_dir.join("폴더");
        let nested = folder.join("중첩").join("더 깊이");
        fs::create_dir_all(&nested).unwrap();
        fs::write(&file, pattern(2 * 1024 * 1024 + 5, 1)).unwrap();
        fs::write(
            ads(&file, "Zone.Identifier"),
            "[ZoneTransfer]\r\nZoneId=3\r\n",
        )
        .unwrap();
        fs::write(folder.join("a.bin"), pattern(1000, 2)).unwrap();
        fs::write(folder.join("중첩").join("빈 파일"), "").unwrap();
        let deep = nested.join("깊은 파일 😀.txt");
        fs::write(&deep, pattern(70_000, 3)).unwrap();
        fs::write(ads(&deep, "메모"), "다른 스트림").unwrap();
        set_old_times(&file, false, 1);
        set_old_times(&deep, false, 2);
        set_old_times(&nested, true, 3);
        set_old_times(&folder.join("중첩"), true, 4);
        set_old_times(&folder, true, 5);
        attrib(&file, &["+r", "+h"]);
        attrib(&nested, &["+h"]);

        let mut queue = JobQueue::new();
        let events = copy_and_wait(&mut queue, vec![file.clone(), folder.clone()], &target_dir);
        let id = events
            .iter()
            .find_map(|e| match e {
                JobEvent::Finished { job, .. } => Some(*job),
                _ => None,
            })
            .unwrap();
        let summary = summary_of(&events, id);
        assert_eq!(summary.files_copied, 4, "{summary:?}");
        assert_eq!(summary.folders_copied, 3, "{summary:?}");
        assert!(summary.failures.is_empty(), "{summary:?}");
        assert_eq!(summary.conflicts_skipped, 0);
        assert!(
            events.contains(&JobEvent::StateChanged {
                job: id,
                state: JobState::Finished
            }),
            "{events:?}"
        );

        assert_same_tree(&file, &target_dir.join("파일 하나.txt"));
        assert_same_tree(&folder, &target_dir.join("폴더"));
        assert_eq!(
            attributes(&target_dir.join("파일 하나.txt")) & (READONLY | HIDDEN),
            READONLY | HIDDEN
        );
        assert_eq!(
            fs::read(ads(&target_dir.join("파일 하나.txt"), "Zone.Identifier")).unwrap(),
            b"[ZoneTransfer]\r\nZoneId=3\r\n"
        );
        assert_eq!(
            fs::read_to_string(ads(
                &target_dir
                    .join("폴더")
                    .join("중첩")
                    .join("더 깊이")
                    .join("깊은 파일 😀.txt"),
                "메모"
            ))
            .unwrap(),
            "다른 스트림"
        );
        assert!(!queue.has_unfinished());
        clear_readonly(dir.path());
    }

    #[test]
    fn 작업_두_개는_앞_작업이_끝난_뒤_뒤_작업이_시작한다() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("원본");
        fs::create_dir_all(&source).unwrap();
        for i in 0..20 {
            fs::write(source.join(format!("{i}.bin")), pattern(200_000, i)).unwrap();
        }
        let first_target = dir.path().join("첫째");
        let second_target = dir.path().join("둘째");
        fs::create_dir_all(&first_target).unwrap();
        fs::create_dir_all(&second_target).unwrap();

        let mut queue = JobQueue::new();
        let first = queue
            .copy(vec![source.clone()], first_target.clone())
            .unwrap();
        let second = queue
            .copy(vec![source.clone()], second_target.clone())
            .unwrap();
        assert_ne!(first, second);
        assert!(queue.has_unfinished());
        let events = collect_until_finished(&mut queue, &[first, second]);

        let position = |wanted: &JobEvent| {
            events
                .iter()
                .position(|e| e == wanted)
                .unwrap_or_else(|| panic!("{wanted:?} 없음: {events:?}"))
        };
        let queued_second = position(&JobEvent::StateChanged {
            job: second,
            state: JobState::Queued,
        });
        let running_first = position(&JobEvent::StateChanged {
            job: first,
            state: JobState::Running,
        });
        let finished_first = position(&JobEvent::StateChanged {
            job: first,
            state: JobState::Finished,
        });
        let running_second = position(&JobEvent::StateChanged {
            job: second,
            state: JobState::Running,
        });
        assert!(running_first < finished_first);
        assert!(queued_second < running_second);
        assert!(
            finished_first < running_second,
            "뒤 작업은 앞 작업이 끝난 뒤 시작한다: {events:?}"
        );
        // 앞 작업이 끝나기 전에는 뒤 작업의 진행 사건이 없다.
        assert!(events[..finished_first].iter().all(|e| match e {
            JobEvent::Progress { job, .. } => *job == first,
            _ => true,
        }));
        assert_same_tree(&source, &first_target.join("원본"));
        assert_same_tree(&source, &second_target.join("원본"));
        assert!(!queue.has_unfinished());
    }

    #[test]
    fn 진행_사건은_초당_10번_이하이고_마지막은_원본_전체와_같다() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("원본");
        let mut expected_items = 1; // 원본 폴더
        let mut expected_bytes = 0;
        for folder in 0..10 {
            let sub = source.join(format!("폴더 {folder}"));
            fs::create_dir_all(&sub).unwrap();
            expected_items += 1;
            for i in 0..40 {
                let len = 30_000 + i * 7;
                fs::write(sub.join(format!("{i}.bin")), pattern(len, i)).unwrap();
                expected_items += 1;
                expected_bytes += len as u64;
            }
        }
        // 큰 파일 하나로 파일 안 진행(콜백)도 지나게 한다.
        let big = 48 * 1024 * 1024;
        fs::write(source.join("큰 파일.bin"), pattern(big, 9)).unwrap();
        expected_items += 1;
        expected_bytes += big as u64;
        let target = dir.path().join("대상");
        fs::create_dir_all(&target).unwrap();

        let mut queue = JobQueue::new();
        let events = copy_and_wait(&mut queue, vec![source.clone()], &target);
        let progress: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                JobEvent::Progress { progress, .. } => Some(progress.clone()),
                _ => None,
            })
            .collect();
        assert!(!progress.is_empty());
        for pair in progress.windows(2) {
            let gap = pair[1].elapsed - pair[0].elapsed;
            assert!(
                gap >= Duration::from_millis(100),
                "진행 사건 사이가 100ms보다 짧다: {gap:?}"
            );
            assert!(pair[1].bytes_done >= pair[0].bytes_done);
            assert!(pair[1].items_done >= pair[0].items_done);
        }
        for p in &progress {
            if !p.counted {
                assert_eq!(p.remaining, Remaining::Calculating, "{p:?}");
            }
            assert!(p.items_done <= p.items_total, "{p:?}");
            assert!(p.bytes_done <= p.bytes_total, "{p:?}");
        }
        let last = progress.last().unwrap();
        assert!(last.counted);
        assert_eq!(last.items_total, expected_items);
        assert_eq!(last.items_done, expected_items);
        assert_eq!(last.bytes_total, expected_bytes);
        assert_eq!(last.bytes_done, expected_bytes);
        assert_eq!(last.remaining, Remaining::Known(Duration::ZERO));
        assert_same_tree(&source, &target.join("원본"));
    }

    #[test]
    fn 훑기가_끝나기_전에_복사가_시작된다() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("원본");
        // 폴더가 많으면 훑기(폴더마다 목록 읽기)가 오래 걸린다. 첫 파일은 앞쪽 폴더에 있다.
        for folder in 0..1500 {
            let sub = source.join(format!("{folder:04}"));
            fs::create_dir_all(&sub).unwrap();
            fs::write(sub.join("a.txt"), "내용").unwrap();
        }
        let target = dir.path().join("대상");
        fs::create_dir_all(&target).unwrap();

        let mut queue = JobQueue::new();
        let events = copy_and_wait(&mut queue, vec![source.clone()], &target);
        let early = events.iter().any(|e| match e {
            JobEvent::Progress { progress, .. } => !progress.counted && progress.bytes_done > 0,
            _ => false,
        });
        assert!(early, "다 세기 전에 복사한 진행 사건이 없다: {events:?}");
        assert_same_tree(&source, &target.join("원본"));
    }

    #[test]
    fn 같은_폴더로_복사하면_복사본_이름을_만든다() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("보고서.txt");
        let folder = dir.path().join("사진.2024");
        let plain = dir.path().join("Makefile");
        fs::write(&file, "본문").unwrap();
        fs::create_dir_all(folder.join("안")).unwrap();
        fs::write(folder.join("안").join("x.jpg"), "jpg").unwrap();
        fs::write(&plain, "all:").unwrap();

        let mut queue = JobQueue::new();
        let sources = vec![file.clone(), folder.clone(), plain.clone()];
        copy_and_wait(&mut queue, sources.clone(), dir.path());
        assert_eq!(
            fs::read_to_string(dir.path().join("보고서 - 복사본.txt")).unwrap(),
            "본문"
        );
        assert_same_tree(&folder, &dir.path().join("사진.2024 - 복사본"));
        assert!(dir.path().join("Makefile - 복사본").is_file());

        copy_and_wait(&mut queue, sources, dir.path());
        assert!(dir.path().join("보고서 - 복사본 (2).txt").is_file());
        assert_same_tree(&folder, &dir.path().join("사진.2024 - 복사본 (2)"));
        assert!(dir.path().join("Makefile - 복사본 (2)").is_file());

        // 같은 이름의 두 원본을 한 작업에서 같은 폴더로 복사해도 서로 다른 이름이 된다.
        copy_and_wait(&mut queue, vec![file.clone(), file.clone()], dir.path());
        assert!(dir.path().join("보고서 - 복사본 (3).txt").is_file());
        assert!(dir.path().join("보고서 - 복사본 (4).txt").is_file());
    }

    #[test]
    fn 긴_경로와_한글_이모지_이름을_복사한다() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("원본 🌏");
        let mut deep = source.clone();
        for i in 0..6 {
            deep.push(format!(
                "아주 긴 폴더 이름 {i} 📁 가나다라마바사아자차카타파하 abcdefghij"
            ));
        }
        fs::create_dir_all(&deep).unwrap();
        let file = deep.join("원본 🎉 파일.txt");
        fs::write(&file, "긴 경로 내용 ✨").unwrap();
        assert!(file.as_os_str().len() > 260);
        let target = dir.path().join("대상 🚀");
        fs::create_dir_all(&target).unwrap();

        let mut queue = JobQueue::new();
        let events = copy_and_wait(&mut queue, vec![source.clone()], &target);
        let id = match events.last().unwrap() {
            JobEvent::Finished { job, .. } => *job,
            other => panic!("{other:?}"),
        };
        let summary = summary_of(&events, id);
        assert!(summary.failures.is_empty(), "{summary:?}");
        let copied = target.join(file.strip_prefix(dir.path()).unwrap());
        assert!(copied.as_os_str().len() > 260);
        assert_eq!(fs::read_to_string(&copied).unwrap(), "긴 경로 내용 ✨");
        assert_same_tree(&source, &target.join("원본 🌏"));
    }

    /// 일시정지·취소·지금 시작을 확인하는 큰 파일의 크기. 복사가 일시정지·취소를 부르기 전에 끝나지 않을 만큼
    /// 크고(무버퍼 입출력 문턱 256MiB도 넘는다), 테스트가 너무 느려지지 않을 만큼 작다.
    const BIG: usize = 300 * 1024 * 1024;

    /// 큰 파일을 1MiB 덩어리로 쓴다(덩어리마다 내용이 달라 어긋난 복사를 잡는다).
    fn write_big(path: &Path) {
        use std::io::Write;
        let mut file = std::io::BufWriter::new(fs::File::create(path).unwrap());
        let chunk = 1024 * 1024;
        for i in 0..BIG / chunk {
            file.write_all(&pattern(chunk, i)).unwrap();
        }
        file.flush().unwrap();
    }

    /// 두 파일의 내용이 같은지 덩어리로 비교한다.
    fn assert_same_content(a: &Path, b: &Path) {
        use std::io::Read;
        assert_eq!(
            fs::metadata(a).unwrap().len(),
            fs::metadata(b).unwrap().len()
        );
        let mut a = std::io::BufReader::new(fs::File::open(a).unwrap());
        let mut b = std::io::BufReader::new(fs::File::open(b).unwrap());
        let mut x = vec![0; 1024 * 1024];
        let mut y = vec![0; 1024 * 1024];
        loop {
            let n = a.read(&mut x).unwrap();
            if n == 0 {
                break;
            }
            b.read_exact(&mut y[..n]).unwrap();
            assert!(x[..n] == y[..n], "내용이 다르다");
        }
    }

    /// `wanted`인 사건이 올 때까지 사건을 `events`에 모은다.
    fn wait_for(
        queue: &mut JobQueue,
        events: &mut Vec<JobEvent>,
        wanted: impl Fn(&JobEvent) -> bool,
    ) {
        let deadline = std::time::Instant::now() + JOB_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
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

    fn state_changed(job: JobId, state: JobState) -> JobEvent {
        JobEvent::StateChanged { job, state }
    }

    fn position(events: &[JobEvent], wanted: &JobEvent) -> usize {
        events
            .iter()
            .position(|e| e == wanted)
            .unwrap_or_else(|| panic!("{wanted:?} 없음: {events:?}"))
    }

    fn is_copying(event: &JobEvent, id: JobId) -> bool {
        matches!(event, JobEvent::Progress { job, progress } if *job == id && progress.bytes_done > 0)
    }

    #[test]
    fn 일시정지_동안_대상_파일이_자라지_않고_이어하면_끝나며_내용이_같다() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("큰 파일.bin");
        write_big(&source);
        let target_dir = dir.path().join("대상");
        fs::create_dir_all(&target_dir).unwrap();
        let target = target_dir.join("큰 파일.bin");

        let mut queue = JobQueue::new();
        let id = queue
            .copy(vec![source.clone()], target_dir.clone())
            .unwrap();
        let mut events = Vec::new();
        wait_for(&mut queue, &mut events, |e| is_copying(e, id));
        assert!(queue.pause(id), "실행 중인 작업은 일시정지된다");
        assert_eq!(queue.state(id), Some(JobState::Paused));
        assert!(!queue.pause(id), "이미 일시정지한 작업은 그대로");

        // 진행 콜백이 멈춤을 볼 때까지 조금 기다린 뒤부터 잰다.
        std::thread::sleep(Duration::from_millis(300));
        events.extend(queue.poll());
        assert!(
            events.contains(&state_changed(id, JobState::Paused)),
            "{events:?}"
        );
        let before = fs::metadata(&target).unwrap().len();
        std::thread::sleep(Duration::from_millis(1000));
        let after = fs::metadata(&target).unwrap().len();
        assert_eq!(before, after, "일시정지 동안 대상 파일 크기가 늘었다");
        let during = queue.poll();
        assert!(
            during
                .iter()
                .all(|e| !matches!(e, JobEvent::Progress { .. } | JobEvent::Finished { .. })),
            "일시정지 동안 진행·끝남이 왔다: {during:?}"
        );
        assert!(queue.has_unfinished());

        assert!(queue.resume(id));
        assert!(!queue.resume(id), "이미 이어 간 작업은 그대로");
        let mut rest = collect_until_finished(&mut queue, &[id]);
        assert_eq!(rest[0], state_changed(id, JobState::Running));
        events.append(&mut rest);
        assert!(events.contains(&state_changed(id, JobState::Finished)));
        let summary = summary_of(&events, id);
        assert_eq!(summary.files_copied, 1);
        assert_eq!(summary.items_done, 1);
        assert_eq!(summary.items_remaining, 0);
        assert!(summary.all_counted);
        assert_same_content(&source, &target);
    }

    #[test]
    fn 취소하면_만들다_만_파일이_없고_다_복사한_항목은_남고_남은_항목_수가_맞다() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("원본");
        fs::create_dir_all(&source).unwrap();
        // NTFS 목록은 이름 차례다: a… 다섯 개를 다 복사한 뒤 큰 파일을 복사하는 중에 취소하고, c… 셋은 못 한다.
        for i in 1..=5 {
            fs::write(source.join(format!("a{i}.bin")), pattern(1000, i)).unwrap();
        }
        let big = source.join("b 큰 파일.bin");
        write_big(&big);
        for i in 1..=3 {
            fs::write(source.join(format!("c{i}.bin")), pattern(1000, i)).unwrap();
        }
        let target = dir.path().join("대상");
        fs::create_dir_all(&target).unwrap();
        let next_source = dir.path().join("다음.txt");
        fs::write(&next_source, "다음 작업").unwrap();
        let next_target = dir.path().join("다음 대상");
        fs::create_dir_all(&next_target).unwrap();

        let mut queue = JobQueue::new();
        let id = queue.copy(vec![source.clone()], target.clone()).unwrap();
        let next = queue
            .copy(vec![next_source.clone()], next_target.clone())
            .unwrap();
        let mut events = Vec::new();
        // 진행 사건은 100ms 간격으로 솎아 큰 파일 복사가 그 안에 끝나면 오지 않으므로(캐시에 든 파일), 사건 대신
        // 대상에 큰 파일이 생기는 것을 보고 곧바로 멈춘 뒤 취소해 복사하는 도중에 취소한다.
        let started = target.join("원본").join("b 큰 파일.bin");
        let deadline = std::time::Instant::now() + JOB_TIMEOUT;
        while !started.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "큰 파일 복사가 시작하지 않았다: {events:?}"
            );
            events.append(&mut queue.poll());
            std::thread::sleep(std::time::Duration::from_micros(200));
        }
        assert!(queue.pause(id), "큰 파일 복사가 이미 끝났다: {events:?}");
        assert!(queue.cancel(id));
        assert!(!queue.cancel(id), "취소 중인 작업은 다시 취소하지 않는다");
        assert!(!queue.pause(id), "취소 중인 작업은 일시정지하지 않는다");
        let mut rest = collect_until_finished(&mut queue, &[id, next]);
        events.append(&mut rest);

        let cancelled = position(&events, &state_changed(id, JobState::Cancelled));
        let finished = position(
            &events,
            &JobEvent::Finished {
                job: id,
                summary: summary_of(&events, id),
            },
        );
        assert_eq!(finished, cancelled + 1);
        let summary = summary_of(&events, id);
        assert_eq!(summary.files_copied, 5, "{summary:?}");
        assert_eq!(summary.folders_copied, 1, "{summary:?}");
        assert!(summary.failures.is_empty(), "{summary:?}");
        assert!(summary.all_counted, "{summary:?}");
        // 원본 폴더 + a 다섯 개를 했고, 큰 파일 + c 셋이 남았다.
        assert_eq!(summary.items_done, 6, "{summary:?}");
        assert_eq!(summary.items_remaining, 4, "{summary:?}");

        let copied = target.join("원본");
        let mut names: Vec<_> = fs::read_dir(&copied)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["a1.bin", "a2.bin", "a3.bin", "a4.bin", "a5.bin"],
            "만들다 만 파일이나 못 한 항목이 있다"
        );
        for i in 1..=5 {
            let name = format!("a{i}.bin");
            assert_same_content(&source.join(&name), &copied.join(&name));
        }

        // 취소한 작업 뒤의 작업은 막히지 않고 돈다.
        assert!(events.contains(&state_changed(next, JobState::Finished)));
        assert!(position(&events, &state_changed(next, JobState::Running)) > cancelled);
        assert_eq!(
            fs::read_to_string(next_target.join("다음.txt")).unwrap(),
            "다음 작업"
        );
        assert!(!queue.has_unfinished());
    }

    #[test]
    fn 대기_중에_일시정지한_작업은_차례가_와도_시작하지_않고_뒤_작업이_먼저_돈다() {
        let dir = TempDir::new().unwrap();
        let big = dir.path().join("큰 파일.bin");
        write_big(&big);
        let small = dir.path().join("작은 파일.txt");
        fs::write(&small, "작은").unwrap();
        let targets: Vec<PathBuf> = (0..4)
            .map(|i| {
                let target = dir.path().join(format!("대상 {i}"));
                fs::create_dir_all(&target).unwrap();
                target
            })
            .collect();

        let mut queue = JobQueue::new();
        let first = queue.copy(vec![big.clone()], targets[0].clone()).unwrap();
        let mut events = Vec::new();
        wait_for(&mut queue, &mut events, |e| {
            *e == state_changed(first, JobState::Running)
        });
        // 시작하자마자 멈춰 둔다(차례를 쥔 채 멈추므로 뒤 작업을 넣는 동안 앞 작업이 끝나 버리지 않는다).
        assert!(queue.pause(first), "앞 작업이 이미 끝났다: {events:?}");
        let paused = queue.copy(vec![small.clone()], targets[1].clone()).unwrap();
        let after = queue.copy(vec![small.clone()], targets[2].clone()).unwrap();
        let dropped = queue.copy(vec![small.clone()], targets[3].clone()).unwrap();
        assert_eq!(queue.state(after), Some(JobState::Queued));
        assert!(queue.pause(paused));
        assert!(queue.pause(dropped));
        assert_eq!(queue.state(paused), Some(JobState::Paused));
        // 도는 앞 작업을 취소하면 차례는 일시정지한 작업을 건너 다음 작업으로 간다.
        assert!(queue.cancel(first));
        wait_for(
            &mut queue,
            &mut events,
            |e| matches!(e, JobEvent::Finished { job, .. } if *job == after),
        );
        assert!(
            events.contains(&state_changed(first, JobState::Cancelled)),
            "{events:?}"
        );
        assert!(events.contains(&state_changed(after, JobState::Finished)));
        assert!(
            !events.contains(&state_changed(paused, JobState::Running)),
            "일시정지한 대기 작업이 시작했다: {events:?}"
        );
        assert_eq!(queue.state(paused), Some(JobState::Paused));
        assert!(!targets[0].join("큰 파일.bin").exists());
        assert!(!targets[1].join("작은 파일.txt").exists());
        assert!(targets[2].join("작은 파일.txt").is_file());

        // 시작 전에 일시정지한 작업을 취소하면 곧바로 끝나고 남은 항목은 넘긴 원본 수다.
        assert!(queue.cancel(dropped));
        let mut tail = queue.poll();
        assert_eq!(
            tail,
            [
                state_changed(dropped, JobState::Cancelled),
                JobEvent::Finished {
                    job: dropped,
                    summary: facet_core::job::Summary {
                        items_remaining: 1,
                        ..Default::default()
                    }
                }
            ]
        );
        events.append(&mut tail);

        // 이어 가면 대기로 돌아가 곧 시작한다.
        assert!(queue.resume(paused));
        let mut rest = collect_until_finished(&mut queue, &[paused]);
        assert_eq!(rest[0], state_changed(paused, JobState::Queued));
        assert_eq!(rest[1], state_changed(paused, JobState::Running));
        events.append(&mut rest);
        assert!(events.contains(&state_changed(paused, JobState::Finished)));
        assert!(targets[1].join("작은 파일.txt").is_file());
        assert!(!targets[3].join("작은 파일.txt").exists());
        assert!(!queue.has_unfinished());
    }

    #[test]
    fn 지금_시작한_작업은_앞_작업과_나란히_돌고_차례_실행은_이어진다() {
        let dir = TempDir::new().unwrap();
        let big = dir.path().join("큰 파일.bin");
        write_big(&big);
        let small = dir.path().join("급한 파일.txt");
        fs::write(&small, "급함").unwrap();
        let targets: Vec<PathBuf> = (0..3)
            .map(|i| {
                let target = dir.path().join(format!("대상 {i}"));
                fs::create_dir_all(&target).unwrap();
                target
            })
            .collect();

        let mut queue = JobQueue::new();
        let first = queue.copy(vec![big.clone()], targets[0].clone()).unwrap();
        let urgent = queue.copy(vec![small.clone()], targets[1].clone()).unwrap();
        let later = queue.copy(vec![small.clone()], targets[2].clone()).unwrap();
        let mut events = Vec::new();
        // 앞 작업이 시작하자마자(큰 파일을 복사하는 동안) 지금 시작한다.
        wait_for(&mut queue, &mut events, |e| {
            *e == state_changed(first, JobState::Running)
        });
        // 복사 속도에 기대지 않도록 앞 작업을 멈춰 순서를 고정한다(차례를 쥔 채 멈춤).
        assert!(queue.pause(first), "앞 작업이 이미 끝났다: {events:?}");
        assert!(queue.start_now(urgent));
        assert!(!queue.start_now(urgent), "이미 시작한 작업");
        assert!(!queue.start_now(first), "실행 중인 작업");
        // 앞 작업이 멈춰 있는 채로 급한 작업이 끝난다: 나란히 돈 것이다.
        wait_for(
            &mut queue,
            &mut events,
            |e| matches!(e, JobEvent::Finished { job, .. } if *job == urgent),
        );
        assert!(
            !events.contains(&state_changed(first, JobState::Finished)),
            "{events:?}"
        );
        assert!(
            !events.contains(&state_changed(later, JobState::Running)),
            "멈춘 앞 작업이 차례를 쥐고 있는데 다음 작업이 시작했다: {events:?}"
        );
        assert!(targets[1].join("급한 파일.txt").is_file());
        // 앞 작업을 이어 가면 끝나고, 그제야 차례의 다음 작업이 돈다.
        assert!(queue.resume(first));
        let mut rest = collect_until_finished(&mut queue, &[first, later]);
        events.append(&mut rest);

        let running_first = position(&events, &state_changed(first, JobState::Running));
        let running_urgent = position(&events, &state_changed(urgent, JobState::Running));
        let finished_urgent = position(&events, &state_changed(urgent, JobState::Finished));
        let finished_first = position(&events, &state_changed(first, JobState::Finished));
        let running_later = position(&events, &state_changed(later, JobState::Running));
        assert!(
            running_first < running_urgent && finished_urgent < finished_first,
            "{events:?}"
        );
        assert!(finished_first < running_later, "{events:?}");
        assert_same_content(&big, &targets[0].join("큰 파일.bin"));
        assert!(targets[1].join("급한 파일.txt").is_file());
        assert!(targets[2].join("급한 파일.txt").is_file());
        assert!(!queue.has_unfinished());
    }

    #[test]
    fn 끝난_작업에_일시정지_취소_지금_시작을_하면_아무_일도_없다() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("a.txt");
        fs::write(&source, "a").unwrap();
        let target = dir.path().join("대상");
        fs::create_dir_all(&target).unwrap();

        let mut queue = JobQueue::new();
        let id = queue.copy(vec![source.clone()], target.clone()).unwrap();
        collect_until_finished(&mut queue, &[id]);
        assert_eq!(queue.state(id), Some(JobState::Finished));
        assert!(!queue.pause(id));
        assert!(!queue.resume(id));
        assert!(!queue.cancel(id));
        assert!(!queue.start_now(id));
        assert!(queue.poll().is_empty(), "끝난 작업에 사건이 왔다");
        assert!(!queue.has_unfinished());
        assert_eq!(fs::read_to_string(target.join("a.txt")).unwrap(), "a");
    }
}
