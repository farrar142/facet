//! 탭의 선택·커서 상태와 글자로 이동의 입력 처리.
//!
//! 선택은 행 번호가 아니라 항목 이름으로 기억한다. 그래서 정렬을 바꾸거나 다시 읽어도 유지된다.
//! 이 모듈은 상태 자료형과 순수한 도우미만 둔다. 목록을 아는 동작은 [`crate::tab::Tab`]이 한다.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::time::{Duration, Instant};

/// 글자로 이동에서 이 시간 안에 이어 친 글자는 접두어로 이어 붙인다.
pub const TYPE_AHEAD_TIMEOUT: Duration = Duration::from_secs(1);

/// 커서를 옮기는 방법.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorMove {
    /// 한 줄 위.
    Up,
    /// 한 줄 아래.
    Down,
    /// 한 쪽 위. 쪽 크기(줄 수)는 부르는 쪽이 준다.
    PageUp(usize),
    /// 한 쪽 아래.
    PageDown(usize),
    /// 처음.
    First,
    /// 끝.
    Last,
}

/// 상태 표시줄 수치.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusSummary {
    /// 보이는 항목 수.
    pub item_count: usize,
    /// 선택한 항목 수.
    pub selected_count: usize,
    /// 선택한 파일의 크기 합(바이트). 폴더는 더하지 않는다.
    pub selected_size: u64,
}

/// 탭이 한 위치에서 갖는 보기 상태. 뒤로·앞으로 기록에 남긴다.
#[derive(Debug, Default)]
pub(crate) struct ViewState {
    pub selection: HashSet<OsString>,
    pub cursor: Option<OsString>,
    pub anchor: Option<OsString>,
    pub scroll: usize,
}

/// 글자로 이동의 입력 버퍼.
#[derive(Debug, Default)]
pub(crate) struct TypeAhead {
    buffer: Vec<char>,
    last: Option<Instant>,
}

/// 글자 하나를 넣은 뒤의 찾기 방법.
pub(crate) struct TypeSearch {
    /// 소문자로 접은 접두어.
    pub prefix: Vec<char>,
    /// 커서 항목 자신을 후보에 넣는가(접두어를 늘려 갈 때). 아니면 커서 다음부터 찾는다.
    pub include_cursor: bool,
}

impl TypeAhead {
    pub fn push(&mut self, c: char, now: Instant) -> TypeSearch {
        let expired = self
            .last
            .is_none_or(|last| now.saturating_duration_since(last) > TYPE_AHEAD_TIMEOUT);
        if expired {
            self.buffer.clear();
        }
        self.last = Some(now);
        self.buffer.extend(c.to_lowercase());
        let first = self.buffer[0];
        if self.buffer.iter().all(|&b| b == first) {
            // 첫 글자이거나 같은 글자를 거듭 쳤다: 그 글자로 시작하는 항목을 차례로 돈다.
            TypeSearch {
                prefix: vec![first],
                include_cursor: false,
            }
        } else {
            TypeSearch {
                prefix: self.buffer.clone(),
                include_cursor: true,
            }
        }
    }
}

/// 이름이 `prefix`(소문자로 접은 글자들)로 시작하는가. 대소문자를 가리지 않는다.
pub(crate) fn starts_with_folded(name: &OsStr, prefix: &[char]) -> bool {
    let name = name.to_string_lossy();
    let mut chars = name.chars().flat_map(char::to_lowercase);
    prefix.iter().all(|&p| chars.next() == Some(p))
}
