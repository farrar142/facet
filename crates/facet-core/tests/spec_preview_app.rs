// target: crates/facet-core/tests/spec_preview_app.rs
// AC: #339: Alt+P 명령과 켜고 끄기·기본 꺼짐, 창마다 켜짐·폭과 새 창의 물려받기·폭 자르기, 세션 왕복과 앞선 파일, 텍스트·없음·여러 항목·폴더 내용, 빠른 선택에서 마지막 항목만, 활성 패널 따라가기, 켜면 지금 선택, 밖에서 고침·지움 따라가기(Windows), 잠긴 파일(Windows)
//! 앱([`App`])의 미리보기 영역(`Window::preview_pane`, `App::preview`·`sync_preview`, `Update::Preview`, 명령
//! `TogglePreview`, 세션의 `preview` 필드)을 임시 폴더의 실제 파일로 확인한다. 화면은 사람이 확인한다. 변경 감시를
//! 쓰는 따라가기와 잠긴 파일은 Windows에서만 돈다.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::command::{CommandId, Key, Shortcut, find_by_shortcut};
use facet_core::preview::{
    PREVIEW_WIDTH_DEFAULT, PREVIEW_WIDTH_MAX, PREVIEW_WIDTH_MIN, PreviewBody, PreviewContent,
    PreviewPane, PreviewProblem,
};
use facet_core::session::Session;
use facet_core::tab::ReadOutcome;
use facet_core::window::{App, Execution, Update, WindowId};
use tempfile::TempDir;

/// CI 러너가 느려도 흔들리지 않게 넉넉히 기다린다. 조건이 맞으면 곧바로 끝난다.
const WAIT: Duration = Duration::from_secs(10);

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, text).unwrap();
    path
}

/// `a.txt`(「알파」), `b.txt`(「브라보」), `c.txt`(「찰리」), `폴더`(파일 둘)가 든 임시 폴더.
fn tree() -> TempDir {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "a.txt", "알파");
    write(dir.path(), "b.txt", "브라보");
    write(dir.path(), "c.txt", "찰리");
    let folder = dir.path().join("폴더");
    fs::create_dir(&folder).unwrap();
    write(&folder, "x.txt", "x");
    write(&folder, "y.txt", "y");
    dir
}

fn app_at(path: &Path) -> App {
    let mut app = App::new(Some(Location::file_system(path)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

/// 활성 창의 미리보기 영역을 켠다.
fn turn_on(app: &mut App) {
    if !app.active_window().preview_pane().visible {
        assert_eq!(app.execute(CommandId::TogglePreview), Execution::Done);
    }
    assert!(app.active_window().preview_pane().visible);
}

fn index_of(app: &App, name: &str) -> usize {
    app.active_tab()
        .items()
        .position(|item| item.name == name)
        .unwrap_or_else(|| panic!("{name}이 목록에 없다"))
}

/// 그 이름들만 고르고 미리보기에 알린다.
fn select(app: &mut App, names: &[&str]) {
    let indices: Vec<usize> = names.iter().map(|n| index_of(app, n)).collect();
    let tab = app.active_tab_mut();
    if let Some((first, rest)) = indices.split_first() {
        tab.select_only(*first);
        for index in rest {
            tab.toggle_select(*index);
        }
    }
    app.sync_preview();
}

/// 활성 창의 미리보기 내용이 조건에 맞을 때까지 받는다. 받는 동안 본 내용을 모두 `seen`에 남긴다.
fn wait_content_seen(
    app: &mut App,
    seen: &mut Vec<PreviewContent>,
    wanted: impl Fn(&PreviewContent) -> bool,
) -> PreviewContent {
    let window = app.active_window_id();
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(content) = app.preview(window) {
            if seen.last() != Some(content) {
                seen.push(content.clone());
            }
            if wanted(content) {
                return content.clone();
            }
        }
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(
            !left.is_zero(),
            "미리보기가 시간 안에 오지 않았다: {:?}",
            app.preview(window)
        );
        app.wait_update(left);
    }
}

fn wait_content(app: &mut App, wanted: impl Fn(&PreviewContent) -> bool) -> PreviewContent {
    wait_content_seen(app, &mut Vec::new(), wanted)
}

fn body_of<'a>(content: &'a PreviewContent, name: &str) -> Option<&'a PreviewBody> {
    match content {
        PreviewContent::Item { item, body, .. } if item.name == name => Some(body),
        _ => None,
    }
}

