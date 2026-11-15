//! 단일 실행의 Windows 부분. `unsafe`는 모두 이 파일 안에 있다.
//!
//! # 이름
//!
//! 이름 끝에 붙는 식별자는 `<바탕>-<사용자 SID>-<로그온 세션 LUID>-<무결성 수준 RID>`다.
//!
//! - 뮤텍스: `Local\facet-instance-<식별자>`. `Local\`은 터미널 서비스 세션 범위이고, 식별자가
//!   사용자와 로그온 세션까지 좁힌다.
//! - 파이프: `\\.\pipe\facet-instance-<식별자>`. 파이프 이름은 기계 전체에서 하나이므로 식별자가 범위를 정한다.
//!
//! 관리자 권한으로 실행한 facet은 연결된 관리자 토큰이라 로그온 세션 LUID가 다르고 무결성 수준도
//! 높으므로(High) 일반 facet(Medium)과 이름이 겹치지 않는다.
//!
//! # 파이프 보안 설명자
//!
//! [`pipe_sddl`]: `O:<사용자>D:P(D;;FA;;;NU)(A;;FA;;;<사용자>)S:(ML;;NW;;;<무결성 수준>)`
//!
//! - **같은 사용자만 연다.** DACL은 보호(`P`, 상속 없음)이고 허용 항목은 주인 프로세스를 돌리는 사용자
//!   SID 하나뿐이다. 다른 사용자(같은 기계의 다른 로그인, 서비스 계정 등)는 열 수 없다.
//!   Administrators·SYSTEM에도 따로 주지 않는다.
//! - **원격 클라이언트는 거부한다.** `PIPE_REJECT_REMOTE_CLIENTS`로 막고, DACL에서 NETWORK도 한 번 더 거부한다.
//! - **무결성 수준이 낮은 쪽은 쓰지 못한다.** 강제 레이블을 주인의 무결성 수준으로 걸고 「위로 쓰기
//!   금지」(`NW`)를 둔다. 같은 사용자라도 관리자 권한 facet(High)의 파이프에 일반 프로세스(Medium)가
//!   쓰지 못한다.
//! - **소유자는 사용자 SID다.** 넘기는 쪽([`forward`])은 연 파이프의 소유자가 자기 사용자 SID인지 확인하고
//!   아니면 넘기지 않는다([`ForwardError::NotOurs`]). 다른 사용자가 이름을 먼저 차지해도 그쪽으로 경로를
//!   보내지 않는다.
//! - 첫 인스턴스는 `FILE_FLAG_FIRST_PIPE_INSTANCE`로 만든다. 같은 이름의 파이프가 이미 있으면 받지 않는다.
//! - 넘기는 쪽은 `SECURITY_IDENTIFICATION`으로 연다. 주인은 넘기는 쪽을 가장(impersonate)할 수 없다.
//!
//! 다른 사용자로 막히는지는 CI에서 확인할 수 없어 사람이 확인한다.
//!
//! # 대화
//!
//! 넘기는 쪽이 본문 길이(`u32`, 리틀 엔디언)와 본문([`crate::single_instance::encode_launch`])을 쓰면,
//! 주인이 1바이트로 답한다(1 = 받음, 0 = 버림). 길이가 [`MAX_LAUNCH_MESSAGE_LEN`]을 넘으면 본문을 읽지 않고
//! 버린다. 받는 쪽의 읽기·쓰기는 [`CONNECTION_TIMEOUT`] 안에 끝나야 하고, 넘으면 그 연결을 버린다.

use std::ffi::c_void;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_BROKEN_PIPE,
    ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GetLastError,
    HANDLE, HLOCAL, LocalFree, WAIT_OBJECT_0, WIN32_ERROR,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_KERNEL_OBJECT,
};
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, OWNER_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_INFORMATION_CLASS,
    TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_STATISTICS, TOKEN_USER, TokenIntegrityLevel,
    TokenStatistics, TokenUser,
};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX, ReadFile,
    SECURITY_IDENTIFICATION, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, GetCurrentProcess, OpenProcessToken, SetEvent,
    WaitForMultipleObjects,
};
use windows::core::{PCWSTR, PWSTR};

