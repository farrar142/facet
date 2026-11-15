//! 한 앱의 창 여러 개: 창 만들기(「새 창」 명령), 닫기(그 창의 탭이 닫은 탭 목록에 들어감), 활성 창 바꾸기,
//! 창이 모두 없어지면 앱이 끝남, 여러 창의 탭이 동시에 읽어도 결과가 제 탭으로 감을 공개 인터페이스로 확인한다.
//!
//! 앱([`App`])은 일꾼 스레드에서 읽으므로 결과를 기다린다. CI 러너가 느려도 흔들리지 않도록 기다리는 시간
//! ([`WAIT`])을 넉넉히 두고, 조건이 맞는 즉시 끝낸다.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::command::{CommandId, Key, Shortcut, find_by_shortcut};
use facet_core::tab::{Navigation, ReadOutcome, Tab};
use facet_core::window::{App, Execution, TabId, WindowId, home_location};
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

/// `path`를 연 앱. 첫 읽기가 끝날 때까지 기다린다.
fn app_at(path: &Path) -> App {
    let mut app = App::new(Some(loc(path)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

/// 모든 창의 모든 탭.
fn all_tabs(app: &App) -> impl Iterator<Item = (TabId, &Tab)> + '_ {
    app.windows()
        .iter()
        .flat_map(|w| w.panels())
        .flat_map(|p| p.tabs())
}

/// 기다리는 읽기가 없어질 때까지 앱이 받은 것을 탭에 넘긴다.
fn settle(app: &mut App) {
    let deadline = Instant::now() + WAIT;
    while all_tabs(app).any(|(_, tab)| tab.pending_location().is_some()) {
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

/// 창 번호들을 차례로.
fn window_ids(app: &App) -> Vec<WindowId> {
    app.windows().iter().map(|w| w.id()).collect()
}

/// 닫은 탭 목록의 위치들, 먼저 되살아날 것부터.
fn closed_locations(app: &App) -> Vec<Location> {
    app.closed_tabs().map(|t| t.location().clone()).collect()
}

#[test]
fn 새_창_명령은_ctrl_n이고_늘_켜져_있다() {
    let command = CommandId::NewWindow;
    assert!(CommandId::ALL.contains(&command));
    assert_eq!(command.as_str(), "new-window");
    assert_eq!(CommandId::from_id("new-window"), Some(command));
    let shortcut = Shortcut::new(Key::Letter('N')).ctrl();
    assert_eq!(command.shortcuts(), [shortcut]);
    assert_eq!(find_by_shortcut(shortcut), Some(command));
    // 한글 모드에서 N 키는 「ㅜ」를 낸다.
    assert_eq!(
        find_by_shortcut(Shortcut::new(Key::from_char('ㅜ').unwrap()).ctrl()),
        Some(command)
    );

    let dir = tree();
    let app = app_at(dir.path());
    assert!(app.can_execute(command));
}

#[test]
fn 새_창은_활성_탭의_위치_탭_하나로_열려_활성_창이_된다() {
    let dir = tree();
    let a = dir.path().join("a");
    let mut app = app_at(dir.path());
    assert!(app.navigate(Navigation::Go(loc(&a))));
    settle(&mut app);
    let first = app.active_window_id();

    assert_eq!(app.execute(CommandId::NewWindow), Execution::Done);
    assert_eq!(app.windows().len(), 2);
    let second = app.active_window_id();
    assert_ne!(first, second);
    assert_eq!(window_ids(&app), vec![first, second]);

    // 새 창: 패널 하나, 탭 하나, 활성 탭과 같은 위치. 기록은 비어 있다.
    let window = app.window(second).expect("새 창");
    assert_eq!(window.panels().len(), 1);
    assert_eq!(window.active_panel().len(), 1);
    assert_eq!(app.active_tab().location(), &loc(&a));
    assert!(!app.active_tab().can_go_back());
    // 새 창의 탭은 그 위치를 읽는다.
    settle(&mut app);
    assert_eq!(names(app.active_tab()), ["a1"]);

    // 먼저 창은 그대로다.
    let old = app.window(first).expect("먼저 창");
    assert_eq!(old.active_panel().active_tab().location(), &loc(&a));
    assert!(old.active_panel().active_tab().can_go_back());
}

#[test]
fn 창_만들기는_준_위치로_열고_활성_창_바꾸기로_명령이_그_창에_간다() {
    let dir = tree();
    let b = dir.path().join("b");
    let mut app = app_at(dir.path());
    let first = app.active_window_id();
    let second = app.open_window(loc(&b));
    settle(&mut app);
    assert_eq!(app.active_window_id(), second);
    assert_eq!(app.active_tab().location(), &loc(&b));

    // 첫 창을 활성으로: 명령은 그 창의 활성 탭에 듣는다.
    assert!(app.activate_window(first));
    assert_eq!(app.active_window_id(), first);
    assert_eq!(app.active_tab().location(), &loc(dir.path()));
    assert_eq!(app.execute(CommandId::NewTab), Execution::Done);
    settle(&mut app);
    assert_eq!(app.window(first).unwrap().active_panel().len(), 2);
    assert_eq!(app.window(second).unwrap().active_panel().len(), 1);

    // 두 번째 창의 탭은 숨지 않는다: 창마다 보이는 탭이 보인다.
    let shown = app.window(second).unwrap().active_panel().active_tab();
    assert!(shown.is_shown());
    assert!(
        app.window(first)
            .unwrap()
            .active_panel()
            .active_tab()
            .is_shown()
    );

    // 활성 탭 바꾸기([`App::activate_tab`])도 그 탭의 창을 활성으로 한다.
    let id = app.window(second).unwrap().active_panel().active_tab_id();
    assert!(app.activate_tab(id));
    assert_eq!(app.active_window_id(), second);

    // 없는 창이면 아무것도 하지 않는다.
    assert!(app.close_window(first));
    assert!(!app.activate_window(first));
    assert_eq!(app.active_window_id(), second);
}

#[test]
fn 창을_닫으면_그_창의_탭들이_닫은_탭_목록에_들어가고_활성_탭이_먼저_되살아난다() {
    let dir = tree();
    let [a, b, c] = ["a", "b", "c"].map(|name| dir.path().join(name));
    let mut app = app_at(dir.path());
    let first = app.active_window_id();

    // 두 번째 창: 왼쪽 패널 [b, c(활성)], 오른쪽 패널 [a].
    let second = app.open_window(loc(&b));
    app.open_tab(loc(&c), true);
    app.split_panel();
    settle(&mut app);
    assert!(app.navigate(Navigation::Go(loc(&a))));
    settle(&mut app);
    // 왼쪽 패널을 활성으로 해서 창의 활성 탭은 c다.
    assert!(app.activate_panel(0));
    assert_eq!(app.active_tab().location(), &loc(&c));

    assert!(app.close_window(second));
    assert_eq!(window_ids(&app), vec![first]);
    // 남은 창이 활성이다.
    assert_eq!(app.active_window_id(), first);
    assert!(app.window(second).is_none());
    assert!(!app.is_finished());

    // 창에 보이던 차례(왼쪽 패널부터)로 넣고, 창의 활성 탭은 마지막에 넣어 먼저 되살아난다.
    assert_eq!(closed_locations(&app), vec![loc(&c), loc(&a), loc(&b)]);
    // 오른쪽 패널의 탭은 기록째 들어간다(dir → a).
    assert!(!app.closed_tabs().nth(1).unwrap().back_history().is_empty());

    // Ctrl+Shift+T로 하나씩 활성 창에 되살아난다.
    assert_eq!(app.execute(CommandId::ReopenClosedTab), Execution::Done);
    assert_eq!(app.active_tab().location(), &loc(&c));
    assert_eq!(app.execute(CommandId::ReopenClosedTab), Execution::Done);
    assert_eq!(app.active_tab().location(), &loc(&a));
    assert!(app.active_tab().can_go_back());
    assert_eq!(app.execute(CommandId::ReopenClosedTab), Execution::Done);
    assert_eq!(app.active_tab().location(), &loc(&b));
    settle(&mut app);
    assert_eq!(app.windows().len(), 1);
    assert_eq!(app.active_window().active_panel().len(), 4);
    assert!(!app.can_execute(CommandId::ReopenClosedTab));

    // 없는 창은 닫지 못한다.
    assert!(!app.close_window(second));
}

#[test]
fn 창의_마지막_탭을_닫으면_창이_닫히고_다른_창이_활성이_된다() {
    let dir = tree();
    let b = dir.path().join("b");
    let mut app = app_at(dir.path());
    let first = app.active_window_id();
    let second = app.open_window(loc(&b));
    settle(&mut app);

    assert_eq!(app.execute(CommandId::CloseTab), Execution::WindowClosed);
    assert_eq!(window_ids(&app), vec![first]);
    assert_eq!(app.active_window_id(), first);
    assert!(app.window(second).is_none());
    assert!(!app.is_finished());
    assert_eq!(closed_locations(&app), vec![loc(&b)]);

    // 활성 창이 앞쪽 창이어도, 뒤쪽 창을 닫은 뒤 활성 창은 그대로다.
    let third = app.open_window(loc(&b));
    assert!(app.activate_window(first));
    let id = app.window(third).unwrap().active_panel().active_tab_id();
    let closed = app.close_tab(id).expect("닫힘");
    assert!(closed.panel && closed.window);
    assert_eq!(app.active_window_id(), first);
}

#[test]
fn 창이_모두_없어지면_앱이_끝나고_새_창_명령으로_다시_연다() {
    let dir = tree();
    let b = dir.path().join("b");
    let mut app = app_at(dir.path());
    let first = app.active_window_id();
    let second = app.open_window(loc(&b));
    settle(&mut app);

    // 첫 창은 닫기 단추로, 두 번째 창은 마지막 탭 닫기로.
    assert!(app.close_window(first));
    assert!(!app.is_finished());
    assert_eq!(app.active_window_id(), second);
    assert_eq!(app.execute(CommandId::CloseTab), Execution::WindowClosed);
    // 창이 모두 없어졌다: 앱이 끝났다(지금은 작업이 없어 바로 끝난다).
    assert!(app.is_finished());
    assert!(app.windows().is_empty());
    assert_eq!(closed_locations(&app), vec![loc(&b), loc(dir.path())]);
    // 끝난 앱에서 탭 명령은 꺼진다.
    assert!(!app.can_execute(CommandId::NewTab));
    assert!(!app.can_execute(CommandId::ReopenClosedTab));
    assert_eq!(app.execute(CommandId::CloseTab), Execution::Disabled);

    // 창이 없을 때의 새 창은 사용자 홈 폴더로 연다(작업만 남은 프로세스에 실행이 오는 경우).
    assert!(app.can_execute(CommandId::NewWindow));
    assert_eq!(app.execute(CommandId::NewWindow), Execution::Done);
    assert!(!app.is_finished());
    assert_eq!(app.windows().len(), 1);
    assert_eq!(app.active_tab().location(), &home_location());
    settle(&mut app);
}

#[test]
fn 여러_창의_탭이_동시에_읽어도_결과가_제_탭으로_간다() {
    let dir = tree();
    let folders = ["a", "b", "c"].map(|name| {
        let folder = dir.path().join(name);
        // 폴더마다 다른 이름의 파일을 많이 넣어 읽기가 겹치게 한다.
        for i in 0..200 {
            fs::write(folder.join(format!("{name}-{i:03}.txt")), b"x").unwrap();
        }
        folder
    });
    let mut app = app_at(dir.path());
    let first = app.active_window_id();

    // 창 셋, 창마다 탭 둘(보이는 탭 하나, 숨은 탭 하나). 보이는 탭들이 한꺼번에 읽는다.
    let mut windows = vec![first];
    for folder in &folders[1..] {
        windows.push(app.open_window(loc(folder)));
    }
    assert!(app.activate_window(first));
    assert!(app.navigate(Navigation::Go(loc(&folders[0]))));
    settle(&mut app);

    // 동시에: 창마다 보이는 탭이 다른 폴더(오른쪽으로 하나씩 민 것)로 간다.
    let mut expected: Vec<(TabId, &Path)> = Vec::new();
    for (i, window) in windows.iter().enumerate() {
        assert!(app.activate_window(*window));
        let target = &folders[(i + 1) % folders.len()];
        assert!(app.navigate(Navigation::Go(loc(target))));
        expected.push((app.active_tab_id(), target));
    }
    settle(&mut app);
    for (id, folder) in expected {
        let tab = app.tab(id).expect("열린 탭");
        assert_eq!(tab.location(), &loc(folder));
        let name = folder.file_name().unwrap().to_string_lossy().into_owned();
        let mut items = names(tab);
        let mut on_disk: Vec<String> = fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        items.sort();
        on_disk.sort();
        assert_eq!(items, on_disk, "{name}");
        let prefix = format!("{name}-");
        assert_eq!(items.iter().filter(|i| i.starts_with(&prefix)).count(), 200);
    }
}
