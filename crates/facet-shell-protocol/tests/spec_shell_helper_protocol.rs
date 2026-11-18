// target: crates/facet-shell-protocol/tests/spec_shell_helper_protocol.rs
// AC: #318: 버전 인사 받아들임·거절, 셸 메뉴 요청 왕복(한글·이모지·긴 경로), 항목 트리 응답 왕복, 너무 큰·잘린·깨진 프레임
//! 셸 확장 도우미 프로토콜의 버전 인사, 셸 메뉴 요청·항목 트리 응답의 바이트 왕복, 프레임 오류를 공개 인터페이스로만
//! 확인한다. 바이트 스트림 위에서 도는 순수 크레이트라 모든 환경에서 돈다.

use std::path::PathBuf;

use facet_shell_protocol::{
    Icon, MAX_FRAME_LEN, MAX_SUPPORTED_VERSION, MIN_SUPPORTED_VERSION, MenuItem, MenuTarget,
    PROTOCOL_VERSION, ReadError, Request, Response, ShellCommand, ShellSubmenu, answer_hello,
    read_message, write_message,
};

/// 260자를 넘는 한글·이모지 경로.
fn long_path() -> PathBuf {
    let mut path = PathBuf::from(r"C:\시험");
    for i in 0..30 {
        path.push(format!("아주 긴 폴더 이름 {i} 📁"));
    }
    path.push("보고서 😀.txt");
    assert!(path.as_os_str().len() > 260);
    path
}

fn round_trip_request(request: &Request) -> Request {
    let mut bytes = Vec::new();
    write_message(&mut bytes, request).unwrap();
    let mut stream = bytes.as_slice();
    let got = read_message::<Request>(&mut stream).unwrap();
    assert!(stream.is_empty(), "프레임 하나를 다 읽는다");
    got
}

fn round_trip_response(response: &Response) -> Response {
    let mut bytes = Vec::new();
    write_message(&mut bytes, response).unwrap();
    let mut stream = bytes.as_slice();
    let got = read_message::<Response>(&mut stream).unwrap();
    assert!(stream.is_empty(), "프레임 하나를 다 읽는다");
    got
}

/// 명령·구분선·중첩 하위 메뉴·아이콘·꺼진 항목이 모두 든 트리.
fn tree() -> Vec<MenuItem> {
    let icon = Icon {
        width: 2,
        height: 1,
        rgba: vec![255, 0, 0, 255, 0, 0, 255, 128],
    };
    vec![
        MenuItem::Command(ShellCommand {
            id: 1,
            text: "7-Zip으로 압축 😀".into(),
            icon: Some(icon.clone()),
            enabled: true,
        }),
        MenuItem::Separator,
        MenuItem::Submenu(ShellSubmenu {
            text: "보내기".into(),
            icon: None,
            enabled: true,
            items: vec![
                MenuItem::Command(ShellCommand {
                    id: 2,
                    text: "압축(ZIP) 폴더".into(),
                    icon: Some(icon),
                    enabled: true,
                }),
                MenuItem::Submenu(ShellSubmenu {
                    text: "더 보기".into(),
                    icon: None,
                    enabled: false,
                    items: vec![MenuItem::Command(ShellCommand {
                        id: u32::MAX,
                        text: String::new(),
                        icon: None,
                        enabled: false,
                    })],
                }),
            ],
        }),
        MenuItem::Command(ShellCommand {
            id: 3,
            text: "꺼진 항목".into(),
            icon: None,
            enabled: false,
        }),
    ]
}

#[test]
fn hello_within_the_supported_range_is_accepted() {
    assert_eq!(MAX_SUPPORTED_VERSION, PROTOCOL_VERSION);
    assert_eq!(
        answer_hello(PROTOCOL_VERSION),
        Response::HelloAccepted {
            version: PROTOCOL_VERSION
        }
    );
    assert_eq!(
        answer_hello(MIN_SUPPORTED_VERSION),
        Response::HelloAccepted {
            version: MIN_SUPPORTED_VERSION
        }
    );
}

