//! 복사 중 이름 충돌(대상에 같은 이름이 이미 있음)의 질문과 답을 임시 폴더의 실제 파일로 확인한다.
//!
//! 파일 하나의 복사는 facet-win(`CopyFile2`)에 맡기므로 Windows 전용이다.

#![cfg(windows)]

use std::fs::{self, FileTimes, OpenOptions};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use facet_core::job::{
    AnswerError, Conflict, ConflictAnswer, ItemKind, JobEvent, JobId, JobQueue, JobState,
    Resolution, Summary,
};
use tempfile::TempDir;

/// 사건을 기다리는 넉넉한 한도.
const TIMEOUT: Duration = Duration::from_secs(120);

const READONLY: u32 = 0x1;

/// `wanted`인 사건이 올 때까지 사건을 `events`에 모은다.
fn wait_for(queue: &mut JobQueue, events: &mut Vec<JobEvent>, wanted: impl Fn(&JobEvent) -> bool) {
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

/// `job`의 다음 이름 충돌 질문을 기다린다. 그 전에 작업이 끝나면 실패한다.
fn wait_conflict(queue: &mut JobQueue, events: &mut Vec<JobEvent>, job: JobId) -> Conflict {
    wait_for(queue, events, |e| match e {
        JobEvent::Conflict { job: id, .. } => *id == job,
        JobEvent::Finished { job: id, .. } => *id == job,
        _ => false,
    });
    match events.last().unwrap() {
        JobEvent::Conflict { conflict, .. } => conflict.clone(),
        other => panic!("질문 전에 작업이 끝났다: {other:?} / {events:?}"),
    }
}

/// `job`이 끝날 때까지 기다려 결과를 돌려준다. 그 사이 그 작업의 질문이 오면 실패한다.
fn wait_finished(queue: &mut JobQueue, events: &mut Vec<JobEvent>, job: JobId) -> Summary {
    let start = events.len();
    wait_for(
        queue,
        events,
        |e| matches!(e, JobEvent::Finished { job: id, .. } if *id == job),
    );
    assert!(
        !events[start..]
            .iter()
            .any(|e| matches!(e, JobEvent::Conflict { job: id, .. } if *id == job)),
        "더 묻지 않아야 한다: {events:?}"
    );
    match events.last().unwrap() {
        JobEvent::Finished { summary, .. } => summary.clone(),
        _ => unreachable!(),
    }
}

fn state_changed(job: JobId, state: JobState) -> JobEvent {
    JobEvent::StateChanged { job, state }
}

fn set_modified(path: &Path, seconds: u64) {
    let file = OpenOptions::new()
        .access_mode(0x100) // FILE_WRITE_ATTRIBUTES
        .open(path)
        .unwrap();
    file.set_times(
        FileTimes::new().set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)),
    )
    .unwrap();
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
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

/// 원본 「a.txt」(새 내용, 시각 `source_time`)와 대상 「a.txt」(원래 내용, 시각 `target_time`)를 만든다.
fn conflicting_file(dir: &Path, source_time: u64, target_time: u64) -> (PathBuf, PathBuf) {
    let source_dir = dir.join("원본");
    let target_dir = dir.join("대상");
    fs::create_dir_all(&source_dir).unwrap();
    fs::create_dir_all(&target_dir).unwrap();
    let source = source_dir.join("a.txt");
    fs::write(&source, "새 내용").unwrap();
    fs::write(target_dir.join("a.txt"), "원래").unwrap();
    set_modified(&source, source_time);
    set_modified(&target_dir.join("a.txt"), target_time);
    (source, target_dir)
}

/// 원본 하나를 복사하다 충돌에 `answer`로 답하고 끝날 때까지 기다린다.
fn copy_answering(
    queue: &mut JobQueue,
    source: &Path,
    target: &Path,
    answer: Resolution,
) -> Summary {
    let id = queue
        .copy(vec![source.to_path_buf()], target.to_path_buf())
        .unwrap();
    let mut events = Vec::new();
    wait_conflict(queue, &mut events, id);
    queue
        .answer_conflict(id, ConflictAnswer::once(answer))
        .unwrap();
    wait_finished(queue, &mut events, id)
}

