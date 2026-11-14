//! 탐색기의 이름 비교(`StrCmpLogicalW`) 래퍼를 몇 가지 이름 쌍으로 확인한다.
#![cfg(windows)]

use std::cmp::Ordering;

use facet_win::compare_names_like_explorer;

#[test]
fn 숫자는_값으로_비교한다() {
    assert_eq!(
        compare_names_like_explorer("file2", "file10"),
        Ordering::Less
    );
    assert_eq!(
        compare_names_like_explorer("file10", "file2"),
        Ordering::Greater
    );
    assert_eq!(
        compare_names_like_explorer("파일2", "파일10"),
        Ordering::Less
    );
    assert_eq!(
        compare_names_like_explorer("v1.9.txt", "v1.10.txt"),
        Ordering::Less
    );
}

#[test]
fn 대소문자를_가리지_않는다() {
    assert_eq!(compare_names_like_explorer("abc", "ABC"), Ordering::Equal);
    assert_eq!(compare_names_like_explorer("a", "B"), Ordering::Less);
    assert_eq!(compare_names_like_explorer("B", "a"), Ordering::Greater);
}

#[test]
fn 같은_이름은_같다() {
    assert_eq!(compare_names_like_explorer("", ""), Ordering::Equal);
    assert_eq!(
        compare_names_like_explorer("보고서.docx", "보고서.docx"),
        Ordering::Equal
    );
}

#[test]
fn 앞부분이_같으면_짧은_쪽이_앞이다() {
    assert_eq!(compare_names_like_explorer("abc", "abcd"), Ordering::Less);
    assert_eq!(compare_names_like_explorer("", "a"), Ordering::Less);
}
