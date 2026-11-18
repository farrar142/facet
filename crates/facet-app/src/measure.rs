//! 화면 수치 측정용 계측(`cargo xtask measure`가 켠다).
//!
//! 환경 변수 [`ENV_OUT`]이 없는 평소 실행에서는 [`frame_counter`]가 아무것도 돌려주지 않고,
//! [`mark_loaded`]는 원자 변수 둘에 값을 쓸 뿐이다. 그래서 평소 실행 경로의 속도는 달라지지 않는다.
//!
//! 측정 모드에서는 목록이 창에 들어간 뒤 처음 그려진 프레임이 화면에 나갈 때까지의 시간을 재고,
//! [`ENV_OPEN`]이 있으면 그 뒤 이미 떠 있는 창에서 그 폴더로 이동해(주소창이나 경로 조각으로 갈 때와 같은
//! 길) 그 목록이 든 첫 프레임이 화면에 나갈 때까지를 잰다(폴더 열기부터 첫 화면까지).
//! 그렇지 않고 [`ENV_SCROLL_ROWS`]가 있으면 프레임마다 그 수만큼의 행씩 목록 끝까지 스크롤하며
//! 프레임 사이의 시간을 모은다. 끝나면 결과를 파일에 쓰고 이벤트 루프를 끝낸다.
//!
//! 프레임은 렌더러의 그리기 알림이 아니라 winit의 「다시 그려라」 이벤트로 센다. Slint의 winit 백엔드는
//! 그 이벤트를 받으면 그 자리에서 그리고 화면에 내보낸다(present). 그래서 이벤트를 받을 때 이벤트 루프에
//! 일을 하나 맡겨 두면, 그 일은 그 프레임을 내보낸 뒤에 돈다. 그리기 알림을 부르지 않는 skia(D3D)나
//! 그리기 알림이 없는 software 렌더러에서도 femtovg와 같은 구간(내보내고 이벤트 루프로 돌아온 때)을 잰다.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use facet_core::Location;
use facet_core::measure::{
    ENV_GRID, ENV_INSTANT, ENV_MENU, ENV_OPEN, ENV_OUT, ENV_RENAME, ENV_SCROLL_ROWS, ENV_SEARCH,
    ENV_START_US, ENV_WINDOWS, ScreenReport,
};
use slint::winit_030::winit::event::WindowEvent;
use slint::{ComponentHandle, Model};

use crate::AppWindow;

/// 창마다(창 열쇠 [`window_key`]) 목록(또는 읽기 오류)이 들어간 횟수. 창마다 제 목록이 든 프레임만 센다(창 여럿을 잴 때
/// `ENV_WINDOWS`가 필요하다).
static WINDOW_LOADS: Mutex<Vec<(u64, usize)>> = Mutex::new(Vec::new());

/// 첫 프레임을 낸 창 수([`ENV_WINDOWS`]를 줬을 때).
static FIRST_FRAMES: AtomicUsize = AtomicUsize::new(0);

/// 코어 창 번호를 측정이 쓰는 열쇠로 바꾼다.
pub fn window_key(window: facet_core::window::WindowId) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    window.hash(&mut hasher);
    hasher.finish()
}

fn window_loads(key: u64) -> usize {
    WINDOW_LOADS.lock().map_or(0, |loads| {
        loads.iter().find(|(k, _)| *k == key).map_or(0, |(_, n)| *n)
    })
}

/// 마지막으로 창에 들어간 즉시 검색 결과가 실시간 탐색으로 강등된 것인가.
static DEMOTED: AtomicBool = AtomicBool::new(false);

/// 활성 탭의 즉시 검색이 강등됐는지 알린다([`mark_loaded`]와 함께 부른다).
pub fn mark_demoted(demoted: bool) {
    DEMOTED.store(demoted, Ordering::Relaxed);
}

/// 마지막으로 창에 들어간 항목 수.
static ROWS: AtomicUsize = AtomicUsize::new(0);

/// 목록(또는 읽기 오류)이 창에 들어갔음을 알린다. 이동한 위치를 읽을 때마다 부른다.
pub fn mark_loaded(window: u64, rows: usize) {
    ROWS.store(rows, Ordering::Relaxed);
    if let Ok(mut loads) = WINDOW_LOADS.lock() {
        match loads.iter_mut().find(|(k, _)| *k == window) {
            Some((_, n)) => *n += 1,
            None => loads.push((window, 1)),
        }
    }
}

