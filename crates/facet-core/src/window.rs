//! 화면 상태: 앱 > 창 > 패널 > 탭.
//!
//! 창은 패널 목록을, 패널은 탭 목록을 갖고, 앱은 창 목록을 갖는다. 창은 여럿이고 패널은 하나나 둘(좌우),
//! 패널마다 탭이 여럿이다. 모든 창은 UI 프로세스 하나가 띄운다.
//!
//! 창 연산: 만들기([`App::open_window`], 「새 창」 명령은 활성 탭의 위치로), 닫기([`App::close_window`]: 그 창의
//! 탭들이 닫은 탭 목록에 들어간다), 활성 창 바꾸기([`App::activate_window`]). 창은 바뀌지 않는 번호
//! ([`WindowId`])로 가리킨다. 명령은 활성 창의 활성 패널의 활성 탭에 듣는다. 창마다 패널의 보이는 탭이 보이므로,
//! 활성 창을 바꿔도 다른 창의 탭은 숨지 않는다. 창이 하나도 남지 않고 끝나지 않은 작업도 없으면 앱이 끝난다
//! ([`App::is_finished`]: 프로세스를 끝낼지 정하는 한 곳). 창이 없는데 작업이 남아 있으면 화면은 진행 창을 띄운다
//! ([`App::shows_progress_window`]).
//!
//! 패널 연산: 나누기([`App::split_panel`]: 새 오른쪽 패널이 활성 탭과 같은 위치의 탭 하나로 시작해 활성이 된다),
//! 합치기([`App::merge_panels`]: 오른쪽 패널의 탭이 왼쪽 패널 탭 끝에 순서대로 붙는다), 활성 패널 바꾸기
//! ([`App::activate_panel`], [`App::switch_panel`]). 분할 비율(왼쪽 패널이 차지하는 몫)은 창의 상태다
//! ([`Window::split_ratio`], 한도 [`SPLIT_RATIO_MIN`]..[`SPLIT_RATIO_MAX`] 안으로 자른다).
//!
//! 탭 연산은 이 상태 모델에서 하고 앱은 결과를 그린다: 탭 열기([`App::open_tab`], 휠 클릭은
//! [`App::open_in_new_tab`]), 닫기([`App::close_tab`]: 패널의 마지막 탭이면 패널이, 창의 마지막 패널이면 창이
//! 닫히고, 창이 없으면 앱이 끝난다 [`App::is_finished`]), 활성 탭 바꾸기([`App::activate_tab`],
//! [`App::switch_tab`]: 다음·이전은 끝에서 처음으로 돈다). 패널에서 활성 탭만 보이고, 나머지는 숨어 변경 감시를
//! 놓는다([`Tab::hide`]). 다시 보이면 갖고 있던 목록을 먼저 보이며 다시 읽고 감시를 다시 건다([`Tab::show`]).
//!
//! 탭 옮기기([`App::move_tab`])는 연산 하나로 같은 패널 안 순서 바꾸기, 반대편 패널로, 다른 창으로, 새 창으로를
//! 모두 한다([`TabDestination`]). 옮긴 탭은 목적지 패널의 활성 탭이 되고 그 패널·창이 활성이 된다. 떠난 패널·창이
//! 비면 닫힌다. 탭의 기록·선택·정렬·읽은 목록은 함께 따라간다.
//!
//! 닫은 탭은 앱 전체에 하나인 닫은 탭 목록에 최근 [`CLOSED_TABS_LIMIT`]개까지 남는다([`App::closed_tabs`]). 탭을
//! 닫으면 들어가고, 여러 탭을 한 번에 넣을 수도 있다([`App::remember_closed_tabs`], 창 닫기가 쓴다). 되살리기
//! ([`App::reopen_closed_tab`])는 마지막에 넣은 탭부터 위치·기록·정렬을 그대로 활성 창의 활성 패널에 활성 탭으로
//! 열고 그 위치를 다시 읽는다. 이 목록은 저장하지 않는다.
//!
//! 세션([`crate::session`]): [`App::session`]이 지금 창·패널·탭·위치·정렬·분할 비율·창 자리([`Window::placement`])를
//! 세션으로 내고, [`App::restore`]가 세션을 되살린다(보이는 탭만 읽기 시작하고, 나머지는 처음 보일 때 읽는다).
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
//! 반대편 패널로 복사·이동: 명령은 확인 줄을 띄우라고만 하고([`Execution::ConfirmTransfer`]), 확인 줄에 보일 원본·대상·
//! 항목 수는 [`App::transfer_plan`]이 준다. 확인하면 [`App::start_transfer`]가 앱 전체에 하나인 작업 대기열
//! ([`crate::job::JobQueue`])에 넣고 작업 영역([`crate::work_area::WorkArea`], [`App::work_area`])에 더한다. 작업
//! 대기열의 사건은 같은 깨우기로 알리고 [`App::poll_jobs`]가 작업 영역에 반영한다.
//!
//! 주소창: 친 글자로 가기([`App::go_to_text`])는 해석한 뒤 다른 이동처럼 일꾼 스레드에서 읽는다. 자동 완성 후보
//! ([`App::complete_address`])는 폴더를 읽느라 막히므로 일꾼 스레드에서 읽고 같은 통로로 받는다.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use facet_win::{OpenError, OpenResult, ShellWorker};

use crate::command::{self, CommandId, CommandState};
use crate::details::Sort;
use crate::edit::{self, EditError, Edited};
use crate::history::{
    History, NotUndoable, OperationKind, Outcomes, Record, RecordId, Reversal, UndoError,
};
use crate::job::{
    AnswerError, ConflictAnswer, CopyRejection, DeleteRejection, ErrorAnswer, JobEvent, JobId,
    JobKind, JobQueue, JournalSink, NotRecyclableAnswer, RevertJob,
};
use crate::launch::LaunchTarget;
use crate::placement::Placement;
use crate::session::{PanelSession, Session, TabSession, WindowSession};
use crate::tab::{
    ChangeOutcome, ClosedTab, CompletionId, CompletionResult, Navigation, ReadOutcome, ReadRequest,
    ReadResult, Tab,
};
use crate::watch::ChangeBatch;
use crate::work_area::WorkArea;
use crate::{Candidate, Location, provider_for};

/// 닫은 탭 목록에 남기는 최근 탭 수. 넘으면 가장 오래된 탭이 빠진다.
pub const CLOSED_TABS_LIMIT: usize = 20;

/// 최근 대상 폴더를 이만큼까지 남긴다([`App::recent_targets`]).
pub const RECENT_TARGETS_LIMIT: usize = 10;

/// 창이 가질 수 있는 패널 수(좌우 둘).
pub const MAX_PANELS: usize = 2;

/// 분할 비율의 기본값이자 반반으로 맞출 때의 값([`Window::reset_split_ratio`]).
pub const SPLIT_RATIO_DEFAULT: f32 = 0.5;

/// 분할 비율의 하한. 왼쪽 패널이 창 너비의 이만큼보다 좁아지지 않는다.
pub const SPLIT_RATIO_MIN: f32 = 0.2;

/// 분할 비율의 상한. 오른쪽 패널이 창 너비의 `1 - SPLIT_RATIO_MAX`보다 좁아지지 않는다.
pub const SPLIT_RATIO_MAX: f32 = 0.8;

/// 앱 안의 창 하나를 가리키는 번호. 창이 닫혀도 다른 창에 다시 쓰지 않는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(u64);

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

/// 탭을 어디로 옮기는가([`App::move_tab`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabDestination {
    /// 창 `window`의 `panel`번째 패널(0이 왼쪽)의 `slot`번째 자리. 자리는 옮기기 전 그 패널의 탭 사이 틈이다:
    /// 0은 첫 탭 앞, `n`은 `n`번째 탭(0부터) 앞, 탭 수 이상이면 끝이다. 같은 패널 안이면 옮기는 탭 바로 앞뒤
    /// 자리(`t`, `t + 1`)는 순서를 바꾸지 않는다.
    Panel {
        window: WindowId,
        panel: usize,
        slot: usize,
    },
    /// 그 탭 하나로 새 창을 연다(창 밖으로 끌어 놓음).
    NewWindow,
}

/// 탭을 옮긴 결과([`App::move_tab`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Moved {
    /// 탭이 옮겨 간 창. 새 창으로 옮겼으면 그 새 창이다.
    pub window: WindowId,
    /// 떠난 패널·창이 비어 닫혔는가.
    pub closed: Closed,
}

/// facet의 최상위 화면 하나. 패널을 하나 또는 둘(좌우) 갖는다.
#[derive(Debug)]
pub struct Window {
    id: WindowId,
    panels: Vec<Panel>,
    active: usize,
    /// 분할 비율: 패널이 둘일 때 왼쪽 패널이 차지하는 몫(0..1). 합쳐도 남아 다시 나누면 그대로 쓴다.
    split_ratio: f32,
    /// 화면 자리·크기·최대화. 앱이 창을 띄우고 옮길 때마다 알린다. 아직 모르면 `None`이다.
    placement: Option<Placement>,
}

impl Window {
    fn new(id: WindowId, panel: Panel) -> Self {
        Window {
            id,
            panels: vec![panel],
            active: 0,
            split_ratio: SPLIT_RATIO_DEFAULT,
            placement: None,
        }
    }

