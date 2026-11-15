//! 탭: 패널 안에서 위치 하나를 보여 주는 단위.
//!
//! 탭은 위치, 뒤로·앞으로 기록, 정렬, 목록을 갖는다. 이동은 두 단계다.
//!
//! 1. [`Tab::navigate`]가 갈 곳을 정하고 읽기 요청([`ReadRequest`])을 돌려준다. 이때 탭의 위치와
//!    기록은 아직 바뀌지 않는다.
//! 2. 요청을 (보통 일꾼 스레드에서) [`ReadRequest::run`]으로 읽고, 그 결과를 [`Tab::finish`]에 넘긴다.
//!    읽기가 성공했을 때만 이동을 확정한다. 실패하면 탭은 원래 위치에 남고 오류를 갖는다.
//!    실패한 위치는 기록에 남지 않는다.
//!
//! 요청마다 번호를 붙인다. 탭이 기다리는 것은 마지막 요청 하나뿐이고, 그보다 앞선 요청의 결과는
//! 늦게 오면 버린다([`ReadOutcome::Stale`]).
//!
//! 이 모듈은 스레드를 만들지 않는다. 일꾼 스레드로 읽고 결과를 탭에 넘기는 일은
//! [`crate::window::App`]이 한다. 그래서 탭만 따로 화면 없이, 결과가 오는 순서를 정해 가며 시험할 수 있다.

use std::ffi::OsString;
use std::time::{Duration, Instant};

use crate::details::{DetailsList, Sort};
use crate::{Item, ListError, Location, provider_for};

/// 읽기가 이만큼 넘게 걸리면 탭이 「읽는 중」을 드러낸다([`Tab::is_reading`]).
pub const READING_DELAY: Duration = Duration::from_millis(100);

/// 탭에서 위치를 옮기는 이동.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Navigation {
    /// 주어진 위치로 간다(주소창, 경로 조각 등). 새 위치로 가는 것이라 앞으로 기록을 버린다.
    Go(Location),
    /// 지금 위치 안의 이름이 `name`인 폴더로 들어간다. 새 위치로 가는 것이다.
    Open(OsString),
    /// 뒤로 기록의 가장 가까운 위치로 간다.
    Back,
    /// 앞으로 기록의 가장 가까운 위치로 간다.
    Forward,
    /// 상위 위치로 간다. 새 위치로 가는 것이다. 드라이브 루트처럼 상위가 없으면 꺼진다.
    Up,
    /// 지금 위치를 다시 읽는다. 기록은 바뀌지 않는다.
    Refresh,
}

/// 읽기 요청의 번호. 한 탭 안에서 요청할 때마다 커진다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestId(u64);

/// 탭이 낸 읽기 요청 하나: 어느 위치를 어떤 정렬로 읽을지.
///
/// `Send`이므로 일꾼 스레드에 넘겨 [`ReadRequest::run`]한다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadRequest {
    id: RequestId,
    location: Location,
    sort: Sort,
}

impl ReadRequest {
    /// 요청 번호.
    pub fn id(&self) -> RequestId {
        self.id
    }

    /// 읽을 위치.
    pub fn location(&self) -> &Location {
        &self.location
    }

    /// 위치의 항목 목록을 공급자로 읽고 요청의 정렬로 정렬한다. 끝날 때까지 막으므로 UI 스레드에서
    /// 부르지 않는다.
    pub fn run(&self) -> ReadResult {
        let list = provider_for(&self.location)
            .list(&self.location)
            .map(|items| DetailsList::new(items, self.sort));
        ReadResult { id: self.id, list }
    }
}

/// 읽기 요청의 결과. [`ReadRequest::run`]만 만든다.
#[derive(Debug)]
pub struct ReadResult {
    id: RequestId,
    list: Result<DetailsList, ListError>,
}

impl ReadResult {
    /// 어느 요청의 결과인가.
    pub fn id(&self) -> RequestId {
        self.id
    }
}

/// 읽기 결과를 탭에 넘긴 결과.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOutcome {
    /// 읽기가 성공해 이동(또는 새로 고침)을 확정했다.
    Committed,
    /// 읽기가 실패했다. 탭은 원래 위치에 남고 [`Tab::error`]에 까닭이 있다.
    Failed,
    /// 탭이 기다리는 요청의 결과가 아니라 버렸다(사용자가 이미 다른 곳으로 갔다).
    Stale,
}

