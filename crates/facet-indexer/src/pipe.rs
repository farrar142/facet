//! 질의 파이프 서버(Windows 전용). 이 모듈과 [`crate::volume`]만 `unsafe`를 쓴다.
//!
//! - 서버는 파이프 이름의 **첫 인스턴스**를 만든다(`FILE_FLAG_FIRST_PIPE_INSTANCE`). 같은 이름의
//!   파이프가 이미 있으면 다른 프로세스가 먼저 가로챈 것이므로 시작하지 않는다.
//! - 원격 클라이언트를 거부한다(`PIPE_REJECT_REMOTE_CLIENTS`, 그리고 보안 설명자에서 NETWORK 거부).
//! - 보안 설명자([`security_sddl`])는 대화형 로그온 그룹(`S-1-5-4`)에 읽기·쓰기만 준다.
//!   인스턴스를 더 만드는 권한(`FILE_CREATE_PIPE_INSTANCE`)은 주지 않는다. 서버 자신의 계정
//!   (서비스면 LocalSystem)만 모든 권한을 가진다. 그래야 서버가 인스턴스를 더 만든다.
//! - 연결마다 스레드 하나가 [`crate::session::serve`]로 대화한다. 한 연결에서 읽기와 쓰기가 동시에
//!   일어나므로 겹친 입출력(overlapped)으로 연다. 동기 핸들은 한 핸들의 입출력을 줄 세워, 읽기가
//!   막힌 동안 쓰기도 막힌다.

use std::ffi::c_void;
use std::io::{self, Read, Write};
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::slice;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED,
    ERROR_PIPE_NOT_CONNECTED, GetLastError, HANDLE, INVALID_HANDLE_VALUE, LocalFree, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetAce,
    GetSecurityDescriptorControl, GetTokenInformation, PSECURITY_DESCRIPTOR, PSID,
    SE_DACL_PROTECTED, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, INFINITE, OpenProcessToken, SetEvent, WaitForMultipleObjects,
};

use crate::session::{self, Backend};
use crate::volume::Handle;

/// 대화형 로그온 그룹(INTERACTIVE)의 SID.
pub const INTERACTIVE_SID: &str = "S-1-5-4";

/// 네트워크 로그온 그룹(NETWORK)의 SID. 보안 설명자가 이 그룹을 거부한다.
pub const NETWORK_SID: &str = "S-1-5-2";

/// 대화형 로그온 그룹에 주는 권한: 읽기(`FILE_GENERIC_READ`)와 데이터 쓰기(`FILE_WRITE_DATA`).
/// `FILE_GENERIC_WRITE`는 `FILE_APPEND_DATA`(파이프에서는 [`FILE_CREATE_PIPE_INSTANCE`])를
/// 담으므로 쓰지 않는다. 클라이언트는 이 권한만 청해 연다.
pub const CLIENT_ACCESS: u32 = 0x0012_0089 | 0x0002;

/// 파이프 인스턴스를 더 만드는 권한. 서버 계정 말고는 갖지 않아야 한다.
pub const FILE_CREATE_PIPE_INSTANCE: u32 = 0x0004;

/// 파일 객체의 모든 권한(`FILE_ALL_ACCESS`).
pub const FILE_ALL_ACCESS: u32 = 0x001F_01FF;

/// 파이프 버퍼 크기(방향마다).
const BUFFER: u32 = 64 * 1024;

/// 파이프 보안 설명자(SDDL). `owner_sid`는 서버 계정의 SID다.
///
/// - 보호(`P`): 상속받는 ACE가 없다.
/// - NETWORK 거부: 원격 클라이언트는 `PIPE_REJECT_REMOTE_CLIENTS`로도 막지만 한 번 더 막는다.
/// - 서버 계정 모든 권한: 인스턴스를 더 만든다.
/// - 대화형 로그온 그룹: [`CLIENT_ACCESS`]만.
pub fn security_sddl(owner_sid: &str) -> String {
    format!("D:P(D;;FA;;;NU)(A;;FA;;;{owner_sid})(A;;0x{CLIENT_ACCESS:x};;;IU)")
}

