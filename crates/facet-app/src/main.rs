//! facet 앱. 코어를 부르기만 하는 얇은 껍질이다.
//!
//! 코어의 「창 > 패널 > 탭」 상태([`App`])를 자세히 보기로 그린다. 사용자 입력(키, 클릭, 마우스 버튼)은 코어의
//! 명령이나 탭 동작으로 바꿔 넘기기만 하고, 정렬·선택·이동의 판단은 모두 코어가 한다.
//!
//! - 목록 읽기는 코어가 일꾼 스레드에서 한다. 결과가 오면 코어가 깨우기([`App::set_waker`])를 부르고, 앱은
//!   이벤트 루프에서 [`App::poll`]로 받아 다시 그린다.
//! - 행 문자열은 화면에 보이는 행만 그때그때 만든다([`TabRows`]).
//! - 실행 인자로 받은 폴더(없으면 사용자 홈 폴더)를 연다.
//! - 패널 위에 탭 줄을 그린다. 탭 연산(열기·닫기·오가기)은 코어가 하고, 앱은 탭을 누름·휠 클릭을 넘긴다.
//! - 창이 나뉘었으면 두 패널을 나란히 그린다. 패널마다 탭 줄·주소창·자세히 보기·상태 표시줄을 갖고, 패널을 누르면
//!   활성 패널이 바뀐다. 키 입력과 명령은 활성 패널의 활성 탭으로 간다. 패널 나누기·합치기·분할 비율은 코어가 한다.
//! - 코어의 창마다 Slint 창을 하나씩 그린다(한 프로세스, 한 이벤트 루프). 창에서 일어난 입력은 먼저 그 창을 코어의
//!   활성 창으로 알린 뒤 넘기므로, 어느 창에서 친 키든 그 창의 활성 패널의 활성 탭으로 간다. 명령으로 창이 생기거나
//!   닫히면 코어의 창 목록에 화면을 맞추고(`sync_windows`), 창이 남지 않으면([`App::is_finished`]) 이벤트 루프를
//!   끝낸다. 제목 줄의 닫기 단추는 코어의 창 닫기([`App::close_window`])다. 깨우기 하나로 모든 창을 갱신한다.
//! - 주소창은 코어의 경로 조각([`facet_core::Provider::segments`]), 글자 해석([`App::go_to_text`]), 자동 완성
//!   ([`App::complete_address`], 일꾼 스레드)을 그리기만 한다.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use facet_core::command::{self, CommandId, Key};
use facet_core::details::{self, Column, Direction};
use facet_core::file_list;
use facet_core::name_input::NameInput;
use facet_core::selection::{CursorMove, Pick};
use facet_core::tab::{AutoRefresh, Navigation, ReadOutcome, Relocation, Tab};
use facet_core::window::{App, Execution, MAX_PANELS, Panel, TabId, Update, WindowId};
use facet_core::{ListError, Location, OpenError, ResolveError, provider_for};
use slint::winit_030::winit::event::WindowEvent;
use slint::winit_030::{EventResult, WinitWindowAccessor};
use slint::{
    CloseRequestResponse, ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString,
    Timer, TimerMode, VecModel,
};

use crate::address::AddressEdit;
use crate::palette::Palette;

