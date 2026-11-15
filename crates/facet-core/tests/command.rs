//! 명령 목록, 기본 단축키, 실행 가능 여부, 명령 팔레트 검색, 명령 실행을 공개 인터페이스로만 확인한다.

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::time::Duration;

use facet_core::Location;
use facet_core::command::{self, CommandId, Key, Shortcut, find_by_shortcut};
use facet_core::details::{Column, Direction, Sort};
use facet_core::tab::{Navigation, ReadOutcome, Tab};
use facet_core::window::{App, Execution};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(10);

fn key(k: Key) -> Shortcut {
    Shortcut::new(k)
}

fn letter(c: char) -> Key {
    Key::from_char(c).unwrap()
}

/// 테스트가 앱 대신 주는 명령 이름. 실제 화면 문장은 앱이 만든다.
fn name(command: CommandId) -> &'static str {
    match command {
        CommandId::Open => "열기",
        CommandId::Back => "뒤로",
        CommandId::Forward => "앞으로",
        CommandId::Up => "위로",
        CommandId::Refresh => "새로 고침",
        CommandId::SelectAll => "모두 선택",
        CommandId::ToggleCursorSelection => "커서 항목 선택 토글",
        CommandId::EditAddress => "주소창 고쳐 쓰기",
        CommandId::ToggleHidden => "숨김 항목 보이기/감추기",
        CommandId::CommandPalette => "명령 팔레트",
        CommandId::SortByName => "이름으로 정렬",
        CommandId::SortByModified => "수정한 날짜로 정렬",
        CommandId::SortByType => "유형으로 정렬",
        CommandId::SortBySize => "크기로 정렬",
        CommandId::NewTab => "새 탭",
        CommandId::CloseTab => "탭 닫기",
        CommandId::NextTab => "다음 탭",
        CommandId::PreviousTab => "이전 탭",
        CommandId::GoToTab(n) => [
            "",
            "1번째 탭",
            "2번째 탭",
            "3번째 탭",
            "4번째 탭",
            "5번째 탭",
            "6번째 탭",
            "7번째 탭",
            "8번째 탭",
        ][usize::from(n).min(8)],
        CommandId::LastTab => "마지막 탭",
        CommandId::OpenInNewTab => "새 탭에서 열기",
        CommandId::ReopenClosedTab => "닫은 탭 되살리기",
        CommandId::TogglePanelSplit => "패널 나누기/합치기",
        CommandId::SwitchPanel => "활성 패널 바꾸기",
    }
}

/// `root/a/a1`, `root/b`, `root/파일.txt`가 있는 임시 폴더.
fn tree() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("a").join("a1")).unwrap();
    fs::create_dir(dir.path().join("b")).unwrap();
    fs::write(dir.path().join("파일.txt"), b"x").unwrap();
    dir
}

fn go(tab: &mut Tab, navigation: Navigation) {
    let request = tab.navigate(navigation).expect("이동할 수 있어야 한다");
    assert_eq!(tab.finish(request.run()), ReadOutcome::Committed);
}

fn tab_at(path: &Path) -> Tab {
    let mut tab = Tab::new(Location::file_system(path), Sort::default(), false);
    go(&mut tab, Navigation::Refresh);
    tab
}

