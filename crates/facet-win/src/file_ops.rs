//! 복사·이동 엔진이 쓰는 파일 하나·이름 하나 단위의 기본 연산(ADR-0005).
//!
//! 엔진(작업 대기열·훑기·이름 충돌·오류 처리)은 코어가 만들고, 여기는 안전한 함수만 드러낸다.
//!
//! - [`copy_file`]: 파일 하나의 바이트 복사(`CopyFile2`). 진행 콜백이 복사한 바이트를 알리고 계속·멈춤·취소를 정한다.
//!   수정 시각·만든 시각·속성·대체 데이터 스트림을 보존하고, 보안 설명자는 복사하지 않는다(대상 폴더의 상속 권한).
//! - [`create_folder`], [`copy_folder_metadata`]: 폴더 만들기와 폴더 시각·속성 맞추기(안의 항목을 다 쓴 뒤 부른다).
//! - [`rename`]: 같은 볼륨 안 이름 바꾸기(이동). 복사로 바꾸지 않고, 다른 볼륨이면 [`FileOpErrorKind::CrossVolume`].
//! - [`replace`]: 덮어쓰기용 바꿔치기(임시 이름 파일 → 기존 이름, 읽기 전용 대상 포함).
//! - [`remove_file`], [`remove_empty_folder`]: 파일·빈 폴더 지우기(읽기 전용도).
//! - [`file_id`]: 파일·폴더의 파일 ID(볼륨 일련 번호 포함).
//! - [`read_link`], [`create_link`], [`remove_link`]: 심볼릭 링크(파일·폴더)와 정션 자체를 읽고 만들고 지운다
//!   (따라 들어가지 않는 복사가 링크 자체를 복사할 때).
//!
//! 경로는 260자를 넘어도 된다. 안에서 확장 길이 경로 접두사(`\\?\`)를 붙인다. 실패는 [`FileOpError`]이고 종류
//! [`FileOpErrorKind`]로 이유를 나눈다. 화면 문장은 담지 않는다(ADR-0007). Windows가 아니면 모든 연산이
//! [`FileOpErrorKind::Unsupported`]다.

use std::fmt;
use std::path::{Path, PathBuf};

/// 진행 콜백이 받는 값: 지금까지 복사한 바이트와 전체 바이트(대체 데이터 스트림 포함).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyProgress {
    /// 지금까지 복사한 바이트. 콜백마다 줄지 않는다.
    pub copied: u64,
    /// 전체 바이트.
    pub total: u64,
}

/// 진행 콜백이 돌려주는 다음 동작.
///
/// 「멈춤」은 따로 값이 없다: 콜백이 돌아오지 않고 그 자리에서 기다리면 복사가 그 자리(파일 경계가 아닌 덩어리
/// 사이)에서 멈추고, 콜백이 돌아오면 이어서 복사한다. 멈춘 동안 취소하려면 기다림을 풀고 [`CopyStep::Cancel`]을
/// 돌려준다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyStep {
    /// 계속 복사한다.
    Continue,
    /// 복사를 그만두고 만들다 만 대상 파일을 지운다. 결과는 [`FileOpErrorKind::Cancelled`].
    Cancel,
}

/// 실패의 종류. 엔진이 이유를 보이고 다시 시도할지 정하는 데 쓴다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileOpErrorKind {
    /// 다른 프로그램이 쓰고 있다(공유 위반·잠금 위반). 잠시 뒤 다시 시도할 수 있다.
    Locked,
    /// 접근 거부(권한, 또는 지울 수 없는 상태).
    AccessDenied,
    /// 디스크가 가득 찼다.
    DiskFull,
    /// 원본이 없다.
    SourceMissing,
    /// 대상이 이미 있다.
    TargetExists,
    /// 원본과 대상이 다른 볼륨이라 이름 바꾸기로 옮길 수 없다.
    CrossVolume,
    /// 진행 콜백이 취소했다.
    Cancelled,
    /// 이 환경(Windows가 아닌 곳)에서는 할 수 없다.
    Unsupported,
    /// 그 밖의 오류. 원래 코드는 [`FileOpError::code`]에 있다.
    Other,
}

