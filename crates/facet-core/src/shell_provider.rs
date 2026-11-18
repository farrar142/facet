//! 셸 공급자: 가상 폴더(내 PC·휴지통·휴대폰)를 셸 네임스페이스로 읽는다(ADR-0004, S12).
//!
//! - 위치는 [`Location::Shell`]이고 셸 항목 식별자(PIDL) 바이트와 종류([`VirtualFolder`])를 갖는다. 내 PC는 정해진
//!   식별자([`THIS_PC_ID`])라 셸을 부르지 않고 만든다([`Location::this_pc`]).
//! - 셸 호출은 모두 프로세스에 하나인 전용 COM STA 스레드([`facet_win::shell_thread::ShellThread`])에서 한다. 스레드는
//!   처음 가상 폴더를 읽을 때 만든다(콜드 스타트에 들지 않음). 목록 읽기는 탭의 읽기 일꾼 스레드가 그 스레드에 맡기고
//!   기다린다. UI 스레드는 셸을 부르지 않는다.
//! - 목록은 [`LIST_BATCH`]개씩 묶음으로 열거하고, 묶음마다 지금까지 읽은 항목 수를 알린다(탭의 [`crate::tab::Tab::loading`]).
//!   탭이 다른 곳으로 가면 열거를 그만둔다.
//! - 열거하며 알게 된 폴더 자신의 표시 이름과 항목의 위치(실제 경로가 있으면 파일 시스템 위치, 없으면 그 항목의
//!   가상 폴더 위치)는 프로세스 안에 기억해 둔다: 경로 조각·탭 제목의 이름([`ShellProvider::segments`])과 항목 열기
//!   ([`ShellProvider::child`])가 셸을 다시 부르지 않고 쓴다.
//! - 내 PC의 항목은 드라이브(드라이브 문자 루트를 실제 경로로 가진 항목: 로컬·이동식·연결된 네트워크 드라이브)와 장치
//!   (실제 경로가 없고 `\\?\` 장치 경로를 품은 폴더: 연결된 휴대폰)만 둔다. 그 밖의 셸 항목(사용자 폴더 바로 가기,
//!   제3자 네임스페이스 확장)은 spec #27의 범위 밖이라 뺀다.
//! - 되는 일: 내 PC는 열기만 된다(드라이브 안의 작업은 파일 시스템 공급자로 넘어간 뒤의 일이다).
//! - 드라이브 용량([`DriveSpace`]): 내 PC의 드라이브 항목은 목록을 읽을 때 드라이브로 적혀 용량을 모르는 채로 먼저
//!   보이고, 탭이 목록을 받은 뒤 드라이브마다 따로 띄운 스레드가 파일 시스템 API([`facet_win::devices::disk_space`])로
//!   얻어 채운다([`query_space`]). 끊긴 네트워크 드라이브·빈 카드 리더는 다른 드라이브를 기다리게 하지 않고 모름으로 남는다.
//! - 변경 감시: 내 PC는 볼륨·휴대용 장치가 붙거나 떨어지면([`facet_win::devices::watch_devices`]) 그리고 드라이브 문자
//!   목록이 바뀌면([`DRIVE_POLL`]마다 확인) 목록을 다시 읽으라고 알린다. 다른 가상 폴더는 아직 감시하지 않는다.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime};

pub use facet_win::shell_folder::ShellEntry;
use facet_win::shell_folder::{self, ShellFolderError};
use facet_win::shell_thread::{CallError, ShellThread};

use crate::provider::{Candidate, PathSegment, Provider, ResolveError, Resolved};
use crate::watch::{Watch, WatchError, WatchEvent};
use crate::{
    Attributes, Capabilities, FileSystemProvider, Item, ListError, Location, ShellLocation,
    VirtualFolder,
};

/// #355: 셸 호출의 기본 시간 제한.
pub const CALL_TIMEOUT: Duration = Duration::ZERO; // #355

