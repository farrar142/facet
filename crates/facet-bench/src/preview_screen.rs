//! #344: 미리보기 화면 수치 측정(`cargo xtask measure-preview`)의 판정.

use std::time::Duration;

/// #344: 이미지 표시 기준(150ms).
pub const IMAGE_SHOW_LIMIT: Duration = Duration::ZERO; // #344
/// #344: 텍스트 표시 기준(50ms).
pub const TEXT_SHOW_LIMIT: Duration = Duration::ZERO; // #344
/// #344: 켠 때 화살표 프레임 중앙값이 끈 때의 몇 배까지인가(1.1).
pub const ARROW_SLOWDOWN_LIMIT: f64 = 0.0; // #344

/// #344: 미리보기 화면 측정 결과.
#[derive(Debug, Clone)]
pub struct PreviewScreenResult {
    pub arrow_off: Vec<Duration>,
    pub arrow_on: Vec<Duration>,
    pub image_shows: Vec<Duration>,
    pub text_shows: Vec<Duration>,
    pub refresh_hz: Option<f64>,
}

impl PreviewScreenResult {
    /// #344: 판정.
    pub fn verdict(&self) -> PreviewVerdict {
        todo!("#344")
    }
}

/// #344: 판정 결과.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewVerdict {
    pub arrow_pass: bool,
    pub image_pass: bool,
    pub text_pass: bool,
}

impl PreviewVerdict {
    /// #344: 모두 통과인가.
    pub fn pass(&self) -> bool {
        todo!("#344")
    }
}
