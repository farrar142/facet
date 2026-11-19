// target: crates/facet-core/tests/spec_install_quit.rs
// AC: #472: 끝나지 않은 작업이 있으면 Busy이고 그대로, 작업이 없으면 Quitting과 창 닫힘·앱 끝남, 작업 취소를 함께 청하면 취소 뒤 끝남, Busy의 작업 수
//! 설치 프로그램이 청하는 끝내기(`App::request_quit`)를 확인한다. 끝나지 않은 작업은 이름 충돌로 답을 기다리는 복사
//! 작업으로 만든다(답할 때까지 끝나지 않아 흔들리지 않는다, `tests/progress_window.rs`와 같은 방법). 작업이 없는 경우는
//! 모든 플랫폼, 작업을 쓰는 경우는 Windows 전용이다.

use std::path::Path;
use std::time::Duration;

use facet_core::Location;
use facet_core::tab::ReadOutcome;
use facet_core::window::{App, QuitReply};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(10);

fn app_at(path: &Path) -> App {
    let mut app = App::new(Some(Location::file_system(path)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

#[test]
#[ignore = "#472"]
fn 작업이_없으면_창을_모두_닫고_끝난다() {
    let dir = TempDir::new().unwrap();
    let mut app = app_at(dir.path());
    app.open_window(Location::file_system(dir.path()));
    assert!(app.has_windows());

    assert_eq!(app.request_quit(false), QuitReply::Quitting);
    assert!(!app.has_windows(), "창이 모두 닫힌다");
    assert!(app.is_finished());
}

#[cfg(windows)]
mod with_jobs {
    use std::fs;
    use std::time::Instant;

    use facet_core::job::{JobId, JobKind, JobState};

    use super::*;

    /// `root/src/파일.txt`와 같은 이름의 파일이 이미 든 `root/dst`.
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

    /// `src`를 보는 앱에서 `파일.txt`를 `dst`로 복사하는 작업을 넣고 이름 충돌 질문이 올 때까지 기다린다.
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

    fn wait_finished(app: &mut App) {
        let deadline = Instant::now() + WAIT;
        while !app.is_finished() {
            assert!(Instant::now() < deadline, "앱이 끝나지 않았다");
            app.poll_jobs();
            app.wait_update(Duration::from_millis(50));
        }
        app.poll_jobs();
    }

    #[test]
    #[ignore = "#472"]
    fn 끝나지_않은_작업이_있으면_거절하고_아무것도_바꾸지_않는다() {
        let dir = conflicting_tree();
        let (mut app, job) = app_with_waiting_job(dir.path());
        let session = app.session();

        assert_eq!(app.request_quit(false), QuitReply::Busy { jobs: 1 });
        assert!(app.has_windows(), "창이 그대로다");
        assert_eq!(app.session(), session, "창·탭이 그대로다");
        assert_eq!(app.jobs().state(job), Some(JobState::AwaitingAnswer));
        assert!(app.work_area().entry(job).unwrap().question.is_some());
        assert!(!app.is_finished());
        assert_eq!(
            fs::read(dir.path().join("dst").join("파일.txt")).unwrap(),
            b"old target"
        );
    }

    #[test]
    #[ignore = "#472"]
    fn 작업_수는_끝나지_않은_작업_수다() {
        let dir = conflicting_tree();
        let (mut app, _first) = app_with_waiting_job(dir.path());
        // 같은 충돌을 하나 더 넣는다.
        let dst = dir.path().join("dst");
        let plan = app
            .transfer_plan_to(JobKind::Copy, &dst.to_string_lossy())
            .expect("대상 해석");
        app.start_transfer(&plan).expect("두 번째 작업");
        app.poll_jobs();

        assert_eq!(app.request_quit(false), QuitReply::Busy { jobs: 2 });
    }

    #[test]
    #[ignore = "#472"]
    fn 작업_취소를_함께_청하면_작업을_취소하고_끝난다() {
        let dir = conflicting_tree();
        let (mut app, job) = app_with_waiting_job(dir.path());

        assert_eq!(app.request_quit(true), QuitReply::Quitting);
        assert!(!app.has_windows(), "창이 모두 닫힌다");
        wait_finished(&mut app);
        assert_eq!(app.jobs().state(job), Some(JobState::Cancelled));
        assert_eq!(
            fs::read(dir.path().join("dst").join("파일.txt")).unwrap(),
            b"old target",
            "취소한 작업은 대상을 덮어쓰지 않았다"
        );
    }
}
