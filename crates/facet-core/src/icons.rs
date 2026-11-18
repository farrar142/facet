//! 파일 종류별 아이콘: 키 규칙과 메모리 캐시(얻기는 UI 밖 스레드).
//!
//! 아이콘은 키([`IconKey`])와 크기마다 한 번만 얻는다. 키는 [`icon_key`]가 정한다: 폴더는 폴더 키 하나, 파일마다
//! 아이콘이 다른 확장자([`PER_FILE_EXTENSIONS`]: 실행 파일·아이콘 파일·바로 가기)는 전체 경로, 그 밖은 소문자
//! 확장자, 확장자가 없으면 확장자 없음 키다. 온라인 전용 항목은 고유 아이콘을 얻으려고 파일을 읽으면 내려받기가
//! 일어나므로 확장자 키로 본다.
//!
//! 캐시([`IconCache`])의 조회([`IconCache::get`])는 기다리지 않는다: 이미 얻었으면 그림을, 아니면 곧바로 `None`을
//! 돌려주고 처음 묻는 키·크기의 얻기를 캐시의 일꾼 스레드 하나에 맡긴다. 얻기가 끝나면(얻지 못했어도) 결과를
//! 기억하고 사건(키·크기)을 쌓고 깨우기를 부른다. 얻지 못한 키·크기는 다시 얻지 않고 계속 `None`이라 화면은
//! 내장 아이콘을 그린다. 실제 얻기([`IconCache::new`])는 안전 래퍼의 [`facet_win::icon`]이고 셸 확장 도우미를
//! 거치지 않는다(ADR-0006의 「아이콘」 항목). 그림을 그리는 쪽(자세히 보기, 뒤의 그리드 보기)이 같은 캐시를 쓴다.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::Item;

pub use facet_win::icon::IconImage;

/// 아이콘 캐시의 키.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IconKey {
    /// 폴더.
    Folder,
    /// 확장자(점 없는 소문자)가 같은 파일들.
    Extension(String),
    /// 확장자가 없는 파일.
    NoExtension,
    /// 파일마다 아이콘이 다른 파일 하나(전체 경로).
    File(PathBuf),
}

/// 파일마다 아이콘이 다른 확장자(소문자, 점 없음).
pub const PER_FILE_EXTENSIONS: [&str; 4] = ["exe", "ico", "lnk", "url"];

/// 항목의 아이콘 키. `folder`는 항목이 든 폴더다.
pub fn icon_key(folder: &Path, item: &Item) -> IconKey {
    if item.is_folder {
        return IconKey::Folder;
    }
    let name = item.name.to_string_lossy();
    let extension = match name.rfind('.') {
        // 점으로 시작하는 이름(`.gitignore`)과 점으로 끝나는 이름은 확장자가 없다.
        Some(dot) if dot > 0 && dot + 1 < name.len() => name[dot + 1..].to_lowercase(),
        _ => return IconKey::NoExtension,
    };
    if !item.attributes.online_only && PER_FILE_EXTENSIONS.contains(&extension.as_str()) {
        return IconKey::File(folder.join(&item.name));
    }
    IconKey::Extension(extension)
}

/// 얻기 함수.
type Loader = dyn Fn(&IconKey, u32) -> Option<IconImage> + Send + Sync;
/// 깨우기.
type Waker = dyn Fn() + Send + Sync;

/// 키·크기 하나의 상태.
enum Slot {
    /// 얻는 중.
    Pending,
    /// 얻음(`None`이면 얻지 못함).
    Done(Option<Arc<IconImage>>),
}

/// 캐시와 일꾼 스레드가 함께 쓰는 상태.
struct Shared {
    slots: Mutex<HashMap<(IconKey, u32), Slot>>,
    events: Mutex<VecDeque<(IconKey, u32)>>,
    event_ready: Condvar,
    waker: Mutex<Option<Arc<Waker>>>,
}

/// 아이콘 메모리 캐시. 여러 스레드가 나눠 쓴다(`Send + Sync`).
pub struct IconCache {
    shared: Arc<Shared>,
    loader: Arc<Loader>,
    /// 일꾼 스레드에 얻기를 맡기는 통로. 처음 얻기를 맡길 때 스레드를 띄운다. 캐시를 버리면 스레드가 끝난다.
    requests: Mutex<Option<Sender<(IconKey, u32)>>>,
}

impl IconCache {
    /// 시스템 아이콘을 얻는 캐시([`facet_win::icon`]). 파일 하나의 고유 아이콘을 얻지 못하면 그 확장자 아이콘을 쓴다.
    pub fn new() -> IconCache {
        IconCache::with_loader(load_system_icon)
    }

