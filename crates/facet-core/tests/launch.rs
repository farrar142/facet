//! 실행 인자 해석([`resolve_launch`])과 실행 인자로 새 창 열기([`App::open_launch_window`], [`App::launch`])를
//! 공개 인터페이스로 확인한다.
//!
//! 앱([`App`])은 일꾼 스레드에서 읽으므로 결과를 기다린다. CI 러너가 느려도 흔들리지 않도록 기다리는 시간
//! ([`WAIT`])을 넉넉히 두고, 조건이 맞는 즉시 끝낸다.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use facet_core::launch::{LaunchTarget, resolve_launch};
use facet_core::tab::{ReadOutcome, Tab};
use facet_core::window::{App, home_location};
use facet_core::{Location, ResolveError};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(10);

fn loc(path: &Path) -> Location {
    Location::file_system(path)
}

/// `root/a/a1`, `root/b`, `root/c`, `root/파일.txt`가 있는 임시 폴더.
fn tree() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("a").join("a1")).unwrap();
    fs::create_dir(dir.path().join("b")).unwrap();
    fs::create_dir(dir.path().join("c")).unwrap();
    fs::write(dir.path().join("파일.txt"), b"x").unwrap();
    dir
}

fn paths(items: &[&str]) -> Vec<PathBuf> {
    items.iter().map(PathBuf::from).collect()
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

fn names(tab: &Tab) -> Vec<String> {
    tab.items()
        .map(|i| i.name.to_string_lossy().into_owned())
        .collect()
}

/// 앱의 마지막 창(방금 연 창)의 탭들.
fn last_window_tabs(app: &App) -> Vec<&Tab> {
    let window = app.windows().last().expect("창이 있다");
    assert_eq!(window.panels().len(), 1, "패널은 하나");
    window.panels()[0].tabs().map(|(_, tab)| tab).collect()
}

#[test]
fn 인자가_없으면_홈_폴더_하나다() {
    let dir = tree();
    let targets = resolve_launch(dir.path(), &[]);
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].location, home_location());
    assert!(targets[0].error.is_none());
    assert!(targets[0].select.is_none());
}

#[test]
fn 상대_경로는_넘긴_작업_폴더_기준으로_푼다() {
    let dir = tree();
    let cwd = std::env::current_dir().unwrap();
    assert_ne!(
        cwd,
        dir.path(),
        "작업 폴더는 프로세스의 현재 폴더와 달라야 한다"
    );
    let targets = resolve_launch(
        &dir.path().join("a"),
        &paths(&["a1", ".", "..", "../b", "./a1/../../c"]),
    );
    let locations: Vec<_> = targets.iter().map(|t| t.location.clone()).collect();
    assert_eq!(
        locations,
        vec![
            loc(&dir.path().join("a").join("a1")),
            loc(&dir.path().join("a")),
            loc(dir.path()),
            loc(&dir.path().join("b")),
            loc(&dir.path().join("c")),
        ]
    );
    assert!(targets.iter().all(|t| t.error.is_none()));
}

#[test]
fn 전체_경로는_작업_폴더와_상관없다() {
    let dir = tree();
    let other = TempDir::new().unwrap();
    let b = dir.path().join("b");
    let targets = resolve_launch(other.path(), std::slice::from_ref(&b));
    assert_eq!(targets[0].location, loc(&b));
    assert!(targets[0].error.is_none());
}

#[test]
fn 감싼_큰따옴표와_앞뒤_공백은_걷는다() {
    let dir = tree();
    let targets = resolve_launch(dir.path(), &paths(&["  \"b\"  "]));
    assert_eq!(targets[0].location, loc(&dir.path().join("b")));
    assert!(targets[0].error.is_none());
}

#[test]
fn 파일_경로는_그_폴더로_가서_파일을_고른다() {
    let dir = tree();
    let targets = resolve_launch(dir.path(), &paths(&["파일.txt"]));
    assert_eq!(targets[0].location, loc(dir.path()));
    assert_eq!(
        targets[0].select.as_deref(),
        Some(std::ffi::OsStr::new("파일.txt"))
    );
    assert!(targets[0].error.is_none());
}

#[test]
fn 해석하지_못한_경로도_탭이_되고_까닭을_갖는다() {
    let dir = tree();
    let targets = resolve_launch(dir.path(), &paths(&["없는 폴더", "b"]));
    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0].location, loc(&dir.path().join("없는 폴더")));
    assert!(
        matches!(&targets[0].error, Some(ResolveError::NotFound { location }) if *location == loc(&dir.path().join("없는 폴더"))),
        "{:?}",
        targets[0].error
    );
    assert!(targets[1].error.is_none());
}

#[test]
fn 쓸_수_없는_글자는_주소창과_같은_까닭이다() {
    let dir = tree();
    let targets = resolve_launch(dir.path(), &paths(&["a|b"]));
    assert!(
        matches!(
            targets[0].error,
            Some(ResolveError::InvalidCharacter { character: '|' })
        ),
        "{:?}",
        targets[0].error
    );
}

