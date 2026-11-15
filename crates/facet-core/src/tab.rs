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
//! 앱은 읽을 때 위치의 변경 감시도 함께 건다([`ReadRequest::run_watched`]). 감시는 탭이 갖고, 다른 위치로
//! 옮기면 버려져 풀린다. 묶은 변경([`ChangeBatch`])은 [`Tab::apply_changes`]가 바뀐 항목만 목록에 반영한다.
//! 알림을 잃었거나 위치가 사라졌으면 [`Tab::reload`]로 다시 읽고, 위치가 사라졌으면 가장 가까운 남은 상위
//! 위치(없으면 사용자 홈 폴더)로 옮긴다([`Tab::relocation`]).
//!
//! 탭은 보이거나 숨는다. 패널에서 활성이 아닌 탭은 숨어서 감시를 놓는다([`Tab::hide`]). 목록은 그대로 갖고
//! 있다가, 다시 보이면([`Tab::show`]) 그 목록을 먼저 보이며 다시 읽고 감시를 다시 건다. 숨은 동안 끝난 읽기의
//! 감시는 버린다.
//!
//! 요청마다 번호를 붙인다. 탭이 기다리는 것은 마지막 요청 하나뿐이고, 그보다 앞선 요청의 결과는
//! 늦게 오면 버린다([`ReadOutcome::Stale`]).
//!
//! 이 모듈은 스레드를 만들지 않는다. 일꾼 스레드로 읽고 결과를 탭에 넘기는 일은
//! [`crate::window::App`]이 한다. 그래서 탭만 따로 화면 없이, 결과가 오는 순서를 정해 가며 시험할 수 있다.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::details::{DetailsList, Sort};
use crate::selection::{CursorMove, Pick, StatusSummary, TypeAhead, ViewState, starts_with_folded};
use crate::watch::{self, ChangeBatch, ChangeSink, Watch, WatchError};
use crate::window::home_location;
use facet_win::OpenError;

use crate::launch::LaunchTarget;
use crate::{Candidate, Item, ListError, Location, ResolveError, provider_for};

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
    /// 위치가 사라졌으면 가장 가까운 남은 상위(없으면 홈)를 읽는가([`Tab::reload`]).
    climb: bool,
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
    /// 부르지 않는다. 변경 감시는 걸지 않는다(탭의 [`Tab::auto_refresh`]는 [`AutoRefresh::NotWatched`]).
    pub fn run(&self) -> ReadResult {
        self.read(None)
    }

    /// [`ReadRequest::run`]처럼 읽되, 읽기 전에 위치의 변경 감시를 건다. 묶은 변경은 묶기 스레드에서
    /// `sink`로 넘어온다. 그것을 탭의 [`Tab::apply_changes`]에 넘긴다.
    ///
    /// 감시를 걸 수 없어도 읽기는 한다. 그러면 탭은 「자동 새로 고침 안 됨」([`AutoRefresh::Off`])이다.
    pub fn run_watched(&self, sink: impl Fn(ChangeBatch) + Send + Sync + 'static) -> ReadResult {
        self.read(Some(Arc::new(sink)))
    }

    fn read(&self, sink: Option<ChangeSink>) -> ReadResult {
        // 감시를 먼저 건다: 읽는 사이의 변경도 놓치지 않는다.
        let attempt = |location: &Location| {
            let watch = sink
                .as_ref()
                .map(|sink| watch::start(location, self.id, Arc::clone(sink)));
            let list = provider_for(location)
                .list(location)
                .map(|items| DetailsList::new(items, self.sort));
            (list, watch)
        };
        let result = |location: Location, (list, watch), relocation| ReadResult {
            id: self.id,
            location,
            list,
            watch,
            relocation,
        };
        let first = attempt(&self.location);
        // 권한이 없을 뿐이면 위치는 남아 있다.
        let vanished =
            matches!(&first.0, Err(e) if !matches!(e, ListError::PermissionDenied { .. }));
        if !(self.climb && vanished) {
            return result(self.location.clone(), first, None);
        }
        let provider = provider_for(&self.location);
        let mut candidate = provider.parent(&self.location);
        while let Some(location) = candidate {
            let read = attempt(&location);
            if read.0.is_ok() {
                let relocation = Relocation::ToParent {
                    vanished: self.location.clone(),
                };
                return result(location, read, Some(relocation));
            }
            candidate = provider.parent(&location);
        }
        let home = home_location();
        if home != self.location {
            let read = attempt(&home);
            if read.0.is_ok() {
                let relocation = Relocation::ToHome {
                    vanished: self.location.clone(),
                };
                return result(home, read, Some(relocation));
            }
        }
        result(self.location.clone(), first, None)
    }
}

/// 보던 위치가 사라져 다른 위치로 옮긴 까닭. [`Tab::relocation`]이 갖는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Relocation {
    /// 보던 위치 `vanished`가 사라져 가장 가까운 남은 상위 위치로 옮겼다.
    ToParent { vanished: Location },
    /// 보던 위치 `vanished`와 그 상위가 모두 사라져(드라이브가 빠짐 등) 사용자 홈 폴더로 옮겼다.
    ToHome { vanished: Location },
}

impl Relocation {
    /// 사라진 위치.
    pub fn vanished(&self) -> &Location {
        match self {
            Self::ToParent { vanished } | Self::ToHome { vanished } => vanished,
        }
    }

