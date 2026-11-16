//! **내용 검색 결과 위치**: 내용 검색 엔진을 탭의 검색 결과 위치로 받기를 임시 폴더의 실제 파일로 확인한다.
//!
//! - 내용 검색을 시작하면 탭이 결과 위치가 되고 첫 일치 줄·일치 수가 있는 결과가 들어오며, 뒤로·앞으로·중지·F5가 이름
//!   검색 결과와 같다. 틀린 정규식이면 위치가 그대로이고 까닭이 온다.
//! - 「일치」 열(일치 수) 정렬, 종류별 건너뛴 수, 「내용 검색」 명령이 팔레트에 있음.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use facet_core::command::CommandId;
use facet_core::content_search::ContentMode;
use facet_core::details::{Column, Direction, Sort};
use facet_core::tab::{Navigation, ReadOutcome};
use facet_core::window::{App, ContentSearchError, Execution};
use facet_core::{Location, SearchKind};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(30);

/// `a.txt`(「찾는말」 1번), `b.txt`(3번), `c.md`(2번), `d.txt`(없음), `bin.dat`(NUL이 든 바이너리).
fn tree() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("a.txt"), "첫 줄\n  여기 찾는말 이 있다  \n끝").unwrap();
    fs::write(root.join("b.txt"), "찾는말 찾는말\n찾는말").unwrap();
    fs::write(root.join("sub").join("c.md"), "찾는말\n찾는말").unwrap();
    fs::write(root.join("d.txt"), "없다").unwrap();
    fs::write(
        root.join("bin.dat"),
        b"\0\0\0 \xEC\xB0\xBE\xEB\x8A\x94\xEB\xA7\x90",
    )
    .unwrap();
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

fn wait_search(app: &mut App) {
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

fn content_search(app: &mut App, text: &str, mode: ContentMode, names: &str) {
    app.start_content_search(text, mode, names).unwrap();
    wait_read(app);
    wait_search(app);
}

fn names(app: &App) -> Vec<String> {
    let tab = app.active_tab();
    (0..tab.len())
        .map(|i| tab.get(i).unwrap().name.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn 내용_검색을_시작하면_결과_위치가_되고_첫_일치_줄과_일치_수가_들어온다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    content_search(&mut app, "찾는말", ContentMode::Literal, "");

    let tab = app.active_tab();
    let scope = tab.location().search_scope().expect("검색 결과 위치");
    assert_eq!(scope.query, "찾는말");
    assert!(matches!(scope.kind, SearchKind::Content { .. }));
    assert_eq!(tab.location().file_system_path(), None);

    let mut found = names(&app);
    found.sort();
    assert_eq!(found, ["a.txt", "b.txt", "c.md"]);
    let at = |name: &str| {
        let i = (0..tab.len())
            .find(|&i| tab.get(i).unwrap().name == name)
            .unwrap();
        tab.hit(i).expect("내용 검색 결과는 일치를 갖는다").clone()
    };
    assert_eq!(at("a.txt").first_line, "여기 찾는말 이 있다");
    assert_eq!(at("a.txt").matches, 1);
    assert_eq!(at("b.txt").matches, 3);
    assert_eq!(at("c.md").matches, 2);
}

#[test]
fn 파일_이름_조건과_정규식으로_찾는다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    content_search(&mut app, "찾는말", ContentMode::Literal, "*.md");
    assert_eq!(names(&app), ["c.md"]);

    content_search(&mut app, "찾.말$", ContentMode::Regex, "");
    let mut found = names(&app);
    found.sort();
    assert_eq!(found, ["b.txt", "c.md"], "줄 끝의 일치만");
}

#[test]
fn 뒤로_가면_원래_폴더이고_앞으로_가면_다시_검색하고_f5도_다시_검색한다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    content_search(&mut app, "찾는말", ContentMode::Literal, "");

    assert!(app.navigate(Navigation::Back));
    wait_read(&mut app);
    assert_eq!(
        app.active_tab().location(),
        &Location::file_system(dir.path())
    );
    assert!(app.active_tab().search().is_none());

    assert!(app.navigate(Navigation::Forward));
    wait_read(&mut app);
    wait_search(&mut app);
    assert_eq!(app.active_tab().len(), 3);

    fs::write(dir.path().join("d.txt"), "찾는말 새로").unwrap();
    assert!(app.navigate(Navigation::Refresh));
    wait_read(&mut app);
    wait_search(&mut app);
    assert_eq!(
        app.active_tab().len(),
        4,
        "F5는 같은 검색을 처음부터 다시 한다"
    );
}

