//! **영구 삭제** 작업을 임시 폴더의 실제 파일로 확인한다: 중첩 폴더·읽기 전용·260자를 넘는 경로, 잠긴 파일의
//! 항목별 오류 질문과 답(다시 시도·건너뛰기·나머지 오류도 건너뛰기·작업 취소), 링크만 지우기(정션, 만들 수 있으면
//! 폴더 심볼릭 링크), 차례를 기다리지 않는 곧바로 시작, 작업 기록(되돌릴 수 없는 작업).
//!
//! 파일 하나·폴더 하나 지우기는 facet-win에 맡기므로 Windows 전용이다. 정션은 관리자·개발자 모드 없이 만들 수
//! 있어 늘 돌고, 폴더 심볼릭 링크는 만들 수 있는 환경에서만 확인한다.

#![cfg(windows)]

use std::fs::{self, File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use facet_core::job::{
    AnswerError, DeleteRejection, ErrorAnswer, FileOpErrorKind, ItemFailure, JobEvent, JobId,
    JobQueue, JobState, Progress, Summary,
};
use tempfile::TempDir;

/// 사건을 기다리는 넉넉한 한도.
const TIMEOUT: Duration = Duration::from_secs(120);

fn state_changed(job: JobId, state: JobState) -> JobEvent {
    JobEvent::StateChanged { job, state }
}

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

/// `job`이 끝날 때까지 기다려 마지막 상태와 결과를 돌려준다. 그 사이 그 작업의 질문이 오면 실패한다.
fn wait_finished(
    queue: &mut JobQueue,
    events: &mut Vec<JobEvent>,
    job: JobId,
) -> (JobState, Summary) {
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
    let n = events.len();
    match (&events[n - 2], &events[n - 1]) {
        (JobEvent::StateChanged { state, .. }, JobEvent::Finished { summary, .. }) => {
            (*state, summary.clone())
        }
        other => panic!("끝남 바로 앞에 마지막 상태가 온다: {other:?}"),
    }
}

/// 영구 삭제를 넣고 끝날 때까지 기다린다(질문이 없어야 한다).
fn delete(queue: &mut JobQueue, sources: Vec<PathBuf>) -> (JobState, Summary, Vec<JobEvent>) {
    let job = queue.delete_permanently(sources).unwrap();
    let mut events = Vec::new();
    let (state, summary) = wait_finished(queue, &mut events, job);
    (state, summary, events)
}

fn last_progress(events: &[JobEvent]) -> Progress {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            JobEvent::Progress { progress, .. } => Some(progress.clone()),
            _ => None,
        })
        .expect("진행 사건")
}

/// 다른 프로그램이 공유 없이 연 것처럼 파일을 잠근다. 돌려준 것을 버리면 풀린다.
fn lock(path: &Path) -> File {
    OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .unwrap()
}

fn set_readonly(path: &Path) {
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions).unwrap();
}

/// `mklink`로 링크를 만든다(`/J` 정션, `/D` 폴더 심볼릭 링크). 성공했는지 돌려준다.
fn mklink(flag: &str, link: &Path, target: &Path) -> bool {
    Command::new("cmd")
        .arg("/c")
        .arg("mklink")
        .arg(flag)
        .arg(link)
        .arg(target)
        .output()
        .is_ok_and(|out| out.status.success())
}

/// 폴더 안 항목 이름(이름 순).
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn 원본이_없으면_작업을_만들지_않는다() {
    let mut queue = JobQueue::new();
    assert_eq!(
        queue.delete_permanently(Vec::new()),
        Err(DeleteRejection::NoSources)
    );
    assert!(queue.poll().is_empty(), "거부된 작업은 사건이 없다");
    assert!(!queue.has_unfinished());
}

