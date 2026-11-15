//! 인덱서 서비스 실행 파일(`facet-indexer`)의 라이브러리 쪽.
//!
//! `unsafe`와 Windows API는 작업 공간에서 이 크레이트에만 둔다(spec #22). 그것은 [`volume`]
//! (볼륨 핸들, 드라이브 문자 목록), [`pipe`](질의 파이프 서버), [`service`](서비스 관리자와의
//! 대화, 설치·제거)에 모으고 Windows에서만 컴파일한다. 나머지는 플랫폼과 상관없는 순수 코드라
//! 어디서나 테스트한다.
//!
//! 지금 있는 동작:
//!
//! - 서비스 본체(하위 명령 `service`, 서비스 관리자가 띄운다). 질의 파이프 서버를 띄우고,
//!   드라이브 문자가 있는 로컬 NTFS 볼륨마다 MFT 전체를 읽어 첫 인덱스를 만들고([`build`]) USN
//!   저널을 따라가 변경을 적용한다. 따라갈 수 없게 되면 스스로 다시 만든다([`follow`]).
//!   볼륨 여럿의 인덱스와 상태는 [`volumes::Volumes`]가 들고 질의에 답한다. NTFS가 아닌 볼륨은
//!   상태 질의에 「인덱스 없음(까닭)」으로 보인다. 인덱스는 볼륨마다 `%ProgramData%\facet\index\`
//!   ([`index_dir`], SYSTEM과 Administrators만 쓴다)에 저장해 다시 뜬 뒤 읽고 꺼져 있던 동안의
//!   변경을 따라잡는다([`follow::IndexFile`]).
//! - 개발용 설치·제거(하위 명령 `install`·`uninstall`, 관리자 권한). 제거는 인덱스 폴더도 지운다.
//! - 개발용 하위 명령 `dump-mft`. 관리자 권한으로 NTFS 볼륨을 열어 MFT의 원시 레코드를
//!   덩어리째 읽어 파일로 떠 낸다. MFT 파서 테스트에 쓰는 「MFT 조각」을 만드는 데
//!   쓴다(`testdata/mft/README.md`).
//! - 질의 파이프 서버([`pipe::PipeServer`])와 연결 하나의 대화([`session`]). 개발용 하위 명령
//!   `serve-sample`이 견본 인덱스로 서버를 띄운다.

pub mod build;
pub mod cli;
pub mod dump;
pub mod follow;
#[cfg(windows)]
pub mod index_dir;
pub mod layout;
#[cfg(windows)]
pub mod pipe;
pub mod sample;
#[cfg(windows)]
pub mod service;
pub mod session;
#[cfg(windows)]
pub mod volume;
pub mod volumes;

/// 인덱서 서비스의 서비스 이름(서비스 관리자에 등록하는 이름).
pub const SERVICE_NAME: &str = "facet-indexer";

/// 서비스 관리 창에 보이는 이름.
pub const SERVICE_DISPLAY_NAME: &str = "facet 인덱서 서비스";

/// 서비스 관리 창에 보이는 설명.
pub const SERVICE_DESCRIPTION: &str =
    "로컬 NTFS 볼륨의 MFT를 읽어 파일 이름 인덱스를 만들고 facet의 질의에 답합니다.";
