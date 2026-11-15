//! 반대편 패널로 복사·이동: 명령과 기본 단축키(F7·F8), 실행 가능 여부, 원본·대상 고르기, 확인 줄에 보일 대상·항목
//! 수, 확인 뒤 작업 대기열에 넣기와 작업 영역, 작업이 끝난 뒤 변경 감시로 목록이 갱신되는지를 공개 인터페이스로
//! 확인한다.
//!
//! 앱([`App`])은 일꾼 스레드에서 읽고 작업도 일꾼 스레드에서 돌므로 결과를 기다린다. CI 러너가 느려도 흔들리지 않도록
//! 기다리는 시간([`WAIT`])을 넉넉히 두고, 조건이 맞는 즉시 끝낸다.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::command::{CommandId, Key, Shortcut, find_by_shortcut};
use facet_core::job::{CopyRejection, JobKind};
use facet_core::tab::{Navigation, ReadOutcome, Tab};
use facet_core::window::{App, Execution, TransferPlan};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(10);

fn loc(path: &Path) -> Location {
    Location::file_system(path)
}

/// `root/src/a/a1`, `root/src/b`, `root/src/파일.txt`, 빈 `root/dst`가 있는 임시 폴더.
fn tree() -> TempDir {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(src.join("a").join("a1")).unwrap();
    fs::write(src.join("a").join("안.txt"), b"in a").unwrap();
    fs::create_dir(src.join("b")).unwrap();
    fs::write(src.join("파일.txt"), b"x").unwrap();
    fs::create_dir(dir.path().join("dst")).unwrap();
    dir
}

/// 기다리는 읽기가 없어질 때까지 앱이 받은 것을 탭에 넘긴다.
fn settle(app: &mut App) {
    let deadline = Instant::now() + WAIT;
    while app
        .windows()
        .iter()
        .flat_map(|w| w.panels())
        .flat_map(|p| p.tabs())
        .any(|(_, tab)| tab.pending_location().is_some())
    {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "읽기가 시간 안에 끝나지 않았다");
        app.wait_update(left);
    }
}

