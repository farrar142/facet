//! 코어의 자연 정렬 비교가 탐색기의 이름 비교(`StrCmpLogicalW`, 래퍼 크레이트)와 같은지 확인한다.
//! Windows에서만 돈다.
#![cfg(windows)]

mod common;

use common::names::{ORDERED, SAME_IGNORING_CASE};
use facet_core::details::compare_names;
use facet_win::compare_names_like_explorer;

#[test]
fn 이름_표의_모든_쌍에서_탐색기와_같다() {
    let mut names: Vec<&str> = ORDERED.to_vec();
    for (a, b) in SAME_IGNORING_CASE {
        names.push(a);
        names.push(b);
    }
    for a in &names {
        for b in &names {
            assert_eq!(
                compare_names(a, b),
                compare_names_like_explorer(a, b),
                "{a:?}와 {b:?}"
            );
        }
    }
}

#[test]
fn 탐색기도_이름_표를_같은_순서로_본다() {
    let mut shuffled: Vec<&str> = ORDERED.iter().rev().copied().collect();
    shuffled.sort_by(|a, b| compare_names_like_explorer(a, b));
    assert_eq!(shuffled, ORDERED);
}
