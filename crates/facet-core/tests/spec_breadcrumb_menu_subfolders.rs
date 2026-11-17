// target: crates/facet-core/tests/spec_breadcrumb_menu_subfolders.rs
// AC: #303: 폴더만·자연 정렬, 숨김·시스템 폴더는 숨김 보이기를 따름, 다음 조각 번호, 없음·권한 없음의 까닭, 막지 않는 요청과 늦게 온 결과 버리기, 검색 결과 위치의 조각
//! 주소창 경로 조각의 › 펼침 메뉴가 쓰는 하위 폴더 목록(`App::request_subfolders` → `Update::Subfolders`)을 임시 폴더의
//! 실제 파일로 확인한다: 폴더만 자연 정렬로 담기, 숨김·시스템 속성 폴더와 숨김 보이기(Windows 전용), 활성 탭 위치에서 다음
//! 조각에 해당하는 폴더의 번호, 없는 폴더와 권한 없는 폴더의 까닭(권한 거부는 Windows 전용이고 걸 수 없으면 건너뜀), 하위
//! 폴더 1만 개에서도 곧바로 돌아오는 요청과 늦게 온 앞선 결과 버리기, 검색 결과 위치에서 검색어 조각은 요청이 없음.
//! 나머지는 모든 플랫폼에서 돈다.

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use facet_core::details::compare_names;
use facet_core::tab::ReadOutcome;
use facet_core::window::{App, Subfolders, SubfoldersError, SubfoldersId, Update};
use facet_core::{FileSystemProvider, Location, PathSegment, Provider, provider_for};
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
            app.wait_update(left).expect("하위 폴더 목록 결과")
            && got == id
        {
            return list;
        }
    }
}

/// 요청하고 결과(마지막 요청이므로 목록이 있음)를 받는다.
fn list_of(app: &mut App, location: &Location) -> Listed {
    let id = app
        .request_subfolders(location)
        .expect("실제 폴더 위치는 요청이 된다");
    wait_subfolders(app, id).expect("마지막 요청의 결과는 목록을 담는다")
}

fn names(list: &Subfolders) -> Vec<String> {
    list.folders
        .iter()
        .map(|folder| folder.name.to_string_lossy().into_owned())
        .collect()
}

fn current_name(list: &Subfolders) -> Option<String> {
    list.current
        .map(|i| list.folders[i].name.to_string_lossy().into_owned())
}

/// 활성 탭 위치의 경로 조각.
fn tab_segments(app: &App) -> Vec<PathSegment> {
    let location = app.active_tab().location().clone();
    provider_for(&location).segments(&location)
}

/// 조각들 가운데 위치가 `location`인 것.
fn segment_at(segments: &[PathSegment], location: &Location) -> PathSegment {
    segments
        .iter()
        .find(|segment| &segment.location == location)
        .unwrap_or_else(|| panic!("{location:?} 조각이 없다: {segments:?}"))
        .clone()
}

#[test]
fn 하위_폴더_목록은_폴더만_자연_정렬로_담는다() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    for name in ["폴더10", "폴더2", "폴더1", "beta", "Alpha"] {
        fs::create_dir(root.join(name)).unwrap();
    }
    fs::write(root.join("file.txt"), b"x").unwrap();
    fs::write(root.join("폴더3.txt"), b"x").unwrap();
    let mut app = app_at(root);

    let list = list_of(&mut app, &Location::file_system(root)).unwrap();
    let got = names(&list);
    let mut expected: Vec<String> = ["폴더10", "폴더2", "폴더1", "beta", "Alpha"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    expected.sort_by(|a, b| compare_names(a, b));
    assert_eq!(got, expected, "폴더만, 자세히 보기와 같은 자연 정렬");
    let position = |name: &str| got.iter().position(|n| n == name).unwrap();
    assert!(position("폴더1") < position("폴더2"));
    assert!(position("폴더2") < position("폴더10"));
    for folder in &list.folders {
        assert_eq!(
            folder.location,
            Location::file_system(root.join(&folder.name))
        );
    }
    assert_eq!(list.home, None, "하위 폴더 목록에는 홈 번호가 없다");
}

#[test]
fn 하위_폴더가_없으면_빈_목록이다() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("only-file.txt"), b"x").unwrap();
    let mut app = app_at(dir.path());
    let list = list_of(&mut app, &Location::file_system(dir.path())).unwrap();
    assert!(list.folders.is_empty(), "{list:?}");
    assert_eq!(list.current, None);
}

#[cfg(windows)]
fn attrib(flag: &str, path: &Path) {
    let status = std::process::Command::new("attrib")
        .arg(flag)
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success(), "attrib {flag} 실패");
}