    fn with_vanished(self, vanished: Location) -> Self {
        match self {
            Self::ToParent { .. } => Self::ToParent { vanished },
            Self::ToHome { .. } => Self::ToHome { vanished },
        }
    }
}

/// 탭의 자동 새로 고침(변경 감시) 상태.
#[derive(Debug, Clone, Copy)]
pub enum AutoRefresh<'a> {
    /// 지금 위치를 감시하고 있다. 밖에서 바뀌면 목록이 스스로 갱신된다.
    On,
    /// 「자동 새로 고침 안 됨」: 지금 위치에 감시를 걸 수 없었다. 새로 고침(F5)은 된다.
    Off(&'a WatchError),
    /// 감시를 걸지 않고 읽었다([`ReadRequest::run`]), 또는 아직 읽지 않았다.
    NotWatched,
}

/// 묶은 변경을 탭에 넘긴 결과.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeOutcome {
    /// 바뀐 항목을 목록에 반영했다.
    Applied,
    /// 알림을 잃었거나 위치가 사라졌다. [`Tab::reload`]로 다시 읽어야 한다.
    Reload,
    /// 탭이 지금 갖고 있지 않은 감시의 변경이라 버렸다(이미 다른 위치로 옮겼다).
    Stale,
}

/// 읽기 요청의 결과. [`ReadRequest::run`]만 만든다.
#[derive(Debug)]
pub struct ReadResult {
    id: RequestId,
    /// 실제로 읽은 위치. 사라진 위치에서 옮겼으면 요청한 위치와 다르다.
    location: Location,
    list: Result<DetailsList, ListError>,
    /// 건 감시. 감시를 걸지 않고 읽었으면 `None`이다.
    watch: Option<Result<Watch, WatchError>>,
    relocation: Option<Relocation>,
}

impl ReadResult {
    /// 어느 요청의 결과인가.
    pub fn id(&self) -> RequestId {
        self.id
    }
}

/// 자동 완성 요청의 번호. 한 탭 안에서 요청할 때마다 커진다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CompletionId(u64);

/// 주소창 글자의 자동 완성 요청. `Send`이므로 일꾼 스레드에 넘겨 [`CompletionRequest::run`]한다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionRequest {
    id: CompletionId,
    location: Location,
    text: String,
}

impl CompletionRequest {
    /// 요청 번호.
    pub fn id(&self) -> CompletionId {
        self.id
    }

    /// 후보를 읽는다. 폴더를 읽느라 막히므로 UI 스레드에서 부르지 않는다.
    pub fn run(&self) -> CompletionResult {
        CompletionResult {
            id: self.id,
            candidates: provider_for(&self.location).complete(&self.text),
        }
    }
}

/// 자동 완성 요청의 결과. [`CompletionRequest::run`]만 만든다.
#[derive(Debug)]
pub struct CompletionResult {
    id: CompletionId,
    candidates: Vec<Candidate>,
}

impl CompletionResult {
    /// 어느 요청의 결과인가.
    pub fn id(&self) -> CompletionId {
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
    /// 자동 다시 읽기: 위치가 사라졌으면 남은 상위로 옮긴다. 기록은 바뀌지 않는다.
    Reload,
}

/// 기다리는 읽기 요청.
#[derive(Debug, Clone)]
struct Pending {
    id: RequestId,
    location: Location,
    kind: Kind,
    started: Instant,
    /// 위로 갈 때 방금 있던 폴더의 이름. 확정되면 그 항목을 선택하고 커서를 둔다.
    reselect: Option<OsString>,
}

/// 닫은 탭에서 되살릴 것: 위치, 뒤로·앞으로 기록, 정렬, 닫을 때의 선택·커서·스크롤. 목록은 담지 않는다
/// (되살리면 위치를 다시 읽는다). [`Tab::close`]가 만들고 [`Tab::reopen`]이 탭으로 되돌린다.
#[derive(Debug)]
pub struct ClosedTab {
    location: Location,
    back: Vec<Location>,
    back_views: Vec<ViewState>,
    forward: Vec<Location>,
    forward_views: Vec<ViewState>,
    sort: Sort,
    view: ViewState,
}

impl ClosedTab {
    /// 닫을 때의 위치.
    pub fn location(&self) -> &Location {
        &self.location
    }

    /// 뒤로 기록. 가장 가까운 위치가 끝이다.
    pub fn back_history(&self) -> &[Location] {
        &self.back
    }

    /// 앞으로 기록. 가장 가까운 위치가 끝이다.
    pub fn forward_history(&self) -> &[Location] {
        &self.forward
    }

