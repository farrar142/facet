//! facet 앱. 코어를 부르기만 하는 얇은 껍질이다.
//!
//! 코어의 「창 > 패널 > 탭」 상태([`App`])를 자세히 보기로 그린다. 사용자 입력(키, 클릭, 마우스 버튼)은 코어의
//! 명령이나 탭 동작으로 바꿔 넘기기만 하고, 정렬·선택·이동의 판단은 모두 코어가 한다.
//!
//! - 목록 읽기는 코어가 일꾼 스레드에서 한다. 결과가 오면 코어가 깨우기([`App::set_waker`])를 부르고, 앱은
//!   이벤트 루프에서 [`App::poll`]로 받아 다시 그린다.
//! - 행 문자열은 화면에 보이는 행만 그때그때 만든다([`TabRows`]).
//! - 시작하면 세션 파일(`%APPDATA%\facet\session.json`, [`facet_core::session`])을 읽어 창·패널·탭을 되살린다
//!   ([`App::restore`]). 파일이 없으면 기본 창으로, 깨졌으면 기본 창으로 뜨고 첫 창 위에 그 사실과 남긴 파일 자리를
//!   보인다. 상태가 바뀌면 코어의 [`Saver`]에 넘기고(디스크는 일꾼 스레드가 2초를 묶어 쓴다), 끝낼 때 한 번 더 쓴다.
//!   측정 모드는 `FACET_SESSION_PATH`가 없으면 세션 파일을 읽지도 쓰지도 않는다.
//!   창을 옮기거나 크기를 바꾸거나 최대화하면 그 자리를 코어 창에 알려 세션에 남기고, 되살린 창은 그 자리·크기로
//!   띄워 최대화를 되살린다. 놓였던 모니터가 없으면 주 모니터 안으로 옮긴다([`facet_core::placement::fit`]).
//! - 실행 인자로 받은 경로마다 탭 하나를 연다: 되살린 세션이 있으면 마지막 활성 창의 활성 패널에 더하고, 없으면 그
//!   탭들로(없으면 사용자 홈 폴더) 창을 연다. 해석은 코어가 한다
//!   ([`facet_core::launch::resolve_launch`], 상대 경로는 작업 폴더 기준).
//! - 단일 실행([`instance`]): 첫 프로세스가 주인이 되고, 다시 실행하면 인자와 작업 폴더를 주인에게 넘기고 끝난다.
//!   주인은 받은 인자로 새 창을 열어 앞으로 가져온다([`App::open_launch_window`]). 측정 모드는 끼지 않는다.
//! - 패널 위에 탭 줄을 그린다. 탭 연산(열기·닫기·오가기·옮기기)은 코어가 하고, 앱은 탭을 누름·휠 클릭·끌기를
//!   넘긴다. 탭을 끌면(끌기 시작 문턱과 놓일 자리는 [`tab_drag`]) 같은 패널 안 순서를 바꾸거나 반대편 패널·다른 창의
//!   탭 줄로 옮기고, facet의 어느 창 위도 아닌 곳에 놓으면 그 탭으로 새 창을 놓은 자리에 띄운다([`App::move_tab`]).
//!   창 사이 끌기는 facet이 직접 한다(OLE 드래그 앤 드롭을 쓰지 않는다): 누른 창이 마우스를 붙잡고 있어 창 밖의
//!   점도 그 창 기준으로 오고, 이를 화면 점으로 바꿔 그 점 아래 창을 Windows에게 묻는다
//!   (`facet_win::top_level_window_at`).
//! - 창이 나뉘었으면 두 패널을 나란히 그린다. 패널마다 탭 줄·주소창·자세히 보기·상태 표시줄을 갖고, 패널을 누르면
//!   활성 패널이 바뀐다. 키 입력과 명령은 활성 패널의 활성 탭으로 간다. 패널 나누기·합치기·분할 비율은 코어가 한다.
//! - 코어의 창마다 Slint 창을 하나씩 그린다(한 프로세스, 한 이벤트 루프). 창에서 일어난 입력은 먼저 그 창을 코어의
//!   활성 창으로 알린 뒤 넘기므로, 어느 창에서 친 키든 그 창의 활성 패널의 활성 탭으로 간다. 명령으로 창이 생기거나
//!   닫히면 코어의 창 목록에 화면을 맞추고(`sync_windows`), 창이 남지 않고 끝나지 않은 작업도 없으면
//!   ([`App::is_finished`]) 이벤트 루프를 끝낸다. 제목 줄의 닫기 단추는 코어의 창 닫기([`App::close_window`])다.
//!   깨우기 하나로 모든 창을 갱신한다.
//! - 진행 창: 보통 창을 모두 닫았는데 작업이 남아 있으면([`App::shows_progress_window`]) 작업 영역만 담은 작은 창
//!   (`ProgressWindow`)을 띄운다. 거기서도 단추와 질문 대화에 답하고, 「새 창」·Ctrl+N이나 다시 실행으로 보통 창이
//!   생기면 닫는다. 작업이 모두 끝나면 닫고 앱을 끝낸다. 사용자가 닫으려 하면 닫지 않고 까닭을 보인다.
//! - 반대편 패널로 복사·이동(F7·F8): 코어가 확인 줄을 띄우라고 하면([`Execution::ConfirmTransfer`]) 창 아래에 대상과
//!   항목 수([`App::transfer_plan`])를 보이고, Enter면 작업 대기열에 넣는다([`App::start_transfer`], 막히면 까닭을
//!   보인다). Esc면 아무것도 하지 않는다.
//! - 복사·이동 대상 선택 창(「복사 위치…」「이동 위치…」, 패널이 하나일 때의 F7·F8): 코어가
//!   [`Execution::ChooseTransferTarget`]을 주면 창 위에 경로 입력란·반대편 패널 위치·최근 대상을 보인다
//!   ([`App::transfer_choice`], 상태는 `src/target.rs`). 글자는 [`App::transfer_plan_to`]가 주소창과 같은 규칙으로
//!   해석하고, Enter가 확인이라 곧바로 작업 대기열에 넣는다. 해석하지 못하면 까닭을 보이고 창은 그대로다.
//! - 작업 영역: 창 아래쪽에 작업마다 진행 표시를 그린다([`App::work_area`], 모든 창에 같은 것). 작업 대기열의 사건은
//!   같은 깨우기로 오고([`App::poll_jobs`], 작업마다 초당 10번 이하) 그때만 다시 그린다. 일시정지·이어하기·취소·
//!   지금 시작·닫기 단추는 코어의 작업 대기열을 부른다.
//! - 주소창은 코어의 경로 조각([`facet_core::Provider::segments`]), 글자 해석([`App::go_to_text`]), 자동 완성
//!   ([`App::complete_address`], 일꾼 스레드)을 그리기만 한다.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use facet_core::command::{self, CommandId, Key};
use facet_core::details::{self, Column, Direction};
use facet_core::edit::{EditError, EditKind, Edited};
use facet_core::file_list;
use facet_core::history::UndoError;
use facet_core::job::{
    ConflictAnswer, CopyRejection, ErrorAnswer, JobId, JobKind, JobState, Remaining,
};
use facet_core::launch::{LaunchTarget, resolve_launch};
use facet_core::placement::{self, Monitor, Rect, WindowState};
use facet_core::selection::{CursorMove, Pick};
use facet_core::session::{self, Loaded, SAVE_DELAY, Saver};
use facet_core::tab::{AutoRefresh, Navigation, ReadOutcome, Relocation, Tab};
use facet_core::window::{
    App, Execution, MAX_PANELS, Panel, TabDestination, TabId, TargetError, TransferPlan, Update,
    WindowId,
};
use facet_core::work_area::JobEntry;
use facet_core::{ListError, Location, OpenError, ResolveError, provider_for};
use slint::winit_030::winit::event::WindowEvent;
use slint::winit_030::{EventResult, WinitWindowAccessor};
use slint::{
    CloseRequestResponse, ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker,
    PhysicalPosition, PhysicalSize, SharedString, Timer, TimerMode, VecModel,
};

use crate::address::AddressEdit;
use crate::palette::Palette;
use crate::rename::{Confirm, Rename};
use crate::tab_drag::{Frame, Hover, Point, ScreenPoint, Strip, TabDrag, Target};
use crate::target::{Section, TargetChooser};

mod address;
mod history_view;
mod ime;
mod instance;
mod keys;
mod measure;
mod palette;
mod question;
mod rename;
mod tab_drag;
mod target;
mod text;

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    let launch = instance::this_launch();
    // 단일 실행: 이미 떠 있는 facet이 있으면 인자를 넘기고 끝난다. 주인 확인은 뮤텍스 하나라 기다리지 않는다.
    // 측정 모드는 끼지 않는다(잴 때마다 띄운 프로세스가 스스로 창을 연다).
    let mut owner = if measure::is_enabled() {
        None
    } else {
        match instance::start(&launch) {
            instance::Start::Run(owner) => owner,
            instance::Start::Forwarded => return Ok(()),
        }
    };

    // 창 만들기(렌더러 준비)가 콜드 스타트에서 가장 오래 걸리므로, 목록은 그보다 먼저 읽기 시작해 겹친다.
    // 세션 파일 읽기와 해석은 작아(탭 100개에도 몇 ms) 여기서 한다. 저장은 일꾼 스레드가 한다.
    let session_path = session::session_path(measure::is_enabled());
    let loaded = session_path
        .as_deref()
        .map_or(Loaded::Missing, session::load);
    let broken = match &loaded {
        Loaded::Broken { error, kept } => {
            eprintln!("세션 파일을 되살리지 못함: {error}");
            Some(kept.clone())
        }
        Loaded::Missing | Loaded::Restored(_) => None,
    };
    let saved = loaded.session().cloned();
    // 경로 인자가 없으면 되살린 세션만 연다(세션이 없으면 코어가 홈 폴더 창을 연다).
    let targets = if launch.paths.is_empty() {
        Vec::new()
    } else {
        resolve_launch(&launch.working_dir, &launch.paths)
    };
    let app = Rc::new(RefCell::new(App::restore(loaded.into_session(), targets)));
    APP.with(|slot| *slot.borrow_mut() = Some(Rc::clone(&app)));
    if let Some(path) = session_path {
        let saver = Saver::spawn(path, SAVE_DELAY, saved);
        SAVER.with(|slot| *slot.borrow_mut() = Some(saver));
    }

    let first = app.borrow().active_window_id();
    let view = open_view(&app, first)?;
    restore_placement(&view);
    if let Some(kept) = broken {
        view.ui.set_session_broken(true);
        let kept = kept.map(|path| path.display().to_string());
        view.ui.set_session_kept(kept.unwrap_or_default().into());
    }
    // 결과는 일꾼 스레드에서 온다. 이벤트 루프를 깨워 UI 스레드에서 받는다. 깨우기 하나로 모든 창을 갱신한다.
    app.borrow_mut().set_waker(|| {
        // 이벤트 루프가 이미 끝났으면(창을 모두 닫았으면) 버린다.
        let _ = slint::invoke_from_event_loop(poll_all);
    });
    // 되살린 세션의 나머지 창도 만든다(사용자 입력을 기다리지 않는다). 결과를 받기 전에 만들어야 그 창의 탭이 읽은
    // 목록도 그 창에 들어간다.
    sync_windows();
    // 깨우기를 걸기 전에 이미 온 결과(빨리 끝난 첫 읽기)를 받는다. 그래야 첫 화면부터 목록이 보인다.
    poll_all();
    view.ui.show()?;
    if views().len() > 1 {
        // 나머지 창이 먼저 떴으므로 활성 창을 앞으로 가져온다.
        view.ui
            .window()
            .with_winit_window(|winit| winit.focus_window());
    }
    drop(view);

    // 수신기는 이벤트 루프가 생긴 뒤(첫 창을 만든 뒤) 연다. 받은 인자는 수신기 스레드에서 해석하고 UI 스레드에는
    // 새 창 열기만 맡긴다. 그 사이 두 번째 실행이 오면 넘기는 쪽이 파이프가 생길 때까지 다시 시도한다.
    if let Some(owner) = owner.as_mut()
        && let Err(error) = owner.listen(|launch| {
            let targets = resolve_launch(&launch.working_dir, &launch.paths);
            // 이벤트 루프가 이미 끝났으면(마지막 창을 닫음) 버린다.
            let _ = slint::invoke_from_event_loop(move || open_launch_window(targets));
        })
    {
        eprintln!("단일 실행 수신기를 열지 못함: {error}");
    }

    // 창을 닫아도 이벤트 루프는 스스로 끝나지 않는다. 끝낼지는 코어가 정한다([`App::is_finished`], [`sync_windows`]).
    let result = slint::run_event_loop_until_quit();
    // 주인 자리를 바로 내놓는다: 그 뒤의 실행은 이 프로세스에 넘기지 않고 스스로 주인이 된다.
    drop(owner);
    // Slint의 스레드 지역 상태보다 먼저 버린다. 스레드가 끝날 때 버리면 Slint 타이머·모델이 이미 사라진
    // 상태를 건드려 멈춘다.
    let views = VIEWS.with(|slot| std::mem::take(&mut *slot.borrow_mut()));
    for view in &views {
        let _ = view.ui.hide();
    }
    drop(views);
    if let Some(progress) = PROGRESS.with(|slot| slot.borrow_mut().take()) {
        let _ = progress.ui.hide();
    }
    // 마지막 상태를 한 번 더 쓴다. 창이 모두 닫혀 끝났으면 마지막 창이 있던 세션이 남는다.
    save_session();
    if let Some(saver) = SAVER.with(|slot| slot.borrow_mut().take())
        && let Err(error) = saver.finish()
    {
        eprintln!("세션을 저장하지 못함: {error}");
    }
    APP.with(|slot| slot.borrow_mut().take());
    result
}