#[test]
fn 중첩_폴더를_영구_삭제하면_모두_사라지고_읽기_전용도_지워진다() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("지울 폴더 🗑");
    let deep = root.join("안").join("더 안");
    fs::create_dir_all(&deep).unwrap();
    fs::create_dir_all(root.join("빈 폴더")).unwrap();
    fs::write(root.join("a.txt"), "a").unwrap();
    fs::write(root.join("안").join("b.txt"), "bb").unwrap();
    fs::write(deep.join("c.txt"), "ccc").unwrap();
    fs::write(deep.join("읽기 전용.txt"), "읽기").unwrap();
    set_readonly(&deep.join("읽기 전용.txt"));
    let readonly_folder = root.join("읽기 전용 폴더");
    fs::create_dir_all(&readonly_folder).unwrap();
    fs::write(readonly_folder.join("d.txt"), "d").unwrap();
    set_readonly(&readonly_folder);
    let single = dir.path().join("파일 하나.txt");
    fs::write(&single, "하나").unwrap();
    set_readonly(&single);
    let kept = dir.path().join("남는 파일.txt");
    fs::write(&kept, "남음").unwrap();

    let mut queue = JobQueue::new();
    let (state, summary, events) = delete(&mut queue, vec![root.clone(), single.clone()]);

    assert_eq!(state, JobState::Finished);
    assert!(!root.exists());
    assert!(!single.exists());
    assert_eq!(names(dir.path()), ["남는 파일.txt"]);
    // 폴더 5(지울 폴더·안·더 안·빈 폴더·읽기 전용 폴더) + 파일 6.
    assert_eq!(summary.items_deleted, 11);
    assert_eq!(summary.items_done, 11);
    assert_eq!(summary.items_remaining, 0);
    assert!(summary.all_counted);
    assert!(summary.failures.is_empty());
    assert_eq!(summary.items_completed(), 11);

    // 사건: 대기 → (차례를 기다리지 않고) 실행 → 진행 → 끝남.
    let job = match &events[0] {
        JobEvent::StateChanged { job, state } => {
            assert_eq!(*state, JobState::Queued);
            *job
        }
        other => panic!("첫 사건은 대기다: {other:?}"),
    };
    assert_eq!(events[1], state_changed(job, JobState::Running));
    let progress = last_progress(&events);
    assert!(progress.counted);
    assert_eq!(progress.items_done, 11);
    assert_eq!(progress.items_total, 11);
    assert_eq!(progress.bytes_done, progress.bytes_total);
    assert!(!queue.has_unfinished());
}

#[test]
fn 경로가_260자를_넘어도_지운다() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("긴 경로");
    let mut deep = root.clone();
    for i in 0..8 {
        deep = deep.join(format!("{i}-{}", "긴".repeat(40)));
    }
    fs::create_dir_all(&deep).unwrap();
    let file = deep.join(format!("{}.txt", "파일".repeat(30)));
    fs::write(&file, "긴 경로의 파일").unwrap();
    set_readonly(&file);
    assert!(
        file.as_os_str().len() > 400,
        "시험 경로가 260자를 넘어야 한다"
    );

    let mut queue = JobQueue::new();
    let (state, summary, _) = delete(&mut queue, vec![root.clone()]);
    assert_eq!(state, JobState::Finished);
    assert!(!root.exists());
    assert_eq!(summary.items_deleted, 10);

    // 긴 경로의 파일 하나를 바로 넘겨도 지운다.
    fs::create_dir_all(&deep).unwrap();
    fs::write(&file, "다시").unwrap();
    let (state, summary, _) = delete(&mut queue, vec![file.clone()]);
    assert_eq!(state, JobState::Finished);
    assert!(!file.exists());
    assert!(deep.exists());
    assert_eq!(summary.items_deleted, 1);
}

#[test]
fn 없는_원본은_이미_지워진_것으로_보고_묻지_않는다() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("a.txt");
    fs::write(&file, "a").unwrap();
    let mut queue = JobQueue::new();
    let (state, summary, _) = delete(&mut queue, vec![dir.path().join("없음.txt"), file.clone()]);
    assert_eq!(state, JobState::Finished);
    assert!(!file.exists());
    assert_eq!(summary.items_deleted, 1);
    assert_eq!(summary.items_done, 2);
}

