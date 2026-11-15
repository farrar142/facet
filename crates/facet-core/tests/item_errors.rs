//! 복사 중 항목별 오류(잠김·접근 거부·원본 없음·디스크 가득 참)의 질문과 답을 임시 폴더의 실제 파일로 확인한다.
//!
//! 파일 하나의 복사는 facet-win(`CopyFile2`)에 맡기므로 Windows 전용이다. 디스크 가득 참은 작은 시험 볼륨을 채워
//! 보는 실제 볼륨 테스트(`real_volume_`, `FACET_TEST_VOLUME`)다.

#![cfg(windows)]

use std::env;
use std::fs::{self, File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use facet_core::job::{
    AnswerError, ConflictAnswer, ErrorAnswer, FileOpErrorKind, ItemFailure, JobEvent, JobId,
    JobQueue, JobState, Resolution, Summary,
};
use tempfile::TempDir;

/// 사건을 기다리는 넉넉한 한도.
const TIMEOUT: Duration = Duration::from_secs(120);

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

/// `job`의 다음 항목별 오류 질문을 기다린다. 그 전에 작업이 끝나면 실패한다.
fn wait_error(queue: &mut JobQueue, events: &mut Vec<JobEvent>, job: JobId) -> ItemFailure {
    wait_for(queue, events, |e| match e {
        JobEvent::ItemError { job: id, .. } | JobEvent::Finished { job: id, .. } => *id == job,
        _ => false,
    });
    let n = events.len();
    match &events[n - 1] {
        JobEvent::ItemError { error, .. } => {
            assert_eq!(
                events[n - 2],
                state_changed(job, JobState::AwaitingAnswer),
                "질문 바로 앞에 답 기다림 상태가 온다"
            );
            error.clone()
        }
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
        !events[start..].iter().any(|e| matches!(
            e,
            JobEvent::ItemError { job: id, .. } | JobEvent::Conflict { job: id, .. } if *id == job
        )),
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

fn names(folder: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn icacls(path: &Path, args: &[&str]) {
    let out = Command::new("icacls")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "icacls {} {args:?} 실패: {}",
        path.display(),
        String::from_utf8_lossy(&out.stdout)
    );
}

/// 항목 하나를 복사하지 못하게 막는 방법.
#[derive(Clone, Copy, Debug)]
enum Block {
    /// 다른 프로그램이 공유 없이 열어 둔 파일(잠김).
    Lock,
    /// 모두(Everyone)에게 내용 읽기(폴더면 목록 읽기)를 거부한 항목(접근 거부).
    Deny,
}

impl Block {
    fn kind(self) -> FileOpErrorKind {
        match self {
            Block::Lock => FileOpErrorKind::Locked,
            Block::Deny => FileOpErrorKind::AccessDenied,
        }
    }

    fn apply(self, path: &Path) -> Blocked {
        match self {
            Block::Lock => Blocked::Lock(Some(
                OpenOptions::new()
                    .read(true)
                    .share_mode(0)
                    .open(path)
                    .unwrap(),
            )),
            Block::Deny => {
                icacls(path, &["/deny", "*S-1-1-0:(RD)"]);
                Blocked::Deny(Some(path.to_path_buf()))
            }
        }
    }
}

/// 막아 둔 항목. 버리거나 [`Blocked::release`]하면 풀린다.
enum Blocked {
    Lock(Option<File>),
    Deny(Option<PathBuf>),
}

impl Blocked {
    fn release(&mut self) {
        match self {
            Blocked::Lock(file) => drop(file.take()),
            Blocked::Deny(path) => {
                if let Some(path) = path.take() {
                    icacls(&path, &["/remove:d", "*S-1-1-0"]);
                }
            }
        }
    }
}

impl Drop for Blocked {
    fn drop(&mut self) {
        self.release();
    }
}

/// 원본 폴더에 「1 앞.txt」, 「2 막힘.txt」, 「3 뒤.txt」를 만들고 대상 폴더를 만든다.
fn three_files(dir: &Path) -> ([PathBuf; 3], PathBuf) {
    let source_dir = dir.join("원본");
    let target = dir.join("대상");
    fs::create_dir_all(&source_dir).unwrap();
    fs::create_dir_all(&target).unwrap();
    let files = ["1 앞.txt", "2 막힘.txt", "3 뒤.txt"].map(|name| {
        let path = source_dir.join(name);
        fs::write(&path, name).unwrap();
        path
    });
    (files, target)
}

/// 막힌 파일에서 질문이 오고, 막힘을 푼 뒤 다시 시도하면 복사된다.
fn retry_copies_after_release(block: Block) {
    let dir = TempDir::new().unwrap();
    let (files, target) = three_files(dir.path());
    let mut blocked = block.apply(&files[1]);
    let mut queue = JobQueue::new();
    let id = queue.copy(files.to_vec(), target.clone()).unwrap();
    let mut events = Vec::new();
    let error = wait_error(&mut queue, &mut events, id);
    assert_eq!(error.source, files[1]);
    assert_eq!(error.kind, block.kind());
    assert_eq!(queue.state(id), Some(JobState::AwaitingAnswer));
    // 묻는 동안 만들다 만 파일이 없다.
    assert_eq!(names(&target), ["1 앞.txt"]);
    // 다른 종류의 답·조작은 받지 않는다.
    assert_eq!(
        queue.answer_conflict(id, ConflictAnswer::once(Resolution::Skip)),
        Err(AnswerError::NotAsking)
    );
    assert!(!queue.pause(id));
    assert!(!queue.start_now(id));

    blocked.release();
    queue.answer_error(id, ErrorAnswer::Retry).unwrap();
    assert_eq!(
        queue.answer_error(id, ErrorAnswer::Retry),
        Err(AnswerError::NotAsking),
        "이미 답한 작업"
    );
    let summary = wait_finished(&mut queue, &mut events, id);
    assert!(events.contains(&state_changed(id, JobState::Running)));
    assert!(events.contains(&state_changed(id, JobState::Finished)));
    assert_eq!(names(&target), ["1 앞.txt", "2 막힘.txt", "3 뒤.txt"]);
    assert_eq!(
        fs::read_to_string(target.join("2 막힘.txt")).unwrap(),
        "2 막힘.txt"
    );
    assert_eq!(summary.files_copied, 3, "{summary:?}");
    assert!(summary.failures.is_empty(), "{summary:?}");
    assert_eq!(summary.errors_skipped(), 0);
}

/// 건너뛰기면 그 파일만 빠지고 나머지는 복사되며, 작업은 실패로 끝나고 건너뛴 항목이 결과에 남는다.
fn skip_leaves_out_only_that_file(block: Block) {
    let dir = TempDir::new().unwrap();
    let (files, target) = three_files(dir.path());
    let _blocked = block.apply(&files[1]);
    let mut queue = JobQueue::new();
    let id = queue.copy(files.to_vec(), target.clone()).unwrap();
    let mut events = Vec::new();
    wait_error(&mut queue, &mut events, id);
    queue.answer_error(id, ErrorAnswer::Skip).unwrap();
    let summary = wait_finished(&mut queue, &mut events, id);
    assert!(events.contains(&state_changed(id, JobState::Failed)));
    assert_eq!(names(&target), ["1 앞.txt", "3 뒤.txt"]);
    assert_eq!(summary.files_copied, 2, "{summary:?}");
    assert_eq!(summary.errors_skipped(), 1, "{summary:?}");
    assert_eq!(
        summary.failures,
        [ItemFailure {
            source: files[1].clone(),
            kind: block.kind()
        }]
    );
    assert_eq!(summary.items_done, 3, "건너뛴 항목도 한 것으로 센다");
    assert_eq!(summary.items_remaining, 0);
}

/// 작업 취소면 작업이 취소로 끝나고, 앞서 끝낸 항목은 남고, 막힌 파일과 뒤 파일은 대상에 없다.
fn cancel_ends_the_job(block: Block) {
    let dir = TempDir::new().unwrap();
    let (files, target) = three_files(dir.path());
    let _blocked = block.apply(&files[1]);
    let mut queue = JobQueue::new();
    let id = queue.copy(files.to_vec(), target.clone()).unwrap();
    let mut events = Vec::new();
    wait_error(&mut queue, &mut events, id);
    queue.answer_error(id, ErrorAnswer::CancelJob).unwrap();
    assert_eq!(
        queue.answer_error(id, ErrorAnswer::Skip),
        Err(AnswerError::NotAsking),
        "취소 중인 작업"
    );
    let summary = wait_finished(&mut queue, &mut events, id);
    assert!(events.contains(&state_changed(id, JobState::Cancelled)));
    assert_eq!(names(&target), ["1 앞.txt"]);
    assert_eq!(summary.files_copied, 1, "{summary:?}");
    assert!(summary.failures.is_empty(), "{summary:?}");
    assert!(summary.all_counted, "{summary:?}");
    assert_eq!(summary.items_done, 1, "{summary:?}");
    assert_eq!(summary.items_remaining, 2, "{summary:?}");
    assert!(!queue.has_unfinished());
}

#[test]
fn 잠긴_파일은_질문이_오고_풀린_뒤_다시_시도하면_복사된다() {
    retry_copies_after_release(Block::Lock);
}

#[test]
fn 잠긴_파일을_건너뛰면_그_파일만_빠진다() {
    skip_leaves_out_only_that_file(Block::Lock);
}

#[test]
fn 잠긴_파일에서_작업을_취소하면_취소로_끝난다() {
    cancel_ends_the_job(Block::Lock);
}

#[test]
fn 읽기를_막은_파일은_접근_거부_질문이_오고_풀린_뒤_다시_시도하면_복사된다() {
    retry_copies_after_release(Block::Deny);
}

#[test]
fn 읽기를_막은_파일을_건너뛰면_그_파일만_빠진다() {
    skip_leaves_out_only_that_file(Block::Deny);
}

#[test]
fn 읽기를_막은_파일에서_작업을_취소하면_취소로_끝난다() {
    cancel_ends_the_job(Block::Deny);
}

#[test]
fn 목록을_읽지_못한_폴더는_풀린_뒤_다시_시도하면_다시_훑어_복사한다() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("폴더");
    fs::create_dir_all(source.join("안")).unwrap();
    fs::write(source.join("a.txt"), "a").unwrap();
    fs::write(source.join("안").join("b.txt"), "b").unwrap();
    let target = dir.path().join("대상");
    fs::create_dir_all(&target).unwrap();
    let mut blocked = Block::Deny.apply(&source);
    // 백업 권한이 켜진 프로세스 등은 폴더의 거부 ACE를 무시한다(`file_system_provider.rs`와 같은 판단).
    if fs::read_dir(&source).is_ok() {
        eprintln!("이 환경에서는 폴더 목록 읽기를 막을 수 없어 건너뛴다");
        return;
    }

    let mut queue = JobQueue::new();
    let id = queue.copy(vec![source.clone()], target.clone()).unwrap();
    let mut events = Vec::new();
    let error = wait_error(&mut queue, &mut events, id);
    assert_eq!(error.source, source);
    assert_eq!(error.kind, FileOpErrorKind::AccessDenied);
    blocked.release();
    queue.answer_error(id, ErrorAnswer::Retry).unwrap();
    let summary = wait_finished(&mut queue, &mut events, id);
    assert!(events.contains(&state_changed(id, JobState::Finished)));
    assert_eq!(
        fs::read_to_string(target.join("폴더").join("안").join("b.txt")).unwrap(),
        "b"
    );
    assert_eq!(
        fs::read_to_string(target.join("폴더").join("a.txt")).unwrap(),
        "a"
    );
    assert_eq!(summary.files_copied, 2, "{summary:?}");
    assert_eq!(summary.folders_copied, 2, "{summary:?}");
    // 다시 훑은 항목을 한 번만 센다: 폴더 둘과 파일 둘.
    assert_eq!(summary.items_done, 4, "{summary:?}");
    assert_eq!(summary.items_remaining, 0, "{summary:?}");
}

#[test]
fn 나머지_오류도_건너뛰기_뒤에는_같은_작업의_다른_오류를_묻지_않는다() {
    let dir = TempDir::new().unwrap();
    let (files, target) = three_files(dir.path());
    let _first = Block::Lock.apply(&files[0]);
    let _last = Block::Deny.apply(&files[2]);
    let mut queue = JobQueue::new();
    let id = queue.copy(files.to_vec(), target.clone()).unwrap();
    let mut events = Vec::new();
    let error = wait_error(&mut queue, &mut events, id);
    assert_eq!(error.source, files[0]);
    queue.answer_error(id, ErrorAnswer::SkipAll).unwrap();
    // `wait_finished`가 그 뒤 질문이 오지 않았음을 확인한다.
    let summary = wait_finished(&mut queue, &mut events, id);
    assert!(events.contains(&state_changed(id, JobState::Failed)));
    assert_eq!(names(&target), ["2 막힘.txt"]);
    assert_eq!(summary.errors_skipped(), 2, "{summary:?}");
    assert_eq!(
        summary.failures,
        [
            ItemFailure {
                source: files[0].clone(),
                kind: FileOpErrorKind::Locked
            },
            ItemFailure {
                source: files[2].clone(),
                kind: FileOpErrorKind::AccessDenied
            },
        ]
    );

    // 다른 작업은 다시 묻는다(「나머지」는 그 작업에만).
    let other_target = dir.path().join("다른 대상");
    fs::create_dir_all(&other_target).unwrap();
    let other = queue.copy(vec![files[0].clone()], other_target).unwrap();
    wait_error(&mut queue, &mut events, other);
    queue.answer_error(other, ErrorAnswer::Skip).unwrap();
    wait_finished(&mut queue, &mut events, other);
}

#[test]
fn 훑은_뒤_복사_전에_사라진_원본은_원본_없음_질문으로_온다() {
    let dir = TempDir::new().unwrap();
    let (files, target) = three_files(dir.path());
    // 첫 파일을 잠가 작업을 질문에 세워 두고, 그동안 훑기가 원본을 모두 센 뒤 셋째 파일을 지운다.
    let mut blocked = Block::Lock.apply(&files[0]);
    let mut queue = JobQueue::new();
    let id = queue.copy(files.to_vec(), target.clone()).unwrap();
    let mut events = Vec::new();
    wait_error(&mut queue, &mut events, id);
    // 훑기는 답을 기다리는 동안에도 돈다. 원본 셋을 보는 데는 순간이면 되지만 넉넉히 기다린다.
    thread::sleep(Duration::from_millis(500));
    fs::remove_file(&files[2]).unwrap();
    blocked.release();
    queue.answer_error(id, ErrorAnswer::Retry).unwrap();

    let error = wait_error(&mut queue, &mut events, id);
    assert_eq!(error.source, files[2]);
    assert_eq!(error.kind, FileOpErrorKind::SourceMissing);
    queue.answer_error(id, ErrorAnswer::Skip).unwrap();
    let summary = wait_finished(&mut queue, &mut events, id);
    assert!(summary.all_counted);
    assert_eq!(names(&target), ["1 앞.txt", "2 막힘.txt"]);
    assert_eq!(
        summary.failures,
        [ItemFailure {
            source: files[2].clone(),
            kind: FileOpErrorKind::SourceMissing
        }]
    );
}

#[test]
fn 처음부터_없는_원본도_원본_없음_질문으로_온다() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("대상");
    fs::create_dir_all(&target).unwrap();
    let present = dir.path().join("있음.txt");
    fs::write(&present, "x").unwrap();
    let missing = dir.path().join("없음.txt");

    let mut queue = JobQueue::new();
    let id = queue
        .copy(vec![missing.clone(), present.clone()], target.clone())
        .unwrap();
    let mut events = Vec::new();
    let error = wait_error(&mut queue, &mut events, id);
    assert_eq!(error.source, missing);
    assert_eq!(error.kind, FileOpErrorKind::SourceMissing);
    queue.answer_error(id, ErrorAnswer::Skip).unwrap();
    let summary = wait_finished(&mut queue, &mut events, id);
    assert_eq!(summary.files_copied, 1);
    assert_eq!(summary.errors_skipped(), 1);
    assert!(target.join("있음.txt").is_file());
    assert!(events.contains(&state_changed(id, JobState::Failed)));
}

#[test]
fn 훑을_때_없던_원본이_생긴_뒤_다시_시도하면_다시_훑어_복사한다() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("대상");
    fs::create_dir_all(&target).unwrap();
    let source = dir.path().join("나중 폴더");

    let mut queue = JobQueue::new();
    let id = queue.copy(vec![source.clone()], target.clone()).unwrap();
    let mut events = Vec::new();
    let error = wait_error(&mut queue, &mut events, id);
    assert_eq!(error.source, source);
    assert_eq!(error.kind, FileOpErrorKind::SourceMissing);

    fs::create_dir_all(source.join("안")).unwrap();
    fs::write(source.join("a.txt"), "a").unwrap();
    fs::write(source.join("안").join("b.txt"), "b").unwrap();
    queue.answer_error(id, ErrorAnswer::Retry).unwrap();
    let summary = wait_finished(&mut queue, &mut events, id);
    assert!(events.contains(&state_changed(id, JobState::Finished)));
    assert_eq!(
        fs::read_to_string(target.join("나중 폴더").join("안").join("b.txt")).unwrap(),
        "b"
    );
    assert_eq!(
        fs::read_to_string(target.join("나중 폴더").join("a.txt")).unwrap(),
        "a"
    );
    assert_eq!(summary.files_copied, 2, "{summary:?}");
    assert_eq!(summary.folders_copied, 2, "{summary:?}");
    // 다시 훑은 항목을 한 번만 센다: 폴더 둘과 파일 둘.
    assert_eq!(summary.items_done, 4, "{summary:?}");
    assert_eq!(summary.items_remaining, 0, "{summary:?}");
    assert!(summary.failures.is_empty(), "{summary:?}");
}