    /// 얻기 함수를 주입한 캐시. 얻기 함수는 캐시의 일꾼 스레드에서 키·크기마다 한 번 불린다.
    pub fn with_loader(
        loader: impl Fn(&IconKey, u32) -> Option<IconImage> + Send + Sync + 'static,
    ) -> IconCache {
        IconCache {
            shared: Arc::new(Shared {
                slots: Mutex::new(HashMap::new()),
                events: Mutex::new(VecDeque::new()),
                event_ready: Condvar::new(),
                waker: Mutex::new(None),
            }),
            loader: Arc::new(loader),
            requests: Mutex::new(None),
        }
    }

    /// 캐시에 있으면 그림, 없으면 곧바로 `None`이다. 처음 묻는 키·크기면 얻기를 일꾼 스레드에 맡긴다. 얻지 못한
    /// 키·크기는 늘 `None`이다.
    pub fn get(&self, key: &IconKey, size: u32) -> Option<Arc<IconImage>> {
        {
            let mut slots = lock(&self.shared.slots);
            match slots.get(&(key.clone(), size)) {
                Some(Slot::Done(icon)) => return icon.clone(),
                Some(Slot::Pending) => return None,
                None => {
                    slots.insert((key.clone(), size), Slot::Pending);
                }
            }
        }
        self.request(key.clone(), size);
        None
    }

    /// 얻기가 끝날 때(얻지 못했어도) 부를 깨우기. 일꾼 스레드에서 불린다.
    pub fn set_waker(&self, waker: impl Fn() + Send + Sync + 'static) {
        *lock(&self.shared.waker) = Some(Arc::new(waker));
    }

    /// 지난 번 뒤로 얻기가 끝난 키·크기들(끝난 차례). 기다리지 않는다.
    pub fn poll(&self) -> Vec<(IconKey, u32)> {
        lock(&self.shared.events).drain(..).collect()
    }

    /// 얻기가 끝난 키·크기 하나를 `timeout`까지 기다린다.
    pub fn wait_event(&self, timeout: Duration) -> Option<(IconKey, u32)> {
        let deadline = Instant::now() + timeout;
        let mut events = lock(&self.shared.events);
        loop {
            if let Some(event) = events.pop_front() {
                return Some(event);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            events = self
                .shared
                .event_ready
                .wait_timeout(events, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// 일꾼 스레드에 얻기를 맡긴다. 스레드가 없으면 띄운다.
    fn request(&self, key: IconKey, size: u32) {
        let mut requests = lock(&self.requests);
        let sender = requests.get_or_insert_with(|| {
            let (sender, receiver) = mpsc::channel::<(IconKey, u32)>();
            let shared = Arc::clone(&self.shared);
            let loader = Arc::clone(&self.loader);
            // 스레드를 띄우지 못하면 받는 쪽이 사라져 아래 보내기가 실패하고, 얻지 못한 것으로 기억한다.
            let _ = thread::Builder::new()
                .name("facet-icons".into())
                .spawn(move || {
                    for (key, size) in receiver {
                        let icon = loader(&key, size).map(Arc::new);
                        finish(&shared, key, size, icon);
                    }
                });
            sender
        });
        if let Err(mpsc::SendError((key, size))) = sender.send((key, size)) {
            *requests = None;
            drop(requests);
            finish(&self.shared, key, size, None);
        }
    }
}

impl Default for IconCache {
    fn default() -> Self {
        Self::new()
    }
}

/// 얻기 결과를 기억하고 사건을 쌓고 깨운다.
fn finish(shared: &Shared, key: IconKey, size: u32, icon: Option<Arc<IconImage>>) {
    lock(&shared.slots).insert((key.clone(), size), Slot::Done(icon));
    lock(&shared.events).push_back((key, size));
    shared.event_ready.notify_all();
    let waker = lock(&shared.waker).clone();
    if let Some(waker) = waker {
        waker();
    }
}

/// 시스템 아이콘 얻기.
fn load_system_icon(key: &IconKey, size: u32) -> Option<IconImage> {
    match key {
        IconKey::Folder => facet_win::icon::folder_icon(size),
        IconKey::Extension(extension) => facet_win::icon::extension_icon(extension, size),
        IconKey::NoExtension => facet_win::icon::extension_icon("", size),
        IconKey::File(path) => facet_win::icon::file_icon(path, size).or_else(|| {
            let extension = path
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            facet_win::icon::extension_icon(&extension, size)
        }),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
