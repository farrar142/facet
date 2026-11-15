//! 창의 패널 둘: 나누기·합치기, 활성 패널 바꾸기, 분할 비율, 패널의 마지막 탭 닫기를 공개 인터페이스로 확인한다.
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
use facet_core::window::{
    App, Closed, Execution, SPLIT_RATIO_DEFAULT, SPLIT_RATIO_MAX, SPLIT_RATIO_MIN, TabId,
};
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

/// 창의 `panel`번째 패널의 탭 번호들.
fn panel_ids(app: &App, panel: usize) -> Vec<TabId> {
    app.active_window().panels()[panel]
        .tabs()
        .map(|(id, _)| id)
        .collect()
}

fn tab(app: &App, id: TabId) -> &Tab {
    app.tab(id).expect("열린 탭")
}

fn names(tab: &Tab) -> Vec<String> {
    tab.items()
        .map(|i| i.name.to_string_lossy().into_owned())
        .collect()
}

/// `a`로 옮겨 다 읽을 때까지 기다린다.
fn open_a(app: &mut App, dir: &Path) {
    assert!(app.navigate(Navigation::Go(loc(&dir.join("a")))));
    settle(app);
}

#[test]
fn 나누면_오른쪽_새_패널이_활성_탭과_같은_위치의_탭_하나로_시작하고_활성이_된다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    open_a(&mut app, dir.path());
    let sort = Sort {
        column: Column::Size,
        direction: Direction::Descending,
    };
    app.set_sort(sort);
    let left = app.active_tab_id();
    assert_eq!(app.active_window().panels().len(), 1);
    assert!(!app.active_window().is_split());

    let right = app.split_panel().expect("나눴다");
    let window = app.active_window();
    assert!(window.is_split());
    assert_eq!(window.panels().len(), 2);
    assert_eq!(window.active_panel_index(), 1);
    assert_eq!(panel_ids(&app, 0), [left]);
    assert_eq!(panel_ids(&app, 1), [right]);
    assert_eq!(app.active_tab_id(), right);

    // 새 탭은 활성 탭과 같은 위치를 읽고 정렬도 따른다. 기록은 비어 있다.
    let new = tab(&app, right);
    assert_eq!(new.pending_location(), Some(&loc(&dir.path().join("a"))));
    assert_eq!(new.sort(), sort);
    settle(&mut app);
    let new = tab(&app, right);
    assert_eq!(new.location(), &loc(&dir.path().join("a")));
    assert_eq!(names(new), ["a1"]);
    assert!(!new.can_go_back());

    // 두 패널의 활성 탭은 모두 보인다.
    assert!(tab(&app, left).is_shown());
    assert!(new.is_shown());

    // 패널은 최대 둘이다.
    assert_eq!(app.split_panel(), None);
    assert_eq!(app.active_window().panels().len(), 2);
}

#[test]
fn 합치면_오른쪽_패널의_탭이_왼쪽_탭_끝에_순서대로_붙는다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    let l1 = app.active_tab_id();
    let l2 = app.open_tab(loc(&dir.path().join("a")), true);
    let r1 = app.split_panel().unwrap();
    let r2 = app.open_tab(loc(&dir.path().join("b")), true);
    let r3 = app.open_tab(loc(&dir.path().join("a").join("a1")), false);
    settle(&mut app);
    assert_eq!(panel_ids(&app, 0), [l1, l2]);
    assert_eq!(panel_ids(&app, 1), [r1, r2, r3]);

    // 오른쪽 패널이 활성이고 그 활성 탭은 r2다.
    assert!(app.merge_panels());
    let window = app.active_window();
    assert_eq!(window.panels().len(), 1);
    assert!(!window.is_split());
    assert_eq!(window.active_panel_index(), 0);
    assert_eq!(panel_ids(&app, 0), [l1, l2, r1, r2, r3]);
    // 활성 패널의 활성 탭이 그대로 활성이고, 다른 패널에서 보이던 탭은 숨는다.
    assert_eq!(app.active_tab_id(), r2);
    assert!(tab(&app, r2).is_shown());
    assert!(!tab(&app, l2).is_shown());
    assert_eq!(tab(&app, l2).watched_location(), None);

    // 하나뿐이면 합칠 것이 없다.
    assert!(!app.merge_panels());
}

