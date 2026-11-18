//! 도우미 미리보기(#343)의 경계와 수명: 미리보기 처리기 코드가 UI 프로세스 쪽 크레이트에서 불리지 않음, 미리보기를
//! 그리던 도우미가 밖에서 끝나면 「미리보기를 불러오지 못함」이 되고 다음 선택에 다시 뜸, 실제 모드 도우미가 처리기 없는
//! 형식에 「처리기 없음」으로 답함. 실제 처리기와 창 붙이기는 사람이 확인한다(`docs/preview-checklist.md`).

use std::fs;
use std::path::Path;

/// 미리보기 처리기 싣기(`facet_win::preview_handler`)는 셸 확장 도우미만 부른다: UI 프로세스(`facet`)를 이루는 크레이트의
/// 소스는 그 모듈을 부르지 않는다(부르지 않는 함수는 `facet` 실행 파일에 링크되지 않고 처리기 DLL이 실리지 않는다,
/// ADR-0006).
#[test]
fn ui_crates_do_not_call_the_preview_handler_module() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for name in ["facet-app", "facet-core"] {
        let mut stack = vec![crates.join(name).join("src")];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = fs::read_to_string(&path).unwrap();
                    assert!(
                        !["::preview_handler", "{preview_handler", ", preview_handler"]
                            .iter()
                            .any(|module| text.contains(module)),
                        "{}가 미리보기 처리기 모듈을 부른다",
                        path.display()
                    );
                }
            }
        }
    }
}

#[cfg(windows)]
mod helper {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use facet_core::Location;
    use facet_core::command::CommandId;
    use facet_core::preview::{
        PreviewBody, PreviewContent, PreviewHost, PreviewProblem, ShellPreview,
    };
    use facet_core::shell_helper::HelperOptions;
    use facet_core::tab::ReadOutcome;
    use facet_core::window::{App, Execution};
    use tempfile::TempDir;

    const WAIT: Duration = Duration::from_secs(20);

    const HOST: PreviewHost = PreviewHost {
        parent: 0,
        x: 100,
        y: 100,
        width: 400,
        height: 300,
    };

    fn options(fake: bool) -> HelperOptions {
        let mut options =
            HelperOptions::new(PathBuf::from(env!("CARGO_BIN_EXE_facet-shell-helper")));
        if fake {
            options.args = vec!["--fake".into()];
        }
        options.crash_log = None;
        options
    }

    fn app_at(path: &Path, fake: bool) -> App {
        let mut app = App::new(Some(Location::file_system(path)));
        let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
        assert_eq!(outcome, ReadOutcome::Committed);
        app.enable_shell_helper(options(fake));
        app.set_preview_handler_lookup(|_| true);
        assert_eq!(app.execute(CommandId::TogglePreview), Execution::Done);
        let window = app.active_window_id();
        app.set_preview_host(window, Some(HOST));
        app
    }

    fn select(app: &mut App, name: &str) {
        let index = app
            .active_tab()
            .items()
            .position(|item| item.name == name)
            .unwrap_or_else(|| panic!("{name}이 목록에 없다"));
        app.active_tab_mut().select_only(index);
        app.sync_preview();
    }

    fn wait_body(app: &mut App, name: &str, wanted: impl Fn(&PreviewBody) -> bool) {
        let window = app.active_window_id();
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(PreviewContent::Item { item, body, .. }) = app.preview(window)
                && item.name == name
                && wanted(body)
            {
                return;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                !left.is_zero(),
                "{name}의 미리보기가 오지 않았다: {:?}",
                app.preview(window)
            );
            app.wait_update(left.min(Duration::from_millis(200)));
        }
    }

    fn showing(body: &PreviewBody) -> bool {
        *body == PreviewBody::Shell(ShellPreview::Showing)
    }

    fn helper_pid(app: &App) -> u32 {
        app.shell_helper()
            .and_then(|helper| helper.process_id())
            .expect("도우미를 띄웠다")
    }

    #[test]
    fn killing_the_helper_while_showing_fails_the_preview_and_the_next_selection_shows_again() {
        let dir = TempDir::new().unwrap();
        for name in ["a.fakeprev", "b.fakeprev"] {
            fs::write(dir.path().join(name), b"preview me").unwrap();
        }
        let mut app = app_at(dir.path(), true);
        select(&mut app, "a.fakeprev");
        wait_body(&mut app, "a.fakeprev", showing);
        let pid = helper_pid(&app);

        // 작업 관리자로 끝내는 것과 같다.
        let status = std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output()
            .expect("taskkill");
        assert!(status.status.success(), "{status:?}");

        wait_body(&mut app, "a.fakeprev", |body| {
            *body == PreviewBody::Unavailable(PreviewProblem::ShellFailed)
        });

        select(&mut app, "b.fakeprev");
        wait_body(&mut app, "b.fakeprev", showing);
        assert_ne!(helper_pid(&app), pid, "새 도우미 프로세스");
    }

    #[test]
    fn the_real_helper_answers_no_handler_for_an_unregistered_type() {
        let dir = TempDir::new().unwrap();
        let name = "doc.facetnopreviewhandler";
        fs::write(dir.path().join(name), b"nothing").unwrap();
        let mut app = app_at(dir.path(), false);
        select(&mut app, name);
        wait_body(&mut app, name, |body| *body == PreviewBody::Info);
    }
}
