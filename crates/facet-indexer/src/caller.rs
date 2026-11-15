//! 질의한 사용자의 토큰으로 폴더 「목록 읽기」 권한을 검사한다(Windows 전용).
//!
//! 파이프 서버는 연결마다 [`PipeCaller`]를 둔다. 처음 검사할 때 파이프 클라이언트로 가장
//! (`ImpersonateNamedPipeClient`)해 그 스레드 토큰(가장 수준 토큰)을 복제해 두고 바로 되돌린다
//! (`RevertToSelf`). 검사는 가장하지 않고, 서비스 계정으로 폴더의 보안 설명자를 읽어
//! (`GetFileSecurityW`) 그 토큰으로 `AccessCheck`(`FILE_LIST_DIRECTORY`)를 부른다. 토큰을 잡지
//! 못했거나 보안 설명자를 읽지 못하면 읽을 수 없는 것으로 친다(닫힌 쪽으로 실패).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io;
use std::mem::size_of;
use std::ptr::null_mut;
use std::sync::{Arc, OnceLock};

use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, GetLastError, HANDLE};
use windows_sys::Win32::Security::{
    AccessCheck, DACL_SECURITY_INFORMATION, DuplicateToken, GENERIC_MAPPING,
    GROUP_SECURITY_INFORMATION, GetFileSecurityW, GetTokenInformation, OWNER_SECURITY_INFORMATION,
    PRIVILEGE_SET, RevertToSelf, SID_AND_ATTRIBUTES, SecurityImpersonation, TOKEN_DUPLICATE,
    TOKEN_GROUPS, TOKEN_IMPERSONATE, TOKEN_INFORMATION_CLASS, TOKEN_QUERY, TOKEN_USER, TokenGroups,
    TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ALL_ACCESS, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
    FILE_LIST_DIRECTORY,
};
use windows_sys::Win32::System::Pipes::ImpersonateNamedPipeClient;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken,
};

use crate::access::Caller;
use crate::pipe::sid_string;
use crate::volume::Handle;

/// 그룹을 접근 검사에 쓴다(`SE_GROUP_ENABLED`).
const SE_GROUP_ENABLED: u32 = 0x0000_0004;
/// 그룹을 거부 ACE에만 쓴다(`SE_GROUP_USE_FOR_DENY_ONLY`).
const SE_GROUP_USE_FOR_DENY_ONLY: u32 = 0x0000_0010;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// 긴 경로도 열리게 `\\?\`를 붙인다(이미 `\\`로 시작하면 그대로).
fn long_path(path: &str) -> String {
    if path.starts_with(r"\\") {
        path.to_owned()
    } else {
        format!(r"\\?\{path}")
    }
}

/// 사용자 하나의 가장 수준 토큰과 그 사용자 SID.
pub struct ClientToken {
    token: Handle,
    sid: String,
    /// 검사 결과를 기억할 열쇠: SID와 토큰 그룹(사용함·거부 전용 상태 포함)의 요약.
    key: String,
}

