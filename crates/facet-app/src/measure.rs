//! 화면 수치 측정용 계측(`cargo xtask measure`가 켠다).
//!
//! 환경 변수 [`ENV_OUT`]이 없는 평소 실행에서는 [`install`]이 아무것도 걸지 않고,
//! [`mark_loaded`]는 원자 변수에 값 하나를 쓸 뿐이다. 그래서 평소 실행 경로의 속도는 달라지지 않는다.
//!
//! 측정 모드에서는 목록이 창에 들어간 뒤 처음 그려진 프레임이 화면에 나갈 때까지의 시간을 재고,
//! [`ENV_SCROLL_ROWS`]가 있으면 프레임마다 그 수만큼의 행씩 목록 끝까지 스크롤하며
//! 프레임 사이의 시간을 모은다. 끝나면 결과를 파일에 쓰고 이벤트 루프를 끝낸다.
//!
//! 프레임은 렌더러의 그리기 알림이 아니라 winit의 「다시 그려라」 이벤트로 센다. Slint의 winit 백엔드는
//! 그 이벤트를 받으면 그 자리에서 그리고 화면에 내보낸다(present). 그래서 이벤트를 받을 때 이벤트 루프에
//! 일을 하나 맡겨 두면, 그 일은 그 프레임을 내보낸 뒤에 돈다. 그리기 알림을 부르지 않는 skia(D3D)나
//! 그리기 알림이 없는 software 렌더러에서도 femtovg와 같은 구간(내보내고 이벤트 루프로 돌아온 때)을 잰다.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use facet_core::measure::{ENV_OUT, ENV_SCROLL_ROWS, ENV_START_US, ScreenReport};
use slint::winit_030::winit::event::WindowEvent;
use slint::winit_030::{EventResult, WinitWindowAccessor};
use slint::{ComponentHandle, Model};

use crate::AppWindow;

/// 목록(또는 읽기 오류)이 창에 들어갔는가.
static LOADED: AtomicBool = AtomicBool::new(false);

/// 창에 들어간 항목 수.
static ROWS: AtomicUsize = AtomicUsize::new(0);

/// 목록이 창에 들어갔음을 알린다. 읽기 오류였으면 `rows`는 0이다.
pub fn mark_loaded(rows: usize) {
    ROWS.store(rows, Ordering::Relaxed);
    LOADED.store(true, Ordering::Release);
}

struct State {
    out: String,
    start_us: u64,
    scroll_rows: usize,
    report: ScreenReport,
    last_frame: Option<Instant>,
    /// 그리는 프레임이 있어, 그것을 내보낸 뒤의 시각을 기다리는 중이다.
    pending: bool,
    /// 목록 맨 끝까지 내렸다: 다음에 내보내는 프레임이 마지막이다.
    at_end: bool,
    finished: bool,
}

impl State {
    fn finish(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        if let Err(e) = std::fs::write(&self.out, self.report.to_text()) {
            eprintln!("측정 결과를 쓰지 못함: {}: {e}", self.out);
        }
        let _ = slint::quit_event_loop();
    }
}

fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64)
}

/// 측정 모드이면 창에 프레임 세기를 건다. 아니면 아무것도 하지 않는다.
pub fn install(ui: &AppWindow) {
    let Some(out) = std::env::var(ENV_OUT).ok().filter(|v| !v.is_empty()) else {
        return;
    };
    let env_number = |name: &str| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
    };
    let state = Arc::new(Mutex::new(State {
        out,
        start_us: env_number(ENV_START_US),
        scroll_rows: env_number(ENV_SCROLL_ROWS) as usize,
        report: ScreenReport::default(),
        last_frame: None,
        pending: false,
        at_end: false,
        finished: false,
    }));

    let weak = ui.as_weak();
    ui.window().on_winit_window_event(move |_, event| {
        if !matches!(event, WindowEvent::RedrawRequested) || !LOADED.load(Ordering::Acquire) {
            return EventResult::Propagate;
        }
        if let Ok(mut s) = state.lock()
            && !s.finished
            && !s.pending
        {
            // 이 이벤트를 넘기면 Slint가 곧바로 그리고 내보낸다. 맡긴 일은 그 뒤에 돈다.
            s.pending = true;
            let weak = weak.clone();
            let state = Arc::clone(&state);
            let _ = slint::invoke_from_event_loop(move || presented(&weak, &state));
        }
        EventResult::Propagate
    });
}

/// 목록이 든 프레임을 화면에 내보낸 뒤마다 한 번.
///
/// 첫 번째면 첫 화면 시각을 적는다. 스크롤을 잴 거면 앞 프레임과의 간격을 모으고 한 걸음 더 내린다.
/// 스크롤 프레임은 첫 화면 뒤부터 재어, 창이 처음 뜨는 비용이 섞이지 않게 한다.
fn presented(weak: &slint::Weak<AppWindow>, state: &Mutex<State>) {
    {
        let Ok(mut s) = state.lock() else { return };
        s.pending = false;
        if s.finished {
            return;
        }
        let now = Instant::now();
        if s.report.first_frame.is_none() {
            let elapsed_us = now_us().saturating_sub(s.start_us);
            s.report.first_frame = Some(Duration::from_micros(elapsed_us));
            s.report.rows = ROWS.load(Ordering::Relaxed);
            if s.scroll_rows == 0 {
                s.finish();
                return;
            }
        } else if let Some(last) = s.last_frame {
            s.report.frames.push(now - last);
        }
        s.last_frame = Some(now);
        if s.at_end {
            s.finish();
            return;
        }
    }
    if let Some(ui) = weak.upgrade() {
        step(&ui, state);
    }
}

/// 목록을 `scroll_rows`행 내린다. 더 내릴 곳이 없으면 측정을 끝낸다.
fn step(ui: &AppWindow, state: &Mutex<State>) {
    let Ok(mut s) = state.lock() else { return };
    let count = ui.get_rows().row_count();
    let row_height = ui.get_row_height();
    let max_offset = (count as f32 * row_height - ui.get_view_height()).max(0.0);
    let current = -ui.get_content_y();
    let next = (current + s.scroll_rows as f32 * row_height).min(max_offset);
    if next <= current {
        // 움직일 곳이 없으면 다시 그려지지 않으므로 여기서 끝낸다.
        s.finish();
        return;
    }
    s.at_end = next >= max_offset;
    ui.set_content_y(-next);
}