#[test]
fn 충돌_질문에는_두_항목의_이름_크기_수정_시각_종류가_있다() {
    let dir = TempDir::new().unwrap();
    let (source, target_dir) = conflicting_file(dir.path(), 1_700_000_000, 1_600_000_000);
    let mut queue = JobQueue::new();
    let id = queue
        .copy(vec![source.clone()], target_dir.clone())
        .unwrap();
    let mut events = Vec::new();
    let conflict = wait_conflict(&mut queue, &mut events, id);
    let n = events.len();
    assert_eq!(events[n - 2], state_changed(id, JobState::AwaitingAnswer));
    assert_eq!(queue.state(id), Some(JobState::AwaitingAnswer));

    assert_eq!(conflict.source.path, source);
    assert_eq!(conflict.target.path, target_dir.join("a.txt"));
    assert_eq!(conflict.source.kind, ItemKind::File);
    assert_eq!(conflict.target.kind, ItemKind::File);
    assert_eq!(conflict.source.size, Some("새 내용".len() as u64));
    assert_eq!(conflict.target.size, Some("원래".len() as u64));
    assert_eq!(
        conflict.source.modified,
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000))
    );
    assert_eq!(
        conflict.target.modified,
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000))
    );
    assert!(conflict.can_overwrite);
    assert!(conflict.overwrite_is_permanent, "덮어쓰기는 되살릴 수 없다");

    // 일시정지·지금 시작은 답 기다림 중에 하지 않는다.
    assert!(!queue.pause(id));
    assert!(!queue.start_now(id));
    queue
        .answer_conflict(id, ConflictAnswer::once(Resolution::Skip))
        .unwrap();
    assert_eq!(
        queue.answer_conflict(id, ConflictAnswer::once(Resolution::Skip)),
        Err(AnswerError::NotAsking),
        "이미 답한 작업"
    );
    let summary = wait_finished(&mut queue, &mut events, id);
    assert!(events.contains(&state_changed(id, JobState::Running)));
    assert!(events.contains(&state_changed(id, JobState::Finished)));
    assert_eq!(summary.conflicts_skipped, 1);
}

#[test]
fn 덮어쓰기는_대상을_원본으로_바꾼다() {
    let dir = TempDir::new().unwrap();
    let (source, target_dir) = conflicting_file(dir.path(), 1_600_000_000, 1_700_000_000);
    let mut queue = JobQueue::new();
    let summary = copy_answering(&mut queue, &source, &target_dir, Resolution::Overwrite);
    assert_eq!(read(&target_dir.join("a.txt")), "새 내용");
    assert_eq!(names(&target_dir), ["a.txt"], "임시 파일이 남았다");
    assert_eq!(summary.files_copied, 1, "{summary:?}");
    assert_eq!(summary.files_overwritten, 1, "{summary:?}");
    assert!(summary.failures.is_empty());
}

#[test]
fn 건너뛰기는_대상을_그대로_둔다() {
    let dir = TempDir::new().unwrap();
    let (source, target_dir) = conflicting_file(dir.path(), 1_700_000_000, 1_600_000_000);
    let mut queue = JobQueue::new();
    let summary = copy_answering(&mut queue, &source, &target_dir, Resolution::Skip);
    assert_eq!(read(&target_dir.join("a.txt")), "원래");
    assert_eq!(names(&target_dir), ["a.txt"]);
    assert_eq!(summary.conflicts_skipped, 1, "{summary:?}");
    assert_eq!(summary.files_copied, 0, "{summary:?}");
}

#[test]
fn 둘_다_유지는_이름_2를_만들고_다시_하면_이름_3을_만든다() {
    let dir = TempDir::new().unwrap();
    let (source, target_dir) = conflicting_file(dir.path(), 1_700_000_000, 1_600_000_000);
    let mut queue = JobQueue::new();
    let summary = copy_answering(&mut queue, &source, &target_dir, Resolution::KeepBoth);
    assert_eq!(summary.files_copied, 1, "{summary:?}");
    assert_eq!(read(&target_dir.join("a.txt")), "원래");
    assert_eq!(read(&target_dir.join("a (2).txt")), "새 내용");

    copy_answering(&mut queue, &source, &target_dir, Resolution::KeepBoth);
    assert_eq!(read(&target_dir.join("a (3).txt")), "새 내용");
    assert_eq!(names(&target_dir), ["a (2).txt", "a (3).txt", "a.txt"]);

    // 폴더는 이름 전체 뒤에 붙인다.
    let folder = dir.path().join("원본").join("사진.2024");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("x.jpg"), "jpg").unwrap();
    let target_file = target_dir.join("사진.2024");
    fs::write(&target_file, "같은 이름의 파일").unwrap();
    copy_answering(&mut queue, &folder, &target_dir, Resolution::KeepBoth);
    assert_eq!(read(&target_dir.join("사진.2024 (2)").join("x.jpg")), "jpg");
    assert_eq!(read(&target_file), "같은 이름의 파일");
}