/// 파일 연산의 실패: 종류와 운영체제가 돌려준 원래 코드.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileOpError {
    kind: FileOpErrorKind,
    code: Option<i32>,
}

impl FileOpError {
    /// 종류와 원래 코드로 만든다.
    pub fn new(kind: FileOpErrorKind, code: Option<i32>) -> Self {
        FileOpError { kind, code }
    }

    /// 종류.
    pub fn kind(&self) -> FileOpErrorKind {
        self.kind
    }

    /// 운영체제가 돌려준 원래 코드. Win32 오류 코드이고, Win32 코드로 풀 수 없는 HRESULT면 HRESULT 그대로다.
    /// 취소·지원하지 않음처럼 운영체제 오류가 아니면 `None`.
    pub fn code(&self) -> Option<i32> {
        self.code
    }

    /// Win32 오류 코드를 종류로 나눈다(「원본 없음」은 코드만으로 원본·대상을 가를 수 없어 부르는 쪽이 정한다).
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn from_win32(code: u32) -> Self {
        FileOpError::new(classify_win32(code), Some(code as i32))
    }

    #[cfg(not(windows))]
    pub(crate) fn unsupported() -> Self {
        FileOpError::new(FileOpErrorKind::Unsupported, None)
    }
}

impl fmt::Display for FileOpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 화면 문장이 아니라 기록용 표시다(ADR-0007). 화면 문장은 앱이 종류로 고른다.
        match self.code {
            Some(code) => write!(f, "{:?} (os error {code})", self.kind),
            None => write!(f, "{:?}", self.kind),
        }
    }
}

impl std::error::Error for FileOpError {}

/// 파일 연산의 결과.
pub type FileOpResult<T> = Result<T, FileOpError>;

pub(crate) const ERROR_FILE_NOT_FOUND: u32 = 2;
pub(crate) const ERROR_PATH_NOT_FOUND: u32 = 3;
pub(crate) const ERROR_ACCESS_DENIED: u32 = 5;
pub(crate) const ERROR_NOT_SAME_DEVICE: u32 = 17;
pub(crate) const ERROR_SHARING_VIOLATION: u32 = 32;
pub(crate) const ERROR_LOCK_VIOLATION: u32 = 33;
pub(crate) const ERROR_HANDLE_DISK_FULL: u32 = 39;
pub(crate) const ERROR_FILE_EXISTS: u32 = 80;
pub(crate) const ERROR_DISK_FULL: u32 = 112;
pub(crate) const ERROR_ALREADY_EXISTS: u32 = 183;
pub(crate) const ERROR_PRIVILEGE_NOT_HELD: u32 = 1314;

/// Win32 오류 코드의 종류. 「찾을 수 없음」은 여기서 [`FileOpErrorKind::SourceMissing`]으로 두고, 원본이 실제로
/// 있으면 부르는 쪽이 [`FileOpErrorKind::Other`]로 바꾼다(대상 폴더가 없을 때).
pub(crate) fn classify_win32(code: u32) -> FileOpErrorKind {
    match code {
        ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION => FileOpErrorKind::Locked,
        // 심볼릭 링크를 만들 권한이 없다(관리자도 개발자 모드도 아님): 사용자에게는 접근 거부다.
        ERROR_ACCESS_DENIED | ERROR_PRIVILEGE_NOT_HELD => FileOpErrorKind::AccessDenied,
        ERROR_DISK_FULL | ERROR_HANDLE_DISK_FULL => FileOpErrorKind::DiskFull,
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => FileOpErrorKind::SourceMissing,
        ERROR_FILE_EXISTS | ERROR_ALREADY_EXISTS => FileOpErrorKind::TargetExists,
        ERROR_NOT_SAME_DEVICE => FileOpErrorKind::CrossVolume,
        _ => FileOpErrorKind::Other,
    }
}

/// HRESULT가 Win32 오류(`FACILITY_WIN32`, `0x8007xxxx`)면 그 코드.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn win32_from_hresult(hresult: i32) -> Option<u32> {
    let value = hresult as u32;
    (value & 0xFFFF_0000 == 0x8007_0000).then_some(value & 0xFFFF)
}

