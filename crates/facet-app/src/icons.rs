//! 자세히 보기 행의 파일 종류별 아이콘.
//!
//! 코어의 아이콘 캐시([`IconCache`])를 UI 스레드 하나에 두고 행을 그릴 때 묻기만 한다: 캐시 조회는 기다리지 않고,
//! 얻기는 캐시의 일꾼 스레드가 한다. 아직 없거나 얻지 못하면 `None`이라 화면은 내장 아이콘을 그린다. 얻기가 끝나면
//! 캐시가 깨우기를 부르고, 이벤트 루프에서 [`take_loaded`]가 사건을 비워 그 아이콘을 기다리던 행만 다시 그리게 한다.
//! 얻은 그림은 Slint 그림으로 한 번만 바꿔 둔다(행을 그릴 때마다 화소를 복사하지 않음).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use facet_core::Item;
use facet_core::icons::{IconCache, IconKey, icon_key};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

/// 화면에 그리는 아이콘 크기(논리 픽셀).
pub const ICON_LOGICAL_SIZE: f32 = 16.0;

struct Icons {
    cache: IconCache,
    /// 얻은 아이콘의 Slint 그림.
    images: RefCell<HashMap<(IconKey, u32), Image>>,
}

thread_local! {
    static ICONS: RefCell<Option<Icons>> = const { RefCell::new(None) };
}

/// 아이콘 캐시를 띄운다. `waker`는 얻기가 끝날 때 일꾼 스레드에서 불린다(이벤트 루프를 깨운다).
pub fn start(waker: impl Fn() + Send + Sync + 'static) {
    let cache = IconCache::new();
    cache.set_waker(waker);
    ICONS.with(|slot| {
        *slot.borrow_mut() = Some(Icons {
            cache,
            images: RefCell::new(HashMap::new()),
        });
    });
}

/// 배율 `scale`인 창에서 아이콘을 얻을 화소 크기.
pub fn pixel_size(scale: f32) -> u32 {
    let size = (ICON_LOGICAL_SIZE * scale).round();
    if size.is_finite() && size >= 1.0 {
        (size as u32).min(256)
    } else {
        ICON_LOGICAL_SIZE as u32
    }
}

/// `folder` 안 항목의 아이콘(`size` 화소). 아직 없거나 얻지 못했거나 캐시를 띄우지 않았으면 `None`이다. 기다리지
/// 않는다.
pub fn icon(folder: &Path, item: &Item, size: u32) -> Option<Image> {
    icon_for(&icon_key(folder, item), size)
}

/// 아이콘 키 `key`의 아이콘(`size` 화소, 키는 코어의 `Tab::icon_key`). 아직 없거나 얻지 못했거나 캐시를 띄우지
/// 않았으면 `None`이다. 기다리지 않는다.
pub fn icon_for(key: &IconKey, size: u32) -> Option<Image> {
    ICONS.with(|slot| {
        let slot = slot.borrow();
        let icons = slot.as_ref()?;
        let key = (key.clone(), size);
        if let Some(image) = icons.images.borrow().get(&key) {
            return Some(image.clone());
        }
        let picture = icons.cache.get(&key.0, size)?;
        let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
            &picture.rgba,
            picture.width,
            picture.height,
        );
        let image = Image::from_rgba8(buffer);
        icons.images.borrow_mut().insert(key, image.clone());
        Some(image)
    })
}

/// 지난 번 뒤로 얻기가 끝난 아이콘이 있는가(사건을 비운다).
pub fn take_loaded() -> bool {
    ICONS.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|icons| !icons.cache.poll().is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 화소_크기는_배율을_따른다() {
        assert_eq!(pixel_size(1.0), 16);
        assert_eq!(pixel_size(1.25), 20);
        assert_eq!(pixel_size(2.0), 32);
        assert_eq!(pixel_size(f32::NAN), 16);
        assert_eq!(pixel_size(100.0), 256);
    }

    #[test]
    fn 캐시를_띄우지_않았으면_내장_아이콘이다() {
        let item = Item {
            name: "a.txt".into(),
            is_folder: false,
            size: 0,
            modified: std::time::SystemTime::UNIX_EPOCH,
            attributes: Default::default(),
        };
        assert!(icon(Path::new("C:\\"), &item, 16).is_none());
        assert!(!take_loaded());
    }
}
