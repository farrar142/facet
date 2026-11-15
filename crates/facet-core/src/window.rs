//! 화면 상태: 앱 > 창 > 패널 > 탭.
//!
//! 지금(S1)은 창·패널·탭을 각 하나씩만 만든다. 그래도 창은 패널 목록을, 패널은 탭 목록을 갖고, 앱은 창 목록을
//! 갖는다. 탭 여러 개·패널 둘·창 여러 개(S2)는 목록에 더하고 활성 번호를 옮기는 것으로 늘린다.
//!
//! 앱은 탭의 읽기 요청을 일꾼 스레드에서 읽고([`App::navigate`]), 그 결과를 요청한 탭에 넘긴다
//! ([`App::poll`]·[`App::wait`]). 결과는 탭 번호로 찾아가므로 탭이 여럿이어도 섞이지 않는다.
//! 숨김 항목 보이기/감추기는 앱 전체에 하나인 상태다. 기본은 감춤이고 저장하지 않는다.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use crate::Location;
use crate::details::Sort;
use crate::tab::{Navigation, ReadOutcome, ReadRequest, ReadResult, Tab};

/// 앱 안의 탭 하나를 가리키는 번호. 탭을 다른 패널·창으로 옮겨도 바뀌지 않는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TabId(u64);

/// 창을 나눈 영역. 자기 탭 묶음을 갖는다.
#[derive(Debug)]
pub struct Panel {
    tabs: Vec<(TabId, Tab)>,
    active: usize,
}

impl Panel {
    fn new(id: TabId, tab: Tab) -> Self {
        Panel {
            tabs: vec![(id, tab)],
            active: 0,
        }
    }

    /// 탭들을 차례로. 지금은 하나뿐이다.
    pub fn tabs(&self) -> impl ExactSizeIterator<Item = (TabId, &Tab)> + '_ {
        self.tabs.iter().map(|(id, tab)| (*id, tab))
    }

    /// 보이고 있는 탭의 번호.
    pub fn active_tab_id(&self) -> TabId {
        self.tabs[self.active].0
    }

    /// 보이고 있는 탭.
    pub fn active_tab(&self) -> &Tab {
        &self.tabs[self.active].1
    }

    /// 보이고 있는 탭.
    pub fn active_tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active].1
    }

    fn tabs_mut(&mut self) -> impl Iterator<Item = (TabId, &mut Tab)> + '_ {
        self.tabs.iter_mut().map(|(id, tab)| (*id, tab))
    }
}

/// facet의 최상위 화면 하나. 패널을 하나 또는 둘 갖는다(지금은 하나).
#[derive(Debug)]
pub struct Window {
    panels: Vec<Panel>,
    active: usize,
}

impl Window {
    fn new(panel: Panel) -> Self {
        Window {
            panels: vec![panel],
            active: 0,
        }
    }

    /// 패널들을 차례로.
    pub fn panels(&self) -> &[Panel] {
        &self.panels
    }

    /// 활성 패널: 키보드 입력을 받는 패널.
    pub fn active_panel(&self) -> &Panel {
        &self.panels[self.active]
    }

    /// 활성 패널.
    pub fn active_panel_mut(&mut self) -> &mut Panel {
        &mut self.panels[self.active]
    }
}

/// 일꾼 스레드가 돌려준 읽기 결과.
struct Done {
    tab: TabId,
    result: ReadResult,
}

/// 앱 전체의 화면 상태: 창 목록과 앱 전체에 하나인 설정(숨김 항목 보이기).
pub struct App {
    windows: Vec<Window>,
    active: usize,
    show_hidden: bool,
    next_tab: u64,
    sender: Sender<Done>,
    receiver: Receiver<Done>,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("windows", &self.windows)
            .field("active", &self.active)
            .field("show_hidden", &self.show_hidden)
            .finish_non_exhaustive()
    }
}

impl App {
    /// 창·패널·탭 하나씩으로 시작하고, 탭이 `start`를 읽기 시작한다. `start`가 없으면 사용자 홈 폴더
    /// ([`home_location`])를 연다. 숨김 항목은 감춘다.
    ///
    /// 읽기는 일꾼 스레드에서 하므로 돌아왔을 때 목록은 아직 비어 있을 수 있다. 시작 위치를 읽지 못하면
    /// 탭은 그 위치에 빈 목록과 오류를 갖는다.
    pub fn new(start: Option<Location>) -> Self {
        let (sender, receiver) = mpsc::channel();
        let mut app = App {
            windows: Vec::new(),
            active: 0,
            show_hidden: false,
            next_tab: 0,
            sender,
            receiver,
        };
        let id = app.new_tab_id();
        let location = start.unwrap_or_else(home_location);
        let tab = Tab::new(location, Sort::default(), app.show_hidden);
        app.windows.push(Window::new(Panel::new(id, tab)));
        app.navigate(Navigation::Refresh);
        app
    }

