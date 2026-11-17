// target: crates/facet-core/tests/spec_batch_rename_entry_command.rs
// AC: #290: 기본 단축키 Ctrl+M 하나, Ctrl+M(소문자·「ㅡ」)으로 찾으면 BatchRename이고 겹치지 않음, Ctrl+M 실행의 켜짐, 팔레트 단축키
//! 일괄 이름 바꾸기 명령(`CommandId::BatchRename`)의 기본 단축키 Ctrl+M을 확인한다: 단축키 목록, 키로 찾기(두벌식
//! 「ㅡ」 포함)와 다른 명령과 겹치지 않음, 임시 폴더의 앱에서 Ctrl+M으로 찾은 명령의 실행 결과, 팔레트 줄의 단축키.
//! 파일을 바꾸지 않으므로 모든 환경에서 돈다.

use std::fs;
use std::path::Path;
use std::time::Duration;

use facet_core::Location;
use facet_core::command::{CommandId, Key, Shortcut, find_by_shortcut};
use facet_core::tab::ReadOutcome;
use facet_core::window::{App, Execution};
use tempfile::TempDir;

/// CI 러너가 느려도 흔들리지 않게 넉넉히 기다린다. 결과가 오면 곧바로 끝난다.
const WAIT: Duration = Duration::from_secs(20);

const NAMES: [&str; 3] = ["a.txt", "b.txt", "c.txt"];

const CTRL_M: Shortcut = Shortcut::new(Key::Letter('M')).ctrl();

fn folder() -> TempDir {
    let dir = TempDir::new().unwrap();
    for name in NAMES {
        fs::write(dir.path().join(name), name).unwrap();
    }
    dir
}

fn app_at(path: &Path) -> App {
    let mut app = App::new(Some(Location::file_system(path)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

fn names_on_disk(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn batch_rename_has_ctrl_m_as_its_only_default_shortcut() {
    assert_eq!(CommandId::BatchRename.shortcuts(), &[CTRL_M]);
}

#[test]
fn ctrl_m_finds_batch_rename_by_key_and_no_other_command_uses_it() {
    assert_eq!(find_by_shortcut(CTRL_M), Some(CommandId::BatchRename));

    // 소문자로 만든 키도, 한글 모드에서 M 키가 내는 「ㅡ」도 같은 키다.
    let lower = Shortcut::new(Key::Letter('m')).ctrl();
    assert_eq!(find_by_shortcut(lower), Some(CommandId::BatchRename));
    let hangul = Key::from_char('ㅡ').expect("두벌식 ㅡ는 M 키");
    assert_eq!(
        find_by_shortcut(Shortcut::new(hangul).ctrl()),
        Some(CommandId::BatchRename)
    );

    let users: Vec<CommandId> = CommandId::ALL
        .into_iter()
        .filter(|command| command.shortcuts().contains(&CTRL_M))
        .collect();
    assert_eq!(users, [CommandId::BatchRename], "Ctrl+M은 한 명령만 쓴다");

    // 수식 키가 다르면 일괄 이름 바꾸기가 아니다.
    for other in [
        Shortcut::new(Key::Letter('M')),
        Shortcut::new(Key::Letter('M')).ctrl().shift(),
        Shortcut::new(Key::Letter('M')).alt(),
    ] {
        assert_ne!(
            find_by_shortcut(other),
            Some(CommandId::BatchRename),
            "{other:?}"
        );
    }
}

#[test]
fn ctrl_m_opens_the_window_only_with_two_or_more_selected() {
    let dir = folder();
    let mut app = app_at(dir.path());
    let command = find_by_shortcut(CTRL_M).expect("Ctrl+M에 명령이 있다");

    app.active_tab_mut().select_only(0);
    assert_eq!(app.execute(command), Execution::Disabled, "하나만 골랐다");

    app.active_tab_mut().toggle_select(2);
    assert_eq!(app.execute(command), Execution::BatchRename);
    assert_eq!(
        names_on_disk(dir.path()),
        NAMES,
        "창을 맡기기만 하고 바꾸지 않는다"
    );
}

#[test]
fn palette_row_shows_ctrl_m() {
    let dir = folder();
    let app = app_at(dir.path());
    let name = |c: CommandId| {
        if c == CommandId::BatchRename {
            "일괄 이름 바꾸기"
        } else {
            c.as_str()
        }
    };
    let entries = app.search_commands("일괄", name);
    let entry = entries
        .iter()
        .find(|e| e.command == CommandId::BatchRename)
        .expect("팔레트에서 찾아진다");
    assert_eq!(entry.shortcuts, &[CTRL_M]);
}
