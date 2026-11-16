//! 복사(Ctrl+C)·붙여넣기(Ctrl+V)·경로 복사(Ctrl+Shift+C)를 공개 인터페이스로 확인한다.
//!
//! - 명령의 단축키(한글 모드 포함)·팔레트 검색·켜짐/꺼짐(원본 없음, 클립보드에 파일 없음, 실제 폴더 아님)
//! - 붙여넣기 대상(폴더 하나 선택 → 그 안, 그 밖 → 현재 폴더), 경로 복사 글자
//! - 클립보드를 흉내 낸 입력(`ClipboardFiles`)으로 붙여넣은 작업이 반대편 패널 복사·이동(S3·S4)과 같은 길인지
//!   (같은 폴더 「- 복사본」, 잘라내기 붙여넣기 = 이동, 이동 뒤 실행취소로 원래대로) — Windows 전용
//! - 실제 클립보드로 복사 → 붙여넣기 왕복(Windows 전용, 열 수 없으면 건너뛰었다고 출력)

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::command::{CommandId, Key, Shortcut, find_by_shortcut};
use facet_core::tab::{Navigation, ReadOutcome, Tab};
use facet_core::window::{App, ClipboardDone, ClipboardFailure, Execution, Update};
use facet_win::clipboard::format::ClipboardOperation;
use facet_win::clipboard::{ClipboardError, ClipboardFiles};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(20);

fn loc(path: &Path) -> Location {
    Location::file_system(path)
}

/// `root/src/a/안.txt`, `root/src/b`, `root/src/파일.txt`, 빈 `root/dst`가 있는 임시 폴더.
fn tree() -> TempDir {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(src.join("a")).unwrap();
    fs::write(src.join("a").join("안.txt"), b"in a").unwrap();
    fs::create_dir(src.join("b")).unwrap();
    fs::write(src.join("파일.txt"), b"x").unwrap();
    fs::create_dir(dir.path().join("dst")).unwrap();
    dir
}

fn app_at(dir: &Path) -> App {
    let mut app = App::new(Some(loc(dir)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

fn index_of(tab: &Tab, name: &str) -> usize {
    tab.items()
        .position(|item| item.name == name)
        .unwrap_or_else(|| panic!("{name}이 목록에 없다"))
}

fn files(paths: &[PathBuf], operation: ClipboardOperation) -> ClipboardFiles {
    ClipboardFiles {
        paths: paths.to_vec(),
        operation,
        sequence: 7,
    }
}

#[test]
fn 단축키는_ctrl_c_ctrl_v_ctrl_shift_c이고_한글_모드에서도_같다() {
    use CommandId::*;
    let key = |c: char| Key::from_char(c).unwrap();
    assert_eq!(Copy.shortcuts(), &[Shortcut::new(key('c')).ctrl()]);
    assert_eq!(Paste.shortcuts(), &[Shortcut::new(key('v')).ctrl()]);
    assert_eq!(
        CopyPath.shortcuts(),
        &[Shortcut::new(key('c')).ctrl().shift()]
    );
    // C 키는 「ㅊ」, V 키는 「ㅍ」을 낸다.
    assert_eq!(
        find_by_shortcut(Shortcut::new(key('ㅊ')).ctrl()),
        Some(Copy)
    );
    assert_eq!(
        find_by_shortcut(Shortcut::new(key('ㅍ')).ctrl()),
        Some(Paste)
    );
    assert_eq!(
        find_by_shortcut(Shortcut::new(key('ㅊ')).ctrl().shift()),
        Some(CopyPath)
    );
    assert_eq!(Copy.as_str(), "copy");
    assert_eq!(Paste.as_str(), "paste");
    assert_eq!(CopyPath.as_str(), "copy-path");
    assert_eq!(CommandId::from_id("copy-path"), Some(CopyPath));
}

fn name(command: CommandId) -> &'static str {
    match command {
        CommandId::Copy => "복사",
        CommandId::Cut => "잘라내기",
        CommandId::Paste => "붙여넣기",
        CommandId::CopyPath => "경로 복사",
        other => other.as_str(),
    }
}

#[test]
fn 팔레트에_뜨고_꺼짐_켜짐이_상태를_따른다() {
    let dir = tree();
    let src = dir.path().join("src");
    let mut app = app_at(&src);

    let state = |app: &App, command: CommandId| {
        let found = app.search_commands("", name);
        found
            .iter()
            .find(|entry| entry.command == command)
            .unwrap_or_else(|| panic!("{command:?}이 팔레트에 없다"))
            .enabled
    };

    // 선택도 커서도 없으면 복사는 꺼지고, 경로 복사는 현재 폴더 때문에 켜진다. 붙여넣기는 클립보드에 파일이 없어 꺼진다.
    assert!(app.active_tab().cursor().is_none());
    assert!(!app.can_execute(CommandId::Copy));
    assert!(app.can_execute(CommandId::CopyPath));
    assert!(!app.can_execute(CommandId::Paste));
    assert!(!state(&app, CommandId::Copy));
    assert!(state(&app, CommandId::CopyPath));
    assert!(!state(&app, CommandId::Paste));
    assert_eq!(app.execute(CommandId::Copy), Execution::Disabled);
    assert_eq!(app.execute(CommandId::Paste), Execution::Disabled);
    // 팔레트 검색.
    let found = app.search_commands("붙여", name);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].command, CommandId::Paste);

    // 원본이 생기면 복사가 켜진다.
    let file = index_of(app.active_tab(), "파일.txt");
    app.active_tab_mut().select_only(file);
    assert!(app.can_execute(CommandId::Copy));
    assert!(state(&app, CommandId::Copy));

    // 클립보드 변경 알림이 「파일이 있음」을 알리면 붙여넣기가 켜지고, 없어지면 꺼진다.
    assert!(!app.clipboard_has_files());
    app.set_clipboard_files(Some(files(
        &[src.join("파일.txt")],
        ClipboardOperation::Copy,
    )));
    assert!(app.clipboard_has_files());
    assert!(app.can_execute(CommandId::Paste));
    assert!(state(&app, CommandId::Paste));
    app.set_clipboard_files(None);
    assert!(!app.can_execute(CommandId::Paste));

    // 탭 하나만 보아도: 위치가 실제 폴더라야 붙여넣기·경로 복사가 켜진다.
    assert!(CommandId::Paste.is_enabled(app.active_tab()));
    assert!(CommandId::CopyPath.is_enabled(app.active_tab()));
}

