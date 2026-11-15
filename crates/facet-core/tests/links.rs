//! 복사·이동이 폴더 심볼릭 링크·정션·파일 심볼릭 링크를 따라 들어가지 않고 링크 자체를 대상에 만드는지 임시
//! 폴더의 실제 링크로 확인한다(Windows 전용).
//!
//! 정션은 관리자·개발자 모드 없이 만들 수 있어 늘 돈다. 심볼릭 링크 테스트는 링크를 만들 수 있는 환경(관리자나
//! 개발자 모드)에서만 돌고, 못 만들면 건너뛰었다고 출력한다.

#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use facet_core::job::{
    ErrorAnswer, FileOpErrorKind, JobEvent, JobId, JobQueue, JobState, Progress, Summary,
};
use facet_win::file_ops::{self, LinkKind, fault};
use tempfile::TempDir;

/// 작업 하나가 끝날 때까지 기다리는 넉넉한 한도. 순환을 따라 들어가면 이 안에 끝나지 않는다.
const TIMEOUT: Duration = Duration::from_secs(60);

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

fn finished(events: &[JobEvent], id: JobId) -> Summary {
    events
        .iter()
        .find_map(|event| match event {
            JobEvent::Finished { job, summary } if *job == id => Some(summary.clone()),
            _ => None,
        })
        .expect("끝남 사건")
}

fn last_progress(events: &[JobEvent], id: JobId) -> Progress {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            JobEvent::Progress { job, progress } if *job == id => Some(progress.clone()),
            _ => None,
        })
        .expect("진행 사건")
}

/// 작업을 넣고 끝날 때까지 기다려 사건을 돌려준다. 그 사이 질문이 오면 실패한다.
fn run(queue: &mut JobQueue, id: JobId) -> Vec<JobEvent> {
    let mut events = Vec::new();
    wait_for(
        queue,
        &mut events,
        |e| matches!(e, JobEvent::Finished { job, .. } if *job == id),
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, JobEvent::ItemError { .. } | JobEvent::Conflict { .. })),
        "묻지 않아야 한다: {events:?}"
    );
    events
}

fn copy(queue: &mut JobQueue, sources: Vec<PathBuf>, target: &Path) -> Vec<JobEvent> {
    let id = queue.copy(sources, target.to_path_buf()).unwrap();
    run(queue, id)
}

fn job_of(events: &[JobEvent]) -> JobId {
    match events.last().unwrap() {
        JobEvent::Finished { job, .. } => *job,
        other => panic!("{other:?}"),
    }
}

/// `mklink`로 링크를 만든다(`/J` 정션, `/D` 폴더 심볼릭 링크, 없으면 파일 심볼릭 링크). 성공했는지 돌려준다.
fn mklink(flag: Option<&str>, link: &Path, target: &Path) -> bool {
    let mut command = Command::new("cmd");
    command.arg("/c").arg("mklink");
    if let Some(flag) = flag {
        command.arg(flag);
    }
    command
        .arg(link)
        .arg(target)
        .output()
        .is_ok_and(|out| out.status.success())
}

fn junction(link: &Path, target: &Path) {
    assert!(mklink(Some("/J"), link, target), "mklink /J");
}

/// `path`가 링크 자체이고(따라 들어간 복사본이 아니고) `kind`로 `target`을 가리키는지.
fn assert_link(path: &Path, kind: LinkKind, target: &Path) {
    let metadata = fs::symlink_metadata(path).unwrap();
    assert!(
        metadata.file_type().is_symlink(),
        "{}는 링크여야 한다",
        path.display()
    );
    let link = file_ops::read_link(path).unwrap();
    assert_eq!(link.kind, kind, "{}", path.display());
    assert_eq!(link.target, target, "{}", path.display());
}

/// 링크를 지운다(테스트가 끝난 뒤 임시 폴더 지우기가 링크를 따라 들어가지 않게).
fn remove_link(path: &Path) {
    file_ops::remove_link(path).unwrap();
}

