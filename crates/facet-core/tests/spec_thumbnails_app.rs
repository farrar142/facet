// target: crates/facet-core/tests/spec_thumbnails_app.rs
// AC: #330: 계획(폴더·이미지가 아닌 파일은 아이콘만, 온라인 전용 이미지는 캐시만, 보통 이미지는 만들기, 확장자 대소문자 무시)
// AC: #330: 그리드 요청을 넘기면 이미지 칸마다 Update::Thumbnail(긴 변 = 탭의 칸 크기 한 변, 작은 그림은 원래 크기), 아이콘만인 항목은 결과 없음
// AC: #330: 처음엔 캐시가 아니고 같은 캐시 폴더로 새로 띄운 앱에선 캐시에서 옴, 파일을 고치면 새 그림, 깨진 이미지는 실패로 오고 나머지는 계속
// AC: #330: 요청은 곧바로 돌아옴, 「썸네일 캐시 비우기」 명령(목록에 있고 늘 켜짐, 실행 뒤 다음 요청은 캐시가 아님)
//! 앱에 이은 썸네일(`App::enable_thumbnails`·`request_thumbnails` → `Update::Thumbnail`)을 임시 폴더의 실제 그림 파일과
//! 임시 캐시 폴더로 확인한다. 그림은 이 파일 안의 작은 PNG 인코더(압축하지 않는 deflate)가 만든다. 항목별 계획
//! (`thumbnail::plan`)은 목록 항목 값으로 본다(온라인 전용 속성은 실제 파일에 흉내 낼 수 없음). 모든 플랫폼에서 돈다.
//! 그리드 화면의 그리기는 사람이 확인한다.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use facet_core::command::CommandId;
use facet_core::tab::ReadOutcome;
use facet_core::thumbnail::{Thumbnail, ThumbnailError, ThumbnailPlan, plan};
use facet_core::view::{GridSize, GridSpans};
use facet_core::window::{App, Execution, ThumbnailOptions, Update};
use facet_core::{Attributes, Item, Location};
use tempfile::TempDir;

/// CI 러너가 느려도 흔들리지 않게 넉넉히 기다린다. 결과가 오면 곧바로 끝난다.
const WAIT: Duration = Duration::from_secs(30);
/// 결과가 「오지 않음」을 볼 때 기다리는 시간.
const QUIET: Duration = Duration::from_millis(500);

const RED: [u8; 4] = [200, 40, 40, 255];
const BLUE: [u8; 4] = [40, 40, 200, 255];

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
    // 행마다 필터 바이트 0 뒤에 화소들.
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

/// 그림 둘(큰 PNG 600×400 빨강, 작은 PNG 40×30), 글자 파일, 하위 폴더.
fn photos() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("big.png"), png(600, 400, RED)).unwrap();
    fs::write(dir.path().join("small.PNG"), png(40, 30, BLUE)).unwrap();
    fs::write(dir.path().join("notes.txt"), b"not a picture").unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    dir
}

/// 그 폴더를 연 앱. 썸네일을 켜고 활성 탭을 그리드 `size`로 둔다.
fn app(folder: &Path, cache: &Path, size: GridSize) -> App {
    let mut app = App::new(Some(Location::file_system(folder)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app.enable_thumbnails(ThumbnailOptions::new(cache));
    assert_eq!(app.execute(CommandId::GridView(size)), Execution::Done);
    app
}

/// 활성 탭의 모든 항목이 보이는 칸이다.
fn request_all(app: &mut App) {
    let len = app.active_tab().len();
    let tab = app.active_tab_id();
    app.request_thumbnails(
        tab,
        GridSpans {
            visible: 0..len,
            nearby: 0..len,
        },
    );
}

struct Got {
    name: String,
    side: u32,
    result: Result<Arc<Thumbnail>, ThumbnailError>,
    from_cache: bool,
}

fn got(
    path: PathBuf,
    side: u32,
    result: Result<Arc<Thumbnail>, ThumbnailError>,
    from_cache: bool,
) -> Got {
    Got {
        name: path.file_name().unwrap().to_string_lossy().into_owned(),
        side,
        result,
        from_cache,
    }
}

/// 썸네일 결과를 `count`개 받는다. 다른 결과(읽기 등)는 넘긴다.
fn collect(app: &mut App, count: usize) -> Vec<Got> {
    let deadline = Instant::now() + WAIT;
    let mut out = Vec::new();
    while out.len() < count {
        let left = deadline.saturating_duration_since(Instant::now());
        match app.wait_update(left) {
            Some((
                _,
                Update::Thumbnail {
                    path,
                    side,
                    result,
                    from_cache,
                },
            )) => out.push(got(path, side, result, from_cache)),
            Some(_) => {}
            None => panic!("썸네일 결과 {count}개 가운데 {}개만 왔다", out.len()),
        }
    }
    out
}

/// 잠시 기다리며 그사이 온 썸네일 결과.
fn quiet(app: &mut App) -> Vec<Got> {
    let deadline = Instant::now() + QUIET;
    let mut out = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return out;
        }
        if let Some((
            _,
            Update::Thumbnail {
                path,
                side,
                result,
                from_cache,
            },
        )) = app.wait_update(left)
        {
            out.push(got(path, side, result, from_cache));
        }
    }
}

