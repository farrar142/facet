//! **검색 결과 위치**: 실시간 탐색 결과를 탭의 위치로 받기를 임시 폴더의 실제 파일로 공개 인터페이스에서 확인한다.
//!
//! - 검색을 시작하면 탭이 결과 위치가 되고 결과가 들어오며, 뒤로 가면 원래 폴더다. 다른 곳으로 가거나 탭·창을 닫으면
//!   탐색이 멈추고, 새로 고침(F5)은 같은 검색을 다시 하며, 「검색 중지」는 결과를 남긴다.
//! - 다른 폴더의 같은 이름 항목을 따로 고르고, 정렬(위치 열 포함) 뒤에도 선택이 남는다.
//! - 「폴더에서 보기」, 결과 위치에서 꺼지는 명령(새 폴더·붙여넣기), 오른쪽 메뉴 첫 항목.
//! - facet이 한 작업(휴지통·영구 삭제, 이동, 이름바꾸기)이 결과 목록에 반영되고, 결과 항목의 복사가 제 폴더 경로로
//!   된다(Windows 전용).

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use facet_core::command::CommandId;
use facet_core::details::{Column, Direction, Sort};
use facet_core::live_search::SearchEvent;
use facet_core::menu::{MenuEntry, MenuKind};
use facet_core::search_results::SearchFeed;
use facet_core::tab::{Navigation, ReadOutcome, Tab};
use facet_core::window::{App, Execution, Update};
use facet_core::{Location, SearchScope};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(30);

/// `root/a/보고서.txt`, `root/b/보고서.txt`, `root/b/c/보고서 초안.md`, `root/기타.txt`, `root/a/보고서 폴더/`가 있는 나무.
fn tree() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("a").join("보고서 폴더")).unwrap();
    fs::create_dir_all(root.join("b").join("c")).unwrap();
    fs::write(root.join("a").join("보고서.txt"), b"a").unwrap();
    fs::write(root.join("b").join("보고서.txt"), b"bb").unwrap();
    fs::write(root.join("b").join("c").join("보고서 초안.md"), b"ccc").unwrap();
    fs::write(root.join("기타.txt"), b"x").unwrap();
    dir
}