/// #331: 그리드 측정의 종류.
#[derive(Clone, Copy, PartialEq, Eq)]
enum GridMode {
    Fill,
    Scroll,
}

struct State {
    out: String,
    /// 이 창의 열쇠([`window_key`]).
    key: u64,
    /// 이 수만큼의 창이 첫 프레임을 낼 때까지를 첫 화면으로 잰다. 1이면 보통의 측정이다.
    expected_windows: usize,
    start_us: u64,
    scroll_rows: usize,
    /// 첫 화면 뒤에 이동할 폴더.
    open: Option<PathBuf>,
    /// 폴더로 이동을 시작한 때.
    open_started: Option<Instant>,
    /// 첫 화면 뒤에 할 이름바꾸기(옛 이름, 새 이름). 시작한 때는 `open_started`다.
    rename: Option<(String, String)>,
    /// 첫 화면 뒤에 오른쪽 메뉴를 잴 것인가.
    menu: bool,
    /// 오른쪽 메뉴를 재려고 전체 선택을 시작했다(그 프레임이 나가면 메뉴를 연다).
    menu_selecting: bool,
    /// 첫 화면 뒤에 시작할 실시간 탐색의 이름 조각. 시작한 때는 `open_started`다.
    search: Option<String>,
    /// 첫 화면 뒤에 즉시 검색에 넘길 질의 글자. 넘긴 때는 `open_started`다.
    instant: Option<String>,
    /// #331: 첫 화면 뒤에 그리드(보통 크기)로 바꾸고 잴 것: 값 `fill`이면 한 화면 채우기, 그 밖은 스크롤.
    grid: Option<GridMode>,
    /// 그리드로 바꾸는 프레임을 지났다(스크롤 측정).
    grid_started: bool,
    /// 이 횟수보다 많이 목록이 들어온 뒤의 프레임만 센다. 이동한 폴더의 목록을 기다릴 때 쓴다.
    loads_seen: usize,
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

/// 측정 모드인가(환경 변수 [`ENV_OUT`]이 있다). 측정 모드는 단일 실행에 끼지 않는다: 잴 때마다 띄운 프로세스가
/// 떠 있는 facet에 넘기지 않고 스스로 창을 연다.
pub fn is_enabled() -> bool {
    std::env::var_os(ENV_OUT).is_some_and(|v| !v.is_empty())
}

/// 즉시 검색 측정인가(측정 모드이고 [`ENV_INSTANT`]가 있다). 이때만 측정 모드에서도 인덱서 연결을 띄운다.
pub fn instant_enabled() -> bool {
    is_enabled() && std::env::var_os(ENV_INSTANT).is_some_and(|v| !v.is_empty())
}

/// 그리드 측정인가(측정 모드이고 [`ENV_GRID`]가 있다). 이때만 측정 모드에서도 썸네일을 켠다.
pub fn grid_enabled() -> bool {
    is_enabled() && std::env::var_os(ENV_GRID).is_some_and(|v| !v.is_empty())
}

/// 측정 모드이면 프레임 세기(창의 winit 이벤트마다 부를 것)를 돌려준다. 아니면 `None`이다.
///
/// Slint는 창마다 winit 이벤트 훅을 하나만 두므로 훅은 `main`이 걸고, 키 위치 적기([`crate::keys`])와 이것을
/// 차례로 부른다.
pub fn frame_counter(ui: &AppWindow, key: u64) -> Option<impl FnMut(&WindowEvent) + 'static> {
    let out = std::env::var(ENV_OUT).ok().filter(|v| !v.is_empty())?;
    let env_number = |name: &str| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
    };
    let state = Arc::new(Mutex::new(State {
        out,
        key,
        expected_windows: (env_number(ENV_WINDOWS) as usize).max(1),
        start_us: env_number(ENV_START_US),
        scroll_rows: env_number(ENV_SCROLL_ROWS) as usize,
        open: std::env::var_os(ENV_OPEN)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from),
        open_started: None,
        rename: std::env::var(ENV_RENAME)
            .ok()
            .and_then(|v| v.split_once('|').map(|(a, b)| (a.to_owned(), b.to_owned()))),
        menu: std::env::var_os(ENV_MENU).is_some_and(|v| !v.is_empty()),
        menu_selecting: false,
        search: std::env::var(ENV_SEARCH).ok().filter(|v| !v.is_empty()),
        instant: std::env::var(ENV_INSTANT).ok().filter(|v| !v.is_empty()),
        grid: std::env::var(ENV_GRID)
            .ok()
            .filter(|v| !v.is_empty())
            .map(|v| {
                if v == "fill" {
                    GridMode::Fill
                } else {
                    GridMode::Scroll
                }
            }),
        grid_started: false,
        loads_seen: 0,
        report: ScreenReport::default(),
        last_frame: None,
        pending: false,
        at_end: false,
        finished: false,
    }));

    let weak = ui.as_weak();
    Some(move |event: &WindowEvent| {
        if !matches!(event, WindowEvent::RedrawRequested) {
            return;
        }
        if let Ok(mut s) = state.lock()
            && window_loads(s.key) > s.loads_seen
            && !(s.expected_windows > 1 && s.report.first_frame.is_some())
            && !s.finished
            && !s.pending
        {
            // 이 이벤트를 넘기면 Slint가 곧바로 그리고 내보낸다. 맡긴 일은 그 뒤에 돈다.
            s.pending = true;
            let weak = weak.clone();
            let state = Arc::clone(&state);
            let _ = slint::invoke_from_event_loop(move || presented(&weak, &state));
        }
    })
}

