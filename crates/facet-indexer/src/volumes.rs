//! 볼륨 여럿의 인덱스를 들고 질의에 답하는 [`Backend`]. 플랫폼과 상관없는 코드다.
//!
//! 볼륨마다 상태가 하나 있다.
//!
//! - **만드는 중**: 인덱스를 (다시) 만들고 있다. 앞선 인덱스가 있으면 그것으로 답하고, 응답의
//!   `building`에 그 볼륨을 적는다.
//! - **준비됨**: 인덱스가 있다. USN 저널의 변경은 [`Volumes::update`]로 그 자리에서 적용한다.
//! - **인덱스 없음**: NTFS가 아니거나, 네트워크 볼륨이거나, 만들지 못했다. 까닭을 담는다.
//! - **떨어짐**: 볼륨이 떨어졌다([`Volumes::detach`]). 상태 질의와 질의 결과에 나오지 않는다. 그
//!   볼륨을 따라가던 스레드가 뒤늦게 상태를 바꾸려 해도 바꾸지 않는다(떨어진 볼륨을 되살리지
//!   않는다). 다시 붙으면([`Volumes::attach`]) 새로 시작한다.
//!
//! 질의는 인덱스가 있는 볼륨마다 실행해 결과를 합친다. 정렬을 주었으면 볼륨마다 정렬된 결과를
//! 정렬 순서대로 섞고, 경로 정렬과 정렬 없음은 볼륨 이름 순서(`C:`, `D:`, ...)로 잇는다. 결과
//! 수 제한은 합친 결과에 건다.
//!
//! 파이프로 받은 질의([`Backend`])는 질의한 사용자([`Caller`])가 목록을 읽을 수 없는 폴더 아래
//! 결과를 내보내지 않는다([`crate::access`]). 정렬·제한 뒤 내보낼 결과만 검사하고, 걸러진 만큼 뒤
//! 결과를 더 꺼내 채운다. 전체 개수는 거르기 전에 센 값이라 「걸러지기 전 상한」이다
//! ([`Page::total_is_upper_bound`]). 검사 결과는 사용자마다 기억해 두고(볼륨 목록 하나에 하나),
//! USN 변경으로 폴더 보안이 바뀌거나 경로에 다른 폴더가 놓이면 그 아래 기억을 버린다
//! ([`Volumes::forget_access`]).

use std::cmp::Ordering;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, RwLock, RwLockReadGuard};

use facet_index::{
    self as index, Freshness, Hit, Index, JournalInfo, JournalState, Order, SortKey, UsnOutcome,
};
use facet_protocol::{
    ErrorKind, ErrorMessage, NoIndexReason, QueryMessage, StatusMessage, VolumeState, VolumeStatus,
};

use crate::access::{AccessCache, Caller};
use crate::session::{Backend, Cancel};

/// 걸러진 결과를 채우려고 한 번에 더 꺼내는 결과 수의 상한. 꺼낼 수를 두 배씩 늘리다가 여기서
/// 멈춘다(많이 걸러지는 질의가 결과 몇 개씩 꺼내느라 느려지지 않게).
const MAX_REFILL_BATCH: usize = 1 << 16;

/// [`GetDriveTypeW`](https://learn.microsoft.com/windows/win32/api/fileapi/nf-fileapi-getdrivetypew)의
/// 값 가운데 여기서 가르는 것.
pub mod drive_type {
    /// 종류를 알 수 없다.
    pub const UNKNOWN: u32 = 0;
    /// 루트 경로가 없다(드라이브 문자에 볼륨이 붙어 있지 않다).
    pub const NO_ROOT_DIR: u32 = 1;
    /// 네트워크 드라이브.
    pub const REMOTE: u32 = 4;
}

/// 볼륨 하나를 어떻게 다룰지.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// 로컬 NTFS 볼륨: MFT를 읽어 인덱스를 만든다.
    Index,
    /// 인덱스를 만들지 않는다. 상태 질의에 이 까닭으로 보인다.
    NotIndexed(NoIndexReason),
}

