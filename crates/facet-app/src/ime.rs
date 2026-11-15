//! 한글 조합(preedit) 시작 때 선택을 지우는 도우미.
//!
//! Slint `TextInput`은 선택이 있어도 조합 중인 글자를 커서 자리에 끼워 넣을 뿐
//! 선택을 지우지 않는다(조합을 확정해야 선택이 바뀐다). 그래서 조합이 시작될 때
//! 앱이 선택을 먼저 지운다.

/// `text`에서 바이트 위치 `anchor`와 `cursor` 사이(순서 무관)를 지운 글자와, 지운 자리(바이트)를 준다.
/// 위치가 글자 경계가 아니거나 범위를 벗어나면 가까운 경계로 맞춘다.
pub fn remove_selection(text: &str, anchor: i32, cursor: i32) -> (String, i32) {
    let clamp = |at: i32| {
        let mut at = usize::try_from(at).unwrap_or(0).min(text.len());
        while !text.is_char_boundary(at) {
            at -= 1;
        }
        at
    };
    let (a, c) = (clamp(anchor), clamp(cursor));
    let (start, end) = (a.min(c), a.max(c));
    let mut out = String::with_capacity(text.len() - (end - start));
    out.push_str(&text[..start]);
    out.push_str(&text[end..]);
    (out, start as i32)
}

#[cfg(test)]
mod tests {
    use super::remove_selection;

    #[test]
    fn whole_name_selected_becomes_empty() {
        assert_eq!(remove_selection("sample.txt", 0, 10), (String::new(), 0));
        assert_eq!(remove_selection("sample.txt", 10, 0), (String::new(), 0));
    }

    #[test]
    fn partial_selection_keeps_the_rest() {
        assert_eq!(
            remove_selection("sample.txt", 0, 6),
            (".txt".to_string(), 0)
        );
        assert_eq!(remove_selection("가나다", 3, 6), ("가다".to_string(), 3));
    }

    #[test]
    fn no_selection_changes_nothing() {
        assert_eq!(remove_selection("abc", 2, 2), ("abc".to_string(), 2));
    }

    #[test]
    fn out_of_range_or_mid_char_positions_are_clamped() {
        assert_eq!(remove_selection("가나", 0, 99), (String::new(), 0));
        assert_eq!(remove_selection("가나", -5, 4), ("나".to_string(), 0));
    }
}
