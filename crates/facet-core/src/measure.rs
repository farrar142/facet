//! 화면 수치 측정에서 앱과 측정 도구(`facet-bench screen`)가 주고받는 약속.
//!
//! 측정 도구가 환경 변수로 앱을 측정 모드로 띄우면, 앱은 결과를 [`ENV_OUT`]이 가리키는
//! 파일에 한 줄씩 적고 끝난다. 환경 변수가 없는 평소 실행은 이 모듈을 건드리지 않는다.
//!
//! 파일 형식(줄마다 `이름 값`):
//!
//! ```text
//! rows 100000        # 앱이 읽은 항목 수
//! first_frame_us 123 # 측정 도구가 앱을 띄운 때부터 목록이 든 첫 프레임이 창에 나올 때까지(마이크로초)
//! frame_us 16700     # 스크롤 중 프레임 하나의 시간(마이크로초). 프레임마다 한 줄
//! ```

use std::time::Duration;

/// 설정하면 앱이 측정 모드가 되고 결과를 이 경로의 파일에 쓴다.
pub const ENV_OUT: &str = "FACET_MEASURE_OUT";
/// 측정 도구가 앱을 띄우기 직전의 시각(UNIX epoch 기준 마이크로초).
pub const ENV_START_US: &str = "FACET_MEASURE_START_US";
/// 0보다 크면 첫 화면 뒤에 프레임마다 이만큼의 행씩 목록 끝까지 자동으로 스크롤한다.
pub const ENV_SCROLL_ROWS: &str = "FACET_MEASURE_SCROLL_ROWS";

/// 앱이 쓰고 측정 도구가 읽는 측정 결과.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScreenReport {
    /// 앱이 읽은 항목 수.
    pub rows: usize,
    /// 실행부터 목록이 든 첫 프레임이 창에 나올 때까지.
    pub first_frame: Option<Duration>,
    /// 스크롤하는 동안의 프레임 시간들.
    pub frames: Vec<Duration>,
}

impl ScreenReport {
    /// 파일에 적을 문자열.
    pub fn to_text(&self) -> String {
        let mut out = format!("rows {}\n", self.rows);
        if let Some(d) = self.first_frame {
            out.push_str(&format!("first_frame_us {}\n", d.as_micros()));
        }
        for f in &self.frames {
            out.push_str(&format!("frame_us {}\n", f.as_micros()));
        }
        out
    }

    /// [`ScreenReport::to_text`]의 거꾸로. 모르는 줄이나 깨진 값이 있으면 오류다.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut report = Self::default();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let (key, value) = line
                .split_once(' ')
                .ok_or_else(|| format!("형식이 맞지 않는 줄: {line}"))?;
            let number: u64 = value
                .trim()
                .parse()
                .map_err(|_| format!("값이 수가 아닌 줄: {line}"))?;
            match key {
                "rows" => report.rows = number as usize,
                "first_frame_us" => report.first_frame = Some(Duration::from_micros(number)),
                "frame_us" => report.frames.push(Duration::from_micros(number)),
                _ => return Err(format!("모르는 항목: {line}")),
            }
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 쓴_결과를_그대로_읽는다() {
        let report = ScreenReport {
            rows: 100_000,
            first_frame: Some(Duration::from_micros(123_456)),
            frames: vec![Duration::from_micros(16_700), Duration::from_micros(33_000)],
        };
        assert_eq!(ScreenReport::parse(&report.to_text()), Ok(report));
    }

    #[test]
    fn 첫_화면이_없으면_없는_채로_읽는다() {
        let report = ScreenReport::parse("rows 3\n").unwrap();
        assert_eq!(report.rows, 3);
        assert_eq!(report.first_frame, None);
        assert!(report.frames.is_empty());
    }

    #[test]
    fn 깨진_줄은_오류다() {
        assert!(ScreenReport::parse("rows abc").is_err());
        assert!(ScreenReport::parse("nope 1").is_err());
        assert!(ScreenReport::parse("rows").is_err());
    }
}
