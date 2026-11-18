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
        todo!("#327")
    }

    /// #327: 한 단계 큰 칸 크기.
    pub fn larger(self) -> Option<GridSize> {
        todo!("#327")
    }

    /// #327: 한 단계 작은 칸 크기.
    pub fn smaller(self) -> Option<GridSize> {
        todo!("#327")
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
    pub fn for_width(_viewport_width: u32, _cell_width: u32) -> GridLayout {
        todo!("#327")
    }

    /// #327: 항목 `len`개의 행 수.
    pub fn rows(self, _len: usize) -> usize {
        todo!("#327")
    }

    /// #327: 순번의 (행, 열).
    pub fn position(self, _index: usize) -> (usize, usize) {
        todo!("#327")
    }

    /// #327: (행, 열)의 순번.
    pub fn index(self, _row: usize, _column: usize, _len: usize) -> Option<usize> {
        todo!("#327")
    }

    /// #327: 화살표 이동 뒤 순번.
    pub fn step(self, _from: usize, _movement: GridMove, _len: usize) -> usize {
        todo!("#327")
    }

    /// #327: 보이는 칸과 위아래 한 화면의 순번 범위.
    pub fn spans(self, _first_row: usize, _visible_rows: usize, _len: usize) -> GridSpans {
        todo!("#327")
    }
}
