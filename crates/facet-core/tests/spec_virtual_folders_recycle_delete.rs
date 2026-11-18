// target: crates/facet-core/tests/spec_virtual_folders_recycle_delete.rs
// AC: #354: 휴지통 안의 삭제·영구 삭제는 영구 삭제 확인을 맡기고 지우지 않는다(항목 메뉴의 삭제 줄은 켜짐), 확인하면 그 항목만 사라지고 되돌릴 수 없는 기록, 휴지통 비우기는 확인을 맡기고 지우지 않는다, 휴지통 밖의 비우기는 NotAllowedHere
//! 휴지통 위치 안의 영구 삭제와 「휴지통 비우기」를 실제 휴지통으로 공개 인터페이스(`App`)에서 확인한다: 휴지통에서 고른
//! 항목의 삭제·영구 삭제가 먼저 영구 삭제 확인을 맡기고, 확인하면 그 항목(휴지통 안의 내용과 정보 파일)만 사라지며 다른
//! 항목은 남고 작업 기록에 되돌릴 수 없는 작업으로 남는지, 비우기는 확인만 맡기고 아무것도 지우지 않는지, 휴지통 밖에서는
//! 위치가 막는지. 러너의 휴지통을 실제로 비우는 테스트는 하지 않는다(다른 항목을 지울 수 있다). 휴지통을 쓸 수 없는
//! 계정이면 건너뛰었다고 출력하고, 넣은 항목은 끝날 때(실패해도) 휴지통에서 지운다. Windows 전용.
#![cfg(windows)]