#[test]
fn 더_새것만_덮어쓰기는_원본이_새것일_때만_덮어쓴다() {
    let dir = TempDir::new().unwrap();
    let (source, target_dir) = conflicting_file(dir.path(), 1_700_000_000, 1_600_000_000);
    let mut queue = JobQueue::new();
    let summary = copy_answering(
        &mut queue,
        &source,
        &target_dir,
        Resolution::OverwriteIfNewer,
    );
    assert_eq!(read(&target_dir.join("a.txt")), "새 내용", "원본이 새것");
    assert_eq!(summary.files_overwritten, 1, "{summary:?}");

    let older = TempDir::new().unwrap();
    let (source, target_dir) = conflicting_file(older.path(), 1_600_000_000, 1_700_000_000);
    let summary = copy_answering(
        &mut queue,
        &source,
        &target_dir,
        Resolution::OverwriteIfNewer,
    );
    assert_eq!(read(&target_dir.join("a.txt")), "원래", "원본이 헌것");
    assert_eq!(summary.conflicts_skipped, 1, "{summary:?}");
    assert_eq!(summary.files_copied, 0, "{summary:?}");
    assert_eq!(names(&target_dir), ["a.txt"]);

    // 시각이 같으면 새것이 아니다.
    let same = TempDir::new().unwrap();
    let (source, target_dir) = conflicting_file(same.path(), 1_650_000_000, 1_650_000_000);
    copy_answering(
        &mut queue,
        &source,
        &target_dir,
        Resolution::OverwriteIfNewer,
    );
    assert_eq!(read(&target_dir.join("a.txt")), "원래");
}

#[test]
fn 나머지에도_적용하면_같은_작업의_남은_충돌에_묻지_않는다() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("원본");
    let target = dir.path().join("대상");
    fs::create_dir_all(source.join("폴더")).unwrap();
    fs::create_dir_all(target.join("폴더")).unwrap();
    for i in 0..5 {
        fs::write(
            source.join("폴더").join(format!("{i}.txt")),
            format!("새 {i}"),
        )
        .unwrap();
        fs::write(target.join("폴더").join(format!("{i}.txt")), "원래").unwrap();
    }
    fs::write(source.join("b.txt"), "새 b").unwrap();
    fs::write(target.join("b.txt"), "원래 b").unwrap();

    let mut queue = JobQueue::new();
    let id = queue
        .copy(
            vec![source.join("폴더"), source.join("b.txt")],
            target.clone(),
        )
        .unwrap();
    let mut events = Vec::new();
    let conflict = wait_conflict(&mut queue, &mut events, id);
    assert_eq!(
        conflict.source.kind,
        ItemKind::File,
        "폴더끼리는 묻지 않는다"
    );
    queue
        .answer_conflict(id, ConflictAnswer::for_rest(Resolution::Overwrite))
        .unwrap();
    let summary = wait_finished(&mut queue, &mut events, id);
    assert_eq!(summary.files_overwritten, 6, "{summary:?}");
    for i in 0..5 {
        assert_eq!(
            read(&target.join("폴더").join(format!("{i}.txt"))),
            format!("새 {i}")
        );
    }
    assert_eq!(read(&target.join("b.txt")), "새 b");

    // 다른 작업에는 이어지지 않는다: 다시 묻는다.
    let again = queue
        .copy(vec![source.join("b.txt")], target.clone())
        .unwrap();
    wait_conflict(&mut queue, &mut events, again);
    assert!(queue.cancel(again));
    wait_for(
        &mut queue,
        &mut events,
        |e| matches!(e, JobEvent::Finished { job, .. } if *job == again),
    );
}

