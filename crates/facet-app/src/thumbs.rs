//! 그리드 칸의 썸네일 그림과 요청 범위 기억(#330).
//!
//! 코어가 UI 밖 작업자에서 만든 그림(`Update::Thumbnail`)을 Slint 그림으로 한 번만 바꿔 둔다. UI 스레드는 디코딩·
//! 파일 읽기·캐시 읽기를 하지 않고 이미 받은 그림을 찾기만 한다. 같은 스크롤 위치에서는 요청을 되풀이하지 않는다.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use facet_core::thumbnail::Thumbnail;
use facet_core::view::GridSpans;
use facet_core::window::{TabId, WindowId};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

/// 들고 있을 그림 수 상한. 넘으면 모두 버린다(보이는 칸은 다시 요청하면 캐시에서 곧바로 온다).
const MAX_IMAGES: usize = 3000;

/// 지난 요청의 조건. 같으면 다시 요청하지 않는다.
#[derive(Clone, PartialEq, Eq)]
struct Requested {
    tab: TabId,
    spans: GridSpans,
    len: usize,
    side: u32,
}

#[derive(Default)]
struct Store {
    images: HashMap<(PathBuf, u32), Image>,
    requested: HashMap<(WindowId, usize), Requested>,
}

thread_local! {
    static STORE: RefCell<Store> = RefCell::new(Store::default());
}

/// 코어가 만든 썸네일을 Slint 그림으로 바꿔 둔다.
pub fn store(path: &Path, side: u32, thumbnail: &Thumbnail) {
    let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
        &thumbnail.rgba,
        thumbnail.width,
        thumbnail.height,
    );
    let image = Image::from_rgba8(buffer);
    STORE.with(|store| {
        let mut store = store.borrow_mut();
        if store.images.len() >= MAX_IMAGES {
            store.images.clear();
        }
        store.images.insert((path.to_path_buf(), side), image);
    });
}

/// 그 칸의 썸네일 그림. 아직 없으면 `None`이다(칸은 종류별 아이콘을 그린다).
pub fn get(path: &Path, side: u32) -> Option<Image> {
    STORE.with(|store| {
        store
            .borrow()
            .images
            .get(&(path.to_path_buf(), side))
            .cloned()
    })
}

/// 이 요청이 지난 요청과 다르면 기억하고 `true`.
pub fn note_request(
    window: WindowId,
    panel: usize,
    tab: TabId,
    spans: &GridSpans,
    len: usize,
    side: u32,
) -> bool {
    let now = Requested {
        tab,
        spans: spans.clone(),
        len,
        side,
    };
    STORE.with(|store| {
        let mut store = store.borrow_mut();
        if store.requested.get(&(window, panel)) == Some(&now) {
            return false;
        }
        store.requested.insert((window, panel), now);
        true
    })
}

/// 그 패널의 지난 요청을 잊는다(목록이 바뀌었을 때: 같은 범위라도 다시 요청한다).
pub fn forget_request(window: WindowId, panel: usize) {
    STORE.with(|store| {
        store.borrow_mut().requested.remove(&(window, panel));
    });
}
