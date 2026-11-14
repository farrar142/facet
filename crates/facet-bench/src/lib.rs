//! facet 측정 도구.
//!
//! - 시험 폴더 생성: 실제 폴더와 비슷한 모양(한글·이모지·긴 이름, 폴더와 파일이 섞임)의
//!   항목을 정해진 수만큼 만든다. 같은 수로 다시 돌리면 결과가 같다.
//! - 코어 벤치마크: 「위치의 항목 목록 읽기」([`facet_core::list_location`])를 재고
//!   상한([`LIST_LIMIT`])과 비교한다. `cargo xtask ci`가 이것을 부른다.

pub mod screen;

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use facet_core::{ListError, list_location};

/// 벤치마크 폴더의 기본 항목 수.
pub const DEFAULT_COUNT: usize = 100_000;

/// 10만 항목 목록 읽기의 속도 기준(로컬, 릴리스 빌드). spec #1의 통과 기준이다.
pub const LIST_TARGET: Duration = Duration::from_millis(100);

/// CI가 실패시키는 10만 항목 목록 읽기의 상한: 로컬 기준 [`LIST_TARGET`]의 10배.
///
/// 공유 러너는 디스크·CPU 편차가 커서 로컬 기준을 그대로 쓰면 이유 없는 실패가 난다.
/// 이 상한은 알고리즘이 한 단계 나빠지는 실수(예: 항목마다 시스템 호출을 더 하거나
/// 제곱 시간이 드는 처리)를 잡는 용도이지 미세한 회귀를 잡는 용도가 아니다
/// (`docs/roadmap.md`의 「속도는 두 군데서 지킨다」).
pub const LIST_LIMIT: Duration = Duration::from_millis(1000);

/// 시험 폴더의 항목 하나의 모양.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestEntry {
    pub name: String,
    pub is_folder: bool,
    /// 파일의 바이트 크기. 폴더는 0.
    pub size: u64,
}

/// 시험 폴더의 `index`번째 항목. 같은 번호는 늘 같은 항목이다.
///
/// 열 개마다 두 개가 폴더이고, 나머지는 확장자·길이·문자가 다른 파일이다.
pub fn test_folder_entry(index: usize) -> TestEntry {
    let i = index;
    let (name, is_folder) = match i % 10 {
        0 => (format!("보고서 {i:06}.txt"), false),
        1 => (format!("photo_{i:06} 🎉.jpg"), false),
        2 => (
            format!("{}{i:06}.md", "아주 긴 이름을 가진 회의록 문서 ".repeat(6)),
            false,
        ),
        3 => (format!("data-{i:06}.bin"), false),
        4 => (format!("폴더 {i:06}"), true),
        5 => (format!("README {i:06}"), false),
        6 => (format!("🦀🚀 이모지 {i:06}.rs"), false),
        7 => (
            format!("Quarterly Report {i:06} (final) - copy.xlsx"),
            false,
        ),
        8 => (format!("archive.{i:06}.tar.gz"), false),
        _ => (format!("📁 프로젝트 {i:06}"), true),
    };
    let size = if is_folder {
        0
    } else {
        (i as u64).wrapping_mul(37) % 1024
    };
    TestEntry {
        name,
        is_folder,
        size,
    }
}

/// [`make_test_folder`]의 결과.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MakeReport {
    /// 새로 만든 항목 수.
    pub created: usize,
    /// 이미 있던 항목 수(크기를 고친 파일 포함).
    pub kept: usize,
}