#[test]
fn 정션은_같은_대상을_가리키는_정션으로_복사되고_안은_복사되지_않는다() {
    let dir = TempDir::new().unwrap();
    let outside = dir.path().join("밖");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("b.txt"), "밖의 내용").unwrap();
    let source = dir.path().join("원본");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("a.txt"), "a").unwrap();
    junction(&source.join("바깥 🔗"), &outside);
    let target = dir.path().join("대상");
    fs::create_dir(&target).unwrap();

    let mut queue = JobQueue::new();
    let events = copy(&mut queue, vec![source.clone()], &target);
    let id = job_of(&events);
    let summary = finished(&events, id);
    assert_eq!(summary.links_copied, 1, "{summary:?}");
    assert_eq!(summary.files_copied, 1, "{summary:?}");
    assert_eq!(summary.bytes_copied, 1, "{summary:?}");
    assert!(summary.failures.is_empty(), "{summary:?}");

    let copied = target.join("원본").join("바깥 🔗");
    assert_link(&copied, LinkKind::Junction, &outside);
    // 링크를 지우면 대상에 남는 것은 a.txt뿐이다: 정션 안의 내용은 복사되지 않았다.
    remove_link(&copied);
    let names: Vec<_> = fs::read_dir(target.join("원본"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["a.txt"]);
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);

    // 진행: 링크는 항목 하나(바이트 0)다. 폴더 + a.txt + 링크.
    let progress = last_progress(&events, id);
    assert_eq!(progress.items_total, 3, "{progress:?}");
    assert_eq!(progress.items_done, 3, "{progress:?}");
    assert_eq!(progress.bytes_total, 1, "{progress:?}");
    assert_eq!(progress.bytes_done, 1, "{progress:?}");
    remove_link(&source.join("바깥 🔗"));
}

#[test]
fn 자기_상위를_가리키는_정션이_든_폴더_복사가_끝난다() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("원본");
    let sub = source.join("하위");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("a.txt"), "a").unwrap();
    // 따라 들어가면 끝나지 않는 고리 둘: 상위(원본)와 자기 자신.
    junction(&sub.join("위로"), &source);
    junction(&sub.join("나"), &sub);
    let target = dir.path().join("대상");
    fs::create_dir(&target).unwrap();

    let mut queue = JobQueue::new();
    let events = copy(&mut queue, vec![source.clone()], &target);
    let summary = finished(&events, job_of(&events));
    assert_eq!(summary.links_copied, 2, "{summary:?}");
    assert_eq!(summary.files_copied, 1, "{summary:?}");
    assert_eq!(summary.folders_copied, 2, "{summary:?}");
    let copied = target.join("원본").join("하위");
    // 복사한 정션은 원본 쪽 폴더를 그대로 가리킨다(복사본을 가리키도록 바꾸지 않는다).
    assert_link(&copied.join("위로"), LinkKind::Junction, &source);
    assert_link(&copied.join("나"), LinkKind::Junction, &sub);
    assert_eq!(fs::read_to_string(copied.join("a.txt")).unwrap(), "a");
    for path in [
        copied.join("위로"),
        copied.join("나"),
        sub.join("위로"),
        sub.join("나"),
    ] {
        remove_link(&path);
    }
}

#[test]
fn 정션_자체를_원본으로_넘기면_정션을_만들고_같은_폴더면_복사본_이름이다() {
    let dir = TempDir::new().unwrap();
    let outside = dir.path().join("밖");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("b.txt"), "b").unwrap();
    let link = dir.path().join("고리");
    junction(&link, &outside);
    let target = dir.path().join("대상");
    fs::create_dir(&target).unwrap();

    let mut queue = JobQueue::new();
    let events = copy(&mut queue, vec![link.clone()], &target);
    let summary = finished(&events, job_of(&events));
    assert_eq!(summary.links_copied, 1, "{summary:?}");
    assert_eq!(summary.files_copied, 0, "{summary:?}");
    assert_link(&target.join("고리"), LinkKind::Junction, &outside);

    let events = copy(&mut queue, vec![link.clone()], dir.path());
    assert_eq!(finished(&events, job_of(&events)).links_copied, 1);
    let same_folder = dir.path().join("고리 - 복사본");
    assert_link(&same_folder, LinkKind::Junction, &outside);

    for path in [target.join("고리"), same_folder, link] {
        remove_link(&path);
    }
}

