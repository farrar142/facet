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
//! - 휴지통([`Location::recycle_bin`], 정해진 식별자 [`RECYCLE_BIN_ID`])은 셸이 여러 드라이브의 휴지통을 하나로 열거한
//!   것을 그대로 보인다. 항목마다 셸이 준 휴지통 안 경로(`$R...`) 옆의 정보 파일(`$I...`)을 읽어 원래 이름(확장자 포함,
//!   탐색기의 확장자 숨김 설정과 상관없음)·원래 크기·원래 위치·지운 날짜([`RecycledEntry`])를 얻는다. 되는 일은 복원·
//!   비우기·영구 삭제뿐이다(이름바꾸기·새 폴더·복사해 넣기·복사해 오기·이동·휴지통 삭제·열기는 안 됨).
//!   facet이 이 프로세스에서 휴지통으로 보낸 항목([`recycle`])은 정보 파일의 경로 대신 보낼 때 넘긴 경로(사용자가 보던
//!   경로: 짧은 이름·`subst` 드라이브 등을 정보 파일은 풀어서 적는다)를 원래 위치로 보인다.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime};

use facet_win::recycle::RecycleOutcome;
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

/// 휴지통(`::{645FF040-5081-101B-9F08-00AA002F954E}`)의 셸 항목 식별자: 바탕 화면 아래 CLSID 조각 하나(길이 20,
/// 종류 0x1F, 정렬 0x60, CLSID 16바이트)와 끝(0 두 바이트).
pub const RECYCLE_BIN_ID: [u8; 22] = [
    0x14, 0x00, 0x1F, 0x60, 0x40, 0xF0, 0x5F, 0x64, 0x81, 0x50, 0x1B, 0x10, 0x9F, 0x08, 0x00, 0xAA,
    0x00, 0x2F, 0x95, 0x4E, 0x00, 0x00,
];

/// 열거 묶음 하나의 항목 수. 묶음마다 읽은 항목 수를 탭에 알린다: 셸 항목 하나는 셸 호출 몇 번(이름·속성·식별자)이라
/// 느린 장치에서도 묶음 하나가 금방 차고, 1만 항목 휴지통에서도 알림이 40번 남짓이라 깨우기가 넘치지 않는다.
pub const LIST_BATCH: usize = 256;

/// 목록 읽기 하나를 기다리는 시간. 넘으면 그 셸 스레드를 버리고(다음 요청은 새 스레드) 읽기는 실패한다. spec #27의 셸
/// 호출 시간 제한(10초)과 같다. 「응답 없음」·「다시 시도」 화면과 시험용 시간 바꾸기는 #355가 한다.
const LIST_TIMEOUT: Duration = Duration::from_secs(10);

/// #351: 내 PC의 드라이브 용량(바이트, 모르면 `None`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriveSpace {
    pub total: Option<u64>,
    pub free: Option<u64>,
}

/// 휴지통 항목의 원래 전체 경로(지우기 전)와 지운 시각.
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
        list_streaming(location, None, Arc::new(AtomicBool::new(false))).map(|listed| listed.items)
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

    /// 가상 폴더는 아직 감시하지 않는다(탭은 감시를 걸지 않고 읽는다).
    fn watch(
        &self,
        location: &Location,
        _on_event: Box<dyn FnMut(WatchEvent) + Send>,
    ) -> Result<Watch, WatchError> {
        Err(WatchError::Unsupported {
            location: location.clone(),
        })
    }

    fn item(&self, _location: &Location, _name: &OsStr) -> Result<Option<Item>, ListError> {
        Ok(None)
    }

    /// 휴지통은 복원·비우기·영구 삭제만 되고, 그 밖의 가상 폴더는 열기만 된다.
    fn capabilities(&self, location: &Location) -> Capabilities {
        match location.virtual_folder() {
            Some(VirtualFolder::RecycleBin) => Capabilities {
                restore: true,
                empty: true,
                delete_permanently: true,
                ..Capabilities::default()
            },
            _ => Capabilities {
                open: true,
                ..Capabilities::default()
            },
        }
    }
}

