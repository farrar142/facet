// target: crates/facet-core/tests/spec_sync_folders_background.rs
// AC: #452: 실시간 탐색은 펼쳐지지 않은 온라인 폴더로 내려가지 않고(그 폴더 자신은 나옴) 목록·내용 요청 0, 진행·요약의 수 1
// AC: #452: 내용 검색은 이 장치에 있는 파일만 일치, 온라인 전용 건너뛴 수, 내용 요청 0
// AC: #452: 썸네일은 이 장치에 있는 이미지만 오고 온라인 전용 이미지는 내용 요청 0, 미리보기는 「온라인 전용」이고 내용 요청 0
// AC: #452: 목록에서 읽은 온라인 전용 .exe의 아이콘 키는 파일 고유 키가 아님
//! 배경 동작(실시간 탐색·내용 검색·썸네일·미리보기·아이콘)이 내용 읽기 관문을 지나 온라인 전용 항목을 몰래 내려받지
//! 않는지, 시험용 동기화 공급자(`facet_win::cloud_files::testing`)가 센 내용 요청(FETCH_DATA)·목록 요청
//! (FETCH_PLACEHOLDERS) 수로 확인한다. Windows에서만 돈다. 동기화 루트를 등록할 수 없는 환경이면 건너뛰었다고
//! 출력하고, 환경 변수 `FACET_REQUIRE_SYNC_ROOT`가 있으면 그 경우를 실패로 본다.
#![cfg(windows)]

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::command::CommandId;
use facet_core::content_search::{ContentEvent, ContentRequest, ContentSearch};
use facet_core::icons::{IconKey, icon_key};
use facet_core::live_search::{LiveSearch, SearchEvent, SearchRequest};
use facet_core::preview::{PreviewBody, PreviewContent, PreviewProblem};
use facet_core::tab::ReadOutcome;
use facet_core::view::{GridSize, GridSpans};
use facet_core::window::{App, Execution, ThumbnailOptions, Update};
use facet_win::cloud_files::testing::TestSyncRoot;
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(30);
/// 결과가 「오지 않음」을 볼 때 기다리는 시간.
const QUIET: Duration = Duration::from_millis(800);
const ENV_REQUIRE: &str = "FACET_REQUIRE_SYNC_ROOT";

fn sync_root(dir: &Path, test: &str) -> Option<TestSyncRoot> {
    match TestSyncRoot::register(dir) {
        Ok(root) => Some(root),
        Err(error) => {
            assert!(
                std::env::var_os(ENV_REQUIRE).is_none(),
                "{ENV_REQUIRE}가 있는데 동기화 루트를 등록하지 못했다: {error}"
            );
            println!("{test}: 동기화 루트를 등록할 수 없어 건너뜀: {error}");
            None
        }
    }
}

