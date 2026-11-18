// target: crates/facet-core/tests/spec_zip_location.rs
// AC: #423: zip 위치의 목록, 경로 조각·상위·하위, 주소창 글자, 열기·위로·뒤로·앞으로, 항목 경로, 압축된 크기와 정렬·필터, zip 파일 판정, 열 수 없는 zip·손상·같은 이름, 꺼진 명령, zip 안의 zip, 세션
//! zip 파일 안을 위치로 여는 zip 공급자를 임시 폴더의 실제 zip 파일로 확인한다: 공급자의 목록·경로 조각·상위·하위·
//! 주소창 해석, 앱에서 zip 파일 열기와 zip 안 폴더 들어가기·위로·뒤로·앞으로, 항목 경로, 압축된 크기 열과 정렬,
//! 필터, `.zip`과 시그니처로 하는 판정, 열 수 없는 zip의 읽기 오류와 손상·같은 이름 상태 값, zip 안에서 꺼지는 명령,
//! zip 안의 zip 열기 거절, 세션 왕복. 시험 zip은 `spec_zip_support.rs`의 도우미로 만든다. 모든 플랫폼에서 돈다.

#[path = "spec_zip_support.rs"]
mod zip_support;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use facet_core::command::CommandId;
use facet_core::details::{Column, Direction, Sort};
use facet_core::session::Session;
use facet_core::tab::{Navigation, ReadOutcome};
use facet_core::window::{App, Execution, Update};
use facet_core::zip_provider::is_zip_file;
use facet_core::{ListError, Location, provider_for};
use tempfile::TempDir;
use zip_support::Entry;

const WAIT: Duration = Duration::from_secs(30);
const REPORT_TIME: u64 = 1_700_000_000;

/// 시험 폴더: `자료.zip`(문서/2024/보고서.txt, 문서/readme.txt, 문서/메모.txt, 맨 위.txt, 안쪽.zip)과 옆 파일 하나.
struct Fixture {
    dir: TempDir,
    zip: PathBuf,
}

fn fixture() -> Fixture {
    let dir = TempDir::new().unwrap();
    let inner = zip_support::build(&[Entry::stored("깊은.txt", b"deep")]);
    let zip = dir.path().join("자료.zip");
    zip_support::write_zip(
        &zip,
        &[
            Entry::stored("문서/2024/보고서.txt", "보고서 내용".as_bytes())
                .modified_unix(REPORT_TIME as u32),
            Entry::deflated("문서/readme.txt", &zip_support::repeated_text(20)),
            Entry::stored("문서/메모.txt", "메모".as_bytes()),
            Entry::stored("맨 위.txt", b"top"),
            Entry::stored("안쪽.zip", &inner),
        ],
    );
    std::fs::write(dir.path().join("옆.txt"), "next").unwrap();
    Fixture { dir, zip }
}

fn zip_loc(zip: &Path, inner: &str) -> Location {
    Location::zip(zip, inner)
}

