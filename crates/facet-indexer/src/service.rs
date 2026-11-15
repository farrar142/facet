//! Windows 서비스(Windows 전용): 서비스 관리자가 띄우는 본체, 개발용 설치·제거.
//!
//! - 본체([`run_service`]): 서비스 관리자에 붙어 질의 파이프 서버를 띄우고, 드라이브 문자가 있는
//!   로컬 NTFS 볼륨마다 첫 인덱스를 만든 뒤 USN 저널을 따라간다([`index_volumes`]). 멈추라는
//!   제어가 오면 따라가기에 멈추라고 알리고 파이프 서버를 닫고 끝난다.
//! - 설치([`install`]): 이 실행 파일을 LocalSystem 계정의 자동 시작 서비스로 등록하고 시작한다.
//! - 제거([`uninstall`]): 서비스를 멈추고 지운다.

use std::ffi::c_void;
use std::io;
use std::path::Path;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use facet_protocol::NoIndexReason;
use windows_sys::Win32::Foundation::{
    ERROR_CALL_NOT_IMPLEMENTED, ERROR_FAILED_SERVICE_CONTROLLER_CONNECT,
    ERROR_SERVICE_DOES_NOT_EXIST, ERROR_SERVICE_EXISTS, ERROR_SERVICE_MARKED_FOR_DELETE,
    ERROR_SERVICE_NOT_ACTIVE, NO_ERROR,
};
use windows_sys::Win32::System::Services::{
    ChangeServiceConfig2W, CloseServiceHandle, ControlService, CreateServiceW, DeleteService,
    OpenSCManagerW, OpenServiceW, QueryServiceStatus, RegisterServiceCtrlHandlerExW, SC_HANDLE,
    SC_MANAGER_ALL_ACCESS, SERVICE_ACCEPT_SHUTDOWN, SERVICE_ACCEPT_STOP, SERVICE_ALL_ACCESS,
    SERVICE_AUTO_START, SERVICE_CONFIG_DESCRIPTION, SERVICE_CONTROL_INTERROGATE,
    SERVICE_CONTROL_SHUTDOWN, SERVICE_CONTROL_STOP, SERVICE_DESCRIPTIONW, SERVICE_ERROR_NORMAL,
    SERVICE_RUNNING, SERVICE_START_PENDING, SERVICE_STATUS, SERVICE_STATUS_CURRENT_STATE,
    SERVICE_STOP_PENDING, SERVICE_STOPPED, SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
    SetServiceStatus, StartServiceCtrlDispatcherW, StartServiceW,
};

use crate::follow::{self, Stop};
use crate::pipe::PipeServer;
use crate::volume::{self, NEED_ADMIN, is_elevated};
use crate::volumes::{Plan, Volumes};
use crate::{SERVICE_DESCRIPTION, SERVICE_DISPLAY_NAME, SERVICE_NAME};

/// 설치·제거에서 서비스가 시작되거나 멈추기를 기다리는 시간.
const WAIT: Duration = Duration::from_secs(30);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn last_error() -> io::Error {
    io::Error::last_os_error()
}

/// 서비스 관리자 핸들. 버릴 때 닫는다.
struct ScHandle(SC_HANDLE);

impl Drop for ScHandle {
    fn drop(&mut self) {
        // SAFETY: self.0은 OpenSCManagerW/OpenServiceW/CreateServiceW가 돌려준 유효한 핸들이고 한 번만
        // 닫는다.
        unsafe { CloseServiceHandle(self.0) };
    }
}

fn open_manager() -> Result<ScHandle, String> {
    // SAFETY: 로컬 컴퓨터의 기본 데이터베이스를 연다(이름 둘 다 널).
    let h = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_ALL_ACCESS) };
    if h.is_null() {
        return Err(format!("서비스 관리자를 열지 못함: {}", last_error()));
    }
    Ok(ScHandle(h))
}

/// 이름이 [`SERVICE_NAME`]인 서비스를 연다. 없으면 `Ok(None)`.
fn open_service(manager: &ScHandle) -> Result<Option<ScHandle>, String> {
    let name = wide(SERVICE_NAME);
    // SAFETY: manager는 열린 핸들이고 name은 NUL로 끝나는 UTF-16 문자열이다.
    let h = unsafe { OpenServiceW(manager.0, name.as_ptr(), SERVICE_ALL_ACCESS) };
    if h.is_null() {
        let e = last_error();
        if e.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST as i32) {
            return Ok(None);
        }
        return Err(format!("서비스 {SERVICE_NAME}를 열지 못함: {e}"));
    }
    Ok(Some(ScHandle(h)))
}

