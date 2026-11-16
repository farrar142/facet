//! 클립보드 전용 STA 스레드의 Windows API 호출([`crate::clipboard`]).
//!
//! 스레드 하나가 OLE를 초기화하고 메시지 전용 창을 만들어 메시지 루프를 돈다. 다른 스레드의 요청은 채널에 넣고
//! 창에 [`WM_REQUEST`]를 올려 깨운다. 클립보드 변경 알림(`WM_CLIPBOARDUPDATE`)은 창 프로시저가 받아 [`WM_UPDATE`]를
//! 한 번만 올려 두고(여러 알림을 하나로 묶음), 루프가 그것을 받아 요약을 읽어 콜백에 넘긴다. 창 프로시저 안에서
//! 클립보드를 열거나 콜백을 부르지 않는다.

use std::cell::Cell;
use std::mem::ManuallyDrop;
use std::path::PathBuf;
use std::sync::{Once, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{
    DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL,
};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
    GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, RemoveClipboardFormatListener, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Ole::{
    CF_HDROP, CF_UNICODETEXT, OleGetClipboard, OleInitialize, OleUninitialize,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, HWND_MESSAGE,
    MSG, PostMessageW, RegisterClassW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLIPBOARDUPDATE,
    WNDCLASSW,
};
use windows::core::{PCWSTR, w};

use crate::clipboard::format::{
    self, ClipboardOperation, PASTE_SUCCEEDED, PERFORMED_DROP_EFFECT, PREFERRED_DROP_EFFECT,
};
use crate::clipboard::{
    ChangeCallback, ClipboardError, ClipboardFiles, OPEN_RETRY_INTERVAL, OPEN_TIMEOUT,
};
use crate::windows_impl::to_wide;

/// 다른 스레드가 요청을 넣었다.
const WM_REQUEST: u32 = WM_APP + 1;
/// 클립보드가 바뀌었다(창 프로시저가 올림).
const WM_UPDATE: u32 = WM_APP + 2;
/// 스레드를 멈춘다.
const WM_STOP: u32 = WM_APP + 3;

const CLASS_NAME: PCWSTR = w!("facet-clipboard");

type Request = Box<dyn FnOnce(&State) + Send + 'static>;

thread_local! {
    /// 이 스레드에 [`WM_UPDATE`]를 올려 두었고 아직 받지 않았다.
    static UPDATE_POSTED: Cell<bool> = const { Cell::new(false) };
}

fn failed(error: windows::core::Error) -> ClipboardError {
    ClipboardError::Failed {
        code: error.code().0,
    }
}

/// 다른 스레드에서 클립보드 스레드를 부르는 손잡이.
pub(crate) struct Worker {
    requests: mpsc::Sender<Request>,
    /// 메시지 전용 창 핸들 값(`HWND`는 `Send`가 아니어서 값으로 든다).
    window: isize,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    pub(crate) fn start(on_change: ChangeCallback) -> Result<Self, ClipboardError> {
        let (requests, receiver) = mpsc::channel::<Request>();
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("facet-clipboard".into())
            .spawn(move || run(on_change, receiver, ready_tx))
            .map_err(|_| ClipboardError::Stopped)?;
        match ready_rx.recv() {
            Ok(Ok(window)) => Ok(Self {
                requests,
                window,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(ClipboardError::Stopped)
            }
        }
    }

    fn post(&self, message: u32) -> Result<(), ClipboardError> {
        let hwnd = HWND(self.window as *mut _);
        // SAFETY: 핸들은 이 손잡이가 사는 동안 살아 있는 클립보드 스레드의 창이다(창이 이미 없어도 호출은 실패만
        // 한다). 매개변수는 쓰지 않는 0이다.
        unsafe { PostMessageW(Some(hwnd), message, WPARAM(0), LPARAM(0)) }
            .map_err(|_| ClipboardError::Stopped)
    }

    /// `op`을 클립보드 스레드에서 실행하고 답을 기다린다.
    fn call<T: Send + 'static>(
        &self,
        op: impl FnOnce(&State) -> Result<T, ClipboardError> + Send + 'static,
    ) -> Result<T, ClipboardError> {
        let (tx, rx) = mpsc::channel();
        self.requests
            .send(Box::new(move |state| {
                let _ = tx.send(op(state));
            }))
            .map_err(|_| ClipboardError::Stopped)?;
        self.post(WM_REQUEST)?;
        rx.recv().map_err(|_| ClipboardError::Stopped)?
    }

    pub(crate) fn set_files(
        &self,
        paths: &[PathBuf],
        operation: ClipboardOperation,
    ) -> Result<(), ClipboardError> {
        let hdrop = format::hdrop_bytes(paths);
        let effect = format::preferred_drop_effect_bytes(operation);
        self.call(move |state| {
            state.write(&[(u32::from(CF_HDROP.0), &hdrop), (state.preferred, &effect)])
        })
    }

    pub(crate) fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        let bytes: Vec<u8> = text
            .encode_utf16()
            .chain(std::iter::once(0))
            .flat_map(u16::to_le_bytes)
            .collect();
        self.call(move |state| state.write(&[(u32::from(CF_UNICODETEXT.0), &bytes)]))
    }

    pub(crate) fn files(&self) -> Result<Option<ClipboardFiles>, ClipboardError> {
        self.call(State::files)
    }

    pub(crate) fn text(&self) -> Result<Option<String>, ClipboardError> {
        self.call(State::text)
    }

    pub(crate) fn finish_cut_paste(&self, sequence: u32) -> Result<bool, ClipboardError> {
        self.call(move |state| state.finish_cut_paste(sequence))
    }

    pub(crate) fn clear(&self) -> Result<(), ClipboardError> {
        self.call(|state| state.write(&[]))
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if self.post(WM_STOP).is_ok()
            && let Some(thread) = self.thread.take()
        {
            let _ = thread.join();
        }
    }
}