#[test]
fn 인자로_새_창을_열면_경로마다_탭_하나가_순서대로_첫_탭이_활성이다() {
    let dir = tree();
    let mut app = App::new(Some(loc(dir.path())));
    settle(&mut app);
    let first = app.active_window_id();
    let targets = resolve_launch(dir.path(), &paths(&["b", "a", "c"]));
    let id = app.open_launch_window(targets);
    assert_ne!(id, first);
    assert_eq!(app.windows().len(), 2);
    assert_eq!(app.active_window_id(), id, "새 창이 활성 창");
    let window = app.window(id).unwrap();
    assert_eq!(window.panels().len(), 1);
    assert_eq!(window.panels()[0].active_index(), 0, "첫 탭이 활성");
    let locations: Vec<_> = last_window_tabs(&app)
        .iter()
        .map(|t| t.location().clone())
        .collect();
    assert_eq!(
        locations,
        vec![
            loc(&dir.path().join("b")),
            loc(&dir.path().join("a")),
            loc(&dir.path().join("c")),
        ]
    );
    // 첫 탭만 보이고 읽는다. 나머지는 숨은 채로 처음 보일 때 읽는다.
    let shown: Vec<_> = last_window_tabs(&app)
        .iter()
        .map(|t| t.is_shown())
        .collect();
    assert_eq!(shown, vec![true, false, false]);
    assert_eq!(
        app.active_tab().pending_location(),
        Some(&loc(&dir.path().join("b")))
    );
    assert!(last_window_tabs(&app)[1].pending_location().is_none());
    settle(&mut app);
    // 앞 창의 탭은 그대로 보인다.
    assert!(app.windows()[0].panels()[0].active_tab().is_shown());
}

#[test]
fn 인자_없이_새_창을_열면_홈_폴더_탭_하나다() {
    let dir = tree();
    let mut app = App::new(Some(loc(dir.path())));
    settle(&mut app);
    app.open_launch_window(resolve_launch(dir.path(), &[]));
    let tabs = last_window_tabs(&app);
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].location(), &home_location());
    // 빈 목록을 넘겨도 홈 폴더 탭 하나다.
    app.open_launch_window(Vec::new());
    let tabs = last_window_tabs(&app);
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].location(), &home_location());
}

#[test]
fn 파일_인자의_탭은_읽으면_그_파일이_골라져_있다() {
    let dir = tree();
    let mut app = App::launch(resolve_launch(dir.path(), &paths(&["파일.txt"])));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    let tab = app.active_tab();
    assert_eq!(tab.location(), &loc(dir.path()));
    let index = names(tab).iter().position(|n| n == "파일.txt").unwrap();
    assert_eq!(tab.cursor(), Some(index));
    assert!(tab.is_selected(index));
    assert_eq!(tab.status().selected_count, 1);
}

#[test]
fn 해석하지_못한_인자의_탭은_읽지_않고_까닭을_보인다() {
    let dir = tree();
    let mut app = App::launch(resolve_launch(dir.path(), &paths(&["없는 폴더", "b"])));
    assert_eq!(last_window_tabs(&app).len(), 2);
    let tab = app.active_tab();
    assert_eq!(tab.location(), &loc(&dir.path().join("없는 폴더")));
    assert!(matches!(
        tab.address_error(),
        Some(ResolveError::NotFound { .. })
    ));
    assert!(tab.pending_location().is_none(), "읽지 않는다");
    // 다른 탭은 그대로 열린다.
    assert_eq!(
        last_window_tabs(&app)[1].location(),
        &loc(&dir.path().join("b"))
    );
    settle(&mut app);
}

#[test]
fn 첫_실행도_같은_해석으로_창_하나를_연다() {
    let dir = tree();
    let mut app = App::launch(resolve_launch(dir.path(), &paths(&["a", "b"])));
    assert_eq!(app.windows().len(), 1);
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    assert_eq!(app.active_tab().location(), &loc(&dir.path().join("a")));
    assert_eq!(names(app.active_tab()), vec!["a1"]);
    assert_eq!(last_window_tabs(&app).len(), 2);
}

#[test]
fn 창이_하나도_없어도_인자로_새_창을_연다() {
    let dir = tree();
    let mut app = App::new(Some(loc(dir.path())));
    settle(&mut app);
    let only = app.active_window_id();
    assert!(app.close_window(only));
    assert!(app.is_finished());
    let id = app.open_launch_window(vec![LaunchTarget {
        location: loc(&dir.path().join("c")),
        select: None,
        error: None,
    }]);
    assert!(!app.is_finished());
    assert_eq!(app.active_window_id(), id);
    assert_eq!(app.active_tab().location(), &loc(&dir.path().join("c")));
    settle(&mut app);
    assert!(app.active_tab().error().is_none());
}

#[test]
fn 새_창의_정렬은_활성_탭을_따른다() {
    use facet_core::details::{Column, Direction, Sort};
    let dir = tree();
    let mut app = App::new(Some(loc(dir.path())));
    settle(&mut app);
    let sort = Sort {
        column: Column::Size,
        direction: Direction::Descending,
    };
    app.set_sort(sort);
    app.open_launch_window(resolve_launch(dir.path(), &paths(&["a", "b"])));
    assert!(last_window_tabs(&app).iter().all(|t| t.sort() == sort));
    settle(&mut app);
}
