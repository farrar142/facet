//! facet의 Windows API 안전 래퍼.
//!
//! 셸 호출은 전부 `unsafe`다(ADR-0002). 작업 공간에서 이 크레이트만 `unsafe_code`를 허용하고,
//! 밖에는 안전한 함수만 드러낸다. Windows 전용 부분은 조건부로 두어 다른 환경에서도 작업 공간이 빌드된다.
//!
//! - [`compare_names_like_explorer`]: 탐색기의 이름 비교(`StrCmpLogicalW`). Windows에서만 있다.
//! - [`ShellWorker`]: COM을 초기화한 전용 일꾼 스레드에서 셸을 부른다. 기본 프로그램으로 열기를 한다.
//! - [`DirectoryWatcher`]: 폴더 하나의 변경을 감시 스레드에서 알린다(`ReadDirectoryChangesW`).
//! - [`top_level_window_at`]: 화면 점 아래 보이는 최상위 창(`WindowFromPoint`). 탭을 창 사이로 끌 때 쓴다.
//!   Windows에서만 있다.
//! - [`monitors`]: 붙어 있는 모니터마다 작업 영역과 주 모니터 여부(`EnumDisplayMonitors`). 세션의 창 자리를 보이는
//!   곳으로 고칠 때 쓴다. Windows가 아니면 빈 목록이다.
//! - [`drive_letters`]: 지금 있는 드라이브 문자들(`GetLogicalDrives`, 문자 순서). 주소창 맨 앞 › 목록이 쓴다.
//!   Windows가 아니면 빈 목록이다.
//! - [`devices`]: 드라이브 용량([`devices::disk_space`], 이 사용자가 쓸 수 있는 남은 공간)과 볼륨·휴대용 장치의
//!   붙임·뗌 알림([`devices::watch_devices`]). 내 PC가 쓴다. Windows가 아니면 용량은 `None`, 알림은 「지원하지 않음」이다.
//! - [`single_instance`]: 단일 실행. 주인 되기([`single_instance::claim`], 사용자 로그온 세션 범위 뮤텍스),
//!   받기([`single_instance::Owner::listen`], 같은 사용자만 여는 파이프의 수신기 스레드),
//!   넘기기([`single_instance::forward`], 인자와 작업 폴더). Windows가 아니면 늘 주인이 된다.
//! - [`file_ops`]: 복사·이동 엔진의 파일 하나·이름 하나 단위 연산. 파일 복사(`CopyFile2`, 진행·멈춤·취소, 시각·속성·
//!   대체 데이터 스트림 보존), 폴더 만들기·시각·속성 맞추기, 같은 볼륨 이름 바꾸기, 바꿔치기, 지우기, 파일 ID, 오류
//!   종류. Windows가 아니면 「지원하지 않음」이다.
//! - [`recycle`]: 휴지통으로 보내기와 휴지통에서 복원하기(`IFileOperation`, 부를 때마다 띄우는 전용 STA 스레드, 시간
//!   제한). 셸이 휴지통 대신 영구 삭제하려는 항목은 지우지 않고 멈춘다. 볼륨에 휴지통이 있는지 미리 묻기. Windows가
//!   아니면 「지원하지 않음」이다.
//! - [`clipboard`]: 클립보드를 소유하는 전용 STA 스레드([`clipboard::Clipboard`]: 파일 목록·글자 올리기와 읽기, 잘라내기
//!   붙여넣기 뒤 알림과 비우기, 변경 알림 콜백)와 탐색기와 같은 형식([`clipboard::format`]). 메서드는 그 스레드를
//!   기다리므로 UI 스레드에서 부르지 않는다. Windows가 아니면 「지원하지 않음」이다.
//! - [`drop_target`]: 창 하나를 OLE 드롭 대상으로 등록해(`RegisterDragDrop`) 다른 앱에서 끌어 온 파일 목록(`CF_HDROP`,
//!   [`clipboard::format`]으로 읽음)·원본이 허용한 효과·키를 처리기에 넘기고 처리기가 고른 효과를 돌려준다. 이동은 셸의
//!   최적화된 이동 규약을 따른다. 창을 만든 UI 스레드(STA)에서만 등록한다. Windows가 아니면 「지원하지 않음」이다.
//! - [`pipe_client`]: 이름 있는 파이프의 클라이언트 쪽(겹친 입출력). 읽는 쪽·쓰는 쪽을 다른 스레드에서 동시에 쓰고, 읽기에
//!   마감 시각을 두고, 닫개로 기다리던 읽기를 깨운다. 코어의 인덱서 서비스 연결이 쓴다. Windows가 아니면 붙지 못한다.
//! - [`icon`]: 파일 종류별 시스템 아이콘(확장자·폴더·파일 하나의 고유 아이콘)을 요청 크기의 RGBA로 얻는다(시스템 아이콘
//!   목록). 부른 스레드에서 셸을 기다리므로 UI 스레드에서 부르지 않는다. Windows가 아니면 `None`이다.
//! - [`process`]: 다른 프로세스가 끝나기를 기다린다(셸 확장 도우미의 부모 감시). Windows가 아니면 「지원하지 않음」이다.
//! - [`shell_thread`]: 셸 호출용 전용 COM STA 스레드([`shell_thread::ShellThread`], 처음 요청 때 띄움, 시간 제한을 넘으면
//!   그 스레드를 버리고 다음 요청은 새 스레드에서, 패닉은 그 요청만 실패). Windows가 아니면 COM 없는 보통 스레드다.
//! - [`shell_folder`]: 셸 폴더 하나(PIDL 바이트로 가리킴)의 표시 이름과 항목 열거(표시 이름·실제 경로·폴더 여부·PIDL).
//!   COM STA 스레드([`shell_thread`])에서만 부른다. 셸 공급자(내 PC)가 쓴다. Windows가 아니면 「지원하지 않음」이다.
//! - [`shell_transfer`]: 셸 항목(휴대폰 등 실제 경로가 없는 항목)과 PC 사이의 파일 전송(스트림으로 읽어 파일에 쓰기,
//!   `IFileOperation`으로 넣기·폴더 만들기·영구 삭제). COM STA 스레드에서만 부른다. Windows가 아니면 「지원하지 않음」이다.
//! - [`shell_dialog`]: Windows 속성 대화상자와 「연결 프로그램」 대화상자(셸 확장 도우미만 부름, 닫힐 때까지 기다림)와
//!   포그라운드 권한 넘기기. Windows가 아니면 「지원하지 않음」이다.
//! - [`shell_copy`]: **측정 전용** 셸 복사(`IFileOperation`, 전용 스레드의 COM, 대화 없음). 탐색기 비교 측정만 부르고
//!   앱은 부르지 않는다. Windows가 아니면 「지원하지 않음」이다.

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

