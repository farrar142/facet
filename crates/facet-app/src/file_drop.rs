//! 다른 앱에서 끌어 온 파일을 놓을 자리: 창 기준 점이 어느 패널의 어느 행 위인지 정하는 순수 계산.
//!
//! - 패널의 목록 행 위: 폴더 행이면 그 폴더([`Place::FolderRow`]), 파일 행이면 놓을 수 없음([`Place::FileRow`]).
//! - 탭 줄의 탭 위: 그 탭 위치([`Place::Tab`]). 0.5초 머물면 그 탭이 활성 탭이 된다([`Dwell`]).
//! - 주소창의 경로 조각 위: 그 조각의 위치([`Place::Crumb`], 상위 폴더).
//! - 패널 안의 그 밖의 곳(탭 줄 위 끝부터 목록 아래 끝까지: 탭 줄의 빈 곳, 주소창의 빈 곳, 머리글, 목록의 빈 곳): 그
//!   패널 활성 탭 위치([`Place::Panel`]).
//! - 어느 패널도 아닌 곳(작업 영역, 상태 표시줄, 패널 사이, 목록의 스크롤 막대): 놓을 수 없음([`Place::Outside`]).
//!
//! 복사·이동·놓을 수 없음은 코어의 끌어 놓기 결과 결정(`facet_core::dragdrop::decide`)이 정한다.

use std::time::{Duration, Instant};

use crate::marquee::List;
use crate::tab_drag::{Point, Strip};

/// 끌어 온 항목이 탭 위에 머물러 그 탭이 활성 탭이 되기까지 걸리는 시간. 탭을 지나가며 우연히 열리지 않을 만큼
/// 길고(손이 탭 줄을 가로지르는 데 0.2초 안팎), 일부러 기다릴 때 답답하지 않을 만큼 짧은 값이다. 탐색기가 폴더
/// 위에서 폴더를 여는 머묾(Windows 기본 마우스 머묾 0.4초, 탐색기 폴더 자동 열림 0.5~0.8초)과 같은 감각이다.
pub const TAB_DWELL: Duration = Duration::from_millis(500);

/// 주소창의 경로 조각 하나의 가로 자리(창 기준).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrumbArea {
    pub left: f32,
    pub width: f32,
}

/// 패널 하나의 자리: 패널 위 끝(탭 줄 위, 창 기준), 탭 줄, 주소창의 세로 범위와 경로 조각, 목록 자리. 가로 범위는
/// 목록의 가로 범위를 쓴다.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelArea {
    pub top: f32,
    pub strip: Strip,
    /// 주소창의 위 끝과 높이.
    pub address_top: f32,
    pub address_height: f32,
    pub crumbs: Vec<CrumbArea>,
    pub list: List,
}

impl PanelArea {
    fn contains(&self, at: Point) -> bool {
        let list = &self.list;
        list.width > 0.0
            && at.x >= list.left
            && at.x < list.left + list.width
            && at.y >= self.top
            && at.y < list.top + list.height
    }

    /// 점 아래 탭(그 패널 안 번호). 탭 줄 높이 안이고 탭이 있는 곳이어야 한다(빈 곳은 `None`).
    fn tab_at(&self, at: Point) -> Option<usize> {
        let strip = &self.strip;
        let inside = at.x >= strip.left
            && at.x < strip.left + strip.width
            && at.y >= strip.top
            && at.y < strip.top + strip.height;
        if !inside || strip.tab_width <= 0.0 {
            return None;
        }
        let along = ((at.x - strip.left + strip.scroll) / strip.tab_width).floor();
        (along >= 0.0 && (along as usize) < strip.tabs).then_some(along as usize)
    }

    /// 점 아래 경로 조각(번호).
    fn crumb_at(&self, at: Point) -> Option<usize> {
        if at.y < self.address_top || at.y >= self.address_top + self.address_height {
            return None;
        }
        self.crumbs
            .iter()
            .position(|crumb| at.x >= crumb.left && at.x < crumb.left + crumb.width)
    }

    /// 점 아래 목록 행(항목 번호). 목록의 보이는 영역 밖이거나 마지막 행 아래면 `None`.
    fn row_at(&self, at: Point) -> Option<usize> {
        let list = &self.list;
        if at.y < list.top || at.y >= list.top + list.height || list.row_height <= 0.0 {
            return None;
        }
        let y = at.y - list.top + list.scroll;
        let row = (y / list.row_height).floor();
        (row >= 0.0 && (row as usize) < list.rows).then_some(row as usize)
    }
}

