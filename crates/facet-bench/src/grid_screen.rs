//! #331: 그리드 화면 수치 측정(`cargo xtask measure-grid`)의 상수와 판정.

use std::time::Duration;

/// #331: 측정에 쓰는 사진 수.
pub const GRID_IMAGE_COUNT: usize = 10_000;
/// #331: 캐시가 찬 두 번째 열기에서 한 화면 채우기 상한.
pub const GRID_FILL_LIMIT: Duration = Duration::from_millis(100);

/// #331: 그리드 측정의 판정.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridVerdict {
    pub fill_median: Duration,
    pub fill_pass: bool,
    pub frame_worst: Duration,
    pub frame_pass: bool,
}

impl GridVerdict {
    /// #331: 두 수치가 모두 통과했는가.
    pub fn pass(&self) -> bool {
        todo!("#331")
    }
}

/// #331: 채우기 시간들과 프레임 시간들을 주사율로 판정한다.
pub fn judge(_fills: &[Duration], _frames: &[Duration], _refresh_hz: f64) -> GridVerdict {
    todo!("#331")
}
