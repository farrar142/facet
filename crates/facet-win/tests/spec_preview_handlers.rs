// target: crates/facet-win/tests/spec_preview_handlers.rs
// AC: #342: `has_preview_handler`는 등록되지 않은 확장자에 `false`
//! 셸 미리보기 처리기 조회(`facet_win::preview::has_preview_handler`, 레지스트리만 읽고 처리기를 싣지 않음)를 확인한다.
//! 어떤 처리기가 설치돼 있는지는 기계마다 달라 등록되지 않은 확장자만 본다. 모든 환경에서 돈다(Windows가 아니면 늘 `false`).

use facet_win::preview::has_preview_handler;

#[test]
fn an_unregistered_extension_has_no_preview_handler() {
    assert!(!has_preview_handler("facet-no-such-extension-7f3a"));
    assert!(!has_preview_handler(""));
}
