//! facet의 질의 프로토콜: 인덱서 서비스와 클라이언트(시험용 질의 명령, facet 앱)가 주고받는
//! 메시지, 그 이진 형식, 길이 접두 프레임, 버전 인사(spec #22의 크레이트 (2)).
//!
//! Windows API를 쓰지 않는 순수 크레이트다. 바이트 스트림([`std::io::Read`]·[`std::io::Write`])
//! 위에서 동작하므로 파이프 없이 테스트한다.
//!
//! 연결 순서:
//!
//! 1. 클라이언트가 [`Request::Hello`]에 자기 버전([`PROTOCOL_VERSION`])을 담아 보낸다.
//! 2. 서비스는 [`answer_hello`]로 받아들이거나, 지원 버전 범위를 담아 거절한다.
//! 3. 받아들였으면 클라이언트는 [`Request::Query`]·[`Request::Resume`]·[`Request::Status`]를
//!    보내고 응답을 하나씩 받는다.
//!
//! ```
//! use facet_protocol::{
//!     answer_hello, read_message, write_message, Case, Condition, PROTOCOL_VERSION,
//!     QueryMessage, Request, Response,
//! };
//!
//! let mut pipe = Vec::new();
//! write_message(&mut pipe, &Request::Hello { version: PROTOCOL_VERSION }).unwrap();
//! let query = QueryMessage {
//!     condition: Condition::NameContains { part: "보고서".into(), case: Case::Insensitive },
//!     sort: None,
//!     limit: Some(100),
//! };
//! write_message(&mut pipe, &Request::Query(query.clone())).unwrap();
//!
//! let mut stream = pipe.as_slice();
//! let Request::Hello { version } = read_message(&mut stream).unwrap() else { panic!() };
//! assert_eq!(answer_hello(version), Response::HelloAccepted { version: PROTOCOL_VERSION });
//! let Request::Query(got) = read_message(&mut stream).unwrap() else { panic!() };
//! assert_eq!(got, query);
//! let _index_query = got.to_query().unwrap(); // facet_index::Query
//! ```

mod convert;
mod frame;
mod message;
mod wire;

pub use frame::{MAX_FRAME_LEN, ReadError, WriteError, read_frame, write_frame};
pub use message::{
    Case, Condition, ErrorKind, ErrorMessage, HitMessage, Message, NoIndexReason, Order,
    PageMessage, QueryMessage, Request, Response, ResumeMessage, SortKey, StatusMessage,
    VolumeState, VolumeStatus,
};
pub use wire::{DecodeError, MAX_DEPTH};

use std::io::{Read, Write};

/// 이 크레이트가 말하는 프로토콜 버전. 메시지 형식을 바꾸면 올린다.
pub const PROTOCOL_VERSION: u16 = 1;

/// 서비스가 받아들이는 가장 낮은 클라이언트 버전.
pub const MIN_SUPPORTED_VERSION: u16 = 1;

/// 서비스가 받아들이는 가장 높은 클라이언트 버전.
pub const MAX_SUPPORTED_VERSION: u16 = PROTOCOL_VERSION;

/// 클라이언트의 버전 인사에 대한 서비스의 답. 지원 범위 안이면 받아들이고, 아니면 지원 버전
/// 범위를 담아 거절한다.
pub fn answer_hello(client_version: u16) -> Response {
    if (MIN_SUPPORTED_VERSION..=MAX_SUPPORTED_VERSION).contains(&client_version) {
        Response::HelloAccepted {
            version: client_version,
        }
    } else {
        Response::HelloRejected {
            client_version,
            min_version: MIN_SUPPORTED_VERSION,
            max_version: MAX_SUPPORTED_VERSION,
        }
    }
}

/// 메시지 하나를 프레임으로 쓴다. 본문이 [`MAX_FRAME_LEN`]을 넘으면 쓰지 않고 오류다.
pub fn write_message(w: &mut impl Write, message: &impl Message) -> Result<(), WriteError> {
    write_frame(w, &message.to_bytes())
}

/// 프레임 하나를 읽어 메시지로 푼다. 너무 크거나 잘리거나 깨진 프레임은 오류 값이다.
pub fn read_message<M: Message>(r: &mut impl Read) -> Result<M, ReadError> {
    let body = read_frame(r)?;
    Ok(M::from_bytes(&body)?)
}