/// 보안 설명자의 접근 제어 항목(ACE) 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ace {
    /// 허용이면 `true`, 거부면 `false`.
    pub allow: bool,
    /// 대상 SID(예: `S-1-5-4`).
    pub sid: String,
    /// 권한 비트.
    pub mask: u32,
}

/// 만들어진 파이프에 실제로 걸린 보안 설명자.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipeSecurity {
    /// DACL이 상속을 막는가.
    pub protected: bool,
    /// DACL의 ACE들. 허용·거부가 아닌 ACE는 담지 않는다.
    pub aces: Vec<Ace>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn last_error() -> io::Error {
    io::Error::last_os_error()
}

/// `LocalFree`로 풀어야 하는 메모리.
struct Local(*mut c_void);

impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: self.0은 LocalAlloc 계열로 할당돼 Windows API가 돌려준 메모리이고 한 번만 푼다.
            unsafe { LocalFree(self.0) };
        }
    }
}

// SAFETY: 보안 설명자 메모리는 만든 뒤 읽기만 하고, LocalFree는 어느 스레드에서 불러도 된다.
unsafe impl Send for Local {}

/// NUL로 끝나는 UTF-16 문자열(Windows가 할당)을 읽는다.
///
/// # Safety
/// `p`는 NUL로 끝나는 유효한 UTF-16 문자열을 가리켜야 한다.
unsafe fn read_wide(p: *const u16) -> String {
    let mut len = 0;
    // SAFETY: 호출자가 NUL로 끝남을 보장하므로 NUL까지 읽는다.
    while unsafe { *p.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: 위에서 센 len개는 모두 유효하다.
    String::from_utf16_lossy(unsafe { slice::from_raw_parts(p, len) })
}

/// SID를 `S-1-...` 문자열로.
///
/// # Safety
/// `sid`는 유효한 SID를 가리켜야 한다.
unsafe fn sid_string(sid: PSID) -> io::Result<String> {
    let mut out: *mut u16 = null_mut();
    // SAFETY: 호출자가 sid의 유효함을 보장하고, out은 쓸 수 있는 지역 변수다.
    if unsafe { ConvertSidToStringSidW(sid, &mut out) } == 0 {
        return Err(last_error());
    }
    let out = Local(out.cast());
    // SAFETY: ConvertSidToStringSidW가 NUL로 끝나는 문자열을 할당해 돌려줬다.
    Ok(unsafe { read_wide(out.0.cast()) })
}

/// 이 프로세스를 돌리는 계정의 SID(예: 서비스면 LocalSystem의 `S-1-5-18`).
pub fn current_user_sid() -> io::Result<String> {
    let mut token: HANDLE = null_mut();
    // SAFETY: GetCurrentProcess는 가짜 핸들을 돌려주고, token은 쓸 수 있는 지역 변수다.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(last_error());
    }
    let token = Handle(token);
    let mut needed = 0u32;
    // SAFETY: 크기만 묻는 호출이다(버퍼 없음, 길이 0).
    unsafe { GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut needed) };
    // TOKEN_USER는 포인터를 담으므로 u64로 잡아 정렬을 맞춘다.
    let mut buf = vec![0u64; (needed as usize).div_ceil(8).max(1)];
    // SAFETY: buf는 needed바이트 이상이고 그 크기를 함께 넘긴다.
    let ok = unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buf.as_mut_ptr().cast(),
            (buf.len() * 8) as u32,
            &mut needed,
        )
    };
    if ok == 0 {
        return Err(last_error());
    }
    // SAFETY: 성공했으므로 buf 앞머리는 TOKEN_USER이고 SID는 buf 안을 가리킨다.
    unsafe {
        let user = &*buf.as_ptr().cast::<TOKEN_USER>();
        sid_string(user.User.Sid)
    }
}