use crate::single_instance::{
    ForwardError, InstanceName, Launch, MAX_LAUNCH_MESSAGE_LEN, decode_launch,
};
use crate::windows_impl::to_wide;

/// 받는 쪽이 연결 하나에 쓰는 최대 시간. 넘기는 쪽이 멈춰도 수신기가 막히지 않는다.
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(2);

/// 넘기는 쪽이 파이프가 아직 없을 때 다시 시도하는 간격.
const RETRY_INTERVAL: Duration = Duration::from_millis(20);

/// 파이프 버퍼 크기(방향마다).
const BUFFER: u32 = 64 * 1024;

const ACCEPTED: u8 = 1;
const REJECTED: u8 = 0;

/// 받은 실행을 넘길 콜백.
pub(crate) type Handler = Box<dyn FnMut(Launch) + Send>;

/// 닫아야 하는 핸들. 버리면 닫는다.
struct OwnedHandle(HANDLE);

// SAFETY: 커널 핸들은 어느 스레드에서 써도 된다. 닫기는 Drop에서 한 번만 한다.
unsafe impl Send for OwnedHandle {}
// SAFETY: 공유해서 쓰는 것은 SetEvent·WaitForMultipleObjects처럼 스레드 안전한 호출뿐이다.
unsafe impl Sync for OwnedHandle {}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: 이 값이 맡은 열린 핸들이고 여기서 한 번만 닫는다.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// `LocalFree`로 풀어야 하는 메모리.
struct Local(*mut c_void);

// SAFETY: 만든 뒤 읽기만 하고, LocalFree는 어느 스레드에서 불러도 된다.
unsafe impl Send for Local {}

impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: Windows API가 LocalAlloc 계열로 할당해 돌려준 메모리이고 한 번만 푼다.
            let _ = unsafe { LocalFree(Some(HLOCAL(self.0))) };
        }
    }
}

fn to_io_error(error: &windows::core::Error) -> io::Error {
    match WIN32_ERROR::from_error(error) {
        Some(code) => io::Error::from_raw_os_error(code.0 as i32),
        None => io::Error::other(error.message()),
    }
}

fn last_error() -> WIN32_ERROR {
    // SAFETY: 이 스레드의 마지막 오류 코드를 읽을 뿐이다.
    unsafe { GetLastError() }
}

/// 이 프로세스의 사용자·로그온 세션·무결성 수준.
struct Identity {
    user_sid: String,
    logon: u64,
    integrity: u32,
}

/// 토큰 정보 하나. 포인터를 담는 구조체가 있어 `u64`로 잡아 정렬을 맞춘다.
fn token_info(token: &OwnedHandle, class: TOKEN_INFORMATION_CLASS) -> io::Result<Vec<u64>> {
    let mut needed = 0u32;
    // SAFETY: 크기만 묻는 호출이다(버퍼 없음, 길이 0). 실패(버퍼 부족)는 예상한 것이다.
    let _ = unsafe { GetTokenInformation(token.0, class, None, 0, &mut needed) };
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8).max(1)];
    // SAFETY: buffer는 needed바이트 이상이고 그 크기를 함께 넘긴다.
    unsafe {
        GetTokenInformation(
            token.0,
            class,
            Some(buffer.as_mut_ptr().cast()),
            (buffer.len() * 8) as u32,
            &mut needed,
        )
    }
    .map_err(|error| to_io_error(&error))?;
    Ok(buffer)
}

/// SID를 `S-1-...` 문자열로.
///
/// # Safety
/// `sid`는 유효한 SID를 가리켜야 한다.
unsafe fn sid_string(sid: PSID) -> io::Result<String> {
    let mut text = PWSTR::null();
    // SAFETY: 호출자가 sid의 유효함을 보장하고, text는 쓸 수 있는 지역 변수다.
    unsafe { ConvertSidToStringSidW(sid, &mut text) }.map_err(|error| to_io_error(&error))?;
    let text = Local(text.0.cast());
    // SAFETY: ConvertSidToStringSidW가 NUL로 끝나는 문자열을 할당해 돌려줬다.
    unsafe { PCWSTR(text.0.cast()).to_string() }
        .map_err(|_| io::Error::other("SID 문자열이 깨졌습니다"))
}