/// 놓을 자리.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// 패널 `panel`의 폴더 행 `row`(항목 번호).
    FolderRow { panel: usize, row: usize },
    /// 패널 `panel`의 활성 탭 위치.
    Panel { panel: usize },
    /// 패널 `panel`의 `tab`번째 탭의 위치.
    Tab { panel: usize, tab: usize },
    /// 패널 `panel`의 활성 탭 주소창 경로 조각 `crumb`(`Provider::segments`의 번호)의 위치.
    Crumb { panel: usize, crumb: usize },
    /// 파일 행 위: 놓을 수 없음.
    FileRow,
    /// 어느 패널도 아님: 놓을 수 없음.
    Outside,
}

/// 창 기준 점 `at`의 놓을 자리. `panels`는 패널마다 자리(없는 패널은 `None`), `is_folder(panel, row)`는 그 행이
/// 폴더인가.
pub fn place_at(
    at: Point,
    panels: &[Option<PanelArea>],
    is_folder: impl Fn(usize, usize) -> bool,
) -> Place {
    for (panel, area) in panels.iter().enumerate() {
        let Some(area) = area else { continue };
        if !area.contains(at) {
            continue;
        }
        if let Some(tab) = area.tab_at(at) {
            return Place::Tab { panel, tab };
        }
        if let Some(crumb) = area.crumb_at(at) {
            return Place::Crumb { panel, crumb };
        }
        return match area.row_at(at) {
            Some(row) if is_folder(panel, row) => Place::FolderRow { panel, row },
            Some(_) => Place::FileRow,
            None => Place::Panel { panel },
        };
    }
    Place::Outside
}

/// 탭 위 머묾 판정. 시각은 부르는 쪽이 넘긴다. 같은 탭 위에 [`TAB_DWELL`] 동안 머물면 한 번 `true`를 돌려주고,
/// 탭을 떠나거나 다른 탭으로 옮기면 처음부터 센다.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Dwell {
    /// 지금 머무는 탭(패널, 탭 번호), 머문 때, 이미 알렸는지.
    on: Option<(usize, usize, Instant, bool)>,
}

impl Dwell {
    /// 끄는 점이 `over`(패널, 탭 번호) 위라고 알린다(탭 위가 아니면 `None`). 머문 시간이 찼으면 `true`.
    pub fn update(&mut self, over: Option<(usize, usize)>, now: Instant) -> bool {
        let Some((panel, tab)) = over else {
            self.on = None;
            return false;
        };
        match &mut self.on {
            Some((p, t, since, opened)) if (*p, *t) == (panel, tab) => {
                if !*opened && now.saturating_duration_since(*since) >= TAB_DWELL {
                    *opened = true;
                    return true;
                }
                false
            }
            _ => {
                self.on = Some((panel, tab, now, false));
                false
            }
        }
    }