/// 지금 화면 상태를 세션 저장에 알린다. 바뀌었을 때만 일꾼 스레드에 넘기고 디스크는 기다리지 않는다.
fn save_session() {
    let Some(app) = APP.with(|slot| slot.borrow().clone()) else {
        return;
    };
    let Ok(app) = app.try_borrow() else { return };
    let session = app.session();
    SAVER.with(|slot| {
        if let Some(saver) = slot.borrow_mut().as_mut() {
            saver.update(session);
        }
    });
}

thread_local! {
    /// UI 스레드의 코어 앱. 모든 창이 함께 쓴다.
    static APP: RefCell<Option<Rc<RefCell<App>>>> = const { RefCell::new(None) };
    /// 세션 저장(일꾼 스레드). 세션 파일 자리가 없으면(측정 모드 등) `None`이다.
    static SAVER: RefCell<Option<Saver>> = const { RefCell::new(None) };
    /// UI 스레드의 화면 상태: 코어의 창마다 하나, 연 차례로.
    static VIEWS: RefCell<Vec<Rc<View>>> = const { RefCell::new(Vec::new()) };
    /// 탭을 창 밖에 놓아 생긴, 아직 띄우지 않은 창의 자리와 크기. `sync_windows`가 띄울 때 쓰고 지운다.
    static PLACEMENTS: RefCell<Vec<(WindowId, Placement)>> = const { RefCell::new(Vec::new()) };
    /// 진행 창. 보통 창이 없는데 작업이 남아 있을 때만 있다([`sync_progress`]).
    static PROGRESS: RefCell<Option<ProgressView>> = const { RefCell::new(None) };
}

/// 새 창을 띄울 자리(바깥 테두리 왼쪽 위의 화면 점)와 크기(물리 픽셀).
#[derive(Debug, Clone, Copy)]
struct Placement {
    origin: ScreenPoint,
    size: PhysicalSize,
}

/// 되살린 창을 세션에 남은 자리에 놓는다(띄우기 전에 부른다). 지금 모니터들에서 보이지 않는 자리면 주 모니터
/// 안으로 고친다([`placement::fit`]). 최대화한 창은 보통 자리에 놓고 최대화한다: 그 모니터에서 최대화되고, 최대화를
/// 풀면 보통 자리로 돌아간다. 자리를 모르는 창(새 창)은 Windows가 정한 자리에 뜬다.
///
/// 자리와 크기는 물리 픽셀이다(좌표 단위는 [`facet_core::placement`]). Slint는 물리 크기를 그대로 winit에 넘기고,
/// 배율이 다른 모니터로 옮겨지며 배율이 바뀌어도 그 물리 크기를 지킨다.
fn restore_placement(view: &View) {
    let Some(saved) = view
        .app
        .borrow()
        .window(view.window)
        .and_then(|window| window.placement())
    else {
        return;
    };
    let rect = placement::fit(saved.normal, &monitors());
    let window = view.ui.window();
    window.set_size(PhysicalSize::new(rect.width, rect.height));
    window.set_position(PhysicalPosition::new(rect.x, rect.y));
    if saved.maximized {
        window.set_maximized(true);
    }
}

/// 지금 붙어 있는 모니터들의 작업 영역(Windows 호출은 `facet_win`에 있다).
fn monitors() -> Vec<Monitor> {
    facet_win::monitors()
        .into_iter()
        .filter_map(|m| {
            let width = u32::try_from(m.right.checked_sub(m.left)?).ok()?;
            let height = u32::try_from(m.bottom.checked_sub(m.top)?).ok()?;
            Some(Monitor {
                work_area: Rect::new(m.left, m.top, width, height),
                primary: m.primary,
            })
        })
        .collect()
}

/// 창 `window`의 지금 자리를 코어에 알리고 세션 저장에 넘긴다. 최대화한 창은 최대화를 풀었을 때의 자리를 그대로
/// 두고, 최소화한 창의 자리는 남기지 않는다([`placement::Placement::observe`]).
fn note_placement(window: WindowId) {
    let Some(view) = view_of(window) else { return };
    let Some((rect, state)) = view
        .ui
        .window()
        .with_winit_window(|winit| {
            let position = winit.outer_position().ok()?;
            let size = winit.inner_size();
            let state = if winit.is_minimized() == Some(true) {
                WindowState::Minimized
            } else if winit.is_maximized() {
                WindowState::Maximized
            } else {
                WindowState::Normal
            };
            Some((
                Rect::new(position.x, position.y, size.width, size.height),
                state,
            ))
        })
        .flatten()
    else {
        return;
    };
    {
        let Ok(mut app) = view.app.try_borrow_mut() else {
            return;
        };
        let Some(core) = app.window_mut(window) else {
            return;
        };
        let placement = placement::Placement::observe(core.placement(), rect, state);
        core.set_placement(placement);
    }
    save_session();
}

/// 열린 창의 화면 상태들. 부르는 동안 목록을 빌려 두지 않도록 복사해 준다.
fn views() -> Vec<Rc<View>> {
    VIEWS.with(|slot| slot.borrow().clone())
}

/// 코어 창 `window`의 화면 상태.
fn view_of(window: WindowId) -> Option<Rc<View>> {
    VIEWS.with(|slot| slot.borrow().iter().find(|v| v.window == window).cloned())
}

/// 코어 창 `window`의 Slint 창을 만들고 콜백을 잇는다. 보이기는 부르는 쪽이 한다. 측정 모드면 창마다 프레임 세기를 건다.
fn open_view(app: &Rc<RefCell<App>>, window: WindowId) -> Result<Rc<View>, slint::PlatformError> {
    let ui = AppWindow::new()?;
    // Slint는 창마다 winit 이벤트 훅을 하나만 둔다: 키 위치를 적고(Ctrl+백슬래시), 그 창이 포커스를 얻으면 활성
    // 창으로 알리고, 측정 모드면 프레임을 센다.
    let mut frames = measure::frame_counter(&ui, measure::window_key(window));
    let hook_app = Rc::downgrade(app);
    // 창을 옮기거나 크기를 바꾸면(최대화·최소화 포함) 이벤트를 다 처리한 뒤 그 자리를 코어에 알린다. 끄는 동안 이벤트가
    // 잇달아 와도 한 번만 맡긴다.
    let placement_pending = Rc::new(Cell::new(false));
    ui.window().on_winit_window_event(move |_, event| {
        keys::note_window_event(event);
        if let WindowEvent::Focused(true) = event
            && let Some(app) = hook_app.upgrade()
            && let Ok(mut app) = app.try_borrow_mut()
        {
            app.activate_window(window);
        }
        if matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_))
            && !placement_pending.replace(true)
        {
            let pending = Rc::clone(&placement_pending);
            Timer::single_shot(Duration::ZERO, move || {
                pending.set(false);
                note_placement(window);
            });
        }
        if let Some(frames) = frames.as_mut() {
            frames(event);
        }
        EventResult::Propagate
    });
    ui.window().on_close_requested(move || {
        close_window(window);
        CloseRequestResponse::HideWindow
    });
    connect(&ui, window);
    let view = View::new(ui, Rc::clone(app), window);
    VIEWS.with(|slot| slot.borrow_mut().push(Rc::clone(&view)));
    view.render();
    view.render_jobs();
    for panel in 0..MAX_PANELS {
        view.restore_scroll(panel);
    }
    Ok(view)
}

/// 창 `window`의 화면 상태로 `f`를 부른다. 사용자가 그 창에서 무언가를 했으므로 먼저 그 창을 코어의 활성 창으로
/// 하고(명령은 활성 창의 활성 패널의 활성 탭으로 간다), 부른 뒤 코어의 창 목록에 화면을 맞춘다.
fn with_view(window: WindowId, f: impl FnOnce(&View)) {
    let Some(view) = view_of(window) else { return };
    {
        let mut app = view.app.borrow_mut();
        if app.has_windows() && app.active_window_id() != window {
            app.activate_window(window);
        }
    }
    f(&view);
    drop(view);
    sync_windows();
}

/// 첫 창(남아 있는 가장 먼저 연 창)의 화면 상태로 `f`를 부른다. 측정 모드가 쓴다.
fn with_first_view(f: impl FnOnce(&View)) {
    let window = VIEWS.with(|slot| slot.borrow().first().map(|v| v.window));
    if let Some(window) = window {
        with_view(window, f);
    }
}

/// 제목 줄의 닫기 단추로 창 `window`를 닫는다: 코어가 그 창의 탭들을 닫은 탭 목록에 넣는다.
fn close_window(window: WindowId) {
    if let Some(view) = view_of(window) {
        view.remember_scroll();
        view.app.borrow_mut().close_window(window);
    }
    sync_windows();
    // 활성 창이 바뀌었을 수 있다(닫은 탭 되살리기는 활성 창에 연다). 남은 창들을 다시 그린다.
    for view in views() {
        view.render();
    }
}

/// 다시 실행한 facet이 넘긴 인자로 새 창을 열고 앞으로 가져온다. 창이 하나도 없어도 연다.
fn open_launch_window(targets: Vec<LaunchTarget>) {
    let Some(app) = APP.with(|slot| slot.borrow().clone()) else {
        return;
    };
    let window = app.borrow_mut().open_launch_window(targets);
    sync_windows();
    if let Some(view) = view_of(window) {
        view.ui
            .window()
            .with_winit_window(|winit| winit.focus_window());
    }
}

/// 코어의 창 목록에 화면을 맞춘다: 새 창은 Slint 창을 만들어 띄우고, 닫힌 창은 감춘다. 진행 창도 맞춘다
/// ([`sync_progress`]). 창이 남지 않고 끝나지 않은 작업도 없어 앱이 끝났으면([`App::is_finished`]) 이벤트 루프를
/// 끝낸다.
fn sync_windows() {
    let Some(app) = APP.with(|slot| slot.borrow().clone()) else {
        return;
    };
    let open: Vec<WindowId> = app.borrow().windows().iter().map(|w| w.id()).collect();
    let (closed, kept): (Vec<_>, Vec<_>) = views()
        .into_iter()
        .partition(|view| !open.contains(&view.window));
    if !closed.is_empty() {
        VIEWS.with(|slot| *slot.borrow_mut() = kept.clone());
        for view in &closed {
            let _ = view.ui.hide();
        }
        // 그 창의 콜백 안에서 불렸을 수 있으므로 창은 이벤트 루프로 돌아간 뒤 버린다.
        Timer::single_shot(Duration::ZERO, move || drop(closed));
    }
    for window in open {
        if kept.iter().any(|view| view.window == window) {
            continue;
        }
        match open_view(&app, window) {
            Ok(view) => {
                let placement = PLACEMENTS.with(|slot| {
                    let mut pending = slot.borrow_mut();
                    let at = pending.iter().position(|(id, _)| *id == window)?;
                    Some(pending.remove(at).1)
                });
                if let Some(placement) = placement {
                    let window = view.ui.window();
                    window.set_size(placement.size);
                    window.set_position(PhysicalPosition::new(
                        placement.origin.x.round() as i32,
                        placement.origin.y.round() as i32,
                    ));
                } else {
                    restore_placement(&view);
                }
                let _ = view.ui.show();
            }
            Err(error) => eprintln!("창을 만들지 못함: {error}"),
        }
    }
    // 보통 창이 모두 닫혔는데 작업이 남았으면 진행 창을 띄우고, 보통 창이 다시 생겼거나 작업이 끝났으면 닫는다.
    sync_progress(&app);
    // 창·패널·탭·분할 비율이 바뀌었을 수 있다(사용자 입력은 모두 여기를 지난다). 진행 창만 남았으면 세션은 `None`이라
    // 넘기지 않고, 파일에는 마지막 보통 창 상태가 남는다.
    save_session();
    if app.borrow().is_finished() {
        let _ = slint::quit_event_loop();
    }
}

/// 화면 점 `at` 아래 보이는 facet 창. 다른 프로그램의 창이 위에 있거나 창이 없으면 `None`이다. Windows에게 그 점의
/// 맨 위 창을 묻고(`facet_win::top_level_window_at`, 겹친 창의 앞뒤를 안다) 그 창을 그리는 화면 상태를 찾는다.
#[cfg(windows)]
fn view_under(at: ScreenPoint) -> Option<Rc<View>> {
    let hit = facet_win::top_level_window_at(at.x.floor() as i32, at.y.floor() as i32)?;
    views().into_iter().find(|view| view.handle() == Some(hit))
}

/// 화면 점 `at`을 품은 facet 창(먼저 연 창부터). Windows가 아니면 창의 앞뒤를 모르므로 자리만 본다.
#[cfg(not(windows))]
fn view_under(at: ScreenPoint) -> Option<Rc<View>> {
    views().into_iter().find(|view| {
        let Some(frame) = view.frame() else {
            return false;
        };
        let size = view.ui.window().size();
        let inside = frame.to_window(at);
        let scale = frame.scale.max(f32::EPSILON);
        inside.x >= 0.0
            && inside.y >= 0.0
            && inside.x < size.width as f32 / scale
            && inside.y < size.height as f32 / scale
    })
}

/// 모든 창에 끄는 탭(흐리게, 누른 창의 그 패널)과 놓일 자리(세로 선, 목적지 창의 그 패널)를 넣는다. `None`이면
/// 지운다.
fn show_tab_drag(drag: Option<&TabDrag<WindowId>>) {
    for view in views() {
        for panel in 0..MAX_PANELS {
            let dragged = drag
                .filter(|drag| drag.window == view.window && drag.panel == panel)
                .map_or(-1, |drag| count(drag.index));
            let slot = drag
                .and_then(|drag| drag.slot_in(view.window, panel))
                .map_or(-1, count);
            PanelUi::of(&view.ui, panel).set_tab_drag(dragged, slot);
        }
    }
}

