//! 그 자리 이름바꾸기와 새 폴더: 명령과 단축키(F2·Ctrl+Shift+N), 실행 가능 여부, 실행 전에 막기, 임시 폴더의 실제
//! 파일로 이름바꾸기(대소문자만 바꾸기, 원본 사라짐, 260자 넘는 경로)와 새 폴더(「바탕 (2)」), 바꾼 뒤 선택·커서,
//! 결과에 담긴 경로·크기·수정 시각을 공개 인터페이스로 확인한다.
//!
//! 앱([`App`])은 일꾼 스레드에서 읽고 바꾸므로 결과를 기다린다. CI 러너가 느려도 흔들리지 않도록 기다리는 시간
//! ([`WAIT`])을 넉넉히 두고, 결과가 오는 즉시 끝낸다.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::command::{CommandId, Key, Shortcut, find_by_shortcut};
use facet_core::edit::{EditError, EditKind, Edited};
use facet_core::name_input::NameError;
use facet_core::selection::CursorMove;
use facet_core::tab::{ReadOutcome, Tab};
use facet_core::window::{App, Execution, Update};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(10);

/// `dir`을 연 앱. 첫 읽기가 끝날 때까지 기다린다.
fn app_at(dir: &Path) -> App {
    let mut app = App::new(Some(Location::file_system(dir)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

/// 이름바꾸기·새 폴더의 결과가 올 때까지 앱이 받은 것을 탭에 넘긴다.
fn wait_edited(app: &mut App) -> Result<Edited, EditError> {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(
            !left.is_zero(),
            "이름바꾸기·새 폴더 결과가 시간 안에 오지 않았다"
        );
        if let Some((_, Update::Edited(result))) = app.wait_update(left) {
            return result;
        }
    }
}

/// `duration` 동안 온 것(변경 감시의 묶음 등)을 모두 탭에 넘긴다.
fn pump(app: &mut App, duration: Duration) {
    let deadline = Instant::now() + duration;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        app.wait_update(left);
    }
}

/// 활성 탭에서 이름이 `name`인 항목을 커서·선택으로 둔다.
fn pick(app: &mut App, name: &str) {
    let tab = app.active_tab_mut();
    let index = index_of(tab, name);
    tab.select_only(index);
}

fn index_of(tab: &Tab, name: &str) -> usize {
    tab.items()
        .position(|item| item.name == name)
        .unwrap_or_else(|| panic!("{name}이 목록에 없다"))
}

fn names(tab: &Tab) -> Vec<String> {
    tab.items()
        .map(|item| item.name.to_string_lossy().into_owned())
        .collect()
}

/// 폴더 안 항목 이름(디스크 그대로의 대소문자), 이름 순.
fn disk_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn cursor_name(tab: &Tab) -> String {
    tab.cursor_item()
        .expect("커서 항목")
        .name
        .to_string_lossy()
        .into_owned()
}

#[test]
fn 이름_바꾸기는_f2_새_폴더는_ctrl_shift_n이고_다른_명령과_겹치지_않는다() {
    assert_eq!(
        CommandId::Rename.shortcuts(),
        &[Shortcut::new(Key::Function(2))]
    );
    assert_eq!(
        CommandId::NewFolder.shortcuts(),
        &[Shortcut::new(Key::Letter('N')).ctrl().shift()]
    );
    assert_eq!(
        find_by_shortcut(Shortcut::new(Key::Function(2))),
        Some(CommandId::Rename)
    );
    assert_eq!(
        find_by_shortcut(Shortcut::new(Key::Letter('n')).ctrl().shift()),
        Some(CommandId::NewFolder)
    );
    // 한글 모드에서도 같다: N 키는 「ㅜ」를 낸다.
    assert_eq!(
        find_by_shortcut(Shortcut::new(Key::from_char('ㅜ').unwrap()).ctrl().shift()),
        Some(CommandId::NewFolder)
    );
    // Ctrl+N은 그대로 새 창이다.
    assert_eq!(
        find_by_shortcut(Shortcut::new(Key::Letter('N')).ctrl()),
        Some(CommandId::NewWindow)
    );
    for command in [CommandId::Rename, CommandId::NewFolder] {
        assert!(CommandId::ALL.contains(&command));
        assert_eq!(CommandId::from_id(command.as_str()), Some(command));
        for shortcut in command.shortcuts() {
            let users: Vec<_> = CommandId::ALL
                .into_iter()
                .filter(|c| c.shortcuts().contains(shortcut))
                .collect();
            assert_eq!(users, vec![command], "{shortcut:?}");
        }
    }
    assert_eq!(CommandId::Rename.as_str(), "rename");
    assert_eq!(CommandId::NewFolder.as_str(), "new-folder");
}

#[test]
fn 이름_바꾸기는_커서_항목이_있어야_켜지고_새_폴더는_실제_폴더면_켜진다() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.txt"), b"a").unwrap();
    let mut app = app_at(dir.path());
    assert_eq!(app.active_tab().cursor(), None);
    assert!(!app.can_execute(CommandId::Rename));
    assert_eq!(app.execute(CommandId::Rename), Execution::Disabled);
    assert!(app.can_execute(CommandId::NewFolder));

    app.active_tab_mut().move_cursor(CursorMove::First);
    assert!(app.can_execute(CommandId::Rename));
    assert_eq!(app.execute(CommandId::Rename), Execution::Rename);
    // 새 폴더는 바탕 이름을 화면이 정하므로 실행하면 화면이 할 일을 돌려준다.
    assert_eq!(app.execute(CommandId::NewFolder), Execution::NewFolder);

    // 빈 폴더에서도 새 폴더는 켜지고 이름 바꾸기는 꺼진다.
    let empty = TempDir::new().unwrap();
    let app = app_at(empty.path());
    assert!(app.can_execute(CommandId::NewFolder));
    assert!(!app.can_execute(CommandId::Rename));
}