mod address;
mod ime;
mod keys;
mod measure;
mod palette;
mod text;

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    // 창 만들기(렌더러 준비)가 콜드 스타트에서 가장 오래 걸리므로, 목록은 그보다 먼저 읽기 시작해 겹친다.
    let start = std::env::args_os().nth(1).map(|arg| {
        let path = PathBuf::from(arg);
        Location::file_system(std::path::absolute(&path).unwrap_or(path))
    });
    let app = Rc::new(RefCell::new(App::new(start)));
    APP.with(|slot| *slot.borrow_mut() = Some(Rc::clone(&app)));

    let first = app.borrow().active_window_id();
    let view = open_view(&app, first, true)?;
    // 결과는 일꾼 스레드에서 온다. 이벤트 루프를 깨워 UI 스레드에서 받는다. 깨우기 하나로 모든 창을 갱신한다.
    app.borrow_mut().set_waker(|| {
        // 이벤트 루프가 이미 끝났으면(창을 모두 닫았으면) 버린다.
        let _ = slint::invoke_from_event_loop(poll_all);
    });
    // 깨우기를 걸기 전에 이미 온 결과(빨리 끝난 첫 읽기)를 받는다. 그래야 첫 화면부터 목록이 보인다.
    poll_all();
    view.ui.show()?;
    drop(view);

    // 창을 닫아도 이벤트 루프는 스스로 끝나지 않는다. 끝낼지는 코어가 정한다([`App::is_finished`], [`sync_windows`]).
    let result = slint::run_event_loop_until_quit();
    // Slint의 스레드 지역 상태보다 먼저 버린다. 스레드가 끝날 때 버리면 Slint 타이머·모델이 이미 사라진
    // 상태를 건드려 멈춘다.
    let views = VIEWS.with(|slot| std::mem::take(&mut *slot.borrow_mut()));
    for view in &views {
        let _ = view.ui.hide();
    }
    drop(views);
    APP.with(|slot| slot.borrow_mut().take());
    result
}

thread_local! {
    /// UI 스레드의 코어 앱. 모든 창이 함께 쓴다.
    static APP: RefCell<Option<Rc<RefCell<App>>>> = const { RefCell::new(None) };
    /// UI 스레드의 화면 상태: 코어의 창마다 하나, 연 차례로.
    static VIEWS: RefCell<Vec<Rc<View>>> = const { RefCell::new(Vec::new()) };
}

/// 열린 창의 화면 상태들. 부르는 동안 목록을 빌려 두지 않도록 복사해 준다.
fn views() -> Vec<Rc<View>> {
    VIEWS.with(|slot| slot.borrow().clone())
}

/// 코어 창 `window`의 화면 상태.
fn view_of(window: WindowId) -> Option<Rc<View>> {
    VIEWS.with(|slot| slot.borrow().iter().find(|v| v.window == window).cloned())
}

