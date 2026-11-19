// target: crates/facet-core/tests/spec_settings_shortcuts.rs
// AC: #460: 탐색기 프리셋 = 지금 기본 단축키, Total Commander 표, 두 프리셋 충돌 없음, 사용자 변경 덧씌우기, 프리셋 바꿔도 변경 유지, 단축키 빼기, 모두 프리셋 기본으로, 문맥별 충돌과 마지막에 정한 명령, 입력란 문맥, 키 글자 왕복, 설정 파일의 프리셋·변경·틀린 키 글자·뒤 줄이 이김
//! 단축키 모델(`facet_core::keymap`)과 설정 파일의 단축키(`Settings::keymap`·`set_shortcuts` 등)를 확인한다. 프리셋·
//! 사용자 변경·문맥·충돌은 순수 값으로, 파일 왕복은 임시 폴더의 실제 `settings.toml`로 본다. 모든 환경에서 돈다.

use std::fs;

use facet_core::command::{CommandId, Key, Shortcut};
use facet_core::keymap::{self, Context, Keymap, Preset};
use facet_core::settings::{FILE_NAME, Saved, Settings, Value, ValueError};
use tempfile::TempDir;

fn key(k: Key) -> Shortcut {
    Shortcut::new(k)
}

fn f(n: u8) -> Shortcut {
    key(Key::Function(n))
}

fn letter(c: char) -> Shortcut {
    key(Key::Letter(c))
}

#[test]
#[ignore = "#460"]
fn the_explorer_preset_is_todays_default_shortcuts() {
    let keymap = Keymap::new(Preset::Explorer);
    assert_eq!(keymap.preset(), Preset::Explorer);
    for command in CommandId::ALL {
        assert_eq!(
            keymap::preset_shortcuts(Preset::Explorer, command),
            command.shortcuts().to_vec(),
            "{command:?}"
        );
        assert_eq!(
            keymap.shortcuts(command),
            command.shortcuts().to_vec(),
            "{command:?}"
        );
        assert!(!keymap.is_overridden(command));
    }
    assert_eq!(
        keymap.resolve(f(5), Context::List),
        Some(CommandId::Refresh)
    );
    assert_eq!(
        keymap.resolve(f(7), Context::List),
        Some(CommandId::CopyToOtherPanel)
    );
}

#[test]
#[ignore = "#460"]
fn the_total_commander_preset_follows_the_ticket_table() {
    let tc = |command| keymap::preset_shortcuts(Preset::TotalCommander, command);
    assert_eq!(tc(CommandId::Open), vec![f(4), key(Key::Enter)]);
    assert_eq!(tc(CommandId::CopyToOtherPanel), vec![f(5)]);
    assert_eq!(tc(CommandId::MoveToOtherPanel), vec![f(6)]);
    assert_eq!(tc(CommandId::Rename), vec![f(6).shift(), f(2)]);
    assert_eq!(
        tc(CommandId::NewFolder),
        vec![f(7), letter('N').ctrl().shift()]
    );
    assert_eq!(tc(CommandId::Delete), vec![f(8), key(Key::Delete)]);
    assert_eq!(tc(CommandId::SwitchPanel), vec![key(Key::Tab)]);
    assert_eq!(tc(CommandId::Refresh), vec![letter('R').ctrl()]);
    assert_eq!(
        tc(CommandId::Search),
        vec![f(7).alt(), letter('F').ctrl(), letter('E').ctrl()]
    );
    assert_eq!(
        tc(CommandId::TogglePreview),
        vec![letter('Q').ctrl(), letter('P').alt()]
    );
    // 표의 키를 잃은 명령.
    assert_eq!(
        tc(CommandId::EditAddress),
        vec![letter('D').alt(), letter('L').ctrl()]
    );
    // 표에 없는 명령은 탐색기 그대로.
    assert_eq!(tc(CommandId::Copy), vec![letter('C').ctrl()]);
    assert_eq!(
        tc(CommandId::DeletePermanently),
        vec![key(Key::Delete).shift()]
    );

    let keymap = Keymap::new(Preset::TotalCommander);
    let list = |s| keymap.resolve(s, Context::List);
    assert_eq!(list(f(3)), None, "F3은 비운다");
    assert_eq!(list(f(4)), Some(CommandId::Open));
    assert_eq!(list(f(5)), Some(CommandId::CopyToOtherPanel));
    assert_eq!(list(f(6)), Some(CommandId::MoveToOtherPanel));
    assert_eq!(list(f(6).shift()), Some(CommandId::Rename));
    assert_eq!(list(f(7)), Some(CommandId::NewFolder));
    assert_eq!(list(f(8)), Some(CommandId::Delete));
    assert_eq!(list(key(Key::Delete)), Some(CommandId::Delete));
    assert_eq!(list(letter('R').ctrl()), Some(CommandId::Refresh));
    assert_eq!(list(f(7).alt()), Some(CommandId::Search));
    assert_eq!(list(letter('Q').ctrl()), Some(CommandId::TogglePreview));
    assert_eq!(list(key(Key::Tab)), Some(CommandId::SwitchPanel));
}