#[cfg(windows)]
#[test]
fn 이름을_바꾸면_디스크에서_바뀌고_결과에_원래_새_경로와_크기_수정_시각이_있다() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("보고서.txt"), b"hello").unwrap();
    let mut app = app_at(dir.path());
    pick(&mut app, "보고서.txt");

    app.rename("보고서.txt".as_ref(), "결과 🎉.txt").unwrap();
    let edited = wait_edited(&mut app).expect("이름바꾸기");

    assert_eq!(disk_names(dir.path()), ["결과 🎉.txt"]);
    assert_eq!(edited.kind, EditKind::Rename);
    assert_eq!(
        edited.original.as_deref(),
        Some(dir.path().join("보고서.txt").as_path())
    );
    assert_eq!(edited.path, dir.path().join("결과 🎉.txt"));
    assert!(!edited.is_folder);
    assert_eq!(edited.size, 5);
    let metadata = fs::metadata(dir.path().join("결과 🎉.txt")).unwrap();
    assert_eq!(edited.modified, metadata.modified().unwrap());
    // 목록에도 곧바로 들어갔다(변경 감시를 기다리지 않는다).
    assert_eq!(names(app.active_tab()), ["결과 🎉.txt"]);
}

#[cfg(windows)]
#[test]
fn 폴더_이름도_바꾼다() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("옛 폴더")).unwrap();
    fs::write(dir.path().join("옛 폴더").join("안.txt"), b"x").unwrap();
    let mut app = app_at(dir.path());
    pick(&mut app, "옛 폴더");
    app.rename("옛 폴더".as_ref(), "새 이름").unwrap();
    let edited = wait_edited(&mut app).expect("이름바꾸기");
    assert!(edited.is_folder);
    assert!(dir.path().join("새 이름").join("안.txt").is_file());
    assert!(!dir.path().join("옛 폴더").exists());
}

#[test]
fn 같은_폴더에_같은_이름이_있으면_막고_아무것도_바꾸지_않는다() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.txt"), b"a").unwrap();
    fs::write(dir.path().join("b.txt"), b"b").unwrap();
    let mut app = app_at(dir.path());
    pick(&mut app, "a.txt");

    // 대소문자만 다른 다른 항목도 같은 이름이다.
    assert_eq!(
        app.rename("a.txt".as_ref(), "b.txt"),
        Err(EditError::NameTaken)
    );
    assert_eq!(
        app.rename("a.txt".as_ref(), "B.TXT"),
        Err(EditError::NameTaken)
    );
    assert_eq!(disk_names(dir.path()), ["a.txt", "b.txt"]);
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"a");
    assert_eq!(fs::read(dir.path().join("b.txt")).unwrap(), b"b");
    assert_eq!(names(app.active_tab()), ["a.txt", "b.txt"]);
    assert_eq!(cursor_name(app.active_tab()), "a.txt");
}