/// 파일·폴더의 파일 ID: 볼륨 일련 번호와 그 볼륨 안의 128비트 ID. 같은 볼륨에서 이름을 바꿔도 그대로다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId {
    /// 볼륨 일련 번호.
    pub volume_serial: u64,
    /// 볼륨 안의 파일 ID.
    pub file_id: [u8; 16],
}

/// 이 크기(바이트) 이상인 파일은 무버퍼 입출력(`COPY_FILE_NO_BUFFERING`)으로 복사한다.
///
/// 근거: Microsoft는 `COPY_FILE_NO_BUFFERING`을 「매우 큰 파일」에 권하고(CopyFile2 문서), 무버퍼 복사는 운영체제
/// 파일 캐시를 거치지 않아 큰 파일 하나가 캐시의 다른 내용(방금 연 폴더 목록 등)을 밀어내지 않는다. 대신 캐시의
/// 미리 읽기·쓰기 모으기를 잃어 작은 파일에서는 오히려 느리다. robocopy `/J`·탐색기도 수백 MB 이상에서만 무버퍼가
/// 이득이라는 것이 일반적인 관찰이라, 흔한 문서·사진(수십 MB 이하)은 버퍼를 쓰고 영상·디스크 이미지 같은 큰 파일만
/// 무버퍼로 가도록 256MiB로 둔다.
pub const UNBUFFERED_COPY_THRESHOLD: u64 = 256 * 1024 * 1024;

/// `source` 파일 하나를 `target` 경로로 복사한다. `target`이 이미 있으면 [`FileOpErrorKind::TargetExists`]다
/// (덮어쓰기는 엔진이 임시 이름에 복사한 뒤 [`replace`]로 한다).
///
/// 내용과 함께 수정 시각·만든 시각·속성(읽기 전용·숨김 등)·대체 데이터 스트림(`Zone.Identifier` 포함)을 원본과
/// 같게 한다. 보안 설명자는 복사하지 않아 대상 폴더의 상속 권한을 받는다. [`UNBUFFERED_COPY_THRESHOLD`] 이상인
/// 파일은 무버퍼 입출력을 쓴다.
///
/// `progress`는 덩어리를 하나 쓸 때마다 이 스레드에서 불린다([`CopyStep`]: 계속·취소, 돌아오지 않고 기다리면
/// 멈춤). 취소하거나 실패하면 만들다 만 대상 파일을 남기지 않는다. 콜백이 패닉하면 복사를 취소한 뒤 그 패닉을
/// 이어서 일으킨다.
pub fn copy_file(
    source: &Path,
    target: &Path,
    progress: &mut dyn FnMut(CopyProgress) -> CopyStep,
) -> FileOpResult<()> {
    #[cfg(windows)]
    {
        crate::windows_file_ops::copy_file(source, target, progress)
    }
    #[cfg(not(windows))]
    {
        let _ = (source, target, progress);
        Err(FileOpError::unsupported())
    }
}