#[test]
fn 오류_답을_기다리는_동안_대기열의_다른_작업이_끝난다() {
    let dir = TempDir::new().unwrap();
    let (files, target) = three_files(dir.path());
    let mut blocked = Block::Lock.apply(&files[1]);
    let other_source = dir.path().join("다른.txt");
    fs::write(&other_source, "다른 작업").unwrap();
    let other_target = dir.path().join("다른 대상");
    fs::create_dir_all(&other_target).unwrap();

    let mut queue = JobQueue::new();
    let asking = queue.copy(files.to_vec(), target.clone()).unwrap();
    let other = queue
        .copy(vec![other_source.clone()], other_target.clone())
        .unwrap();
    let mut events = Vec::new();
    wait_error(&mut queue, &mut events, asking);
    // 답하지 않은 채 뒤 작업이 차례를 받아 끝난다.
    let summary = wait_finished(&mut queue, &mut events, other);
    assert_eq!(summary.files_copied, 1);
    assert_eq!(
        fs::read_to_string(other_target.join("다른.txt")).unwrap(),
        "다른 작업"
    );
    assert_eq!(queue.state(asking), Some(JobState::AwaitingAnswer));

    // 답하면 이어서 끝난다.
    blocked.release();
    queue.answer_error(asking, ErrorAnswer::Retry).unwrap();
    let summary = wait_finished(&mut queue, &mut events, asking);
    assert_eq!(summary.files_copied, 3);
    assert!(!queue.has_unfinished());
}

