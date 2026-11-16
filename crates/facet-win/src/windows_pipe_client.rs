//! 파이프 클라이언트의 Windows 부분([`crate::pipe_client`]). `unsafe`는 모두 이 파일 안에 있다.
//!
//! 파이프는 std의 파일 열기에 `FILE_FLAG_OVERLAPPED`를 주어 연다. 읽는 쪽과 쓰는 쪽은 같은 핸들을 나눠 갖고
//! 저마다 이벤트를 가진다(겹친 입출력은 입출력마다 완료 이벤트가 따로 있어야 한다). 닫개는 수동 재설정 이벤트
//! 하나를 켠다. 읽기·쓰기는 완료 이벤트와 그 이벤트를 함께 기다리므로, 닫은 뒤에 시작한 입출력도 곧바로 끝난다.

use std::fs::OpenOptions;
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_PIPE_BUSY,
    HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT, WIN32_ERROR,
};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_OVERLAPPED, ReadFile, SECURITY_IDENTIFICATION, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::WaitNamedPipeW;
use windows::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects};
use windows::core::PCWSTR;

use crate::pipe_client::ConnectError;
use crate::windows_impl::to_wide;

/// 바쁜 파이프를 다시 열어 볼 때 한 번에 기다리는 가장 긴 시간.
const BUSY_WAIT: Duration = Duration::from_millis(50);

/// 이벤트 핸들. 버리면 닫는다.
struct Event(HANDLE);

// SAFETY: 커널 이벤트 핸들은 어느 스레드에서 써도 된다. 닫기는 Drop에서 한 번만 한다.
unsafe impl Send for Event {}
// SAFETY: 나눠 쓰는 것은 SetEvent·WaitForMultipleObjects처럼 스레드 안전한 호출뿐이다.
unsafe impl Sync for Event {}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: 이 값이 맡은 열린 핸들이고 여기서 한 번만 닫는다.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn event() -> io::Result<Event> {
    // SAFETY: 이름 없는 수동 재설정 이벤트를 꺼진 채로 만든다. 핸들은 Event가 맡는다.
    let h =
        unsafe { CreateEventW(None, true, false, PCWSTR::null()) }.map_err(|e| to_io_error(&e))?;
    Ok(Event(h))
}

fn to_io_error(error: &windows::core::Error) -> io::Error {
    match WIN32_ERROR::from_error(error) {
        Some(code) => io::Error::from_raw_os_error(code.0 as i32),
        None => io::Error::other(error.message()),
    }
}

pub struct Client(OwnedHandle);

pub struct Reader {
    io: Io,
    deadline: Option<Instant>,
}

pub struct Writer(Io);

#[derive(Clone)]
pub struct Closer(Arc<Event>);

/// 읽는 쪽 또는 쓰는 쪽: 나눠 가진 파이프 핸들, 자기 완료 이벤트, 닫기 이벤트.
struct Io {
    pipe: Arc<OwnedHandle>,
    done: Event,
    stop: Arc<Event>,
}

pub fn connect(name: &str, access: u32, deadline: Instant) -> Result<Client, ConnectError> {
    let wide = to_wide(name.as_ref());
    loop {
        let opened = OpenOptions::new()
            .access_mode(access)
            .custom_flags(FILE_FLAG_OVERLAPPED.0)
            .security_qos_flags(SECURITY_IDENTIFICATION.0)
            .open(name);
        let error = match opened {
            Ok(file) => return Ok(Client(OwnedHandle::from(file))),
            Err(error) => error,
        };
        match error.raw_os_error().map(|c| c as u32) {
            Some(code) if code == ERROR_FILE_NOT_FOUND.0 => return Err(ConnectError::NotFound),
            Some(code) if code == ERROR_PIPE_BUSY.0 => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(ConnectError::Busy);
                }
                let millis = remaining.min(BUSY_WAIT).as_millis().max(1) as u32;
                // SAFETY: wide는 NUL로 끝나는 UTF-16 파이프 이름이다. 인스턴스가 빌 때까지 길게는 millis 기다린다.
                let waited = unsafe { WaitNamedPipeW(PCWSTR(wide.as_ptr()), millis) };
                if !waited.as_bool() {
                    // 그 사이 파이프가 사라졌거나 시간이 다 됐다. 다시 열어 보며 가린다.
                    thread::yield_now();
                }
            }
            _ => return Err(ConnectError::Other(error)),
        }
    }
}

