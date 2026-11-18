// target: crates/facet-core/tests/spec_virtual_folders_recycle_bin.rs
// AC: #352: 휴지통 열기·주소창 이름과 지운 파일의 원래 이름·원래 위치·지운 날짜·크기, 다른 볼륨의 휴지통도 한 목록, 원래 위치·지운 날짜 정렬, 휴지통의 되는 일과 막힌 명령, 메뉴의 꺼진 줄, 끌기 원본 아님, 세션
//! 셸 공급자의 「휴지통」 위치를 실제 휴지통으로 공개 인터페이스(`App`·`Tab`)에서 확인한다. 임시 폴더의 파일을 S4의
//! 휴지통 삭제로 넣고 휴지통을 열어 그 항목이 원래 이름·원래 위치·지운 날짜·크기로 보이는지, 원래 위치·지운 날짜로
//! 정렬되는지, 휴지통 안에서 이름바꾸기·새 폴더·붙여넣기·복사·잘라내기 등이 위치에 막혀(`NotAllowedHere`) 아무것도
//! 바꾸지 않고 메뉴에서 꺼져 있는지, 끌기 원본이 되지 않는지, 세션으로 되살아나는지. 다른 볼륨은
//! `FACET_TEST_OTHER_VOLUME_DIR`이 있을 때만 본다. 휴지통을 쓸 수 없는 계정이면 건너뛰었다고 출력하고, 넣은 항목은
//! 끝날 때(실패해도) 휴지통에서 지운다. 러너의 휴지통을 비우지 않는다. Windows 전용.
#![cfg(windows)]

#[path = "common/bin.rs"]
mod bin;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use facet_core::command::CommandId;
use facet_core::details::{Column, Direction, Sort};
use facet_core::job::{self, JobId, JobQuestion, JobState, NotRecyclableAnswer, Summary};
use facet_core::menu::{Menu, MenuKind};
use facet_core::session::Session;
use facet_core::tab::ReadOutcome;
use facet_core::window::{App, Execution};
use facet_core::{Location, VirtualFolder};
use facet_win::recycle::RecycledItem;
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(60);

/// 지운 날짜를 견줄 때의 여유.
const CLOCK_SLACK: Duration = Duration::from_secs(5);

/// 테스트가 휴지통에 넣은 항목. 버릴 때 휴지통에 남아 있으면 지운다.
#[derive(Default)]
struct Bin(Vec<RecycledItem>);

impl Bin {
    fn keep(&mut self, summary: &Summary) {
        self.0.extend(summary.recycled.iter().cloned());
    }
}

impl Drop for Bin {
    fn drop(&mut self) {
        bin::purge(&self.0);
    }
}

