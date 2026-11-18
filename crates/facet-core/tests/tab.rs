//! 탭 상태와 위치 이동을 공개 인터페이스로만 확인한다. 임시 폴더에 실제 파일과 폴더를 만들고 결과를 본다.
//!
//! 탭([`Tab`])만 따로 다루는 테스트는 읽기 요청을 이 스레드에서 돌려 결과가 오는 순서를 정한다.
//! 앱([`App`])을 거치는 테스트는 실제로 일꾼 스레드에서 읽는다.

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use facet_core::details::{Column, Direction, Sort};
use facet_core::tab::{Navigation, READING_DELAY, ReadOutcome, Tab};
use facet_core::window::{App, Update, home_location};
use facet_core::{ListError, Location};
use tempfile::TempDir;

/// 결과를 기다리는 넉넉한 시간. 넘으면 일꾼 스레드가 결과를 보내지 않은 것이다.
const WAIT: Duration = Duration::from_secs(10);

fn loc(path: &Path) -> Location {
    Location::file_system(path)
}

/// `root/a/a1`, `root/b`, `root/파일.txt`가 있는 임시 폴더.
fn tree() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("a").join("a1")).unwrap();
    fs::write(dir.path().join("a").join("a.txt"), b"a").unwrap();
    fs::create_dir(dir.path().join("b")).unwrap();
    fs::write(dir.path().join("b").join("b.txt"), b"b").unwrap();
    fs::write(dir.path().join("파일.txt"), b"x").unwrap();
    dir
}

fn names(tab: &Tab) -> Vec<String> {
    tab.items()
        .map(|i| i.name.to_string_lossy().into_owned())
        .collect()
}

/// 이동을 시작하고 그 요청을 곧바로 이 스레드에서 읽어 넘긴다.
fn go(tab: &mut Tab, navigation: Navigation) -> ReadOutcome {
    let request = tab.navigate(navigation).expect("이동할 수 있어야 한다");
    tab.finish(request.run())
}

fn tab_at(path: &Path) -> Tab {
    let mut tab = Tab::new(loc(path), Sort::default(), false);
    assert_eq!(go(&mut tab, Navigation::Refresh), ReadOutcome::Committed);
    tab
}

#[test]
fn 열기_뒤로_앞으로_위로_뒤의_위치와_목록() {
    let dir = tree();
    let root = dir.path();
    let mut tab = tab_at(root);
    assert_eq!(names(&tab), ["a", "b", "파일.txt"]);
    assert!(!tab.can_go_back() && !tab.can_go_forward());

    // 열기: 폴더 안으로.
    assert_eq!(
        go(&mut tab, Navigation::Open(OsString::from("a"))),
        ReadOutcome::Committed
    );
    assert_eq!(tab.location(), &loc(&root.join("a")));
    assert_eq!(names(&tab), ["a1", "a.txt"]);
    assert_eq!(tab.back_history(), [loc(root)]);

    go(&mut tab, Navigation::Open(OsString::from("a1")));
    assert_eq!(tab.location(), &loc(&root.join("a").join("a1")));
    assert!(tab.is_empty());
    assert_eq!(tab.back_history(), [loc(root), loc(&root.join("a"))]);

    // 뒤로 두 번.
    assert_eq!(go(&mut tab, Navigation::Back), ReadOutcome::Committed);
    assert_eq!(tab.location(), &loc(&root.join("a")));
    assert_eq!(names(&tab), ["a1", "a.txt"]);
    go(&mut tab, Navigation::Back);
    assert_eq!(tab.location(), &loc(root));
    assert!(!tab.can_go_back());
    assert_eq!(
        tab.forward_history(),
        [loc(&root.join("a").join("a1")), loc(&root.join("a"))]
    );
    assert!(
        tab.navigate(Navigation::Back).is_none(),
        "뒤로 기록이 비었다"
    );

    // 앞으로.
    assert_eq!(go(&mut tab, Navigation::Forward), ReadOutcome::Committed);
    assert_eq!(tab.location(), &loc(&root.join("a")));
    assert_eq!(names(&tab), ["a1", "a.txt"]);
    assert_eq!(tab.back_history(), [loc(root)]);
    assert_eq!(tab.forward_history(), [loc(&root.join("a").join("a1"))]);

    // 위로: 새 위치로 가는 것이라 앞으로 기록을 버린다.
    assert!(tab.can_go_up());
    assert_eq!(go(&mut tab, Navigation::Up), ReadOutcome::Committed);
    assert_eq!(tab.location(), &loc(root));
    assert_eq!(names(&tab), ["a", "b", "파일.txt"]);
    assert_eq!(tab.back_history(), [loc(root), loc(&root.join("a"))]);
    assert!(tab.forward_history().is_empty());
}