/// 일꾼 스레드에서 온 결과를 모두 탭에 넘기고, 창마다 제 탭의 결과를 그린다.
fn poll_all() {
    let Some(app) = APP.with(|slot| slot.borrow().clone()) else {
        return;
    };
    let views = views();
    // 지금 스크롤 위치를 탭에 알린다: 이동이 확정되면 떠나는 위치의 기록에 남는다.
    for view in &views {
        view.remember_scroll();
    }
    let (updates, jobs) = {
        let mut app = app.borrow_mut();
        (app.poll(), app.poll_jobs())
    };
    if jobs {
        for view in &views {
            view.render_jobs();
        }
        render_progress(&app);
        // 진행 창만 남은 동안 마지막 작업이 끝났으면 진행 창을 닫고 앱을 끝낸다.
        if !app.borrow().has_windows() {
            sync_windows();
        }
    }
    if app.borrow().is_finished() {
        sync_windows();
        return;
    }
    if updates.is_empty() {
        return;
    }
    for view in &views {
        view.receive(&updates);
    }
    // 이동이 확정되거나 사라진 위치에서 옮겨 위치가 바뀌었을 수 있다.
    save_session();
}

/// 창의 콜백을 그 창의 화면 상태에 잇는다.
fn connect(ui: &AppWindow, w: WindowId) {
    ui.on_key(move |text, ctrl, alt, shift| {
        let mut handled = false;
        with_view(w, |view| handled = view.key(&text, ctrl, alt, shift));
        handled
    });
    ui.on_typed(move |text| with_view(w, |view| view.typed(&text)));
    ui.on_panel_pressed(move |index| with_view(w, |view| view.panel_pressed(index)));
    ui.on_split_dragged(move |ratio| with_view(w, |view| view.split_dragged(ratio)));
    ui.on_split_reset(move || with_view(w, View::split_reset));
    ui.on_row_pressed(move |row, ctrl, shift| {
        with_view(w, |view| {
            view.row_pressed(row, Pick::for_click(ctrl, shift))
        });
    });
    ui.on_row_opened(move |row| with_view(w, |view| view.row_opened(row)));
    ui.on_row_middle_pressed(move |row| with_view(w, |view| view.row_middle_pressed(row)));
    ui.on_tab_pressed(move |index| with_view(w, |view| view.tab_pressed(index)));
    ui.on_tab_drag_start(move |panel, index, x, y| {
        with_view(w, |view| view.tab_drag_start(panel, index, Point { x, y }));
    });
    ui.on_tab_drag_move(move |x, y| with_view(w, |view| view.tab_drag_move(Point { x, y })));
    ui.on_tab_drag_end(move |x, y| {
        let mut clicked = false;
        with_view(w, |view| clicked = view.tab_drag_end(Point { x, y }));
        clicked
    });
    ui.on_tab_drag_cancel(move || with_view(w, View::tab_drag_cancel));
    ui.on_sort_by(move |column| {
        let column = match column {
            1 => Column::Modified,
            2 => Column::Type,
            3 => Column::Size,
            _ => Column::Name,
        };
        with_view(w, |view| view.execute(CommandId::sort_by(column)));
    });
    ui.on_mouse_history(move |forward| {
        let command = if forward {
            CommandId::Forward
        } else {
            CommandId::Back
        };
        with_view(w, |view| view.execute(command));
    });
    ui.on_begin_rename(move || with_view(w, View::begin_rename));
    ui.on_confirm_rename(move |text| {
        let mut closed = true;
        with_view(w, |view| closed = view.confirm_rename(&text));
        closed
    });
    ui.on_cancel_rename(move || {
        let mut closed = true;
        with_view(w, |view| closed = view.cancel_rename());
        closed
    });
    ui.on_rename_edited(move |text| with_view(w, |view| view.rename_edited(&text)));
    ui.on_answer_extension(move |yes| {
        let mut closed = true;
        with_view(w, |view| closed = view.answer_extension(yes));
        closed
    });
    ui.on_remove_selection(|text, anchor, cursor| {
        ime::remove_selection(&text, anchor, cursor).0.into()
    });
    ui.on_segment_pressed(move |index| with_view(w, |view| view.segment_pressed(index)));
    ui.on_begin_address_edit(move || with_view(w, View::begin_address_edit));
    ui.on_address_edited(move |text| with_view(w, |view| view.address_edited(&text)));
    ui.on_address_step(move |step| {
        let mut end = -1;
        with_view(w, |view| end = view.address_step(step >= 0));
        end
    });
    ui.on_address_go(move |text| {
        let mut went = false;
        with_view(w, |view| went = view.address_go(&text));
        went
    });
    ui.on_candidate_pressed(move |index| with_view(w, |view| view.candidate_pressed(index)));
    ui.on_cancel_address_edit(move || with_view(w, View::close_address_edit));
    ui.on_palette_edited(move |text| with_view(w, |view| view.palette_edited(&text)));
    ui.on_palette_step(move |step| with_view(w, |view| view.palette_step(step >= 0)));
    ui.on_palette_run(move |index| {
        let mut ran = false;
        with_view(w, |view| ran = view.palette_run(index));
        ran
    });
    ui.on_palette_close(move || with_view(w, View::close_palette));
    ui.on_history_close(move || with_view(w, View::close_history));
    ui.on_target_edited(move |text| with_view(w, |view| view.target_edited(&text)));
    ui.on_target_step(move |step| {
        let mut end = -1;
        with_view(w, |view| end = view.target_step(step >= 0));
        end
    });
    ui.on_target_accept(move |text| {
        let mut closed = false;
        with_view(w, |view| closed = view.target_accept(&text));
        closed
    });
    ui.on_target_row_pressed(move |index| {
        let mut closed = false;
        with_view(w, |view| closed = view.target_row_pressed(index));
        closed
    });
    ui.on_target_cancel(move || with_view(w, View::close_target));
    ui.on_confirm_accept(move || with_view(w, View::accept_transfer));
    ui.on_confirm_cancel(move || with_view(w, View::close_transfer));
    ui.on_job_pause(move |id| job_action(id, JobAction::Pause));
    ui.on_job_resume(move |id| job_action(id, JobAction::Resume));
    ui.on_job_cancel(move |id| job_action(id, JobAction::Cancel));
    ui.on_job_start_now(move |id| job_action(id, JobAction::StartNow));
    ui.on_job_dismiss(move |id| job_action(id, JobAction::Dismiss));
    ui.on_job_answer_conflict(move |id, choice, rest| {
        job_action(
            id,
            JobAction::AnswerConflict(question::conflict_answer(choice, rest)),
        );
    });
    ui.on_job_answer_error(move |id, choice| {
        job_action(id, JobAction::AnswerError(question::error_answer(choice)));
    });
}

/// 진행 표시의 단추.
#[derive(Debug, Clone, Copy)]
enum JobAction {
    Pause,
    Resume,
    Cancel,
    StartNow,
    Dismiss,
    /// 질문 대화의 답. 다른 창에서 먼저 답했으면 코어가 받지 않는다(한 번만 적용).
    AnswerConflict(ConflictAnswer),
    AnswerError(ErrorAnswer),
}

/// 진행 표시의 단추를 눌렀다: 화면에 넘긴 작업 번호로 작업을 찾아 코어의 작업 대기열을 부르고, 모든 창의 작업 영역을
/// 다시 그린다. 끝난 작업에 부르면 코어가 아무것도 하지 않는다.
fn job_action(id: i32, action: JobAction) {
    let Some(app) = APP.with(|slot| slot.borrow().clone()) else {
        return;
    };
    {
        let mut app = app.borrow_mut();
        let Some(job) = app
            .work_area()
            .entries()
            .iter()
            .map(|entry| entry.id)
            .find(|job| job_number(*job) == id)
        else {
            return;
        };
        match action {
            JobAction::Pause => {
                app.jobs_mut().pause(job);
            }
            JobAction::Resume => {
                app.jobs_mut().resume(job);
            }
            JobAction::Cancel => {
                app.jobs_mut().cancel(job);
            }
            JobAction::StartNow => {
                app.jobs_mut().start_now(job);
            }
            JobAction::Dismiss => {
                app.dismiss_job(job);
            }
            // 이미 답한 질문(다른 창)이나 고를 수 없는 답이면 받지 않고 질문이 그대로 남는다.
            JobAction::AnswerConflict(answer) => {
                let _ = app.answer_conflict(job, answer);
            }
            JobAction::AnswerError(answer) => {
                let _ = app.answer_error(job, answer);
            }
        }
    }
    refresh_jobs();
}

/// 작업 대기열에서 온 사건을 작업 영역에 반영하고 모든 창의 작업 영역을 다시 그린다.
fn refresh_jobs() {
    let Some(app) = APP.with(|slot| slot.borrow().clone()) else {
        return;
    };
    if let Ok(mut app) = app.try_borrow_mut() {
        app.poll_jobs();
    }
    for view in views() {
        view.render_jobs();
    }
    render_progress(&app);
}

/// 작업 영역의 작업들을 진행 표시 모델에 넣는다(보통 창의 작업 영역과 진행 창이 함께 쓴다). 같은 작업들이면 바뀐
/// 줄만 고쳐 단추 같은 화면 요소를 다시 만들지 않는다. 질문을 기다리는 작업 수를 돌려준다. 앱을 빌리지 못하면
/// `None`이다.
/// 작업 영역의 줄을 고친다. 질문 대화가 붙은 작업 수와 못 한 항목 줄이 붙은 작업 수를 돌려준다.
fn render_job_rows(app: &RefCell<App>, jobs: &VecModel<JobRow>) -> Option<(usize, usize)> {
    let rows: Vec<JobRow> = {
        let app = app.try_borrow().ok()?;
        app.work_area().entries().iter().map(job_row).collect()
    };
    let questions = rows.iter().filter(|row| row.ask != JobAsk::None).count();
    let reports = rows.iter().filter(|row| !row.not_done.is_empty()).count();
    let same =
        jobs.row_count() == rows.len() && jobs.iter().zip(&rows).all(|(old, new)| old.id == new.id);
    if !same {
        jobs.set_vec(rows);
        return Some((questions, reports));
    }
    for (i, row) in rows.into_iter().enumerate() {
        if jobs.row_data(i).as_ref() != Some(&row) {
            jobs.set_row_data(i, row);
        }
    }
    Some((questions, reports))
}

/// 진행 창: 보통 창이 하나도 없는데 끝나지 않은 작업이 남아 있을 때만 떠 있는 작은 창([`App::shows_progress_window`]).
/// 코어의 창이 아니어서 창 목록·세션에 들지 않는다.
struct ProgressView {
    ui: ProgressWindow,
    jobs: Rc<VecModel<JobRow>>,
}

impl ProgressView {
    fn open() -> Result<Self, slint::PlatformError> {
        let ui = ProgressWindow::new()?;
        let jobs = Rc::new(VecModel::default());
        ui.set_jobs(ModelRc::from(Rc::clone(&jobs)));
        // 닫기 단추로는 닫지 않는다: 닫으며 작업을 취소하면 사용자가 모르는 사이 작업을 잃을 수 있고, 창 없이 두면
        // 프로세스가 보이지 않게 남는다. 그만두려면 작업마다 취소하고, 작업이 모두 끝나면 저절로 닫힌다.
        let weak = ui.as_weak();
        ui.window().on_close_requested(move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_close_refused(true);
            }
            CloseRequestResponse::KeepWindowShown
        });
        ui.on_new_window(open_new_window);
        ui.on_key(|text, ctrl, alt, shift| {
            let command =
                keys::shortcut_of(&text, ctrl, alt, shift).and_then(command::find_by_shortcut);
            if command == Some(CommandId::NewWindow) {
                open_new_window();
                return true;
            }
            false
        });
        ui.on_job_pause(move |id| job_action(id, JobAction::Pause));
        ui.on_job_resume(move |id| job_action(id, JobAction::Resume));
        ui.on_job_cancel(move |id| job_action(id, JobAction::Cancel));
        ui.on_job_start_now(move |id| job_action(id, JobAction::StartNow));
        ui.on_job_dismiss(move |id| job_action(id, JobAction::Dismiss));
        ui.on_job_answer_conflict(move |id, choice, rest| {
            job_action(
                id,
                JobAction::AnswerConflict(question::conflict_answer(choice, rest)),
            );
        });
        ui.on_job_answer_error(move |id, choice| {
            job_action(id, JobAction::AnswerError(question::error_answer(choice)));
        });
        Ok(ProgressView { ui, jobs })
    }

    fn render(&self, app: &RefCell<App>) {
        render_job_rows(app, &self.jobs);
    }
}

/// 진행 창의 「새 창」과 Ctrl+N: 코어의 「새 창」 명령(창이 없으면 사용자 홈 폴더)으로 보통 창을 열고 앞으로
/// 가져온다. 보통 창이 생기면 진행 창은 닫힌다.
fn open_new_window() {
    let Some(app) = APP.with(|slot| slot.borrow().clone()) else {
        return;
    };
    if app.borrow_mut().execute(CommandId::NewWindow) != Execution::Done {
        return;
    }
    let window = app.borrow().active_window_id();
    sync_windows();
    if let Some(view) = view_of(window) {
        view.ui
            .window()
            .with_winit_window(|winit| winit.focus_window());
    }
}