fn app_at(dir: &Path) -> App {
    let mut app = App::new(Some(Location::file_system(dir)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

/// 작업이 끝나거나 질문할 때까지 받는다. 질문이면 그 질문.
fn wait_question(app: &mut App, job: JobId) -> Option<JobQuestion> {
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll_jobs();
        app.poll();
        let entry = app.work_area().entry(job).expect("작업 영역의 작업");
        if let Some(question) = &entry.question {
            return Some(question.clone());
        }
        if entry.summary.is_some() {
            return None;
        }
        assert!(Instant::now() < deadline, "작업이 끝나지 않았다");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// 파일들을 휴지통 삭제하고 끝날 때까지 기다린다. 이 계정이 휴지통을 쓸 수 없으면 건너뛴다고 출력하고 `false`.
fn recycle(app: &mut App, sources: Vec<PathBuf>, bin: &mut Bin, test: &str) -> bool {
    if !sources.iter().all(|source| job::has_recycle_bin(source)) {
        println!("건너뜀({test}): 휴지통이 없는 볼륨입니다");
        return false;
    }
    let job = app.recycle(sources).expect("작업을 시작한다");
    match wait_question(app, job) {
        None => {}
        Some(JobQuestion::NotRecyclable(_)) => {
            app.answer_not_recyclable(job, NotRecyclableAnswer::CancelJob)
                .unwrap();
            wait_question(app, job);
            println!("건너뜀({test}): 이 계정은 휴지통을 쓸 수 없습니다");
            return false;
        }
        Some(other) => panic!("묻지 않아야 한다: {other:?}"),
    }
    let entry = app.work_area().entry(job).unwrap();
    let summary = entry.summary.clone().unwrap();
    bin.keep(&summary);
    assert_eq!(entry.state, JobState::Finished, "{summary:?}");
    true
}

/// 활성 탭이 `expected`에 닿아 목록을 다 읽을 때까지 결과를 받는다.
fn settle(app: &mut App, expected: &Location) {
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll();
        let tab = app.active_tab();
        if tab.pending_location().is_none() {
            if let Some(error) = tab.error() {
                panic!("읽기가 실패했다: {error}");
            }
            if tab.location() == expected && tab.loading().is_none() {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "{expected}에 닿지 못했다: 지금 {}",
            tab.location()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn open_recycle_bin(app: &mut App) {
    assert_eq!(app.execute(CommandId::OpenRecycleBin), Execution::Done);
    settle(app, &Location::recycle_bin());
}

/// 휴지통 목록에서 원래 경로가 `original`인 항목의 번호.
fn find(app: &App, original: &Path) -> Option<usize> {
    let tab = app.active_tab();
    (0..tab.len()).find(|&i| {
        tab.recycled(i)
            .is_some_and(|entry| entry.original == original)
    })
}

fn index_of(app: &App, original: &Path) -> usize {
    find(app, original).unwrap_or_else(|| panic!("휴지통에 {}가 없다", original.display()))
}

/// 메뉴에서 명령 `id` 줄이 켜져 있는가.
fn row_enabled(menu: &Menu, id: CommandId) -> bool {
    menu.commands()
        .find(|command| command.id == id)
        .unwrap_or_else(|| panic!("메뉴에 {id:?}가 없다"))
        .enabled
}

/// 휴지통 안에서 위치가 막는 명령.
const BLOCKED: [CommandId; 10] = [
    CommandId::Rename,
    CommandId::BatchRename,
    CommandId::NewFolder,
    CommandId::Paste,
    CommandId::Copy,
    CommandId::Cut,
    CommandId::CopyTo,
    CommandId::MoveTo,
    CommandId::CopyToOtherPanel,
    CommandId::MoveToOtherPanel,
];

#[test]
#[ignore = "#352"]
fn recycle_bin_location_is_made_without_the_shell() {
    let recycle_bin = Location::recycle_bin();
    assert_eq!(recycle_bin, Location::recycle_bin());
    assert_ne!(recycle_bin, Location::this_pc());
    assert_eq!(
        recycle_bin.virtual_folder(),
        Some(VirtualFolder::RecycleBin)
    );
    assert_eq!(recycle_bin.file_system_path(), None);
}

#[test]
#[ignore = "#352"]
fn a_recycled_file_shows_its_name_original_location_date_and_size() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("지운 파일.txt");
    write(&path, "다섯 글자");
    let size = fs::metadata(&path).unwrap().len();
    let mut app = app_at(dir.path());
    let mut bin = Bin::default();

    let before = SystemTime::now() - CLOCK_SLACK;
    if !recycle(&mut app, vec![path.clone()], &mut bin, "보이기") {
        return;
    }
    let after = SystemTime::now() + CLOCK_SLACK;

    open_recycle_bin(&mut app);
    let tab = app.active_tab();
    assert_eq!(
        tab.location().virtual_folder(),
        Some(VirtualFolder::RecycleBin)
    );
    let index = index_of(&app, &path);
    let item = tab.get(index).unwrap();
    assert_eq!(item.name, "지운 파일.txt", "원래 이름(확장자 포함)");
    assert!(!item.is_folder);
    assert_eq!(item.size, size);
    let entry = tab.recycled(index).unwrap();
    assert_eq!(entry.original, path);
    assert!(
        before <= entry.deleted && entry.deleted <= after,
        "지운 날짜 {:?}가 지운 때가 아니다",
        entry.deleted
    );
}

#[test]
#[ignore = "#352"]
fn address_names_go_to_the_recycle_bin() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("주소로 찾기.txt");
    write(&path, "a");
    let mut bin = Bin::default();
    let mut app = app_at(dir.path());
    if !recycle(&mut app, vec![path.clone()], &mut bin, "주소창") {
        return;
    }
    for text in ["휴지통", "shell:RecycleBinFolder"] {
        let mut app = app_at(dir.path());
        assert!(app.go_to_text(text), "{text}");
        settle(&mut app, &Location::recycle_bin());
        index_of(&app, &path);
    }
}

#[test]
#[ignore = "#352"]
fn recycle_bins_of_other_volumes_show_in_the_same_list() {
    let Some(other) = std::env::var_os("FACET_TEST_OTHER_VOLUME_DIR") else {
        println!("건너뜀(다른 볼륨): FACET_TEST_OTHER_VOLUME_DIR이 없습니다");
        return;
    };
    let dir = TempDir::new().unwrap();
    let other = TempDir::new_in(other).unwrap();
    let here = dir.path().join("이쪽.txt");
    let there = other.path().join("저쪽.txt");
    write(&here, "a");
    write(&there, "b");
    let mut bin = Bin::default();
    let mut app = app_at(dir.path());
    if !recycle(&mut app, vec![here.clone()], &mut bin, "다른 볼륨") {
        return;
    }
    if !recycle(&mut app, vec![there.clone()], &mut bin, "다른 볼륨") {
        return;
    }

    open_recycle_bin(&mut app);
    index_of(&app, &here);
    index_of(&app, &there);
}

#[test]
#[ignore = "#352"]
fn sorting_by_original_location_and_date_deleted() {
    let dir = TempDir::new().unwrap();
    let first = dir.path().join("가").join("같은 이름.txt");
    let second = dir.path().join("나").join("같은 이름.txt");
    write(&first, "a");
    write(&second, "b");
    let mut bin = Bin::default();
    let mut app = app_at(dir.path());
    if !recycle(&mut app, vec![first.clone()], &mut bin, "정렬") {
        return;
    }
    // 지운 날짜가 갈리게 조금 기다린다.
    std::thread::sleep(Duration::from_millis(1100));
    if !recycle(&mut app, vec![second.clone()], &mut bin, "정렬") {
        return;
    }
    open_recycle_bin(&mut app);

    for column in [Column::OriginalLocation, Column::DateDeleted] {
        for direction in [Direction::Ascending, Direction::Descending] {
            app.set_sort(Sort { column, direction });
            let a = index_of(&app, &first);
            let b = index_of(&app, &second);
            match direction {
                Direction::Ascending => assert!(a < b, "{column:?} 오름차순: {a} {b}"),
                Direction::Descending => assert!(a > b, "{column:?} 내림차순: {a} {b}"),
            }
        }
    }

    app.set_sort(Sort {
        column: Column::DateDeleted,
        direction: Direction::Ascending,
    });
    let tab = app.active_tab();
    let dates: Vec<SystemTime> = (0..tab.len())
        .filter_map(|i| tab.recycled(i).map(|entry| entry.deleted))
        .collect();
    assert_eq!(dates.len(), tab.len(), "모든 항목이 휴지통 항목이다");
    assert!(
        dates.windows(2).all(|pair| pair[0] <= pair[1]),
        "지운 날짜 오름차순이 아니다"
    );
}

#[test]
#[ignore = "#352"]
fn the_recycle_bin_can_restore_empty_and_delete_permanently_only() {
    let dir = TempDir::new().unwrap();
    let mut app = app_at(dir.path());
    open_recycle_bin(&mut app);
    let capabilities = app.active_tab().capabilities();
    assert!(capabilities.restore, "{capabilities:?}");
    assert!(capabilities.empty, "{capabilities:?}");
    assert!(capabilities.delete_permanently, "{capabilities:?}");
    assert!(!capabilities.rename, "{capabilities:?}");
    assert!(!capabilities.new_folder, "{capabilities:?}");
    assert!(!capabilities.copy_into, "{capabilities:?}");
    assert!(!capabilities.copy_from, "{capabilities:?}");
    assert!(!capabilities.move_items, "{capabilities:?}");
    assert!(!capabilities.recycle, "{capabilities:?}");
}

#[test]
#[ignore = "#352"]
fn blocked_commands_are_not_allowed_here_and_change_nothing() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("막힌 명령.txt");
    let other = dir.path().join("다른 파일.txt");
    write(&path, "a");
    write(&other, "b");
    let mut bin = Bin::default();
    let mut app = app_at(dir.path());
    if !recycle(
        &mut app,
        vec![path.clone(), other.clone()],
        &mut bin,
        "막힌 명령",
    ) {
        return;
    }
    open_recycle_bin(&mut app);
    let first = index_of(&app, &path);
    let second = index_of(&app, &other);
    app.active_tab_mut().select_only(first);
    app.active_tab_mut().toggle_select(second);
    let jobs = app.work_area().entries().len();

    for command in BLOCKED {
        assert!(!app.can_execute(command), "{command:?}는 꺼진다");
        assert!(
            !command.is_enabled(app.active_tab()),
            "{command:?}는 꺼진다"
        );
        assert_eq!(
            app.execute(command),
            Execution::NotAllowedHere,
            "{command:?}"
        );
    }

    app.poll_jobs();
    app.poll();
    assert_eq!(app.work_area().entries().len(), jobs, "작업을 넣지 않았다");
    assert!(!path.exists(), "아무것도 되살리지 않았다");
    assert!(!other.exists());
    assert_eq!(
        app.active_tab().location(),
        &Location::recycle_bin(),
        "위치가 그대로다"
    );
    index_of(&app, &path);
    index_of(&app, &other);
}

#[test]
#[ignore = "#352"]
fn the_item_menu_keeps_its_order_and_disables_blocked_rows() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("메뉴.txt");
    write(&path, "a");
    write(&dir.path().join("남는 파일.txt"), "b");
    let mut bin = Bin::default();
    let mut app = app_at(dir.path());

    // 실제 폴더의 항목 메뉴(견줄 줄 순서).
    let kept = (0..app.active_tab().len())
        .find(|&i| app.active_tab().get(i).unwrap().name == "남는 파일.txt")
        .unwrap();
    assert_eq!(app.context_click(0, Some(kept)), Some(MenuKind::Item));
    let folder_menu = app.context_menu(MenuKind::Item);

    if !recycle(&mut app, vec![path.clone()], &mut bin, "메뉴") {
        return;
    }
    open_recycle_bin(&mut app);
    let index = index_of(&app, &path);
    assert_eq!(app.context_click(0, Some(index)), Some(MenuKind::Item));
    let menu = app.context_menu(MenuKind::Item);

    let ids = |menu: &Menu| menu.commands().map(|c| c.id).collect::<Vec<_>>();
    assert_eq!(ids(&menu), ids(&folder_menu), "줄 순서는 그대로다");
    assert_eq!(menu.entries.len(), folder_menu.entries.len());
    for command in BLOCKED {
        if menu.commands().any(|c| c.id == command) {
            assert!(!row_enabled(&menu, command), "{command:?} 줄은 꺼진다");
            assert_eq!(row_enabled(&menu, command), app.can_execute(command));
        }
    }
}

#[test]
#[ignore = "#352"]
fn recycled_items_are_not_drag_sources() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("끌기.txt");
    write(&path, "a");
    let mut bin = Bin::default();
    let mut app = app_at(dir.path());
    if !recycle(&mut app, vec![path.clone()], &mut bin, "끌기") {
        return;
    }
    open_recycle_bin(&mut app);
    let index = index_of(&app, &path);
    app.active_tab_mut().select_only(index);
    assert_eq!(app.drag_sources(), None);
}

#[test]
#[ignore = "#352"]
fn a_recycle_bin_tab_comes_back_from_the_session() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("세션.txt");
    write(&path, "a");
    let mut bin = Bin::default();
    let mut app = app_at(dir.path());
    if !recycle(&mut app, vec![path.clone()], &mut bin, "세션") {
        return;
    }
    open_recycle_bin(&mut app);
    let json = app.session().expect("창이 있다").to_json();
    drop(app);

    let session = Session::from_json(&json).expect("세션을 읽는다");
    let mut restored = App::restore(Some(session), Vec::new());
    settle(&mut restored, &Location::recycle_bin());
    index_of(&restored, &path);
}
