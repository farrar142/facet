// target: crates/facet-core/tests/spec_batch_rename_entry_numbered.rs
// AC: #291: 여러 항목 F2는 Execution::Rename, 고른 항목 모두 「바탕 (n)」+제 확장자(목록 순서), 커서 확장자 있음·없음이 같음, 선택 밖과 겹치는 번호 건너뜀, 기록 한 건과 실행취소·다시 실행 한 번, 막힌 경우 작업·기록 없음
//! 여러 항목을 고른 채 F2로 확정한 이름을 탐색기처럼 번호 붙여 고른 항목 모두에 쓰는 입구(`App::rename_numbered`)를
//! 임시 폴더의 실제 파일로 확인한다: 번호와 확장자, 목록 순서, 바탕 이름 정하기, 선택 밖 이름과 겹치는 번호 건너뛰기,
//! 작업 기록 한 건과 실행취소·다시 실행, 막힌 입력.
//!
//! 이름 바꾸기는 Windows 파일 연산이라 Windows에서만 돈다.
#![cfg(windows)]

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::command::CommandId;
use facet_core::history::OperationKind;
use facet_core::job::{JobId, JobKind, JobState};
use facet_core::tab::ReadOutcome;
use facet_core::window::{App, BatchRenameError, Execution};
use tempfile::TempDir;

/// CI 러너가 느려도 흔들리지 않게 넉넉히 기다린다. 결과가 오면 곧바로 끝난다.
const WAIT: Duration = Duration::from_secs(30);

/// 파일(내용은 제 이름)과 폴더(`/`로 끝나는 이름)를 만든다.
fn folder_with(names: &[&str]) -> TempDir {
    let dir = TempDir::new().unwrap();
    for name in names {
        match name.strip_suffix('/') {
            Some(folder) => fs::create_dir(dir.path().join(folder)).unwrap(),
            None => fs::write(dir.path().join(name), name).unwrap(),
        }
    }
    dir
}

fn app_at(path: &Path) -> App {
    let mut app = App::new(Some(Location::file_system(path)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

fn index_of(app: &App, name: &str) -> usize {
    let tab = app.active_tab();
    (0..tab.len())
        .find(|&i| tab.get(i).is_some_and(|item| item.name == name))
        .unwrap_or_else(|| panic!("목록에 {name}이 없다"))
}

/// `names`를 고르고 커서를 `cursor`(고른 것 가운데 하나)에 둔다.
fn select(app: &mut App, names: &[&str], cursor: &str) {
    let mut order: Vec<usize> = names
        .iter()
        .filter(|name| **name != cursor)
        .map(|name| index_of(app, name))
        .collect();
    order.push(index_of(app, cursor));
    let tab = app.active_tab_mut();
    tab.select_only(order[0]);
    for &index in &order[1..] {
        tab.toggle_select(index);
    }
    assert_eq!(tab.selected_count(), names.len());
}

fn names_on_disk(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn sorted(names: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = names.iter().map(|name| name.to_string()).collect();
    names.sort();
    names
}

/// 작업이 끝날 때까지 작업 사건(과 탭 쪽 결과)을 받고 마지막 상태를 돌려준다.
fn wait_job(app: &mut App, job: JobId) -> JobState {
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll_jobs();
        app.poll();
        if let Some(entry) = app.work_area().entry(job)
            && entry.summary.is_some()
        {
            return entry.state;
        }
        assert!(Instant::now() < deadline, "작업이 시간 안에 끝나지 않았다");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn rename_numbered(app: &mut App, typed: &str) {
    let job = app.rename_numbered(typed).expect("번호 붙이기를 시작한다");
    assert_eq!(
        app.work_area().entry(job).map(|entry| entry.kind),
        Some(JobKind::BatchRename)
    );
    assert_eq!(wait_job(app, job), JobState::Finished);
}

#[test]
#[ignore = "#291"]
fn f2_with_many_selected_still_opens_the_name_input_and_keeps_the_selection() {
    let dir = folder_with(&["a.txt", "b.jpg", "c.txt"]);
    let mut app = app_at(dir.path());
    select(&mut app, &["a.txt", "b.jpg", "c.txt"], "a.txt");

    assert_eq!(app.execute(CommandId::Rename), Execution::Rename);
    let tab = app.active_tab();
    assert_eq!(tab.selected_count(), 3, "선택은 그대로다");
    assert_eq!(
        tab.cursor_item().map(|item| item.name.clone()),
        Some("a.txt".into()),
        "입력란은 커서 항목 행에 열린다"
    );
}

#[test]
#[ignore = "#291"]
fn numbers_every_selected_item_in_list_order_keeping_each_extension() {
    let dir = folder_with(&["a.txt", "b.jpg", "c.txt", "d/"]);
    let mut app = app_at(dir.path());
    select(&mut app, &["a.txt", "b.jpg", "d"], "a.txt");

    // 번호는 목록 순서: 같은 순서의 항목 목록에서 기대 이름을 만든다.
    let items = app.batch_rename_items().expect("세 항목을 골랐다");
    let mut expected: Vec<String> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let name = item.name.to_string_lossy();
            let ext = match (item.is_folder, name.rfind('.')) {
                (false, Some(dot)) => &name[dot..],
                _ => "",
            };
            format!("사진 ({}){ext}", i + 1)
        })
        .collect();
    expected.push("c.txt".to_string());
    expected.sort();

    rename_numbered(&mut app, "사진");
    assert_eq!(names_on_disk(dir.path()), expected);
    assert!(
        dir.path()
            .join(
                expected
                    .iter()
                    .find(|name| !name.contains('.'))
                    .expect("폴더는 확장자 없이 번호만")
            )
            .is_dir()
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("c.txt")).unwrap(),
        "c.txt",
        "고르지 않은 항목은 그대로"
    );
}

#[test]
#[ignore = "#291"]
fn typed_name_keeping_the_cursor_extension_gives_the_same_names() {
    let names = ["a.txt", "b.jpg", "c.txt"];
    let mut results = Vec::new();
    for typed in ["사진", "사진.txt", "사진.TXT"] {
        let dir = folder_with(&names);
        let mut app = app_at(dir.path());
        select(&mut app, &names, "a.txt");
        rename_numbered(&mut app, typed);
        results.push((typed, names_on_disk(dir.path())));
    }
    let expected = sorted(&["사진 (1).txt", "사진 (2).jpg", "사진 (3).txt"]);
    for (typed, on_disk) in results {
        assert_eq!(on_disk, expected, "친 이름 {typed}");
    }
}

#[test]
#[ignore = "#291"]
fn skips_numbers_taken_by_unselected_items() {
    let dir = folder_with(&["a.txt", "b.txt", "c.txt", "사진 (2).txt"]);
    let mut app = app_at(dir.path());
    select(&mut app, &["a.txt", "b.txt", "c.txt"], "a.txt");

    rename_numbered(&mut app, "사진");
    assert_eq!(
        names_on_disk(dir.path()),
        sorted(&[
            "사진 (1).txt",
            "사진 (2).txt",
            "사진 (3).txt",
            "사진 (4).txt"
        ])
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("사진 (2).txt")).unwrap(),
        "사진 (2).txt",
        "선택 밖 항목은 그대로이고 그 번호는 건너뛴다"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("사진 (3).txt")).unwrap(),
        "b.txt"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("사진 (4).txt")).unwrap(),
        "c.txt"
    );
}