#[test]
#[ignore = "#460"]
fn neither_preset_has_a_conflict() {
    for preset in [Preset::Explorer, Preset::TotalCommander] {
        assert_eq!(Keymap::new(preset).conflicts(), vec![], "{preset:?}");
    }
}

#[test]
#[ignore = "#460"]
fn preset_ids_round_trip() {
    for preset in [Preset::Explorer, Preset::TotalCommander] {
        assert_eq!(Preset::from_id(preset.as_str()), Some(preset));
    }
    assert_eq!(Preset::Explorer.as_str(), "explorer");
    assert_eq!(Preset::TotalCommander.as_str(), "total-commander");
    assert_eq!(Preset::from_id("vim"), None);
}

#[test]
#[ignore = "#460"]
fn user_changes_overlay_the_preset() {
    let mut keymap = Keymap::new(Preset::Explorer);
    keymap.set_shortcuts(CommandId::Refresh, vec![letter('R').ctrl(), f(9)]);
    assert!(keymap.is_overridden(CommandId::Refresh));
    assert_eq!(
        keymap.shortcuts(CommandId::Refresh),
        vec![letter('R').ctrl(), f(9)]
    );
    assert_eq!(
        keymap.resolve(letter('R').ctrl(), Context::List),
        Some(CommandId::Refresh)
    );
    // 소문자 글자도 같은 키다.
    assert_eq!(
        keymap.resolve(letter('r').ctrl(), Context::List),
        Some(CommandId::Refresh)
    );
    assert_eq!(
        keymap.resolve(f(9), Context::List),
        Some(CommandId::Refresh)
    );
    // 변경이 프리셋의 F5를 대신한다.
    assert_eq!(keymap.resolve(f(5), Context::List), None);
    // 다른 명령은 그대로다.
    assert_eq!(
        keymap.resolve(letter('A').ctrl(), Context::List),
        Some(CommandId::SelectAll)
    );

    keymap.reset(CommandId::Refresh);
    assert!(!keymap.is_overridden(CommandId::Refresh));
    assert_eq!(
        keymap.resolve(f(5), Context::List),
        Some(CommandId::Refresh)
    );
    assert_eq!(keymap.resolve(f(9), Context::List), None);
}

#[test]
#[ignore = "#460"]
fn changing_the_preset_keeps_user_changes() {
    let mut keymap = Keymap::new(Preset::Explorer);
    keymap.set_shortcuts(CommandId::Rename, vec![f(9)]);
    keymap.set_preset(Preset::TotalCommander);
    assert_eq!(keymap.preset(), Preset::TotalCommander);
    assert_eq!(keymap.shortcuts(CommandId::Rename), vec![f(9)]);
    assert!(keymap.is_overridden(CommandId::Rename));
    assert_eq!(keymap.resolve(f(9), Context::List), Some(CommandId::Rename));
    assert_eq!(keymap.resolve(f(6).shift(), Context::List), None);
    // 변경하지 않은 명령은 새 프리셋을 따른다.
    assert_eq!(
        keymap.resolve(f(5), Context::List),
        Some(CommandId::CopyToOtherPanel)
    );
    keymap.set_preset(Preset::Explorer);
    assert_eq!(keymap.shortcuts(CommandId::Rename), vec![f(9)]);
    assert_eq!(
        keymap.resolve(f(5), Context::List),
        Some(CommandId::Refresh)
    );
}

