//! zip 공급자: zip 파일 안을 위치로 연다(ADR-0004, #423).
//!
//! 위치는 「zip 파일의 실제 경로 + zip 안 경로」([`crate::ZipLocation`])다. 목록은 자체 리더
//! ([`crate::zip_archive::Archive`])로 central directory만 읽어 만들고, 연 zip은 파일의 경로·크기·수정 시각을 키로
//! 프로세스 안에 몇 개 보관해 같은 zip 안에서 폴더를 옮길 때 다시 읽지 않는다. 목록 읽기는 탭의 읽기 일꾼 스레드에서
//! 한다(UI 스레드는 zip을 읽지 않는다).
//!
//! zip 안에서는 읽기만 한다: 되는 일은 열기·꺼내기(복사의 원본)·넣기(복사의 대상)뿐이다([`Provider::capabilities`]).
//! 바깥 변경은 따라가지 않는다(감시를 걸 수 없어 「자동 새로 고침 안 됨」). 화면 문장은 없다(ADR-0007).

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::file_system::FileSystemProvider;
use crate::provider::{Candidate, Capabilities, PathSegment, Provider, ResolveError, Resolved};
use crate::selection::starts_with_folded;
use crate::watch::{Watch, WatchError, WatchEvent};
use crate::zip_archive::{Archive, ArchiveEntry, ArchiveError};
use crate::{Attributes, Item, ListError, Location, ZipLocation};

/// 보관하는 연 zip의 수. 넘으면 가장 오래 쓰지 않은 것을 버린다.
const CACHE_LIMIT: usize = 8;

/// zip 파일의 시그니처들: 로컬 머리(보통), 빈 zip의 끝 기록, 나눠 쓴 zip의 표시.
const SIGNATURES: [[u8; 4]; 3] = [*b"PK\x03\x04", *b"PK\x05\x06", *b"PK\x07\x08"];

/// 확장자 `.zip`(대소문자 무시)이고 파일 앞 4바이트가 zip 시그니처인 파일인가. 폴더·없는 파일·읽을 수 없는 파일은
/// 아니다. `.docx`·`.jar` 같은 zip 기반 형식은 위치로 열지 않는다. 파일 앞 4바이트만 읽는다.
pub fn is_zip_file(path: &Path) -> bool {
    if !has_zip_extension(path) {
        return false;
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return false;
    }
    let mut head = [0u8; 4];
    file.read_exact(&mut head).is_ok() && SIGNATURES.contains(&head)
}

/// 이름의 확장자가 `.zip`(대소문자 무시)인가.
fn has_zip_extension(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
}

/// zip 위치의 상태: 같은 경로가 앞에 또 있어 가려진 기록 수와 central directory 일부를 읽지 못했는지.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ArchiveNotes {
    /// 같은 경로가 둘 이상이라 가려진(앞에 나온) 기록 수. 목록에는 뒤에 나온 기록이 보인다.
    pub duplicates: usize,
    /// central directory 일부를 읽지 못해 읽을 수 있는 항목만으로 목록을 만들었다.
    pub damaged: bool,
}

/// zip 위치의 목록과 그 위치에만 있는 값.
#[derive(Debug, Clone)]
pub(crate) struct ZipListing {
    /// 항목들(이름순 아님).
    pub items: Vec<Item>,
    /// 항목마다 압축된 크기(`items`와 같은 차례).
    pub compressed: Vec<u64>,
    /// 같은 이름·손상 상태.
    pub notes: ArchiveNotes,
}

/// 보관한 zip의 키: 경로·크기·수정 시각. 셋 중 하나가 바뀌면 다시 읽는다.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CacheKey {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
}

/// 연 zip 보관함(가장 최근에 쓴 것이 끝).
static CACHE: Mutex<Vec<(CacheKey, Arc<Archive>)>> = Mutex::new(Vec::new());

/// zip 파일을 연다(보관한 것이 있으면 그것). 파일 메타데이터를 한 번 읽는다.
fn open_archive(path: &Path) -> Result<Arc<Archive>, ArchiveError> {
    let metadata = std::fs::metadata(path).map_err(ArchiveError::Io)?;
    if metadata.is_dir() {
        return Err(ArchiveError::Io(std::io::Error::from(
            std::io::ErrorKind::NotADirectory,
        )));
    }
    let key = CacheKey {
        path: path.to_path_buf(),
        len: metadata.len(),
        modified: metadata.modified().ok(),
    };
    {
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(at) = cache.iter().position(|(k, _)| *k == key) {
            let entry = cache.remove(at);
            let archive = Arc::clone(&entry.1);
            cache.push(entry);
            return Ok(archive);
        }
    }
    let archive = Arc::new(Archive::open(path)?);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|(k, _)| k.path != key.path);
    if cache.len() >= CACHE_LIMIT {
        cache.remove(0);
    }
    cache.push((key, Arc::clone(&archive)));
    Ok(archive)
}