    /// 정렬.
    pub fn sort(&self) -> Sort {
        self.sort
    }
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
    /// `back`과 짝을 이루는, 그 위치를 떠날 때의 보기 상태(선택·커서·스크롤).
    back_views: Vec<ViewState>,
    /// 앞으로 기록. 가장 가까운 것이 끝이다.
    forward: Vec<Location>,
    /// `forward`와 짝을 이루는 보기 상태.
    forward_views: Vec<ViewState>,
    sort: Sort,
    show_hidden: bool,
    /// 숨김 항목까지 모두 담은 목록.
    list: DetailsList,
    /// 숨김 항목을 감출 때 보이는 항목의 `list` 안 번호. `None`이면 모두 보인다.
    visible: Option<Vec<usize>>,
    pending: Option<Pending>,
    next_id: u64,
    error: Option<ListError>,
    /// 마지막 주소창 글자 해석이 실패한 까닭.
    address_error: Option<ResolveError>,
    /// 마지막 자동 완성 요청 번호. 이보다 앞선 요청의 결과는 버린다.
    next_completion: u64,
    /// 선택한 항목의 이름. 보이는 항목만 든다.
    selection: HashSet<OsString>,
    /// 커서 항목의 이름과 보이는 항목 안 번호(정렬·새로 고침 뒤 `reindex`로 맞춘다).
    cursor: Option<(OsString, usize)>,
    /// Shift 범위의 기준 항목 이름.
    anchor: Option<OsString>,
    /// 화면이 맨 위에 보이는 행 번호. 화면이 알려 주는 값을 그대로 보관한다.
    scroll: usize,
    type_ahead: TypeAhead,
    /// 지금 위치의 감시와 그것을 건 읽기 요청 번호. 버리면 풀린다.
    watching: Option<(RequestId, Watch)>,
    /// 지금 위치에 감시를 걸지 못한 까닭.
    watch_error: Option<WatchError>,
    /// 보던 위치가 사라져 옮긴 까닭. 다음 이동을 시작하면 지운다.
    relocation: Option<Relocation>,
    /// 마지막으로 파일을 기본 프로그램으로 열지 못한 까닭.
    open_error: Option<OpenError>,
    /// 보이는 탭인가. 숨은 탭은 감시를 갖지 않는다.
    shown: bool,
}

impl Tab {
    /// `location`을 보여 줄 탭. 목록은 비어 있고 기록도 없다.
    ///
    /// 목록을 채우려면 [`Navigation::Refresh`]로 읽는다.
    pub fn new(location: Location, sort: Sort, show_hidden: bool) -> Self {
        Tab {
            location,
            back: Vec::new(),
            back_views: Vec::new(),
            forward: Vec::new(),
            forward_views: Vec::new(),
            sort,
            show_hidden,
            list: DetailsList::new(Vec::new(), sort),
            visible: None,
            pending: None,
            next_id: 0,
            error: None,
            address_error: None,
            next_completion: 0,
            selection: HashSet::new(),
            cursor: None,
            anchor: None,
            scroll: 0,
            type_ahead: TypeAhead::default(),
            watching: None,
            watch_error: None,
            relocation: None,
            open_error: None,
            shown: true,
        }
    }

    /// 탭을 닫아 되살릴 것만 남긴다([`ClosedTab`]): 위치, 뒤로·앞으로 기록(과 그 보기 상태), 정렬, 지금 보기
    /// 상태(선택·커서·스크롤). 목록과 감시는 버린다.
    pub fn close(mut self) -> ClosedTab {
        let view = self.take_view();
        ClosedTab {
            location: self.location,
            back: self.back,
            back_views: self.back_views,
            forward: self.forward,
            forward_views: self.forward_views,
            sort: self.sort,
            view,
        }
    }

    /// 닫은 탭을 되살린다. 위치·기록·정렬·보기 상태는 닫을 때 그대로이고 목록은 비어 있다. 보이는 탭이며, 목록을
    /// 채우려면 [`Navigation::Refresh`]로 다시 읽는다(선택·커서는 이름으로 남는다).
    pub fn reopen(closed: ClosedTab, show_hidden: bool) -> Tab {
        let ClosedTab {
            location,
            back,
            back_views,
            forward,
            forward_views,
            sort,
            view,
        } = closed;
        let mut tab = Tab::new(location, sort, show_hidden);
        tab.back = back;
        tab.back_views = back_views;
        tab.forward = forward;
        tab.forward_views = forward_views;
        tab.selection = view.selection;
        tab.cursor = view.cursor.map(|name| (name, 0));
        tab.anchor = view.anchor;
        tab.scroll = view.scroll;
        tab
    }

    /// 실행 인자 하나([`LaunchTarget`])를 보일 탭. 목록은 비어 있고 기록도 없다. 파일 경로였으면 그 파일을 고른
    /// 채로(선택·커서·범위 기준이 그 이름) 시작해, 읽으면 그 항목이 골라져 있다. 해석하지 못한 인자면
    /// [`Tab::address_error`]에 까닭을 갖는다(이때는 읽지 않는다).
    pub fn launched(target: LaunchTarget, sort: Sort, show_hidden: bool) -> Tab {
        let LaunchTarget {
            location,
            select,
            error,
        } = target;
        let mut tab = Tab::new(location, sort, show_hidden);
        tab.address_error = error;
        if let Some(name) = select {
            tab.selection.insert(name.clone());
            tab.cursor = Some((name.clone(), 0));
            tab.anchor = Some(name);
        }
        tab
    }

    /// 탭 제목: 위치의 폴더 이름. 드라이브 루트는 드라이브 이름(`C:`)이다. 화면 문장이 아니라 이름이다.
    pub fn title(&self) -> String {
        provider_for(&self.location)
            .segments(&self.location)
            .pop()
            .map(|segment| segment.label)
            .unwrap_or_else(|| self.location.to_string())
    }

    /// 보이는 탭인가. 새로 만든 탭은 보인다.
    pub fn is_shown(&self) -> bool {
        self.shown
    }

