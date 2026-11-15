//! 폴더 변경 감시의 Windows 부분. `unsafe`는 모두 이 파일 안에 있다.
//!
//! 폴더를 겹친 입출력(`FILE_FLAG_OVERLAPPED`)으로 열고, 감시 스레드가 `ReadDirectoryChangesW`의 완료와
//! 멈춤 이벤트를 함께 기다린다. 멈추라고 하면 걸린 읽기를 취소하고 그 완료까지 기다린 뒤 핸들을 닫는다.

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_NOTIFY_ENUM_DIR, HANDLE, WAIT_OBJECT_0, WIN32_ERROR,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE, FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_CREATION,
    FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
    FILE_NOTIFY_CHANGE_SIZE, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    ReadDirectoryChangesW,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForMultipleObjects,
};
use windows::core::PCWSTR;

use crate::watch::{DirectoryChange, parse_notifications};

/// 알림 버퍼 크기(4바이트 단위). 64KB: 네트워크 폴더가 받는 최대 크기다.
const BUFFER_WORDS: usize = 64 * 1024 / 4;

const FILTER: FILE_NOTIFY_CHANGE = FILE_NOTIFY_CHANGE(
    FILE_NOTIFY_CHANGE_FILE_NAME.0
        | FILE_NOTIFY_CHANGE_DIR_NAME.0
        | FILE_NOTIFY_CHANGE_ATTRIBUTES.0
        | FILE_NOTIFY_CHANGE_SIZE.0
        | FILE_NOTIFY_CHANGE_LAST_WRITE.0
        | FILE_NOTIFY_CHANGE_CREATION.0,
);

type Sink = Box<dyn FnMut(DirectoryChange) + Send>;

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

fn new_event() -> io::Result<OwnedHandle> {
    // SAFETY: 이름 없는 수동 재설정 이벤트를 만든다. 돌려받은 핸들은 OwnedHandle이 맡는다.
    let event = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }?;
    Ok(OwnedHandle(event))
}

/// 감시 스레드가 쓰는 상태. 겹친 읽기가 걸려 있는 동안 버퍼와 OVERLAPPED가 움직이지 않도록 상자에 둔다.
struct State {
    /// 읽기가 걸려 있는가. 걸려 있으면 핸들을 닫기 전에 취소하고 완료를 기다려야 한다.
    pending: bool,
    overlapped: OVERLAPPED,
    buffer: Box<[u32; BUFFER_WORDS]>,
    /// 읽기 완료 이벤트. `overlapped.hEvent`와 같다.
    io_event: OwnedHandle,
    directory: OwnedHandle,
}

// SAFETY: State는 만든 스레드에서 감시 스레드로 한 번 옮겨질 뿐 함께 쓰이지 않는다. OVERLAPPED 안의
// 포인터는 이 상자 안의 이벤트 핸들뿐이다.
unsafe impl Send for State {}

impl State {
    /// 다음 변경을 기다리는 읽기를 건다.
    fn issue(&mut self) -> io::Result<()> {
        // SAFETY: 이벤트 핸들은 열려 있다.
        unsafe { ResetEvent(self.io_event.0) }?;
        // SAFETY: 폴더 핸들은 겹친 입출력으로 열렸다. 버퍼와 overlapped는 상자 안에 있어 움직이지 않고,
        // 읽기가 끝나거나 취소된 완료를 기다리기 전에는 버리지 않는다(Drop 참고).
        unsafe {
            ReadDirectoryChangesW(
                self.directory.0,
                self.buffer.as_mut_ptr().cast(),
                (BUFFER_WORDS * 4) as u32,
                false,
                FILTER,
                None,
                Some(&mut self.overlapped),
                None,
            )
        }
        .map_err(|error| to_io_error(&error))?;
        self.pending = true;
        Ok(())
    }

    /// 끝난 읽기의 결과: 받은 바이트 수.
    fn complete(&mut self) -> windows::core::Result<u32> {
        let mut bytes = 0;
        // SAFETY: 이벤트가 신호를 받았으므로 읽기는 끝났다. 기다리지 않는다.
        let result =
            unsafe { GetOverlappedResult(self.directory.0, &self.overlapped, &mut bytes, false) };
        self.pending = false;
        result.map(|()| bytes)
    }
}