fn identity() -> io::Result<Identity> {
    let mut token = HANDLE::default();
    // SAFETY: GetCurrentProcess는 닫지 않아도 되는 가짜 핸들이고, token은 쓸 수 있는 지역 변수다.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
        .map_err(|error| to_io_error(&error))?;
    let token = OwnedHandle(token);

    let user = token_info(&token, TokenUser)?;
    // SAFETY: 성공했으므로 버퍼 앞머리는 TOKEN_USER이고 SID는 버퍼 안을 가리킨다.
    let user_sid = unsafe { sid_string((*user.as_ptr().cast::<TOKEN_USER>()).User.Sid) }?;

    let statistics = token_info(&token, TokenStatistics)?;
    // SAFETY: 성공했으므로 버퍼 앞머리는 TOKEN_STATISTICS다.
    let logon_id = unsafe { (*statistics.as_ptr().cast::<TOKEN_STATISTICS>()).AuthenticationId };
    let logon = (u64::from(logon_id.HighPart as u32) << 32) | u64::from(logon_id.LowPart);

    let label = token_info(&token, TokenIntegrityLevel)?;
    // SAFETY: 성공했으므로 버퍼 앞머리는 TOKEN_MANDATORY_LABEL이고 SID는 버퍼 안을 가리킨다. 무결성 수준
    // SID(S-1-16-<RID>)의 마지막 하위 권한이 RID다.
    let integrity = unsafe {
        let sid = (*label.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()).Label.Sid;
        let count = *GetSidSubAuthorityCount(sid);
        if count == 0 {
            return Err(io::Error::other("무결성 수준 SID가 비었습니다"));
        }
        *GetSidSubAuthority(sid, u32::from(count) - 1)
    };
    Ok(Identity {
        user_sid,
        logon,
        integrity,
    })
}

fn instance_id(name: &InstanceName, identity: &Identity) -> String {
    format!(
        "facet-instance-{}-{}-{:016x}-{:x}",
        name.as_str(),
        identity.user_sid,
        identity.logon,
        identity.integrity
    )
}

fn mutex_name(name: &InstanceName, identity: &Identity) -> String {
    format!(r"Local\{}", instance_id(name, identity))
}

fn pipe_name(name: &InstanceName, identity: &Identity) -> String {
    format!(r"\\.\pipe\{}", instance_id(name, identity))
}

/// 이 프로세스가 주인일 때 쓰는 파이프의 전체 이름.
pub(crate) fn pipe_path(name: &InstanceName) -> io::Result<String> {
    Ok(pipe_name(name, &identity()?))
}

/// 파이프 보안 설명자(SDDL). 모듈 문서의 「파이프 보안 설명자」를 보라.
fn pipe_sddl(identity: &Identity) -> String {
    let sid = &identity.user_sid;
    format!(
        "O:{sid}D:P(D;;FA;;;NU)(A;;FA;;;{sid})S:(ML;;NW;;;S-1-16-{})",
        identity.integrity
    )
}

fn security_descriptor(sddl: &str) -> io::Result<Local> {
    let text = to_wide(sddl.as_ref());
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: text는 NUL로 끝나는 UTF-16 문자열이고 descriptor는 쓸 수 있는 지역 변수다.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(text.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
    }
    .map_err(|error| to_io_error(&error))?;
    Ok(Local(descriptor.0))
}

/// 주인 자리: 뮤텍스와 수신기.
pub(crate) struct Owner {
    // 필드는 적은 순서대로 버려진다. 수신기를 먼저 멈추고 뮤텍스를 놓는다.
    receiver: Option<Receiver>,
    pipe_name: String,
    sddl: String,
    _mutex: OwnedHandle,
}

