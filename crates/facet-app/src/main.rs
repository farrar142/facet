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
//! - 주소창은 코어의 경로 조각([`facet_core::Provider::segments`]), 글자 해석([`App::go_to_text`]), 자동 완성
//!   ([`App::complete_address`], 일꾼 스레드)을 그리기만 한다.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use facet_core::command::{self, CommandId, Key};
use facet_core::details::{self, Column, Direction};
use facet_core::file_list;
use facet_core::name_input::NameInput;
use facet_core::selection::{CursorMove, Pick};
use facet_core::tab::{AutoRefresh, Navigation, ReadOutcome, Relocation};
use facet_core::window::{App, Execution, Update};
use facet_core::{ListError, Location, OpenError, ResolveError, provider_for};
use slint::{
    ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString, Timer, TimerMode,
    VecModel,
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
    let app = App::new(start);

    let ui = AppWindow::new()?;
    measure::install(&ui);

    let view = View::new(&ui, app);
    VIEW.with(|slot| *slot.borrow_mut() = Some(Rc::clone(&view)));
    // 결과는 일꾼 스레드에서 온다. 이벤트 루프를 깨워 UI 스레드에서 받는다.
    view.app.borrow_mut().set_waker(|| {
        // 이벤트 루프가 이미 끝났으면(창을 닫았으면) 버린다.
        let _ = slint::invoke_from_event_loop(|| with_view(|view| view.poll()));
    });
    // 깨우기를 걸기 전에 이미 온 결과(빨리 끝난 첫 읽기)를 받는다. 그래야 첫 화면부터 목록이 보인다.
    view.poll();
    view.render();
    connect(&ui);
    drop(view);

    let result = ui.run();
    // Slint의 스레드 지역 상태보다 먼저 버린다. 스레드가 끝날 때 버리면 Slint 타이머·모델이 이미 사라진
    // 상태를 건드려 멈춘다.
    let view = VIEW.with(|slot| slot.borrow_mut().take());
    drop(view);
    result
}

thread_local! {
    /// UI 스레드의 화면 상태. 이벤트 루프에서 부르는 일이 찾아 쓴다.
    static VIEW: RefCell<Option<Rc<View>>> = const { RefCell::new(None) };
}

/// UI 스레드에서 화면 상태로 `f`를 부른다.
fn with_view(f: impl FnOnce(&View)) {
    let view = VIEW.with(|slot| slot.borrow().clone());
    if let Some(view) = view {
        f(&view);
    }
}

/// 창의 콜백을 화면 상태에 잇는다.
fn connect(ui: &AppWindow) {
    ui.on_key(|text, ctrl, alt, shift| {
        let mut handled = false;
        with_view(|view| handled = view.key(&text, ctrl, alt, shift));
        handled
    });
    ui.on_typed(|text| with_view(|view| view.typed(&text)));
    ui.on_row_pressed(|row, ctrl, shift| {
        with_view(|view| view.row_pressed(row, Pick::for_click(ctrl, shift)));
    });
    ui.on_row_opened(|row| with_view(|view| view.row_opened(row)));
    ui.on_row_middle_pressed(|row| with_view(|view| view.row_middle_pressed(row)));
    ui.on_tab_pressed(|index| with_view(|view| view.tab_pressed(index)));
    ui.on_sort_by(|column| {
        let column = match column {
            1 => Column::Modified,
            2 => Column::Type,
            3 => Column::Size,
            _ => Column::Name,
        };
        with_view(|view| view.execute(CommandId::sort_by(column)));
    });
    ui.on_mouse_history(|forward| {
        let command = if forward {
            CommandId::Forward
        } else {
            CommandId::Back
        };
        with_view(|view| view.execute(command));
    });
    ui.on_begin_rename(|| with_view(View::begin_rename));
    ui.on_confirm_rename(|text| with_view(|view| view.confirm_rename(&text)));
    ui.on_cancel_rename(|| with_view(View::cancel_rename));
    ui.on_remove_selection(|text, anchor, cursor| {
        ime::remove_selection(&text, anchor, cursor).0.into()
    });
    ui.on_segment_pressed(|index| with_view(|view| view.segment_pressed(index)));
    ui.on_begin_address_edit(|| with_view(View::begin_address_edit));
    ui.on_address_edited(|text| with_view(|view| view.address_edited(&text)));
    ui.on_address_step(|step| {
        let mut end = -1;
        with_view(|view| end = view.address_step(step >= 0));
        end
    });
    ui.on_address_go(|text| {
        let mut went = false;
        with_view(|view| went = view.address_go(&text));
        went
    });
    ui.on_candidate_pressed(|index| with_view(|view| view.candidate_pressed(index)));
    ui.on_cancel_address_edit(|| with_view(View::close_address_edit));
    ui.on_palette_edited(|text| with_view(|view| view.palette_edited(&text)));
    ui.on_palette_step(|step| with_view(|view| view.palette_step(step >= 0)));
    ui.on_palette_run(|index| {
        let mut ran = false;
        with_view(|view| ran = view.palette_run(index));
        ran
    });
    ui.on_palette_close(|| with_view(View::close_palette));
}