pub mod clipboard;
pub mod devices;
pub mod drag_source;
pub mod drop_target;
pub mod file_ops;
pub mod icon;
pub mod pipe_client;
pub mod preview;
pub mod process;
pub mod recycle;
pub mod shell_copy;
pub mod shell_dialog;
pub mod shell_folder;
pub mod shell_thread;
pub mod shell_transfer;
pub mod single_instance;
mod watch;
#[cfg(windows)]
mod windows_clipboard;
#[cfg(windows)]
mod windows_devices;
#[cfg(windows)]
mod windows_drag_source;
#[cfg(windows)]
mod windows_drop_target;
#[cfg(windows)]
mod windows_file_ops;
#[cfg(windows)]
mod windows_icon;
#[cfg(windows)]
mod windows_impl;
#[cfg(windows)]
mod windows_instance;
#[cfg(windows)]
mod windows_pipe_client;
#[cfg(windows)]
mod windows_recycle;
#[cfg(windows)]
mod windows_screen;
#[cfg(windows)]
mod windows_shell_copy;
#[cfg(windows)]
mod windows_shell_folder;
#[cfg(windows)]
mod windows_shell_thread;
#[cfg(windows)]
mod windows_shell_transfer;
#[cfg(windows)]
mod windows_watch;

pub use watch::{DirectoryChange, DirectoryWatcher};

