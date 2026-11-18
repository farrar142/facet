//! 썸네일 캐시(GLOSSARY의 **썸네일 캐시**, 인덱스와 다르다): 만든 썸네일을 디스크에 압축(PNG)해 두고 메모리에는
//! 최근 그림을 상한까지 둔다. 항목 하나가 파일 하나이고 파일 이름은 키의 해시다. 파일 머리에 키를 적어 두어
//! (해시가 부딪혀도) 키가 같을 때만 꺼내고, 깨진 파일은 꺼낼 때 지운다. 화면 문장 없음(ADR-0007).
//! 여러 스레드가 함께 쓴다: 디스크 읽기·쓰기는 잠금 밖에서 하고 잠금은 목록 갱신에만 쓴다.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder};

use crate::thumbnail::{Thumbnail, ThumbnailKey};

/// #328: 디스크 크기 상한(1GB).
pub const DEFAULT_DISK_LIMIT: u64 = 1 << 30;
/// #328: 메모리 크기 상한(256MB).
pub const DEFAULT_MEMORY_LIMIT: u64 = 256 << 20;

const MAGIC: &[u8; 4] = b"FTC1";
const EXTENSION: &str = "thumb";
const TEMP_PREFIX: &str = ".tmp-";

/// #328: 캐시 상한.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheLimits {
    pub disk_bytes: u64,
    pub memory_bytes: u64,
}

impl Default for CacheLimits {
    fn default() -> Self {
        CacheLimits {
            disk_bytes: DEFAULT_DISK_LIMIT,
            memory_bytes: DEFAULT_MEMORY_LIMIT,
        }
    }
}

/// #328: 기본 캐시 위치(`%LOCALAPPDATA%\facet\thumbs`). Windows가 아니면 `None`.
pub fn default_cache_dir() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(Path::new(&local).join("facet").join("thumbs"))
}

struct DiskEntry {
    size: u64,
    used: SystemTime,
}

struct MemoryEntry {
    key: ThumbnailKey,
    thumbnail: Arc<Thumbnail>,
    tick: u64,
}

#[derive(Default)]
struct Inner {
    disk: HashMap<String, DiskEntry>,
    disk_bytes: u64,
    memory: HashMap<String, MemoryEntry>,
    memory_bytes: u64,
    tick: u64,
}

/// #328: 썸네일 캐시.
pub struct ThumbnailCache {
    dir: PathBuf,
    limits: CacheLimits,
    inner: Mutex<Inner>,
    temp_counter: AtomicU64,
}