#[test]
fn 새_위치로_가면_앞으로_기록을_버린다() {
    let dir = tree();
    let root = dir.path();
    let mut tab = tab_at(root);
    go(&mut tab, Navigation::Open(OsString::from("a")));
    go(&mut tab, Navigation::Open(OsString::from("a1")));
    go(&mut tab, Navigation::Back);
    go(&mut tab, Navigation::Back);
    assert_eq!(tab.forward_history().len(), 2);

    go(&mut tab, Navigation::Go(loc(&root.join("b"))));
    assert_eq!(tab.location(), &loc(&root.join("b")));
    assert_eq!(names(&tab), ["b.txt"]);
    assert!(tab.forward_history().is_empty());
    assert!(!tab.can_go_forward());
    assert!(tab.navigate(Navigation::Forward).is_none());
    assert_eq!(tab.back_history(), [loc(root)]);
}

#[test]
fn 같은_위치로_가면_기록을_건드리지_않는다() {
    let dir = tree();
    let mut tab = tab_at(dir.path());
    go(&mut tab, Navigation::Go(loc(dir.path())));
    assert!(tab.back_history().is_empty());
}

#[test]
fn 실패한_이동은_원래_위치에_남고_오류를_갖고_기록에_남지_않는다() {
    let dir = tree();
    let root = dir.path();
    let mut tab = tab_at(root);
    go(&mut tab, Navigation::Open(OsString::from("a")));
    go(&mut tab, Navigation::Back);
    let back_before = tab.back_history().to_vec();
    let forward_before = tab.forward_history().to_vec();
    let items_before = names(&tab);

    // 없는 위치.
    assert_eq!(
        go(&mut tab, Navigation::Open(OsString::from("없는 폴더"))),
        ReadOutcome::Failed
    );
    assert_eq!(tab.location(), &loc(root));
    assert!(matches!(
        tab.error(),
        Some(ListError::NotFound { location }) if *location == loc(&root.join("없는 폴더"))
    ));
    assert_eq!(tab.back_history(), back_before);
    assert_eq!(
        tab.forward_history(),
        forward_before,
        "앞으로 기록도 그대로"
    );
    assert_eq!(names(&tab), items_before, "목록도 그대로");
    assert!(tab.pending_location().is_none());

    // 폴더가 아닌 곳(파일).
    assert_eq!(
        go(&mut tab, Navigation::Open(OsString::from("파일.txt"))),
        ReadOutcome::Failed
    );
    assert_eq!(tab.location(), &loc(root));
    assert!(tab.error().is_some());
    assert_eq!(tab.back_history(), back_before);

    // 다음 이동을 시작하면 오류를 지운다.
    assert_eq!(go(&mut tab, Navigation::Forward), ReadOutcome::Committed);
    assert!(tab.error().is_none());
    assert_eq!(tab.location(), &loc(&root.join("a")));
    assert_eq!(tab.back_history(), [loc(root)]);
}