    /// 탭을 숨긴다: 감시를 놓는다(버려서 푼다). 목록·선택·커서·스크롤·기록·정렬은 그대로 갖는다. 숨은 동안 끝난
    /// 읽기도 감시를 남기지 않는다.
    pub fn hide(&mut self) {
        self.shown = false;
        self.watching = None;
        self.watch_error = None;
    }

    /// 숨은 탭을 다시 보인다. 갖고 있던 목록은 그대로 두고, 지금 위치를 다시 읽을 요청을 돌려준다. 그 요청을
    /// 감시와 함께 읽어([`ReadRequest::run_watched`]) [`Tab::finish`]에 넘기면 새 목록으로 바뀌고 감시가 다시
    /// 걸린다. 선택·커서·스크롤은 이름으로 남는다. 숨은 동안 위치가 사라졌으면 [`Tab::reload`]처럼 가장 가까운
    /// 남은 상위로 옮긴다.
    ///
    /// 이미 보이면 아무것도 하지 않고 `None`이다. 기다리는 읽기가 있어도 `None`이다(그 읽기가 감시를 건다).
    pub fn show(&mut self) -> Option<ReadRequest> {
        if self.shown {
            return None;
        }
        self.shown = true;
        self.reload()
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

    /// 마지막 [`Tab::go_to_text`]가 글자를 해석하지 못한 까닭. 다음 이동을 시작하면 지운다.
    pub fn address_error(&self) -> Option<&ResolveError> {
        self.address_error.as_ref()
    }

    /// 보던 위치가 사라져 다른 위치로 옮겼으면 그 까닭. 다음 이동을 시작하면 지운다.
    pub fn relocation(&self) -> Option<&Relocation> {
        self.relocation.as_ref()
    }

    /// 마지막으로 파일을 기본 프로그램으로 열지 못한 까닭(연결된 프로그램 없음 등). 다음 이동이나 다음 열기를
    /// 시작하면 지운다. 여는 일은 [`crate::window::App::execute`]가 한다.
    pub fn open_error(&self) -> Option<&OpenError> {
        self.open_error.as_ref()
    }

    pub(crate) fn set_open_error(&mut self, error: Option<OpenError>) {
        self.open_error = error;
    }

    /// 자동 새로 고침 상태. 감시를 걸 수 없는 위치면 [`AutoRefresh::Off`]다.
    pub fn auto_refresh(&self) -> AutoRefresh<'_> {
        match (&self.watching, &self.watch_error) {
            (Some(_), _) => AutoRefresh::On,
            (None, Some(error)) => AutoRefresh::Off(error),
            (None, None) => AutoRefresh::NotWatched,
        }
    }

    /// 감시하고 있는 위치. 감시가 없으면 `None`이다.
    pub fn watched_location(&self) -> Option<&Location> {
        self.watching.as_ref().map(|_| &self.location)
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
        let mut reselect = None;
        let (location, kind) = match navigation {
            Navigation::Go(location) => (location, Kind::New),
            Navigation::Open(name) => (provider.child(&self.location, &name), Kind::New),
            Navigation::Back => (self.back.last()?.clone(), Kind::Back),
            Navigation::Forward => (self.forward.last()?.clone(), Kind::Forward),
            Navigation::Up => {
                let parent = provider.parent(&self.location)?;
                reselect = self
                    .location
                    .file_system_path()
                    .and_then(|path| path.file_name())
                    .map(OsStr::to_os_string);
                (parent, Kind::New)
            }
            Navigation::Refresh => (self.location.clone(), Kind::Refresh),
        };
        self.address_error = None;
        self.relocation = None;
        self.open_error = None;
        Some(self.request(location, kind, reselect))
    }

    fn request(
        &mut self,
        location: Location,
        kind: Kind,
        reselect: Option<OsString>,
    ) -> ReadRequest {
        let id = RequestId(self.next_id);
        self.next_id += 1;
        self.error = None;
        self.pending = Some(Pending {
            id,
            location: location.clone(),
            kind,
            started: Instant::now(),
            reselect,
        });
        ReadRequest {
            id,
            location,
            sort: self.sort,
            climb: kind == Kind::Reload,
        }
    }

    /// 자동 다시 읽기: 지금 위치를 다시 읽되, 위치가 사라졌으면 가장 가까운 남은 상위 위치로, 상위가 하나도
    /// 없으면 사용자 홈 폴더로 옮긴다. 옮기면 [`Tab::relocation`]에 까닭을 두고, 사라진 위치는 기록에 넣지 않는다.
    ///
    /// [`Tab::apply_changes`]가 [`ChangeOutcome::Reload`]를 돌려줄 때 부른다. 이미 기다리는 읽기가 있으면
    /// 그것을 가로채지 않고 `None`을 돌려준다(그 읽기가 목록을 새로 맞춘다).
    pub fn reload(&mut self) -> Option<ReadRequest> {
        if self.pending.is_some() {
            return None;
        }
        let location = self.location.clone();
        Some(self.request(location, Kind::Reload, None))
    }