#[test]
#[ignore = "#460"]
fn removing_every_shortcut_of_a_command_is_a_change() {
    let mut keymap = Keymap::new(Preset::Explorer);
    keymap.set_shortcuts(CommandId::Delete, vec![]);
    assert!(keymap.is_overridden(CommandId::Delete));
    assert_eq!(keymap.shortcuts(CommandId::Delete), vec![]);
    assert_eq!(keymap.resolve(key(Key::Delete), Context::List), None);
    // 프리셋을 바꿔도 빠진 채다.
    keymap.set_preset(Preset::TotalCommander);
    assert_eq!(keymap.shortcuts(CommandId::Delete), vec![]);
    assert_eq!(keymap.resolve(f(8), Context::List), None);
}

#[test]
#[ignore = "#460"]
fn reset_all_drops_every_change_and_keeps_the_preset() {
    let mut keymap = Keymap::new(Preset::TotalCommander);
    keymap.set_shortcuts(CommandId::Delete, vec![]);
    keymap.set_shortcuts(CommandId::Undo, vec![f(11)]);
    keymap.reset_all();
    assert_eq!(keymap.preset(), Preset::TotalCommander);
    for command in CommandId::ALL {
        assert!(!keymap.is_overridden(command), "{command:?}");
        assert_eq!(
            keymap.shortcuts(command),
            keymap::preset_shortcuts(Preset::TotalCommander, command),
            "{command:?}"
        );
    }
    assert_eq!(keymap.resolve(f(8), Context::List), Some(CommandId::Delete));
}

#[test]
#[ignore = "#460"]
fn command_contexts_follow_the_ticket() {
    for command in [
        CommandId::CommandPalette,
        CommandId::NewTab,
        CommandId::CloseTab,
        CommandId::NextTab,
        CommandId::PreviousTab,
        CommandId::GoToTab(1),
        CommandId::GoToTab(8),
        CommandId::LastTab,
        CommandId::ReopenClosedTab,
        CommandId::TogglePanelSplit,
        CommandId::NewWindow,
        CommandId::Search,
        CommandId::Filter,
        CommandId::TogglePreview,
        CommandId::EditAddress,
        CommandId::OpenSettings,
    ] {
        assert_eq!(keymap::context_of(command), Context::Global, "{command:?}");
    }
    for command in [
        CommandId::Open,
        CommandId::Undo,
        CommandId::Copy,
        CommandId::Paste,
        CommandId::Rename,
        CommandId::Delete,
        CommandId::SwitchPanel,
        CommandId::Refresh,
        CommandId::SelectAll,
        CommandId::About,
        CommandId::OpenSettingsFile,
    ] {
        assert_eq!(keymap::context_of(command), Context::List, "{command:?}");
    }
}

#[test]
#[ignore = "#460"]
fn the_input_context_only_hears_global_commands() {
    let keymap = Keymap::new(Preset::Explorer);
    // 전역 명령은 입력란에서도 듣는다.
    assert_eq!(
        keymap.resolve(letter('T').ctrl(), Context::Input),
        Some(CommandId::NewTab)
    );
    assert_eq!(
        keymap.resolve(letter('P').ctrl().shift(), Context::Input),
        Some(CommandId::CommandPalette)
    );
    // 목록 명령은 입력란의 글자 편집으로 남는다.
    assert_eq!(keymap.resolve(letter('Z').ctrl(), Context::Input), None);
    assert_eq!(keymap.resolve(letter('C').ctrl(), Context::Input), None);
    assert_eq!(keymap.resolve(f(2), Context::Input), None);
    assert_eq!(keymap.resolve(key(Key::Delete), Context::Input), None);
    assert_eq!(keymap.resolve(key(Key::Tab), Context::Input), None);
    // 목록에서는 둘 다 듣는다.
    assert_eq!(
        keymap.resolve(letter('T').ctrl(), Context::List),
        Some(CommandId::NewTab)
    );
    assert_eq!(
        keymap.resolve(letter('Z').ctrl(), Context::List),
        Some(CommandId::Undo)
    );
}

