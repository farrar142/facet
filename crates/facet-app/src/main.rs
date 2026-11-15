//! facet 앱. 코어를 부르기만 하는 얇은 껍질이다.
//!
//! 코어의 「창 > 패널 > 탭」 상태([`App`])를 자세히 보기로 그린다. 사용자 입력(키, 클릭, 마우스 버튼)은 코어의
//! 명령이나 탭 동작으로 바꿔 넘기기만 하고, 정렬·선택·이동의 판단은 모두 코어가 한다.
//!
//! - 목록 읽기는 코어가 일꾼 스레드에서 한다. 결과가 오면 코어가 깨우기([`App::set_waker`])를 부르고, 앱은
//!   이벤트 루프에서 [`App::poll`]로 받아 다시 그린다.
//! - 행 문자열은 화면에 보이는 행만 그때그때 만든다([`TabRows`]).
//! - 실행 인자로 받은 폴더(없으면 사용자 홈 폴더)를 연다.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use facet_core::command::{self, CommandId, Key, Shortcut};
use facet_core::details::{self, Column, Direction};
use facet_core::file_list;
use facet_core::name_input::NameInput;
use facet_core::selection::{CursorMove, Pick};
use facet_core::tab::ReadOutcome;
use facet_core::window::{App, Update};
use facet_core::{ListError, Location, OpenError};
use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, Timer, TimerMode};

mod keys;
mod measure;
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
}

/// UI 스레드의 화면 상태: 코어의 앱과 창.
struct View {
    ui: slint::Weak<AppWindow>,
    app: Rc<RefCell<App>>,
    rows: Rc<TabRows>,
    /// 열려 있는 이름 입력란. 화면의 `editing`과 함께 바뀐다.
    name_input: RefCell<Option<NameInput>>,
    /// 「읽는 중」을 드러낼 때 다시 그리는 타이머.
    reading_timer: Timer,
    /// 첫 목록(또는 읽기 오류)이 창에 들어갔는가. 측정 모드가 쓴다.
    loaded: Cell<bool>,
}

impl View {
    fn new(ui: &AppWindow, app: App) -> Rc<Self> {
        let app = Rc::new(RefCell::new(app));
        let rows = Rc::new(TabRows {
            app: Rc::clone(&app),
            notify: ModelNotify::default(),
        });
        ui.set_rows(ModelRc::from(Rc::clone(&rows)));
        Rc::new(View {
            ui: ui.as_weak(),
            app,
            rows,
            name_input: RefCell::default(),
            reading_timer: Timer::default(),
            loaded: Cell::new(false),
        })
    }

    /// 일꾼 스레드에서 온 결과를 모두 탭에 넘기고 다시 그린다.
    fn poll(&self) {
        // 지금 스크롤 위치를 탭에 알린다: 이동이 확정되면 떠나는 위치의 기록에 남는다.
        self.remember_scroll();
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

    fn mark_loaded(&self) {
        if !self.loaded.replace(true) {
            measure::mark_loaded(self.app.borrow().active_tab().len());
        }
    }

    /// 활성 탭의 상태를 창에 넣는다.
    fn render(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        {
            let app = self.app.borrow();
            let tab = app.active_tab();
            ui.set_location(tab.location().to_string().into());

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

    /// 키 입력 하나. 명령이나 커서 이동으로 처리했으면 `true`, 글자 입력이면 `false`다.
    fn key(&self, text: &str, ctrl: bool, alt: bool, shift: bool) -> bool {
        let Some(key) = keys::key_of(text) else {
            return false;
        };
        let shortcut = Shortcut {
            key,
            ctrl,
            alt,
            shift,
        };
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
        // 주소창 고쳐 쓰기(#59)와 명령 팔레트(#60)는 아직 화면이 없다.
        let _ = self.app.borrow_mut().execute(command);
        self.render();
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
        self.app
            .try_borrow()
            .map_or(0, |app| app.active_tab().len())
    }

    fn row_data(&self, row: usize) -> Option<Row> {
        let app = self.app.try_borrow().ok()?;
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
