//! 볼륨 붙이기·떼기: 지금 있는 볼륨 목록을 받아 대상 볼륨을 더하고 뺀다. 플랫폼과 상관없는
//! 코드다. 서비스는 [`POLL`]마다 드라이브 문자 목록(`crate::volume::local_volumes`)을 읽어
//! [`Mounts::sync`]에 넘긴다.
//!
//! - 새로 나타난 볼륨은 [`Volumes::attach`]로 더한다. 인덱스를 만들 볼륨이면 스레드 하나를 띄워
//!   인덱스를 만들고 따라간다([`follow::run`]). NTFS가 아니면 「인덱스 없음(까닭)」으로 둔다.
//! - 사라진 볼륨은 [`Volumes::detach`]로 뺀다. 상태 질의와 질의 결과에서 바로 빠지고, 그 볼륨을
//!   따라가던 스레드는 다음 확인에서 스스로 끝난다.
//! - 같은 드라이브 문자에 다른 다룰 방법(예: exFAT로 다시 포맷)이 보이면 뗐다가 다시 붙인 것으로 본다.
//! - 떨어진 볼륨의 스레드가 아직 끝나지 않았으면, 같은 드라이브 문자의 볼륨을 다시 붙이는 일은 그
//!   스레드가 끝난 뒤의 [`Mounts::sync`]로 미룬다. 그래야 옛 스레드가 새 볼륨의 상태를 건드리지
//!   않는다.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use facet_protocol::NoIndexReason;

use crate::follow::{self, IndexFile, Source, Stop};
use crate::volumes::{Plan, Volumes};

/// 서비스가 볼륨 목록을 다시 읽는 간격. 드라이브 문자 목록과 볼륨 정보를 읽는 값은 작고, USB
/// 드라이브를 꽂은 뒤 1초 안에 상태 질의에 보이게 한다.
pub const POLL: Duration = Duration::from_secs(1);

/// 붙어 있는 볼륨 하나.
struct Mounted {
    plan: Plan,
    /// 인덱스를 만들고 따라가는 스레드.
    thread: Option<JoinHandle<()>>,
}

/// 대상 볼륨 목록과 볼륨마다의 따라가기 스레드.
pub struct Mounts {
    volumes: Arc<Volumes>,
    stop: Stop,
    index_dir: Option<PathBuf>,
    poll: Duration,
    mounted: BTreeMap<String, Mounted>,
    /// 떨어진 볼륨의, 아직 끝나지 않았을 수 있는 스레드.
    retiring: Vec<(String, JoinHandle<()>)>,
}

impl Mounts {
    /// 볼륨이 하나도 붙지 않은 목록. 스레드는 `stop`이 서면 끝나고, 저널을 `poll`마다 확인한다
    /// (서비스는 [`follow::POLL`]). `index_dir`을 주면 볼륨마다 그 폴더에 인덱스를 저장한다
    /// ([`follow::IndexFile`], 저장 주기는 [`follow::SAVE_POLICY`]).
    pub fn new(
        volumes: Arc<Volumes>,
        stop: Stop,
        index_dir: Option<&Path>,
        poll: Duration,
    ) -> Self {
        Self {
            volumes,
            stop,
            index_dir: index_dir.map(Path::to_path_buf),
            poll,
            mounted: BTreeMap::new(),
            retiring: Vec::new(),
        }
    }

    /// 지금 있는 볼륨 목록 `present`(볼륨 이름과 다룰 방법)에 맞춘다: 사라진 볼륨은 빼고 새 볼륨은
    /// 더한다. 인덱스를 만들 새 볼륨마다 `open`으로 그 볼륨을 읽을 [`Source`]를 얻어 스레드를 띄운다.
    pub fn sync<S, F>(&mut self, present: &[(String, Plan)], mut open: F)
    where
        S: Source + 'static,
        F: FnMut(&str) -> S,
    {
        // 사라졌거나 다룰 방법이 바뀐 볼륨을 뗀다.
        let gone: Vec<String> = self
            .mounted
            .iter()
            .filter(|(name, m)| !present.iter().any(|(n, p)| n == *name && *p == m.plan))
            .map(|(name, _)| name.clone())
            .collect();
        for name in gone {
            self.volumes.detach(&name);
            if let Some(Mounted {
                thread: Some(t), ..
            }) = self.mounted.remove(&name)
            {
                self.retiring.push((name, t));
            }
        }
        // 끝난 스레드를 거둔다.
        let (done, running): (Vec<_>, Vec<_>) = std::mem::take(&mut self.retiring)
            .into_iter()
            .partition(|(_, t)| t.is_finished());
        for (_, t) in done {
            let _ = t.join();
        }
        self.retiring = running;
        // 새 볼륨을 붙인다.
        for (name, plan) in present {
            if self.mounted.contains_key(name) || self.retiring.iter().any(|(n, _)| n == name) {
                continue;
            }
            self.volumes.attach(name, plan);
            let thread = match plan {
                Plan::NotIndexed(_) => None,
                Plan::Index => self.spawn(name, open(name)),
            };
            self.mounted.insert(
                name.clone(),
                Mounted {
                    plan: plan.clone(),
                    thread,
                },
            );
        }
    }

    fn spawn<S: Source + 'static>(&self, name: &str, mut source: S) -> Option<JoinHandle<()>> {
        let volumes = Arc::clone(&self.volumes);
        let stop = self.stop.clone();
        let poll = self.poll;
        let file = self
            .index_dir
            .as_deref()
            .map(|dir| IndexFile::in_dir(dir, name, follow::SAVE_POLICY));
        let volume = name.to_owned();
        let label: String = name.chars().filter(char::is_ascii_alphanumeric).collect();
        let spawned = thread::Builder::new()
            .name(format!("facet-index-{label}"))
            .spawn(move || follow::run(&volumes, &volume, &mut source, &stop, poll, file.as_ref()));
        match spawned {
            Ok(t) => Some(t),
            Err(e) => {
                self.volumes.set_not_indexed(
                    name,
                    NoIndexReason::Failed {
                        message: format!("인덱스를 만들 스레드를 띄우지 못함: {e}"),
                    },
                );
                None
            }
        }
    }

    /// 아직 끝나지 않았을 수 있는 모든 스레드(붙은 볼륨과 떨어진 볼륨의 것). 멈출 때 기다리는 데 쓴다.
    pub fn into_threads(self) -> Vec<JoinHandle<()>> {
        self.mounted
            .into_values()
            .filter_map(|m| m.thread)
            .chain(self.retiring.into_iter().map(|(_, t)| t))
            .collect()
    }
}
