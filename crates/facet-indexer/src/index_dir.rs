//! 인덱스 폴더(Windows 전용): 서비스가 볼륨마다 인덱스 파일을 두는 `%ProgramData%\facet\index\`.
//!
//! 인덱스에는 모든 사용자의 파일 이름이 들어 있으므로 폴더는 SYSTEM과 Administrators만 읽고 쓴다
//! ([`DIR_SDDL`]: 소유자 Administrators, 상속을 막은 DACL에 두 계정만 모든 권한, 하위 폴더와
//! 파일에 상속). `%ProgramData%` 아래는 일반 사용자도 폴더를 만들 수 있으므로 서비스보다 먼저 누가
//! 만들어 둔 폴더를 그대로 믿지 않는다([`prepare`]).
//!
//! - 없으면 처음부터 그 권한으로 만든다(만든 뒤 권한을 거는 사이의 틈이 없다).
//! - 있으면 소유자와 DACL을 바꿔 건다. 소유자가 SYSTEM·Administrators가 아니었던 인덱스 폴더는
//!   남이 넣어 둔 파일(자기 권한을 따로 건 파일 등)이 있을 수 있으므로 비우고 다시 만든다.
//! - 폴더 자리에 링크(심볼릭 링크·접합)나 파일이 있으면 그것을 지우고 폴더를 만든다. 링크를 따라
//!   다른 곳에 쓰지 않는다.

use std::ffi::c_void;
use std::fs;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HANDLE, LUID};
use windows_sys::Win32::Security::Authorization::{
    GetNamedSecurityInfoW, SE_FILE_OBJECT, SetNamedSecurityInfoW,
};
use windows_sys::Win32::Security::{
    ACL, AdjustTokenPrivileges, DACL_SECURITY_INFORMATION, GetSecurityDescriptorControl,
    GetSecurityDescriptorDacl, GetSecurityDescriptorOwner, LUID_AND_ATTRIBUTES,
    LookupPrivilegeValueW, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED, SE_PRIVILEGE_ENABLED, SE_RESTORE_NAME,
    SE_TAKE_OWNERSHIP_NAME, SECURITY_ATTRIBUTES, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES,
    UNPROTECTED_DACL_SECURITY_INFORMATION,
};
use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::pipe::{Ace, Local, dacl_aces, security_descriptor, sid_string};
use crate::volume::Handle;

/// 인덱스 폴더와 그 부모(`facet`) 폴더의 보안 설명자(SDDL): 소유자 Administrators, 상속을 막은
/// DACL에 SYSTEM과 Administrators만 모든 권한(하위 폴더·파일에 상속).
pub const DIR_SDDL: &str = "O:BAD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";

/// 만들 때 거는 보안 설명자. 소유자는 만드는 계정의 기본 소유자(서비스와 관리자 토큰은
/// Administrators)이고, 만든 뒤 [`DIR_SDDL`]로 소유자까지 맞춘다.
const CREATE_SDDL: &str = "D:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";

/// LocalSystem의 SID.
pub const SYSTEM_SID: &str = "S-1-5-18";
/// Administrators 그룹의 SID.
pub const ADMINISTRATORS_SID: &str = "S-1-5-32-544";

/// 서비스의 인덱스 폴더: `%ProgramData%\facet\index`. 환경 변수가 없으면 `C:\ProgramData`.
pub fn default_dir() -> PathBuf {
    let base = std::env::var_os("ProgramData")
        .filter(|v| !v.is_empty())
        .map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from);
    base.join("facet").join("index")
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// 폴더나 파일에 걸린 소유자와 DACL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Security {
    /// 소유자 SID(예: `S-1-5-32-544`).
    pub owner: String,
    /// DACL이 상속을 막는가.
    pub protected: bool,
    /// DACL의 허용·거부 ACE들.
    pub aces: Vec<Ace>,
}

