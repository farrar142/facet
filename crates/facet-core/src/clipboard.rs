//! 클립보드 형식: 탐색기와 같은 바이트·글자를 만들고 읽는 순수 함수.
//!
//! 정본은 [`facet_win::clipboard::format`]이다. 클립보드를 소유하는 STA 스레드([`facet_win::clipboard::Clipboard`])가
//! 같은 형식을 써야 하는데 코어가 그 크레이트에 기대므로, 형식은 거기 두고 여기서 그대로 다시 내보낸다.
//! 화면 문장은 없다(ADR-0007).

pub use facet_win::clipboard::format::*;

use facet_win::clipboard::ClipboardFiles;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::Path;

/// 잘라내기 표시: 클립보드가 알려 준 「잘라낸 경로들」([`ClipboardOperation::Move`]인 파일 목록)과 같은 항목인지 묻는
/// 자료 구조. 행을 그릴 때마다 묻기 때문에 경로를 소문자로 바꿔 해시 집합에 담아 항목 하나를 상수 시간에 판정한다
/// (1만 개여도 목록 그리기가 느려지지 않는다). 경로는 대소문자를 무시하고 `/`와 `\`, 끝의 구분자를 같게 본다.
/// 폴더가 다르면 이름이 같아도 다른 항목이다. 화면 문장은 없다(ADR-0007).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CutMarks {
    paths: HashSet<String>,
}

fn normalize(path: &str) -> String {
    let mut key = path.replace('/', "\\").to_lowercase();
    while key.len() > 1 && key.ends_with('\\') {
        key.pop();
    }
    key
}

impl CutMarks {
    /// 잘라낸 경로들로 만든다.
    pub fn new<P: AsRef<Path>>(paths: impl IntoIterator<Item = P>) -> Self {
        CutMarks {
            paths: paths
                .into_iter()
                .map(|path| normalize(&path.as_ref().to_string_lossy()))
                .collect(),
        }
    }

    /// 클립보드 요약에서 만든다: 잘라내기 표시가 있는 파일 목록만 표시가 된다(복사거나 파일이 없으면 비어 있음).
    pub fn from_clipboard(files: Option<&ClipboardFiles>) -> Self {
        match files {
            Some(files) if files.operation == ClipboardOperation::Move => Self::new(&files.paths),
            _ => Self::default(),
        }
    }

    /// 잘라낸 경로가 하나도 없는가.
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// 잘라낸 경로 수.
    pub fn len(&self) -> usize {
        self.paths.len()
    }

    /// 그 폴더 안의 그 이름 항목이 잘라낸 것인가.
    pub fn contains(&self, folder: &Path, name: &OsStr) -> bool {
        if self.paths.is_empty() {
            return false;
        }
        let mut key = folder.to_string_lossy().replace('/', "\\");
        if !key.ends_with('\\') {
            key.push('\\');
        }
        key.push_str(&name.to_string_lossy());
        self.paths.contains(&normalize(&key))
    }
}