/// zip 안 경로(`\` 구분)를 리더의 경로(`/` 구분)로.
fn archive_path(inner: &str) -> String {
    inner.replace('\\', "/")
}

/// 리더의 항목을 목록 항목으로. 시각을 모르면 유닉스 시각 0이다.
fn item_of(entry: &ArchiveEntry) -> Item {
    Item {
        name: OsString::from(&entry.name),
        is_folder: entry.is_folder,
        size: if entry.is_folder { 0 } else { entry.size },
        modified: entry.modified.unwrap_or(SystemTime::UNIX_EPOCH),
        attributes: Attributes::default(),
    }
}

/// 공급자가 받은 zip 위치. 다른 위치면 `None`.
fn zip_of(location: &Location) -> Option<&ZipLocation> {
    location.zip_location()
}

/// zip을 열지 못한 까닭을 목록 읽기 오류로.
fn list_error(location: &Location, error: ArchiveError) -> ListError {
    match error {
        ArchiveError::NotZip | ArchiveError::Damaged => ListError::BrokenArchive {
            location: location.clone(),
        },
        ArchiveError::Io(source) => ListError::from_io(location.clone(), source),
    }
}

/// zip 위치의 목록을 읽는다: 항목·압축된 크기·상태. 막으므로 UI 스레드에서 부르지 않는다.
pub(crate) fn list_detailed(location: &Location) -> Result<ZipListing, ListError> {
    let Some(zip) = zip_of(location) else {
        return Err(ListError::NotFound {
            location: location.clone(),
        });
    };
    let archive = open_archive(zip.archive()).map_err(|e| list_error(location, e))?;
    let Some(children) = archive.children(&archive_path(zip.inner())) else {
        return Err(ListError::NotFound {
            location: location.clone(),
        });
    };
    Ok(ZipListing {
        items: children.iter().map(item_of).collect(),
        compressed: children
            .iter()
            .map(|e| if e.is_folder { 0 } else { e.compressed_size })
            .collect(),
        notes: ArchiveNotes {
            duplicates: archive.duplicates(),
            damaged: archive.damaged(),
        },
    })
}

/// 경로 `path`가 zip 파일 안을 가리키면 그 위치로 해석한다: 경로의 앞쪽 가운데 실제로 있는 가장 깊은 곳이 zip
/// 파일([`is_zip_file`])이면 나머지가 zip 안 경로다. zip 안 폴더면 그 위치, zip 안 파일이면 그 폴더에서 그 파일을
/// 고른다. `path` 자신이 zip 파일이면 zip 루트다. zip과 상관없으면 `None`.
pub(crate) fn resolve_in_zip(path: &Path) -> Option<Result<Resolved, ResolveError>> {
    let (archive, inner) = split_archive(path)?;
    let location = Location::zip(&archive, inner);
    let zip = location.zip_location()?.clone();
    let opened = match open_archive(&archive) {
        Ok(opened) => opened,
        Err(_) => {
            // 열 수 없는 zip이어도 그 위치로 가서 목록 읽기 오류로 까닭을 보인다.
            return Some(Ok(Resolved {
                location,
                select: None,
            }));
        }
    };
    if opened.children(&archive_path(zip.inner())).is_some() {
        return Some(Ok(Resolved {
            location,
            select: None,
        }));
    }
    // 파일이면 그 폴더에서 고른다.
    let (parent, name) = match zip.inner().rsplit_once('\\') {
        Some((parent, name)) => (parent, name),
        None => ("", zip.inner()),
    };
    let is_file = opened
        .children(&archive_path(parent))
        .is_some_and(|children| children.iter().any(|e| e.name == name && !e.is_folder));
    if is_file {
        return Some(Ok(Resolved {
            location: Location::zip(&archive, parent),
            select: Some(OsString::from(name)),
        }));
    }
    Some(Err(ResolveError::NotFound { location }))
}

/// 경로를 zip 파일과 zip 안 경로로 가른다. 경로 자신이나 앞쪽 하나가 zip 파일이어야 한다.
fn split_archive(path: &Path) -> Option<(PathBuf, String)> {
    let mut rest: Vec<&OsStr> = Vec::new();
    let mut current = path;
    loop {
        if has_zip_extension(current) && is_zip_file(current) {
            let inner = rest
                .iter()
                .rev()
                .map(|part| part.to_string_lossy())
                .collect::<Vec<_>>()
                .join("\\");
            return Some((current.to_path_buf(), inner));
        }
        // 실제로 있는 폴더에 닿으면 그 위에 zip은 없다(zip 안 경로가 아니다).
        if current.is_dir() {
            return None;
        }
        rest.push(current.file_name()?);
        current = current.parent()?;
    }
}