#[test]
#[ignore = "#460"]
fn plain_letter_keys_never_fire_in_an_input() {
    let mut keymap = Keymap::new(Preset::Explorer);
    // 전역 명령에 수정키 없는 글자·숫자·스페이스·Shift 글자를 준다.
    keymap.set_shortcuts(
        CommandId::NewTab,
        vec![
            letter('Q'),
            key(Key::Digit(7)),
            key(Key::Space),
            letter('Q').shift(),
            letter('Q').alt(),
        ],
    );
    assert_eq!(
        keymap.resolve(letter('Q'), Context::List),
        Some(CommandId::NewTab)
    );
    assert_eq!(keymap.resolve(letter('Q'), Context::Input), None);
    assert_eq!(keymap.resolve(key(Key::Digit(7)), Context::Input), None);
    assert_eq!(keymap.resolve(key(Key::Space), Context::Input), None);
    assert_eq!(keymap.resolve(letter('Q').shift(), Context::Input), None);
    // 수정키가 있으면 듣는다.
    assert_eq!(
        keymap.resolve(letter('Q').alt(), Context::Input),
        Some(CommandId::NewTab)
    );
}

#[test]
#[ignore = "#460"]
fn conflicts_in_the_same_context_let_the_last_set_command_win() {
    let mut keymap = Keymap::new(Preset::Explorer);
    // 실행취소(목록)에 새로 고침(목록)의 F5를 준다: 사용자 변경이 프리셋보다 나중이다.
    keymap.set_shortcuts(CommandId::Undo, vec![letter('Z').ctrl(), f(5)]);
    assert_eq!(keymap.resolve(f(5), Context::List), Some(CommandId::Undo));
    assert_eq!(
        keymap.conflicts(),
        vec![keymap::Conflict {
            shortcut: f(5),
            commands: conflict_commands(&keymap, f(5)),
            winner: CommandId::Undo,
        }]
    );
    let conflict = &keymap.conflicts()[0];
    assert_eq!(conflict.commands.len(), 2);
    assert!(conflict.commands.contains(&CommandId::Undo));
    assert!(conflict.commands.contains(&CommandId::Refresh));
    // 진 명령도 단축키 목록에는 남는다(충돌로 표시할 것).
    assert_eq!(keymap.shortcuts(CommandId::Refresh), vec![f(5)]);

    // 사용자 변경끼리는 나중에 정한 것이 이긴다.
    keymap.set_shortcuts(CommandId::Refresh, vec![f(5)]);
    assert_eq!(
        keymap.resolve(f(5), Context::List),
        Some(CommandId::Refresh)
    );
    assert_eq!(keymap.conflicts()[0].winner, CommandId::Refresh);

    // 겹침을 풀면 충돌이 사라진다.
    keymap.reset(CommandId::Undo);
    assert_eq!(keymap.conflicts(), vec![]);
}

/// 충돌 목록에서 그 키의 명령들(차례는 구현이 정한다).
fn conflict_commands(keymap: &Keymap, shortcut: Shortcut) -> Vec<CommandId> {
    keymap
        .conflicts()
        .into_iter()
        .find(|c| c.shortcut == shortcut)
        .map(|c| c.commands)
        .unwrap_or_default()
}

#[test]
#[ignore = "#460"]
fn a_global_command_conflicts_with_every_context() {
    let mut keymap = Keymap::new(Preset::Explorer);
    // 이름 바꾸기(목록)에 새 탭(전역)의 Ctrl+T를 준다.
    keymap.set_shortcuts(CommandId::Rename, vec![letter('T').ctrl()]);
    let conflicts = keymap.conflicts();
    assert_eq!(conflicts.len(), 1, "{conflicts:?}");
    assert_eq!(conflicts[0].shortcut, letter('T').ctrl());
    assert_eq!(conflicts[0].winner, CommandId::Rename);
    assert_eq!(
        keymap.resolve(letter('T').ctrl(), Context::List),
        Some(CommandId::Rename)
    );
    // 진 명령은 그 키로 어디서도 동작하지 않는다.
    assert_eq!(keymap.resolve(letter('T').ctrl(), Context::Input), None);
}

