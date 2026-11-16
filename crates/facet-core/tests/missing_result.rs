//! 검색 결과에서 연 항목(열기·새 탭에서 열기·「폴더에서 보기」)이 디스크에 없으면 열지 않고 까닭을 돌려주며 그 항목을
//! 결과에서 뺀다(패닉 없음). 임시 폴더의 실제 파일과 실시간 탐색 결과로 공개 인터페이스에서 확인한다. 즉시 검색 결과는
//! `tests/instant_follow.rs`가 본다.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::command::CommandId;
use facet_core::tab::{ReadOutcome, Tab};
use facet_core::window::{App, Execution};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(30);

/// `root/보고서 1.txt`, `root/보고서 2.txt`, `root/하위/보고서 3.txt`, `root/보고서 폴더/보고서 안.txt`.
fn tree() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("하위")).unwrap();
    fs::create_dir_all(root.join("보고서 폴더")).unwrap();
    fs::write(root.join("보고서 1.txt"), b"1").unwrap();
    fs::write(root.join("보고서 2.txt"), b"2").unwrap();
    fs::write(root.join("하위").join("보고서 3.txt"), b"3").unwrap();
    fs::write(root.join("보고서 폴더").join("보고서 안.txt"), b"4").unwrap();
    dir
}

fn app_at(dir: &Path) -> App {
    let mut app = App::new(Some(Location::file_system(dir)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

fn wait_read(app: &mut App) {
    loop {
        let (tab, outcome) = app.wait(WAIT).expect("읽기 결과");
        if tab == app.active_tab_id() {
            assert_eq!(outcome, ReadOutcome::Committed);
            return;
        }
    }
}

fn search(app: &mut App, query: &str) {
    app.start_search(query).unwrap();
    wait_read(app);
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll();
        let status = app.active_tab().search().expect("검색 결과 위치");
        if status.summary.is_some() && !status.running {
            return;
        }
        assert!(Instant::now() < deadline, "검색이 끝나지 않았다");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn index_of(tab: &Tab, path: &Path) -> usize {
    (0..tab.len())
        .find(|&i| tab.item_path(i).as_deref() == Some(path))
        .unwrap_or_else(|| panic!("결과에 없다: {}", path.display()))
}

fn paths(tab: &Tab) -> Vec<std::path::PathBuf> {
    (0..tab.len()).filter_map(|i| tab.item_path(i)).collect()
}

#[test]
fn 밖에서_지운_파일을_열면_까닭이_오고_결과에서_빠진다() {
    let dir = tree();
    let root = dir.path();
    let mut app = app_at(root);
    search(&mut app, "보고서");
    let gone = root.join("하위").join("보고서 3.txt");
    let kept = root.join("보고서 1.txt");
    let before = app.active_tab().len();
    assert_eq!(before, 5, "{:?}", paths(app.active_tab()));

    // 남을 항목을 고르고, 지운 항목에 커서를 둔다.
    let index = index_of(app.active_tab(), &kept);
    app.active_tab_mut().select_only(index);
    fs::remove_file(&gone).unwrap();
    let index = index_of(app.active_tab(), &gone);
    app.active_tab_mut().toggle_select(index);

    assert_eq!(app.execute(CommandId::Open), Execution::ItemMissing);
    let tab = app.active_tab();
    assert_eq!(tab.missing_item(), Some(gone.as_path()));
    assert_eq!(tab.len(), before - 1);
    assert!(!paths(tab).contains(&gone));
    assert!(tab.location().is_search_results(), "위치는 그대로");
    assert_eq!(
        tab.selected_paths(),
        std::slice::from_ref(&kept),
        "남은 선택은 그대로"
    );

    // 있는 항목을 열면 까닭이 지워진다(파일은 기본 프로그램으로 여니 여기서는 폴더를 연다).
    let index = index_of(app.active_tab(), &root.join("보고서 폴더"));
    app.active_tab_mut().select_only(index);
    assert_eq!(app.execute(CommandId::Open), Execution::Done);
    assert_eq!(app.active_tab().missing_item(), None);
    wait_read(&mut app);
    assert_eq!(
        app.active_tab().location(),
        &Location::file_system(root.join("보고서 폴더"))
    );
}

#[test]
fn 밖에서_지운_폴더를_새_탭에서_열면_탭이_생기지_않고_그_아래_결과까지_빠진다() {
    let dir = tree();
    let root = dir.path();
    let other = root.join("보고서 상자");
    fs::create_dir_all(&other).unwrap();
    let mut app = app_at(root);
    search(&mut app, "보고서");
    let folder = root.join("보고서 폴더");
    fs::remove_dir_all(&folder).unwrap();
    let tabs = app.active_window().active_panel().tabs().len();
    let index = index_of(app.active_tab(), &folder);
    app.active_tab_mut().select_only(index);

    assert_eq!(app.execute(CommandId::OpenInNewTab), Execution::ItemMissing);
    assert_eq!(
        app.active_window().active_panel().tabs().len(),
        tabs,
        "새 탭이 생기지 않는다"
    );
    let tab = app.active_tab();
    assert_eq!(tab.missing_item(), Some(folder.as_path()));
    let left = paths(tab);
    assert!(!left.contains(&folder));
    assert!(
        !left.iter().any(|p| p.starts_with(&folder)),
        "폴더 아래 결과도 빠진다: {left:?}"
    );
    assert_eq!(left.len(), 4);

    // 휠 클릭(앱이 부르는 같은 함수)도 같다.
    fs::remove_dir_all(&other).unwrap();
    let index = index_of(app.active_tab(), &other);
    assert_eq!(app.open_in_new_tab(index), None);
    assert_eq!(app.active_window().active_panel().tabs().len(), tabs);
    assert_eq!(app.active_tab().missing_item(), Some(other.as_path()));
    assert!(!paths(app.active_tab()).contains(&other));
}

#[test]
fn 밖에서_지운_파일의_폴더에서_보기는_가지_않고_결과에서_뺀다() {
    let dir = tree();
    let root = dir.path();
    let mut app = app_at(root);
    search(&mut app, "보고서");
    let gone = root.join("보고서 2.txt");
    let index = index_of(app.active_tab(), &gone);
    app.active_tab_mut().select_only(index);
    fs::remove_file(&gone).unwrap();
    let back = app.active_tab().back_history().len();

    assert!(app.can_execute(CommandId::ShowInFolder));
    assert_eq!(app.execute(CommandId::ShowInFolder), Execution::ItemMissing);
    let tab = app.active_tab();
    assert!(tab.location().is_search_results(), "위치는 그대로");
    assert!(tab.pending_location().is_none(), "이동을 시작하지 않는다");
    assert_eq!(tab.back_history().len(), back);
    assert_eq!(tab.missing_item(), Some(gone.as_path()));
    assert!(!paths(tab).contains(&gone));

    // 있는 항목은 그대로 폴더에서 보인다.
    let kept = root.join("보고서 1.txt");
    let index = index_of(app.active_tab(), &kept);
    app.active_tab_mut().select_only(index);
    assert_eq!(app.execute(CommandId::ShowInFolder), Execution::Done);
    assert_eq!(app.active_tab().missing_item(), None);
    wait_read(&mut app);
    assert_eq!(app.active_tab().location(), &Location::file_system(root));
}