#[path = "common/bin.rs"]
mod bin;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::command::CommandId;
use facet_core::history::{NotUndoable, OperationKind};
use facet_core::job::{self, JobId, JobQuestion, JobState, NotRecyclableAnswer, Summary};
use facet_core::menu::MenuKind;
use facet_core::tab::{Navigation, ReadOutcome};
use facet_core::window::{App, DeleteWarning, Execution};
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

    fn item(&self, original: &Path) -> &RecycledItem {
        self.0
            .iter()
            .find(|item| item.original() == original)
            .expect("휴지통 항목")
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
            return Some(question.clone());
        }
        if entry.summary.is_some() {
            return None;
        }
        assert!(Instant::now() < deadline, "작업이 끝나지 않았다");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn recycle(app: &mut App, sources: Vec<PathBuf>, bin: &mut Bin, test: &str) -> bool {
    if !sources.iter().all(|source| job::has_recycle_bin(source)) {
        println!("건너뜀({test}): 휴지통이 없는 볼륨입니다");
        return false;
    }
    let job = app.recycle(sources).expect("작업을 시작한다");
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

fn open_recycle_bin(app: &mut App) {
    assert_eq!(app.execute(CommandId::OpenRecycleBin), Execution::Done);
    settle(app, &Location::recycle_bin());
}

fn select(app: &mut App, original: &Path) {
    let index =
        find(app, original).unwrap_or_else(|| panic!("휴지통에 {}가 없다", original.display()));
    app.active_tab_mut().select_only(index);
}

fn refresh(app: &mut App) {
    assert!(app.navigate(Navigation::Refresh));
    settle(app, &Location::recycle_bin());
}

/// 휴지통 항목의 정보 파일(`$I...`) 경로.
fn info_file(item: &RecycledItem) -> PathBuf {
    let recycled = item.recycled();
    let name = recycled
        .file_name()
        .unwrap()
        .to_string_lossy()
        .replacen("$R", "$I", 1);
    recycled.with_file_name(name)
}

/// 휴지통 삭제한 두 파일(지울 것, 남길 것)로 휴지통을 연다.
fn two_recycled(dir: &TempDir, bin: &mut Bin, test: &str) -> Option<(App, PathBuf, PathBuf)> {
    let gone = dir.path().join("지울 항목.txt");
    let kept = dir.path().join("남길 항목.txt");
    fs::write(&gone, "a").unwrap();
    fs::write(&kept, "b").unwrap();
    let mut app = app_at(dir.path());
    if !recycle(&mut app, vec![gone.clone(), kept.clone()], bin, test) {
        return None;
    }
    open_recycle_bin(&mut app);
    Some((app, gone, kept))
}

#[test]
fn delete_in_the_recycle_bin_asks_for_permanent_delete_first() {
    let dir = TempDir::new().unwrap();
    let mut bin = Bin::default();
    let Some((mut app, gone, _kept)) = two_recycled(&dir, &mut bin, "확인") else {
        return;
    };
    select(&mut app, &gone);
    let jobs = app.work_area().entries().len();

    for command in [CommandId::Delete, CommandId::DeletePermanently] {
        assert!(app.can_execute(command), "{command:?}는 켜진다");
        assert_eq!(
            app.execute(command),
            Execution::ConfirmDelete(DeleteWarning::Permanent),
            "{command:?}"
        );
        let plan = app.delete_plan(command).expect("지울 내용");
        assert_eq!(plan.warning, Some(DeleteWarning::Permanent));
        assert_eq!(plan.sources.len(), 1);
    }

    let index = find(&app, &gone);
    assert_eq!(app.context_click(0, index), Some(MenuKind::Item));
    let menu = app.context_menu(MenuKind::Item);
    let delete = menu
        .commands()
        .find(|c| c.id == CommandId::Delete)
        .expect("삭제 줄");
    assert!(delete.enabled, "메뉴의 삭제 줄이 켜진다");

    app.poll_jobs();
    assert_eq!(
        app.work_area().entries().len(),
        jobs,
        "확인 전에는 지우지 않는다"
    );
    assert!(bin.item(&gone).recycled().exists());
    refresh(&mut app);
    assert!(find(&app, &gone).is_some());
}

#[test]
fn confirming_deletes_only_that_item_for_good() {
    let dir = TempDir::new().unwrap();
    let mut bin = Bin::default();
    let Some((mut app, gone, kept)) = two_recycled(&dir, &mut bin, "영구 삭제") else {
        return;
    };
    select(&mut app, &gone);
    let plan = app
        .delete_plan(CommandId::DeletePermanently)
        .expect("지울 내용");
    let job = app.start_delete(&plan).expect("작업을 넣는다");
    if let Some(question) = wait_question(&mut app, job) {
        panic!("묻지 않아야 한다: {question:?}");
    }
    assert_eq!(
        app.work_area().entry(job).unwrap().state,
        JobState::Finished
    );

    let item = bin.item(&gone);
    assert!(!item.recycled().exists(), "휴지통 안의 내용이 사라진다");
    assert!(!info_file(item).exists(), "휴지통 정보 파일도 사라진다");
    assert!(!gone.exists(), "되살리지 않는다");
    assert!(bin.item(&kept).recycled().exists(), "다른 항목은 그대로다");

    refresh(&mut app);
    assert!(find(&app, &gone).is_none(), "휴지통 목록에서 빠진다");
    assert!(find(&app, &kept).is_some());

    let record = app.history().records().last().expect("작업 기록");
    assert_eq!(record.kind(), OperationKind::Delete);
    assert_eq!(
        record.not_undoable_reason(),
        Some(NotUndoable::PermanentDelete)
    );
}

#[test]
fn empty_recycle_bin_asks_first_and_deletes_nothing() {
    let dir = TempDir::new().unwrap();
    let mut bin = Bin::default();
    let Some((mut app, gone, kept)) = two_recycled(&dir, &mut bin, "비우기") else {
        return;
    };
    let jobs = app.work_area().entries().len();

    assert!(app.can_execute(CommandId::EmptyRecycleBin));
    assert_eq!(
        app.execute(CommandId::EmptyRecycleBin),
        Execution::ConfirmEmptyRecycleBin
    );

    app.poll_jobs();
    assert_eq!(
        app.work_area().entries().len(),
        jobs,
        "확인 전에는 비우지 않는다"
    );
    assert!(bin.item(&gone).recycled().exists());
    assert!(bin.item(&kept).recycled().exists());
    refresh(&mut app);
    assert!(find(&app, &gone).is_some());
    assert!(find(&app, &kept).is_some());
}

#[test]
fn empty_recycle_bin_is_not_allowed_outside_the_recycle_bin() {
    let dir = TempDir::new().unwrap();
    let mut app = app_at(dir.path());
    assert!(!app.can_execute(CommandId::EmptyRecycleBin));
    assert_eq!(
        app.execute(CommandId::EmptyRecycleBin),
        Execution::NotAllowedHere
    );
    assert!(app.work_area().is_empty());
}