/// 클립보드 스레드 안에서만 쓰는 상태.
struct State {
    hwnd: HWND,
    preferred: u32,
    performed: u32,
    succeeded: u32,
}

fn register_format(name: &str) -> u32 {
    let wide = to_wide(std::ffi::OsStr::new(name));
    // SAFETY: wide는 NUL로 끝나는 UTF-16 이름이고 호출 동안 살아 있다.
    unsafe { RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) }
}

/// 열어 둔 클립보드. 버리면 닫는다.
struct Opened;

impl Drop for Opened {
    fn drop(&mut self) {
        // SAFETY: 이 스레드가 연 클립보드를 닫는다.
        let _ = unsafe { CloseClipboard() };
    }
}

impl State {
    /// 클립보드를 연다. 다른 프로세스가 잡고 있으면 시간 제한까지 다시 열어 본다.
    fn open(&self) -> Result<Opened, ClipboardError> {
        let deadline = Instant::now() + OPEN_TIMEOUT;
        loop {
            // SAFETY: self.hwnd는 이 스레드의 살아 있는 창이다.
            if unsafe { OpenClipboard(Some(self.hwnd)) }.is_ok() {
                return Ok(Opened);
            }
            if Instant::now() >= deadline {
                return Err(ClipboardError::Busy);
            }
            thread::sleep(OPEN_RETRY_INTERVAL);
        }
    }

    /// 클립보드를 비우고 `items`(형식, 바이트)를 올린다. 비어 있으면 비우기만 한다.
    fn write(&self, items: &[(u32, &[u8])]) -> Result<(), ClipboardError> {
        let _opened = self.open()?;
        // SAFETY: 클립보드를 이 스레드의 창으로 열었다.
        unsafe { EmptyClipboard() }.map_err(failed)?;
        for &(format, bytes) in items {
            let memory = global_from(bytes)?;
            // SAFETY: 클립보드를 열었고 memory는 GMEM_MOVEABLE로 잡아 다 쓴 메모리다. 성공하면 시스템이 소유한다.
            if let Err(error) = unsafe { SetClipboardData(format, Some(HANDLE(memory.0))) } {
                // SAFETY: 실패하면 소유권이 넘어가지 않았으니 잡은 쪽이 놓는다.
                let _ = unsafe { GlobalFree(Some(memory)) };
                return Err(failed(error));
            }
        }
        Ok(())
    }

    fn files(&self) -> Result<Option<ClipboardFiles>, ClipboardError> {
        let _opened = self.open()?;
        // SAFETY: 인자 없는 호출이다.
        let sequence = unsafe { GetClipboardSequenceNumber() };
        let Some(bytes) = clipboard_bytes(u32::from(CF_HDROP.0)) else {
            return Ok(None);
        };
        let paths = match format::parse_hdrop(&bytes) {
            Ok(paths) if !paths.is_empty() => paths,
            _ => return Ok(None),
        };
        let operation = clipboard_bytes(self.preferred)
            .and_then(|bytes| format::parse_preferred_drop_effect(&bytes))
            .unwrap_or(ClipboardOperation::Copy);
        Ok(Some(ClipboardFiles {
            paths,
            operation,
            sequence,
        }))
    }

