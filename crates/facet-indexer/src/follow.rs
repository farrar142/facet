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
//!
//! 볼륨이 떨어지면([`Volumes::detach`]) 따라가기는 다음 확인에서 그만둔다. 그 뒤로는 상태를 바꾸려
//! 해도 바뀌지 않으므로 떨어진 볼륨이 결과에 되살아나지 않는다. 기다리는 동안에는 볼륨 핸들을
//! 놓아([`Source::release`]) 사용자가 USB 드라이브를 안전하게 뗄 수 있게 한다.
//!
//! 인덱스 파일([`IndexFile`])을 주면 인덱스를 디스크에 저장해 다시 뜬 뒤에 쓴다.
//!
//! - 시작할 때 그 파일을 먼저 읽는다. 저장한 저널 상태가 지금 저널과 맞으면([`Freshness::Current`])
//!   바로 「준비됨」으로 두고 저장한 USN부터 따라가 꺼져 있던 동안의 변경을 따라잡는다. 저널 ID가
//!   바뀌었거나 넘쳤으면 읽은 인덱스로 답하며 「만드는 중」으로 두고 다시 만든다.
//! - 파일이 깨졌거나(검사 값 틀림, 잘림, 인덱스 파일이 아님) 형식 버전이 다르면 지우고 다시 만든다.
//! - 인덱스를 (다시) 만들면 바로 저장한다. 따라가는 동안에는 적용한 레코드 수와 지난 시간으로
//!   저장하고([`SavePolicy`]), 멈추라는 신호를 받으면 저장하지 않은 변경이 있을 때 한 번 더 저장한다.
//!   저장한 인덱스와 저장한 USN은 늘 짝이 맞으므로, 저장이 늦어도 다시 뜬 뒤 따라잡을 레코드가
//!   늘어날 뿐이다. 저장에 실패하면 따라가기는 그대로 하고 다음 주기에 다시 저장한다.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

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

/// 따라가는 동안 인덱스 파일을 저장하는 주기. 저장하지 않은 변경이 있고 둘 가운데 하나를 넘으면
/// 저장한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavePolicy {
    /// 마지막 저장 뒤 적용한 USN 레코드 수가 이만큼이 되면 저장한다.
    pub changes: usize,
    /// 마지막 저장 뒤 이만큼 지났으면 저장한다.
    pub interval: Duration,
}

/// 서비스의 저장 주기: 레코드 2만 개 또는 5분.
///
/// 저장은 인덱스 전체를 다시 쓰므로(항목 100만 개면 수십 MB) 자주 하지 않는다. 그렇다고 너무 드물면
/// 갑자기 꺼진 뒤(멈출 때 저장하지 못함) 따라잡을 레코드가 많아지고, 그 사이 저널이 넘치면 다시
/// 만들어야 한다. 기본 크기의 저널(32MB)은 레코드 수십만 개를 담으므로 2만 개와 5분은 넉넉히 안쪽이다.
pub const SAVE_POLICY: SavePolicy = SavePolicy {
    changes: 20_000,
    interval: Duration::from_secs(300),
};

/// 볼륨 하나의 인덱스 파일.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexFile {
    /// 파일 경로.
    pub path: PathBuf,
    /// 따라가는 동안 저장하는 주기.
    pub policy: SavePolicy,
}

impl IndexFile {
    /// 폴더 `dir` 안의 볼륨 `volume`(예: `C:`) 인덱스 파일([`file_name`]).
    pub fn in_dir(dir: &Path, volume: &str, policy: SavePolicy) -> Self {
        Self {
            path: dir.join(file_name(volume)),
            policy,
        }
    }
}