    /// 묶은 변경을 받는다. 지금 위치의 감시에서 온 것이면 바뀐 항목만 목록에 반영해 다시 정렬한다. 선택과
    /// 스크롤 위치는 그대로이고, 사라진 항목만 선택에서 빠진다.
    ///
    /// 알림을 잃었거나 위치가 사라졌으면 반영하지 않고 [`ChangeOutcome::Reload`]를 돌려준다.
    pub fn apply_changes(&mut self, batch: ChangeBatch) -> ChangeOutcome {
        if self
            .watching
            .as_ref()
            .is_none_or(|(id, _)| *id != batch.watch)
        {
            return ChangeOutcome::Stale;
        }
        if batch.reload {
            return ChangeOutcome::Reload;
        }
        if !batch.changes.is_empty() {
            self.list.apply_changes(batch.changes);
            self.update_visible();
            self.prune_selection();
            self.reindex();
        }
        ChangeOutcome::Applied
    }

    /// 주소창에 친 글자로 간다: 해석 → 이동 시작. 파일 경로를 쳤으면 그 폴더로 가고 도착하면 그 파일을 선택한다.
    ///
    /// 해석에 실패하면 `None`을 돌려주고 [`Tab::address_error`]에 까닭을 둔다. 위치·기록·기다리는 읽기는
    /// 그대로다. 성공하면 [`Tab::navigate`]처럼 읽기 요청을 돌려주고 [`Tab::finish`]로 확정한다.
    pub fn go_to_text(&mut self, text: &str) -> Option<ReadRequest> {
        let resolved = match provider_for(&self.location).resolve(text) {
            Ok(resolved) => resolved,
            Err(error) => {
                self.address_error = Some(error);
                return None;
            }
        };
        let request = self.navigate(Navigation::Go(resolved.location))?;
        if let Some(pending) = &mut self.pending {
            pending.reselect = resolved.select;
        }
        Some(request)
    }

    /// 고쳐 쓰는 글자의 자동 완성 후보를 읽을 요청. 앞선 요청의 결과는 [`Tab::finish_completion`]이 버린다.
    pub fn complete(&mut self, text: &str) -> CompletionRequest {
        let id = CompletionId(self.next_completion);
        self.next_completion += 1;
        CompletionRequest {
            id,
            location: self.location.clone(),
            text: text.to_string(),
        }
    }

    /// 자동 완성 결과를 받는다. 마지막 요청의 결과면 후보를, 늦게 온 앞선 요청의 결과면 `None`을 돌려준다.
    pub fn finish_completion(&self, result: CompletionResult) -> Option<Vec<Candidate>> {
        (result.id.0 + 1 == self.next_completion).then_some(result.candidates)
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
        let ReadResult {
            id,
            location: read_location,
            list,
            watch,
            relocation,
        } = result;
        let mut list = match list {
            Ok(list) => list,
            Err(error) => {
                self.error = Some(error);
                return ReadOutcome::Failed;
            }
        };
        let mut restored = None;
        let reselect = pending.reselect.filter(|_| pending.kind == Kind::New);
        let moved = read_location != self.location;
        match pending.kind {
            Kind::New if pending.location != self.location => {
                let previous = std::mem::replace(&mut self.location, pending.location);
                self.back.push(previous);
                let view = self.take_view();
                self.back_views.push(view);
                self.forward.clear();
                self.forward_views.clear();
                restored = Some(ViewState::default());
            }
            Kind::Back => {
                self.back.pop();
                let previous = std::mem::replace(&mut self.location, pending.location);
                self.forward.push(previous);
                let view = self.take_view();
                self.forward_views.push(view);
                restored = self.back_views.pop();
            }
            Kind::Forward => {
                self.forward.pop();
                let previous = std::mem::replace(&mut self.location, pending.location);
                self.back.push(previous);
                let view = self.take_view();
                self.back_views.push(view);
                restored = self.forward_views.pop();
            }
            Kind::Reload if moved => {
                // 사라진 위치는 기록에 넣지 않고, 그 위치의 보기 상태도 버린다.
                self.location = read_location;
                self.take_view();
                restored = Some(ViewState::default());
                // 옮긴 곳도 이어서 사라지면 다시 옮긴다. 사용자가 보던 위치는 처음 사라진 곳이다.
                self.relocation = match (self.relocation.take(), relocation) {
                    (Some(first), Some(again)) => {
                        Some(again.with_vanished(first.vanished().clone()))
                    }
                    (_, relocation) => relocation,
                };
            }
            Kind::New | Kind::Refresh | Kind::Reload => {}
        }
        match watch {
            // 숨은 탭은 감시를 갖지 않는다: 숨은 동안 끝난 읽기의 감시는 버린다.
            Some(_) if !self.shown => {
                self.watching = None;
                self.watch_error = None;
            }
            Some(Ok(watch)) => {
                self.watching = Some((id, watch));
                self.watch_error = None;
            }
            Some(Err(error)) => {
                self.watching = None;
                self.watch_error = Some(error);
            }
            // 감시 없이 읽었다: 다른 위치로 옮겼으면 앞 위치의 감시를 푼다.
            None if moved => {
                self.watching = None;
                self.watch_error = None;
            }
            None => {}
        }
        // 읽는 사이에 정렬을 바꿨으면 지금 정렬로 맞춘다.
        if list.sort() != self.sort {
            list.set_sort(self.sort);
        }
        self.list = list;
        self.update_visible();
        if let Some(view) = restored {
            self.selection = view.selection;
            self.cursor = view.cursor.map(|name| (name, 0));
            self.anchor = view.anchor;
            self.scroll = view.scroll;
        }
        self.prune_selection();
        self.reindex();
        if let Some(name) = reselect
            && let Some(index) = self.position_of(&name)
        {
            self.select_only(index);
        }
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
        self.reindex();
    }