impl Drop for State {
    fn drop(&mut self) {
        if self.pending {
            let mut bytes = 0;
            // SAFETY: 걸린 읽기를 취소하고, 그 완료(취소 포함)를 기다려 커널이 버퍼를 더 쓰지 않게 한 뒤에
            // 버퍼와 핸들을 버린다.
            unsafe {
                let _ = CancelIoEx(self.directory.0, Some(&self.overlapped));
                let _ = GetOverlappedResult(self.directory.0, &self.overlapped, &mut bytes, true);
            }
        }
    }
}

/// 걸린 감시. 버리면 감시 스레드를 멈추고 끝날 때까지 기다린다.
pub(crate) struct Watch {
    stop: Arc<OwnedHandle>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        // SAFETY: 이벤트 핸들은 Arc가 살려 두고 있다.
        let _ = unsafe { SetEvent(self.stop.0) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// `path` 폴더의 감시를 건다. 첫 읽기까지 이 스레드에서 걸어 보아, 감시를 받지 않는 파일 시스템이면
/// 여기서 오류로 돌려준다.
pub(crate) fn watch(path: &Path, sink: Sink) -> io::Result<Watch> {
    let wide = crate::windows_impl::to_wide(path.as_os_str());
    if wide[..wide.len() - 1].contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "경로에 NUL 문자가 있습니다",
        ));
    }
    // SAFETY: wide는 NUL로 끝나는 UTF-16 경로이고 호출이 끝날 때까지 살아 있다. 폴더를 열려면
    // FILE_FLAG_BACKUP_SEMANTICS가 필요하다. 지우기를 공유해 감시 중에도 폴더를 지울 수 있게 한다.
    let directory = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
            None,
        )
    }
    .map_err(|error| to_io_error(&error))?;
    let directory = OwnedHandle(directory);
    let io_event = new_event()?;
    let stop = Arc::new(new_event()?);
    let mut state = Box::new(State {
        pending: false,
        overlapped: OVERLAPPED {
            hEvent: io_event.0,
            ..Default::default()
        },
        buffer: Box::new([0; BUFFER_WORDS]),
        io_event,
        directory,
    });
    state.issue()?;
    let thread_stop = Arc::clone(&stop);
    let thread = thread::Builder::new()
        .name("facet-watch-io".into())
        .spawn(move || run(state, &thread_stop, sink))?;
    Ok(Watch {
        stop,
        thread: Some(thread),
    })
}

fn run(mut state: Box<State>, stop: &OwnedHandle, mut sink: Sink) {
    loop {
        let handles = [state.io_event.0, stop.0];
        // SAFETY: 두 핸들 모두 이 함수가 끝날 때까지 열려 있다.
        let woke = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
        if woke != WAIT_OBJECT_0 {
            // 멈추라는 신호(또는 기다리기 실패). 걸린 읽기는 State의 Drop이 정리한다.
            return;
        }
        match state.complete() {
            Ok(0) => sink(DirectoryChange::Overflow),
            Ok(bytes) => {
                for name in parse_notifications(&state.buffer[..], bytes as usize) {
                    sink(DirectoryChange::Changed(name));
                }
            }
            Err(error) if WIN32_ERROR::from_error(&error) == Some(ERROR_NOTIFY_ENUM_DIR) => {
                sink(DirectoryChange::Overflow);
            }
            Err(error) => {
                sink(DirectoryChange::Ended(to_io_error(&error)));
                return;
            }
        }
        if let Err(error) = state.issue() {
            sink(DirectoryChange::Ended(error));
            return;
        }
    }
}

/// Win32 오류 코드가 든 오류는 그 코드로 바꾸어 [`io::ErrorKind`]가 맞게 한다.
fn to_io_error(error: &windows::core::Error) -> io::Error {
    match WIN32_ERROR::from_error(error) {
        Some(code) => io::Error::from_raw_os_error(code.0 as i32),
        None => io::Error::other(error.message()),
    }
}
