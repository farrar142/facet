//! 탭 옮기기([`App::move_tab`]): 같은 패널 안 순서 바꾸기, 반대편 패널로, 다른 창으로, 새 창으로를 공개
//! 인터페이스로 확인한다. 탭 순서, 활성 탭·패널·창, 기록·선택·정렬·읽은 목록이 따라가는 것, 떠난 패널·창이 비면
//! 닫히는 것, 보이던 탭은 다시 읽지 않고 숨어 있던 탭은 보이며 다시 읽는 것을 본다.
//!
//! 앱([`App`])은 일꾼 스레드에서 읽으므로 결과를 기다린다. CI 러너가 느려도 흔들리지 않도록 기다리는 시간
//! ([`WAIT`])을 넉넉히 두고, 조건이 맞는 즉시 끝낸다.

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::details::{Column, Direction, Sort};
use facet_core::tab::{Navigation, ReadOutcome, Tab};
use facet_core::window::{App, Closed, Moved, TabDestination, TabId, WindowId};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(10);

const KEPT: Closed = Closed {
    panel: false,
    window: false,
};

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

fn tab(app: &App, id: TabId) -> &Tab {
    app.tab(id).expect("열린 탭")
}

fn names(tab: &Tab) -> Vec<String> {
    tab.items()
        .map(|i| i.name.to_string_lossy().into_owned())
        .collect()
}

/// 창 `window`의 `panel`번째 패널의 탭 번호들.
fn panel_ids(app: &App, window: WindowId, panel: usize) -> Vec<TabId> {
    app.window(window).expect("열린 창").panels()[panel]
        .tabs()
        .map(|(id, _)| id)
        .collect()
}

fn to(window: WindowId, panel: usize, slot: usize) -> TabDestination {
    TabDestination::Panel {
        window,
        panel,
        slot,
    }
}

/// `root` 탭 하나로 시작해 `a`·`b` 탭을 차례로 열어 다 읽은 앱과 세 탭의 번호. 활성 탭은 `b`다.
fn three_tabs(dir: &Path) -> (App, [TabId; 3]) {
    let mut app = app_at(dir);
    let root = app.active_tab_id();
    let a = app.open_tab(loc(&dir.join("a")), true);
    let b = app.open_tab(loc(&dir.join("b")), true);
    settle(&mut app);
    (app, [root, a, b])
}

#[test]
fn 같은_패널_안에서_옮기면_순서가_바뀌고_옮긴_탭이_활성이_된다() {
    let dir = tree();
    let (mut app, [root, a, b]) = three_tabs(dir.path());
    let w = app.active_window_id();
    assert_eq!(panel_ids(&app, w, 0), [root, a, b]);

    // 숨은 첫 탭을 끝으로 옮긴다: 활성이 되어 갖고 있던 목록을 보이며 다시 읽는다. 보이던 b는 숨는다.
    let moved = app.move_tab(root, to(w, 0, 3));
    assert_eq!(
        moved,
        Some(Moved {
            window: w,
            closed: KEPT
        })
    );
    assert_eq!(panel_ids(&app, w, 0), [a, b, root]);
    assert_eq!(app.active_tab_id(), root);
    assert_eq!(app.active_window().active_panel().active_index(), 2);
    let shown = tab(&app, root);
    assert!(shown.is_shown());
    assert_eq!(shown.pending_location(), Some(&loc(dir.path())));
    assert_eq!(names(shown), ["a", "b", "c", "파일.txt"]);
    assert!(!tab(&app, b).is_shown());
    settle(&mut app);
    assert_eq!(tab(&app, root).watched_location(), Some(&loc(dir.path())));
    assert_eq!(tab(&app, b).watched_location(), None);

    // 옮기는 탭 바로 앞뒤 자리는 순서를 바꾸지 않는다. 보이던 탭이라 다시 읽지 않는다.
    for slot in [2, 3] {
        app.move_tab(root, to(w, 0, slot)).unwrap();
        assert_eq!(panel_ids(&app, w, 0), [a, b, root]);
        assert_eq!(app.active_tab_id(), root);
        assert_eq!(tab(&app, root).pending_location(), None);
    }

    // 앞으로 옮긴다. 탭 수를 넘는 자리는 끝이다.
    app.move_tab(b, to(w, 0, 0)).unwrap();
    assert_eq!(panel_ids(&app, w, 0), [b, a, root]);
    assert_eq!(app.active_tab_id(), b);
    assert!(!tab(&app, root).is_shown());
    settle(&mut app);
    app.move_tab(b, to(w, 0, 99)).unwrap();
    assert_eq!(panel_ids(&app, w, 0), [a, root, b]);
    assert_eq!(app.active_tab_id(), b);
    // 보이던 탭은 다시 읽지 않고 감시도 그대로다.
    let b_tab = tab(&app, b);
    assert_eq!(b_tab.pending_location(), None);
    assert_eq!(b_tab.watched_location(), Some(&loc(&dir.path().join("b"))));

    // 활성이 아닌 탭끼리 자리를 바꿔도 옮긴 탭이 활성이 된다.
    app.move_tab(a, to(w, 0, 2)).unwrap();
    assert_eq!(panel_ids(&app, w, 0), [root, a, b]);
    assert_eq!(app.active_tab_id(), a);
    assert_eq!(
        all_tabs(&app).filter(|(_, t)| t.is_shown()).count(),
        1,
        "패널에서는 활성 탭만 보인다"
    );
}

