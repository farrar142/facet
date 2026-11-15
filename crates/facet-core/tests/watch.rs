//! 위치의 변경 감시와 목록 자동 갱신, 사라진 위치에서 상위로 옮기기를 공개 인터페이스로 확인한다.
//!
//! 임시 폴더를 실제로 바꾸고, 앱([`App`])이 일꾼 스레드에서 받은 변경을 탭에 반영할 때까지 기다린다.
//! CI 러너가 느려도 흔들리지 않도록 기다리는 시간([`WAIT`])을 넉넉히 두고, 조건이 맞는 즉시 끝낸다.
//! 변경 감시는 Windows(`ReadDirectoryChangesW`)에서만 걸리므로 감시를 쓰는 테스트는 Windows 전용이다.

use std::fs;
use std::path::Path;
use std::time::Duration;
#[cfg(windows)]
use std::time::Instant;

use facet_core::Location;
use facet_core::tab::{AutoRefresh, Tab};
use facet_core::window::App;
#[cfg(windows)]
use facet_core::{
    tab::{ChangeOutcome, Navigation, ReadOutcome, Relocation},
    window::{Update, home_location},
};
use tempfile::TempDir;

/// 변경이 반영되기를 기다리는 넉넉한 시간. 감시와 묶기(100ms)는 보통 수백 ms 안에 끝난다.
const WAIT: Duration = Duration::from_secs(10);

fn loc(path: &Path) -> Location {
    Location::file_system(path)
}

fn names(tab: &Tab) -> Vec<String> {
    tab.items()
        .map(|i| i.name.to_string_lossy().into_owned())
        .collect()
}

fn selected(tab: &Tab) -> Vec<String> {
    tab.selected_items()
        .map(|i| i.name.to_string_lossy().into_owned())
        .collect()
}

/// `path`를 연 앱. 첫 읽기가 끝날 때까지 기다린다.
fn app_at(path: &Path) -> App {
    let mut app = App::new(Some(loc(path)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기 결과가 와야 한다");
    assert_eq!(outcome, facet_core::tab::ReadOutcome::Committed);
    app
}

/// `done`이 참이 될 때까지 앱이 받은 것을 탭에 넘긴다. [`WAIT`] 안에 안 되면 실패한다. 받은 것들을 돌려준다.
#[cfg(windows)]
fn wait_until(app: &mut App, what: &str, done: impl Fn(&Tab) -> bool) -> Vec<Update> {
    let deadline = Instant::now() + WAIT;
    let mut updates = Vec::new();
    while !done(app.active_tab()) {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(
            !left.is_zero(),
            "{what}: 시간 안에 맞지 않았다. 목록: {:?}",
            names(app.active_tab())
        );
        if let Some((_, update)) = app.wait_update(left) {
            updates.push(update);
        }
    }
    updates
}

#[cfg(windows)]
#[test]
fn 만들기_지우기_이름바꾸기가_정해진_시간_안에_목록에_반영된다() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("b.txt"), b"b").unwrap();
    let mut app = app_at(dir.path());
    assert!(matches!(app.active_tab().auto_refresh(), AutoRefresh::On));
    assert_eq!(app.active_tab().watched_location(), Some(&loc(dir.path())));

    fs::write(dir.path().join("a.txt"), b"aaaa").unwrap();
    fs::create_dir(dir.path().join("폴더")).unwrap();
    let updates = wait_until(&mut app, "만들기", |tab| {
        names(tab) == ["폴더", "a.txt", "b.txt"]
    });
    // 바뀐 항목만 반영했다: 다시 읽지 않았다.
    assert!(
        updates
            .iter()
            .all(|u| *u == Update::Changes(ChangeOutcome::Applied)),
        "{updates:?}"
    );
    let a = app
        .active_tab()
        .items()
        .find(|i| i.name == "a.txt")
        .unwrap();
    assert_eq!(a.size, 4);
    assert!(app.active_tab().items().next().unwrap().is_folder);

    fs::rename(dir.path().join("a.txt"), dir.path().join("c.txt")).unwrap();
    wait_until(&mut app, "이름 바꾸기", |tab| {
        names(tab) == ["폴더", "b.txt", "c.txt"]
    });

    fs::remove_file(dir.path().join("b.txt")).unwrap();
    fs::remove_dir(dir.path().join("폴더")).unwrap();
    wait_until(&mut app, "지우기", |tab| names(tab) == ["c.txt"]);

    // 크기가 바뀐 파일도 다시 읽는다.
    fs::write(dir.path().join("c.txt"), b"0123456789").unwrap();
    wait_until(&mut app, "고쳐 쓰기", |tab| {
        tab.items().next().is_some_and(|i| i.size == 10)
    });
}