/// 앱을 띄우고 첫 읽기를 기다린다.
fn app_at(path: &Path) -> App {
    let mut app = App::new(Some(Location::file_system(path)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

#[test]
fn s1_명령과_기본_단축키가_모두_명령_목록에_있다() {
    use CommandId::*;
    let expected: [(CommandId, Vec<Shortcut>); 14] = [
        (Open, vec![key(Key::Enter)]),
        (Back, vec![key(Key::Backspace), key(Key::Left).alt()]),
        (Forward, vec![key(Key::Right).alt()]),
        (Up, vec![key(Key::Up).alt()]),
        (Refresh, vec![key(Key::Function(5))]),
        (SelectAll, vec![key(letter('a')).ctrl()]),
        (ToggleCursorSelection, vec![key(Key::Space).ctrl()]),
        (
            EditAddress,
            vec![
                key(letter('d')).alt(),
                key(letter('l')).ctrl(),
                key(Key::Function(4)),
            ],
        ),
        (ToggleHidden, vec![key(letter('h')).ctrl()]),
        (CommandPalette, vec![key(letter('p')).ctrl().shift()]),
        (SortByName, vec![]),
        (SortByModified, vec![]),
        (SortByType, vec![]),
        (SortBySize, vec![]),
    ];
    for (command, shortcuts) in expected {
        assert!(CommandId::ALL.contains(&command), "{command:?}");
        assert_eq!(command.shortcuts(), shortcuts.as_slice(), "{command:?}");
    }
    // 열마다 정렬 명령이 있다.
    for column in [Column::Name, Column::Modified, Column::Type, Column::Size] {
        assert_eq!(CommandId::sort_by(column).sort_column(), Some(column));
    }
}

#[test]
fn s2_탭_명령과_기본_단축키() {
    use CommandId::*;
    let mut expected: Vec<(CommandId, Vec<Shortcut>)> = vec![
        (NewTab, vec![key(letter('t')).ctrl()]),
        (CloseTab, vec![key(letter('w')).ctrl()]),
        (NextTab, vec![key(Key::Tab).ctrl()]),
        (PreviousTab, vec![key(Key::Tab).ctrl().shift()]),
        (LastTab, vec![key(Key::Digit(9)).ctrl()]),
        (OpenInNewTab, vec![]),
        (ReopenClosedTab, vec![key(letter('t')).ctrl().shift()]),
    ];
    for n in 1..=8 {
        expected.push((GoToTab(n), vec![key(Key::Digit(n)).ctrl()]));
    }
    // 패널 명령: 나누기/합치기 Ctrl+백슬래시, 활성 패널 바꾸기 Tab.
    expected.push((TogglePanelSplit, vec![key(Key::Backslash).ctrl()]));
    expected.push((SwitchPanel, vec![key(Key::Tab)]));
    for (command, shortcuts) in &expected {
        assert!(CommandId::ALL.contains(command), "{command:?}");
        assert_eq!(command.shortcuts(), shortcuts.as_slice(), "{command:?}");
    }
    // S1의 14개와 탭·패널 명령이 전부다.
    assert_eq!(CommandId::ALL.len(), 14 + expected.len());
    assert_eq!(GoToTab(3).as_str(), "go-to-tab-3");
    assert_eq!(CommandId::from_id("go-to-tab-8"), Some(GoToTab(8)));
    assert_eq!(CommandId::from_id("go-to-tab-9"), None);

    // 한글 모드에서도 같다: T 키는 「ㅅ」, W 키는 「ㅈ」을 낸다. 숫자와 Tab은 모드와 상관없다.
    assert_eq!(
        find_by_shortcut(key(Key::from_char('ㅅ').unwrap()).ctrl()),
        Some(NewTab)
    );
    assert_eq!(
        find_by_shortcut(key(Key::from_char('ㅈ').unwrap()).ctrl()),
        Some(CloseTab)
    );
    assert_eq!(find_by_shortcut(key(Key::Tab).ctrl()), Some(NextTab));
    assert_eq!(
        find_by_shortcut(key(Key::Tab).ctrl().shift()),
        Some(PreviousTab)
    );
    for n in 1..=8 {
        assert_eq!(
            find_by_shortcut(key(Key::from_char(char::from(b'0' + n)).unwrap()).ctrl()),
            Some(GoToTab(n))
        );
    }
    assert_eq!(find_by_shortcut(key(Key::Digit(9)).ctrl()), Some(LastTab));
    // 수식 키 없는 Tab·숫자는 명령이 아니다(목록의 글자로 이동 등에 남긴다).
    assert_eq!(find_by_shortcut(key(Key::Tab)), None);
    assert_eq!(find_by_shortcut(key(Key::Digit(1))), None);
    assert_eq!(find_by_shortcut(key(Key::Digit(0)).ctrl()), None);
}

#[test]
fn 명령_id는_겹치지_않고_글자로_되찾는다() {
    let mut ids: Vec<_> = CommandId::ALL.iter().map(|c| c.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), CommandId::ALL.len());
    for command in CommandId::ALL {
        assert_eq!(CommandId::from_id(command.as_str()), Some(command));
    }
    assert_eq!(CommandId::from_id("없는 명령"), None);
    // 바뀌지 않는 ID다.
    assert_eq!(CommandId::CommandPalette.as_str(), "command-palette");
}

#[test]
fn 단축키는_두_명령이_함께_쓰지_않는다() {
    let mut all: Vec<Shortcut> = CommandId::ALL
        .iter()
        .flat_map(|c| c.shortcuts().iter().copied())
        .collect();
    let count = all.len();
    all.sort_by_key(|s| format!("{s:?}"));
    all.dedup();
    assert_eq!(all.len(), count);
}

#[test]
fn 키_조합으로_명령을_찾는다() {
    let cases = [
        (key(Key::Enter), CommandId::Open),
        (key(Key::Backspace), CommandId::Back),
        (key(Key::Left).alt(), CommandId::Back),
        (key(Key::Right).alt(), CommandId::Forward),
        (key(Key::Up).alt(), CommandId::Up),
        (key(Key::Function(5)), CommandId::Refresh),
        (key(letter('A')).ctrl(), CommandId::SelectAll),
        (key(Key::Space).ctrl(), CommandId::ToggleCursorSelection),
        (key(letter('D')).alt(), CommandId::EditAddress),
        (key(letter('L')).ctrl(), CommandId::EditAddress),
        (key(Key::Function(4)), CommandId::EditAddress),
        (key(letter('H')).ctrl(), CommandId::ToggleHidden),
        (key(letter('P')).ctrl().shift(), CommandId::CommandPalette),
    ];
    for (shortcut, command) in cases {
        assert_eq!(find_by_shortcut(shortcut), Some(command), "{shortcut:?}");
    }
    // 수식 키가 다르면 다른 조합이다.
    assert_eq!(find_by_shortcut(key(letter('P')).ctrl()), None);
    assert_eq!(find_by_shortcut(key(Key::Left)), None);
    assert_eq!(find_by_shortcut(key(letter('H'))), None);
    assert_eq!(find_by_shortcut(key(Key::Up)), None);
    // 소문자로 만든 영문 키도 같은 키다.
    assert_eq!(
        find_by_shortcut(key(Key::Letter('h')).ctrl()),
        Some(CommandId::ToggleHidden)
    );
}

#[test]
fn 한글_ime가_한글_모드여도_ctrl_조합은_영문_모드와_같다() {
    // 키 이벤트가 낸 글자: 영문 모드, Caps Lock, 한글 모드(두벌식).
    let cases = [
        (['h', 'H', 'ㅗ'], false, CommandId::ToggleHidden),
        (['l', 'L', 'ㅣ'], false, CommandId::EditAddress),
        (['a', 'A', 'ㅁ'], false, CommandId::SelectAll),
        // Shift를 누르면 P 키는 한글 모드에서 「ㅖ」를 낸다.
        (['P', 'p', 'ㅖ'], true, CommandId::CommandPalette),
    ];
    for (chars, shift, command) in cases {
        for c in chars {
            let mut shortcut = key(Key::from_char(c).unwrap()).ctrl();
            if shift {
                shortcut = shortcut.shift();
            }
            assert_eq!(find_by_shortcut(shortcut), Some(command), "{c} {shift}");
        }
    }
    // Alt+D도 한글 모드의 「ㅇ」에서 같다.
    assert_eq!(
        find_by_shortcut(key(Key::from_char('ㅇ').unwrap()).alt()),
        Some(CommandId::EditAddress)
    );
    // 두벌식 자모는 모두 영문 키 하나로 바뀐다.
    for (jamo, letter) in [
        ('ㅂ', 'Q'),
        ('ㅃ', 'Q'),
        ('ㅡ', 'M'),
        ('ㅒ', 'O'),
        ('ㄲ', 'R'),
    ] {
        assert_eq!(Key::from_char(jamo), Some(Key::Letter(letter)));
    }
    // 조합된 음절이나 다른 글자는 키가 아니다.
    assert_eq!(Key::from_char('가'), None);
    assert_eq!(Key::from_char('!'), None);
    assert_eq!(Key::from_char(' '), Some(Key::Space));
    assert_eq!(Key::from_char('7'), Some(Key::Digit(7)));
}

#[test]
fn 뒤로_앞으로_위로의_실행_가능_여부는_탭_상태를_따른다() {
    let dir = tree();
    let mut tab = tab_at(dir.path());
    assert!(!CommandId::Back.is_enabled(&tab));
    assert!(!CommandId::Forward.is_enabled(&tab));
    assert!(CommandId::Up.is_enabled(&tab));

    go(&mut tab, Navigation::Open(OsString::from("a")));
    assert!(CommandId::Back.is_enabled(&tab));
    assert!(!CommandId::Forward.is_enabled(&tab));

    go(&mut tab, Navigation::Back);
    assert!(!CommandId::Back.is_enabled(&tab));
    assert!(CommandId::Forward.is_enabled(&tab));

    // 드라이브 루트에서 위로는 꺼진다.
    let root = dir.path().ancestors().last().unwrap();
    let tab = Tab::new(Location::file_system(root), Sort::default(), false);
    assert!(!CommandId::Up.is_enabled(&tab));
    assert!(CommandId::Refresh.is_enabled(&tab));
}

#[test]
fn 커서와_항목에_따른_실행_가능_여부() {
    let dir = tree();
    let mut tab = tab_at(dir.path());
    assert!(tab.cursor().is_none());
    assert!(!CommandId::Open.is_enabled(&tab));
    assert!(!CommandId::ToggleCursorSelection.is_enabled(&tab));
    assert!(CommandId::SelectAll.is_enabled(&tab));
    tab.select_only(0);
    assert!(CommandId::Open.is_enabled(&tab));
    assert!(CommandId::ToggleCursorSelection.is_enabled(&tab));

    let empty = TempDir::new().unwrap();
    let tab = tab_at(empty.path());
    assert!(!CommandId::SelectAll.is_enabled(&tab));
    for always in [
        CommandId::Refresh,
        CommandId::EditAddress,
        CommandId::ToggleHidden,
        CommandId::CommandPalette,
        CommandId::SortBySize,
    ] {
        assert!(always.is_enabled(&tab), "{always:?}");
    }
}

#[test]
fn 명령_팔레트는_앱이_준_이름에서_부분_일치로_찾고_실행_가능_여부를_준다() {
    let dir = tree();
    let tab = tab_at(dir.path());

    let found = command::search("정렬", name, &tab);
    let commands: Vec<_> = found.iter().map(|e| e.command).collect();
    assert_eq!(
        commands,
        [
            CommandId::SortByName,
            CommandId::SortByModified,
            CommandId::SortByType,
            CommandId::SortBySize
        ]
    );
    assert!(found.iter().all(|e| e.enabled && e.shortcuts.is_empty()));

    // 단축키와 실행 가능 여부를 함께 준다. 꺼진 명령도 결과에 든다.
    let found = command::search("뒤로", name, &tab);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].command, CommandId::Back);
    assert_eq!(found[0].shortcuts, CommandId::Back.shortcuts());
    assert!(!found[0].enabled);

    // 이름 가운데와도 맞는다.
    let found = command::search("선택", name, &tab);
    let commands: Vec<_> = found.iter().map(|e| e.command).collect();
    assert_eq!(
        commands,
        [CommandId::SelectAll, CommandId::ToggleCursorSelection]
    );

    // 이름이 검색어로 시작하는 것이 먼저다.
    let found = command::search("이름", name, &tab);
    assert_eq!(found[0].command, CommandId::SortByName);
    let renamed = |c| match c {
        CommandId::ToggleCursorSelection => "선택 토글",
        c => name(c),
    };
    let found = command::search("선택", renamed, &tab);
    let commands: Vec<_> = found.iter().map(|e| e.command).collect();
    assert_eq!(
        commands,
        [CommandId::ToggleCursorSelection, CommandId::SelectAll]
    );

    // 대소문자를 가리지 않고, 비면 모두다.
    let english = |c: CommandId| c.as_str().to_uppercase();
    let found = command::search("palette", english, &tab);
    assert_eq!(found[0].command, CommandId::CommandPalette);
    assert!(found[0].enabled);
    assert_eq!(
        command::search("  ", name, &tab).len(),
        CommandId::ALL.len()
    );
    assert!(command::search("없는 이름", name, &tab).is_empty());
}