/// SDDL에서 만든 자기 완결 보안 설명자.
fn security_descriptor(sddl: &str) -> io::Result<Local> {
    let text = wide(sddl);
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: text는 NUL로 끝나는 UTF-16 문자열이고 sd는 쓸 수 있는 지역 변수다. 크기는 받지 않는다.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            null_mut(),
        )
    };
    if ok == 0 {
        return Err(last_error());
    }
    Ok(Local(sd))
}

/// 핸들에 실제로 걸린 DACL을 읽는다.
fn read_security(handle: HANDLE) -> io::Result<PipeSecurity> {
    let mut dacl: *mut ACL = null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: handle은 열려 있는 파이프 핸들이고, 받는 포인터들은 쓸 수 있는 지역 변수다.
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut sd,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let sd = Local(sd);
    let mut control = 0u16;
    let mut revision = 0u32;
    // SAFETY: sd.0은 GetSecurityInfo가 돌려준 유효한 보안 설명자다.
    if unsafe { GetSecurityDescriptorControl(sd.0, &mut control, &mut revision) } == 0 {
        return Err(last_error());
    }
    let mut aces = Vec::new();
    if !dacl.is_null() {
        // SAFETY: dacl은 sd 안을 가리키고 sd는 이 함수가 끝날 때까지 살아 있다.
        let count = unsafe { (*dacl).AceCount };
        for i in 0..u32::from(count) {
            let mut ace: *mut c_void = null_mut();
            // SAFETY: i는 ACE 개수보다 작다.
            if unsafe { GetAce(dacl, i, &mut ace) } == 0 {
                return Err(last_error());
            }
            // SAFETY: GetAce가 돌려준 ACE는 ACE_HEADER로 시작한다.
            let kind = unsafe { (*ace.cast::<ACE_HEADER>()).AceType };
            // 0 = ACCESS_ALLOWED_ACE_TYPE, 1 = ACCESS_DENIED_ACE_TYPE. 둘은 배치가 같다.
            if kind > 1 {
                continue;
            }
            let ace = ace.cast::<ACCESS_ALLOWED_ACE>();
            // SAFETY: 허용·거부 ACE는 머리, 권한, SID 순서이고 SID는 SidStart에서 시작한다.
            let (mask, sid) = unsafe {
                let sid_ptr: PSID = (&raw mut (*ace).SidStart).cast();
                ((*ace).Mask, sid_string(sid_ptr)?)
            };
            aces.push(Ace {
                allow: kind == 0,
                sid,
                mask,
            });
        }
    }
    Ok(PipeSecurity {
        protected: control & SE_DACL_PROTECTED != 0,
        aces,
    })
}

/// 파이프 인스턴스 하나를 만든다.
fn create_instance(name: &[u16], sd: &Local, first: bool) -> io::Result<Handle> {
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: 0,
    };
    let mut open_mode = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
    if first {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    // SAFETY: name은 NUL로 끝나는 UTF-16 문자열이고 attributes와 그 보안 설명자는 호출 동안 살아 있다.
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            BUFFER,
            BUFFER,
            0,
            &attributes,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        // SAFETY: 바로 앞 호출의 오류 코드를 읽는다.
        let code = unsafe { GetLastError() };
        if first && code == ERROR_ACCESS_DENIED {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "같은 이름의 파이프가 이미 있습니다(다른 프로세스가 먼저 만들었습니다)",
            ));
        }
        return Err(io::Error::from_raw_os_error(code as i32));
    }
    Ok(Handle(handle))
}

/// 수동으로 되돌리는 이벤트.
fn event() -> io::Result<Handle> {
    // SAFETY: 보안 속성과 이름 없이 수동 리셋, 꺼진 상태로 만든다.
    let h = unsafe { CreateEventW(null(), 1, 0, null()) };
    if h.is_null() {
        return Err(last_error());
    }
    Ok(Handle(h))
}

