//! 창 자리: 창의 화면 위치·크기·최대화([`Placement`])와, 되살릴 때 그 자리를 보이는 곳으로 고치기([`fit`]).
//!
//! # 좌표 단위
//!
//! 모든 값은 **가상 화면 기준 물리 픽셀**이다. 주 모니터의 왼쪽 위가 (0, 0)이고 다른 모니터도 같은 좌표계에 놓인다
//! (주 모니터 왼쪽의 모니터는 음수 좌표). 앱 프로세스는 모니터마다 DPI를 인식(Per-Monitor v2)하므로 Windows가
//! 알려 주는 창 자리·모니터 영역이 모두 이 단위이고, 배율이 다른 모니터 사이에서도 한 좌표계로 비교할 수 있다.
//! 논리 픽셀(배율로 나눈 값)로 남기면 어느 모니터의 배율로 나눴는지에 따라 자리가 어긋나므로 쓰지 않는다.
//!
//! - 위치(`x`, `y`)는 창 바깥 테두리의 왼쪽 위(Windows의 보이지 않는 크기 조절 테두리 포함)다.
//! - 크기(`width`, `height`)는 창 안쪽(제목 줄·테두리를 뺀 그리는 영역)의 크기다.
//!
//! 앱은 창을 옮기거나 크기를 바꿀 때마다 그 창의 자리를 [`Placement::observe`]로 고쳐 코어의 창에 둔다
//! ([`crate::window::Window::placement`]). 최대화한 창은 최대화를 풀었을 때의 자리를 그대로 남긴다. 세션이 이것을
//! 저장하고, 되살릴 때 앱이 지금 모니터 목록으로 [`fit`]한 자리에 창을 띄운 뒤 최대화를 되살린다.

/// 화면 위 직사각형(물리 픽셀). `x`, `y`는 왼쪽 위.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// 오른쪽 끝(이 값은 영역 밖).
    pub fn right(&self) -> i64 {
        i64::from(self.x) + i64::from(self.width)
    }

    /// 아래 끝(이 값은 영역 밖).
    pub fn bottom(&self) -> i64 {
        i64::from(self.y) + i64::from(self.height)
    }

    /// 두 직사각형이 겹치는 너비와 높이. 겹치지 않으면 둘 중 하나가 0이다.
    fn overlap(&self, other: &Rect) -> (u64, u64) {
        let width = self.right().min(other.right()) - i64::from(self.x.max(other.x));
        let height = self.bottom().min(other.bottom()) - i64::from(self.y.max(other.y));
        (width.max(0) as u64, height.max(0) as u64)
    }
}

/// 모니터 하나: 작업 영역(작업 표시줄을 뺀 영역)과 주 모니터 여부.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Monitor {
    pub work_area: Rect,
    pub primary: bool,
}

/// 창의 화면 자리: 최대화하지 않았을 때의 자리와 크기, 최대화 여부.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// 최대화하지 않았을 때(최대화한 창은 최대화를 풀었을 때)의 자리와 크기.
    pub normal: Rect,
    /// 최대화했는가. 최대화한 창은 `normal`이 놓인 모니터에서 최대화된다.
    pub maximized: bool,
}

/// 창이 지금 어떤 모양인가([`Placement::observe`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState {
    Normal,
    Maximized,
    Minimized,
}

impl Placement {
    /// 창이 지금 `rect`에 `state` 모양으로 있을 때 남길 자리. `previous`는 그 창에 남겨 둔 자리다.
    ///
    /// - 보통 창이면 그 자리가 곧 보통 자리다.
    /// - 최대화한 창이면 최대화를 풀었을 때의 자리(`previous`의 보통 자리)를 그대로 두고 최대화만 적는다. 앞서 남긴
    ///   자리가 없으면 지금 자리를 쓴다.
    /// - 최소화한 창의 자리는 화면 밖(Windows는 -32000)이라 남기지 않는다(`previous` 그대로).
    ///
    /// 크기가 0인 자리도 남기지 않는다.
    pub fn observe(
        previous: Option<Placement>,
        rect: Rect,
        state: WindowState,
    ) -> Option<Placement> {
        let usable = rect.width > 0 && rect.height > 0;
        match state {
            WindowState::Minimized => previous,
            WindowState::Normal if usable => Some(Placement {
                normal: rect,
                maximized: false,
            }),
            WindowState::Maximized => match previous {
                Some(previous) => Some(Placement {
                    normal: previous.normal,
                    maximized: true,
                }),
                None if usable => Some(Placement {
                    normal: rect,
                    maximized: true,
                }),
                None => None,
            },
            WindowState::Normal => previous,
        }
    }
}