/// 주소창 글자가 가상 폴더의 이름(`내 PC`·`shell:MyComputerFolder`·`휴지통`·`shell:RecycleBinFolder`, 대소문자
/// 무시)이면 그 위치.
pub(crate) fn resolve_alias(text: &str) -> Option<Location> {
    let text = text.trim();
    let text = text
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(text)
        .trim();
    if text.eq_ignore_ascii_case("shell:MyComputerFolder") || text.eq_ignore_ascii_case("내 PC") {
        Some(Location::this_pc())
    } else if text.eq_ignore_ascii_case("shell:RecycleBinFolder") || text == "휴지통" {
        Some(Location::recycle_bin())
    } else {
        None
    }
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

/// 가상 폴더 하나를 읽은 것: 항목들과, 휴지통이면 항목마다(같은 차례) 휴지통 정보.
pub(crate) struct Listed {
    pub items: Vec<Item>,
    /// 휴지통이면 `items`와 같은 차례로 항목마다 휴지통 정보와 휴지통 안 경로(`$R...`, 같은 이름의 항목을 가르는
    /// 열쇠). 정보 파일을 읽지 못한 항목은 `None`이다. 휴지통이 아니면 비어 있다.
    pub recycled: Vec<Option<(RecycledEntry, PathBuf)>>,
}

/// 가상 폴더 `location`의 항목을 읽는다. 묶음마다 지금까지 읽은 항목 수를 `progress`로 알리고, `cancel`이 서면 남은
/// 열거를 그만둔다(그만둔 읽기의 목록은 그때까지의 것이고 탭이 버린다). 셸 스레드를 기다리므로 UI 스레드에서 부르지
/// 않는다.
pub(crate) fn list_streaming(
    location: &Location,
    progress: Option<Arc<dyn Fn(usize) + Send + Sync>>,
    cancel: Arc<AtomicBool>,
) -> Result<Listed, ListError> {
    let Some(shell) = location.shell_location() else {
        return Err(ListError::NotAFolder {
            location: location.clone(),
        });
    };
    let stopped = Arc::clone(&cancel);
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
        items,
        children,
        mut recycled,
        ..
    } = collector;
    if shell.folder() == VirtualFolder::RecycleBin {
        use_sent_paths(&mut recycled, !stopped.load(Ordering::Relaxed));
    }
    let replaced = {
        let mut known = known();
        if let Some(name) = name.filter(|name| !name.trim().is_empty()) {
            known.names.insert(shell.id().into(), name);
        }
        known.children.insert(shell.id().into(), children)
    };
    // 앞 열거의 표는 잠금 밖에서 버린다.
    drop(replaced);
    Ok(Listed { items, recycled })
}

/// facet이 보낸 휴지통 항목은 보낼 때 넘긴 경로를 원래 경로로 쓰고, 끝까지 읽었으면(`complete`) 휴지통에 더는 없는
/// 기억은 버린다.
fn use_sent_paths(recycled: &mut [Option<(RecycledEntry, PathBuf)>], complete: bool) {
    let mut sent = sent();
    if sent.is_empty() {
        return;
    }
    let mut seen = std::collections::HashSet::new();
    for (entry, key) in recycled.iter_mut().flatten() {
        if let Some(original) = sent.get(key.as_path()) {
            // 정보 파일과 이름이 같을 때만(셸이 다른 이름으로 넣었으면 정보 파일을 믿는다).
            if original.file_name() == entry.original.file_name() {
                entry.original = original.clone();
            }
            seen.insert(key.clone());
        }
    }
    if complete {
        sent.retain(|key, _| seen.contains(key));
    }
}

/// 열거 묶음을 목록 항목으로 바꿔 모은다.
struct Collector {
    folder: VirtualFolder,
    items: Vec<Item>,
    children: HashMap<OsString, Location>,
    /// 휴지통이면 `items`와 같은 차례의 휴지통 정보([`Listed::recycled`]).
    recycled: Vec<Option<(RecycledEntry, PathBuf)>>,
}

impl Collector {
    fn new(folder: VirtualFolder) -> Collector {
        Collector {
            folder,
            items: Vec::new(),
            children: HashMap::new(),
            recycled: Vec::new(),
        }
    }