#[test]
fn 왼쪽이_활성일_때_합치면_왼쪽_활성_탭이_그대로_활성이다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    let l1 = app.active_tab_id();
    let r1 = app.split_panel().unwrap();
    settle(&mut app);
    assert!(app.activate_panel(0));
    assert!(app.merge_panels());
    assert_eq!(panel_ids(&app, 0), [l1, r1]);
    assert_eq!(app.active_tab_id(), l1);
    assert!(!tab(&app, r1).is_shown());
}

#[test]
fn 나누기_합치기_명령은_패널_수에_따라_바꾼다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    assert!(app.can_execute(CommandId::TogglePanelSplit));
    assert_eq!(app.execute(CommandId::TogglePanelSplit), Execution::Done);
    assert_eq!(app.active_window().panels().len(), 2);
    assert!(app.can_execute(CommandId::TogglePanelSplit));
    assert_eq!(app.execute(CommandId::TogglePanelSplit), Execution::Done);
    assert_eq!(app.active_window().panels().len(), 1);
    assert_eq!(app.active_window().panels()[0].len(), 2);
}

#[test]
fn 활성_패널은_누르거나_tab_명령으로_바뀌고_명령은_활성_패널의_활성_탭으로_간다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    let left = app.active_tab_id();

    // 패널이 하나면 Tab 명령은 꺼져 있고 아무것도 하지 않는다.
    assert!(!app.can_execute(CommandId::SwitchPanel));
    assert_eq!(app.execute(CommandId::SwitchPanel), Execution::Disabled);
    assert!(!app.switch_panel());
    assert_eq!(app.active_tab_id(), left);

    let right = app.split_panel().unwrap();
    settle(&mut app);
    assert!(app.can_execute(CommandId::SwitchPanel));
    assert_eq!(app.execute(CommandId::SwitchPanel), Execution::Done);
    assert_eq!(app.active_window().active_panel_index(), 0);
    assert_eq!(app.active_tab_id(), left);
    assert_eq!(app.execute(CommandId::SwitchPanel), Execution::Done);
    assert_eq!(app.active_tab_id(), right);

    // 패널을 누르면 그 패널이 활성이다. 없는 패널이면 아무것도 하지 않는다.
    assert!(app.activate_panel(0));
    assert_eq!(app.active_tab_id(), left);
    assert!(!app.activate_panel(2));
    assert_eq!(app.active_tab_id(), left);

    // 명령은 활성 패널의 활성 탭에 듣는다: 왼쪽 탭만 a로 간다.
    assert!(app.navigate(Navigation::Go(loc(&dir.path().join("a")))));
    settle(&mut app);
    assert_eq!(tab(&app, left).location(), &loc(&dir.path().join("a")));
    assert_eq!(tab(&app, right).location(), &loc(dir.path()));
    // 새 탭은 활성 패널에 열린다.
    assert_eq!(app.execute(CommandId::NewTab), Execution::Done);
    assert_eq!(app.active_window().panels()[0].len(), 2);
    assert_eq!(app.active_window().panels()[1].len(), 1);
    // 다른 패널로 바꿔도 각 패널의 보이는 탭은 그대로 보인다.
    assert!(app.activate_panel(1));
    assert!(tab(&app, right).is_shown());
    assert!(app.active_window().panels()[0].active_tab().is_shown());
}