#[test]
fn 뒤로_갈_곳이_사라졌으면_원래_위치에_남고_기록도_그대로다() {
    let dir = tree();
    let root = dir.path();
    let mut tab = tab_at(&root.join("a"));
    go(&mut tab, Navigation::Go(loc(&root.join("b"))));
    fs::remove_dir_all(root.join("a")).unwrap();

    assert_eq!(go(&mut tab, Navigation::Back), ReadOutcome::Failed);
    assert_eq!(tab.location(), &loc(&root.join("b")));
    assert_eq!(tab.back_history(), [loc(&root.join("a"))]);
    assert!(tab.forward_history().is_empty());
    assert!(matches!(tab.error(), Some(ListError::NotFound { .. })));
}

#[test]
fn 늦게_온_읽기_결과는_버린다() {
    let dir = tree();
    let root = dir.path();
    let mut tab = tab_at(root);

    // a로 가는 중에 사용자가 b로 갔다.
    let to_a = tab.navigate(Navigation::Open(OsString::from("a"))).unwrap();
    let to_b = tab.navigate(Navigation::Open(OsString::from("b"))).unwrap();
    assert_ne!(to_a.id(), to_b.id());
    assert_eq!(tab.pending_location(), Some(&loc(&root.join("b"))));

    // b의 결과가 먼저 오고 a의 결과가 늦게 온다.
    let a_result = to_a.run();
    assert_eq!(tab.finish(to_b.run()), ReadOutcome::Committed);
    assert_eq!(tab.finish(a_result), ReadOutcome::Stale);
    assert_eq!(tab.location(), &loc(&root.join("b")));
    assert_eq!(names(&tab), ["b.txt"]);
    assert_eq!(tab.back_history(), [loc(root)], "a는 기록에 남지 않는다");
}

#[test]
fn 앞선_요청의_결과가_먼저_와도_버린다() {
    let dir = tree();
    let root = dir.path();
    let mut tab = tab_at(root);

    let to_a = tab.navigate(Navigation::Open(OsString::from("a"))).unwrap();
    let to_b = tab.navigate(Navigation::Open(OsString::from("b"))).unwrap();
    assert_eq!(tab.finish(to_a.run()), ReadOutcome::Stale);
    assert_eq!(tab.location(), &loc(root), "아직 b를 기다린다");
    assert_eq!(tab.pending_location(), Some(&loc(&root.join("b"))));
    assert_eq!(tab.finish(to_b.run()), ReadOutcome::Committed);
    assert_eq!(tab.location(), &loc(&root.join("b")));
    assert_eq!(tab.back_history(), [loc(root)]);
}

#[test]
fn 읽기가_100ms를_넘으면_읽는_중을_드러낸다() {
    let dir = tree();
    let mut tab = tab_at(dir.path());
    assert_eq!(tab.reading_shown_at(), None);
    assert!(!tab.is_reading(Instant::now() + Duration::from_secs(60)));

    let before = Instant::now();
    let request = tab.navigate(Navigation::Refresh).unwrap();
    let at = tab.reading_shown_at().expect("읽기를 기다린다");
    assert!(at >= before + READING_DELAY);
    assert_eq!(READING_DELAY, Duration::from_millis(100));
    assert!(!tab.is_reading(at - Duration::from_nanos(1)));
    assert!(tab.is_reading(at));

    tab.finish(request.run());
    assert!(!tab.is_reading(at + Duration::from_secs(60)));
}