#[cfg(windows)]
pub use windows_impl::compare_names_like_explorer;
#[cfg(windows)]
pub use windows_screen::top_level_window_at;

/// 모니터 하나: 작업 영역(작업 표시줄을 뺀 영역)과 주 모니터 여부([`monitors`]).
///
/// 좌표는 가상 화면 기준 물리 픽셀이다(주 모니터 왼쪽 위가 0,0, 다른 모니터도 같은 좌표계). 앱 프로세스는 winit이
/// 모니터마다 DPI 인식(Per-Monitor v2)으로 켜므로 배율과 상관없이 실제 픽셀이다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorInfo {
    /// 작업 영역의 왼쪽.
    pub left: i32,
    /// 작업 영역의 위.
    pub top: i32,
    /// 작업 영역의 오른쪽(이 값은 영역 밖).
    pub right: i32,
    /// 작업 영역의 아래(이 값은 영역 밖).
    pub bottom: i32,
    /// 주 모니터인가.
    pub primary: bool,
}

/// 지금 붙어 있는 모니터들. 얻지 못하면(또는 Windows가 아니면) 빈 목록이다.
#[cfg(windows)]
pub fn monitors() -> Vec<MonitorInfo> {
    windows_screen::monitors()
}

/// 지금 붙어 있는 모니터들. Windows가 아니면 늘 빈 목록이다.
#[cfg(not(windows))]
pub fn monitors() -> Vec<MonitorInfo> {
    Vec::new()
}

/// 지금 있는 드라이브 문자들(대문자 `'A'..='Z'`, 문자 순서, `GetLogicalDrives`). 주소창 맨 앞 › 목록이 쓴다.
/// 얻지 못하면 빈 목록이다.
#[cfg(windows)]
pub fn drive_letters() -> Vec<char> {
    windows_screen::drive_letters()
}

/// 지금 있는 드라이브 문자들. Windows가 아니면 늘 빈 목록이다.
#[cfg(not(windows))]
pub fn drive_letters() -> Vec<char> {
    Vec::new()
}

/// `GetLogicalDrives`의 비트 마스크(비트 0이 A)를 드라이브 문자들로 바꾼다.
#[cfg_attr(not(windows), allow(dead_code))]
fn letters_from_mask(mask: u32) -> Vec<char> {
    (0..26u8)
        .filter(|bit| mask & (1 << bit) != 0)
        .map(|bit| char::from(b'A' + bit))
        .collect()
}

/// 기본 프로그램으로 열기가 실패한 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// 파일의 형식에 연결된 프로그램이 없다.
    NoAssociation,
    /// 경로가 셸이 받는 길이(260자)를 넘어 셸이 받지 못했다.
    PathTooLong,
    /// 이 환경(Windows가 아닌 곳)에서는 셸 호출을 할 수 없다.
    Unsupported,
    /// 그 밖의 오류. `code`는 Windows가 돌려준 HRESULT다.
    Other { code: i32, message: String },
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenError::NoAssociation => f.write_str("이 형식의 파일에 연결된 프로그램이 없습니다"),
            OpenError::PathTooLong => f.write_str("경로가 너무 길어 셸이 열 수 없습니다"),
            OpenError::Unsupported => f.write_str("이 환경에서는 기본 프로그램으로 열 수 없습니다"),
            OpenError::Other { code, message } => write!(f, "{message} (0x{code:08X})"),
        }
    }
}

impl std::error::Error for OpenError {}

/// 기본 프로그램으로 열기의 결과.
pub type OpenResult = Result<(), OpenError>;

enum Job {
    Open {
        path: PathBuf,
        reply: mpsc::Sender<OpenResult>,
    },
}