#[test]
fn 붙여넣기_대상은_폴더_하나를_고르면_그_안이고_아니면_현재_폴더다() {
    let dir = tree();
    let src = dir.path().join("src");
    let mut app = app_at(&src);

    // 선택이 없다: 현재 폴더.
    assert_eq!(app.paste_target(), Some(src.clone()));
    // 폴더 하나를 고름: 그 안.
    let a = index_of(app.active_tab(), "a");
    app.active_tab_mut().select_only(a);
    assert_eq!(app.paste_target(), Some(src.join("a")));
    // 파일 하나를 고름: 현재 폴더.
    let file = index_of(app.active_tab(), "파일.txt");
    app.active_tab_mut().select_only(file);
    assert_eq!(app.paste_target(), Some(src.clone()));
    // 폴더 둘: 현재 폴더.
    let a = index_of(app.active_tab(), "a");
    let b = index_of(app.active_tab(), "b");
    app.active_tab_mut().select_only(a);
    app.active_tab_mut().toggle_select(b);
    assert_eq!(app.paste_target(), Some(src.clone()));
    // 폴더와 파일: 현재 폴더.
    let file = index_of(app.active_tab(), "파일.txt");
    app.active_tab_mut().select_only(b);
    app.active_tab_mut().toggle_select(file);
    assert_eq!(app.paste_target(), Some(src));
}

#[test]
fn 경로_복사_글자는_선택한_항목의_전체_경로이고_선택이_없으면_현재_폴더다() {
    let dir = tree();
    let src = dir.path().join("src");
    let mut app = app_at(&src);

    // 선택이 없으면(커서만 있어도) 현재 폴더 경로 하나.
    assert_eq!(app.copy_path_text(), Some(format!("\"{}\"", src.display())));
    app.active_tab_mut()
        .move_cursor(facet_core::selection::CursorMove::First);
    assert_eq!(app.copy_path_text(), Some(format!("\"{}\"", src.display())));

    // 선택이 있으면 목록 순서로 줄마다 하나, 큰따옴표로 감싼다.
    let a = index_of(app.active_tab(), "a");
    let file = index_of(app.active_tab(), "파일.txt");
    app.active_tab_mut().select_only(file);
    app.active_tab_mut().toggle_select(a);
    let expected = format!(
        "\"{}\"\r\n\"{}\"",
        src.join("a").display(),
        src.join("파일.txt").display()
    );
    assert_eq!(app.copy_path_text(), Some(expected));
}

