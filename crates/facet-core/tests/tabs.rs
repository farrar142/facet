//! 패널의 탭 여러 개: 열기·닫기·오가기, 탭마다 따로인 상태, 보이지 않는 탭의 감시 멈춤을 공개 인터페이스로 확인한다.
//!
//! 앱([`App`])은 일꾼 스레드에서 읽으므로 결과를 기다린다. CI 러너가 느려도 흔들리지 않도록 기다리는 시간
//! ([`WAIT`])을 넉넉히 두고, 조건이 맞는 즉시 끝낸다.

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::command::CommandId;
use facet_core::details::{Column, Direction, Sort};
use facet_core::tab::{AutoRefresh, Navigation, ReadOutcome, Tab};
use facet_core::window::{App, Closed, Execution, TabId};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(10);

fn loc(path: &Path) -> Location {
    Location::file_system(path)
}

/// `root/a/a1`, `root/b`, `root/파일.txt`가 있는 임시 폴더.
fn tree() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("a").join("a1")).unwrap();
    fs::create_dir(dir.path().join("b")).unwrap();
    fs::write(dir.path().join("파일.txt"), b"x").unwrap();
    dir
}

/// `path`를 연 앱. 첫 읽기가 끝날 때까지 기다린다.
fn app_at(path: &Path) -> App {
    let mut app = App::new(Some(loc(path)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

/// 활성 패널의 탭 번호들을 차례로.
fn ids(app: &App) -> Vec<TabId> {
    app.active_window()
        .active_panel()
        .tabs()
        .map(|(id, _)| id)
        .collect()
}

fn active(app: &App) -> TabId {
    app.active_tab_id()
}

fn tab(app: &App, id: TabId) -> &Tab {
    app.tab(id).expect("열린 탭")
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

#[test]
fn 새_탭은_지금_위치로_활성_탭_바로_뒤에_열리고_활성이_된다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    let first = active(&app);
    assert_eq!(ids(&app), [first]);

    assert_eq!(app.execute(CommandId::NewTab), Execution::Done);
    let second = active(&app);
    assert_ne!(second, first);
    assert_eq!(ids(&app), [first, second]);
    assert_eq!(app.active_window().active_panel().active_index(), 1);
    assert_eq!(app.active_tab().location(), &loc(dir.path()));
    settle(&mut app);
    assert_eq!(names(app.active_tab()), ["a", "b", "파일.txt"]);
    // 새 탭은 기록 없이 시작한다.
    assert!(!app.active_tab().can_go_back());

    // 첫 탭으로 돌아가 새 탭을 열면 첫 탭 바로 뒤에 들어간다.
    assert_eq!(app.execute(CommandId::GoToTab(1)), Execution::Done);
    assert_eq!(active(&app), first);
    app.execute(CommandId::NewTab);
    let third = active(&app);
    assert_eq!(ids(&app), [first, third, second]);
    assert_eq!(app.active_window().active_panel().active_index(), 1);
    settle(&mut app);
}

#[test]
fn 다음_이전_탭은_끝에서_처음으로_돌고_n번째_마지막_탭으로_간다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    // 탭이 하나면 다음·이전은 꺼지고, 없는 n번째 탭도 꺼진다.
    assert!(!app.can_execute(CommandId::NextTab));
    assert!(!app.can_execute(CommandId::PreviousTab));
    assert!(app.can_execute(CommandId::GoToTab(1)));
    assert!(!app.can_execute(CommandId::GoToTab(2)));
    assert!(app.can_execute(CommandId::LastTab));
    assert!(app.can_execute(CommandId::NewTab));
    assert!(app.can_execute(CommandId::CloseTab));

    app.execute(CommandId::NewTab);
    app.execute(CommandId::NewTab);
    settle(&mut app);
    let all = ids(&app);
    assert_eq!(all.len(), 3);
    assert_eq!(active(&app), all[2]);
    assert!(app.can_execute(CommandId::NextTab));
    assert!(app.can_execute(CommandId::GoToTab(3)));
    assert!(!app.can_execute(CommandId::GoToTab(4)));

    // 끝에서 다음은 처음으로, 처음에서 이전은 끝으로 돈다.
    assert_eq!(app.execute(CommandId::NextTab), Execution::Done);
    assert_eq!(active(&app), all[0]);
    app.execute(CommandId::NextTab);
    assert_eq!(active(&app), all[1]);
    app.execute(CommandId::PreviousTab);
    app.execute(CommandId::PreviousTab);
    assert_eq!(active(&app), all[2]);

    // n번째 탭: 없으면 아무것도 하지 않는다.
    assert_eq!(app.execute(CommandId::GoToTab(2)), Execution::Done);
    assert_eq!(active(&app), all[1]);
    assert_eq!(app.execute(CommandId::GoToTab(8)), Execution::Disabled);
    assert_eq!(active(&app), all[1]);
    // 마지막 탭.
    app.execute(CommandId::LastTab);
    assert_eq!(active(&app), all[2]);
    // 탭 순서는 오가도 그대로다.
    assert_eq!(ids(&app), all);

    // 탭을 눌러 고르는 것도 같은 연산이다.
    assert!(app.activate_tab(all[0]));
    assert_eq!(active(&app), all[0]);
    settle(&mut app);
}

#[test]
fn 탭을_닫으면_이웃_탭이_활성이_되고_마지막_탭을_닫으면_패널과_창이_닫혀_앱이_끝난다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    app.execute(CommandId::NewTab);
    app.execute(CommandId::NewTab);
    settle(&mut app);
    let all = ids(&app);

    // 가운데 탭을 닫으면 오른쪽 이웃이 활성이 된다.
    app.execute(CommandId::GoToTab(2));
    assert_eq!(app.execute(CommandId::CloseTab), Execution::Done);
    assert_eq!(ids(&app), [all[0], all[2]]);
    assert_eq!(active(&app), all[2]);
    assert!(app.tab(all[1]).is_none());

    // 활성이 아닌 탭은 그 탭만 닫고 활성 탭은 그대로다.
    assert_eq!(
        app.close_tab(all[0]),
        Some(Closed {
            panel: false,
            window: false
        })
    );
    assert_eq!(ids(&app), [all[2]]);
    assert_eq!(active(&app), all[2]);
    // 없는 탭은 닫지 못한다.
    assert_eq!(app.close_tab(all[0]), None);

    // 끝의 탭을 닫으면 왼쪽 이웃이 활성이 된다.
    app.execute(CommandId::NewTab);
    let last = active(&app);
    app.execute(CommandId::CloseTab);
    assert_eq!(active(&app), all[2]);
    assert!(app.tab(last).is_none());
    settle(&mut app);
    assert!(!app.is_finished());

    // 패널의 마지막 탭을 닫으면 패널이 닫히고, 창의 마지막 패널이면 창이 닫힌다. 창이 없으면 앱이 끝난다.
    assert_eq!(app.execute(CommandId::CloseTab), Execution::WindowClosed);
    assert!(app.windows().is_empty());
    assert!(app.is_finished());
    // 끝난 앱에서는 아무 명령도 실행하지 않는다.
    assert!(!app.can_execute(CommandId::NewTab));
    assert_eq!(app.execute(CommandId::NewTab), Execution::Disabled);

    // close_tab은 무엇이 함께 닫혔는지 알린다.
    let mut app = app_at(dir.path());
    let only = active(&app);
    assert_eq!(
        app.close_tab(only),
        Some(Closed {
            panel: true,
            window: true
        })
    );
    assert!(app.is_finished());
}

#[test]
fn 탭마다_선택_커서_스크롤_기록_정렬을_따로_갖는다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    let first = active(&app);
    // 첫 탭: 폴더 a로 들어갔다가 돌아와 앞으로 기록을 두고, b를 고르고, 스크롤하고, 크기로 정렬한다.
    assert!(app.navigate(Navigation::Open(OsString::from("a"))));
    settle(&mut app);
    assert!(app.navigate(Navigation::Back));
    settle(&mut app);
    let b = app
        .active_tab()
        .items()
        .position(|i| i.name == "b")
        .unwrap();
    app.active_tab_mut().select_only(b);
    app.active_tab_mut().set_scroll(7);
    app.execute(CommandId::SortBySize);
    let first_sort = app.active_tab().sort();
    let first_cursor = app.active_tab().cursor_item().unwrap().name.clone();

    // 새 탭: 같은 위치에서 기록 없이 시작한다. 이 탭에서는 다른 상태를 만든다.
    app.execute(CommandId::NewTab);
    settle(&mut app);
    let second = active(&app);
    {
        let tab = app.active_tab();
        assert!(!tab.can_go_back() && !tab.can_go_forward());
        assert_eq!(tab.selected_count(), 0);
        assert_eq!(tab.cursor(), None);
        assert_eq!(tab.scroll(), 0);
    }
    assert!(app.navigate(Navigation::Open(OsString::from("a"))));
    settle(&mut app);
    app.active_tab_mut().select_all();
    app.active_tab_mut().set_scroll(2);
    let second_sort = Sort {
        column: Column::Name,
        direction: Direction::Descending,
    };
    app.set_sort(second_sort);

    // 오가도 각자 자리가 남는다(다시 보일 때 다시 읽은 뒤에도).
    for _ in 0..2 {
        app.execute(CommandId::GoToTab(1));
        assert_eq!(active(&app), first);
        settle(&mut app);
        let tab = app.active_tab();
        assert_eq!(tab.location(), &loc(dir.path()));
        assert_eq!(tab.forward_history(), [loc(&dir.path().join("a"))]);
        assert!(!tab.can_go_back());
        assert_eq!(tab.sort(), first_sort);
        assert_eq!(names(tab).len(), 3);
        assert_eq!(
            tab.selected_items()
                .map(|i| i.name.clone())
                .collect::<Vec<_>>(),
            [OsString::from("b")]
        );
        assert_eq!(tab.cursor_item().unwrap().name, first_cursor);
        assert_eq!(tab.scroll(), 7);

        app.execute(CommandId::GoToTab(2));
        assert_eq!(active(&app), second);
        settle(&mut app);
        let tab = app.active_tab();
        assert_eq!(tab.location(), &loc(&dir.path().join("a")));
        assert_eq!(tab.back_history(), [loc(dir.path())]);
        assert_eq!(tab.sort(), second_sort);
        assert_eq!(tab.selected_count(), 1);
        assert_eq!(tab.scroll(), 2);
    }

    // 다른 탭에서 뒤로 가도 이 탭의 기록만 쓴다.
    assert_eq!(app.execute(CommandId::Back), Execution::Done);
    settle(&mut app);
    assert_eq!(app.active_tab().location(), &loc(dir.path()));
    assert_eq!(
        tab(&app, first).forward_history(),
        [loc(&dir.path().join("a"))]
    );
}

