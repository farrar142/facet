//! 볼륨 하나의 인덱스를 최신으로 유지한다: 첫 인덱스를 만들고, USN 저널을 따라가 변경을
//! 적용하고, 따라갈 수 없게 되면 스스로 다시 만든다. 플랫폼과 상관없는 코드다. 볼륨 읽기(MFT로
//! 인덱스 만들기, 저널 정보와 레코드 읽기)는 [`Source`]로 받는다. 실제 NTFS 볼륨은
//! `crate::volume::NtfsSource`(Windows 전용)다.
//!
//! 한 볼륨의 흐름([`run`]):
//!
//! 1. 「만드는 중」으로 두고, 저널 정보를 먼저 읽은 뒤 MFT로 인덱스를 만든다. 저널의 다음 USN을
//!    MFT를 읽기 **전에** 잡아 두므로, 읽는 동안 생긴 변경은 따라가기에서 다시 적용된다(적용은
//!    여러 번 해도 결과가 같다).
//! 2. 「준비됨」으로 바꿔 끼우고, [`POLL`]마다 저널을 확인한다. 저널 ID가 바뀌었거나 넘쳐
//!    읽어야 할 레코드가 사라졌으면([`Freshness::RebuildNeeded`]) 1로 돌아가 다시 만든다. 다시
//!    만드는 동안 질의는 앞선 인덱스로 답하고 응답과 상태 질의에 「만드는 중」을 알린다.
//! 3. 아니면 이어서 레코드를 읽어 인덱스에 적용한다([`Volumes::update`]). 그 볼륨의 이어 받기
//!    표지는 무효가 된다.
//!
//! 저널을 읽지 못하거나 레코드가 깨졌으면 인덱스를 믿을 수 없으므로 다시 만든다. 인덱스를
//! 만들지 못하면 「인덱스 없음(만들지 못함)」으로 두고 그 볼륨은 그만 따라간다.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use facet_index::{Freshness, Index, JournalInfo, JournalState};
use facet_protocol::NoIndexReason;

use crate::volumes::Volumes;

/// 저널을 확인하는 간격. 변경을 1초 안에 질의 결과에 반영하려고 넉넉히 짧게 잡았다. 확인은
/// 볼륨 핸들에 대한 제어 호출 둘이라 싸다.
pub const POLL: Duration = Duration::from_millis(100);

/// 한 번 확인할 때 읽는 레코드 묶음 수의 상한. 변경이 쏟아져도 멈추라는 신호를 늦게 보지 않게 한다.
const MAX_READS_PER_POLL: usize = 256;

/// 볼륨의 지금 저널 정보(`USN_JOURNAL_DATA`에서 쓰는 것).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalNow {
    /// 저널 ID.
    pub journal_id: u64,
    /// 저널에 남은 가장 오래된 USN.
    pub first_usn: i64,
    /// 다음에 쓰일 레코드의 USN.
    pub next_usn: i64,
}

/// 저널에서 읽은 레코드 묶음.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Records {
    /// 다음에 읽기 시작할 USN.
    pub next_usn: i64,
    /// `USN_RECORD_V2`를 이어 붙인 바이트([`Index::apply_usn`]에 그대로 넘긴다). 더 읽을 것이
    /// 없으면 비었다.
    pub bytes: Vec<u8>,
}

/// 볼륨 하나를 읽는 쪽.
pub trait Source: Send {
    /// 지금 저널 정보. 저널이 없으면 만들어도 된다.
    fn journal(&mut self) -> Result<JournalNow, String>;
    /// 저널 `journal_id`의 USN `start`부터 레코드를 한 묶음 읽는다. 기다리지 않는다.
    fn read(&mut self, journal_id: u64, start: i64) -> Result<Records, String>;
    /// MFT 전체를 읽어 인덱스를 처음부터 만든다.
    fn build(&mut self) -> Result<Index, String>;
}

/// 따라가기를 멈추라는 신호. 복제본끼리 같은 신호를 나눈다.
#[derive(Debug, Clone, Default)]
pub struct Stop(Arc<(Mutex<bool>, Condvar)>);

impl Stop {
    /// 서지 않은 새 신호.
    pub fn new() -> Self {
        Self::default()
    }

    /// 신호를 세운다. 기다리는 쪽을 모두 깨운다.
    pub fn stop(&self) {
        let (lock, wake) = &*self.0;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
        wake.notify_all();
    }

