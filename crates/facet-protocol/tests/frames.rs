//! 너무 크거나 잘리거나 깨진 프레임이 panic 없이 오류 값이 되는지, 메모리를 미리 크게 잡지
//! 않는지 공개 인터페이스로만 확인한다.

use std::io::{self, Read};

use facet_protocol::{
    Case, Condition, DecodeError, MAX_DEPTH, MAX_FRAME_LEN, Message, QueryMessage, ReadError,
    Request, Response, WriteError, read_frame, read_message, write_frame, write_message,
};

/// 주어진 바이트를 내주고 끝나는 스트림. `read`에 넘어온 버퍼의 가장 큰 길이를 기록한다.
struct Recording<'a> {
    data: &'a [u8],
    largest_buffer: usize,
}

impl Read for Recording<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.largest_buffer = self.largest_buffer.max(buf.len());
        self.data.read(buf)
    }
}

fn frame(len: u32, body: &[u8]) -> Vec<u8> {
    let mut v = len.to_le_bytes().to_vec();
    v.extend_from_slice(body);
    v
}

#[test]
fn frame_over_the_limit_is_rejected_before_reading_the_body() {
    let bytes = frame(MAX_FRAME_LEN + 1, &[0x04; 16]);
    let mut stream = bytes.as_slice();
    let err = read_frame(&mut stream).unwrap_err();
    assert!(matches!(err, ReadError::TooLarge { len } if len == MAX_FRAME_LEN + 1));
    // 머리 4바이트만 읽었다.
    assert_eq!(stream.len(), 16);

    let bytes = frame(u32::MAX, &[]);
    assert!(matches!(
        read_message::<Request>(&mut bytes.as_slice()),
        Err(ReadError::TooLarge { len: u32::MAX })
    ));
}

#[test]
fn frame_at_the_limit_is_read() {
    let body = vec![7u8; MAX_FRAME_LEN as usize];
    let mut stream = Vec::new();
    write_frame(&mut stream, &body).unwrap();
    assert_eq!(read_frame(&mut stream.as_slice()).unwrap(), body);
}

#[test]
fn writing_a_frame_over_the_limit_fails_without_writing() {
    let body = vec![0u8; MAX_FRAME_LEN as usize + 1];
    let mut out = Vec::new();
    assert!(matches!(
        write_frame(&mut out, &body),
        Err(WriteError::TooLarge { .. })
    ));
    assert!(out.is_empty());

    let huge = Request::Query(QueryMessage {
        condition: Condition::NameContains {
            part: "a".repeat(MAX_FRAME_LEN as usize),
            case: Case::Insensitive,
        },
        sort: None,
        limit: None,
    });
    assert!(matches!(
        write_message(&mut out, &huge),
        Err(WriteError::TooLarge { .. })
    ));
    assert!(out.is_empty());
}

#[test]
fn truncated_frame_is_an_error_and_memory_grows_only_with_data() {
    // 머리는 최대 크기를 말하지만 본문은 10바이트뿐이다.
    let bytes = frame(MAX_FRAME_LEN, &[0; 10]);
    let mut stream = Recording {
        data: &bytes,
        largest_buffer: 0,
    };
    assert!(matches!(read_frame(&mut stream), Err(ReadError::Truncated)));
    assert!(
        stream.largest_buffer <= 64 * 1024,
        "본문 버퍼를 미리 {}바이트 잡았다",
        stream.largest_buffer
    );
}

#[test]
fn stream_ending_at_a_frame_boundary_is_closed_and_inside_a_header_is_truncated() {
    assert!(matches!(
        read_frame(&mut [].as_slice()),
        Err(ReadError::Closed)
    ));
    assert!(matches!(
        read_frame(&mut [1u8, 0].as_slice()),
        Err(ReadError::Truncated)
    ));
}

#[test]
fn every_prefix_of_a_frame_is_an_error() {
    let mut bytes = Vec::new();
    write_message(
        &mut bytes,
        &Request::Query(QueryMessage {
            condition: Condition::And(vec![
                Condition::NameContains {
                    part: "가나".into(),
                    case: Case::Sensitive,
                },
                Condition::Size {
                    min: Some(1),
                    max: None,
                },
            ]),
            sort: None,
            limit: Some(3),
        }),
    )
    .unwrap();
    assert!(read_message::<Request>(&mut bytes.as_slice()).is_ok());
    for cut in 0..bytes.len() {
        assert!(
            read_message::<Request>(&mut &bytes[..cut]).is_err(),
            "{cut}"
        );
    }
}

