//! 실제 셸 미리보기(#343): 미리보기 전용 셸 스레드(COM STA)에서 처리기를 싣고 들고 있다가 자리를 옮기고 내린다. 이
//! 모듈의 함수는 모두 그 셸 스레드에서 돈다(처리기는 실은 스레드에서만 쓴다). 그 스레드가 요청 사이에 메시지를 돌려
//! 처리기 창이 그려진다.

use std::cell::RefCell;
use std::mem::ManuallyDrop;
use std::path::Path;

use facet_shell_protocol::PreviewHost;
use facet_win::preview_handler::{HostRect, LoadError, PreviewHandler};

thread_local! {
    /// 들고 있는 처리기(이 셸 스레드에 하나). 메뉴와 같은 까닭으로 [`ManuallyDrop`]에 둔다: 스레드가 끝나며 남은 것은
    /// COM을 정리한 뒤라 놓지 않고 흘린다(이미 내린 DLL을 부르지 않게).
    static HELD: RefCell<Option<ManuallyDrop<PreviewHandler>>> = const { RefCell::new(None) };
}

/// 들고 있던 처리기를 내리고 `path`의 처리기를 실어 `host`에 붙인다. 처리기가 없으면 [`LoadError::NoHandler`].
pub fn show(path: &Path, host: PreviewHost) -> Result<(), LoadError> {
    unload();
    let handler = PreviewHandler::load(path, rect(host))?;
    HELD.with(|held| *held.borrow_mut() = Some(ManuallyDrop::new(handler)));
    Ok(())
}

/// 들고 있는 처리기를 `host` 자리로 옮긴다. 없으면 아무것도 하지 않는다.
pub fn move_to(host: PreviewHost) {
    HELD.with(|held| {
        if let Some(handler) = held.borrow_mut().as_mut() {
            handler.move_to(rect(host));
        }
    });
}

/// 들고 있는 처리기를 내리고 놓는다(잡고 있던 파일도 놓인다).
pub fn unload() {
    if let Some(handler) = HELD.with(|held| held.borrow_mut().take()) {
        drop(ManuallyDrop::into_inner(handler));
    }
}

fn rect(host: PreviewHost) -> HostRect {
    HostRect {
        parent: host.parent,
        x: host.x,
        y: host.y,
        width: host.width,
        height: host.height,
    }
}