#[test]
#[ignore = "#460"]
fn conflicts_with_lists_the_other_commands_while_typing() {
    let keymap = Keymap::new(Preset::Explorer);
    assert_eq!(
        keymap.conflicts_with(CommandId::Rename, letter('T').ctrl()),
        vec![CommandId::NewTab]
    );
    assert_eq!(
        keymap.conflicts_with(CommandId::Undo, f(5)),
        vec![CommandId::Refresh]
    );
    // 자기 자신의 키는 겹침이 아니다.
    assert_eq!(keymap.conflicts_with(CommandId::Refresh, f(5)), vec![]);
    // 아무도 쓰지 않는 키.
    assert_eq!(keymap.conflicts_with(CommandId::Refresh, f(11)), vec![]);
}

#[test]
#[ignore = "#460"]
fn shortcut_text_round_trips() {
    let cases = [
        (letter('A').ctrl(), "Ctrl+A"),
        (f(5), "F5"),
        (f(6).shift(), "Shift+F6"),
        (letter('F').ctrl().shift(), "Ctrl+Shift+F"),
        (f(7).alt(), "Alt+F7"),
        (letter('Z').ctrl().alt().shift(), "Ctrl+Alt+Shift+Z"),
        (key(Key::Digit(1)).ctrl(), "Ctrl+1"),
        (key(Key::Backslash).ctrl(), "Ctrl+\\"),
        (key(Key::Delete).shift(), "Shift+Delete"),
        (key(Key::Enter).alt(), "Alt+Enter"),
        (key(Key::Escape), "Esc"),
        (key(Key::PageDown), "PageDown"),
        (key(Key::Tab), "Tab"),
        (key(Key::Space).ctrl(), "Ctrl+Space"),
        (f(24), "F24"),
        // 쉼표 키(#462의 「설정」 Ctrl+,)는 글자 그대로다.
        (key(Key::Comma).ctrl(), "Ctrl+,"),
    ];
    for (shortcut, text) in cases {
        assert_eq!(shortcut.to_text(), text);
        assert_eq!(Shortcut::parse(text), Some(shortcut), "{text}");
    }
    // 대소문자를 가리지 않는다.
    assert_eq!(
        Shortcut::parse("ctrl+shift+f"),
        Some(letter('F').ctrl().shift())
    );
    assert_eq!(Shortcut::parse("shift+f6"), Some(f(6).shift()));
    for bad in [
        "",
        "Ctrl+",
        "Ctrl+Hyper+A",
        "F99",
        "Ctrl+AB",
        "Ctrl+K Ctrl+S",
    ] {
        assert_eq!(Shortcut::parse(bad), None, "{bad}");
    }
}