#[test]
#[ignore = "#291"]
fn one_record_undoes_and_redoes_all_names_at_once() {
    let names = ["a.txt", "b.jpg", "c.txt"];
    let dir = folder_with(&names);
    let mut app = app_at(dir.path());
    select(&mut app, &names, "a.txt");
    let numbered = sorted(&["사진 (1).txt", "사진 (2).jpg", "사진 (3).txt"]);

    rename_numbered(&mut app, "사진");
    assert_eq!(names_on_disk(dir.path()), numbered);
    assert_eq!(app.history().records().len(), 1, "기록은 한 건");
    let record = app.history().undo_target().expect("실행취소 대상");
    assert_eq!(record.kind(), OperationKind::BatchRename);
    assert_eq!(record.steps().len(), 3);

    let job = app.undo().expect("실행취소를 시작한다");
    assert_eq!(wait_job(&mut app, job), JobState::Finished);
    assert_eq!(
        names_on_disk(dir.path()),
        sorted(&names),
        "실행취소 한 번에 모두 원래 이름"
    );

    let job = app.redo().expect("다시 실행을 시작한다");
    assert_eq!(wait_job(&mut app, job), JobState::Finished);
    assert_eq!(names_on_disk(dir.path()), numbered);
    assert_eq!(app.history().records().len(), 1);
}

#[test]
#[ignore = "#291"]
fn rejected_input_creates_no_job_and_changes_nothing() {
    let names = ["a.txt", "b.txt", "사진 (1).md", "사진 (2).md"];
    let dir = folder_with(&names);
    let mut app = app_at(dir.path());

    select(&mut app, &["a.txt", "b.txt"], "a.txt");
    assert_eq!(
        app.rename_numbered("a:b"),
        Err(BatchRenameError::Problems),
        "쓸 수 없는 이름"
    );
    // 바탕 이름은 쓸 수 있어도 번호와 확장자를 붙이면 255단위를 넘는다.
    let long = "가".repeat(250);
    assert_eq!(
        app.rename_numbered(&long),
        Err(BatchRenameError::Problems),
        "번호를 붙인 이름이 너무 길다"
    );

    // 이미 번호 이름인 두 항목: 모두 그대로다.
    select(&mut app, &["사진 (1).md", "사진 (2).md"], "사진 (1).md");
    assert_eq!(
        app.rename_numbered("사진"),
        Err(BatchRenameError::NothingToRename)
    );

    select(&mut app, &["a.txt"], "a.txt");
    assert_eq!(
        app.rename_numbered("사진"),
        Err(BatchRenameError::Unavailable),
        "하나만 골랐으면 번호 붙이기가 아니다"
    );

    app.poll_jobs();
    assert!(app.work_area().is_empty(), "작업을 만들지 않는다");
    assert!(app.history().is_empty(), "기록이 없다");
    assert_eq!(names_on_disk(dir.path()), sorted(&names));
}