/// 폴더 하나를 만든다(부모는 있어야 한다). 이미 있으면 [`FileOpErrorKind::TargetExists`]다.
pub fn create_folder(path: &Path) -> FileOpResult<()> {
    #[cfg(windows)]
    {
        crate::windows_file_ops::create_folder(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(FileOpError::unsupported())
    }
}

/// `target` 폴더의 만든 시각·수정 시각·속성을 `source` 폴더와 같게 한다.
///
/// 안에 항목을 쓰면 폴더의 수정 시각이 바뀌므로 엔진은 안의 항목을 다 쓴 뒤 이것을 부른다.
pub fn copy_folder_metadata(source: &Path, target: &Path) -> FileOpResult<()> {
    #[cfg(windows)]
    {
        crate::windows_file_ops::copy_folder_metadata(source, target)
    }
    #[cfg(not(windows))]
    {
        let _ = (source, target);
        Err(FileOpError::unsupported())
    }
}

/// `target` 폴더의 만든 시각·수정 시각·속성을 미리 읽어 둔 원본 폴더의 `metadata`(`std::fs::metadata`)와 같게
/// 한다.
///
/// 이동 엔진은 원본 폴더 안의 항목을 옮기며 지우므로 원본 폴더의 수정 시각이 바뀐다. 그래서 들어갈 때 읽어 둔
/// 값을 안의 항목을 다 쓴 뒤 이것으로 맞춘다.
pub fn apply_folder_metadata(metadata: &std::fs::Metadata, target: &Path) -> FileOpResult<()> {
    #[cfg(windows)]
    {
        crate::windows_file_ops::apply_folder_metadata(metadata, target)
    }
    #[cfg(not(windows))]
    {
        let _ = (metadata, target);
        Err(FileOpError::unsupported())
    }
}

/// 파일·폴더를 같은 볼륨 안의 `target`으로 옮긴다(이름 바꾸기). 복사로 바꾸지 않는다.
///
/// 다른 볼륨이면 [`FileOpErrorKind::CrossVolume`], 대상이 있으면 [`FileOpErrorKind::TargetExists`]다.
pub fn rename(source: &Path, target: &Path) -> FileOpResult<()> {
    #[cfg(windows)]
    {
        crate::windows_file_ops::rename(source, target)
    }
    #[cfg(not(windows))]
    {
        let _ = (source, target);
        Err(FileOpError::unsupported())
    }
}

/// 덮어쓰기용 바꿔치기: `replacement` 파일(보통 임시 이름에 복사한 것)을 기존 파일 `target` 자리로 옮긴다.
///
/// `target`이 읽기 전용이어도 덮어쓴다. 결과 파일의 내용·시각·속성은 `replacement`의 것이다. 같은 볼륨에서만 하고,
/// 다른 볼륨이면 [`FileOpErrorKind::CrossVolume`]이다. 실패하면 `target`은 그대로다(읽기 전용도 되돌린다).
pub fn replace(replacement: &Path, target: &Path) -> FileOpResult<()> {
    #[cfg(windows)]
    {
        crate::windows_file_ops::replace(replacement, target)
    }
    #[cfg(not(windows))]
    {
        let _ = (replacement, target);
        Err(FileOpError::unsupported())
    }
}

/// 파일 하나를 지운다(휴지통이 아니라 바로). 읽기 전용이어도 지운다.
pub fn remove_file(path: &Path) -> FileOpResult<()> {
    #[cfg(windows)]
    {
        crate::windows_file_ops::remove_file(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(FileOpError::unsupported())
    }
}

/// 빈 폴더 하나를 지운다. 읽기 전용이어도 지운다. 비어 있지 않으면 [`FileOpErrorKind::Other`]다.
pub fn remove_empty_folder(path: &Path) -> FileOpResult<()> {
    #[cfg(windows)]
    {
        crate::windows_file_ops::remove_empty_folder(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(FileOpError::unsupported())
    }
}

/// 파일·폴더의 파일 ID(볼륨 일련 번호 포함).
pub fn file_id(path: &Path) -> FileOpResult<FileId> {
    #[cfg(windows)]
    {
        crate::windows_file_ops::file_id(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(FileOpError::unsupported())
    }
}

/// 링크의 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkKind {
    /// 파일 심볼릭 링크.
    FileSymlink,
    /// 폴더 심볼릭 링크.
    FolderSymlink,
    /// 정션(디렉터리 마운트 지점). 늘 절대 경로를 가리킨다.
    Junction,
}

impl LinkKind {
    /// 폴더처럼 보이는 링크인지(폴더 심볼릭 링크·정션). 링크 자체가 디렉터리 항목이라 지울 때도 폴더로 지운다.
    pub fn is_folder(self) -> bool {
        !matches!(self, LinkKind::FileSymlink)
    }
}

/// 링크 하나: 종류와 가리키는 곳.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Link {
    /// 종류.
    pub kind: LinkKind,
    /// 가리키는 곳. 절대 경로면 Win32 경로(`C:\...`, `\\server\share\...`, 드라이브 문자가 없는 볼륨은
    /// `\\?\Volume{...}\...`)이고, 상대 심볼릭 링크면 적힌 그대로의 상대 경로다(링크가 있는 폴더 기준).
    pub target: PathBuf,
    /// 상대 경로 심볼릭 링크인지. 복사하면 상대 경로 그대로 만든다(복사본 자리 기준으로 가리킨다).
    pub relative: bool,
}

/// `path`의 링크 자체(따라가지 않는다)를 읽는다: 심볼릭 링크(파일·폴더)와 정션.
///
/// 링크가 아니거나 다른 종류의 재분석 지점이면 [`FileOpErrorKind::Other`]다.
pub fn read_link(path: &Path) -> FileOpResult<Link> {
    #[cfg(windows)]
    {
        crate::windows_file_ops::read_link(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(FileOpError::unsupported())
    }
}

/// `path`(없는 이름)에 `link`와 같은 종류·가리키는 곳의 링크를 만든다. 가리키는 곳은 없어도 된다.
///
/// 이미 있으면 [`FileOpErrorKind::TargetExists`]다. 심볼릭 링크를 만들 권한이 없으면(관리자도 개발자 모드도
/// 아님, `ERROR_PRIVILEGE_NOT_HELD`) [`FileOpErrorKind::AccessDenied`]다. 정션은 권한 없이 만들 수 있고, 만들다
/// 실패하면 만든 빈 폴더를 지운다. 링크의 시각·속성은 원본 링크를 따르지 않는다(만든 때).
pub fn create_link(path: &Path, link: &Link) -> FileOpResult<()> {
    #[cfg(windows)]
    {
        crate::windows_file_ops::create_link(path, link)
    }
    #[cfg(not(windows))]
    {
        let _ = (path, link);
        Err(FileOpError::unsupported())
    }
}

/// 링크 자체를 지운다(가리키는 곳은 건드리지 않는다). 폴더 링크·정션도, 읽기 전용이어도 지운다.
pub fn remove_link(path: &Path) -> FileOpResult<()> {
    #[cfg(windows)]
    {
        crate::windows_file_ops::remove_link(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(FileOpError::unsupported())
    }
}

/// 재분석 지점 태그: 심볼릭 링크.
pub(crate) const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;
/// 재분석 지점 태그: 정션(마운트 지점).
pub(crate) const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
/// 심볼릭 링크 재분석 데이터의 「상대 경로」 표시.
const SYMLINK_FLAG_RELATIVE: u32 = 1;
/// 재분석 데이터 버퍼의 최대 크기(`MAXIMUM_REPARSE_DATA_BUFFER_SIZE`).
pub(crate) const MAX_REPARSE_BUFFER: usize = 16 * 1024;

/// 재분석 데이터 버퍼(`REPARSE_DATA_BUFFER`)에서 읽은 링크. 버퍼는 Windows가 준 바이트 그대로이고, 종류(파일·
/// 폴더 심볼릭 링크)는 버퍼에 없어 부르는 쪽이 디렉터리 속성으로 정한다.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn parse_reparse(buffer: &[u8], is_folder: bool) -> Option<Link> {
    let u16_at = |at: usize| -> Option<u16> {
        Some(u16::from_le_bytes(buffer.get(at..at + 2)?.try_into().ok()?))
    };
    let u32_at = |at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(buffer.get(at..at + 4)?.try_into().ok()?))
    };
    let tag = u32_at(0)?;
    let (kind, path_start, flags) = match tag {
        IO_REPARSE_TAG_SYMLINK => {
            let kind = if is_folder {
                LinkKind::FolderSymlink
            } else {
                LinkKind::FileSymlink
            };
            (kind, 20, u32_at(16)?)
        }
        IO_REPARSE_TAG_MOUNT_POINT => (LinkKind::Junction, 16, 0),
        _ => return None,
    };
    let offset = usize::from(u16_at(8)?);
    let length = usize::from(u16_at(10)?);
    let bytes = buffer.get(path_start + offset..path_start + offset + length)?;
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let substitute = String::from_utf16(&units).ok()?;
    let relative = kind != LinkKind::Junction && flags & SYMLINK_FLAG_RELATIVE != 0;
    let target = if relative {
        PathBuf::from(substitute)
    } else {
        PathBuf::from(win32_from_nt(&substitute))
    };
    Some(Link {
        kind,
        target,
        relative,
    })
}

/// NT 경로(`\??\C:\a`, `\??\UNC\s\h`)를 Win32 경로(`C:\a`, `\\s\h`)로. 드라이브 문자도 UNC도 아니면 `\\?\`를
/// 붙여 그대로 둔다(`\??\Volume{...}\` 등). `\??\`로 시작하지 않으면 그대로.
fn win32_from_nt(path: &str) -> String {
    let Some(rest) = path.strip_prefix(r"\??\") else {
        return path.to_owned();
    };
    let bytes = rest.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        rest.to_owned()
    } else if let Some(unc) = rest.strip_prefix(r"UNC\") {
        format!(r"\\{unc}")
    } else {
        format!(r"\\?\{rest}")
    }
}