fn find<'a>(all: &'a [Got], name: &str) -> &'a Got {
    all.iter()
        .find(|g| g.name == name)
        .unwrap_or_else(|| panic!("{name}의 결과가 없다"))
}

fn picture(g: &Got) -> &Thumbnail {
    g.result
        .as_ref()
        .unwrap_or_else(|e| panic!("{}: {e:?}", g.name))
}

/// 불투명한 화소의 평균에서 가장 센 채널.
fn dominant(t: &Thumbnail) -> char {
    let mut sum = [0u64; 3];
    for p in t.rgba.as_chunks::<4>().0.iter() {
        for (c, s) in sum.iter_mut().enumerate() {
            *s += u64::from(p[c]);
        }
    }
    let [r, g, b] = sum;
    if r > g && r > b {
        'r'
    } else if b > r && b > g {
        'b'
    } else {
        'g'
    }
}

#[test]
fn 항목마다_아이콘만_캐시만_만들기를_정한다() {
    assert_eq!(plan(&item("사진", true, false)), ThumbnailPlan::IconOnly);
    assert_eq!(
        plan(&item("메모.txt", false, false)),
        ThumbnailPlan::IconOnly
    );
    assert_eq!(
        plan(&item("영상.mp4", false, false)),
        ThumbnailPlan::IconOnly
    );
    assert_eq!(
        plan(&item("그림.psd", false, false)),
        ThumbnailPlan::IconOnly
    );
    assert_eq!(plan(&item("README", false, false)), ThumbnailPlan::IconOnly);
    for name in ["a.jpg", "b.JPEG", "c.png", "d.Gif", "e.webp", "f.BMP"] {
        assert_eq!(
            plan(&item(name, false, false)),
            ThumbnailPlan::Decode,
            "{name}"
        );
        assert_eq!(
            plan(&item(name, false, true)),
            ThumbnailPlan::CacheOnly,
            "온라인 전용 {name}은 내용을 읽지 않는다"
        );
    }
    assert_eq!(
        plan(&item("메모.txt", false, true)),
        ThumbnailPlan::IconOnly
    );
    assert_eq!(plan(&item("폴더", true, true)), ThumbnailPlan::IconOnly);
}

#[test]
fn 그리드_요청을_넘기면_이미지_칸마다_칸_크기의_썸네일이_온다() {
    let dir = photos();
    let cache = TempDir::new().unwrap();
    let side = GridSize::Large.thumbnail_side();
    assert!(
        side < 600 && side > 40,
        "시험 그림 크기가 칸 크기를 사이에 둔다"
    );
    let mut app = app(dir.path(), cache.path(), GridSize::Large);
    request_all(&mut app);
    let all = collect(&mut app, 2);

    let big = find(&all, "big.png");
    assert_eq!(big.side, side);
    assert!(!big.from_cache, "처음에는 만든다");
    let t = picture(big);
    assert_eq!(t.width, side, "긴 변이 칸 크기");
    assert!(
        t.height.abs_diff(side * 2 / 3) <= 1,
        "비율 유지: {}",
        t.height
    );
    assert_eq!(dominant(t), 'r');

    let small = find(&all, "small.PNG");
    let t = picture(small);
    assert_eq!((t.width, t.height), (40, 30), "작은 그림은 키우지 않는다");
    assert_eq!(dominant(t), 'b');

    let more = quiet(&mut app);
    assert!(
        more.is_empty(),
        "글자 파일·폴더는 결과가 없다: {:?}",
        more.iter().map(|g| &g.name).collect::<Vec<_>>()
    );
}