    /// 끌기가 끝났다.
    pub fn reset(&mut self) {
        self.on = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 왼쪽 패널: x 0..400, 탭 줄 위 끝 0, 목록 y 60..300(행 24, 행 20개, 48 넘김). 오른쪽 패널: x 410..800, 행 3개.
    fn panels() -> [Option<PanelArea>; 2] {
        [
            Some(PanelArea {
                top: 0.0,
                strip: Strip {
                    left: 0.0,
                    top: 0.0,
                    width: 400.0,
                    height: 28.0,
                    tab_width: 100.0,
                    scroll: 0.0,
                    tabs: 3,
                },
                address_top: 30.0,
                address_height: 28.0,
                crumbs: vec![
                    CrumbArea {
                        left: 0.0,
                        width: 40.0,
                    },
                    CrumbArea {
                        left: 50.0,
                        width: 60.0,
                    },
                ],
                list: List {
                    left: 0.0,
                    top: 60.0,
                    width: 400.0,
                    height: 240.0,
                    scroll: 48.0,
                    scroll_x: 0.0,
                    row_height: 24.0,
                    rows: 20,
                },
            }),
            Some(PanelArea {
                top: 0.0,
                strip: Strip {
                    left: 410.0,
                    top: 0.0,
                    width: 390.0,
                    height: 28.0,
                    tab_width: 100.0,
                    scroll: 0.0,
                    tabs: 1,
                },
                address_top: 30.0,
                address_height: 28.0,
                crumbs: vec![CrumbArea {
                    left: 410.0,
                    width: 40.0,
                }],
                list: List {
                    left: 410.0,
                    top: 60.0,
                    width: 390.0,
                    height: 240.0,
                    scroll: 0.0,
                    scroll_x: 0.0,
                    row_height: 24.0,
                    rows: 3,
                },
            }),
        ]
    }

    fn at(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    /// 짝수 행이 폴더.
    fn even_folders(_: usize, row: usize) -> bool {
        row.is_multiple_of(2)
    }

    #[test]
    fn 폴더_행_위는_그_폴더이고_넘긴_만큼_뒤의_행이다() {
        // 목록 위에서 0px이면 넘긴 48px 때문에 세 번째 행(2).
        assert_eq!(
            place_at(at(10.0, 60.0), &panels(), even_folders),
            Place::FolderRow { panel: 0, row: 2 }
        );
        assert_eq!(
            place_at(at(500.0, 60.0 + 24.0 * 2.0 + 1.0), &panels(), even_folders),
            Place::FolderRow { panel: 1, row: 2 }
        );
    }

    #[test]
    fn 파일_행_위는_놓을_수_없다() {
        assert_eq!(
            place_at(at(10.0, 60.0 + 24.0), &panels(), even_folders),
            Place::FileRow
        );
    }

    #[test]
    fn 마지막_행_아래의_빈_곳과_탭_줄은_그_패널_위치다() {
        assert_eq!(
            place_at(at(500.0, 60.0 + 24.0 * 3.0), &panels(), even_folders),
            Place::Panel { panel: 1 }
        );
        // 탭 줄의 탭 없는 곳(탭 3개 뒤), 주소창의 조각 없는 곳.
        for point in [at(350.0, 5.0), at(200.0, 40.0)] {
            assert_eq!(
                place_at(point, &panels(), even_folders),
                Place::Panel { panel: 0 },
                "{point:?}"
            );
        }
    }

    #[test]
    fn 패널_밖은_놓을_수_없다() {
        // 패널 사이 틈, 목록 아래(작업 영역), 창 밖.
        for point in [at(405.0, 100.0), at(10.0, 300.0), at(-1.0, 100.0)] {
            assert_eq!(
                place_at(point, &panels(), even_folders),
                Place::Outside,
                "{point:?}"
            );
        }
    }

    #[test]
    fn 나누지_않은_창의_너비_0_패널은_건너뛴다() {
        let mut panels = panels();
        if let Some(area) = panels[1].as_mut() {
            area.list.width = 0.0;
        }
        assert_eq!(
            place_at(at(500.0, 100.0), &panels, even_folders),
            Place::Outside
        );
    }

    #[test]
    fn 탭_위는_그_탭이고_넘긴_만큼_뒤의_탭이다() {
        assert_eq!(
            place_at(at(150.0, 5.0), &panels(), even_folders),
            Place::Tab { panel: 0, tab: 1 }
        );
        assert_eq!(
            place_at(at(415.0, 5.0), &panels(), even_folders),
            Place::Tab { panel: 1, tab: 0 }
        );
        let mut panels = panels();
        if let Some(area) = panels[0].as_mut() {
            area.strip.scroll = 100.0;
        }
        assert_eq!(
            place_at(at(10.0, 5.0), &panels, even_folders),
            Place::Tab { panel: 0, tab: 1 }
        );
    }

    #[test]
    fn 주소창_조각_위는_그_조각이다() {
        assert_eq!(
            place_at(at(10.0, 40.0), &panels(), even_folders),
            Place::Crumb { panel: 0, crumb: 0 }
        );
        assert_eq!(
            place_at(at(60.0, 40.0), &panels(), even_folders),
            Place::Crumb { panel: 0, crumb: 1 }
        );
        // 조각 사이 틈은 패널 위치다.
        assert_eq!(
            place_at(at(45.0, 40.0), &panels(), even_folders),
            Place::Panel { panel: 0 }
        );
    }

    fn after(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn 같은_탭_위에_0_5초_머물면_한_번_열린다() {
        let t0 = Instant::now();
        let mut dwell = Dwell::default();
        assert!(!dwell.update(Some((0, 1)), t0));
        assert!(!dwell.update(Some((0, 1)), after(t0, 499)));
        assert!(dwell.update(Some((0, 1)), after(t0, 500)));
        // 이미 열었으면 계속 머물러도 다시 알리지 않는다.
        assert!(!dwell.update(Some((0, 1)), after(t0, 900)));
    }

    #[test]
    fn 탭을_떠나거나_다른_탭으로_옮기면_처음부터_센다() {
        let t0 = Instant::now();
        let mut dwell = Dwell::default();
        dwell.update(Some((0, 1)), t0);
        assert!(!dwell.update(Some((0, 2)), after(t0, 400)));
        assert!(!dwell.update(Some((0, 2)), after(t0, 800)));
        assert!(dwell.update(Some((0, 2)), after(t0, 900)));
        // 떠났다 돌아오면 처음부터.
        assert!(!dwell.update(None, after(t0, 950)));
        assert!(!dwell.update(Some((0, 2)), after(t0, 1000)));
        assert!(!dwell.update(Some((0, 2)), after(t0, 1400)));
        assert!(dwell.update(Some((0, 2)), after(t0, 1500)));
        dwell.reset();
        assert!(!dwell.update(Some((0, 2)), after(t0, 1600)));
    }
}