#[test]
fn 반대편_패널로_옮기면_기록_선택_정렬_목록이_따라가고_다시_읽지_않는다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    let w = app.active_window_id();
    let root = app.active_tab_id();
    let right = app.split_panel().unwrap();
    settle(&mut app);
    app.activate_panel(0);
    let moving = app.open_tab(loc(dir.path()), true);
    settle(&mut app);
    // 옮길 탭: a로 들어갔다가 돌아와 앞으로 기록을 두고, b를 고르고, 스크롤하고, 정렬을 바꾼다.
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
    app.active_tab_mut().set_scroll(5);
    let sort = Sort {
        column: Column::Name,
        direction: Direction::Descending,
    };
    app.set_sort(sort);
    let before = names(app.active_tab());
    assert_eq!(panel_ids(&app, w, 0), [root, moving]);
    assert_eq!(panel_ids(&app, w, 1), [right]);

    let moved = app.move_tab(moving, to(w, 1, 0));
    assert_eq!(
        moved,
        Some(Moved {
            window: w,
            closed: KEPT
        })
    );
    assert_eq!(panel_ids(&app, w, 0), [root]);
    assert_eq!(panel_ids(&app, w, 1), [moving, right]);
    assert_eq!(app.active_window().active_panel_index(), 1);
    assert_eq!(app.active_tab_id(), moving);
    let moved_tab = tab(&app, moving);
    assert_eq!(
        moved_tab.pending_location(),
        None,
        "보이던 탭은 다시 읽지 않는다"
    );
    assert_eq!(moved_tab.watched_location(), Some(&loc(dir.path())));
    assert_eq!(names(moved_tab), before);
    assert_eq!(moved_tab.location(), &loc(dir.path()));
    assert_eq!(moved_tab.forward_history(), [loc(&dir.path().join("a"))]);
    assert_eq!(moved_tab.sort(), sort);
    assert_eq!(moved_tab.scroll(), 5);
    assert_eq!(
        moved_tab
            .selected_items()
            .map(|i| i.name.clone())
            .collect::<Vec<_>>(),
        [OsString::from("b")]
    );
    // 오른쪽에서 보이던 탭은 숨고, 왼쪽에서는 이웃(남은 탭)이 다시 보이며 읽는다.
    assert!(!tab(&app, right).is_shown());
    assert!(tab(&app, root).is_shown());
    assert_eq!(tab(&app, root).pending_location(), Some(&loc(dir.path())));
    settle(&mut app);

    // 오른쪽 패널의 탭을 모두 왼쪽으로 옮기면 오른쪽 패널이 닫힌다.
    app.move_tab(right, to(w, 0, 0)).unwrap();
    assert_eq!(panel_ids(&app, w, 0), [right, root]);
    assert_eq!(app.active_window().active_panel_index(), 0);
    let moved = app.move_tab(moving, to(w, 0, 1));
    assert_eq!(
        moved,
        Some(Moved {
            window: w,
            closed: Closed {
                panel: true,
                window: false
            }
        })
    );
    let window = app.active_window();
    assert!(!window.is_split());
    assert_eq!(panel_ids(&app, w, 0), [right, moving, root]);
    assert_eq!(app.active_tab_id(), moving);
    assert_eq!(tab(&app, moving).pending_location(), None);
}

#[test]
fn 왼쪽_패널의_마지막_탭을_오른쪽으로_옮기면_왼쪽_패널이_닫힌다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    let w = app.active_window_id();
    let left = app.active_tab_id();
    let right = app.split_panel().unwrap();
    settle(&mut app);

    let moved = app.move_tab(left, to(w, 1, 1)).unwrap();
    assert_eq!(
        moved.closed,
        Closed {
            panel: true,
            window: false
        }
    );
    let window = app.active_window();
    assert!(!window.is_split());
    assert_eq!(window.active_panel_index(), 0);
    assert_eq!(panel_ids(&app, w, 0), [right, left]);
    assert_eq!(app.active_tab_id(), left);
    assert!(!tab(&app, right).is_shown());
}

