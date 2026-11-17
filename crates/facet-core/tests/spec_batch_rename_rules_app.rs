// target: crates/facet-core/tests/spec_batch_rename_rules_app.rs
// AC: #274: App::batch_rename_preview_rules가 선택 항목과 목록의 수정 시각을 넘김·꺼진 명령이면 None, S14a 미리 보기와 같음, App::start_batch_rename_rules의 실행·막힘·기록 하나·실행취소 한 번
//! 앱의 규칙 목록 입구(`App::batch_rename_preview_rules`·`App::start_batch_rename_rules`)를 임시 폴더의 실제 파일로 확인한다:
//! 선택 항목(목록 순서)에 목록이 가진 수정 시각과 넘긴 UTC 차이로 날짜를 넣음, 선택하지 않은 항목과의 겹침, S14a 규칙을
//! 규칙 목록으로 바꾼 미리 보기가 S14a 입구와 같음, 명령이 꺼져 있으면 아무것도 하지 않음. 미리 보기는 파일을 바꾸지 않아
//! 모든 환경에서 돌고, 실제 이름 바꾸기(작업 하나·기록 하나·실행취소 한 번)는 Windows 파일 연산이라 Windows에서만 돈다.

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use facet_core::Location;
use facet_core::batch_rename::{
    BatchProblem, Edge, EntryStatus, Place, Preview, RenameRule, Rule, RuleKind,
};
use facet_core::tab::ReadOutcome;
use facet_core::window::{App, BatchRenameError};
use tempfile::TempDir;

/// CI 러너가 느려도 흔들리지 않게 넉넉히 기다린다. 결과가 오면 곧바로 끝난다.
const WAIT: Duration = Duration::from_secs(30);

const HOUR: i64 = 3600;

/// 2026-11-15 23:30:45 UTC(유닉스 초).
const NOV_15: u64 = 1_794_785_445;
/// 2026-12-31 20:00:00 UTC(유닉스 초).
const DEC_31: u64 = 1_798_747_200;

