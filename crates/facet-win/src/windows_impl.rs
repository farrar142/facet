//! Windows API를 부르는 부분. `unsafe`는 모두 이 파일 안에 있다.

use std::cmp::Ordering;
use std::ffi::OsStr;
use std::io;
use std::marker::PhantomData;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Foundation::{ERROR_INVALID_NAME, ERROR_NO_ASSOCIATION};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Registry::{HKEY, RegCloseKey, RegQueryValueExW};
use windows::Win32::UI::Shell::{
    ASSOCF_INIT_IGNOREUNKNOWN, ASSOCKEY, ASSOCKEY_CLASS, ASSOCKEY_SHELLEXECCLASS, AssocQueryKeyW,
    SE_ERR_NOASSOC, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
    StrCmpLogicalW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HRESULT, PCWSTR, w};

use crate::{OpenError, OpenResult, ShellFailure, classify_shell_failure};

/// 탐색기가 이름을 비교하는 방식(`StrCmpLogicalW`)으로 두 이름을 비교한다.
///
/// 대소문자를 가리지 않고, 이름 속 숫자는 값으로 비교한다(「파일2」 < 「파일10」).
/// 이름에 NUL 문자가 있으면 그 앞까지만 비교한다(파일 이름에는 NUL이 올 수 없다).
pub fn compare_names_like_explorer(a: &str, b: &str) -> Ordering {
    let a = to_wide(OsStr::new(a));
    let b = to_wide(OsStr::new(b));
    // SAFETY: 두 버퍼 모두 NUL로 끝나는 UTF-16 문자열이고, 호출이 끝날 때까지 살아 있다.
    let result = unsafe { StrCmpLogicalW(PCWSTR(a.as_ptr()), PCWSTR(b.as_ptr())) };
    result.cmp(&0)
}

/// NUL로 끝나는 UTF-16 문자열로 바꾼다.
fn to_wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

/// 이 스레드에 들어간 COM 아파트(STA). 버리면 COM을 정리한다.
///
/// `CoUninitialize`는 초기화한 스레드에서 불러야 하므로 다른 스레드로 보낼 수 없게 한다.
pub(crate) struct ComApartment {
    _not_send: PhantomData<*const ()>,
}

impl ComApartment {
    pub(crate) fn enter() -> io::Result<Self> {
        // SAFETY: 예약 인자는 None이고, 성공하면 같은 스레드에서 Drop이 CoUninitialize를 한 번 부른다.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        hr.ok()
            .map_err(|error| io::Error::other(error.to_string()))?;
        Ok(ComApartment {
            _not_send: PhantomData,
        })
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        // SAFETY: enter가 이 스레드에서 성공한 CoInitializeEx와 짝을 이룬다.
        unsafe { CoUninitialize() };
    }
}

/// 연결 정보의 레지스트리 키. 버리면 닫는다.
struct AssocKey(HKEY);

impl AssocKey {
    /// 확장자(`.txt`)의 연결 키를 찾는다. 「알 수 없는 형식」으로 대신하지 않는다.
    fn query(key: ASSOCKEY, extension: &[u16]) -> windows::core::Result<Self> {
        // SAFETY: extension은 NUL로 끝나는 UTF-16 문자열이고 호출이 끝날 때까지 살아 있다.
        // 돌려받은 키는 AssocKey가 맡아 한 번만 닫는다.
        let key = unsafe {
            AssocQueryKeyW(
                ASSOCF_INIT_IGNOREUNKNOWN,
                key,
                PCWSTR(extension.as_ptr()),
                PCWSTR::null(),
            )
        }?;
        Ok(AssocKey(key))
    }

    fn has_value(&self, name: PCWSTR) -> bool {
        // SAFETY: 키는 열려 있고, 값 이름은 NUL로 끝나는 상수 문자열이다. 데이터는 받지 않는다.
        let status = unsafe { RegQueryValueExW(self.0, name, None, None, None, None) };
        status.is_ok()
    }
}

impl Drop for AssocKey {
    fn drop(&mut self) {
        // SAFETY: AssocQueryKeyW가 연 키이고, 여기서 한 번만 닫는다.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

/// 파일의 형식에 셸이 실행할 프로그램이 없는지 본다.
///
/// 셸은 연결이 없는 파일을 열라고 하면 오류 대신 「연결 프로그램」 창을 띄운다(`SEE_MASK_FLAG_NO_UI`를
/// 주어도 그렇다). 그래서 셸을 부르기 전에 확장자의 연결을 직접 확인한다.
/// 바로 가기(`.lnk`처럼 `IsShortcut`이 있는 형식)는 대상으로 열리므로 연결이 없다고 보지 않는다.
fn has_no_association(path: &Path) -> bool {
    let mut extension = std::ffi::OsString::from(".");
    if let Some(ext) = path.extension() {
        extension.push(ext);
    }
    let extension = to_wide(&extension);
    match AssocKey::query(ASSOCKEY_SHELLEXECCLASS, &extension) {
        Ok(_) => false,
        Err(error) if error.code() == HRESULT::from_win32(ERROR_NO_ASSOCIATION.0) => {
            let is_shortcut = AssocKey::query(ASSOCKEY_CLASS, &extension)
                .is_ok_and(|class| class.has_value(w!("IsShortcut")));
            !is_shortcut
        }
        // 그 밖의 실패는 판단하지 않고 셸에 맡긴다.
        Err(_) => false,
    }
}

/// 파일을 기본 프로그램으로 연다. COM을 초기화한 스레드에서 불러야 한다.
pub(crate) fn open_with_default_program(path: &Path) -> OpenResult {
    let wide = to_wide(path.as_os_str());
    let length = wide.len() - 1;
    if wide[..length].contains(&0) {
        return Err(OpenError::Other {
            code: HRESULT::from_win32(ERROR_INVALID_NAME.0).0,
            message: "경로에 NUL 문자가 있습니다".into(),
        });
    }
    if path.is_file() && has_no_association(path) {
        return Err(OpenError::NoAssociation);
    }

    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        // 오류 대화 상자를 띄우지 않고 오류로 돌려받는다. 일꾼 스레드는 메시지 루프가 없으므로
        // 셸이 실행을 마칠 때까지 이 호출 안에서 기다린다.
        fMask: SEE_MASK_FLAG_NO_UI | SEE_MASK_NOASYNC,
        lpFile: PCWSTR(wide.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: info는 cbSize가 맞게 채워졌고, 가리키는 문자열(wide)은 호출이 끝날 때까지 살아 있다.
    // 동사(lpVerb)는 비워 두어 형식의 기본 동사를 쓴다.
    let result = unsafe { ShellExecuteExW(&mut info) };
    let Err(error) = result else {
        return Ok(());
    };
    Err(classify_shell_failure(ShellFailure {
        code: error.code().0,
        message: error.message(),
        no_association: info.hInstApp.0 as usize == SE_ERR_NOASSOC as usize,
        path_len: length,
        path_exists: path.exists(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 알_수_없는_확장자는_연결이_없다() {
        assert!(has_no_association(Path::new(r"C:\x\문서.facetnoassoc7f3a")));
        assert!(has_no_association(Path::new(r"C:\x\확장자없음")));
    }

    #[test]
    fn 실행_파일과_바로_가기는_연결이_있다() {
        assert!(!has_no_association(Path::new(r"C:\x\a.exe")));
        assert!(!has_no_association(Path::new(r"C:\x\a.lnk")));
    }
}