/// 셸 호출을 맡는 전용 일꾼 스레드.
///
/// 스레드는 COM을 STA로 초기화한 뒤 요청을 차례로 처리한다. 요청 함수는 결과를 기다리지 않고
/// 바로 [`mpsc::Receiver`]를 돌려주므로 부르는 쪽(UI 스레드)을 막지 않는다.
/// 값을 버리면 남은 요청을 마저 처리하고 스레드가 끝날 때까지 기다린다.
pub struct ShellWorker {
    sender: Option<mpsc::Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

impl ShellWorker {
    /// 일꾼 스레드를 띄운다. 스레드를 만들지 못하거나 COM 초기화가 실패하면 오류다.
    pub fn spawn() -> io::Result<Self> {
        let (sender, receiver) = mpsc::channel::<Job>();
        let (ready_sender, ready_receiver) = mpsc::channel::<io::Result<()>>();
        let thread = thread::Builder::new()
            .name("facet-shell".into())
            .spawn(move || run_worker(receiver, ready_sender))?;
        match ready_receiver.recv() {
            Ok(Ok(())) => Ok(ShellWorker {
                sender: Some(sender),
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(io::Error::other("셸 일꾼 스레드가 시작하다 끝났습니다"))
            }
        }
    }

    /// 파일을 기본 프로그램으로 연다. 바로 돌아오고, 결과는 돌려준 수신자로 온다.
    pub fn open_with_default_program(
        &self,
        path: impl Into<PathBuf>,
    ) -> mpsc::Receiver<OpenResult> {
        let (reply, receiver) = mpsc::channel();
        let job = Job::Open {
            path: path.into(),
            reply,
        };
        if let Some(sender) = &self.sender
            && let Err(mpsc::SendError(Job::Open { reply, .. })) = sender.send(job)
        {
            let _ = reply.send(Err(OpenError::Other {
                code: 0,
                message: "셸 일꾼 스레드가 끝났습니다".into(),
            }));
        }
        receiver
    }
}

impl Drop for ShellWorker {
    fn drop(&mut self) {
        // 보내는 쪽을 버리면 일꾼의 수신 반복이 끝난다.
        drop(self.sender.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_worker(jobs: mpsc::Receiver<Job>, ready: mpsc::Sender<io::Result<()>>) {
    #[cfg(windows)]
    let _com = match windows_impl::ComApartment::enter() {
        Ok(com) => com,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    for job in jobs {
        match job {
            Job::Open { path, reply } => {
                let _ = reply.send(open_path(&path));
            }
        }
    }
}

/// 셸이 보이는 경로의 최대 길이(UTF-16 단위, 끝의 NUL 제외). Windows의 `MAX_PATH`(260)에서 NUL 하나를 뺀 값.
const SHELL_MAX_PATH_LEN: usize = 259;

/// `HRESULT_FROM_WIN32`: Win32 오류 코드를 HRESULT로 바꾼다.
const fn hresult_from_win32(code: u32) -> i32 {
    (0x8007_0000 | (code & 0xFFFF)) as i32
}

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_PATH_NOT_FOUND: u32 = 3;
const ERROR_INVALID_NAME: u32 = 123;
const ERROR_BAD_PATHNAME: u32 = 161;
const ERROR_FILENAME_EXCED_RANGE: u32 = 206;
const ERROR_NO_ASSOCIATION: u32 = 1155;

/// 셸 호출이 실패했을 때 알 수 있는 것.
#[cfg_attr(not(windows), allow(dead_code))]
struct ShellFailure {
    code: i32,
    message: String,
    /// 셸이 「연결된 프로그램 없음」(`SE_ERR_NOASSOC`)을 돌려주었다.
    no_association: bool,
    /// 경로의 UTF-16 길이(끝의 NUL 제외).
    path_len: usize,
    /// 경로에 실제로 무언가 있다.
    path_exists: bool,
}

/// 셸 호출의 실패를 [`OpenError`]로 나눈다.
///
/// 길이가 셸의 한계를 넘는 경로가 실제로 있는데 셸이 「찾을 수 없음」류로 실패하면 셸이 경로를 받지 못한
/// 것으로 보고 [`OpenError::PathTooLong`]으로 돌려준다. 없는 파일은 길이와 상관없이 그 밖의 오류다.
#[cfg_attr(not(windows), allow(dead_code))]
fn classify_shell_failure(failure: ShellFailure) -> OpenError {
    let path_errors = [
        ERROR_FILE_NOT_FOUND,
        ERROR_PATH_NOT_FOUND,
        ERROR_INVALID_NAME,
        ERROR_BAD_PATHNAME,
        ERROR_FILENAME_EXCED_RANGE,
    ]
    .map(hresult_from_win32);
    if failure.path_len > SHELL_MAX_PATH_LEN
        && failure.path_exists
        && path_errors.contains(&failure.code)
    {
        return OpenError::PathTooLong;
    }
    if failure.no_association || failure.code == hresult_from_win32(ERROR_NO_ASSOCIATION) {
        return OpenError::NoAssociation;
    }
    OpenError::Other {
        code: failure.code,
        message: failure.message,
    }
}

#[cfg(windows)]
fn open_path(path: &std::path::Path) -> OpenResult {
    windows_impl::open_with_default_program(path)
}

#[cfg(not(windows))]
fn open_path(_path: &std::path::Path) -> OpenResult {
    Err(OpenError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(code: u32, path_len: usize, path_exists: bool) -> ShellFailure {
        ShellFailure {
            code: hresult_from_win32(code),
            message: "실패".into(),
            no_association: false,
            path_len,
            path_exists,
        }
    }

    #[test]
    fn 긴_경로가_있는데_셸이_찾지_못하면_경로가_너무_김이다() {
        for code in [
            ERROR_FILE_NOT_FOUND,
            ERROR_PATH_NOT_FOUND,
            ERROR_FILENAME_EXCED_RANGE,
        ] {
            assert_eq!(
                classify_shell_failure(failure(code, 300, true)),
                OpenError::PathTooLong
            );
        }
        assert_eq!(
            classify_shell_failure(failure(ERROR_FILE_NOT_FOUND, SHELL_MAX_PATH_LEN + 1, true)),
            OpenError::PathTooLong
        );
    }

    #[test]
    fn 셸이_받는_길이의_경로는_경로가_너무_김이_아니다() {
        let error = classify_shell_failure(failure(ERROR_FILE_NOT_FOUND, SHELL_MAX_PATH_LEN, true));
        assert!(matches!(error, OpenError::Other { .. }), "{error:?}");
    }

    #[test]
    fn 없는_긴_경로는_그_밖의_오류다() {
        let error = classify_shell_failure(failure(ERROR_FILE_NOT_FOUND, 300, false));
        assert_eq!(
            error,
            OpenError::Other {
                code: hresult_from_win32(ERROR_FILE_NOT_FOUND),
                message: "실패".into()
            }
        );
    }

    #[test]
    fn 셸이_연결_없음을_알리면_연결된_프로그램_없음이다() {
        assert_eq!(
            classify_shell_failure(failure(ERROR_NO_ASSOCIATION, 10, true)),
            OpenError::NoAssociation
        );
        let mut by_instance = failure(ERROR_FILE_NOT_FOUND, 10, true);
        by_instance.no_association = true;
        assert_eq!(
            classify_shell_failure(by_instance),
            OpenError::NoAssociation
        );
    }

    #[test]
    fn 드라이브_마스크는_문자_순서의_대문자다() {
        assert_eq!(letters_from_mask(0), Vec::<char>::new());
        assert_eq!(
            letters_from_mask(0b1 | 0b100 | 0b1000 | (1 << 25)),
            vec!['A', 'C', 'D', 'Z']
        );
        assert_eq!(letters_from_mask(u32::MAX).len(), 26);
    }

    #[test]
    fn hresult_변환은_windows와_같다() {
        assert_eq!(
            hresult_from_win32(ERROR_NO_ASSOCIATION),
            0x8007_0483_u32 as i32
        );
        assert_eq!(
            hresult_from_win32(ERROR_FILE_NOT_FOUND),
            0x8007_0002_u32 as i32
        );
    }
}
