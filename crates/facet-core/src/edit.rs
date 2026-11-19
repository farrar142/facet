//! 그 자리 이름바꾸기와 새 폴더: 작업 대기열을 거치지 않고 일꾼 스레드에서 곧바로 하는 파일 연산.
//!
//! 앱 상태([`crate::window::App::rename`]·[`crate::window::App::new_folder`])가 실행 전에 막을 것(쓸 수 없는 이름,
//! 같은 폴더의 같은 이름)을 막고, 이 모듈의 [`rename`]·[`new_folder`]를 일꾼 스레드에서 부른다. 결과
//! ([`Edited`] 또는 [`EditError`])는 읽기 결과처럼 앱의 깨우기·받기 흐름으로 돌아온다
//! ([`crate::window::Update::Edited`]).
//!
//! 결과에는 작업 기록이 되돌릴 때 쓸 원래 경로·새 경로와 그때의 크기·수정 시각을 담는다. 화면 문장은 만들지 않는다
//! (ADR-0007): 실패는 까닭의 종류만 돌려준다.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use facet_win::file_ops::{self, FileOpErrorKind};

use crate::name_input::{NameError, validate_name};
use crate::{FileSystemProvider, Item, Location, Provider};

/// 새 폴더 이름에 붙이는 번호의 상한. 「바탕 (2)」부터 이 번호까지 모두 있으면 [`EditError::NameTaken`]이다.
pub const NEW_FOLDER_NUMBER_LIMIT: u32 = 10_000;

/// 무엇을 했는가.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    /// 항목 하나의 이름을 바꿨다.
    Rename,
    /// 폴더를 새로 만들었다.
    NewFolder,
}

/// 끝난 이름바꾸기·새 폴더. 다음에 작업 기록이 이것을 남긴다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edited {
    /// 무엇을 했는가.
    pub kind: EditKind,
    /// 원래 경로. 이름바꾸기만 있고 새 폴더는 `None`이다.
    pub original: Option<PathBuf>,
    /// 새 경로: 바꾼 이름의 경로, 만든 폴더의 경로.
    pub path: PathBuf,
    /// 그 항목이 폴더인가.
    pub is_folder: bool,
    /// 끝난 직후의 크기(바이트, 폴더는 파일 시스템이 주는 값).
    pub size: u64,
    /// 끝난 직후의 수정 시각. 다시 읽지 못했으면(그 사이 사라짐) [`SystemTime::UNIX_EPOCH`]이다.
    pub modified: SystemTime,
    /// 끝난 직후 다시 읽은 항목(속성까지). 탭 목록에 곧바로 넣는 데 쓴다.
    pub(crate) item: Option<Item>,
}

impl Edited {
    /// 원래 이름(이름바꾸기).
    pub fn original_name(&self) -> Option<&OsStr> {
        self.original.as_deref().and_then(Path::file_name)
    }

    /// 새 이름.
    pub fn name(&self) -> Option<&OsStr> {
        self.path.file_name()
    }
}

/// 이름바꾸기·새 폴더를 하지 못한 까닭. 화면 문장은 앱이 만든다(ADR-0007).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditError {
    /// 지금 할 수 없다: 활성 탭의 위치가 실제 폴더가 아니거나, 바꿀 항목이 목록에 없다.
    Unavailable,
    /// 새 이름이 원래 이름과 똑같다(대소문자까지). 아무것도 하지 않았다.
    Unchanged,
    /// 쓸 수 없는 이름이다([`validate_name`]).
    InvalidName(NameError),
    /// 같은 폴더에 같은 이름(대소문자 무시)의 다른 항목이 있다. 실행 전에 목록에서 보았거나, 그 사이 생겨 파일
    /// 연산이 「대상 있음」으로 실패했다.
    NameTaken,
    /// 파일 연산이 실패했다: 권한 없음([`FileOpErrorKind::AccessDenied`]), 원본 사라짐
    /// ([`FileOpErrorKind::SourceMissing`]), 잠김 등.
    File(FileOpErrorKind),
}

/// `folder` 안의 `original` 항목 이름을 `new_name`으로 바꾼다(같은 볼륨 이름 바꾸기, [`file_ops::rename`]).
/// 대소문자만 다른 이름이면 그 항목의 대소문자를 바꾼다. 막는 확인(이름·같은 이름)은 부르는 쪽이 이미 했다고 보고
/// 이름만 한 번 더 본다. 막는 일이라 일꾼 스레드에서 부른다. 260자를 넘는 경로도 된다.
pub fn rename(folder: &Path, original: &OsStr, new_name: &str) -> Result<Edited, EditError> {
    validate_name(new_name).map_err(EditError::InvalidName)?;
    if original == OsStr::new(new_name) {
        return Err(EditError::Unchanged);
    }
    let from = folder.join(original);
    let to = folder.join(new_name);
    // 도우미의 미리보기 처리기가 잡고 있으면 내리고 한 번 다시 시도한다(#342).
    crate::shell_helper::retry_held(&from, || file_ops::rename(&from, &to))
        .map_err(|error| file_error(error.kind()))?;
    let item = read_item(folder, OsStr::new(new_name));
    Ok(edited(EditKind::Rename, Some(from), to, item))
}

