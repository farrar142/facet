//! 다른 프로세스가 끝나기를 기다린다. 셸 확장 도우미가 UI 프로세스(부모)를 감시할 때 쓴다(spec #24).
//!
//! Windows가 아니면 [`wait_for_exit`]는 늘 [`io::ErrorKind::Unsupported`]다.

use std::io;

/// 프로세스 번호 `pid`의 프로세스가 끝날 때까지 기다린다(부르는 스레드를 막는다).
///
/// 그 번호의 프로세스가 이미 없으면 곧바로 `Err`(열지 못함)다. 부르는 쪽은 그것도 「끝났다」로 본다.
pub fn wait_for_exit(pid: u32) -> io::Result<()> {
    #[cfg(windows)]
    {
        imp::wait_for_exit(pid)
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

#[cfg(windows)]
mod imp {
    use std::io;

    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{
        INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    pub fn wait_for_exit(pid: u32) -> io::Result<()> {
        // SAFETY: 값으로 받는 인자만 넘긴다. 돌려받은 핸들은 아래에서 한 번 닫는다.
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }
            .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xFFFF))?;
        // SAFETY: `handle`은 방금 연 프로세스 핸들이고 이 함수가 닫기 전까지 살아 있다.
        let waited = unsafe { WaitForSingleObject(handle, INFINITE) };
        // SAFETY: 위에서 연 핸들을 한 번만 닫는다.
        let _ = unsafe { CloseHandle(handle) };
        if waited == WAIT_OBJECT_0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn 끝난_자식을_기다리면_돌아온다() {
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "exit 0"])
            .spawn()
            .unwrap();
        let pid = child.id();
        // 아직 거두지 않았으므로 번호가 남아 있어 열 수 있다.
        assert!(wait_for_exit(pid).is_ok());
        child.wait().unwrap();
    }
}
