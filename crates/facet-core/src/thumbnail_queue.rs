//! #329: 썸네일 요청 대기열(우선순위·취소·실패 기억).

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::thumbnail::{Thumbnail, ThumbnailError, ThumbnailKey};

/// #329: 작업자 수 상한.
pub const MAX_WORKERS: usize = 8;

/// #329: 코어 수에 맞춘 작업자 수.
pub fn worker_count(_cores: usize) -> usize {
    todo!("#329")
}

/// #329: 요청 하나의 결과.
#[derive(Debug, Clone)]
pub struct ThumbnailDone {
    pub owner: u64,
    pub key: ThumbnailKey,
    pub result: Result<Arc<Thumbnail>, ThumbnailError>,
}

/// #329: 썸네일 요청 대기열.
pub struct ThumbnailQueue {
    _todo: Vec<u8>, // #329
}

impl ThumbnailQueue {
    /// #329: 작업자 `workers`개와 만드는 함수로 대기열을 띄운다.
    pub fn new(
        _workers: usize,
        _maker: impl Fn(&ThumbnailKey, &AtomicBool) -> Result<Thumbnail, ThumbnailError>
        + Send
        + Sync
        + 'static,
    ) -> ThumbnailQueue {
        todo!("#329")
    }

    /// #329: 그 주인의 원하는 목록을 바꾼다.
    pub fn want(&self, _owner: u64, _visible: Vec<ThumbnailKey>, _nearby: Vec<ThumbnailKey>) {
        todo!("#329")
    }

    /// #329: 그 주인의 요청을 모두 취소한다.
    pub fn forget(&self, _owner: u64) {
        todo!("#329")
    }

    /// #329: 결과가 올 때 부를 깨우기.
    pub fn set_waker(&self, _waker: impl Fn() + Send + Sync + 'static) {
        todo!("#329")
    }

    /// #329: 온 결과들.
    pub fn poll(&self) -> Vec<ThumbnailDone> {
        todo!("#329")
    }

    /// #329: 결과 하나를 기다린다.
    pub fn wait_event(&self, _timeout: Duration) -> Option<ThumbnailDone> {
        todo!("#329")
    }
}
