// target: crates/facet-core/tests/spec_zip_watch.rs
// AC: #424: 밖에서 다시 쓴 zip은 새로 고침 없이 새 목록, 지운 zip은 zip이 든 실제 폴더로 옮기고 까닭을 남김
//! zip 위치를 보는 탭이 그 zip 파일의 바깥 변경을 따라가는지를 임시 폴더의 실제 zip 파일로 확인한다: 다른 프로그램이
//! zip을 다시 쓰면 목록이 바뀌고, zip을 지우면 탭이 zip이 든 실제 폴더로 옮겨져 `Relocation::ToParent`가 남는다.
//! 변경 감시는 Windows 전용이라 Windows에서만 돈다(감시 테스트처럼 넉넉히 기다린다).

#![cfg(windows)]

#[path = "spec_zip_support.rs"]
mod zip_support;

use std::path::Path;
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::tab::{ReadOutcome, Relocation};
use facet_core::window::App;
use tempfile::TempDir;
use zip_support::Entry;

const WAIT: Duration = Duration::from_secs(30);
const CHANGE_WAIT: Duration = Duration::from_secs(10);

fn app_at(location: Location) -> App {
    let mut app = App::new(Some(location));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

fn tab_names(app: &App) -> Vec<String> {
    let tab = app.active_tab();
    let mut names: Vec<String> = (0..tab.len())
        .map(|i| tab.get(i).unwrap().name.to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// `done`이 참이 될 때까지 앱의 결과를 받으며 기다린다.
fn wait_until(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + CHANGE_WAIT;
    while !done(app) {
        assert!(
            Instant::now() < deadline,
            "{what}: 시간 안에 바뀌지 않았다. 목록 {:?}, 위치 {:?}",
            tab_names(app),
            app.active_tab().location()
        );
        let _ = app.wait_update(Duration::from_millis(50));
        app.poll();
    }
}

fn write_docs(zip: &Path, names: &[&str]) {
    let entries: Vec<Entry> = names
        .iter()
        .map(|name| Entry::stored(&format!("문서/{name}"), name.as_bytes()))
        .collect();
    zip_support::write_zip(zip, &entries);
}

#[test]
#[ignore = "#424"]
fn a_zip_rewritten_outside_shows_the_new_list() {
    let dir = TempDir::new().unwrap();
    let zip = dir.path().join("자료.zip");
    write_docs(&zip, &["처음.txt"]);
    let mut app = app_at(Location::zip(&zip, "문서"));
    assert_eq!(tab_names(&app), ["처음.txt"]);

    // 다른 프로그램이 zip을 다시 쓴다(크기와 내용이 바뀜).
    std::thread::sleep(Duration::from_millis(50));
    write_docs(&zip, &["처음.txt", "새로 넣은.txt"]);

    wait_until(&mut app, "다시 쓴 zip", |app| {
        tab_names(app) == ["새로 넣은.txt", "처음.txt"]
    });
    assert_eq!(app.active_tab().location(), &Location::zip(&zip, "문서"));
}

#[test]
#[ignore = "#424"]
fn a_deleted_zip_moves_the_tab_to_its_folder() {
    let dir = TempDir::new().unwrap();
    let zip = dir.path().join("자료.zip");
    write_docs(&zip, &["처음.txt"]);
    let mut app = app_at(Location::zip(&zip, "문서"));

    std::fs::remove_file(&zip).unwrap();

    let folder = Location::file_system(dir.path());
    wait_until(&mut app, "지운 zip", |app| {
        app.active_tab().location() == &folder
    });
    assert_eq!(
        app.active_tab().relocation(),
        Some(&Relocation::ToParent {
            vanished: Location::zip(&zip, "문서"),
        }),
        "zip을 보던 위치가 사라져 옮겼다는 까닭"
    );
}