#[test]
fn 링크를_만들_권한이_없으면_항목별_오류로_묻고_다시_시도하면_만든다() {
    let dir = TempDir::new().unwrap();
    let outside = dir.path().join("밖");
    fs::create_dir(&outside).unwrap();
    let link = dir.path().join("고리");
    junction(&link, &outside);
    let target = dir.path().join("대상");
    fs::create_dir(&target).unwrap();
    // 대상 폴더 아래에서는 링크를 만들 권한이 없는 환경처럼 운영체제의 권한 없음 코드로 실패하게 한다. (CI는
    // 관리자이고 복원 권한이 켜져 있어 폴더 권한을 막아도 만들어지므로 오류 주입으로 본다.)
    let denied = fault::deny_link_creation_under(&target);

    let mut queue = JobQueue::new();
    let id = queue.copy(vec![link.clone()], target.clone()).unwrap();
    let mut events = Vec::new();
    wait_for(&mut queue, &mut events, |e| {
        matches!(e, JobEvent::ItemError { .. } | JobEvent::Finished { .. })
    });
    let n = events.len();
    match &events[n - 1] {
        JobEvent::ItemError { job, error } => {
            assert_eq!(*job, id);
            assert_eq!(error.source, link);
            assert_eq!(error.kind, FileOpErrorKind::AccessDenied);
            assert_eq!(
                events[n - 2],
                JobEvent::StateChanged {
                    job: id,
                    state: JobState::AwaitingAnswer
                }
            );
        }
        other => panic!("질문이 와야 한다: {other:?}"),
    }
    assert!(fs::read_dir(&target).unwrap().next().is_none());

    drop(denied);
    queue.answer_error(id, ErrorAnswer::Retry).unwrap();
    let start = events.len();
    wait_for(
        &mut queue,
        &mut events,
        |e| matches!(e, JobEvent::Finished { job, .. } if *job == id),
    );
    assert!(
        !events[start..]
            .iter()
            .any(|e| matches!(e, JobEvent::ItemError { .. })),
        "{events:?}"
    );
    let summary = finished(&events, id);
    assert_eq!(summary.links_copied, 1, "{summary:?}");
    assert!(summary.failures.is_empty(), "{summary:?}");
    assert_link(&target.join("고리"), LinkKind::Junction, &outside);
    remove_link(&target.join("고리"));
    remove_link(&link);
}

#[test]
fn 링크를_만들_권한이_없어_건너뛰면_실패로_끝나고_링크만_빠진다() {
    let dir = TempDir::new().unwrap();
    let outside = dir.path().join("밖");
    fs::create_dir(&outside).unwrap();
    let source = dir.path().join("원본");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("a.txt"), "a").unwrap();
    let link = source.join("고리");
    junction(&link, &outside);
    let target = dir.path().join("대상");
    fs::create_dir(&target).unwrap();
    let copied = target.join("원본");
    // 링크 만들기만 권한 없음으로 실패한다: a.txt와 폴더는 복사되고 링크만 빠진다.
    let _denied = fault::deny_link_creation_under(&target);

    let mut queue = JobQueue::new();
    let id = queue.copy(vec![source.clone()], target.clone()).unwrap();
    let mut events = Vec::new();
    wait_for(&mut queue, &mut events, |e| {
        matches!(e, JobEvent::ItemError { .. } | JobEvent::Finished { .. })
    });
    match events.last().unwrap() {
        JobEvent::ItemError { error, .. } => {
            assert_eq!(error.source, link);
            assert_eq!(error.kind, FileOpErrorKind::AccessDenied);
        }
        other => panic!("질문이 와야 한다: {other:?}"),
    }
    queue.answer_error(id, ErrorAnswer::Skip).unwrap();
    wait_for(
        &mut queue,
        &mut events,
        |e| matches!(e, JobEvent::Finished { job, .. } if *job == id),
    );
    let summary = finished(&events, id);
    assert_eq!(summary.links_copied, 0, "{summary:?}");
    assert_eq!(summary.errors_skipped(), 1, "{summary:?}");
    assert!(events.contains(&JobEvent::StateChanged {
        job: id,
        state: JobState::Failed
    }));
    assert_eq!(summary.files_copied, 1, "{summary:?}");
    assert_eq!(fs::read_to_string(copied.join("a.txt")).unwrap(), "a");
    assert!(fs::symlink_metadata(copied.join("고리")).is_err());
    remove_link(&link);
}