#[test]
fn 정렬_바꾸기는_파일_시스템을_다시_읽지_않는다() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("작은.txt"), b"1").unwrap();
    fs::write(dir.path().join("큰.txt"), vec![b'x'; 5000]).unwrap();
    fs::write(dir.path().join("중간.txt"), vec![b'x'; 2000]).unwrap();
    let mut tab = tab_at(dir.path());
    assert_eq!(names(&tab), ["작은.txt", "중간.txt", "큰.txt"]);

    // 읽은 뒤 파일 시스템을 바꾼다. 다시 읽으면 목록이 달라진다.
    fs::remove_file(dir.path().join("중간.txt")).unwrap();
    fs::write(dir.path().join("새.txt"), b"new").unwrap();

    let by_size = Sort {
        column: Column::Size,
        direction: Direction::Descending,
    };
    tab.set_sort(by_size);
    assert_eq!(tab.sort(), by_size);
    assert_eq!(names(&tab), ["큰.txt", "중간.txt", "작은.txt"]);
    assert!(tab.pending_location().is_none(), "읽기 요청이 없다");

    // 새로 고치면 바뀐 파일 시스템을 지금 정렬로 보인다.
    go(&mut tab, Navigation::Refresh);
    assert_eq!(names(&tab), ["큰.txt", "새.txt", "작은.txt"]);
}

#[test]
fn 읽는_사이에_바꾼_정렬로_결과를_보인다() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.txt"), b"1").unwrap();
    fs::write(dir.path().join("b.txt"), b"22").unwrap();
    let mut tab = Tab::new(loc(dir.path()), Sort::default(), false);
    let request = tab.navigate(Navigation::Refresh).unwrap();
    tab.set_sort(Sort {
        column: Column::Name,
        direction: Direction::Descending,
    });
    tab.finish(request.run());
    assert_eq!(names(&tab), ["b.txt", "a.txt"]);
}

#[cfg(windows)]
fn create_hidden(path: &Path, folder: bool) {
    use std::os::windows::fs::MetadataExt;
    use std::process::Command;
    if folder {
        fs::create_dir(path).unwrap();
    } else {
        fs::write(path, b"h").unwrap();
    }
    let status = Command::new("attrib").arg("+h").arg(path).status().unwrap();
    assert!(status.success());
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    let attrs = fs::metadata(path).unwrap().file_attributes();
    assert_ne!(attrs & FILE_ATTRIBUTE_HIDDEN, 0, "숨김 속성이 붙지 않았다");
}

/// Windows가 아닌 곳에서는 점으로 시작하는 이름이 숨김이다.
#[cfg(not(windows))]
fn create_hidden(path: &Path, folder: bool) {
    if folder {
        fs::create_dir(path).unwrap();
    } else {
        fs::write(path, b"h").unwrap();
    }
}

/// 숨김 파일·폴더와 보이는 파일이 있는 폴더.
fn with_hidden() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("보임.txt"), b"v").unwrap();
    create_hidden(&dir.path().join(".숨김.txt"), false);
    create_hidden(&dir.path().join(".숨김폴더"), true);
    dir
}

#[test]
fn 숨김_항목_감추기와_보이기가_목록에_반영된다() {
    let dir = with_hidden();
    let mut tab = tab_at(dir.path());
    assert!(!tab.show_hidden());
    assert_eq!(names(&tab), ["보임.txt"]);
    assert_eq!(tab.len(), 1);
    assert!(tab.get(1).is_none());

    tab.set_show_hidden(true);
    assert_eq!(names(&tab), [".숨김폴더", ".숨김.txt", "보임.txt"]);
    assert_eq!(tab.len(), 3);

    // 정렬을 바꿔도 감추기가 이어진다.
    tab.set_show_hidden(false);
    tab.set_sort(Sort {
        column: Column::Name,
        direction: Direction::Descending,
    });
    assert_eq!(names(&tab), ["보임.txt"]);
}

