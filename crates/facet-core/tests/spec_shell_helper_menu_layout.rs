// target: crates/facet-core/tests/spec_shell_helper_menu_layout.rs
// AC: #321: 항목 메뉴 19줄·빈 곳 메뉴 18줄 고정 순서(검색 결과 위치 포함, 맥락과 상관없음), 「연결 프로그램」·「속성」 켜짐, Alt+Enter와 ID 글자·팔레트
//! 오른쪽 메뉴의 자체 항목 「연결 프로그램」·「속성」을 더한 순수 구성(`menu::build`, 대표 결정 2026-11-18의 안 A)과
//! 두 명령의 켜짐·기본 단축키·ID 글자·팔레트를 확인한다. 순수 함수라 모든 환경에서 돈다.

use facet_core::command::{CommandId, Key, Shortcut, find_by_shortcut};
use facet_core::details::{Column, Direction, Sort};
use facet_core::menu::{Menu, MenuContext, MenuEntry, MenuKind, SubmenuId, build};

fn context(kind: MenuKind) -> MenuContext {
    MenuContext {
        kind,
        selected_count: 0,
        has_cursor: false,
        cursor_is_folder: false,
        single_folder_selected: false,
        location_is_folder: true,
        location_is_results: false,
        panel_count: 1,
        opposite_is_folder: false,
        clipboard_has_files: false,
        sort: Sort {
            column: Column::Name,
            direction: Direction::Ascending,
        },
        show_hidden: false,
        can_go_back: true,
        can_go_forward: true,
        can_go_up: true,
        can_undo: true,
        location_is_sync_folder: false,
    }
}

/// 커서가 파일 하나에 있고 그것만 고른 항목 메뉴 맥락.
fn file_context() -> MenuContext {
    MenuContext {
        selected_count: 1,
        has_cursor: true,
        ..context(MenuKind::Item)
    }
}

/// 메뉴의 한 줄을 견주기 쉬운 모양으로 펼친다.
#[derive(Debug, PartialEq, Eq)]
enum Row {
    Command(CommandId),
    Submenu(SubmenuId),
    Separator,
}

fn rows(menu: &Menu) -> Vec<Row> {
    menu.entries
        .iter()
        .map(|entry| match entry {
            MenuEntry::Command(c) => Row::Command(c.id),
            MenuEntry::Submenu(s) => Row::Submenu(s.id),
            MenuEntry::Separator => Row::Separator,
        })
        .collect()
}

fn enabled(menu: &Menu, id: CommandId) -> bool {
    menu.commands()
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("{id:?} 줄이 없다"))
        .enabled
}

fn item_layout() -> Vec<Row> {
    use CommandId::*;
    let c = Row::Command;
    vec![
        c(Open),
        c(OpenWith),
        c(OpenInNewTab),
        Row::Separator,
        c(Cut),
        c(Copy),
        c(Paste),
        c(CopyPath),
        Row::Separator,
        c(CopyToOtherPanel),
        c(MoveToOtherPanel),
        c(CopyTo),
        c(MoveTo),
        Row::Separator,
        c(Rename),
        c(BatchRename),
        c(Delete),
        Row::Separator,
        c(Properties),
    ]
}

fn background_layout() -> Vec<Row> {
    use CommandId::*;
    let c = Row::Command;
    vec![
        c(Back),
        c(Forward),
        c(Up),
        Row::Separator,
        c(Paste),
        c(NewFolder),
        c(Undo),
        Row::Separator,
        Row::Submenu(SubmenuId::SortBy),
        c(ToggleHidden),
        c(Refresh),
        Row::Separator,
        c(Search),
        c(TogglePanelSplit),
        Row::Separator,
        c(CopyPath),
        Row::Separator,
        c(Properties),
    ]
}

#[test]
fn item_menu_has_open_with_below_open_and_properties_at_the_end() {
    let menu = build(&file_context());
    assert_eq!(menu.kind, MenuKind::Item);
    assert_eq!(rows(&menu), item_layout());
    assert_eq!(menu.entries.len(), 19);
}

#[test]
fn search_results_item_menu_keeps_the_same_order_after_its_two_head_rows() {
    let menu = build(&MenuContext {
        location_is_folder: false,
        location_is_results: true,
        ..file_context()
    });
    let rows = rows(&menu);
    assert_eq!(
        rows[..2],
        [Row::Command(CommandId::ShowInFolder), Row::Separator]
    );
    assert_eq!(rows[2..], item_layout()[..]);
}

#[test]
fn background_menu_ends_with_the_folder_properties() {
    let menu = build(&context(MenuKind::Background));
    assert_eq!(menu.kind, MenuKind::Background);
    assert_eq!(rows(&menu), background_layout());
    assert_eq!(menu.entries.len(), 18);
}