/// `path`에 걸린 소유자와 DACL을 읽는다.
pub fn read_security(path: &Path) -> io::Result<Security> {
    let name = wide(path);
    let mut owner: PSID = null_mut();
    let mut dacl: *mut ACL = null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: name은 NUL로 끝나는 UTF-16 경로이고 받는 포인터들은 쓸 수 있는 지역 변수다.
    let status = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
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
    // SAFETY: sd.0은 GetNamedSecurityInfoW가 돌려준 유효한 보안 설명자다.
    if unsafe { GetSecurityDescriptorControl(sd.0, &mut control, &mut revision) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: owner와 dacl은 sd 안을 가리키고(dacl은 널일 수 있다) sd는 여기까지 살아 있다.
    let (owner, aces) = unsafe { (sid_string(owner)?, dacl_aces(dacl)?) };
    Ok(Security {
        owner,
        protected: control & SE_DACL_PROTECTED != 0,
        aces,
    })
}

/// `path`에 SDDL `sddl`의 소유자(있으면)와 DACL을 건다. 상속을 막는 DACL(`P`)이면 막고, 아니면
/// 부모에게서 상속받는다. 상속되는 ACE는 하위 폴더·파일에 퍼진다.
///
/// 남이 소유한 폴더의 소유자를 바꾸려면 소유권 가져오기 권한이 필요하므로, 이 프로세스 토큰의
/// `SeTakeOwnershipPrivilege`·`SeRestorePrivilege`를 켜 본다(관리자와 LocalSystem에게 있다).
pub fn apply_sddl(path: &Path, sddl: &str) -> io::Result<()> {
    enable_ownership_privileges();
    let sd = security_descriptor(sddl)?;
    let mut owner: PSID = null_mut();
    let mut defaulted = 0;
    // SAFETY: sd.0은 SDDL에서 만든 유효한 보안 설명자이고 받는 포인터들은 지역 변수다.
    if unsafe { GetSecurityDescriptorOwner(sd.0, &mut owner, &mut defaulted) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut present = 0;
    let mut dacl: *mut ACL = null_mut();
    // SAFETY: 위와 같다.
    if unsafe { GetSecurityDescriptorDacl(sd.0, &mut present, &mut dacl, &mut defaulted) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut control = 0u16;
    let mut revision = 0u32;
    // SAFETY: 위와 같다.
    if unsafe { GetSecurityDescriptorControl(sd.0, &mut control, &mut revision) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut info = DACL_SECURITY_INFORMATION;
    info |= if control & SE_DACL_PROTECTED != 0 {
        PROTECTED_DACL_SECURITY_INFORMATION
    } else {
        UNPROTECTED_DACL_SECURITY_INFORMATION
    };
    if !owner.is_null() {
        info |= OWNER_SECURITY_INFORMATION;
    }
    let name = wide(path);
    // SAFETY: name은 NUL로 끝나는 경로이고, owner와 dacl은 살아 있는 sd 안을 가리킨다(owner는 넣지
    // 않을 때 널).
    let status = unsafe {
        SetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            info,
            owner,
            null_mut(),
            dacl,
            null(),
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    Ok(())
}

/// 이 프로세스 토큰의 소유권 관련 권한을 켜 본다. 없거나 실패하면 그냥 둔다(소유자를 바꾸는 쪽이
/// 오류를 낸다).
fn enable_ownership_privileges() {
    let mut token: HANDLE = null_mut();
    // SAFETY: GetCurrentProcess는 가짜 핸들을 돌려주고 token은 쓸 수 있는 지역 변수다.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES, &mut token) } == 0 {
        return;
    }
    let token = Handle(token);
    for name in [SE_TAKE_OWNERSHIP_NAME, SE_RESTORE_NAME] {
        let mut luid = LUID {
            LowPart: 0,
            HighPart: 0,
        };
        // SAFETY: name은 NUL로 끝나는 상수 문자열이고 luid는 지역 변수다.
        if unsafe { LookupPrivilegeValueW(null(), name, &mut luid) } == 0 {
            continue;
        }
        let privileges = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        // SAFETY: token은 TOKEN_ADJUST_PRIVILEGES로 연 토큰이고 privileges는 호출 동안 살아 있다.
        // 앞 상태는 받지 않는다.
        unsafe {
            AdjustTokenPrivileges(
                token.0,
                0,
                &privileges,
                size_of::<TOKEN_PRIVILEGES>() as u32,
                null_mut(),
                null_mut(),
            )
        };
    }
}

/// `path`에 폴더를 처음부터 [`CREATE_SDDL`] 권한으로 만들고 소유자까지 [`DIR_SDDL`]로 맞춘다.
fn create_locked(path: &Path) -> io::Result<()> {
    let sd = security_descriptor(CREATE_SDDL)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0.cast::<c_void>(),
        bInheritHandle: 0,
    };
    let name = wide(path);
    // SAFETY: name은 NUL로 끝나는 경로이고 attributes와 그 보안 설명자는 호출 동안 살아 있다.
    if unsafe { CreateDirectoryW(name.as_ptr(), &attributes) } == 0 {
        return Err(io::Error::last_os_error());
    }
    apply_sddl(path, DIR_SDDL)
}

fn is_trusted_owner(sid: &str) -> bool {
    sid == SYSTEM_SID || sid == ADMINISTRATORS_SID
}

/// `path`가 SYSTEM·Administrators만 쓰는 폴더가 되게 한다(모듈 문서). `wipe_untrusted`면 소유자를
/// 믿을 수 없던 폴더를 비우고 다시 만든다.
fn lock_down(path: &Path, wipe_untrusted: bool) -> io::Result<()> {
    let meta = match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return create_locked(path),
        Err(e) => return Err(e),
        Ok(meta) => meta,
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        // 링크(디렉터리 링크·접합은 remove_dir, 파일 링크는 remove_file)나 파일. 링크는 가리키는
        // 곳을 건드리지 않고 링크만 지운다.
        if meta.is_dir() || fs::remove_file(path).is_err() {
            fs::remove_dir(path)?;
        }
        return create_locked(path);
    }
    let trusted = read_security(path).is_ok_and(|s| is_trusted_owner(&s.owner));
    apply_sddl(path, DIR_SDDL)?;
    if !trusted && wipe_untrusted {
        fs::remove_dir_all(path)?;
        return create_locked(path);
    }
    Ok(())
}

/// 인덱스 폴더 `dir`과 그 부모 폴더를 SYSTEM과 Administrators만 쓰도록 준비한다(없으면 만든다,
/// 모듈 문서). 관리자 권한이 필요하다.
pub fn prepare(dir: &Path) -> Result<(), String> {
    let parent = dir
        .parent()
        .ok_or_else(|| format!("{}에 부모 폴더가 없음", dir.display()))?;
    if let Some(base) = parent.parent() {
        fs::create_dir_all(base).map_err(|e| format!("{}를 만들지 못함: {e}", base.display()))?;
    }
    // 부모를 먼저 잠가야 그 아래 인덱스 폴더를 남이 바꿔치기하지 못한다.
    lock_down(parent, false)
        .map_err(|e| format!("{}의 권한을 걸지 못함: {e}", parent.display()))?;
    lock_down(dir, true).map_err(|e| format!("{}의 권한을 걸지 못함: {e}", dir.display()))
}

/// 인덱스 폴더 `dir`을 지운다. 그 부모(`facet`)가 비면 그것도 지운다. 없으면 아무것도 하지 않는다.
pub fn remove(dir: &Path) -> Result<(), String> {
    match fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("인덱스 폴더 {}를 지우지 못함: {e}", dir.display())),
    }
    if let Some(parent) = dir.parent() {
        // 비어 있지 않으면(다른 것이 들어 있으면) 남긴다.
        let _ = fs::remove_dir(parent);
    }
    Ok(())
}
