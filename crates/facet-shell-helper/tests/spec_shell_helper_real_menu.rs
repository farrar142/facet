// target: crates/facet-shell-helper/tests/spec_shell_helper_real_menu.rs
// AC: #323: 파일 하나·여러 개의 셸 메뉴에 켜진 명령과 「보내기」 하위 메뉴, 폴더 배경의 셸 메뉴에 켜진 명령, 없는 파일은 실패로 끝나고 도우미는 살아 있음
//! 시험용 모드 없이 띄운 실제 셸 확장 도우미가 임시 폴더의 파일·폴더 배경에 대해 실제 셸 메뉴(`IContextMenu`)를 만들어
//! 항목 트리로 돌려주는지 확인한다. 러너마다 설치된 확장이 달라 목록 전체는 견주지 않고 Windows가 기본으로 넣는 항목
//! (「보내기」 하위 메뉴)만 본다. 항목 실행은 창이 뜨므로 자동으로 하지 않는다. Windows에서만 돈다.
#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use facet_core::shell_helper::{
    HelperEvent, HelperOptions, Reply, RequestError, RequestId, ShellHelper,
};
use facet_shell_protocol::{MenuItem, MenuTarget};
use tempfile::TempDir;

/// 셸 초기화가 처음에는 느릴 수 있어 넉넉히 기다린다. 결과가 오면 곧바로 끝난다.
const WAIT: Duration = Duration::from_secs(60);

/// 실제 셸 모드(시험용 인자 없음). 죽은 기록은 남기지 않는다.
fn real_helper() -> ShellHelper {
    let mut options = HelperOptions::new(PathBuf::from(env!("CARGO_BIN_EXE_facet-shell-helper")));
    options.crash_log = None;
    options.reply_timeout = Duration::from_secs(30);
    options.stall_timeout = Duration::from_secs(50);
    ShellHelper::start(options)
}

fn file(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, b"facet").unwrap();
    path
}

fn reply_for(helper: &ShellHelper, id: RequestId) -> Result<Reply, RequestError> {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "{id:?}의 결과가 오지 않았다");
        if let Some(HelperEvent::Reply { id: got, result }) = helper.wait_event(left)
            && got == id
        {
            return result;
        }
    }
}

fn menu(helper: &ShellHelper, target: MenuTarget) -> Vec<MenuItem> {
    let id = helper.menu(target);
    match reply_for(helper, id) {
        Ok(Reply::Menu(items)) => items,
        other => panic!("셸 메뉴가 와야 한다: {other:?}"),
    }
}

/// 가속 글자 표시(`&`)를 걷고 소문자로.
fn plain(text: &str) -> String {
    text.replace('&', "").trim().to_lowercase()
}

fn has_enabled_command(items: &[MenuItem]) -> bool {
    items.iter().any(|item| match item {
        MenuItem::Command(command) => command.enabled && !command.text.trim().is_empty(),
        MenuItem::Submenu(sub) => has_enabled_command(&sub.items),
        MenuItem::Separator => false,
    })
}

/// 「보내기」(Send to) 하위 메뉴가 항목을 담고 있는가.
fn has_send_to(items: &[MenuItem]) -> bool {
    items.iter().any(|item| match item {
        MenuItem::Submenu(sub) => {
            let text = plain(&sub.text);
            (text.contains("send to") || text.contains("보내기")) && !sub.items.is_empty()
        }
        _ => false,
    })
}

fn texts(items: &[MenuItem]) -> Vec<String> {
    items
        .iter()
        .map(|item| match item {
            MenuItem::Command(command) => command.text.clone(),
            MenuItem::Submenu(sub) => format!("{} ▶", sub.text),
            MenuItem::Separator => "--".into(),
        })
        .collect()
}

fn is_alive(pid: u32) -> bool {
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .expect("tasklist");
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

#[test]
fn a_file_menu_has_an_enabled_command_and_the_send_to_submenu() {
    let dir = TempDir::new().unwrap();
    let a = file(dir.path(), "보고서.txt");
    let helper = real_helper();

    let items = menu(&helper, MenuTarget::Items(vec![a]));
    assert!(has_enabled_command(&items), "{:?}", texts(&items));
    assert!(
        has_send_to(&items),
        "「보내기」가 없다: {:?}",
        texts(&items)
    );
}

#[test]
fn several_files_get_the_send_to_submenu_too() {
    let dir = TempDir::new().unwrap();
    let a = file(dir.path(), "a.txt");
    let b = file(dir.path(), "b.txt");
    let c = file(dir.path(), "이모지 😀.txt");
    let helper = real_helper();

    let items = menu(&helper, MenuTarget::Items(vec![a, b, c]));
    assert!(has_enabled_command(&items), "{:?}", texts(&items));
    assert!(
        has_send_to(&items),
        "「보내기」가 없다: {:?}",
        texts(&items)
    );
}

#[test]
fn the_folder_background_menu_has_an_enabled_command() {
    let dir = TempDir::new().unwrap();
    file(dir.path(), "a.txt");
    let helper = real_helper();

    let items = menu(&helper, MenuTarget::Background(dir.path().to_path_buf()));
    assert!(has_enabled_command(&items), "{:?}", texts(&items));
}

#[test]
fn a_missing_file_fails_without_ending_the_helper() {
    let dir = TempDir::new().unwrap();
    let a = file(dir.path(), "a.txt");
    let helper = real_helper();
    menu(&helper, MenuTarget::Items(vec![a.clone()]));
    let pid = helper.process_id().unwrap();

    let id = helper.menu(MenuTarget::Items(vec![dir.path().join("없는 파일.txt")]));
    let result = reply_for(&helper, id);
    assert!(
        matches!(result, Err(RequestError::Failed(_))),
        "없는 파일은 실패로 끝난다: {result:?}"
    );
    assert!(is_alive(pid), "도우미는 죽지 않는다");

    let items = menu(&helper, MenuTarget::Items(vec![a]));
    assert!(has_enabled_command(&items));
    assert_eq!(helper.process_id(), Some(pid), "같은 도우미가 답한다");
}