/// 드라이브 문자 하나의 종류(`GetDriveTypeW`)와 파일 시스템 이름(`GetVolumeInformationW`, 읽지
/// 못했으면 `None`)으로 다룰 방법을 정한다. 목록에 넣지 않을 것(볼륨이 붙어 있지 않음, 매체가
/// 없는 드라이브)은 `None`이다.
///
/// 네트워크 드라이브는 파일 시스템과 상관없이 「네트워크 볼륨」이다. 그 밖(고정·이동식·CD·RAM
/// 디스크)은 로컬 볼륨이고, 파일 시스템이 NTFS면 인덱스를 만들고 아니면 「NTFS가 아님」이다.
pub fn classify(drive_type: u32, filesystem: Option<&str>) -> Option<Plan> {
    match drive_type {
        drive_type::UNKNOWN | drive_type::NO_ROOT_DIR => None,
        drive_type::REMOTE => Some(Plan::NotIndexed(NoIndexReason::Network)),
        _ => match filesystem {
            None => None,
            Some(fs) if fs.eq_ignore_ascii_case("NTFS") => Some(Plan::Index),
            Some(fs) => Some(Plan::NotIndexed(NoIndexReason::NotNtfs {
                filesystem: fs.to_owned(),
            })),
        },
    }
}

/// 질의와 USN 따라가기가 함께 쓰는 볼륨 하나의 인덱스.
///
/// 질의는 읽기 잠금을 쥐고 실행하고, 따라가기는 쓰기 잠금을 잠깐 쥐고 바꾼다. 그래서 바꿀 때
/// 인덱스를 복제하지 않는다.
#[derive(Debug)]
struct Shared {
    index: RwLock<Index>,
    /// 바꿀 때마다 1씩 는다(쓰기 잠금 안에서). 이어 받기 표지가 만들 때의 값과 견줘 무효를 가린다.
    generation: AtomicU64,
}

impl Shared {
    fn new(index: Index) -> Self {
        Self {
            index: RwLock::new(index),
            generation: AtomicU64::new(0),
        }
    }

    fn read(&self) -> RwLockReadGuard<'_, Index> {
        // 잠근 채 패닉한 쪽이 있어도 인덱스는 질의할 수 있다(바꾸던 중이면 결과가 덜 맞을 뿐이다).
        self.index.read().unwrap_or_else(|e| e.into_inner())
    }

    fn generation(&self) -> u64 {
        self.generation.load(AtomicOrdering::Acquire)
    }
}

/// 볼륨 하나의 상태.
#[derive(Debug, Clone)]
enum Slot {
    /// 만드는 중. 앞선 인덱스가 있으면 그것으로 답한다.
    Building(Option<Arc<Shared>>),
    Ready(Arc<Shared>),
    NotIndexed(NoIndexReason),
    /// 떨어졌다. 상태 질의와 질의에 나오지 않고, [`Volumes::attach`] 말고는 바꾸지 않는다.
    Detached,
}

impl Slot {
    /// 질의에 쓸 인덱스.
    fn index(&self) -> Option<&Arc<Shared>> {
        match self {
            Self::Building(old) => old.as_ref(),
            Self::Ready(index) => Some(index),
            Self::NotIndexed(_) | Self::Detached => None,
        }
    }
}

/// 볼륨 여럿의 인덱스. 여러 연결과 인덱스를 만드는 스레드가 함께 쓴다.
#[derive(Debug, Default)]
pub struct Volumes {
    /// 볼륨 이름(예: `C:`) → 상태. 이름 순서다.
    slots: RwLock<BTreeMap<String, Slot>>,
    /// 사용자마다의 폴더 권한 검사 결과.
    access: AccessCache,
}

/// 질의나 이어 받기의 응답: 결과 조각, 전체 개수, 이어 받기 표지, 만드는 중인 볼륨.
#[derive(Debug)]
pub struct Page {
    pub hits: Vec<Hit>,
    /// 모든 볼륨에서 질의에 맞는 결과 전체의 개수. 이어 받아도 처음 질의의 값이다. 권한으로
    /// 걸렀으면 거르기 전의 개수다.
    pub total: usize,
    /// `total`이 「걸러지기 전 상한」인가(권한으로 걸러 내보낸 결과인가).
    pub total_is_upper_bound: bool,
    /// 남은 결과가 있으면 그것을 받을 표지.
    pub next: Option<Cursor>,
    /// 인덱스를 만드는 중이어서 앞선 인덱스로(또는 결과 없이) 답한 볼륨들.
    pub building: Vec<String>,
}

