//! **검색 결과 위치**(GLOSSARY): 검색 결과를 탭 안의 위치처럼 보이는 것.
//!
//! - 위치는 [`Location::SearchResults`]이고 검색한 폴더와 검색어([`crate::SearchScope`])를 갖는다. 실제 폴더가 아니어서
//!   [`Location::file_system_path`]가 `None`이다. 탭 제목·주소창 경로 조각은 검색한 폴더의 조각 뒤에 검색어 조각 하나를
//!   붙인 것이다([`SearchResultsProvider`]). 세션에는 검색한 폴더로 남는다([`Location::for_session`]).
//! - 목록은 읽지 않는다: 검색 결과 위치로 가는 읽기는 곧바로 빈 목록으로 확정되고(뒤로·앞으로 기록에 남음), 결과는
//!   검색이 흘려 넣는다. 결과를 넣는 쪽은 [`SearchFeed`]다. 지금은 실시간 탐색([`crate::live_search::LiveSearch`])이
//!   구현하고, 즉시 검색(S8)도 같은 것을 구현해 같은 위치에 결과를 넣는다. 탭은 피드를 갖고 있다가 다른 곳으로 가거나
//!   닫히면 버린다(버리면 검색이 멈춘다).
//! - 결과 항목은 이름이 겹칠 수 있어(다른 폴더의 같은 이름) 탭의 선택·커서·범위 기준이 항목의 전체 경로로 동작한다.
//! - 결과는 탐색 시점의 것이고 변경 감시를 걸지 않는다. facet이 한 작업(삭제·휴지통 삭제·이동·이름바꾸기와 그
//!   실행취소·다시 실행)만 [`PathChange`]로 결과 목록에 반영한다.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::history::{Reversal, Step};
use crate::live_search::{LiveSearch, SearchEvent};
use crate::provider::{Candidate, PathSegment, Provider, ResolveError, Resolved};
use crate::watch::{Watch, WatchError, WatchEvent};
use crate::{FileSystemProvider, Item, ListError, Location};

/// 검색 결과 위치의 공급자. 목록은 비어 있고(결과는 검색이 넣는다) 감시를 걸 수 없다. 주소창 글자 해석과 자동 완성은
/// 파일 시스템 공급자와 같다(결과 위치에서 주소창에 경로를 치면 그 폴더로 간다).
#[derive(Debug, Clone, Copy, Default)]
pub struct SearchResultsProvider;

fn folder_location(location: &Location) -> Location {
    match location.search_scope() {
        Some(scope) => Location::file_system(&scope.folder),
        None => location.clone(),
    }
}

impl Provider for SearchResultsProvider {
    /// 검색 결과 위치는 읽을 목록이 없다: 늘 빈 목록이다. 결과는 [`SearchFeed`]가 넣는다.
    fn list(&self, _location: &Location) -> Result<Vec<Item>, ListError> {
        Ok(Vec::new())
    }

    /// 검색한 폴더 안의 이름. 결과 항목은 제 폴더가 따로 있으므로 탭은 이것을 쓰지 않고 항목의 전체 경로로 간다.
    fn child(&self, location: &Location, name: &OsStr) -> Location {
        FileSystemProvider.child(&folder_location(location), name)
    }

    /// 위로 가면 검색한 폴더다.
    fn parent(&self, location: &Location) -> Option<Location> {
        location
            .search_scope()
            .map(|scope| Location::file_system(&scope.folder))
    }

    /// 검색한 폴더의 조각 뒤에 검색어 조각 하나(이름은 검색어 그대로, 화면 문장은 앱이 붙인다).
    fn segments(&self, location: &Location) -> Vec<PathSegment> {
        let Some(scope) = location.search_scope() else {
            return Vec::new();
        };
        let mut segments = FileSystemProvider.segments(&Location::file_system(&scope.folder));
        segments.push(PathSegment {
            label: scope.query.clone(),
            location: location.clone(),
        });
        segments
    }