pub fn split(client: Client) -> io::Result<(Reader, Writer, Closer)> {
    let pipe = Arc::new(client.0);
    let stop = Arc::new(event()?);
    let reader = Reader {
        io: Io {
            pipe: Arc::clone(&pipe),
            done: event()?,
            stop: Arc::clone(&stop),
        },
        deadline: None,
    };
    let writer = Writer(Io {
        pipe,
        done: event()?,
        stop: Arc::clone(&stop),
    });
    Ok((reader, writer, Closer(stop)))
}

pub fn set_deadline(reader: &mut Reader, deadline: Option<Instant>) {
    reader.deadline = deadline;
}

fn aborted() -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, "파이프를 닫았습니다")
}

impl Io {
    fn handle(&self) -> HANDLE {
        HANDLE(self.pipe.as_raw_handle())
    }

    /// 시작한 겹친 입출력 하나를 기다린다. 닫기 신호나 마감이 먼저 오면 그 입출력을 취소하고 완료를 기다린 뒤
    /// 오류다(커널이 버퍼를 더 쓰지 않게). 취소 직전에 끝난 입출력은 그 결과를 돌려준다(읽은 데이터를 잃지 않게).
    fn finish(&self, ov: &OVERLAPPED, deadline: Option<Instant>) -> io::Result<u32> {
        let millis = match deadline {
            Some(d) => d
                .saturating_duration_since(Instant::now())
                .as_millis()
                .min(u128::from(INFINITE - 1)) as u32,
            None => INFINITE,
        };
        let handles = [self.done.0, self.stop.0];
        // SAFETY: 두 이벤트 핸들은 이 함수가 끝날 때까지 열려 있다.
        let woke = unsafe { WaitForMultipleObjects(&handles, false, millis) };
        let mut bytes = 0u32;
        if woke == WAIT_OBJECT_0 {
            // SAFETY: 완료 이벤트가 켜졌으므로 ov의 입출력은 끝났다.
            return match unsafe { GetOverlappedResult(self.handle(), ov, &mut bytes, false) } {
                Ok(()) => Ok(bytes),
                Err(e) => Err(to_io_error(&e)),
            };
        }
        // SAFETY: ov로 건 입출력을 취소하고, 그 완료를 기다려 ov와 버퍼를 버리기 전에 커널이 손을 떼게 한다.
        let result = unsafe {
            let _ = CancelIoEx(self.handle(), Some(ov));
            GetOverlappedResult(self.handle(), ov, &mut bytes, true)
        };
        if woke == WAIT_TIMEOUT {
            if result.is_ok() && bytes > 0 {
                return Ok(bytes);
            }
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "파이프 읽기가 마감 시각을 넘었습니다",
            ));
        }
        Err(aborted())
    }
}

fn is_broken(code: Option<WIN32_ERROR>) -> bool {
    code == Some(ERROR_BROKEN_PIPE)
}

pub fn read(reader: &mut Reader, buf: &mut [u8]) -> io::Result<usize> {
    if buf.is_empty() {
        return Ok(0);
    }
    let io = &reader.io;
    let mut ov = OVERLAPPED {
        hEvent: io.done.0,
        ..Default::default()
    };
    let len = buf.len().min(u32::MAX as usize);
    // SAFETY: 파이프는 겹친 입출력으로 열렸다. buf와 ov는 finish가 완료(또는 취소 완료)를 기다릴 때까지 산다.
    let started = unsafe { ReadFile(io.handle(), Some(&mut buf[..len]), None, Some(&raw mut ov)) };
    if let Err(error) = started {
        let code = WIN32_ERROR::from_error(&error);
        if is_broken(code) {
            return Ok(0);
        }
        if code != Some(ERROR_IO_PENDING) {
            return Err(to_io_error(&error));
        }
    }
    match io.finish(&ov, reader.deadline) {
        Ok(n) => Ok(n as usize),
        Err(e) if e.raw_os_error() == Some(ERROR_BROKEN_PIPE.0 as i32) => Ok(0),
        Err(e) => Err(e),
    }
}