fn fnv(bytes: &[u8], seed: u64) -> u64 {
    let mut hash = seed;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn encode_key(key: &ThumbnailKey) -> Vec<u8> {
    let path = key.path.to_string_lossy();
    let since = key
        .modified
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    let mut out = Vec::with_capacity(path.len() + 32);
    out.extend_from_slice(&(path.len() as u32).to_le_bytes());
    out.extend_from_slice(path.as_bytes());
    out.extend_from_slice(&key.side.to_le_bytes());
    out.extend_from_slice(&since.as_secs().to_le_bytes());
    out.extend_from_slice(&since.subsec_nanos().to_le_bytes());
    out.extend_from_slice(&key.file_size.to_le_bytes());
    out
}

fn file_name(key_bytes: &[u8]) -> String {
    let a = fnv(key_bytes, 0xcbf2_9ce4_8422_2325);
    let b = fnv(key_bytes, 0x8422_2325_cbf2_9ce4 ^ key_bytes.len() as u64);
    format!("{a:016x}{b:016x}.{EXTENSION}")
}

fn encode_file(key_bytes: &[u8], thumbnail: &Thumbnail) -> std::io::Result<Vec<u8>> {
    let invalid = |m: &str| std::io::Error::new(std::io::ErrorKind::InvalidInput, m.to_owned());
    if thumbnail.rgba.len() as u64 != u64::from(thumbnail.width) * u64::from(thumbnail.height) * 4 {
        return Err(invalid("썸네일 크기와 화소 수가 맞지 않습니다"));
    }
    let mut out = Vec::with_capacity(key_bytes.len() + 64);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(key_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(key_bytes);
    let mut png = Vec::new();
    PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::Sub)
        .write_image(
            &thumbnail.rgba,
            thumbnail.width,
            thumbnail.height,
            ExtendedColorType::Rgba8,
        )
        .map_err(|e| invalid(&e.to_string()))?;
    out.extend_from_slice(&png);
    Ok(out)
}

enum Decoded {
    Hit(Thumbnail),
    /// 정상 파일이지만 다른 키(해시 충돌).
    Other,
    Broken,
}

fn decode_file(bytes: &[u8], key_bytes: &[u8]) -> Decoded {
    let Some(rest) = bytes.strip_prefix(MAGIC.as_slice()) else {
        return Decoded::Broken;
    };
    let Some((len, rest)) = rest.split_first_chunk::<4>() else {
        return Decoded::Broken;
    };
    let len = u32::from_le_bytes(*len) as usize;
    if rest.len() < len {
        return Decoded::Broken;
    }
    let (stored, png) = rest.split_at(len);
    if stored != key_bytes {
        return Decoded::Other;
    }
    match image::load_from_memory_with_format(png, image::ImageFormat::Png) {
        Ok(img) => {
            let img = img.into_rgba8();
            Decoded::Hit(Thumbnail {
                width: img.width(),
                height: img.height(),
                rgba: img.into_raw(),
            })
        }
        Err(_) => Decoded::Broken,
    }
}

impl ThumbnailCache {
    /// #328: 캐시 폴더를 연다(없으면 만든다). 있는 항목은 크기와 마지막으로 쓴 때만 읽는다.
    pub fn open(dir: &Path, limits: CacheLimits) -> std::io::Result<ThumbnailCache> {
        fs::create_dir_all(dir)?;
        let mut inner = Inner::default();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(TEMP_PREFIX) {
                let _ = fs::remove_file(&path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some(EXTENSION) {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            inner.disk_bytes += meta.len();
            inner.disk.insert(
                name,
                DiskEntry {
                    size: meta.len(),
                    used: meta.modified().unwrap_or(UNIX_EPOCH),
                },
            );
        }
        Ok(ThumbnailCache {
            dir: dir.to_path_buf(),
            limits,
            inner: Mutex::new(inner),
            temp_counter: AtomicU64::new(0),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 메모리에 넣고 상한을 넘으면 가장 오래 쓰지 않은 것부터 민다.
    fn remember(&self, inner: &mut Inner, name: &str, key: &ThumbnailKey, t: &Arc<Thumbnail>) {
        let size = t.rgba.len() as u64;
        if size > self.limits.memory_bytes {
            return;
        }
        inner.tick += 1;
        let tick = inner.tick;
        if let Some(old) = inner.memory.insert(
            name.to_owned(),
            MemoryEntry {
                key: key.clone(),
                thumbnail: Arc::clone(t),
                tick,
            },
        ) {
            inner.memory_bytes -= old.thumbnail.rgba.len() as u64;
        }
        inner.memory_bytes += size;
        while inner.memory_bytes > self.limits.memory_bytes {
            let Some(oldest) = inner
                .memory
                .iter()
                .min_by_key(|(_, e)| e.tick)
                .map(|(n, _)| n.clone())
            else {
                break;
            };
            if let Some(e) = inner.memory.remove(&oldest) {
                inner.memory_bytes -= e.thumbnail.rgba.len() as u64;
            }
        }
    }

    /// #328: 키로 꺼낸다. 깨진 파일은 지우고 없는 것으로 본다.
    pub fn get(&self, key: &ThumbnailKey) -> Option<Arc<Thumbnail>> {
        let key_bytes = encode_key(key);
        let name = file_name(&key_bytes);
        {
            let mut inner = self.lock();
            inner.tick += 1;
            let tick = inner.tick;
            let hit = match inner.memory.get_mut(&name) {
                Some(e) if e.key == *key => {
                    e.tick = tick;
                    Some(Arc::clone(&e.thumbnail))
                }
                _ => None,
            };
            if let Some(found) = hit {
                if let Some(d) = inner.disk.get_mut(&name) {
                    d.used = SystemTime::now();
                }
                return Some(found);
            }
            if !inner.disk.contains_key(&name) {
                return None;
            }
        }
        let path = self.dir.join(&name);
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.forget_disk(&name);
                return None;
            }
            Err(_) => return None,
        };
        match decode_file(&bytes, &key_bytes) {
            Decoded::Hit(t) => {
                let t = Arc::new(t);
                let now = SystemTime::now();
                if let Ok(f) = fs::OpenOptions::new().write(true).open(&path) {
                    let _ = f.set_modified(now);
                }
                let mut inner = self.lock();
                if let Some(d) = inner.disk.get_mut(&name) {
                    d.used = now;
                }
                self.remember(&mut inner, &name, key, &t);
                Some(t)
            }
            Decoded::Other => None,
            Decoded::Broken => {
                let _ = fs::remove_file(&path);
                self.forget_disk(&name);
                None
            }
        }
    }

    fn forget_disk(&self, name: &str) {
        let mut inner = self.lock();
        if let Some(d) = inner.disk.remove(name) {
            inner.disk_bytes -= d.size;
        }
        if let Some(m) = inner.memory.remove(name) {
            inner.memory_bytes -= m.thumbnail.rgba.len() as u64;
        }
    }

    /// #328: 넣는다(디스크에 압축해 쓰고 메모리에도 둔다).
    pub fn put(&self, key: &ThumbnailKey, thumbnail: Arc<Thumbnail>) -> std::io::Result<()> {
        let key_bytes = encode_key(key);
        let name = file_name(&key_bytes);
        let bytes = encode_file(&key_bytes, &thumbnail)?;
        let temp = self.dir.join(format!(
            "{TEMP_PREFIX}{}-{}",
            std::process::id(),
            self.temp_counter.fetch_add(1, Ordering::Relaxed)
        ));
        let written =
            fs::write(&temp, &bytes).and_then(|()| fs::rename(&temp, self.dir.join(&name)));
        if let Err(e) = written {
            let _ = fs::remove_file(&temp);
            return Err(e);
        }
        let mut inner = self.lock();
        let size = bytes.len() as u64;
        if let Some(old) = inner.disk.insert(
            name.clone(),
            DiskEntry {
                size,
                used: SystemTime::now(),
            },
        ) {
            inner.disk_bytes -= old.size;
        }
        inner.disk_bytes += size;
        self.remember(&mut inner, &name, key, &thumbnail);
        Ok(())
    }

    /// #328: 디스크 상한 아래가 될 때까지 가장 오래 쓰지 않은 것부터 지운다.
    pub fn trim(&self) -> std::io::Result<()> {
        let mut inner = self.lock();
        if inner.disk_bytes <= self.limits.disk_bytes {
            return Ok(());
        }
        let mut order: Vec<(SystemTime, String)> = inner
            .disk
            .iter()
            .map(|(n, e)| (e.used, n.clone()))
            .collect();
        order.sort();
        let mut result = Ok(());
        for (_, name) in order {
            if inner.disk_bytes <= self.limits.disk_bytes {
                break;
            }
            if let Err(e) = Self::remove_entry(&self.dir, &mut inner, &name) {
                result = Err(e);
            }
        }
        result
    }

    fn remove_entry(dir: &Path, inner: &mut Inner, name: &str) -> std::io::Result<()> {
        match fs::remove_file(dir.join(name)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        if let Some(d) = inner.disk.remove(name) {
            inner.disk_bytes -= d.size;
        }
        if let Some(m) = inner.memory.remove(name) {
            inner.memory_bytes -= m.thumbnail.rgba.len() as u64;
        }
        Ok(())
    }

    /// #328: 모두 비운다.
    pub fn clear(&self) -> std::io::Result<()> {
        let mut inner = self.lock();
        let mut result = Ok(());
        let names: Vec<String> = inner.disk.keys().cloned().collect();
        for name in names {
            if let Err(e) = Self::remove_entry(&self.dir, &mut inner, &name) {
                result = Err(e);
            }
        }
        inner.memory.clear();
        inner.memory_bytes = 0;
        result
    }

    /// #328: 디스크에 둔 크기.
    pub fn disk_bytes(&self) -> u64 {
        self.lock().disk_bytes
    }

    /// #328: 메모리에 둔 크기.
    pub fn memory_bytes(&self) -> u64 {
        self.lock().memory_bytes
    }
}