/// 이름과 수정 시각(유닉스 초)으로 파일을 만든다.
fn folder_with(files: &[(&str, u64)]) -> TempDir {
    let dir = TempDir::new().unwrap();
    for &(name, unix) in files {
        let path = dir.path().join(name);
        fs::write(&path, name).unwrap();
        let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(unix))
            .unwrap();
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

/// 이름들을 고른다(첫 이름만 고른 뒤 나머지를 더함).
fn select(app: &mut App, names: &[&str]) {
    let indices: Vec<usize> = names.iter().map(|name| index_of(app, name)).collect();
    let tab = app.active_tab_mut();
    tab.select_only(indices[0]);
    for &index in &indices[1..] {
        tab.toggle_select(index);
    }
}

fn names(preview: &Preview) -> Vec<&str> {
    preview
        .entries()
        .iter()
        .map(|entry| entry.new_name.as_str())
        .collect()
}

fn date_first() -> Rule {
    Rule::new(RuleKind::Date {
        format: "yyyy-MM-dd".to_string(),
        place: Place::Start,
        separator: " ".to_string(),
    })
}

fn replace(find: &str, with: &str) -> Rule {
    Rule::new(RuleKind::Replace {
        find: find.to_string(),
        replace: with.to_string(),
        regex: false,
        case_sensitive: false,
        all: true,
    })
}

#[test]
#[ignore = "#274"]
fn preview_rules_uses_the_modified_time_from_the_list() {
    let dir = folder_with(&[("b.txt", DEC_31), ("a.txt", NOV_15), ("c.txt", NOV_15)]);
    let mut app = app_at(dir.path());
    select(&mut app, &["b.txt", "a.txt"]);

    let preview = app
        .batch_rename_preview_rules(&[date_first()], 0)
        .expect("두 항목을 골랐다");
    assert_eq!(
        names(&preview),
        ["2026-11-15 a.txt", "2026-12-31 b.txt"],
        "목록 순서, 항목마다 제 수정 시각"
    );
    assert!(preview.can_execute());

    let preview = app
        .batch_rename_preview_rules(&[date_first()], 9 * HOUR)
        .unwrap();
    assert_eq!(
        names(&preview),
        ["2026-11-16 a.txt", "2027-01-01 b.txt"],
        "넘긴 UTC 차이로 지역 날짜를 만든다"
    );
}

#[test]
#[ignore = "#274"]
fn preview_rules_checks_names_left_in_the_folder() {
    let dir = folder_with(&[("a.txt", NOV_15), ("b.txt", NOV_15), ("c.txt", NOV_15)]);
    let mut app = app_at(dir.path());
    select(&mut app, &["a.txt", "b.txt"]);

    let preview = app
        .batch_rename_preview_rules(&[replace("a", "c")], 0)
        .unwrap();
    let statuses: Vec<EntryStatus> = preview.entries().iter().map(|e| e.status).collect();
    assert_eq!(
        statuses,
        [
            EntryStatus::Problem(BatchProblem::NameTaken),
            EntryStatus::Unchanged
        ],
        "선택하지 않은 c.txt와 겹친다"
    );
    assert!(!preview.can_execute());
}

#[test]
#[ignore = "#274"]
fn preview_rules_matches_the_s14a_entry_point() {
    let dir = folder_with(&[
        ("사진 1.jpg", NOV_15),
        ("사진 2.jpg", NOV_15),
        ("메모.txt", NOV_15),
        ("휴가 1.jpg", NOV_15),
    ]);
    let mut app = app_at(dir.path());
    select(&mut app, &["사진 1.jpg", "사진 2.jpg", "메모.txt"]);

    let rules = [
        RenameRule {
            find: "사진".to_string(),
            replace: "휴가".to_string(),
            ..RenameRule::default()
        },
        RenameRule {
            prefix: "2026 ".to_string(),
            suffix: "!".to_string(),
            ..RenameRule::default()
        },
        RenameRule::default(),
    ];
    for rule in rules {
        let old = app.batch_rename_preview(&rule).unwrap();
        let new = app
            .batch_rename_preview_rules(&rule.to_rules(), 9 * HOUR)
            .unwrap();
        assert_eq!(new.entries(), old.entries(), "{rule:?}");
        assert_eq!(
            (
                new.rename_count(),
                new.unchanged_count(),
                new.problem_count()
            ),
            (
                old.rename_count(),
                old.unchanged_count(),
                old.problem_count()
            ),
            "{rule:?}"
        );
    }
}

#[test]
#[ignore = "#274"]
fn nothing_happens_when_the_command_is_off_or_blocked() {
    let dir = folder_with(&[("a.txt", NOV_15), ("b.txt", NOV_15), ("c.txt", NOV_15)]);
    let mut app = app_at(dir.path());

    select(&mut app, &["a.txt"]);
    assert!(
        app.batch_rename_preview_rules(&[date_first()], 0).is_none(),
        "한 항목만 고르면 명령이 꺼져 있다"
    );
    assert_eq!(
        app.start_batch_rename_rules(&[date_first()], 0),
        Err(BatchRenameError::Unavailable)
    );

    select(&mut app, &["a.txt", "b.txt"]);
    assert_eq!(
        app.start_batch_rename_rules(&[replace("a", "c")], 0),
        Err(BatchRenameError::Problems)
    );
    assert_eq!(
        app.start_batch_rename_rules(&[replace("zzz", "y")], 0),
        Err(BatchRenameError::NothingToRename)
    );
    let insert = Rule {
        enabled: false,
        ..Rule::new(RuleKind::Insert {
            text: "x".to_string(),
            from: Edge::Start,
            at: 0,
        })
    };
    assert_eq!(
        app.start_batch_rename_rules(&[insert], 0),
        Err(BatchRenameError::NothingToRename),
        "꺼진 규칙은 아무것도 바꾸지 않는다"
    );
    assert_eq!(app.history().records().len(), 0);

    let mut on_disk: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    on_disk.sort();
    assert_eq!(on_disk, ["a.txt", "b.txt", "c.txt"]);
}

#[cfg(windows)]
mod on_disk {
    use std::time::Instant;

    use facet_core::history::OperationKind;
    use facet_core::job::{JobId, JobKind, JobState, Summary};

    use super::*;

    fn names_on_disk(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// 작업이 끝날 때까지 작업 사건(과 탭 쪽 결과)을 받고 마지막 상태와 결과를 돌려준다.
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
    #[ignore = "#274"]
    fn start_rules_renames_on_disk_as_one_job_and_undoes_once() {
        let dir = folder_with(&[
            ("사진 1.jpeg", NOV_15),
            ("사진 2.jpeg", DEC_31),
            ("메모.txt", NOV_15),
        ]);
        let mut app = app_at(dir.path());
        select(&mut app, &["사진 1.jpeg", "사진 2.jpeg", "메모.txt"]);
        let rules = [
            date_first(),
            Rule::new(RuleKind::Extension {
                new: Some("jpg".to_string()),
            }),
        ];

        let job = app
            .start_batch_rename_rules(&rules, 0)
            .expect("일괄 이름 바꾸기를 시작한다");
        assert_eq!(
            app.work_area().entry(job).map(|entry| entry.kind),
            Some(JobKind::BatchRename)
        );
        let (state, summary) = wait_job(&mut app, job);
        assert_eq!(state, JobState::Finished);
        assert_eq!(summary.items_renamed, 3);
        assert_eq!(
            names_on_disk(dir.path()),
            [
                "2026-11-15 메모.jpg",
                "2026-11-15 사진 1.jpg",
                "2026-12-31 사진 2.jpg"
            ]
        );

        assert_eq!(app.history().records().len(), 1, "기록은 한 건");
        let record = app.history().undo_target().unwrap();
        assert_eq!(record.kind(), OperationKind::BatchRename);
        assert_eq!(record.items_completed(), 3);

        let job = app.undo().expect("실행취소를 시작한다");
        let (state, _) = wait_job(&mut app, job);
        assert_eq!(state, JobState::Finished);
        assert_eq!(
            names_on_disk(dir.path()),
            ["메모.txt", "사진 1.jpeg", "사진 2.jpeg"],
            "실행취소 한 번에 모두 돌아온다"
        );
    }
}