pub fn write(writer: &mut Writer, buf: &[u8]) -> io::Result<usize> {
    if buf.is_empty() {
        return Ok(0);
    }
    let io = &writer.0;
    let mut ov = OVERLAPPED {
        hEvent: io.done.0,
        ..Default::default()
    };
    let len = buf.len().min(u32::MAX as usize);
    // SAFETY: 파이프는 겹친 입출력으로 열렸다. buf와 ov는 finish가 완료를 기다릴 때까지 산다.
    let started = unsafe { WriteFile(io.handle(), Some(&buf[..len]), None, Some(&raw mut ov)) };
    if let Err(error) = started
        && WIN32_ERROR::from_error(&error) != Some(ERROR_IO_PENDING)
    {
        return Err(to_io_error(&error));
    }
    let written = io.finish(&ov, None)?;
    if written == 0 {
        return Err(io::ErrorKind::WriteZero.into());
    }
    Ok(written as usize)
}

pub fn close(closer: &Closer) {
    // SAFETY: 이벤트 핸들은 Arc가 살려 두고 있다.
    let _ = unsafe { SetEvent(closer.0.0) };
}

/// 시험 도우미의 Windows 부분([`crate::pipe_client::testing`]).
pub mod testing {
    use std::io;
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use std::sync::Mutex;
    use std::time::Duration;

    use windows::Win32::Foundation::{ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, WIN32_ERROR};
    use windows::Win32::Storage::FileSystem::{FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX};
    use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };
    use windows::Win32::System::Threading::WaitForSingleObject;
    use windows::core::PCWSTR;

    use super::{Client, event, to_io_error};
    use crate::windows_impl::to_wide;
    use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};

    pub struct Listener {
        name: Vec<u16>,
        next: Mutex<Option<OwnedHandle>>,
    }

    fn instance(name: &[u16]) -> io::Result<OwnedHandle> {
        // SAFETY: name은 NUL로 끝나는 UTF-16 문자열이다. 보안 속성은 기본값(만든 사용자)이다.
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024,
                64 * 1024,
                0,
                None,
            )
        };
        if handle.is_invalid() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: 방금 만든 열린 핸들이고 OwnedHandle이 맡아 한 번만 닫는다.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle.0) })
    }

    pub fn bind(name: &str) -> io::Result<Listener> {
        let name = to_wide(name.as_ref());
        let first = instance(&name)?;
        Ok(Listener {
            name,
            next: Mutex::new(Some(first)),
        })
    }

    pub fn accept(listener: &Listener, timeout: Duration) -> io::Result<Option<Client>> {
        let mut slot = listener
            .next
            .lock()
            .map_err(|_| io::Error::other("잠금이 깨졌습니다"))?;
        let pipe = match slot.take() {
            Some(p) => p,
            None => instance(&listener.name)?,
        };
        let raw = HANDLE(std::os::windows::io::AsRawHandle::as_raw_handle(&pipe));
        let done = event()?;
        let mut ov = OVERLAPPED {
            hEvent: done.0,
            ..Default::default()
        };
        // SAFETY: pipe는 겹친 입출력으로 만든 서버 인스턴스이고 ov는 완료(또는 취소 완료)를 기다릴 때까지 산다.
        let connected = match unsafe { ConnectNamedPipe(raw, Some(&mut ov)) } {
            Ok(()) => true,
            Err(error) => match WIN32_ERROR::from_error(&error) {
                Some(ERROR_PIPE_CONNECTED) => true,
                Some(ERROR_IO_PENDING) => {
                    let millis = timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32;
                    // SAFETY: done은 열린 이벤트 핸들이다.
                    let woke = unsafe { WaitForSingleObject(done.0, millis) };
                    let mut ignored = 0u32;
                    if woke == WAIT_OBJECT_0 {
                        // SAFETY: 이벤트가 켜졌으므로 연결 기다리기는 끝났다.
                        unsafe { GetOverlappedResult(raw, &ov, &mut ignored, false) }
                            .map_err(|e| to_io_error(&e))?;
                        true
                    } else {
                        // SAFETY: ov로 건 연결 기다리기를 취소하고 끝나기를 기다린다.
                        unsafe {
                            let _ = CancelIoEx(raw, Some(&ov));
                            let _ = GetOverlappedResult(raw, &ov, &mut ignored, true);
                        }
                        false
                    }
                }
                _ => return Err(to_io_error(&error)),
            },
        };
        if !connected {
            *slot = Some(pipe);
            return Ok(None);
        }
        // 다음 클라이언트가 「파이프 없음」을 보지 않게 곧바로 다음 인스턴스를 만들어 둔다.
        *slot = Some(instance(&listener.name)?);
        Ok(Some(Client(pipe)))
    }
}