/// UI 스레드의 화면 상태: 코어의 앱과 창.
struct View {
    ui: slint::Weak<AppWindow>,
    app: Rc<RefCell<App>>,
    rows: Rc<TabRows>,
    /// 열려 있는 이름 입력란. 화면의 `editing`과 함께 바뀐다.
    name_input: RefCell<Option<NameInput>>,
    /// 주소창 경로 조각의 이름. 바뀔 때만 고쳐 화면 요소를 다시 만들지 않는다.
    segments: Rc<VecModel<SharedString>>,
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
    /// 활성 패널의 탭 줄.
    tabs: Rc<VecModel<TabItem>>,
}

impl View {
    fn new(ui: &AppWindow, app: App) -> Rc<Self> {
        let app = Rc::new(RefCell::new(app));
        let rows = Rc::new(TabRows {
            app: Rc::clone(&app),
            notify: ModelNotify::default(),
        });
        ui.set_rows(ModelRc::from(Rc::clone(&rows)));
        let segments = Rc::new(VecModel::default());
        ui.set_segments(ModelRc::from(Rc::clone(&segments)));
        let candidates = Rc::new(VecModel::default());
        ui.set_candidates(ModelRc::from(Rc::clone(&candidates)));
        let palette_rows = Rc::new(VecModel::default());
        ui.set_palette_rows(ModelRc::from(Rc::clone(&palette_rows)));
        let tabs = Rc::new(VecModel::default());
        ui.set_tabs(ModelRc::from(Rc::clone(&tabs)));
        Rc::new(View {
            tabs,
            ui: ui.as_weak(),
            app,
            rows,
            name_input: RefCell::default(),
            segments,
            address: RefCell::default(),
            candidates,
            palette: RefCell::default(),
            palette_rows,
            reading_timer: Timer::default(),
        })
    }

    /// 일꾼 스레드에서 온 결과를 모두 탭에 넘기고 다시 그린다.
    fn poll(&self) {
        // 지금 스크롤 위치를 탭에 알린다: 이동이 확정되면 떠나는 위치의 기록에 남는다.
        self.remember_scroll();
        if self.app.borrow().is_finished() {
            return;
        }
        let (updates, active) = {
            let mut app = self.app.borrow_mut();
            let updates = app.poll();
            (updates, app.active_window().active_panel().active_tab_id())
        };
        if updates.is_empty() {
            return;
        }
        let mut committed = false;
        for (tab, update) in updates {
            if tab != active {
                continue;
            }
            match update {
                Update::Read(ReadOutcome::Committed) => committed = true,
                Update::Read(ReadOutcome::Failed) => self.mark_loaded(),
                Update::Completion { id, candidates } => self.receive_candidates(id, candidates),
                _ => {}
            }
        }
        if committed {
            // 다른 위치의 목록이 들어왔으면 이름 입력란은 닫는다.
            self.close_rename();
        }
        self.render();
        if committed {
            self.mark_loaded();
            self.restore_scroll();
        }
    }

    /// 읽기 결과(목록이나 오류)가 창에 들어갔음을 측정 모드에 알린다.
    fn mark_loaded(&self) {
        measure::mark_loaded(self.app.borrow().active_tab().len());
    }