fn shows_text(content: &PreviewContent, name: &str, text: &str) -> bool {
    matches!(body_of(content, name), Some(PreviewBody::Text(t)) if t.text == text)
}

#[test]
#[ignore = "#339"]
fn alt_p_is_the_preview_pane_command() {
    let p = Shortcut::new(Key::Letter('P')).alt();
    assert_eq!(find_by_shortcut(p), Some(CommandId::TogglePreview));
    assert!(CommandId::TogglePreview.shortcuts().contains(&p));
    assert_eq!(CommandId::TogglePreview.as_str(), "toggle-preview");
    assert!(CommandId::ALL.contains(&CommandId::TogglePreview));
}

#[test]
#[ignore = "#339"]
fn the_command_toggles_the_active_window_pane_which_starts_off() {
    let dir = tree();
    let mut app = app_at(dir.path());
    let window = app.active_window_id();
    assert_eq!(app.active_window().preview_pane(), PreviewPane::default());
    assert!(!PreviewPane::default().visible, "기본은 꺼짐");
    assert!(app.can_execute(CommandId::TogglePreview), "늘 켜져 있다");
    assert!(app.preview(window).is_none(), "꺼져 있으면 내용이 없다");

    assert_eq!(app.execute(CommandId::TogglePreview), Execution::Done);
    assert!(app.active_window().preview_pane().visible);
    assert!(app.preview(window).is_some());

    assert_eq!(app.execute(CommandId::TogglePreview), Execution::Done);
    assert!(!app.active_window().preview_pane().visible);
    assert!(app.preview(window).is_none());
}

#[test]
#[ignore = "#339"]
fn the_width_starts_at_the_default_and_is_clamped() {
    assert_eq!(PREVIEW_WIDTH_DEFAULT, 320.0);
    assert_eq!(PREVIEW_WIDTH_MIN, 160.0);
    assert_eq!(PREVIEW_WIDTH_MAX, 1600.0);
    assert_eq!(PreviewPane::default().width, PREVIEW_WIDTH_DEFAULT);

    let dir = tree();
    let mut app = app_at(dir.path());
    let window = app.active_window_mut();
    assert_eq!(window.set_preview_width(500.0), 500.0);
    assert_eq!(window.preview_pane().width, 500.0);
    assert_eq!(window.set_preview_width(10.0), PREVIEW_WIDTH_MIN);
    assert_eq!(window.set_preview_width(99_999.0), PREVIEW_WIDTH_MAX);
    window.set_preview_width(400.0);
    window.set_preview_width(f32::NAN);
    assert_eq!(window.preview_pane().width, 400.0, "NaN이면 그대로");
}

#[test]
#[ignore = "#339"]
fn each_window_keeps_its_own_pane_and_a_new_window_inherits_the_active_one() {
    let dir = tree();
    let mut app = app_at(dir.path());
    let first = app.active_window_id();
    turn_on(&mut app);
    app.active_window_mut().set_preview_width(450.0);

    let second = app.open_window(Location::file_system(dir.path()));
    assert_ne!(first, second);
    assert_eq!(app.active_window_id(), second);
    assert_eq!(
        app.window(second).unwrap().preview_pane(),
        PreviewPane {
            visible: true,
            width: 450.0
        },
        "새 창은 활성 창의 켜짐·폭을 물려받는다"
    );

    assert_eq!(app.execute(CommandId::TogglePreview), Execution::Done);
    app.active_window_mut().set_preview_width(200.0);
    assert!(!app.window(second).unwrap().preview_pane().visible);
    assert_eq!(
        app.window(first).unwrap().preview_pane(),
        PreviewPane {
            visible: true,
            width: 450.0
        },
        "다른 창의 영역은 그대로"
    );
}