    fn resolve(&self, text: &str) -> Result<Resolved, ResolveError> {
        FileSystemProvider.resolve(text)
    }

    fn complete(&self, text: &str) -> Vec<Candidate> {
        FileSystemProvider.complete(text)
    }

    /// 결과는 탐색 시점의 것이라 감시하지 않는다(탭은 감시를 걸지 않고 읽는다).
    fn watch(
        &self,
        location: &Location,
        _on_event: Box<dyn FnMut(WatchEvent) + Send>,
    ) -> Result<Watch, WatchError> {
        Err(WatchError::Unsupported {
            location: location.clone(),
        })
    }

    fn item(&self, _location: &Location, _name: &OsStr) -> Result<Option<Item>, ListError> {
        Ok(None)
    }
}

/// 검색 결과 위치에 결과를 흘려 넣는 것. 실시간 탐색이 구현하고, 즉시 검색(S8)도 같은 것을 구현한다. 탭이 갖고
/// 있다가 다른 곳으로 가거나 닫히면 버린다: 버리면 검색을 멈춰야 한다.
pub trait SearchFeed: Send {
    /// 이미 온 사건을 모두 꺼낸다. 막지 않는다.
    fn poll(&mut self) -> Vec<SearchEvent>;

    /// 검색을 멈춘다(「검색 중지」). 지금까지 넣은 결과는 탭에 남는다. 그 뒤로는 끝남 사건만 온다.
    fn stop(&self);
}

impl SearchFeed for LiveSearch {
    fn poll(&mut self) -> Vec<SearchEvent> {
        LiveSearch::poll(self)
    }

    fn stop(&self) {
        self.cancel();
    }
}

/// facet이 한 작업이 결과 목록에 주는 변화.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathChange {
    /// 항목(폴더면 그 아래 전부)이 그 자리에서 사라졌다(지움·휴지통·옮김). 결과에서 빠진다.
    Removed(PathBuf),
    /// 항목이 `from`에서 `to`로 갔다. 같은 폴더 안이면 이름을 바꾼 것이라 결과의 항목 이름이 바뀌고(폴더면 그 아래
    /// 결과의 위치도 바뀜), 다른 폴더로 갔으면 옮긴 것이라 결과에서 빠진다.
    Renamed { from: PathBuf, to: PathBuf },
}

impl PathChange {
    /// 끝난 작업(실행취소·다시 실행이 아님)의 항목별 결과 하나가 주는 변화.
    pub fn of_step(step: &Step) -> Option<PathChange> {
        match step {
            Step::Renamed { from, to, .. }
            | Step::Moved { from, to, .. }
            | Step::MovedFolder { from, to, .. } => Some(PathChange::Renamed {
                from: from.clone(),
                to: to.clone(),
            }),
            Step::Deleted { path } | Step::Recycled { path, .. } => {
                Some(PathChange::Removed(path.clone()))
            }
            Step::Overwrote {
                source,
                moved: true,
                ..
            } => Some(PathChange::Removed(source.clone())),
            Step::Overwrote { .. } | Step::Copied { .. } | Step::CreatedFolder { .. } => None,
        }
    }

    /// 실행취소(`Undo`)·다시 실행(`Redo`)이 되돌린 항목별 결과(기록에 있던 모습) 하나가 주는 변화. 다시 실행은 처음
    /// 작업과 같다. 실행취소는 반대다: 되바꾼 이름·되옮긴 항목은 `to`에서 `from`으로, 휴지통으로 보낸 복사본·지운 새
    /// 폴더는 사라짐. 휴지통에서 복원한 항목은 결과에 다시 넣지 않는다(F5로 다시 탐색한다).
    pub fn of_reverted(step: &Step, reversal: Reversal) -> Option<PathChange> {
        if reversal == Reversal::Redo {
            return Self::of_step(step);
        }
        match step {
            Step::Renamed { from, to, .. }
            | Step::Moved { from, to, .. }
            | Step::MovedFolder { from, to, .. } => Some(PathChange::Renamed {
                from: to.clone(),
                to: from.clone(),
            }),
            Step::Copied { path, .. } | Step::CreatedFolder { path, .. } => {
                Some(PathChange::Removed(path.clone()))
            }
            Step::Overwrote {
                source,
                path,
                moved: true,
                ..
            } => Some(PathChange::Renamed {
                from: path.clone(),
                to: source.clone(),
            }),
            Step::Overwrote { .. } | Step::Deleted { .. } | Step::Recycled { .. } => None,
        }
    }
}

