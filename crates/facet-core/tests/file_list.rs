//! 파일 목록의 선택 이동·따라 스크롤·표시 문자열. 코어 공개 인터페이스만 부른다.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{FixedOffset, Utc};
use facet_core::file_list::{
    Movement, format_modified, format_modified_local, format_size, item_text, move_selection,
    scroll_to_show,
};
use facet_core::{Attributes, Item};

// ---- 선택 이동 ----

#[test]
fn empty_list_has_no_selection() {
    for m in [
        Movement::Up,
        Movement::Down,
        Movement::PageUp,
        Movement::PageDown,
        Movement::Home,
        Movement::End,
    ] {
        assert_eq!(move_selection(None, m, 0, 10), None);
        assert_eq!(move_selection(Some(3), m, 0, 10), None);
    }
}

#[test]
fn arrows_move_one_row_and_stop_at_ends() {
    assert_eq!(move_selection(Some(5), Movement::Down, 10, 4), Some(6));
    assert_eq!(move_selection(Some(5), Movement::Up, 10, 4), Some(4));
    assert_eq!(move_selection(Some(9), Movement::Down, 10, 4), Some(9));
    assert_eq!(move_selection(Some(0), Movement::Up, 10, 4), Some(0));
}

#[test]
fn page_keys_move_one_page_and_clamp() {
    assert_eq!(
        move_selection(Some(0), Movement::PageDown, 100, 20),
        Some(20)
    );
    assert_eq!(
        move_selection(Some(90), Movement::PageDown, 100, 20),
        Some(99)
    );
    assert_eq!(
        move_selection(Some(50), Movement::PageUp, 100, 20),
        Some(30)
    );
    assert_eq!(move_selection(Some(5), Movement::PageUp, 100, 20), Some(0));
}

#[test]
fn page_keys_move_at_least_one_row_when_viewport_is_tiny() {
    assert_eq!(move_selection(Some(3), Movement::PageDown, 10, 0), Some(4));
    assert_eq!(move_selection(Some(3), Movement::PageUp, 10, 0), Some(2));
}

#[test]
fn home_and_end_jump_to_first_and_last() {
    assert_eq!(
        move_selection(Some(50), Movement::Home, 100_000, 20),
        Some(0)
    );
    assert_eq!(
        move_selection(Some(50), Movement::End, 100_000, 20),
        Some(99_999)
    );
}

#[test]
fn without_selection_keys_select_from_the_start_or_end() {
    assert_eq!(move_selection(None, Movement::Down, 10, 4), Some(0));
    assert_eq!(move_selection(None, Movement::Up, 10, 4), Some(0));
    assert_eq!(move_selection(None, Movement::Home, 10, 4), Some(0));
    assert_eq!(move_selection(None, Movement::End, 10, 4), Some(9));
}

#[test]
fn selection_beyond_a_shrunk_list_is_clamped() {
    assert_eq!(move_selection(Some(50), Movement::Up, 10, 4), Some(8));
}

// ---- 따라 스크롤 ----

#[test]
fn visible_selection_does_not_scroll() {
    // 행 높이 20, 화면 100(5행), 지금 40에서 시작(2~6행 보임).
    assert_eq!(scroll_to_show(3, 20.0, 40.0, 100.0), 40.0);
    assert_eq!(scroll_to_show(2, 20.0, 40.0, 100.0), 40.0);
    assert_eq!(scroll_to_show(6, 20.0, 40.0, 100.0), 40.0);
}

#[test]
fn selection_above_scrolls_up_to_its_top() {
    assert_eq!(scroll_to_show(1, 20.0, 40.0, 100.0), 20.0);
    assert_eq!(scroll_to_show(0, 20.0, 40.0, 100.0), 0.0);
}

#[test]
fn selection_below_scrolls_down_until_its_bottom_shows() {
    assert_eq!(scroll_to_show(7, 20.0, 40.0, 100.0), 60.0);
    assert_eq!(scroll_to_show(99_999, 20.0, 0.0, 100.0), 1_999_900.0);
}

#[test]
fn viewport_shorter_than_a_row_shows_the_row_top() {
    assert_eq!(scroll_to_show(5, 20.0, 0.0, 10.0), 100.0);
}

// ---- 표시 문자열 ----

#[test]
fn sizes_use_bytes_then_binary_units() {
    assert_eq!(format_size(0), "0 B");
    assert_eq!(format_size(1023), "1023 B");
    assert_eq!(format_size(1024), "1.0 KB");
    assert_eq!(format_size(1536), "1.5 KB");
    assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
    assert_eq!(format_size(3 * 1024 * 1024 * 1024), "3.0 GB");
    assert_eq!(format_size(2 * 1024_u64.pow(4)), "2.0 TB");
    assert_eq!(format_size(u64::MAX), "16384.0 PB");
}

#[test]
fn modified_time_is_shown_in_the_given_time_zone() {
    // 2024-03-05 06:07:08 UTC
    let t = UNIX_EPOCH + Duration::from_secs(1_709_618_828);
    assert_eq!(format_modified(t, &Utc), "2024-03-05 06:07");
    let seoul = FixedOffset::east_opt(9 * 3600).unwrap();
    assert_eq!(format_modified(t, &seoul), "2024-03-05 15:07");
}

#[test]
fn modified_time_before_1970_does_not_panic() {
    let t = UNIX_EPOCH - Duration::from_secs(86_400);
    assert_eq!(format_modified(t, &Utc), "1969-12-31 00:00");
}

#[test]
fn modified_time_out_of_range_does_not_panic() {
    // 표현 범위를 벗어난 시각은 빈 문자열이다.
    let far = SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(u64::MAX / 4));
    if let Some(far) = far {
        assert_eq!(format_modified(far, &Utc), "");
    }
}

#[test]
fn item_text_shows_kind_size_and_local_time() {
    let modified = UNIX_EPOCH + Duration::from_secs(1_709_618_828);
    let file = Item {
        name: "한글 이름 😀.txt".into(),
        is_folder: false,
        size: 2048,
        modified,
        attributes: Attributes::default(),
    };
    let text = item_text(&file);
    assert_eq!(text.name, "한글 이름 😀.txt");
    assert_eq!(text.kind, "파일");
    assert_eq!(text.size, "2.0 KB");
    assert_eq!(text.modified, format_modified_local(modified));
    assert!(!text.modified.is_empty());

    let folder = Item {
        name: "폴더".into(),
        is_folder: true,
        size: 0,
        modified,
        attributes: Attributes::default(),
    };
    let text = item_text(&folder);
    assert_eq!(text.kind, "폴더");
    assert_eq!(text.size, "");
}
