//! 화면 수치 측정용 계측(`cargo xtask measure`가 켠다).
//!
//! 환경 변수 [`ENV_OUT`]이 없는 평소 실행에서는 [`install`]이 아무것도 걸지 않고,
//! [`mark_loaded`]는 원자 변수에 값 하나를 쓸 뿐이다. 그래서 평소 실행 경로의 속도는 달라지지 않는다.
//!
//! 측정 모드에서는 목록이 창에 들어간 뒤 처음 그려진 프레임이 화면에 나갈 때까지의 시간을 재고,
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
    /// 목록이 든 첫 프레임을 그렸고, 화면에 내보낸 뒤의 시각을 기다리는 중이다.
    first_pending: bool,
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
        first_pending: false,
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
            if s.finished || s.first_pending {
                return;
            }
            let weak = weak.clone();
            let state = Arc::clone(&state);
            if s.report.first_frame.is_none() {
                // 그리기 알림은 화면에 내보내기(present) 전에 온다. 첫 화면을 처음 내보낼 때는
                // 수십 ms 걸리기도 하므로, 내보내고 이벤트 루프로 돌아온 때를 첫 화면 시각으로 삼는다.
                // 스크롤 프레임도 이때부터 재어, 창이 처음 뜨는 비용이 스크롤에 섞이지 않게 한다.
                s.first_pending = true;
                let _ = slint::invoke_from_event_loop(move || first_presented(&weak, &state));
                return;
            }
            let now = Instant::now();
            if let Some(last) = s.last_frame {
                s.report.frames.push(now - last);
            }
            s.last_frame = Some(now);
            if s.at_end {
                s.finish();
                return;
            }
            // 다음 프레임을 위해 한 걸음 스크롤한다. 그리는 중에는 창을 건드리지 않고 이벤트 루프에 맡긴다.
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

/// 목록이 든 첫 프레임을 화면에 내보낸 뒤 한 번: 첫 화면 시각을 적고, 스크롤을 잴 거면 첫 걸음을 내딛는다.
fn first_presented(weak: &slint::Weak<AppWindow>, state: &Mutex<State>) {
    {
        let Ok(mut s) = state.lock() else { return };
        s.first_pending = false;
        let elapsed_us = now_us().saturating_sub(s.start_us);
        s.report.first_frame = Some(Duration::from_micros(elapsed_us));
        s.report.rows = ROWS.load(Ordering::Relaxed);
        s.last_frame = Some(Instant::now());
        if s.scroll_rows == 0 {
            s.finish();
            return;
        }
    }
    if let Some(ui) = weak.upgrade() {
        step(&ui, state);
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