/// 내 PC(`::{20D04FE0-3AEA-1069-A2D8-08002B30309D}`)의 셸 항목 식별자: 바탕 화면 아래 CLSID 조각 하나(길이 20,
/// 종류 0x1F, 정렬 0x50, CLSID 16바이트)와 끝(0 두 바이트).
pub const THIS_PC_ID: [u8; 22] = [
    0x14, 0x00, 0x1F, 0x50, 0xE0, 0x4F, 0xD0, 0x20, 0xEA, 0x3A, 0x69, 0x10, 0xA2, 0xD8, 0x08, 0x00,
    0x2B, 0x30, 0x30, 0x9D, 0x00, 0x00,
];

/// 열거 묶음 하나의 항목 수. 묶음마다 읽은 항목 수를 탭에 알린다: 셸 항목 하나는 셸 호출 몇 번(이름·속성·식별자)이라
/// 느린 장치에서도 묶음 하나가 금방 차고, 1만 항목 휴지통에서도 알림이 40번 남짓이라 깨우기가 넘치지 않는다.
pub const LIST_BATCH: usize = 256;

/// 목록 읽기 하나를 기다리는 시간. 넘으면 그 셸 스레드를 버리고(다음 요청은 새 스레드) 읽기는 실패한다. spec #27의 셸
/// 호출 시간 제한(10초)과 같다. 「응답 없음」·「다시 시도」 화면과 시험용 시간 바꾸기는 #355가 한다.
const LIST_TIMEOUT: Duration = Duration::from_secs(10);

/// 내 PC의 드라이브 용량(바이트). 모르거나 아직 채우지 않았으면 `None`이다(화면은 「—」).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriveSpace {
    /// 전체 크기.
    pub total: Option<u64>,
    /// 이 사용자가 쓸 수 있는 남은 공간(탐색기와 같은 값).
    pub free: Option<u64>,
}

impl DriveSpace {
    /// 아직 모르는 용량.
    pub const UNKNOWN: DriveSpace = DriveSpace {
        total: None,
        free: None,
    };

    /// 쓴 몫(0..=1, 남은 공간이 적을수록 큼): 이름 열의 사용량 막대. 둘 중 하나라도 모르거나 전체 크기가 0이면 `None`.
    pub fn used_fraction(&self) -> Option<f32> {
        let (total, free) = (self.total?, self.free?);
        if total == 0 {
            return None;
        }
        let used = total.saturating_sub(free);
        Some((used as f64 / total as f64) as f32)
    }
}

/// 내 PC 감시가 드라이브 문자 목록을 다시 보는 간격. 장치 알림은 볼륨이 붙은 때 오지만 드라이브 문자는 조금 뒤에
/// 붙고, 네트워크 드라이브 연결은 장치 알림이 없다. 문자 목록 읽기(`GetLogicalDrives`)는 값 하나라 1초마다 봐도
/// 공짜에 가깝고, 꽂은 USB가 1초 안에 보이면 「새로 고침 없이」로 충분하다.
pub const DRIVE_POLL: Duration = Duration::from_secs(1);

/// 장치 알림을 받은 뒤 다시 읽기 전에 기다리는 시간. 장치 하나를 꽂으면 인터페이스 알림이 여럿 몰려 오고 셸이 새
/// 드라이브를 알기까지 조금 걸린다: 몰린 알림을 한 번의 다시 읽기로 묶는다.
const DEVICE_SETTLE: Duration = Duration::from_millis(300);

/// #352: 휴지통 항목의 원래 전체 경로와 지운 시각.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecycledEntry {
    pub original: PathBuf,
    pub deleted: SystemTime,
}

/// #355: 프로세스 전체의 셸 호출 시간 제한을 바꾼다(시험용).
#[doc(hidden)]
pub fn set_call_timeout(_timeout: Duration) {
    todo!("#355")
}

/// #355: 시험용 셸 호출 오류 주입.
#[doc(hidden)]
pub mod fault {
    use std::time::Duration;

