//! #342: 미리보기 요청·응답의 이진 형식 왕복.

use std::path::PathBuf;

use facet_shell_protocol::{Message, PreviewHost, Request, Response};

const HOST: PreviewHost = PreviewHost {
    parent: 0x1234_5678_9abc,
    x: -1920,
    y: 40,
    width: 400,
    height: 300,
};

#[test]
fn preview_requests_round_trip() {
    for request in [
        Request::Preview {
            id: 7,
            path: PathBuf::from(r"C:\폴더\문서 😀.pdf"),
            host: HOST,
        },
        Request::MovePreview { id: 8, host: HOST },
        Request::UnloadPreview { id: 9 },
    ] {
        let bytes = request.to_bytes();
        assert_eq!(Request::from_bytes(&bytes), Ok(request));
    }
}

#[test]
fn preview_responses_round_trip() {
    for response in [
        Response::PreviewShowing { id: 1 },
        Response::NoPreviewHandler { id: 2 },
    ] {
        let bytes = response.to_bytes();
        assert_eq!(Response::from_bytes(&bytes), Ok(response));
    }
}

#[test]
fn a_truncated_preview_request_is_rejected() {
    let bytes = Request::Preview {
        id: 7,
        path: PathBuf::from(r"C:\a.pdf"),
        host: HOST,
    }
    .to_bytes();
    assert!(Request::from_bytes(&bytes[..bytes.len() - 1]).is_err());
}
