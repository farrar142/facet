//! 다른 앱에서 끌어 온 파일을 놓을 자리: 창 기준 점이 어느 패널의 어느 행 위인지 정하는 순수 계산.
//!
//! - 패널의 목록 행 위: 폴더 행이면 그 폴더([`Place::FolderRow`]), 파일 행이면 놓을 수 없음([`Place::FileRow`]).
//! - 패널 안의 그 밖의 곳(탭 줄 위 끝부터 목록 아래 끝까지: 탭 줄, 주소창, 머리글, 목록의 빈 곳): 그 패널 활성 탭
//!   위치([`Place::Panel`]). 탭 위·주소창 조각 위에 놓아 그곳으로 보내는 것은 뒤 티켓이다.
//! - 어느 패널도 아닌 곳(작업 영역, 상태 표시줄, 패널 사이, 목록의 스크롤 막대): 놓을 수 없음([`Place::Outside`]).
//!
//! 복사·이동·놓을 수 없음은 코어의 끌어 놓기 결과 결정(`facet_core::dragdrop::decide`)이 정한다.

use crate::marquee::List;
use crate::tab_drag::Point;

/// 패널 하나의 자리: 패널 위 끝(탭 줄 위, 창 기준)과 목록 자리. 가로 범위는 목록의 가로 범위를 쓴다.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelArea {
    pub top: f32,
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
        return match area.row_at(at) {
            Some(row) if is_folder(panel, row) => Place::FolderRow { panel, row },
            Some(_) => Place::FileRow,
            None => Place::Panel { panel },
        };
    }
    Place::Outside
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 왼쪽 패널: x 0..400, 탭 줄 위 끝 0, 목록 y 60..300(행 24, 행 20개, 48 넘김). 오른쪽 패널: x 410..800, 행 3개.
    fn panels() -> [Option<PanelArea>; 2] {
        [
            Some(PanelArea {
                top: 0.0,
                list: List {
                    left: 0.0,
                    top: 60.0,
                    width: 400.0,
                    height: 240.0,
                    scroll: 48.0,
                    row_height: 24.0,
                    rows: 20,
                },
            }),
            Some(PanelArea {
                top: 0.0,
                list: List {
                    left: 410.0,
                    top: 60.0,
                    width: 390.0,
                    height: 240.0,
                    scroll: 0.0,
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
        assert_eq!(
            place_at(at(10.0, 5.0), &panels(), even_folders),
            Place::Panel { panel: 0 }
        );
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
}
