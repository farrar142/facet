//! 셸의 `IFileOperation`으로 복사하는 Windows API 호출(측정 전용, [`crate::shell_copy`]).

use std::path::{Path, PathBuf};

use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::Win32::UI::Shell::{
    FOF_NO_UI, FileOperation, IFileOperation, IFileOperationProgressSink, IShellItem,
    SHCreateItemFromParsingName,
};
use windows::core::PCWSTR;

use crate::shell_copy::ShellCopyError;
use crate::windows_impl::{ComApartment, to_wide};

fn failed(error: windows::core::Error) -> ShellCopyError {
    ShellCopyError::Failed {
        code: error.code().0,
        message: error.message(),
    }
}

/// 경로의 셸 항목.
fn shell_item(path: &Path) -> Result<IShellItem, ShellCopyError> {
    let wide = to_wide(path.as_os_str());
    // SAFETY: wide는 NUL로 끝나는 UTF-16 경로이고 호출이 끝날 때까지 살아 있다. 바인드 문맥은 주지 않는다.
    unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None) }.map_err(failed)
}

/// 이 스레드에서 COM을 STA로 초기화하고 `sources`를 `target` 안으로 복사한다. 끝나면 COM을 정리한다.
pub(crate) fn copy_items(sources: &[PathBuf], target: &Path) -> Result<(), ShellCopyError> {
    let _com = ComApartment::enter().map_err(|e| ShellCopyError::Failed {
        code: 0,
        message: e.to_string(),
    })?;
    // COM 객체는 _com보다 먼저 버려지도록 안쪽 함수에서 만든다.
    perform(sources, target)
}

fn perform(sources: &[PathBuf], target: &Path) -> Result<(), ShellCopyError> {
    // SAFETY: 이 스레드는 COM을 초기화했다(ComApartment). FileOperation은 IFileOperation을 구현하는 셸 클래스다.
    let operation: IFileOperation =
        unsafe { CoCreateInstance(&FileOperation, None, CLSCTX_ALL) }.map_err(failed)?;
    // SAFETY: operation은 살아 있는 COM 객체이고 플래그는 상수다. FOF_NO_UI: 진행 창·확인·오류 대화를 띄우지 않는다.
    unsafe { operation.SetOperationFlags(FOF_NO_UI) }.map_err(failed)?;
    let destination = shell_item(target)?;
    for source in sources {
        let item = shell_item(source)?;
        // SAFETY: 두 셸 항목과 operation은 살아 있는 COM 객체다. 새 이름은 주지 않아(null) 원래 이름을 쓰고,
        // 진행 알림 객체도 주지 않는다.
        unsafe {
            operation.CopyItem(
                &item,
                &destination,
                PCWSTR::null(),
                None::<&IFileOperationProgressSink>,
            )
        }
        .map_err(failed)?;
    }
    // SAFETY: operation은 살아 있는 COM 객체이고, 넣은 작업을 이 스레드에서 끝까지 실행한다.
    unsafe { operation.PerformOperations() }.map_err(failed)?;
    // SAFETY: operation은 살아 있는 COM 객체이고 작업을 다 실행했다.
    let aborted = unsafe { operation.GetAnyOperationsAborted() }.map_err(failed)?;
    if aborted.as_bool() {
        return Err(ShellCopyError::Aborted);
    }
    Ok(())
}
