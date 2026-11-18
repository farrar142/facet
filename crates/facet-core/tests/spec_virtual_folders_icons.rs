// target: crates/facet-core/tests/spec_virtual_folders_icons.rs
// AC: #358: 같은 아이콘은 한 번만 얻는다, 내 PC의 드라이브와 휴지통 항목에 셸 아이콘(키가 정해지고 얻어짐)
//! 가상 폴더 항목의 아이콘 키를 공개 인터페이스(`Tab::icon_key`, `icons::IconCache`)에서 확인한다: 휴지통에서 같은
//! 형식의 항목들은 아이콘 키 하나를 나눠 써 얻기 함수가 한 번만 불리고 형식이 다르면 키가 다른지, 내 PC의 시스템
//! 드라이브는 보통 폴더 아이콘이 아닌 제 키를 갖고 시스템 아이콘 캐시가 그 그림을 얻는지, 실제 폴더 목록의 키는 지금
//! 규칙(`icons::icon_key`) 그대로인지. 임시 폴더의 파일을 S4의 휴지통 삭제로 넣고 끝날 때(실패해도) 휴지통에서
//! 지운다. 휴지통을 쓸 수 없는 계정이면 건너뛰었다고 출력한다. 화면의 그림은 사람이 확인한다. Windows 전용.
#![cfg(windows)]

#[path = "common/bin.rs"]
mod bin;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use facet_core::command::CommandId;
use facet_core::icons::{IconCache, IconImage, IconKey, icon_key};
use facet_core::job::{self, JobId, JobQuestion, JobState, NotRecyclableAnswer};
use facet_core::tab::ReadOutcome;
use facet_core::window::{App, Execution};
use facet_core::{Location, VirtualFolder};
use facet_win::recycle::RecycledItem;
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(60);
/// 「다시 얻지 않음」을 볼 때 기다리는 시간.
const QUIET: Duration = Duration::from_millis(300);
/// 아이콘 크기(픽셀).
const SIZE: u32 = 16;

/// 테스트가 휴지통에 넣은 항목. 버릴 때 휴지통에 남아 있으면 지운다.
#[derive(Default)]
struct Bin(Vec<RecycledItem>);

impl Drop for Bin {
    fn drop(&mut self) {
        bin::purge(&self.0);
    }
}