#[test]
fn 같은_이름_폴더는_묻지_않고_합치고_안의_파일_충돌만_묻는다() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("원본").join("폴더");
    let target = dir.path().join("대상");
    fs::create_dir_all(source.join("안")).unwrap();
    fs::create_dir_all(target.join("폴더").join("안")).unwrap();
    fs::write(source.join("x.txt"), "새 x").unwrap();
    fs::write(source.join("y.txt"), "새 y").unwrap();
    fs::write(source.join("안").join("w.txt"), "새 w").unwrap();
    fs::write(target.join("폴더").join("x.txt"), "원래 x").unwrap();
    fs::write(target.join("폴더").join("z.txt"), "원래 z").unwrap();
    attrib(&target.join("폴더"), &["+h"]);
    let target_attributes = fs::metadata(target.join("폴더")).unwrap().file_attributes();

    let mut queue = JobQueue::new();
    let id = queue.copy(vec![source.clone()], target.clone()).unwrap();
    let mut events = Vec::new();
    let conflict = wait_conflict(&mut queue, &mut events, id);
    assert_eq!(conflict.source.path, source.join("x.txt"));
    queue
        .answer_conflict(id, ConflictAnswer::once(Resolution::Skip))
        .unwrap();
    let summary = wait_finished(&mut queue, &mut events, id);
    let conflicts = events
        .iter()
        .filter(|e| matches!(e, JobEvent::Conflict { .. }))
        .count();
    assert_eq!(conflicts, 1, "안의 파일 충돌 하나만 묻는다: {events:?}");
    assert_eq!(summary.folders_merged, 2, "{summary:?}");
    assert_eq!(summary.folders_copied, 0, "{summary:?}");
    assert_eq!(summary.files_copied, 2, "{summary:?}");

    let merged = target.join("폴더");
    assert_eq!(names(&merged), ["x.txt", "y.txt", "z.txt", "안"]);
    assert_eq!(read(&merged.join("x.txt")), "원래 x");
    assert_eq!(read(&merged.join("y.txt")), "새 y");
    assert_eq!(read(&merged.join("z.txt")), "원래 z");
    assert_eq!(read(&merged.join("안").join("w.txt")), "새 w");
    assert_eq!(
        fs::metadata(&merged).unwrap().file_attributes(),
        target_attributes,
        "합친 폴더의 속성은 대상 것 그대로"
    );
}

#[test]
fn 파일과_폴더가_부딪히면_덮어쓰기를_고를_수_없다() {
    let dir = TempDir::new().unwrap();
    let source_dir = dir.path().join("원본");
    let target = dir.path().join("대상");
    fs::create_dir_all(source_dir.join("b")).unwrap();
    fs::write(source_dir.join("b").join("안.txt"), "안").unwrap();
    fs::write(source_dir.join("a"), "파일 a").unwrap();
    fs::create_dir_all(target.join("a")).unwrap();
    fs::write(target.join("b"), "파일 b").unwrap();

    let mut queue = JobQueue::new();
    // 원본 파일 ↔ 대상 폴더.
    let id = queue
        .copy(
            vec![source_dir.join("a"), source_dir.join("b")],
            target.clone(),
        )
        .unwrap();
    let mut events = Vec::new();
    let conflict = wait_conflict(&mut queue, &mut events, id);
    assert_eq!(conflict.source.kind, ItemKind::File);
    assert_eq!(conflict.target.kind, ItemKind::Folder);
    assert_eq!(conflict.target.size, None);
    assert!(!conflict.can_overwrite);
    for resolution in [Resolution::Overwrite, Resolution::OverwriteIfNewer] {
        assert_eq!(
            queue.answer_conflict(id, ConflictAnswer::for_rest(resolution)),
            Err(AnswerError::OverwriteNotAllowed)
        );
    }
    assert_eq!(queue.state(id), Some(JobState::AwaitingAnswer));
    queue
        .answer_conflict(id, ConflictAnswer::once(Resolution::KeepBoth))
        .unwrap();

    // 원본 폴더 ↔ 대상 파일.
    let conflict = wait_conflict(&mut queue, &mut events, id);
    assert_eq!(conflict.source.kind, ItemKind::Folder);
    assert_eq!(conflict.target.kind, ItemKind::File);
    assert!(!conflict.can_overwrite);
    assert_eq!(
        queue.answer_conflict(id, ConflictAnswer::once(Resolution::Overwrite)),
        Err(AnswerError::OverwriteNotAllowed)
    );
    queue
        .answer_conflict(id, ConflictAnswer::once(Resolution::Skip))
        .unwrap();
    let summary = wait_finished(&mut queue, &mut events, id);
    assert!(summary.failures.is_empty(), "{summary:?}");
    assert_eq!(summary.conflicts_skipped, 1, "{summary:?}");

    assert!(target.join("a").is_dir());
    assert_eq!(read(&target.join("a (2)")), "파일 a");
    assert_eq!(read(&target.join("b")), "파일 b");
    assert_eq!(names(&target), ["a", "a (2)", "b"]);
}