/// 진행 창을 코어에 맞춘다: 띄워야 하는데 없으면 만들어 띄우고, 필요 없어졌으면 감춘다(그 창의 콜백 안에서 불렸을
/// 수 있으므로 버리기는 이벤트 루프로 돌아간 뒤).
fn sync_progress(app: &RefCell<App>) {
    let wanted = app.borrow().shows_progress_window();
    let open = PROGRESS.with(|slot| slot.borrow().is_some());
    if wanted && !open {
        match ProgressView::open() {
            Ok(view) => {
                view.render(app);
                let _ = view.ui.show();
                PROGRESS.with(|slot| *slot.borrow_mut() = Some(view));
            }
            Err(error) => eprintln!("진행 창을 만들지 못함: {error}"),
        }
    } else if !wanted
        && open
        && let Some(view) = PROGRESS.with(|slot| slot.borrow_mut().take())
    {
        let _ = view.ui.hide();
        Timer::single_shot(Duration::ZERO, move || drop(view));
    }
}

/// 진행 창이 떠 있으면 다시 그린다.
fn render_progress(app: &RefCell<App>) {
    PROGRESS.with(|slot| {
        if let Some(view) = slot.borrow().as_ref() {
            view.render(app);
        }
    });
}

/// 화면에 넘기는 작업 번호.
fn job_number(job: JobId) -> i32 {
    i32::try_from(job.get()).unwrap_or(i32::MAX)
}

fn count64(n: u64) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// 작업 영역의 작업 하나를 진행 표시의 값으로. 수치 글자는 여기서 만들고 문장은 화면이 만든다.
fn job_row(entry: &JobEntry) -> JobRow {
    let progress = entry.progress.as_ref();
    let summary = entry.summary.as_ref();
    let asked = question::view(entry.question.as_ref());
    let remaining = match progress.map(|p| p.remaining) {
        Some(Remaining::Known(left)) => text::remaining_text(left),
        Some(Remaining::Calculating) | None => String::new(),
    };
    JobRow {
        id: job_number(entry.id),
        moving: entry.kind == JobKind::Move,
        reverting: match entry.kind {
            // 삭제 작업의 진행 표시는 뒤 티켓(앱)이 따로 그린다. 그때까지는 종류 표시 없이 보인다.
            JobKind::Copy | JobKind::Move | JobKind::Delete => 0,
            JobKind::Undo => 1,
            JobKind::Redo => 2,
        },
        target: entry.target.display().to_string().into(),
        status: match entry.state {
            JobState::Queued => JobStatus::Queued,
            JobState::Running => JobStatus::Running,
            JobState::Paused => JobStatus::Paused,
            JobState::AwaitingAnswer => JobStatus::AwaitingAnswer,
            JobState::Finished => JobStatus::Finished,
            JobState::Failed => JobStatus::Failed,
            JobState::Cancelled => JobStatus::Cancelled,
        },
        terminal: entry.state.is_terminal(),
        items_done: progress.map_or(0, |p| count64(p.items_done)),
        // 진행이 오기 전에는 넘긴 원본 수를 보인다(다 세지 않은 수).
        items_total: progress.map_or(count(entry.sources), |p| count64(p.items_total)),
        counted: progress.is_some_and(|p| p.counted),
        bytes_done: file_list::format_size(progress.map_or(0, |p| p.bytes_done)).into(),
        bytes_total: file_list::format_size(progress.map_or(0, |p| p.bytes_total)).into(),
        speed: progress
            .map(|p| file_list::format_size(p.bytes_per_second))
            .unwrap_or_default()
            .into(),
        remaining: remaining.into(),
        current: progress
            .and_then(|p| p.current.as_ref())
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
            .into(),
        finished: summary.is_some(),
        completed: summary.map_or(0, |s| count64(s.items_completed())),
        skipped: summary.map_or(0, |s| count64(s.items_skipped())),
        left: summary.map_or(0, |s| count64(s.items_remaining)),
        not_done: summary.map(text::not_done_text).unwrap_or_default().into(),
        ask: asked.ask,
        source: asked.source,
        target_item: asked.target,
        newer: asked.newer,
        larger: asked.larger,
        can_overwrite: asked.can_overwrite,
        overwrite_permanent: asked.overwrite_permanent,
        error_path: asked.error_path.into(),
        error_reason: asked.error_reason,
    }
}

/// 시작 전에 막힌 까닭을 확인 줄의 종류와 경로로. 문장은 화면이 만든다.
fn transfer_problem(rejection: &CopyRejection, plan: &TransferPlan) -> (TransferProblem, String) {
    match rejection {
        CopyRejection::NoSources => (TransferProblem::NoSources, String::new()),
        CopyRejection::TargetMissing => (
            TransferProblem::TargetMissing,
            plan.target.display().to_string(),
        ),
        CopyRejection::TargetNotFolder => (
            TransferProblem::TargetNotFolder,
            plan.target.display().to_string(),
        ),
        CopyRejection::IntoItself(source) => {
            (TransferProblem::IntoItself, source.display().to_string())
        }
        CopyRejection::TargetInsideSource(source) => (
            TransferProblem::TargetInsideSource,
            source.display().to_string(),
        ),
    }
}

/// UI 스레드의 창 하나의 화면 상태: 코어의 창 하나와 그것을 그리는 Slint 창.
struct View {
    /// 그리는 코어 창.
    window: WindowId,
    ui: AppWindow,
    app: Rc<RefCell<App>>,
    /// 패널마다(0 왼쪽, 1 오른쪽) 화면에 넘긴 모델.
    panels: [PanelModels; MAX_PANELS],
    /// 열려 있거나 확정해 결과를 기다리는 이름바꾸기. 열려 있으면 화면의 `editing`이 그 행이다.
    rename: RefCell<Option<Rename>>,
    /// 새 폴더를 만들라고 넘기고 결과를 기다리는 탭. 결과가 오면 만든 폴더의 행에 이름 입력란을 연다.
    new_folder_tab: Cell<Option<TabId>>,
    /// 주소창을 고쳐 쓰는 중이면 그 상태. 화면의 `address-editing`과 함께 바뀐다.
    address: RefCell<Option<AddressEdit>>,
    /// 자동 완성 후보의 이름.
    candidates: Rc<VecModel<SharedString>>,
    /// 명령 팔레트가 열려 있으면 그 검색 결과와 고른 줄.
    palette: RefCell<Option<Palette>>,
    /// 명령 팔레트의 검색 결과 줄.
    palette_rows: Rc<VecModel<PaletteRow>>,
    /// 「읽는 중」을 드러낼 때 다시 그리는 타이머.
    reading_timer: Timer,
    /// 켜져 있으면 다음 `render`가 행 모델을 reset하지 않는다(행 눌러 고르기가 바뀐 행만 알리는 동안).
    keep_rows: Cell<bool>,
    /// 누른 탭의 끌기 상태. 탭을 누르면 생기고 놓거나 끊기면 없어진다.
    tab_drag: RefCell<Option<TabDrag<WindowId>>>,
    /// 복사·이동 대상 선택 창이 열려 있으면 그 상태. 화면의 `target-open`과 함께 바뀐다.
    target: RefCell<Option<TargetChooser>>,
    /// 대상 선택 창의 줄.
    target_rows: Rc<VecModel<TargetRow>>,
    /// 확인 줄이 열려 있으면 확인하면 넣을 복사·이동. 화면의 `confirm-open`과 함께 바뀐다.
    transfer: RefCell<Option<TransferPlan>>,
    /// 작업 영역의 진행 표시들.
    jobs: Rc<VecModel<JobRow>>,
}

/// 패널 하나의 화면 모델: 보이는 탭의 행, 탭 줄, 주소창 경로 조각. 탭 줄과 경로 조각은 바뀔 때만 고쳐 화면 요소를
/// 다시 만들지 않는다.
struct PanelModels {
    rows: Rc<TabRows>,
    tabs: Rc<VecModel<TabItem>>,
    segments: Rc<VecModel<SharedString>>,
}

impl View {
    fn new(ui: AppWindow, app: Rc<RefCell<App>>, window: WindowId) -> Rc<Self> {
        let panels = [0, 1].map(|panel| PanelModels {
            rows: Rc::new(TabRows {
                app: Rc::clone(&app),
                window,
                panel,
                notify: ModelNotify::default(),
            }),
            tabs: Rc::new(VecModel::default()),
            segments: Rc::new(VecModel::default()),
        });
        let [left, right] = &panels;
        ui.set_left_rows(ModelRc::from(Rc::clone(&left.rows)));
        ui.set_left_tabs(ModelRc::from(Rc::clone(&left.tabs)));
        ui.set_left_segments(ModelRc::from(Rc::clone(&left.segments)));
        ui.set_right_rows(ModelRc::from(Rc::clone(&right.rows)));
        ui.set_right_tabs(ModelRc::from(Rc::clone(&right.tabs)));
        ui.set_right_segments(ModelRc::from(Rc::clone(&right.segments)));
        let candidates = Rc::new(VecModel::default());
        ui.set_candidates(ModelRc::from(Rc::clone(&candidates)));
        let palette_rows = Rc::new(VecModel::default());
        ui.set_palette_rows(ModelRc::from(Rc::clone(&palette_rows)));
        let target_rows = Rc::new(VecModel::default());
        ui.set_target_rows(ModelRc::from(Rc::clone(&target_rows)));
        let jobs = Rc::new(VecModel::default());
        ui.set_jobs(ModelRc::from(Rc::clone(&jobs)));
        Rc::new(View {
            window,
            ui,
            app,
            panels,
            rename: RefCell::default(),
            new_folder_tab: Cell::new(None),
            address: RefCell::default(),
            candidates,
            palette: RefCell::default(),
            palette_rows,
            reading_timer: Timer::default(),
            keep_rows: Cell::new(false),
            tab_drag: RefCell::default(),
            target: RefCell::default(),
            target_rows,
            transfer: RefCell::default(),
            jobs,
        })
    }

    /// 작업 영역을 그린다. 같은 작업들이면 바뀐 줄만 고쳐 단추 같은 화면 요소를 다시 만들지 않는다.
    fn render_jobs(&self) {
        if let Some((questions, reports)) = render_job_rows(&self.app, &self.jobs) {
            self.ui.set_job_questions(count(questions));
            self.ui.set_job_reports(count(reports));
        }
    }

    /// 반대편 패널로 복사·이동의 확인 줄을 연다: 대상과 항목 수를 보인다. 지금 할 수 없으면 열지 않는다.
    fn open_transfer(&self, kind: JobKind) {
        let Some(plan) = self.app.borrow().transfer_plan(kind) else {
            return;
        };
        self.close_rename();
        self.close_address_edit();
        let ui = &self.ui;
        ui.set_confirm_moving(kind == JobKind::Move);
        ui.set_confirm_target(plan.target.display().to_string().into());
        ui.set_confirm_items(count(plan.items()));
        ui.set_confirm_problem(TransferProblem::None);
        ui.set_confirm_problem_path(SharedString::new());
        ui.set_confirm_open(true);
        *self.transfer.borrow_mut() = Some(plan);
    }

    /// 확인 줄의 Enter: 작업을 대기열에 넣고 확인 줄을 닫는다. 시작 전에 막히면 작업을 만들지 않고 까닭을 보인다(다음
    /// Enter·Esc로 닫는다).
    fn accept_transfer(&self) {
        let plan = self.transfer.borrow().clone();
        let Some(plan) = plan else {
            self.close_transfer();
            return;
        };
        if self.ui.get_confirm_problem() != TransferProblem::None {
            self.close_transfer();
            return;
        }
        let started = self.app.borrow_mut().start_transfer(&plan);
        match started {
            Ok(_) => {
                self.close_transfer();
                refresh_jobs();
                // 최근 대상이 바뀌었다.
                save_session();
            }
            Err(rejection) => {
                let (problem, path) = transfer_problem(&rejection, &plan);
                self.ui.set_confirm_problem(problem);
                self.ui.set_confirm_problem_path(path.into());
            }
        }
    }

    /// 복사·이동의 대상 선택 창을 연다: 원본 수를 보이고 반대편 패널 위치와 최근 대상을 줄로 보인다. 지금 할 수 없으면
    /// 열지 않는다.
    fn open_target(&self, kind: JobKind) {
        let Some(choice) = self.app.borrow().transfer_choice(kind) else {
            return;
        };
        self.close_rename();
        self.close_address_edit();
        self.close_palette();
        self.close_transfer();
        let ui = &self.ui;
        ui.set_target_moving(kind == JobKind::Move);
        ui.set_target_items(count(choice.sources.len()));
        ui.set_target_text(SharedString::new());
        self.clear_target_problem();
        *self.target.borrow_mut() = Some(TargetChooser::new(choice));
        self.show_target_rows();
        ui.set_target_open(true);
    }

    /// 선택 창을 닫는다(Esc): 아무것도 하지 않는다.
    fn close_target(&self) {
        if self.target.borrow_mut().take().is_some() {
            self.ui.set_target_open(false);
            self.target_rows.set_vec(Vec::new());
            self.ui.set_target_highlight(-1);
        }
    }

    /// 보여 준 까닭을 지운다.
    fn clear_target_problem(&self) {
        let ui = &self.ui;
        ui.set_target_problem(Problem::None);
        ui.set_target_problem_value(SharedString::new());
        ui.set_target_problem_detail(SharedString::new());
        ui.set_target_transfer_problem(TransferProblem::None);
        ui.set_target_transfer_problem_path(SharedString::new());
    }

    /// 선택 창의 줄과 고른 줄을 화면에 넣는다.
    fn show_target_rows(&self) {
        let (rows, highlight) = match self.target.borrow().as_ref() {
            Some(chooser) => (
                chooser
                    .rows()
                    .into_iter()
                    .map(|row| TargetRow {
                        path: row.path.into(),
                        section: match row.section {
                            Section::Opposite => TargetSection::Opposite,
                            Section::Recent => TargetSection::Recent,
                            Section::Completion => TargetSection::Completion,
                        },
                    })
                    .collect::<Vec<_>>(),
                chooser.highlight(),
            ),
            None => (Vec::new(), None),
        };
        if !self.target_rows.iter().eq(rows.iter().cloned()) {
            self.target_rows.set_vec(rows);
        }
        self.ui.set_target_highlight(highlight.map_or(-1, count));
    }