fn app_at(location: Location) -> App {
    let mut app = App::new(Some(location));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

/// 작업이 끝나거나 질문할 때까지 받는다. 질문이면 그 질문.
fn wait_question(app: &mut App, job: JobId) -> Option<JobQuestion> {
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll_jobs();
        app.poll();
        let entry = app.work_area().entry(job).expect("작업 영역의 작업");
        if let Some(question) = &entry.question {
            return Some(question.clone());
        }
        if entry.summary.is_some() {
            return None;
        }
        assert!(Instant::now() < deadline, "작업이 끝나지 않았다");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// 파일들을 휴지통 삭제하고 끝날 때까지 기다린다. 이 계정이 휴지통을 쓸 수 없으면 건너뛴다고 출력하고 `false`.
fn recycle(app: &mut App, sources: Vec<PathBuf>, bin: &mut Bin, test: &str) -> bool {
    if !sources.iter().all(|source| job::has_recycle_bin(source)) {
        println!("건너뜀({test}): 휴지통이 없는 볼륨입니다");
        return false;
    }
    let job = app.recycle(sources).expect("작업을 시작한다");
    match wait_question(app, job) {
        None => {}
        Some(JobQuestion::NotRecyclable(_)) => {
            app.answer_not_recyclable(job, NotRecyclableAnswer::CancelJob)
                .unwrap();
            wait_question(app, job);
            println!("건너뜀({test}): 이 계정은 휴지통을 쓸 수 없습니다");
            return false;
        }
        Some(other) => panic!("묻지 않아야 한다: {other:?}"),
    }
    let entry = app.work_area().entry(job).unwrap();
    let summary = entry.summary.clone().unwrap();
    bin.0.extend(summary.recycled.iter().cloned());
    assert_eq!(entry.state, JobState::Finished, "{summary:?}");
    true
}

/// 활성 탭이 `expected`에 닿아 목록을 다 읽을 때까지 결과를 받는다.
fn settle(app: &mut App, expected: &Location) {
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll();
        let tab = app.active_tab();
        if tab.pending_location().is_none() {
            if let Some(error) = tab.error() {
                panic!("읽기가 실패했다: {error}");
            }
            if tab.location() == expected && tab.loading().is_none() {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "{expected}에 닿지 못했다: 지금 {}",
            tab.location()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// 휴지통 목록에서 원래 경로가 `original`인 항목의 번호.
fn recycled_index(app: &App, original: &Path) -> usize {
    let tab = app.active_tab();
    (0..tab.len())
        .find(|&i| {
            tab.recycled(i)
                .is_some_and(|entry| entry.original == original)
        })
        .unwrap_or_else(|| panic!("휴지통에 {}가 없다", original.display()))
}

/// 얻기 함수가 불린 키들을 적는 캐시. 그림은 늘 준다.
fn counting_cache() -> (IconCache, Arc<Mutex<Vec<IconKey>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let shared = Arc::clone(&calls);
    let cache = IconCache::with_loader(move |key: &IconKey, size: u32| {
        shared.lock().unwrap().push(key.clone());
        Some(IconImage {
            width: size,
            height: size,
            rgba: vec![255; (size * size * 4) as usize],
        })
    });
    (cache, calls)
}

/// `key`의 그림이 캐시에 들 때까지 기다린다.
fn wait_icon(cache: &IconCache, key: &IconKey) -> Arc<IconImage> {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(icon) = cache.get(key, SIZE) {
            return icon;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "{key:?}의 아이콘을 얻지 못했다");
        cache.wait_event(left.min(Duration::from_millis(100)));
    }
}

#[test]
#[ignore = "#358"]
fn recycled_items_of_one_type_share_one_icon() {
    let dir = TempDir::new().unwrap();
    let first = dir.path().join("첫째.txt");
    let second = dir.path().join("둘째.TXT");
    let picture = dir.path().join("사진.jpg");
    for path in [&first, &second, &picture] {
        fs::write(path, "내용").unwrap();
    }
    let mut app = app_at(Location::file_system(dir.path()));
    let mut bin = Bin::default();
    let sources = vec![first.clone(), second.clone(), picture.clone()];
    if !recycle(&mut app, sources, &mut bin, "같은 아이콘") {
        return;
    }

    assert_eq!(app.execute(CommandId::OpenRecycleBin), Execution::Done);
    settle(&mut app, &Location::recycle_bin());
    let tab = app.active_tab();
    let key_of = |original: &Path| {
        tab.icon_key(recycled_index(&app, original))
            .unwrap_or_else(|| panic!("{}의 아이콘 키가 없다", original.display()))
    };
    let first_key = key_of(&first);
    let second_key = key_of(&second);
    let picture_key = key_of(&picture);
    assert_eq!(first_key, second_key, "같은 형식은 아이콘 키 하나");
    assert_ne!(first_key, picture_key, "다른 형식은 다른 아이콘");

    let (cache, calls) = counting_cache();
    for key in [&first_key, &second_key, &picture_key] {
        cache.get(key, SIZE);
    }
    wait_icon(&cache, &first_key);
    wait_icon(&cache, &second_key);
    wait_icon(&cache, &picture_key);
    std::thread::sleep(QUIET);
    let calls = calls.lock().unwrap();
    let times = |key: &IconKey| calls.iter().filter(|k| *k == key).count();
    assert_eq!(
        times(&first_key),
        1,
        "같은 아이콘은 한 번만 얻는다: {calls:?}"
    );
    assert_eq!(times(&picture_key), 1, "{calls:?}");
}

#[test]
#[ignore = "#358"]
fn the_system_drive_has_its_own_shell_icon() {
    let dir = TempDir::new().unwrap();
    let mut app = app_at(Location::file_system(dir.path()));
    assert_eq!(app.execute(CommandId::OpenThisPc), Execution::Done);
    settle(&mut app, &Location::this_pc());

    let tab = app.active_tab();
    assert_eq!(tab.location().virtual_folder(), Some(VirtualFolder::ThisPc));
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_owned());
    let root = PathBuf::from(format!("{drive}\\"));
    let index = (0..tab.len())
        .find(|&i| tab.item_path(i).as_deref() == Some(root.as_path()))
        .unwrap_or_else(|| panic!("내 PC에 {} 항목이 없다", root.display()));
    let key = tab.icon_key(index).expect("드라이브의 아이콘 키");
    assert_ne!(
        key,
        IconKey::Folder,
        "드라이브는 보통 폴더 아이콘이 아니라 셸이 주는 드라이브 아이콘이다"
    );

    let cache = IconCache::new();
    let icon = wait_icon(&cache, &key);
    assert_eq!((icon.width, icon.height), (SIZE, SIZE));
    assert_eq!(icon.rgba.len(), (SIZE * SIZE * 4) as usize);
}

#[test]
#[ignore = "#358"]
fn items_of_a_real_folder_keep_their_icon_keys() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("문서.txt"), "내용").unwrap();
    fs::write(dir.path().join("도구.exe"), "내용").unwrap();
    fs::create_dir(dir.path().join("하위")).unwrap();
    let app = app_at(Location::file_system(dir.path()));

    let tab = app.active_tab();
    assert_eq!(tab.len(), 3);
    for index in 0..tab.len() {
        let item = tab.get(index).unwrap();
        let folder = tab.folder_of(index).unwrap();
        assert_eq!(
            tab.icon_key(index),
            Some(icon_key(folder, item)),
            "{:?}",
            item.name
        );
    }
    assert_eq!(tab.icon_key(tab.len()), None, "목록 밖 번호");
}
