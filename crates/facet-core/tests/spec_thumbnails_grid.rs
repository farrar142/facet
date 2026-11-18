// target: crates/facet-core/tests/spec_thumbnails_grid.rs
// AC: #327: 새 탭은 자세히 보기·보통, 보기 방식과 칸 크기는 탭마다 따로, 칸 크기마다 썸네일 한 변(작게<보통<크게<아주 크게)
// AC: #327: details-view(Ctrl+Shift+6)·grid-view-*(Ctrl+Shift+4·3·2·1) 명령이 목록·팔레트에 있고 늘 켜지며 켜짐 표시, 기존 Ctrl+숫자는 그대로
// AC: #327: 자세히 보기로 바꿔도 칸 크기는 남음, Ctrl+휠(zoom_grid)은 한 단계씩 끝에서 멈춤
// AC: #327: 세션에 탭마다 보기 방식·칸 크기가 남고 되살아남, 필드 없는 앞선 세션 파일은 자세히 보기·보통
// AC: #327: 격자 계산(열 수·행 수·순번↔행열·화살표 이동·보이는 칸과 위아래 한 화면), 그리드 화살표의 선택 규칙과 보기를 바꿔도 선택·커서 유지
//! 그리드 보기의 코어 몫을 확인한다: 탭의 보기 상태(`Tab::view`·`set_view`·`zoom_grid`), 보기 명령(ID·단축키·실행·켜짐
//! 표시·팔레트), 세션 왕복, 순수 격자 계산(`view::GridLayout`), 그리드의 2차원 커서 이동(`Tab::move_grid_cursor`). 임시
//! 폴더의 실제 앱으로 돌고 모든 플랫폼에서 돈다. 그리드 화면 자체는 사람이 확인한다.

use std::fs;
use std::path::Path;
use std::time::Duration;

use facet_core::Location;
use facet_core::command::{CommandId, Key, Shortcut, find_by_shortcut};
use facet_core::selection::Pick;
use facet_core::session::Session;
use facet_core::tab::ReadOutcome;
use facet_core::view::{GridLayout, GridMove, GridSize, GridSpans, View, ViewMode};
use facet_core::window::{App, Execution};
use tempfile::TempDir;

/// CI 러너가 느려도 흔들리지 않게 넉넉히 기다린다. 결과가 오면 곧바로 끝난다.
const WAIT: Duration = Duration::from_secs(20);

/// 보기 명령, ID 글자, Ctrl+Shift와 함께 누르는 숫자(탐색기와 같음).
const VIEW_COMMANDS: [(CommandId, &str, u8); 5] = [
    (CommandId::DetailsView, "details-view", 6),
    (
        CommandId::GridView(GridSize::ExtraLarge),
        "grid-view-extra-large",
        1,
    ),
    (CommandId::GridView(GridSize::Large), "grid-view-large", 2),
    (CommandId::GridView(GridSize::Medium), "grid-view-medium", 3),
    (CommandId::GridView(GridSize::Small), "grid-view-small", 4),
];

/// 파일 `count`개(f00.txt, f01.txt, …)가 든 폴더.
fn folder(count: usize) -> TempDir {
    let dir = TempDir::new().unwrap();
    for i in 0..count {
        fs::write(dir.path().join(format!("f{i:02}.txt")), b"x").unwrap();
    }
    dir
}