#[test]
fn 늦게_온_결과는_요청한_탭에만_들어간다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    let first = active(&app);
    // 첫 탭이 a를 읽는 동안 새 탭을 연다. 새 탭은 첫 탭의 지금 위치(아직 루트)에서 연다.
    assert!(app.navigate(Navigation::Open(OsString::from("a"))));
    app.execute(CommandId::NewTab);
    let second = active(&app);
    settle(&mut app);
    assert_eq!(tab(&app, first).location(), &loc(&dir.path().join("a")));
    assert_eq!(names(tab(&app, first)), ["a1"]);
    assert_eq!(tab(&app, second).location(), &loc(dir.path()));
    assert_eq!(names(tab(&app, second)), ["a", "b", "파일.txt"]);
    // 보이지 않는 동안 끝난 읽기는 감시를 남기지 않는다.
    assert_eq!(tab(&app, first).watched_location(), None);
}

#[test]
fn 폴더를_새_탭에서_열면_지금_탭은_그대로_두고_뒤에_붙인다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    let first = active(&app);
    let index = |app: &App, name: &str| {
        app.active_tab()
            .items()
            .position(|i| i.name == name)
            .unwrap()
    };

    // 파일은 새 탭에서 열지 않는다.
    let file = index(&app, "파일.txt");
    assert_eq!(app.open_in_new_tab(file), None);
    assert_eq!(ids(&app).len(), 1);

    let a = index(&app, "a");
    let opened = app.open_in_new_tab(a).expect("폴더는 새 탭에서 연다");
    assert_eq!(ids(&app), [first, opened]);
    // 활성 탭은 그대로다.
    assert_eq!(active(&app), first);
    assert_eq!(app.active_tab().location(), &loc(dir.path()));
    let new = tab(&app, opened);
    assert_eq!(new.location(), &loc(&dir.path().join("a")));
    assert!(!new.is_shown());
    // 보이지 않으므로 아직 읽지도, 감시하지도 않는다.
    assert!(new.pending_location().is_none());
    assert!(matches!(new.auto_refresh(), AutoRefresh::NotWatched));

    // 명령: 커서 항목이 폴더일 때만 켜진다.
    assert!(!app.can_execute(CommandId::OpenInNewTab));
    app.active_tab_mut().select_only(file);
    assert!(!app.can_execute(CommandId::OpenInNewTab));
    let b = index(&app, "b");
    app.active_tab_mut().select_only(b);
    assert!(app.can_execute(CommandId::OpenInNewTab));
    assert_eq!(app.execute(CommandId::OpenInNewTab), Execution::Done);
    // 활성 탭 바로 뒤에 붙는다.
    let all = ids(&app);
    assert_eq!(all.len(), 3);
    assert_eq!(all[0], first);
    assert_eq!(all[2], opened);
    assert_eq!(tab(&app, all[1]).location(), &loc(&dir.path().join("b")));
    assert_eq!(active(&app), first);

    // 처음 보일 때 읽는다.
    app.execute(CommandId::LastTab);
    assert_eq!(active(&app), opened);
    assert!(app.active_tab().is_shown());
    settle(&mut app);
    assert_eq!(names(app.active_tab()), ["a1"]);
}