#[test]
fn 폴더에_열기를_실행하면_그_안으로_간다() {
    let dir = tree();
    let mut app = app_at(dir.path());
    // 커서가 없으면 열기는 꺼져 있다.
    assert_eq!(app.execute(CommandId::Open), Execution::Disabled);

    let index = app
        .active_tab()
        .items()
        .position(|i| i.name == "a")
        .unwrap();
    app.active_tab_mut().select_only(index);
    assert!(app.can_execute(CommandId::Open));
    assert_eq!(app.execute(CommandId::Open), Execution::Done);
    let (_, outcome) = app.wait(WAIT).expect("읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    assert_eq!(
        app.active_tab().location(),
        &Location::file_system(dir.path().join("a"))
    );

    // 뒤로·앞으로·위로도 명령으로 실행한다.
    assert_eq!(app.execute(CommandId::Forward), Execution::Disabled);
    assert_eq!(app.execute(CommandId::Back), Execution::Done);
    app.wait(WAIT).expect("읽기");
    assert_eq!(
        app.active_tab().location(),
        &Location::file_system(dir.path())
    );
    assert_eq!(app.execute(CommandId::Forward), Execution::Done);
    app.wait(WAIT).expect("읽기");
    assert_eq!(app.execute(CommandId::Up), Execution::Done);
    app.wait(WAIT).expect("읽기");
    assert_eq!(
        app.active_tab().location(),
        &Location::file_system(dir.path())
    );
    // 위로 가면 방금 있던 폴더가 커서 항목이다.
    assert_eq!(app.active_tab().cursor_item().unwrap().name, "a");
}

