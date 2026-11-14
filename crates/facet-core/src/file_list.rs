//! 파일 목록 화면의 판단: 키보드로 선택 옮기기, 선택을 따라 스크롤하기, 항목 표시 문자열.
//!
//! 화면 계층은 이 함수들을 부르기만 한다. 길이는 화면의 논리 픽셀 단위 `f32`다.

use std::time::SystemTime;

use chrono::{DateTime, TimeZone, Utc};

/// 선택을 옮기는 키보드 동작.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Movement {
    /// 한 행 위(↑).
    Up,
    /// 한 행 아래(↓).
    Down,
    /// 한 화면 위(Page Up).
    PageUp,
    /// 한 화면 아래(Page Down).
    PageDown,
    /// 첫 항목(Home).
    Home,
    /// 마지막 항목(End).
    End,
}

/// `count`개 항목 목록에서 `current` 선택을 `movement`만큼 옮긴 새 선택.
///
/// `page_rows`는 화면에 한 번에 보이는 행 수다. 0이어도 Page 키는 한 행은 움직인다.
/// 목록이 비면 선택이 없다(`None`). 선택이 없을 때는 End만 마지막으로, 나머지는 첫 항목으로 간다.
pub fn move_selection(
    current: Option<usize>,
    movement: Movement,
    count: usize,
    page_rows: usize,
) -> Option<usize> {
    let last = count.checked_sub(1)?;
    let page = page_rows.max(1);
    let Some(current) = current.map(|c| c.min(last)) else {
        return Some(if movement == Movement::End { last } else { 0 });
    };
    Some(match movement {
        Movement::Up => current.saturating_sub(1),
        Movement::Down => current.saturating_add(1).min(last),
        Movement::PageUp => current.saturating_sub(page),
        Movement::PageDown => current.saturating_add(page).min(last),
        Movement::Home => 0,
        Movement::End => last,
    })
}

/// `index`번 행이 화면에 다 보이도록 하는 새 스크롤 위치(목록 맨 위에서 화면 맨 위까지의 거리).
///
/// 모든 행 높이가 `row_height`로 같다고 본다. 이미 보이면 `offset`을 그대로 돌려준다.
/// 위로 벗어났으면 그 행이 맨 위에, 아래로 벗어났으면 그 행이 맨 아래에 오게 한다.
/// 화면이 한 행보다 낮으면 행의 위쪽을 보인다.
pub fn scroll_to_show(index: usize, row_height: f32, offset: f32, viewport_height: f32) -> f32 {
    let top = index as f32 * row_height;
    let bottom = top + row_height;
    if top < offset || viewport_height < row_height {
        top
    } else if bottom > offset + viewport_height {
        bottom - viewport_height
    } else {
        offset
    }
}

/// 바이트 크기를 사람이 읽는 문자열로. 1024 미만은 바이트, 그 위는 1024 단위로 소수 한 자리.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// 수정 시각을 `tz` 시간대의 `YYYY-MM-DD HH:MM`으로. 표현할 수 없는 시각은 빈 문자열이다.
pub fn format_modified<Tz: TimeZone>(time: SystemTime, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let utc = match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs())
            .ok()
            .and_then(|s| DateTime::<Utc>::from_timestamp(s, after.subsec_nanos())),
        Err(before) => {
            let before = before.duration();
            // 1970년 이전: 초를 내림해 분 단위가 맞게 한다.
            let secs = i64::try_from(before.as_secs())
                .ok()
                .and_then(|s| s.checked_add(i64::from(before.subsec_nanos() > 0)));
            secs.and_then(|s| DateTime::<Utc>::from_timestamp(-s, 0))
        }
    };
    utc.map(|t| t.with_timezone(tz).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}
