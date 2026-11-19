// target: crates/facet-core/tests/spec_sync_folders_instant.rs
// AC: #451: 즉시 검색 결과에서 인덱스 속성이 온라인 전용이면 온라인 전용, 고정이면 항상 유지, 동기화 폴더 밖의 보통 항목은 배지 없음
//! 즉시 검색 결과 위치의 배지(`Tab::sync_state`)를 같은 프로세스의 인덱서 파이프 서버(합성 인덱스, 임시 폴더 경로를
//! 닮음)로 확인한다. 인덱스가 준 속성 값만으로 가르므로 동기화 루트는 필요 없다. 관리자 권한이 필요 없고 Windows
//! 전용이다(파이프).
#![cfg(windows)]

use std::path::{Component, Path};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::indexer_client::{CONNECT_TIMEOUT, ClientOptions};
use facet_core::sync_status::SyncState;
use facet_core::tab::ReadOutcome;
use facet_core::window::App;
use facet_index::{ATTRIBUTE_DIRECTORY, Index, NewItem};
use facet_indexer::pipe::PipeServer;
use facet_indexer::sample::SampleBackend;
use facet_indexer::volumes::Volumes;
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(30);
const ROOT: u64 = 5;

const ARCHIVE: u32 = 0x20;
const REPARSE_POINT: u32 = 0x400;
const PINNED: u32 = 0x8_0000;
const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;

fn unique_pipe() -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        r"\\.\pipe\facet-core-sync-instant-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// `dir`와 같은 경로를 가진 합성 인덱스: `dir` 아래 이름과 속성 값이 주어진 파일들.
fn index_for(dir: &Path, files: &[(&str, u32)]) -> (String, Index) {
    let mut components = dir.components();
    let volume = match components.next() {
        Some(Component::Prefix(prefix)) => prefix.as_os_str().to_string_lossy().into_owned(),
        other => panic!("드라이브 경로가 아니다: {other:?}"),
    };
    let mut index = Index::new(&volume, ROOT);
    let mut next = 100u64;
    let mut parent = ROOT;
    for component in components {
        if let Component::Normal(name) = component {
            let number = next;
            next += 1;
            index
                .insert(NewItem {
                    number,
                    parent,
                    name: &name.to_string_lossy(),
                    size: 0,
                    modified: 133_000_000_000_000_000,
                    attributes: ATTRIBUTE_DIRECTORY,
                })
                .unwrap();
            parent = number;
        }
    }
    for (n, &(name, attributes)) in files.iter().enumerate() {
        index
            .insert(NewItem {
                number: 10_000 + n as u64,
                parent,
                name,
                size: 100,
                modified: 133_000_000_000_000_000,
                attributes,
            })
            .unwrap();
    }
    (volume, index)
}

#[test]
#[ignore = "#451"]
fn instant_results_badge_from_index_attributes() {
    let dir = TempDir::new().unwrap();
    let (volume, index) = index_for(
        dir.path(),
        &[
            (
                "보고서 온라인.txt",
                ARCHIVE | REPARSE_POINT | RECALL_ON_DATA_ACCESS,
            ),
            ("보고서 고정.txt", ARCHIVE | REPARSE_POINT | PINNED),
            ("보고서 보통.txt", ARCHIVE),
        ],
    );
    let backend = Arc::new(SampleBackend(Volumes::with_index(&volume, index)));
    let pipe = unique_pipe();
    let _server = PipeServer::start(&pipe, backend).unwrap();
    let mut app = App::new(Some(Location::file_system(dir.path())));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app.enable_indexer(ClientOptions {
        pipe,
        connect_timeout: CONNECT_TIMEOUT,
        reconnect_interval: Duration::from_millis(100),
    });

    app.set_instant_query("보고서");
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll();
        let tab = app.active_tab();
        let settled = tab
            .instant()
            .is_some_and(|s| !s.loading && s.total.is_some());
        if settled && tab.len() == 3 {
            break;
        }
        assert!(Instant::now() < deadline, "즉시 검색 결과가 오지 않았다");
        std::thread::sleep(Duration::from_millis(5));
    }
    let tab = app.active_tab();
    let state_of = |name: &str| {
        let index = tab
            .items()
            .position(|item| item.name == name)
            .unwrap_or_else(|| panic!("{name}이 결과에 없다"));
        tab.sync_state(index)
    };
    assert_eq!(state_of("보고서 온라인.txt"), Some(SyncState::OnlineOnly));
    assert_eq!(state_of("보고서 고정.txt"), Some(SyncState::AlwaysKeep));
    assert_eq!(
        state_of("보고서 보통.txt"),
        None,
        "동기화 폴더가 아닌 곳의 보통 항목은 배지가 없다"
    );
}