    fn text(&self) -> Result<Option<String>, ClipboardError> {
        let _opened = self.open()?;
        let Some(bytes) = clipboard_bytes(u32::from(CF_UNICODETEXT.0)) else {
            return Ok(None);
        };
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .take_while(|&unit| unit != 0)
            .collect();
        Ok(Some(String::from_utf16_lossy(&units)))
    }

    fn finish_cut_paste(&self, sequence: u32) -> Result<bool, ClipboardError> {
        // SAFETY: 인자 없는 호출이다.
        if unsafe { GetClipboardSequenceNumber() } != sequence {
            return Ok(false);
        }
        // 원본 앱의 데이터 개체에 알린다. 받지 않는 개체(직접 올린 데이터 등)도 있으므로 실패는 무시한다.
        // SAFETY: 이 스레드는 OleInitialize로 OLE를 초기화했다.
        if let Ok(object) = unsafe { OleGetClipboard() } {
            let effect = format::performed_drop_effect_bytes(ClipboardOperation::Move);
            let _ = set_data(&object, self.performed, &effect);
            let succeeded = format::paste_succeeded_bytes(ClipboardOperation::Move);
            let _ = set_data(&object, self.succeeded, &succeeded);
        }
        self.write(&[])?;
        Ok(true)
    }
}

/// `bytes`를 담은 이동 가능한 전역 메모리(빈 바이트도 1바이트를 잡는다).
fn global_from(bytes: &[u8]) -> Result<HGLOBAL, ClipboardError> {
    // SAFETY: 크기만 넘긴다.
    let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)) }.map_err(failed)?;
    // SAFETY: memory는 방금 잡은 핸들이다.
    let pointer = unsafe { GlobalLock(memory) }.cast::<u8>();
    if pointer.is_null() {
        // SAFETY: 잡은 메모리를 놓는다.
        let _ = unsafe { GlobalFree(Some(memory)) };
        return Err(failed(windows::core::Error::from_thread()));
    }
    // SAFETY: pointer는 bytes.len() 이상 크기의 잠근 메모리이고 bytes와 겹치지 않는다.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len()) };
    // SAFETY: 위에서 잠근 것을 푼다. 잠금 수가 0이 되면 FALSE이므로 결과는 보지 않는다.
    let _ = unsafe { GlobalUnlock(memory) };
    Ok(memory)
}

/// 열어 둔 클립보드에서 `format`의 바이트. 없거나 읽지 못하면 `None`.
fn clipboard_bytes(format: u32) -> Option<Vec<u8>> {
    if format == 0 {
        return None;
    }
    // SAFETY: 클립보드는 부르는 쪽이 열었다.
    unsafe { IsClipboardFormatAvailable(format) }.ok()?;
    // SAFETY: 클립보드는 부르는 쪽이 열었다. 돌려받은 핸들은 클립보드가 소유하고 닫기 전까지만 쓴다.
    let handle = unsafe { GetClipboardData(format) }.ok()?;
    let memory = HGLOBAL(handle.0);
    // SAFETY: memory는 클립보드가 준 전역 메모리 핸들이다.
    let size = unsafe { GlobalSize(memory) };
    // SAFETY: 같은 핸들을 잠근다.
    let pointer = unsafe { GlobalLock(memory) }.cast::<u8>();
    if pointer.is_null() {
        return None;
    }
    // SAFETY: pointer는 size바이트의 잠근 메모리이고 잠근 동안만 읽는다.
    let bytes = unsafe { std::slice::from_raw_parts(pointer, size) }.to_vec();
    // SAFETY: 위에서 잠근 것을 푼다.
    let _ = unsafe { GlobalUnlock(memory) };
    Some(bytes)
}