    /// 신호가 섰는가.
    pub fn is_stopped(&self) -> bool {
        *self.0.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 많아야 `timeout` 동안 기다린다. 신호가 섰으면 `true`.
    pub fn wait(&self, timeout: Duration) -> bool {
        let (lock, wake) = &*self.0;
        let stopped = lock.lock().unwrap_or_else(|e| e.into_inner());
        let (stopped, _) = wake
            .wait_timeout_while(stopped, timeout, |s| !*s)
            .unwrap_or_else(|e| e.into_inner());
        *stopped
    }
}

/// 따라가기를 그만두고 할 일.
enum Next {
    /// 인덱스를 다시 만든다.
    Rebuild,
    /// 그 볼륨이 더는 준비된 볼륨이 아니다(다른 쪽이 상태를 바꿨다). 그만 따라간다.
    Gone,
}

/// 볼륨 `volume`의 인덱스를 만들고 `stop`이 설 때까지 따라간다. `poll`마다 저널을 확인한다
/// (서비스는 [`POLL`]).
///
/// 인덱스를 만들지 못하면 「인덱스 없음(만들지 못함)」으로 두고 돌아온다. 인덱스를 만드는 도중에는
/// 멈추라는 신호를 보지 않는다(다 만든 뒤에 본다).
pub fn run(volumes: &Volumes, volume: &str, source: &mut impl Source, stop: &Stop, poll: Duration) {
    loop {
        volumes.set_building(volume);
        let state = match build(source) {
            Ok((index, state)) => {
                volumes.set_ready(volume, index);
                state
            }
            Err(message) => {
                volumes.set_not_indexed(volume, NoIndexReason::Failed { message });
                return;
            }
        };
        match follow(volumes, volume, source, stop, poll, state) {
            None | Some(Next::Gone) => return,
            Some(Next::Rebuild) => {}
        }
    }
}

/// 저널 정보를 먼저 잡고 인덱스를 만든다. 인덱스의 저널 상태는 그 저널 정보의 다음 USN이다.
fn build(source: &mut impl Source) -> Result<(Index, JournalState), String> {
    let now = source.journal()?;
    let mut index = source.build()?;
    // 레코드를 처음 적용할 때 만드는 항목 찾기 표를 「준비됨」 전에 만들어 둔다. 그러지 않으면 큰
    // 볼륨에서 첫 변경의 반영이 그만큼 늦는다. 빈 바이트는 아무것도 바꾸지 않는다.
    index
        .apply_usn(&[])
        .map_err(|e| format!("인덱스를 준비하지 못함: {e}"))?;
    let state = JournalState {
        journal_id: now.journal_id,
        usn: now.next_usn,
    };
    index.set_journal(state);
    Ok((index, state))
}

/// 멈추라는 신호가 서면 `None`, 다시 만들거나 그만둬야 하면 그 까닭.
///
/// `state.usn`은 다음에 읽을 USN이다. 그 앞의 레코드는 모두 적용했다.
fn follow(
    volumes: &Volumes,
    volume: &str,
    source: &mut impl Source,
    stop: &Stop,
    poll: Duration,
    mut state: JournalState,
) -> Option<Next> {
    loop {
        if stop.wait(poll) {
            return None;
        }
        let Ok(now) = source.journal() else {
            return Some(Next::Rebuild);
        };
        let info = JournalInfo {
            journal_id: now.journal_id,
            first_usn: now.first_usn,
        };
        match volumes.check_journal(volume, info) {
            None => return Some(Next::Gone),
            Some(Freshness::RebuildNeeded(_)) => return Some(Next::Rebuild),
            Some(Freshness::Current) => {}
        }
        for _ in 0..MAX_READS_PER_POLL {
            let Ok(records) = source.read(state.journal_id, state.usn) else {
                return Some(Next::Rebuild);
            };
            let next = JournalState {
                journal_id: state.journal_id,
                usn: records.next_usn.max(state.usn),
            };
            if records.bytes.is_empty() {
                // 적용할 것은 없지만 읽은 자리는 앞으로 옮긴다. 저널 상태는 질의 결과에 보이지
                // 않으므로 이어 받기 표지는 그대로 산다.
                if next != state && !volumes.set_journal(volume, next) {
                    return Some(Next::Gone);
                }
                state = next;
                break;
            }
            let applied = volumes.update(volume, |index| {
                let outcome = index.apply_usn(&records.bytes);
                index.set_journal(next);
                outcome
            });
            match applied {
                None => return Some(Next::Gone),
                Some(Err(_)) => return Some(Next::Rebuild),
                Some(Ok(_)) => {}
            }
            state = next;
            if stop.is_stopped() {
                return None;
            }
        }
    }
}