/// 이어 받기 표지. 볼륨마다 아직 내보내지 않은 결과를 들고 있다.
///
/// 표지를 만든 뒤 어느 볼륨의 인덱스가 바뀌면(다시 만들어 바꿔 끼우거나 USN 저널의 변경을
/// 적용하면) 표지는 무효다.
#[derive(Debug)]
pub struct Cursor {
    sort: Option<(SortKey, Order)>,
    total: usize,
    /// 권한으로 걸러 내보내는가.
    filtered: bool,
    /// 볼륨 이름 순서.
    parts: Vec<Part>,
    /// 볼륨들에서 합쳐 꺼내고 권한 검사도 마쳤지만 아직 내보내지 않은 결과(정렬 순서).
    pending: VecDeque<Hit>,
}

/// 이어 받기 표지에서 볼륨 하나의 몫.
#[derive(Debug)]
struct Part {
    volume: String,
    index: Arc<Shared>,
    /// 표지를 만들 때 그 인덱스의 [`Shared::generation`].
    generation: u64,
    /// 꺼내 두었지만 아직 내보내지 않은 결과(그 볼륨 안에서는 정렬 순서).
    buffered: VecDeque<Hit>,
    /// 그 볼륨의 남은 결과.
    next: Option<index::Cursor>,
}

impl Part {
    /// 꺼내 둔 결과가 `limit`개가 되거나 남은 결과가 없을 때까지 채운다.
    fn fill(&mut self, limit: usize) -> Result<(), ErrorMessage> {
        while self.buffered.len() < limit {
            let Some(cursor) = self.next.take() else {
                break;
            };
            let page = self
                .index
                .read()
                .resume(cursor, limit - self.buffered.len())?;
            self.buffered.extend(page.hits);
            self.next = page.next;
        }
        Ok(())
    }

    fn is_done(&self) -> bool {
        self.buffered.is_empty() && self.next.is_none()
    }
}

/// 인덱스의 이름 정렬과 같은 비교: 대소문자를 접은 이름, 같으면 원래 이름.
fn compare_names(a: &str, b: &str) -> Ordering {
    let folded = |s: &str| s.chars().flat_map(char::to_lowercase).collect::<Vec<_>>();
    folded(a).cmp(&folded(b)).then_with(|| a.cmp(b))
}

fn compare(key: SortKey, a: &Hit, b: &Hit) -> Ordering {
    match key {
        SortKey::Size => a.size.cmp(&b.size),
        SortKey::Modified => a.modified.cmp(&b.modified),
        SortKey::Name | SortKey::Path => compare_names(&a.name, &b.name),
    }
}

impl Cursor {
    /// 볼륨들에서 다음 결과를 많아야 `limit`개 꺼낸다.
    fn take(&mut self, limit: usize) -> Result<Vec<Hit>, ErrorMessage> {
        for part in &mut self.parts {
            part.fill(limit)?;
        }
        let mut hits = Vec::new();
        while hits.len() < limit {
            let mut fronts = self
                .parts
                .iter()
                .enumerate()
                .filter_map(|(i, p)| p.buffered.front().map(|h| (i, h)));
            let pick = match self.sort {
                // 볼륨 하나를 다 낸 뒤 다음 볼륨. 경로 내림차순은 뒤 볼륨부터.
                None | Some((SortKey::Path, Order::Ascending)) => fronts.next(),
                Some((SortKey::Path, Order::Descending)) => fronts.next_back(),
                Some((key, order)) => fronts.reduce(|best, cand| {
                    let ord = compare(key, cand.1, best.1);
                    let ord = if order == Order::Descending {
                        ord.reverse()
                    } else {
                        ord
                    };
                    // 같으면 앞 볼륨이 먼저다.
                    if ord == Ordering::Less { cand } else { best }
                }),
            };
            let Some((i, _)) = pick else { break };
            let hit = self.parts[i]
                .buffered
                .pop_front()
                .expect("고른 볼륨에 결과가 있다");
            hits.push(hit);
            // 꺼내 둔 것을 다 쓴 볼륨은 남은 결과에서 더 채운다. 그래야 다른 볼륨과 비교할 앞
            // 결과가 늘 있다.
            let part = &mut self.parts[i];
            if part.buffered.is_empty() {
                part.fill(limit - hits.len())?;
            }
        }
        self.parts.retain(|p| !p.is_done());
        Ok(hits)
    }