#[test]
fn 오류_답을_기다리는_작업도_취소한다() {
    let dir = TempDir::new().unwrap();
    let (files, target) = three_files(dir.path());
    let _blocked = Block::Lock.apply(&files[1]);
    let mut queue = JobQueue::new();
    let id = queue.copy(files.to_vec(), target.clone()).unwrap();
    let mut events = Vec::new();
    wait_error(&mut queue, &mut events, id);
    assert!(queue.cancel(id));
    assert_eq!(
        queue.answer_error(id, ErrorAnswer::Retry),
        Err(AnswerError::NotAsking)
    );
    let summary = wait_finished(&mut queue, &mut events, id);
    assert!(events.contains(&state_changed(id, JobState::Cancelled)));
    assert_eq!(names(&target), ["1 앞.txt"]);
    assert_eq!(summary.items_remaining, 2);
}

#[test]
fn 충돌을_기다리는_작업에는_오류_답을_할_수_없다() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("a.txt");
    fs::write(&source, "새").unwrap();
    let target = dir.path().join("대상");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("a.txt"), "원래").unwrap();
    let mut queue = JobQueue::new();
    let id = queue.copy(vec![source], target).unwrap();
    let mut events = Vec::new();
    wait_for(
        &mut queue,
        &mut events,
        |e| matches!(e, JobEvent::Conflict { job, .. } if *job == id),
    );
    assert_eq!(
        queue.answer_error(id, ErrorAnswer::Skip),
        Err(AnswerError::NotAsking)
    );
    queue
        .answer_conflict(id, ConflictAnswer::once(Resolution::Skip))
        .unwrap();
    wait_finished(&mut queue, &mut events, id);
    assert_eq!(
        queue.answer_error(id, ErrorAnswer::Skip),
        Err(AnswerError::NotAsking),
        "끝난 작업"
    );
}