#[test]
#[ignore = "#339"]
fn the_pane_is_saved_in_the_session_and_restored() {
    let dir = tree();
    let mut app = app_at(dir.path());
    turn_on(&mut app);
    app.active_window_mut().set_preview_width(480.0);
    let session = app.session().expect("창이 있다");
    let saved = PreviewPane {
        visible: true,
        width: 480.0,
    };
    assert_eq!(session.windows[0].preview, saved);

    let read = Session::from_json(&session.to_json()).expect("다시 읽는다");
    assert_eq!(read.windows[0].preview, saved);

    let restored = App::restore(Some(read), Vec::new());
    assert_eq!(restored.active_window().preview_pane(), saved);
}

#[test]
#[ignore = "#339"]
fn an_older_session_without_the_field_reads_as_off() {
    let dir = tree();
    let mut app = app_at(dir.path());
    turn_on(&mut app);
    let json = app.session().unwrap().to_json();
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let window = value["windows"][0].as_object_mut().expect("창 객체");
    assert!(
        window.remove("preview").is_some(),
        "JSON 필드 이름은 preview"
    );
    let older = serde_json::to_string(&value).unwrap();

    let read = Session::from_json(&older).expect("앞선 파일도 읽힌다");
    assert_eq!(read.windows[0].preview, PreviewPane::default());
}

#[test]
#[ignore = "#339"]
fn one_text_file_shows_its_text_and_no_selection_shows_nothing() {
    let dir = tree();
    let mut app = app_at(dir.path());
    turn_on(&mut app);
    app.sync_preview();
    wait_content(&mut app, |c| *c == PreviewContent::Nothing);

    select(&mut app, &["b.txt"]);
    let content = wait_content(&mut app, |c| shows_text(c, "b.txt", "브라보"));
    match content {
        PreviewContent::Item { path, item, .. } => {
            assert_eq!(path, dir.path().join("b.txt"));
            assert_eq!(item.size, "브라보".len() as u64);
        }
        other => panic!("항목이어야 한다: {other:?}"),
    }

    app.active_tab_mut().clear_selection();
    app.sync_preview();
    wait_content(&mut app, |c| *c == PreviewContent::Nothing);
}

#[test]
#[ignore = "#339"]
fn several_items_show_the_count_and_the_file_size_sum() {
    let dir = tree();
    let mut app = app_at(dir.path());
    turn_on(&mut app);
    select(&mut app, &["a.txt", "c.txt", "폴더"]);
    let total = ("알파".len() + "찰리".len()) as u64;
    let content = wait_content(&mut app, |c| matches!(c, PreviewContent::Many { .. }));
    assert_eq!(
        content,
        PreviewContent::Many {
            count: 3,
            total_size: total
        },
        "폴더의 크기는 세지 않는다"
    );
}

#[test]
#[ignore = "#339"]
fn a_folder_shows_its_item_count() {
    let dir = tree();
    let mut app = app_at(dir.path());
    turn_on(&mut app);
    select(&mut app, &["폴더"]);
    wait_content(&mut app, |c| {
        body_of(c, "폴더") == Some(&PreviewBody::Folder { count: Some(2) })
    });
}

#[test]
#[ignore = "#339"]
fn rapid_selection_changes_end_on_the_last_item_only() {
    let dir = tree();
    let mut app = app_at(dir.path());
    turn_on(&mut app);
    select(&mut app, &["a.txt"]);
    select(&mut app, &["b.txt"]);
    select(&mut app, &["c.txt"]);
    let mut seen = Vec::new();
    wait_content_seen(&mut app, &mut seen, |c| shows_text(c, "c.txt", "찰리"));
    // 마지막 선택 뒤 본 내용은 모두 c.txt에 대한 것이다.
    for content in &seen {
        assert!(
            body_of(content, "c.txt").is_some(),
            "앞선 항목의 내용이 나타났다: {content:?}"
        );
    }
    // 결과가 더 와도 c.txt 그대로다.
    let window = app.active_window_id();
    let deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < deadline {
        app.wait_update(Duration::from_millis(50));
        assert!(shows_text(app.preview(window).unwrap(), "c.txt", "찰리"));
    }
}

