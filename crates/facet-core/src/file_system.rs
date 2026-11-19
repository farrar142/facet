//! 파일 시스템 공급자: 실제 폴더를 파일 시스템 API로 직접 읽는다(ADR-0004).

use std::fs::{self, Metadata};
use std::io;
use std::path::{Component, Path, PathBuf, Prefix};

use crate::provider::{Candidate, PathSegment, Provider, ResolveError, Resolved};
use crate::selection::starts_with_folded;
use crate::watch::{Watch, WatchError, WatchEvent};
use crate::{Attributes, Item, ListError, Location};

/// 로컬, 네트워크, 동기화 폴더 같은 실제 경로를 읽는 공급자.
///
/// 260자를 넘는 경로도 읽는다. 표준 라이브러리가 Windows에서 긴 절대 경로에
/// `\\?\` 접두어를 붙여 넘기므로 따로 할 일이 없다.
#[derive(Debug, Clone, Copy, Default)]
pub struct FileSystemProvider;

impl Provider for FileSystemProvider {
    /// 실제 폴더의 항목 목록을 읽는다.
    ///
    /// 숨김 항목도 포함한다. 순서는 파일 시스템이 돌려준 순서이며 정렬하지 않는다.
    /// 항목마다 시스템 호출을 더 하지 않는다: Windows에서는 디렉터리 열거 결과에
    /// 크기·시각·속성이 이미 들어 있어 [`DirEntry::metadata`]가 그것을 재사용한다.
    fn list(&self, location: &Location) -> Result<Vec<Item>, ListError> {
        let path = path_of(location);
        let entries = fs::read_dir(path).map_err(|e| ListError::from_io(location.clone(), e))?;
        let mut items = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| ListError::from_io(location.clone(), e))?;
            let entry_error = |e| ListError::from_io(Location::file_system(entry.path()), e);
            let metadata = entry.metadata().map_err(entry_error)?;
            items.push(
                make_item(entry.file_name(), &|| entry.path(), &metadata).map_err(entry_error)?,
            );
        }
        Ok(items)
    }

    /// 폴더 안 항목 하나를 읽는다. 목록 읽기와 같게 링크를 따라가지 않고 링크 자신의 정보를 읽는다.
    fn item(&self, location: &Location, name: &std::ffi::OsStr) -> Result<Option<Item>, ListError> {
        let path = path_of(location).join(name);
        let error = |e| ListError::from_io(Location::file_system(&path), e);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(error(e)),
        };
        make_item(name.to_os_string(), &|| path.clone(), &metadata)
            .map(Some)
            .map_err(error)
    }

    /// `ReadDirectoryChangesW`로 폴더 하나(하위 폴더 안은 빼고)를 감시한다. Windows가 아니면 감시할 수 없다.
    fn watch(
        &self,
        location: &Location,
        mut on_event: Box<dyn FnMut(WatchEvent) + Send>,
    ) -> Result<Watch, WatchError> {
        use facet_win::{DirectoryChange, DirectoryWatcher};
        let watcher = DirectoryWatcher::watch(path_of(location), move |change| {
            on_event(match change {
                DirectoryChange::Changed(name) => WatchEvent::Changed(name),
                DirectoryChange::Overflow => WatchEvent::Overflow,
                DirectoryChange::Ended(_) => WatchEvent::Ended,
            });
        })
        .map_err(|source| match source.kind() {
            io::ErrorKind::Unsupported => WatchError::Unsupported {
                location: location.clone(),
            },
            _ => WatchError::Io {
                location: location.clone(),
                source,
            },
        })?;
        Ok(Watch::new(watcher))
    }

    /// 폴더 안의 항목 경로. 글자로만 잇고 파일 시스템에 묻지 않는다.
    fn child(&self, location: &Location, name: &std::ffi::OsStr) -> Location {
        Location::file_system(path_of(location).join(name))
    }

    /// 한 단계 위 폴더. 드라이브 루트(`C:\`)와 네트워크 공유 루트(`\\server\share`)에는 없다.
    ///
    /// 경로를 글자로만 다루고 파일 시스템에 묻지 않는다. `..`을 풀지 않는다.
    fn parent(&self, location: &Location) -> Option<Location> {
        let parent = path_of(location).parent()?;
        if parent.as_os_str().is_empty() {
            return None;
        }
        Some(Location::file_system(parent))
    }

    /// 드라이브(또는 네트워크 공유, 루트)부터 이 폴더까지의 조각.
    ///
    /// 드라이브 조각의 이름은 `C:`이고 위치는 `C:\`다. 긴 경로 접두어(`\\?\`)는
    /// 이름에서 빼고 위치에는 그대로 둔다.
    fn segments(&self, location: &Location) -> Vec<PathSegment> {
        let mut segments = Vec::new();
        let mut current = PathBuf::new();
        let mut components = path_of(location).components().peekable();
        while let Some(component) = components.next() {
            current.push(component.as_os_str());
            let label = match component {
                Component::Prefix(prefix) => {
                    // 루트 구분자는 드라이브 조각에 붙인다.
                    if components.next_if_eq(&Component::RootDir).is_some() {
                        current.push(Component::RootDir.as_os_str());
                    }
                    prefix_label(prefix.kind(), component.as_os_str().to_string_lossy())
                }
                Component::RootDir => component.as_os_str().to_string_lossy().into_owned(),
                other => other.as_os_str().to_string_lossy().into_owned(),
            };
            segments.push(PathSegment {
                label,
                location: Location::file_system(current.clone()),
            });
        }
        segments
    }

    /// 주소창 글자를 해석한다.
    ///
    /// 앞뒤 공백과 감싼 큰따옴표를 걷고 `%NAME%`을 환경 변수 값으로 펼친다. 끝 구분자는 있어도 없어도
    /// 같다. 전체 경로여야 한다. 폴더면 그 폴더로, 파일이면 파일을 열지 않고 그 폴더로 가서 파일을 고르게 한다.
    /// 가상 폴더의 이름(`내 PC`·`shell:MyComputerFolder`)이면 그 위치다.
    fn resolve(&self, text: &str) -> Result<Resolved, ResolveError> {
        if let Some(location) = crate::shell_provider::resolve_alias(text) {
            return Ok(Resolved {
                location,
                select: None,
            });
        }
        resolve_path(parse_address(text)?)
    }

    /// 마지막 구분자 앞까지를 폴더로 보고, 그 하위 폴더 중 뒤쪽 글자로 시작하는 이름을 이름순으로 낸다.
    /// 대소문자를 가리지 않는다. 폴더를 읽을 수 없거나 글자를 해석할 수 없으면 후보가 없다.
    fn complete(&self, text: &str) -> Vec<Candidate> {
        let text = text.trim_start();
        let Some(split) = text.rfind(['\\', '/']) else {
            return Vec::new();
        };
        let (dir_text, typed) = text.split_at(split + 1);
        let Ok(dir) = parse_address(dir_text) else {
            return Vec::new();
        };
        let prefix: Vec<char> = typed.chars().flat_map(char::to_lowercase).collect();
        let Ok(entries) = fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut found: Vec<Candidate> = entries
            .filter_map(Result::ok)
            .filter(|entry| starts_with_folded(&entry.file_name(), &prefix))
            .filter(|entry| match entry.file_type() {
                Ok(kind) if kind.is_symlink() => {
                    fs::metadata(entry.path()).is_ok_and(|m| m.is_dir())
                }
                Ok(kind) => kind.is_dir(),
                Err(_) => false,
            })
            .map(|entry| Candidate {
                name: entry.file_name(),
                location: Location::file_system(entry.path()),
            })
            .collect();
        found.sort_by_cached_key(|c| c.name.to_string_lossy().to_lowercase());
        found
    }
}