#[test]
fn 나머지에도_적용한_덮어쓰기는_파일과_폴더_충돌에는_다시_묻는다() {
    let dir = TempDir::new().unwrap();
    let source_dir = dir.path().join("원본");
    let target = dir.path().join("대상");
    fs::create_dir_all(&source_dir).unwrap();
    fs::create_dir_all(target.join("b")).unwrap();
    fs::write(source_dir.join("a"), "새 a").unwrap();
    fs::write(source_dir.join("b"), "새 b").unwrap();
    fs::write(target.join("a"), "원래 a").unwrap();

    let mut queue = JobQueue::new();
    let id = queue
        .copy(
            vec![source_dir.join("a"), source_dir.join("b")],
            target.clone(),
        )
        .unwrap();
    let mut events = Vec::new();
    wait_conflict(&mut queue, &mut events, id);
    queue
        .answer_conflict(id, ConflictAnswer::for_rest(Resolution::Overwrite))
        .unwrap();
    let conflict = wait_conflict(&mut queue, &mut events, id);
    assert!(!conflict.can_overwrite);
    queue
        .answer_conflict(id, ConflictAnswer::once(Resolution::Skip))
        .unwrap();
    wait_finished(&mut queue, &mut events, id);
    assert_eq!(read(&target.join("a")), "새 a");
    assert!(target.join("b").is_dir());
}

#[test]
fn 읽기_전용_파일을_덮어쓴다() {
    let dir = TempDir::new().unwrap();
    let (source, target_dir) = conflicting_file(dir.path(), 1_700_000_000, 1_600_000_000);
    let target = target_dir.join("a.txt");
    attrib(&target, &["+r"]);
    let mut queue = JobQueue::new();
    let summary = copy_answering(&mut queue, &source, &target_dir, Resolution::Overwrite);
    assert!(summary.failures.is_empty(), "{summary:?}");
    assert_eq!(read(&target), "새 내용");
    assert_eq!(
        fs::metadata(&target).unwrap().file_attributes() & READONLY,
        0,
        "결과 속성은 원본 것"
    );
    assert_eq!(names(&target_dir), ["a.txt"]);
}

/// 덮어쓰기 중 취소를 확인할 큰 파일의 크기. 취소를 부르기 전에 복사가 끝나지 않을 만큼 크다.
const BIG: usize = 300 * 1024 * 1024;

fn write_big(path: &Path) {
    use std::io::Write;
    let mut file = std::io::BufWriter::new(fs::File::create(path).unwrap());
    let chunk: Vec<u8> = (0..1024 * 1024).map(|i| (i * 31) as u8).collect();
    for _ in 0..BIG / chunk.len() {
        file.write_all(&chunk).unwrap();
    }
    file.flush().unwrap();
}

#[test]
fn 덮어쓰기_중_취소하면_원래_파일이_그대로이고_임시_파일이_없다() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("큰 파일.bin");
    write_big(&source);
    let target_dir = dir.path().join("대상");
    fs::create_dir_all(&target_dir).unwrap();
    let target = target_dir.join("큰 파일.bin");
    fs::write(&target, "원래").unwrap();

    let mut queue = JobQueue::new();
    let id = queue
        .copy(vec![source.clone()], target_dir.clone())
        .unwrap();
    let mut events = Vec::new();
    wait_conflict(&mut queue, &mut events, id);
    queue
        .answer_conflict(id, ConflictAnswer::once(Resolution::Overwrite))
        .unwrap();
    wait_for(
        &mut queue,
        &mut events,
        |e| matches!(e, JobEvent::Progress { job, progress } if *job == id && progress.bytes_done > 0),
    );
    assert!(queue.cancel(id));
    wait_for(
        &mut queue,
        &mut events,
        |e| matches!(e, JobEvent::Finished { job, .. } if *job == id),
    );
    assert!(events.contains(&state_changed(id, JobState::Cancelled)));
    assert_eq!(read(&target), "원래");
    assert_eq!(names(&target_dir), ["큰 파일.bin"], "임시 파일이 남았다");
}

