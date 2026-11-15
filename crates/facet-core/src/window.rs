//! 화면 상태: 앱 > 창 > 패널 > 탭.
//!
//! 창은 패널 목록을, 패널은 탭 목록을 갖고, 앱은 창 목록을 갖는다. 지금은 창·패널이 하나씩이고 패널에 탭이
//! 여럿이다. 패널 둘·창 여러 개는 목록에 더하고 활성 번호를 옮기는 것으로 늘린다.
//!
//! 탭 연산은 이 상태 모델에서 하고 앱은 결과를 그린다: 탭 열기([`App::open_tab`], 휠 클릭은
//! [`App::open_in_new_tab`]), 닫기([`App::close_tab`]: 패널의 마지막 탭이면 패널이, 창의 마지막 패널이면 창이
//! 닫히고, 창이 없으면 앱이 끝난다 [`App::is_finished`]), 활성 탭 바꾸기([`App::activate_tab`],
//! [`App::switch_tab`]: 다음·이전은 끝에서 처음으로 돈다). 패널에서 활성 탭만 보이고, 나머지는 숨어 변경 감시를
//! 놓는다([`Tab::hide`]). 다시 보이면 갖고 있던 목록을 먼저 보이며 다시 읽고 감시를 다시 건다([`Tab::show`]).
//!
//! 앱은 탭의 읽기 요청을 일꾼 스레드에서 읽고([`App::navigate`]), 그 결과를 요청한 탭에 넘긴다
//! ([`App::poll`]·[`App::wait`]). 결과는 탭 번호로 찾아가므로 탭이 여럿이어도 섞이지 않는다.
//! 읽을 때 위치의 변경 감시도 함께 걸어, 묶은 변경을 같은 통로로 받아 탭에 반영한다. 알림을 잃었거나 위치가
//! 사라졌으면 그 탭을 다시 읽는다([`crate::tab::Tab::reload`]).
//! 숨김 항목 보이기/감추기는 앱 전체에 하나인 상태다. 기본은 감춤이고 저장하지 않는다.
//!
//! 명령([`crate::command::CommandId`])은 활성 탭에 듣는다([`App::execute`]). 파일 열기는 셸 일꾼 스레드
//! ([`ShellWorker`], 처음 열 때 띄운다)가 하고, 그 결과도 같은 통로로 받아 탭에 넘긴다.
//!
//! 주소창: 친 글자로 가기([`App::go_to_text`])는 해석한 뒤 다른 이동처럼 일꾼 스레드에서 읽는다. 자동 완성 후보
//! ([`App::complete_address`])는 폴더를 읽느라 막히므로 일꾼 스레드에서 읽고 같은 통로로 받는다.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use facet_win::{OpenError, OpenResult, ShellWorker};

use crate::command::{self, CommandId};
use crate::details::Sort;
use crate::tab::{
    ChangeOutcome, CompletionId, CompletionResult, Navigation, ReadOutcome, ReadRequest,
    ReadResult, Tab,
};
use crate::watch::ChangeBatch;
use crate::{Candidate, Location, provider_for};

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

    /// 탭들을 차례로.
    pub fn tabs(&self) -> impl ExactSizeIterator<Item = (TabId, &Tab)> + '_ {
        self.tabs.iter().map(|(id, tab)| (*id, tab))
    }

    /// 탭 수. 열린 패널에는 탭이 하나 이상 있다.
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// 탭이 없는가. 열린 패널은 늘 탭을 갖는다(마지막 탭을 닫으면 패널이 닫힌다).
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// 보이고 있는 탭이 몇 번째인가(0부터).
    pub fn active_index(&self) -> usize {
        self.active
    }

    /// 탭 `id`가 몇 번째인가.
    pub fn position(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|(tab, _)| *tab == id)
    }

    /// 활성 탭을 바꿀 때 갈 탭의 번호(0부터). 없는 탭이면 `None`이다.
    pub fn switch_target(&self, switch: TabSwitch) -> Option<usize> {
        let len = self.tabs.len();
        if len == 0 {
            return None;
        }
        match switch {
            TabSwitch::Next => Some((self.active + 1) % len),
            TabSwitch::Previous => Some((self.active + len - 1) % len),
            TabSwitch::Nth(n) => (n >= 1 && n <= len).then(|| n - 1),
            TabSwitch::Last => Some(len - 1),
        }
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

/// 활성 탭을 어디로 바꾸는가([`App::switch_tab`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabSwitch {
    /// 다음 탭. 끝에서는 처음으로 돈다.
    Next,
    /// 이전 탭. 처음에서는 끝으로 돈다.
    Previous,
    /// n번째 탭(1부터).
    Nth(usize),
    /// 마지막 탭.
    Last,
}