    /// 선택 창의 글자가 바뀌었다: 까닭을 지우고 하위 폴더 후보를 일꾼 스레드에서 새로 읽는다(글자가 비면 반대편
    /// 위치와 최근 대상으로 돌아간다).
    fn target_edited(&self, text: &str) {
        self.clear_target_problem();
        {
            let mut target = self.target.borrow_mut();
            let Some(chooser) = target.as_mut() else {
                return;
            };
            if text.trim().is_empty() {
                chooser.cleared();
            } else {
                let id = self.app.borrow_mut().complete_address(text);
                chooser.requested(id);
            }
        }
        self.show_target_rows();
    }

    /// 선택 창이 기다리는 자동 완성 결과를 받는다(늦게 온 앞선 요청의 결과는 버린다).
    fn receive_target_candidates(
        &self,
        id: facet_core::tab::CompletionId,
        candidates: Option<Vec<facet_core::Candidate>>,
    ) {
        let received = self
            .target
            .borrow_mut()
            .as_mut()
            .is_some_and(|chooser| chooser.receive(id, candidates));
        if received {
            self.show_target_rows();
        }
    }

    /// Tab·화살표로 다음(`forward`)이나 앞 줄을 골라 입력란을 그 줄의 경로로 바꾼다. 입력 커서를 둘 글자 끝의 바이트
    /// 위치를 돌려주고, 고를 줄이 없으면 -1이다.
    fn target_step(&self, forward: bool) -> i32 {
        let row = self
            .target
            .borrow_mut()
            .as_mut()
            .and_then(|chooser| chooser.step(forward));
        let Some(row) = row else {
            return -1;
        };
        self.clear_target_problem();
        self.show_target_rows();
        let end = i32::try_from(row.path.len()).unwrap_or(i32::MAX);
        self.ui.set_target_text(row.path.into());
        end
    }

    /// Enter: 입력란의 글자를 대상 폴더로 해석해 작업을 대기열에 넣는다. 넣었으면 창을 닫고 `true`다. 해석하지
    /// 못했거나 시작 전에 막히면 까닭을 보이고 창은 그대로 두며 `false`다.
    fn target_accept(&self, text: &str) -> bool {
        let Some(kind) = self
            .target
            .borrow()
            .as_ref()
            .map(|chooser| chooser.choice().kind)
        else {
            return true;
        };
        let planned = self.app.borrow().transfer_plan_to(kind, text);
        let plan = match planned {
            Ok(plan) => plan,
            Err(TargetError::Unavailable) => {
                self.close_target();
                return true;
            }
            Err(error) => {
                self.clear_target_problem();
                let (problem, value, detail) = match &error {
                    TargetError::Resolve(error) => address_problem(error),
                    TargetError::NotAFolder { text } => {
                        (Problem::NotAFolder, text.clone(), String::new())
                    }
                    TargetError::Unavailable => (Problem::None, String::new(), String::new()),
                };
                self.ui.set_target_problem(problem);
                self.ui.set_target_problem_value(value.into());
                self.ui.set_target_problem_detail(detail.into());
                return false;
            }
        };
        let started = self.app.borrow_mut().start_transfer(&plan);
        match started {
            Ok(_) => {
                self.close_target();
                refresh_jobs();
                // 최근 대상이 바뀌었다.
                save_session();
                true
            }
            Err(rejection) => {
                self.clear_target_problem();
                let (problem, path) = transfer_problem(&rejection, &plan);
                self.ui.set_target_transfer_problem(problem);
                self.ui.set_target_transfer_problem_path(path.into());
                false
            }
        }
    }

    /// 줄을 눌렀다. 반대편 패널 위치·최근 대상이면 그 폴더로 바로 시작하고(닫았으면 `true`), 자동 완성 후보면 그
    /// 폴더 경로를 입력란에 넣고 그 아래 후보를 읽는다(창은 그대로, `false`).
    fn target_row_pressed(&self, index: i32) -> bool {
        let row = usize::try_from(index).ok().and_then(|index| {
            self.target
                .borrow()
                .as_ref()
                .and_then(|chooser| chooser.row(index))
        });
        let Some(row) = row else {
            return false;
        };
        match row.section {
            Section::Completion => {
                self.ui.set_target_text(row.path.clone().into());
                self.target_edited(&row.path);
                false
            }
            Section::Opposite | Section::Recent => self.target_accept(&row.path),
        }
    }

    /// 확인 줄을 닫는다(Esc): 아무것도 하지 않는다.
    fn close_transfer(&self) {
        self.transfer.borrow_mut().take();
        self.ui.set_confirm_open(false);
    }

    /// 탭에 넘긴 결과 가운데 이 창의 패널에 보이는 탭의 것을 그린다. 결과는 탭 번호로 찾아가므로 다른 창의 탭
    /// 결과는 건너뛴다.
    fn receive(&self, updates: &[(TabId, Update)]) {
        let (visible, active) = {
            let app = self.app.borrow();
            let Some(window) = app.window(self.window) else {
                return;
            };
            let visible: Vec<_> = window
                .panels()
                .iter()
                .map(|panel| panel.active_tab_id())
                .collect();
            (visible, window.active_panel_index())
        };
        let mut committed = [false; MAX_PANELS];
        let mut touched = false;
        let mut edits = Vec::new();
        for (tab, update) in updates {
            let Some(panel) = visible.iter().position(|id| id == tab) else {
                continue;
            };
            touched = true;
            match update {
                Update::Read(ReadOutcome::Committed) => committed[panel] = true,
                Update::Read(ReadOutcome::Failed) if panel == active => self.mark_loaded(),
                Update::Completion { id, candidates } if panel == active => {
                    self.receive_candidates(*id, candidates.clone());
                    self.receive_target_candidates(*id, candidates.clone());
                }
                Update::Edited(result) if panel == active => edits.push((*tab, result)),
                _ => {}
            }
        }
        if !touched {
            return;
        }
        if committed[active] {
            // 다른 위치의 목록이 들어왔으면 이름 입력란은 닫는다.
            self.close_rename();
        }
        self.render();
        if committed[active] {
            self.mark_loaded();
        }
        for (panel, committed) in committed.into_iter().enumerate() {
            if committed {
                self.restore_scroll(panel);
            }
        }
        for (tab, result) in edits {
            self.receive_edited(tab, result);
        }
    }

    /// 읽기 결과(목록이나 오류)가 창에 들어갔음을 측정 모드에 알린다.
    fn mark_loaded(&self) {
        let app = self.app.borrow();
        if let Some(window) = app.window(self.window) {
            measure::mark_loaded(
                measure::window_key(self.window),
                window.active_panel().active_tab().len(),
            );
        }
    }

    /// 활성 패널 번호(0 왼쪽).
    fn active_panel(&self) -> usize {
        let app = self.app.borrow();
        app.window(self.window)
            .map_or(0, |window| window.active_panel_index())
    }

    /// 창(패널이 몇인가, 활성 패널, 분할 비율)과 패널마다 보이는 탭의 상태를 화면에 넣는다.
    fn render(&self) {
        let ui = &self.ui;
        {
            let app = self.app.borrow();
            let Some(window) = app.window(self.window) else {
                return;
            };
            ui.set_split(window.is_split());
            ui.set_active_panel(count(window.active_panel_index()));
            ui.set_split_ratio(window.split_ratio());
            ui.set_folder_title(window.active_panel().active_tab().title().into());
        }
        let now = Instant::now();
        let mut redraw_at: Option<Instant> = None;
        for panel in 0..MAX_PANELS {
            if let Some(at) = self.render_panel(ui, panel, now) {
                redraw_at = Some(redraw_at.map_or(at, |earlier| earlier.min(at)));
            }
        }
        self.follow_rename_row();
        if let Some(at) = redraw_at {
            // 「읽는 중」을 드러낼 때가 오면 다시 그린다.
            let window = self.window;
            self.reading_timer
                .start(TimerMode::SingleShot, at - now, move || {
                    if let Some(view) = view_of(window) {
                        view.render();
                    }
                });
        }
    }

    /// 패널 하나의 보이는 탭을 화면에 넣는다. 그 패널이 없으면(나뉘지 않음) 비운다. 「읽는 중」을 앞으로 드러낼
    /// 때가 있으면 그 시각을 돌려준다.
    fn render_panel(&self, ui: &AppWindow, panel: usize, now: Instant) -> Option<Instant> {
        let models = &self.panels[panel];
        let app = self.app.borrow();
        let Some(shown) = app
            .window(self.window)
            .and_then(|window| window.panels().get(panel))
        else {
            if models.tabs.row_count() > 0 {
                models.tabs.set_vec(Vec::new());
                models.segments.set_vec(Vec::new());
                set_panel_data(ui, panel, PanelData::default());
                models.rows.notify.reset();
            }
            return None;
        };
        self.render_tabs(ui, panel, shown);
        let tab = shown.active_tab();
        let labels: Vec<SharedString> = provider_for(tab.location())
            .segments(tab.location())
            .into_iter()
            .map(|segment| segment.label.into())
            .collect();
        if !models.segments.iter().eq(labels.iter().cloned()) {
            models.segments.set_vec(labels);
        }

        let (address_problem, address_problem_value, address_problem_detail) =
            match tab.address_error() {
                Some(error) => address_problem(error),
                None => (Problem::None, String::new(), String::new()),
            };
        let (list_problem, list_problem_location, list_problem_detail) = match tab.error() {
            Some(error) => list_problem(error),
            None => (Problem::None, String::new(), String::new()),
        };
        let (open_problem, open_problem_detail) = match tab.open_error() {
            Some(error) => open_problem(error),
            None => (Problem::None, String::new()),
        };
        let (relocation, relocation_vanished) = match tab.relocation() {
            Some(Relocation::ToParent { vanished }) => (1, vanished.to_string()),
            Some(Relocation::ToHome { vanished }) => (2, vanished.to_string()),
            None => (0, String::new()),
        };
        let sort = tab.sort();
        let status = tab.status();
        let selected_size = if status.selected_size > 0 {
            file_list::format_size(status.selected_size)
        } else {
            String::new()
        };
        set_panel_data(
            ui,
            panel,
            PanelData {
                address_problem,
                address_problem_value: address_problem_value.into(),
                address_problem_detail: address_problem_detail.into(),
                reading: tab.is_reading(now),
                list_problem,
                list_problem_location: list_problem_location.into(),
                list_problem_detail: list_problem_detail.into(),
                open_problem,
                open_problem_detail: open_problem_detail.into(),
                auto_refresh_off: matches!(tab.auto_refresh(), AutoRefresh::Off(_)),
                relocation,
                relocation_vanished: relocation_vanished.into(),
                sort_column: match sort.column {
                    Column::Name => 0,
                    Column::Modified => 1,
                    Column::Type => 2,
                    Column::Size => 3,
                },
                sort_descending: sort.direction == Direction::Descending,
                item_count: count(status.item_count),
                selected_count: count(status.selected_count),
                selected_size: selected_size.into(),
            },
        );
        let redraw_at = tab.reading_shown_at().filter(|at| *at > now);
        drop(app);
        // 화면은 보이는 행만 다시 묻는다. 행 요소를 새로 만들면 누르던 행의 TouchArea가 사라져 더블 클릭이
        // 이어지지 않으므로, 고르기만 바뀐 때는 reset하지 않는다.
        if !self.keep_rows.get() {
            models.rows.notify.reset();
        }
        redraw_at
    }

    /// 패널의 탭 줄: 제목(폴더 이름), 툴팁(전체 경로), 활성 탭. 바뀔 때만 고쳐 화면 요소를 다시 만들지 않고,
    /// 활성 탭이 줄 밖이면 보이게 넘긴다.
    fn render_tabs(&self, ui: &AppWindow, panel: usize, shown: &Panel) {
        let active = shown.active_index();
        let items: Vec<TabItem> = shown
            .tabs()
            .enumerate()
            .map(|(i, (_, tab))| TabItem {
                title: tab.title().into(),
                path: tab.location().to_string().into(),
                active: i == active,
            })
            .collect();
        let tabs = &self.panels[panel].tabs;
        if !tabs.iter().eq(items.iter().cloned()) {
            tabs.set_vec(items);
        }
        let strip = PanelUi::of(ui, panel);
        let offset = file_list::scroll_to_show(
            active,
            strip.tab_width(),
            -strip.tab_strip_x(),
            strip.tab_strip_width(),
        );
        strip.set_tab_strip_x(-offset);
    }

    /// 패널을 눌렀다: 그 패널을 활성 패널로 한다. 이미 활성이면 아무것도 하지 않는다.
    fn panel_pressed(&self, index: i32) {
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        if index == self.active_panel() {
            return;
        }
        self.remember_scroll();
        if self.app.borrow_mut().activate_panel(index) {
            self.panel_switched();
        }
    }

    /// 활성 패널이 바뀌었다(두 패널의 보이는 탭은 그대로): 이름 입력란·주소창 고쳐 쓰기를 닫고 다시 그린다.
    /// 스크롤 위치는 패널마다 그대로 둔다.
    fn panel_switched(&self) {
        self.close_rename();
        self.close_address_edit();
        self.render();
    }

    /// 패널 사이 경계를 끌었다: 코어가 한도 안으로 자른 분할 비율을 그린다.
    fn split_dragged(&self, ratio: f32) {
        if let Some(window) = self.app.borrow_mut().window_mut(self.window) {
            window.set_split_ratio(ratio);
        }
        self.render();
    }

