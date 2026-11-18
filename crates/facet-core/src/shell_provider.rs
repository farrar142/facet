//! 셸 공급자: 가상 폴더(내 PC·휴지통·휴대폰)를 셸 네임스페이스로 읽는다(ADR-0004, S12).
//!
//! - 위치는 [`Location::Shell`]이고 셸 항목 식별자(PIDL) 바이트와 종류([`VirtualFolder`])를 갖는다. 내 PC는 정해진
//!   식별자([`THIS_PC_ID`])라 셸을 부르지 않고 만든다([`Location::this_pc`]).
//! - 셸 호출은 전용 COM STA 스레드([`facet_win::shell_thread::ShellThread`])에서 한다: 내 PC·휴지통은 프로세스에
//!   하나인 스레드, 휴대폰은 장치마다 하나 더(한 장치가 멈춰도 내 PC·휴지통·다른 장치는 산다). 스레드는
//!   처음 그 가상 폴더를 읽을 때 만든다(콜드 스타트에 들지 않음). 목록 읽기는 탭의 읽기 일꾼 스레드가 그 스레드에 맡기고
//!   기다린다. UI 스레드는 셸을 부르지 않는다. 호출마다 시간 제한([`CALL_TIMEOUT`])이 있어 넘으면 그 스레드를 버리고
//!   읽기는 「응답 없음」([`ListError::NotResponding`])으로 실패한다. 다음 호출은 새 스레드에서 하고 버린 스레드의 늦은
//!   결과는 버려진다. 탭은 [`crate::tab::Tab::retry`]로 같은 읽기를 다시 한다.
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
//!   목록이 바뀌면([`DRIVE_POLL`]마다 확인) 목록을 다시 읽으라고 알린다. 휴대폰 위치는 장치 알림이 오면 그 장치(장치
//!   식별자 [`device_id`])가 내 PC에 있는지 보고, 빠지거나 다시 꽂혔으면 다시 읽으라고 알린다. 휴지통은 감시하지 않는다.
//! - 휴대폰([`VirtualFolder::Device`]): 위치의 위(상위)는 식별자의 마지막 조각을 뗀 것이고 장치 바로 위는 내 PC다.
//!   열은 셸 속성의 크기·수정한 날짜다(장치가 주는 만큼). 읽지 못한 까닭은 장치가 빠짐([`ListError::Disconnected`])과
//!   잠김·「파일 전송」이 아님([`ListError::DeviceNotReady`], 장치 바로 아래가 비어 있을 때도)으로 가른다
//!   ([`device_problem`]). 빠진 장치의 위치를 다시 읽을 때 식별자가 바뀌었으면 내 PC에서 같은 장치(장치 식별자가 같은
//!   항목)를 찾아 같은 해석 이름의 새 식별자로 이어 읽는다([`relink_parsing_name`]). 되는 일은 열기뿐이다(이름바꾸기·새
//!   폴더는 spec #27 밖이라 꺼 둔다).
//! - 휴지통([`Location::recycle_bin`], 정해진 식별자 [`RECYCLE_BIN_ID`])은 셸이 여러 드라이브의 휴지통을 하나로 열거한
//!   것을 그대로 보인다. 항목마다 셸이 준 휴지통 안 경로(`$R...`) 옆의 정보 파일(`$I...`)을 읽어 원래 이름(확장자 포함,
//!   탐색기의 확장자 숨김 설정과 상관없음)·원래 크기·원래 위치·지운 날짜([`RecycledEntry`])를 얻는다. 되는 일은 복원·
//!   비우기·영구 삭제뿐이다(이름바꾸기·새 폴더·복사해 넣기·복사해 오기·이동·휴지통 삭제·열기는 안 됨).
//!   facet이 이 프로세스에서 휴지통으로 보낸 항목([`recycle`])은 정보 파일의 경로 대신 보낼 때 넘긴 경로(사용자가 보던
//!   경로: 짧은 이름·`subst` 드라이브 등을 정보 파일은 풀어서 적는다)를 원래 위치로 보인다.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
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