    /// 숨김 항목 보이기/감추기. 앱 전체에 하나인 상태를 따르므로 보통 [`crate::window::App::set_show_hidden`]이
    /// 부른다. 다시 읽지 않는다.
    pub fn set_show_hidden(&mut self, show_hidden: bool) {
        if show_hidden == self.show_hidden {
            return;
        }
        self.show_hidden = show_hidden;
        self.update_visible();
        self.prune_selection();
        self.reindex();
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

    // --- 선택·커서 ---

    /// 지금 보기 상태를 꺼내 간다(기록에 남길 때). 선택·커서·기준은 비고 스크롤은 0이 된다.
    fn take_view(&mut self) -> ViewState {
        ViewState {
            selection: std::mem::take(&mut self.selection),
            cursor: self.cursor.take().map(|(name, _)| name),
            anchor: self.anchor.take(),
            scroll: std::mem::take(&mut self.scroll),
        }
    }

    /// 이름이 `name`인 보이는 항목의 번호. 항목 수만큼 훑는다.
    fn position_of(&self, name: &OsStr) -> Option<usize> {
        (0..self.len()).find(|&i| self.get(i).is_some_and(|item| item.name == name))
    }

    /// 보이지 않거나 사라진 항목을 선택·기준에서 뺀다. 이름 집합을 한 번 만들어 대조하므로 항목 수에 비례한다.
    fn prune_selection(&mut self) {
        if !self.selection.is_empty() {
            let mut selection = std::mem::take(&mut self.selection);
            let present: HashSet<&OsStr> = self.items().map(|i| i.name.as_os_str()).collect();
            selection.retain(|name| present.contains(name.as_os_str()));
            self.selection = selection;
        }
        if let Some(anchor) = &self.anchor
            && self.position_of(anchor).is_none()
        {
            self.anchor = None;
        }
    }

    /// 커서의 번호를 지금 목록에 맞춘다. 커서 항목이 사라졌으면 원래 번호 근처 항목으로 옮긴다.
    fn reindex(&mut self) {
        let Some((name, old)) = self.cursor.take() else {
            return;
        };
        let len = self.len();
        if len == 0 {
            return;
        }
        let index = self.position_of(&name).unwrap_or_else(|| old.min(len - 1));
        if let Some(item) = self.get(index) {
            self.cursor = Some((item.name.clone(), index));
        }
    }

    /// 커서 항목의 번호.
    pub fn cursor(&self) -> Option<usize> {
        self.cursor.as_ref().map(|(_, index)| *index)
    }

    /// 커서 항목.
    pub fn cursor_item(&self) -> Option<&Item> {
        self.get(self.cursor()?)
    }

    fn set_cursor(&mut self, index: usize) -> Option<usize> {
        let name = self.get(index)?.name.clone();
        self.cursor = Some((name, index));
        Some(index)
    }

    /// 커서를 옮긴다. 선택은 바꾸지 않는다. 옮긴 뒤의 번호를 돌려주고, 항목이 없으면 `None`이다.
    ///
    /// 커서가 없으면 첫 항목에 둔다. 끝에서는 더 가지 않고 그 자리에 머문다.
    pub fn move_cursor(&mut self, movement: CursorMove) -> Option<usize> {
        let len = self.len();
        if len == 0 {
            return None;
        }
        let target = match (self.cursor(), movement) {
            (_, CursorMove::First) => 0,
            (_, CursorMove::Last) => len - 1,
            (None, _) => 0,
            (Some(at), CursorMove::Up) => at.saturating_sub(1),
            (Some(at), CursorMove::Down) => (at + 1).min(len - 1),
            (Some(at), CursorMove::PageUp(page)) => at.saturating_sub(page.max(1)),
            (Some(at), CursorMove::PageDown(page)) => (at + page.max(1)).min(len - 1),
        };
        self.set_cursor(target)
    }

    /// `index`번째 항목이 선택됐는가.
    pub fn is_selected(&self, index: usize) -> bool {
        self.get(index)
            .is_some_and(|item| self.selection.contains(&item.name))
    }

    /// 선택한 항목 수.
    pub fn selected_count(&self) -> usize {
        self.selection.len()
    }

    /// 선택한 항목을 정렬된 순서로.
    pub fn selected_items(&self) -> impl Iterator<Item = &Item> + '_ {
        self.items()
            .filter(|item| self.selection.contains(&item.name))
    }

    /// 단일 선택(클릭): `index`번째만 선택하고 커서와 범위 기준을 거기 둔다. 범위 밖이면 아무것도 안 한다.
    pub fn select_only(&mut self, index: usize) {
        let Some(name) = self.get(index).map(|i| i.name.clone()) else {
            return;
        };
        self.selection.clear();
        self.selection.insert(name.clone());
        self.anchor = Some(name);
        self.set_cursor(index);
    }