    /// 패널 사이 경계를 두 번 눌렀다: 반반으로 맞춘다.
    fn split_reset(&self) {
        if let Some(window) = self.app.borrow_mut().window_mut(self.window) {
            window.reset_split_ratio();
        }
        self.render();
    }

    /// 키 입력 하나. 명령이나 커서 이동으로 처리했으면 `true`, 글자 입력이면 `false`다.
    fn key(&self, text: &str, ctrl: bool, alt: bool, shift: bool) -> bool {
        let Some(shortcut) = keys::shortcut_of(text, ctrl, alt, shift) else {
            return false;
        };
        let key = shortcut.key;
        if let Some(command) = command::find_by_shortcut(shortcut) {
            self.execute(command);
            return true;
        }
        if alt {
            return false;
        }
        let movement = match key {
            Key::Up => CursorMove::Up,
            Key::Down => CursorMove::Down,
            Key::PageUp => CursorMove::PageUp(self.page_rows()),
            Key::PageDown => CursorMove::PageDown(self.page_rows()),
            Key::Home => CursorMove::First,
            Key::End => CursorMove::Last,
            _ => return false,
        };
        let index = self
            .app
            .borrow_mut()
            .active_tab_mut()
            .move_cursor_picking(movement, Pick::for_cursor_key(ctrl, shift));
        self.render();
        if let Some(index) = index {
            self.show_row(index);
        }
        true
    }

    /// 명령을 활성 탭에서 실행한다.
    fn execute(&self, command: CommandId) {
        self.remember_scroll();
        let before = self.app.borrow().active_tab_id();
        // 실행취소·다시 실행은 시작하기 전에 대상 작업의 이름을 정해 둔다(시작하면 대상 기록이 목록에서 빠진다).
        let reversal = self.reversal_notice(command);
        let execution = self.app.borrow_mut().execute(command);
        // 창이 생기거나 닫혔으면 화면을 맞추고(창이 남지 않았으면 앱을 끝낸다), 다른 창도 다시 그린다(숨김 항목
        // 보이기처럼 앱 전체에 듣는 명령이 있다).
        sync_windows();
        for view in views() {
            if view.window != self.window {
                view.render();
            }
        }
        if self.app.borrow().window(self.window).is_none() {
            // 이 창의 마지막 탭을 닫아 창이 닫혔다.
            return;
        }
        if self.app.borrow().active_window_id() != self.window {
            // 새 창이 활성 창이 되었다. 이 창은 그대로 그린다.
            self.render();
            return;
        }
        if command == CommandId::SwitchPanel && execution == Execution::Done {
            self.panel_switched();
        } else if self.app.borrow().active_tab_id() != before {
            self.tab_switched();
        } else {
            self.render();
        }
        match execution {
            Execution::EditAddress => self.begin_address_edit(),
            Execution::ShowCommandPalette => self.open_palette(),
            Execution::ConfirmTransfer(kind) => self.open_transfer(kind),
            Execution::ChooseTransferTarget(kind) => self.open_target(kind),
            Execution::Rename => self.begin_rename(),
            Execution::NewFolder => {
                // 만든 폴더는 결과를 받을 때 목록에 들어가 선택·커서가 되고, 그 행에 이름 입력란이 열린다
                // (`receive_edited`).
                self.close_rename();
                let tab = self.app.borrow().active_tab_id();
                let started = self.app.borrow_mut().new_folder(text::NEW_FOLDER_BASE);
                match started {
                    Ok(()) => self.new_folder_tab.set(Some(tab)),
                    Err(error) => self
                        .ui
                        .set_notice(text::new_folder_error_text(error).into()),
                }
            }
            Execution::UndoRefused(UndoError::NotUndoable(_)) | Execution::Done => {
                if let Some(notice) = reversal.and_then(|(started, refused)| match execution {
                    Execution::Done => Some(started),
                    _ => refused,
                }) {
                    self.ui.set_notice(notice.into());
                }
            }
            Execution::ShowHistory => self.open_history(),
            Execution::UndoRefused(_) | Execution::Disabled | Execution::WindowClosed => {}
        }
    }

    /// 실행취소·다시 실행이면 상태 줄 글자: 시작하면 보일 「〈작업 이름〉 실행취소」와, 마지막 작업이 되돌릴 수 없어
    /// 거절되면 보일 까닭. 다른 명령이거나 대상이 없으면 `None`.
    fn reversal_notice(&self, command: CommandId) -> Option<(String, Option<String>)> {
        let app = self.app.borrow();
        let history = app.history();
        match command {
            CommandId::Undo => history.undo_target().map(|record| {
                (
                    text::undo_notice(record),
                    record
                        .not_undoable_reason()
                        .map(|reason| text::undo_refused_text(record, reason)),
                )
            }),
            CommandId::Redo => history
                .redo_target()
                .map(|record| (text::redo_notice(record), None)),
            _ => None,
        }
    }

    /// 작업 기록 보기를 연다: 지금 작업 기록의 줄(최근 것부터)을 창 위에 겹쳐 보인다. 열 때의 기록을 보인다.
    fn open_history(&self) {
        self.close_rename();
        self.close_address_edit();
        let rows: Vec<HistoryRow> = history_view::lines(self.app.borrow().history())
            .into_iter()
            .map(|line| HistoryRow {
                name: line.name.into(),
                completed: count64(line.completed),
                skipped: count64(line.skipped),
                not_undoable: line.not_undoable.unwrap_or_default().into(),
            })
            .collect();
        self.ui
            .set_history_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
        self.ui.set_history_open(true);
    }

    /// 작업 기록 보기를 닫는다.
    fn close_history(&self) {
        self.ui.set_history_open(false);
    }

    /// 활성 탭이 바뀌었다(탭 바꾸기·열기·닫기, 패널 나누기·합치기): 이름 입력란·주소창 고쳐 쓰기를 닫고, 패널마다
    /// 보이는 탭이 갖고 있던 목록과 스크롤 위치를 바로 그린다. 다시 읽은 결과는 나중에 와서 바꾼다.
    fn tab_switched(&self) {
        self.close_rename();
        self.close_address_edit();
        self.close_target();
        self.render();
        for panel in 0..MAX_PANELS {
            self.restore_scroll(panel);
        }
    }

    /// 탭 줄의 `index`번째 탭을 눌렀다: 그 탭으로 간다.
    fn tab_pressed(&self, index: i32) {
        let id = {
            let app = self.app.borrow();
            let Some(window) = app.window(self.window) else {
                return;
            };
            let panel = window.active_panel();
            usize::try_from(index)
                .ok()
                .and_then(|index| panel.tabs().nth(index))
                .map(|(id, _)| id)
        };
        let Some(id) = id else { return };
        if id == self.app.borrow().active_tab_id() {
            return;
        }
        self.remember_scroll();
        self.app.borrow_mut().activate_tab(id);
        self.tab_switched();
    }

    /// 패널 `panel`의 `index`번째 탭을 눌렀다: 끌기를 기다린다. 놓을 때까지 움직이지 않으면 누르기다.
    fn tab_drag_start(&self, panel: i32, index: i32, at: Point) {
        let (Ok(panel), Ok(index)) = (usize::try_from(panel), usize::try_from(index)) else {
            return;
        };
        // 누른 점이 탭의 왼쪽 위에서 떨어진 거리: 창 밖에 놓아 새 창을 띄울 때 탭이 손 아래 오게 한다.
        let grab = self.tab_strips()[..].get(panel).copied().flatten().map_or(
            Point { x: 0.0, y: 0.0 },
            |strip| {
                let origin = strip.tab_origin(index);
                Point {
                    x: at.x - origin.x,
                    y: at.y - origin.y,
                }
            },
        );
        *self.tab_drag.borrow_mut() = Some(TabDrag::new(self.window, panel, index, at, grab));
    }

    /// 누른 탭을 끌고 있다: 끌기 시작 문턱을 넘었으면 끄는 탭을 흐리게 하고 놓일 자리(이 창이든 다른 창이든)를
    /// 보인다. 창 밖으로 끌어도 창이 마우스를 붙잡고 있어 이 창 기준 점(음수나 창보다 큰 값)으로 온다.
    fn tab_drag_move(&self, at: Point) {
        let shown = {
            let mut drag = self.tab_drag.borrow_mut();
            let Some(drag) = drag.as_mut() else { return };
            drag.moved(at, || self.hover(at));
            drag.is_dragging().then(|| drag.clone())
        };
        if let Some(drag) = shown {
            show_tab_drag(Some(&drag));
        }
    }

    /// 누른 탭을 놓았다. 끌기 시작 문턱을 넘지 않았으면 누르기라 참이다(화면이 그 탭으로 간다). 끌었으면 놓인 곳으로
    /// 탭을 옮기고(탭 줄이면 그 자리, facet의 어느 창 위도 아니면 새 창, 창 위의 탭 줄 밖이면 그대로) 거짓이다.
    fn tab_drag_end(&self, at: Point) -> bool {
        let Some(mut drag) = self.tab_drag.borrow_mut().take() else {
            return true;
        };
        drag.moved(at, || self.hover(at));
        show_tab_drag(None);
        if !drag.is_dragging() {
            return true;
        }
        self.drop_tab(&drag);
        false
    }

    /// 끌기가 끊겼다(창이 마우스를 놓침 등): 아무것도 옮기지 않는다.
    fn tab_drag_cancel(&self) {
        if self.tab_drag.borrow_mut().take().is_some() {
            show_tab_drag(None);
        }
    }

    /// 패널마다 탭 줄의 자리와 탭 수. 없는 패널(나뉘지 않음)은 `None`이다.
    fn tab_strips(&self) -> [Option<Strip>; MAX_PANELS] {
        let app = self.app.borrow();
        let panels = app.window(self.window).map(|window| window.panels());
        std::array::from_fn(|panel| {
            let tabs = panels?.get(panel)?.len();
            Some(PanelUi::of(&self.ui, panel).tab_strip(tabs))
        })
    }

    /// 이 창의 Windows 창 핸들 값. 아직 화면에 없으면 `None`이다.
    #[cfg(windows)]
    fn handle(&self) -> Option<std::num::NonZeroIsize> {
        use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        self.ui
            .window()
            .with_winit_window(|winit| match winit.window_handle().ok()?.as_raw() {
                RawWindowHandle::Win32(handle) => Some(handle.hwnd),
                _ => None,
            })
            .flatten()
    }

    /// 이 창의 화면 자리와 배율. 아직 화면에 없으면 `None`이다.
    fn frame(&self) -> Option<Frame> {
        let window = self.ui.window();
        let scale = window.scale_factor();
        window
            .with_winit_window(|winit| {
                let client = winit.inner_position().ok()?;
                let outer = winit.outer_position().ok()?;
                Some(Frame {
                    client: ScreenPoint {
                        x: f64::from(client.x),
                        y: f64::from(client.y),
                    },
                    outer: ScreenPoint {
                        x: f64::from(outer.x),
                        y: f64::from(outer.y),
                    },
                    scale,
                })
            })
            .flatten()
    }

    /// 이 창 기준 점 `at` 아래: facet의 어느 창(그 창 기준 점과 탭 줄)인가, 아니면 facet 창 밖인가. 창 자리를 모르면
    /// 이 창 안으로 본다.
    fn hover(&self, at: Point) -> Hover<WindowId> {
        let here = || Hover::Window {
            window: self.window,
            at,
            strips: self.tab_strips().to_vec(),
        };
        let Some(frame) = self.frame() else {
            return here();
        };
        let screen = frame.to_screen(at);
        let Some(view) = view_under(screen) else {
            return Hover::Outside(screen);
        };
        if view.window == self.window {
            return here();
        }
        let Some(other) = view.frame() else {
            return Hover::Outside(screen);
        };
        Hover::Window {
            window: view.window,
            at: other.to_window(screen),
            strips: view.tab_strips().to_vec(),
        }
    }

    /// 끈 탭을 놓은 곳으로 옮긴다(코어의 탭 옮기기). 옮긴 탭이 목적지 패널의 활성 탭, 그 패널이 활성 패널, 그 창이
    /// 활성 창이 된다. 보이던 탭은 다시 읽지 않고 갖고 있던 목록을 그대로 보인다. 다른 창으로 옮겼으면 그 창을 앞으로
    /// 가져오고, facet 창 밖이면 놓은 자리에 이 창 크기로 새 창을 띄운다(`sync_windows`가 띄울 때 놓는다).
    fn drop_tab(&self, drag: &TabDrag<WindowId>) {
        let Some(target) = drag.target() else { return };
        let id = {
            let app = self.app.borrow();
            app.window(self.window)
                .and_then(|window| window.panels().get(drag.panel))
                .and_then(|panel| panel.tabs().nth(drag.index))
                .map(|(id, _)| id)
        };
        let Some(id) = id else { return };
        self.remember_scroll();
        match target {
            Target::Strip {
                window,
                panel,
                slot,
            } => {
                let other = (window != self.window).then(|| view_of(window)).flatten();
                if let Some(other) = &other {
                    other.remember_scroll();
                }
                let destination = TabDestination::Panel {
                    window,
                    panel,
                    slot,
                };
                if self.app.borrow_mut().move_tab(id, destination).is_none() {
                    return;
                }
                if let Some(other) = other {
                    other.tab_switched();
                    other
                        .ui
                        .window()
                        .with_winit_window(|winit| winit.focus_window());
                }
            }
            Target::NewWindow(at) => {
                let first_tab =
                    self.tab_strips()[0].map_or(Point { x: 0.0, y: 0.0 }, |strip| Point {
                        x: strip.left,
                        y: strip.top,
                    });
                let placement = self.frame().map(|frame| Placement {
                    origin: drag.new_window_origin(at, &frame, first_tab),
                    size: self.ui.window().size(),
                });
                let Some(moved) = self
                    .app
                    .borrow_mut()
                    .move_tab(id, TabDestination::NewWindow)
                else {
                    return;
                };
                if let Some(placement) = placement {
                    PLACEMENTS.with(|slot| slot.borrow_mut().push((moved.window, placement)));
                }
            }
        }
        if self.app.borrow().window(self.window).is_some() {
            // 떠난 창이 남아 있다(탭이 남음): 이웃 탭이 보인다.
            self.tab_switched();
        }
    }

