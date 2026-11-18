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
//! open_frame_us 45   # [`ENV_OPEN`]의 폴더로 이동을 시작한 때부터 그 목록이 든 첫 프레임이 나올 때까지
//! windows 2          # [`ENV_WINDOWS`]를 줬을 때, 첫 프레임이 나온 창 수
//! rename_frame_us 9 # [`ENV_RENAME`]의 이름바꾸기를 확정한 때부터 목록에 새 이름이 든 첫 프레임이 나올 때까지
//! menu_frame_us 20000 # [`ENV_MENU`]의 오른쪽 메뉴를 열 때부터 메뉴가 그려진 첫 프레임이 나올 때까지
//! search_frame_us 40000 # [`ENV_SEARCH`]의 실시간 탐색을 시작한 때부터 첫 결과가 든 프레임이 나올 때까지
//! instant_frame_us 30000 # [`ENV_INSTANT`]의 즉시 검색 글자를 넘긴 때부터 첫 결과가 든 프레임이 나올 때까지
//! instant_demoted 1  # 즉시 검색이 실시간 탐색으로 강등됐다(있으면 측정 실패)
//! frame_us 16700     # 스크롤 중 프레임 하나의 시간(마이크로초). 프레임마다 한 줄
//! ```

use std::time::Duration;

/// 설정하면 앱이 측정 모드가 되고 결과를 이 경로의 파일에 쓴다.
pub const ENV_OUT: &str = "FACET_MEASURE_OUT";
/// 측정 도구가 앱을 띄우기 직전의 시각(UNIX epoch 기준 마이크로초).
pub const ENV_START_US: &str = "FACET_MEASURE_START_US";
/// 설정하면 첫 화면이 나온 뒤 이미 떠 있는 창에서 이 경로의 폴더로 이동하고, 그 목록이 든 첫 프레임이
/// 나올 때까지를 잰다(폴더 열기부터 첫 화면까지). 이때 `rows`는 이동한 폴더의 항목 수이고 스크롤은 하지 않는다.
pub const ENV_OPEN: &str = "FACET_MEASURE_OPEN";
/// `옛 이름|새 이름`(`|`는 이름에 못 쓰는 글자라 구분자로 쓴다)을 주면 첫 화면 뒤에 활성 탭 폴더의 그 항목을 코어
/// 이름바꾸기(`App::rename`, 이름 입력란 확정이 부르는 것과 같음)로 새 이름으로 바꾸고, 확정한 때부터 목록에 새
/// 이름이 든 첫 프레임이 화면에 나갈 때까지를 잰다(`rename_frame`). 스크롤은 하지 않는다.
pub const ENV_RENAME: &str = "FACET_MEASURE_RENAME";
/// 설정하면(값은 무엇이든 비어 있지 않게) 첫 화면 뒤에 활성 탭 폴더 전체를 선택하고(그 프레임이 나갈 때까지 기다림),
/// 그 상태에서 첫 행의 항목 메뉴를 열어(오른쪽 클릭과 같은 길) 열 때부터 메뉴가 그려진 첫 프레임이 화면에 나갈
/// 때까지를 잰다(`menu_frame`). 스크롤은 하지 않는다.
pub const ENV_MENU: &str = "FACET_MEASURE_MENU";
/// 이름 조각을 주면 첫 화면 뒤에 활성 탭 폴더 아래로 그 조각의 실시간 탐색을 검색창 Enter와 같은 길로 시작하고,
/// 시작한 때부터 결과가 하나 이상 든 첫 프레임이 화면에 나갈 때까지를 잰다(`search_frame`). 이때 `rows`는 그
/// 프레임의 결과 수이고 스크롤은 하지 않는다.
pub const ENV_SEARCH: &str = "FACET_MEASURE_SEARCH";
/// 질의 글자를 주면 첫 화면 뒤 그 글자를 검색창 입력과 같은 길(`App::set_instant_query`)로 즉시 검색에 넘기고, 넘긴 때부터
/// 결과가 하나 이상 든 첫 프레임이 화면에 나갈 때까지를 잰다(`instant_frame`). 이때 `rows`는 그 프레임의 결과 수이고
/// 스크롤은 하지 않는다. 인덱서 서비스 파이프 이름은 `indexer_client::ENV_PIPE`로 넘기고, 이 변수가 있을 때만 앱이
/// 측정 모드에서도 인덱서 연결을 띄운다.
pub const ENV_INSTANT: &str = "FACET_MEASURE_INSTANT";
/// 2 이상이면 창이 이만큼 뜰 때까지를 첫 화면으로 잰다(세션 복원 콜드 스타트). 창마다 목록이 든 첫 프레임이
/// 나온 뒤, 마지막 창의 첫 프레임까지의 시간이 `first_frame`이다. `rows`는 마지막 창이 읽은 항목 수이고 스크롤과
/// 폴더 열기는 하지 않는다.
pub const ENV_WINDOWS: &str = "FACET_MEASURE_WINDOWS";
/// 0보다 크면 첫 화면 뒤에 프레임마다 이만큼의 행씩 목록 끝까지 자동으로 스크롤한다.
pub const ENV_SCROLL_ROWS: &str = "FACET_MEASURE_SCROLL_ROWS";
/// #331: 그리드 한 화면 채우기 측정.
pub const ENV_GRID: &str = "FACET_MEASURE_GRID"; // #331