/// 토큰 정보 하나를 읽는다. 포인터를 담는 구조체이므로 u64로 잡아 정렬을 맞춘다.
fn token_information(token: &Handle, class: TOKEN_INFORMATION_CLASS) -> io::Result<Vec<u64>> {
    let mut needed = 0u32;
    // SAFETY: 크기만 묻는 호출이다(버퍼 없음, 길이 0).
    unsafe { GetTokenInformation(token.0, class, null_mut(), 0, &mut needed) };
    let mut buf = vec![0u64; (needed as usize).div_ceil(8).max(1)];
    // SAFETY: buf는 needed바이트 이상이고 그 크기를 함께 넘긴다.
    let ok = unsafe {
        GetTokenInformation(
            token.0,
            class,
            buf.as_mut_ptr().cast(),
            (buf.len() * 8) as u32,
            &mut needed,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(buf)
}

impl ClientToken {
    /// 이 프로세스를 돌리는 사용자의 토큰(시험용). 프로세스 토큰을 가장 수준으로 복제한다.
    pub fn current() -> io::Result<Self> {
        let mut process: HANDLE = null_mut();
        // SAFETY: GetCurrentProcess는 가짜 핸들을 돌려주고, process는 쓸 수 있는 지역 변수다.
        if unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_DUPLICATE | TOKEN_QUERY,
                &mut process,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let process = Handle(process);
        let mut token: HANDLE = null_mut();
        // SAFETY: process는 TOKEN_DUPLICATE로 연 토큰이고 token은 쓸 수 있는 지역 변수다.
        if unsafe { DuplicateToken(process.0, SecurityImpersonation, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Self::from_token(Handle(token))
    }

    /// 파이프 클라이언트의 토큰. 서버가 클라이언트에게서 한 번 이상 읽은 뒤에만 된다.
    fn of_pipe_client(pipe: HANDLE) -> io::Result<Self> {
        // SAFETY: pipe는 연결된 서버 쪽 파이프 핸들이다. 성공하면 이 스레드가 클라이언트로
        // 가장하므로 아래에서 반드시 되돌린다.
        if unsafe { ImpersonateNamedPipeClient(pipe) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut token: HANDLE = null_mut();
        // SAFETY: 가장 중인 이 스레드의 토큰을 연다. 접근 검사는 서비스 계정으로 한다(OpenAsSelf).
        let opened = unsafe {
            OpenThreadToken(
                GetCurrentThread(),
                TOKEN_QUERY | TOKEN_IMPERSONATE | TOKEN_DUPLICATE,
                1,
                &mut token,
            )
        };
        let error = io::Error::last_os_error();
        // SAFETY: 이 스레드의 가장을 끝낸다. 되돌리지 못하면 이 스레드가 클라이언트 권한으로 계속
        // 돌게 되므로 프로세스를 멈춘다(Windows 문서가 권하는 처리).
        if unsafe { RevertToSelf() } == 0 {
            std::process::abort();
        }
        if opened == 0 {
            return Err(error);
        }
        Self::from_token(Handle(token))
    }

    fn from_token(token: Handle) -> io::Result<Self> {
        let user = token_information(&token, TokenUser)?;
        // SAFETY: 성공했으므로 버퍼 앞머리는 TOKEN_USER이고 SID는 버퍼 안을 가리킨다.
        let sid = unsafe {
            let user = &*user.as_ptr().cast::<TOKEN_USER>();
            sid_string(user.User.Sid)?
        };
        let groups = token_information(&token, TokenGroups)?;
        // 같은 사용자도 토큰마다 그룹이 다르다(예: 관리자의 상승된 토큰과 아닌 토큰). 권한 검사
        // 결과는 그룹에 따라 다르므로 기억할 열쇠에 그룹과 그 상태를 넣는다.
        let mut entries = Vec::new();
        // SAFETY: 성공했으므로 버퍼 앞머리는 TOKEN_GROUPS이고, Groups는 GroupCount개가 버퍼 안에
        // 이어 있으며 SID들도 버퍼 안을 가리킨다.
        unsafe {
            let groups = groups.as_ptr().cast::<TOKEN_GROUPS>();
            let count = (*groups).GroupCount as usize;
            let first = (&raw const (*groups).Groups).cast::<SID_AND_ATTRIBUTES>();
            for i in 0..count {
                let group = &*first.add(i);
                let state = group.Attributes & (SE_GROUP_ENABLED | SE_GROUP_USE_FOR_DENY_ONLY);
                entries.push(format!("{}:{state:x}", sid_string(group.Sid)?));
            }
        }
        entries.sort();
        let mut hasher = DefaultHasher::new();
        entries.hash(&mut hasher);
        let key = format!("{sid}#{:016x}", hasher.finish());
        Ok(Self { token, sid, key })
    }

    /// 사용자 SID(예: `S-1-5-21-...`).
    pub fn sid(&self) -> &str {
        &self.sid
    }

    /// 이 사용자가 폴더 `folder`의 목록을 읽을 수 있는가(`FILE_LIST_DIRECTORY`).
    pub fn check_list(&self, folder: &str) -> io::Result<bool> {
        let path = wide(&long_path(folder));
        let info =
            OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
        let mut needed = 0u32;
        // SAFETY: 크기만 묻는 호출이다(버퍼 없음, 길이 0). path는 NUL로 끝난다.
        let ok = unsafe { GetFileSecurityW(path.as_ptr(), info, null_mut(), 0, &mut needed) };
        if ok == 0 {
            // SAFETY: 바로 앞 호출의 오류 코드를 읽는다.
            let code = unsafe { GetLastError() };
            if code != ERROR_INSUFFICIENT_BUFFER {
                return Err(io::Error::from_raw_os_error(code as i32));
            }
        }
        // 보안 설명자는 포인터 크기 정렬을 바라므로 u64로 잡는다.
        let mut sd = vec![0u64; (needed as usize).div_ceil(8).max(1)];
        // SAFETY: sd는 needed바이트 이상이고 그 크기를 함께 넘긴다.
        if unsafe {
            GetFileSecurityW(
                path.as_ptr(),
                info,
                sd.as_mut_ptr().cast(),
                (sd.len() * 8) as u32,
                &mut needed,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mapping = GENERIC_MAPPING {
            GenericRead: FILE_GENERIC_READ,
            GenericWrite: FILE_GENERIC_WRITE,
            GenericExecute: FILE_GENERIC_EXECUTE,
            GenericAll: FILE_ALL_ACCESS,
        };
        let mut privileges = PRIVILEGE_SET::default();
        let mut privileges_len = size_of::<PRIVILEGE_SET>() as u32;
        let mut granted = 0u32;
        let mut status = 0;
        // SAFETY: sd는 GetFileSecurityW가 채운 자기 완결 보안 설명자이고, token은 가장 수준 토큰이다.
        // 나머지는 쓸 수 있는 지역 변수와 그 크기다.
        let ok = unsafe {
            AccessCheck(
                sd.as_mut_ptr().cast(),
                self.token.0,
                FILE_LIST_DIRECTORY,
                &mapping,
                &mut privileges,
                &mut privileges_len,
                &mut granted,
                &mut status,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(status != 0)
    }
}

impl Caller for ClientToken {
    fn user(&self) -> Option<String> {
        Some(self.key.clone())
    }

    fn can_list(&self, folder: &str) -> bool {
        self.check_list(folder).unwrap_or(false)
    }
}

/// 파이프 연결 하나의 상대. 처음 물을 때 클라이언트 토큰을 잡는다(그 전에 클라이언트가 버전
/// 인사를 보내 서버가 읽었어야 한다). 잡지 못하면 아무 폴더도 읽을 수 없는 것으로 친다.
pub struct PipeCaller {
    pipe: Arc<Handle>,
    token: OnceLock<Option<ClientToken>>,
}

impl PipeCaller {
    pub(crate) fn new(pipe: Arc<Handle>) -> Self {
        Self {
            pipe,
            token: OnceLock::new(),
        }
    }

    fn token(&self) -> Option<&ClientToken> {
        self.token
            .get_or_init(|| match ClientToken::of_pipe_client(self.pipe.0) {
                Ok(t) => Some(t),
                Err(e) => {
                    eprintln!(
                        "파이프 클라이언트의 토큰을 잡지 못했습니다(결과를 모두 거릅니다): {e}"
                    );
                    None
                }
            })
            .as_ref()
    }
}

impl Caller for PipeCaller {
    fn user(&self) -> Option<String> {
        self.token().map(|t| t.key.clone())
    }

    fn can_list(&self, folder: &str) -> bool {
        self.token().is_some_and(|t| t.can_list(folder))
    }
}
