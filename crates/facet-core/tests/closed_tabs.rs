//! 닫은 탭 되살리기: 앱 전체에 하나인 닫은 탭 목록(최근 [`CLOSED_TABS_LIMIT`]개)과 「닫은 탭 되살리기」 명령을
//! 공개 인터페이스로 확인한다.
//!
//! 앱([`App`])은 일꾼 스레드에서 읽으므로 결과를 기다린다. CI 러너가 느려도 흔들리지 않도록 기다리는 시간
//! ([`WAIT`])을 넉넉히 두고, 조건이 맞는 즉시 끝낸다.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::command::{CommandId, Key, Shortcut, find_by_shortcut};
use facet_core::details::{Column, Direction, Sort};
use facet_core::tab::{Navigation, ReadOutcome, Tab};
use facet_core::window::{App, CLOSED_TABS_LIMIT, Execution, TabId};
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

/// 활성 탭을 `location`으로 옮기고 읽기가 끝날 때까지 기다린다.
fn go(app: &mut App, location: Location) {
    assert!(app.navigate(Navigation::Go(location)));
    settle(app);
}

/// 닫은 탭 목록의 위치들, 먼저 되살아날 것부터.
fn closed_locations(app: &App) -> Vec<Location> {
    app.closed_tabs().map(|t| t.location().clone()).collect()
}

#[test]
fn 닫은_탭_되살리기_명령은_ctrl_shift_t이고_닫은_탭이_없으면_꺼진다() {
    let command = CommandId::ReopenClosedTab;
    assert!(CommandId::ALL.contains(&command));
    assert_eq!(command.as_str(), "reopen-closed-tab");
    assert_eq!(CommandId::from_id("reopen-closed-tab"), Some(command));
    let shortcut = Shortcut::new(Key::Letter('T')).ctrl().shift();
    assert_eq!(command.shortcuts(), [shortcut]);
    assert_eq!(find_by_shortcut(shortcut), Some(command));
    // 한글 모드에서도 같다: T 키는 「ㅅ」을 낸다.
    assert_eq!(
        find_by_shortcut(Shortcut::new(Key::from_char('ㅅ').unwrap()).ctrl().shift()),
        Some(command)
    );

    let dir = tree();
    let mut app = app_at(dir.path());
    assert_eq!(app.closed_tabs().len(), 0);
    assert!(!app.can_execute(command));
    assert_eq!(app.execute(command), Execution::Disabled);
    assert_eq!(app.reopen_closed_tab(), None);
    // 명령 팔레트에도 꺼진 채로 보인다.
    let found = app.search_commands("reopen", |c: CommandId| c.as_str());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].command, command);
    assert!(!found[0].enabled);

    // 탭을 닫으면 켜진다.
    app.execute(CommandId::NewTab);
    app.execute(CommandId::CloseTab);
    assert!(app.can_execute(command));
    let found = app.search_commands("reopen", |c: CommandId| c.as_str());
    assert!(found[0].enabled);

    // 되살리면 목록이 비어 다시 꺼진다.
    assert_eq!(app.execute(command), Execution::Done);
    assert!(!app.can_execute(command));
    settle(&mut app);
}

#[test]
fn 마지막에_닫은_탭부터_되살아난다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    let a = loc(&dir.path().join("a"));
    let b = loc(&dir.path().join("b"));
    let a1 = loc(&dir.path().join("a").join("a1"));
    for location in [&a, &b, &a1] {
        app.execute(CommandId::NewTab);
        go(&mut app, location.clone());
    }
    // 탭: root, a, b, a1. a를 닫고, b를 닫고, a1을 닫는다.
    let all = ids(&app);
    for id in [all[1], all[2], all[3]] {
        app.close_tab(id).expect("열린 탭");
    }
    assert_eq!(closed_locations(&app), [a1.clone(), b.clone(), a.clone()]);

    for expected in [&a1, &b, &a] {
        let id = app.reopen_closed_tab().expect("닫은 탭");
        assert_eq!(app.active_tab_id(), id);
        assert_eq!(app.active_tab().location(), expected);
    }
    assert_eq!(app.reopen_closed_tab(), None);
    settle(&mut app);
}

#[test]
fn 스물한_개를_닫으면_가장_오래된_탭이_빠지고_스무_개가_남는다() {
    assert_eq!(CLOSED_TABS_LIMIT, 20);
    let dir = tree();
    let mut app = app_at(dir.path());
    // 위치로 구별하도록 서로 다른 위치의 탭을 하나씩 넣는다.
    for n in 0..=CLOSED_TABS_LIMIT {
        let tabs = vec![Tab::new(
            loc(&dir.path().join(format!("closed-{n}"))),
            Sort::default(),
            false,
        )];
        app.remember_closed_tabs(tabs);
    }
    assert_eq!(app.closed_tabs().len(), CLOSED_TABS_LIMIT);
    let locations = closed_locations(&app);
    // 가장 오래된 closed-0이 빠지고, 가장 최근 것이 먼저다.
    assert_eq!(
        locations.first(),
        Some(&loc(&dir
            .path()
            .join(format!("closed-{CLOSED_TABS_LIMIT}"))))
    );
    assert_eq!(locations.last(), Some(&loc(&dir.path().join("closed-1"))));
    assert!(!locations.contains(&loc(&dir.path().join("closed-0"))));

    // 명령으로 닫아도 같은 상한을 지킨다.
    let mut app = app_at(dir.path());
    for _ in 0..=CLOSED_TABS_LIMIT {
        app.execute(CommandId::NewTab);
        assert_eq!(app.execute(CommandId::CloseTab), Execution::Done);
    }
    assert_eq!(app.closed_tabs().len(), CLOSED_TABS_LIMIT);
    settle(&mut app);
}