#[test]
#[ignore = "#460"]
fn the_preset_and_changes_survive_the_settings_file() {
    let dir = TempDir::new().unwrap();
    let mut settings = Settings::load(dir.path());
    assert_eq!(settings.keymap().preset(), Preset::Explorer);
    assert_eq!(
        settings.set("shortcuts.preset", Value::Choice("total-commander")),
        Ok(Saved::Written)
    );
    assert_eq!(settings.keymap().preset(), Preset::TotalCommander);
    assert_eq!(
        settings.set_shortcuts(CommandId::Refresh, vec![f(9), letter('R').ctrl().shift()]),
        Saved::Written
    );
    assert_eq!(
        settings.set_shortcuts(CommandId::Delete, vec![]),
        Saved::Written
    );

    let reloaded = Settings::load(dir.path());
    let keymap = reloaded.keymap();
    assert_eq!(keymap.preset(), Preset::TotalCommander);
    assert_eq!(
        keymap.shortcuts(CommandId::Refresh),
        vec![f(9), letter('R').ctrl().shift()]
    );
    assert!(keymap.is_overridden(CommandId::Delete));
    assert_eq!(keymap.shortcuts(CommandId::Delete), vec![]);
    assert_eq!(
        keymap.resolve(f(5), Context::List),
        Some(CommandId::CopyToOtherPanel)
    );

    // 파일은 사람이 읽을 수 있는 모양이다.
    let text = fs::read_to_string(dir.path().join(FILE_NAME)).unwrap();
    let parsed: toml::Table = text.parse().unwrap();
    assert_eq!(
        parsed["shortcuts"]["preset"].as_str(),
        Some("total-commander")
    );
    let refresh: Vec<&str> = parsed["shortcuts"]["commands"]["refresh"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(refresh, vec!["F9", "Ctrl+Shift+R"]);
    assert_eq!(
        parsed["shortcuts"]["commands"]["delete"]
            .as_array()
            .map(|a| a.len()),
        Some(0)
    );
}

#[test]
#[ignore = "#460"]
fn resetting_shortcuts_removes_them_from_the_file() {
    let dir = TempDir::new().unwrap();
    let mut settings = Settings::load(dir.path());
    settings.set_shortcuts(CommandId::Refresh, vec![f(9)]);
    settings.set_shortcuts(CommandId::Undo, vec![f(11)]);
    assert_eq!(settings.reset_shortcuts(CommandId::Refresh), Saved::Written);
    assert!(!settings.keymap().is_overridden(CommandId::Refresh));
    let reloaded = Settings::load(dir.path());
    assert!(!reloaded.keymap().is_overridden(CommandId::Refresh));
    assert_eq!(reloaded.keymap().shortcuts(CommandId::Undo), vec![f(11)]);

    assert_eq!(settings.reset_all_shortcuts(), Saved::Written);
    let reloaded = Settings::load(dir.path());
    for command in CommandId::ALL {
        assert!(!reloaded.keymap().is_overridden(command), "{command:?}");
    }
}

#[test]
#[ignore = "#460"]
fn a_bad_shortcut_text_ignores_only_that_change_and_stays_in_the_file() {
    let dir = TempDir::new().unwrap();
    let original = "\
[shortcuts.commands]
refresh = [\"Ctrl+Hyper+R\"]
undo = [\"F11\"]
no-such-command = [\"F12\"]
";
    fs::write(dir.path().join(FILE_NAME), original).unwrap();
    let mut settings = Settings::load(dir.path());
    assert!(settings.syntax_error().is_none());
    let problems = settings.problems();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0].key, "shortcuts.commands.refresh");
    assert_eq!(problems[0].error, ValueError::InvalidShortcut);
    assert_eq!(problems[0].line, 2);
    // 그 변경만 무시한다.
    assert!(!settings.keymap().is_overridden(CommandId::Refresh));
    assert_eq!(settings.keymap().shortcuts(CommandId::Refresh), vec![f(5)]);
    assert_eq!(settings.keymap().shortcuts(CommandId::Undo), vec![f(11)]);

    // 다른 변경을 써도 틀린 글자와 모르는 명령은 파일에 남는다.
    assert_eq!(
        settings.set_shortcuts(CommandId::Redo, vec![f(10)]),
        Saved::Written
    );
    let after = fs::read_to_string(dir.path().join(FILE_NAME)).unwrap();
    assert!(after.contains("refresh = [\"Ctrl+Hyper+R\"]"), "{after}");
    assert!(after.contains("no-such-command = [\"F12\"]"), "{after}");
    assert_eq!(
        Settings::load(dir.path())
            .keymap()
            .shortcuts(CommandId::Redo),
        vec![f(10)]
    );
}

#[test]
#[ignore = "#460"]
fn in_a_reloaded_file_the_later_line_wins_a_conflict() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join(FILE_NAME),
        "[shortcuts.commands]\nundo = [\"F9\"]\nredo = [\"F9\"]\n",
    )
    .unwrap();
    let settings = Settings::load(dir.path());
    assert_eq!(
        settings.keymap().resolve(f(9), Context::List),
        Some(CommandId::Redo)
    );
    let conflicts = settings.keymap().conflicts();
    assert_eq!(conflicts.len(), 1, "{conflicts:?}");
    assert_eq!(conflicts[0].winner, CommandId::Redo);

    fs::write(
        dir.path().join(FILE_NAME),
        "[shortcuts.commands]\nredo = [\"F9\"]\nundo = [\"F9\"]\n",
    )
    .unwrap();
    assert_eq!(
        Settings::load(dir.path())
            .keymap()
            .resolve(f(9), Context::List),
        Some(CommandId::Undo)
    );
}