fn query_state(service: &ScHandle) -> Result<SERVICE_STATUS_CURRENT_STATE, String> {
    // SAFETY: SERVICE_STATUS는 정수만 담은 C 구조체라 0으로 채워도 유효하다.
    let mut status: SERVICE_STATUS = unsafe { std::mem::zeroed() };
    // SAFETY: service는 열린 핸들이고 status는 쓸 수 있는 지역 변수다.
    if unsafe { QueryServiceStatus(service.0, &mut status) } == 0 {
        return Err(format!("서비스 상태를 읽지 못함: {}", last_error()));
    }
    Ok(status.dwCurrentState)
}

/// 서비스가 `want` 상태가 될 때까지 기다린다. `give_up` 상태가 되면 일찍 그만둔다.
fn wait_state(
    service: &ScHandle,
    want: SERVICE_STATUS_CURRENT_STATE,
    give_up: Option<SERVICE_STATUS_CURRENT_STATE>,
) -> Result<(), String> {
    let deadline = Instant::now() + WAIT;
    loop {
        let state = query_state(service)?;
        if state == want {
            return Ok(());
        }
        if Some(state) == give_up || Instant::now() >= deadline {
            return Err(format!(
                "서비스가 상태 {want}이 되지 않음(지금 {state}, {}초 기다림)",
                WAIT.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// 설치 명령: `exe`를 서비스 [`SERVICE_NAME`](LocalSystem, 자동 시작)으로 등록하고 시작한다.
/// 시작하지 못하면 등록을 되돌린다. 이미 설치돼 있으면 오류다.
pub fn install(exe: &Path) -> Result<(), String> {
    if !is_elevated() {
        return Err(NEED_ADMIN.to_owned());
    }
    let manager = open_manager()?;
    let name = wide(SERVICE_NAME);
    let display = wide(SERVICE_DISPLAY_NAME);
    let command = wide(&format!("\"{}\" service", exe.display()));
    // SAFETY: 문자열은 모두 NUL로 끝나는 UTF-16이다. 계정(널)은 LocalSystem이고, 그룹·태그·의존
    // 서비스·암호는 쓰지 않는다.
    let h = unsafe {
        CreateServiceW(
            manager.0,
            name.as_ptr(),
            display.as_ptr(),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            command.as_ptr(),
            null(),
            null_mut(),
            null(),
            null(),
            null(),
        )
    };
    if h.is_null() {
        let e = last_error();
        return Err(match e.raw_os_error().map(|c| c as u32) {
            Some(ERROR_SERVICE_EXISTS) => format!(
                "서비스 {SERVICE_NAME}가 이미 설치돼 있습니다. 먼저 제거(uninstall)하십시오"
            ),
            Some(ERROR_SERVICE_MARKED_FOR_DELETE) => format!(
                "서비스 {SERVICE_NAME}가 지워지는 중입니다. 서비스 관리 창을 닫고 다시 하십시오"
            ),
            _ => format!("서비스를 등록하지 못함: {e}"),
        });
    }
    let service = ScHandle(h);
    let mut description = wide(SERVICE_DESCRIPTION);
    let info = SERVICE_DESCRIPTIONW {
        lpDescription: description.as_mut_ptr(),
    };
    // SAFETY: info와 그 문자열은 호출 동안 살아 있다. 설명은 없어도 되므로 실패는 무시한다.
    unsafe {
        ChangeServiceConfig2W(
            service.0,
            SERVICE_CONFIG_DESCRIPTION,
            (&info as *const SERVICE_DESCRIPTIONW).cast::<c_void>(),
        )
    };
    let started = start(&service);
    if let Err(e) = started {
        // SAFETY: service는 방금 만든 서비스의 열린 핸들이다.
        unsafe { DeleteService(service.0) };
        return Err(format!("{e}. 등록을 되돌렸습니다"));
    }
    Ok(())
}

fn start(service: &ScHandle) -> Result<(), String> {
    // SAFETY: service는 열린 핸들이고 인자는 넘기지 않는다.
    if unsafe { StartServiceW(service.0, 0, null()) } == 0 {
        return Err(format!("서비스를 시작하지 못함: {}", last_error()));
    }
    wait_state(service, SERVICE_RUNNING, Some(SERVICE_STOPPED))
}

/// 제거 명령의 결과.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Uninstalled {
    /// 멈추고 지웠다.
    Removed,
    /// 설치돼 있지 않았다.
    NotInstalled,
}

/// 제거 명령: 서비스 [`SERVICE_NAME`]을 멈추고 지운다. 지워질 때까지 기다린다.
pub fn uninstall() -> Result<Uninstalled, String> {
    if !is_elevated() {
        return Err(NEED_ADMIN.to_owned());
    }
    let manager = open_manager()?;
    let Some(service) = open_service(&manager)? else {
        return Ok(Uninstalled::NotInstalled);
    };
    if query_state(&service)? != SERVICE_STOPPED {
        // SAFETY: SERVICE_STATUS는 정수만 담은 C 구조체라 0으로 채워도 유효하다.
        let mut status: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        // SAFETY: service는 열린 핸들이고 status는 쓸 수 있는 지역 변수다.
        if unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) } == 0 {
            let e = last_error();
            if e.raw_os_error() != Some(ERROR_SERVICE_NOT_ACTIVE as i32) {
                return Err(format!("서비스를 멈추지 못함: {e}"));
            }
        }
        wait_state(&service, SERVICE_STOPPED, None)?;
    }
    // SAFETY: service는 열린 핸들이다.
    if unsafe { DeleteService(service.0) } == 0 {
        let e = last_error();
        if e.raw_os_error() != Some(ERROR_SERVICE_MARKED_FOR_DELETE as i32) {
            return Err(format!("서비스를 지우지 못함: {e}"));
        }
    }
    // 핸들을 모두 닫아야 서비스 관리자가 실제로 지운다.
    drop(service);
    let deadline = Instant::now() + WAIT;
    while open_service(&manager)?.is_some() {
        if Instant::now() >= deadline {
            return Err(format!(
                "서비스를 지우라고 했지만 {}초 안에 사라지지 않음(서비스 관리 창이 열려 있으면 닫으십시오)",
                WAIT.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
    Ok(Uninstalled::Removed)
}

/// 볼륨들의 인덱스를 만들고 최신으로 유지하기 시작한다. 인덱스를 만들 볼륨은 「만드는 중」으로 두고
/// 볼륨마다 스레드 하나가 MFT를 읽어 첫 인덱스를 만든 뒤 USN 저널을 따라간다([`follow::run`]).
/// 저널을 따라갈 수 없게 되면 그 스레드가 인덱스를 다시 만든다. 만들지 못하면 「인덱스 없음(만들지
/// 못함)」이다. 인덱스를 만들지 않는 볼륨은 바로 「인덱스 없음(까닭)」이 된다.
///
/// 스레드는 `stop`이 서면 끝난다(인덱스를 만드는 중이면 다 만든 뒤에).
///
/// 볼륨마다 스레드를 따로 두는 것은, 큰 볼륨(C:)을 읽는 동안 작은 볼륨이 기다리지 않게 하려는 것이다.
pub fn index_volumes(
    volumes: &Arc<Volumes>,
    targets: Vec<(String, Plan)>,
    stop: &Stop,
) -> Vec<JoinHandle<()>> {
    let mut threads = Vec::new();
    for (name, plan) in targets {
        match plan {
            Plan::NotIndexed(reason) => volumes.set_not_indexed(&name, reason),
            Plan::Index => {
                volumes.set_building(&name);
                let Some(drive) = name.chars().next() else {
                    continue;
                };
                let shared = Arc::clone(volumes);
                let stop = stop.clone();
                let spawned = thread::Builder::new()
                    .name(format!("facet-index-{drive}"))
                    .spawn(move || {
                        let mut source = volume::NtfsSource::new(drive);
                        follow::run(&shared, &name, &mut source, &stop, follow::POLL);
                    });
                match spawned {
                    Ok(t) => threads.push(t),
                    Err(e) => volumes.set_not_indexed(
                        &format!("{drive}:"),
                        NoIndexReason::Failed {
                            message: format!("인덱스를 만들 스레드를 띄우지 못함: {e}"),
                        },
                    ),
                }
            }
        }
    }
    threads
}

/// 서비스 관리자가 띄운 프로세스에서 서비스 본체를 돌린다. 서비스가 멈추면 돌아온다.
pub fn run_service() -> Result<(), String> {
    let mut name = wide(SERVICE_NAME);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: name.as_mut_ptr(),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: null_mut(),
            lpServiceProc: None,
        },
    ];
    // SAFETY: table은 널 항목으로 끝나고 서비스가 멈출 때까지(이 호출이 돌아올 때까지) 살아 있다.
    if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
        let e = last_error();
        if e.raw_os_error() == Some(ERROR_FAILED_SERVICE_CONTROLLER_CONNECT as i32) {
            return Err(
                "서비스 관리자가 띄운 프로세스가 아닙니다. 서비스는 install 명령으로 설치해 시작합니다"
                    .to_owned(),
            );
        }
        return Err(format!("서비스 관리자에 붙지 못함: {e}"));
    }
    Ok(())
}

/// 서비스 상태 핸들(`SERVICE_STATUS_HANDLE`).
static STATUS_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
/// 대기 상태 알림의 진행 번호.
static CHECKPOINT: AtomicU32 = AtomicU32::new(0);
/// 멈추라는 제어를 받았는가.
static STOP: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

fn report(state: SERVICE_STATUS_CURRENT_STATE, exit_code: u32) {
    let pending = matches!(state, SERVICE_START_PENDING | SERVICE_STOP_PENDING);
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: if state == SERVICE_RUNNING {
            SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
        } else {
            0
        },
        dwWin32ExitCode: exit_code,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: if pending {
            CHECKPOINT.fetch_add(1, Ordering::Relaxed) + 1
        } else {
            0
        },
        dwWaitHint: if pending { 10_000 } else { 0 },
    };
    let handle = STATUS_HANDLE.load(Ordering::Acquire);
    if !handle.is_null() {
        // SAFETY: handle은 RegisterServiceCtrlHandlerExW가 돌려준 상태 핸들이고(닫지 않는다), status는
        // 호출 동안 살아 있다.
        unsafe { SetServiceStatus(handle, &status) };
    }
}

/// 서비스 관리자가 보내는 제어를 받는다.
unsafe extern "system" fn control_handler(
    control: u32,
    _event_type: u32,
    _event_data: *mut c_void,
    _context: *mut c_void,
) -> u32 {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            report(SERVICE_STOP_PENDING, NO_ERROR);
            let (lock, wake) = &STOP;
            *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
            wake.notify_all();
            NO_ERROR
        }
        SERVICE_CONTROL_INTERROGATE => NO_ERROR,
        _ => ERROR_CALL_NOT_IMPLEMENTED,
    }
}