/// `folder` 안에 폴더를 만든다. 이름은 `base`이고, 같은 이름(파일이든 폴더든)이 있으면 「`base` (2)」, 「(3)」 …
/// 처음 비어 있는 번호다. 그 사이 다른 프로그램이 같은 이름을 만들어도 다음 번호로 넘어간다. 막는 일이라 일꾼
/// 스레드에서 부른다.
pub fn new_folder(folder: &Path, base: &str) -> Result<Edited, EditError> {
    validate_name(base).map_err(EditError::InvalidName)?;
    for number in 1..=NEW_FOLDER_NUMBER_LIMIT {
        let name = numbered_name(base, number);
        validate_name(&name).map_err(EditError::InvalidName)?;
        let path = folder.join(&name);
        match file_ops::create_folder(&path) {
            Ok(()) => {
                let item = read_item(folder, OsStr::new(&name));
                return Ok(edited(EditKind::NewFolder, None, path, item));
            }
            Err(error) if error.kind() == FileOpErrorKind::TargetExists => {}
            Err(error) => return Err(EditError::File(error.kind())),
        }
    }
    Err(EditError::NameTaken)
}

/// 새 폴더의 `number`번째 이름: 1이면 `base`, 아니면 「`base` (n)」.
pub fn numbered_name(base: &str, number: u32) -> String {
    if number <= 1 {
        base.to_string()
    } else {
        format!("{base} ({number})")
    }
}

/// 같은 폴더의 다른 항목 이름 가운데 `new_name`과 대소문자 없이 같은 것이 있는가. `original`(바꿀 항목 자신)은
/// 빼므로 대소문자만 바꾸는 이름은 걸리지 않는다.
pub(crate) fn name_taken<'a>(
    names: impl IntoIterator<Item = &'a OsStr>,
    original: &OsStr,
    new_name: &str,
) -> bool {
    let wanted = new_name.to_lowercase();
    names
        .into_iter()
        .filter(|name| *name != original)
        .any(|name| name.to_string_lossy().to_lowercase() == wanted)
}

fn file_error(kind: FileOpErrorKind) -> EditError {
    match kind {
        FileOpErrorKind::TargetExists => EditError::NameTaken,
        kind => EditError::File(kind),
    }
}

/// 끝난 항목을 다시 읽는다. 그 사이 사라졌으면 `None`이다.
fn read_item(folder: &Path, name: &OsStr) -> Option<Item> {
    FileSystemProvider
        .item(&Location::file_system(folder), name)
        .ok()
        .flatten()
}

/// #429: 방금 만든 새 zip(파일) `path`를 다시 읽어 목록에 넣고 선택할 결과로 만든다(새 폴더와 같은 모양).
pub(crate) fn created_file(path: &Path) -> Option<Edited> {
    let folder = path.parent()?;
    let item = read_item(folder, path.file_name()?)?;
    Some(edited(
        EditKind::NewFolder,
        None,
        path.to_path_buf(),
        Some(item),
    ))
}

fn edited(kind: EditKind, original: Option<PathBuf>, path: PathBuf, item: Option<Item>) -> Edited {
    let (is_folder, size, modified) = match &item {
        Some(item) => (item.is_folder, item.size, item.modified),
        None => (kind == EditKind::NewFolder, 0, SystemTime::UNIX_EPOCH),
    };
    Edited {
        kind,
        original,
        path,
        is_folder,
        size,
        modified,
        item,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered_names() {
        assert_eq!(numbered_name("새 폴더", 1), "새 폴더");
        assert_eq!(numbered_name("새 폴더", 2), "새 폴더 (2)");
        assert_eq!(numbered_name("새 폴더", 10), "새 폴더 (10)");
    }

    #[test]
    fn name_taken_ignores_case_and_the_item_itself() {
        let names = [OsStr::new("readme.txt"), OsStr::new("b.txt")];
        assert!(!name_taken(names, OsStr::new("readme.txt"), "README.txt"));
        assert!(name_taken(names, OsStr::new("readme.txt"), "B.TXT"));
        assert!(!name_taken(names, OsStr::new("readme.txt"), "c.txt"));
    }
}
