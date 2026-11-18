// target: crates/facet-core/tests/spec_virtual_folders_capabilities.rs
// AC: #348: 공급자가 되는 일을 값으로 알리고 탭이 준다, 실제 폴더는 복원·비우기 말고 모두 된다, 되는 일과 명령의 묶음, 묶이지 않은 명령은 막히지 않는다
//! **되는 일**(공급자가 위치마다 알리는 가능한 작업)의 값과, 그 값에 따라 명령이 막히는지를 정하는 코어 함수를 확인한다:
//! 실제 폴더의 되는 일, 되는 일 하나를 뺄 때마다 그에 묶인 명령만 막히는 것, 되는 일이 하나도 없어도 막히지 않는 명령.
//! 가짜 공급자를 쓰지 않고 공급자 값과 코어 함수만 본다. 모든 플랫폼에서 돈다.

use facet_core::command::CommandId;
use facet_core::details::Sort;
use facet_core::tab::Tab;
use facet_core::{Capabilities, Location, provider_for};
use tempfile::TempDir;

/// 되는 일 하나를 끄는 함수.
type Clear = fn(&mut Capabilities);

/// 되는 일 하나와 그것에 묶인 명령들.
const BINDINGS: [(&str, Clear, &[CommandId]); 9] = [
    (
        "open",
        |c| c.open = false,
        &[CommandId::Open, CommandId::OpenInNewTab],
    ),
    (
        "copy_from",
        |c| c.copy_from = false,
        &[
            CommandId::Copy,
            CommandId::CopyToOtherPanel,
            CommandId::CopyTo,
        ],
    ),
    (
        "move_items",
        |c| c.move_items = false,
        &[
            CommandId::Cut,
            CommandId::MoveToOtherPanel,
            CommandId::MoveTo,
        ],
    ),
    ("copy_into", |c| c.copy_into = false, &[CommandId::Paste]),
    (
        "rename",
        |c| c.rename = false,
        &[CommandId::Rename, CommandId::BatchRename],
    ),
    (
        "new_folder",
        |c| c.new_folder = false,
        &[CommandId::NewFolder],
    ),
    (
        "delete_permanently",
        |c| c.delete_permanently = false,
        &[CommandId::DeletePermanently],
    ),
    (
        "properties",
        |c| c.properties = false,
        &[CommandId::Properties],
    ),
    // 휴지통 삭제만 꺼져도 영구 삭제로 지울 수 있어 아무 명령도 막히지 않는다.
    ("recycle", |c| c.recycle = false, &[]),
];

/// 되는 일과 상관없는 명령의 보기.
const UNBOUND: [CommandId; 16] = [
    CommandId::Back,
    CommandId::Forward,
    CommandId::Up,
    CommandId::Refresh,
    CommandId::SelectAll,
    CommandId::ToggleHidden,
    CommandId::CommandPalette,
    CommandId::SortByName,
    CommandId::NewTab,
    CommandId::CloseTab,
    CommandId::NewWindow,
    CommandId::TogglePanelSplit,
    CommandId::Undo,
    CommandId::Filter,
    CommandId::Search,
    CommandId::EditAddress,
];

/// 묶음 표에 든 모든 명령.
fn bound_commands() -> Vec<CommandId> {
    let mut all: Vec<CommandId> = BINDINGS
        .iter()
        .flat_map(|(_, _, commands)| commands.iter().copied())
        .collect();
    all.push(CommandId::Delete);
    all
}

#[test]
#[ignore = "#348"]
fn all_and_default_capabilities() {
    let all = Capabilities::ALL;
    assert!(
        all.open
            && all.copy_from
            && all.copy_into
            && all.move_items
            && all.recycle
            && all.delete_permanently
            && all.rename
            && all.new_folder
            && all.restore
            && all.empty
            && all.properties
            && all.undoable,
        "ALL은 모든 되는 일이 켜져 있다: {all:?}"
    );
    let none = Capabilities::default();
    assert_eq!(
        none,
        Capabilities {
            open: false,
            copy_from: false,
            copy_into: false,
            move_items: false,
            recycle: false,
            delete_permanently: false,
            rename: false,
            new_folder: false,
            restore: false,
            empty: false,
            properties: false,
            undoable: false,
        }
    );
}

#[test]
#[ignore = "#348"]
fn a_real_folder_can_do_everything_but_restore_and_empty() {
    let dir = TempDir::new().unwrap();
    let location = Location::file_system(dir.path());
    let expected = Capabilities {
        restore: false,
        empty: false,
        ..Capabilities::ALL
    };
    assert_eq!(provider_for(&location).capabilities(&location), expected);

    let tab = Tab::new(location, Sort::default(), false);
    assert_eq!(tab.capabilities(), expected, "탭은 공급자의 값을 준다");
}

#[test]
#[ignore = "#348"]
fn every_command_is_allowed_when_everything_can_be_done() {
    for command in bound_commands().into_iter().chain(UNBOUND) {
        assert!(
            command.is_allowed_by(&Capabilities::ALL),
            "{command:?}는 모든 되는 일이 있으면 막히지 않는다"
        );
    }
}

#[test]
#[ignore = "#348"]
fn clearing_one_capability_blocks_only_its_commands() {
    let all = bound_commands();
    for (name, clear, blocked) in BINDINGS {
        let mut capabilities = Capabilities::ALL;
        clear(&mut capabilities);
        for &command in &all {
            assert_eq!(
                command.is_allowed_by(&capabilities),
                !blocked.contains(&command),
                "{name}을 끄면 {command:?}가 막히는가"
            );
        }
        for command in UNBOUND {
            assert!(
                command.is_allowed_by(&capabilities),
                "{name}을 꺼도 {command:?}는 막히지 않는다"
            );
        }
    }
}

#[test]
#[ignore = "#348"]
fn delete_needs_recycle_or_permanent_delete() {
    let only_recycle = Capabilities {
        recycle: true,
        ..Capabilities::default()
    };
    assert!(CommandId::Delete.is_allowed_by(&only_recycle));
    assert!(!CommandId::DeletePermanently.is_allowed_by(&only_recycle));

    let only_permanent = Capabilities {
        delete_permanently: true,
        ..Capabilities::default()
    };
    assert!(CommandId::Delete.is_allowed_by(&only_permanent));
    assert!(CommandId::DeletePermanently.is_allowed_by(&only_permanent));

    let neither = Capabilities {
        recycle: false,
        delete_permanently: false,
        ..Capabilities::ALL
    };
    assert!(!CommandId::Delete.is_allowed_by(&neither));
    assert!(!CommandId::DeletePermanently.is_allowed_by(&neither));
}

#[test]
#[ignore = "#348"]
fn unbound_commands_are_allowed_with_no_capabilities() {
    let none = Capabilities::default();
    for command in UNBOUND {
        assert!(
            command.is_allowed_by(&none),
            "{command:?}는 되는 일과 상관없다"
        );
    }
    for command in bound_commands() {
        assert!(
            !command.is_allowed_by(&none),
            "{command:?}는 되는 일이 없으면 막힌다"
        );
    }
}
