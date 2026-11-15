//! 실행 인자 해석: facet을 실행할 때 받은 경로 목록을 새 창에 열 탭들로 바꾼다.
//!
//! 첫 실행의 인자와, 두 번째 실행이 떠 있는 facet에 넘긴 인자(`facet_win::single_instance::Launch`)가 같은
//! 해석을 쓴다([`resolve_launch`]). 경로마다 탭 하나이고(넘긴 순서대로), 경로가 없으면 사용자 홈 폴더 탭 하나다.
//!
//! 경로는 주소창과 같은 규칙으로 해석한다: 앞뒤 공백과 감싼 큰따옴표를 걷고, `%NAME%`을 펼치고, 폴더면 그
//! 폴더, 파일이면 그 파일이 든 폴더로 가서 그 파일을 고른다. 다른 점은 상대 경로를 받는다는 것이다: 상대
//! 경로는 그 실행의 작업 폴더 기준으로 풀고 `..`·`.`을 정리한다. 해석하지 못한 경로도 탭이 되고, 그 탭이
//! 까닭([`crate::tab::Tab::address_error`])을 보인다.
//!
//! 해석은 경로마다 파일 시스템에 한 번 묻는다. 두 번째 실행에서 받은 인자는 UI 스레드가 아닌 수신기 스레드에서
//! 해석한다. 새 창은 [`crate::window::App::open_launch_window`]가 연다.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::file_system::{parse_text, resolve_path};
use crate::window::home_location;
use crate::{Location, ResolveError};

/// 실행 인자 하나를 해석한 결과: 새 창에 열 탭 하나.
#[derive(Debug)]
pub struct LaunchTarget {
    /// 탭이 볼 위치. 해석하지 못했으면 인자가 가리키던 경로(작업 폴더 기준으로 푼 것)다.
    pub location: Location,
    /// 파일 경로를 받았으면 도착해서 고를 그 파일의 이름.
    pub select: Option<OsString>,
    /// 해석하지 못했으면 그 까닭. 탭은 읽지 않고 이 까닭을 보인다.
    pub error: Option<ResolveError>,
}

impl LaunchTarget {
    /// 사용자 홈 폴더 탭(인자가 없을 때).
    pub fn home() -> Self {
        LaunchTarget {
            location: home_location(),
            select: None,
            error: None,
        }
    }
}

/// 실행 인자(경로 목록)를 그 실행의 작업 폴더 `working_dir` 기준으로 해석한다. 경로마다 하나씩 넘긴 순서대로
/// 돌려주고, 경로가 없으면 사용자 홈 폴더 하나다.
pub fn resolve_launch(working_dir: &Path, paths: &[PathBuf]) -> Vec<LaunchTarget> {
    if paths.is_empty() {
        return vec![LaunchTarget::home()];
    }
    paths
        .iter()
        .map(|path| resolve_one(working_dir, path))
        .collect()
}

fn resolve_one(working_dir: &Path, arg: &Path) -> LaunchTarget {
    let parsed = parse_text(&arg.to_string_lossy());
    let path = match &parsed {
        Ok(path) => absolute(working_dir, path),
        // 해석하지 못한 글자도 탭의 위치(제목)로는 인자 그대로를 쓴다.
        Err(_) => absolute(working_dir, arg),
    };
    let resolved = parsed.and_then(|_| resolve_path(path.clone()));
    match resolved {
        Ok(resolved) => LaunchTarget {
            location: resolved.location,
            select: resolved.select,
            error: None,
        },
        Err(error) => LaunchTarget {
            location: Location::file_system(path),
            select: None,
            error: Some(error),
        },
    }
}

/// `path`가 상대 경로면 `working_dir` 기준으로 풀고, `.`·`..`을 정리한다. 디스크에 묻지 않는다.
fn absolute(working_dir: &Path, path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        working_dir.join(path)
    };
    let absolute = std::path::absolute(&joined).unwrap_or(joined);
    normalize(&absolute)
}

/// `.`을 빼고 `..`은 앞 조각과 함께 지운다(루트 위로는 올라가지 않는다). Windows의 `std::path::absolute`는 이미
/// 정리하지만 다른 플랫폼은 `..`을 남긴다.
fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}