/// 탭을 닫으면서 함께 닫힌 것([`App::close_tab`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closed {
    /// 패널의 마지막 탭이라 패널이 닫혔다.
    pub panel: bool,
    /// 창의 마지막 패널이라 창이 닫혔다.
    pub window: bool,
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

/// 일꾼 스레드가 보내는 것: 읽기 결과나 묶은 변경.
enum Message {
    Read {
        tab: TabId,
        result: ReadResult,
    },
    Changes {
        tab: TabId,
        batch: ChangeBatch,
    },
    Opened {
        tab: TabId,
        result: OpenResult,
    },
    Completed {
        tab: TabId,
        result: CompletionResult,
    },
}

/// 앱이 정한 깨우기([`App::set_waker`]).
type Waker = Arc<dyn Fn() + Send + Sync>;

/// 일꾼 스레드가 결과를 보내는 곳. 보낸 뒤 깨우기가 있으면 부른다.
#[derive(Clone)]
struct Outbox {
    sender: Sender<Message>,
    waker: Arc<Mutex<Option<Waker>>>,
}

impl Outbox {
    fn send(&self, message: Message) {
        // 앱이 이미 없어졌으면 결과를 받을 곳이 없다.
        if self.sender.send(message).is_err() {
            return;
        }
        let waker = self.waker.lock().ok().and_then(|w| w.clone());
        if let Some(waker) = waker {
            waker();
        }
    }
}

/// 탭에 넘긴 결과 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    /// 읽기 결과.
    Read(ReadOutcome),
    /// 묶은 변경. [`ChangeOutcome::Reload`]였으면 앱이 이미 다시 읽기를 시작했다.
    Changes(ChangeOutcome),
    /// 기본 프로그램으로 열기의 결과: 열었는가. 열지 못했으면 그 탭의 [`Tab::open_error`]에 까닭이 있다.
    Opened(bool),
    /// 자동 완성 후보([`App::complete_address`]). 그 탭의 마지막 요청 결과면 후보를, 늦게 온 앞선 요청의
    /// 결과면 `None`을 담는다.
    Completion {
        id: CompletionId,
        candidates: Option<Vec<Candidate>>,
    },
}

/// 명령을 실행한 결과.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Execution {
    /// 실행했다. 이동·열기처럼 결과가 나중에 오는 것은 [`App::poll`]·[`App::wait_update`]로 받는다.
    Done,
    /// 지금 실행할 수 없는 명령이라 아무것도 하지 않았다([`CommandId::is_enabled`]).
    Disabled,
    /// 화면이 할 일: 주소창을 글자로 고쳐 쓰는 모드로 바꾸고 전체 경로를 선택한다.
    EditAddress,
    /// 화면이 할 일: 명령 팔레트를 연다.
    ShowCommandPalette,
    /// 마지막 탭을 닫아 창이 닫혔다. 창이 남지 않았으면([`App::is_finished`]) 앱을 끝낸다.
    WindowClosed,
}

