// target: crates/facet-core/tests/spec_preview_image.rs
// AC: #341: 이미지 내용의 크기(긴 변 = min(요청, 원래), 비율 ±1)와 색, 깨진·빈 이미지는 「보지 않음(깨짐)」, 앱에서 영역 크기대로 오고 크기를 바꾸면 다시 옴
//! 사진 미리보기를 임시 폴더의 실제 BMP 파일(시험이 직접 만듦)로 확인한다: 내용 만들기(`load_preview`)가 요청한 한 변에
//! 맞춰 줄인 그림을 내고, 깨진 파일은 이유로 끝나며, 앱(`App::set_preview_side`·`App::preview`)이 영역 크기대로 그림을
//! 받는다. 흐린 자리·EXIF 회전·화면 그리기는 사람이 확인한다. 모든 환경에서 돈다.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use facet_core::command::CommandId;
use facet_core::preview::{
    PreviewBody, PreviewContent, PreviewProblem, PreviewRequest, load_preview,
};
use facet_core::tab::ReadOutcome;
use facet_core::thumbnail::Thumbnail;
use facet_core::window::{App, Execution};
use facet_core::{Attributes, Item, Location};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(20);

/// 위 절반은 빨강, 아래 절반은 파랑인 24비트 BMP(행은 아래에서 위로 저장).
fn bmp(width: u32, height: u32) -> Vec<u8> {
    let row = (width * 3).div_ceil(4) * 4;
    let image_size = row * height;
    let mut bytes = Vec::with_capacity((54 + image_size) as usize);
    bytes.extend_from_slice(b"BM");
    bytes.extend_from_slice(&(54 + image_size).to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&54u32.to_le_bytes());
    bytes.extend_from_slice(&40u32.to_le_bytes());
    bytes.extend_from_slice(&(width as i32).to_le_bytes());
    bytes.extend_from_slice(&(height as i32).to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&24u16.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&image_size.to_le_bytes());
    bytes.extend_from_slice(&2835u32.to_le_bytes());
    bytes.extend_from_slice(&2835u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    for y in (0..height).rev() {
        let (b, g, r) = if y < height / 2 {
            (0u8, 0u8, 255u8)
        } else {
            (255, 0, 0)
        };
        for _ in 0..width {
            bytes.extend_from_slice(&[b, g, r]);
        }
        bytes.resize(bytes.len() + (row - width * 3) as usize, 0);
    }
    bytes
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, bytes).unwrap();
    path
}

fn request(path: &Path, side: u32) -> PreviewRequest {
    let meta = fs::metadata(path).unwrap();
    PreviewRequest {
        path: path.to_path_buf(),
        item: Item {
            name: path.file_name().unwrap().to_os_string(),
            is_folder: false,
            size: meta.len(),
            modified: meta.modified().unwrap(),
            attributes: Attributes::default(),
        },
        show_hidden: false,
        image_side: side,
    }
}

fn image_of(body: &PreviewBody) -> Option<&Thumbnail> {
    match body {
        PreviewBody::Image { image, .. } => Some(image),
        _ => None,
    }
}

fn pixel(image: &Thumbnail, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * image.width + x) * 4) as usize;
    image.rgba[i..i + 4].try_into().unwrap()
}

fn near(a: u32, b: u32) -> bool {
    a.abs_diff(b) <= 1
}

#[test]
#[ignore = "#341"]
fn an_image_is_scaled_to_the_requested_side_keeping_its_ratio() {
    let dir = TempDir::new().unwrap();
    let path = write(dir.path(), "사진.bmp", &bmp(400, 200));
    let body = load_preview(&request(&path, 100), &AtomicBool::new(false), &|_| false);
    match &body {
        PreviewBody::Image { image, placeholder } => {
            assert!(!placeholder, "디코딩한 그림은 흐린 자리가 아니다");
            assert_eq!(image.width, 100);
            assert!(near(image.height, 50), "비율: {}", image.height);
            assert_eq!(image.rgba.len(), (image.width * image.height * 4) as usize);
            let top = pixel(image, 2, 2);
            assert!(
                top[0] > 200 && top[2] < 60 && top[3] == 255,
                "위는 빨강: {top:?}"
            );
            let bottom = pixel(image, 2, image.height - 3);
            assert!(bottom[2] > 200 && bottom[0] < 60, "아래는 파랑: {bottom:?}");
        }
        other => panic!("그림이어야 한다: {other:?}"),
    }
}

#[test]
#[ignore = "#341"]
fn a_small_image_is_not_enlarged() {
    let dir = TempDir::new().unwrap();
    let path = write(dir.path(), "small.BMP", &bmp(40, 20));
    let body = load_preview(&request(&path, 500), &AtomicBool::new(false), &|_| false);
    let image = image_of(&body).expect("그림");
    assert_eq!((image.width, image.height), (40, 20));
}

#[test]
#[ignore = "#341"]
fn broken_and_empty_images_are_corrupt_without_panicking() {
    let dir = TempDir::new().unwrap();
    let mut cut = bmp(64, 64);
    cut.truncate(80);
    let cases = [
        write(
            dir.path(),
            "garbage.png",
            b"\x89PNG\r\n\x1a\nthis is not a png at all",
        ),
        write(dir.path(), "empty.jpg", b""),
        write(dir.path(), "cut.bmp", &cut),
    ];
    for path in &cases {
        assert_eq!(
            load_preview(&request(path, 128), &AtomicBool::new(false), &|_| false),
            PreviewBody::Unavailable(PreviewProblem::Corrupt),
            "{}",
            path.display()
        );
    }
}

fn app_at(path: &Path) -> App {
    let mut app = App::new(Some(Location::file_system(path)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

/// 활성 창의 미리보기에 그 긴 변의 그림이 올 때까지 받는다.
fn wait_image_side(app: &mut App, side: u32) -> Thumbnail {
    let window = app.active_window_id();
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(PreviewContent::Item { body, .. }) = app.preview(window)
            && let Some(image) = image_of(body)
            && image.width.max(image.height) == side
        {
            return image.clone();
        }
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(
            !left.is_zero(),
            "그림이 오지 않았다: {:?}",
            app.preview(window).map(|c| match c {
                PreviewContent::Item { body, .. } =>
                    format!("{:?}", image_of(body).map(|i| (i.width, i.height))),
                other => format!("{other:?}"),
            })
        );
        app.wait_update(left);
    }
}

#[test]
#[ignore = "#341"]
fn the_app_previews_an_image_at_the_pane_size_and_follows_size_changes() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "사진.bmp", &bmp(400, 200));
    let mut app = app_at(dir.path());
    assert_eq!(app.execute(CommandId::TogglePreview), Execution::Done);
    let window = app.active_window_id();
    app.set_preview_side(window, 120);

    let index = app
        .active_tab()
        .items()
        .position(|i| i.name == "사진.bmp")
        .unwrap();
    app.active_tab_mut().select_only(index);
    app.sync_preview();
    let image = wait_image_side(&mut app, 120);
    assert!(near(image.height, 60));

    app.set_preview_side(window, 200);
    let image = wait_image_side(&mut app, 200);
    assert!(near(image.height, 100), "영역이 커지면 그 크기로 다시 온다");
}