    /// 활성 탭의 상태를 창에 넣는다.
    fn render(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        if self.app.borrow().is_finished() {
            return;
        }
        self.render_tabs(&ui);
        {
            let app = self.app.borrow();
            let tab = app.active_tab();
            let labels: Vec<SharedString> = provider_for(tab.location())
                .segments(tab.location())
                .into_iter()
                .map(|segment| segment.label.into())
                .collect();
            if !self.segments.iter().eq(labels.iter().cloned()) {
                self.segments.set_vec(labels);
            }

            let (problem, value, detail) = match tab.address_error() {
                Some(error) => address_problem(error),
                None => (Problem::None, String::new(), String::new()),
            };
            ui.set_address_problem(problem);
            ui.set_address_problem_value(value.into());
            ui.set_address_problem_detail(detail.into());

            let now = Instant::now();
            ui.set_reading(tab.is_reading(now));
            if let Some(at) = tab.reading_shown_at().filter(|at| *at > now) {
                // 「읽는 중」을 드러낼 때가 오면 다시 그린다.
                self.reading_timer
                    .start(TimerMode::SingleShot, at - now, || with_view(View::render));
            }

            let (problem, location, detail) = match tab.error() {
                Some(error) => list_problem(error),
                None => (Problem::None, String::new(), String::new()),
            };
            ui.set_list_problem(problem);
            ui.set_list_problem_location(location.into());
            ui.set_list_problem_detail(detail.into());
            let (problem, detail) = match tab.open_error() {
                Some(error) => open_problem(error),
                None => (Problem::None, String::new()),
            };
            ui.set_open_problem(problem);
            ui.set_open_problem_detail(detail.into());

            ui.set_auto_refresh_off(matches!(tab.auto_refresh(), AutoRefresh::Off(_)));
            let (relocation, vanished) = match tab.relocation() {
                Some(Relocation::ToParent { vanished }) => (1, vanished.to_string()),
                Some(Relocation::ToHome { vanished }) => (2, vanished.to_string()),
                None => (0, String::new()),
            };
            ui.set_relocation(relocation);
            ui.set_relocation_vanished(vanished.into());

            let sort = tab.sort();
            ui.set_sort_column(match sort.column {
                Column::Name => 0,
                Column::Modified => 1,
                Column::Type => 2,
                Column::Size => 3,
            });
            ui.set_sort_descending(sort.direction == Direction::Descending);

            let status = tab.status();
            ui.set_item_count(count(status.item_count));
            ui.set_selected_count(count(status.selected_count));
            let size = if status.selected_size > 0 {
                file_list::format_size(status.selected_size)
            } else {
                String::new()
            };
            ui.set_selected_size(size.into());
        }
        // 화면은 보이는 행만 다시 묻는다.
        self.rows.notify.reset();
    }