#[test]
#[ignore = "#339"]
fn the_pane_follows_the_active_panel() {
    let dir = tree();
    let mut app = app_at(dir.path());
    turn_on(&mut app);
    select(&mut app, &["a.txt"]);
    wait_content(&mut app, |c| shows_text(c, "a.txt", "알파"));

    app.split_panel().expect("나눈다");
    let deadline = Instant::now() + WAIT;
    while app.active_tab().items().count() == 0 {
        assert!(Instant::now() < deadline, "오른쪽 패널 읽기");
        app.wait_update(Duration::from_millis(100));
    }
    assert_eq!(app.active_window().active_panel_index(), 1);
    select(&mut app, &["b.txt"]);
    wait_content(&mut app, |c| shows_text(c, "b.txt", "브라보"));

    assert!(app.switch_panel());
    app.sync_preview();
    wait_content(&mut app, |c| shows_text(c, "a.txt", "알파"));
}

#[test]
#[ignore = "#339"]
fn turning_the_pane_on_shows_the_current_selection() {
    let dir = tree();
    let mut app = app_at(dir.path());
    select(&mut app, &["c.txt"]);
    let window = app.active_window_id();
    assert!(app.preview(window).is_none());
    turn_on(&mut app);
    wait_content(&mut app, |c| shows_text(c, "c.txt", "찰리"));
}

#[test]
#[ignore = "#339"]
fn a_preview_update_names_the_window() {
    let dir = tree();
    let mut app = app_at(dir.path());
    turn_on(&mut app);
    let window: WindowId = app.active_window_id();
    select(&mut app, &["a.txt"]);
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "Update::Preview가 오지 않았다");
        if let Some((_, Update::Preview(got))) = app.wait_update(left) {
            assert_eq!(got, window);
            break;
        }
    }
}

#[cfg(windows)]
#[test]
#[ignore = "#339"]
fn editing_the_previewed_file_outside_updates_the_preview() {
    let dir = tree();
    let mut app = app_at(dir.path());
    turn_on(&mut app);
    select(&mut app, &["a.txt"]);
    wait_content(&mut app, |c| shows_text(c, "a.txt", "알파"));

    fs::write(dir.path().join("a.txt"), "고친 내용").unwrap();
    wait_content(&mut app, |c| shows_text(c, "a.txt", "고친 내용"));
}

#[cfg(windows)]
#[test]
#[ignore = "#339"]
fn deleting_the_previewed_file_shows_it_is_gone() {
    let dir = tree();
    let mut app = app_at(dir.path());
    turn_on(&mut app);
    select(&mut app, &["b.txt"]);
    wait_content(&mut app, |c| shows_text(c, "b.txt", "브라보"));

    fs::remove_file(dir.path().join("b.txt")).unwrap();
    let missing = PreviewBody::Unavailable(PreviewProblem::Missing);
    wait_content(&mut app, |c| body_of(c, "b.txt") == Some(&missing));
    // 화면이 입력 뒤 늘 부르는 sync_preview로는 사라짐 표시가 지워지지 않는다(사용자가 다른 항목을 고를 때까지).
    app.sync_preview();
    let window = app.active_window_id();
    assert_eq!(
        body_of(app.preview(window).unwrap(), "b.txt"),
        Some(&missing)
    );
}

#[cfg(windows)]
#[test]
#[ignore = "#339"]
fn a_locked_file_shows_the_reason_and_the_app_keeps_going() {
    use std::os::windows::fs::OpenOptionsExt;

    let dir = tree();
    let path = write(dir.path(), "locked.log", "잠김");
    let mut app = app_at(dir.path());
    turn_on(&mut app);
    let deadline = Instant::now() + WAIT;
    while !app.active_tab().items().any(|i| i.name == "locked.log") {
        assert!(Instant::now() < deadline, "목록에 나타나야 한다");
        app.wait_update(Duration::from_millis(100));
    }
    let _held = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    select(&mut app, &["locked.log"]);
    let locked = PreviewBody::Unavailable(PreviewProblem::Locked);
    wait_content(&mut app, |c| body_of(c, "locked.log") == Some(&locked));

    select(&mut app, &["a.txt"]);
    wait_content(&mut app, |c| shows_text(c, "a.txt", "알파"));
}
