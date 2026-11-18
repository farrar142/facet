//! 공급자: 한 종류의 위치를 읽고 조작하는 방법(ADR-0004).
//!
//! 파일 시스템 공급자, 검색 결과 공급자, 셸 공급자(가상 폴더, S12)가 있다. 압축 파일(S13)도 같은 인터페이스를
//! 구현한다.

use std::ffi::OsStr;

use crate::file_system::FileSystemProvider;
use crate::watch::{Watch, WatchError, WatchEvent};
use crate::{Item, ListError, Location};

/// 한 종류의 위치를 읽는 방법.
///
/// 메서드는 이 공급자의 위치만 받는다. 어느 공급자에 넘길지는 [`provider_for`]가 고른다.
pub trait Provider: Send + Sync {
    /// `location`의 항목 목록을 읽는다. 순서는 공급자가 돌려준 순서이며 정렬하지 않는다.
    fn list(&self, location: &Location) -> Result<Vec<Item>, ListError>;

    /// `location` 안의 이름이 `name`인 항목의 위치. 있는지는 묻지 않는다.
    fn child(&self, location: &Location, name: &OsStr) -> Location;

    /// `location`의 상위 위치. 더 올라갈 곳이 없으면(드라이브 루트 등) `None`이다.
    fn parent(&self, location: &Location) -> Option<Location>;

    /// 주소창에 보일 경로 조각. 맨 위 조각부터 `location` 자신까지 차례로 담는다.
    fn segments(&self, location: &Location) -> Vec<PathSegment>;

    /// 주소창에 친 글자를 위치로 해석한다. 파일 시스템에 한 번 묻지만 파일을 열지는 않는다.
    ///
    /// 실패하면 까닭이 담긴 오류다.
    fn resolve(&self, text: &str) -> Result<Resolved, ResolveError>;

    /// 고쳐 쓰는 글자의 자동 완성 후보. 하위 폴더만, 이름순으로 낸다. 폴더를 읽느라 막히므로
    /// [`crate::tab::CompletionRequest`]로 UI 스레드 밖에서 부른다.
    fn complete(&self, text: &str) -> Vec<Candidate>;

    /// `location`의 변경 감시를 건다. 바뀐 항목(하위 위치 안은 빼고)은 감시 스레드에서 `on_event`로 알린다.
    /// 돌려준 [`Watch`]를 버리면 감시가 풀리고 그 뒤로는 `on_event`를 부르지 않는다.
    ///
    /// 감시를 걸 수 없는 위치면 까닭이 담긴 오류다. 묶기와 항목 다시 읽기는 [`crate::tab`]이 한다.
    fn watch(
        &self,
        location: &Location,
        on_event: Box<dyn FnMut(WatchEvent) + Send>,
    ) -> Result<Watch, WatchError>;

    /// `location` 안의 이름이 `name`인 항목 하나를 읽는다. 없으면 `Ok(None)`이다. 변경 알림을 받은 항목만
    /// 다시 읽는 데 쓴다.
    fn item(&self, location: &Location, name: &OsStr) -> Result<Option<Item>, ListError>;

    /// `location`의 되는 일. 기본값은 실제 폴더와 같다: 복원·비우기 말고 모두 된다.
    fn capabilities(&self, _location: &Location) -> Capabilities {
        Capabilities {
            restore: false,
            empty: false,
            ..Capabilities::ALL
        }
    }
}

/// 공급자가 위치마다 알리는 되는 일.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities {
    pub open: bool,
    pub copy_from: bool,
    pub copy_into: bool,
    pub move_items: bool,
    pub recycle: bool,
    pub delete_permanently: bool,
    pub rename: bool,
    pub new_folder: bool,
    pub restore: bool,
    pub empty: bool,
    pub properties: bool,
    pub undoable: bool,
}

impl Capabilities {
    /// 모든 되는 일이 켜진 값.
    pub const ALL: Capabilities = Capabilities {
        open: true,
        copy_from: true,
        copy_into: true,
        move_items: true,
        recycle: true,
        delete_permanently: true,
        rename: true,
        new_folder: true,
        restore: true,
        empty: true,
        properties: true,
        undoable: true,
    };
}

/// 주소창 글자를 해석한 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// 갈 위치. 파일 경로를 쳤으면 그 파일이 든 폴더다.
    pub location: Location,
    /// 파일 경로를 쳤으면 도착해서 선택할 파일의 이름.
    pub select: Option<std::ffi::OsString>,
}

/// 자동 완성 후보 하나: 하위 폴더.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// 폴더 이름.
    pub name: std::ffi::OsString,
    /// 그 폴더의 위치.
    pub location: Location,
}

/// 주소창 글자를 해석하지 못한 까닭.
#[derive(Debug)]
pub enum ResolveError {
    /// 글자가 비었다.
    Empty,
    /// 경로에 쓸 수 없는 글자가 있다.
    InvalidCharacter { character: char },
    /// 환경 변수가 정의돼 있지 않다.
    UndefinedVariable { name: String },
    /// 전체 경로가 아니다.
    NotAbsolute { text: String },
    /// 경로가 없다.
    NotFound { location: Location },
    /// 경로를 확인할 권한이 없다.
    PermissionDenied { location: Location },
    /// 그 밖의 입출력 오류.
    Io {
        location: Location,
        source: std::io::Error,
    },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "주소가 비었습니다"),
            Self::InvalidCharacter { character } => {
                write!(f, "주소에 쓸 수 없는 글자가 있습니다: {character:?}")
            }
            Self::UndefinedVariable { name } => write!(f, "환경 변수가 없습니다: %{name}%"),
            Self::NotAbsolute { text } => write!(f, "전체 경로가 아닙니다: {text}"),
            Self::NotFound { location } => write!(f, "위치가 없습니다: {location}"),
            Self::PermissionDenied { location } => {
                write!(f, "위치를 확인할 권한이 없습니다: {location}")
            }
            Self::Io { location, source } => {
                write!(f, "위치를 확인하지 못했습니다: {location}: {source}")
            }
        }
    }
}

impl std::error::Error for ResolveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// 주소창의 경로 조각 하나. 누르면 [`PathSegment::location`]으로 간다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathSegment {
    /// 화면에 보일 이름.
    pub label: String,
    /// 이 조각이 가리키는 위치.
    pub location: Location,
}

/// `location`을 읽는 공급자.
pub fn provider_for(location: &Location) -> &'static dyn Provider {
    match location {
        Location::FileSystem(_) => &FileSystemProvider,
        Location::SearchResults(_) => &crate::search_results::SearchResultsProvider,
        Location::Shell(_) => &crate::shell_provider::ShellProvider,
    }
}