#[test]
fn 숨김_항목_상태는_앱_전체에_하나이고_기본은_감춤이다() {
    let dir = with_hidden();
    let mut app = App::new(Some(loc(dir.path())));
    assert!(!app.show_hidden());
    assert_eq!(app.wait(WAIT).map(|(_, o)| o), Some(ReadOutcome::Committed));
    assert_eq!(names(app.active_tab()), ["보임.txt"]);

    app.set_show_hidden(true);
    assert!(app.show_hidden());
    assert_eq!(
        names(app.active_tab()),
        [".숨김폴더", ".숨김.txt", "보임.txt"]
    );

    // 다른 위치로 가도 앱의 상태를 따른다.
    fs::create_dir(dir.path().join("안")).unwrap();
    create_hidden(&dir.path().join("안").join(".속"), false);
    assert!(app.navigate(Navigation::Open(OsString::from("안"))));
    assert_eq!(app.wait(WAIT).map(|(_, o)| o), Some(ReadOutcome::Committed));
    assert_eq!(names(app.active_tab()), [".속"]);
    app.set_show_hidden(false);
    assert!(app.active_tab().is_empty());
}

#[test]
fn 창_패널_탭_구조() {
    let dir = tree();
    let app = App::new(Some(loc(dir.path())));
    assert_eq!(app.windows().len(), 1);
    let window = app.active_window();
    assert_eq!(window.panels().len(), 1);
    let panel = window.active_panel();
    assert_eq!(panel.tabs().len(), 1);
    let (id, tab) = panel.tabs().next().unwrap();
    assert_eq!(id, panel.active_tab_id());
    assert_eq!(tab.location(), &loc(dir.path()));
    assert!(std::ptr::eq(tab, app.active_tab()));
}

#[test]
fn 앱은_일꾼_스레드에서_읽어_탭에_넘긴다() {
    let dir = tree();
    let root = dir.path();
    let mut app = App::new(Some(loc(root)));
    let tab_id = app.active_window().active_panel().active_tab_id();
    assert_eq!(app.wait(WAIT), Some((tab_id, ReadOutcome::Committed)));
    assert_eq!(names(app.active_tab()), ["a", "b", "파일.txt"]);

    assert!(app.navigate(Navigation::Open(OsString::from("a"))));
    assert_eq!(app.wait(WAIT), Some((tab_id, ReadOutcome::Committed)));
    assert_eq!(app.active_tab().location(), &loc(&root.join("a")));

    assert!(app.navigate(Navigation::Back));
    assert_eq!(app.wait(WAIT), Some((tab_id, ReadOutcome::Committed)));
    assert_eq!(app.active_tab().location(), &loc(root));
    assert!(!app.navigate(Navigation::Back), "뒤로 기록이 비었다");

    // 실패한 이동.
    assert!(app.navigate(Navigation::Open(OsString::from("없음"))));
    assert_eq!(app.wait(WAIT), Some((tab_id, ReadOutcome::Failed)));
    assert_eq!(app.active_tab().location(), &loc(root));
    assert!(app.active_tab().error().is_some());

    // 더 올 결과가 없다.
    assert!(app.poll().is_empty());
}

#[test]
fn 앱에서도_늦게_온_결과는_버린다() {
    let dir = tree();
    let root = dir.path();
    let mut app = App::new(Some(loc(root)));
    app.wait(WAIT).unwrap();

    assert!(app.navigate(Navigation::Open(OsString::from("a"))));
    assert!(app.navigate(Navigation::Open(OsString::from("b"))));
    let mut outcomes = vec![app.wait(WAIT).unwrap().1, app.wait(WAIT).unwrap().1];
    outcomes.sort_by_key(|o| *o == ReadOutcome::Stale);
    assert_eq!(outcomes, [ReadOutcome::Committed, ReadOutcome::Stale]);
    assert_eq!(app.active_tab().location(), &loc(&root.join("b")));
    assert_eq!(app.active_tab().back_history(), [loc(root)]);
}

#[test]
fn 앱의_정렬_바꾸기는_다시_읽지_않는다() {
    let dir = tree();
    let mut app = App::new(Some(loc(dir.path())));
    app.wait(WAIT).unwrap();
    app.set_sort(Sort {
        column: Column::Name,
        direction: Direction::Descending,
    });
    assert_eq!(names(app.active_tab()), ["b", "a", "파일.txt"]);
    assert!(app.active_tab().pending_location().is_none());
    assert!(
        app.wait(Duration::from_millis(50)).is_none(),
        "읽기 결과가 오지 않는다"
    );
}