#[test]
fn 선택_숨김_정렬_화면_명령을_실행한다() {
    let dir = tree();
    let mut app = app_at(dir.path());

    assert_eq!(app.execute(CommandId::SelectAll), Execution::Done);
    assert_eq!(app.active_tab().selected_count(), 3);
    app.active_tab_mut().select_only(0);
    assert_eq!(
        app.execute(CommandId::ToggleCursorSelection),
        Execution::Done
    );
    assert_eq!(app.active_tab().selected_count(), 0);

    assert!(!app.show_hidden());
    app.execute(CommandId::ToggleHidden);
    assert!(app.show_hidden());
    assert!(app.active_tab().show_hidden());
    app.execute(CommandId::ToggleHidden);
    assert!(!app.show_hidden());

    // 열마다 정렬: 다른 열이면 오름차순, 같은 열이면 방향을 뒤집는다.
    app.execute(CommandId::SortBySize);
    assert_eq!(
        app.active_tab().sort(),
        Sort {
            column: Column::Size,
            direction: Direction::Ascending
        }
    );
    app.execute(CommandId::SortBySize);
    assert_eq!(app.active_tab().sort().direction, Direction::Descending);
    app.execute(CommandId::SortByName);
    assert_eq!(app.active_tab().sort(), Sort::default());

    assert_eq!(app.execute(CommandId::EditAddress), Execution::EditAddress);
    assert_eq!(
        app.execute(CommandId::CommandPalette),
        Execution::ShowCommandPalette
    );
    assert_eq!(app.execute(CommandId::Refresh), Execution::Done);
    assert_eq!(app.wait(WAIT).unwrap().1, ReadOutcome::Committed);

    // 앱의 팔레트 검색은 활성 탭의 상태를 따른다.
    let found = app.search_commands("위로", name);
    assert!(found[0].enabled);
    let found = app.search_commands("앞으로", name);
    assert!(!found[0].enabled);
}

/// 연결된 프로그램이 없는 파일을 열면 탭이 그 까닭을 갖는다. 실제 프로그램이 뜨는 경로는 자동 테스트하지 않는다.
#[cfg(windows)]
#[test]
fn 연결된_프로그램이_없는_파일을_열면_탭이_까닭을_갖는다() {
    use facet_core::OpenError;
    use facet_core::window::Update;

    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("문서.facetnoassoc7f3a"), b"facet").unwrap();
    let mut app = app_at(dir.path());
    app.active_tab_mut().select_only(0);
    assert_eq!(app.execute(CommandId::Open), Execution::Done);
    let opened = loop {
        match app.wait_update(Duration::from_secs(60)).expect("열기 결과") {
            (_, Update::Opened(opened)) => break opened,
            _ => continue,
        }
    };
    assert!(!opened);
    assert_eq!(
        app.active_tab().open_error(),
        Some(&OpenError::NoAssociation)
    );
    // 다음 이동을 시작하면 지운다.
    app.execute(CommandId::Refresh);
    assert_eq!(app.active_tab().open_error(), None);
}