#[test]
fn 되살린_탭은_위치_기록_정렬을_갖고_활성_패널에_활성_탭으로_열려_다시_읽는다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    let root = loc(dir.path());
    let a = loc(&dir.path().join("a"));
    let a1 = loc(&dir.path().join("a").join("a1"));
    let b = loc(&dir.path().join("b"));

    // 두 번째 탭: root → a → a1 → (뒤로) a. 뒤로 기록 [root], 앞으로 기록 [a1]. 정렬은 크기 내림.
    app.execute(CommandId::NewTab);
    go(&mut app, a.clone());
    go(&mut app, a1.clone());
    assert!(app.navigate(Navigation::Back));
    settle(&mut app);
    let sort = Sort {
        column: Column::Size,
        direction: Direction::Descending,
    };
    app.set_sort(sort);
    assert_eq!(app.active_tab().location(), &a);
    assert_eq!(app.active_tab().back_history(), std::slice::from_ref(&root));
    assert_eq!(
        app.active_tab().forward_history(),
        std::slice::from_ref(&a1)
    );
    let closed = app.active_tab_id();
    assert_eq!(app.execute(CommandId::CloseTab), Execution::Done);
    assert!(app.tab(closed).is_none());
    let first = app.active_tab_id();

    // 닫은 사이에 폴더에 항목이 생겼다: 되살린 탭은 위치를 다시 읽어야 보인다.
    fs::write(dir.path().join("a").join("새 파일.txt"), b"xyz").unwrap();

    // 다른 탭을 하나 더 열어 두고 첫 탭으로 돌아가 되살린다: 활성 탭 바로 뒤에 활성으로 열린다.
    app.execute(CommandId::NewTab);
    go(&mut app, b.clone());
    let other = app.active_tab_id();
    app.activate_tab(first);
    settle(&mut app);
    assert_eq!(app.execute(CommandId::ReopenClosedTab), Execution::Done);
    let reopened = app.active_tab_id();
    assert_ne!(reopened, closed);
    assert_eq!(ids(&app), [first, reopened, other]);
    assert_eq!(app.active_window().active_panel().active_index(), 1);
    // 앞서 보이던 탭은 숨는다.
    assert!(!app.tab(first).unwrap().is_shown());
    let tab = app.active_tab();
    assert!(tab.is_shown());
    assert_eq!(tab.location(), &a);
    assert_eq!(tab.back_history(), std::slice::from_ref(&root));
    assert_eq!(tab.forward_history(), std::slice::from_ref(&a1));
    assert_eq!(tab.sort(), sort);
    // 위치를 다시 읽는다.
    assert_eq!(tab.pending_location(), Some(&a));
    settle(&mut app);
    let mut found = names(app.active_tab());
    found.sort();
    assert_eq!(found, ["a1", "새 파일.txt"]);
    assert_eq!(app.active_tab().location(), &a);

    // 기록도 그대로 쓸 수 있다.
    assert_eq!(app.execute(CommandId::Forward), Execution::Done);
    settle(&mut app);
    assert_eq!(app.active_tab().location(), &a1);
    assert_eq!(app.execute(CommandId::Back), Execution::Done);
    settle(&mut app);
    assert_eq!(app.execute(CommandId::Back), Execution::Done);
    settle(&mut app);
    assert_eq!(app.active_tab().location(), &root);
}

#[test]
fn 여러_탭을_한_번에_넣으면_넣은_순서의_반대로_되살아난다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    let a = loc(&dir.path().join("a"));
    let b = loc(&dir.path().join("b"));
    let a1 = loc(&dir.path().join("a").join("a1"));
    // 창 닫기가 그 창의 탭을 차례로 넣는 것처럼, 탭 셋을 한 번에 넣는다.
    let tabs = [&a, &b, &a1].map(|l| Tab::new(l.clone(), Sort::default(), false));
    app.remember_closed_tabs(tabs);
    assert_eq!(closed_locations(&app), [a1.clone(), b.clone(), a.clone()]);

    // 그 뒤에 닫은 탭이 먼저다.
    app.execute(CommandId::NewTab);
    app.execute(CommandId::CloseTab);
    let root = loc(dir.path());
    assert_eq!(
        closed_locations(&app),
        [root.clone(), a1.clone(), b.clone(), a.clone()]
    );

    for expected in [&root, &a1, &b, &a] {
        assert_eq!(app.execute(CommandId::ReopenClosedTab), Execution::Done);
        assert_eq!(app.active_tab().location(), expected);
    }
    assert!(!app.can_execute(CommandId::ReopenClosedTab));
    settle(&mut app);
}