/// 메시지 본문의 모든 앞 조각과, 바이트 하나를 바꾼 본문이 panic 없이 결과를 낸다.
fn survives_corruption<M: Message>(message: &M) {
    let bytes = message.to_bytes();
    for cut in 0..bytes.len() {
        assert_eq!(
            M::from_bytes(&bytes[..cut]).err(),
            Some(DecodeError::UnexpectedEnd),
            "{cut}바이트에서 자른 본문"
        );
    }
    for at in 0..bytes.len() {
        for value in [0x00, 0x01, 0x02, 0x7f, 0x80, 0xff] {
            let mut broken = bytes.clone();
            broken[at] = value;
            let _ = M::from_bytes(&broken);
        }
    }
    let mut longer = bytes.clone();
    longer.push(0);
    assert_eq!(
        M::from_bytes(&longer).err(),
        Some(DecodeError::TrailingBytes)
    );
}

#[test]
fn corrupted_bodies_are_errors_not_panics() {
    survives_corruption(&Request::Query(QueryMessage {
        condition: Condition::Or(vec![
            Condition::NameRegex {
                pattern: "x+".into(),
                case: Case::Insensitive,
            },
            Condition::Not(Box::new(Condition::Extension {
                extension: "txt".into(),
            })),
            Condition::Modified {
                min: Some(1),
                max: Some(2),
            },
        ]),
        sort: Some((
            facet_protocol::SortKey::Size,
            facet_protocol::Order::Descending,
        )),
        limit: Some(10),
    }));
    survives_corruption(&Request::Status);
    survives_corruption(&Response::Page(facet_protocol::PageMessage {
        hits: vec![facet_protocol::HitMessage {
            path: r"C:\a".into(),
            name: "a".into(),
            size: 1,
            modified: 2,
            attributes: 3,
        }],
        total: 1,
        total_is_upper_bound: true,
        next: Some(4),
        building: vec!["C:".into()],
    }));
    survives_corruption(&Response::Status(facet_protocol::StatusMessage {
        volumes: vec![facet_protocol::VolumeStatus {
            volume: "C:".into(),
            state: facet_protocol::VolumeState::NotIndexed(
                facet_protocol::NoIndexReason::NotNtfs {
                    filesystem: "FAT32".into(),
                },
            ),
        }],
    }));
}

#[test]
fn unknown_tags_and_bad_text_are_errors() {
    assert!(matches!(
        Request::from_bytes(&[0x7e]),
        Err(DecodeError::UnknownTag { .. })
    ));
    assert!(matches!(
        Response::from_bytes(&[0x01, 1, 0]),
        Err(DecodeError::UnknownTag { .. })
    ));
    assert_eq!(
        Request::from_bytes(&[]).err(),
        Some(DecodeError::UnexpectedEnd)
    );
    // 질의: 이름 부분 일치, 길이 2의 UTF-8이 아닌 문자열.
    assert_eq!(
        Request::from_bytes(&[0x02, 0, 2, 0, 0, 0, 0xff, 0xfe, 0, 0, 0]).err(),
        Some(DecodeError::InvalidUtf8)
    );
}

#[test]
fn huge_declared_lengths_are_rejected_without_allocating() {
    // 질의: 「그리고」 조건이 항목 40억 개를 말하지만 바이트는 없다.
    assert_eq!(
        Request::from_bytes(&[0x02, 8, 0xff, 0xff, 0xff, 0xff]).err(),
        Some(DecodeError::UnexpectedEnd)
    );
    // 질의: 이름 부분 일치 조각이 4GiB라고 말한다.
    assert_eq!(
        Request::from_bytes(&[0x02, 0, 0xff, 0xff, 0xff, 0xff, 0]).err(),
        Some(DecodeError::UnexpectedEnd)
    );
    // 결과 조각: 결과 40억 개.
    assert_eq!(
        Response::from_bytes(&[0x83, 0xff, 0xff, 0xff, 0xff]).err(),
        Some(DecodeError::UnexpectedEnd)
    );
}

#[test]
fn condition_trees_deeper_than_the_limit_are_rejected_without_overflowing_the_stack() {
    let leaf = [3u8, 0, 0, 0, 0]; // 경로 범위 조건, 빈 폴더 경로
    let tail = [0u8, 0]; // 정렬 없음, 제한 없음
    let nested = |depth: usize| {
        let mut b = vec![0x02];
        b.extend(std::iter::repeat_n(10u8, depth)); // 「아님」
        b.extend_from_slice(&leaf);
        b.extend_from_slice(&tail);
        b
    };
    assert!(Request::from_bytes(&nested(MAX_DEPTH - 1)).is_ok());
    assert_eq!(
        Request::from_bytes(&nested(MAX_DEPTH)).err(),
        Some(DecodeError::TooDeep)
    );
    assert_eq!(
        Request::from_bytes(&nested(1_000_000)).err(),
        Some(DecodeError::TooDeep)
    );
}