/// 클립보드 스레드가 없는 앱: 복사·붙여넣기는 상태 줄에 보일 까닭으로 실패하고 앱은 계속 돈다.
#[test]
fn 클립보드_스레드가_없으면_까닭을_알리고_앱은_계속_돈다() {
    let dir = tree();
    let src = dir.path().join("src");
    let mut app = app_at(&src);
    let file = index_of(app.active_tab(), "파일.txt");
    app.active_tab_mut().select_only(file);
    app.set_clipboard_files(Some(files(
        &[src.join("파일.txt")],
        ClipboardOperation::Copy,
    )));

    for command in [CommandId::Copy, CommandId::CopyPath, CommandId::Paste] {
        assert_eq!(app.execute(command), Execution::Done, "{command:?}");
        let (_, update) = app.wait_update(WAIT).expect("결과");
        assert_eq!(
            update,
            Update::Clipboard(Err(ClipboardFailure::Clipboard(ClipboardError::Stopped))),
            "{command:?}"
        );
    }
    // 아무 작업도 만들지 않았다.
    assert!(app.work_area().is_empty());
    assert!(!dir.path().join("src").join("파일 - 복사본.txt").exists());
}

#[cfg(not(windows))]
#[test]
fn windows가_아니면_클립보드를_띄울_수_없다() {
    let mut app = App::new(Some(loc(Path::new("."))));
    assert_eq!(app.enable_clipboard(), Err(ClipboardError::Unsupported));
}

#[cfg(windows)]
mod windows {
    use super::*;
    use facet_core::history::OperationKind;
    use facet_core::job::{JobId, JobKind, JobState, Summary};