/// 연결 하나의 읽는 쪽 또는 쓰는 쪽. 같은 핸들을 나눠 갖고 저마다 이벤트를 가진다.
struct PipeIo {
    pipe: Arc<Handle>,
    event: Handle,
}

impl PipeIo {
    fn new(pipe: Arc<Handle>) -> io::Result<Self> {
        Ok(Self {
            pipe,
            event: event()?,
        })
    }

    /// 겹친 입출력 하나를 시작하고 끝날 때까지 기다린다. `start`는 OVERLAPPED를 받아
    /// ReadFile/WriteFile을 부른다.
    fn transfer(&self, start: impl FnOnce(*mut OVERLAPPED) -> i32) -> io::Result<usize> {
        // SAFETY: OVERLAPPED는 0으로 채워도 되는 C 구조체다.
        let mut ov: OVERLAPPED = unsafe { zeroed() };
        ov.hEvent = self.event.0;
        if start(&mut ov) == 0 {
            // SAFETY: 바로 앞 호출의 오류 코드를 읽는다.
            let code = unsafe { GetLastError() };
            if code != ERROR_IO_PENDING {
                return Err(io::Error::from_raw_os_error(code as i32));
            }
        }
        let mut done = 0u32;
        // SAFETY: ov는 시작한 입출력의 구조체이고, 기다리므로(TRUE) 끝나기 전에 사라지지 않는다.
        if unsafe { GetOverlappedResult(self.pipe.0, &ov, &mut done, 1) } == 0 {
            return Err(last_error());
        }
        Ok(done as usize)
    }
}

fn is_disconnect(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error().map(|c| c as u32),
        Some(ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED)
    )
}

impl Read for PipeIo {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let len = buf.len().min(u32::MAX as usize) as u32;
        let ptr = buf.as_mut_ptr();
        let h = self.pipe.0;
        // SAFETY: buf는 len바이트 쓸 수 있고 transfer가 입출력이 끝날 때까지 기다린다.
        match self.transfer(|ov| unsafe { ReadFile(h, ptr, len, null_mut(), ov) }) {
            Err(e) if is_disconnect(&e) => Ok(0),
            other => other,
        }
    }
}

impl Write for PipeIo {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let len = buf.len().min(u32::MAX as usize) as u32;
        let ptr = buf.as_ptr();
        let h = self.pipe.0;
        // SAFETY: buf는 len바이트 읽을 수 있고 transfer가 입출력이 끝날 때까지 기다린다.
        self.transfer(|ov| unsafe { WriteFile(h, ptr, len, null_mut(), ov) })
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 돌고 있는 파이프 서버. 버리면 새 연결 받기를 멈춘다(이미 받은 연결은 상대가 끊을 때까지 돈다).
pub struct PipeServer {
    stop: Arc<Handle>,
    accept: Option<JoinHandle<()>>,
    security: PipeSecurity,
}

impl PipeServer {
    /// `name`(예: [`facet_protocol::PIPE_NAME`])의 첫 인스턴스를 만들고 연결 받기를 시작한다.
    /// 같은 이름의 파이프가 이미 있으면 [`io::ErrorKind::AddrInUse`] 오류다.
    pub fn start(name: &str, backend: Arc<dyn Backend>) -> io::Result<Self> {
        let sd = security_descriptor(&security_sddl(&current_user_sid()?))?;
        let name = wide(name);
        let first = create_instance(&name, &sd, true)?;
        let security = read_security(first.0)?;
        let stop = Arc::new(event()?);
        let stop_for_thread = Arc::clone(&stop);
        let accept = thread::Builder::new()
            .name("facet-pipe-accept".into())
            .spawn(move || accept_loop(name, sd, first, &stop_for_thread, backend))?;
        Ok(Self {
            stop,
            accept: Some(accept),
            security,
        })
    }