// ---- 한 가지 색 PNG(압축하지 않는 deflate)

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in bytes {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn png(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
    let row: Vec<u8> = std::iter::once(0)
        .chain(color.repeat(width as usize))
        .collect();
    let raw = row.repeat(height as usize);
    let mut zlib = vec![0x78, 0x01];
    let chunks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        zlib.push(u8::from(i + 1 == chunks.len()));
        let len = chunk.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(chunk);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    png_chunk(&mut out, b"IHDR", &ihdr);
    png_chunk(&mut out, b"IDAT", &zlib);
    png_chunk(&mut out, b"IEND", &[]);
    out
}

// ---- 앱

fn app_at(path: &Path) -> App {
    let mut app = App::new(Some(Location::file_system(path)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

fn index_of(app: &App, name: &str) -> usize {
    app.active_tab()
        .items()
        .position(|item| item.name == name)
        .unwrap_or_else(|| panic!("{name}이 목록에 없다"))
}

#[test]
fn live_search_does_not_descend_into_unexpanded_online_folders_and_counts_them() {
    let dir = TempDir::new().unwrap();
    let Some(root) = sync_root(dir.path(), "실시간 탐색") else {
        return;
    };
    root.add_online_file("보고서 온라인.txt", b"online");
    root.add_online_folder(
        "보고서 폴더",
        &[("보고서 안 1.txt", b"1"), ("보고서 안 2.txt", b"2")],
    );
    fs::create_dir(dir.path().join("로컬 폴더")).unwrap();
    fs::write(dir.path().join("로컬 폴더").join("보고서 로컬.txt"), b"l").unwrap();

    let request = SearchRequest::name(Location::file_system(dir.path()), "보고서", false);
    let mut search = LiveSearch::start(request).expect("탐색을 시작한다");
    let deadline = Instant::now() + WAIT;
    let mut names = Vec::new();
    let mut last_progress = None;
    let summary = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match search.wait_event(left).expect("탐색이 시간 안에 끝난다") {
            SearchEvent::Batch { items, progress } => {
                names.extend(
                    items
                        .iter()
                        .map(|f| f.name().to_string_lossy().into_owned()),
                );
                last_progress = Some(progress);
            }
            SearchEvent::Finished(summary) => break summary,
        }
    };
    names.sort();
    assert_eq!(
        names,
        vec![
            "보고서 로컬.txt".to_string(),
            "보고서 온라인.txt".to_string(),
            "보고서 폴더".to_string(),
        ],
        "펼쳐지지 않은 온라인 폴더 안의 항목은 나오지 않는다"
    );
    assert_eq!(summary.online_folders_skipped, 1);
    assert_eq!(
        last_progress.expect("묶음이 왔다").online_folders_skipped,
        1
    );
    assert_eq!(root.fetch_placeholders_count(), 0);
    assert_eq!(root.fetch_data_count(), 0);
}

#[test]
fn content_search_skips_and_counts_online_only_files() {
    let dir = TempDir::new().unwrap();
    let Some(root) = sync_root(dir.path(), "내용 검색") else {
        return;
    };
    root.add_online_file("온라인 1.txt", "찾는글자 하나".as_bytes());
    root.add_online_file("온라인 2.txt", "찾는글자 둘".as_bytes());
    fs::write(dir.path().join("로컬.txt"), "여기에도 찾는글자".as_bytes()).unwrap();

    let request = ContentRequest::new(Location::file_system(dir.path()), "찾는글자");
    let mut search = ContentSearch::start(request).expect("내용 검색을 시작한다");
    let deadline = Instant::now() + WAIT;
    let mut names = Vec::new();
    let summary = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match search.wait_event(left).expect("검색이 시간 안에 끝난다") {
            ContentEvent::Batch { items, .. } => {
                names.extend(
                    items
                        .iter()
                        .map(|f| f.item.name.to_string_lossy().into_owned()),
                );
            }
            ContentEvent::Finished(summary) => break summary,
        }
    };
    assert_eq!(names, vec!["로컬.txt".to_string()]);
    assert_eq!(summary.progress.skipped.online_only, 2);
    assert_eq!(root.fetch_data_count(), 0);
}

#[test]
fn thumbnails_do_not_download_online_only_images() {
    let dir = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let Some(root) = sync_root(dir.path(), "썸네일") else {
        return;
    };
    root.add_online_file("온라인.png", &png(300, 200, [200, 40, 40, 255]));
    fs::write(
        dir.path().join("로컬.png"),
        png(300, 200, [40, 40, 200, 255]),
    )
    .unwrap();

    let mut app = app_at(dir.path());
    app.enable_thumbnails(ThumbnailOptions::new(cache.path()));
    assert_eq!(
        app.execute(CommandId::GridView(GridSize::Medium)),
        Execution::Done
    );
    let len = app.active_tab().len();
    let tab = app.active_tab_id();
    app.request_thumbnails(
        tab,
        GridSpans {
            visible: 0..len,
            nearby: 0..len,
        },
    );
    let mut got = Vec::new();
    let deadline = Instant::now() + WAIT;
    let mut quiet_until = None;
    loop {
        let now = Instant::now();
        if quiet_until.is_some_and(|t| now >= t) {
            break;
        }
        assert!(now < deadline, "로컬 이미지의 썸네일이 오지 않았다");
        if let Some((_, Update::Thumbnail { path, result, .. })) =
            app.wait_update(Duration::from_millis(50))
        {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if name == "로컬.png" && result.is_ok() && quiet_until.is_none() {
                // 로컬 이미지가 온 뒤로도 잠시 더 받아 본다.
                quiet_until = Some(Instant::now() + QUIET);
            }
            got.push((name, result.is_ok()));
        }
    }
    assert!(got.contains(&("로컬.png".to_string(), true)));
    assert!(
        !got.iter().any(|(name, ok)| name == "온라인.png" && *ok),
        "온라인 전용 이미지의 썸네일을 만들지 않는다: {got:?}"
    );
    assert_eq!(root.fetch_data_count(), 0);
}

#[test]
fn preview_refuses_online_only_items_without_downloading() {
    let dir = TempDir::new().unwrap();
    let Some(root) = sync_root(dir.path(), "미리보기") else {
        return;
    };
    root.add_online_file("온라인.txt", "미리 보면 안 되는 글".as_bytes());
    fs::write(dir.path().join("로컬.txt"), "로컬 글").unwrap();

    let mut app = app_at(dir.path());
    if !app.active_window().preview_pane().visible {
        assert_eq!(app.execute(CommandId::TogglePreview), Execution::Done);
    }
    let index = index_of(&app, "온라인.txt");
    app.active_tab_mut().select_only(index);
    app.sync_preview();
    let window = app.active_window_id();
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll();
        if let Some(PreviewContent::Item { path, body, .. }) = app.preview(window)
            && path.file_name().is_some_and(|n| n == "온라인.txt")
            && *body != PreviewBody::Loading
        {
            assert_eq!(*body, PreviewBody::Unavailable(PreviewProblem::OnlineOnly));
            break;
        }
        assert!(Instant::now() < deadline, "미리보기 내용이 정해지지 않았다");
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(QUIET);
    app.poll();
    assert_eq!(root.fetch_data_count(), 0);
}

#[test]
fn online_only_exe_gets_no_per_file_icon() {
    let dir = TempDir::new().unwrap();
    let Some(root) = sync_root(dir.path(), "아이콘") else {
        return;
    };
    root.add_online_file("프로그램.exe", b"MZ");
    let app = app_at(dir.path());
    let index = index_of(&app, "프로그램.exe");
    let item = app.active_tab().get(index).expect("항목").clone();
    assert_eq!(
        icon_key(dir.path(), &item),
        IconKey::Extension("exe".to_string())
    );
    assert_eq!(root.fetch_data_count(), 0);
}