    /// Shift 범위: 범위 기준부터 `index`까지를 선택하고(앞선 선택은 버린다) 커서를 `index`에 둔다.
    /// 기준이 없으면 커서, 그것도 없으면 `index` 자신이 기준이다.
    pub fn select_range(&mut self, index: usize) {
        if index >= self.len() {
            return;
        }
        let anchor = self
            .anchor
            .as_deref()
            .and_then(|name| self.position_of(name))
            .or(self.cursor())
            .unwrap_or(index);
        let (from, to) = (anchor.min(index), anchor.max(index));
        let names: HashSet<OsString> = (from..=to)
            .filter_map(|i| self.get(i).map(|item| item.name.clone()))
            .collect();
        let anchor_name = self.get(anchor).map(|item| item.name.clone());
        self.selection = names;
        self.anchor = anchor_name;
        self.set_cursor(index);
    }

    /// Ctrl 토글: `index`번째의 선택을 뒤집고 커서와 범위 기준을 거기 둔다.
    pub fn toggle_select(&mut self, index: usize) {
        let Some(name) = self.get(index).map(|i| i.name.clone()) else {
            return;
        };
        if !self.selection.remove(&name) {
            self.selection.insert(name.clone());
        }
        self.anchor = Some(name);
        self.set_cursor(index);
    }

    /// `index`번째 항목을 가리킨다(클릭 등): `pick`대로 선택을 바꾸고 커서를 거기 둔다. 범위 밖이면 아무것도 안 한다.
    pub fn pick(&mut self, index: usize, pick: Pick) {
        match pick {
            Pick::Only => self.select_only(index),
            Pick::Range => self.select_range(index),
            Pick::Toggle => self.toggle_select(index),
            Pick::CursorOnly => {
                self.set_cursor(index);
            }
        }
    }

    /// 커서 키: 커서를 옮기고([`Tab::move_cursor`]) 옮긴 항목을 `pick`대로 고른다. 옮긴 뒤의 번호를 돌려주고,
    /// 항목이 없으면 `None`이다.
    pub fn move_cursor_picking(&mut self, movement: CursorMove, pick: Pick) -> Option<usize> {
        // 범위의 기준은 옮기기 전 커서다(기준이 없을 때).
        if pick == Pick::Range && self.anchor.is_none() {
            self.anchor = self.cursor_item().map(|item| item.name.clone());
        }
        let index = self.move_cursor(movement)?;
        self.pick(index, pick);
        Some(index)
    }

    /// 글자로 이동하고([`Tab::type_to_find`]) 찾은 항목만 선택한다(탐색기와 같다). 찾지 못하면 선택도 커서도
    /// 그대로고 `None`이다.
    pub fn type_to_select(&mut self, c: char, now: Instant) -> Option<usize> {
        let index = self.type_to_find(c, now)?;
        self.select_only(index);
        Some(index)
    }

    /// 커서 항목의 선택을 뒤집는다(Ctrl+Space). 커서가 없으면 아무것도 안 한다.
    pub fn toggle_cursor_selection(&mut self) {
        if let Some(index) = self.cursor() {
            self.toggle_select(index);
        }
    }

    /// 보이는 항목을 모두 선택한다. 커서는 그대로다.
    pub fn select_all(&mut self) {
        self.selection = self.items().map(|item| item.name.clone()).collect();
    }

    /// 선택을 모두 푼다. 커서는 그대로다.
    pub fn clear_selection(&mut self) {
        self.selection.clear();
    }

    /// 화면이 맨 위에 보이는 행 번호.
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// 화면이 맨 위에 보이는 행 번호를 알린다. 뒤로·앞으로 기록에 함께 남는다.
    pub fn set_scroll(&mut self, scroll: usize) {
        self.scroll = scroll;
    }

    /// 글자로 이동: `c`로 시작하는 항목으로 커서를 옮기고 그 번호를 돌려준다. 없으면 커서는 그대로고 `None`이다.
    ///
    /// - [`crate::selection::TYPE_AHEAD_TIMEOUT`] 안에 이어 친 글자는 접두어로 이어 붙인다.
    ///   그보다 늦으면 새로 시작한다.
    /// - 같은 글자를 거듭 치면 그 글자로 시작하는 항목을 커서 다음부터 차례로 돈다.
    /// - 대소문자를 가리지 않는다. 한글은 IME가 조합을 끝낸 글자를 넘긴다.
    ///
    /// 시각 `now`는 부르는 쪽이 넘긴다. 선택은 바꾸지 않는다.
    pub fn type_to_find(&mut self, c: char, now: Instant) -> Option<usize> {
        let search = self.type_ahead.push(c, now);
        let len = self.len();
        if len == 0 {
            return None;
        }
        let start = match self.cursor() {
            Some(at) if search.include_cursor => at,
            Some(at) => at + 1,
            None => 0,
        };
        let found = (0..len).map(|offset| (start + offset) % len).find(|&i| {
            self.get(i)
                .is_some_and(|item| starts_with_folded(&item.name, &search.prefix))
        })?;
        self.set_cursor(found)
    }

    /// 상태 표시줄 수치: 항목 수, 선택 수, 선택한 파일 크기 합.
    pub fn status(&self) -> StatusSummary {
        let selected_size = if self.selection.is_empty() {
            0
        } else {
            self.selected_items()
                .filter(|item| !item.is_folder)
                .map(|item| item.size)
                .sum()
        };
        StatusSummary {
            item_count: self.len(),
            selected_count: self.selection.len(),
            selected_size,
        }
    }
}

#[cfg(test)]
mod tests {
    //! 감시를 걸 수 없는 위치처럼 실제로 만들기 어려운 결과를 직접 넣어 본다. 실제 감시는 `tests/watch.rs`.