    /// 다음 결과 가운데 `keep`을 지나는 것을 많아야 `limit`개 꺼낸다. 걸러진 만큼 뒤 결과를 더
    /// 꺼내 채우고, 더 꺼냈지만 내보내지 않은 것은 다음 조각을 위해 남긴다.
    fn take_kept(
        &mut self,
        limit: usize,
        keep: impl Fn(&Hit) -> bool,
        cancel: Option<&Cancel>,
    ) -> Result<Vec<Hit>, ErrorMessage> {
        let mut hits = Vec::new();
        let mut batch = 0usize;
        while hits.len() < limit {
            if let Some(hit) = self.pending.pop_front() {
                hits.push(hit);
                continue;
            }
            if self.parts.is_empty() {
                break;
            }
            if cancel.is_some_and(Cancel::is_cancelled) {
                return Err(cancelled());
            }
            let need = limit - hits.len();
            batch = need.max(batch.saturating_mul(2).min(MAX_REFILL_BATCH));
            let taken = self.take(batch)?;
            self.pending.extend(taken.into_iter().filter(|h| keep(h)));
        }
        Ok(hits)
    }

    fn is_done(&self) -> bool {
        self.parts.is_empty() && self.pending.is_empty()
    }
}

fn cancelled() -> ErrorMessage {
    ErrorMessage {
        kind: ErrorKind::Cancelled,
        message: "질의가 취소됐습니다".to_owned(),
    }
}

impl Volumes {
    /// 볼륨이 하나도 없는 빈 목록.
    pub fn new() -> Self {
        Self::default()
    }

    /// 인덱스가 준비된 볼륨 하나(`volume`, 예: `C:`)로 된 목록.
    pub fn with_index(volume: &str, index: Index) -> Self {
        let volumes = Self::new();
        volumes.set_ready(volume, index);
        volumes
    }