pub(crate) fn claim(name: &InstanceName) -> io::Result<Option<Owner>> {
    let identity = identity()?;
    let mutex = to_wide(mutex_name(name, &identity).as_ref());
    // SAFETY: mutex는 NUL로 끝나는 UTF-16 이름이고 호출 동안 살아 있다. 기본 보안으로 만들고
    // 처음부터 잡지는 않는다(뮤텍스는 있는 것만으로 주인을 뜻한다).
    let created = unsafe { CreateMutexW(None, false, PCWSTR(mutex.as_ptr())) };
    // 성공 경로의 래퍼는 다른 API를 부르지 않으므로 마지막 오류는 CreateMutexW의 것이다.
    let already = last_error() == ERROR_ALREADY_EXISTS;
    match created {
        Ok(handle) => {
            let handle = OwnedHandle(handle);
            if already {
                return Ok(None);
            }
            Ok(Some(Owner {
                receiver: None,
                pipe_name: pipe_name(name, &identity),
                sddl: pipe_sddl(&identity),
                _mutex: handle,
            }))
        }
        // 이미 있지만 열 권한이 없다(다른 무결성 수준 등). 주인이 있는 것으로 본다.
        Err(error) if WIN32_ERROR::from_error(&error) == Some(ERROR_ACCESS_DENIED) => Ok(None),
        Err(error) => Err(to_io_error(&error)),
    }
}

impl Owner {
    pub(crate) fn listen(&mut self, handler: Handler) -> io::Result<()> {
        if self.receiver.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "수신기를 이미 열었습니다",
            ));
        }
        self.receiver = Some(Receiver::start(&self.pipe_name, &self.sddl, handler)?);
        Ok(())
    }
}

fn new_event() -> io::Result<OwnedHandle> {
    // SAFETY: 이름 없는 수동 재설정 이벤트를 만든다. 돌려받은 핸들은 OwnedHandle이 맡는다.
    let event = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
        .map_err(|error| to_io_error(&error))?;
    Ok(OwnedHandle(event))
}

/// 파이프 인스턴스 하나를 만든다. 첫 인스턴스는 같은 이름의 파이프가 이미 있으면 실패한다.
fn create_instance(name: &[u16], descriptor: &Local, first: bool) -> io::Result<OwnedHandle> {
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    let mut open_mode = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
    if first {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    // SAFETY: name은 NUL로 끝나는 UTF-16 문자열이고 attributes와 그 보안 설명자는 호출 동안 살아 있다.
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            BUFFER,
            BUFFER,
            0,
            Some(&attributes),
        )
    };
    if handle.is_invalid() {
        let code = last_error();
        if first && code == ERROR_ACCESS_DENIED {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "같은 이름의 파이프가 이미 있습니다",
            ));
        }
        return Err(io::Error::from_raw_os_error(code.0 as i32));
    }
    Ok(OwnedHandle(handle))
}

/// 도는 수신기. 버리면 수신기 스레드를 멈추고 끝날 때까지 기다린다.
struct Receiver {
    stop: Arc<OwnedHandle>,
    thread: Option<JoinHandle<()>>,
}

