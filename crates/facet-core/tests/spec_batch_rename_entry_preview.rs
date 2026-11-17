// target: crates/facet-core/tests/spec_batch_rename_entry_preview.rs
// AC: #292: 요청 번호와 Update::BatchPreview가 동기 미리 보기와 같음, 잇단 두 요청은 앞이 None·뒤가 뒤 규칙, 꺼져 있으면 None·사건 없음, 요청 때의 항목으로 만듦
//! 일괄 이름 바꾸기 미리 보기를 일꾼 스레드에서 계산하는 입구(`App::request_batch_rename_preview`)와 그 결과
//! (`Update::BatchPreview`)를 임시 폴더를 연 앱에서 확인한다: 동기 입구(`App::batch_rename_preview_rules`)와 같은 값,
//! 앞선 요청의 결과 버리기, 꺼진 명령, 요청 뒤 선택 바뀜. 파일을 바꾸지 않으므로 모든 환경에서 돈다.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::batch_rename::{Edge, Preview, Rule, RuleKind};
use facet_core::tab::ReadOutcome;
use facet_core::window::{App, BatchPreviewId, TabId, Update};
use tempfile::TempDir;

/// CI 러너가 느려도 흔들리지 않게 넉넉히 기다린다. 결과가 오면 곧바로 끝난다.
const WAIT: Duration = Duration::from_secs(20);

const NAMES: [&str; 4] = ["a.txt", "b.txt", "c.txt", "d.txt"];

/// 받은 미리 보기 결과: 요청 번호와 미리 보기(버렸으면 `None`).
type Received = Vec<(BatchPreviewId, Option<Preview>)>;

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

fn index_of(app: &App, name: &str) -> usize {
    let tab = app.active_tab();
    (0..tab.len())
        .find(|&i| tab.get(i).is_some_and(|item| item.name == name))
        .unwrap_or_else(|| panic!("목록에 {name}이 없다"))
}

fn select(app: &mut App, names: &[&str]) {
    let indices: Vec<usize> = names.iter().map(|name| index_of(app, name)).collect();
    let tab = app.active_tab_mut();
    tab.select_only(indices[0]);
    for &index in &indices[1..] {
        tab.toggle_select(index);
    }
}

fn replace(find: &str, with: &str) -> Vec<Rule> {
    vec![Rule::new(RuleKind::Replace {
        find: find.to_string(),
        replace: with.to_string(),
        regex: false,
        case_sensitive: false,
        all: true,
    })]
}

/// `until`의 모든 번호가 올 때까지 결과를 받는다. 미리 보기 결과는 그 탭 번호와 함께 모은다.
fn receive_until(app: &mut App, until: &[BatchPreviewId]) -> (Received, Vec<TabId>) {
    let deadline = Instant::now() + WAIT;
    let mut received = Vec::new();
    let mut tabs = Vec::new();
    while !until
        .iter()
        .all(|want| received.iter().any(|(id, _)| id == want))
    {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "미리 보기 결과가 시간 안에 오지 않았다");
        if let Some((tab, Update::BatchPreview { id, preview })) = app.wait_update(left) {
            received.push((id, preview));
            tabs.push(tab);
        }
    }
    (received, tabs)
}

#[test]
fn the_result_arrives_as_an_update_equal_to_the_synchronous_preview() {
    let dir = folder();
    let mut app = app_at(dir.path());
    select(&mut app, &["a.txt", "c.txt"]);
    let rules = replace("a", "z");
    let expected = app
        .batch_rename_preview_rules(&rules, 0)
        .expect("동기 미리 보기");
    let tab = app.active_tab_id();

    let id = app
        .request_batch_rename_preview(&rules, 0)
        .expect("명령이 켜져 있다");
    let (received, tabs) = receive_until(&mut app, &[id]);
    assert_eq!(received, vec![(id, Some(expected))]);
    assert_eq!(tabs, [tab], "요청 때의 활성 탭으로 온다");
}

#[test]
fn an_earlier_request_is_discarded_once_and_only_the_last_one_is_drawn() {
    let dir = folder();
    let mut app = app_at(dir.path());
    select(&mut app, &["a.txt", "b.txt", "c.txt"]);
    let first = replace("a", "x");
    let second = replace("b", "y");
    let expected = app.batch_rename_preview_rules(&second, 0).unwrap();
    assert_ne!(app.batch_rename_preview_rules(&first, 0).unwrap(), expected);

    let one = app.request_batch_rename_preview(&first, 0).unwrap();
    let two = app.request_batch_rename_preview(&second, 0).unwrap();
    assert_ne!(one, two, "요청마다 번호가 다르다");

    let (received, _) = receive_until(&mut app, &[one, two]);
    // 조금 더 받아 같은 요청의 결과가 두 번 오지 않는지 본다.
    std::thread::sleep(Duration::from_millis(200));
    let late: Vec<_> = app
        .poll()
        .into_iter()
        .filter(|(_, update)| matches!(update, Update::BatchPreview { .. }))
        .collect();
    assert!(late.is_empty(), "요청마다 결과는 하나다: {late:?}");

    let of = |want: BatchPreviewId| -> Vec<&Option<Preview>> {
        received
            .iter()
            .filter(|(id, _)| *id == want)
            .map(|(_, preview)| preview)
            .collect()
    };
    assert_eq!(of(one), [&None], "앞선 요청은 미리 보기 없이 한 번");
    assert_eq!(of(two), [&Some(expected)], "마지막 요청의 미리 보기");
}

#[test]
fn a_disabled_command_gives_no_request_and_no_update() {
    let dir = folder();
    let mut app = app_at(dir.path());
    select(&mut app, &["a.txt"]);

    assert_eq!(
        app.request_batch_rename_preview(&replace("a", "z"), 0),
        None
    );
    std::thread::sleep(Duration::from_millis(300));
    let updates: Vec<_> = app
        .poll()
        .into_iter()
        .filter(|(_, update)| matches!(update, Update::BatchPreview { .. }))
        .collect();
    assert!(updates.is_empty(), "결과 사건이 없다: {updates:?}");
}

#[test]
fn the_result_uses_the_items_selected_when_requesting() {
    let dir = folder();
    let mut app = app_at(dir.path());
    select(&mut app, &["a.txt", "c.txt"]);
    let rules = vec![Rule::new(RuleKind::Insert {
        text: "p-".to_string(),
        from: Edge::Start,
        at: 0,
    })];
    let expected = app.batch_rename_preview_rules(&rules, 0).unwrap();

    let id = app.request_batch_rename_preview(&rules, 0).unwrap();
    select(&mut app, &NAMES);

    let (received, _) = receive_until(&mut app, &[id]);
    let preview = received[0].1.clone().expect("버리지 않았다");
    let originals: Vec<_> = preview
        .entries()
        .iter()
        .map(|entry| entry.original.to_string_lossy().into_owned())
        .collect();
    assert_eq!(originals, ["a.txt", "c.txt"], "요청 때 고른 두 항목");
    assert_eq!(preview, expected);
}