    /// 창의 번호.
    pub fn id(&self) -> WindowId {
        self.id
    }

    /// 패널들을 차례로(왼쪽부터).
    pub fn panels(&self) -> &[Panel] {
        &self.panels
    }

    /// 패널이 둘로 나뉘어 있는가.
    pub fn is_split(&self) -> bool {
        self.panels.len() > 1
    }

    /// 활성 패널: 키보드 입력을 받는 패널.
    pub fn active_panel(&self) -> &Panel {
        &self.panels[self.active]
    }

    /// 활성 패널.
    pub fn active_panel_mut(&mut self) -> &mut Panel {
        &mut self.panels[self.active]
    }

    /// `index`번째 패널(0이 왼쪽). 없으면 `None`이다.
    pub fn panel_mut(&mut self, index: usize) -> Option<&mut Panel> {
        self.panels.get_mut(index)
    }

    /// **반대편 패널**: 패널이 둘일 때 활성 패널이 아닌 쪽. 패널이 하나면 `None`이다.
    pub fn opposite_panel(&self) -> Option<&Panel> {
        if !self.is_split() {
            return None;
        }
        self.panels.get(1 - self.active)
    }

    /// 활성 패널이 몇 번째인가(0이 왼쪽).
    pub fn active_panel_index(&self) -> usize {
        self.active
    }

    /// 분할 비율: 패널이 둘일 때 왼쪽 패널이 창 너비에서 차지하는 몫.
    pub fn split_ratio(&self) -> f32 {
        self.split_ratio
    }

    /// 분할 비율을 정한다. 어느 패널도 너무 좁아지지 않게 [`SPLIT_RATIO_MIN`]..[`SPLIT_RATIO_MAX`] 안으로
    /// 자르고, 숫자가 아니면(NaN) 바꾸지 않는다. 정해진 비율을 돌려준다.
    pub fn set_split_ratio(&mut self, ratio: f32) -> f32 {
        if !ratio.is_nan() {
            self.split_ratio = ratio.clamp(SPLIT_RATIO_MIN, SPLIT_RATIO_MAX);
        }
        self.split_ratio
    }

    /// 분할 비율을 반반([`SPLIT_RATIO_DEFAULT`])으로 맞춘다(경계를 두 번 누름).
    pub fn reset_split_ratio(&mut self) {
        self.split_ratio = SPLIT_RATIO_DEFAULT;
    }

    /// 창의 화면 자리·크기·최대화(가상 화면 기준 물리 픽셀, [`crate::placement`]). 세션에 남고, 되살린 창은 이
    /// 자리로 뜬다. 앱이 아직 알리지 않았으면(띄우기 전) `None`이다.
    pub fn placement(&self) -> Option<Placement> {
        self.placement
    }

    /// 창의 화면 자리를 정한다. 앱이 창을 옮기거나 크기를 바꾸거나 최대화할 때마다 알린다
    /// ([`Placement::observe`]로 고친 값).
    pub fn set_placement(&mut self, placement: Option<Placement>) {
        self.placement = placement;
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
    Edited {
        tab: TabId,
        result: Result<Edited, EditError>,
        /// 새 폴더 직후의 이름 입력란 확정이라 새 폴더 기록에 묶는다([`App::rename_new_folder`]).
        merge: bool,
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
    /// 이름바꾸기·새 폴더의 결과([`App::rename`]·[`App::new_folder`]). 성공했고 그 탭이 그 폴더를 보고 있으면 목록에
    /// 이미 반영했고, 바꾼(만든) 항목이 선택·커서다(새 폴더면 화면은 [`Tab::cursor`] 행에 이름 입력란을 연다).
    /// 실패면 까닭이고 아무것도 바뀌지 않았다.
    Edited(Result<Edited, EditError>),
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
    /// 화면이 할 일: 반대편 패널로 복사·이동의 확인 줄을 띄운다. 보일 원본·대상·항목 수는
    /// [`App::transfer_plan`]이 주고, 확인하면 [`App::start_transfer`]로 작업을 넣는다.
    ConfirmTransfer(JobKind),
    /// 화면이 할 일: 복사·이동의 대상 선택 창을 연다(「복사 위치…」「이동 위치…」, 패널이 하나일 때의 F7·F8).
    /// 보일 내용은 [`App::transfer_choice`]가 주고, 고른 글자는 [`App::transfer_plan_to`]로 해석해
    /// [`App::start_transfer`]에 넘긴다.
    ChooseTransferTarget(JobKind),
    /// 화면이 할 일: 커서 항목의 이름 입력란을 연다(F2). 확정하면 [`App::rename`]에 넘긴다.
    Rename,
    /// 화면이 할 일: 활성 탭 위치에 새 폴더를 만든다. 처음 이름(바탕 이름)은 화면 문장이라 화면이 정해
    /// [`App::new_folder`]에 넘긴다.
    NewFolder,
    /// 실행취소·다시 실행을 시작하지 않았다: 마지막 기록이 되돌릴 수 없는 작업이다
    /// ([`UndoError::NotUndoable`]). 화면은 그 까닭을 보인다. 그 앞 기록은 되돌리지 않았다.
    UndoRefused(UndoError),
    /// 화면이 할 일: 작업 기록([`App::history`])을 읽기 전용 목록으로 연다.
    ShowHistory,
}

/// 대상 선택 창에 보일 내용([`App::transfer_choice`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferChoice {
    /// 복사·이동.
    pub kind: JobKind,
    /// 원본 항목의 경로들: 활성 탭의 선택(목록 순서), 선택이 없으면 커서 항목 하나.
    pub sources: Vec<PathBuf>,
    /// 반대편 패널의 활성 탭 위치(패널이 둘이고 실제 폴더일 때).
    pub opposite: Option<PathBuf>,
    /// 최근 대상 폴더, 가장 최근 것부터(최대 [`RECENT_TARGETS_LIMIT`]개).
    pub recent: Vec<PathBuf>,
}

/// 대상 선택 창에 친 글자를 대상 폴더로 해석하지 못한 까닭([`App::transfer_plan_to`]).
#[derive(Debug)]
pub enum TargetError {
    /// 지금 이 복사·이동을 실행할 수 없다(원본이 없음 등).
    Unavailable,
    /// 주소창과 같은 글자 해석이 실패했다(빔·없는 위치·쓸 수 없는 글자 등).
    Resolve(crate::provider::ResolveError),
    /// 폴더가 아니라 파일이나 실제 폴더가 아닌 위치다.
    NotAFolder { text: String },
}

impl std::fmt::Display for TargetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => write!(f, "지금은 복사·이동할 수 없습니다"),
            Self::Resolve(error) => write!(f, "{error}"),
            Self::NotAFolder { text } => write!(f, "폴더가 아닙니다: {text}"),
        }
    }
}

impl std::error::Error for TargetError {}

/// 반대편 패널로 복사·이동할 내용([`App::transfer_plan`]): 확인 줄에 보일 대상과 항목 수, 확인하면 넣을 작업.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferPlan {
    /// 복사·이동.
    pub kind: JobKind,
    /// 원본 항목의 경로들: 활성 탭의 선택(목록 순서), 선택이 없으면 커서 항목 하나.
    pub sources: Vec<PathBuf>,
    /// 대상 폴더: 반대편 패널의 활성 탭 위치.
    pub target: PathBuf,
}

impl TransferPlan {
    /// 확인 줄에 보일 항목 수(넘긴 원본 수. 폴더 안은 세지 않는다).
    pub fn items(&self) -> usize {
        self.sources.len()
    }
}