/// Win32 절대 경로를 정션의 대체 이름(NT 경로)으로: `C:\a` → `\??\C:\a`, `\\s\h` → `\??\UNC\s\h`,
/// `\\?\X` → `\??\X`. 절대 경로가 아니면 `None`(정션은 상대 경로를 가리킬 수 없다).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn nt_from_win32(path: &str) -> Option<String> {
    if let Some(rest) = path
        .strip_prefix(r"\\?\")
        .or_else(|| path.strip_prefix(r"\??\"))
    {
        return Some(format!(r"\??\{rest}"));
    }
    if let Some(unc) = path.strip_prefix(r"\\") {
        return Some(format!(r"\??\UNC\{unc}"));
    }
    let bytes = path.as_bytes();
    (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\')
        .then(|| format!(r"\??\{path}"))
}

/// 정션의 재분석 데이터 버퍼(`REPARSE_DATA_BUFFER`의 마운트 지점)를 만든다: 대체 이름 `substitute`(NT 경로)와
/// 표시 이름 `print`. 너무 길면 `None`.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn junction_buffer(substitute: &str, print: &str) -> Option<Vec<u8>> {
    let substitute: Vec<u16> = substitute.encode_utf16().collect();
    let print: Vec<u16> = print.encode_utf16().collect();
    // 두 이름 모두 NUL로 끝낸다(탐색기·mklink가 만드는 것과 같다). 길이에는 NUL을 넣지 않는다.
    let substitute_len = substitute.len() * 2;
    let print_len = print.len() * 2;
    let path_bytes = substitute_len + 2 + print_len + 2;
    let data_len = 8 + path_bytes;
    if 8 + data_len > MAX_REPARSE_BUFFER {
        return None;
    }
    let mut buffer = Vec::with_capacity(8 + data_len);
    buffer.extend_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
    buffer.extend_from_slice(&(data_len as u16).to_le_bytes());
    buffer.extend_from_slice(&0u16.to_le_bytes());
    buffer.extend_from_slice(&0u16.to_le_bytes());
    buffer.extend_from_slice(&(substitute_len as u16).to_le_bytes());
    buffer.extend_from_slice(&((substitute_len + 2) as u16).to_le_bytes());
    buffer.extend_from_slice(&(print_len as u16).to_le_bytes());
    for unit in substitute.iter().chain(&[0]).chain(&print).chain(&[0]) {
        buffer.extend_from_slice(&unit.to_le_bytes());
    }
    Some(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn win32_오류를_종류로_나눈다() {
        let cases = [
            (ERROR_SHARING_VIOLATION, FileOpErrorKind::Locked),
            (ERROR_LOCK_VIOLATION, FileOpErrorKind::Locked),
            (ERROR_ACCESS_DENIED, FileOpErrorKind::AccessDenied),
            (ERROR_PRIVILEGE_NOT_HELD, FileOpErrorKind::AccessDenied),
            (ERROR_DISK_FULL, FileOpErrorKind::DiskFull),
            (ERROR_HANDLE_DISK_FULL, FileOpErrorKind::DiskFull),
            (ERROR_FILE_NOT_FOUND, FileOpErrorKind::SourceMissing),
            (ERROR_PATH_NOT_FOUND, FileOpErrorKind::SourceMissing),
            (ERROR_FILE_EXISTS, FileOpErrorKind::TargetExists),
            (ERROR_ALREADY_EXISTS, FileOpErrorKind::TargetExists),
            (ERROR_NOT_SAME_DEVICE, FileOpErrorKind::CrossVolume),
            (87, FileOpErrorKind::Other),
        ];
        for (code, kind) in cases {
            let error = FileOpError::from_win32(code);
            assert_eq!(error.kind(), kind, "{code}");
            assert_eq!(error.code(), Some(code as i32));
        }
    }

    #[test]
    fn 정션_버퍼를_만들고_다시_읽는다() {
        let buffer = junction_buffer(r"\??\C:\가 나\다", r"C:\가 나\다").unwrap();
        let link = parse_reparse(&buffer, true).unwrap();
        assert_eq!(
            link,
            Link {
                kind: LinkKind::Junction,
                target: PathBuf::from(r"C:\가 나\다"),
                relative: false,
            }
        );
        // 머리의 데이터 길이는 머리 8바이트를 뺀 길이다.
        let data_len = u16::from_le_bytes([buffer[4], buffer[5]]) as usize;
        assert_eq!(data_len + 8, buffer.len());
    }

    /// 심볼릭 링크 재분석 버퍼(시험용).
    fn symlink_buffer(substitute: &str, flags: u32) -> Vec<u8> {
        let name: Vec<u8> = substitute
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&IO_REPARSE_TAG_SYMLINK.to_le_bytes());
        buffer.extend_from_slice(&((12 + name.len()) as u16).to_le_bytes());
        buffer.extend_from_slice(&0u16.to_le_bytes());
        buffer.extend_from_slice(&0u16.to_le_bytes());
        buffer.extend_from_slice(&(name.len() as u16).to_le_bytes());
        buffer.extend_from_slice(&0u16.to_le_bytes());
        buffer.extend_from_slice(&0u16.to_le_bytes());
        buffer.extend_from_slice(&flags.to_le_bytes());
        buffer.extend_from_slice(&name);
        buffer
    }

    #[test]
    fn 심볼릭_링크_버퍼를_읽는다() {
        let absolute = parse_reparse(&symlink_buffer(r"\??\UNC\서버\공유", 0), false).unwrap();
        assert_eq!(absolute.kind, LinkKind::FileSymlink);
        assert_eq!(absolute.target, PathBuf::from(r"\\서버\공유"));
        assert!(!absolute.relative);

        let relative =
            parse_reparse(&symlink_buffer(r"..\옆", SYMLINK_FLAG_RELATIVE), true).unwrap();
        assert_eq!(relative.kind, LinkKind::FolderSymlink);
        assert_eq!(relative.target, PathBuf::from(r"..\옆"));
        assert!(relative.relative);

        // 다른 태그나 잘린 버퍼는 링크가 아니다.
        let mut other = symlink_buffer("x", 0);
        other[0] = 0x1B;
        assert_eq!(parse_reparse(&other, false), None);
        let full = symlink_buffer(r"\??\C:\a", 0);
        assert_eq!(parse_reparse(&full[..full.len() - 2], false), None);
    }

    #[test]
    fn nt_경로와_win32_경로를_오간다() {
        for (win32, nt) in [
            (r"C:\a\b", r"\??\C:\a\b"),
            (r"\\서버\공유\a", r"\??\UNC\서버\공유\a"),
            (r"\\?\Volume{1234}\a", r"\??\Volume{1234}\a"),
        ] {
            assert_eq!(nt_from_win32(win32).as_deref(), Some(nt));
            assert_eq!(win32_from_nt(nt), win32);
        }
        assert_eq!(nt_from_win32(r"\\?\C:\a").as_deref(), Some(r"\??\C:\a"));
        assert_eq!(nt_from_win32(r"..\a"), None);
        assert_eq!(nt_from_win32(r"C:a"), None);
    }

    #[test]
    fn hresult에서_win32_코드를_꺼낸다() {
        assert_eq!(win32_from_hresult(0x8007_0020_u32 as i32), Some(32));
        assert_eq!(win32_from_hresult(0x8000_4005_u32 as i32), None);
        assert_eq!(win32_from_hresult(0), None);
    }

    #[test]
    fn 표시는_종류와_코드다() {
        assert_eq!(
            FileOpError::from_win32(32).to_string(),
            "Locked (os error 32)"
        );
        assert_eq!(
            FileOpError::new(FileOpErrorKind::Cancelled, None).to_string(),
            "Cancelled"
        );
    }
}