#[test]
fn 시작_위치를_정하지_않으면_홈_폴더를_연다() {
    let home = std::env::home_dir().expect("시험 기계에는 홈 폴더가 있다");
    assert_eq!(home_location(), loc(&home));
    let mut app = App::new(None);
    assert_eq!(app.active_tab().location(), &loc(&home));
    assert_eq!(app.active_tab().pending_location(), Some(&loc(&home)));
    assert_eq!(app.wait(WAIT).map(|(_, o)| o), Some(ReadOutcome::Committed));
    assert_eq!(app.active_tab().location(), &loc(&home));
}

/// 드라이브 루트에서 위로는 내 PC로 간다(#350). 파일 시스템 공급자의 드라이브 루트에는 여전히 상위가 없다(탭이 잇는다).
#[cfg(windows)]
#[test]
fn 드라이브_루트에서_위로는_내_pc다() {
    let dir = tree();
    let root = dir
        .path()
        .ancestors()
        .last()
        .expect("루트가 있다")
        .to_path_buf();
    assert!(
        facet_core::provider_for(&loc(&root))
            .parent(&loc(&root))
            .is_none()
    );
    let mut tab = Tab::new(loc(&root), Sort::default(), false);
    assert!(tab.can_go_up());
    assert!(tab.navigate(Navigation::Up).is_some());
    assert_eq!(tab.pending_location(), Some(&Location::this_pc()));
    let mut this_pc = Tab::new(Location::this_pc(), Sort::default(), false);
    assert!(!this_pc.can_go_up());
    assert!(this_pc.navigate(Navigation::Up).is_none());
}

/// 네트워크 공유 루트(UNC)는 드라이브 루트가 아니라 그대로 상위가 없다.
#[test]
fn 네트워크_공유_루트에서는_위로가_꺼진다() {
    let root = Path::new(r"\\server\share\");
    let mut tab = Tab::new(loc(root), Sort::default(), false);
    assert!(!tab.can_go_up());
    assert!(tab.navigate(Navigation::Up).is_none());
    assert!(tab.pending_location().is_none());
}

#[cfg(not(windows))]
#[test]
fn 루트에서는_위로가_꺼진다() {
    let mut tab = Tab::new(loc(Path::new("/")), Sort::default(), false);
    assert!(!tab.can_go_up());
    assert!(tab.navigate(Navigation::Up).is_none());
}

#[test]
fn 결과가_오면_앱이_정한_깨우기를_부른다() {
    let dir = tree();
    let root = dir.path();
    let mut app = App::new(Some(loc(root)));
    let (woke, woken) = std::sync::mpsc::channel();
    app.set_waker(move || {
        let _ = woke.send(());
    });
    // 깨우기를 걸기 전에 온 결과는 깨우지 않을 수 있으므로, 건 뒤에 한 번 거둬 간다.
    let mut committed = app
        .poll()
        .into_iter()
        .any(|(_, u)| u == Update::Read(ReadOutcome::Committed));
    if !committed {
        woken
            .recv_timeout(WAIT)
            .expect("첫 읽기 결과가 깨워야 한다");
        committed = app
            .poll()
            .into_iter()
            .any(|(_, u)| u == Update::Read(ReadOutcome::Committed));
    }
    assert!(committed);

    // 막지 않는 poll만으로 받는다: 깨우기가 불린 뒤에는 결과가 이미 와 있다.
    assert!(app.navigate(Navigation::Open(OsString::from("a"))));
    woken.recv_timeout(WAIT).expect("이동 결과가 깨워야 한다");
    let updates = app.poll();
    assert!(updates.contains(&(
        app.active_window().active_panel().active_tab_id(),
        Update::Read(ReadOutcome::Committed)
    )));
    assert_eq!(app.active_tab().location(), &loc(&root.join("a")));
}