/// 전체 경로 하나를 위치로 해석한다(주소창과 실행 인자가 함께 쓴다). 파일 시스템에 한 번 묻는다. 폴더면 그
/// 폴더로, 파일이면 그 폴더로 가서 파일을 고르게 한다.
pub(crate) fn resolve_path(path: PathBuf) -> Result<Resolved, ResolveError> {
    let location = Location::file_system(&path);
    let metadata = fs::metadata(&path).map_err(|source| match source.kind() {
        io::ErrorKind::NotFound => ResolveError::NotFound {
            location: location.clone(),
        },
        io::ErrorKind::PermissionDenied => ResolveError::PermissionDenied {
            location: location.clone(),
        },
        _ => ResolveError::Io {
            location: location.clone(),
            source,
        },
    })?;
    if !metadata.is_dir()
        && let (Some(parent), Some(name)) = (path.parent(), path.file_name())
    {
        return Ok(Resolved {
            location: Location::file_system(parent),
            select: Some(name.to_os_string()),
        });
    }
    Ok(Resolved {
        location,
        select: None,
    })
}

/// 주소창 글자를 경로로 바꾼다: 다듬고, 환경 변수를 펼치고, 글자를 검사하고, 끝 구분자를 없앤다. 전체 경로가
/// 아니면 오류다.
fn parse_address(text: &str) -> Result<PathBuf, ResolveError> {
    let (expanded, normalized) = parse(text)?;
    if !normalized.is_absolute() {
        return Err(ResolveError::NotAbsolute { text: expanded });
    }
    Ok(normalized)
}

/// 주소 글자를 경로로 바꾼다: 다듬고, 감싼 큰따옴표를 걷고, 환경 변수를 펼치고, 글자를 검사하고, 끝 구분자를
/// 없앤다. 상대 경로일 수 있다(실행 인자는 작업 폴더 기준으로 푼다).
pub(crate) fn parse_text(text: &str) -> Result<PathBuf, ResolveError> {
    parse(text).map(|(_, normalized)| normalized)
}