const VOLUME_VAR: &str = "FACET_TEST_VOLUME";

/// 작은 시험 볼륨을 채우는 파일. 버리면 지운다.
struct Filler(PathBuf);

impl Filler {
    /// `dir`에 남은 공간이 1MiB 아래가 될 때까지 큰 파일을 만든다(내용은 쓰지 않고 크기만 잡는다).
    fn fill(dir: &Path) -> Filler {
        const MIB: u64 = 1 << 20;
        let path = dir.join("채우기.bin");
        let file = File::create(&path).unwrap();
        let filler = Filler(path);
        // 크게 잡아 보고 안 되면 줄인다. 시험 볼륨은 64MB다.
        let mut step = 64 * MIB;
        let mut size = 0;
        while step >= MIB / 4 {
            if file.set_len(size + step).is_ok() {
                size += step;
            } else {
                step /= 2;
            }
        }
        assert!(size > 0, "시험 볼륨을 채우지 못했다");
        filler
    }
}

impl Drop for Filler {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[test]
#[ignore = "실제 볼륨 테스트: 관리자 권한과 FACET_TEST_VOLUME이 필요하다(cargo xtask ci가 관리자일 때 돌린다)"]
fn real_volume_disk_full_retry_continues_from_that_item() {
    let volume = env::var(VOLUME_VAR).unwrap_or_else(|_| {
        panic!("실제 볼륨 테스트에는 시험 NTFS 볼륨의 드라이브 문자를 담은 FACET_TEST_VOLUME이 필요합니다")
    });
    let root = PathBuf::from(format!(r"{volume}\"));
    let work = root.join(format!("facet-disk-full-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).unwrap();
    let target = work.join("대상");
    fs::create_dir_all(&target).unwrap();

    // 원본은 임시 폴더(시험 볼륨 밖)에 둔다. 작은 파일 둘 사이에 남은 공간보다 큰 파일.
    let dir = TempDir::new().unwrap();
    let small_first = dir.path().join("1 작음.txt");
    let big = dir.path().join("2 큼.bin");
    let small_last = dir.path().join("3 작음.txt");
    fs::write(&small_first, "앞").unwrap();
    fs::write(&big, vec![0x5a; 4 << 20]).unwrap();
    fs::write(&small_last, "뒤").unwrap();

    let filler = Filler::fill(&work);
    let mut queue = JobQueue::new();
    let id = queue
        .copy(
            vec![small_first.clone(), big.clone(), small_last.clone()],
            target.clone(),
        )
        .unwrap();
    let mut events = Vec::new();
    let error = wait_error(&mut queue, &mut events, id);
    assert_eq!(error.source, big);
    assert_eq!(error.kind, FileOpErrorKind::DiskFull);
    // 만들다 만 파일은 질문 전에 지웠다. 앞 파일은 이미 복사됐다.
    assert_eq!(names(&target), ["1 작음.txt"]);
    // 다시 복사하면 이름 충돌로 묻게 되므로, 앞 파일을 바꿔 두고 다시 복사하지 않는지 본다.
    fs::write(target.join("1 작음.txt"), "바꿈").unwrap();

    drop(filler);
    queue.answer_error(id, ErrorAnswer::Retry).unwrap();
    // `wait_finished`가 그 뒤 질문(이름 충돌 포함)이 오지 않았음을 확인한다.
    let summary = wait_finished(&mut queue, &mut events, id);
    assert!(events.contains(&state_changed(id, JobState::Finished)));
    assert_eq!(
        fs::read_to_string(target.join("1 작음.txt")).unwrap(),
        "바꿈",
        "이미 복사한 항목을 다시 복사했다"
    );
    assert_eq!(
        fs::read(target.join("2 큼.bin")).unwrap(),
        fs::read(&big).unwrap()
    );
    assert_eq!(fs::read_to_string(target.join("3 작음.txt")).unwrap(), "뒤");
    assert_eq!(summary.files_copied, 3, "{summary:?}");
    assert!(summary.failures.is_empty(), "{summary:?}");

    drop(queue);
    let _ = fs::remove_dir_all(&work);
}