/// 확정되면 기록을 어떻게 바꾸는가.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// 새 위치: 지금 위치를 뒤로 기록에 넣고 앞으로 기록을 버린다.
    New,
    Back,
    Forward,
    Refresh,
}

/// 기다리는 읽기 요청.
#[derive(Debug, Clone)]
struct Pending {
    id: RequestId,
    location: Location,
    kind: Kind,
    started: Instant,
}

/// 패널 안에서 위치 하나를 보여 주는 단위.
///
/// 목록은 탭의 정렬로 정렬해 둔다. 숨김 항목을 감출 때([`Tab::show_hidden`]가 `false`) 숨김 속성이
/// 있는 항목은 [`Tab::len`]·[`Tab::get`]·[`Tab::items`]에 나오지 않는다. 감춘 항목도 탭은 갖고 있어서
/// 보이기로 바꿀 때 다시 읽지 않는다.
#[derive(Debug)]
pub struct Tab {
    location: Location,
    /// 뒤로 기록. 가장 가까운 것이 끝이다.
    back: Vec<Location>,
    /// 앞으로 기록. 가장 가까운 것이 끝이다.
    forward: Vec<Location>,
    sort: Sort,
    show_hidden: bool,
    /// 숨김 항목까지 모두 담은 목록.
    list: DetailsList,
    /// 숨김 항목을 감출 때 보이는 항목의 `list` 안 번호. `None`이면 모두 보인다.
    visible: Option<Vec<usize>>,
    pending: Option<Pending>,
    next_id: u64,
    error: Option<ListError>,
}

impl Tab {
    /// `location`을 보여 줄 탭. 목록은 비어 있고 기록도 없다.
    ///
    /// 목록을 채우려면 [`Navigation::Refresh`]로 읽는다.
    pub fn new(location: Location, sort: Sort, show_hidden: bool) -> Self {
        Tab {
            location,
            back: Vec::new(),
            forward: Vec::new(),
            sort,
            show_hidden,
            list: DetailsList::new(Vec::new(), sort),
            visible: None,
            pending: None,
            next_id: 0,
            error: None,
        }
    }

    /// 지금 위치. 이동은 읽기가 성공해야 바뀐다.
    pub fn location(&self) -> &Location {
        &self.location
    }

    /// 뒤로 기록. 가장 가까운 위치(뒤로 가면 갈 곳)가 끝이다.
    pub fn back_history(&self) -> &[Location] {
        &self.back
    }

    /// 앞으로 기록. 가장 가까운 위치(앞으로 가면 갈 곳)가 끝이다.
    pub fn forward_history(&self) -> &[Location] {
        &self.forward
    }

    /// 뒤로 갈 수 있는가.
    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    /// 앞으로 갈 수 있는가.
    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    /// 위로 갈 수 있는가. 드라이브 루트처럼 상위 위치가 없으면 꺼진다.
    pub fn can_go_up(&self) -> bool {
        provider_for(&self.location)
            .parent(&self.location)
            .is_some()
    }

    /// 지금 정렬.
    pub fn sort(&self) -> Sort {
        self.sort
    }

    /// 숨김 항목을 보이는가.
    pub fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    /// 마지막 이동(또는 새로 고침)이 실패한 까닭. 다음 이동을 시작하면 지운다.
    pub fn error(&self) -> Option<&ListError> {
        self.error.as_ref()
    }

    /// 읽기를 기다리는 위치. 없으면 `None`이다.
    pub fn pending_location(&self) -> Option<&Location> {
        self.pending.as_ref().map(|p| &p.location)
    }

    /// 「읽는 중」을 드러낼 시각: 기다리는 읽기를 시작하고 [`READING_DELAY`]가 지난 때. 기다리는 읽기가
    /// 없으면 `None`이다. 화면은 이 시각에 다시 그리도록 타이머를 건다.
    pub fn reading_shown_at(&self) -> Option<Instant> {
        self.pending.as_ref().map(|p| p.started + READING_DELAY)
    }

    /// `now`에 「읽는 중」을 드러내는가: 읽기를 기다린 지 [`READING_DELAY`]가 지났는가.
    pub fn is_reading(&self, now: Instant) -> bool {
        self.reading_shown_at().is_some_and(|at| now >= at)
    }

    /// 보이는 항목 수.
    pub fn len(&self) -> usize {
        match &self.visible {
            Some(visible) => visible.len(),
            None => self.list.len(),
        }
    }

    /// 보이는 항목이 없는가.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 보이는 항목 중 정렬된 순서로 `index`번째.
    pub fn get(&self, index: usize) -> Option<&Item> {
        match &self.visible {
            Some(visible) => self.list.get(*visible.get(index)?),
            None => self.list.get(index),
        }
    }