/// zip 안 폴더 글자의 자동 완성: 마지막 구분자 앞까지가 zip 위치이면 그 하위 폴더 가운데 뒤쪽 글자로 시작하는
/// 이름을 이름순으로 낸다.
pub(crate) fn complete_in_zip(dir: &Path, typed: &str) -> Vec<Candidate> {
    let Some((archive, inner)) = split_archive(dir) else {
        return Vec::new();
    };
    let Ok(opened) = open_archive(&archive) else {
        return Vec::new();
    };
    let Some(children) = opened.children(&archive_path(&inner)) else {
        return Vec::new();
    };
    let prefix: Vec<char> = typed.chars().flat_map(char::to_lowercase).collect();
    let folder = Location::zip(&archive, inner);
    let mut found: Vec<Candidate> = children
        .iter()
        .filter(|e| e.is_folder && starts_with_folded(OsStr::new(&e.name), &prefix))
        .map(|e| Candidate {
            name: OsString::from(&e.name),
            location: ZipProvider.child(&folder, OsStr::new(&e.name)),
        })
        .collect();
    found.sort_by_cached_key(|c| c.name.to_string_lossy().to_lowercase());
    found
}

/// zip 파일 안을 읽는 공급자.
#[derive(Debug, Clone, Copy, Default)]
pub struct ZipProvider;

impl Provider for ZipProvider {
    /// zip 안 폴더의 바로 아래 항목. 폴더 기록이 없는 중간 폴더도 폴더 항목이다. 열 수 없는 zip은
    /// [`ListError::BrokenArchive`], zip 안에 없는 폴더는 [`ListError::NotFound`]다.
    fn list(&self, location: &Location) -> Result<Vec<Item>, ListError> {
        list_detailed(location).map(|listing| listing.items)
    }

    fn child(&self, location: &Location, name: &OsStr) -> Location {
        let Some(zip) = zip_of(location) else {
            return location.clone();
        };
        let name = name.to_string_lossy();
        if zip.inner().is_empty() {
            Location::zip(zip.archive(), name.into_owned())
        } else {
            Location::zip(zip.archive(), format!("{}\\{name}", zip.inner()))
        }
    }

    /// zip 안 폴더의 위는 한 단계 위 zip 안 폴더, zip 루트의 위는 zip이 든 실제 폴더다.
    fn parent(&self, location: &Location) -> Option<Location> {
        let zip = zip_of(location)?;
        if zip.inner().is_empty() {
            return zip.archive().parent().map(Location::file_system);
        }
        let parent = zip
            .inner()
            .rsplit_once('\\')
            .map_or("", |(parent, _)| parent);
        Some(Location::zip(zip.archive(), parent))
    }

    /// zip이 든 실제 폴더까지의 조각, zip 파일 조각(zip 루트), zip 안 폴더 조각들.
    fn segments(&self, location: &Location) -> Vec<PathSegment> {
        let Some(zip) = zip_of(location) else {
            return Vec::new();
        };
        let mut segments =
            FileSystemProvider.segments(&Location::file_system(zip.archive().to_path_buf()));
        let root = Location::zip(zip.archive(), "");
        match segments.last_mut() {
            Some(last) => last.location = root,
            None => segments.push(PathSegment {
                label: zip.archive().display().to_string(),
                location: root,
            }),
        }
        let mut inner = String::new();
        for part in zip.inner().split('\\').filter(|p| !p.is_empty()) {
            if !inner.is_empty() {
                inner.push('\\');
            }
            inner.push_str(part);
            segments.push(PathSegment {
                label: part.to_string(),
                location: Location::zip(zip.archive(), inner.clone()),
            });
        }
        segments
    }

    /// 주소창 글자는 파일 시스템 공급자와 같게 해석한다(zip 안 경로도 그쪽이 알아본다).
    fn resolve(&self, text: &str) -> Result<Resolved, ResolveError> {
        FileSystemProvider.resolve(text)
    }

    fn complete(&self, text: &str) -> Vec<Candidate> {
        FileSystemProvider.complete(text)
    }

    /// 바깥 변경은 아직 따라가지 않는다: 감시를 걸 수 없다(「자동 새로 고침 안 됨」).
    fn watch(
        &self,
        location: &Location,
        _on_event: Box<dyn FnMut(WatchEvent) + Send>,
    ) -> Result<Watch, WatchError> {
        Err(WatchError::Unsupported {
            location: location.clone(),
        })
    }

    fn item(&self, location: &Location, name: &OsStr) -> Result<Option<Item>, ListError> {
        Ok(self
            .list(location)?
            .into_iter()
            .find(|item| item.name == name))
    }

    /// 열기·꺼내기(복사의 원본)·넣기(복사의 대상)만 된다.
    fn capabilities(&self, _location: &Location) -> Capabilities {
        Capabilities {
            open: true,
            copy_from: true,
            copy_into: true,
            ..Capabilities::default()
        }
    }
}
