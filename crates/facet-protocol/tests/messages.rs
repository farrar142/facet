//! 모든 메시지가 직렬화 → 역직렬화 왕복에서 같은지, 버전 인사가 맞게 답하는지 공개 인터페이스로만 확인한다.

use facet_protocol::{
    Case, Condition, ErrorKind, ErrorMessage, HitMessage, MAX_SUPPORTED_VERSION,
    MIN_SUPPORTED_VERSION, Message, NoIndexReason, Order, PROTOCOL_VERSION, PageMessage,
    QueryMessage, Request, Response, ResumeMessage, SortKey, StatusMessage, VolumeState,
    VolumeStatus, answer_hello, read_message, write_message,
};

/// 모든 조건 종류가 들어간 조건 트리.
pub fn every_condition() -> Condition {
    Condition::And(vec![
        Condition::NameContains {
            part: "보고서 📄".into(),
            case: Case::Insensitive,
        },
        Condition::NameWildcard {
            pattern: "*.T?T".into(),
            case: Case::Sensitive,
        },
        Condition::NameRegex {
            pattern: r"^\d{4}-".into(),
            case: Case::Insensitive,
        },
        Condition::Under {
            folder: r"C:\Users\사용자".into(),
        },
        Condition::Size {
            min: Some(10),
            max: None,
        },
        Condition::Modified {
            min: None,
            max: Some(u64::MAX),
        },
        Condition::Attributes { mask: 0x12 },
        Condition::Extension {
            extension: ".rs".into(),
        },
        Condition::Or(vec![]),
        Condition::Not(Box::new(Condition::Or(vec![
            Condition::Size {
                min: Some(0),
                max: Some(0),
            },
            Condition::And(vec![]),
        ]))),
    ])
}

fn requests() -> Vec<Request> {
    let mut out = vec![
        Request::Hello { version: 0 },
        Request::Hello {
            version: PROTOCOL_VERSION,
        },
        Request::Hello { version: u16::MAX },
        Request::Resume(ResumeMessage {
            cursor: u64::MAX,
            limit: 100,
        }),
        Request::Status,
        Request::Query(QueryMessage {
            condition: every_condition(),
            sort: None,
            limit: None,
        }),
    ];
    for key in [
        SortKey::Name,
        SortKey::Path,
        SortKey::Size,
        SortKey::Modified,
    ] {
        for order in [Order::Ascending, Order::Descending] {
            out.push(Request::Query(QueryMessage {
                condition: Condition::NameContains {
                    part: String::new(),
                    case: Case::Insensitive,
                },
                sort: Some((key, order)),
                limit: Some(u32::MAX),
            }));
        }
    }
    out
}

fn responses() -> Vec<Response> {
    let hit = HitMessage {
        path: r"C:\문서\긴 이름 😀.txt".into(),
        name: "긴 이름 😀.txt".into(),
        size: 1234,
        modified: 133_000_000_000_000_000,
        attributes: 0x20,
    };
    let mut out = vec![
        Response::HelloAccepted { version: 1 },
        Response::HelloRejected {
            client_version: 9,
            min_version: 1,
            max_version: 3,
        },
        Response::Page(PageMessage {
            hits: vec![],
            total: 0,
            next: None,
            building: vec![],
        }),
        Response::Page(PageMessage {
            hits: vec![hit.clone(), hit],
            total: 5_000_000,
            next: Some(7),
            building: vec!["C:".into(), "D:".into()],
        }),
        Response::Status(StatusMessage { volumes: vec![] }),
        Response::Status(StatusMessage {
            volumes: vec![
                VolumeStatus {
                    volume: "C:".into(),
                    state: VolumeState::Ready { items: 1_000_000 },
                },
                VolumeStatus {
                    volume: "D:".into(),
                    state: VolumeState::Building { items: Some(3) },
                },
                VolumeStatus {
                    volume: "E:".into(),
                    state: VolumeState::Building { items: None },
                },
                VolumeStatus {
                    volume: "F:".into(),
                    state: VolumeState::NotIndexed(NoIndexReason::NotNtfs {
                        filesystem: "exFAT".into(),
                    }),
                },
                VolumeStatus {
                    volume: "Z:".into(),
                    state: VolumeState::NotIndexed(NoIndexReason::Network),
                },
                VolumeStatus {
                    volume: "G:".into(),
                    state: VolumeState::NotIndexed(NoIndexReason::Failed {
                        message: "MFT를 읽지 못함".into(),
                    }),
                },
            ],
        }),
    ];
    for kind in [
        ErrorKind::BadMessage,
        ErrorKind::NotGreeted,
        ErrorKind::InvalidQuery,
        ErrorKind::StaleCursor,
        ErrorKind::Internal,
        ErrorKind::Cancelled,
    ] {
        out.push(Response::Error(ErrorMessage {
            kind,
            message: "설명".into(),
        }));
    }
    out
}

#[test]
fn every_request_survives_round_trip() {
    for request in requests() {
        let bytes = request.to_bytes();
        assert_eq!(Request::from_bytes(&bytes).unwrap(), request);
    }
}

#[test]
fn every_response_survives_round_trip() {
    for response in responses() {
        let bytes = response.to_bytes();
        assert_eq!(Response::from_bytes(&bytes).unwrap(), response);
    }
}

#[test]
fn messages_survive_round_trip_through_a_stream_of_frames() {
    let mut stream = Vec::new();
    for r in requests() {
        write_message(&mut stream, &r).unwrap();
    }
    for r in responses() {
        write_message(&mut stream, &r).unwrap();
    }
    let mut reader = stream.as_slice();
    for r in requests() {
        assert_eq!(read_message::<Request>(&mut reader).unwrap(), r);
    }
    for r in responses() {
        assert_eq!(read_message::<Response>(&mut reader).unwrap(), r);
    }
    assert!(reader.is_empty());
}

#[test]
fn hello_with_supported_version_is_accepted() {
    assert_eq!(
        answer_hello(PROTOCOL_VERSION),
        Response::HelloAccepted {
            version: PROTOCOL_VERSION
        }
    );
    assert!((MIN_SUPPORTED_VERSION..=MAX_SUPPORTED_VERSION).contains(&PROTOCOL_VERSION));
}

#[test]
fn hello_with_unsupported_version_is_rejected_with_supported_range() {
    for v in [0, MAX_SUPPORTED_VERSION + 1, u16::MAX] {
        assert_eq!(
            answer_hello(v),
            Response::HelloRejected {
                client_version: v,
                min_version: MIN_SUPPORTED_VERSION,
                max_version: MAX_SUPPORTED_VERSION,
            }
        );
    }
}

/// 버전 인사의 형식은 버전이 바뀌어도 그대로여야 한다. 바이트를 고정해 둔다.
#[test]
fn hello_layout_is_fixed() {
    assert_eq!(
        Request::Hello { version: 0x0102 }.to_bytes(),
        [0x01, 0x02, 0x01]
    );
    assert_eq!(
        Response::HelloAccepted { version: 0x0102 }.to_bytes(),
        [0x81, 0x02, 0x01]
    );
    assert_eq!(
        Response::HelloRejected {
            client_version: 9,
            min_version: 1,
            max_version: 2,
        }
        .to_bytes(),
        [0x82, 9, 0, 1, 0, 2, 0]
    );
}