    /// 보이는 항목을 정렬된 순서로.
    pub fn items(&self) -> impl Iterator<Item = &Item> + '_ {
        (0..self.len()).filter_map(|i| self.get(i))
    }

    /// 이동을 시작한다. 읽어야 할 요청을 돌려주고, 이동할 수 없으면(기록이 없음, 상위가 없음) `None`이다.
    ///
    /// 위치와 기록은 [`Tab::finish`]가 성공한 결과를 받을 때 바뀐다. 앞서 기다리던 요청이 있으면 이 요청이
    /// 그것을 대신하고, 앞선 요청의 결과는 버린다. 시작하면 지난 오류를 지운다.
    pub fn navigate(&mut self, navigation: Navigation) -> Option<ReadRequest> {
        let provider = provider_for(&self.location);
        let (location, kind) = match navigation {
            Navigation::Go(location) => (location, Kind::New),
            Navigation::Open(name) => (provider.child(&self.location, &name), Kind::New),
            Navigation::Back => (self.back.last()?.clone(), Kind::Back),
            Navigation::Forward => (self.forward.last()?.clone(), Kind::Forward),
            Navigation::Up => (provider.parent(&self.location)?, Kind::New),
            Navigation::Refresh => (self.location.clone(), Kind::Refresh),
        };
        let id = RequestId(self.next_id);
        self.next_id += 1;
        self.error = None;
        self.pending = Some(Pending {
            id,
            location: location.clone(),
            kind,
            started: Instant::now(),
        });
        Some(ReadRequest {
            id,
            location,
            sort: self.sort,
        })
    }

    /// 읽기 결과를 받는다. 기다리는 요청의 결과일 때만 반영한다.
    ///
    /// - 성공: 위치를 옮기고 기록을 고친다. 새 위치로 가면 지금 위치를 뒤로 기록에 넣고 앞으로 기록을 버린다
    ///   (지금 위치와 같은 곳이면 기록을 건드리지 않는다). 뒤로·앞으로 가면 지금 위치를 반대쪽 기록에 넣는다.
    /// - 실패: 위치·목록·기록을 그대로 두고 오류를 갖는다.
    pub fn finish(&mut self, result: ReadResult) -> ReadOutcome {
        let Some(pending) = self.pending.take_if(|p| p.id == result.id) else {
            return ReadOutcome::Stale;
        };
        let mut list = match result.list {
            Ok(list) => list,
            Err(error) => {
                self.error = Some(error);
                return ReadOutcome::Failed;
            }
        };
        match pending.kind {
            Kind::New if pending.location != self.location => {
                let previous = std::mem::replace(&mut self.location, pending.location);
                self.back.push(previous);
                self.forward.clear();
            }
            Kind::Back => {
                self.back.pop();
                let previous = std::mem::replace(&mut self.location, pending.location);
                self.forward.push(previous);
            }
            Kind::Forward => {
                self.forward.pop();
                let previous = std::mem::replace(&mut self.location, pending.location);
                self.back.push(previous);
            }
            Kind::New | Kind::Refresh => {}
        }
        // 읽는 사이에 정렬을 바꿨으면 지금 정렬로 맞춘다.
        if list.sort() != self.sort {
            list.set_sort(self.sort);
        }
        self.list = list;
        self.update_visible();
        ReadOutcome::Committed
    }

    /// 정렬을 바꾼다. 가진 목록을 다시 정렬할 뿐 파일 시스템을 다시 읽지 않는다.
    pub fn set_sort(&mut self, sort: Sort) {
        if sort == self.sort {
            return;
        }
        self.sort = sort;
        self.list.set_sort(sort);
        self.update_visible();
    }

    /// 숨김 항목 보이기/감추기. 앱 전체에 하나인 상태를 따르므로 보통 [`crate::window::App::set_show_hidden`]이
    /// 부른다. 다시 읽지 않는다.
    pub fn set_show_hidden(&mut self, show_hidden: bool) {
        if show_hidden == self.show_hidden {
            return;
        }
        self.show_hidden = show_hidden;
        self.update_visible();
    }

    fn update_visible(&mut self) {
        self.visible = if self.show_hidden || !self.list.iter().any(|i| i.attributes.hidden) {
            None
        } else {
            Some(
                self.list
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| !item.attributes.hidden)
                    .map(|(i, _)| i)
                    .collect(),
            )
        };
    }
}