/// 앱 전체의 화면 상태: 창 목록과 앱 전체에 하나인 설정(숨김 항목 보이기).
pub struct App {
    windows: Vec<Window>,
    active: usize,
    show_hidden: bool,
    next_tab: u64,
    next_window: u64,
    /// 닫은 탭 목록. 가장 최근에 닫은 탭이 끝이다. [`CLOSED_TABS_LIMIT`]개까지.
    closed: VecDeque<ClosedTab>,
    /// 최근 대상 폴더. 가장 최근 것이 앞이다. [`RECENT_TARGETS_LIMIT`]개까지.
    recent_targets: Vec<PathBuf>,
    /// 셸 일꾼 스레드. 콜드 스타트를 늦추지 않도록 처음 파일을 열 때 띄운다.
    shell: Option<ShellWorker>,
    /// 작업 대기열. 앱 전체에 하나다(일꾼 스레드는 처음 작업을 넣을 때 뜬다).
    jobs: JobQueue,
    /// 작업 영역에 보일 작업들. 모든 창이 같은 것을 그린다.
    work_area: WorkArea,
    /// 작업 기록과 다시 실행 목록. 앱 전체에 하나이고 메모리에만 있다.
    history: History,
    /// 도는 복사·이동 작업이 한 일을 적는 곳. 끝나면 작업 기록으로 옮긴다.
    journals: HashMap<JobId, (OperationKind, JournalSink)>,
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
        let mut app = App::empty();
        app.push_window(start.unwrap_or_else(home_location), Sort::default());
        app
    }

    /// 실행 인자를 해석한 탭들([`crate::launch::resolve_launch`])로 창 하나를 연 앱([`App::open_launch_window`]).
    /// 첫 실행이 쓴다.
    pub fn launch(targets: Vec<LaunchTarget>) -> Self {
        let mut app = App::empty();
        app.open_launch_window(targets);
        app
    }

    /// 세션([`Session`])을 되살린 앱. 세션이 없으면(첫 실행, 깨진 파일) [`App::launch`]처럼 `launch`로 창 하나를 연다
    /// (`launch`가 비면 사용자 홈 폴더 탭 하나).
    ///
    /// 세션이 있으면 창·패널·탭·위치·정렬·분할 비율·창마다 활성 패널·패널마다 활성 탭·마지막 활성 창을 그대로
    /// 되살린다. 기록은 비어 있다. 콜드 스타트를 늦추지 않도록 각 창의 각 패널에서 보이는(활성) 탭만 일꾼 스레드에서
    /// 읽기 시작하고, 나머지 탭은 숨은 채로 두었다가 처음 보일 때 읽는다. 위치가 사라졌으면 읽을 때(보이는 탭은
    /// 곧바로, 숨은 탭은 처음 보일 때) 가장 가까운 남은 상위로, 상위가 하나도 없으면 홈 폴더로 옮기고
    /// [`Tab::relocation`]에 원래 위치를 남긴다.
    ///
    /// 그 뒤 `launch`(첫 실행의 경로 인자)가 있으면 마지막 활성 창의 활성 패널에 탭으로 더한다
    /// ([`App::open_launch_tabs`]). 세션의 활성 번호가 범위 밖이면 끝으로 자르고, 탭이 없는 패널과 패널이 없는 창은
    /// 빼고, 패널은 [`MAX_PANELS`]개까지만 쓴다. 남는 창이 없으면 세션이 없는 것처럼 연다.
    pub fn restore(session: Option<Session>, launch: Vec<LaunchTarget>) -> Self {
        let mut app = App::empty();
        if let Some(session) = session {
            for target in session.recent_targets.iter().rev() {
                push_recent(&mut app.recent_targets, target.clone());
            }
            app.push_session(session);
        }
        if !app.has_windows() {
            app.open_launch_window(launch);
        } else {
            app.open_launch_tabs(launch);
        }
        app
    }

    /// 세션의 창들을 더하고 보이는 탭만 읽기 시작한다.
    fn push_session(&mut self, session: Session) {
        let mut active = None;
        for (w, window) in session.windows.into_iter().enumerate() {
            let mut panels = Vec::new();
            let mut active_panel = 0;
            for (p, panel) in window.panels.into_iter().enumerate() {
                if panel.tabs.is_empty() || panels.len() == MAX_PANELS {
                    continue;
                }
                if p <= window.active_panel {
                    active_panel = panels.len();
                }
                let active_tab = panel.active_tab.min(panel.tabs.len() - 1);
                let tabs = panel
                    .tabs
                    .into_iter()
                    .map(|saved| {
                        let mut tab = Tab::new(saved.location, saved.sort, self.show_hidden);
                        tab.hide();
                        (self.new_tab_id(), tab)
                    })
                    .collect();
                panels.push(Panel {
                    tabs,
                    active: active_tab,
                });
            }
            if panels.is_empty() {
                continue;
            }
            let id = WindowId(self.next_window);
            self.next_window += 1;
            let mut restored = Window::new(id, panels.remove(0));
            restored.panels.extend(panels);
            restored.active = active_panel;
            restored.set_split_ratio(window.split_ratio);
            restored.placement = window.placement;
            if w <= session.active_window {
                active = Some(self.windows.len());
            }
            self.windows.push(restored);
        }
        self.active = active.unwrap_or(0);
        let mut reads = Vec::new();
        for panel in self.windows.iter_mut().flat_map(|w| w.panels.iter_mut()) {
            let (id, tab) = &mut panel.tabs[panel.active];
            if let Some(request) = tab.show() {
                reads.push((*id, request));
            }
        }
        for (id, request) in reads {
            self.spawn_read(id, request);
        }
    }

    /// 지금 화면 상태를 세션으로([`crate::session::save`]로 저장한다). 보통 창이 하나도 없으면(앱이 끝남, 진행 창만 남음)
    /// `None`이다. 그래서 진행 창이 떠 있는 동안 저장한 세션은 마지막 보통 창 상태 그대로다.
    pub fn session(&self) -> Option<Session> {
        if !self.has_windows() {
            return None;
        }
        let windows = self
            .windows
            .iter()
            .map(|window| WindowSession {
                panels: window
                    .panels
                    .iter()
                    .map(|panel| PanelSession {
                        tabs: panel
                            .tabs
                            .iter()
                            .map(|(_, tab)| TabSession {
                                location: tab.location().clone(),
                                sort: tab.sort(),
                            })
                            .collect(),
                        active_tab: panel.active,
                    })
                    .collect(),
                active_panel: window.active,
                split_ratio: window.split_ratio,
                placement: window.placement,
            })
            .collect();
        Some(Session {
            windows,
            active_window: self.active,
            recent_targets: self.recent_targets.clone(),
        })
    }

    /// 실행 인자를 해석한 탭들을 활성 창의 활성 패널에서 활성 탭 바로 뒤에 넘긴 순서대로 더하고, 첫 탭을 활성 탭으로
    /// 한다(앞서 보이던 탭은 숨는다). 첫 탭만 읽기 시작하고 나머지는 숨은 채로 두었다가 처음 보일 때 읽는다. 해석하지
    /// 못한 인자의 탭은 읽지 않고 까닭을 보인다. 정렬은 활성 탭을 따른다. 탭이 없거나 앱이 끝났으면 아무것도 하지
    /// 않는다. 세션을 되살린 첫 실행이 경로 인자를 받았을 때 쓴다.
    pub fn open_launch_tabs(&mut self, targets: Vec<LaunchTarget>) {
        if targets.is_empty() || !self.has_windows() {
            return;
        }
        let sort = self.active_tab().sort();
        let show_hidden = self.show_hidden;
        let mut tabs = Vec::with_capacity(targets.len());
        for (index, target) in targets.into_iter().enumerate() {
            let mut tab = Tab::launched(target, sort, show_hidden);
            if index > 0 {
                tab.hide();
            }
            tabs.push((self.new_tab_id(), tab));
        }
        let panel = self.active_window_mut().active_panel_mut();
        let at = panel.active + 1;
        panel.tabs[panel.active].1.hide();
        panel.tabs.splice(at..at, tabs);
        panel.active = at;
        if self.active_tab().address_error().is_none() {
            self.navigate(Navigation::Refresh);
        }
    }

    /// 창이 없는 앱.
    fn empty() -> Self {
        let (sender, receiver) = mpsc::channel();
        App {
            windows: Vec::new(),
            active: 0,
            show_hidden: false,
            next_tab: 0,
            next_window: 0,
            closed: VecDeque::new(),
            recent_targets: Vec::new(),
            shell: None,
            jobs: JobQueue::new(),
            work_area: WorkArea::new(),
            history: History::new(),
            journals: HashMap::new(),
            outbox: Outbox {
                sender,
                waker: Arc::default(),
            },
            receiver,
        }
    }

    /// 실행 인자로 새 창을 연다(두 번째 실행이 넘긴 인자, 첫 실행의 인자). 해석한 인자마다 탭 하나를 넘긴 순서대로
    /// 패널 하나에 두고 첫 탭을 활성 탭으로, 새 창을 활성 창으로 한다. 탭이 없으면 사용자 홈 폴더 탭 하나다.
    ///
    /// 첫 탭만 일꾼 스레드에서 읽기 시작하고, 나머지 탭은 숨은 채로 두었다가 처음 보일 때 읽는다. 해석하지 못한
    /// 인자의 탭은 읽지 않고 까닭([`Tab::address_error`])을 보인다. 정렬은 활성 탭을 따르고(창이 없으면 기본 정렬)
    /// 기록은 비어 있다. 창이 하나도 없는 앱(마지막 창을 닫은 뒤)에서도 연다.
    pub fn open_launch_window(&mut self, targets: Vec<LaunchTarget>) -> WindowId {
        let sort = if !self.has_windows() {
            Sort::default()
        } else {
            self.active_tab().sort()
        };
        let mut targets = targets;
        if targets.is_empty() {
            targets.push(LaunchTarget::home());
        }
        let mut tabs = Vec::with_capacity(targets.len());
        for (index, target) in targets.into_iter().enumerate() {
            let mut tab = Tab::launched(target, sort, self.show_hidden);
            if index > 0 {
                tab.hide();
            }
            tabs.push((self.new_tab_id(), tab));
        }
        let id = WindowId(self.next_window);
        self.next_window += 1;
        self.windows
            .push(Window::new(id, Panel { tabs, active: 0 }));
        self.active = self.windows.len() - 1;
        if self.active_tab().address_error().is_none() {
            self.navigate(Navigation::Refresh);
        }
        id
    }

    /// `location`을 보일 탭 하나로 새 창을 열어 활성 창으로 하고, 그 위치를 일꾼 스레드에서 읽는다. 정렬은 활성
    /// 탭을 따르고(창이 없으면 기본 정렬) 기록은 비어 있다. 다른 창의 보이는 탭은 그대로 보인다.
    pub fn open_window(&mut self, location: Location) -> WindowId {
        let sort = if !self.has_windows() {
            Sort::default()
        } else {
            self.active_tab().sort()
        };
        self.push_window(location, sort)
    }

    fn push_window(&mut self, location: Location, sort: Sort) -> WindowId {
        let tab_id = self.new_tab_id();
        let tab = Tab::new(location, sort, self.show_hidden);
        let id = self.push_window_with(tab_id, tab);
        self.navigate(Navigation::Refresh);
        id
    }

    /// 탭 하나로 새 창을 목록 끝에 더해 활성 창으로 한다. 읽기는 시작하지 않는다.
    fn push_window_with(&mut self, tab_id: TabId, tab: Tab) -> WindowId {
        let id = WindowId(self.next_window);
        self.next_window += 1;
        self.windows.push(Window::new(id, Panel::new(tab_id, tab)));
        self.active = self.windows.len() - 1;
        id
    }

    /// 창 `id`를 닫는다(제목 줄의 닫기 단추). 그 창의 탭들을 닫은 탭 목록에 넣는다: 창에 보이던 차례(왼쪽 패널부터,
    /// 패널 안에서는 탭 차례)로 넣되 창의 활성 탭(활성 패널의 보이는 탭)은 마지막에 넣어 가장 먼저 되살아난다.
    /// 닫은 탭의 감시는 풀리고 늦게 온 결과는 버린다. 활성 창을 닫으면 그 앞 창(첫 창이었으면 새로 첫 창)이 활성이
    /// 된다. 창이 하나도 남지 않으면 앱이 끝난다([`App::is_finished`]). 없는 창이면 `false`다.
    pub fn close_window(&mut self, id: WindowId) -> bool {
        let Some(w) = self.windows.iter().position(|window| window.id == id) else {
            return false;
        };
        let window = self.remove_window(w);
        let active = (window.active, window.panels[window.active].active);
        let mut last = None;
        let mut tabs = Vec::new();
        for (p, panel) in window.panels.into_iter().enumerate() {
            for (t, (_, tab)) in panel.tabs.into_iter().enumerate() {
                if (p, t) == active {
                    last = Some(tab);
                } else {
                    tabs.push(tab);
                }
            }
        }
        tabs.extend(last);
        self.remember_closed_tabs(tabs);
        true
    }

    /// `w`번째 창을 목록에서 빼고 활성 창 번호를 맞춘다.
    fn remove_window(&mut self, w: usize) -> Window {
        let window = self.windows.remove(w);
        if self.active > w || (self.active == w && self.active > 0) {
            self.active -= 1;
        }
        window
    }

    /// 창 `id`를 활성 창으로 한다(사용자가 그 창을 눌렀다). 창의 보이는 탭들은 그대로다. 없는 창이면 `false`다.
    pub fn activate_window(&mut self, id: WindowId) -> bool {
        match self.windows.iter().position(|window| window.id == id) {
            Some(w) => {
                self.active = w;
                true
            }
            None => false,
        }
    }

    /// 창 `id`. 닫혔으면 `None`이다.
    pub fn window(&self, id: WindowId) -> Option<&Window> {
        self.windows.iter().find(|window| window.id == id)
    }

    /// 창 `id`. 닫혔으면 `None`이다.
    pub fn window_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.windows.iter_mut().find(|window| window.id == id)
    }

    /// 활성 창의 번호.
    pub fn active_window_id(&self) -> WindowId {
        self.active_window().id
    }

    /// 일꾼 스레드가 결과(읽기 결과, 묶은 변경, 열기 결과)를 보낼 때마다 부를 깨우기를 정한다. 화면은 여기서 자기
    /// 이벤트 루프를 깨워 [`App::poll`]을 부른다. 깨우기는 일꾼 스레드에서 불리므로 막지 말아야 한다.
    ///
    /// 정하기 전에 이미 온 결과로는 깨우지 않으므로, 정한 뒤 [`App::poll`]을 한 번 부른다.
    ///
    /// 작업 대기열의 사건도 같은 깨우기로 알린다. 화면은 깨어나면 [`App::poll_jobs`]도 부른다.
    pub fn set_waker(&mut self, waker: impl Fn() + Send + Sync + 'static) {
        let waker: Waker = Arc::new(waker);
        let jobs = Arc::clone(&waker);
        self.jobs.set_waker(move || jobs());
        if let Ok(mut slot) = self.outbox.waker.lock() {
            *slot = Some(waker);
        }
    }

    fn new_tab_id(&mut self) -> TabId {
        let id = TabId(self.next_tab);
        self.next_tab += 1;
        id
    }

    /// 창들을 차례로(연 차례). 마지막 창이 닫히면 비어 있다.
    pub fn windows(&self) -> &[Window] {
        &self.windows
    }

    /// 앱이 끝났는가: 프로세스를 끝낼지 정하는 한 곳이다. 창 목록과 작업 대기열을 함께 본다: 창이 하나도 남지 않고
    /// 작업 대기열에 끝나지 않은 작업(대기·실행 중·일시정지·답 기다림, [`JobQueue::has_unfinished`])도 없으면
    /// 끝난다. 창이 없어도 작업이 남아 있으면 끝나지 않고 화면은 진행 창을 띄운다([`App::shows_progress_window`]).
    /// 그 작업이 모두 끝나면(끝남·실패·취소) 끝난다.
    pub fn is_finished(&self) -> bool {
        !self.has_windows() && !self.jobs.has_unfinished()
    }

    /// 보통 창이 하나라도 있는가. 없으면 활성 창·탭을 묻지 않는다. 「새 창」 명령과 실행 인자로 창 열기
    /// ([`App::open_launch_window`])는 창이 없어도 된다.
    pub fn has_windows(&self) -> bool {
        !self.windows.is_empty()
    }

    /// 화면이 진행 창(작업 영역만 담은 작은 창)을 띄워야 하는가: 보통 창이 하나도 없는데 끝나지 않은 작업이 남아
    /// 있다. 보통 창이 다시 생기거나 작업이 모두 끝나면 `false`다. 진행 창은 코어의 창이 아니어서 창 목록·세션에
    /// 들지 않는다(세션은 마지막 보통 창 상태를 남긴다, [`App::session`]이 `None`).
    pub fn shows_progress_window(&self) -> bool {
        !self.has_windows() && self.jobs.has_unfinished()
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

    /// 탭 `id`를 닫고 닫은 탭 목록에 넣는다([`App::reopen_closed_tab`]로 되살린다). 그 탭이 패널의 활성 탭이면 오른쪽 이웃(끝이었으면 왼쪽 이웃)이 활성이 되어 다시 보인다.
    /// 패널의 마지막 탭이면 패널이 닫히고, 창의 마지막 패널이면 창이 닫힌다. 창이 하나도 남지 않으면 앱이
    /// 끝난다([`App::is_finished`]). 없는 탭이면 `None`이다.
    ///
    /// 닫은 탭의 감시는 풀리고, 그 탭에 늦게 온 결과는 버린다.
    pub fn close_tab(&mut self, id: TabId) -> Option<Closed> {
        let (w, p, t) = self.find(id)?;
        let (tab, closed) = self.take_tab(w, p, t);
        self.remember_closed(tab);
        Some(closed)
    }

    /// `w`번째 창의 `p`번째 패널에서 `t`번째 탭을 빼낸다. 그 탭이 패널의 활성 탭이면 오른쪽 이웃(끝이었으면 왼쪽
    /// 이웃)이 활성이 되어 다시 보인다. 패널에 탭이 남지 않으면 패널이, 창에 패널이 남지 않으면 창이 닫힌다.
    /// 빼낸 탭은 보이던 그대로(감시도 그대로) 돌려준다.
    fn take_tab(&mut self, w: usize, p: usize, t: usize) -> (Tab, Closed) {
        let (_, tab) = self.windows[w].panels[p].tabs.remove(t);
        let window = &mut self.windows[w];
        let panel = &mut window.panels[p];
        if panel.tabs.is_empty() {
            window.panels.remove(p);
            let window_closed = window.panels.is_empty();
            if window_closed {
                self.remove_window(w);
            } else if window.active >= p && window.active > 0 {
                window.active -= 1;
            }
            let closed = Closed {
                panel: true,
                window: window_closed,
            };
            return (tab, closed);
        }
        if t < panel.active {
            panel.active -= 1;
        } else if t == panel.active {
            panel.active = panel.active.min(panel.tabs.len() - 1);
            let (next_id, next) = &mut panel.tabs[panel.active];
            let next_id = *next_id;
            if let Some(request) = next.show() {
                self.spawn_read(next_id, request);
            }
        }
        let closed = Closed {
            panel: false,
            window: false,
        };
        (tab, closed)
    }

    /// 탭 `id`를 옮긴다: 같은 패널 안의 다른 자리(순서 바꾸기), 다른 패널(반대편 패널), 다른 창의 패널, 새 창
    /// ([`TabDestination`]). 탭 번호는 그대로이고, 위치·선택·커서·스크롤·기록·정렬·읽은 목록이 함께 따라간다.
    ///
    /// 옮긴 탭은 목적지 패널의 활성 탭이 되고, 그 패널이 활성 패널, 그 창이 활성 창이 된다. 목적지 패널에서 보이던
    /// 탭은 숨는다. 보이던 탭은 다시 읽지 않고 감시도 그대로 갖는다. 숨어 있던 탭은 갖고 있던 목록을 그대로 보이며
    /// 다시 읽어 감시를 다시 건다([`Tab::show`]). 떠난 패널에서는 [`App::close_tab`]처럼 이웃이 활성이 되고,
    /// 탭이 남지 않으면 그 패널이, 창에 탭이 남지 않으면 그 창이 닫힌다(창이 하나도 남지 않을 수는 없다: 탭이
    /// 어딘가로 간다).
    ///
    /// 없는 탭, 닫힌 창, 없는 패널이면 아무것도 하지 않고 `None`이다.
    pub fn move_tab(&mut self, id: TabId, to: TabDestination) -> Option<Moved> {
        let (w, p, t) = self.find(id)?;
        let target = match to {
            TabDestination::Panel {
                window,
                panel,
                slot,
            } => {
                let wd = self.windows.iter().position(|x| x.id == window)?;
                if panel >= self.windows[wd].panels.len() {
                    return None;
                }
                Some((wd, panel, slot))
            }
            TabDestination::NewWindow => None,
        };
        let not_closed = Closed {
            panel: false,
            window: false,
        };
        if let Some((wd, pd, slot)) = target
            && (wd, pd) == (w, p)
        {
            self.reorder_tab(w, p, t, slot);
            self.show_active_tab();
            return Some(Moved {
                window: self.windows[w].id,
                closed: not_closed,
            });
        }
        let (tab, closed) = self.take_tab(w, p, t);
        let window = match target {
            Some((mut wd, mut pd, slot)) => {
                if closed.window && wd > w {
                    wd -= 1;
                } else if closed.panel && wd == w && pd > p {
                    pd -= 1;
                }
                let window = &mut self.windows[wd];
                let panel = &mut window.panels[pd];
                panel.tabs[panel.active].1.hide();
                let at = slot.min(panel.tabs.len());
                panel.tabs.insert(at, (id, tab));
                panel.active = at;
                window.active = pd;
                self.active = wd;
                window.id
            }
            None => self.push_window_with(id, tab),
        };
        self.show_active_tab();
        Some(Moved { window, closed })
    }

    /// `w`번째 창의 `p`번째 패널에서 `t`번째 탭을 `slot` 자리로 옮겨 활성 탭으로 하고, 그 패널·창을 활성으로 한다.
    /// 앞서 보이던 탭은 숨는다.
    fn reorder_tab(&mut self, w: usize, p: usize, t: usize, slot: usize) {
        let window = &mut self.windows[w];
        let panel = &mut window.panels[p];
        let previous = panel.tabs[panel.active].0;
        let slot = slot.min(panel.tabs.len());
        let to = if slot > t { slot - 1 } else { slot };
        let entry = panel.tabs.remove(t);
        panel.tabs.insert(to, entry);
        panel.active = to;
        if let Some(index) = panel.position(previous)
            && index != to
        {
            panel.tabs[index].1.hide();
        }
        window.active = p;
        self.active = w;
    }

    /// 활성 탭이 숨어 있으면 다시 보인다(갖고 있던 목록을 두고 다시 읽어 감시를 다시 건다).
    fn show_active_tab(&mut self) {
        let id = self.active_tab_id();
        if let Some(request) = self.active_tab_mut().show() {
            self.spawn_read(id, request);
        }
    }

    /// 닫은 탭 목록: 먼저 되살아날 것(가장 최근에 넣은 것)부터.
    pub fn closed_tabs(&self) -> impl ExactSizeIterator<Item = &ClosedTab> + '_ {
        self.closed.iter().rev()
    }

    /// 탭들을 차례로 닫은 탭 목록에 넣는다. 넣은 순서대로 쌓이므로 마지막에 넣은 탭이 먼저 되살아난다. 목록이
    /// [`CLOSED_TABS_LIMIT`]개를 넘으면 가장 오래된 탭부터 빠진다. 창을 닫을 때 그 창의 탭들을 넣는 데 쓴다.
    pub fn remember_closed_tabs(&mut self, tabs: impl IntoIterator<Item = Tab>) {
        for tab in tabs {
            self.remember_closed(tab);
        }
    }

    fn remember_closed(&mut self, tab: Tab) {
        if self.closed.len() == CLOSED_TABS_LIMIT {
            self.closed.pop_front();
        }
        self.closed.push_back(tab.close());
    }

    /// 마지막에 닫은 탭을 되살린다: 위치·뒤로·앞으로 기록·정렬을 닫을 때 그대로 갖는 탭을 활성 창의 활성 패널에서
    /// 활성 탭 바로 뒤에 열어 활성으로 하고(앞서 보이던 탭은 숨는다), 그 위치를 일꾼 스레드에서 다시 읽는다.
    /// 닫은 탭이 없거나 앱이 끝났으면 `None`이다.
    pub fn reopen_closed_tab(&mut self) -> Option<TabId> {
        if !self.has_windows() {
            return None;
        }
        let closed = self.closed.pop_back()?;
        let id = self.new_tab_id();
        let tab = Tab::reopen(closed, self.show_hidden);
        let panel = self.active_window_mut().active_panel_mut();
        let at = panel.active + 1;
        panel.tabs.insert(at, (id, tab));
        panel.tabs[panel.active].1.hide();
        panel.active = at;
        self.navigate(Navigation::Refresh);
        Some(id)
    }

    /// 활성 창을 좌우 두 패널로 나눈다. 새 오른쪽 패널은 활성 탭과 같은 위치·정렬의 탭 하나(기록은 비어 있음)로
    /// 시작해 활성 패널이 되고, 그 위치를 일꾼 스레드에서 읽는다. 왼쪽 패널의 활성 탭은 그대로 보인다. 이미 둘이면
    /// ([`MAX_PANELS`]) 아무것도 하지 않고 `None`이다.
    pub fn split_panel(&mut self) -> Option<TabId> {
        if !self.has_windows() || self.active_window().panels.len() >= MAX_PANELS {
            return None;
        }
        let id = self.new_tab_id();
        let active = self.active_tab();
        let tab = Tab::new(active.location().clone(), active.sort(), self.show_hidden);
        let window = self.active_window_mut();
        window.panels.push(Panel::new(id, tab));
        window.active = window.panels.len() - 1;
        self.navigate(Navigation::Refresh);
        Some(id)
    }

    /// 활성 창의 두 패널을 하나로 합친다: 오른쪽 패널의 탭들이 왼쪽 패널 탭 끝에 순서대로 붙는다. 활성 패널의 활성
    /// 탭이 그대로 활성 탭이고, 다른 패널에서 보이던 탭은 숨는다. 분할 비율은 남는다. 패널이 하나면 `false`다.
    pub fn merge_panels(&mut self) -> bool {
        if !self.has_windows() || !self.active_window().is_split() {
            return false;
        }
        let window = self.active_window_mut();
        let right = window.panels.remove(1);
        let left = &mut window.panels[0];
        let offset = left.tabs.len();
        if window.active == 0 {
            let mut right = right;
            right.tabs[right.active].1.hide();
            left.tabs.extend(right.tabs);
        } else {
            left.tabs[left.active].1.hide();
            left.active = offset + right.active;
            left.tabs.extend(right.tabs);
        }
        window.active = 0;
        true
    }

    /// 활성 창의 `index`번째 패널(0이 왼쪽)을 활성 패널로 한다. 두 패널의 보이는 탭은 그대로 보인다. 없는 패널이면
    /// `false`다.
    pub fn activate_panel(&mut self, index: usize) -> bool {
        if !self.has_windows() || index >= self.active_window().panels.len() {
            return false;
        }
        self.active_window_mut().active = index;
        true
    }

    /// 활성 패널을 반대편 패널로 바꾼다(Tab). 패널이 하나면 아무것도 하지 않고 `false`다.
    pub fn switch_panel(&mut self) -> bool {
        if !self.has_windows() || !self.active_window().is_split() {
            return false;
        }
        let window = self.active_window_mut();
        window.active = (window.active + 1) % window.panels.len();
        true
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
            Message::Edited {
                tab: id,
                result,
                merge,
            } => {
                if let Ok(edited) = &result {
                    self.record_edit(edited, merge);
                    if let Some(tab) = self.tab_mut(id) {
                        tab.apply_edit(edited);
                    }
                }
                (id, Update::Edited(result))
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

    /// 명령을 활성 창·패널·탭에서 지금 실행할 수 있는가([`CommandId::is_enabled_in_window`]). 앱이 끝났으면 아무 명령도
    /// 실행할 수 없다.
    pub fn can_execute(&self, command: CommandId) -> bool {
        if command == CommandId::NewWindow {
            return true;
        }
        if !self.has_windows() {
            return false;
        }
        match command {
            CommandId::ReopenClosedTab => !self.closed.is_empty(),
            CommandId::Undo => self.history.undo_target().is_some(),
            CommandId::Redo => self.history.redo_target().is_some(),
            CommandId::ShowHistory => !self.history.is_empty(),
            command => command.is_enabled_in_window(self.active_window()),
        }
    }

    /// 명령 팔레트 검색: 앱이 주는 명령 이름에서 찾고, 실행 가능 여부는 [`App::can_execute`]를 따른다
    /// ([`command::search`]).
    pub fn search_commands<S: AsRef<str>>(
        &self,
        query: &str,
        name: impl Fn(CommandId) -> S,
    ) -> Vec<command::PaletteEntry> {
        command::search(query, name, self)
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
            CommandId::ReopenClosedTab => {
                return match self.reopen_closed_tab() {
                    Some(_) => Execution::Done,
                    None => Execution::Disabled,
                };
            }
            CommandId::TogglePanelSplit => {
                if self.active_window().is_split() {
                    self.merge_panels();
                } else {
                    self.split_panel();
                }
                return Execution::Done;
            }
            CommandId::CopyToOtherPanel | CommandId::MoveToOtherPanel => {
                let kind = command.transfer_kind().expect("복사·이동 명령이다");
                return if self.active_window().is_split() {
                    Execution::ConfirmTransfer(kind)
                } else {
                    Execution::ChooseTransferTarget(kind)
                };
            }
            CommandId::CopyTo | CommandId::MoveTo => {
                let kind = command.transfer_kind().expect("복사·이동 명령이다");
                return Execution::ChooseTransferTarget(kind);
            }
            CommandId::Rename => return Execution::Rename,
            CommandId::NewFolder => return Execution::NewFolder,
            CommandId::Undo | CommandId::Redo => {
                let result = if command == CommandId::Undo {
                    self.undo()
                } else {
                    self.redo()
                };
                return match result {
                    Ok(_) => Execution::Done,
                    Err(UndoError::Nothing | UndoError::Busy) => Execution::Disabled,
                    Err(error) => Execution::UndoRefused(error),
                };
            }
            CommandId::ShowHistory => return Execution::ShowHistory,
            CommandId::NewWindow => {
                let location = if !self.has_windows() {
                    home_location()
                } else {
                    self.active_tab().location().clone()
                };
                self.open_window(location);
                return Execution::Done;
            }
            CommandId::SwitchPanel => {
                return if self.switch_panel() {
                    Execution::Done
                } else {
                    Execution::Disabled
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

    /// 활성 탭의 항목 `original`(화면이 넘긴 이름 입력란의 항목, 선택이 여럿이면 커서 항목)의 이름을 `new_name`으로
    /// 바꾼다. 작업 대기열을 거치지 않고 일꾼 스레드에서 곧바로 바꾸며, 결과는 [`Update::Edited`]로 온다
    /// ([`App::poll`]). 막지 않는다.
    ///
    /// 실행 전에 막는 것은 일꾼 스레드를 띄우지 않고 곧바로 까닭을 돌려준다: 위치가 실제 폴더가 아니거나 항목이
    /// 목록에 없음([`EditError::Unavailable`]), 원래 이름과 똑같음([`EditError::Unchanged`]), 쓸 수 없는 이름
    /// ([`EditError::InvalidName`], [`crate::name_input::validate_name`]), 같은 폴더에 같은 이름(대소문자 무시, 숨김
    /// 항목 포함)의 다른 항목([`EditError::NameTaken`]). 대소문자만 다른 이름은 같은 항목의 이름바꾸기라 막지 않고
    /// 디스크에서도 대소문자가 바뀐다. 실행 중의 실패(권한 없음, 그 사이 같은 이름 생김, 원본 사라짐)는 결과의
    /// 까닭이다.
    ///
    /// 바꾼 뒤 목록이 다시 정렬돼도 새 이름의 항목이 선택·커서로 남는다(결과를 받을 때 곧바로 반영,
    /// [`Update::Edited`]).
    pub fn rename(&mut self, original: &std::ffi::OsStr, new_name: &str) -> Result<(), EditError> {
        self.rename_with(original, new_name, false)
    }

    /// 새 폴더([`App::new_folder`]) 직후 열린 이름 입력란의 확정: [`App::rename`]과 같되, 끝나면 그 이름바꾸기를
    /// 새 폴더 기록에 묶어 작업 기록에는 「새 폴더」 작업 하나로 남는다(실행취소하면 바꾼 이름의 폴더가 비어 있으면
    /// 지워진다). 그 사이 다른 작업이 기록됐으면 묶지 못하고 이름바꾸기로 따로 남는다.
    pub fn rename_new_folder(
        &mut self,
        original: &std::ffi::OsStr,
        new_name: &str,
    ) -> Result<(), EditError> {
        self.rename_with(original, new_name, true)
    }

    fn rename_with(
        &mut self,
        original: &std::ffi::OsStr,
        new_name: &str,
        merge: bool,
    ) -> Result<(), EditError> {
        if !self.has_windows() {
            return Err(EditError::Unavailable);
        }
        let tab = self.active_tab();
        let folder = tab
            .location()
            .file_system_path()
            .ok_or(EditError::Unavailable)?
            .to_path_buf();
        if !tab.all_names().any(|name| name == original) {
            return Err(EditError::Unavailable);
        }
        if original == std::ffi::OsStr::new(new_name) {
            return Err(EditError::Unchanged);
        }
        crate::name_input::validate_name(new_name).map_err(EditError::InvalidName)?;
        if edit::name_taken(tab.all_names(), original, new_name) {
            return Err(EditError::NameTaken);
        }
        let original = original.to_os_string();
        let new_name = new_name.to_string();
        self.spawn_edit("facet-rename", merge, move || {
            edit::rename(&folder, &original, &new_name)
        });
        Ok(())
    }

    /// 활성 탭 위치에 새 폴더를 만든다. 이름은 화면이 넘긴 바탕 이름 `base`(한국어는 「새 폴더」)이고, 있으면
    /// 「`base` (2)」, 「(3)」 …이다([`edit::new_folder`]). 일꾼 스레드에서 곧바로 만들며(작업 대기열을 거치지 않음),
    /// 결과는 [`Update::Edited`]로 온다. 받을 때 만든 폴더가 목록에 들어가 선택·커서가 되므로 화면은 그 행에 이름
    /// 입력란을 연다. 위치가 실제 폴더가 아니면 [`EditError::Unavailable`], 바탕 이름을 쓸 수 없으면
    /// [`EditError::InvalidName`]이다.
    pub fn new_folder(&mut self, base: &str) -> Result<(), EditError> {
        if !self.can_execute(CommandId::NewFolder) {
            return Err(EditError::Unavailable);
        }
        crate::name_input::validate_name(base).map_err(EditError::InvalidName)?;
        let folder = self
            .active_tab()
            .location()
            .file_system_path()
            .ok_or(EditError::Unavailable)?
            .to_path_buf();
        let base = base.to_string();
        self.spawn_edit("facet-new-folder", false, move || {
            edit::new_folder(&folder, &base)
        });
        Ok(())
    }

    /// 이름바꾸기·새 폴더 하나를 새 일꾼 스레드에서 하고 결과를 활성 탭 앞으로 보낸다.
    fn spawn_edit(
        &self,
        name: &str,
        merge: bool,
        run: impl FnOnce() -> Result<Edited, EditError> + Send + 'static,
    ) {
        let tab = self.active_tab_id();
        let outbox = self.outbox.clone();
        thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let result = run();
                outbox.send(Message::Edited { tab, result, merge });
            })
            .expect("이름바꾸기·새 폴더 일꾼 스레드를 만들지 못했다");
    }

    /// 반대편 패널로 복사·이동할 내용: 원본은 활성 창의 활성 탭의 선택(목록 순서, 없으면 커서 항목 하나), 대상은
    /// 반대편 패널의 활성 탭 위치다. 그 명령을 지금 실행할 수 없으면(원본이 없음, 패널이 하나, 실제 폴더가 아닌
    /// 위치) `None`이다. 확인 줄은 이것을 보이고, 확인하면 [`App::start_transfer`]에 넘긴다.
    pub fn transfer_plan(&self, kind: JobKind) -> Option<TransferPlan> {
        let sources = self.transfer_sources(kind)?;
        let target = self
            .active_window()
            .opposite_panel()?
            .active_tab()
            .location()
            .file_system_path()?
            .to_path_buf();
        Some(TransferPlan {
            kind,
            sources,
            target,
        })
    }

    /// 복사·이동할 원본: 활성 탭의 선택(목록 순서), 없으면 커서 항목 하나. 그 작업을 지금 실행할 수 없으면 `None`.
    fn transfer_sources(&self, kind: JobKind) -> Option<Vec<PathBuf>> {
        let command = match kind {
            JobKind::Copy => CommandId::CopyTo,
            JobKind::Move => CommandId::MoveTo,
            // 실행취소·다시 실행은 원본을 골라 넣는 작업이 아니다.
            JobKind::Undo | JobKind::Redo | JobKind::Delete | JobKind::Recycle => return None,
        };
        if !self.can_execute(command) {
            return None;
        }
        let tab = self.active_window().active_panel().active_tab();
        let folder = tab.location().file_system_path()?;
        let mut sources: Vec<PathBuf> = tab
            .selected_items()
            .map(|item| folder.join(&item.name))
            .collect();
        if sources.is_empty() {
            sources.push(folder.join(&tab.cursor_item()?.name));
        }
        Some(sources)
    }

    /// 대상 선택 창에 보일 내용. 그 복사·이동을 지금 실행할 수 없으면 `None`이다.
    pub fn transfer_choice(&self, kind: JobKind) -> Option<TransferChoice> {
        let sources = self.transfer_sources(kind)?;
        let opposite = self
            .active_window()
            .opposite_panel()
            .and_then(|panel| panel.active_tab().location().file_system_path())
            .map(PathBuf::from);
        Some(TransferChoice {
            kind,
            sources,
            opposite,
            recent: self.recent_targets.clone(),
        })
    }

    /// 대상 선택 창에서 고른 글자를 대상 폴더로 해석해 [`App::start_transfer`]에 넘길 내용을 만든다. 주소창과 같은
    /// 글자 해석([`crate::Provider::resolve`])을 쓰고 파일 시스템에 한 번 묻는다. 해석하지 못했거나 폴더가 아니면
    /// 까닭을 돌려주고 아무것도 바꾸지 않는다.
    pub fn transfer_plan_to(&self, kind: JobKind, text: &str) -> Result<TransferPlan, TargetError> {
        let sources = self
            .transfer_sources(kind)
            .ok_or(TargetError::Unavailable)?;
        let resolved = provider_for(self.active_tab().location())
            .resolve(text)
            .map_err(TargetError::Resolve)?;
        let not_a_folder = || TargetError::NotAFolder {
            text: text.trim().to_string(),
        };
        if resolved.select.is_some() {
            return Err(not_a_folder());
        }
        let target = resolved
            .location
            .file_system_path()
            .ok_or_else(not_a_folder)?
            .to_path_buf();
        Ok(TransferPlan {
            kind,
            sources,
            target,
        })
    }

    /// 최근 대상 폴더: 실제로 작업을 시작한 대상, 가장 최근 것부터 [`RECENT_TARGETS_LIMIT`]개까지. 같은 폴더는 한
    /// 번만 있고 다시 쓰면 앞으로 온다. 앱 전체에 하나이고 세션 파일에 남는다.
    pub fn recent_targets(&self) -> &[PathBuf] {
        &self.recent_targets
    }

    /// 확인한 복사·이동을 작업 대기열에 넣고 작업 영역에 더한다. 시작 전에 막힌 경우(자기 하위로 복사, 대상 위치
    /// 없음 등)는 작업을 만들지 않고 까닭([`CopyRejection`])을 돌려준다.
    ///
    /// 작업이 끝나 원본·대상 폴더가 바뀌면 그 위치를 보는 탭은 변경 감시로 갱신된다(따로 다시 읽지 않는다).
    pub fn start_transfer(&mut self, plan: &TransferPlan) -> Result<JobId, CopyRejection> {
        let (id, journal) =
            self.jobs
                .submit_recorded(plan.kind, plan.sources.clone(), plan.target.clone())?;
        let kind = if plan.kind == JobKind::Move {
            OperationKind::Move
        } else {
            OperationKind::Copy
        };
        self.journals.insert(id, (kind, journal));
        push_recent(&mut self.recent_targets, plan.target.clone());
        self.work_area
            .add(id, plan.kind, plan.target.clone(), plan.sources.len());
        // 넣자마자 온 대기 사건을 바로 반영한다.
        self.poll_jobs();
        Ok(id)
    }

    /// 고른 항목들(`sources`)을 **영구 삭제**하는 작업을 작업 대기열에 넣어 곧바로 시작하고(차례를 기다리지 않는다,
    /// [`JobQueue::delete_permanently`]) 작업 영역에 더한다([`JobKind::Delete`], 대상 자리에는 지우는 위치: 첫 원본이
    /// 든 폴더). 원본이 없으면 작업을 만들지 않고 까닭([`DeleteRejection`])을 돌려준다.
    ///
    /// 휴지통을 거치지 않으므로 되돌릴 수 없다. 실행 전에 그 사실을 알리고 확인받는 것은 화면 몫이다(이것은 확인을
    /// 마친 뒤 부른다). 진행·항목별 오류 질문·취소·끝남은 다른 작업과 같고 [`App::poll_jobs`]가 받는다.
    ///
    /// 하나라도 지웠으면 끝날 때(끝남·실패·취소 모두) 작업 기록에 **되돌릴 수 없는 작업**(까닭:
    /// [`NotUndoable::PermanentDelete`])으로 남는다. 그 뒤 실행취소는 까닭을 돌려주고 그 앞 작업을 되돌리지 않는다
    /// ([`App::undo`]). 지운 위치를 보는 탭은 변경 감시로 갱신된다.
    pub fn delete_permanently(&mut self, sources: Vec<PathBuf>) -> Result<JobId, DeleteRejection> {
        let location = first_parent(&sources);
        let count = sources.len();
        let (id, journal) = self.jobs.delete_permanently_recorded(sources)?;
        self.journals.insert(id, (OperationKind::Delete, journal));
        self.work_area.add(id, JobKind::Delete, location, count);
        // 넣자마자 온 대기·실행 사건을 바로 반영한다.
        self.poll_jobs();
        Ok(id)
    }

    /// 고른 항목들(`sources`)을 **휴지통 삭제**하는 작업을 작업 대기열에 넣어 곧바로 시작하고([`JobQueue::recycle`])
    /// 작업 영역에 더한다([`JobKind::Recycle`], 대상 자리에는 지우는 위치: 첫 원본이 든 폴더). 원본이 없으면 작업을
    /// 만들지 않고 까닭([`DeleteRejection`])을 돌려준다. 진행·질문(항목별 오류, 휴지통에 들어가지 않는 항목
    /// [`App::answer_not_recyclable`])·취소·끝남은 다른 작업과 같고 [`App::poll_jobs`]가 받는다.
    ///
    /// 끝날 때(끝남·실패·취소 모두) 한 것이 있으면 작업 기록에 남는다([`OperationKind::Recycle`], 휴지통에 보낸
    /// 항목마다 원래 경로와 휴지통 항목). 실행취소는 휴지통에서 복원하고 다시 실행은 다시 휴지통으로 보낸다. 휴지통에
    /// 들어가지 않아 영구 삭제로 진행한 항목이 섞였으면 그 항목만 되돌릴 수 없고(실행취소가 알린다) 기록은 되돌릴 수
    /// 있다: 휴지통에 간 항목은 반드시 되돌아가야 하므로 기록 전체를 되돌릴 수 없다고 두지 않는다. 휴지통에 보낸 것
    /// 없이 모두 영구 삭제로 진행했으면 영구 삭제와 같은 되돌릴 수 없는 기록이다.
    pub fn recycle(&mut self, sources: Vec<PathBuf>) -> Result<JobId, DeleteRejection> {
        let location = first_parent(&sources);
        let count = sources.len();
        let (id, journal) = self.jobs.recycle_recorded(sources)?;
        self.journals.insert(id, (OperationKind::Recycle, journal));
        self.work_area.add(id, JobKind::Recycle, location, count);
        self.poll_jobs();
        Ok(id)
    }

    /// 고른 항목들을 **삭제**한다(Delete): 모두 휴지통이 없는 위치에 있으면([`crate::job::deletes_permanently`])
    /// 영구 삭제 작업([`App::delete_permanently`]), 아니면 휴지통 삭제 작업([`App::recycle`])이다. 영구 삭제가 될
    /// 때 실행 전에 「영구 삭제됩니다」를 알리고 확인받는 것은 화면 몫이다(같은 함수로 미리 안다).
    pub fn delete(&mut self, sources: Vec<PathBuf>) -> Result<JobId, DeleteRejection> {
        if crate::job::deletes_permanently(&sources) {
            self.delete_permanently(sources)
        } else {
            self.recycle(sources)
        }
    }

    /// 작업 대기열에서 온 사건을 모두 작업 영역에 반영한다. 무엇이든 바뀌었으면 `true`다(화면이 다시 그린다).
    pub fn poll_jobs(&mut self) -> bool {
        let mut changed = false;
        for event in self.jobs.poll() {
            if let JobEvent::Finished { job, summary } = &event {
                self.history.finish(*job);
                self.record_transfer(*job, summary);
            }
            changed |= self.work_area.apply(&event);
        }
        changed
    }

    /// 끝난 복사·이동·영구 삭제(끝남·실패·취소 모두)를 작업 기록에 남긴다: 실제로 한 항목만 담고, 한 항목이 없으면
    /// 남기지 않는다. 영구 삭제는 하나라도 지웠으면 되돌릴 수 없는 작업([`NotUndoable::PermanentDelete`])으로 남긴다. 휴지통을 거치지 않고 영구 덮어쓴 파일이 있으면 되돌릴 수 없다고 남긴다(그 파일은 되살릴 수 없으므로).
    /// 휴지통으로 보내고 덮어쓴 파일은 되돌릴 수 있다(옛 파일을 휴지통에서 복원).
    fn record_transfer(&mut self, job: JobId, summary: &crate::job::Summary) {
        let Some((kind, journal)) = self.journals.remove(&job) else {
            return;
        };
        let journal = std::mem::take(&mut *journal.lock().unwrap_or_else(|e| e.into_inner()));
        if journal.steps.is_empty() && !journal.overwrote {
            return;
        }
        let all_deleted = kind == OperationKind::Recycle
            && !journal
                .steps
                .iter()
                .any(|step| matches!(step, crate::history::Step::Recycled { .. }));
        let record = if kind == OperationKind::Delete || all_deleted {
            // 영구 삭제(휴지통에 보낸 것 없이 모두 영구 삭제로 진행한 휴지통 삭제 포함): 되살릴 것이 남지 않는다.
            // 항목별 결과는 넘긴 원본뿐이라 지운 항목 수를 따로 준다.
            Record::not_undoable(
                OperationKind::Delete,
                journal.steps,
                NotUndoable::PermanentDelete,
            )
            .with_completed(summary.items_deleted)
        } else if journal.overwrote {
            Record::not_undoable(kind, journal.steps, NotUndoable::Overwritten)
        } else {
            Record::new(kind, journal.steps)
        };
        self.history
            .record(record.with_skipped(summary.items_skipped()));
    }

    /// 끝난 이름바꾸기·새 폴더를 작업 기록에 남긴다.
    fn record_edit(&mut self, edited: &Edited, merge: bool) {
        if merge
            && let Some(from) = &edited.original
            && self.history.merge_new_folder_rename(
                from,
                &edited.path,
                crate::history::Stamp {
                    is_folder: edited.is_folder,
                    size: edited.size,
                    modified: edited.modified,
                },
            )
        {
            return;
        }
        self.history.record(Record::from_edit(edited));
    }

    /// 작업 기록: 끝난 작업의 기록과 다시 실행 목록. 앱 전체에 하나이고 메모리에만 있다(디스크에 남기지 않는다).
    pub fn history(&self) -> &History {
        &self.history
    }

    /// 끝난 작업을 작업 기록에 남긴다(다시 실행 목록을 비운다). 이름바꾸기·새 폴더는 앱이 결과를 받을 때 스스로
    /// 남기고, 뒤에 들어올 작업(삭제·복사·이동 등)과 시험이 이것으로 남긴다.
    pub fn record_operation(&mut self, record: Record) -> RecordId {
        self.history.record(record)
    }

    /// **실행취소**: 작업 기록의 마지막 기록을 되돌리는 작업을 작업 대기열에 넣고 곧바로 시작한다
    /// ([`JobKind::Undo`], 작업 영역에도 더한다). 진행·항목별 오류 질문·취소·끝남은 다른 작업과 같고
    /// [`App::poll_jobs`]가 받는다.
    ///
    /// 되돌리는 방법: 이름바꾸기는 되바꾸고, 새 폴더는 비어 있으면 지운다(묶인 새 폴더+이름바꾸기도 그 폴더를).
    /// 복사는 만든 항목을 휴지통으로 보내고([`crate::job::Summary::recycled`]), 이동은 대상의 항목을 원래 자리로
    /// 되옮긴다(이동이 만든 대상 폴더는 비면 지운다). 밖에서 바뀐 항목(없음·바뀜·그 자리에 같은 이름·폴더가 비어 있지
    /// 않음·휴지통에 넣을 수 없음)은 건너뛰고 까닭을 결과([`crate::job::Summary::not_reverted`])에 담는다. 아무것도
    /// 덮어쓰거나 영구히 지우지 않는다.
    ///
    /// 끝나면 되돌린 항목은 다시 실행 목록으로 가고, 되돌리지 못한 항목은 기록에 남는다. 시작하지 않으면 까닭:
    /// 기록이 없음([`UndoError::Nothing`]), 다른 실행취소·다시 실행이 도는 중([`UndoError::Busy`]), 마지막 기록이
    /// 되돌릴 수 없는 작업([`UndoError::NotUndoable`], 그 앞 기록으로 넘어가지 않는다).
    pub fn undo(&mut self) -> Result<JobId, UndoError> {
        self.start_reversal(Reversal::Undo)
    }

    /// **다시 실행**: 다시 실행 목록의 마지막(가장 최근에 실행취소한 작업)을 다시 하는 작업을 넣고 곧바로 시작한다
    /// ([`JobKind::Redo`]). 밖의 변경은 실행취소와 같은 규칙으로 건너뛴다. 끝나면 다시 한 항목은 작업 기록으로
    /// 돌아간다. 새 작업이 기록되면 다시 실행 목록은 빈다.
    pub fn redo(&mut self) -> Result<JobId, UndoError> {
        self.start_reversal(Reversal::Redo)
    }

    fn start_reversal(&mut self, reversal: Reversal) -> Result<JobId, UndoError> {
        let record = self.history.check(reversal)?;
        let steps = History::steps_for(record, reversal);
        let target = steps
            .first()
            .and_then(|step| step.path().parent())
            .map(PathBuf::from)
            .unwrap_or_default();
        let count = steps.len();
        let outcomes = Outcomes::default();
        let job = self
            .jobs
            .revert(RevertJob::new(reversal, steps, Arc::clone(&outcomes)));
        self.history.begin(reversal, job, outcomes);
        let kind = match reversal {
            Reversal::Undo => JobKind::Undo,
            Reversal::Redo => JobKind::Redo,
        };
        self.work_area.add(job, kind, target, count);
        self.poll_jobs();
        Ok(job)
    }

    /// 작업 영역: 넣은 작업들의 진행·상태·결과. 앱 전체에 하나라 모든 창에 같은 것이 보인다.
    pub fn work_area(&self) -> &WorkArea {
        &self.work_area
    }

    /// 끝난 작업을 작업 영역에서 닫는다. 없거나 끝나지 않은 작업이면 `false`다.
    pub fn dismiss_job(&mut self, job: JobId) -> bool {
        self.work_area.dismiss(job)
    }

    /// 이름 충돌 질문([`crate::job::JobQuestion::Conflict`])에 답한다. 받으면 작업 영역의 질문을 곧바로 지운다(모든
    /// 창이 같은 작업 영역을 그리므로 어느 창에서 답해도 모든 창에서 사라진다).
    ///
    /// 한 질문에는 한 번만 답한다: 이미 답했거나(다른 창에서 먼저 답함) 그 종류의 질문을 기다리지 않으면
    /// [`AnswerError::NotAsking`]이고 아무 일도 없다. 덮어쓸 수 없는 충돌에 덮어쓰기 답이면
    /// [`AnswerError::OverwriteNotAllowed`]이고 질문은 남는다.
    pub fn answer_conflict(
        &mut self,
        job: JobId,
        answer: ConflictAnswer,
    ) -> Result<(), AnswerError> {
        self.jobs.answer_conflict(job, answer)?;
        self.work_area.answered(job);
        self.poll_jobs();
        Ok(())
    }

    /// 항목별 오류 질문([`crate::job::JobQuestion::Error`])에 답한다. 받으면 작업 영역의 질문을 곧바로 지운다. 한
    /// 질문에는 한 번만 답한다([`App::answer_conflict`]와 같다).
    pub fn answer_error(&mut self, job: JobId, answer: ErrorAnswer) -> Result<(), AnswerError> {
        self.jobs.answer_error(job, answer)?;
        self.work_area.answered(job);
        self.poll_jobs();
        Ok(())
    }

    /// 휴지통에 들어가지 않는 항목의 질문([`crate::job::JobQuestion::NotRecyclable`])에 답한다(영구 삭제로 진행·
    /// 건너뛰기·작업 취소). 받으면 작업 영역의 질문을 곧바로 지운다. 한 질문에는 한 번만 답한다
    /// ([`App::answer_conflict`]와 같다).
    pub fn answer_not_recyclable(
        &mut self,
        job: JobId,
        answer: NotRecyclableAnswer,
    ) -> Result<(), AnswerError> {
        self.jobs.answer_not_recyclable(job, answer)?;
        self.work_area.answered(job);
        self.poll_jobs();
        Ok(())
    }

    /// 작업 대기열(일시정지·이어하기·취소·지금 시작). 사건은 [`App::poll_jobs`]로 받는다: 여기서
    /// [`JobQueue::poll`]을 부르면 작업 영역에 반영되지 않는다.
    pub fn jobs_mut(&mut self) -> &mut JobQueue {
        &mut self.jobs
    }

    /// 작업 대기열.
    pub fn jobs(&self) -> &JobQueue {
        &self.jobs
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

impl CommandState for App {
    fn is_enabled(&self, command: CommandId) -> bool {
        self.can_execute(command)
    }
}

/// 삭제 작업의 작업 영역 대상 자리: 첫 원본이 든 폴더.
fn first_parent(sources: &[PathBuf]) -> PathBuf {
    sources
        .first()
        .and_then(|source| source.parent())
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// `target`을 최근 대상의 맨 앞에 둔다. 같은 폴더(대소문자 무시)가 이미 있으면 그것을 앞으로 옮기고, 넘치면 가장
/// 오래된 것을 뺀다.
fn push_recent(recent: &mut Vec<PathBuf>, target: PathBuf) {
    let key = |path: &PathBuf| path.to_string_lossy().to_lowercase();
    let wanted = key(&target);
    recent.retain(|path| key(path) != wanted);
    recent.insert(0, target);
    recent.truncate(RECENT_TARGETS_LIMIT);
}

/// 사용자 홈 폴더. 알 수 없으면 현재 폴더다.
pub fn home_location() -> Location {
    let path = std::env::home_dir()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    Location::file_system(path)
}