#[cfg(windows)]
#[test]
fn 숨김과_시스템_폴더는_숨김_보이기를_따른다() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    for name in ["보임", "숨김", "시스템"] {
        fs::create_dir(root.join(name)).unwrap();
    }
    attrib("+h", &root.join("숨김"));
    attrib("+s", &root.join("시스템"));
    let mut app = app_at(root);
    let location = Location::file_system(root);

    app.set_show_hidden(false);
    let hidden_off = list_of(&mut app, &location).unwrap();
    assert_eq!(names(&hidden_off), ["보임"], "숨김·시스템 폴더는 빠진다");

    app.set_show_hidden(true);
    let hidden_on = list_of(&mut app, &location).unwrap();
    let mut got = names(&hidden_on);
    got.sort();
    let mut expected = vec!["보임".to_owned(), "숨김".to_owned(), "시스템".to_owned()];
    expected.sort();
    assert_eq!(got, expected, "숨김 보이기를 켜면 모두 나온다");

    attrib("-s", &root.join("시스템"));
    attrib("-h", &root.join("숨김"));
}

#[test]
fn 다음_조각_번호는_활성_탭_경로의_다음_폴더를_가리킨다() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("a").join("b").join("c")).unwrap();
    fs::create_dir(root.join("a").join("가")).unwrap();
    fs::create_dir(root.join("a").join("z")).unwrap();
    fs::create_dir(root.join("0")).unwrap();
    fs::create_dir(root.join("zz")).unwrap();
    let mut app = app_at(&root.join("a").join("b"));
    let segments = tab_segments(&app);

    let root_segment = segment_at(&segments, &Location::file_system(root));
    let list = list_of(&mut app, &root_segment.location).unwrap();
    assert_eq!(current_name(&list).as_deref(), Some("a"), "{list:?}");

    let a_segment = segment_at(&segments, &Location::file_system(root.join("a")));
    let list = list_of(&mut app, &a_segment.location).unwrap();
    assert_eq!(names(&list).len(), 3);
    assert_eq!(current_name(&list).as_deref(), Some("b"), "{list:?}");

    let last = segments.last().unwrap().clone();
    assert_eq!(
        last.location,
        Location::file_system(root.join("a").join("b"))
    );
    let list = list_of(&mut app, &last.location).unwrap();
    assert_eq!(names(&list), ["c"]);
    assert_eq!(list.current, None, "마지막 조각에는 다음 조각이 없다");
}

#[test]
fn 다음_조각은_요청_때의_활성_탭을_따른다() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("a")).unwrap();
    fs::create_dir(root.join("b")).unwrap();
    let mut app = app_at(&root.join("b"));
    let list = list_of(&mut app, &Location::file_system(root)).unwrap();
    assert_eq!(current_name(&list).as_deref(), Some("b"));

    assert!(app.go_to_text(root.join("a").to_str().unwrap()));
    assert_eq!(app.wait(WAIT).unwrap().1, ReadOutcome::Committed);
    let list = list_of(&mut app, &Location::file_system(root)).unwrap();
    assert_eq!(current_name(&list).as_deref(), Some("a"));
}

#[test]
fn 없는_폴더는_없음을_까닭으로_돌려준다() {
    let dir = TempDir::new().unwrap();
    let mut app = app_at(dir.path());
    let missing = Location::file_system(dir.path().join("없는 폴더"));
    assert_eq!(list_of(&mut app, &missing), Err(SubfoldersError::NotFound));
}

#[cfg(windows)]
fn icacls(path: &Path, args: &[&str]) -> bool {
    std::process::Command::new("icacls")
        .arg(path)
        .args(args)
        .status()
        .is_ok_and(|status| status.success())
}

/// 목록 읽기 거부를 걸었다가 버릴 때 푼다.
#[cfg(windows)]
struct Denied(std::path::PathBuf);

#[cfg(windows)]
impl Drop for Denied {
    fn drop(&mut self) {
        icacls(&self.0, &["/remove:d", "*S-1-1-0"]);
    }
}

#[cfg(windows)]
#[test]
fn 권한_없는_폴더는_권한_없음을_까닭으로_돌려준다() {
    let dir = TempDir::new().unwrap();
    let locked = dir.path().join("잠긴 폴더");
    fs::create_dir_all(locked.join("안")).unwrap();
    assert!(icacls(&locked, &["/deny", "*S-1-1-0:(RD)"]), "icacls 실패");
    let _guard = Denied(locked.clone());
    if fs::read_dir(&locked).is_ok() {
        println!("건너뜀: 이 환경(백업 권한 등)에서는 목록 읽기 거부가 듣지 않는다");
        return;
    }
    let mut app = app_at(dir.path());
    assert_eq!(
        list_of(&mut app, &Location::file_system(&locked)),
        Err(SubfoldersError::PermissionDenied)
    );
}

