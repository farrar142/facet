// target: crates/facet-core/tests/spec_virtual_folders_restore.rs
// AC: #353: 복원 작업이 생기고 원래 자리로 돌아와 휴지통에서 빠진다, 같은 이름이면 충돌 질문으로 멈추고 덮어쓰지 않는다, 건너뛰기, 둘 다 유지, 휴지통 밖에서는 NotAllowedHere, 고른 것이 없으면 꺼짐
//! 휴지통 위치의 「복원」을 실제 휴지통으로 공개 인터페이스(`App`)에서 확인한다: 임시 폴더의 파일을 S4의 휴지통 삭제로
//! 넣고 휴지통에서 골라 「복원」하면 복원 작업이 작업 영역에 생기고 원래 자리에 원래 내용으로 돌아오며 휴지통 목록에서
//! 빠지는지, 원래 자리에 같은 이름이 있으면 이름 충돌 질문으로 멈춰 덮어쓰지 않고 건너뛰기·둘 다 유지의 답대로 하는지,
//! 휴지통 밖에서는 위치가 막는지. 휴지통을 쓸 수 없는 계정이면 건너뛰었다고 출력하고, 넣은 항목은 끝날 때(실패해도)
//! 휴지통에서 지운다. Windows 전용.
#![cfg(windows)]

#[path = "common/bin.rs"]
mod bin;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::command::CommandId;
use facet_core::job::{
    self, ConflictAnswer, JobId, JobKind, JobQuestion, JobState, NotRecyclableAnswer, Resolution,
    Summary,
};
use facet_core::tab::{Navigation, ReadOutcome};
use facet_core::window::{App, Execution};
use facet_win::recycle::RecycledItem;
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(60);

/// 테스트가 휴지통에 넣은 항목. 버릴 때 휴지통에 남아 있으면 지운다.
#[derive(Default)]
struct Bin(Vec<RecycledItem>);

impl Bin {
    fn keep(&mut self, summary: &Summary) {
        self.0.extend(summary.recycled.iter().cloned());
    }
}

impl Drop for Bin {
    fn drop(&mut self) {
        bin::purge(&self.0);
    }
}

