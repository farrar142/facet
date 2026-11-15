//! 인덱서 서비스 실행 파일(`facet-indexer`)의 라이브러리 쪽.
//!
//! `unsafe`와 Windows API는 작업 공간에서 이 크레이트에만 둔다(spec #22). 그것은 [`volume`]
//! (볼륨 핸들)과 [`pipe`](질의 파이프 서버)에 모으고 Windows에서만 컴파일한다. 나머지는
//! 플랫폼과 상관없는 순수 코드라 어디서나 테스트한다.
//!
//! 지금 있는 동작:
//!
//! - 개발용 하위 명령 `dump-mft`. 관리자 권한으로 NTFS 볼륨을 열어 MFT의 원시 레코드를
//!   덩어리째 읽어 파일로 떠 낸다. MFT 파서 테스트에 쓰는 「MFT 조각」을 만드는 데
//!   쓴다(`testdata/mft/README.md`).
//! - 질의 파이프 서버([`pipe::PipeServer`])와 연결 하나의 대화([`session`]). 지금은 메모리의
//!   인덱스([`session::MemoryBackend`])에 답한다. 개발용 하위 명령 `serve-sample`이 견본 인덱스로
//!   서버를 띄운다.

pub mod cli;
pub mod dump;
pub mod layout;
#[cfg(windows)]
pub mod pipe;
pub mod sample;
pub mod session;
#[cfg(windows)]
pub mod volume;