fn names_of(items: &[facet_core::Item]) -> Vec<String> {
    let mut names: Vec<String> = items
        .iter()
        .map(|i| i.name.to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn app_at(location: Location) -> App {
    let mut app = App::new(Some(location));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

fn wait_read(app: &mut App) -> ReadOutcome {
    let (_, outcome) = app.wait(WAIT).expect("읽기 결과");
    outcome
}

fn tab_names(app: &App) -> Vec<String> {
    let tab = app.active_tab();
    (0..tab.len())
        .map(|i| tab.get(i).unwrap().name.to_string_lossy().into_owned())
        .collect()
}

fn index_of(app: &App, name: &str) -> usize {
    let tab = app.active_tab();
    (0..tab.len())
        .find(|&i| tab.get(i).unwrap().name == OsStr::new(name))
        .unwrap_or_else(|| panic!("{name}가 목록에 없다: {:?}", tab_names(app)))
}

fn cursor_name(app: &App) -> Option<String> {
    let tab = app.active_tab();
    tab.cursor()
        .and_then(|i| tab.get(i))
        .map(|item| item.name.to_string_lossy().into_owned())
}

/// 커서를 `name`에 두고 열기를 실행한다.
fn open_item(app: &mut App, name: &str) -> Execution {
    let index = index_of(app, name);
    app.active_tab_mut().select_only(index);
    app.execute(CommandId::Open)
}

#[test]
#[ignore = "#423"]
fn the_provider_lists_the_zip_root_and_subfolders() {
    let f = fixture();
    let root = zip_loc(&f.zip, "");
    let items = provider_for(&root).list(&root).expect("zip 루트를 읽는다");
    assert_eq!(names_of(&items), ["맨 위.txt", "문서", "안쪽.zip"]);
    let docs = items.iter().find(|i| i.name == "문서").unwrap();
    assert!(docs.is_folder, "폴더 기록 없는 중간 폴더도 폴더다");
    let top = items.iter().find(|i| i.name == "맨 위.txt").unwrap();
    assert!(!top.is_folder);
    assert_eq!(top.size, 3);

    let year = zip_loc(&f.zip, r"문서\2024");
    let items = provider_for(&year)
        .list(&year)
        .expect("zip 안 폴더를 읽는다");
    assert_eq!(names_of(&items), ["보고서.txt"]);
    assert_eq!(items[0].size, "보고서 내용".len() as u64);
    assert_eq!(
        items[0].modified,
        SystemTime::UNIX_EPOCH + Duration::from_secs(REPORT_TIME)
    );
}

#[test]
#[ignore = "#423"]
fn zip_locations_have_segments_parents_and_children() {
    let f = fixture();
    let year = zip_loc(&f.zip, r"문서\2024");
    assert_eq!(
        year.file_system_path(),
        None,
        "zip 위치는 실제 폴더가 아니다"
    );
    let zip_location = year.zip_location().expect("zip 위치");
    assert_eq!(zip_location.archive(), f.zip.as_path());
    assert_eq!(zip_location.inner(), r"문서\2024");
    assert_eq!(Location::file_system(f.dir.path()).zip_location(), None);

    let provider = provider_for(&year);
    let segments = provider.segments(&year);
    let tail: Vec<(String, Location)> = segments[segments.len() - 3..]
        .iter()
        .map(|s| (s.label.clone(), s.location.clone()))
        .collect();
    assert_eq!(
        tail,
        [
            ("자료.zip".to_string(), zip_loc(&f.zip, "")),
            ("문서".to_string(), zip_loc(&f.zip, "문서")),
            ("2024".to_string(), year.clone()),
        ]
    );
    let before = &segments[segments.len() - 4];
    assert_eq!(before.location, Location::file_system(f.dir.path()));

    assert_eq!(provider.parent(&year), Some(zip_loc(&f.zip, "문서")));
    assert_eq!(
        provider.parent(&zip_loc(&f.zip, "문서")),
        Some(zip_loc(&f.zip, ""))
    );
    assert_eq!(
        provider.parent(&zip_loc(&f.zip, "")),
        Some(Location::file_system(f.dir.path())),
        "zip 루트의 위는 zip이 든 실제 폴더"
    );
    assert_eq!(
        provider.child(&zip_loc(&f.zip, "문서"), OsStr::new("2024")),
        year
    );
    assert_eq!(
        provider.child(&zip_loc(&f.zip, ""), OsStr::new("문서")),
        zip_loc(&f.zip, "문서")
    );
}

#[test]
#[ignore = "#423"]
fn typing_a_path_inside_a_zip_goes_there() {
    let f = fixture();
    let mut app = app_at(Location::file_system(f.dir.path()));

    let text = f.zip.join("문서").join("2024");
    assert!(app.go_to_text(&text.to_string_lossy()));
    assert_eq!(wait_read(&mut app), ReadOutcome::Committed);
    assert_eq!(app.active_tab().location(), &zip_loc(&f.zip, r"문서\2024"));
    assert_eq!(tab_names(&app), ["보고서.txt"]);

    assert!(app.go_to_text(&f.zip.to_string_lossy()));
    assert_eq!(wait_read(&mut app), ReadOutcome::Committed);
    assert_eq!(app.active_tab().location(), &zip_loc(&f.zip, ""));

    let file = f.zip.join("문서").join("readme.txt");
    assert!(app.go_to_text(&file.to_string_lossy()));
    assert_eq!(wait_read(&mut app), ReadOutcome::Committed);
    assert_eq!(app.active_tab().location(), &zip_loc(&f.zip, "문서"));
    assert_eq!(
        cursor_name(&app).as_deref(),
        Some("readme.txt"),
        "zip 안 파일 경로는 그 폴더에서 그 파일을 고른다"
    );
}

#[test]
#[ignore = "#423"]
fn opening_a_zip_file_and_moving_around_works_like_folders() {
    let f = fixture();
    let mut app = app_at(Location::file_system(f.dir.path()));

    assert_eq!(open_item(&mut app, "자료.zip"), Execution::Done);
    assert_eq!(wait_read(&mut app), ReadOutcome::Committed);
    assert_eq!(app.active_tab().location(), &zip_loc(&f.zip, ""));

    assert!(app.navigate(Navigation::Open("문서".into())));
    assert_eq!(wait_read(&mut app), ReadOutcome::Committed);
    assert_eq!(app.active_tab().location(), &zip_loc(&f.zip, "문서"));
    let mut listed = tab_names(&app);
    listed.sort();
    assert_eq!(listed, ["2024", "readme.txt", "메모.txt"]);

    assert!(app.navigate(Navigation::Back));
    assert_eq!(wait_read(&mut app), ReadOutcome::Committed);
    assert_eq!(app.active_tab().location(), &zip_loc(&f.zip, ""));
    assert!(app.navigate(Navigation::Forward));
    assert_eq!(wait_read(&mut app), ReadOutcome::Committed);
    assert_eq!(app.active_tab().location(), &zip_loc(&f.zip, "문서"));

    assert!(app.navigate(Navigation::Up));
    assert_eq!(wait_read(&mut app), ReadOutcome::Committed);
    assert_eq!(app.active_tab().location(), &zip_loc(&f.zip, ""));
    assert_eq!(cursor_name(&app).as_deref(), Some("문서"), "방금 있던 폴더");

    assert!(app.navigate(Navigation::Up));
    assert_eq!(wait_read(&mut app), ReadOutcome::Committed);
    assert_eq!(
        app.active_tab().location(),
        &Location::file_system(f.dir.path())
    );
    assert_eq!(
        cursor_name(&app).as_deref(),
        Some("자료.zip"),
        "zip 루트에서 위로 가면 그 zip 파일을 고른다"
    );
}

#[test]
#[ignore = "#423"]
fn item_paths_join_the_zip_path_and_the_inner_path() {
    let f = fixture();
    let app = app_at(zip_loc(&f.zip, "문서"));
    let index = index_of(&app, "readme.txt");
    assert_eq!(
        app.active_tab().item_path(index),
        Some(f.zip.join("문서").join("readme.txt"))
    );
}

#[test]
#[ignore = "#423"]
fn compressed_sizes_are_shown_and_sortable() {
    let f = fixture();
    let mut app = app_at(zip_loc(&f.zip, "문서"));
    let readme = index_of(&app, "readme.txt");
    let memo = index_of(&app, "메모.txt");
    let deflated = zip_support::fixed_deflate(&zip_support::repeated_text(20)).len() as u64;
    assert_eq!(app.active_tab().compressed_size(readme), Some(deflated));
    assert_eq!(
        app.active_tab().compressed_size(memo),
        Some("메모".len() as u64)
    );

    app.set_sort(Sort {
        column: Column::CompressedSize,
        direction: Direction::Descending,
    });
    assert!(
        index_of(&app, "readme.txt") < index_of(&app, "메모.txt"),
        "압축된 크기 내림차순: {:?}",
        tab_names(&app)
    );
    app.set_sort(Sort {
        column: Column::CompressedSize,
        direction: Direction::Ascending,
    });
    assert!(
        index_of(&app, "메모.txt") < index_of(&app, "readme.txt"),
        "압축된 크기 오름차순: {:?}",
        tab_names(&app)
    );

    let real = app_at(Location::file_system(f.dir.path()));
    assert_eq!(
        real.active_tab().compressed_size(0),
        None,
        "실제 폴더에는 압축된 크기가 없다"
    );
}

#[test]
#[ignore = "#423"]
fn the_filter_works_inside_a_zip() {
    let f = fixture();
    let mut app = app_at(zip_loc(&f.zip, "문서"));
    app.set_filter("read");
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if let (_, Update::Filtered(true)) = app.wait_update(left).expect("필터 결과") {
            break;
        }
    }
    assert_eq!(tab_names(&app), ["readme.txt"]);
    assert_eq!(app.active_tab().location(), &zip_loc(&f.zip, "문서"));
}

#[test]
#[ignore = "#423"]
fn only_zip_files_with_the_signature_open_as_locations() {
    let f = fixture();
    let dir = f.dir.path();
    assert!(is_zip_file(&f.zip));

    let upper = dir.join("대문자.ZIP");
    std::fs::copy(&f.zip, &upper).unwrap();
    assert!(is_zip_file(&upper), "확장자는 대소문자를 가리지 않는다");

    let docx = dir.join("문서.docx");
    std::fs::copy(&f.zip, &docx).unwrap();
    assert!(!is_zip_file(&docx), "zip 기반 형식은 위치로 열지 않는다");

    let fake = dir.join("가짜.zip");
    std::fs::write(&fake, "zip이 아닌 글자").unwrap();
    assert!(!is_zip_file(&fake), "시그니처가 맞지 않는다");

    let folder = dir.join("폴더.zip");
    std::fs::create_dir(&folder).unwrap();
    assert!(!is_zip_file(&folder), "폴더");
    assert!(!is_zip_file(&dir.join("없음.zip")), "없는 파일");
}

#[test]
#[ignore = "#423"]
fn a_zip_that_cannot_be_read_is_a_list_error() {
    let f = fixture();
    let broken = f.dir.path().join("깨짐.zip");
    let mut bytes = b"PK\x03\x04".to_vec();
    bytes.extend(std::iter::repeat_n(0xA5u8, 4096));
    std::fs::write(&broken, &bytes).unwrap();

    let mut app = app_at(Location::file_system(f.dir.path()));
    assert!(app.navigate(Navigation::Go(zip_loc(&broken, ""))));
    assert_eq!(wait_read(&mut app), ReadOutcome::Failed);
    assert!(
        matches!(
            app.active_tab().error(),
            Some(ListError::BrokenArchive { location }) if location == &zip_loc(&broken, "")
        ),
        "까닭: {:?}",
        app.active_tab().error()
    );
    assert_eq!(
        app.active_tab().location(),
        &Location::file_system(f.dir.path()),
        "탭은 원래 위치에 남는다"
    );
}

#[test]
#[ignore = "#423"]
fn damage_and_duplicate_names_are_reported() {
    let dir = TempDir::new().unwrap();
    let mut bytes = zip_support::build(&[
        Entry::stored("같은.txt", b"first"),
        Entry::stored("같은.txt", b"second!"),
        Entry::stored("다른.txt", b"x"),
        Entry::stored("마지막.txt", b"y"),
    ]);
    let signature = 0x0201_4B50u32.to_le_bytes();
    let last = (0..bytes.len() - 4)
        .rev()
        .find(|&i| bytes[i..i + 4] == signature)
        .unwrap();
    bytes[last] = b'X';
    let zip = dir.path().join("상한.zip");
    std::fs::write(&zip, &bytes).unwrap();

    let app = app_at(zip_loc(&zip, ""));
    let names = tab_names(&app);
    assert!(names.contains(&"같은.txt".to_string()), "{names:?}");
    assert!(names.contains(&"다른.txt".to_string()), "{names:?}");
    let same = index_of(&app, "같은.txt");
    assert_eq!(
        app.active_tab().get(same).unwrap().size,
        7,
        "뒤에 나온 기록"
    );
    let notes = app.active_tab().archive_notes().expect("zip 위치의 상태");
    assert_eq!(notes.duplicates, 1);
    assert!(notes.damaged);

    let clean = app_at(Location::file_system(dir.path()));
    assert_eq!(clean.active_tab().archive_notes(), None, "실제 폴더");
}

/// zip 안에서 위치가 막는 명령.
const BLOCKED: [CommandId; 8] = [
    CommandId::Delete,
    CommandId::DeletePermanently,
    CommandId::Rename,
    CommandId::BatchRename,
    CommandId::NewFolder,
    CommandId::Cut,
    CommandId::MoveTo,
    CommandId::MoveToOtherPanel,
];

#[test]
#[ignore = "#423"]
fn writing_commands_are_off_inside_a_zip() {
    let f = fixture();
    let mut app = app_at(zip_loc(&f.zip, "문서"));
    app.split_panel();
    assert_eq!(wait_read(&mut app), ReadOutcome::Committed);
    let first = index_of(&app, "readme.txt");
    let second = index_of(&app, "메모.txt");
    app.active_tab_mut().select_only(first);
    app.active_tab_mut().toggle_select(second);
    let before = std::fs::read(&f.zip).unwrap();

    let palette = app.search_commands("", |command| command.as_str());
    for command in BLOCKED {
        let entry = palette
            .iter()
            .find(|entry| entry.command == command)
            .unwrap_or_else(|| panic!("팔레트에 {command:?}가 보인다"));
        assert!(!entry.enabled, "팔레트의 {command:?}는 꺼진 채 보인다");
        assert!(!app.can_execute(command), "{command:?}는 꺼진다");
        assert_eq!(
            app.execute(command),
            Execution::NotAllowedHere,
            "{command:?}"
        );
    }
    assert!(!app.can_execute(CommandId::Copy), "클립보드 복사는 꺼진다");
    assert_eq!(app.drag_sources(), None, "다른 앱으로 끌 원본이 아니다");
    assert!(app.can_execute(CommandId::Open), "열기는 된다");

    app.poll_jobs();
    assert!(app.work_area().entries().is_empty(), "작업을 넣지 않았다");
    assert_eq!(std::fs::read(&f.zip).unwrap(), before, "zip이 그대로다");
}

#[test]
#[ignore = "#423"]
fn a_zip_inside_a_zip_is_not_opened() {
    let f = fixture();
    let mut app = app_at(zip_loc(&f.zip, ""));
    assert_eq!(open_item(&mut app, "안쪽.zip"), Execution::NestedZip);
    assert_eq!(app.active_tab().location(), &zip_loc(&f.zip, ""));
    assert_eq!(
        app.active_tab().pending_location(),
        None,
        "읽기를 시작하지 않았다"
    );
}

#[test]
#[ignore = "#423"]
fn zip_locations_survive_the_session() {
    let f = fixture();
    let app = app_at(zip_loc(&f.zip, r"문서\2024"));
    let json = app.session().expect("세션").to_json();
    let session = Session::from_json(&json).expect("다시 읽는다");
    let mut restored = App::restore(Some(session), Vec::new());
    assert_eq!(
        restored.active_tab().location(),
        &zip_loc(&f.zip, r"문서\2024")
    );
    assert_eq!(wait_read(&mut restored), ReadOutcome::Committed);
    assert_eq!(tab_names(&restored), ["보고서.txt"]);
}