/// `left`를 연 앱. 첫 읽기가 끝날 때까지 기다린다.
fn app_at(left: &Path) -> App {
    let mut app = App::new(Some(loc(left)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

/// 왼쪽 패널이 `left`, 오른쪽 패널이 `right`를 보이고 왼쪽 패널이 활성인 앱.
fn two_panels(left: &Path, right: &Path) -> App {
    let mut app = app_at(left);
    app.split_panel().expect("나누기");
    settle(&mut app);
    assert!(app.navigate(Navigation::Go(loc(right))));
    settle(&mut app);
    assert!(app.activate_panel(0));
    app
}

/// 활성 탭에서 이름이 `name`인 항목의 번호.
fn index_of(tab: &Tab, name: &str) -> usize {
    tab.items()
        .position(|item| item.name == name)
        .unwrap_or_else(|| panic!("{name}이 목록에 없다"))
}

/// 오른쪽 패널(반대편)의 보이는 탭.
fn right_tab(app: &App) -> &Tab {
    app.active_window().panels()[1].active_tab()
}

/// 왼쪽 패널의 보이는 탭.
fn left_tab(app: &App) -> &Tab {
    app.active_window().panels()[0].active_tab()
}

#[test]
fn 반대편_패널로_복사_이동_명령과_기본_단축키는_f7_f8이다() {
    use CommandId::*;
    assert!(CommandId::ALL.contains(&CopyToOtherPanel));
    assert!(CommandId::ALL.contains(&MoveToOtherPanel));
    assert_eq!(
        CopyToOtherPanel.shortcuts(),
        &[Shortcut::new(Key::Function(7))]
    );
    assert_eq!(
        MoveToOtherPanel.shortcuts(),
        &[Shortcut::new(Key::Function(8))]
    );
    assert_eq!(
        find_by_shortcut(Shortcut::new(Key::Function(7))),
        Some(CopyToOtherPanel)
    );
    assert_eq!(
        find_by_shortcut(Shortcut::new(Key::Function(8))),
        Some(MoveToOtherPanel)
    );
    // 수식 키가 붙으면 다른 조합이다.
    assert_eq!(
        find_by_shortcut(Shortcut::new(Key::Function(7)).shift()),
        None
    );
    assert_eq!(CopyToOtherPanel.as_str(), "copy-to-other-panel");
    assert_eq!(MoveToOtherPanel.as_str(), "move-to-other-panel");
    assert_eq!(
        CommandId::from_id("move-to-other-panel"),
        Some(MoveToOtherPanel)
    );
    assert_eq!(CopyToOtherPanel.transfer_kind(), Some(JobKind::Copy));
    assert_eq!(MoveToOtherPanel.transfer_kind(), Some(JobKind::Move));
    assert_eq!(Refresh.transfer_kind(), None);
}

#[test]
fn 패널이_하나이거나_원본이_없으면_꺼진다() {
    let dir = tree();
    let src = dir.path().join("src");
    let mut app = app_at(&src);
    let commands = [CommandId::CopyToOtherPanel, CommandId::MoveToOtherPanel];

    // 패널 하나: 선택이 있어도 꺼진다.
    let index = index_of(app.active_tab(), "파일.txt");
    app.active_tab_mut().select_only(index);
    for command in commands {
        assert!(!app.can_execute(command), "{command:?}");
        assert_eq!(app.execute(command), Execution::Disabled);
    }
    assert_eq!(app.transfer_plan(JobKind::Copy), None);

    // 패널 둘: 새 오른쪽 패널(활성)에는 선택도 커서도 없어 꺼진다.
    app.split_panel().expect("나누기");
    settle(&mut app);
    assert_eq!(app.active_window().active_panel_index(), 1);
    assert!(app.active_tab().cursor().is_none());
    for command in commands {
        assert!(!app.can_execute(command), "{command:?}");
        assert_eq!(app.execute(command), Execution::Disabled);
    }
    assert_eq!(app.transfer_plan(JobKind::Move), None);
    // 팔레트에도 꺼진 채로 보인다.
    let found = app.search_commands("copy", |c| c.as_str());
    let entry = found
        .iter()
        .find(|e| e.command == CommandId::CopyToOtherPanel)
        .expect("팔레트에 있다");
    assert!(!entry.enabled);

    // 선택이 있는 왼쪽 패널이 활성이면 켜진다.
    assert!(app.activate_panel(0));
    for command in commands {
        assert!(app.can_execute(command), "{command:?}");
    }
    let found = app.search_commands("copy", |c| c.as_str());
    assert!(
        found
            .iter()
            .any(|e| e.command == CommandId::CopyToOtherPanel && e.enabled)
    );

    // 선택을 풀고 커서도 없는 탭(빈 폴더)이면 꺼진다.
    let empty = dir.path().join("dst");
    assert!(app.navigate(Navigation::Go(loc(&empty))));
    settle(&mut app);
    assert!(app.active_tab().is_empty());
    for command in commands {
        assert!(!app.can_execute(command), "{command:?}");
    }

    // 탭 하나만 보아도 원본이 없으면 꺼진다(패널 수는 창이 본다).
    assert!(!CommandId::CopyToOtherPanel.is_enabled(app.active_tab()));
}

#[test]
fn 원본은_선택이고_없으면_커서_항목이며_대상은_반대편_패널의_위치다() {
    let dir = tree();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    let mut app = two_panels(&src, &dst);

    // 선택한 둘: 목록 순서(이름 오름차순: 폴더 a, b 다음 파일)로.
    let a = index_of(app.active_tab(), "a");
    let file = index_of(app.active_tab(), "파일.txt");
    app.active_tab_mut().select_only(file);
    app.active_tab_mut().toggle_select(a);
    assert_eq!(
        app.execute(CommandId::CopyToOtherPanel),
        Execution::ConfirmTransfer(JobKind::Copy)
    );
    let plan = app.transfer_plan(JobKind::Copy).expect("복사할 내용");
    assert_eq!(
        plan,
        TransferPlan {
            kind: JobKind::Copy,
            sources: vec![src.join("a"), src.join("파일.txt")],
            target: dst.clone(),
        }
    );
    // 확인 줄에 보일 대상과 항목 수.
    assert_eq!(plan.items(), 2);
    assert_eq!(plan.target, dst);

    // 선택이 없으면 커서 항목 하나.
    let b = index_of(app.active_tab(), "b");
    app.active_tab_mut().select_only(b);
    app.active_tab_mut().toggle_cursor_selection();
    assert_eq!(app.active_tab().selected_count(), 0);
    assert_eq!(app.active_tab().cursor(), Some(b));
    assert_eq!(
        app.execute(CommandId::MoveToOtherPanel),
        Execution::ConfirmTransfer(JobKind::Move)
    );
    let plan = app.transfer_plan(JobKind::Move).expect("옮길 내용");
    assert_eq!(plan.kind, JobKind::Move);
    assert_eq!(plan.sources, vec![src.join("b")]);
    assert_eq!(plan.items(), 1);
    assert_eq!(plan.target, dst);

    // 명령을 실행해도 확인 전에는 아무것도 넣지 않는다.
    assert!(app.work_area().is_empty());
    assert!(!app.jobs().has_unfinished());

    // 오른쪽 패널을 활성으로 하면 원본과 대상이 뒤바뀐다(오른쪽 빈 폴더라 원본이 없다).
    assert!(app.activate_panel(1));
    assert_eq!(app.transfer_plan(JobKind::Copy), None);
}

#[test]
fn 시작_전에_막히면_작업을_만들지_않고_까닭을_준다() {
    let dir = tree();
    let src = dir.path().join("src");
    // 대상(오른쪽)이 원본 폴더 a 안이다.
    let inside = src.join("a").join("a1");
    let mut app = two_panels(&src, &inside);
    let a = index_of(app.active_tab(), "a");
    app.active_tab_mut().select_only(a);
    let plan = app.transfer_plan(JobKind::Copy).expect("복사할 내용");
    assert_eq!(
        app.start_transfer(&plan),
        Err(CopyRejection::TargetInsideSource(src.join("a")))
    );
    assert!(app.work_area().is_empty());
    assert!(!app.jobs().has_unfinished());

    // 대상 위치가 사라졌다.
    let plan = TransferPlan {
        kind: JobKind::Move,
        sources: vec![src.join("파일.txt")],
        target: dir.path().join("없는 폴더"),
    };
    assert_eq!(app.start_transfer(&plan), Err(CopyRejection::TargetMissing));
    assert!(app.work_area().is_empty());
}

/// 작업 영역의 작업이 모두 끝나고 `done`이 참이 될 때까지 앱이 받은 것(읽기·변경·작업 사건)을 넘긴다.
#[cfg(windows)]
fn wait_until(app: &mut App, mut done: impl FnMut(&App) -> bool) {
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll_jobs();
        let finished = app.work_area().entries().iter().all(|e| e.is_finished());
        if finished && done(app) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "시간 안에 끝나지 않았다: {:?}",
            app.work_area()
        );
        app.wait_update(Duration::from_millis(50));
    }
}

#[cfg(windows)]
fn names(tab: &Tab) -> Vec<String> {
    tab.items()
        .map(|item| item.name.to_string_lossy().into_owned())
        .collect()
}

#[cfg(windows)]
#[test]
fn 확인하면_복사가_돌고_반대편_목록은_변경_감시로_갱신된다() {
    use facet_core::job::JobState;

    let dir = tree();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    let mut app = two_panels(&src, &dst);
    assert!(right_tab(&app).is_empty());

    let a = index_of(app.active_tab(), "a");
    let file = index_of(app.active_tab(), "파일.txt");
    app.active_tab_mut().select_only(a);
    app.active_tab_mut().toggle_select(file);
    let plan = app.transfer_plan(JobKind::Copy).expect("복사할 내용");
    let job = app.start_transfer(&plan).expect("작업을 넣는다");

    // 작업 영역에 곧바로 보인다.
    let entry = app.work_area().entry(job).expect("작업 영역에 있다");
    assert_eq!(entry.kind, JobKind::Copy);
    assert_eq!(entry.target, dst);
    assert_eq!(entry.sources, 2);
    assert!(!entry.is_finished());
    // 끝나지 않은 작업은 닫을 수 없다.
    assert!(!app.dismiss_job(job));

    // 오른쪽 탭은 다시 읽지 않고 변경 감시만으로 새 항목을 보인다.
    wait_until(&mut app, |app| {
        names(right_tab(app)) == ["a".to_owned(), "파일.txt".to_owned()]
    });
    assert!(dst.join("a").join("안.txt").is_file());
    let entry = app.work_area().entry(job).unwrap();
    assert_eq!(entry.state, JobState::Finished);
    let progress = entry.progress.as_ref().expect("진행이 왔다");
    assert_eq!(progress.items_done, progress.items_total);
    assert!(progress.counted);
    let summary = entry.summary.as_ref().expect("결과");
    assert_eq!(summary.items_remaining, 0);
    assert_eq!(summary.errors_skipped(), 0);
    // 원본은 그대로다.
    assert_eq!(names(left_tab(&app)), ["a", "b", "파일.txt"]);

    // 끝난 작업은 닫는다.
    assert!(app.dismiss_job(job));
    assert!(app.work_area().is_empty());
    assert!(!app.dismiss_job(job));
}

#[cfg(windows)]
#[test]
fn 확인하면_이동이_돌고_두_패널의_목록이_변경_감시로_갱신된다() {
    let dir = tree();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    let mut app = two_panels(&src, &dst);

    let b = index_of(app.active_tab(), "b");
    app.active_tab_mut().select_only(b);
    // 확인 줄 없이 실행만 하면 아무 일도 없다(Esc와 같다).
    assert_eq!(
        app.execute(CommandId::MoveToOtherPanel),
        Execution::ConfirmTransfer(JobKind::Move)
    );
    assert!(app.work_area().is_empty());
    assert!(src.join("b").is_dir());

    let plan = app.transfer_plan(JobKind::Move).expect("옮길 내용");
    let job = app.start_transfer(&plan).expect("작업을 넣는다");
    assert_eq!(app.work_area().entry(job).unwrap().kind, JobKind::Move);
    wait_until(&mut app, |app| {
        names(right_tab(app)) == ["b".to_owned()]
            && names(left_tab(app)) == ["a".to_owned(), "파일.txt".to_owned()]
    });
    assert!(dst.join("b").is_dir());
    assert!(!src.join("b").exists());
    let summary = app
        .work_area()
        .entry(job)
        .and_then(|e| e.summary.clone())
        .unwrap();
    assert_eq!(summary.items_renamed, 1);
}

/// 작업 영역은 앱 전체에 하나다: 다른 창이 활성이어도 같은 작업이 보인다.
#[cfg(windows)]
#[test]
fn 작업_영역은_모든_창에_하나다() {
    let dir = tree();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    let mut app = two_panels(&src, &dst);
    let file = index_of(app.active_tab(), "파일.txt");
    app.active_tab_mut().select_only(file);
    let plan = app.transfer_plan(JobKind::Copy).unwrap();
    let job = app.start_transfer(&plan).unwrap();
    let other = app.open_window(loc(&dst));
    settle(&mut app);
    assert_eq!(app.active_window_id(), other);
    assert!(app.work_area().entry(job).is_some());
    wait_until(&mut app, |_| true);
    assert!(dst.join("파일.txt").is_file());
}

#[test]
fn 결과에서_끝낸_항목과_건너뛴_항목을_센다() {
    use facet_core::job::{FileOpErrorKind, ItemFailure, Summary};
    let summary = Summary {
        items_done: 5,
        conflicts_skipped: 1,
        failures: vec![ItemFailure {
            source: "잠긴.txt".into(),
            kind: FileOpErrorKind::Locked,
        }],
        items_remaining: 2,
        ..Summary::default()
    };
    assert_eq!(summary.items_skipped(), 2);
    assert_eq!(summary.items_completed(), 3);
    assert_eq!(Summary::default().items_completed(), 0);
}