#[test]
fn hello_outside_the_supported_range_is_rejected_with_the_range() {
    let rejected = Response::HelloRejected {
        min_version: MIN_SUPPORTED_VERSION,
        max_version: MAX_SUPPORTED_VERSION,
    };
    assert_eq!(answer_hello(MAX_SUPPORTED_VERSION + 1), rejected);
    assert_eq!(answer_hello(u16::MAX), rejected);
    if let Some(below) = MIN_SUPPORTED_VERSION.checked_sub(1) {
        assert_eq!(answer_hello(below), rejected);
    }
}

#[test]
fn hello_messages_round_trip() {
    let hello = Request::Hello {
        version: PROTOCOL_VERSION,
    };
    assert_eq!(round_trip_request(&hello), hello);
    for response in [
        Response::HelloAccepted {
            version: PROTOCOL_VERSION,
        },
        Response::HelloRejected {
            min_version: 1,
            max_version: 7,
        },
    ] {
        assert_eq!(round_trip_response(&response), response);
    }
}

#[test]
fn menu_requests_for_items_and_background_round_trip() {
    let items = Request::Menu {
        id: 42,
        target: MenuTarget::Items(vec![
            PathBuf::from(r"C:\시험\a.txt"),
            PathBuf::from(r"\\서버\공유\이모지 😀.png"),
            long_path(),
        ]),
    };
    assert_eq!(round_trip_request(&items), items);

    let background = Request::Menu {
        id: u64::MAX,
        target: MenuTarget::Background(long_path()),
    };
    assert_eq!(round_trip_request(&background), background);
}

#[test]
fn menu_tree_reply_round_trips() {
    let reply = Response::Menu {
        id: 7,
        items: tree(),
    };
    assert_eq!(round_trip_response(&reply), reply);

    let empty = Response::Menu {
        id: 8,
        items: Vec::new(),
    };
    assert_eq!(round_trip_response(&empty), empty);
}

#[test]
fn several_messages_in_one_stream_are_read_in_order() {
    let first = Request::Hello {
        version: PROTOCOL_VERSION,
    };
    let second = Request::Menu {
        id: 1,
        target: MenuTarget::Background(PathBuf::from(r"C:\시험")),
    };
    let mut bytes = Vec::new();
    write_message(&mut bytes, &first).unwrap();
    write_message(&mut bytes, &second).unwrap();
    let mut stream = bytes.as_slice();
    assert_eq!(read_message::<Request>(&mut stream).unwrap(), first);
    assert_eq!(read_message::<Request>(&mut stream).unwrap(), second);
    assert!(stream.is_empty());
}

#[test]
fn a_frame_over_the_limit_is_rejected_before_reading_the_body() {
    let mut bytes = (MAX_FRAME_LEN + 1).to_le_bytes().to_vec();
    bytes.extend_from_slice(&[0u8; 16]);
    let mut stream = bytes.as_slice();
    let err = read_message::<Response>(&mut stream).unwrap_err();
    assert!(
        matches!(err, ReadError::TooLarge { len } if len == MAX_FRAME_LEN + 1),
        "{err:?}"
    );
    assert_eq!(stream.len(), 16, "본문을 읽지 않았다");
}

#[test]
fn truncated_or_garbage_bytes_are_errors_not_panics() {
    let mut bytes = Vec::new();
    write_message(
        &mut bytes,
        &Response::Menu {
            id: 7,
            items: tree(),
        },
    )
    .unwrap();
    // 모든 길이로 자른 스트림은 오류다.
    for cut in 0..bytes.len() {
        assert!(
            read_message::<Response>(&mut &bytes[..cut]).is_err(),
            "{cut}바이트에서 자른 스트림"
        );
    }
    // 길이 머리는 맞지만 본문이 쓰레기인 프레임.
    for body in [
        vec![0xFFu8; 32],
        vec![0u8; 1],
        (0..=255u8).collect::<Vec<_>>(),
    ] {
        let mut frame = (body.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(&body);
        let _ = read_message::<Response>(&mut frame.as_slice());
        let _ = read_message::<Request>(&mut frame.as_slice());
    }
}