#[test]
fn 분할_비율은_한도_안으로_잘리고_반반으로_맞출_수_있다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    assert_eq!(app.active_window().split_ratio(), SPLIT_RATIO_DEFAULT);
    assert_eq!(SPLIT_RATIO_DEFAULT, 0.5);
    app.split_panel();

    let window = app.active_window_mut();
    assert_eq!(window.set_split_ratio(0.3), 0.3);
    assert_eq!(window.split_ratio(), 0.3);
    // 어느 패널도 너무 좁아지지 않게 한도 안으로 자른다.
    assert_eq!(window.set_split_ratio(0.0), SPLIT_RATIO_MIN);
    assert_eq!(window.set_split_ratio(-3.0), SPLIT_RATIO_MIN);
    assert_eq!(window.set_split_ratio(1.0), SPLIT_RATIO_MAX);
    assert_eq!(window.set_split_ratio(f32::INFINITY), SPLIT_RATIO_MAX);
    // 숫자가 아니면 바꾸지 않는다.
    assert_eq!(window.set_split_ratio(f32::NAN), SPLIT_RATIO_MAX);
    assert!(SPLIT_RATIO_MIN > 0.0 && SPLIT_RATIO_MAX < 1.0);

    window.reset_split_ratio();
    assert_eq!(window.split_ratio(), 0.5);

    // 비율은 창의 상태라 합쳐도 남는다(다시 나누면 그 비율로).
    app.active_window_mut().set_split_ratio(0.7);
    app.merge_panels();
    assert_eq!(app.active_window().split_ratio(), 0.7);
}

#[test]
fn 패널의_마지막_탭을_닫으면_그_패널이_닫히고_남은_패널이_활성이_된다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    let l1 = app.active_tab_id();
    let r1 = app.split_panel().unwrap();
    settle(&mut app);

    // 오른쪽(활성) 패널의 마지막 탭.
    assert_eq!(
        app.close_tab(r1),
        Some(Closed {
            panel: true,
            window: false
        })
    );
    let window = app.active_window();
    assert_eq!(window.panels().len(), 1);
    assert_eq!(window.active_panel_index(), 0);
    assert_eq!(app.active_tab_id(), l1);

    // 왼쪽(활성) 패널의 마지막 탭을 닫으면 오른쪽 패널이 남아 활성이 된다.
    let r2 = app.split_panel().unwrap();
    let r3 = app.open_tab(loc(&dir.path().join("a")), true);
    settle(&mut app);
    assert!(app.activate_panel(0));
    assert_eq!(app.execute(CommandId::CloseTab), Execution::Done);
    let window = app.active_window();
    assert_eq!(window.panels().len(), 1);
    assert_eq!(window.active_panel_index(), 0);
    assert_eq!(panel_ids(&app, 0), [r2, r3]);
    assert_eq!(app.active_tab_id(), r3);
    assert!(!app.is_finished());

    // 비활성 패널의 마지막 탭을 닫아도 활성 패널은 그대로다.
    let r4 = app.split_panel().unwrap();
    settle(&mut app);
    assert!(app.activate_panel(0));
    app.close_tab(r4);
    assert_eq!(app.active_window().panels().len(), 1);
    assert_eq!(app.active_tab_id(), r3);
}

#[test]
fn 패널_명령의_단축키() {
    let shortcut = |key: Key| Shortcut::new(key);
    assert_eq!(
        CommandId::TogglePanelSplit.shortcuts(),
        [shortcut(Key::Backslash).ctrl()]
    );
    assert_eq!(CommandId::SwitchPanel.shortcuts(), [shortcut(Key::Tab)]);
    assert_eq!(
        find_by_shortcut(shortcut(Key::Backslash).ctrl()),
        Some(CommandId::TogglePanelSplit)
    );
    assert_eq!(
        find_by_shortcut(shortcut(Key::Tab)),
        Some(CommandId::SwitchPanel)
    );
    // Ctrl+Tab은 여전히 다음 탭이다.
    assert_eq!(
        find_by_shortcut(shortcut(Key::Tab).ctrl()),
        Some(CommandId::NextTab)
    );
    // 백슬래시 키: 글자 「\」, 한국어 배열에서 그 키가 내는 「₩」(U+20A9)·전각 「￦」(U+FFE6),
    // Ctrl과 함께 오는 제어 글자(U+001C)는 모두 같은 키다.
    for c in ['\\', '₩', '￦', '\u{1c}'] {
        assert_eq!(Key::from_char(c), Some(Key::Backslash), "{c:?}");
    }
    assert_eq!(
        CommandId::from_id("toggle-panel-split"),
        Some(CommandId::TogglePanelSplit)
    );
    assert_eq!(
        CommandId::from_id("switch-panel"),
        Some(CommandId::SwitchPanel)
    );
}
