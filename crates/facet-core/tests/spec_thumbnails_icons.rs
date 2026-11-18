// target: crates/facet-core/tests/spec_thumbnails_icons.rs
// AC: #325: 아이콘 키 규칙(폴더, .exe·.ico·.lnk·.url은 전체 경로, 그 밖은 소문자 확장자, 확장자 없음, 온라인 전용은 확장자 키)
// AC: #325: 같은 키·크기는 한 번만 얻고 얻지 못한 키도 다시 얻지 않음, 조회는 기다리지 않고 다른 스레드에서 얻으며 끝나면 깨우기·사건
// AC: #325: Windows에서 확장자·폴더·실행 파일 고유 아이콘을 요청 크기의 RGBA로 얻음, 목록 항목의 온라인 전용 표시(보통 파일은 꺼짐)
//! 파일 종류별 아이콘의 코어 몫을 확인한다: 키 규칙(`icons::icon_key`), 주입한 얻기 함수로 본 캐시의 동작
//! (`icons::IconCache::with_loader`: 한 번만 얻기·실패 기억·기다리지 않음·깨우기), 목록 항목의 온라인 전용 표시. 실제
//! 시스템 아이콘 얻기(`facet_win::icon`, `IconCache::new`)는 Windows 전용이고 나머지는 모든 플랫폼에서 돈다. 자세히 보기의
//! 그림은 사람이 확인한다.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant, SystemTime};

use facet_core::icons::{IconCache, IconImage, IconKey, PER_FILE_EXTENSIONS, icon_key};
use facet_core::tab::ReadOutcome;
use facet_core::window::App;
use facet_core::{Attributes, Item, Location};
use tempfile::TempDir;

/// CI 러너가 느려도 흔들리지 않게 넉넉히 기다린다. 결과가 오면 곧바로 끝난다.
const WAIT: Duration = Duration::from_secs(20);
/// 「다시 얻지 않음」을 볼 때 기다리는 시간.
const QUIET: Duration = Duration::from_millis(300);

fn item(name: &str, is_folder: bool, online_only: bool) -> Item {
    Item {
        name: OsString::from(name),
        is_folder,
        size: 10,
        modified: SystemTime::UNIX_EPOCH,
        attributes: Attributes {
            online_only,
            ..Attributes::default()
        },
    }
}

fn ext(e: &str) -> IconKey {
    IconKey::Extension(e.to_string())
}

/// 얻기 함수의 기록: 부른 키·크기와 부른 스레드.
#[derive(Default)]
struct Calls {
    keys: Mutex<Vec<(IconKey, u32)>>,
    threads: Mutex<Vec<ThreadId>>,
}

impl Calls {
    fn count(&self, key: &IconKey, size: u32) -> usize {
        self.keys
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, s)| k == key && *s == size)
            .count()
    }
}

fn picture(size: u32) -> IconImage {
    IconImage {
        width: size,
        height: size,
        rgba: vec![255; (size * size * 4) as usize],
    }
}

/// `bad` 확장자는 얻지 못하고, 그 밖은 `delay` 뒤에 그림을 준다.
fn make_cache(delay: Duration) -> (IconCache, Arc<Calls>) {
    let calls = Arc::new(Calls::default());
    let shared = Arc::clone(&calls);
    let cache = IconCache::with_loader(move |key: &IconKey, size: u32| {
        shared.keys.lock().unwrap().push((key.clone(), size));
        shared.threads.lock().unwrap().push(thread::current().id());
        thread::sleep(delay);
        if *key == ext("bad") {
            None
        } else {
            Some(picture(size))
        }
    });
    (cache, calls)
}

/// `key`·`size`를 얻었다는 사건이 올 때까지 기다린다.
fn wait_loaded(cache: &IconCache, key: &IconKey, size: u32) {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match cache.wait_event(left) {
            Some((k, s)) if k == *key && s == size => return,
            Some(_) => {}
            None => panic!("{key:?} {size}을 얻었다는 사건이 오지 않았다"),
        }
    }
}