/// 잠긴 파일이 든 폴더: `폴더/잠긴 곳/잠김.txt`와 그 옆의 다른 항목들.
fn locked_tree(dir: &Path) -> (PathBuf, PathBuf) {
    let root = dir.join("폴더");
    let inner = root.join("잠긴 곳");
    fs::create_dir_all(&inner).unwrap();
    fs::create_dir_all(root.join("다른 곳")).unwrap();
    fs::write(root.join("a.txt"), "a").unwrap();
    fs::write(root.join("다른 곳").join("b.txt"), "b").unwrap();
    fs::write(inner.join("옆.txt"), "옆").unwrap();
    let locked = inner.join("잠김.txt");
    fs::write(&locked, "잠김").unwrap();
    (root, locked)
}

#[test]
fn 잠긴_파일은_묻고_잠금을_푼_뒤_다시_시도하면_지운다() {
    let dir = TempDir::new().unwrap();
    let (root, locked) = locked_tree(dir.path());
    let handle = lock(&locked);

    let mut queue = JobQueue::new();
    let job = queue.delete_permanently(vec![root.clone()]).unwrap();
    let mut events = Vec::new();
    let error = wait_error(&mut queue, &mut events, job);
    assert_eq!(
        error,
        ItemFailure {
            source: locked.clone(),
            kind: FileOpErrorKind::Locked,
        }
    );
    assert_eq!(queue.state(job), Some(JobState::AwaitingAnswer));
    assert!(locked.exists());

    // 잠금을 풀지 않고 다시 시도하면 다시 묻는다.
    queue.answer_error(job, ErrorAnswer::Retry).unwrap();
    let again = wait_error(&mut queue, &mut events, job);
    assert_eq!(again, error);

    drop(handle);
    queue.answer_error(job, ErrorAnswer::Retry).unwrap();
    let (state, summary) = wait_finished(&mut queue, &mut events, job);
    assert_eq!(state, JobState::Finished);
    assert!(!root.exists());
    assert!(summary.failures.is_empty());
    assert_eq!(summary.items_deleted, 7);
    assert_eq!(
        queue.answer_error(job, ErrorAnswer::Retry),
        Err(AnswerError::NotAsking)
    );
}

#[test]
fn 잠긴_파일을_건너뛰면_그_파일과_상위_폴더만_남는다() {
    let dir = TempDir::new().unwrap();
    let (root, locked) = locked_tree(dir.path());
    let handle = lock(&locked);

    let mut queue = JobQueue::new();
    let job = queue.delete_permanently(vec![root.clone()]).unwrap();
    let mut events = Vec::new();
    let error = wait_error(&mut queue, &mut events, job);
    assert_eq!(error.source, locked);
    queue.answer_error(job, ErrorAnswer::Skip).unwrap();
    let (state, summary) = wait_finished(&mut queue, &mut events, job);

    // 상위 폴더가 비지 않아 남는 것은 오류로 다시 묻지 않는다.
    assert_eq!(state, JobState::Failed);
    assert_eq!(summary.failures, vec![error]);
    assert_eq!(summary.items_deleted, 4);
    assert_eq!(summary.items_done, 7);
    assert_eq!(summary.items_remaining, 0);
    assert_eq!(names(&root), ["잠긴 곳"]);
    assert_eq!(names(&root.join("잠긴 곳")), ["잠김.txt"]);
    drop(handle);
    assert_eq!(fs::read_to_string(&locked).unwrap(), "잠김");
}

#[test]
fn 나머지_오류도_건너뛰기는_한_번만_묻는다() {
    let dir = TempDir::new().unwrap();
    let files: Vec<PathBuf> = ["가.txt", "나.txt", "다.txt"]
        .iter()
        .map(|name| {
            let path = dir.path().join(name);
            fs::write(&path, name).unwrap();
            path
        })
        .collect();
    let _first = lock(&files[0]);
    let _second = lock(&files[1]);

    let mut queue = JobQueue::new();
    let job = queue.delete_permanently(files.clone()).unwrap();
    let mut events = Vec::new();
    let error = wait_error(&mut queue, &mut events, job);
    assert_eq!(error.source, files[0]);
    queue.answer_error(job, ErrorAnswer::SkipAll).unwrap();
    let (state, summary) = wait_finished(&mut queue, &mut events, job);
    assert_eq!(state, JobState::Failed);
    assert_eq!(summary.errors_skipped(), 2);
    assert_eq!(summary.items_deleted, 1);
    assert!(files[0].exists());
    assert!(files[1].exists());
    assert!(!files[2].exists());
}

