//! 마지막 창을 닫아도 작업은 끝까지 돈다: 창이 없고 끝나지 않은 작업이 있으면 앱이 끝나지 않고 진행 창을 띄워야
//! 하며([`App::shows_progress_window`]), 작업이 모두 끝나면 끝난다([`App::is_finished`]). 진행 창만 있는 동안에도
//! 새 창·실행 인자로 창 열기가 되고, 세션은 마지막 보통 창 상태를 남긴다.
//!
//! 끝나지 않은 작업은 이름 충돌로 답을 기다리게 해 만든다(답할 때까지 끝나지 않으므로 흔들리지 않는다). 실제 복사가
//! 필요해 작업을 쓰는 테스트는 Windows 전용이다.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::command::CommandId;
use facet_core::tab::ReadOutcome;
use facet_core::window::{App, Execution};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(10);

fn loc(path: &Path) -> Location {
    Location::file_system(path)
}

/// `left`를 연 앱. 첫 읽기가 끝날 때까지 기다린다.
fn app_at(left: &Path) -> App {
    let mut app = App::new(Some(loc(left)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

#[test]
fn 작업이_없으면_마지막_창과_함께_끝나고_진행_창도_없다() {
    let dir = TempDir::new().unwrap();
    let mut app = app_at(dir.path());
    assert!(app.has_windows());
    assert!(!app.shows_progress_window());

    assert_eq!(app.execute(CommandId::CloseTab), Execution::WindowClosed);
    assert!(!app.has_windows());
    assert!(!app.jobs().has_unfinished());
    assert!(app.is_finished());
    assert!(!app.shows_progress_window());
    assert_eq!(app.session(), None);
}

#[cfg(windows)]
mod with_jobs {
    use super::*;
    use facet_core::job::{ConflictAnswer, JobId, JobKind, Resolution};
    use facet_core::launch::resolve_launch;
    use facet_core::session::Session;

    /// `root/src/파일.txt`(원본)와 같은 이름의 파일이 이미 든 `root/dst`.
    fn conflicting_tree() -> TempDir {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::create_dir(&src).unwrap();
        fs::create_dir(&dst).unwrap();
        fs::write(src.join("파일.txt"), b"new").unwrap();
        fs::write(dst.join("파일.txt"), b"old target").unwrap();
        dir
    }

    /// `src`를 보는 앱에서 `파일.txt`를 `dst`로 복사하는 작업을 넣고, 이름 충돌 질문이 올 때까지 기다린다.
    fn app_with_waiting_job(dir: &Path) -> (App, JobId) {
        let src = dir.join("src");
        let dst = dir.join("dst");
        let mut app = app_at(&src);
        let file = app
            .active_tab()
            .items()
            .position(|item| item.name == "파일.txt")
            .expect("원본 파일");
        app.active_tab_mut().select_only(file);
        let plan = app
            .transfer_plan_to(JobKind::Copy, &dst.to_string_lossy())
            .expect("대상 해석");
        let job = app.start_transfer(&plan).expect("작업을 넣는다");
        let deadline = Instant::now() + WAIT;
        while app
            .work_area()
            .entry(job)
            .is_none_or(|e| e.question.is_none())
        {
            assert!(Instant::now() < deadline, "질문이 오지 않았다");
            app.poll_jobs();
            app.wait_update(Duration::from_millis(50));
        }
        (app, job)
    }

    /// 앱이 끝날 때까지 작업 사건을 받는다.
    fn wait_finished(app: &mut App) {
        let deadline = Instant::now() + WAIT;
        while !app.is_finished() {
            assert!(Instant::now() < deadline, "작업이 끝나지 않았다");
            app.poll_jobs();
            app.wait_update(Duration::from_millis(50));
        }
        app.poll_jobs();
    }

    #[test]
    fn 창이_없어도_끝나지_않은_작업이_있으면_끝나지_않고_끝나면_끝난다() {
        let dir = conflicting_tree();
        let (mut app, job) = app_with_waiting_job(dir.path());

        // 마지막 탭을 닫아 마지막 창이 닫혔다.
        assert_eq!(app.execute(CommandId::CloseTab), Execution::WindowClosed);
        assert!(!app.has_windows());
        assert!(!app.is_finished(), "답을 기다리는 작업이 있다");
        assert!(app.shows_progress_window());
        // 창이 없어도 질문에 답할 수 있다(진행 창에서 답함).
        assert!(app.work_area().entry(job).unwrap().question.is_some());
        assert_eq!(
            app.answer_conflict(job, ConflictAnswer::once(Resolution::Overwrite)),
            Ok(())
        );

        wait_finished(&mut app);
        assert!(!app.shows_progress_window());
        assert!(app.work_area().entry(job).unwrap().is_finished());
        assert_eq!(
            fs::read(dir.path().join("dst").join("파일.txt")).unwrap(),
            b"new"
        );
    }

    #[test]
    fn 닫기_단추로_마지막_창을_닫아도_취소한_작업이_끝나면_끝난다() {
        let dir = conflicting_tree();
        let (mut app, job) = app_with_waiting_job(dir.path());

        let window = app.active_window_id();
        assert!(app.close_window(window));
        assert!(!app.is_finished());
        assert!(app.shows_progress_window());

        // 진행 창의 취소 단추.
        assert!(app.jobs_mut().cancel(job));
        wait_finished(&mut app);
        assert!(!app.shows_progress_window());
        assert_eq!(
            fs::read(dir.path().join("dst").join("파일.txt")).unwrap(),
            b"old target"
        );
    }

    #[test]
    fn 진행_창만_있을_때_새_창_명령으로_보통_창을_연다() {
        let dir = conflicting_tree();
        let (mut app, job) = app_with_waiting_job(dir.path());
        assert_eq!(app.execute(CommandId::CloseTab), Execution::WindowClosed);
        assert!(app.shows_progress_window());

        assert!(app.can_execute(CommandId::NewWindow));
        assert_eq!(app.execute(CommandId::NewWindow), Execution::Done);
        assert!(app.has_windows());
        assert_eq!(app.windows().len(), 1);
        // 보통 창이 다시 생기면 진행 창은 필요 없다. 작업은 그대로 돈다.
        assert!(!app.shows_progress_window());
        assert!(app.work_area().entry(job).unwrap().question.is_some());

        assert!(app.jobs_mut().cancel(job));
        let deadline = Instant::now() + WAIT;
        while app.jobs().has_unfinished() {
            assert!(Instant::now() < deadline, "작업이 끝나지 않았다");
            app.poll_jobs();
            app.wait_update(Duration::from_millis(50));
        }
        // 보통 창이 있으니 작업이 끝나도 앱은 끝나지 않는다.
        assert!(!app.is_finished());
    }

    #[test]
    fn 진행_창만_있을_때_실행_인자로_창을_연다() {
        let dir = conflicting_tree();
        let (mut app, job) = app_with_waiting_job(dir.path());
        assert_eq!(app.execute(CommandId::CloseTab), Execution::WindowClosed);
        assert!(app.shows_progress_window());

        let dst = dir.path().join("dst");
        let opened = app.open_launch_window(resolve_launch(dir.path(), &[dst.clone()]));
        assert_eq!(app.active_window_id(), opened);
        assert_eq!(app.active_tab().location(), &loc(&dst));
        assert!(!app.shows_progress_window());
        assert!(!app.is_finished());

        assert!(app.jobs_mut().cancel(job));
    }

    #[test]
    fn 진행_창이_떠_있는_동안_세션은_마지막_보통_창_상태다() {
        let dir = conflicting_tree();
        let (mut app, job) = app_with_waiting_job(dir.path());
        let before: Session = app.session().expect("보통 창이 있다");
        assert_eq!(before.windows.len(), 1);

        assert_eq!(app.execute(CommandId::CloseTab), Execution::WindowClosed);
        assert!(app.shows_progress_window());
        // 창이 없으면 세션을 내지 않는다: 저장기(`Saver::update`)는 `None`을 넘기지 않으므로 파일은 마지막 보통 창
        // 상태로 남는다.
        assert_eq!(app.session(), None);
        // 그 세션 파일을 되살리면 마지막 보통 창이 돌아온다.
        let restored = App::restore(Some(before.clone()), Vec::new());
        assert_eq!(restored.session(), Some(before));

        assert!(app.jobs_mut().cancel(job));
        wait_finished(&mut app);
    }
}