/// 셸 호출(목록 읽기 하나)의 기본 시간 제한(spec #27의 「모든 셸 호출에 시간 제한」). 넘으면 그 셸 스레드를 버리고
/// (다음 호출은 새 스레드) 읽기는 [`ListError::NotResponding`]으로 실패한다. 응답하지 않는 휴대폰·네트워크 셸 폴더가
/// 그 탭을 오래 붙잡지 않으면서, 느리지만 살아 있는 장치(수천 항목의 휴대폰 폴더 첫 열거)는 끝낼 수 있는 길이다.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// 지금 셸 호출 시간 제한(마이크로초). [`set_call_timeout`]이 바꾼다.
static CALL_TIMEOUT_MICROS: AtomicU64 = AtomicU64::new(CALL_TIMEOUT.as_micros() as u64);

fn call_timeout() -> Duration {
    Duration::from_micros(CALL_TIMEOUT_MICROS.load(Ordering::Relaxed))
}

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

/// 셸 항목 식별자 바이트의 조각들(끝 표시 제외). 형식이 틀렸으면 읽은 데까지다.
fn id_chunks(id: &[u8]) -> Vec<&[u8]> {
    let mut chunks = Vec::new();
    let mut rest = id;
    while let Some((cb, _)) = rest.split_first_chunk::<2>() {
        let cb = usize::from(u16::from_le_bytes(*cb));
        if cb < 2 || cb > rest.len() {
            break;
        }
        chunks.push(&rest[..cb]);
        rest = &rest[cb..];
    }
    chunks
}

/// 조각들을 이어 끝 표시(0 두 바이트)를 붙인 식별자.
fn join_chunks(chunks: &[&[u8]]) -> Vec<u8> {
    let mut id: Vec<u8> = chunks.concat();
    id.extend([0, 0]);
    id
}

/// 식별자의 상위(마지막 조각을 뗀 것). 조각이 하나 이하면 `None`.
fn parent_id(id: &[u8]) -> Option<Vec<u8>> {
    let chunks = id_chunks(id);
    (chunks.len() >= 2).then(|| join_chunks(&chunks[..chunks.len() - 1]))
}

/// 휴대폰 위치 식별자의 장치 부분(앞 두 조각: 내 PC와 장치). 장치마다 셸 스레드를 가르는 열쇠다.
fn device_root_id(id: &[u8]) -> Vec<u8> {
    let chunks = id_chunks(id);
    join_chunks(&chunks[..chunks.len().min(2)])
}

/// 휴대폰 위치가 장치 자신(내 PC 바로 아래)인가.
fn is_device_root(id: &[u8]) -> bool {
    let chunks = id_chunks(id);
    chunks.len() == 2 && chunks[0] == &THIS_PC_ID[..THIS_PC_ID.len() - 2]
}

/// 셸 해석 이름(`SIGDN_DESKTOPABSOLUTEPARSING`)에서 장치 식별자: `\\?\`로 시작하는 장치 경로 조각(예:
/// `\\?\usb#vid_18d1&pid_4ee1#0123456789abcdef#{6ac27878-a6fa-4155-ba85-f98f491d4f33}`)을 소문자로 바꾼 것. USB 장치
/// 경로는 제조사·제품 번호와 일련 번호를 품어, 같은 휴대폰을 뽑았다 다시 꽂으면(다른 포트여도) 같다. 장치 경로가 없으면
/// `None`이다.
pub fn device_id(parsing_name: &str) -> Option<String> {
    let (start, end) = device_span(parsing_name)?;
    Some(parsing_name[start..end].to_lowercase())
}

/// 두 해석 이름이 같은 장치의 것인가(장치 식별자가 있고 같다).
pub fn same_device(a: &str, b: &str) -> bool {
    matches!((device_id(a), device_id(b)), (Some(a), Some(b)) if a == b)
}

/// 해석 이름에서 장치 경로 조각의 바이트 범위.
fn device_span(parsing_name: &str) -> Option<(usize, usize)> {
    const PREFIX: &str = r"\\?\";
    let start = parsing_name.find(PREFIX)?;
    let body = start + PREFIX.len();
    let end = parsing_name[body..]
        .find('\\')
        .map_or(parsing_name.len(), |at| body + at);
    (end > body).then_some((start, end))
}

/// 빠졌다 다시 꽂힌 장치의 폴더 `old`(해석 이름)를 지금 내 PC의 장치 항목 `device`(해석 이름)에 이은 해석 이름:
/// `device`의 장치 경로까지에 `old`의 장치 경로 뒤를 붙인다. 같은 장치가 아니면 `None`이다.
pub fn relink_parsing_name(old: &str, device: &str) -> Option<String> {
    if !same_device(old, device) {
        return None;
    }
    let (_, old_end) = device_span(old)?;
    let (_, device_end) = device_span(device)?;
    Some(format!("{}{}", &device[..device_end], &old[old_end..]))
}