#[test]
fn 잠긴_파일에서_작업을_취소하면_이미_지운_것만_지워져_있다() {
    let dir = TempDir::new().unwrap();
    let files: Vec<PathBuf> = ["가.txt", "나.txt", "다.txt"]
        .iter()
        .map(|name| {
            let path = dir.path().join(name);
            fs::write(&path, name).unwrap();
            path
        })
        .collect();
    let folder = dir.path().join("라");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("안.txt"), "안").unwrap();
    let _handle = lock(&files[1]);

    // 넘긴 차례대로 지운다: 가 → 나(잠김, 여기서 취소) → 다 → 라.
    let mut sources = files.clone();
    sources.push(folder.clone());
    let mut queue = JobQueue::new();
    let job = queue.delete_permanently(sources).unwrap();
    let mut events = Vec::new();
    let error = wait_error(&mut queue, &mut events, job);
    assert_eq!(error.source, files[1]);
    queue.answer_error(job, ErrorAnswer::CancelJob).unwrap();
    let (state, summary) = wait_finished(&mut queue, &mut events, job);

    assert_eq!(state, JobState::Cancelled);
    assert_eq!(summary.items_deleted, 1);
    assert!(summary.failures.is_empty());
    assert!(!files[0].exists(), "취소 전에 지운 것은 지워져 있다");
    assert!(files[1].exists());
    assert!(files[2].exists(), "취소 뒤의 항목은 남는다");
    assert_eq!(names(&folder), ["안.txt"]);
    assert!(!queue.has_unfinished());
}

#[test]
fn 답을_기다리는_삭제를_대기열에서_취소해도_남은_것은_그대로다() {
    let dir = TempDir::new().unwrap();
    let (root, locked) = locked_tree(dir.path());
    let _handle = lock(&locked);
    let other = dir.path().join("뒤.txt");
    fs::write(&other, "뒤").unwrap();

    let mut queue = JobQueue::new();
    let job = queue
        .delete_permanently(vec![root.clone(), other.clone()])
        .unwrap();
    let mut events = Vec::new();
    wait_error(&mut queue, &mut events, job);
    assert!(queue.cancel(job));
    let (state, _) = wait_finished(&mut queue, &mut events, job);
    assert_eq!(state, JobState::Cancelled);
    assert!(locked.exists());
    assert!(other.exists());
}

#[test]
fn 정션이_든_폴더를_지우면_링크만_지워지고_가리키던_폴더는_그대로다() {
    let dir = TempDir::new().unwrap();
    let outside = dir.path().join("바깥");
    fs::create_dir_all(outside.join("하위")).unwrap();
    fs::write(outside.join("남는 파일.txt"), "남음").unwrap();
    fs::write(outside.join("하위").join("깊은 파일.txt"), "깊음").unwrap();
    let root = dir.path().join("지울 폴더");
    fs::create_dir_all(root.join("안")).unwrap();
    fs::write(root.join("a.txt"), "a").unwrap();
    assert!(mklink("/J", &root.join("안").join("정션 🔗"), &outside));
    // 자기 상위를 가리키는 정션이 있어도 끝난다.
    assert!(mklink("/J", &root.join("안").join("위로"), &root));

    let mut queue = JobQueue::new();
    let (state, summary, _) = delete(&mut queue, vec![root.clone()]);
    assert_eq!(state, JobState::Finished);
    assert!(!root.exists());
    // 폴더 2 + 파일 1 + 정션 2. 정션 안은 세지도 지우지도 않는다.
    assert_eq!(summary.items_deleted, 5);
    assert_eq!(names(&outside), ["남는 파일.txt", "하위"]);
    assert_eq!(
        fs::read_to_string(outside.join("하위").join("깊은 파일.txt")).unwrap(),
        "깊음"
    );

    // 정션 자체를 넘겨도 링크만 지운다.
    let link = dir.path().join("정션");
    assert!(mklink("/J", &link, &outside));
    let (state, summary, _) = delete(&mut queue, vec![link.clone()]);
    assert_eq!(state, JobState::Finished);
    assert!(fs::symlink_metadata(&link).is_err());
    assert_eq!(summary.items_deleted, 1);
    assert_eq!(names(&outside), ["남는 파일.txt", "하위"]);
}

