// target: crates/facet-core/tests/spec_thumbnails_cache.rs
// AC: #328: 넣은 것을 같은 키로 꺼냄(새로 연 캐시에서도), 키의 한 부분이라도 다르면 못 꺼냄, 압축해 저장
// AC: #328: 정리는 상한 아래가 될 때까지 가장 오래 쓰지 않은 것부터, 메모리 상한, 깨진 캐시 파일은 없는 것으로 보고 지움
// AC: #328: 비우기 뒤 아무것도 못 꺼내고 디스크 크기 0, 기본 위치 %LOCALAPPDATA%\facet\thumbs(Windows), 여러 스레드가 함께 씀
//! 썸네일 캐시(`thumbnail_cache::ThumbnailCache`)를 임시 폴더로 확인한다. 썸네일은 손으로 만든 RGBA 그림이고, 디스크
//! 배치는 구현이 정하므로 시험은 캐시 폴더 안의 파일을 이름으로 찾지 않고 모든 파일을 훑는다. 기본 위치만 Windows 전용,
//! 나머지는 모든 플랫폼에서 돈다.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

use facet_core::thumbnail::{Thumbnail, ThumbnailKey};
use facet_core::thumbnail_cache::{
    CacheLimits, DEFAULT_DISK_LIMIT, DEFAULT_MEMORY_LIMIT, ThumbnailCache,
};
use tempfile::TempDir;

/// 정리가 「가장 오래 쓰지 않은」 차례를 가를 수 있게 쓰기 사이를 띄운다.
const GAP: Duration = Duration::from_millis(50);

fn key(name: &str) -> ThumbnailKey {
    ThumbnailKey {
        path: PathBuf::from(format!(r"C:\사진\{name}")),
        side: 96,
        modified: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        file_size: 1234,
    }
}

fn solid(width: u32, height: u32, color: [u8; 4]) -> Arc<Thumbnail> {
    Arc::new(Thumbnail {
        width,
        height,
        rgba: color.repeat((width * height) as usize),
    })
}

fn no_memory() -> CacheLimits {
    CacheLimits {
        disk_bytes: DEFAULT_DISK_LIMIT,
        memory_bytes: 0,
    }
}

fn open(dir: &Path, limits: CacheLimits) -> ThumbnailCache {
    ThumbnailCache::open(dir, limits).expect("캐시를 연다")
}