/// 휴대폰 위치를 읽지 못한 까닭의 갈래.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceProblem {
    /// 장치가 빠졌다(「연결 끊김」).
    Disconnected,
    /// 장치가 잠겼거나 「파일 전송」이 아니다.
    NotReady,
}

/// 셸이 알린 실패를 휴대폰 위치의 까닭으로 가른다. 휴대폰 갈래가 아니면 `None`(그대로 읽기 오류)이다.
pub fn device_problem(error: &ShellFolderError) -> Option<DeviceProblem> {
    /// `HRESULT_FROM_WIN32(code)`.
    const fn win32(code: u32) -> i32 {
        (0x8007_0000 | code) as i32
    }
    const DISCONNECTED: [i32; 5] = [
        win32(31),   // ERROR_GEN_FAILURE: 장치가 빠지는 중 WPD가 돌려줌
        win32(55),   // ERROR_DEV_NOT_EXIST
        win32(433),  // ERROR_NO_SUCH_DEVICE
        win32(1167), // ERROR_DEVICE_NOT_CONNECTED
        win32(1617), // ERROR_DEVICE_REMOVED
    ];
    const NOT_READY: [i32; 3] = [
        win32(5),   // E_ACCESSDENIED: 잠긴 휴대폰
        win32(21),  // ERROR_NOT_READY
        win32(170), // ERROR_BUSY
    ];
    match error {
        ShellFolderError::NotFound => Some(DeviceProblem::Disconnected),
        ShellFolderError::Failed { code } if DISCONNECTED.contains(code) => {
            Some(DeviceProblem::Disconnected)
        }
        ShellFolderError::Failed { code } if NOT_READY.contains(code) => {
            Some(DeviceProblem::NotReady)
        }
        _ => None,
    }
}

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

/// 휴지통 항목의 원래 전체 경로(지우기 전)와 지운 시각.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecycledEntry {
    pub original: PathBuf,
    pub deleted: SystemTime,
}

/// 프로세스 전체의 셸 호출 시간 제한을 바꾼다(시험용, 앱은 부르지 않는다). 이미 기다리는 호출에는 듣지 않는다.
#[doc(hidden)]
pub fn set_call_timeout(timeout: Duration) {
    let micros = u64::try_from(timeout.as_micros()).unwrap_or(u64::MAX);
    CALL_TIMEOUT_MICROS.store(micros, Ordering::Relaxed);
}

/// 시험용 셸 호출 오류 주입. 앱은 부르지 않는다.
#[doc(hidden)]
pub mod fault {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{LazyLock, Mutex, MutexGuard};
    use std::time::Duration;

    use crate::Location;

    /// 걸린 멈춤: 주입 번호, 위치, 멈출 시간.
    type Stalls = Vec<(u64, Location, Duration)>;

    fn stalls() -> MutexGuard<'static, Stalls> {
        static STALLS: LazyLock<Mutex<Stalls>> = LazyLock::new(Mutex::default);
        STALLS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 살아 있는 동안 주입이 걸려 있다. 버리면 풀린다.
    pub struct Injected {
        id: u64,
    }

    impl Drop for Injected {
        fn drop(&mut self) {
            stalls().retain(|(id, _, _)| *id != self.id);
        }
    }

    /// 돌려준 값이 살아 있는 동안 `location`의 열거가 셸 스레드에서 `stall`만큼 응답하지 않는다(그 뒤 원래대로
    /// 열거한다). 버리면 풀리고, 이미 멈춘 열거는 그대로 `stall`을 채운다.
    pub fn stall_listing(location: &Location, stall: Duration) -> Injected {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        stalls().push((id, location.clone(), stall));
        Injected { id }
    }