#[cfg(windows)]
#[test]
fn 갱신_뒤_선택과_스크롤이_유지되고_사라진_항목만_선택에서_빠진다() {
    let dir = TempDir::new().unwrap();
    for name in ["a", "b", "c", "d"] {
        fs::write(dir.path().join(name), name).unwrap();
    }
    let mut app = app_at(dir.path());
    let tab = app.active_tab_mut();
    tab.select_only(0);
    tab.toggle_select(2);
    tab.toggle_select(3);
    tab.set_scroll(1);
    assert_eq!(selected(tab), ["a", "c", "d"]);

    fs::remove_file(dir.path().join("c")).unwrap();
    fs::write(dir.path().join("0 맨 앞"), b"x").unwrap();
    wait_until(&mut app, "갱신", |tab| {
        names(tab) == ["0 맨 앞", "a", "b", "d"]
    });
    let tab = app.active_tab();
    assert_eq!(selected(tab), ["a", "d"]);
    assert_eq!(tab.scroll(), 1);
    // 커서는 마지막으로 토글한 d에 그대로 있고, 앞에 항목이 생겨 번호만 맞춘다.
    assert_eq!(tab.cursor_item().map(|i| i.name.clone()), Some("d".into()));
    assert_eq!(tab.cursor(), Some(3));
    assert_eq!(tab.status().selected_count, 2);
}

#[cfg(windows)]
#[test]
fn 보고_있는_폴더를_지우면_가장_가까운_남은_상위로_옮기고_이유를_갖는다() {
    let root = TempDir::new().unwrap();
    let a = root.path().join("a");
    let deep = a.join("b").join("c");
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep.join("안의 파일.txt"), b"x").unwrap();
    fs::write(a.join("이웃.txt"), b"x").unwrap();
    let mut app = app_at(&deep);
    let back_before = app.active_tab().back_history().to_vec();

    // c와 그 부모 b를 함께 지운다: 남은 가장 가까운 상위는 a다.
    fs::remove_dir_all(a.join("b")).unwrap();
    wait_until(&mut app, "상위로 옮기기", |tab| {
        tab.location() == &loc(&a)
    });
    let tab = app.active_tab();
    assert_eq!(
        tab.relocation(),
        Some(&Relocation::ToParent {
            vanished: loc(&deep)
        })
    );
    assert_eq!(names(tab), ["이웃.txt"]);
    assert!(tab.error().is_none());
    // 사라진 위치는 기록에 넣지 않는다.
    assert_eq!(tab.back_history(), back_before);
    // 옮긴 위치를 다시 감시한다.
    assert_eq!(tab.watched_location(), Some(&loc(&a)));
    assert!(matches!(tab.auto_refresh(), AutoRefresh::On));
    fs::write(a.join("새 파일.txt"), b"x").unwrap();
    wait_until(&mut app, "옮긴 위치의 갱신", |tab| {
        names(tab) == ["새 파일.txt", "이웃.txt"]
    });
    // 변경이 와도 옮긴 까닭은 남는다. 다음 이동을 시작하면 지운다.
    assert!(app.active_tab().relocation().is_some());
    app.navigate(Navigation::Refresh);
    assert!(app.active_tab().relocation().is_none());
}

