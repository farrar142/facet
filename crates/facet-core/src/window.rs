//! 화면 상태: 앱 > 창 > 패널 > 탭.
//!
//! 지금(S1)은 창·패널·탭을 각 하나씩만 만든다. 그래도 창은 패널 목록을, 패널은 탭 목록을 갖고, 앱은 창 목록을
//! 갖는다. 탭 여러 개·패널 둘·창 여러 개(S2)는 목록에 더하고 활성 번호를 옮기는 것으로 늘린다.
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

use crate::{Candidate, Location};
use crate::command::{self, CommandId};
use crate::details::Sort;
use crate::tab::{
    ChangeOutcome, CompletionId, CompletionResult, Navigation, ReadOutcome, ReadRequest, ReadResult,
    Tab,
};
use crate::watch::ChangeBatch;

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

/// 일꾼 스레드가 보내는 것: 읽기 결과나 묶은 변경.
enum Message {
    Read { tab: TabId, result: ReadResult },
    Changes { tab: TabId, batch: ChangeBatch },
    Opened { tab: TabId, result: OpenResult },
    Completed { tab: TabId, result: CompletionResult },
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

    /// 명령을 활성 탭에서 지금 실행할 수 있는가([`CommandId::is_enabled`]).
    pub fn can_execute(&self, command: CommandId) -> bool {
        command.is_enabled(self.active_tab())
    }

    /// 명령 팔레트 검색: 앱이 주는 명령 이름에서 찾고, 실행 가능 여부는 활성 탭을 따른다([`command::search`]).
    pub fn search_commands<S: AsRef<str>>(
        &self,
        query: &str,
        name: impl Fn(CommandId) -> S,
    ) -> Vec<command::PaletteEntry> {
        command::search(query, name, self.active_tab())
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