    use super::*;

    fn temp_dir() -> tempfile::TempDir {
        tempfile::TempDir::new().unwrap()
    }

    fn batch(tab: &Tab, changes: Vec<(OsString, Option<Item>)>, reload: bool) -> ChangeBatch {
        ChangeBatch {
            watch: tab.watching.as_ref().map(|(id, _)| *id).unwrap(),
            changes,
            reload,
        }
    }

    /// 감시가 걸린 것처럼 읽는다: 버리면 아무것도 하지 않는 감시를 붙인다.
    fn watched_tab(dir: &std::path::Path) -> Tab {
        let mut tab = Tab::new(Location::file_system(dir), Sort::default(), false);
        let request = tab.navigate(Navigation::Refresh).unwrap();
        let mut result = request.run();
        result.watch = Some(Ok(Watch::new(())));
        assert_eq!(tab.finish(result), ReadOutcome::Committed);
        tab
    }

    #[test]
    fn 감시를_걸_수_없으면_자동_새로_고침_안_됨이고_새로_고침은_된다() {
        let dir = temp_dir();
        let location = Location::file_system(dir.path());
        let mut tab = Tab::new(location.clone(), Sort::default(), false);
        assert!(matches!(tab.auto_refresh(), AutoRefresh::NotWatched));

        let request = tab.navigate(Navigation::Refresh).unwrap();
        let mut result = request.run();
        result.watch = Some(Err(WatchError::Unsupported {
            location: location.clone(),
        }));
        assert_eq!(tab.finish(result), ReadOutcome::Committed);
        let AutoRefresh::Off(error) = tab.auto_refresh() else {
            panic!("자동 새로 고침 안 됨이어야 한다");
        };
        assert_eq!(error.location(), &location);
        assert!(error.to_string().contains("감시할 수 없습니다"));
        assert_eq!(tab.watched_location(), None);

        // 새로 고침(F5)은 그대로 된다. 감시 없이 같은 위치를 다시 읽으면 상태는 그대로다.
        std::fs::write(dir.path().join("새 파일"), b"x").unwrap();
        let request = tab.navigate(Navigation::Refresh).unwrap();
        assert_eq!(tab.finish(request.run()), ReadOutcome::Committed);
        assert_eq!(tab.len(), 1);
        assert!(matches!(tab.auto_refresh(), AutoRefresh::Off(_)));

        // 다시 감시가 걸리면 켜진다.
        let request = tab.navigate(Navigation::Refresh).unwrap();
        let mut result = request.run();
        result.watch = Some(Ok(Watch::new(())));
        tab.finish(result);
        assert!(matches!(tab.auto_refresh(), AutoRefresh::On));
    }

    #[test]
    fn 지금_감시가_아닌_변경은_버리고_넘침은_다시_읽기를_요구한다() {
        let dir = temp_dir();
        std::fs::write(dir.path().join("a"), b"a").unwrap();
        let mut tab = watched_tab(dir.path());
        let mut stale = batch(&tab, vec![("a".into(), None)], false);
        stale.watch = RequestId(stale.watch.0 + 100);
        assert_eq!(tab.apply_changes(stale), ChangeOutcome::Stale);
        assert_eq!(tab.len(), 1);

        let overflow = batch(&tab, Vec::new(), true);
        assert_eq!(tab.apply_changes(overflow), ChangeOutcome::Reload);
        let request = tab.reload().expect("기다리는 읽기가 없으면 다시 읽는다");
        // 기다리는 읽기가 있으면 가로채지 않는다.
        assert!(tab.reload().is_none());
        assert_eq!(tab.finish(request.run()), ReadOutcome::Committed);
        assert_eq!(tab.location(), &Location::file_system(dir.path()));
        assert!(tab.relocation().is_none());

        // 감시 없이 다른 위치로 옮기면 앞 위치의 감시가 풀린다.
        let mut tab = watched_tab(dir.path());
        let other = temp_dir();
        let request = tab
            .navigate(Navigation::Go(Location::file_system(other.path())))
            .unwrap();
        tab.finish(request.run());
        assert_eq!(tab.watched_location(), None);
    }

    #[test]
    fn 바뀐_항목만_넣고_빼고_다시_정렬한다() {
        let dir = temp_dir();
        for name in ["b", "d"] {
            std::fs::write(dir.path().join(name), name).unwrap();
        }
        let mut tab = watched_tab(dir.path());
        let item = |name: &str, size| Item {
            name: name.into(),
            is_folder: false,
            size,
            modified: std::time::SystemTime::UNIX_EPOCH,
            attributes: Default::default(),
        };
        let changes = vec![
            ("a".into(), Some(item("a", 1))),
            ("d".into(), None),
            ("b".into(), Some(item("b", 7))),
        ];
        let applied = batch(&tab, changes, false);
        assert_eq!(tab.apply_changes(applied), ChangeOutcome::Applied);
        let names: Vec<_> = tab.items().map(|i| i.name.clone()).collect();
        assert_eq!(names, ["a", "b"]);
        assert_eq!(tab.get(1).unwrap().size, 7);
    }
}