    fn new_tab_id(&mut self) -> TabId {
        let id = TabId(self.next_tab);
        self.next_tab += 1;
        id
    }

    /// 창들을 차례로. 지금은 하나뿐이다.
    pub fn windows(&self) -> &[Window] {
        &self.windows
    }

    /// 사용자가 보고 있는 창.
    pub fn active_window(&self) -> &Window {
        &self.windows[self.active]
    }

    /// 사용자가 보고 있는 창.
    pub fn active_window_mut(&mut self) -> &mut Window {
        &mut self.windows[self.active]
    }

    /// 활성 창의 활성 패널의 보이는 탭. 명령은 이 탭에 듣는다.
    pub fn active_tab(&self) -> &Tab {
        self.active_window().active_panel().active_tab()
    }

    /// 활성 탭.
    pub fn active_tab_mut(&mut self) -> &mut Tab {
        self.active_window_mut().active_panel_mut().active_tab_mut()
    }

    fn active_tab_id(&self) -> TabId {
        self.active_window().active_panel().active_tab_id()
    }

    fn tabs_mut(&mut self) -> impl Iterator<Item = (TabId, &mut Tab)> + '_ {
        self.windows
            .iter_mut()
            .flat_map(|w| w.panels.iter_mut())
            .flat_map(Panel::tabs_mut)
    }

    /// 활성 탭에서 이동을 시작하고 그 위치를 일꾼 스레드에서 읽는다. 이동할 수 없으면(기록이 없음,
    /// 상위가 없음) 아무것도 하지 않고 `false`를 돌려준다.
    ///
    /// 막지 않고 곧바로 돌아온다. 결과는 [`App::poll`]이나 [`App::wait`]가 탭에 넘긴다.
    pub fn navigate(&mut self, navigation: Navigation) -> bool {
        let tab = self.active_tab_id();
        match self.active_tab_mut().navigate(navigation) {
            Some(request) => {
                self.spawn_read(tab, request);
                true
            }
            None => false,
        }
    }

    /// 요청 하나를 새 일꾼 스레드에서 읽는다. 느린 위치(네트워크 등) 하나가 다른 이동을 막지 않도록
    /// 요청마다 스레드를 따로 둔다.
    fn spawn_read(&self, tab: TabId, request: ReadRequest) {
        let sender = self.sender.clone();
        let read = move || {
            let result = request.run();
            // 앱이 이미 없어졌으면 결과를 받을 곳이 없다.
            let _ = sender.send(Done { tab, result });
        };
        thread::Builder::new()
            .name("facet-read".into())
            .spawn(read)
            .expect("목록 읽기 일꾼 스레드를 만들지 못했다");
    }

    /// 이미 도착한 읽기 결과를 모두 탭에 넘긴다. 막지 않는다. 넘긴 결과마다 탭과 결과를 돌려준다.
    pub fn poll(&mut self) -> Vec<(TabId, ReadOutcome)> {
        let mut outcomes = Vec::new();
        while let Ok(done) = self.receiver.try_recv() {
            outcomes.push(self.deliver(done));
        }
        outcomes
    }

    /// 읽기 결과 하나가 올 때까지 `timeout`까지 기다려 탭에 넘긴다. 시간 안에 오지 않으면 `None`이다.
    pub fn wait(&mut self, timeout: Duration) -> Option<(TabId, ReadOutcome)> {
        match self.receiver.recv_timeout(timeout) {
            Ok(done) => Some(self.deliver(done)),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => None,
        }
    }

    fn deliver(&mut self, done: Done) -> (TabId, ReadOutcome) {
        let outcome = match self.tabs_mut().find(|(id, _)| *id == done.tab) {
            Some((_, tab)) => tab.finish(done.result),
            // 탭이 이미 닫혔다.
            None => ReadOutcome::Stale,
        };
        (done.tab, outcome)
    }

    /// 활성 탭의 정렬을 바꾼다. 다시 읽지 않는다.
    pub fn set_sort(&mut self, sort: Sort) {
        self.active_tab_mut().set_sort(sort);
    }

    /// 숨김 항목을 보이는가. 앱 전체에 하나다.
    pub fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    /// 숨김 항목 보이기/감추기를 바꾸고 모든 탭의 목록에 반영한다. 다시 읽지 않는다.
    pub fn set_show_hidden(&mut self, show_hidden: bool) {
        self.show_hidden = show_hidden;
        for (_, tab) in self.tabs_mut() {
            tab.set_show_hidden(show_hidden);
        }
    }
}

/// 사용자 홈 폴더. 알 수 없으면 현재 폴더다.
pub fn home_location() -> Location {
    let path = std::env::home_dir()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    Location::file_system(path)
}