#[test]
fn 다른_창으로_옮기면_그_창이_활성이_되고_떠난_창이_비면_닫힌다() {
    let dir = tree();
    let (mut app, [root, a, b]) = three_tabs(dir.path());
    let first = app.active_window_id();
    let second = app.open_window(loc(&dir.path().join("c")));
    let c = app.active_tab_id();
    settle(&mut app);
    // 첫 창의 숨은 탭 a를 둘째 창의 끝으로 옮긴다.
    app.activate_window(first);
    let moved = app.move_tab(a, to(second, 0, 1));
    assert_eq!(
        moved,
        Some(Moved {
            window: second,
            closed: KEPT
        })
    );
    assert_eq!(app.active_window_id(), second);
    assert_eq!(panel_ids(&app, first, 0), [root, b]);
    assert_eq!(panel_ids(&app, second, 0), [c, a]);
    assert_eq!(app.active_tab_id(), a);
    // 첫 창에서는 b가 그대로 보인다(옮긴 탭은 활성이 아니었다).
    assert!(tab(&app, b).is_shown());
    assert_eq!(tab(&app, b).pending_location(), None);
    assert!(!tab(&app, c).is_shown());
    // 숨어 있던 탭이라 갖고 있던 목록을 보이며 다시 읽는다.
    let shown = tab(&app, a);
    assert!(shown.is_shown());
    assert_eq!(shown.pending_location(), Some(&loc(&dir.path().join("a"))));
    assert_eq!(names(shown), ["a1"]);
    settle(&mut app);
    assert_eq!(
        tab(&app, a).watched_location(),
        Some(&loc(&dir.path().join("a")))
    );

    // 첫 창의 탭을 모두 옮기면 첫 창이 닫힌다. 앱은 끝나지 않는다.
    app.move_tab(root, to(second, 0, 0)).unwrap();
    let moved = app.move_tab(b, to(second, 0, 0)).unwrap();
    assert_eq!(
        moved,
        Moved {
            window: second,
            closed: Closed {
                panel: true,
                window: true
            }
        }
    );
    assert!(app.window(first).is_none());
    assert!(!app.is_finished());
    assert_eq!(app.active_window_id(), second);
    assert_eq!(panel_ids(&app, second, 0), [b, root, c, a]);
    assert_eq!(app.active_tab_id(), b);
    // 옮긴 탭은 닫은 탭 목록에 들어가지 않는다.
    assert_eq!(app.closed_tabs().len(), 0);
}

#[test]
fn 새_창으로_옮기면_그_탭_하나로_창이_열려_활성이_된다() {
    let dir = tree();
    let (mut app, [root, a, b]) = three_tabs(dir.path());
    let first = app.active_window_id();

    let moved = app.move_tab(b, TabDestination::NewWindow).unwrap();
    assert_eq!(moved.closed, KEPT);
    let new = moved.window;
    assert_ne!(new, first);
    assert_eq!(app.windows().len(), 2);
    assert_eq!(app.active_window_id(), new);
    assert_eq!(panel_ids(&app, new, 0), [b]);
    assert_eq!(app.active_tab_id(), b);
    assert!(!app.active_window().is_split());
    // 보이던 탭은 다시 읽지 않는다. 떠난 창에서는 이웃이 보이며 읽는다.
    assert_eq!(tab(&app, b).pending_location(), None);
    assert_eq!(names(tab(&app, b)), Vec::<String>::new());
    assert_eq!(panel_ids(&app, first, 0), [root, a]);
    assert!(tab(&app, a).is_shown());
    settle(&mut app);

    // 창의 마지막 탭을 새 창으로 옮기면 떠난 창이 닫힌다.
    app.move_tab(root, to(new, 0, 0)).unwrap();
    let moved = app.move_tab(a, TabDestination::NewWindow).unwrap();
    assert_eq!(
        moved.closed,
        Closed {
            panel: true,
            window: true
        }
    );
    assert!(app.window(first).is_none());
    assert_eq!(app.windows().len(), 2);
    assert_eq!(app.active_window_id(), moved.window);
    assert_eq!(panel_ids(&app, moved.window, 0), [a]);
    assert_eq!(panel_ids(&app, new, 0), [root, b]);
}

#[test]
fn 없는_탭_창_패널로는_옮기지_않는다() {
    let dir = tree();
    let (mut app, [root, a, b]) = three_tabs(dir.path());
    let w = app.active_window_id();
    let other = app.open_window(loc(dir.path()));
    settle(&mut app);
    app.close_window(other);
    app.activate_window(w);

    // 닫힌 창, 나뉘지 않은 창의 오른쪽 패널.
    assert_eq!(app.move_tab(a, to(other, 0, 0)), None);
    assert_eq!(app.move_tab(a, to(w, 1, 0)), None);
    // 닫힌 탭.
    app.close_tab(b);
    assert_eq!(app.move_tab(b, to(w, 0, 0)), None);
    assert_eq!(panel_ids(&app, w, 0), [root, a]);
    assert_eq!(app.active_tab_id(), a);
}
