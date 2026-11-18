//! 셸 공급자의 목록 흘려 받기를 가짜 셸 폴더(`shell_provider::fake`)로 확인한다: 묶음마다 읽은 수 알림, 탭의
//! `loading`, 다른 곳으로 가면 열거를 그만둠, 세션 왕복. 셸을 부르지 않는다.

use std::ffi::OsStr;
use std::sync::{Arc, Mutex};

use facet_core::details::Sort;
use facet_core::session::Session;
use facet_core::shell_provider::{LIST_BATCH, ShellEntry, fake};
use facet_core::tab::{Navigation, ReadOutcome, Tab};
use facet_core::{Location, VirtualFolder};

fn entries(count: usize) -> Vec<ShellEntry> {
    (0..count)
        .map(|i| ShellEntry {
            name: format!("항목 {i:05}"),
            path: (i == 0).then(std::env::temp_dir),
            parsing_name: format!(r"\\?\fake#{i}"),
            is_folder: true,
            id: vec![4, 0, (i % 256) as u8, (i / 256) as u8, 0, 0],
            size: None,
            modified: None,
        })
        .collect()
}

fn home_tab() -> Tab {
    Tab::new(
        Location::file_system(std::env::temp_dir()),
        Sort::default(),
        false,
    )
}

#[test]
fn 묶음마다_읽은_수를_알리고_다_읽으면_읽는_중이_아니다() {
    let count = LIST_BATCH * 2 + 10;
    let folder = fake::serve(entries(count));
    let mut tab = home_tab();
    assert_eq!(tab.loading(), None);
    let request = tab
        .navigate(Navigation::Go(folder.location().clone()))
        .unwrap();
    assert_eq!(
        tab.loading(),
        Some(0),
        "가상 폴더로 가는 읽기는 읽는 중이다"
    );

    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let result = request.run_streaming(|_| {}, move |n| sink.lock().unwrap().push(n));
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen, [LIST_BATCH, LIST_BATCH * 2, count]);

    assert!(tab.note_loading(request.id(), LIST_BATCH));
    assert_eq!(tab.loading(), Some(LIST_BATCH));
    assert!(
        !tab.note_loading(request.id(), LIST_BATCH),
        "같은 수는 바뀐 것이 아니다"
    );

    assert_eq!(tab.finish(result), ReadOutcome::Committed);
    assert_eq!(tab.loading(), None);
    assert!(
        !tab.note_loading(request.id(), count),
        "다 읽은 뒤의 수는 늦은 것이다"
    );
    assert_eq!(tab.len(), count);
    assert_eq!(tab.location().virtual_folder(), Some(VirtualFolder::Device));

    // 실제 경로가 있는 항목만 경로를 준다.
    let first = (0..tab.len())
        .find(|&i| tab.get(i).unwrap().name == OsStr::new("항목 00000"))
        .unwrap();
    assert_eq!(tab.item_path(first), Some(std::env::temp_dir()));
    let second = (0..tab.len())
        .find(|&i| tab.get(i).unwrap().name == OsStr::new("항목 00001"))
        .unwrap();
    assert_eq!(tab.item_path(second), None);
}

#[test]
fn 실제_폴더로_가는_읽기는_읽는_중_수를_내지_않는다() {
    let mut tab = home_tab();
    let request = tab.navigate(Navigation::Refresh).unwrap();
    assert_eq!(tab.loading(), None);
    let calls = Arc::new(Mutex::new(0));
    let sink = Arc::clone(&calls);
    let result = request.run_streaming(|_| {}, move |_| *sink.lock().unwrap() += 1);
    assert_eq!(*calls.lock().unwrap(), 0);
    assert_eq!(tab.finish(result), ReadOutcome::Committed);
}

#[test]
fn 다른_곳으로_가면_열거를_그만둔다() {
    let folder = fake::serve(entries(LIST_BATCH * 4));
    let mut tab = home_tab();
    let first = tab
        .navigate(Navigation::Go(folder.location().clone()))
        .unwrap();
    // 다른 곳으로 가면 앞 요청은 버려진다.
    let _second = tab.navigate(Navigation::Refresh).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let result = first.run_streaming(|_| {}, move |n| sink.lock().unwrap().push(n));
    assert_eq!(*seen.lock().unwrap(), [LIST_BATCH], "첫 묶음 뒤에 그만둔다");
    assert_eq!(tab.finish(result), ReadOutcome::Stale);
}

#[test]
fn 내_pc_탭은_세션에_가상_폴더로_남고_깨진_식별자는_홈이다() {
    let json = r#"{"version":1,"active_window":0,"windows":[{"active_panel":0,"split_ratio":0.5,"panels":[{"active_tab":0,"tabs":[{"location":{"shell":{"folder":"this_pc","id":"14001f50e04fd020ea3a6910a2d808002b30309d0000"}},"sort":{"column":"name","direction":"ascending"}},{"location":{"shell":{"folder":"device","id":"zz"}},"sort":{"column":"name","direction":"ascending"}}]}]}]}"#;
    let session = Session::from_json(json).expect("세션을 읽는다");
    let tabs = &session.windows[0].panels[0].tabs;
    assert_eq!(
        tabs[0].location.virtual_folder(),
        Some(VirtualFolder::ThisPc)
    );
    assert_eq!(tabs[0].location, Location::this_pc());
    assert_eq!(tabs[1].location.virtual_folder(), None);

    let back = Session::from_json(&session.to_json()).unwrap();
    assert_eq!(back.windows[0].panels[0].tabs[0].location, tabs[0].location);
}

#[test]
fn 주소창의_내_pc_이름은_대소문자를_가리지_않는다() {
    for text in [
        "내 PC",
        "내 pc",
        "  shell:mycomputerfolder ",
        "\"shell:MyComputerFolder\"",
    ] {
        let mut tab = home_tab();
        assert!(tab.go_to_text(text).is_some(), "{text}");
        assert_eq!(tab.pending_location(), Some(&Location::this_pc()), "{text}");
    }
}