    /// `location`의 열거에 걸린 멈춤(가장 나중에 건 것).
    pub(super) fn stall_for(location: &Location) -> Option<Duration> {
        stalls()
            .iter()
            .rev()
            .find(|(_, at, _)| at == location)
            .map(|(_, _, stall)| *stall)
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

    /// 내 PC·휴지통에는 상위가 없다. 휴대폰 위치는 식별자의 마지막 조각을 뗀 위치이고, 장치 자신의 위는 내 PC다.
    fn parent(&self, location: &Location) -> Option<Location> {
        let shell = location.shell_location()?;
        if shell.folder() != VirtualFolder::Device {
            return None;
        }
        let parent = parent_id(shell.id())?;
        if parent[..] == THIS_PC_ID[..] {
            Some(Location::this_pc())
        } else if id_chunks(&parent).len() >= 2 {
            Some(Location::shell(VirtualFolder::Device, parent))
        } else {
            None
        }
    }

    /// 위(상위)부터 그 위치까지의 조각(이름은 셸의 표시 이름): 내 PC·휴지통은 하나, 휴대폰 위치는 내 PC·장치·저장소·
    /// 폴더들.
    fn segments(&self, location: &Location) -> Vec<PathSegment> {
        let mut segments = Vec::new();
        let mut current = Some(location.clone());
        while let Some(location) = current {
            let Some(shell) = location.shell_location() else {
                break;
            };
            segments.push(PathSegment {
                label: display_name(shell),
                location: location.clone(),
            });
            current = self.parent(&location);
        }
        segments.reverse();
        segments
    }

    fn resolve(&self, text: &str) -> Result<Resolved, ResolveError> {
        FileSystemProvider.resolve(text)
    }

    fn complete(&self, text: &str) -> Vec<Candidate> {
        FileSystemProvider.complete(text)
    }

    /// 내 PC는 장치 붙임·뗌과 드라이브 문자 목록의 변화를, 휴대폰 위치는 그 장치가 빠지거나 다시 꽂힌 것을
    /// [`WatchEvent::Overflow`](다시 읽기)로 알린다. 휴지통은 감시하지 않는다.
    fn watch(
        &self,
        location: &Location,
        on_event: Box<dyn FnMut(WatchEvent) + Send>,
    ) -> Result<Watch, WatchError> {
        let watched = match location.virtual_folder() {
            Some(VirtualFolder::ThisPc) => watch_this_pc(on_event),
            Some(VirtualFolder::Device) if is_watched(location, VirtualFolder::Device) => {
                watch_device(location, on_event)
            }
            _ => {
                return Err(WatchError::Unsupported {
                    location: location.clone(),
                });
            }
        };
        watched.map_err(|source| WatchError::Io {
            location: location.clone(),
            source,
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

/// 가상 폴더 `location`(종류 `folder`)을 감시하는가: 내 PC와 휴대폰 위치(시험용 가짜 폴더는 빼고).
pub(crate) fn is_watched(location: &Location, folder: VirtualFolder) -> bool {
    match folder {
        VirtualFolder::ThisPc => true,
        VirtualFolder::Device => location
            .shell_location()
            .is_some_and(|shell| !fakes().contains_key(shell.id())),
        VirtualFolder::RecycleBin => false,
    }
}

/// 휴대폰 위치 감시: 장치 알림이 오면 그 장치가 내 PC에 있는지 보고, 있고 없음이 바뀌었으면 다시 읽으라고 알린다.
/// 감시(돌려준 값)를 버리면 알림이 풀리고 통로가 끊겨 스레드가 끝난다.
fn watch_device(
    location: &Location,
    mut on_event: Box<dyn FnMut(WatchEvent) + Send>,
) -> std::io::Result<Watch> {
    let (sender, receiver) = mpsc::channel::<Signal>();
    let device_sender = Mutex::new(sender.clone());
    let devices = facet_win::devices::watch_devices(move || {
        let sender = device_sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = sender.send(Signal::Device);
    })
    .ok();
    let id: Box<[u8]> = location
        .shell_location()
        .map(|shell| shell.id().into())
        .unwrap_or_default();
    std::thread::Builder::new()
        .name("facet-device-watch".into())
        .spawn(move || {
            // 감시를 거는 때는 그 장치를 읽으려는 때라 있다고 본다.
            let mut present = true;
            while let Ok(Signal::Device) = receiver.recv() {
                std::thread::sleep(DEVICE_SETTLE);
                while receiver.try_recv().is_ok() {}
                // 장치 식별자는 목록을 읽으며 알아 둔 해석 이름에서 얻는다(아직 모르면 이번 알림은 넘긴다).
                let Some(parsing) = known_parsing_name(&id) else {
                    continue;
                };
                if let Some(now) = device_present(&parsing)
                    && now != present
                {
                    present = now;
                    on_event(WatchEvent::Overflow);
                }
            }
        })?;
    Ok(Watch::new((devices, sender)))
}

/// 해석 이름 `parsing`의 장치가 지금 내 PC에 있는가. 내 PC를 읽지 못하면 `None`이다.
fn device_present(parsing: &str) -> Option<bool> {
    this_pc_devices().map(|devices| devices.iter().any(|(_, name)| same_device(name, parsing)))
}

/// 지금 내 PC의 장치 항목들(식별자와 해석 이름). 프로세스의 셸 스레드에서 읽는다. 읽지 못하면 `None`이다.
fn this_pc_devices() -> Option<Vec<(Vec<u8>, String)>> {
    let outcome = shell_thread().call(call_timeout(), || {
        let mut devices = Vec::new();
        shell_folder::enumerate(&THIS_PC_ID, LIST_BATCH, &mut |batch| {
            devices.extend(
                batch
                    .into_iter()
                    .filter(|entry| {
                        entry.path.is_none() && device_id(&entry.parsing_name).is_some()
                    })
                    .map(|entry| (entry.id, entry.parsing_name)),
            );
            true
        })
        .map(|()| devices)
    });
    outcome.ok()?.ok()
}

/// 식별자 `id`의 알아 둔 해석 이름. 그 위치가 모르면 장치 자신의 것을 본다(장치 식별자만 쓰는 쪽).
fn known_parsing_name(id: &[u8]) -> Option<String> {
    let known = known();
    known
        .parsing
        .get(id)
        .or_else(|| known.parsing.get(&device_root_id(id)[..]))
        .cloned()
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
            (Names::default(), collector)
        }
        None => {
            let folder = shell.folder();
            let id = known()
                .relinked
                .get(shell.id())
                .cloned()
                .unwrap_or_else(|| shell.id().to_vec());
            let read = |id: Vec<u8>| {
                enumerate_on(location, id, folder, progress.clone(), Arc::clone(&cancel))
            };
            match read(id.clone()) {
                Ok(listing) => listing,
                Err(Failure::Call(error)) => return Err(call_error(location, error)),
                Err(Failure::Shell(error)) if folder == VirtualFolder::Device => {
                    match device_problem(&error) {
                        Some(DeviceProblem::NotReady) => return Err(not_ready(location)),
                        Some(DeviceProblem::Disconnected) => {
                            // 같은 장치가 다른 식별자로 다시 꽂혔으면 그 식별자로 이어 읽는다.
                            let Some(new_id) = relink(shell.id(), &id) else {
                                return Err(disconnected(location));
                            };
                            match read(new_id.clone()) {
                                Ok(listing) => {
                                    known().relinked.insert(shell.id().into(), new_id);
                                    listing
                                }
                                Err(Failure::Call(error)) => {
                                    return Err(call_error(location, error));
                                }
                                Err(Failure::Shell(error)) => {
                                    return Err(device_error(location, error));
                                }
                            }
                        }
                        None => return Err(list_error(location, error)),
                    }
                }
                Err(Failure::Shell(error)) => return Err(list_error(location, error)),
            }
        }
    };
    let Collector {
        items,
        children,
        mut recycled,
        names,
        parsing,
        ..
    } = collector;
    // 장치 바로 아래가 비어 있으면 잠겼거나 「파일 전송」이 아니다(저장소를 보여 주지 않는다).
    if shell.folder() == VirtualFolder::Device
        && is_device_root(shell.id())
        && items.is_empty()
        && !stopped.load(Ordering::Relaxed)
    {
        return Err(not_ready(location));
    }
    if shell.folder() == VirtualFolder::RecycleBin {
        use_sent_paths(&mut recycled, !stopped.load(Ordering::Relaxed));
    }
    let replaced = {
        let mut known = known();
        if let Some(name) = name.name.filter(|name| !name.trim().is_empty()) {
            known.names.insert(shell.id().into(), name);
        }
        if let Some(parsing_name) = name.parsing {
            known.parsing.insert(shell.id().into(), parsing_name);
        }
        for (id, ancestor) in name.ancestors {
            if !ancestor.trim().is_empty() {
                known.names.insert(id, ancestor);
            }
        }
        for (id, child) in names {
            known.names.entry(id).or_insert(child);
        }
        known.parsing.extend(parsing);
        known.children.insert(shell.id().into(), children)
    };
    // 앞 열거의 표는 잠금 밖에서 버린다.
    drop(replaced);
    Ok(Listed { items, recycled })
}

/// 셸 스레드 호출의 실패.
enum Failure {
    /// 셸이 실패를 알렸다.
    Shell(ShellFolderError),
    /// 시간 제한을 넘었거나 일이 패닉했다.
    Call(CallError),
}

/// 열거하며 알게 된 폴더 자신의 이름들.
#[derive(Default)]
struct Names {
    /// 폴더 자신의 표시 이름.
    name: Option<String>,
    /// 휴대폰 위치면 폴더 자신의 해석 이름(장치 식별자, 다시 꽂힌 장치에 잇기).
    parsing: Option<String>,
    /// 휴대폰 위치면 위(상위)들의 식별자와 표시 이름(경로 조각).
    ancestors: Vec<(Box<[u8]>, String)>,
}

/// 식별자 `id`의 폴더(종류 `folder`)를 그 셸 스레드에서 열거한다. `location`은 시험용 멈춤 주입을 찾는 열쇠다.
fn enumerate_on(
    location: &Location,
    id: Vec<u8>,
    folder: VirtualFolder,
    progress: Option<Arc<dyn Fn(usize) + Send + Sync>>,
    cancel: Arc<AtomicBool>,
) -> Result<(Names, Collector), Failure> {
    let stall = fault::stall_for(location);
    let thread = thread_for(folder, &id);
    let outcome = thread.call(call_timeout(), move || {
        if let Some(stall) = stall {
            std::thread::sleep(stall);
        }
        let mut names = Names {
            name: shell_folder::display_name(&id).ok(),
            ..Names::default()
        };
        let device = folder == VirtualFolder::Device;
        if device {
            names.parsing = shell_folder::parsing_name(&id).ok();
            let mut ancestor = parent_id(&id);
            while let Some(at) = ancestor.filter(|at| id_chunks(at).len() >= 2) {
                if let Ok(name) = shell_folder::display_name(&at) {
                    names.ancestors.push((at.as_slice().into(), name));
                }
                ancestor = parent_id(&at);
            }
        }
        let mut collector = Collector::new(folder);
        let mut on_batch = |batch| {
            collector.add(batch);
            if let Some(progress) = &progress {
                progress(collector.items.len());
            }
            !cancel.load(Ordering::Relaxed)
        };
        let enumerated = if device {
            shell_folder::enumerate_detailed(&id, LIST_BATCH, &mut on_batch)
        } else {
            shell_folder::enumerate(&id, LIST_BATCH, &mut on_batch)
        };
        enumerated.map(|()| (names, collector))
    });
    match outcome {
        Ok(Ok(listing)) => Ok(listing),
        Ok(Err(error)) => Err(Failure::Shell(error)),
        Err(error) => Err(Failure::Call(error)),
    }
}

/// 빠졌던 휴대폰 위치 `original`(지금 읽으려던 식별자 `current`)을 다시 꽂힌 같은 장치에 잇는다: 알아 둔 해석
/// 이름의 장치 식별자로 내 PC에서 같은 장치를 찾아 그 장치 아래 같은 해석 이름의 식별자. 장치가 없거나 식별자가 그대로면
/// `None`이다.
fn relink(original: &[u8], current: &[u8]) -> Option<Vec<u8>> {
    let old = {
        let known = known();
        known
            .parsing
            .get(current)
            .or_else(|| known.parsing.get(original))
            .cloned()
    }?;
    let devices = this_pc_devices()?;
    let parsing = devices
        .iter()
        .find_map(|(_, device)| relink_parsing_name(&old, device))?;
    let lookup = parsing.clone();
    let new_id = shell_thread()
        .call(call_timeout(), move || {
            shell_folder::id_from_parsing_name(&lookup)
        })
        .ok()?
        .ok()?;
    if new_id == current {
        return None;
    }
    known().parsing.insert(new_id.as_slice().into(), parsing);
    Some(new_id)
}

fn disconnected(location: &Location) -> ListError {
    ListError::Disconnected {
        location: location.clone(),
    }
}

fn not_ready(location: &Location) -> ListError {
    ListError::DeviceNotReady {
        location: location.clone(),
    }
}

/// 휴대폰 위치를 읽지 못한 셸 실패를 까닭으로.
fn device_error(location: &Location, error: ShellFolderError) -> ListError {
    match device_problem(&error) {
        Some(DeviceProblem::Disconnected) => disconnected(location),
        Some(DeviceProblem::NotReady) => not_ready(location),
        None => list_error(location, error),
    }
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
    /// 가상 폴더인 항목(휴대폰의 장치·저장소·폴더)의 식별자와 표시 이름(경로 조각).
    names: Vec<(Box<[u8]>, String)>,
    /// 가상 폴더인 항목의 식별자와 해석 이름(장치 식별자).
    parsing: Vec<(Box<[u8]>, String)>,
}

impl Collector {
    fn new(folder: VirtualFolder) -> Collector {
        Collector {
            folder,
            items: Vec::new(),
            children: HashMap::new(),
            recycled: Vec::new(),
            names: Vec::new(),
            parsing: Vec::new(),
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
                    if entry.is_folder {
                        let id: Box<[u8]> = entry.id.as_slice().into();
                        self.names.push((id.clone(), entry.name.clone()));
                        if !entry.parsing_name.is_empty() {
                            self.parsing.push((id, entry.parsing_name.clone()));
                        }
                    }
                    Location::shell(kind, entry.id)
                }
            };
            // 휴대폰 안 항목은 장치가 준 크기·수정한 날짜를 쓴다(없으면 0·알 수 없음, 화면은 비움).
            let (size, modified) = if self.folder == VirtualFolder::Device {
                (
                    entry.size.unwrap_or(0),
                    entry
                        .modified
                        .map_or(SystemTime::UNIX_EPOCH, filetime_to_system_time),
                )
            } else {
                (0, SystemTime::UNIX_EPOCH)
            };
            let name = OsString::from(entry.name);
            self.children.insert(name.clone(), location);
            self.items.push(Item {
                name,
                is_folder: entry.is_folder,
                size,
                modified,
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

/// 셸 스레드 호출의 실패: 시간 제한을 넘었으면 「응답 없음」(그 스레드는 버렸고 늦은 결과는 버려진다), 아니면 읽기 오류.
fn call_error(location: &Location, error: CallError) -> ListError {
    match error {
        CallError::TimedOut => ListError::NotResponding {
            location: location.clone(),
        },
        CallError::Failed => ListError::Io {
            location: location.clone(),
            source: std::io::Error::other(format!("{error:?}")),
        },
    }
}

/// 프로세스에 하나인 셸 스레드(내 PC·휴지통, 처음 부를 때 만든다).
fn shell_thread() -> &'static Arc<ShellThread> {
    static SHELL: OnceLock<Arc<ShellThread>> = OnceLock::new();
    SHELL.get_or_init(|| Arc::new(ShellThread::new()))
}

/// 식별자 `id`의 폴더(종류 `folder`)를 읽을 셸 스레드: 휴대폰은 장치마다 하나(앞 두 조각으로 가름), 그 밖은 프로세스에
/// 하나. 한 장치가 응답하지 않아 그 스레드를 버려도 다른 장치·내 PC·휴지통의 스레드는 그대로다.
fn thread_for(folder: VirtualFolder, id: &[u8]) -> Arc<ShellThread> {
    if folder != VirtualFolder::Device {
        return Arc::clone(shell_thread());
    }
    type Threads = HashMap<Vec<u8>, Arc<ShellThread>>;
    static DEVICES: LazyLock<Mutex<Threads>> = LazyLock::new(Mutex::default);
    let mut devices = DEVICES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Arc::clone(
        devices
            .entry(device_root_id(id))
            .or_insert_with(|| Arc::new(ShellThread::new())),
    )
}

/// 열거하며 알게 된 것.
#[derive(Default)]
struct Known {
    /// 식별자 → 표시 이름.
    names: HashMap<Box<[u8]>, String>,
    /// 가상 폴더 식별자 → (항목 이름 → 위치). 마지막 열거의 것이다.
    children: HashMap<Box<[u8]>, HashMap<OsString, Location>>,
    /// 휴대폰 위치 식별자 → 해석 이름(장치 식별자를 품는다).
    parsing: HashMap<Box<[u8]>, String>,
    /// 다시 꽂힌 장치에 이은 휴대폰 위치: 탭의 위치 식별자 → 지금 읽는 식별자.
    relinked: HashMap<Box<[u8]>, Vec<u8>>,
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

    const PHONE: &str = r"::{20D04FE0-3AEA-1069-A2D8-08002B30309D}\\\?\usb#vid_18d1&pid_4ee1#0123456789ABCDEF#{6ac27878-a6fa-4155-ba85-f98f491d4f33}";

    #[test]
    fn 해석_이름에서_장치_식별자를_얻는다() {
        assert_eq!(
            device_id(PHONE).as_deref(),
            Some(
                r"\\?\usb#vid_18d1&pid_4ee1#0123456789abcdef#{6ac27878-a6fa-4155-ba85-f98f491d4f33}"
            )
        );
        let dcim = format!(r"{PHONE}\SID-{{10001,,25048915968}}\{{00000001-0000}}");
        assert_eq!(device_id(&dcim), device_id(PHONE));
        assert_eq!(device_id(r"C:\사진"), None);
        assert_eq!(device_id(r"::{20D04FE0}\\\?\"), None);
    }

    #[test]
    fn 장치_식별자로_같은_장치를_찾는다() {
        let dcim = format!(r"{PHONE}\SID-{{10001,,25048915968}}\DCIM");
        // 대소문자만 다른 장치 경로는 같은 장치다.
        assert!(same_device(&dcim, &PHONE.to_uppercase()));
        let other = PHONE.replace("0123456789ABCDEF", "FEDCBA9876543210");
        assert!(!same_device(&dcim, &other));
        assert!(!same_device(r"C:\a", r"C:\a"));
    }

    #[test]
    fn 다시_꽂힌_장치에_폴더를_잇는다() {
        let old = format!(r"{PHONE}\SID-{{10001,,25048915968}}\DCIM");
        // 다시 꽂은 장치 항목의 해석 이름(여기서는 대소문자가 다름)에 그 아래 같은 폴더를 잇는다.
        let replugged = PHONE.to_lowercase();
        assert_eq!(
            relink_parsing_name(&old, &replugged),
            Some(format!(r"{replugged}\SID-{{10001,,25048915968}}\DCIM"))
        );
        let other = PHONE.replace("0123456789ABCDEF", "FEDCBA9876543210");
        assert_eq!(relink_parsing_name(&old, &other), None);
    }

    #[test]
    fn 셸_실패를_휴대폰_까닭으로_가른다() {
        let failed = |code: u32| ShellFolderError::Failed { code: code as i32 };
        assert_eq!(
            device_problem(&ShellFolderError::NotFound),
            Some(DeviceProblem::Disconnected)
        );
        assert_eq!(
            device_problem(&failed(0x8007_048F)),
            Some(DeviceProblem::Disconnected)
        );
        assert_eq!(
            device_problem(&failed(0x8007_001F)),
            Some(DeviceProblem::Disconnected)
        );
        assert_eq!(
            device_problem(&failed(0x8007_0005)),
            Some(DeviceProblem::NotReady)
        );
        assert_eq!(
            device_problem(&failed(0x8007_0015)),
            Some(DeviceProblem::NotReady)
        );
        assert_eq!(device_problem(&failed(0x8000_4005)), None);
        assert_eq!(device_problem(&ShellFolderError::InvalidId), None);
    }

    #[test]
    fn 휴대폰_위치의_위는_식별자를_하나씩_뗀_위치이고_장치_위는_내_pc다() {
        let this_pc = &THIS_PC_ID[..THIS_PC_ID.len() - 2];
        let device_chunk = [6u8, 0, 1, 2, 3, 4];
        let storage_chunk = [4u8, 0, 5, 6];
        let device = join_chunks(&[this_pc, &device_chunk]);
        let storage = join_chunks(&[this_pc, &device_chunk, &storage_chunk]);
        let device_location = Location::shell(VirtualFolder::Device, device.clone());
        let storage_location = Location::shell(VirtualFolder::Device, storage.clone());
        assert!(is_device_root(&device));
        assert!(!is_device_root(&storage));
        assert_eq!(device_root_id(&storage), device);
        assert_eq!(
            ShellProvider.parent(&storage_location),
            Some(device_location.clone())
        );
        assert_eq!(
            ShellProvider.parent(&device_location),
            Some(Location::this_pc())
        );
        assert_eq!(ShellProvider.parent(&Location::this_pc()), None);
        assert_eq!(ShellProvider.parent(&Location::recycle_bin()), None);
        let segments: Vec<Location> = ShellProvider
            .segments(&storage_location)
            .into_iter()
            .map(|segment| segment.location)
            .collect();
        assert_eq!(
            segments,
            vec![Location::this_pc(), device_location, storage_location]
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