#[cfg(windows)]
#[test]
fn 보고_있는_폴더만_지우면_부모로_옮긴다() {
    let root = TempDir::new().unwrap();
    let watched = root.path().join("보는 폴더");
    fs::create_dir(&watched).unwrap();
    fs::create_dir(root.path().join("남는 폴더")).unwrap();
    let mut app = app_at(&watched);
    fs::remove_dir(&watched).unwrap();
    wait_until(&mut app, "부모로 옮기기", |tab| {
        tab.location() == &loc(root.path())
    });
    let tab = app.active_tab();
    assert_eq!(names(tab), ["남는 폴더"]);
    assert_eq!(
        tab.relocation().map(Relocation::vanished),
        Some(&loc(&watched))
    );
}

/// 이 기계에 없는 드라이브 문자. 모두 있으면 `None`.
#[cfg(windows)]
fn missing_drive() -> Option<std::path::PathBuf> {
    ('D'..='Z')
        .rev()
        .map(|letter| std::path::PathBuf::from(format!(r"{letter}:\")))
        .find(|root| !root.exists())
}

#[cfg(windows)]
#[test]
fn 상위가_하나도_남지_않으면_홈_폴더로_옮긴다() {
    let Some(drive) = missing_drive() else {
        eprintln!("없는 드라이브 문자가 없어 건너뛴다");
        return;
    };
    let gone = loc(&drive.join("빠진 드라이브").join("폴더"));
    let mut tab = Tab::new(gone.clone(), Default::default(), false);
    let request = tab.reload().expect("기다리는 읽기가 없으면 다시 읽는다");
    assert_eq!(tab.finish(request.run()), ReadOutcome::Committed);
    assert_eq!(tab.location(), &home_location());
    assert_eq!(
        tab.relocation(),
        Some(&Relocation::ToHome { vanished: gone })
    );
}

#[cfg(windows)]
#[test]
fn 다른_위치로_이동하면_이전_위치의_감시가_풀린다() {
    let root = TempDir::new().unwrap();
    let first = root.path().join("첫 폴더");
    let second = root.path().join("둘째 폴더");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    let mut app = app_at(&first);
    assert_eq!(app.active_tab().watched_location(), Some(&loc(&first)));

    app.navigate(Navigation::Go(loc(&second)));
    assert_eq!(app.wait(WAIT).map(|(_, o)| o), Some(ReadOutcome::Committed));
    assert_eq!(app.active_tab().watched_location(), Some(&loc(&second)));

    // 앞 위치의 감시가 살아 있으면 이 변경이 (버려질) 묶음으로라도 온다. 아무것도 오지 않아야 한다.
    fs::write(first.join("뒤에 만든 파일"), b"x").unwrap();
    assert_eq!(app.wait_update(Duration::from_secs(1)), None);

    // 새 위치는 감시한다.
    fs::write(second.join("새 위치의 파일"), b"x").unwrap();
    wait_until(&mut app, "새 위치의 갱신", |tab| {
        names(tab) == ["새 위치의 파일"]
    });
}

/// Windows가 아닌 곳에서는 감시를 걸 수 없다: 「자동 새로 고침 안 됨」이고, 새로 고침은 된다.
#[cfg(not(windows))]
#[test]
fn 감시를_걸_수_없으면_자동_새로_고침_안_됨이다() {
    let dir = TempDir::new().unwrap();
    let mut app = app_at(dir.path());
    let AutoRefresh::Off(error) = app.active_tab().auto_refresh() else {
        panic!("자동 새로 고침 안 됨이어야 한다");
    };
    assert_eq!(error.location(), &loc(dir.path()));
    fs::write(dir.path().join("a"), b"x").unwrap();
    assert!(app.navigate(facet_core::tab::Navigation::Refresh));
    assert_eq!(
        app.wait(WAIT).map(|(_, o)| o),
        Some(facet_core::tab::ReadOutcome::Committed)
    );
    assert_eq!(names(app.active_tab()), ["a"]);
}