/// 시험 폴더를 만들지 못한 까닭.
#[derive(Debug)]
pub enum TestFolderError {
    /// 폴더 안에 시험 폴더의 항목이 아닌 것이 있다. 사용자 파일을 지우지 않으려고 멈춘다.
    Unexpected { path: PathBuf },
    /// 같은 이름이 있지만 폴더/파일 종류가 다르다.
    WrongKind { path: PathBuf },
    /// 입출력 오류.
    Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for TestFolderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unexpected { path } => write!(
                f,
                "시험 폴더의 항목이 아닌 것이 있어 멈춥니다(빈 폴더나 새 경로를 주세요): {}",
                path.display()
            ),
            Self::WrongKind { path } => write!(
                f,
                "이름은 같지만 폴더/파일 종류가 다른 항목이 있습니다: {}",
                path.display()
            ),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for TestFolderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> TestFolderError + '_ {
    move |source| TestFolderError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// `path`에 `count`개 항목([`test_folder_entry`]의 0..count번)이 든 시험 폴더를 만든다.
///
/// 폴더가 없으면 만든다. 이미 있으면 빠진 항목만 만들고 크기가 다른 파일은 고친다.
/// 그래서 같은 `count`로 다시 돌리면 결과(이름·종류·크기)가 같다.
/// 시험 폴더의 항목이 아닌 것이 있으면 아무것도 지우지 않고 오류를 낸다.
pub fn make_test_folder(path: &Path, count: usize) -> Result<MakeReport, TestFolderError> {
    fs::create_dir_all(path).map_err(io_err(path))?;

    let mut wanted: BTreeMap<String, TestEntry> = (0..count)
        .map(test_folder_entry)
        .map(|e| (e.name.clone(), e))
        .collect();

    // 먼저 전부 확인한 뒤에야 쓴다: 모르는 항목이 있으면 아무것도 바꾸지 않는다.
    let existing = list_location(path).map_err(|e| TestFolderError::Io {
        path: path.to_path_buf(),
        source: io::Error::other(e.to_string()),
    })?;
    let mut to_resize = Vec::new();
    let mut kept = 0;
    for item in existing {
        let item_path = path.join(&item.name);
        let Some(entry) = item.name.to_str().and_then(|n| wanted.remove(n)) else {
            return Err(TestFolderError::Unexpected { path: item_path });
        };
        if entry.is_folder != item.is_folder {
            return Err(TestFolderError::WrongKind { path: item_path });
        }
        if !entry.is_folder && entry.size != item.size {
            to_resize.push((item_path, entry.size));
        }
        kept += 1;
    }

    for (item_path, size) in to_resize {
        OpenOptions::new()
            .write(true)
            .open(&item_path)
            .and_then(|f| f.set_len(size))
            .map_err(io_err(&item_path))?;
    }

    let created = wanted.len();
    for entry in wanted.into_values() {
        let item_path = path.join(&entry.name);
        if entry.is_folder {
            fs::create_dir(&item_path).map_err(io_err(&item_path))?;
        } else {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&item_path)
                .and_then(|f| f.set_len(entry.size))
                .map_err(io_err(&item_path))?;
        }
    }

    Ok(MakeReport { created, kept })
}

/// 목록 읽기 측정 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Measurement {
    /// 읽은 항목 수.
    pub item_count: usize,
    /// 회차별 걸린 시간.
    pub times: Vec<Duration>,
}

impl Measurement {
    /// 회차별 시간의 중앙값(짝수 개면 가운데 둘 중 큰 쪽).
    pub fn median(&self) -> Duration {
        let mut sorted = self.times.clone();
        sorted.sort();
        sorted.get(sorted.len() / 2).copied().unwrap_or_default()
    }

    /// 가장 오래 걸린 회차.
    pub fn max(&self) -> Duration {
        self.times.iter().max().copied().unwrap_or_default()
    }
}

/// `path`의 목록 읽기를 `runs`번 잰다. 재기 전에 한 번 읽어 캐시를 데운다.
pub fn measure_list(path: &Path, runs: usize) -> Result<Measurement, ListError> {
    let mut item_count = list_location(path)?.len();
    let mut times = Vec::with_capacity(runs);
    for _ in 0..runs {
        let start = Instant::now();
        let items = list_location(path)?;
        times.push(start.elapsed());
        item_count = items.len();
    }
    Ok(Measurement { item_count, times })
}

/// 중앙값이 `limit`을 넘으면 넘은 수치를 담은 메시지를 돌려준다.
pub fn check_limit(m: &Measurement, limit: Duration) -> Result<(), String> {
    let median = m.median();
    if median <= limit {
        Ok(())
    } else {
        Err(format!(
            "목록 읽기 상한 초과: {}항목 중앙값 {}ms > 상한 {}ms (최악 {}ms)",
            m.item_count,
            median.as_millis(),
            limit.as_millis(),
            m.max().as_millis()
        ))
    }
}