#[test]
fn the_order_is_the_same_whatever_the_context() {
    for selected_count in [0, 1, 2, 10_000] {
        for has_cursor in [false, true] {
            for cursor_is_folder in [false, true] {
                for (folder, results) in [(true, false), (false, false), (false, true)] {
                    for panel_count in [1, 2] {
                        let c = MenuContext {
                            selected_count,
                            has_cursor,
                            cursor_is_folder: has_cursor && cursor_is_folder,
                            location_is_folder: folder,
                            location_is_results: results,
                            panel_count,
                            ..context(MenuKind::Item)
                        };
                        let item = rows(&build(&c));
                        let item = if results { &item[2..] } else { &item[..] };
                        assert_eq!(item, &item_layout()[..], "{c:?}");
                        let background = MenuContext {
                            kind: MenuKind::Background,
                            ..c
                        };
                        assert_eq!(
                            rows(&build(&background)),
                            background_layout(),
                            "{background:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn open_with_is_enabled_only_for_a_file_under_the_cursor() {
    assert!(enabled(&build(&file_context()), CommandId::OpenWith));

    let folder = MenuContext {
        cursor_is_folder: true,
        ..file_context()
    };
    assert!(!enabled(&build(&folder), CommandId::OpenWith), "폴더");

    let nothing = context(MenuKind::Item);
    assert!(!enabled(&build(&nothing), CommandId::OpenWith), "커서 없음");

    let results = MenuContext {
        location_is_folder: false,
        location_is_results: true,
        ..file_context()
    };
    assert!(
        enabled(&build(&results), CommandId::OpenWith),
        "검색 결과의 파일"
    );

    let elsewhere = MenuContext {
        location_is_folder: false,
        ..file_context()
    };
    assert!(
        !enabled(&build(&elsewhere), CommandId::OpenWith),
        "실제 파일 시스템 항목이 아님"
    );
}

#[test]
fn properties_is_enabled_when_there_is_something_to_show() {
    // 항목 메뉴: 고른 항목이 있을 때(오른쪽 클릭은 누른 항목을 고른다). 커서만 있으면 꺼진다: 그때 실행하면 현재
    // 폴더의 속성이라 항목 메뉴의 뜻과 다르다.
    assert!(enabled(&build(&file_context()), CommandId::Properties));
    let selection_only = MenuContext {
        selected_count: 3,
        has_cursor: false,
        ..context(MenuKind::Item)
    };
    assert!(enabled(&build(&selection_only), CommandId::Properties));
    let folder = MenuContext {
        cursor_is_folder: true,
        ..file_context()
    };
    assert!(enabled(&build(&folder), CommandId::Properties));
    let cursor_only = MenuContext {
        selected_count: 0,
        ..file_context()
    };
    assert!(!enabled(&build(&cursor_only), CommandId::Properties));
    assert!(!enabled(
        &build(&context(MenuKind::Item)),
        CommandId::Properties
    ));
    let results = MenuContext {
        location_is_folder: false,
        location_is_results: true,
        ..file_context()
    };
    assert!(enabled(&build(&results), CommandId::Properties));
    let elsewhere = MenuContext {
        location_is_folder: false,
        ..file_context()
    };
    assert!(!enabled(&build(&elsewhere), CommandId::Properties));

    // 빈 곳 메뉴: 위치가 실제 폴더일 때 그 폴더의 속성.
    assert!(enabled(
        &build(&context(MenuKind::Background)),
        CommandId::Properties
    ));
    for (folder, results) in [(false, false), (false, true)] {
        let c = MenuContext {
            location_is_folder: folder,
            location_is_results: results,
            ..context(MenuKind::Background)
        };
        assert!(!enabled(&build(&c), CommandId::Properties), "{c:?}");
    }
}

#[test]
fn alt_enter_is_properties_and_open_with_has_no_default_shortcut() {
    let alt_enter = Shortcut::new(Key::Enter).alt();
    assert_eq!(find_by_shortcut(alt_enter), Some(CommandId::Properties));
    assert_eq!(CommandId::Properties.shortcuts().first(), Some(&alt_enter));
    assert!(CommandId::OpenWith.shortcuts().is_empty());
    // Enter만 누르면 그대로 열기다.
    assert_eq!(
        find_by_shortcut(Shortcut::new(Key::Enter)),
        Some(CommandId::Open)
    );
}

#[test]
fn the_two_commands_have_fixed_ids_and_appear_in_the_palette() {
    assert_eq!(CommandId::OpenWith.as_str(), "open-with");
    assert_eq!(CommandId::Properties.as_str(), "properties");
    assert_eq!(CommandId::from_id("open-with"), Some(CommandId::OpenWith));
    assert_eq!(
        CommandId::from_id("properties"),
        Some(CommandId::Properties)
    );
    assert!(CommandId::ALL.contains(&CommandId::OpenWith));
    assert!(CommandId::ALL.contains(&CommandId::Properties));
}