    /// 행을 휠 클릭했다: 폴더면 새 탭에서 연다. 지금 탭은 그대로다.
    fn row_middle_pressed(&self, row: i32) {
        let Ok(index) = usize::try_from(row) else {
            return;
        };
        if self.app.borrow_mut().open_in_new_tab(index).is_some() {
            self.render();
        }
    }

    /// 명령 팔레트를 연다. 모든 명령이 보이고 첫 줄이 골라진다.
    fn open_palette(&self) {
        let ui = &self.ui;
        self.close_rename();
        self.close_address_edit();
        *self.palette.borrow_mut() = Some(Palette::default());
        self.palette_edited("");
        ui.set_palette_text(SharedString::new());
        ui.set_palette_open(true);
    }

    fn close_palette(&self) {
        self.palette.borrow_mut().take();
        self.ui.set_palette_open(false);
    }

    /// 검색어가 바뀌었다: 코어의 명령 팔레트 검색으로 다시 찾는다. 앱은 찾지 않는다.
    fn palette_edited(&self, query: &str) {
        let entries = self.app.borrow().search_commands(query, text::command_name);
        let rows: Vec<PaletteRow> = entries
            .iter()
            .map(|entry| PaletteRow {
                name: text::command_name(entry.command).into(),
                shortcut: entry
                    .shortcuts
                    .first()
                    .map(text::shortcut_text)
                    .unwrap_or_default()
                    .into(),
                enabled: entry.enabled,
            })
            .collect();
        let highlight = if entries.is_empty() { None } else { Some(0) };
        self.palette_rows.set_vec(rows);
        if let Some(palette) = self.palette.borrow_mut().as_mut() {
            palette.set(entries, highlight);
        }
        self.show_palette_highlight();
    }

    fn palette_step(&self, forward: bool) {
        if let Some(palette) = self.palette.borrow_mut().as_mut() {
            palette.step(forward);
        }
        self.show_palette_highlight();
    }

    fn show_palette_highlight(&self) {
        let ui = &self.ui;
        let highlight = self.palette.borrow().as_ref().and_then(Palette::highlight);
        ui.set_palette_highlight(highlight.map_or(-1, count));
    }

    /// `index`번 줄(음수면 고른 줄)의 명령을 실행한다. 실행할 수 없는 명령이면 팔레트를 그대로 두고 `false`다.
    fn palette_run(&self, index: i32) -> bool {
        let command = {
            let palette = self.palette.borrow();
            let Some(palette) = palette.as_ref() else {
                return false;
            };
            let index = usize::try_from(index).ok().or(palette.highlight());
            index.and_then(|index| palette.command_at(index))
        };
        let Some(command) = command else {
            return false;
        };
        self.close_palette();
        self.execute(command);
        true
    }

    /// 활성 탭을 `location`으로 옮기기 시작한다.
    fn go(&self, location: Location) {
        self.remember_scroll();
        self.app.borrow_mut().navigate(Navigation::Go(location));
        self.render();
    }

    /// 경로 조각을 눌렀다: 그 위치로 간다.
    fn segment_pressed(&self, index: i32) {
        let location = {
            let app = self.app.borrow();
            let location = app.active_tab().location();
            let segments = provider_for(location).segments(location);
            usize::try_from(index)
                .ok()
                .and_then(|index| segments.into_iter().nth(index))
                .map(|segment| segment.location)
        };
        if let Some(location) = location {
            self.go(location);
        }
    }

    /// 주소창을 글자로 고쳐 쓰는 모드로 바꾼다. 지금 위치를 채우고, 화면이 전체를 선택한다.
    fn begin_address_edit(&self) {
        let ui = &self.ui;
        // 이름 입력란과 함께 열지 않는다.
        self.close_rename();
        let text = self.app.borrow().active_tab().location().to_string();
        *self.address.borrow_mut() = Some(AddressEdit::default());
        self.show_candidates();
        ui.set_address_text(text.into());
        ui.set_address_editing(true);
    }

    /// 고쳐 쓰는 모드를 닫고 경로 조각 모드로 돌아간다.
    fn close_address_edit(&self) {
        self.address.borrow_mut().take();
        self.show_candidates();
        self.ui.set_address_editing(false);
    }

    /// 고쳐 쓰는 글자가 바뀌었다: 자동 완성 후보를 일꾼 스레드에서 새로 읽는다.
    fn address_edited(&self, text: &str) {
        {
            let mut address = self.address.borrow_mut();
            let Some(address) = address.as_mut() else {
                return;
            };
            let id = self.app.borrow_mut().complete_address(text);
            address.request(id);
        }
        self.show_candidates();
    }

    fn receive_candidates(
        &self,
        id: facet_core::tab::CompletionId,
        candidates: Option<Vec<facet_core::Candidate>>,
    ) {
        let received = self
            .address
            .borrow_mut()
            .as_mut()
            .is_some_and(|address| address.receive(id, candidates));
        if received {
            self.show_candidates();
        }
    }

    /// 자동 완성 후보와 고른 후보를 화면에 넣는다.
    fn show_candidates(&self) {
        let ui = &self.ui;
        let address = self.address.borrow();
        let (names, highlight) = match address.as_ref() {
            Some(address) => (
                address
                    .candidates()
                    .iter()
                    .map(|c| SharedString::from(c.name.to_string_lossy().as_ref()))
                    .collect(),
                address.highlight(),
            ),
            None => (Vec::new(), None),
        };
        if !self.candidates.iter().eq(names.iter().cloned()) {
            self.candidates.set_vec(names);
            ui.set_candidate_content_y(0.0);
        }
        ui.set_candidate_highlight(highlight.map_or(-1, count));
        if let Some(index) = highlight {
            let offset = file_list::scroll_to_show(
                index,
                ui.get_candidate_row_height(),
                -ui.get_candidate_content_y(),
                ui.get_candidate_view_height(),
            );
            ui.set_candidate_content_y(-offset);
        }
    }

    /// Tab·화살표로 다음(`forward`)이나 앞 후보를 골라 고쳐 쓰는 글자를 그 후보의 경로로 바꾼다. 입력 커서를 둘
    /// 글자 끝의 바이트 위치를 돌려주고, 고를 후보가 없으면 -1이다.
    fn address_step(&self, forward: bool) -> i32 {
        let ui = &self.ui;
        let text = {
            let mut address = self.address.borrow_mut();
            let Some(candidate) = address.as_mut().and_then(|a| a.step(forward)) else {
                return -1;
            };
            candidate.location.to_string()
        };
        self.show_candidates();
        let end = i32::try_from(text.len()).unwrap_or(i32::MAX);
        ui.set_address_text(text.into());
        end
    }

    /// Enter: 고쳐 쓴 글자로 간다. 해석했으면 고쳐 쓰는 모드를 닫고 `true`다. 해석하지 못했으면 모드를 그대로
    /// 두어 고칠 수 있게 하고, 까닭을 보이고 `false`다. 탭은 원래 위치에 남는다.
    fn address_go(&self, text: &str) -> bool {
        self.remember_scroll();
        let went = self.app.borrow_mut().go_to_text(text);
        if went {
            self.close_address_edit();
        }
        self.render();
        went
    }

    /// 자동 완성 후보를 눌렀다: 그 폴더로 간다.
    fn candidate_pressed(&self, index: i32) {
        let location = self.address.borrow().as_ref().and_then(|address| {
            let index = usize::try_from(index).ok()?;
            Some(address.candidate(index)?.location.clone())
        });
        self.close_address_edit();
        if let Some(location) = location {
            self.go(location);
        }
    }

    /// 글자로 이동: IME가 조합을 끝낸 글자들.
    fn typed(&self, text: &str) {
        let now = Instant::now();
        let mut found = None;
        {
            let mut app = self.app.borrow_mut();
            let tab = app.active_tab_mut();
            for c in text.chars() {
                found = tab.type_to_select(c, now).or(found);
            }
        }
        self.render();
        if let Some(index) = found {
            self.show_row(index);
        }
    }

    fn row_pressed(&self, row: i32, pick: Pick) {
        let Ok(index) = usize::try_from(row) else {
            return;
        };
        let panel = self.active_panel();
        let before = self.row_marks();
        self.app.borrow_mut().active_tab_mut().pick(index, pick);
        let after = self.row_marks();
        // 목록은 그대로고 선택·커서만 바뀌었으니 바뀐 행만 알린다. 행 요소가 그대로 남아야 더블 클릭이 된다.
        if before.len() == after.len() {
            self.keep_rows.set(true);
            self.render();
            self.keep_rows.set(false);
            for (i, (a, b)) in before.iter().zip(&after).enumerate() {
                if a != b {
                    self.panels[panel].rows.notify.row_changed(i);
                }
            }
        } else {
            self.render();
        }
    }

    /// 활성 탭의 행마다 (선택됨, 커서).
    fn row_marks(&self) -> Vec<(bool, bool)> {
        let app = self.app.borrow();
        let tab = app.active_tab();
        let cursor = tab.cursor();
        (0..tab.len())
            .map(|i| (tab.is_selected(i), cursor == Some(i)))
            .collect()
    }

    /// 행을 두 번 눌렀다: 그 행을 고르고 연다(폴더면 들어가고 파일이면 기본 프로그램으로).
    fn row_opened(&self, row: i32) {
        let Ok(index) = usize::try_from(row) else {
            return;
        };
        self.app
            .borrow_mut()
            .active_tab_mut()
            .pick(index, Pick::Only);
        self.execute(CommandId::Open);
    }

    /// 활성 패널 한 쪽의 행 수.
    fn page_rows(&self) -> usize {
        let ui = &self.ui;
        let row_height = ui.get_row_height();
        if row_height > 0.0 {
            (PanelUi::of(ui, self.active_panel()).view_height() / row_height) as usize
        } else {
            1
        }
    }

    /// 패널마다 맨 위에 보이는 행 번호를 그 패널의 보이는 탭에 알린다.
    fn remember_scroll(&self) {
        let ui = &self.ui;
        let row_height = ui.get_row_height();
        if row_height <= 0.0 {
            return;
        }
        let mut app = self.app.borrow_mut();
        let Some(window) = app.window_mut(self.window) else {
            return;
        };
        for panel in 0..MAX_PANELS {
            if let Some(shown) = window.panel_mut(panel) {
                let content_y = PanelUi::of(ui, panel).content_y();
                let top = (-content_y / row_height).max(0.0) as usize;
                shown.active_tab_mut().set_scroll(top);
            }
        }
    }

    /// 패널의 보이는 탭이 기억한 스크롤 위치로 가고(뒤로 가면 되살아난다), 활성 패널이면 커서 항목을 보인다.
    fn restore_scroll(&self, panel: usize) {
        let ui = &self.ui;
        let (scroll, cursor, active) = {
            let app = self.app.borrow();
            let Some(window) = app.window(self.window) else {
                return;
            };
            let Some(shown) = window.panels().get(panel) else {
                return;
            };
            let tab = shown.active_tab();
            (
                tab.scroll(),
                tab.cursor(),
                window.active_panel_index() == panel,
            )
        };
        PanelUi::of(ui, panel).set_content_y(-(scroll as f32 * ui.get_row_height()));
        if active && let Some(index) = cursor {
            self.show_row(index);
        }
    }

    /// 활성 패널에서 `index`번 행이 화면 밖이면 그 행이 보이게 스크롤한다.
    fn show_row(&self, index: usize) {
        let ui = &self.ui;
        let list = PanelUi::of(ui, self.active_panel());
        let offset = file_list::scroll_to_show(
            index,
            ui.get_row_height(),
            -list.content_y(),
            list.view_height(),
        );
        list.set_content_y(-offset);
    }

    /// F2: 커서 항목(선택이 여럿이어도 커서 항목 하나)의 이름 입력란을 연다. 파일은 확장자 앞까지 고른 채 연다.
    fn begin_rename(&self) {
        let opened = {
            let app = self.app.borrow();
            let tab = app.active_tab();
            tab.cursor().and_then(|index| {
                let item = tab.get(index)?;
                Some((index, Rename::open(&item.name, item.is_folder, false)))
            })
        };
        if let Some((index, rename)) = opened {
            let text = rename.original().to_owned();
            self.open_rename(index, rename, &text);
        }
    }

    /// 새 폴더를 만들었다: 그 행에 이름 입력란을 전체 선택으로 연다. 그만두면 「새 폴더」로 남는다.
    fn begin_new_folder_rename(&self, edited: &Edited) {
        let Some(name) = edited.name() else {
            return;
        };
        let Some(index) = self.row_of(name) else {
            return;
        };
        let rename = Rename::open(name, true, true);
        let text = rename.original().to_owned();
        self.open_rename(index, rename, &text);
    }

    /// `index`번 행에 `text`를 채운 이름 입력란을 연다(다른 입력이 열려 있으면 열지 않는다).
    fn open_rename(&self, index: usize, rename: Rename, text: &str) {
        let ui = &self.ui;
        if self.address.borrow().is_some()
            || ui.get_palette_open()
            || ui.get_target_open()
            || ui.get_confirm_open()
        {
            return;
        }
        // 입력란이 화면 밖 행에 열리지 않게 먼저 그 행을 보인다.
        self.show_row(index);
        let (start, end) = rename.selection(text);
        ui.set_edit_text(text.into());
        ui.set_edit_selection_start(i32::try_from(start).unwrap_or(0));
        ui.set_edit_selection_end(i32::try_from(end).unwrap_or(0));
        ui.set_notice(SharedString::new());
        // 다시 열 때 입력란 요소를 새로 만들게(선택·포커스) 먼저 닫는다.
        ui.set_editing(-1);
        *self.rename.borrow_mut() = Some(rename);
        self.render_rename();
        ui.set_editing(i32::try_from(index).unwrap_or(i32::MAX));
    }