#[test]
fn 다시_열면_캐시에서_오고_파일을_고치면_새로_만든다() {
    let dir = photos();
    let cache = TempDir::new().unwrap();
    {
        let mut first = app(dir.path(), cache.path(), GridSize::Medium);
        request_all(&mut first);
        let all = collect(&mut first, 2);
        assert!(all.iter().all(|g| !g.from_cache && g.result.is_ok()));
    }

    let mut second = app(dir.path(), cache.path(), GridSize::Medium);
    request_all(&mut second);
    let all = collect(&mut second, 2);
    for g in &all {
        assert!(g.from_cache, "{}: 한 번 본 폴더는 캐시에서 온다", g.name);
    }
    let t = picture(find(&all, "big.png"));
    assert_eq!(t.width, GridSize::Medium.thumbnail_side());
    assert_eq!(dominant(t), 'r');
    drop(second);

    // 사진을 고친다(내용·크기·수정 시각이 바뀜).
    std::thread::sleep(Duration::from_millis(50));
    fs::write(dir.path().join("big.png"), png(500, 400, BLUE)).unwrap();
    let mut third = app(dir.path(), cache.path(), GridSize::Medium);
    request_all(&mut third);
    let all = collect(&mut third, 2);
    let big = find(&all, "big.png");
    assert!(!big.from_cache, "고친 파일은 새로 만든다");
    assert_eq!(dominant(picture(big)), 'b', "새 그림");
    assert!(
        find(&all, "small.PNG").from_cache,
        "고치지 않은 파일은 그대로 캐시"
    );
}

#[test]
fn 깨진_이미지는_실패로_오고_나머지_칸은_채워진다() {
    let dir = TempDir::new().unwrap();
    let mut broken = b"\x89PNG\r\n\x1a\n".to_vec();
    broken.extend_from_slice(&[0x5A; 500]);
    fs::write(dir.path().join("a-broken.png"), broken).unwrap();
    fs::write(dir.path().join("b-empty.jpg"), b"").unwrap();
    fs::write(dir.path().join("c-good.png"), png(64, 64, RED)).unwrap();
    let cache = TempDir::new().unwrap();
    let mut app = app(dir.path(), cache.path(), GridSize::Small);
    request_all(&mut app);
    let all = collect(&mut app, 3);
    assert!(find(&all, "a-broken.png").result.is_err());
    assert!(find(&all, "b-empty.jpg").result.is_err());
    assert_eq!(dominant(picture(find(&all, "c-good.png"))), 'r');
}

#[test]
fn 요청은_곧바로_돌아온다() {
    let dir = TempDir::new().unwrap();
    let count = 300;
    for i in 0..count {
        fs::write(dir.path().join(format!("{i:03}.png")), png(120, 90, RED)).unwrap();
    }
    let cache = TempDir::new().unwrap();
    let mut app = app(dir.path(), cache.path(), GridSize::Medium);
    let started = Instant::now();
    request_all(&mut app);
    let took = started.elapsed();
    assert!(
        took < Duration::from_millis(200),
        "UI 스레드에서 파일을 읽지 않는다: {took:?}"
    );
    let all = collect(&mut app, count);
    assert!(all.iter().all(|g| g.result.is_ok()));
}

#[test]
fn 썸네일_캐시_비우기_명령이_있고_비운_뒤에는_캐시가_아니다() {
    let command = CommandId::ClearThumbnailCache;
    assert_eq!(command.as_str(), "clear-thumbnail-cache");
    assert_eq!(CommandId::from_id("clear-thumbnail-cache"), Some(command));
    assert!(CommandId::ALL.contains(&command));
    assert!(command.shortcuts().is_empty(), "기본 단축키 없음");

    let dir = photos();
    let cache = TempDir::new().unwrap();
    let mut app = app(dir.path(), cache.path(), GridSize::Medium);
    assert!(app.can_execute(command), "늘 켜진다");
    request_all(&mut app);
    collect(&mut app, 2);

    assert_eq!(app.execute(command), Execution::Done);
    request_all(&mut app);
    let all = collect(&mut app, 2);
    for g in &all {
        assert!(!g.from_cache, "{}: 비운 뒤에는 다시 만든다", g.name);
        assert!(g.result.is_ok());
    }
}