    use crate::Location;

    /// #355: 살아 있는 동안 주입이 걸려 있다.
    pub struct Injected {}

    impl Drop for Injected {
        fn drop(&mut self) {} // #355
    }

    /// #355: `location`의 열거가 `stall`만큼 응답하지 않게 한다.
    pub fn stall_listing(_location: &Location, _stall: Duration) -> Injected {
        todo!("#355")
    }
}

/// 가상 폴더를 읽는 공급자.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShellProvider;

impl Provider for ShellProvider {
    /// 셸 스레드에서 열거한다. 끝날 때까지 막으므로 UI 스레드에서 부르지 않는다. 탭은 묶음마다 수를 받는
    /// [`list_streaming`]을 쓴다.
    fn list(&self, location: &Location) -> Result<Vec<Item>, ListError> {
        list_streaming(location, None, Arc::new(AtomicBool::new(false)))
    }

    /// 열거하며 알아 둔 항목의 위치: 실제 경로가 있으면(드라이브) 그 파일 시스템 위치, 없으면 그 항목의 가상 폴더
    /// 위치. 아직 열거하지 않은 이름이면 지금 위치 그대로다.
    fn child(&self, location: &Location, name: &OsStr) -> Location {
        location
            .shell_location()
            .and_then(|shell| known().children.get(shell.id())?.get(name).cloned())
            .unwrap_or_else(|| location.clone())
    }

    /// 내 PC에는 상위가 없다.
    fn parent(&self, _location: &Location) -> Option<Location> {
        None
    }

    /// 그 위치 하나의 조각(이름은 셸의 표시 이름).
    fn segments(&self, location: &Location) -> Vec<PathSegment> {
        let Some(shell) = location.shell_location() else {
            return Vec::new();
        };
        vec![PathSegment {
            label: display_name(shell),
            location: location.clone(),
        }]
    }

    fn resolve(&self, text: &str) -> Result<Resolved, ResolveError> {
        FileSystemProvider.resolve(text)
    }

    fn complete(&self, text: &str) -> Vec<Candidate> {
        FileSystemProvider.complete(text)
    }

    /// 내 PC는 장치 붙임·뗌과 드라이브 문자 목록의 변화를 [`WatchEvent::Overflow`](다시 읽기)로 알린다. 다른 가상
    /// 폴더는 아직 감시하지 않는다.
    fn watch(
        &self,
        location: &Location,
        on_event: Box<dyn FnMut(WatchEvent) + Send>,
    ) -> Result<Watch, WatchError> {
        if location.virtual_folder() != Some(VirtualFolder::ThisPc) {
            return Err(WatchError::Unsupported {
                location: location.clone(),
            });
        }
        watch_this_pc(on_event).map_err(|source| WatchError::Io {
            location: location.clone(),
            source,
        })
    }

    fn item(&self, _location: &Location, _name: &OsStr) -> Result<Option<Item>, ListError> {
        Ok(None)
    }

    /// 열기만 된다.
    fn capabilities(&self, _location: &Location) -> Capabilities {
        Capabilities {
            open: true,
            ..Capabilities::default()
        }
    }
}

/// 내 PC 감시 스레드가 받는 것.
enum Signal {
    /// 장치가 붙거나 떨어졌다.
    Device,
}