/// 캐시 폴더 안의 모든 파일(하위 폴더 포함).
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(folder) = stack.pop() {
        for entry in fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

#[test]
#[ignore = "#328"]
fn 넣은_썸네일을_같은_키로_꺼낸다() {
    let root = TempDir::new().unwrap();
    let cache = open(&root.path().join("thumbs"), CacheLimits::default());
    let picture = solid(96, 72, [200, 40, 40, 255]);
    cache.put(&key("a.jpg"), picture.clone()).unwrap();
    let got = cache.get(&key("a.jpg")).expect("넣은 것을 꺼낸다");
    assert_eq!(*got, *picture);
    assert!(cache.get(&key("b.jpg")).is_none(), "넣지 않은 키");
}

#[test]
#[ignore = "#328"]
fn 새로_연_캐시도_디스크에서_꺼낸다() {
    let root = TempDir::new().unwrap();
    let dir = root.path().join("thumbs");
    let picture = solid(64, 48, [40, 180, 40, 128]);
    {
        let cache = open(&dir, CacheLimits::default());
        cache.put(&key("a.png"), picture.clone()).unwrap();
    }
    let cache = open(&dir, CacheLimits::default());
    assert_eq!(cache.memory_bytes(), 0, "새로 연 캐시의 메모리는 비어 있다");
    let got = cache.get(&key("a.png")).expect("디스크에서 꺼낸다");
    assert_eq!(*got, *picture, "알파까지 그대로");
    assert!(cache.disk_bytes() > 0);
}

#[test]
#[ignore = "#328"]
fn 키의_한_부분이라도_다르면_꺼내지_못한다() {
    let root = TempDir::new().unwrap();
    let cache = open(&root.path().join("thumbs"), no_memory());
    let base = key("a.jpg");
    cache.put(&base, solid(96, 72, [200, 40, 40, 255])).unwrap();
    let mut other_path = base.clone();
    other_path.path = PathBuf::from(r"C:\사진\b.jpg");
    let mut other_side = base.clone();
    other_side.side = 192;
    let mut edited = base.clone();
    edited.modified += Duration::from_secs(1);
    let mut resized = base.clone();
    resized.file_size += 1;
    for (what, k) in [
        ("경로", other_path),
        ("한 변", other_side),
        ("수정 시각", edited),
        ("파일 크기", resized),
    ] {
        assert!(cache.get(&k).is_none(), "{what}가 다르면 다른 항목");
    }
    assert!(cache.get(&base).is_some());
}

#[test]
#[ignore = "#328"]
fn 그림은_압축해_저장한다() {
    let root = TempDir::new().unwrap();
    let cache = open(&root.path().join("thumbs"), CacheLimits::default());
    cache
        .put(&key("a.jpg"), solid(256, 256, [10, 20, 30, 255]))
        .unwrap();
    let raw = 256 * 256 * 4;
    let disk = cache.disk_bytes();
    assert!(disk > 0, "디스크에 썼다");
    assert!(disk < raw / 4, "날 RGBA {raw}바이트보다 훨씬 작다: {disk}");
}

#[test]
#[ignore = "#328"]
fn 정리는_상한_아래가_될_때까지_가장_오래_쓰지_않은_것부터_지운다() {
    let root = TempDir::new().unwrap();
    let picture = solid(96, 96, [10, 20, 30, 255]);
    // 항목 하나의 디스크 크기를 먼저 잰다(같은 그림이라 항목마다 같다).
    let per = {
        let probe = open(&root.path().join("probe"), no_memory());
        probe.put(&key("probe.jpg"), picture.clone()).unwrap();
        probe.disk_bytes()
    };
    assert!(per > 0);
    let limit = per * 3 + per / 2;
    let cache = open(
        &root.path().join("thumbs"),
        CacheLimits {
            disk_bytes: limit,
            memory_bytes: 0,
        },
    );
    for name in ["0.jpg", "1.jpg", "2.jpg"] {
        cache.put(&key(name), picture.clone()).unwrap();
        thread::sleep(GAP);
    }
    assert!(cache.get(&key("0.jpg")).is_some(), "0을 다시 썼다");
    thread::sleep(GAP);
    cache.put(&key("3.jpg"), picture.clone()).unwrap();
    cache.trim().unwrap();

    assert!(
        cache.disk_bytes() <= limit,
        "정리 뒤 {} > 상한 {limit}",
        cache.disk_bytes()
    );
    assert!(
        cache.get(&key("1.jpg")).is_none(),
        "가장 오래 쓰지 않은 1이 지워진다"
    );
    for name in ["0.jpg", "2.jpg", "3.jpg"] {
        assert!(cache.get(&key(name)).is_some(), "{name}은 남는다");
    }
}

#[test]
#[ignore = "#328"]
fn 상한_아래면_정리가_아무것도_지우지_않는다() {
    let root = TempDir::new().unwrap();
    let cache = open(&root.path().join("thumbs"), no_memory());
    for name in ["0.jpg", "1.jpg"] {
        cache
            .put(&key(name), solid(96, 96, [10, 20, 30, 255]))
            .unwrap();
    }
    let before = cache.disk_bytes();
    cache.trim().unwrap();
    assert_eq!(cache.disk_bytes(), before);
    assert!(cache.get(&key("0.jpg")).is_some());
    assert!(cache.get(&key("1.jpg")).is_some());
}

#[test]
#[ignore = "#328"]
fn 메모리에_둔_그림은_메모리_상한을_넘지_않고_밀려난_것도_디스크에서_꺼낸다() {
    let root = TempDir::new().unwrap();
    let one = 64 * 64 * 4;
    let limit = CacheLimits {
        disk_bytes: DEFAULT_DISK_LIMIT,
        memory_bytes: one * 3,
    };
    let cache = open(&root.path().join("thumbs"), limit);
    for i in 0..10u8 {
        cache
            .put(
                &key(&format!("{i}.jpg")),
                solid(64, 64, [i * 20, 40, 40, 255]),
            )
            .unwrap();
        assert!(
            cache.memory_bytes() <= limit.memory_bytes,
            "메모리 {} > 상한 {}",
            cache.memory_bytes(),
            limit.memory_bytes
        );
    }
    assert!(cache.memory_bytes() > 0, "최근 그림은 메모리에 둔다");
    for i in 0..10u8 {
        let got = cache
            .get(&key(&format!("{i}.jpg")))
            .unwrap_or_else(|| panic!("{i}을 꺼낸다"));
        assert_eq!(*got, *solid(64, 64, [i * 20, 40, 40, 255]));
        assert!(cache.memory_bytes() <= limit.memory_bytes);
    }
}

#[test]
#[ignore = "#328"]
fn 깨진_캐시_파일은_없는_것으로_보고_지우며_다시_넣으면_꺼낸다() {
    let garbage = b"this is not a thumbnail, just garbage bytes".repeat(4);
    for (what, damage) in [("쓰레기로 덮음", true), ("반으로 잘림", false)] {
        let root = TempDir::new().unwrap();
        let dir = root.path().join("thumbs");
        let picture = solid(96, 72, [200, 40, 40, 255]);
        {
            let cache = open(&dir, CacheLimits::default());
            cache.put(&key("a.jpg"), picture.clone()).unwrap();
        }
        let written = files(&dir);
        assert!(!written.is_empty(), "{what}: 디스크에 파일이 있다");
        for path in &written {
            if damage {
                fs::write(path, &garbage).unwrap();
            } else {
                let bytes = fs::read(path).unwrap();
                fs::write(path, &bytes[..bytes.len() / 2]).unwrap();
            }
        }

        let cache = open(&dir, CacheLimits::default());
        assert!(
            cache.get(&key("a.jpg")).is_none(),
            "{what}: 깨진 항목은 없는 것"
        );
        if damage {
            for path in files(&dir) {
                assert_ne!(
                    fs::read(&path).unwrap(),
                    garbage,
                    "{what}: 깨진 파일은 지운다: {}",
                    path.display()
                );
            }
        }
        cache.put(&key("a.jpg"), picture.clone()).unwrap();
        drop(cache);
        let cache = open(&dir, CacheLimits::default());
        let got = cache.get(&key("a.jpg")).expect("다시 넣은 것을 꺼낸다");
        assert_eq!(*got, *picture, "{what}");
    }
}

#[test]
#[ignore = "#328"]
fn 비우면_아무것도_꺼내지_못하고_디스크_크기가_0이다() {
    let root = TempDir::new().unwrap();
    let dir = root.path().join("thumbs");
    let cache = open(&dir, CacheLimits::default());
    for name in ["0.jpg", "1.jpg", "2.jpg"] {
        cache
            .put(&key(name), solid(96, 96, [10, 20, 30, 255]))
            .unwrap();
    }
    cache.clear().unwrap();
    assert_eq!(cache.disk_bytes(), 0);
    assert_eq!(cache.memory_bytes(), 0);
    for name in ["0.jpg", "1.jpg", "2.jpg"] {
        assert!(cache.get(&key(name)).is_none(), "{name}");
    }
    drop(cache);
    let cache = open(&dir, CacheLimits::default());
    assert_eq!(cache.disk_bytes(), 0, "다시 열어도 비어 있다");
    assert!(cache.get(&key("0.jpg")).is_none());
}

#[test]
#[ignore = "#328"]
fn 여러_스레드가_함께_넣고_꺼낸다() {
    let root = TempDir::new().unwrap();
    let cache = Arc::new(open(&root.path().join("thumbs"), CacheLimits::default()));
    let workers: Vec<_> = (0..4u8)
        .map(|t| {
            let cache = Arc::clone(&cache);
            thread::spawn(move || {
                for i in 0..20u8 {
                    let k = key(&format!("{t}-{i}.jpg"));
                    let picture = solid(32, 32, [t * 50, i, 7, 255]);
                    cache.put(&k, picture.clone()).unwrap();
                    assert_eq!(*cache.get(&k).expect("방금 넣은 것"), *picture);
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().expect("패닉하지 않는다");
    }
    for t in 0..4u8 {
        for i in 0..20u8 {
            assert!(cache.get(&key(&format!("{t}-{i}.jpg"))).is_some());
        }
    }
}

#[test]
#[ignore = "#328"]
fn 기본_상한은_디스크_1gb_메모리_256mb다() {
    assert_eq!(
        CacheLimits::default(),
        CacheLimits {
            disk_bytes: DEFAULT_DISK_LIMIT,
            memory_bytes: DEFAULT_MEMORY_LIMIT,
        }
    );
    assert!(
        [1_000_000_000, 1 << 30].contains(&DEFAULT_DISK_LIMIT),
        "{DEFAULT_DISK_LIMIT}"
    );
    assert!(
        [256_000_000, 256 << 20].contains(&DEFAULT_MEMORY_LIMIT),
        "{DEFAULT_MEMORY_LIMIT}"
    );
}

#[cfg(windows)]
#[test]
#[ignore = "#328"]
fn 기본_위치는_localappdata_아래_facet_thumbs다() {
    use facet_core::thumbnail_cache::default_cache_dir;

    let local = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA");
    assert_eq!(
        default_cache_dir(),
        Some(Path::new(&local).join("facet").join("thumbs"))
    );
}
