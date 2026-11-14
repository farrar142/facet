//! 공급자: 한 종류의 위치를 읽고 조작하는 방법(ADR-0004).
//!
//! 지금은 파일 시스템 공급자 하나뿐이다. 압축 파일(S13)과 셸 공급자(S12)가 같은
//! 인터페이스를 구현한다. 주소창 글자 해석과 변경 감시는 뒤 티켓이 여기에 더한다.

use crate::file_system::FileSystemProvider;
use crate::{Item, ListError, Location};

/// 한 종류의 위치를 읽는 방법.
///
/// 메서드는 이 공급자의 위치만 받는다. 어느 공급자에 넘길지는 [`provider_for`]가 고른다.
pub trait Provider: Send + Sync {
    /// `location`의 항목 목록을 읽는다. 순서는 공급자가 돌려준 순서이며 정렬하지 않는다.
    fn list(&self, location: &Location) -> Result<Vec<Item>, ListError>;

    /// `location`의 상위 위치. 더 올라갈 곳이 없으면(드라이브 루트 등) `None`이다.
    fn parent(&self, location: &Location) -> Option<Location>;

    /// 주소창에 보일 경로 조각. 맨 위 조각부터 `location` 자신까지 차례로 담는다.
    fn segments(&self, location: &Location) -> Vec<PathSegment>;
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
    }
}