fn app_at(path: &Path) -> App {
    let mut app = App::new(Some(Location::file_system(path)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

fn grid(size: GridSize) -> View {
    View {
        mode: ViewMode::Grid,
        grid_size: size,
    }
}

fn shortcut(digit: u8) -> Shortcut {
    Shortcut::new(Key::Digit(digit)).ctrl().shift()
}

#[test]
#[ignore = "#327"]
fn 새_탭은_자세히_보기_보통_크기다() {
    assert_eq!(ViewMode::default(), ViewMode::Details);
    assert_eq!(GridSize::default(), GridSize::Medium);
    assert_eq!(
        View::default(),
        View {
            mode: ViewMode::Details,
            grid_size: GridSize::Medium,
        }
    );
    let dir = folder(3);
    let app = app_at(dir.path());
    assert_eq!(app.active_tab().view(), View::default());
}

#[test]
#[ignore = "#327"]
fn 칸_크기는_네_단계이고_한_변이_차례로_커진다() {
    assert_eq!(
        GridSize::ALL,
        [
            GridSize::Small,
            GridSize::Medium,
            GridSize::Large,
            GridSize::ExtraLarge,
        ]
    );
    assert!(GridSize::Small.thumbnail_side() > 0);
    for pair in GridSize::ALL.windows(2) {
        assert!(
            pair[0].thumbnail_side() < pair[1].thumbnail_side(),
            "{:?} < {:?}",
            pair[0],
            pair[1]
        );
        assert_eq!(pair[0].larger(), Some(pair[1]));
        assert_eq!(pair[1].smaller(), Some(pair[0]));
    }
    assert_eq!(GridSize::Small.smaller(), None);
    assert_eq!(GridSize::ExtraLarge.larger(), None);
}

#[test]
#[ignore = "#327"]
fn 보기_명령은_바뀌지_않는_id와_탐색기와_같은_단축키를_갖는다() {
    for (command, id, digit) in VIEW_COMMANDS {
        assert_eq!(command.as_str(), id);
        assert_eq!(CommandId::from_id(id), Some(command));
        assert!(CommandId::ALL.contains(&command), "{id}가 명령 목록에 있다");
        assert_eq!(command.shortcuts().first(), Some(&shortcut(digit)), "{id}");
        assert_eq!(find_by_shortcut(shortcut(digit)), Some(command), "{id}");
    }
    // Shift 없는 Ctrl+숫자는 그대로 탭 고르기다.
    assert_eq!(
        find_by_shortcut(Shortcut::new(Key::Digit(1)).ctrl()),
        Some(CommandId::GoToTab(1))
    );
    assert_eq!(
        find_by_shortcut(Shortcut::new(Key::Digit(6)).ctrl()),
        Some(CommandId::GoToTab(6))
    );
}

#[test]
#[ignore = "#327"]
fn 명령으로_보기를_바꾸고_지금_보기에_켜짐_표시가_붙는다() {
    let dir = folder(3);
    let mut app = app_at(dir.path());
    let checked = |app: &App| -> Vec<&'static str> {
        VIEW_COMMANDS
            .iter()
            .filter(|(c, _, _)| c.is_checked(app.active_tab()))
            .map(|(_, id, _)| *id)
            .collect()
    };
    assert_eq!(checked(&app), ["details-view"]);

    assert_eq!(
        app.execute(CommandId::GridView(GridSize::Large)),
        Execution::Done
    );
    assert_eq!(app.active_tab().view(), grid(GridSize::Large));
    assert_eq!(checked(&app), ["grid-view-large"]);

    assert_eq!(app.execute(CommandId::DetailsView), Execution::Done);
    assert_eq!(
        app.active_tab().view(),
        View {
            mode: ViewMode::Details,
            grid_size: GridSize::Large,
        },
        "자세히 보기로 바꿔도 칸 크기는 남는다"
    );
    assert_eq!(checked(&app), ["details-view"]);

    assert_eq!(
        app.execute(CommandId::GridView(GridSize::Small)),
        Execution::Done
    );
    assert_eq!(app.active_tab().view(), grid(GridSize::Small));
}

#[test]
#[ignore = "#327"]
fn 보기_명령은_빈_폴더에서도_켜지고_팔레트에_나온다() {
    let dir = folder(0);
    let app = app_at(dir.path());
    let palette = app.search_commands("", |c| c.as_str());
    for (command, id, _) in VIEW_COMMANDS {
        assert!(app.can_execute(command), "{id}는 늘 켜진다");
        let entry = palette
            .iter()
            .find(|e| e.command == command)
            .unwrap_or_else(|| panic!("{id}가 팔레트에 없다"));
        assert!(entry.enabled);
    }
}

#[test]
#[ignore = "#327"]
fn 보기_방식과_칸_크기는_탭마다_따로다() {
    let dir = folder(3);
    let mut app = app_at(dir.path());
    let first = app.active_tab_id();
    let second = app.open_tab(Location::file_system(dir.path()), true);
    assert_eq!(app.active_tab_id(), second);
    assert_eq!(
        app.active_tab().view(),
        View::default(),
        "새 탭은 기본 보기"
    );
    app.execute(CommandId::GridView(GridSize::ExtraLarge));

    assert!(app.activate_tab(first));
    assert_eq!(app.active_tab().view(), View::default(), "첫 탭은 그대로");
    app.execute(CommandId::GridView(GridSize::Small));

    assert!(app.activate_tab(second));
    assert_eq!(app.active_tab().view(), grid(GridSize::ExtraLarge));
    assert!(app.activate_tab(first));
    assert_eq!(app.active_tab().view(), grid(GridSize::Small));
}

#[test]
#[ignore = "#327"]
fn ctrl_휠은_칸_크기를_한_단계씩_바꾸고_끝에서_멈춘다() {
    let dir = folder(3);
    let mut app = app_at(dir.path());
    app.execute(CommandId::GridView(GridSize::Medium));
    let tab = app.active_tab_mut();
    assert!(tab.zoom_grid(1));
    assert_eq!(tab.view(), grid(GridSize::Large));
    assert!(tab.zoom_grid(5));
    assert_eq!(tab.view(), grid(GridSize::ExtraLarge), "끝을 넘지 않는다");
    assert!(!tab.zoom_grid(1), "끝에서는 바뀌지 않는다");
    assert_eq!(tab.view(), grid(GridSize::ExtraLarge));
    assert!(tab.zoom_grid(-1));
    assert_eq!(tab.view(), grid(GridSize::Large));
    assert!(tab.zoom_grid(-10));
    assert_eq!(tab.view(), grid(GridSize::Small));
    assert!(!tab.zoom_grid(-1));
    assert!(!tab.zoom_grid(0));
    assert_eq!(tab.view(), grid(GridSize::Small));
}

#[test]
#[ignore = "#327"]
fn 세션에_탭마다_보기가_남고_되살아난다() {
    let dir = folder(3);
    let mut app = app_at(dir.path());
    app.execute(CommandId::GridView(GridSize::Large));
    app.open_tab(Location::file_system(dir.path()), true);
    app.active_tab_mut().set_view(View {
        mode: ViewMode::Details,
        grid_size: GridSize::Small,
    });
    let expected = [
        grid(GridSize::Large),
        View {
            mode: ViewMode::Details,
            grid_size: GridSize::Small,
        },
    ];

    let session = app.session().expect("창이 있다");
    let tabs = &session.windows[0].panels[0].tabs;
    assert_eq!(tabs.iter().map(|t| t.view).collect::<Vec<_>>(), expected);

    let parsed = Session::from_json(&session.to_json()).expect("다시 읽는다");
    assert_eq!(parsed, session, "JSON 왕복");

    let restored = App::restore(Some(parsed), Vec::new());
    let views: Vec<View> = restored.windows()[0].panels()[0]
        .tabs()
        .map(|(_, tab)| tab.view())
        .collect();
    assert_eq!(views, expected);
}

const OLDER_FILE: &str = r#"{
  "version": 1,
  "active_window": 0,
  "windows": [
    {
      "placement": null,
      "split_ratio": 0.5,
      "active_panel": 0,
      "panels": [
        { "active_tab": 0, "tabs": [ { "location": { "file_system": "C:\\" }, "sort": { "column": "name", "direction": "ascending" } } ] }
      ]
    }
  ]
}"#;

#[test]
#[ignore = "#327"]
fn 보기가_없는_앞선_세션_파일은_자세히_보기_보통으로_읽는다() {
    let session = Session::from_json(OLDER_FILE).expect("앞선 파일도 읽는다");
    assert_eq!(session.windows[0].panels[0].tabs[0].view, View::default());
    let restored = App::restore(Some(session), Vec::new());
    assert_eq!(restored.active_tab().view(), View::default());
}

#[test]
#[ignore = "#327"]
fn 창_폭과_칸_폭으로_열_수를_정하고_적어도_한_열이다() {
    assert_eq!(GridLayout::for_width(1000, 150).columns, 6);
    assert_eq!(GridLayout::for_width(300, 150).columns, 2);
    assert_eq!(GridLayout::for_width(299, 150).columns, 1);
    assert_eq!(GridLayout::for_width(149, 150).columns, 1);
    assert_eq!(GridLayout::for_width(0, 150).columns, 1);
}

#[test]
#[ignore = "#327"]
fn 순번과_행열을_오간다() {
    let layout = GridLayout { columns: 6 };
    assert_eq!(layout.rows(0), 0);
    assert_eq!(layout.rows(6), 1);
    assert_eq!(layout.rows(13), 3);
    assert_eq!(layout.position(0), (0, 0));
    assert_eq!(layout.position(7), (1, 1));
    assert_eq!(layout.position(12), (2, 0));
    assert_eq!(layout.index(1, 1, 13), Some(7));
    assert_eq!(layout.index(2, 0, 13), Some(12));
    assert_eq!(layout.index(2, 1, 13), None, "마지막 행의 빈 칸");
    assert_eq!(layout.index(0, 6, 13), None, "열 밖");
    assert_eq!(layout.index(3, 0, 13), None, "행 밖");
}

#[test]
#[ignore = "#327"]
fn 화살표는_행과_열을_따라_움직인다() {
    // 열 넷, 항목 열: 0..=3 / 4..=7 / 8..=9
    let layout = GridLayout { columns: 4 };
    let len = 10;
    for (from, movement, to) in [
        (3, GridMove::Right, 4),
        (9, GridMove::Right, 9),
        (4, GridMove::Left, 3),
        (0, GridMove::Left, 0),
        (5, GridMove::Up, 1),
        (2, GridMove::Up, 2),
        (1, GridMove::Down, 5),
        (5, GridMove::Down, 9),
        (6, GridMove::Down, 9),
        (7, GridMove::Down, 9),
        (8, GridMove::Down, 8),
        (9, GridMove::Down, 9),
    ] {
        assert_eq!(
            layout.step(from, movement, len),
            to,
            "{from}에서 {movement:?}"
        );
    }
}

#[test]
#[ignore = "#327"]
fn 보이는_칸과_위아래_한_화면을_순번_범위로_준다() {
    let layout = GridLayout { columns: 4 };
    assert_eq!(
        layout.spans(10, 3, 100),
        GridSpans {
            visible: 40..52,
            nearby: 28..64,
        }
    );
    assert_eq!(
        layout.spans(0, 3, 100),
        GridSpans {
            visible: 0..12,
            nearby: 0..24,
        },
        "맨 위"
    );
    assert_eq!(
        layout.spans(23, 3, 100),
        GridSpans {
            visible: 92..100,
            nearby: 80..100,
        },
        "맨 아래"
    );
    assert_eq!(
        layout.spans(0, 3, 5),
        GridSpans {
            visible: 0..5,
            nearby: 0..5,
        },
        "한 화면보다 짧은 목록"
    );
}

#[test]
#[ignore = "#327"]
fn 그리드_화살표는_커서_키와_같은_선택_규칙이고_보기를_바꿔도_선택이_남는다() {
    let dir = folder(10);
    let mut app = app_at(dir.path());
    app.execute(CommandId::GridView(GridSize::Medium));
    let layout = GridLayout { columns: 4 };
    let tab = app.active_tab_mut();
    tab.select_only(5);
    assert_eq!(
        tab.move_grid_cursor(GridMove::Down, layout, Pick::Only),
        Some(9)
    );
    assert_eq!(tab.cursor(), Some(9));
    assert_eq!(tab.selected_indices().collect::<Vec<_>>(), [9]);

    assert_eq!(
        tab.move_grid_cursor(GridMove::Up, layout, Pick::Range),
        Some(5)
    );
    assert_eq!(tab.cursor(), Some(5));
    assert_eq!(
        tab.selected_indices().collect::<Vec<_>>(),
        [5, 6, 7, 8, 9],
        "범위 기준(9)부터 5까지"
    );

    assert_eq!(
        tab.move_grid_cursor(GridMove::Left, layout, Pick::CursorOnly),
        Some(4)
    );
    assert_eq!(tab.cursor(), Some(4));
    assert_eq!(tab.selected_count(), 5, "커서만 옮김");

    app.execute(CommandId::DetailsView);
    let tab = app.active_tab();
    assert_eq!(tab.cursor(), Some(4), "보기를 바꿔도 커서는 그대로");
    assert_eq!(tab.selected_indices().collect::<Vec<_>>(), [5, 6, 7, 8, 9]);
}