#[test]
fn 숨김_항목과_같은_이름도_막는다() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.txt"), b"a").unwrap();
    let hidden = dir.path().join("숨김.txt");
    fs::write(&hidden, b"h").unwrap();
    #[cfg(windows)]
    {
        let status = std::process::Command::new("attrib")
            .arg("+h")
            .arg(&hidden)
            .status()
            .unwrap();
        assert!(status.success());
    }
    let mut app = app_at(dir.path());
    pick(&mut app, "a.txt");
    assert_eq!(
        app.rename("a.txt".as_ref(), "숨김.txt"),
        Err(EditError::NameTaken)
    );
    assert!(dir.path().join("a.txt").is_file());
}

#[test]
fn 쓸_수_없는_이름_똑같은_이름_없는_항목은_실행_전에_막는다() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.txt"), b"a").unwrap();
    let mut app = app_at(dir.path());
    pick(&mut app, "a.txt");
    assert_eq!(
        app.rename("a.txt".as_ref(), "a?.txt"),
        Err(EditError::InvalidName(NameError::ForbiddenChar('?')))
    );
    assert_eq!(
        app.rename("a.txt".as_ref(), "CON.txt"),
        Err(EditError::InvalidName(NameError::Reserved))
    );
    assert_eq!(
        app.rename("a.txt".as_ref(), ""),
        Err(EditError::InvalidName(NameError::Empty))
    );
    assert_eq!(
        app.rename("a.txt".as_ref(), "a.txt"),
        Err(EditError::Unchanged)
    );
    assert_eq!(
        app.rename("없음.txt".as_ref(), "b.txt"),
        Err(EditError::Unavailable)
    );
    assert_eq!(
        app.new_folder("새/폴더"),
        Err(EditError::InvalidName(NameError::ForbiddenChar('/')))
    );
    assert_eq!(disk_names(dir.path()), ["a.txt"]);
}

#[cfg(windows)]
#[test]
fn 대소문자만_바꾼_이름은_디스크에서도_대소문자가_바뀐다() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("readme.txt"), b"read me").unwrap();
    let mut app = app_at(dir.path());
    pick(&mut app, "readme.txt");

    app.rename("readme.txt".as_ref(), "README.txt").unwrap();
    let edited = wait_edited(&mut app).expect("대소문자 바꾸기");

    assert_eq!(disk_names(dir.path()), ["README.txt"]);
    assert_eq!(fs::read(dir.path().join("README.txt")).unwrap(), b"read me");
    assert_eq!(edited.path, dir.path().join("README.txt"));
    assert_eq!(names(app.active_tab()), ["README.txt"]);
    assert_eq!(cursor_name(app.active_tab()), "README.txt");
}

#[cfg(windows)]
#[test]
fn 그_사이_원본이_사라지면_원본_없음이_까닭이다() {
    use facet_win::file_ops::FileOpErrorKind;

    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("사라질.txt"), b"x").unwrap();
    let mut app = app_at(dir.path());
    pick(&mut app, "사라질.txt");
    // 목록에는 아직 있지만 디스크에서는 지웠다(변경 감시가 알리기 전).
    fs::remove_file(dir.path().join("사라질.txt")).unwrap();

    app.rename("사라질.txt".as_ref(), "새 이름.txt").unwrap();
    assert_eq!(
        wait_edited(&mut app),
        Err(EditError::File(FileOpErrorKind::SourceMissing))
    );
    assert!(disk_names(dir.path()).is_empty());
}

#[cfg(windows)]
#[test]
fn 그_사이_같은_이름이_생기면_같은_이름이_까닭이고_덮어쓰지_않는다() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.txt"), b"a").unwrap();
    let mut app = app_at(dir.path());
    pick(&mut app, "a.txt");
    // 목록에는 아직 없지만 디스크에는 생겼다.
    fs::write(dir.path().join("b.txt"), b"other").unwrap();

    app.rename("a.txt".as_ref(), "b.txt").unwrap();
    assert_eq!(wait_edited(&mut app), Err(EditError::NameTaken));
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"a");
    assert_eq!(fs::read(dir.path().join("b.txt")).unwrap(), b"other");
}