fn app_at(dir: &Path) -> App {
    let mut app = App::new(Some(Location::file_system(dir)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

/// 활성 탭의 이동이 확정될 때까지 기다린다.
fn wait_read(app: &mut App) {
    loop {
        let (tab, outcome) = app.wait(WAIT).expect("읽기 결과");
        // 다른 탭(닫으며 다시 보인 이웃 등)의 읽기는 건너뛴다.
        if tab == app.active_tab_id() {
            assert_eq!(outcome, ReadOutcome::Committed);
            return;
        }
    }
}

/// 활성 탭의 검색이 끝날 때까지 결과를 받는다.
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

/// 검색을 시작해 결과 위치로 옮기고 끝날 때까지 받는다.
fn search(app: &mut App, query: &str) {
    app.start_search(query).unwrap();
    wait_read(app);
    wait_search(app);
}

/// 보이는 결과의 전체 경로(목록 순서).
fn paths(tab: &Tab) -> Vec<PathBuf> {
    (0..tab.len()).filter_map(|i| tab.item_path(i)).collect()
}

fn sorted(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.sort();
    paths
}

fn index_of(tab: &Tab, path: &Path) -> usize {
    (0..tab.len())
        .find(|&i| tab.item_path(i).as_deref() == Some(path))
        .unwrap_or_else(|| panic!("결과에 없다: {path:?} / {:?}", paths(tab)))
}

/// 버리면 표지를 세우는 가짜 피드(탭이 피드를 버리는지 = 검색이 멈추는지 본다).
struct Probe(Arc<AtomicBool>);

impl SearchFeed for Probe {
    fn poll(&mut self) -> Vec<SearchEvent> {
        Vec::new()
    }

    fn stop(&self) {}
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// 활성 탭의 피드를 가짜로 바꾸고 그 표지를 돌려준다.
fn probe(app: &mut App) -> Arc<AtomicBool> {
    let dropped = Arc::new(AtomicBool::new(false));
    assert!(
        app.active_tab_mut()
            .attach_search(Box::new(Probe(Arc::clone(&dropped))))
    );
    dropped
}

#[test]
fn 검색을_시작하면_탭이_결과_위치가_되고_결과가_들어오며_뒤로_가면_원래_폴더다() {
    let dir = tree();
    let root = dir.path();
    let mut app = app_at(root);
    assert!(
        app.start_search("   ").is_err(),
        "빈 검색어는 시작하지 않는다"
    );

    app.start_search("보고서").unwrap();
    wait_read(&mut app);
    let tab = app.active_tab();
    assert_eq!(
        tab.location(),
        &Location::SearchResults(SearchScope {
            folder: root.to_path_buf(),
            query: "보고서".into(),
            kind: facet_core::SearchKind::Name,
        })
    );
    assert_eq!(
        tab.location().file_system_path(),
        None,
        "실제 폴더가 아니다"
    );
    assert_eq!(tab.back_history(), [Location::file_system(root)]);
    assert_eq!(tab.title(), "보고서");
    let segments = facet_core::provider_for(tab.location()).segments(tab.location());
    assert_eq!(segments.last().unwrap().label, "보고서");
    assert_eq!(
        segments[segments.len() - 2].location,
        Location::file_system(root)
    );

    wait_search(&mut app);
    let tab = app.active_tab();
    let expected = sorted(vec![
        root.join("a").join("보고서 폴더"),
        root.join("a").join("보고서.txt"),
        root.join("b").join("보고서.txt"),
        root.join("b").join("c").join("보고서 초안.md"),
    ]);
    assert_eq!(sorted(paths(tab)), expected);
    let status = tab.search().unwrap();
    assert!(!status.running);
    assert_eq!(status.progress.found, 4);
    assert_eq!(status.progress.folders_skipped, 0);
    assert!(status.progress.folders_scanned >= 5);
    assert_eq!(
        status.summary.unwrap().end,
        facet_core::live_search::SearchEnd::Finished
    );
    assert!(!app.can_execute(CommandId::StopSearch));

    // 뒤로 가면 원래 폴더이고 검색 상태는 없다. 앞으로 가면 같은 검색을 다시 한다.
    assert_eq!(app.execute(CommandId::Back), Execution::Done);
    wait_read(&mut app);
    assert_eq!(app.active_tab().location(), &Location::file_system(root));
    assert!(app.active_tab().search().is_none());
    assert_eq!(app.execute(CommandId::Forward), Execution::Done);
    wait_read(&mut app);
    wait_search(&mut app);
    assert_eq!(sorted(paths(app.active_tab())), expected);
}

#[test]
fn 결과는_찾는_대로_들어오고_update_search로_알린다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    app.start_search("보고서").unwrap();
    wait_read(&mut app);
    let tab_id = app.active_tab_id();
    let deadline = Instant::now() + WAIT;
    let mut told = false;
    while app.active_tab().search().unwrap().summary.is_none() {
        told |= app.poll().contains(&(tab_id, Update::Search));
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(told, "결과가 들어오면 Update::Search가 온다");
    assert_eq!(app.active_tab().len(), 4);
}

#[test]
fn 다른_곳으로_가거나_탭이나_창을_닫으면_탐색이_멈춘다() {
    let dir = tree();
    let root = dir.path();

    // 다른 곳으로 가기.
    let mut app = app_at(root);
    app.start_search("보고서").unwrap();
    wait_read(&mut app);
    let dropped = probe(&mut app);
    assert!(app.navigate(Navigation::Go(Location::file_system(root.join("a")))));
    assert!(!dropped.load(Ordering::SeqCst), "확정 전에는 멈추지 않는다");
    wait_read(&mut app);
    assert!(dropped.load(Ordering::SeqCst), "다른 곳으로 가면 멈춘다");
    assert!(app.active_tab().search().is_none());

    // 탭 닫기.
    app.start_search("보고서").unwrap();
    wait_read(&mut app);
    let dropped = probe(&mut app);
    let id = app.active_tab_id();
    app.open_tab(Location::file_system(root), false);
    app.close_tab(id).unwrap();
    assert!(dropped.load(Ordering::SeqCst), "탭을 닫으면 멈춘다");

    // 창 닫기.
    let window = app.open_window(Location::file_system(root));
    wait_read(&mut app);
    app.start_search("보고서").unwrap();
    wait_read(&mut app);
    let dropped = probe(&mut app);
    assert!(app.close_window(window));
    assert!(dropped.load(Ordering::SeqCst), "창을 닫으면 멈춘다");
}

#[test]
fn 숨은_탭의_결과는_그대로이고_다시_보여도_다시_찾지_않는다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    search(&mut app, "보고서");
    let results = app.active_tab_id();
    let dropped = probe(&mut app);
    app.open_tab(Location::file_system(dir.path()), true);
    wait_read(&mut app);
    assert!(app.activate_tab(results));
    assert!(
        !dropped.load(Ordering::SeqCst),
        "숨었다 보여도 피드는 그대로다"
    );
    assert_eq!(app.active_tab().len(), 4);
}

#[test]
fn 새로_고침은_같은_검색을_다시_하고_밖의_변경은_따라가지_않는다() {
    let dir = tree();
    let root = dir.path();
    let mut app = app_at(root);
    search(&mut app, "보고서");
    assert_eq!(app.active_tab().len(), 4);

    // 밖에서 만든 파일은 결과에 저절로 들어오지 않는다.
    fs::write(root.join("b").join("새 보고서.txt"), b"n").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    app.poll();
    assert_eq!(app.active_tab().len(), 4);

    assert_eq!(app.execute(CommandId::Refresh), Execution::Done);
    wait_read(&mut app);
    wait_search(&mut app);
    let tab = app.active_tab();
    assert_eq!(tab.len(), 5);
    index_of(tab, &root.join("b").join("새 보고서.txt"));
    // 새로 고침은 기록을 바꾸지 않는다.
    assert_eq!(tab.back_history(), [Location::file_system(root)]);
}

#[test]
fn 검색_중지는_탐색을_멈추고_결과를_남긴다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    app.start_search("보고서").unwrap();
    wait_read(&mut app);
    // 검색이 도는 동안에만 켜진다. 결과 몇 개를 직접 넣은 것처럼 두고 멈춘다.
    assert!(app.can_execute(CommandId::StopSearch));
    let dropped = probe(&mut app);
    assert!(app.can_execute(CommandId::StopSearch));
    assert_eq!(app.execute(CommandId::StopSearch), Execution::Done);
    assert!(!app.can_execute(CommandId::StopSearch));
    let status = app.active_tab().search().unwrap();
    assert!(!status.running);
    assert!(app.active_tab().location().is_search_results());
    drop(dropped);

    // 실제 탐색을 멈추면 끝남 사건이 취소로 오고, 그때까지의 결과는 남는다.
    assert_eq!(app.execute(CommandId::Refresh), Execution::Done);
    wait_read(&mut app);
    assert_eq!(app.execute(CommandId::StopSearch), Execution::Done);
    let deadline = Instant::now() + WAIT;
    while app.active_tab().search().unwrap().summary.is_none() {
        app.poll();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let tab = app.active_tab();
    let status = tab.search().unwrap();
    assert!(!status.running);
    assert!(tab.len() <= 4);
    assert!(tab.location().is_search_results(), "결과 위치에 남는다");
}

#[test]
fn 다른_폴더의_같은_이름을_따로_고르고_정렬_뒤에도_선택이_남는다() {
    let dir = tree();
    let root = dir.path();
    let mut app = app_at(root);
    search(&mut app, "보고서.txt");
    let a = root.join("a").join("보고서.txt");
    let b = root.join("b").join("보고서.txt");
    let tab = app.active_tab_mut();
    assert_eq!(tab.len(), 2);
    assert_eq!(tab.get(0).unwrap().name, tab.get(1).unwrap().name);

    let ia = index_of(tab, &a);
    tab.select_only(ia);
    assert!(tab.is_selected(ia));
    assert!(
        !tab.is_selected(index_of(tab, &b)),
        "같은 이름이어도 다른 항목이다"
    );
    assert_eq!(tab.selected_paths(), std::slice::from_ref(&a));
    assert_eq!(tab.cursor_path(), Some(a.clone()));
    assert_eq!(tab.folder_of(ia), Some(root.join("a").as_path()));

    // 위치 열로 정렬: 폴더 순서대로, 방향을 뒤집으면 거꾸로.
    for direction in [Direction::Ascending, Direction::Descending] {
        tab.set_sort(Sort {
            column: Column::Location,
            direction,
        });
        let order = paths(tab);
        let expected = match direction {
            Direction::Ascending => [a.clone(), b.clone()],
            Direction::Descending => [b.clone(), a.clone()],
        };
        assert_eq!(order, expected);
        assert_eq!(
            tab.selected_paths(),
            std::slice::from_ref(&a),
            "정렬 뒤에도 선택이 남는다"
        );
        assert_eq!(tab.cursor_path(), Some(a.clone()));
    }
    tab.set_sort(Sort {
        column: Column::Size,
        direction: Direction::Descending,
    });
    assert_eq!(paths(tab), [b.clone(), a.clone()]);
    assert_eq!(tab.selected_paths(), std::slice::from_ref(&a));

    // Ctrl 토글·모두 선택·범위도 경로로.
    let ib = index_of(tab, &b);
    tab.toggle_select(ib);
    assert_eq!(
        sorted(tab.selected_paths()),
        sorted(vec![a.clone(), b.clone()])
    );
    tab.toggle_select(index_of(tab, &a));
    assert_eq!(tab.selected_paths(), std::slice::from_ref(&b));
    tab.select_all();
    assert_eq!(tab.selected_count(), 2);

    // 정렬 명령: 위치로 정렬은 결과 위치에서만 켜진다.
    assert!(app.can_execute(CommandId::SortByLocation));
    assert_eq!(app.execute(CommandId::SortByLocation), Execution::Done);
    assert_eq!(app.active_tab().sort().column, Column::Location);
}

#[test]
fn 결과는_찾은_수가_많아도_정렬을_지킨다() {
    let dir = TempDir::new().unwrap();
    for f in 0..20 {
        let folder = dir.path().join(format!("폴더{f}"));
        fs::create_dir(&folder).unwrap();
        for i in 0..20 {
            fs::write(folder.join(format!("항목{i}.txt")), vec![0; i]).unwrap();
        }
    }
    let mut app = app_at(dir.path());
    search(&mut app, "항목");
    let tab = app.active_tab();
    assert_eq!(tab.len(), 400);
    let names: Vec<String> = tab
        .items()
        .map(|i| i.name.to_string_lossy().into_owned())
        .collect();
    let mut expected = names.clone();
    expected.sort_by(|x, y| facet_core::details::compare_names(x, y));
    assert_eq!(names, expected, "이름 오름차순(같은 이름은 위치 순)");
}

#[test]
fn 폴더에서_보기는_상위_폴더를_열고_그_항목을_고르며_기록에_남는다() {
    let dir = tree();
    let root = dir.path();
    let mut app = app_at(root);
    assert!(
        !app.can_execute(CommandId::ShowInFolder),
        "폴더 목록에서는 꺼진다"
    );
    search(&mut app, "보고서 초안");
    let results = app.active_tab().location().clone();
    assert!(
        !app.can_execute(CommandId::ShowInFolder),
        "커서가 없으면 꺼진다"
    );
    app.active_tab_mut().select_only(0);
    assert!(app.can_execute(CommandId::ShowInFolder));

    assert_eq!(app.execute(CommandId::ShowInFolder), Execution::Done);
    wait_read(&mut app);
    let tab = app.active_tab();
    let folder = root.join("b").join("c");
    assert_eq!(tab.location(), &Location::file_system(&folder));
    let cursor = tab.cursor().unwrap();
    assert_eq!(
        tab.get(cursor).unwrap().name,
        OsString::from("보고서 초안.md")
    );
    assert_eq!(tab.selected_count(), 1);
    assert!(tab.is_selected(cursor));
    assert_eq!(tab.back_history().last(), Some(&results));
    assert!(tab.search().is_none());

    // 뒤로 가면 결과 위치(같은 검색을 다시 한다).
    assert_eq!(app.execute(CommandId::Back), Execution::Done);
    wait_read(&mut app);
    wait_search(&mut app);
    assert_eq!(app.active_tab().location(), &results);
    assert_eq!(app.active_tab().len(), 1);
}

#[test]
fn 결과_폴더를_열면_그_폴더로_간다() {
    let dir = tree();
    let root = dir.path();
    let mut app = app_at(root);
    search(&mut app, "보고서 폴더");
    app.active_tab_mut().select_only(0);
    assert_eq!(app.execute(CommandId::Open), Execution::Done);
    wait_read(&mut app);
    assert_eq!(
        app.active_tab().location(),
        &Location::file_system(root.join("a").join("보고서 폴더"))
    );
}

#[test]
fn 결과_위치에서_새_폴더와_붙여넣기는_꺼지고_오른쪽_메뉴_첫_항목은_폴더에서_보기다() {
    let dir = tree();
    let root = dir.path();
    let mut app = app_at(root);
    app.set_clipboard_files(Some(facet_win::clipboard::ClipboardFiles {
        paths: vec![root.join("기타.txt")],
        operation: facet_win::clipboard::format::ClipboardOperation::Copy,
        sequence: 1,
    }));
    assert!(app.can_execute(CommandId::Paste));
    assert!(app.can_execute(CommandId::NewFolder));

    search(&mut app, "보고서");
    app.active_tab_mut().select_only(0);
    assert!(!app.can_execute(CommandId::NewFolder));
    assert!(!app.can_execute(CommandId::Paste));
    assert_eq!(app.execute(CommandId::NewFolder), Execution::Disabled);
    assert_eq!(app.paste_target(), None);
    for command in [
        CommandId::Open,
        CommandId::Copy,
        CommandId::Cut,
        CommandId::CopyPath,
        CommandId::CopyTo,
        CommandId::MoveTo,
        CommandId::CopyToOtherPanel,
        CommandId::Delete,
        CommandId::DeletePermanently,
        CommandId::Rename,
        CommandId::ShowInFolder,
    ] {
        assert!(app.can_execute(command), "{command:?}");
    }

    let menu = app.context_menu(MenuKind::Item);
    let MenuEntry::Command(first) = &menu.entries[0] else {
        panic!("첫 줄은 명령이다: {menu:?}");
    };
    assert_eq!(first.id, CommandId::ShowInFolder);
    assert!(first.enabled);
    assert_eq!(menu.entries[1], MenuEntry::Separator);
    // 메뉴의 켜짐은 명령의 켜짐과 같다(붙여넣기는 늘 꺼짐).
    for command in menu.commands() {
        assert_eq!(
            command.enabled,
            app.can_execute(command.id),
            "{:?}",
            command.id
        );
    }
    let background = app.context_menu(MenuKind::Background);
    for command in background.commands() {
        if matches!(command.id, CommandId::Paste | CommandId::NewFolder) {
            assert!(!command.enabled, "{:?}", command.id);
        }
    }

    // 폴더 목록의 항목 메뉴에는 「폴더에서 보기」가 없다.
    assert_eq!(app.execute(CommandId::Back), Execution::Done);
    wait_read(&mut app);
    app.active_tab_mut().select_only(0);
    let menu = app.context_menu(MenuKind::Item);
    assert!(menu.commands().all(|c| c.id != CommandId::ShowInFolder));
}

#[test]
fn 결과_항목의_원본과_경로_복사는_항목마다_제_폴더_경로다() {
    let dir = tree();
    let root = dir.path();
    let mut app = app_at(root);
    search(&mut app, "보고서.txt");
    let a = root.join("a").join("보고서.txt");
    let b = root.join("b").join("보고서.txt");
    app.active_tab_mut().select_all();

    let mut sources = app.drag_sources().unwrap();
    sources.sort();
    assert_eq!(sources, [a.clone(), b.clone()]);
    let plan = app
        .transfer_plan_to(
            facet_core::job::JobKind::Copy,
            &root.join("a").to_string_lossy(),
        )
        .unwrap();
    assert_eq!(sorted(plan.sources), sorted(vec![a.clone(), b.clone()]));
    let delete = app.delete_plan(CommandId::DeletePermanently).unwrap();
    assert_eq!(sorted(delete.sources), sorted(vec![a.clone(), b.clone()]));

    let text = app.copy_path_text().unwrap();
    assert!(text.contains(&*a.to_string_lossy()), "{text}");
    assert!(text.contains(&*b.to_string_lossy()), "{text}");
    // 선택이 없으면 검색한 폴더다.
    app.active_tab_mut().clear_selection();
    assert_eq!(
        app.copy_path_text().unwrap(),
        facet_core::clipboard::copy_as_path_text(&[root.to_path_buf()])
    );
}

#[test]
fn 세션에는_검색한_폴더로_남는다() {
    let dir = tree();
    let root = dir.path();
    let mut app = app_at(root);
    search(&mut app, "보고서");
    let session = app.session().unwrap();
    assert_eq!(
        session.windows[0].panels[0].tabs[0].location,
        Location::file_system(root)
    );
    let json = session.to_json();
    let read = facet_core::session::Session::from_json(&json).unwrap();
    assert_eq!(read, session);
}

#[cfg(windows)]
mod windows {
    //! facet이 한 작업이 결과 목록에 반영되는 것(실제 파일 작업).

    #[path = "../common/bin.rs"]
    mod bin;

    use super::*;
    use facet_core::job::{JobId, JobKind, JobQuestion, JobState, NotRecyclableAnswer, Summary};
    use facet_core::window::TransferPlan;

    fn wait_job(app: &mut App, job: JobId) -> Option<(JobState, Summary)> {
        let deadline = Instant::now() + WAIT;
        loop {
            app.poll_jobs();
            app.poll();
            let entry = app.work_area().entry(job).expect("작업 영역의 작업");
            if let Some(question) = &entry.question {
                // 휴지통을 쓸 수 없는 계정이면 작업을 취소하고 알린다.
                if matches!(question, JobQuestion::NotRecyclable(_)) {
                    app.answer_not_recyclable(job, NotRecyclableAnswer::CancelJob)
                        .unwrap();
                    continue;
                }
                panic!("묻지 않아야 한다: {question:?}");
            }
            if let Some(summary) = &entry.summary {
                return Some((entry.state, summary.clone()));
            }
            assert!(Instant::now() < deadline, "작업이 끝나지 않았다");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn 휴지통으로_지운_결과는_빠지고_실행취소해도_다시_넣지_않는다() {
        let test = "휴지통으로_지운_결과는_빠진다";
        let dir = tree();
        let root = dir.path();
        if !facet_core::job::has_recycle_bin(root) {
            println!("건너뜀({test}): 임시 폴더의 볼륨에 휴지통이 없습니다");
            return;
        }
        let mut app = app_at(root);
        search(&mut app, "보고서");
        let a = root.join("a").join("보고서.txt");
        let index = index_of(app.active_tab(), &a);
        app.active_tab_mut().select_only(index);
        let plan = app.delete_plan(CommandId::Delete).unwrap();
        assert_eq!(plan.sources, std::slice::from_ref(&a));
        assert!(!plan.is_permanent());
        let job = app.start_delete(&plan).unwrap();
        let (_, summary) = wait_job(&mut app, job).unwrap();
        let _bin = Bin(summary.recycled.clone());
        if summary.recycled.is_empty() {
            println!("건너뜀({test}): 이 계정은 휴지통을 쓸 수 없습니다");
            return;
        }
        assert!(!a.exists());
        let tab = app.active_tab();
        assert_eq!(tab.len(), 3);
        assert!(!paths(tab).contains(&a));
        assert_eq!(tab.selected_count(), 0, "지운 항목은 선택에서 빠진다");
    }

    #[test]
    fn 영구_삭제한_폴더의_결과는_그_아래까지_빠진다() {
        let dir = tree();
        let root = dir.path();
        let mut app = app_at(root);
        search(&mut app, "보고서");
        assert_eq!(app.active_tab().len(), 4);
        let job = app.delete_permanently(vec![root.join("b")]).unwrap();
        let (state, _) = wait_job(&mut app, job).unwrap();
        assert_eq!(state, JobState::Finished);
        assert_eq!(
            sorted(paths(app.active_tab())),
            sorted(vec![
                root.join("a").join("보고서 폴더"),
                root.join("a").join("보고서.txt"),
            ])
        );
    }

    #[test]
    fn 옮긴_결과는_빠지고_결과_항목의_복사는_제_폴더_경로로_된다() {
        let dir = tree();
        let root = dir.path();
        let target = TempDir::new_in(root).unwrap();
        let mut app = app_at(root);
        search(&mut app, "보고서.txt");
        let a = root.join("a").join("보고서.txt");
        let b = root.join("b").join("보고서.txt");

        // b의 「보고서.txt」를 복사: 원본은 그 항목의 폴더 경로다.
        let index = index_of(app.active_tab(), &b);
        app.active_tab_mut().select_only(index);
        let plan = app
            .transfer_plan_to(JobKind::Copy, &target.path().to_string_lossy())
            .unwrap();
        assert_eq!(plan.sources, std::slice::from_ref(&b));
        let job = app.start_transfer(&plan).unwrap();
        let (state, _) = wait_job(&mut app, job).unwrap();
        assert_eq!(state, JobState::Finished);
        assert_eq!(fs::read(target.path().join("보고서.txt")).unwrap(), b"bb");
        assert_eq!(app.active_tab().len(), 2, "복사는 결과를 바꾸지 않는다");

        // a의 「보고서.txt」를 다른 폴더로 옮기면 결과에서 빠진다.
        let moved_to = root.join("b").join("c");
        let job = app
            .start_transfer(&TransferPlan {
                kind: JobKind::Move,
                sources: vec![a.clone()],
                target: moved_to.clone(),
            })
            .unwrap();
        let (state, _) = wait_job(&mut app, job).unwrap();
        assert_eq!(state, JobState::Finished);
        assert!(moved_to.join("보고서.txt").exists());
        assert_eq!(paths(app.active_tab()), std::slice::from_ref(&b));

        // 이동을 실행취소하면 옮겼던 자리의 항목이 되돌아가는 것이라 결과는 그대로다(결과 밖의 경로).
        let job = app.undo().unwrap();
        wait_job(&mut app, job).unwrap();
        assert!(a.exists());
        assert_eq!(paths(app.active_tab()), [b]);
    }

    #[test]
    fn 이름을_바꾼_결과는_새_이름이_되고_실행취소하면_되돌아온다() {
        let dir = tree();
        let root = dir.path();
        let mut app = app_at(root);
        search(&mut app, "보고서");
        let b = root.join("b").join("보고서.txt");
        let renamed = root.join("b").join("보고서 최종.txt");
        let index = index_of(app.active_tab(), &b);
        app.active_tab_mut().select_only(index);

        // 이름 입력란은 커서 항목에 열린다: 같은 이름이 a에도 있지만 커서 항목(b)의 것을 바꾼다.
        assert_eq!(app.execute(CommandId::Rename), Execution::Rename);
        app.rename(std::ffi::OsStr::new("보고서.txt"), "보고서 최종.txt")
            .unwrap();
        let deadline = Instant::now() + WAIT;
        loop {
            let updates = app.poll();
            if updates
                .iter()
                .any(|(_, update)| matches!(update, Update::Edited(_)))
            {
                break;
            }
            assert!(Instant::now() < deadline, "이름바꾸기 결과가 오지 않았다");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(renamed.exists());
        assert!(
            root.join("a").join("보고서.txt").exists(),
            "다른 폴더의 같은 이름은 그대로"
        );
        let tab = app.active_tab();
        assert!(paths(tab).contains(&renamed));
        assert!(!paths(tab).contains(&b));
        assert_eq!(tab.len(), 4);
        assert_eq!(
            tab.selected_paths(),
            std::slice::from_ref(&renamed),
            "선택이 새 이름을 따라간다"
        );

        // 실행취소하면 결과도 원래 이름으로.
        let job = app.undo().unwrap();
        wait_job(&mut app, job).unwrap();
        assert!(b.exists());
        let tab = app.active_tab();
        assert!(paths(tab).contains(&b));
        assert!(!paths(tab).contains(&renamed));

        // 폴더 목록 탭에서 폴더 이름을 바꾸면 다른 탭의 결과에도 반영되고, 그 아래 결과의 위치도 바뀐다.
        app.navigate(Navigation::Go(Location::file_system(root)));
        wait_read(&mut app);
        let results = app.open_tab(
            Location::search_results(root.to_path_buf(), "보고서"),
            false,
        );
        app.activate_tab(results);
        wait_read(&mut app);
        wait_search(&mut app);
        let folder_tab = app
            .windows()
            .iter()
            .flat_map(|w| w.panels())
            .flat_map(|p| p.tabs())
            .find(|(_, tab)| !tab.location().is_search_results())
            .map(|(id, _)| id)
            .unwrap();
        app.activate_tab(folder_tab);
        app.rename(std::ffi::OsStr::new("b"), "비").unwrap();
        let deadline = Instant::now() + WAIT;
        loop {
            if app
                .poll()
                .iter()
                .any(|(_, update)| matches!(update, Update::Edited(_)))
            {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        app.poll();
        let results_tab = app.tab(results).unwrap();
        let new_paths = sorted(paths(results_tab));
        assert!(
            new_paths.contains(&root.join("비").join("c").join("보고서 초안.md")),
            "{new_paths:?}"
        );
        assert!(new_paths.contains(&root.join("비").join("보고서.txt")));
        assert!(!new_paths.iter().any(|p| p.starts_with(root.join("b"))));
    }

    /// 테스트가 휴지통에 넣은 항목. 버릴 때 휴지통에서 지운다.
    struct Bin(Vec<facet_win::recycle::RecycledItem>);

    impl Drop for Bin {
        fn drop(&mut self) {
            bin::purge(&self.0);
        }
    }
}

#[test]
fn 검색_명령은_ctrl_f와_ctrl_e이고_한글_모드에서도_같다() {
    use facet_core::command::Key;
    use facet_core::command::find_by_shortcut;
    let shortcuts = CommandId::Search.shortcuts();
    assert_eq!(shortcuts.len(), 2);
    assert_eq!(find_by_shortcut(shortcuts[0]), Some(CommandId::Search));
    assert_eq!(find_by_shortcut(shortcuts[1]), Some(CommandId::Search));
    assert_eq!(CommandId::from_id("search"), Some(CommandId::Search));
    // 두벌식에서 F·E 자리의 「ㄹ」「ㄷ」도 같은 키다.
    assert_eq!(Key::from_char('ㄹ'), Key::from_char('f'));
    assert_eq!(Key::from_char('ㄷ'), Key::from_char('e'));
    let dir = TempDir::new().unwrap();
    let mut app = App::new(Some(Location::file_system(dir.path())));
    app.wait(WAIT).expect("첫 읽기");
    assert!(app.can_execute(CommandId::Search));
    assert_eq!(app.execute(CommandId::Search), Execution::Search);
}
