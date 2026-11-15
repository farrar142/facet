//! 위치의 변경 감시: 공급자가 알리는 변경을 [`BATCH_INTERVAL`] 단위로 묶어 바뀐 항목만 다시 읽는다.
//!
//! 탭이 위치를 읽을 때([`crate::tab::ReadRequest::run_watched`]) 감시를 함께 건다. 공급자의 감시
//! ([`crate::Provider::watch`])가 알리는 이름을 묶기 스레드가 받아, 첫 알림부터 [`BATCH_INTERVAL`]
//! 동안 모은 뒤 이름마다 항목을 다시 읽어([`crate::Provider::item`]) [`ChangeBatch`] 하나로 넘긴다.
//! 탭은 그것을 [`crate::tab::Tab::apply_changes`]로 목록에 반영한다.
//!
//! 알림 버퍼가 넘쳤거나 감시가 스스로 끝났으면(위치가 사라짐 등) 묶음은 다시 읽기를 요구한다
//! ([`ChangeBatch::needs_reload`]).

use std::collections::HashSet;
use std::ffi::OsString;
use std::fmt;
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use crate::tab::RequestId;
use crate::{Item, Location, provider_for};

/// 변경 알림을 묶는 시간. 첫 알림부터 이만큼 모아 한 번에 반영한다.
pub const BATCH_INTERVAL: Duration = Duration::from_millis(100);

/// 공급자의 감시가 알리는 변경 하나.
#[derive(Debug)]
pub enum WatchEvent {
    /// 이 이름의 항목이 생겼거나, 사라졌거나, 바뀌었다. 어느 쪽인지는 항목을 다시 읽어 안다.
    Changed(OsString),
    /// 알림을 잃었다(버퍼 넘침). 위치를 다시 읽어야 한다. 감시는 계속된다.
    Overflow,
    /// 감시가 스스로 끝났다(위치가 사라짐, 연결 끊김 등). 위치를 다시 읽어야 한다.
    Ended,
}

/// 걸린 감시. 버리면 풀린다. 공급자가 감시를 붙든 값(스레드·핸들)을 담는다.
pub struct Watch {
    _guard: Box<dyn Send>,
}

impl Watch {
    /// 버리면 감시가 풀리는 값 `guard`를 감시로 삼는다. 공급자가 쓴다.
    pub fn new(guard: impl Send + 'static) -> Self {
        Watch {
            _guard: Box::new(guard),
        }
    }
}

impl fmt::Debug for Watch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Watch").finish_non_exhaustive()
    }
}

/// 감시를 걸 수 없는 까닭. 이 위치는 「자동 새로 고침 안 됨」이다.
#[derive(Debug)]
pub enum WatchError {
    /// 이 공급자나 파일 시스템, 환경이 변경 감시를 받지 않는다.
    Unsupported { location: Location },
    /// 감시를 걸다 실패했다.
    Io {
        location: Location,
        source: std::io::Error,
    },
}

impl WatchError {
    /// 감시를 걸지 못한 위치.
    pub fn location(&self) -> &Location {
        match self {
            Self::Unsupported { location } | Self::Io { location, .. } => location,
        }
    }
}

impl fmt::Display for WatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { location } => {
                write!(f, "이 위치는 변경을 감시할 수 없습니다: {location}")
            }
            Self::Io { location, source } => {
                write!(f, "변경 감시를 걸지 못했습니다: {location}: {source}")
            }
        }
    }
}

impl std::error::Error for WatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Unsupported { .. } => None,
        }
    }
}

/// 묶은 변경 하나: 어느 감시의 것인지와, 바뀐 이름마다 다시 읽은 항목(사라졌으면 `None`).
#[derive(Debug)]
pub struct ChangeBatch {
    pub(crate) watch: RequestId,
    pub(crate) changes: Vec<(OsString, Option<Item>)>,
    pub(crate) reload: bool,
}

impl ChangeBatch {
    /// 이 감시를 건 읽기 요청의 번호.
    pub fn watch(&self) -> RequestId {
        self.watch
    }

    /// 바뀐 항목 수.
    pub fn len(&self) -> usize {
        self.changes.len()
    }

    /// 바뀐 항목이 없는가.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// 위치를 통째로 다시 읽어야 하는가(알림을 잃었거나 감시가 끝났다).
    pub fn needs_reload(&self) -> bool {
        self.reload
    }
}

/// 묶은 변경을 받는 곳. 묶기 스레드에서 부른다.
pub type ChangeSink = Arc<dyn Fn(ChangeBatch) + Send + Sync>;

/// `location`의 감시를 걸고 묶기 스레드를 띄운다. 묶음에는 읽기 요청 번호 `id`를 붙인다.
///
/// 돌려준 감시를 버리면 공급자의 감시가 풀리고, 묶기 스레드도 알림 통로가 끊겨 끝난다.
pub(crate) fn start(
    location: &Location,
    id: RequestId,
    sink: ChangeSink,
) -> Result<Watch, WatchError> {
    let (sender, receiver) = mpsc::channel();
    let watch = provider_for(location).watch(
        location,
        Box::new(move |event| {
            let _ = sender.send(event);
        }),
    )?;
    let batch_location = location.clone();
    thread::Builder::new()
        .name("facet-watch".into())
        .spawn(move || batch_loop(&batch_location, id, &receiver, &*sink))
        .map_err(|source| WatchError::Io {
            location: location.clone(),
            source,
        })?;
    Ok(watch)
}

/// 묶는 동안 모은 알림.
#[derive(Default)]
struct Collected {
    names: Vec<OsString>,
    seen: HashSet<OsString>,
    reload: bool,
    ended: bool,
}

impl Collected {
    fn take(&mut self, event: WatchEvent) {
        match event {
            WatchEvent::Changed(name) => {
                if self.seen.insert(name.clone()) {
                    self.names.push(name);
                }
            }
            WatchEvent::Overflow => self.reload = true,
            WatchEvent::Ended => {
                self.reload = true;
                self.ended = true;
            }
        }
    }
}

fn batch_loop(
    location: &Location,
    id: RequestId,
    receiver: &mpsc::Receiver<WatchEvent>,
    sink: &(dyn Fn(ChangeBatch) + Send + Sync),
) {
    let provider = provider_for(location);
    while let Ok(first) = receiver.recv() {
        let mut collected = Collected::default();
        collected.take(first);
        let deadline = Instant::now() + BATCH_INTERVAL;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(left) {
                Ok(event) => collected.take(event),
                Err(RecvTimeoutError::Timeout) => break,
                // 감시가 스스로 끝나 알림 통로를 닫았다: 모은 것을 넘긴다.
                Err(RecvTimeoutError::Disconnected) if collected.ended => break,
                // 탭이 감시를 버렸다: 받을 탭이 이미 다른 곳에 있다.
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        let changes = if collected.reload {
            Vec::new()
        } else {
            collected
                .names
                .into_iter()
                .filter_map(|name| match provider.item(location, &name) {
                    Ok(item) => Some((name, item)),
                    // 읽지 못한 항목은 건드리지 않는다. 다음 변경이나 새로 고침이 맞춘다.
                    Err(_) => None,
                })
                .collect()
        };
        sink(ChangeBatch {
            watch: id,
            changes,
            reload: collected.reload,
        });
        if collected.ended {
            return;
        }
    }
}