#[test]
fn 폴더_심볼릭_링크가_든_폴더를_지우면_링크만_지워진다() {
    let dir = TempDir::new().unwrap();
    let outside = dir.path().join("바깥");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("남는 파일.txt"), "남음").unwrap();
    let root = dir.path().join("지울 폴더");
    fs::create_dir_all(&root).unwrap();
    if !mklink("/D", &root.join("링크"), &outside) {
        // 관리자도 개발자 모드도 아니면 심볼릭 링크를 만들 수 없다. 정션 테스트가 같은 길을 확인한다.
        eprintln!("심볼릭 링크를 만들 수 없어 건너뛴다");
        return;
    }
    let mut queue = JobQueue::new();
    let (state, summary, _) = delete(&mut queue, vec![root.clone()]);
    assert_eq!(state, JobState::Finished);
    assert!(!root.exists());
    assert_eq!(summary.items_deleted, 2);
    assert_eq!(names(&outside), ["남는 파일.txt"]);
}

#[test]
fn 앞선_작업이_돌고_있어도_삭제는_곧바로_시작한다() {
    let dir = TempDir::new().unwrap();
    // 앞 작업: 큰 파일의 복사. 시작하자마자 멈춰 차례를 쥔 채 세워 둔다.
    let big = dir.path().join("큰 파일.bin");
    {
        use std::io::Write;
        let mut file = File::create(&big).unwrap();
        let chunk = vec![0x5a_u8; 1024 * 1024];
        for _ in 0..256 {
            file.write_all(&chunk).unwrap();
        }
    }
    let target = dir.path().join("대상");
    fs::create_dir_all(&target).unwrap();
    let small = dir.path().join("작은 파일.txt");
    fs::write(&small, "작은").unwrap();
    let doomed = dir.path().join("지울 폴더");
    fs::create_dir_all(&doomed).unwrap();
    fs::write(doomed.join("a.txt"), "a").unwrap();

    let mut queue = JobQueue::new();
    let first = queue.copy(vec![big.clone()], target.clone()).unwrap();
    let mut events = Vec::new();
    wait_for(&mut queue, &mut events, |e| {
        *e == state_changed(first, JobState::Running)
    });
    // 차례를 쥔 채 멈춘다: 차례를 기다리는 작업은 시작하지 못한다.
    assert!(queue.pause(first), "앞 작업이 이미 끝났다: {events:?}");
    let waiting = queue.copy(vec![small.clone()], target.clone()).unwrap();

    let delete = queue.delete_permanently(vec![doomed.clone()]).unwrap();
    assert_ne!(
        queue.state(delete),
        Some(JobState::Queued),
        "삭제는 넣자마자 시작한다"
    );
    let (state, summary) = wait_finished(&mut queue, &mut events, delete);
    assert_eq!(state, JobState::Finished);
    assert_eq!(summary.items_deleted, 2);
    assert!(!doomed.exists());
    // 그동안 앞 작업은 멈춰 있고 차례를 기다리는 작업은 시작하지 않았다.
    assert_eq!(queue.state(first), Some(JobState::Paused));
    assert_eq!(queue.state(waiting), Some(JobState::Queued));
    assert!(!events.contains(&state_changed(first, JobState::Finished)));
    assert!(!events.contains(&state_changed(waiting, JobState::Running)));

    assert!(queue.cancel(first));
    wait_for(
        &mut queue,
        &mut events,
        |e| matches!(e, JobEvent::Finished { job, .. } if *job == waiting),
    );
    assert!(target.join("작은 파일.txt").exists());
}

mod app {
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use facet_core::Location;
    use facet_core::command::CommandId;
    use facet_core::history::{NotUndoable, OperationKind, UndoError};
    use facet_core::job::{DeleteRejection, JobId, JobKind, JobState, Summary};
    use facet_core::tab::ReadOutcome;
    use facet_core::window::{App, Execution, Update};
    use tempfile::TempDir;

    const WAIT: Duration = Duration::from_secs(60);

    fn app_at(dir: &Path) -> App {
        let mut app = App::new(Some(Location::file_system(dir)));
        let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
        assert_eq!(outcome, ReadOutcome::Committed);
        app
    }