impl Receiver {
    fn start(pipe_name: &str, sddl: &str, handler: Handler) -> io::Result<Self> {
        let descriptor = security_descriptor(sddl)?;
        let name = to_wide(pipe_name.as_ref());
        let first = create_instance(&name, &descriptor, true)?;
        let stop = Arc::new(new_event()?);
        let thread_stop = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("facet-instance".into())
            .spawn(move || accept_loop(&name, &descriptor, first, &thread_stop, handler))?;
        Ok(Receiver {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        // SAFETY: 이벤트 핸들은 Arc가 살려 두고 있다.
        let _ = unsafe { SetEvent(self.stop.0) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// 겹친 입출력 하나를 기다린다. `stop` 신호나 `deadline`이 먼저 오면 취소하고 오류다.
fn finish(
    pipe: &OwnedHandle,
    overlapped: &OVERLAPPED,
    stop: &OwnedHandle,
    deadline: Option<Instant>,
) -> io::Result<u32> {
    let millis = match deadline {
        Some(deadline) => deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .min(u128::from(u32::MAX - 1)) as u32,
        None => u32::MAX, // INFINITE
    };
    let handles = [overlapped.hEvent, stop.0];
    // SAFETY: 두 핸들 모두 이 함수가 끝날 때까지 열려 있다.
    let woke = unsafe { WaitForMultipleObjects(&handles, false, millis) };
    let mut bytes = 0u32;
    if woke != WAIT_OBJECT_0 {
        // SAFETY: overlapped로 건 입출력을 취소하고, 그 완료를 기다려 커널이 버퍼를 더 쓰지 않게 한다.
        unsafe {
            let _ = CancelIoEx(pipe.0, Some(overlapped));
            let _ = GetOverlappedResult(pipe.0, overlapped, &mut bytes, true);
        }
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "연결이 제때 끝나지 않았습니다",
        ));
    }
    // SAFETY: 이벤트가 신호를 받았으므로 입출력은 끝났다.
    unsafe { GetOverlappedResult(pipe.0, overlapped, &mut bytes, false) }
        .map_err(|error| to_io_error(&error))?;
    Ok(bytes)
}

fn overlapped(event: &OwnedHandle) -> OVERLAPPED {
    OVERLAPPED {
        hEvent: event.0,
        ..Default::default()
    }
}

/// 연결을 기다린다. 연결되면 `true`, 멈추라는 신호면 `false`.
fn wait_connect(pipe: &OwnedHandle, event: &OwnedHandle, stop: &OwnedHandle) -> io::Result<bool> {
    let mut ov = overlapped(event);
    // SAFETY: pipe는 겹친 입출력으로 연 서버 인스턴스이고, ov는 완료를 기다릴 때까지(finish) 산다.
    match unsafe { ConnectNamedPipe(pipe.0, Some(&mut ov)) } {
        Ok(()) => return Ok(true),
        Err(error) => match WIN32_ERROR::from_error(&error) {
            Some(ERROR_PIPE_CONNECTED) => return Ok(true),
            Some(ERROR_IO_PENDING) => {}
            _ => return Err(to_io_error(&error)),
        },
    }
    match finish(pipe, &ov, stop, None) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::TimedOut => Ok(false),
        Err(error) => Err(error),
    }
}

/// 연결 하나의 읽기·쓰기. 모두 `deadline` 안에 끝나야 한다.
struct Connection<'a> {
    pipe: OwnedHandle,
    event: OwnedHandle,
    stop: &'a OwnedHandle,
    deadline: Instant,
}

impl Connection<'_> {
    fn read_some(&self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut ov = overlapped(&self.event);
        // SAFETY: 파이프는 겹친 입출력으로 열렸다. buffer와 ov는 finish가 완료(또는 취소 완료)를 기다릴
        // 때까지 살아 있다.
        let started = unsafe { ReadFile(self.pipe.0, Some(buffer), None, Some(&raw mut ov)) };
        if let Err(error) = started {
            match WIN32_ERROR::from_error(&error) {
                Some(ERROR_IO_PENDING) => {}
                Some(ERROR_BROKEN_PIPE) => return Ok(0),
                _ => return Err(to_io_error(&error)),
            }
        }
        match finish(&self.pipe, &ov, self.stop, Some(self.deadline)) {
            Ok(bytes) => Ok(bytes as usize),
            Err(error) if error.raw_os_error() == Some(ERROR_BROKEN_PIPE.0 as i32) => Ok(0),
            Err(error) => Err(error),
        }
    }

    fn read_exact(&self, buffer: &mut [u8]) -> io::Result<()> {
        let mut filled = 0;
        while filled < buffer.len() {
            match self.read_some(&mut buffer[filled..])? {
                0 => return Err(io::ErrorKind::UnexpectedEof.into()),
                n => filled += n,
            }
        }
        Ok(())
    }