/// 앱이 쓰고 측정 도구가 읽는 측정 결과.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScreenReport {
    /// 앱이 읽은 항목 수.
    pub rows: usize,
    /// 첫 프레임을 낸 창 수. [`ENV_WINDOWS`]를 줬을 때만 적는다(아니면 0).
    pub windows: usize,
    /// 실행부터 목록이 든 첫 프레임이 창에 나올 때까지.
    pub first_frame: Option<Duration>,
    /// 이미 떠 있는 창에서 폴더로 이동을 시작한 때부터 그 목록이 든 첫 프레임이 창에 나올 때까지.
    pub open_frame: Option<Duration>,
    /// 이름바꾸기를 확정한 때부터 목록에 새 이름이 든 첫 프레임이 창에 나올 때까지.
    pub rename_frame: Option<Duration>,
    /// 오른쪽 메뉴를 열 때부터 메뉴가 그려진 첫 프레임이 창에 나올 때까지.
    pub menu_frame: Option<Duration>,
    /// 실시간 탐색을 시작한 때부터 첫 결과가 든 프레임이 창에 나올 때까지.
    pub search_frame: Option<Duration>,
    /// 즉시 검색 글자를 넘긴 때부터 첫 결과가 든 프레임까지(줄 `instant_frame_us`).
    pub instant_frame: Option<Duration>,
    /// 즉시 검색이 강등됐다(줄 `instant_demoted 1`).
    pub instant_demoted: bool,
    /// #331: 그리드 한 화면이 썸네일로 다 차기까지.
    pub grid_fill: Option<Duration>,
    /// 스크롤하는 동안의 프레임 시간들.
    pub frames: Vec<Duration>,
}

impl ScreenReport {
    /// 파일에 적을 문자열.
    pub fn to_text(&self) -> String {
        let mut out = format!("rows {}\n", self.rows);
        if self.windows > 0 {
            out.push_str(&format!(
                "windows {}
",
                self.windows
            ));
        }
        if let Some(d) = self.first_frame {
            out.push_str(&format!("first_frame_us {}\n", d.as_micros()));
        }
        if let Some(d) = self.open_frame {
            out.push_str(&format!(
                "open_frame_us {}
",
                d.as_micros()
            ));
        }
        if let Some(d) = self.rename_frame {
            out.push_str(&format!(
                "rename_frame_us {}
",
                d.as_micros()
            ));
        }
        if let Some(d) = self.menu_frame {
            out.push_str(&format!(
                "menu_frame_us {}
",
                d.as_micros()
            ));
        }
        if let Some(d) = self.search_frame {
            out.push_str(&format!(
                "search_frame_us {}
",
                d.as_micros()
            ));
        }
        if let Some(d) = self.instant_frame {
            out.push_str(&format!("instant_frame_us {}\n", d.as_micros()));
        }
        if self.instant_demoted {
            out.push_str("instant_demoted 1\n");
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
                "windows" => report.windows = number as usize,
                "first_frame_us" => report.first_frame = Some(Duration::from_micros(number)),
                "open_frame_us" => report.open_frame = Some(Duration::from_micros(number)),
                "rename_frame_us" => report.rename_frame = Some(Duration::from_micros(number)),
                "menu_frame_us" => report.menu_frame = Some(Duration::from_micros(number)),
                "search_frame_us" => report.search_frame = Some(Duration::from_micros(number)),
                "instant_frame_us" => report.instant_frame = Some(Duration::from_micros(number)),
                "instant_demoted" => report.instant_demoted = number != 0,
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
            windows: 2,
            first_frame: Some(Duration::from_micros(123_456)),
            open_frame: Some(Duration::from_micros(45_678)),
            rename_frame: Some(Duration::from_micros(9_000)),
            menu_frame: Some(Duration::from_micros(20_000)),
            search_frame: Some(Duration::from_micros(40_000)),
            instant_frame: Some(Duration::from_micros(30_000)),
            instant_demoted: true,
            grid_fill: None,
            frames: vec![Duration::from_micros(16_700), Duration::from_micros(33_000)],
        };
        assert_eq!(ScreenReport::parse(&report.to_text()), Ok(report));
    }

    #[test]
    fn 첫_화면이_없으면_없는_채로_읽는다() {
        let report = ScreenReport::parse("rows 3\n").unwrap();
        assert_eq!(report.rows, 3);
        assert_eq!(report.first_frame, None);
        assert_eq!(report.open_frame, None);
        assert!(report.frames.is_empty());
    }

    #[test]
    fn 깨진_줄은_오류다() {
        assert!(ScreenReport::parse("rows abc").is_err());
        assert!(ScreenReport::parse("nope 1").is_err());
        assert!(ScreenReport::parse("rows").is_err());
    }
}