/// `path`가 `ancestor`와 같거나 그 아래인가(경로 조각으로 비교, 대소문자 무시).
pub(crate) fn is_within(path: &Path, ancestor: &Path) -> bool {
    let mut path = path.components();
    for want in ancestor.components() {
        match path.next() {
            Some(have) if same_component(have.as_os_str(), want.as_os_str()) => {}
            _ => return false,
        }
    }
    true
}

/// Windows 파일 이름처럼 대소문자를 가리지 않고 같은 조각인가.
fn same_component(a: &OsStr, b: &OsStr) -> bool {
    a == b || a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

/// `path`가 `ancestor`의 아래면 `ancestor`를 `replacement`로 바꾼 경로. 아니면 `None`이다.
pub(crate) fn rebase(path: &Path, ancestor: &Path, replacement: &Path) -> Option<PathBuf> {
    if !is_within(path, ancestor) {
        return None;
    }
    let rest: PathBuf = path
        .components()
        .skip(ancestor.components().count())
        .collect();
    Some(if rest.as_os_str().is_empty() {
        replacement.to_path_buf()
    } else {
        replacement.join(rest)
    })
}

/// 탭의 검색 결과 상태: 검색 중인가, 진행, 끝난 요약. [`crate::tab::Tab::search`]가 준다.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchStatus {
    /// 검색이 도는 중인가(피드가 끝남 사건을 보내지 않았고 멈추지 않았다).
    pub running: bool,
    /// 지금까지의 진행: 찾은 수, 훑은 폴더 수, 건너뛴 폴더 수.
    pub progress: crate::live_search::SearchProgress,
    /// 끝났으면 요약(끝남·취소, 걸린 시간).
    pub summary: Option<crate::live_search::SearchSummary>,
    /// 검색을 시작할 때의 숨김 항목 보이기. 결과 목록은 이 상태를 따른다(뒤에 바꿔도 결과는 그대로다).
    pub show_hidden: bool,
    /// 내용 검색이면 읽은 파일 수·바이트와 종류별 건너뛴 수. 이름 검색이면 `None`이다.
    pub content: Option<ContentStatus>,
}

/// 내용 검색 결과 위치의 진행: 읽은 파일과 종류별 건너뛴 수(바이너리·온라인 전용·큰 파일·읽지 못함).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContentStatus {
    /// 종류별로 건너뛴 파일 수.
    pub skipped: crate::content_search::Skipped,
    /// 읽은 파일 수.
    pub files_read: usize,
    /// 읽은 바이트.
    pub bytes_read: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 아래_경로를_대소문자_없이_알아보고_바꾼다() {
        let base = Path::new("C:/a/B");
        assert!(is_within(Path::new("C:/a/b/c"), base));
        assert!(is_within(Path::new("C:/A/B"), base));
        assert!(!is_within(Path::new("C:/a/bc"), base));
        assert_eq!(
            rebase(Path::new("C:/a/b/c/d"), base, Path::new("C:/a/x")),
            Some(PathBuf::from("C:/a/x/c/d"))
        );
        assert_eq!(
            rebase(Path::new("C:/a/b"), base, Path::new("C:/a/x")),
            Some(PathBuf::from("C:/a/x"))
        );
        assert_eq!(rebase(Path::new("C:/z"), base, Path::new("C:/a/x")), None);
    }
}
