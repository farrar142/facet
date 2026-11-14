//! 화면 수치 측정용 계측(`cargo xtask measure`가 켠다).
//!
//! 환경 변수 [`ENV_OUT`]이 없는 평소 실행에서는 [`install`]이 아무것도 걸지 않고,
//! [`mark_loaded`]는 원자 변수에 값 하나를 쓸 뿐이다. 그래서 평소 실행 경로의 속도는 달라지지 않는다.
//!
//! 측정 모드에서는 목록이 창에 들어간 뒤 처음 그려진 프레임까지의 시간을 재고,
//! [`ENV_SCROLL_ROWS`]가 있으면 프레임마다 그 수만큼의 행씩 목록 끝까지 스크롤하며
//! 프레임 사이의 시간을 모은다. 끝나면 결과를 파일에 쓰고 이벤트 루프를 끝낸다.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use facet_core::measure::{ENV_OUT, ENV_SCROLL_ROWS, ENV_START_US, ScreenReport};
use slint::{ComponentHandle, Model, RenderingState};

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
    /// 목록 맨 끝까지 내렸다: 다음에 그려지는 프레임이 마지막이다.
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

/// 측정 모드이면 창에 그리기 알림을 건다. 아니면 아무것도 하지 않는다.
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
        at_end: false,
        finished: false,
    }));

    let weak = ui.as_weak();
    let result = ui
        .window()
        .set_rendering_notifier(move |rendering_state, _| {
            if !matches!(rendering_state, RenderingState::AfterRendering)
                || !LOADED.load(Ordering::Acquire)
            {
                return;
            }
            let Ok(mut s) = state.lock() else { return };
            if s.finished {
                return;
            }
            let now = Instant::now();
            if s.report.first_frame.is_none() {
                let elapsed_us = now_us().saturating_sub(s.start_us);
                s.report.first_frame = Some(Duration::from_micros(elapsed_us));
                s.report.rows = ROWS.load(Ordering::Relaxed);
                s.last_frame = Some(now);
                if s.scroll_rows == 0 {
                    s.finish();
                    return;
                }
            } else {
                if let Some(last) = s.last_frame {
                    s.report.frames.push(now - last);
                }
                s.last_frame = Some(now);
                if s.at_end {
                    s.finish();
                    return;
                }
            }
            // 다음 프레임을 위해 한 걸음 스크롤한다. 그리는 중에는 창을 건드리지 않고 이벤트 루프에 맡긴다.
            let weak = weak.clone();
            let state = Arc::clone(&state);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    step(&ui, &state);
                }
            });
        });
    if let Err(e) = result {
        eprintln!("이 렌더러는 그리기 알림을 지원하지 않아 측정할 수 없습니다: {e}");
        std::process::exit(2);
    }
}

/// 프레임마다 한 번: 목록을 `scroll_rows`행 내린다. 더 내릴 곳이 없으면 측정을 끝낸다.
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