    /// 첫 인스턴스에 실제로 걸린 보안 설명자.
    pub fn security(&self) -> &PipeSecurity {
        &self.security
    }
}

impl Drop for PipeServer {
    fn drop(&mut self) {
        // SAFETY: stop은 살아 있는 이벤트 핸들이다.
        unsafe { SetEvent(self.stop.0) };
        if let Some(t) = self.accept.take() {
            let _ = t.join();
        }
    }
}

/// 연결을 기다린다. 연결되면 `true`, 멈추라는 신호면 `false`.
fn wait_connect(pipe: &Handle, stop: &Handle) -> io::Result<bool> {
    let connected = event()?;
    // SAFETY: OVERLAPPED는 0으로 채워도 되는 C 구조체다.
    let mut ov: OVERLAPPED = unsafe { zeroed() };
    ov.hEvent = connected.0;
    // SAFETY: pipe는 겹친 입출력으로 연 서버 인스턴스이고 ov는 이 함수가 끝날 때까지 산다.
    if unsafe { ConnectNamedPipe(pipe.0, &mut ov) } != 0 {
        return Ok(true);
    }
    // SAFETY: 바로 앞 호출의 오류 코드를 읽는다.
    match unsafe { GetLastError() } {
        ERROR_PIPE_CONNECTED => return Ok(true),
        ERROR_IO_PENDING => {}
        code => return Err(io::Error::from_raw_os_error(code as i32)),
    }
    let handles = [connected.0, stop.0];
    // SAFETY: 두 핸들 모두 살아 있는 이벤트다.
    let woke = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
    let stopping = woke != WAIT_OBJECT_0;
    if stopping {
        // SAFETY: ov로 시작한 연결 기다리기를 취소한다. 아래에서 끝나기를 기다린다.
        unsafe { CancelIoEx(pipe.0, &ov) };
    }
    let mut ignored = 0u32;
    // SAFETY: ov가 끝날 때까지 기다리므로 ov를 버리기 전에 입출력이 끝난다.
    let ok = unsafe { GetOverlappedResult(pipe.0, &ov, &mut ignored, 1) } != 0;
    if stopping {
        return Ok(false);
    }
    if ok { Ok(true) } else { Err(last_error()) }
}

fn accept_loop(name: Vec<u16>, sd: Local, first: Handle, stop: &Handle, backend: Arc<dyn Backend>) {
    let mut listening = Some(first);
    loop {
        let pipe = match listening.take() {
            Some(p) => p,
            None => match create_instance(&name, &sd, false) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("파이프 인스턴스를 만들지 못했습니다: {e}");
                    // SAFETY: stop은 살아 있는 이벤트 핸들이다.
                    let woke = unsafe { WaitForMultipleObjects(1, &stop.0, 0, 1000) };
                    if woke == WAIT_OBJECT_0 {
                        return;
                    }
                    continue;
                }
            },
        };
        match wait_connect(&pipe, stop) {
            Ok(true) => {
                let backend = Arc::clone(&backend);
                let spawned = thread::Builder::new()
                    .name("facet-pipe-conn".into())
                    .spawn(move || serve_connection(pipe, backend.as_ref()));
                if let Err(e) = spawned {
                    eprintln!("연결 스레드를 만들지 못했습니다: {e}");
                    thread::sleep(Duration::from_millis(100));
                }
            }
            Ok(false) => return,
            // 클라이언트가 붙자마자 끊는 등. 이 인스턴스는 버리고 새로 만든다.
            Err(_) => {}
        }
    }
}

fn serve_connection(pipe: Handle, backend: &dyn Backend) {
    let pipe = Arc::new(pipe);
    let (Ok(reader), Ok(writer)) = (PipeIo::new(Arc::clone(&pipe)), PipeIo::new(pipe)) else {
        return;
    };
    session::serve(reader, writer, backend);
}