fn app_at(dir: &Path) -> App {
    let mut app = App::new(Some(Location::file_system(dir)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

fn wait_question(app: &mut App, job: JobId) -> Option<JobQuestion> {
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll_jobs();
        app.poll();
        let entry = app.work_area().entry(job).expect("작업 영역의 작업");
        if let Some(question) = &entry.question {
            assert_eq!(entry.state, JobState::AwaitingAnswer);
            return Some(question.clone());
        }
        if entry.summary.is_some() {
            return None;
        }
        assert!(Instant::now() < deadline, "작업이 끝나지 않았다");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// 작업이 묻지 않고 끝날 때까지 기다려 마지막 상태를 돌려준다.
fn wait_job(app: &mut App, job: JobId) -> JobState {
    if let Some(question) = wait_question(app, job) {
        panic!("묻지 않아야 한다: {question:?}");
    }
    app.work_area().entry(job).unwrap().state
}

fn recycle(app: &mut App, path: &Path, bin: &mut Bin, test: &str) -> bool {
    if !job::has_recycle_bin(path) {
        println!("건너뜀({test}): 휴지통이 없는 볼륨입니다");
        return false;
    }
    let job = app
        .recycle(vec![path.to_path_buf()])
        .expect("작업을 시작한다");
    match wait_question(app, job) {
        None => {}
        Some(JobQuestion::NotRecyclable(_)) => {
            app.answer_not_recyclable(job, NotRecyclableAnswer::CancelJob)
                .unwrap();
            wait_question(app, job);
            println!("건너뜀({test}): 이 계정은 휴지통을 쓸 수 없습니다");
            return false;
        }
        Some(other) => panic!("묻지 않아야 한다: {other:?}"),
    }
    let entry = app.work_area().entry(job).unwrap();
    bin.keep(entry.summary.as_ref().unwrap());
    assert_eq!(entry.state, JobState::Finished);
    true
}

fn settle(app: &mut App, expected: &Location) {
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll();
        let tab = app.active_tab();
        if tab.pending_location().is_none() {
            if let Some(error) = tab.error() {
                panic!("읽기가 실패했다: {error}");
            }
            if tab.location() == expected && tab.loading().is_none() {
                return;
            }
        }
        assert!(Instant::now() < deadline, "{expected}에 닿지 못했다");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn find(app: &App, original: &Path) -> Option<usize> {
    let tab = app.active_tab();
    (0..tab.len()).find(|&i| {
        tab.recycled(i)
            .is_some_and(|entry| entry.original == original)
    })
}

/// 휴지통을 열고 원래 경로가 `original`인 항목을 고른다.
fn open_and_select(app: &mut App, original: &Path) {
    assert_eq!(app.execute(CommandId::OpenRecycleBin), Execution::Done);
    settle(app, &Location::recycle_bin());
    let index =
        find(app, original).unwrap_or_else(|| panic!("휴지통에 {}가 없다", original.display()));
    app.active_tab_mut().select_only(index);
}

/// 「복원」을 실행하고 넣은 복원 작업을 돌려준다.
fn restore(app: &mut App) -> JobId {
    let before: Vec<JobId> = app.work_area().entries().iter().map(|e| e.id).collect();
    assert!(app.can_execute(CommandId::Restore));
    assert_eq!(app.execute(CommandId::Restore), Execution::Done);
    let entry = app
        .work_area()
        .entries()
        .iter()
        .find(|e| !before.contains(&e.id))
        .expect("작업 영역에 새 작업");
    assert_eq!(entry.kind, JobKind::Restore);
    entry.id
}

/// 휴지통을 다시 읽어 `original` 항목이 아직 있는가.
fn still_recycled(app: &mut App, original: &Path) -> bool {
    assert!(app.navigate(Navigation::Refresh));
    settle(app, &Location::recycle_bin());
    find(app, original).is_some()
}

/// 같은 이름 파일이 이미 있는 원래 자리로 복원을 시작해 이름 충돌 질문까지 간다.
fn restore_into_conflict(
    dir: &TempDir,
    bin: &mut Bin,
    test: &str,
) -> Option<(App, PathBuf, JobId)> {
    let path = dir.path().join("겹침.txt");
    fs::write(&path, "지운 내용").unwrap();
    let mut app = app_at(dir.path());
    if !recycle(&mut app, &path, bin, test) {
        return None;
    }
    fs::write(&path, "새 내용").unwrap();
    open_and_select(&mut app, &path);
    let job = restore(&mut app);
    match wait_question(&mut app, job) {
        Some(JobQuestion::Conflict(conflict)) => {
            assert_eq!(conflict.target.path, path, "원래 자리의 항목과 부딪힌다");
        }
        other => panic!("이름 충돌을 물어야 한다: {other:?}"),
    }
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "새 내용",
        "묻는 동안 아무것도 덮어쓰지 않는다"
    );
    Some((app, path, job))
}

#[test]
fn restore_puts_the_file_back_and_removes_it_from_the_recycle_bin() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("하위").join("되살릴 파일.txt");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "원래 내용").unwrap();
    let mut bin = Bin::default();
    let mut app = app_at(dir.path());
    if !recycle(&mut app, &path, &mut bin, "복원") {
        return;
    }
    // 원래 폴더가 없어도 만들어 되살린다.
    fs::remove_dir(path.parent().unwrap()).unwrap();

    open_and_select(&mut app, &path);
    let job = restore(&mut app);
    assert_eq!(wait_job(&mut app, job), JobState::Finished);

    assert_eq!(fs::read_to_string(&path).unwrap(), "원래 내용");
    assert!(!still_recycled(&mut app, &path), "휴지통 목록에서 빠진다");
}

#[test]
fn skip_keeps_the_existing_file_and_the_recycled_item() {
    let dir = TempDir::new().unwrap();
    let mut bin = Bin::default();
    let Some((mut app, path, job)) = restore_into_conflict(&dir, &mut bin, "건너뛰기") else {
        return;
    };
    app.answer_conflict(job, ConflictAnswer::once(Resolution::Skip))
        .unwrap();
    wait_job(&mut app, job);

    assert_eq!(fs::read_to_string(&path).unwrap(), "새 내용");
    assert!(!dir.path().join("겹침 (2).txt").exists());
    assert!(
        still_recycled(&mut app, &path),
        "건너뛴 항목은 휴지통에 남는다"
    );
}

#[test]
fn keep_both_restores_under_a_numbered_name() {
    let dir = TempDir::new().unwrap();
    let mut bin = Bin::default();
    let Some((mut app, path, job)) = restore_into_conflict(&dir, &mut bin, "둘 다 유지") else {
        return;
    };
    app.answer_conflict(job, ConflictAnswer::once(Resolution::KeepBoth))
        .unwrap();
    assert_eq!(wait_job(&mut app, job), JobState::Finished);

    assert_eq!(fs::read_to_string(&path).unwrap(), "새 내용");
    assert_eq!(
        fs::read_to_string(dir.path().join("겹침 (2).txt")).unwrap(),
        "지운 내용"
    );
    assert!(!still_recycled(&mut app, &path));
}

#[test]
fn restore_is_disabled_when_nothing_is_chosen_in_the_recycle_bin() {
    let dir = TempDir::new().unwrap();
    let mut app = app_at(dir.path());
    assert_eq!(app.execute(CommandId::OpenRecycleBin), Execution::Done);
    settle(&mut app, &Location::recycle_bin());

    // 어느 항목에도 맞지 않는 필터로 보이는 항목을 모두 가려 선택과 커서를 없앤다.
    let tab = app.active_tab_mut();
    let request = tab
        .request_filter("facet-복원-시험-맞는-항목-없음-7f3a9c")
        .expect("필터 요청");
    tab.finish_filter(request.run());
    assert!(tab.is_empty(), "보이는 항목이 없다");
    assert_eq!(tab.selected_count(), 0);
    assert_eq!(tab.cursor(), None);

    assert!(
        !app.can_execute(CommandId::Restore),
        "고른 것이 없으면 꺼진다"
    );
    assert_eq!(app.execute(CommandId::Restore), Execution::Disabled);
    assert!(app.work_area().is_empty(), "작업을 넣지 않았다");
}

#[test]
fn restore_is_not_allowed_outside_the_recycle_bin() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("보통 파일.txt"), "a").unwrap();
    let mut app = app_at(dir.path());
    app.active_tab_mut().select_only(0);
    assert!(!app.can_execute(CommandId::Restore));
    assert_eq!(app.execute(CommandId::Restore), Execution::NotAllowedHere);
    assert!(app.work_area().is_empty(), "작업을 넣지 않았다");
}