    fn slots_mut(&self) -> std::sync::RwLockWriteGuard<'_, BTreeMap<String, Slot>> {
        // 잠근 채 패닉한 스레드가 있어도 상태 자체는 늘 온전하다(값을 통째로 바꾸기만 한다).
        self.slots.write().unwrap_or_else(|e| e.into_inner())
    }

    fn slots(&self) -> std::sync::RwLockReadGuard<'_, BTreeMap<String, Slot>> {
        self.slots.read().unwrap_or_else(|e| e.into_inner())
    }

    /// `volume`의 상태를 `slot`으로 바꾼다. 떨어진 볼륨이면 바꾸지 않고 `false`.
    fn put(slots: &mut BTreeMap<String, Slot>, volume: &str, slot: Slot) -> bool {
        if matches!(slots.get(volume), Some(Slot::Detached)) {
            return false;
        }
        slots.insert(volume.to_owned(), slot);
        true
    }

    /// 볼륨 `volume`이 (다시) 붙었다. `plan`이 [`Plan::Index`]면 앞선 인덱스 없이 「만드는 중」,
    /// 아니면 「인덱스 없음(까닭)」으로 둔다. 떨어진 볼륨이어도 바꾼다.
    pub fn attach(&self, volume: &str, plan: &Plan) {
        let slot = match plan {
            Plan::Index => Slot::Building(None),
            Plan::NotIndexed(reason) => Slot::NotIndexed(reason.clone()),
        };
        self.slots_mut().insert(volume.to_owned(), slot);
    }

    /// 볼륨 `volume`이 떨어졌다. 상태 질의와 질의 결과에서 빠지고, 인덱스를 버리고, 그 볼륨의 결과를
    /// 든 이어 받기 표지는 무효가 된다. 그 볼륨을 따라가던 스레드가 상태를 바꾸려 해도
    /// [`Volumes::attach`] 전까지는 바꾸지 않는다. 모르는 볼륨이면 아무것도 하지 않는다.
    pub fn detach(&self, volume: &str) {
        if let Some(slot) = self.slots_mut().get_mut(volume) {
            *slot = Slot::Detached;
        }
    }

    /// 폴더 권한 검사 결과의 기억. 시험과 측정이 기억의 크기를 보는 데 쓴다.
    pub fn access(&self) -> &AccessCache {
        &self.access
    }

    /// USN 변경을 적용한 결과(`outcome`)에 따라 폴더 권한 검사의 기억을 버린다. 보안이 바뀐 폴더와
    /// 새 자리에 놓인 폴더(그 경로에 앞서 있던 폴더의 기억은 틀렸다) 아래를 버리고, 볼륨 루트까지
    /// 닿지 않아 경로를 모르는 보안 변경이 있으면 모두 버린다.
    pub fn forget_access(&self, outcome: &UsnOutcome) {
        if outcome.security_changes.iter().any(|c| c.path.is_none()) {
            self.access.clear();
            return;
        }
        self.access.forget_all_under(
            outcome
                .security_changes
                .iter()
                .filter_map(|c| c.path.as_deref())
                .chain(outcome.placed_folders.iter().map(String::as_str)),
        );
    }

    /// `volume`의 인덱스를 (다시) 만들기 시작한다. 앞선 인덱스가 있으면 다 만들 때까지 그것으로
    /// 답한다. 떨어진 볼륨이면 바꾸지 않고 `false`.
    pub fn set_building(&self, volume: &str) -> bool {
        let mut slots = self.slots_mut();
        let old = slots.get(volume).and_then(Slot::index).cloned();
        Self::put(&mut slots, volume, Slot::Building(old))
    }

    /// `volume`의 인덱스를 다시 만들기 시작한다. 다 만들 때까지 `previous`(예: 디스크에서 읽은
    /// 앞선 인덱스)로 답한다. 떨어진 볼륨이면 바꾸지 않고 `false`.
    pub fn set_building_from(&self, volume: &str, previous: Index) -> bool {
        Self::put(
            &mut self.slots_mut(),
            volume,
            Slot::Building(Some(Arc::new(Shared::new(previous)))),
        )
    }

    /// 준비된 볼륨 `volume`의 인덱스를 읽기 잠금을 쥐고 `read`에 넘긴다(예: 파일로 저장). 그동안
    /// 질의는 함께 돌고 변경 적용은 기다린다. 준비된 볼륨이 아니면 `None`.
    pub fn read_ready<R>(&self, volume: &str, read: impl FnOnce(&Index) -> R) -> Option<R> {
        let shared = match self.slots().get(volume) {
            Some(Slot::Ready(shared)) => Arc::clone(shared),
            _ => return None,
        };
        let index = shared.read();
        Some(read(&index))
    }

    /// `volume`의 인덱스를 다 만들었다. 앞선 인덱스를 바꿔 끼운다(그 인덱스의 이어 받기 표지는
    /// 무효가 된다). 그동안의 폴더 보안 변경은 알 수 없으므로 권한 검사의 기억을 모두 버린다.
    /// 떨어진 볼륨이면 바꾸지 않고 `false`.
    pub fn set_ready(&self, volume: &str, index: Index) -> bool {
        self.access.clear();
        Self::put(
            &mut self.slots_mut(),
            volume,
            Slot::Ready(Arc::new(Shared::new(index))),
        )
    }

    /// 준비된 볼륨 `volume`의 인덱스를 `change`로 바꾼다(USN 저널의 변경 적용). 그 볼륨의 결과를
    /// 든 이어 받기 표지는 무효가 된다. 준비된 볼륨이 아니면(만드는 중, 인덱스 없음, 모르는 볼륨)
    /// 바꾸지 않고 `None`이다.
    ///
    /// 바꾸는 동안 그 볼륨의 질의는 기다린다. `change`는 짧아야 한다.
    pub fn update<R>(&self, volume: &str, change: impl FnOnce(&mut Index) -> R) -> Option<R> {
        let shared = match self.slots().get(volume) {
            Some(Slot::Ready(shared)) => Arc::clone(shared),
            _ => return None,
        };
        let mut index = shared.index.write().unwrap_or_else(|e| e.into_inner());
        let result = change(&mut index);
        shared.generation.fetch_add(1, AtomicOrdering::AcqRel);
        Some(result)
    }

    /// 준비된 볼륨 `volume`의 인덱스가 든 저널 상태를 `info`와 견준다([`Index::check_journal`]).
    /// 준비된 볼륨이 아니면 `None`.
    pub fn check_journal(&self, volume: &str, info: JournalInfo) -> Option<Freshness> {
        match self.slots().get(volume) {
            Some(Slot::Ready(shared)) => Some(shared.read().check_journal(info)),
            _ => None,
        }
    }

    /// 준비된 볼륨 `volume`의 인덱스가 든 저널 상태를 `state`로 바꾼다. 저널 상태는 질의 결과에
    /// 보이지 않으므로 이어 받기 표지는 그대로 산다. 준비된 볼륨이 아니면 `false`.
    pub fn set_journal(&self, volume: &str, state: JournalState) -> bool {
        let shared = match self.slots().get(volume) {
            Some(Slot::Ready(shared)) => Arc::clone(shared),
            _ => return false,
        };
        shared
            .index
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .set_journal(state);
        true
    }

    /// `volume`에는 인덱스가 없다. 앞선 인덱스가 있었으면 버린다. 떨어진 볼륨이면 바꾸지 않고
    /// `false`.
    pub fn set_not_indexed(&self, volume: &str, reason: NoIndexReason) -> bool {
        Self::put(&mut self.slots_mut(), volume, Slot::NotIndexed(reason))
    }

    fn building(slots: &BTreeMap<String, Slot>) -> Vec<String> {
        slots
            .iter()
            .filter(|(_, s)| matches!(s, Slot::Building(_)))
            .map(|(v, _)| v.clone())
            .collect()
    }

    /// 볼륨들의 상태.
    pub fn status(&self) -> StatusMessage {
        let volumes = self
            .slots()
            .iter()
            .filter_map(|(volume, slot)| {
                let state = match slot {
                    Slot::Building(old) => VolumeState::Building {
                        items: old.as_ref().map(|i| i.read().len() as u64),
                    },
                    Slot::Ready(index) => VolumeState::Ready {
                        items: index.read().len() as u64,
                    },
                    Slot::NotIndexed(reason) => VolumeState::NotIndexed(reason.clone()),
                    Slot::Detached => return None,
                };
                Some(VolumeStatus {
                    volume: volume.clone(),
                    state,
                })
            })
            .collect();
        StatusMessage { volumes }
    }

    /// 질의를 인덱스가 있는 모든 볼륨에 실행한다. 권한으로 거르지 않는다(서비스 안과 시험이 쓴다.
    /// 파이프로 받은 질의는 [`Volumes::search_as`]).
    pub fn search(&self, query: &QueryMessage, cancel: &Cancel) -> Result<Page, ErrorMessage> {
        self.run_search(query, None, cancel)
    }

    /// 질의를 `caller`로 실행한다: `caller`가 목록을 읽을 수 없는 폴더 아래 결과는 빼고, 걸러진
    /// 만큼 뒤 결과로 결과 수 제한을 채운다. 전체 개수는 거르기 전의 개수다.
    pub fn search_as(
        &self,
        query: &QueryMessage,
        caller: &dyn Caller,
        cancel: &Cancel,
    ) -> Result<Page, ErrorMessage> {
        self.run_search(query, Some(caller), cancel)
    }

    fn run_search(
        &self,
        query: &QueryMessage,
        caller: Option<&dyn Caller>,
        cancel: &Cancel,
    ) -> Result<Page, ErrorMessage> {
        let q = query.to_query()?;
        let limit = query.limit.map_or(usize::MAX, |n| n as usize);
        let (snapshot, building) = {
            let slots = self.slots();
            let snapshot: Vec<(String, Arc<Shared>)> = slots
                .iter()
                .filter_map(|(v, s)| s.index().map(|i| (v.clone(), Arc::clone(i))))
                .collect();
            (snapshot, Self::building(&slots))
        };
        let mut parts = Vec::with_capacity(snapshot.len());
        let mut total = 0;
        for (volume, index) in snapshot {
            if cancel.is_cancelled() {
                return Err(cancelled());
            }
            let (page, generation) = {
                let guard = index.read();
                // 읽기 잠금을 쥔 동안은 판이 바뀌지 않는다.
                (guard.search(&q), index.generation())
            };
            total += page.total;
            parts.push(Part {
                volume,
                index,
                generation,
                buffered: page.hits.into(),
                next: page.next,
            });
        }
        let mut cursor = Cursor {
            sort: query.sort.map(|(k, o)| (k.into(), o.into())),
            total,
            filtered: caller.is_some(),
            parts,
            pending: VecDeque::new(),
        };
        let hits = self.take(&mut cursor, limit, caller, Some(cancel))?;
        Ok(Self::page(cursor, hits, building))
    }

    /// 표지에서 다음 결과를 꺼낸다. `caller`가 있으면 권한으로 거른다.
    fn take(
        &self,
        cursor: &mut Cursor,
        limit: usize,
        caller: Option<&dyn Caller>,
        cancel: Option<&Cancel>,
    ) -> Result<Vec<Hit>, ErrorMessage> {
        match caller {
            None => cursor.take_kept(limit, |_| true, cancel),
            Some(caller) => {
                cursor.take_kept(limit, |h| self.access.visible(caller, &h.path), cancel)
            }
        }
    }

    /// 이어 받기 표지로 다음 결과를 많아야 `limit`개 받는다. 표지를 만든 뒤 어느 볼륨의 인덱스가
    /// 바뀌었으면 [`ErrorKind::StaleCursor`]다. 권한으로 거른 질의([`Volumes::search_as`])의 표지는
    /// [`Volumes::resume_as`]로만 이어 받는다.
    pub fn resume(&self, cursor: Cursor, limit: usize) -> Result<Page, ErrorMessage> {
        self.run_resume(cursor, limit, None)
    }

    /// 이어 받기 표지로 다음 결과를 `caller`로 걸러 많아야 `limit`개 받는다.
    pub fn resume_as(
        &self,
        cursor: Cursor,
        limit: usize,
        caller: &dyn Caller,
    ) -> Result<Page, ErrorMessage> {
        self.run_resume(cursor, limit, Some(caller))
    }

    fn run_resume(
        &self,
        mut cursor: Cursor,
        limit: usize,
        caller: Option<&dyn Caller>,
    ) -> Result<Page, ErrorMessage> {
        if cursor.filtered != caller.is_some() {
            return Err(ErrorMessage {
                kind: ErrorKind::StaleCursor,
                message: "권한 거르기가 다른 이어 받기 표지입니다. 다시 질의하십시오".to_owned(),
            });
        }
        let building = {
            let slots = self.slots();
            let fresh = cursor.parts.iter().all(|p| {
                slots
                    .get(&p.volume)
                    .and_then(Slot::index)
                    .is_some_and(|i| Arc::ptr_eq(i, &p.index) && i.generation() == p.generation)
            });
            if !fresh {
                return Err(index::ResumeError::Stale.into());
            }
            Self::building(&slots)
        };
        let hits = self.take(&mut cursor, limit, caller, None)?;
        Ok(Self::page(cursor, hits, building))
    }

    fn page(cursor: Cursor, hits: Vec<Hit>, building: Vec<String>) -> Page {
        Page {
            hits,
            total: cursor.total,
            total_is_upper_bound: cursor.filtered,
            next: (!cursor.is_done()).then_some(cursor),
            building,
        }
    }
}

/// 파이프로 받은 질의는 늘 질의한 사용자로 거른다.
impl Backend for Volumes {
    fn search(
        &self,
        query: &QueryMessage,
        caller: &dyn Caller,
        cancel: &Cancel,
    ) -> Result<Page, ErrorMessage> {
        Volumes::search_as(self, query, caller, cancel)
    }

    fn resume(
        &self,
        cursor: Cursor,
        limit: usize,
        caller: &dyn Caller,
    ) -> Result<Page, ErrorMessage> {
        Volumes::resume_as(self, cursor, limit, caller)
    }

    fn status(&self) -> StatusMessage {
        Volumes::status(self)
    }
}