    fn rename(app: &mut App, from: &str, to: &str) {
        app.rename(from.as_ref(), to)
            .expect("이름바꾸기를 시작한다");
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "이름바꾸기 결과가 오지 않았다");
            if let Some((_, Update::Edited(result))) = app.wait_update(left) {
                result.expect("이름바꾸기가 성공한다");
                return;
            }
        }
    }

    fn wait_job(app: &mut App, job: JobId) -> (JobState, Summary) {
        let deadline = Instant::now() + WAIT;
        loop {
            app.poll_jobs();
            app.poll();
            if let Some(entry) = app.work_area().entry(job)
                && let Some(summary) = &entry.summary
            {
                return (entry.state, summary.clone());
            }
            assert!(Instant::now() < deadline, "작업이 시간 안에 끝나지 않았다");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn 영구_삭제_뒤_실행취소는_까닭을_돌려주고_그_앞_이름바꾸기는_되돌아가지_않는다() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.txt"), "a").unwrap();
        let doomed = dir.path().join("지울 폴더");
        fs::create_dir_all(doomed.join("안")).unwrap();
        fs::write(doomed.join("안").join("x.txt"), "x").unwrap();
        fs::write(dir.path().join("지울 파일.txt"), "y").unwrap();
        let mut app = app_at(dir.path());
        rename(&mut app, "a.txt", "b.txt");

        assert_eq!(
            app.delete_permanently(Vec::new()),
            Err(DeleteRejection::NoSources)
        );
        let job = app
            .delete_permanently(vec![doomed.clone(), dir.path().join("지울 파일.txt")])
            .expect("영구 삭제를 시작한다");
        // 작업 영역이 삭제 종류와 지우는 위치를 담는다.
        let entry = app.work_area().entry(job).expect("작업 영역에 있다");
        assert_eq!(entry.kind, JobKind::Delete);
        assert_eq!(entry.target, dir.path());
        assert_eq!(entry.sources, 2);
        let (state, summary) = wait_job(&mut app, job);
        assert_eq!(state, JobState::Finished);
        assert_eq!(summary.items_deleted, 4);
        assert!(!doomed.exists());

        // 되돌릴 수 없는 작업(까닭: 영구 삭제)으로 남는다.
        let record = app.history().undo_target().expect("기록이 남는다");
        assert_eq!(record.kind(), OperationKind::Delete);
        assert!(!record.is_undoable());
        assert_eq!(
            record.not_undoable_reason(),
            Some(NotUndoable::PermanentDelete)
        );
        assert_eq!(record.items_completed(), 4);
        assert_eq!(record.items_skipped(), 0);
        let tops: Vec<_> = record.top_steps().iter().map(|s| s.path()).collect();
        assert_eq!(
            tops,
            [doomed.as_path(), &dir.path().join("지울 파일.txt")],
            "맨 위 항목은 넘긴 원본이다"
        );
        assert_eq!(app.history().records().len(), 2);

        // 실행취소는 까닭을 돌려주고 그 앞 이름바꾸기로 넘어가지 않는다.
        let refused = UndoError::NotUndoable(NotUndoable::PermanentDelete);
        assert!(app.can_execute(CommandId::Undo));
        assert_eq!(
            app.execute(CommandId::Undo),
            Execution::UndoRefused(refused)
        );
        assert_eq!(app.undo(), Err(refused));
        std::thread::sleep(Duration::from_millis(200));
        app.poll_jobs();
        assert!(dir.path().join("b.txt").exists());
        assert!(!dir.path().join("a.txt").exists());
        assert!(!doomed.exists());
        assert_eq!(app.history().records().len(), 2);
    }

    #[test]
    fn 하나도_지우지_못한_영구_삭제는_기록에_남지_않는다() {
        let dir = TempDir::new().unwrap();
        let mut app = app_at(dir.path());
        let job = app
            .delete_permanently(vec![dir.path().join("없음.txt")])
            .unwrap();
        let (state, summary) = wait_job(&mut app, job);
        assert_eq!(state, JobState::Finished);
        assert_eq!(summary.items_deleted, 0);
        assert!(app.history().is_empty());
    }
}