/// [`parse_text`]와 같고, 펼친 글자도 함께 돌려준다.
fn parse(text: &str) -> Result<(String, PathBuf), ResolveError> {
    let text = text.trim();
    let text = text
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(text)
        .trim();
    if text.is_empty() {
        return Err(ResolveError::Empty);
    }
    let expanded = expand_variables(text)?;
    // 긴 경로 접두어(`\\?\`)의 물음표는 글자로 치지 않는다.
    let body = expanded.strip_prefix(r"\\?\").unwrap_or(&expanded);
    if let Some(character) = body
        .chars()
        .find(|c| matches!(c, '<' | '>' | '"' | '|' | '?' | '*') || c.is_control())
    {
        return Err(ResolveError::InvalidCharacter { character });
    }
    // 구성 요소로 다시 이어 끝 구분자와 겹친 구분자를 정리한다.
    let normalized: PathBuf = Path::new(&expanded).components().collect();
    Ok((expanded, normalized))
}

/// `%NAME%`을 환경 변수 값으로 바꾼다. 짝이 없는 `%`는 그대로 둔다.
fn expand_variables(text: &str) -> Result<String, ResolveError> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('%') else {
            break;
        };
        let name = &after[..end];
        out.push_str(&rest[..start]);
        if name.is_empty() {
            out.push('%');
        } else {
            let value = std::env::var_os(name).ok_or_else(|| ResolveError::UndefinedVariable {
                name: name.to_string(),
            })?;
            out.push_str(&value.to_string_lossy());
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// 이름과 메타데이터(링크를 따라가지 않은 것)로 항목을 만든다. Windows에서는 시스템 호출을 더 하지 않고
/// 경로도 만들지 않는다. `path`는 Windows가 아닌 곳에서 링크의 대상을 볼 때만 부른다.
pub(crate) fn make_item(
    name: std::ffi::OsString,
    path: &dyn Fn() -> PathBuf,
    metadata: &Metadata,
) -> io::Result<Item> {
    Ok(Item {
        is_folder: is_folder(path, metadata),
        size: metadata.len(),
        modified: metadata.modified()?,
        attributes: attributes(&name, metadata),
        name,
    })
}

/// 공급자가 받은 위치의 경로. 파일 시스템 공급자는 실제 폴더만 받지만, 잘못 넘어온 검색 결과 위치는 검색한 폴더로 본다.
/// 잘못 넘어온 가상 폴더 위치는 빈 경로다(읽으면 실패한다).
fn path_of(location: &Location) -> &Path {
    match location {
        Location::FileSystem(path) => path,
        Location::SearchResults(scope) => &scope.folder,
        Location::Shell(_) | Location::Zip(_) => Path::new(""),
    }
}

/// 드라이브·공유 접두어의 보이는 이름. 긴 경로 접두어(`\\?\`, `\\?\UNC\`)를 뺀다.
fn prefix_label(prefix: Prefix<'_>, raw: std::borrow::Cow<'_, str>) -> String {
    match prefix {
        Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
            format!("{}:", char::from(letter))
        }
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => format!(
            r"\\{}\{}",
            server.to_string_lossy(),
            share.to_string_lossy()
        ),
        Prefix::Verbatim(name) => name.to_string_lossy().into_owned(),
        Prefix::DeviceNS(_) => raw.into_owned(),
    }
}

#[cfg(windows)]
fn is_folder(_path: &dyn Fn() -> PathBuf, metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    // 폴더 심볼릭 링크와 정션도 디렉터리 속성을 가진다.
    metadata.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0
}

#[cfg(not(windows))]
fn is_folder(path: &dyn Fn() -> PathBuf, metadata: &Metadata) -> bool {
    if metadata.is_symlink() {
        // 링크일 때만 대상을 한 번 더 본다.
        fs::metadata(path()).is_ok_and(|m| m.is_dir())
    } else {
        metadata.is_dir()
    }
}

/// 열거 결과에 이미 든 속성 비트를 읽는다. 시스템 호출을 더 하지 않는다.
#[cfg(windows)]
fn attributes(_name: &std::ffi::OsStr, metadata: &Metadata) -> Attributes {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
    let bits = metadata.file_attributes();
    Attributes {
        hidden: bits & FILE_ATTRIBUTE_HIDDEN != 0,
        system: bits & FILE_ATTRIBUTE_SYSTEM != 0,
        read_only: bits & FILE_ATTRIBUTE_READONLY != 0,
        // 열거 결과의 속성 값만 본다(파일을 열면 내려받기가 일어난다).
        online_only: crate::content_search::is_online_only(bits),
        bits,
    }
}

/// Windows가 아닌 곳(개발용): 점으로 시작하는 이름을 숨김으로, 쓰기 권한이 없으면 읽기 전용으로 본다.
#[cfg(not(windows))]
fn attributes(name: &std::ffi::OsStr, metadata: &Metadata) -> Attributes {
    Attributes {
        hidden: name.as_encoded_bytes().starts_with(b"."),
        system: false,
        read_only: metadata.permissions().readonly(),
        online_only: false,
        bits: 0,
    }
}