/// 창이 「보인다」고 치려면 제목 줄 띠([`TITLE_STRIP`])가 한 모니터의 작업 영역과 겹치는 너비가 이만큼(창이 더
/// 좁으면 창 너비)이어야 한다. 그래야 제목 줄을 잡아 옮길 수 있다.
pub const MIN_VISIBLE_WIDTH: u32 = 100;

/// 창 바깥 테두리 위쪽에서 제목 줄로 치는 높이(물리 픽셀). 이 띠가 작업 영역과 [`MIN_VISIBLE_HEIGHT`]만큼 겹쳐야
/// 보인다고 친다.
pub const TITLE_STRIP: u32 = 32;

/// 제목 줄 띠가 작업 영역과 겹쳐야 하는 높이(창이 더 낮으면 창 높이).
pub const MIN_VISIBLE_HEIGHT: u32 = 16;

/// 창 자리 `rect`를 지금 모니터들(`monitors`)에서 보이는 자리로 고친다.
///
/// - 제목 줄 띠가 어느 모니터의 작업 영역과 충분히 겹치면(가로 [`MIN_VISIBLE_WIDTH`], 세로 [`MIN_VISIBLE_HEIGHT`])
///   그대로 둔다. 두 모니터에 걸친 창도 그대로다.
/// - 아니면(창이 놓였던 모니터를 뺐을 때 등) 주 모니터(없으면 첫 모니터)의 작업 영역 안으로 옮긴다. 작업 영역보다
///   크면 그 크기로 줄인다. 주 모니터와 조금이라도 겹치던 창은 가장 가까운 안쪽 자리로 밀어 넣고, 전혀 겹치지 않던
///   창은 가운데에 둔다.
/// - 모니터를 하나도 모르면 그대로 둔다.
pub fn fit(rect: Rect, monitors: &[Monitor]) -> Rect {
    let title = Rect {
        height: rect.height.min(TITLE_STRIP),
        ..rect
    };
    let need_width = u64::from(rect.width.min(MIN_VISIBLE_WIDTH));
    let need_height = u64::from(title.height.min(MIN_VISIBLE_HEIGHT));
    let visible = monitors.iter().any(|monitor| {
        let (width, height) = title.overlap(&monitor.work_area);
        width > 0 && height > 0 && width >= need_width && height >= need_height
    });
    let Some(target) = monitors
        .iter()
        .find(|m| m.primary)
        .or_else(|| monitors.first())
    else {
        return rect;
    };
    if visible {
        return rect;
    }
    let area = target.work_area;
    let width = rect.width.min(area.width);
    let height = rect.height.min(area.height);
    let (overlap_width, overlap_height) = rect.overlap(&area);
    let (x, y) = if overlap_width > 0 && overlap_height > 0 {
        (
            clamp_start(rect.x, width, area.x, area.width),
            clamp_start(rect.y, height, area.y, area.height),
        )
    } else {
        (
            center_start(width, area.x, area.width),
            center_start(height, area.y, area.height),
        )
    };
    Rect::new(x, y, width, height)
}

/// 길이 `length`인 구간의 시작 `start`를 `[area_start, area_start + area_length)` 안으로 민다
/// (`length <= area_length`).
fn clamp_start(start: i32, length: u32, area_start: i32, area_length: u32) -> i32 {
    let last = i64::from(area_start) + i64::from(area_length - length);
    i64::from(start).clamp(i64::from(area_start), last) as i32
}

/// 길이 `length`인 구간을 `[area_start, area_start + area_length)` 가운데에 둔 시작.
fn center_start(length: u32, area_start: i32, area_length: u32) -> i32 {
    (i64::from(area_start) + i64::from((area_length - length) / 2)) as i32
}
