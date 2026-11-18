//! #327: 보기 방식(자세히 보기·그리드 보기)과 그리드 격자 계산.

use std::ops::Range;

/// #327: 보기 방식.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ViewMode {
    #[default]
    Details,
    Grid,
}

/// #327: 그리드 칸 크기.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GridSize {
    Small,
    #[default]
    Medium,
    Large,
    ExtraLarge,
}

impl GridSize {
    /// #327: 작은 것부터 모든 칸 크기.
    pub const ALL: [GridSize; 4] = [
        GridSize::Small,
        GridSize::Medium,
        GridSize::Large,
        GridSize::ExtraLarge,
    ];

    /// #327: 이 칸 크기의 썸네일 한 변(화소).
    pub fn thumbnail_side(self) -> u32 {
        match self {
            GridSize::Small => 64,
            GridSize::Medium => 96,
            GridSize::Large => 160,
            GridSize::ExtraLarge => 256,
        }
    }

    /// #327: 한 단계 큰 칸 크기.
    pub fn larger(self) -> Option<GridSize> {
        match self {
            GridSize::Small => Some(GridSize::Medium),
            GridSize::Medium => Some(GridSize::Large),
            GridSize::Large => Some(GridSize::ExtraLarge),
            GridSize::ExtraLarge => None,
        }
    }

    /// #327: 한 단계 작은 칸 크기.
    pub fn smaller(self) -> Option<GridSize> {
        match self {
            GridSize::Small => None,
            GridSize::Medium => Some(GridSize::Small),
            GridSize::Large => Some(GridSize::Medium),
            GridSize::ExtraLarge => Some(GridSize::Large),
        }
    }
}

/// #327: 탭의 보기.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct View {
    pub mode: ViewMode,
    pub grid_size: GridSize,
}

/// #327: 그리드의 화살표 이동.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridMove {
    Left,
    Right,
    Up,
    Down,
}

/// #327: 그리드 격자(열 수).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridLayout {
    pub columns: usize,
}

/// #327: 보이는 칸과 위아래 한 화면의 순번 범위.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridSpans {
    pub visible: Range<usize>,
    pub nearby: Range<usize>,
}

impl GridLayout {
    /// #327: 창 폭과 칸 폭으로 격자를 정한다(최소 한 열).
    pub fn for_width(viewport_width: u32, cell_width: u32) -> GridLayout {
        let columns = viewport_width.checked_div(cell_width).unwrap_or(1) as usize;
        GridLayout {
            columns: columns.max(1),
        }
    }

    /// #327: 항목 `len`개의 행 수.
    pub fn rows(self, len: usize) -> usize {
        len.div_ceil(self.columns)
    }

    /// #327: 순번의 (행, 열).
    pub fn position(self, index: usize) -> (usize, usize) {
        (index / self.columns, index % self.columns)
    }

    /// #327: (행, 열)의 순번.
    pub fn index(self, row: usize, column: usize, len: usize) -> Option<usize> {
        if column >= self.columns {
            return None;
        }
        let index = row.checked_mul(self.columns)?.checked_add(column)?;
        (index < len).then_some(index)
    }

    /// #327: 화살표 이동 뒤 순번.
    pub fn step(self, from: usize, movement: GridMove, len: usize) -> usize {
        if len == 0 {
            return 0;
        }
        let last = len - 1;
        let from = from.min(last);
        match movement {
            GridMove::Left => from.saturating_sub(1),
            GridMove::Right => (from + 1).min(last),
            GridMove::Up => {
                if from >= self.columns {
                    from - self.columns
                } else {
                    from
                }
            }
            GridMove::Down => {
                let below = from + self.columns;
                if below <= last {
                    below
                } else if self.position(from).0 + 1 < self.rows(len) {
                    last
                } else {
                    from
                }
            }
        }
    }

    /// #327: 보이는 칸과 위아래 한 화면의 순번 범위.
    pub fn spans(self, first_row: usize, visible_rows: usize, len: usize) -> GridSpans {
        let rows = self.rows(len);
        let first = first_row.min(rows);
        let end_row = (first + visible_rows).min(rows);
        let cell = |row: usize| (row * self.columns).min(len);
        let near_first = first.saturating_sub(visible_rows);
        let near_end = (end_row + visible_rows).min(rows);
        GridSpans {
            visible: cell(first)..cell(end_row),
            nearby: cell(near_first)..cell(near_end),
        }
    }
}