#[test]
fn 틀린_정규식이면_위치가_그대로이고_까닭이_온다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    let before = app.active_tab().location().clone();
    let error = app
        .start_content_search("(열리지 않은", ContentMode::Regex, "")
        .unwrap_err();
    assert!(matches!(error, ContentSearchError::Regex(reason) if !reason.is_empty()));
    assert_eq!(app.active_tab().location(), &before);
    assert!(app.active_tab().back_history().is_empty(), "기록도 그대로");

    assert_eq!(
        app.start_content_search("", ContentMode::Literal, ""),
        Err(ContentSearchError::EmptyText)
    );
    assert_eq!(app.active_tab().location(), &before);
}

#[test]
fn 검색_중지는_결과를_남기고_이름_검색과_따로_센다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    assert!(
        app.start_content_search("찾는말", ContentMode::Literal, "")
            .is_ok()
    );
    wait_read(&mut app);
    // 중지 명령은 검색이 도는 중에만 켜진다. 이미 끝났을 수 있으므로 켜졌을 때만 실행한다.
    if app.can_execute(CommandId::StopSearch) {
        assert_eq!(app.execute(CommandId::StopSearch), Execution::Done);
    }
    wait_search(&mut app);
    assert!(!app.can_execute(CommandId::StopSearch));
}

#[test]
fn 일치_열로_정렬하면_일치_수_순서이고_내용_검색에서만_켜진다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    assert!(
        !app.can_execute(CommandId::SortByMatches),
        "폴더에서는 꺼짐"
    );
    content_search(&mut app, "찾는말", ContentMode::Literal, "");
    assert!(app.can_execute(CommandId::SortByMatches));

    app.set_sort(Sort {
        column: Column::Matches,
        direction: Direction::Ascending,
    });
    assert_eq!(names(&app), ["a.txt", "c.md", "b.txt"]);
    app.set_sort(Sort {
        column: Column::Matches,
        direction: Direction::Descending,
    });
    assert_eq!(names(&app), ["b.txt", "c.md", "a.txt"]);

    // 이름 검색 결과에서는 「일치」 정렬 명령이 꺼진다.
    app.start_search("txt").unwrap();
    wait_read(&mut app);
    assert!(!app.can_execute(CommandId::SortByMatches));
}

#[test]
fn 종류별_건너뛴_수가_탭에서_읽힌다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    content_search(&mut app, "찾는말", ContentMode::Literal, "");
    let status = app.active_tab().search().unwrap();
    let content = status.content.expect("내용 검색의 진행");
    assert_eq!(content.skipped.binary, 1, "bin.dat");
    assert_eq!(content.skipped.online_only, 0);
    assert_eq!(content.skipped.too_large, 0);
    assert_eq!(content.skipped.unreadable, 0);
    assert_eq!(status.progress.found, 3);
    assert!(content.files_read >= 4);
}

#[test]
fn 내용_검색_명령은_팔레트에_있고_검색창을_내용으로_맡긴다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    let found = app.search_commands("content-search", |c: CommandId| c.as_str());
    assert!(found.iter().any(|c| c.command == CommandId::ContentSearch));
    assert!(CommandId::ContentSearch.shortcuts().is_empty());
    assert!(app.can_execute(CommandId::ContentSearch));
    assert_eq!(
        app.execute(CommandId::ContentSearch),
        Execution::ContentSearch
    );
}

#[test]
fn 내용_검색_결과에서_폴더에서_보기와_다른_곳으로_가기로_검색이_멈춘다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    content_search(&mut app, "찾는말", ContentMode::Literal, "");
    let tab = app.active_tab_mut();
    let index = (0..tab.len())
        .find(|&i| tab.get(i).unwrap().name == "c.md")
        .unwrap();
    tab.select_only(index);
    assert!(app.can_execute(CommandId::ShowInFolder));
    assert_eq!(app.execute(CommandId::ShowInFolder), Execution::Done);
    wait_read(&mut app);
    assert_eq!(
        app.active_tab().location(),
        &Location::file_system(dir.path().join("sub"))
    );
}