#[cfg(windows)]
#[test]
fn 경로가_260자를_넘어도_이름을_바꾼다() {
    let dir = TempDir::new().unwrap();
    let mut deep = dir.path().to_path_buf();
    while deep.as_os_str().encode_wide_len() < 300 {
        deep.push("a-rather-long-folder-name-for-long-paths-0123456789");
    }
    fs::create_dir_all(&deep).unwrap();
    let file = deep.join("long-path-file-before.txt");
    fs::write(&file, b"long").unwrap();
    assert!(file.as_os_str().encode_wide_len() > 260);

    let mut app = app_at(&deep);
    pick(&mut app, "long-path-file-before.txt");
    app.rename(
        "long-path-file-before.txt".as_ref(),
        "long-path-file-after.txt",
    )
    .unwrap();
    let edited = wait_edited(&mut app).expect("긴 경로 이름바꾸기");
    assert_eq!(edited.path, deep.join("long-path-file-after.txt"));
    assert_eq!(edited.size, 4);
    assert_eq!(disk_names(&deep), ["long-path-file-after.txt"]);
    assert_eq!(cursor_name(app.active_tab()), "long-path-file-after.txt");
}

/// UTF-16 단위 길이(Windows 경로 길이 한도의 단위).
trait WideLen {
    fn encode_wide_len(&self) -> usize;
}

impl WideLen for std::ffi::OsStr {
    fn encode_wide_len(&self) -> usize {
        self.to_string_lossy().encode_utf16().count()
    }
}

#[cfg(windows)]
#[test]
fn 바꾼_뒤_다시_정렬된_목록에서_새_이름_항목이_선택_커서다() {
    let dir = TempDir::new().unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        fs::write(dir.path().join(name), name).unwrap();
    }
    let mut app = app_at(dir.path());
    pick(&mut app, "a.txt");
    assert_eq!(app.active_tab().cursor(), Some(0));

    app.rename("a.txt".as_ref(), "z.txt").unwrap();
    wait_edited(&mut app).expect("이름바꾸기");

    let check = |app: &App| {
        let tab = app.active_tab();
        assert_eq!(names(tab), ["b.txt", "c.txt", "z.txt"]);
        assert_eq!(tab.cursor(), Some(2));
        assert_eq!(cursor_name(tab), "z.txt");
        assert!(tab.is_selected(2));
        assert_eq!(tab.selected_count(), 1);
        let selected: Vec<_> = tab.selected_items().map(|i| i.name.clone()).collect();
        assert_eq!(selected, ["z.txt"]);
    };
    check(&app);
    // 뒤에 변경 감시가 같은 변경을 알려도 그대로다.
    pump(&mut app, Duration::from_millis(500));
    check(&app);
}

#[cfg(windows)]
#[test]
fn 새_폴더를_두_번_만들면_바탕과_바탕_2가_생기고_만든_폴더가_선택_커서다() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("파일.txt"), b"x").unwrap();
    let mut app = app_at(dir.path());

    app.new_folder("바탕").unwrap();
    let first = wait_edited(&mut app).expect("첫 새 폴더");
    assert_eq!(first.kind, EditKind::NewFolder);
    assert_eq!(first.original, None);
    assert_eq!(first.path, dir.path().join("바탕"));
    assert!(first.is_folder);
    let metadata = fs::metadata(&first.path).unwrap();
    assert_eq!(first.modified, metadata.modified().unwrap());
    assert_eq!(cursor_name(app.active_tab()), "바탕");

    app.new_folder("바탕").unwrap();
    let second = wait_edited(&mut app).expect("두 번째 새 폴더");
    assert_eq!(second.path, dir.path().join("바탕 (2)"));
    assert!(dir.path().join("바탕").is_dir());
    assert!(dir.path().join("바탕 (2)").is_dir());

    let check = |app: &App| {
        let tab = app.active_tab();
        assert_eq!(cursor_name(tab), "바탕 (2)");
        assert_eq!(tab.selected_count(), 1);
        assert!(tab.is_selected(tab.cursor().unwrap()));
        assert!(names(tab).contains(&"바탕".to_string()));
    };
    check(&app);
    pump(&mut app, Duration::from_millis(500));
    check(&app);
}

#[cfg(windows)]
#[test]
fn 같은_이름의_파일이_있어도_새_폴더는_다음_번호로_만든다() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("새 폴더"), b"file").unwrap();
    fs::create_dir(dir.path().join("새 폴더 (2)")).unwrap();
    let mut app = app_at(dir.path());
    app.new_folder("새 폴더").unwrap();
    let edited = wait_edited(&mut app).expect("새 폴더");
    assert_eq!(edited.path, dir.path().join("새 폴더 (3)"));
    assert!(edited.path.is_dir());
}