/// 볼륨 `volume`(예: `C:`)의 인덱스 파일 이름(예: `C.facetidx`). 영문자·숫자가 아닌 글자는 `_`로
/// 바꾼다.
pub fn file_name(volume: &str) -> String {
    let stem: String = volume
        .trim_end_matches(['\\', '/', ':'])
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("{stem}.facetidx")
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
    /// 다음 확인까지 쥐고 있을 필요가 없는 것(볼륨 핸들 등)을 놓는다. 볼륨 핸들을 쥐고 있으면
    /// 「하드웨어 안전하게 제거」가 막힌다. 다음 호출에서 다시 연다.
    fn release(&mut self) {}
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
/// (서비스는 [`POLL`]). `file`을 주면 인덱스를 그 파일에 저장하고, 시작할 때 그 파일이 쓸 만하면
/// 다시 만들지 않고 쓴다(모듈 문서).
///
/// 인덱스를 만들지 못하면 「인덱스 없음(만들지 못함)」으로 두고 돌아온다. 인덱스를 만드는 도중에는
/// 멈추라는 신호를 보지 않는다(다 만든 뒤에 본다).
pub fn run(
    volumes: &Volumes,
    volume: &str,
    source: &mut impl Source,
    stop: &Stop,
    poll: Duration,
    file: Option<&IndexFile>,
) {
    let mut loaded = file.and_then(|f| load(volumes, volume, source, f));
    loop {
        let state = match loaded.take() {
            Some(state) => state,
            None => {
                if !volumes.set_building(volume) {
                    // 볼륨이 떨어졌다([`Volumes::detach`]).
                    return;
                }
                match build(source) {
                    Ok((index, state)) => {
                        volumes.set_ready(volume, index);
                        if let Some(f) = file {
                            let _ = save(volumes, volume, f);
                        }
                        state
                    }
                    Err(message) => {
                        volumes.set_not_indexed(volume, NoIndexReason::Failed { message });
                        return;
                    }
                }
            }
        };
        let mut saver = file.map(Saver::new);
        let next = follow(volumes, volume, source, stop, poll, state, saver.as_mut());
        if next.is_none()
            && let Some(saver) = &mut saver
        {
            // 멈출 때 한 번 더 저장한다.
            saver.flush(volumes, volume);
        }
        match next {
            None | Some(Next::Gone) => return,
            Some(Next::Rebuild) => {}
        }
    }
}

/// 저장한 인덱스 파일을 읽는다. 지금 저널과 맞으면 「준비됨」으로 두고 그 저널 상태를 돌려준다.
/// 저널이 바뀌었거나 넘쳤으면 읽은 인덱스로 답하는 「만드는 중」으로 두고 `None`(다시 만든다).
/// 파일이 없으면 `None`, 깨졌거나 형식 버전이 다르면 지우고 `None`.
fn load(
    volumes: &Volumes,
    volume: &str,
    source: &mut impl Source,
    file: &IndexFile,
) -> Option<JournalState> {
    let index = match Index::load(&file.path) {
        Ok(index) => index,
        Err(facet_index::StoreError::Io(io::ErrorKind::NotFound, _)) => return None,
        Err(_) => {
            // 버리고 다시 만든다. 다시 만들면 새 파일로 바꿔치기하지만, 만들지 못해도 깨진 파일을
            // 남기지 않는다.
            let _ = fs::remove_file(&file.path);
            return None;
        }
    };
    let fresh = source.journal().ok().is_some_and(|now| {
        index.check_journal(JournalInfo {
            journal_id: now.journal_id,
            first_usn: now.first_usn,
        }) == Freshness::Current
    });
    match index.journal() {
        Some(saved) if fresh => {
            volumes.set_ready(volume, index);
            Some(saved)
        }
        _ => {
            volumes.set_building_from(volume, index);
            None
        }
    }
}

/// 준비된 볼륨의 인덱스를 파일에 저장한다. 저장했으면 `true`.
fn save(volumes: &Volumes, volume: &str, file: &IndexFile) -> bool {
    volumes
        .read_ready(volume, |index| index.save(&file.path).is_ok())
        .unwrap_or(false)
}

/// 따라가는 동안의 저장 주기를 센다.
struct Saver<'a> {
    file: &'a IndexFile,
    /// 마지막 저장 뒤 적용한 레코드 수.
    pending: usize,
    /// 마지막 저장(또는 저장 시도) 시각.
    since: Instant,
}

impl<'a> Saver<'a> {
    fn new(file: &'a IndexFile) -> Self {
        Self {
            file,
            pending: 0,
            since: Instant::now(),
        }
    }

    /// 레코드 `records`개를 적용했다.
    fn applied(&mut self, records: usize) {
        // 해석한 레코드가 없는 묶음도 저널 상태는 바꿨으므로 적어도 하나로 센다.
        self.pending = self.pending.saturating_add(records.max(1));
    }

    /// 주기가 됐으면 저장한다.
    fn tick(&mut self, volumes: &Volumes, volume: &str) {
        let due = self.pending >= self.file.policy.changes
            || (self.pending > 0 && self.since.elapsed() >= self.file.policy.interval);
        if due {
            self.flush(volumes, volume);
        }
    }

    /// 저장하지 않은 변경이 있으면 저장한다. 실패하면 다음 주기에 다시 한다.
    fn flush(&mut self, volumes: &Volumes, volume: &str) {
        if self.pending == 0 {
            return;
        }
        if save(volumes, volume, self.file) {
            self.pending = 0;
        }
        self.since = Instant::now();
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
    mut saver: Option<&mut Saver<'_>>,
) -> Option<Next> {
    loop {
        if let Some(saver) = saver.as_deref_mut() {
            saver.tick(volumes, volume);
        }
        source.release();
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
                Some(Ok(outcome)) => {
                    // 보안이 바뀌었거나 다른 폴더가 놓인 경로 아래의 권한 검사 기억을 버린다.
                    volumes.forget_access(&outcome);
                    if let Some(saver) = saver.as_deref_mut() {
                        saver.applied(outcome.records);
                    }
                }
            }
            state = next;
            if stop.is_stopped() {
                return None;
            }
        }
    }
}