    fn add(&mut self, batch: Vec<ShellEntry>) {
        self.items.reserve(batch.len());
        for entry in batch {
            if entry.name.is_empty() || !self.keeps(&entry) {
                continue;
            }
            if self.folder == VirtualFolder::RecycleBin {
                self.add_recycled(entry);
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

    /// 휴지통 항목 하나: 정보 파일(`$I...`)을 읽을 수 있으면 원래 이름·크기·위치·지운 날짜로, 아니면 셸의 표시
    /// 이름만으로 둔다. 휴지통 항목은 열 수 없으므로 위치 표에 넣지 않는다.
    fn add_recycled(&mut self, entry: ShellEntry) {
        let info = recycled_path(&entry).and_then(|path| {
            let info = read_recycle_info(&info_path(&path)?)?;
            Some((info, path))
        });
        match info {
            Some((info, path)) => {
                let name = info
                    .original
                    .file_name()
                    .map_or_else(|| OsString::from(&entry.name), OsStr::to_os_string);
                self.items.push(Item {
                    name,
                    is_folder: entry.is_folder,
                    size: if entry.is_folder { 0 } else { info.size },
                    modified: info.deleted,
                    attributes: Attributes::default(),
                });
                self.recycled.push(Some((
                    RecycledEntry {
                        original: info.original,
                        deleted: info.deleted,
                    },
                    path,
                )));
            }
            None => {
                self.items.push(Item {
                    name: OsString::from(entry.name),
                    is_folder: entry.is_folder,
                    size: 0,
                    modified: SystemTime::UNIX_EPOCH,
                    attributes: Attributes::default(),
                });
                self.recycled.push(None);
            }
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

/// 휴지통으로 보낸다([`facet_win::recycle::recycle`]와 같다). 보낸 항목은 휴지통 안 경로 → 넘긴 경로를 기억해 두고
/// 휴지통 목록이 원래 위치로 쓴다(모듈 문서). 작업 스레드에서 부른다.
pub(crate) fn recycle(paths: &[PathBuf]) -> Vec<RecycleOutcome> {
    let outcomes = facet_win::recycle::recycle(paths);
    let mut sent = sent();
    for outcome in &outcomes {
        if let RecycleOutcome::Recycled(item) = outcome {
            sent.insert(item.recycled().to_path_buf(), item.original().to_path_buf());
        }
    }
    outcomes
}

/// facet이 휴지통으로 보낸 항목: 휴지통 안 경로 → 보낼 때 넘긴 경로. 휴지통을 읽을 때 휴지통에 없는 것은 뺀다.
fn sent() -> MutexGuard<'static, HashMap<PathBuf, PathBuf>> {
    static SENT: LazyLock<Mutex<HashMap<PathBuf, PathBuf>>> = LazyLock::new(Mutex::default);
    SENT.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 휴지통 항목의 휴지통 안 경로(`...\$Recycle.Bin\<SID>\$R...`): 셸이 준 실제 경로, 없으면 해석 이름.
fn recycled_path(entry: &ShellEntry) -> Option<PathBuf> {
    let path = entry
        .path
        .clone()
        .unwrap_or_else(|| PathBuf::from(&entry.parsing_name));
    let name = path.file_name()?.to_str()?;
    (name.len() > 2 && name.get(..2)?.eq_ignore_ascii_case("$R")).then_some(path)
}

/// 휴지통 안 경로 `$R...` 옆의 정보 파일 `$I...`.
fn info_path(recycled: &Path) -> Option<PathBuf> {
    let name = recycled.file_name()?.to_str()?;
    Some(recycled.with_file_name(format!("$I{}", name.get(2..)?)))
}

/// 휴지통 정보 파일(`$I...`)의 내용.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecycleInfo {
    /// 지우기 전의 전체 경로.
    pub original: PathBuf,
    /// 원래 크기(폴더는 안의 합).
    pub size: u64,
    /// 지운 시각.
    pub deleted: SystemTime,
}

/// 정보 파일 하나를 읽는다. 읽지 못하거나 형식이 아니면 `None`.
fn read_recycle_info(path: &Path) -> Option<RecycleInfo> {
    parse_recycle_info(&std::fs::read(path).ok()?)
}

/// 휴지통 정보 파일(`$I...`)의 바이트를 푼다. 머리 8바이트는 판(1: Vista~8.1, 2: 10 이상), 그 뒤 원래 크기 8바이트,
/// 지운 시각(`FILETIME`) 8바이트. 판 1은 이어 260글자 고정 UTF-16 경로, 판 2는 글자 수(끝 NUL 포함) 4바이트와 그만큼의
/// UTF-16 경로다. 경로는 NUL에서 끊는다. 형식이 아니면 `None`.
pub(crate) fn parse_recycle_info(bytes: &[u8]) -> Option<RecycleInfo> {
    let u64_at = |at: usize| -> Option<u64> {
        Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
    };
    let version = u64_at(0)?;
    let size = u64_at(8)?;
    let filetime = u64_at(16)?;
    let units: &[u8] = match version {
        1 => bytes.get(24..24 + 520)?,
        2 => {
            let count = u32::from_le_bytes(bytes.get(24..28)?.try_into().ok()?) as usize;
            bytes.get(28..28 + count.checked_mul(2)?)?
        }
        _ => return None,
    };
    let units: Vec<u16> = units
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .take_while(|&unit| unit != 0)
        .collect();
    if units.is_empty() {
        return None;
    }
    #[cfg(windows)]
    let original = {
        use std::os::windows::ffi::OsStringExt;
        PathBuf::from(OsString::from_wide(&units))
    };
    #[cfg(not(windows))]
    let original = PathBuf::from(String::from_utf16_lossy(&units));
    Some(RecycleInfo {
        original,
        size,
        deleted: filetime_to_system_time(filetime),
    })
}

/// `FILETIME`(1601-01-01부터 100ns 단위)을 시각으로.
fn filetime_to_system_time(filetime: u64) -> SystemTime {
    const UNIX_EPOCH_FILETIME: u64 = 116_444_736_000_000_000;
    match filetime.checked_sub(UNIX_EPOCH_FILETIME) {
        Some(since) => {
            SystemTime::UNIX_EPOCH
                + Duration::from_secs(since / 10_000_000)
                + Duration::from_nanos((since % 10_000_000) * 100)
        }
        None => SystemTime::UNIX_EPOCH,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    fn header(version: u64, size: u64, filetime: u64) -> Vec<u8> {
        [version, size, filetime]
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }

    /// 2026-11-18 00:00:00 UTC의 FILETIME.
    const FILETIME: u64 = 116_444_736_000_000_000 + 1_794_960_000 * 10_000_000 + 5;

    fn expected_time() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_794_960_000) + Duration::from_nanos(500)
    }

    #[test]
    fn 판_2_정보_파일을_푼다() {
        let path = r"C:\사진\여름 🌊.jpg";
        let mut bytes = header(2, 1234, FILETIME);
        let units = path.encode_utf16().count() as u32 + 1;
        bytes.extend(units.to_le_bytes());
        bytes.extend(utf16(path));
        bytes.extend([0, 0]);
        let info = parse_recycle_info(&bytes).unwrap();
        assert_eq!(info.original, PathBuf::from(path));
        assert_eq!(info.size, 1234);
        assert_eq!(info.deleted, expected_time());
    }

    #[test]
    fn 판_1_정보_파일을_푼다() {
        let path = r"D:\문서\a.txt";
        let mut bytes = header(1, 7, FILETIME);
        let mut name = utf16(path);
        name.resize(520, 0);
        bytes.extend(name);
        let info = parse_recycle_info(&bytes).unwrap();
        assert_eq!(info.original, PathBuf::from(path));
        assert_eq!(info.size, 7);
        assert_eq!(info.deleted, expected_time());
    }

    #[test]
    fn 형식이_아니면_none() {
        assert_eq!(parse_recycle_info(&[]), None);
        assert_eq!(parse_recycle_info(&header(3, 0, FILETIME)), None);
        // 판 2인데 글자 수만큼 바이트가 없다.
        let mut short = header(2, 0, FILETIME);
        short.extend(100u32.to_le_bytes());
        short.extend(utf16("C:"));
        assert_eq!(parse_recycle_info(&short), None);
        // 경로가 비었다.
        let mut empty = header(2, 0, FILETIME);
        empty.extend(1u32.to_le_bytes());
        empty.extend([0, 0]);
        assert_eq!(parse_recycle_info(&empty), None);
    }

    #[test]
    fn 정보_파일은_휴지통_안_경로_옆이다() {
        let recycled = Path::new(r"C:\$Recycle.Bin\S-1-5-21\$RZH5LXC.txt");
        assert_eq!(
            info_path(recycled).unwrap(),
            Path::new(r"C:\$Recycle.Bin\S-1-5-21\$IZH5LXC.txt")
        );
    }

    #[test]
    fn 주소창_이름으로_휴지통을_찾는다() {
        for text in [
            "휴지통",
            "shell:RecycleBinFolder",
            "SHELL:recyclebinfolder",
            " \"휴지통\" ",
        ] {
            assert_eq!(resolve_alias(text), Some(Location::recycle_bin()), "{text}");
        }
        assert_eq!(resolve_alias("내 PC"), Some(Location::this_pc()));
        assert_eq!(resolve_alias("휴지"), None);
    }
}