    fn write_all(&self, bytes: &[u8]) -> io::Result<()> {
        let mut ov = overlapped(&self.event);
        // SAFETY: 파이프는 겹친 입출력으로 열렸다. bytes와 ov는 finish가 완료를 기다릴 때까지 살아 있다.
        let started = unsafe { WriteFile(self.pipe.0, Some(bytes), None, Some(&raw mut ov)) };
        if let Err(error) = started
            && WIN32_ERROR::from_error(&error) != Some(ERROR_IO_PENDING)
        {
            return Err(to_io_error(&error));
        }
        let written = finish(&self.pipe, &ov, self.stop, Some(self.deadline))?;
        if written as usize != bytes.len() {
            return Err(io::ErrorKind::WriteZero.into());
        }
        Ok(())
    }

    /// 넘기는 쪽의 메시지를 받아 답한다. 받은 실행이거나, 버렸으면 `None`.
    fn exchange(&self) -> Option<Launch> {
        let mut len = [0u8; 4];
        self.read_exact(&mut len).ok()?;
        let len = u32::from_le_bytes(len) as usize;
        if len > MAX_LAUNCH_MESSAGE_LEN {
            let _ = self.write_all(&[REJECTED]);
            return None;
        }
        let mut body = vec![0u8; len];
        self.read_exact(&mut body).ok()?;
        let launch = decode_launch(&body).ok();
        let reply = if launch.is_some() { ACCEPTED } else { REJECTED };
        self.write_all(&[reply]).ok()?;
        // 끊기(DisconnectNamedPipe)나 닫기가 읽히지 않은 답을 버리지 않도록, 넘기는 쪽이 닫을 때까지 기다린다.
        let mut rest = [0u8; 1];
        while let Ok(n) = self.read_some(&mut rest) {
            if n == 0 {
                break;
            }
        }
        launch
    }
}

fn accept_loop(
    name: &[u16],
    descriptor: &Local,
    first: OwnedHandle,
    stop: &OwnedHandle,
    mut handler: Handler,
) {
    let Ok(event) = new_event() else { return };
    let mut listening = Some(first);
    loop {
        let pipe = match listening.take() {
            Some(pipe) => pipe,
            None => match create_instance(name, descriptor, false) {
                Ok(pipe) => pipe,
                Err(_) => {
                    // 잠깐 뒤 다시 만든다. 그 사이 멈추라는 신호가 오면 끝낸다.
                    // SAFETY: stop은 열린 이벤트 핸들이다.
                    let woke = unsafe { WaitForMultipleObjects(&[stop.0], false, 100) };
                    if woke == WAIT_OBJECT_0 {
                        return;
                    }
                    continue;
                }
            },
        };
        match wait_connect(&pipe, &event, stop) {
            Ok(true) => {}
            Ok(false) => return,
            // 넘기는 쪽이 붙자마자 끊는 등. 이 인스턴스는 버리고 새로 만든다.
            Err(_) => continue,
        }
        // 이 연결을 다루는 동안 다음 넘기는 쪽이 붙을 수 있게 다음 인스턴스를 먼저 만든다.
        listening = create_instance(name, descriptor, false).ok();
        let Ok(io_event) = new_event() else { continue };
        let connection = Connection {
            pipe,
            event: io_event,
            stop,
            deadline: Instant::now() + CONNECTION_TIMEOUT,
        };
        let launch = connection.exchange();
        drop(connection);
        if let Some(launch) = launch {
            handler(launch);
        }
        // SAFETY: stop은 열린 이벤트 핸들이다. 기다리지 않고 상태만 본다.
        if unsafe { WaitForMultipleObjects(&[stop.0], false, 0) } == WAIT_OBJECT_0 {
            return;
        }
    }
}

/// 연 파이프의 소유자 SID 문자열.
fn pipe_owner(file: &std::fs::File) -> io::Result<String> {
    let handle = HANDLE(file.as_raw_handle());
    let mut owner = PSID::default();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: handle은 읽기 권한(READ_CONTROL 포함)으로 연 파이프이고, 받는 포인터는 쓸 수 있는 지역 변수다.
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            None,
            None,
            Some(&mut descriptor),
        )
    };
    if status.0 != 0 {
        return Err(io::Error::from_raw_os_error(status.0 as i32));
    }
    let _descriptor = Local(descriptor.0);
    // SAFETY: owner는 descriptor 안을 가리키고 descriptor는 이 함수가 끝날 때까지 살아 있다.
    unsafe { sid_string(owner) }
}