/// 데이터 개체에 `format`의 바이트를 넘긴다(넘기면 개체가 메모리를 소유).
fn set_data(object: &IDataObject, format: u32, bytes: &[u8]) -> Result<(), ClipboardError> {
    let memory = global_from(bytes)?;
    let format_etc = FORMATETC {
        cfFormat: format as u16,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let medium = STGMEDIUM {
        tymed: TYMED_HGLOBAL.0 as u32,
        u: STGMEDIUM_0 { hGlobal: memory },
        pUnkForRelease: ManuallyDrop::new(None),
    };
    // SAFETY: object는 살아 있는 데이터 개체이고 두 구조체는 호출 동안 산다. fRelease=true라 성공하면 개체가
    // 메모리를 놓는다.
    let result = unsafe { object.SetData(&format_etc, &medium, true) };
    if let Err(error) = result {
        // SAFETY: 실패하면 소유권이 넘어가지 않았으니 잡은 쪽이 놓는다.
        let _ = unsafe { GlobalFree(Some(memory)) };
        return Err(failed(error));
    }
    Ok(())
}

/// 창 프로시저: 클립보드 변경 알림을 루프에 넘긴다(아직 넘기지 않았을 때만).
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_CLIPBOARDUPDATE {
        if !UPDATE_POSTED.replace(true) {
            // SAFETY: hwnd는 지금 메시지를 받는 이 스레드의 창이다.
            let _ = unsafe { PostMessageW(Some(hwnd), WM_UPDATE, WPARAM(0), LPARAM(0)) };
        }
        return LRESULT(0);
    }
    // SAFETY: 받은 인자를 그대로 기본 처리에 넘긴다.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

/// 메시지 전용 창을 만들고 변경 알림을 건다.
fn create_window() -> Result<HWND, ClipboardError> {
    // SAFETY: 이 프로세스 실행 파일의 모듈 핸들을 묻는다.
    let instance = unsafe { GetModuleHandleW(PCWSTR::null()) }.map_err(failed)?;
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        // SAFETY: class는 호출 동안 살아 있고 이름은 정적 문자열이다. 실패하면 아래 창 만들기가 실패한다.
        unsafe { RegisterClassW(&class) };
    });
    // SAFETY: 위에서 등록한 클래스로 부모가 HWND_MESSAGE인 메시지 전용 창을 만든다. 제목·메뉴·매개변수는 없다.
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            PCWSTR::null(),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            None,
        )
    }
    .map_err(failed)?;
    // SAFETY: hwnd는 방금 만든 이 스레드의 창이다.
    if let Err(error) = unsafe { AddClipboardFormatListener(hwnd) } {
        // SAFETY: 만든 창을 닫는다.
        let _ = unsafe { DestroyWindow(hwnd) };
        return Err(failed(error));
    }
    Ok(hwnd)
}

/// 클립보드 스레드 본체.
fn run(
    on_change: ChangeCallback,
    requests: mpsc::Receiver<Request>,
    ready: mpsc::Sender<Result<isize, ClipboardError>>,
) {
    // SAFETY: 이 스레드에서 처음 OLE를 초기화한다(STA). 성공하면 끝에서 OleUninitialize로 짝을 맞춘다.
    if let Err(error) = unsafe { OleInitialize(None) } {
        let _ = ready.send(Err(failed(error)));
        return;
    }
    match create_window() {
        Ok(hwnd) => {
            let state = State {
                hwnd,
                preferred: register_format(PREFERRED_DROP_EFFECT),
                performed: register_format(PERFORMED_DROP_EFFECT),
                succeeded: register_format(PASTE_SUCCEEDED),
            };
            let _ = ready.send(Ok(hwnd.0 as isize));
            // 시작할 때도 한 번 알린다.
            UPDATE_POSTED.set(true);
            // SAFETY: hwnd는 이 스레드의 창이다.
            let _ = unsafe { PostMessageW(Some(hwnd), WM_UPDATE, WPARAM(0), LPARAM(0)) };
            pump(&state, &on_change, &requests);
            // SAFETY: 위에서 건 알림을 풀고 이 스레드의 창을 닫는다.
            unsafe {
                let _ = RemoveClipboardFormatListener(hwnd);
                let _ = DestroyWindow(hwnd);
            }
        }
        Err(error) => {
            let _ = ready.send(Err(error));
        }
    }
    // SAFETY: 위의 OleInitialize와 짝을 이룬다. OLE 객체는 모두 버렸다.
    unsafe { OleUninitialize() };
}

/// 메시지 루프. [`WM_STOP`]이나 `WM_QUIT`이 오면 돌아온다.
fn pump(state: &State, on_change: &ChangeCallback, requests: &mpsc::Receiver<Request>) {
    let mut message = MSG::default();
    loop {
        // SAFETY: message는 이 스레드의 지역 변수이고, 이 스레드의 모든 창 메시지를 받는다.
        let got = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if got.0 <= 0 {
            return;
        }
        if message.hwnd == state.hwnd {
            match message.message {
                WM_REQUEST => {
                    while let Ok(request) = requests.try_recv() {
                        request(state);
                    }
                    continue;
                }
                WM_UPDATE => {
                    UPDATE_POSTED.set(false);
                    // 다른 프로세스가 오래 잡고 있어 읽지 못하면 이번 알림은 건너뛴다(다음 변경 때 다시 알린다).
                    if let Ok(summary) = state.files() {
                        on_change(summary);
                    }
                    continue;
                }
                WM_STOP => return,
                _ => {}
            }
        }
        // SAFETY: GetMessageW가 채운 메시지를 그 창 프로시저로 보낸다.
        unsafe { DispatchMessageW(&message) };
    }
}