/// 내 PC 감시: 장치 알림을 받고 드라이브 문자 목록을 [`DRIVE_POLL`]마다 보는 스레드. 감시(돌려준 값)를 버리면 알림이
/// 풀리고 통로가 끊겨 스레드가 끝난다.
fn watch_this_pc(mut on_event: Box<dyn FnMut(WatchEvent) + Send>) -> std::io::Result<Watch> {
    let (sender, receiver) = mpsc::channel::<Signal>();
    let device_sender = Mutex::new(sender.clone());
    // 장치 알림을 걸지 못해도(Windows가 아님 등) 드라이브 문자 확인은 한다.
    let devices = facet_win::devices::watch_devices(move || {
        let sender = device_sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = sender.send(Signal::Device);
    })
    .ok();
    std::thread::Builder::new()
        .name("facet-this-pc-watch".into())
        .spawn(move || {
            let mut letters = facet_win::drive_letters();
            loop {
                match receiver.recv_timeout(DRIVE_POLL) {
                    Ok(Signal::Device) => {
                        std::thread::sleep(DEVICE_SETTLE);
                        while receiver.try_recv().is_ok() {}
                        letters = facet_win::drive_letters();
                        on_event(WatchEvent::Overflow);
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        let now = facet_win::drive_letters();
                        if now != letters {
                            letters = now;
                            on_event(WatchEvent::Overflow);
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        })?;
    Ok(Watch::new((devices, sender)))
}

/// 가상 폴더 `location`의 목록 `list`에서 드라이브 항목(이름과 루트 경로). 내 PC가 아니면 빈 목록이다.
pub(crate) fn drive_roots(
    location: &Location,
    names: impl Iterator<Item = OsString>,
) -> Vec<(OsString, PathBuf)> {
    if location.virtual_folder() != Some(VirtualFolder::ThisPc) {
        return Vec::new();
    }
    names
        .filter_map(|name| {
            let path = child_path(location, &name).filter(|path| is_drive_root(path))?;
            Some((name, path))
        })
        .collect()
}

/// 드라이브 `root`의 용량을 따로 띄운 스레드에서 얻어 `done`에 넘긴다. 곧바로 돌아온다. 같은 드라이브를 이미 얻는
/// 중이면(다른 탭, 막혀 있는 끊긴 네트워크 드라이브 등) 스레드를 더 띄우지 않고 그 결과를 함께 받는다.
pub(crate) fn query_space(root: PathBuf, done: impl FnOnce(DriveSpace) + Send + 'static) {
    type Waiters = HashMap<PathBuf, Vec<Box<dyn FnOnce(DriveSpace) + Send>>>;
    static IN_FLIGHT: LazyLock<Mutex<Waiters>> = LazyLock::new(Mutex::default);
    let in_flight = || {
        IN_FLIGHT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    };
    {
        let mut waiters = in_flight();
        if let Some(waiting) = waiters.get_mut(&root) {
            waiting.push(Box::new(done));
            return;
        }
        waiters.insert(root.clone(), vec![Box::new(done)]);
    }
    let spawned = std::thread::Builder::new()
        .name("facet-drive-space".into())
        .spawn({
            let root = root.clone();
            move || {
                let space = facet_win::devices::disk_space(&root);
                let space = DriveSpace {
                    total: space.map(|space| space.total),
                    free: space.map(|space| space.free),
                };
                let waiting = in_flight().remove(&root).unwrap_or_default();
                for done in waiting {
                    done(space);
                }
            }
        });
    if spawned.is_err() {
        in_flight().remove(&root);
    }
}

/// 주소창 글자가 가상 폴더의 이름(`내 PC`·`shell:MyComputerFolder`, 대소문자 무시)이면 그 위치.
pub(crate) fn resolve_alias(text: &str) -> Option<Location> {
    let text = text.trim();
    let text = text
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(text)
        .trim();
    (text.eq_ignore_ascii_case("shell:MyComputerFolder") || text.eq_ignore_ascii_case("내 PC"))
        .then(Location::this_pc)
}

/// 가상 폴더의 표시 이름: 열거하며 셸이 알려 준 이름. 아직 모르면 그 가상 폴더의 셸 이름(`shell:MyComputerFolder` 등)이다.
pub(crate) fn display_name(shell: &ShellLocation) -> String {
    if let Some(name) = known().names.get(shell.id()) {
        return name.clone();
    }
    match shell.folder() {
        VirtualFolder::ThisPc => "shell:MyComputerFolder".into(),
        VirtualFolder::RecycleBin => "shell:RecycleBinFolder".into(),
        VirtualFolder::Device => "shell:".into(),
    }
}

/// 가상 폴더 `location` 안 항목 `name`의 실제 경로(드라이브). 열거하며 알아 둔 것이다.
pub(crate) fn child_path(location: &Location, name: &OsStr) -> Option<PathBuf> {
    let shell = location.shell_location()?;
    known()
        .children
        .get(shell.id())?
        .get(name)?
        .file_system_path()
        .map(Path::to_path_buf)
}

/// 가상 폴더 `location`의 항목을 읽는다. 묶음마다 지금까지 읽은 항목 수를 `progress`로 알리고, `cancel`이 서면 남은
/// 열거를 그만둔다(그만둔 읽기의 목록은 그때까지의 것이고 탭이 버린다). 셸 스레드를 기다리므로 UI 스레드에서 부르지
/// 않는다.
pub(crate) fn list_streaming(
    location: &Location,
    progress: Option<Arc<dyn Fn(usize) + Send + Sync>>,
    cancel: Arc<AtomicBool>,
) -> Result<Vec<Item>, ListError> {
    let Some(shell) = location.shell_location() else {
        return Err(ListError::NotAFolder {
            location: location.clone(),
        });
    };
    let fake = fakes().get(shell.id()).cloned();
    let (name, collector) = match fake {
        Some(entries) => {
            let mut collector = Collector::new(shell.folder());
            for batch in entries.chunks(LIST_BATCH) {
                collector.add(batch.to_vec());
                if let Some(progress) = &progress {
                    progress(collector.items.len());
                }
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
            }
            (None, collector)
        }
        None => {
            let id = shell.id().to_vec();
            let folder = shell.folder();
            let outcome = shell_thread().call(LIST_TIMEOUT, move || {
                let name = shell_folder::display_name(&id).ok();
                let mut collector = Collector::new(folder);
                shell_folder::enumerate(&id, LIST_BATCH, &mut |batch| {
                    collector.add(batch);
                    if let Some(progress) = &progress {
                        progress(collector.items.len());
                    }
                    !cancel.load(Ordering::Relaxed)
                })
                .map(|()| (name, collector))
            });
            match outcome {
                Ok(Ok(listing)) => listing,
                Ok(Err(error)) => return Err(list_error(location, error)),
                Err(error) => return Err(call_error(location, error)),
            }
        }
    };
    let Collector {
        items, children, ..
    } = collector;
    let replaced = {
        let mut known = known();
        if let Some(name) = name.filter(|name| !name.trim().is_empty()) {
            known.names.insert(shell.id().into(), name);
        }
        known.children.insert(shell.id().into(), children)
    };
    // 앞 열거의 표는 잠금 밖에서 버린다.
    drop(replaced);
    Ok(items)
}

/// 열거 묶음을 목록 항목으로 바꿔 모은다.
struct Collector {
    folder: VirtualFolder,
    items: Vec<Item>,
    children: HashMap<OsString, Location>,
}

impl Collector {
    fn new(folder: VirtualFolder) -> Collector {
        Collector {
            folder,
            items: Vec::new(),
            children: HashMap::new(),
        }
    }

    fn add(&mut self, batch: Vec<ShellEntry>) {
        self.items.reserve(batch.len());
        for entry in batch {
            if entry.name.is_empty() || !self.keeps(&entry) {
                continue;
            }
            let location = match entry.path {
                Some(path) => Location::file_system(path),
                None => {
                    let kind = match self.folder {
                        VirtualFolder::RecycleBin => VirtualFolder::RecycleBin,
                        VirtualFolder::ThisPc | VirtualFolder::Device => VirtualFolder::Device,
                    };
                    Location::shell(kind, entry.id)
                }
            };
            let name = OsString::from(entry.name);
            self.children.insert(name.clone(), location);
            self.items.push(Item {
                name,
                is_folder: entry.is_folder,
                size: 0,
                modified: SystemTime::UNIX_EPOCH,
                attributes: Attributes::default(),
            });
        }
    }

    /// 내 PC에서는 드라이브와 장치만 둔다(모듈 문서).
    fn keeps(&self, entry: &ShellEntry) -> bool {
        if self.folder != VirtualFolder::ThisPc {
            return true;
        }
        match &entry.path {
            Some(path) => is_drive_root(path),
            None => entry.is_folder && entry.parsing_name.contains(r"\\?\"),
        }
    }
}

/// 드라이브 문자 루트(`C:\`, `\\?\C:\`)인가.
pub(crate) fn is_drive_root(path: &Path) -> bool {
    use std::path::Prefix;
    let mut components = path.components();
    matches!(
        (components.next(), components.next(), components.next()),
        (Some(Component::Prefix(prefix)), Some(Component::RootDir), None)
            if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
    )
}

fn list_error(location: &Location, error: ShellFolderError) -> ListError {
    let location = location.clone();
    match error {
        ShellFolderError::NotFound => ListError::NotFound { location },
        other => ListError::Io {
            location,
            source: std::io::Error::other(other),
        },
    }
}

fn call_error(location: &Location, error: CallError) -> ListError {
    let kind = match error {
        CallError::TimedOut => std::io::ErrorKind::TimedOut,
        _ => std::io::ErrorKind::Other,
    };
    ListError::Io {
        location: location.clone(),
        source: std::io::Error::new(kind, format!("{error:?}")),
    }
}

/// 프로세스에 하나인 셸 스레드(처음 부를 때 만든다).
fn shell_thread() -> &'static ShellThread {
    static SHELL: OnceLock<ShellThread> = OnceLock::new();
    SHELL.get_or_init(ShellThread::new)
}

/// 열거하며 알게 된 것.
#[derive(Default)]
struct Known {
    /// 식별자 → 표시 이름.
    names: HashMap<Box<[u8]>, String>,
    /// 가상 폴더 식별자 → (항목 이름 → 위치). 마지막 열거의 것이다.
    children: HashMap<Box<[u8]>, HashMap<OsString, Location>>,
}

fn known() -> MutexGuard<'static, Known> {
    static KNOWN: LazyLock<Mutex<Known>> = LazyLock::new(Mutex::default);
    KNOWN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

type Fakes = HashMap<Box<[u8]>, Arc<Vec<ShellEntry>>>;

fn fakes() -> MutexGuard<'static, Fakes> {
    static FAKES: LazyLock<Mutex<Fakes>> = LazyLock::new(Mutex::default);
    FAKES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 시험·벤치마크용 가짜 셸 폴더: 셸을 부르지 않고 정해 둔 항목을 셸 열거처럼 묶음으로 낸다(공급자 추상화를 지나는
/// 목록 전달만 잰다). 앱은 부르지 않는다.
#[doc(hidden)]
pub mod fake {
    use super::*;

    /// 살아 있는 동안 [`FakeFolder::location`]의 열거가 넘긴 항목을 낸다.
    pub struct FakeFolder {
        location: Location,
    }

    impl FakeFolder {
        /// 가짜 폴더의 위치(장치 폴더).
        pub fn location(&self) -> &Location {
            &self.location
        }
    }

    impl Drop for FakeFolder {
        fn drop(&mut self) {
            if let Some(shell) = self.location.shell_location() {
                fakes().remove(shell.id());
                known().children.remove(shell.id());
            }
        }
    }

    /// `entries`를 내는 가짜 셸 폴더를 건다.
    pub fn serve(entries: Vec<ShellEntry>) -> FakeFolder {
        static NEXT: AtomicU16 = AtomicU16::new(0);
        let [lo, hi] = NEXT.fetch_add(1, Ordering::Relaxed).to_le_bytes();
        let id = vec![6, 0, 0xFA, 0xCE, lo, hi, 0, 0];
        fakes().insert(id.clone().into(), Arc::new(entries));
        FakeFolder {
            location: Location::shell(VirtualFolder::Device, id),
        }
    }
}