/// 목록이 든 프레임을 화면에 내보낸 뒤마다 한 번.
///
/// 첫 번째면 첫 화면 시각을 적는다. 폴더 열기를 잴 거면 그 폴더로 이동을 시작하고, 이동한 폴더의 목록이
/// 든 첫 프레임에서 그 시간을 적고 끝낸다. 스크롤을 잴 거면 앞 프레임과의 간격을 모으고 한 걸음 더 내린다.
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
            if s.expected_windows > 1 {
                // 창 여럿: 마지막 창이 첫 프레임을 낼 때 끝낸다(그 시각이 모든 창의 첫 화면이다).
                let shown = FIRST_FRAMES.fetch_add(1, Ordering::AcqRel) + 1;
                if shown >= s.expected_windows {
                    s.report.windows = shown;
                    s.finish();
                }
                return;
            }
            if let Some(open) = s.open.clone() {
                // 이동한 폴더의 목록이 들어오기 전의 프레임(「읽는 중」 등)은 세지 않는다.
                s.loads_seen = window_loads(s.key);
                s.open_started = Some(Instant::now());
                drop(s);
                crate::with_first_view(|view| view.go(Location::file_system(open)));
                return;
            }
            if let Some((from, to)) = s.rename.clone() {
                s.loads_seen = window_loads(s.key);
                s.open_started = Some(Instant::now());
                drop(s);
                crate::with_first_view(|view| {
                    if !view.measure_rename(&from, &to) {
                        // 바꾸지 못하면 결과 없이 끝낸다(측정 도구가 실패로 본다).
                        if let Ok(mut s) = state.lock() {
                            s.finish();
                        }
                    }
                });
                return;
            }
            if let Some(query) = s.search.clone() {
                // 결과 위치로 옮긴 빈 목록의 프레임은 세지 않는다(결과가 든 프레임을 기다림, 아래).
                s.loads_seen = window_loads(s.key);
                s.open_started = Some(Instant::now());
                drop(s);
                crate::with_first_view(|view| {
                    if !view.measure_search(&query)
                        && let Ok(mut s) = state.lock()
                    {
                        s.finish();
                    }
                });
                return;
            }
            if let Some(query) = s.instant.clone() {
                // 결과 위치로 옮긴 빈 목록의 프레임은 세지 않는다(결과가 든 프레임을 기다림, 아래).
                s.loads_seen = window_loads(s.key);
                s.open_started = Some(Instant::now());
                drop(s);
                crate::with_first_view(|view| {
                    if !view.measure_instant(&query)
                        && let Ok(mut s) = state.lock()
                    {
                        s.finish();
                    }
                });
                return;
            }
            if s.grid.is_some() {
                // 그리드로 바꾼 프레임부터 잰다(바꾸기 전 프레임은 세지 않음, 아래).
                s.loads_seen = window_loads(s.key);
                s.open_started = Some(Instant::now());
                drop(s);
                crate::with_first_view(|view| {
                    if !view.measure_grid_start()
                        && let Ok(mut s) = state.lock()
                    {
                        s.finish();
                    }
                });
                return;
            }
            if s.menu {
                // 먼저 전체를 선택하고 그 프레임이 나가면 메뉴를 연다(선택을 그리는 비용이 메뉴 시간에 섞이지 않게).
                s.loads_seen = window_loads(s.key);
                s.menu_selecting = true;
                drop(s);
                crate::with_first_view(|view| {
                    if !view.measure_select_all()
                        && let Ok(mut s) = state.lock()
                    {
                        s.finish();
                    }
                });
                return;
            }
            if s.scroll_rows == 0 {
                s.finish();
                return;
            }
        } else if s.menu_selecting {
            s.menu_selecting = false;
            s.loads_seen = window_loads(s.key);
            s.open_started = Some(Instant::now());
            drop(s);
            crate::with_first_view(|view| {
                if !view.measure_open_menu()
                    && let Ok(mut s) = state.lock()
                {
                    s.finish();
                }
            });
            return;
        } else if let Some(mode) = s.grid {
            let started = s.open_started.unwrap_or(now);
            if mode == GridMode::Fill {
                drop(s);
                let filled = crate::with_first_view_value(|view| view.measure_grid_filled());
                let Ok(mut s) = state.lock() else { return };
                if filled {
                    s.report.grid_fill = Some(now - started);
                    s.report.rows = ROWS.load(Ordering::Relaxed);
                    s.finish();
                } else {
                    // 아직 덜 찬 프레임: 다음 썸네일이 들어오기를 기다린다.
                    s.loads_seen = window_loads(s.key);
                }
                return;
            }
            if s.grid_started {
                if let Some(last) = s.last_frame {
                    s.report.frames.push(now - last);
                }
            } else {
                s.grid_started = true;
                s.report.rows = ROWS.load(Ordering::Relaxed);
            }
        } else if let Some(started) = s.open_started {
            if s.search.is_some() {
                let rows = ROWS.load(Ordering::Relaxed);
                if rows == 0 {
                    // 결과가 아직 없는 프레임(빈 결과 위치·진행만 바뀐 묶음): 다음 결과를 기다린다.
                    s.loads_seen = window_loads(s.key);
                    return;
                }
                s.report.search_frame = Some(now - started);
                s.report.rows = rows;
                s.finish();
                return;
            }
            if s.instant.is_some() {
                let rows = ROWS.load(Ordering::Relaxed);
                if rows == 0 {
                    // 결과가 아직 없는 프레임(빈 결과 위치·응답 전): 다음 결과를 기다린다.
                    s.loads_seen = window_loads(s.key);
                    return;
                }
                s.report.instant_frame = Some(now - started);
                s.report.instant_demoted = DEMOTED.load(Ordering::Relaxed);
                s.report.rows = rows;
                s.finish();
                return;
            }
            if s.menu {
                s.report.menu_frame = Some(now - started);
            } else if s.rename.is_some() {
                s.report.rename_frame = Some(now - started);
            } else {
                s.report.open_frame = Some(now - started);
            }
            s.report.rows = ROWS.load(Ordering::Relaxed);
            s.finish();
            return;
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
    if s.grid.is_some() {
        // #331: 그리드는 한 줄(칸 높이)씩이 아니라 `scroll_rows`행(칸 줄)씩 내린다.
        match ui_grid_step(ui, s.scroll_rows) {
            Some(at_end) => s.at_end = at_end,
            None => s.finish(),
        }
        return;
    }
    // 측정은 패널 하나(왼쪽)로 한다.
    let count = ui.get_left_rows().row_count();
    let row_height = ui.get_row_height();
    let max_offset = (count as f32 * row_height - ui.get_left_view_height()).max(0.0);
    let current = -ui.get_left_content_y();
    let next = (current + s.scroll_rows as f32 * row_height).min(max_offset);
    if next <= current {
        // 움직일 곳이 없으면 다시 그려지지 않으므로 여기서 끝낸다.
        s.finish();
        return;
    }
    s.at_end = next >= max_offset;
    ui.set_left_content_y(-next);
}

/// #331: 그리드 목록을 `lines`줄 내린다. 내렸으면 맨 끝에 닿았는지, 더 내릴 곳이 없으면 `None`이다.
fn ui_grid_step(ui: &AppWindow, lines: usize) -> Option<bool> {
    let cell = ui.get_left_grid_cell_height();
    let columns = usize::try_from(ui.get_left_grid_columns())
        .unwrap_or(1)
        .max(1);
    let len = ui.get_left_rows().row_count();
    let max_offset =
        (len.div_ceil(columns) as f32 * cell - ui.get_left_grid_view_height()).max(0.0);
    let current = -ui.get_left_grid_content_y();
    let next = (current + lines as f32 * cell).min(max_offset);
    if next <= current {
        return None;
    }
    ui.set_left_grid_content_y(-next);
    Some(next >= max_offset)
}