/// 앱 전체의 화면 상태: 창 목록과 앱 전체에 하나인 설정(숨김 항목 보이기).
pub struct App {
    windows: Vec<Window>,
    active: usize,
    show_hidden: bool,
    next_tab: u64,
    /// 셸 일꾼 스레드. 콜드 스타트를 늦추지 않도록 처음 파일을 열 때 띄운다.
    shell: Option<ShellWorker>,
    outbox: Outbox,
    receiver: Receiver<Message>,
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
            shell: None,
            outbox: Outbox {
                sender,
                waker: Arc::default(),
            },
            receiver,
        };
        let id = app.new_tab_id();
        let location = start.unwrap_or_else(home_location);
        let tab = Tab::new(location, Sort::default(), app.show_hidden);
        app.windows.push(Window::new(Panel::new(id, tab)));
        app.navigate(Navigation::Refresh);
        app
    }

    /// 일꾼 스레드가 결과(읽기 결과, 묶은 변경, 열기 결과)를 보낼 때마다 부를 깨우기를 정한다. 화면은 여기서 자기
    /// 이벤트 루프를 깨워 [`App::poll`]을 부른다. 깨우기는 일꾼 스레드에서 불리므로 막지 말아야 한다.
    ///
    /// 정하기 전에 이미 온 결과로는 깨우지 않으므로, 정한 뒤 [`App::poll`]을 한 번 부른다.
    pub fn set_waker(&mut self, waker: impl Fn() + Send + Sync + 'static) {
        if let Ok(mut slot) = self.outbox.waker.lock() {
            *slot = Some(Arc::new(waker));
        }
    }

    fn new_tab_id(&mut self) -> TabId {
        let id = TabId(self.next_tab);
        self.next_tab += 1;
        id
    }

    /// 창들을 차례로. 지금은 하나뿐이고, 마지막 창이 닫히면 비어 있다.
    pub fn windows(&self) -> &[Window] {
        &self.windows
    }

    /// 창이 하나도 남지 않아 앱이 끝났는가. 끝난 앱에서는 활성 창·탭을 묻지 않는다(창이 없다).
    pub fn is_finished(&self) -> bool {
        self.windows.is_empty()
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

    /// 활성 탭의 번호.
    pub fn active_tab_id(&self) -> TabId {
        self.active_window().active_panel().active_tab_id()
    }

    /// 열린 탭 `id`. 닫혔으면 `None`이다.
    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        self.windows
            .iter()
            .flat_map(|w| w.panels.iter())
            .flat_map(Panel::tabs)
            .find(|(tab_id, _)| *tab_id == id)
            .map(|(_, tab)| tab)
    }

    /// 탭 `id`가 있는 창·패널·탭 번호.
    fn find(&self, id: TabId) -> Option<(usize, usize, usize)> {
        self.windows.iter().enumerate().find_map(|(w, window)| {
            window
                .panels
                .iter()
                .enumerate()
                .find_map(|(p, panel)| panel.position(id).map(|t| (w, p, t)))
        })
    }

    /// `location`을 보일 새 탭을 활성 패널의 활성 탭 바로 뒤에 연다. 정렬은 활성 탭을 따르고 기록은 비어 있다.
    ///
    /// `activate`면 새 탭을 활성으로 하고 읽기 시작한다(앞서 보이던 탭은 숨는다). 아니면 지금 탭을 그대로 두고,
    /// 새 탭은 숨은 채로 두었다가 처음 보일 때 읽는다.
    pub fn open_tab(&mut self, location: Location, activate: bool) -> TabId {
        let id = self.new_tab_id();
        let sort = self.active_tab().sort();
        let mut tab = Tab::new(location, sort, self.show_hidden);
        if !activate {
            tab.hide();
        }
        let panel = self.active_window_mut().active_panel_mut();
        let at = panel.active + 1;
        panel.tabs.insert(at, (id, tab));
        if activate {
            panel.tabs[panel.active].1.hide();
            panel.active = at;
            self.navigate(Navigation::Refresh);
        }
        id
    }

    /// 활성 탭의 `index`번째 보이는 항목이 폴더면 그 폴더를 새 탭으로 연다(휠 클릭). 지금 탭이 그대로 활성이고
    /// 새 탭은 그 바로 뒤에 숨은 채로 붙는다([`App::open_tab`]). 폴더가 아니거나 항목이 없으면 `None`이다.
    pub fn open_in_new_tab(&mut self, index: usize) -> Option<TabId> {
        let tab = self.active_tab();
        let item = tab.get(index).filter(|item| item.is_folder)?;
        let location = provider_for(tab.location()).child(tab.location(), &item.name);
        Some(self.open_tab(location, false))
    }

    /// 탭 `id`를 활성 탭으로 한다: 그 창·패널을 활성으로 하고, 패널에서 보이던 탭은 숨기고, 그 탭을 다시 보인다
    /// ([`Tab::show`]: 갖고 있던 목록을 두고 다시 읽으며 감시를 다시 건다). 없는 탭이면 `false`다.
    pub fn activate_tab(&mut self, id: TabId) -> bool {
        let Some((w, p, t)) = self.find(id) else {
            return false;
        };
        self.active = w;
        self.windows[w].active = p;
        self.show_tab(t);
        true
    }

    /// 활성 패널에서 활성 탭을 바꾼다. 갈 탭이 없으면(n번째 탭이 없음) 아무것도 하지 않고 `false`다.
    pub fn switch_tab(&mut self, switch: TabSwitch) -> bool {
        match self.active_window().active_panel().switch_target(switch) {
            Some(index) => {
                self.show_tab(index);
                true
            }
            None => false,
        }
    }

    /// 활성 패널의 `index`번째 탭을 활성으로 한다. 앞서 보이던 탭은 숨는다.
    fn show_tab(&mut self, index: usize) {
        let panel = self.active_window_mut().active_panel_mut();
        if panel.active != index {
            panel.tabs[panel.active].1.hide();
            panel.active = index;
        }
        let (id, tab) = &mut panel.tabs[index];
        let id = *id;
        if let Some(request) = tab.show() {
            self.spawn_read(id, request);
        }
    }

    /// 탭 `id`를 닫는다. 그 탭이 패널의 활성 탭이면 오른쪽 이웃(끝이었으면 왼쪽 이웃)이 활성이 되어 다시 보인다.
    /// 패널의 마지막 탭이면 패널이 닫히고, 창의 마지막 패널이면 창이 닫힌다. 창이 하나도 남지 않으면 앱이
    /// 끝난다([`App::is_finished`]). 없는 탭이면 `None`이다.
    ///
    /// 닫은 탭의 감시는 풀리고, 그 탭에 늦게 온 결과는 버린다.
    pub fn close_tab(&mut self, id: TabId) -> Option<Closed> {
        let (w, p, t) = self.find(id)?;
        let window = &mut self.windows[w];
        let panel = &mut window.panels[p];
        panel.tabs.remove(t);
        if panel.tabs.is_empty() {
            window.panels.remove(p);
            let window_closed = window.panels.is_empty();
            if window_closed {
                self.windows.remove(w);
                if self.active >= w && self.active > 0 {
                    self.active -= 1;
                }
            } else if window.active >= p && window.active > 0 {
                window.active -= 1;
            }
            return Some(Closed {
                panel: true,
                window: window_closed,
            });
        }
        if t < panel.active {
            panel.active -= 1;
        } else if t == panel.active {
            panel.active = panel.active.min(panel.tabs.len() - 1);
            let (next_id, tab) = &mut panel.tabs[panel.active];
            let next_id = *next_id;
            if let Some(request) = tab.show() {
                self.spawn_read(next_id, request);
            }
        }
        Some(Closed {
            panel: false,
            window: false,
        })
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
    /// 요청마다 스레드를 따로 둔다. 읽기 전에 위치의 변경 감시를 건다.
    fn spawn_read(&self, tab: TabId, request: ReadRequest) {
        let outbox = self.outbox.clone();
        let changes = self.outbox.clone();
        let read = move || {
            let result = request.run_watched(move |batch| {
                changes.send(Message::Changes { tab, batch });
            });
            outbox.send(Message::Read { tab, result });
        };
        thread::Builder::new()
            .name("facet-read".into())
            .spawn(read)
            .expect("목록 읽기 일꾼 스레드를 만들지 못했다");
    }

    /// 이미 도착한 읽기 결과와 묶은 변경을 모두 탭에 넘긴다. 막지 않는다. 넘긴 것마다 탭과 결과를 돌려준다.
    pub fn poll(&mut self) -> Vec<(TabId, Update)> {
        let mut updates = Vec::new();
        while let Ok(message) = self.receiver.try_recv() {
            updates.push(self.deliver(message));
        }
        updates
    }

    /// 읽기 결과 하나가 올 때까지 `timeout`까지 기다려 탭에 넘긴다. 시간 안에 오지 않으면 `None`이다.
    /// 그 사이에 온 묶은 변경도 탭에 넘긴다.
    pub fn wait(&mut self, timeout: Duration) -> Option<(TabId, ReadOutcome)> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if let (tab, Update::Read(outcome)) = self.wait_update(left)? {
                return Some((tab, outcome));
            }
        }
    }

    /// 읽기 결과나 묶은 변경 하나가 올 때까지 `timeout`까지 기다려 탭에 넘긴다. 시간 안에 오지 않으면 `None`이다.
    pub fn wait_update(&mut self, timeout: Duration) -> Option<(TabId, Update)> {
        match self.receiver.recv_timeout(timeout) {
            Ok(message) => Some(self.deliver(message)),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => None,
        }
    }

    fn deliver(&mut self, message: Message) -> (TabId, Update) {
        match message {
            Message::Read { tab: id, result } => {
                let outcome = match self.tab_mut(id) {
                    Some(tab) => tab.finish(result),
                    // 탭이 이미 닫혔다.
                    None => ReadOutcome::Stale,
                };
                (id, Update::Read(outcome))
            }
            Message::Changes { tab: id, batch } => {
                let Some(tab) = self.tab_mut(id) else {
                    return (id, Update::Changes(ChangeOutcome::Stale));
                };
                let outcome = tab.apply_changes(batch);
                if outcome == ChangeOutcome::Reload
                    && let Some(request) = tab.reload()
                {
                    self.spawn_read(id, request);
                }
                (id, Update::Changes(outcome))
            }
            Message::Opened { tab: id, result } => {
                let opened = result.is_ok();
                if let Some(tab) = self.tab_mut(id) {
                    tab.set_open_error(result.err());
                }
                (id, Update::Opened(opened))
            }
            Message::Completed { tab: id, result } => {
                let completion = result.id();
                let candidates = self
                    .tab_mut(id)
                    .and_then(|tab| tab.finish_completion(result));
                (
                    id,
                    Update::Completion {
                        id: completion,
                        candidates,
                    },
                )
            }
        }
    }

    fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs_mut()
            .find(|(tab_id, _)| *tab_id == id)
            .map(|(_, tab)| tab)
    }

    /// 주소창에 친 글자로 활성 탭을 옮긴다([`Tab::go_to_text`]). 해석했으면 일꾼 스레드에서 읽기 시작하고
    /// `true`다. 결과는 다른 이동처럼 [`App::poll`]로 받는다. 해석하지 못했으면 읽지 않고 `false`이며, 탭은
    /// 원래 위치에 남고 [`Tab::address_error`]에 까닭이 있다.
    pub fn go_to_text(&mut self, text: &str) -> bool {
        let tab = self.active_tab_id();
        match self.active_tab_mut().go_to_text(text) {
            Some(request) => {
                self.spawn_read(tab, request);
                true
            }
            None => false,
        }
    }

    /// 주소창에 고쳐 쓰는 글자의 자동 완성 후보를 일꾼 스레드에서 읽기 시작한다. 막지 않고 곧바로 요청 번호를
    /// 돌려준다. 후보는 [`Update::Completion`]으로 온다. 앞선 요청의 결과가 늦게 오면 후보 없이(`None`) 온다.
    pub fn complete_address(&mut self, text: &str) -> CompletionId {
        let tab = self.active_tab_id();
        let request = self.active_tab_mut().complete(text);
        let id = request.id();
        let outbox = self.outbox.clone();
        let read = move || {
            let result = request.run();
            outbox.send(Message::Completed { tab, result });
        };
        thread::Builder::new()
            .name("facet-complete".into())
            .spawn(read)
            .expect("자동 완성 일꾼 스레드를 만들지 못했다");
        id
    }

    /// 활성 탭의 정렬을 바꾼다. 다시 읽지 않는다.
    pub fn set_sort(&mut self, sort: Sort) {
        self.active_tab_mut().set_sort(sort);
    }

    /// 명령을 활성 패널·탭에서 지금 실행할 수 있는가([`CommandId::is_enabled_in`]). 앱이 끝났으면 아무 명령도
    /// 실행할 수 없다.
    pub fn can_execute(&self, command: CommandId) -> bool {
        !self.is_finished() && command.is_enabled_in(self.active_window().active_panel())
    }

    /// 명령 팔레트 검색: 앱이 주는 명령 이름에서 찾고, 실행 가능 여부는 활성 패널·탭을 따른다([`command::search`]).
    pub fn search_commands<S: AsRef<str>>(
        &self,
        query: &str,
        name: impl Fn(CommandId) -> S,
    ) -> Vec<command::PaletteEntry> {
        command::search(query, name, self.active_window().active_panel())
    }

    /// 명령을 활성 탭에서 실행한다. 지금 실행할 수 없으면 아무것도 하지 않고 [`Execution::Disabled`]다.
    ///
    /// 열기는 커서 항목이 폴더면 그 안으로 가고, 파일이면 셸 일꾼 스레드에서 기본 프로그램으로 연다. 여는 결과는
    /// [`Update::Opened`]로 오고, 실패하면 탭의 [`Tab::open_error`]에 까닭이 남는다. 막지 않는다.
    pub fn execute(&mut self, command: CommandId) -> Execution {
        if !self.can_execute(command) {
            return Execution::Disabled;
        }
        let navigation = match command {
            CommandId::Open => return self.open_cursor_item(),
            CommandId::Back => Navigation::Back,
            CommandId::Forward => Navigation::Forward,
            CommandId::Up => Navigation::Up,
            CommandId::Refresh => Navigation::Refresh,
            CommandId::SelectAll => {
                self.active_tab_mut().select_all();
                return Execution::Done;
            }
            CommandId::ToggleCursorSelection => {
                self.active_tab_mut().toggle_cursor_selection();
                return Execution::Done;
            }
            CommandId::EditAddress => return Execution::EditAddress,
            CommandId::ToggleHidden => {
                self.set_show_hidden(!self.show_hidden);
                return Execution::Done;
            }
            CommandId::CommandPalette => return Execution::ShowCommandPalette,
            CommandId::SortByName
            | CommandId::SortByModified
            | CommandId::SortByType
            | CommandId::SortBySize => {
                if let Some(column) = command.sort_column() {
                    let sort = command::sort_toggled(self.active_tab().sort(), column);
                    self.set_sort(sort);
                }
                return Execution::Done;
            }
            CommandId::NewTab => {
                let location = self.active_tab().location().clone();
                self.open_tab(location, true);
                return Execution::Done;
            }
            CommandId::OpenInNewTab => {
                let opened = self
                    .active_tab()
                    .cursor()
                    .and_then(|index| self.open_in_new_tab(index));
                return match opened {
                    Some(_) => Execution::Done,
                    None => Execution::Disabled,
                };
            }
            CommandId::CloseTab => {
                let id = self.active_tab_id();
                return match self.close_tab(id) {
                    Some(Closed { window: true, .. }) => Execution::WindowClosed,
                    Some(_) => Execution::Done,
                    None => Execution::Disabled,
                };
            }
            CommandId::NextTab
            | CommandId::PreviousTab
            | CommandId::GoToTab(_)
            | CommandId::LastTab => {
                let switch = match command {
                    CommandId::NextTab => TabSwitch::Next,
                    CommandId::PreviousTab => TabSwitch::Previous,
                    CommandId::GoToTab(n) => TabSwitch::Nth(usize::from(n)),
                    _ => TabSwitch::Last,
                };
                return if self.switch_tab(switch) {
                    Execution::Done
                } else {
                    Execution::Disabled
                };
            }
        };
        if self.navigate(navigation) {
            Execution::Done
        } else {
            Execution::Disabled
        }
    }

    fn open_cursor_item(&mut self) -> Execution {
        let tab = self.active_tab();
        let Some(item) = tab.cursor_item() else {
            return Execution::Disabled;
        };
        if item.is_folder {
            let name = item.name.clone();
            self.navigate(Navigation::Open(name));
            return Execution::Done;
        }
        let path = tab
            .location()
            .file_system_path()
            .map(|folder| folder.join(&item.name));
        let id = self.active_tab_id();
        self.active_tab_mut().set_open_error(None);
        let Some(path) = path else {
            // 실제 폴더 경로가 없는 위치(가상 폴더 등)의 파일은 아직 열 수 없다.
            self.active_tab_mut()
                .set_open_error(Some(OpenError::Unsupported));
            return Execution::Done;
        };
        if self.shell.is_none() {
            match ShellWorker::spawn() {
                Ok(worker) => self.shell = Some(worker),
                Err(error) => {
                    self.active_tab_mut().set_open_error(Some(OpenError::Other {
                        code: 0,
                        message: error.to_string(),
                    }));
                    return Execution::Done;
                }
            }
        }
        let Some(shell) = &self.shell else {
            return Execution::Done;
        };
        let reply = shell.open_with_default_program(path);
        let outbox = self.outbox.clone();
        let forward = move || {
            if let Ok(result) = reply.recv() {
                outbox.send(Message::Opened { tab: id, result });
            }
        };
        thread::Builder::new()
            .name("facet-open".into())
            .spawn(forward)
            .expect("열기 결과를 기다리는 스레드를 만들지 못했다");
        Execution::Done
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
