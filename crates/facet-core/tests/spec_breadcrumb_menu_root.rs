// target: crates/facet-core/tests/spec_breadcrumb_menu_root.rs
// AC: #305: 맨 앞 목록에 홈 폴더(마지막 줄, 홈 번호), 시스템 드라이브(이름 C: 꼴·위치 C:\ 꼴, 문자 순서), 활성 탭의 드라이브가 다음 조각, 하위 폴더 요청과 같은 번호 차례
//! 주소창 맨 앞 › 펼침 메뉴가 쓰는 목록(`App::request_root_folders` → `Update::Subfolders`)을 확인한다: 사용자 홈 폴더가
//! 드라이브들 뒤 마지막 줄에 하나 있고 홈 번호가 그것을 가리킴, 앞선 요청은 뒤 요청(하위 폴더 목록 포함)이 있으면 목록 없이
//! 옴(모든 플랫폼), 시스템 드라이브가 경로 조각과 같은 이름·위치로 있고 드라이브는 문자 순서이며 활성 탭의 드라이브가 다음
//! 조각 번호임(Windows 전용).

use std::path::Path;
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::tab::ReadOutcome;
use facet_core::window::{App, Subfolders, SubfoldersError, SubfoldersId, Update, home_location};
#[cfg(windows)]
use facet_core::{FileSystemProvider, Provider};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(30);

type Listed = Result<Subfolders, SubfoldersError>;

fn app_at(path: &Path) -> App {
    let mut app = App::new(Some(Location::file_system(path)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

/// `id` 요청의 결과가 올 때까지 기다린다. 다른 결과(읽기 등)는 넘긴다.
fn wait_subfolders(app: &mut App, id: SubfoldersId) -> Option<Listed> {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if let (_, Update::Subfolders { id: got, list }) =
            app.wait_update(left).expect("맨 앞 목록 결과")
            && got == id
        {
            return list;
        }
    }
}

fn root_list(app: &mut App) -> Subfolders {
    let id = app.request_root_folders();
    wait_subfolders(app, id)
        .expect("마지막 요청의 결과는 목록을 담는다")
        .expect("맨 앞 목록은 실패하지 않는다")
}

#[test]
fn 맨_앞_목록의_마지막_줄은_사용자_홈_폴더다() {
    let dir = TempDir::new().unwrap();
    let mut app = app_at(dir.path());
    let list = root_list(&mut app);
    let home = list.home.expect("홈 번호가 있다");
    assert_eq!(home, list.folders.len() - 1, "홈은 드라이브들 뒤 마지막 줄");
    assert_eq!(list.folders[home].location, home_location());
    let homes = list
        .folders
        .iter()
        .filter(|folder| folder.location == home_location())
        .count();
    assert_eq!(homes, 1, "{list:?}");
}

#[test]
fn 맨_앞_목록_요청도_하위_폴더_요청과_같은_번호_차례를_쓴다() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("안")).unwrap();
    let mut app = app_at(dir.path());

    let root = app.request_root_folders();
    let sub = app
        .request_subfolders(&Location::file_system(dir.path()))
        .unwrap();
    assert_ne!(root, sub);
    let (root_list, sub_list) = wait_both(&mut app, root, sub);
    assert_eq!(root_list, None, "뒤 요청이 있으면 목록 없음");
    let list = sub_list.unwrap().unwrap();
    assert_eq!(list.folders.len(), 1);

    let sub = app
        .request_subfolders(&Location::file_system(dir.path()))
        .unwrap();
    let root = app.request_root_folders();
    let (sub_list, root_list) = wait_both(&mut app, sub, root);
    assert_eq!(sub_list, None, "뒤 요청이 있으면 목록 없음");
    assert!(root_list.unwrap().is_ok());
}

/// 두 요청의 결과를 도착 차례와 상관없이 모두 받는다.
fn wait_both(
    app: &mut App,
    first: SubfoldersId,
    second: SubfoldersId,
) -> (Option<Listed>, Option<Listed>) {
    let mut results: [Option<Option<Listed>>; 2] = [None, None];
    let deadline = Instant::now() + WAIT;
    while results.iter().any(Option::is_none) {
        let left = deadline.saturating_duration_since(Instant::now());
        let (_, update) = app.wait_update(left).expect("두 요청의 결과");
        let Update::Subfolders { id, list } = update else {
            continue;
        };
        let slot = if id == first {
            0
        } else if id == second {
            1
        } else {
            panic!("요청하지 않은 번호: {id:?}");
        };
        assert!(results[slot].is_none(), "요청마다 결과는 하나");
        results[slot] = Some(list);
    }
    let [a, b] = results;
    (a.unwrap(), b.unwrap())
}

/// 드라이브 줄(홈 줄 앞)의 이름들.
#[cfg(windows)]
fn drive_names(list: &Subfolders) -> Vec<String> {
    let end = list.home.unwrap_or(list.folders.len());
    list.folders[..end]
        .iter()
        .map(|folder| folder.name.to_string_lossy().into_owned())
        .collect()
}

#[cfg(windows)]
fn system_drive() -> String {
    std::env::var("SystemDrive")
        .unwrap_or_else(|_| "C:".to_owned())
        .to_uppercase()
}

#[cfg(windows)]
#[test]
fn 맨_앞_목록에_시스템_드라이브가_조각과_같은_이름과_위치로_있다() {
    let dir = TempDir::new().unwrap();
    let mut app = app_at(dir.path());
    let list = root_list(&mut app);
    let drives = drive_names(&list);
    let system = system_drive();
    let index = drives
        .iter()
        .position(|name| name.eq_ignore_ascii_case(&system))
        .unwrap_or_else(|| panic!("{system} 드라이브가 없다: {drives:?}"));
    let root = Location::file_system(format!("{system}\\"));
    assert!(
        list.folders[index]
            .location
            .file_system_path()
            .is_some_and(|path| path
                .to_string_lossy()
                .eq_ignore_ascii_case(&format!("{system}\\"))),
        "드라이브 위치는 루트: {:?}",
        list.folders[index].location
    );
    let segment = FileSystemProvider.segments(&root).remove(0);
    assert!(segment.label.eq_ignore_ascii_case(&drives[index]));

    for name in &drives {
        assert_eq!(name.len(), 2, "드라이브 이름은 「C:」 꼴: {name}");
        assert!(name.ends_with(':'));
    }
    let mut sorted = drives.clone();
    sorted.sort_by_key(|name| name.to_uppercase());
    assert_eq!(drives, sorted, "드라이브는 문자 순서");
}

#[cfg(windows)]
#[test]
fn 맨_앞_목록의_다음_조각은_활성_탭의_드라이브다() {
    let dir = TempDir::new().unwrap();
    let mut app = app_at(dir.path());
    let list = root_list(&mut app);
    let tab_location = app.active_tab().location().clone();
    let first = FileSystemProvider.segments(&tab_location).remove(0);
    let current = list.current.expect("활성 탭의 드라이브가 있다");
    assert!(
        list.folders[current]
            .name
            .to_string_lossy()
            .eq_ignore_ascii_case(&first.label),
        "{:?} / {first:?}",
        list.folders[current]
    );
    assert_ne!(Some(current), list.home);
}