#[test]
fn 심볼릭_링크는_링크_자체로_복사된다_개발자_모드에서만() {
    let dir = TempDir::new().unwrap();
    let outside = dir.path().join("밖");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("b.txt"), "b").unwrap();
    let source = dir.path().join("원본");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("a.txt"), "a").unwrap();
    let folder_link = source.join("폴더 링크");
    if !mklink(Some("/D"), &folder_link, &outside) {
        println!("건너뜀: 이 환경은 심볼릭 링크를 만들 수 없다(관리자도 개발자 모드도 아님)");
        return;
    }
    // 원본 자신을 가리키는 폴더 링크(고리), 상대 경로 파일 링크, 가리키는 것이 없는 파일 링크.
    assert!(mklink(Some("/D"), &source.join("고리"), &source));
    assert!(mklink(None, &source.join("상대"), Path::new("a.txt")));
    let dangling = dir.path().join("없는 파일.txt");
    assert!(mklink(None, &source.join("끊김"), &dangling));
    let target = dir.path().join("대상");
    fs::create_dir(&target).unwrap();

    let mut queue = JobQueue::new();
    let events = copy(&mut queue, vec![source.clone()], &target);
    let id = job_of(&events);
    let summary = finished(&events, id);
    assert_eq!(summary.links_copied, 4, "{summary:?}");
    assert_eq!(summary.files_copied, 1, "{summary:?}");
    assert!(summary.failures.is_empty(), "{summary:?}");
    let copied = target.join("원본");
    assert_link(&copied.join("폴더 링크"), LinkKind::FolderSymlink, &outside);
    assert_link(&copied.join("고리"), LinkKind::FolderSymlink, &source);
    assert_link(
        &copied.join("상대"),
        LinkKind::FileSymlink,
        Path::new("a.txt"),
    );
    assert!(file_ops::read_link(&copied.join("상대")).unwrap().relative);
    // 상대 링크는 복사본 자리 기준으로 복사본의 a.txt를 가리킨다.
    assert_eq!(fs::read_to_string(copied.join("상대")).unwrap(), "a");
    assert_link(&copied.join("끊김"), LinkKind::FileSymlink, &dangling);
    let progress = last_progress(&events, id);
    assert_eq!(progress.items_total, 6, "{progress:?}");
    assert_eq!(progress.bytes_total, 1, "{progress:?}");

    for name in ["폴더 링크", "고리", "상대", "끊김"] {
        remove_link(&copied.join(name));
        remove_link(&source.join(name));
    }
}

#[test]
fn 이동의_복사_단계도_링크_자체를_옮긴다() {
    // 같은 볼륨 이동은 이름 바꾸기라 링크도 그대로 옮겨진다. 볼륨을 넘는 이동은 두 번째 볼륨이 있을 때만 본다.
    let Some(other) = std::env::var_os("FACET_TEST_OTHER_VOLUME_DIR").map(PathBuf::from) else {
        println!("건너뜀: FACET_TEST_OTHER_VOLUME_DIR가 없다(볼륨을 넘는 이동)");
        return;
    };
    let dir = TempDir::new().unwrap();
    let outside = dir.path().join("밖");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("b.txt"), "b").unwrap();
    let source = dir.path().join("원본");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("a.txt"), "a").unwrap();
    junction(&source.join("바깥"), &outside);
    junction(&source.join("고리"), &source);
    fs::create_dir_all(&other).unwrap();
    let target = TempDir::new_in(&other).unwrap();
    if fs::canonicalize(target.path()).unwrap().components().next()
        == fs::canonicalize(dir.path()).unwrap().components().next()
    {
        println!("건너뜀: FACET_TEST_OTHER_VOLUME_DIR가 같은 볼륨이다");
        return;
    }

    let mut queue = JobQueue::new();
    let id = queue
        .move_items(vec![source.clone()], target.path().to_path_buf())
        .unwrap();
    let events = run(&mut queue, id);
    let summary = finished(&events, id);
    assert_eq!(summary.links_copied, 2, "{summary:?}");
    assert!(summary.failures.is_empty(), "{summary:?}");
    let moved = target.path().join("원본");
    assert_link(&moved.join("바깥"), LinkKind::Junction, &outside);
    assert_link(&moved.join("고리"), LinkKind::Junction, &source);
    assert_eq!(fs::read_to_string(moved.join("a.txt")).unwrap(), "a");
    // 원본(링크 포함)은 사라지고, 링크가 가리키던 폴더의 내용은 그대로다.
    assert!(fs::symlink_metadata(&source).is_err());
    assert_eq!(fs::read_to_string(outside.join("b.txt")).unwrap(), "b");
    remove_link(&moved.join("바깥"));
    remove_link(&moved.join("고리"));
}