#[test]
fn 탭_제목은_폴더_이름이고_드라이브_루트는_드라이브_이름이다() {
    let dir = tree();
    let tab = Tab::new(loc(&dir.path().join("a")), Sort::default(), false);
    assert_eq!(tab.title(), "a");
    let root = dir.path().ancestors().last().unwrap();
    let tab = Tab::new(loc(root), Sort::default(), false);
    let expected: String = root
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_owned();
    #[cfg(windows)]
    assert_eq!(tab.title(), expected);
    #[cfg(not(windows))]
    let _ = expected;
}

#[cfg(windows)]
#[test]
fn 보이지_않는_탭은_감시를_놓고_다시_보이면_이전_목록을_두고_다시_읽으며_감시를_건다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    let first = active(&app);
    assert!(matches!(app.active_tab().auto_refresh(), AutoRefresh::On));

    app.execute(CommandId::NewTab);
    settle(&mut app);
    let second = active(&app);
    // 숨은 첫 탭은 감시를 놓았지만 목록은 그대로 갖고 있다.
    let hidden = tab(&app, first);
    assert!(!hidden.is_shown());
    assert_eq!(hidden.watched_location(), None);
    assert!(matches!(hidden.auto_refresh(), AutoRefresh::NotWatched));
    assert_eq!(names(hidden), ["a", "b", "파일.txt"]);
    assert_eq!(tab(&app, second).watched_location(), Some(&loc(dir.path())));

    // 숨은 동안 바뀐 것은 반영되지 않는다(감시가 없다). 보이는 새 탭에는 반영된다.
    fs::write(dir.path().join("새 파일.txt"), b"new").unwrap();
    let deadline = Instant::now() + WAIT;
    while names(app.active_tab()).len() != 4 {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "보이는 탭에 변경이 반영되지 않았다");
        app.wait_update(left);
    }
    assert_eq!(names(tab(&app, first)).len(), 3);

    // 다시 보이면 이전 목록을 먼저 두고 다시 읽는다. 다 읽으면 감시를 다시 건다.
    app.execute(CommandId::GoToTab(1));
    let shown = tab(&app, first);
    assert!(shown.is_shown());
    assert_eq!(shown.pending_location(), Some(&loc(dir.path())));
    assert_eq!(names(shown).len(), 3);
    assert_eq!(tab(&app, second).watched_location(), None);
    settle(&mut app);
    let shown = tab(&app, first);
    assert_eq!(names(shown).len(), 4);
    assert_eq!(shown.watched_location(), Some(&loc(dir.path())));
}
