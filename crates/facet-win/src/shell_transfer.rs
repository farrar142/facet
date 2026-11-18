//! 셸 항목(휴대폰처럼 실제 파일 시스템 경로가 없는 항목)과 PC 사이의 파일 전송(S12, spec #27의 휴대폰 복사·이동·삭제).
//! `unsafe`는 `windows_shell_transfer.rs`에 있다.
//!
//! 셸 항목은 [`crate::shell_folder`]처럼 셸 항목 식별자(PIDL) 바이트로 가리킨다. 함수는 부른 스레드에서 셸을 부르고
//! 기다리므로 COM STA 스레드([`crate::shell_thread::ShellThread`])에서만 부른다. UI 스레드에서 부르지 않는다.
//!
//! - 가져오기([`read_to_file`]): 셸 항목의 내용을 스트림(`BHID_Stream`)으로 덩어리씩 읽어 PC의 새 파일에 쓴다. 덩어리마다
//!   읽은 바이트를 콜백에 알리고, 콜백이 `false`면 그만두고 만들다 만 파일을 지운다(취소).
//! - 넣기([`copy_into`]): PC 파일을 셸 폴더 안으로 셸의 `IFileOperation`(대화 없음, `FOF_NO_UI`)으로 복사한다. 같은 이름이
//!   있으면 셸이 바꿔치기한다(덮어쓰기: 휴대폰에는 휴지통이 없어 되살릴 수 없다). 묻는 것은 부르는 쪽이 먼저 한다.
//! - 폴더 만들기([`new_folder`])·영구 삭제([`delete`]): 같은 `IFileOperation`이다. 삭제는 휴지통을 거치지 않는다.
//! - 폴더 안 항목([`children`]): 파일 이름(확장자 포함, `PKEY_FileName`)·폴더 여부·크기·수정한 날짜.
//!
//! 실패는 [`FileOpError`](종류와 원래 코드)다. 화면 문장은 없다. Windows가 아니면 「지원하지 않음」이다.

use std::path::Path;

use crate::file_ops::FileOpResult;

/// 셸 폴더 안의 항목 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellChild {
    /// 파일 이름(확장자 포함). 셸 속성 `PKEY_FileName`, 없으면 표시 이름.
    pub name: String,
    /// 폴더인가.
    pub is_folder: bool,
    /// 이 항목의 절대 PIDL 바이트.
    pub id: Vec<u8>,
    /// 크기(바이트). 장치가 주지 않으면 없다.
    pub size: Option<u64>,
    /// 수정한 날짜(`FILETIME`). 장치가 주지 않으면 없다.
    pub modified: Option<u64>,
}

/// 가져오기의 덩어리 크기. 휴대폰(MTP)의 전송 단위보다 넉넉히 커 호출 수가 적고, 진행 알림(0.1초 간격)보다 자주 돈다.
pub const READ_CHUNK: usize = 1024 * 1024;

/// 셸 항목 `id`의 파일 이름(확장자 포함). COM STA 스레드에서 부른다.
#[cfg(windows)]
pub fn file_name(id: &[u8]) -> FileOpResult<String> {
    crate::windows_shell_transfer::file_name(id)
}

/// Windows가 아니면 늘 「지원하지 않음」이다.
#[cfg(not(windows))]
pub fn file_name(_id: &[u8]) -> FileOpResult<String> {
    Err(unsupported())
}

/// 셸 폴더 `id` 안의 항목들. COM STA 스레드에서 부른다.
#[cfg(windows)]
pub fn children(id: &[u8]) -> FileOpResult<Vec<ShellChild>> {
    crate::windows_shell_transfer::children(id)
}

/// Windows가 아니면 늘 「지원하지 않음」이다.
#[cfg(not(windows))]
pub fn children(_id: &[u8]) -> FileOpResult<Vec<ShellChild>> {
    Err(unsupported())
}

/// 셸 항목 `id`의 내용을 `target`(없는 이름의 새 파일)에 쓴다. 덩어리마다 지금까지 쓴 바이트로 `on_progress`를 부르고,
/// `false`면 그만두고 만들다 만 파일을 지운 뒤 [`crate::file_ops::FileOpErrorKind::Cancelled`]다. 실패해도 만들다 만
/// 파일을 지운다. `target`이 이미 있으면 [`crate::file_ops::FileOpErrorKind::TargetExists`]이고 건드리지 않는다.
/// COM STA 스레드에서 부른다.
#[cfg(windows)]
pub fn read_to_file(
    id: &[u8],
    target: &Path,
    on_progress: &mut dyn FnMut(u64) -> bool,
) -> FileOpResult<u64> {
    crate::windows_shell_transfer::read_to_file(id, target, on_progress)
}

/// Windows가 아니면 늘 「지원하지 않음」이다.
#[cfg(not(windows))]
pub fn read_to_file(
    _id: &[u8],
    _target: &Path,
    _on_progress: &mut dyn FnMut(u64) -> bool,
) -> FileOpResult<u64> {
    Err(unsupported())
}

/// PC 파일 `source`를 셸 폴더 `folder` 안으로 `name`(없으면 원래 이름)으로 복사한다. 같은 이름이 있으면 바꿔치기한다.
/// COM STA 스레드에서 부른다.
#[cfg(windows)]
pub fn copy_into(source: &Path, folder: &[u8], name: Option<&str>) -> FileOpResult<()> {
    crate::windows_shell_transfer::copy_into(source, folder, name)
}

/// Windows가 아니면 늘 「지원하지 않음」이다.
#[cfg(not(windows))]
pub fn copy_into(_source: &Path, _folder: &[u8], _name: Option<&str>) -> FileOpResult<()> {
    Err(unsupported())
}

/// 셸 폴더 `folder` 안에 폴더 `name`을 만든다. COM STA 스레드에서 부른다.
#[cfg(windows)]
pub fn new_folder(folder: &[u8], name: &str) -> FileOpResult<()> {
    crate::windows_shell_transfer::new_folder(folder, name)
}

/// Windows가 아니면 늘 「지원하지 않음」이다.
#[cfg(not(windows))]
pub fn new_folder(_folder: &[u8], _name: &str) -> FileOpResult<()> {
    Err(unsupported())
}

/// 셸 항목 `id`(폴더면 안까지)를 휴지통을 거치지 않고 지운다. COM STA 스레드에서 부른다.
#[cfg(windows)]
pub fn delete(id: &[u8]) -> FileOpResult<()> {
    crate::windows_shell_transfer::delete(id)
}

/// Windows가 아니면 늘 「지원하지 않음」이다.
#[cfg(not(windows))]
pub fn delete(_id: &[u8]) -> FileOpResult<()> {
    Err(unsupported())
}

#[cfg(not(windows))]
fn unsupported() -> crate::file_ops::FileOpError {
    crate::file_ops::FileOpError::new(crate::file_ops::FileOpErrorKind::Unsupported, None)
}
