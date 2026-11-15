//! 파일 시스템 공급자: 실제 폴더를 파일 시스템 API로 직접 읽는다(ADR-0004).

use std::fs::{self, DirEntry, Metadata};
use std::path::{Component, Path, PathBuf, Prefix};

use crate::provider::{PathSegment, Provider};
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
            let name = entry.file_name();
            items.push(Item {
                is_folder: is_folder(&entry, &metadata),
                size: metadata.len(),
                modified: metadata.modified().map_err(entry_error)?,
                attributes: attributes(&name, &metadata),
                name,
            });
        }
        Ok(items)
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
}

fn path_of(location: &Location) -> &Path {
    let Location::FileSystem(path) = location;
    path
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
fn is_folder(_entry: &DirEntry, metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    // 폴더 심볼릭 링크와 정션도 디렉터리 속성을 가진다.
    metadata.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0
}

#[cfg(not(windows))]
fn is_folder(entry: &DirEntry, metadata: &Metadata) -> bool {
    if metadata.is_symlink() {
        // 링크일 때만 대상을 한 번 더 본다.
        fs::metadata(entry.path()).is_ok_and(|m| m.is_dir())
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
    }
}

/// Windows가 아닌 곳(개발용): 점으로 시작하는 이름을 숨김으로, 쓰기 권한이 없으면 읽기 전용으로 본다.
#[cfg(not(windows))]
fn attributes(name: &std::ffi::OsStr, metadata: &Metadata) -> Attributes {
    Attributes {
        hidden: name.as_encoded_bytes().starts_with(b"."),
        system: false,
        read_only: metadata.permissions().readonly(),
    }
}