#[test]
fn 덮어쓰기_복사가_실패하면_원래_파일이_그대로이고_임시_파일이_없다() {
    let dir = TempDir::new().unwrap();
    let (source, target_dir) = conflicting_file(dir.path(), 1_700_000_000, 1_600_000_000);
    let mut queue = JobQueue::new();
    let id = queue
        .copy(vec![source.clone()], target_dir.clone())
        .unwrap();
    let mut events = Vec::new();
    wait_conflict(&mut queue, &mut events, id);
    // 질문이 온 뒤 원본을 공유 없이 열어 둔다: 복사가 원본을 읽지 못한다.
    let lock = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&source)
        .unwrap();
    queue
        .answer_conflict(id, ConflictAnswer::once(Resolution::Overwrite))
        .unwrap();
    let summary = wait_finished(&mut queue, &mut events, id);
    drop(lock);
    assert_eq!(summary.failures.len(), 1, "{summary:?}");
    assert_eq!(summary.failures[0].source, source);
    assert!(events.contains(&state_changed(id, JobState::Failed)));
    assert_eq!(read(&target_dir.join("a.txt")), "원래");
    assert_eq!(names(&target_dir), ["a.txt"], "임시 파일이 남았다");
}

#[test]
fn 충돌_답을_기다리는_동안_대기열의_다른_작업이_끝난다() {
    let dir = TempDir::new().unwrap();
    let (source, target_dir) = conflicting_file(dir.path(), 1_700_000_000, 1_600_000_000);
    let other_source = dir.path().join("다른.txt");
    fs::write(&other_source, "다른 작업").unwrap();
    let other_target = dir.path().join("다른 대상");
    fs::create_dir_all(&other_target).unwrap();

    let mut queue = JobQueue::new();
    let asking = queue
        .copy(vec![source.clone()], target_dir.clone())
        .unwrap();
    let other = queue
        .copy(vec![other_source.clone()], other_target.clone())
        .unwrap();
    let mut events = Vec::new();
    wait_conflict(&mut queue, &mut events, asking);
    // 답하지 않은 채 뒤 작업이 차례를 받아 끝난다.
    let summary = wait_finished(&mut queue, &mut events, other);
    assert_eq!(summary.files_copied, 1);
    assert_eq!(read(&other_target.join("다른.txt")), "다른 작업");
    assert_eq!(queue.state(asking), Some(JobState::AwaitingAnswer));

    // 답하면 이어서 끝난다.
    queue
        .answer_conflict(asking, ConflictAnswer::once(Resolution::Overwrite))
        .unwrap();
    wait_finished(&mut queue, &mut events, asking);
    assert_eq!(read(&target_dir.join("a.txt")), "새 내용");
    assert!(!queue.has_unfinished());
}

#[test]
fn 답을_기다리는_작업도_취소한다() {
    let dir = TempDir::new().unwrap();
    let (source, target_dir) = conflicting_file(dir.path(), 1_700_000_000, 1_600_000_000);
    let mut queue = JobQueue::new();
    let id = queue
        .copy(vec![source.clone()], target_dir.clone())
        .unwrap();
    let mut events = Vec::new();
    wait_conflict(&mut queue, &mut events, id);
    assert!(queue.cancel(id));
    assert_eq!(
        queue.answer_conflict(id, ConflictAnswer::once(Resolution::Overwrite)),
        Err(AnswerError::NotAsking),
        "취소 중인 작업"
    );
    wait_for(
        &mut queue,
        &mut events,
        |e| matches!(e, JobEvent::Finished { job, .. } if *job == id),
    );
    assert!(events.contains(&state_changed(id, JobState::Cancelled)));
    assert_eq!(read(&target_dir.join("a.txt")), "원래");
    assert!(!queue.has_unfinished());
}

#[test]
fn 답을_기다리지_않는_작업에는_답할_수_없다() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("a.txt");
    fs::write(&source, "a").unwrap();
    let target = dir.path().join("대상");
    fs::create_dir_all(&target).unwrap();
    let mut queue = JobQueue::new();
    let id = queue.copy(vec![source], target).unwrap();
    let mut events = Vec::new();
    wait_finished(&mut queue, &mut events, id);
    assert_eq!(
        queue.answer_conflict(id, ConflictAnswer::once(Resolution::Skip)),
        Err(AnswerError::NotAsking)
    );
}