/// 코어 창 `window`의 Slint 창을 만들고 콜백을 잇는다. 보이기는 부르는 쪽이 한다. `measure`면 측정 모드의 프레임
/// 세기를 건다(첫 창만).
fn open_view(
    app: &Rc<RefCell<App>>,
    window: WindowId,
    measure: bool,
) -> Result<Rc<View>, slint::PlatformError> {
    let ui = AppWindow::new()?;
    // Slint는 창마다 winit 이벤트 훅을 하나만 둔다: 키 위치를 적고(Ctrl+백슬래시), 그 창이 포커스를 얻으면 활성
    // 창으로 알리고, 측정 모드면 프레임을 센다.
    let mut frames = if measure {
        measure::frame_counter(&ui)
    } else {
        None
    };
    let hook_app = Rc::downgrade(app);
    ui.window().on_winit_window_event(move |_, event| {
        keys::note_window_event(event);
        if let WindowEvent::Focused(true) = event
            && let Some(app) = hook_app.upgrade()
            && let Ok(mut app) = app.try_borrow_mut()
        {
            app.activate_window(window);
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
        if !app.is_finished() && app.active_window_id() != window {
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

/// 코어의 창 목록에 화면을 맞춘다: 새 창은 Slint 창을 만들어 띄우고, 닫힌 창은 감춘다. 창이 남지 않아 앱이
/// 끝났으면([`App::is_finished`]) 이벤트 루프를 끝낸다.
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
        match open_view(&app, window, false) {
            Ok(view) => {
                let _ = view.ui.show();
            }
            Err(error) => eprintln!("창을 만들지 못함: {error}"),
        }
    }
    if app.borrow().is_finished() {
        let _ = slint::quit_event_loop();
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
    let updates = {
        let mut app = app.borrow_mut();
        if app.is_finished() {
            return;
        }
        app.poll()
    };
    if updates.is_empty() {
        return;
    }
    for view in &views {
        view.receive(&updates);
    }
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
    ui.on_confirm_rename(move |text| with_view(w, |view| view.confirm_rename(&text)));
    ui.on_cancel_rename(move || with_view(w, View::cancel_rename));
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
}

/// UI 스레드의 창 하나의 화면 상태: 코어의 창 하나와 그것을 그리는 Slint 창.
struct View {
    /// 그리는 코어 창.
    window: WindowId,
    ui: AppWindow,
    app: Rc<RefCell<App>>,
    /// 패널마다(0 왼쪽, 1 오른쪽) 화면에 넘긴 모델.
    panels: [PanelModels; MAX_PANELS],
    /// 열려 있는 이름 입력란. 화면의 `editing`과 함께 바뀐다.
    name_input: RefCell<Option<NameInput>>,
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
        Rc::new(View {
            window,
            ui,
            app,
            panels,
            name_input: RefCell::default(),
            address: RefCell::default(),
            candidates,
            palette: RefCell::default(),
            palette_rows,
            reading_timer: Timer::default(),
        })
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
                }
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
    }

    /// 읽기 결과(목록이나 오류)가 창에 들어갔음을 측정 모드에 알린다.
    fn mark_loaded(&self) {
        let app = self.app.borrow();
        if let Some(window) = app.window(self.window) {
            measure::mark_loaded(window.active_panel().active_tab().len());
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
        }
        let now = Instant::now();
        let mut redraw_at: Option<Instant> = None;
        for panel in 0..MAX_PANELS {
            if let Some(at) = self.render_panel(ui, panel, now) {
                redraw_at = Some(redraw_at.map_or(at, |earlier| earlier.min(at)));
            }
        }
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
        // 화면은 보이는 행만 다시 묻는다.
        models.rows.notify.reset();
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
            Execution::Done | Execution::Disabled | Execution::WindowClosed => {}
        }
    }

    /// 활성 탭이 바뀌었다(탭 바꾸기·열기·닫기, 패널 나누기·합치기): 이름 입력란·주소창 고쳐 쓰기를 닫고, 패널마다
    /// 보이는 탭이 갖고 있던 목록과 스크롤 위치를 바로 그린다. 다시 읽은 결과는 나중에 와서 바꾼다.
    fn tab_switched(&self) {
        self.close_rename();
        self.close_address_edit();
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
        self.app.borrow_mut().active_tab_mut().pick(index, pick);
        self.render();
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

    /// F2: 커서 항목의 이름 입력란을 연다.
    fn begin_rename(&self) {
        let ui = &self.ui;
        if self.address.borrow().is_some() {
            return;
        }
        let opened = {
            let app = self.app.borrow();
            let tab = app.active_tab();
            tab.cursor().and_then(|index| {
                let item = tab.get(index)?;
                let input = NameInput::open(std::slice::from_ref(item), Some(0))?;
                Some((index, input))
            })
        };
        let Some((index, input)) = opened else {
            return;
        };
        // 입력란이 화면 밖 행에 열리지 않게 먼저 그 행을 보인다.
        self.show_row(index);
        ui.set_edit_text(input.original().into());
        ui.set_notice(Notice::None);
        ui.set_editing(i32::try_from(index).unwrap_or(i32::MAX));
        *self.name_input.borrow_mut() = Some(input);
    }

    /// Enter: 입력을 확정한다. 파일 이름은 바꾸지 않는다.
    fn confirm_rename(&self, text: &str) {
        let ui = &self.ui;
        ui.set_editing(-1);
        if let Some(input) = self.name_input.borrow_mut().take() {
            let entered = input.confirm(text);
            ui.set_notice_entered(entered.entered.into());
            ui.set_notice_original(entered.original.into());
            ui.set_notice(Notice::Entered);
        }
    }

    /// Esc: 입력을 취소한다.
    fn cancel_rename(&self) {
        let ui = &self.ui;
        ui.set_editing(-1);
        if let Some(input) = self.name_input.borrow_mut().take() {
            input.cancel();
            ui.set_notice(Notice::Cancelled);
        }
    }

    /// 알림 없이 이름 입력란을 닫는다.
    fn close_rename(&self) {
        if self.name_input.borrow_mut().take().is_some() {
            self.ui.set_editing(-1);
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