#[test]
fn 늦게_온_앞선_요청의_결과는_목록_없이_온다() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("a").join("안")).unwrap();
    fs::create_dir(root.join("b")).unwrap();
    let mut app = app_at(root);

    let first = app
        .request_subfolders(&Location::file_system(root))
        .unwrap();
    let second = app
        .request_subfolders(&Location::file_system(root.join("a")))
        .unwrap();
    assert_ne!(first, second);

    let mut current = None;
    let mut stale = 0;
    let deadline = Instant::now() + WAIT;
    while current.is_none() || stale == 0 {
        let left = deadline.saturating_duration_since(Instant::now());
        let (_, update) = app.wait_update(left).expect("두 요청의 결과");
        let Update::Subfolders { id, list } = update else {
            continue;
        };
        match list {
            Some(found) => {
                assert_eq!(id, second, "목록은 마지막 요청의 것만 담는다");
                current = Some(found);
            }
            None => {
                assert_eq!(id, first, "앞선 요청의 결과는 목록이 없다");
                stale += 1;
            }
        }
    }
    assert_eq!(stale, 1);
    assert_eq!(names(&current.unwrap().unwrap()), ["안"]);
}

#[test]
fn 하위_폴더_1만_개여도_요청은_곧바로_돌아온다() {
    const COUNT: usize = 10_000;
    let dir = TempDir::new().unwrap();
    let big = dir.path().join("큰 폴더");
    fs::create_dir(&big).unwrap();
    for i in 0..COUNT {
        fs::create_dir(big.join(format!("폴더 {i}"))).unwrap();
    }
    let mut app = app_at(dir.path());

    let started = Instant::now();
    let id = app
        .request_subfolders(&Location::file_system(&big))
        .unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(100),
        "요청은 읽기를 기다리지 않는다: {elapsed:?}"
    );
    let list = wait_subfolders(&mut app, id).unwrap().unwrap();
    assert_eq!(list.folders.len(), COUNT);
    let got = names(&list);
    let mut sorted = got.clone();
    sorted.sort_by(|a, b| compare_names(a, b));
    assert_eq!(got, sorted, "자연 정렬");
    assert_eq!(got[..3], ["폴더 0", "폴더 1", "폴더 2"]);
}

#[test]
fn 검색_결과_위치에서는_검색어_조각만_요청이_없다() {
    let dir = TempDir::new().unwrap();
    let folder = dir.path().join("a");
    fs::create_dir_all(folder.join("안")).unwrap();
    let mut app = app_at(dir.path());

    let results = Location::search_results(&folder, "보고서");
    let segments = provider_for(&results).segments(&results);
    let (query, folders) = segments.split_last().unwrap();
    assert_eq!(query.location, results);
    assert!(
        app.request_subfolders(&query.location).is_none(),
        "검색어 조각 뒤에는 ›가 없다"
    );
    assert!(app.request_subfolders(&results).is_none());
    assert_eq!(
        folders.len(),
        FileSystemProvider
            .segments(&Location::file_system(&folder))
            .len()
    );
    let last_folder = folders.last().unwrap().location.clone();
    assert_eq!(last_folder, Location::file_system(&folder));
    assert_eq!(names(&list_of(&mut app, &last_folder).unwrap()), ["안"]);

    let instant = Location::instant_search_results(&folder, "보고서", false);
    let segments = provider_for(&instant).segments(&instant);
    assert_eq!(segments.len(), 1);
    assert!(
        app.request_subfolders(&segments[0].location).is_none(),
        "모든 볼륨 즉시 검색 위치의 유일한 조각도 요청이 없다"
    );
}

#[test]
fn 검색_결과_위치의_다음_조각은_검색한_폴더까지다() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("a").join("안")).unwrap();
    fs::create_dir(root.join("b")).unwrap();
    fs::write(root.join("a").join("보고서.txt"), b"x").unwrap();
    let mut app = app_at(&root.join("a"));
    app.start_search("보고서").unwrap();
    assert_eq!(app.wait(WAIT).unwrap().1, ReadOutcome::Committed);
    assert!(app.active_tab().location().is_search_results());

    let segments = tab_segments(&app);
    let root_segment = segment_at(&segments, &Location::file_system(root));
    let list = list_of(&mut app, &root_segment.location).unwrap();
    assert_eq!(current_name(&list).as_deref(), Some("a"));

    let folder_segment = segment_at(&segments, &Location::file_system(root.join("a")));
    let list = list_of(&mut app, &folder_segment.location).unwrap();
    assert_eq!(names(&list), ["안"]);
    assert_eq!(list.current, None, "검색어 조각은 폴더가 아니다");
}

#[test]
fn 하위_폴더_이름은_디스크의_이름_그대로다() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("MiXeD 대소문자")).unwrap();
    let mut app = app_at(dir.path());
    let list = list_of(&mut app, &Location::file_system(dir.path())).unwrap();
    assert_eq!(list.folders.len(), 1);
    assert_eq!(list.folders[0].name, OsString::from("MiXeD 대소문자"));
}