    /// 이름 입력란 곁의 까닭과 확장자 확인을 화면에 넣는다.
    fn render_rename(&self) {
        let ui = &self.ui;
        let rename = self.rename.borrow();
        let problem = rename
            .as_ref()
            .and_then(Rename::problem)
            .map(text::rename_problem_text)
            .unwrap_or_default();
        ui.set_edit_problem(problem.into());
        ui.set_edit_asking_extension(rename.as_ref().is_some_and(Rename::asking_extension));
    }

    /// 활성 탭에서 이름이 `name`인 행.
    fn row_of(&self, name: &std::ffi::OsStr) -> Option<usize> {
        self.app
            .borrow()
            .active_tab()
            .items()
            .position(|item| item.name == name)
    }

    /// 목록이 바뀌었다(정렬·변경 감시): 열린 이름 입력란을 그 항목의 지금 행으로 옮기고, 항목이 사라졌으면 닫는다.
    fn follow_rename_row(&self) {
        let name = match self.rename.borrow().as_ref() {
            Some(rename) if rename.pending().is_none() => rename.original_name().to_owned(),
            _ => return,
        };
        match self.row_of(&name) {
            Some(index) => {
                let index = i32::try_from(index).unwrap_or(i32::MAX);
                if self.ui.get_editing() != index {
                    self.ui.set_editing(index);
                }
            }
            None => self.close_rename(),
        }
    }

    /// Enter·입력란 밖 클릭: 확정한다. 입력란을 닫았으면 `true`, 막혀 남았으면 `false`.
    fn confirm_rename(&self, text: &str) -> bool {
        let decision = match self.rename.borrow_mut().as_mut() {
            Some(rename) if rename.pending().is_none() => rename.confirm(text),
            _ => return true,
        };
        self.decide_rename(decision, text)
    }

    /// 확장자 바꿈 확인의 답. 입력란을 닫았으면 `true`.
    fn answer_extension(&self, yes: bool) -> bool {
        let text = self.ui.get_edit_text().to_string();
        let decision = match self.rename.borrow_mut().as_mut() {
            Some(rename) if rename.pending().is_none() => rename.answer_extension(yes, &text),
            _ => return true,
        };
        self.decide_rename(decision, &text)
    }

    /// 확정의 결과대로 한다: 닫기, 남기기(까닭·확인을 보임), 코어 이름바꾸기에 넘기기(UI 스레드는 기다리지 않는다).
    fn decide_rename(&self, decision: Confirm, text: &str) -> bool {
        match decision {
            Confirm::Close => {
                self.close_rename();
                true
            }
            Confirm::Stay | Confirm::AskExtension => {
                self.render_rename();
                false
            }
            Confirm::Submit {
                name,
                after_new_folder,
            } => {
                let Some(original) = self
                    .rename
                    .borrow()
                    .as_ref()
                    .map(|rename| rename.original_name().to_owned())
                else {
                    return true;
                };
                let result = {
                    let mut app = self.app.borrow_mut();
                    if after_new_folder {
                        app.rename_new_folder(&original, &name)
                    } else {
                        app.rename(&original, &name)
                    }
                };
                match result {
                    Ok(()) => {
                        if let Some(rename) = self.rename.borrow_mut().as_mut() {
                            rename.submitted(&name);
                        }
                        self.ui.set_editing(-1);
                        self.render_rename();
                        true
                    }
                    Err(EditError::Unchanged) => {
                        self.close_rename();
                        true
                    }
                    Err(error) => {
                        if let Some(rename) = self.rename.borrow_mut().as_mut() {
                            rename.failed(error, text);
                        }
                        self.render_rename();
                        false
                    }
                }
            }
        }
    }

    /// 이름 입력란의 글자가 바뀌었다: 이름 확인을 다시 한다.
    fn rename_edited(&self, text: &str) {
        if let Some(rename) = self.rename.borrow_mut().as_mut() {
            rename.edited(text);
        }
        self.render_rename();
    }

    /// Esc: 그만둔다(새 폴더 직후면 「새 폴더」로 남는다). 확장자를 묻는 중이면 입력란으로 돌아가고 `false`.
    fn cancel_rename(&self) -> bool {
        let closes = self.rename.borrow_mut().as_mut().is_none_or(Rename::cancel);
        if closes {
            self.close_rename();
        } else {
            self.render_rename();
        }
        closes
    }

    /// 이름 입력란을 닫는다. 결과를 기다리던 이름바꾸기도 잊는다(실패하면 상태 줄에 까닭이 보인다).
    fn close_rename(&self) {
        if self.rename.borrow_mut().take().is_some() {
            self.ui.set_editing(-1);
            self.render_rename();
        }
    }

    /// 이름바꾸기·새 폴더의 결과가 활성 패널의 탭에 왔다. 코어가 이미 목록에 반영해 새 이름이 선택·커서다.
    fn receive_edited(&self, tab: TabId, result: &Result<Edited, EditError>) {
        let pending = self
            .rename
            .borrow()
            .as_ref()
            .filter(|rename| rename.pending().is_some())
            .map(|rename| rename.original_name().to_owned());
        let is_rename = pending.is_some()
            && result
                .as_ref()
                .map_or(self.new_folder_tab.get() != Some(tab), |edited| {
                    edited.kind == EditKind::Rename
                });
        if let Some(original) = pending.filter(|_| is_rename) {
            match result {
                Ok(_) => {
                    self.rename.borrow_mut().take();
                    self.render_rename();
                    let cursor = self.app.borrow().active_tab().cursor();
                    if let Some(index) = cursor {
                        self.show_row(index);
                    }
                }
                Err(error) => match self.row_of(&original) {
                    // 까닭을 두고 친 이름으로 입력란을 다시 연다.
                    Some(index) => {
                        let reopened = self.rename.borrow_mut().take().map(|mut rename| {
                            let text = rename.failed(*error, "");
                            (rename, text)
                        });
                        if let Some((rename, text)) = reopened {
                            self.open_rename(index, rename, &text);
                        }
                    }
                    None => {
                        self.close_rename();
                        self.ui.set_notice(text::rename_error_text(*error).into());
                    }
                },
            }
            return;
        }
        if self.new_folder_tab.get() == Some(tab) {
            self.new_folder_tab.set(None);
            match result {
                Ok(edited) if edited.kind == EditKind::NewFolder => {
                    self.begin_new_folder_rename(edited)
                }
                Ok(_) => {}
                Err(error) => self
                    .ui
                    .set_notice(text::new_folder_error_text(*error).into()),
            }
            return;
        }
        if let Err(error) = result {
            self.ui.set_notice(text::rename_error_text(*error).into());
        }
    }
}

fn count(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// 패널 하나(0 왼쪽, 1 오른쪽)의 화면 값: 목록 스크롤과 탭 줄. 창의 `left-*`·`right-*` 속성을 고른다.
struct PanelUi<'a> {
    ui: &'a AppWindow,
    right: bool,
}

impl<'a> PanelUi<'a> {
    fn of(ui: &'a AppWindow, panel: usize) -> Self {
        PanelUi {
            ui,
            right: panel == 1,
        }
    }

    fn content_y(&self) -> f32 {
        if self.right {
            self.ui.get_right_content_y()
        } else {
            self.ui.get_left_content_y()
        }
    }

    fn set_content_y(&self, y: f32) {
        if self.right {
            self.ui.set_right_content_y(y);
        } else {
            self.ui.set_left_content_y(y);
        }
    }

    fn view_height(&self) -> f32 {
        if self.right {
            self.ui.get_right_view_height()
        } else {
            self.ui.get_left_view_height()
        }
    }

    fn tab_width(&self) -> f32 {
        if self.right {
            self.ui.get_right_tab_width()
        } else {
            self.ui.get_left_tab_width()
        }
    }

    fn tab_strip_x(&self) -> f32 {
        if self.right {
            self.ui.get_right_tab_strip_x()
        } else {
            self.ui.get_left_tab_strip_x()
        }
    }

    fn set_tab_strip_x(&self, x: f32) {
        if self.right {
            self.ui.set_right_tab_strip_x(x);
        } else {
            self.ui.set_left_tab_strip_x(x);
        }
    }

    fn tab_strip_width(&self) -> f32 {
        if self.right {
            self.ui.get_right_tab_strip_width()
        } else {
            self.ui.get_left_tab_strip_width()
        }
    }

    /// 탭 줄의 창 기준 자리·크기와 넘긴 거리. 탭 수는 부르는 쪽이 준다.
    fn tab_strip(&self, tabs: usize) -> Strip {
        let ui = self.ui;
        let (left, top, height) = if self.right {
            (
                ui.get_right_tab_strip_left(),
                ui.get_right_tab_strip_top(),
                ui.get_right_tab_strip_height(),
            )
        } else {
            (
                ui.get_left_tab_strip_left(),
                ui.get_left_tab_strip_top(),
                ui.get_left_tab_strip_height(),
            )
        };
        Strip {
            left,
            top,
            width: self.tab_strip_width(),
            height,
            tab_width: self.tab_width(),
            scroll: -self.tab_strip_x(),
            tabs,
        }
    }

    /// 탭 끌기 표시: 끄는 탭과 놓일 자리(없으면 -1).
    fn set_tab_drag(&self, dragged: i32, slot: i32) {
        if self.right {
            self.ui.set_right_dragged_tab(dragged);
            self.ui.set_right_drop_slot(slot);
        } else {
            self.ui.set_left_dragged_tab(dragged);
            self.ui.set_left_drop_slot(slot);
        }
    }
}

/// 패널 하나의 수치·알림을 화면에 넣는다.
fn set_panel_data(ui: &AppWindow, panel: usize, data: PanelData) {
    if panel == 1 {
        ui.set_right_data(data);
    } else {
        ui.set_left_data(data);
    }
}

/// 목록 읽기 오류를 화면의 종류와 값으로. 문장은 화면이 만든다.
fn list_problem(error: &ListError) -> (Problem, String, String) {
    let location = error.location().to_string();
    match error {
        ListError::NotFound { .. } => (Problem::NotFound, location, String::new()),
        ListError::PermissionDenied { .. } => (Problem::PermissionDenied, location, String::new()),
        ListError::NotAFolder { .. } => (Problem::NotAFolder, location, String::new()),
        ListError::Io { source, .. } => (Problem::Io, location, source.to_string()),
    }
}

/// 주소창 글자 해석 오류를 화면의 종류와 값·자세한 내용으로. 문장은 화면이 만든다.
fn address_problem(error: &ResolveError) -> (Problem, String, String) {
    let none = String::new;
    match error {
        ResolveError::Empty => (Problem::AddressEmpty, none(), none()),
        ResolveError::InvalidCharacter { character } => (
            Problem::AddressInvalidCharacter,
            format!("{character:?}"),
            none(),
        ),
        ResolveError::UndefinedVariable { name } => (
            Problem::AddressUndefinedVariable,
            format!("%{name}%"),
            none(),
        ),
        ResolveError::NotAbsolute { text } => (Problem::AddressNotAbsolute, text.clone(), none()),
        ResolveError::NotFound { location } => {
            (Problem::AddressNotFound, location.to_string(), none())
        }
        ResolveError::PermissionDenied { location } => (
            Problem::AddressPermissionDenied,
            location.to_string(),
            none(),
        ),
        ResolveError::Io { location, source } => {
            (Problem::AddressIo, location.to_string(), source.to_string())
        }
    }
}

/// 기본 프로그램으로 열기 오류를 화면의 종류와 값으로.
fn open_problem(error: &OpenError) -> (Problem, String) {
    match error {
        OpenError::NoAssociation => (Problem::NoAssociation, String::new()),
        OpenError::PathTooLong => (Problem::PathTooLong, String::new()),
        OpenError::Unsupported => (Problem::OpenUnsupported, String::new()),
        OpenError::Other { code, message } => {
            (Problem::OpenFailed, text::os_error_detail(message, *code))
        }
    }
}

/// 창 하나의 패널 하나(`panel`번째)의 보이는 탭의 항목들. 행 문자열은 화면이 그 행을 물을 때만 만든다. 그 패널이 없으면
/// 비어 있다.
struct TabRows {
    app: Rc<RefCell<App>>,
    window: WindowId,
    panel: usize,
    notify: ModelNotify,
}

impl TabRows {
    /// 이 패널의 보이는 탭.
    fn tab<'a>(&self, app: &'a App) -> Option<&'a Tab> {
        let panel = app.window(self.window)?.panels().get(self.panel)?;
        Some(panel.active_tab())
    }
}

impl Model for TabRows {
    type Data = Row;

    fn row_count(&self) -> usize {
        self.app
            .try_borrow()
            .map_or(0, |app| self.tab(&app).map_or(0, Tab::len))
    }

    fn row_data(&self, row: usize) -> Option<Row> {
        let app = self.app.try_borrow().ok()?;
        let tab = self.tab(&app)?;
        let item = tab.get(row)?;
        Some(Row {
            name: item.name.to_string_lossy().as_ref().into(),
            folder: item.is_folder,
            modified: details::modified_text(item).into(),
            kind: text::file_type_text(&details::file_type(item)).into(),
            size: details::size_text(item).into(),
            selected: tab.is_selected(row),
            cursor: tab.cursor() == Some(row),
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}