    /// 작업이 끝날 때까지 사건을 받고 결과를 돌려준다.
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
            assert!(
                Instant::now() < deadline,
                "작업이 시간 안에 끝나지 않았다: {:?}",
                app.work_area()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn 복사_붙여넣기는_반대편_패널_복사와_같은_길이고_같은_폴더면_복사본이_된다() {
        let dir = tree();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        let mut app = app_at(&src);

        // 다른 폴더로 붙여넣기: 작업 영역에 복사 작업이 생기고 내용이 복사된다.
        let job = app
            .paste_files(
                files(
                    &[src.join("파일.txt"), src.join("a")],
                    ClipboardOperation::Copy,
                ),
                dst.clone(),
            )
            .expect("작업을 넣는다");
        let entry = app.work_area().entry(job).expect("작업 영역에 있다");
        assert_eq!(entry.kind, JobKind::Copy);
        assert_eq!(entry.target, dst);
        let (state, summary) = wait_job(&mut app, job);
        assert_eq!(state, JobState::Finished, "{summary:?}");
        assert_eq!(names(&dst), ["a", "파일.txt"]);
        assert_eq!(fs::read(dst.join("a").join("안.txt")).unwrap(), b"in a");
        // 원본은 그대로다.
        assert!(src.join("파일.txt").is_file());
        // 작업 기록에 복사로 남는다.
        assert_eq!(
            app.history().records().last().map(|r| r.kind()),
            Some(OperationKind::Copy)
        );

        // 같은 폴더로 붙여넣기: 「- 복사본」.
        let job = app
            .paste_files(
                files(&[src.join("파일.txt")], ClipboardOperation::Copy),
                src.clone(),
            )
            .expect("작업을 넣는다");
        let (state, summary) = wait_job(&mut app, job);
        assert_eq!(state, JobState::Finished, "{summary:?}");
        assert!(src.join("파일 - 복사본.txt").is_file());
    }

    #[test]
    fn 잘라내기_붙여넣기는_이동이고_실행취소로_원래대로_돌아간다() {
        let dir = tree();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        let mut app = app_at(&src);

        let job = app
            .paste_files(
                files(&[src.join("파일.txt")], ClipboardOperation::Move),
                dst.clone(),
            )
            .expect("작업을 넣는다");
        assert_eq!(app.work_area().entry(job).unwrap().kind, JobKind::Move);
        let (state, summary) = wait_job(&mut app, job);
        assert_eq!(state, JobState::Finished, "{summary:?}");
        assert!(!src.join("파일.txt").exists());
        assert_eq!(fs::read(dst.join("파일.txt")).unwrap(), b"x");
        assert_eq!(
            app.history().records().last().map(|r| r.kind()),
            Some(OperationKind::Move)
        );

        // 이동 뒤 실행취소로 원래대로.
        let undo = app.undo().expect("실행취소");
        let (state, summary) = wait_job(&mut app, undo);
        assert_eq!(state, JobState::Finished, "{summary:?}");
        assert_eq!(fs::read(src.join("파일.txt")).unwrap(), b"x");
        assert!(!dst.join("파일.txt").exists());
    }

    #[test]
    fn 자기_하위로_붙여넣기는_시작_전에_막는다() {
        let dir = tree();
        let src = dir.path().join("src");
        let mut app = app_at(&src);
        let result = app.paste_files(
            files(&[src.join("a")], ClipboardOperation::Copy),
            src.join("a"),
        );
        assert!(result.is_err());
        assert!(app.work_area().is_empty());
    }

    /// 실제 클립보드로 복사 → 붙여넣기 왕복. 클립보드를 쓸 수 없으면 건너뛴다고 출력한다.
    #[test]
    fn 실제_클립보드로_복사하고_붙여넣는다() {
        let dir = tree();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        let mut app = app_at(&src);
        if let Err(error) = app.enable_clipboard() {
            println!("건너뜀: 클립보드 스레드를 띄울 수 없습니다({error})");
            return;
        }
        let file = index_of(app.active_tab(), "파일.txt");
        app.active_tab_mut().select_only(file);

        // 복사.
        assert_eq!(app.execute(CommandId::Copy), Execution::Done);
        match wait_clipboard(&mut app) {
            Ok(done) => assert_eq!(done, ClipboardDone::Copied { items: 1 }),
            Err(failure) => {
                println!("건너뜀: 클립보드를 열 수 없습니다({failure:?})");
                return;
            }
        }
        // 변경 알림이 오면 붙여넣기가 켜진다(클립보드를 부르지 않는 앱 상태).
        let deadline = Instant::now() + WAIT;
        while !app.can_execute(CommandId::Paste) {
            assert!(Instant::now() < deadline, "변경 알림이 오지 않았다");
            std::thread::sleep(Duration::from_millis(10));
        }

        // 다른 폴더(폴더 하나를 선택)와 같은 폴더에 붙여넣는다.
        assert!(app.navigate(Navigation::Go(loc(dir.path()))));
        let (_, outcome) = app.wait(WAIT).expect("읽기");
        assert_eq!(outcome, ReadOutcome::Committed);
        let dst_index = index_of(app.active_tab(), "dst");
        app.active_tab_mut().select_only(dst_index);
        assert_eq!(app.paste_target(), Some(dst.clone()));
        assert_eq!(app.execute(CommandId::Paste), Execution::Done);
        let job = match wait_clipboard(&mut app) {
            Ok(ClipboardDone::Pasted { job, kind }) => {
                assert_eq!(kind, JobKind::Copy);
                job
            }
            other => panic!("붙여넣기가 작업을 넣지 못했다: {other:?}"),
        };
        let (state, summary) = wait_job(&mut app, job);
        assert_eq!(state, JobState::Finished, "{summary:?}");
        assert_eq!(fs::read(dst.join("파일.txt")).unwrap(), b"x");
        assert!(src.join("파일.txt").is_file());

        // 복사는 클립보드를 비우지 않으므로 한 번 더 붙여넣을 수 있다(같은 폴더에 하면 복사본).
        assert!(app.can_execute(CommandId::Paste));

        // 경로 복사: 글자를 올린다(클립보드의 파일은 없어진다).
        assert_eq!(app.execute(CommandId::CopyPath), Execution::Done);
        assert_eq!(
            wait_clipboard(&mut app),
            Ok(ClipboardDone::PathsCopied { items: 1 })
        );
        let deadline = Instant::now() + WAIT;
        while app.can_execute(CommandId::Paste) {
            assert!(Instant::now() < deadline, "변경 알림이 오지 않았다");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// 클립보드 명령의 결과 하나를 기다린다.
    fn wait_clipboard(app: &mut App) -> Result<ClipboardDone, ClipboardFailure> {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "클립보드 결과가 오지 않았다");
            if let Some((_, Update::Clipboard(result))) = app.wait_update(left) {
                return result;
            }
        }
    }
}

#[test]
fn 잘라내기는_ctrl_x이고_한글_모드에서도_같고_원본이_있을_때만_켜진다() {
    use CommandId::Cut;
    let key = |c: char| Key::from_char(c).unwrap();
    assert_eq!(Cut.shortcuts(), &[Shortcut::new(key('x')).ctrl()]);
    assert_eq!(find_by_shortcut(Shortcut::new(key('ㅌ')).ctrl()), Some(Cut));
    assert_eq!(Cut.as_str(), "cut");
    assert_eq!(CommandId::from_id("cut"), Some(Cut));

    let dir = tree();
    let src = dir.path().join("src");
    let mut app = app_at(&src);
    assert!(!app.can_execute(Cut));
    assert_eq!(app.execute(Cut), Execution::Disabled);
    let file = index_of(app.active_tab(), "파일.txt");
    app.active_tab_mut().select_only(file);
    assert!(app.can_execute(Cut));
    assert!(Cut.is_enabled(app.active_tab()));
    let found = app.search_commands("잘라", name);
    assert_eq!(found.len(), 1);
    assert!(found[0].enabled);
}

#[test]
fn 잘라낸_경로_집합은_클립보드_요약을_따르고_바뀌면_풀린다() {
    let dir = tree();
    let src = dir.path().join("src");
    let mut app = app_at(&src);
    assert!(app.cut_marks().is_empty());

    // 잘라내기 표시가 있는 목록만 표시가 된다. 대소문자는 무시한다.
    let cut_path = PathBuf::from(src.to_string_lossy().to_uppercase()).join("파일.txt");
    app.set_clipboard_files(Some(files(&[cut_path], ClipboardOperation::Move)));
    let marks = app.cut_marks();
    assert_eq!(marks.len(), 1);
    assert!(marks.contains(&src, "파일.txt".as_ref()));
    assert!(marks.contains(&src, "파일.TXT".as_ref()));
    // 다른 폴더의 같은 이름은 아니다.
    assert!(!marks.contains(&src.join("a"), "파일.txt".as_ref()));
    assert!(!marks.contains(&src, "b".as_ref()));

    // 같은 요약이 다시 오면 표시는 그대로다(화면이 다시 그리지 않아도 된다).
    app.set_clipboard_files(Some(files(
        &[src.join("파일.txt")],
        ClipboardOperation::Move,
    )));
    assert!(std::sync::Arc::ptr_eq(&marks, &app.cut_marks()));

    // 복사로 바뀌면 풀린다.
    app.set_clipboard_files(Some(files(
        &[src.join("파일.txt")],
        ClipboardOperation::Copy,
    )));
    assert!(app.cut_marks().is_empty());
    // 다시 잘라낸 뒤 클립보드가 비면 풀린다.
    app.set_clipboard_files(Some(files(&[src.join("b")], ClipboardOperation::Move)));
    assert!(app.cut_marks().contains(&src, "b".as_ref()));
    app.set_clipboard_files(None);
    assert!(app.cut_marks().is_empty());
}

#[test]
fn 잘라낸_경로가_1만_개여도_판정은_빠르다() {
    use facet_core::clipboard::CutMarks;
    let folder = Path::new(r"C:\큰 폴더");
    let marks = CutMarks::new((0..10_000).map(|i| folder.join(format!("항목 {i}.txt"))));
    let start = std::time::Instant::now();
    let mut hits = 0;
    for round in 0..10 {
        for i in 0..10_000 {
            let name = format!("항목 {}.txt", i + round * 5_000);
            hits += usize::from(marks.contains(folder, name.as_ref()));
        }
    }
    assert!(hits > 0);
    // 10만 번 묻는 데 1초 넘게 걸리면 상수 시간 판정이 아니다(로컬은 수십 ms).
    assert!(start.elapsed() < Duration::from_secs(1));
}
