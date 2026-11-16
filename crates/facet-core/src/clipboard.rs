//! 클립보드 형식: 탐색기와 같은 바이트·글자를 만들고 읽는 순수 함수.
//!
//! 정본은 [`facet_win::clipboard::format`]이다. 클립보드를 소유하는 STA 스레드([`facet_win::clipboard::Clipboard`])가
//! 같은 형식을 써야 하는데 코어가 그 크레이트에 기대므로, 형식은 거기 두고 여기서 그대로 다시 내보낸다.
//! 화면 문장은 없다(ADR-0007).

pub use facet_win::clipboard::format::*;