/// 서비스 본체. 서비스 관리자가 따로 띄운 스레드에서 부른다.
unsafe extern "system" fn service_main(_argc: u32, _argv: *mut windows_sys::core::PWSTR) {
    let name = wide(SERVICE_NAME);
    // SAFETY: name은 NUL로 끝나는 UTF-16 문자열이고 control_handler는 프로세스 끝까지 살아 있는
    // 함수다. 문맥은 쓰지 않는다.
    let handle =
        unsafe { RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(control_handler), null()) };
    if handle.is_null() {
        return;
    }
    STATUS_HANDLE.store(handle, Ordering::Release);
    report(SERVICE_START_PENDING, NO_ERROR);

    let volumes = Arc::new(Volumes::new());
    let server = match PipeServer::start(facet_protocol::PIPE_NAME, volumes.clone()) {
        Ok(s) => s,
        Err(e) => {
            report(SERVICE_STOPPED, e.raw_os_error().map_or(1, |c| c as u32));
            return;
        }
    };
    // 인덱스는 뒤에서 만들고 따라간다. 그동안 상태 질의는 「만드는 중」을 돌려준다.
    let followers = Stop::new();
    let _threads = index_volumes(&volumes, volume::local_volumes(), &followers);
    report(SERVICE_RUNNING, NO_ERROR);

    let (lock, wake) = &STOP;
    let mut stop = lock.lock().unwrap_or_else(|e| e.into_inner());
    while !*stop {
        stop = wake.wait(stop).unwrap_or_else(|e| e.into_inner());
    }
    drop(stop);
    followers.stop();
    drop(server);
    // 따라가기 스레드는 기다리지 않는다(인덱스를 만드는 중이면 오래 걸린다). 멈춤을 알리면
    // 프로세스가 끝나며 함께 끝난다.
    report(SERVICE_STOPPED, NO_ERROR);
}