#[test]
#[ignore = "#325"]
fn 아이콘_키는_폴더_파일별_확장자_확장자_없음으로_나뉜다() {
    assert_eq!(PER_FILE_EXTENSIONS, ["exe", "ico", "lnk", "url"]);
    let folder = Path::new(r"C:\폴더");
    assert_eq!(
        icon_key(folder, &item("사진", true, false)),
        IconKey::Folder
    );
    assert_eq!(
        icon_key(folder, &item("보고서.TXT", false, false)),
        ext("txt")
    );
    assert_eq!(
        icon_key(folder, &item("묶음.tar.gz", false, false)),
        ext("gz")
    );
    assert_eq!(
        icon_key(folder, &item("README", false, false)),
        IconKey::NoExtension
    );
    for name in [
        "setup.exe",
        "SETUP.EXE",
        "앱.ico",
        "바로 가기.lnk",
        "사이트.Url",
    ] {
        assert_eq!(
            icon_key(folder, &item(name, false, false)),
            IconKey::File(folder.join(name)),
            "{name}은 파일마다 다른 아이콘"
        );
    }
}

#[test]
#[ignore = "#325"]
fn 온라인_전용_항목은_파일을_읽지_않게_확장자_키다() {
    let folder = Path::new(r"C:\OneDrive");
    assert_eq!(
        icon_key(folder, &item("setup.exe", false, true)),
        ext("exe")
    );
    assert_eq!(
        icon_key(folder, &item("바로 가기.LNK", false, true)),
        ext("lnk")
    );
    assert_eq!(icon_key(folder, &item("사진.jpg", false, true)), ext("jpg"));
    assert_eq!(icon_key(folder, &item("폴더", true, true)), IconKey::Folder);
}

#[test]
#[ignore = "#325"]
fn 같은_키와_크기는_한_번만_얻는다() {
    let (cache, calls) = make_cache(Duration::from_millis(30));
    let folder = Path::new(r"C:\폴더");
    let txt = icon_key(folder, &item("a.txt", false, false));
    assert_eq!(
        txt,
        icon_key(folder, &item("B.TXT", false, false)),
        "대소문자만 다른 확장자는 같은 키"
    );

    assert!(cache.get(&txt, 16).is_none(), "처음에는 아직 없다");
    let cache = Arc::new(cache);
    let askers: Vec<_> = (0..4)
        .map(|_| {
            let cache = Arc::clone(&cache);
            let txt = txt.clone();
            thread::spawn(move || {
                for _ in 0..50 {
                    let _ = cache.get(&txt, 16);
                }
            })
        })
        .collect();
    for asker in askers {
        asker.join().unwrap();
    }
    wait_loaded(&cache, &txt, 16);
    let got = cache.get(&txt, 16).expect("얻은 뒤에는 캐시에서 준다");
    assert_eq!(*got, picture(16));
    thread::sleep(QUIET);
    assert_eq!(calls.count(&txt, 16), 1, "확장자마다 한 번");

    assert!(cache.get(&txt, 32).is_none(), "크기가 다르면 따로 얻는다");
    wait_loaded(&cache, &txt, 32);
    assert_eq!(cache.get(&txt, 32).as_deref(), Some(&picture(32)));
    assert_eq!(calls.count(&txt, 32), 1);
    assert_eq!(calls.count(&txt, 16), 1);
}

#[test]
#[ignore = "#325"]
fn 얻지_못한_키는_알리고_다시_얻지_않는다() {
    let (cache, calls) = make_cache(Duration::ZERO);
    let bad = ext("bad");
    assert!(cache.get(&bad, 16).is_none());
    wait_loaded(&cache, &bad, 16);
    for _ in 0..10 {
        assert!(
            cache.get(&bad, 16).is_none(),
            "얻지 못했으면 내장 아이콘(없음)"
        );
    }
    thread::sleep(QUIET);
    assert_eq!(calls.count(&bad, 16), 1, "실패도 기억한다");
}

