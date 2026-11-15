//! facet 앱. 코어를 부르기만 하는 얇은 껍질이다.
//!
//! 실행 인자로 받은 폴더(없으면 현재 폴더)의 항목 목록을 창에 보인다.
//! 목록 읽기는 UI 스레드 밖에서 하고, 행 문자열은 화면에 보이는 행만 그때그때 만든다.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::thread;

use facet_core::file_list::{self, Movement};
use facet_core::name_input::NameInput;
use facet_core::{Item, ListError, Location, provider_for};
use slint::{Model, ModelRc, ModelTracker, SharedString};

mod measure;
mod text;

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    // 창 만들기(렌더러 준비)가 콜드 스타트에서 가장 오래 걸리므로, 목록은 그보다 먼저 읽기 시작해 겹친다.
    let opening = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .map_or_else(std::env::current_dir, Ok)
        .map(|path| {
            let location = Location::file_system(path);
            let reading = start_reading(location.clone());
            (location, reading)
        });

    let ui = AppWindow::new()?;
    measure::install(&ui);

    match opening {
        Ok((location, reading)) => open(&ui, &location, reading),
        Err(e) => ui.set_error_text(format!("현재 폴더를 알 수 없습니다: {e}").into()),
    }

    let weak = ui.as_weak();
    ui.on_navigate(move |nav| {
        if let Some(ui) = weak.upgrade() {
            navigate(&ui, movement(nav));
        }
    });
    let weak = ui.as_weak();
    ui.on_select(move |index| {
        if let Some(ui) = weak.upgrade() {
            ui.set_selected(index);
        }
    });

    // 열려 있는 이름 입력란. 화면의 `editing`과 함께 바뀐다.
    let name_input: Rc<RefCell<Option<NameInput>>> = Rc::default();
    let weak = ui.as_weak();
    let state = name_input.clone();
    ui.on_begin_rename(move || {
        let Some(ui) = weak.upgrade() else { return };
        let rows = ui.get_rows();
        let Some(Rows(items)) = rows.as_any().downcast_ref::<Rows>() else {
            return;
        };
        let selected = usize::try_from(ui.get_selected()).ok();
        let Some(input) = NameInput::open(items, selected) else {
            return;
        };
        let index = input.index();
        // 입력란이 화면 밖 행에 열리지 않게 먼저 그 행을 보인다.
        show_row(&ui, index);
        ui.set_edit_text(input.original().into());
        ui.set_notice(Notice::None);
        ui.set_editing(i32::try_from(index).unwrap_or(i32::MAX));
        *state.borrow_mut() = Some(input);
    });
    let weak = ui.as_weak();
    let state = name_input.clone();
    ui.on_confirm_rename(move |text| {
        let Some(ui) = weak.upgrade() else { return };
        ui.set_editing(-1);
        if let Some(input) = state.borrow_mut().take() {
            let entered = input.confirm(&text);
            ui.set_notice_entered(entered.entered.into());
            ui.set_notice_original(entered.original.into());
            ui.set_notice(Notice::Entered);
        }
    });
    let weak = ui.as_weak();
    let state = name_input;
    ui.on_cancel_rename(move || {
        let Some(ui) = weak.upgrade() else { return };
        ui.set_editing(-1);
        if let Some(input) = state.borrow_mut().take() {
            input.cancel();
            ui.set_notice(Notice::Cancelled);
        }
    });

    ui.run()
}

/// 일꾼 스레드에서 읽는 중인 목록.
type Reading = thread::JoinHandle<Result<Vec<Item>, ListError>>;

/// `location`의 목록을 일꾼 스레드에서 읽기 시작한다. 창이 없어도 된다.
fn start_reading(location: Location) -> Reading {
    thread::spawn(move || provider_for(&location).list(&location))
}

/// `location`을 창에 보이고, `reading`이 끝나면 UI 스레드에서 그 목록을 창에 넣는다.
/// 이미 끝났으면 바로 넣어 첫 화면부터 목록이 보이게 한다.
fn open(ui: &AppWindow, location: &Location, reading: Reading) {
    ui.set_location(location.to_string().into());
    ui.set_error_text(SharedString::new());
    if reading.is_finished() {
        show_result(ui, reading.join());
        return;
    }
    ui.set_loading(true);
    let weak = ui.as_weak();
    thread::spawn(move || {
        let result = reading.join();
        // 이벤트 루프가 이미 끝났으면(창을 닫았으면) 버린다.
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak.upgrade() {
                show_result(&ui, result);
            }
        });
    });
}

/// 다 읽은 목록(또는 오류)을 창에 넣는다. UI 스레드에서 부른다.
fn show_result(ui: &AppWindow, result: thread::Result<Result<Vec<Item>, ListError>>) {
    ui.set_loading(false);
    match result {
        Ok(Ok(items)) => {
            measure::mark_loaded(items.len());
            let first = if items.is_empty() { -1 } else { 0 };
            ui.set_rows(ModelRc::from(Rc::new(Rows(items))));
            ui.set_selected(first);
            ui.set_content_y(0.0);
        }
        Ok(Err(e)) => {
            ui.set_error_text(e.to_string().into());
            measure::mark_loaded(0);
        }
        Err(_) => {
            ui.set_error_text("목록을 읽다가 멈췄습니다".into());
            measure::mark_loaded(0);
        }
    }
}

fn movement(nav: Nav) -> Movement {
    match nav {
        Nav::Up => Movement::Up,
        Nav::Down => Movement::Down,
        Nav::PageUp => Movement::PageUp,
        Nav::PageDown => Movement::PageDown,
        Nav::Home => Movement::Home,
        Nav::End => Movement::End,
    }
}

/// 선택을 옮기고, 새 선택이 화면 밖이면 따라 스크롤한다.
fn navigate(ui: &AppWindow, movement: Movement) {
    let count = ui.get_rows().row_count();
    let row_height = ui.get_row_height();
    let view_height = ui.get_view_height();
    let page_rows = if row_height > 0.0 {
        (view_height / row_height) as usize
    } else {
        0
    };
    let current = usize::try_from(ui.get_selected()).ok();
    let Some(next) = file_list::move_selection(current, movement, count, page_rows) else {
        return;
    };
    ui.set_selected(i32::try_from(next).unwrap_or(i32::MAX));
    show_row(ui, next);
}

/// `index`번 행이 화면 밖이면 그 행이 보이게 스크롤한다.
fn show_row(ui: &AppWindow, index: usize) {
    let offset = file_list::scroll_to_show(
        index,
        ui.get_row_height(),
        -ui.get_content_y(),
        ui.get_view_height(),
    );
    ui.set_content_y(-offset);
}

/// 위치의 항목들. 행 문자열은 화면이 그 행을 물을 때만 만든다.
struct Rows(Vec<Item>);

impl Model for Rows {
    type Data = Row;

    fn row_count(&self) -> usize {
        self.0.len()
    }

    fn row_data(&self, row: usize) -> Option<Row> {
        let text = file_list::item_text(self.0.get(row)?);
        Some(Row {
            name: text.name.into(),
            kind: text.kind.into(),
            size: text.size.into(),
            modified: text.modified.into(),
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        // 한 번 넣은 목록은 바뀌지 않는다.
        &()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        // 이름 입력란이 화면 문자열이 아니라 원래 항목을 보도록.
        self
    }
}