fn open_pipe(name: &str, wide: &[u16], deadline: Instant) -> Result<std::fs::File, ForwardError> {
    loop {
        let opened = OpenOptions::new()
            .read(true)
            .write(true)
            .security_qos_flags(SECURITY_IDENTIFICATION.0)
            .open(name);
        let error = match opened {
            Ok(file) => return Ok(file),
            Err(error) => error,
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        match error.raw_os_error().map(|code| code as u32) {
            Some(code) if code == ERROR_FILE_NOT_FOUND.0 => {
                if remaining.is_zero() {
                    return Err(ForwardError::NoOwner);
                }
                thread::sleep(remaining.min(RETRY_INTERVAL));
            }
            Some(code) if code == ERROR_PIPE_BUSY.0 => {
                if remaining.is_zero() {
                    return Err(ForwardError::NoOwner);
                }
                let millis = remaining.as_millis().clamp(1, 1000) as u32;
                // SAFETY: wide는 NUL로 끝나는 UTF-16 파이프 이름이다. 인스턴스가 빌 때까지 길게는 millis 기다린다.
                let _ = unsafe { WaitNamedPipeW(PCWSTR(wide.as_ptr()), millis) };
            }
            Some(code) if code == ERROR_ACCESS_DENIED.0 => return Err(ForwardError::NotOurs),
            _ => return Err(ForwardError::Io(error)),
        }
    }
}

pub(crate) fn forward(
    name: &InstanceName,
    body: &[u8],
    timeout: Duration,
) -> Result<(), ForwardError> {
    let identity = identity().map_err(ForwardError::Io)?;
    let pipe = pipe_name(name, &identity);
    let wide = to_wide(pipe.as_ref());
    let mut file = open_pipe(&pipe, &wide, Instant::now() + timeout)?;
    if pipe_owner(&file).map_err(ForwardError::Io)? != identity.user_sid {
        return Err(ForwardError::NotOurs);
    }
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(body);
    file.write_all(&frame).map_err(ForwardError::Io)?;
    let mut reply = [0u8; 1];
    match file.read(&mut reply) {
        Ok(1) if reply[0] == ACCEPTED => Ok(()),
        Ok(_) => Err(ForwardError::Rejected),
        Err(error) => Err(ForwardError::Io(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 이름은_사용자_로그온_세션_무결성_수준을_담는다() {
        let identity = identity().unwrap();
        assert!(
            identity.user_sid.starts_with("S-1-5-"),
            "{}",
            identity.user_sid
        );
        let name = InstanceName::new("facet-test").unwrap();
        let pipe = pipe_name(&name, &identity);
        assert!(pipe.starts_with(r"\\.\pipe\facet-instance-facet-test-S-1-5-"));
        assert!(pipe.ends_with(&format!(
            "-{:016x}-{:x}",
            identity.logon, identity.integrity
        )));
        assert!(mutex_name(&name, &identity).starts_with(r"Local\facet-instance-facet-test-"));
        // 다른 무결성 수준(관리자 권한)이면 이름이 다르다.
        let elevated = Identity {
            integrity: identity.integrity + 0x1000,
            user_sid: identity.user_sid.clone(),
            logon: identity.logon,
        };
        assert_ne!(pipe_name(&name, &elevated), pipe);
        assert_ne!(mutex_name(&name, &elevated), mutex_name(&name, &identity));
    }

    #[test]
    fn 보안_설명자는_같은_사용자만_허용한다() {
        let identity = identity().unwrap();
        let sddl = pipe_sddl(&identity);
        let sid = &identity.user_sid;
        assert_eq!(
            sddl,
            format!(
                "O:{sid}D:P(D;;FA;;;NU)(A;;FA;;;{sid})S:(ML;;NW;;;S-1-16-{})",
                identity.integrity
            )
        );
        assert_eq!(sddl.matches("(A;").count(), 1, "허용 항목은 사용자 하나뿐");
        security_descriptor(&sddl).unwrap();
    }
}