#[test]
#[ignore = "#325"]
fn 조회는_기다리지_않고_얻기는_다른_스레드에서_하며_끝나면_깨운다() {
    let (cache, calls) = make_cache(Duration::from_millis(500));
    let woken = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&woken);
    cache.set_waker(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    let key = IconKey::File(PathBuf::from(r"C:\앱\setup.exe"));
    let started = Instant::now();
    assert!(cache.get(&key, 32).is_none());
    assert!(cache.get(&ext("zip"), 16).is_none());
    let took = started.elapsed();
    assert!(
        took < Duration::from_millis(100),
        "얻기를 기다리지 않는다: {took:?}"
    );

    let deadline = Instant::now() + WAIT;
    while woken.load(Ordering::SeqCst) == 0 || cache.get(&key, 32).is_none() {
        assert!(Instant::now() < deadline, "얻은 뒤 깨우기가 오지 않았다");
        thread::sleep(Duration::from_millis(5));
    }
    let loaded = cache.poll();
    assert!(loaded.contains(&(key.clone(), 32)), "사건: {loaded:?}");
    let me = thread::current().id();
    assert!(
        calls.threads.lock().unwrap().iter().all(|t| *t != me),
        "부른 스레드에서 얻지 않는다"
    );
}

#[test]
#[ignore = "#325"]
fn 실제_폴더의_보통_파일은_온라인_전용이_아니다() {
    assert!(!Attributes::default().online_only);
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("보통.txt"), b"x").unwrap();
    fs::create_dir(dir.path().join("하위")).unwrap();
    let mut app = App::new(Some(Location::file_system(dir.path())));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    let items: Vec<&Item> = app.active_tab().items().collect();
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|i| !i.attributes.online_only));
}

#[cfg(windows)]
mod windows {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use facet_core::icons::{IconCache, IconImage, IconKey};
    use facet_win::icon::{extension_icon, file_icon, folder_icon};

    fn assert_icon(icon: &IconImage, size: u32, what: &str) {
        assert_eq!((icon.width, icon.height), (size, size), "{what}: 요청 크기");
        assert_eq!(
            icon.rgba.len(),
            (size * size * 4) as usize,
            "{what}: RGBA 길이"
        );
        assert!(
            icon.rgba.as_chunks::<4>().0.iter().any(|p| p[3] > 0),
            "{what}: 보이는 화소가 있다"
        );
    }

    /// 시스템에 늘 있는 실행 파일 하나.
    fn system_program() -> Option<PathBuf> {
        let root = PathBuf::from(std::env::var_os("SystemRoot")?);
        [
            root.join("System32").join("notepad.exe"),
            root.join("regedit.exe"),
            root.join("explorer.exe"),
        ]
        .into_iter()
        .find(|p| p.is_file())
    }

    #[test]
    #[ignore = "#325"]
    fn 확장자와_폴더_아이콘을_요청_크기로_얻는다() {
        for size in [16, 32] {
            let txt = extension_icon("txt", size).expect("txt 아이콘");
            assert_icon(&txt, size, "txt");
            let folder = folder_icon(size).expect("폴더 아이콘");
            assert_icon(&folder, size, "폴더");
            assert_ne!(txt.rgba, folder.rgba, "파일 종류와 폴더는 다른 그림");
        }
    }

    #[test]
    #[ignore = "#325"]
    fn 실행_파일은_일반_exe_아이콘과_다른_고유_아이콘이다() {
        let Some(program) = system_program() else {
            println!("건너뜀: 시스템 실행 파일을 찾지 못했다");
            return;
        };
        let own = file_icon(&program, 32).expect("실행 파일 아이콘");
        assert_icon(&own, 32, "실행 파일");
        let generic = extension_icon("exe", 32).expect("exe 확장자 아이콘");
        assert_ne!(
            own.rgba,
            generic.rgba,
            "{}의 고유 아이콘",
            program.display()
        );
    }

    #[test]
    #[ignore = "#325"]
    fn 실제_캐시는_ui_밖에서_시스템_아이콘을_얻는다() {
        let cache = IconCache::new();
        let key = IconKey::Extension("txt".into());
        let deadline = Instant::now() + Duration::from_secs(20);
        let icon = loop {
            if let Some(icon) = cache.get(&key, 16) {
                break icon;
            }
            assert!(Instant::now() < deadline, "txt 아이콘을 얻지 못했다");
            cache.wait_event(Duration::from_millis(100));
        };
        assert_icon(&icon, 16, "캐시의 txt");
    }
}