    /// 활성 패널의 탭 줄: 제목(폴더 이름), 툴팁(전체 경로), 활성 탭. 바뀔 때만 고쳐 화면 요소를 다시 만들지 않고,
    /// 활성 탭이 줄 밖이면 보이게 넘긴다.
    fn render_tabs(&self, ui: &AppWindow) {
        let (items, active) = {
            let app = self.app.borrow();
            let panel = app.active_window().active_panel();
            let active = panel.active_index();
            let items: Vec<TabItem> = panel
                .tabs()
                .enumerate()
                .map(|(i, (_, tab))| TabItem {
                    title: tab.title().into(),
                    path: tab.location().to_string().into(),
                    active: i == active,
                })
                .collect();
            (items, active)
        };
        if !self.tabs.iter().eq(items.iter().cloned()) {
            self.tabs.set_vec(items);
        }
        let offset = file_list::scroll_to_show(
            active,
            ui.get_tab_width(),
            -ui.get_tab_strip_x(),
            ui.get_tab_strip_width(),
        );
        ui.set_tab_strip_x(-offset);
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
        if self.app.borrow().is_finished() {
            // 마지막 탭을 닫아 창이 남지 않았다: 앱을 끝낸다.
            let _ = slint::quit_event_loop();
            return;
        }
        if self.app.borrow().active_tab_id() != before {
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

    /// 활성 탭이 바뀌었다: 이름 입력란·주소창 고쳐 쓰기를 닫고, 그 탭이 갖고 있던 목록과 스크롤 위치를 바로 그린다.
    /// 다시 읽은 결과는 나중에 와서 바꾼다.
    fn tab_switched(&self) {
        self.close_rename();
        self.close_address_edit();
        self.render();
        self.restore_scroll();
    }

    /// 탭 줄의 `index`번째 탭을 눌렀다: 그 탭으로 간다.
    fn tab_pressed(&self, index: i32) {
        let id = {
            let app = self.app.borrow();
            let panel = app.active_window().active_panel();
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
        let Some(ui) = self.ui.upgrade() else { return };
        self.close_rename();
        self.close_address_edit();
        *self.palette.borrow_mut() = Some(Palette::default());
        self.palette_edited("");
        ui.set_palette_text(SharedString::new());
        ui.set_palette_open(true);
    }

    fn close_palette(&self) {
        self.palette.borrow_mut().take();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_palette_open(false);
        }
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
        let Some(ui) = self.ui.upgrade() else { return };
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
        let Some(ui) = self.ui.upgrade() else { return };
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
        if let Some(ui) = self.ui.upgrade() {
            ui.set_address_editing(false);
        }
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
        let Some(ui) = self.ui.upgrade() else { return };
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
        let Some(ui) = self.ui.upgrade() else {
            return -1;
        };
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

    /// 한 쪽의 행 수.
    fn page_rows(&self) -> usize {
        let Some(ui) = self.ui.upgrade() else {
            return 1;
        };
        let row_height = ui.get_row_height();
        if row_height > 0.0 {
            (ui.get_view_height() / row_height) as usize
        } else {
            1
        }
    }

    /// 맨 위에 보이는 행 번호를 탭에 알린다.
    fn remember_scroll(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let row_height = ui.get_row_height();
        if row_height > 0.0 {
            let top = (-ui.get_content_y() / row_height).max(0.0) as usize;
            self.app.borrow_mut().active_tab_mut().set_scroll(top);
        }
    }

    /// 이동이 확정된 뒤: 탭이 기억한 스크롤 위치로 가고(뒤로 가면 되살아난다) 커서 항목을 보인다.
    fn restore_scroll(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let (scroll, cursor) = {
            let app = self.app.borrow();
            let tab = app.active_tab();
            (tab.scroll(), tab.cursor())
        };
        ui.set_content_y(-(scroll as f32 * ui.get_row_height()));
        if let Some(index) = cursor {
            self.show_row(index);
        }
    }

    /// `index`번 행이 화면 밖이면 그 행이 보이게 스크롤한다.
    fn show_row(&self, index: usize) {
        let Some(ui) = self.ui.upgrade() else { return };
        let offset = file_list::scroll_to_show(
            index,
            ui.get_row_height(),
            -ui.get_content_y(),
            ui.get_view_height(),
        );
        ui.set_content_y(-offset);
    }

    /// F2: 커서 항목의 이름 입력란을 연다.
    fn begin_rename(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
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
        let Some(ui) = self.ui.upgrade() else { return };
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
        let Some(ui) = self.ui.upgrade() else { return };
        ui.set_editing(-1);
        if let Some(input) = self.name_input.borrow_mut().take() {
            input.cancel();
            ui.set_notice(Notice::Cancelled);
        }
    }

    /// 알림 없이 이름 입력란을 닫는다.
    fn close_rename(&self) {
        if self.name_input.borrow_mut().take().is_some()
            && let Some(ui) = self.ui.upgrade()
        {
            ui.set_editing(-1);
        }
    }
}

fn count(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
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

/// 활성 탭의 보이는 항목들. 행 문자열은 화면이 그 행을 물을 때만 만든다.
struct TabRows {
    app: Rc<RefCell<App>>,
    notify: ModelNotify,
}

impl Model for TabRows {
    type Data = Row;

    fn row_count(&self) -> usize {
        self.app.try_borrow().map_or(0, |app| {
            if app.is_finished() {
                0
            } else {
                app.active_tab().len()
            }
        })
    }

    fn row_data(&self, row: usize) -> Option<Row> {
        let app = self.app.try_borrow().ok()?;
        if app.is_finished() {
            return None;
        }
        let tab = app.active_tab();
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
