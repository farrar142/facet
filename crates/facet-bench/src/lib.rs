//! facet 측정 도구.
//!
//! - 시험 폴더 생성: 실제 폴더와 비슷한 모양(한글·이모지·긴 이름, 폴더와 파일이 섞임)의
//!   항목을 정해진 수만큼 만든다. 같은 수로 다시 돌리면 결과가 같다.
//! - 코어 벤치마크: 「위치의 항목 목록 읽기」(파일 시스템 공급자의 [`Provider::list`]), 「읽기+정렬」,
//!   「정렬 바꾸기」([`DetailsList::set_sort`])를 재고 각각 상한([`LIST_LIMIT`]·[`LIST_SORT_LIMIT`]·
//!   [`RESORT_LIMIT`])과 비교한다. `cargo xtask ci`가 이것을 부른다.
//! - 인덱스 벤치마크: 시험 폴더와 같은 이름의 합성 인덱스([`synthetic_index`])를 만들어
//!   메모리([`INDEX_MEMORY_LIMIT`])와 부분 일치 질의 시간([`QUERY_LIMIT`])을 잰다.
//!   `cargo xtask ci`가 이것도 부른다.
//! - MFT 파싱 벤치마크: 저장소의 MFT 조각을 되풀이한 덩어리([`mft_image`])를 인덱스로 만드는
//!   시간([`MFT_PARSE_LIMIT`])을 잰다. `cargo xtask ci`가 이것도 부른다.

pub mod screen;

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use facet_core::details::{Column, DetailsList, Direction, Sort};
use facet_core::{FileSystemProvider, ListError, Location, Provider};

/// 파일 시스템 공급자로 실제 폴더 `path`의 항목 목록을 읽는다.
fn list_folder(path: &Path) -> Result<Vec<facet_core::Item>, ListError> {
    FileSystemProvider.list(&Location::file_system(path))
}

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
    let existing = list_folder(path).map_err(|e| TestFolderError::Io {
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
    let mut item_count = list_folder(path)?.len();
    let mut times = Vec::with_capacity(runs);
    for _ in 0..runs {
        let start = Instant::now();
        let items = list_folder(path)?;
        times.push(start.elapsed());
        item_count = items.len();
    }
    Ok(Measurement { item_count, times })
}

/// 10만 항목 「읽기+정렬」(목록을 읽어 기본 정렬인 이름 오름차순으로 늘어놓기)의 속도 기준(로컬, 릴리스 빌드).
///
/// spec #16의 「10만 항목 폴더 열기부터 첫 화면까지 150ms 미만」에서 코어가 맡는 몫이다. 화면을 그리는 시간은
/// 측정 명령(`cargo xtask measure`)이 따로 잰다.
pub const LIST_SORT_TARGET: Duration = Duration::from_millis(150);

/// CI가 실패시키는 「읽기+정렬」 상한: 로컬 기준 [`LIST_SORT_TARGET`]의 10배.
/// [`LIST_LIMIT`]과 같은 까닭으로 넉넉하다(알고리즘이 한 단계 나빠지는 실수만 잡는다).
pub const LIST_SORT_LIMIT: Duration = Duration::from_millis(1500);

/// 10만 항목 「정렬 바꾸기」(이미 읽은 목록을 다른 열·방향으로 다시 정렬)의 속도 기준(로컬, 릴리스 빌드). spec #16.
pub const RESORT_TARGET: Duration = Duration::from_millis(50);

/// CI가 실패시키는 「정렬 바꾸기」 상한: 로컬 기준 [`RESORT_TARGET`]의 10배.
pub const RESORT_LIMIT: Duration = Duration::from_millis(500);

/// 정렬 바꾸기 벤치마크가 회차마다 차례로 바꾸는 정렬. 이웃한 두 정렬이 늘 다르다.
///
/// 이름 오름 → 내림처럼 순서를 뒤집기만 하는 바꾸기는 정렬 알고리즘이 거꾸로 된 덩어리를 알아보고
/// 거의 공짜로 끝내므로, 열을 바꾸는 쪽을 섞어 실제 비교가 일어나게 한다.
pub const RESORT_CYCLE: [Sort; 5] = [
    Sort {
        column: Column::Size,
        direction: Direction::Descending,
    },
    Sort {
        column: Column::Type,
        direction: Direction::Ascending,
    },
    Sort {
        column: Column::Modified,
        direction: Direction::Descending,
    },
    Sort {
        column: Column::Name,
        direction: Direction::Descending,
    },
    Sort {
        column: Column::Size,
        direction: Direction::Ascending,
    },
];

/// `path`의 「읽기+정렬」(목록 읽기와 기본 정렬)을 `runs`번 잰다. 재기 전에 한 번 해서 캐시를 데운다.
pub fn measure_list_sorted(path: &Path, runs: usize) -> Result<Measurement, ListError> {
    let read_sorted = || -> Result<DetailsList, ListError> {
        Ok(DetailsList::new(list_folder(path)?, Sort::default()))
    };
    let mut item_count = read_sorted()?.len();
    let mut times = Vec::with_capacity(runs);
    for _ in 0..runs {
        let start = Instant::now();
        let list = read_sorted()?;
        times.push(start.elapsed());
        item_count = list.len();
    }
    Ok(Measurement { item_count, times })
}

/// `path`의 목록을 한 번 읽어 두고 「정렬 바꾸기」를 `runs`번 잰다. 회차마다 [`RESORT_CYCLE`]의 다음
/// 정렬로 바꾸며, 재기 전에 그 순환을 한 바퀴 돌려 데운다. 파일 시스템은 다시 읽지 않는다.
pub fn measure_resort(path: &Path, runs: usize) -> Result<Measurement, ListError> {
    let mut list = DetailsList::new(list_folder(path)?, Sort::default());
    for sort in RESORT_CYCLE {
        list.set_sort(sort);
    }
    let mut times = Vec::with_capacity(runs);
    for sort in RESORT_CYCLE.iter().cycle().take(runs) {
        let start = Instant::now();
        list.set_sort(*sort);
        times.push(start.elapsed());
    }
    Ok(Measurement {
        item_count: list.len(),
        times,
    })
}

/// 목록 읽기 중앙값이 `limit`을 넘으면 넘은 수치를 담은 메시지를 돌려준다.
pub fn check_limit(m: &Measurement, limit: Duration) -> Result<(), String> {
    check_median("목록 읽기", m, limit)
}

/// `what`의 중앙값이 `limit`을 넘으면 무엇이 얼마나 넘었는지 담은 메시지를 돌려준다.
pub fn check_median(what: &str, m: &Measurement, limit: Duration) -> Result<(), String> {
    let median = m.median();
    if median <= limit {
        Ok(())
    } else {
        Err(format!(
            "{what} 상한 초과: {}항목 중앙값 {}ms > 상한 {}ms (최악 {}ms)",
            m.item_count,
            median.as_millis(),
            limit.as_millis(),
            m.max().as_millis()
        ))
    }
}

/// 합성 인덱스의 기본 항목 수.
pub const INDEX_COUNT: usize = 1_000_000;

/// 합성 인덱스 100만 개 부분 일치 질의(결과 100개 제한)의 속도 기준(로컬, 릴리스 빌드). spec #22.
pub const QUERY_TARGET: Duration = Duration::from_millis(30);

/// CI가 실패시키는 질의 시간 상한: 로컬 기준 [`QUERY_TARGET`]의 10배.
/// [`LIST_LIMIT`]과 같은 까닭으로 넉넉하다(알고리즘이 한 단계 나빠지는 실수만 잡는다).
pub const QUERY_LIMIT: Duration = Duration::from_millis(300);

/// 합성 인덱스 100만 개가 쓸 수 있는 메모리 상한(spec #22의 「150MB 미만」).
/// 10진 MB로 센다(150 × 10⁶바이트). MiB보다 작아 더 엄격하다.
pub const INDEX_MEMORY_LIMIT: usize = 150_000_000;

/// 벤치마크 질의의 찾을 조각. 대문자로 써서 대소문자 접기를 거치게 하고,
/// 100만 개 중 마지막 쪽 항목 하나(`Quarterly Report 999997 …`)만 맞게 골라
/// 결과 100개 제한에 일찍 멈추지 않고 인덱스 전체를 훑게 한다(나쁜 쪽의 경우).
pub const BENCH_QUERY: &str = "REPORT 99999";

/// 벤치마크 질의의 결과 수 제한.
pub const BENCH_RESULT_LIMIT: usize = 100;

/// 정렬 벤치마크 질의의 찾을 조각. 열 개 중 하나(`Quarterly Report …`)가 맞아 100만 개에서
/// 결과가 10만 개다. 결과 100개 제한 질의가 결과 전체를 매번 다 정렬하면 상한을 넘게 한다.
pub const BENCH_SORTED_QUERY: &str = "REPORT";

/// 정렬 벤치마크 질의의 정렬 기준. 비교가 가장 비싼 경로 정렬(내림)이다.
pub const BENCH_SORT: (facet_index::SortKey, facet_index::Order) =
    (facet_index::SortKey::Path, facet_index::Order::Descending);

/// 합성 인덱스의 볼륨 루트 경로와 루트 번호(NTFS 루트 폴더의 MFT 레코드 번호).
pub const SYNTHETIC_ROOT: &str = r"X:\";
pub const SYNTHETIC_ROOT_NUMBER: u64 = 5;

/// `index`번 항목의 항목 번호. NTFS의 예약 레코드(0..16)를 피한다.
fn synthetic_number(index: usize) -> u64 {
    16 + index as u64
}

/// `index`번 항목이 들어 있는 폴더의 번호.
///
/// 열 개씩 한 묶음이고 묶음마다 폴더가 둘(4번째, 9번째)이다. 첫 묶음은 볼륨 루트 아래,
/// 그 뒤 `k`번 묶음은 `k / 5`번 묶음의 폴더 하나 아래에 둔다. 폴더마다 약 25개 항목이
/// 들고 100만 개에서 깊이는 9 안팎이다.
fn synthetic_parent(index: usize) -> u64 {
    let group = index / 10;
    if group == 0 {
        return SYNTHETIC_ROOT_NUMBER;
    }
    let slot = if group.is_multiple_of(2) { 4 } else { 9 };
    synthetic_number(group / 5 * 10 + slot)
}

/// 항목 `count`개의 합성 인덱스. 이름은 시험 폴더와 같다([`test_folder_entry`]의 0..count번):
/// 한글·이모지·긴 이름이 섞이고 열 개 중 두 개가 폴더다. 같은 수면 늘 같은 인덱스다.
pub fn synthetic_index(count: usize) -> facet_index::Index {
    let mut index = facet_index::Index::new(SYNTHETIC_ROOT, SYNTHETIC_ROOT_NUMBER);
    for i in 0..count {
        let entry = test_folder_entry(i);
        let item = facet_index::NewItem {
            number: synthetic_number(i),
            parent: synthetic_parent(i),
            name: &entry.name,
            size: entry.size,
            // 2022년 무렵부터 1초씩 늦어지는 FILETIME.
            modified: 133_000_000_000_000_000 + i as u64 * 10_000_000,
            attributes: if entry.is_folder {
                facet_index::ATTRIBUTE_DIRECTORY
            } else {
                0x20 // FILE_ATTRIBUTE_ARCHIVE
            },
        };
        index
            .insert(item)
            .expect("시험 폴더의 이름은 인덱스에 넣을 수 있다");
    }
    index
}

/// `index`에 `query`를 `runs`번 재어 돌린다. 재기 전에 한 번 돌려 데운다.
/// [`Measurement::item_count`]는 결과 수다.
pub fn measure_query(
    index: &facet_index::Index,
    query: &facet_index::Query,
    runs: usize,
) -> Measurement {
    let mut item_count = index.query(query).len();
    let mut times = Vec::with_capacity(runs);
    for _ in 0..runs {
        let start = Instant::now();
        let hits = index.query(query);
        times.push(start.elapsed());
        item_count = hits.len();
    }
    Measurement { item_count, times }
}

/// 질의 중앙값이 `limit`을 넘으면 넘은 수치를 담은 메시지를 돌려준다.
pub fn check_query_limit(m: &Measurement, limit: Duration) -> Result<(), String> {
    let median = m.median();
    if median <= limit {
        Ok(())
    } else {
        Err(format!(
            "인덱스 질의 상한 초과: 중앙값 {}ms > 상한 {}ms (최악 {}ms, 결과 {}개)",
            median.as_millis(),
            limit.as_millis(),
            m.max().as_millis(),
            m.item_count
        ))
    }
}

/// 인덱스 메모리가 `limit` 바이트 이상이면 넘은 수치를 담은 메시지를 돌려준다.
pub fn check_memory(bytes: usize, limit: usize) -> Result<(), String> {
    if bytes < limit {
        Ok(())
    } else {
        Err(format!(
            "인덱스 메모리 상한 초과: {:.1}MB ≥ 상한 {:.1}MB",
            bytes as f64 / 1e6,
            limit as f64 / 1e6
        ))
    }
}

/// MFT 파싱 벤치마크의 재료: 저장소의 MFT 조각(`testdata/mft/ntfs-basic.mft`, 레코드 256개).
pub const MFT_FIXTURE: &[u8] = include_bytes!("../../../testdata/mft/ntfs-basic.mft");

/// MFT 조각의 레코드 크기.
pub const MFT_RECORD_SIZE: usize = 1024;

/// MFT 파싱 벤치마크의 기본 레코드 수. 항목 100만 개 볼륨의 MFT 크기 무렵이다.
pub const MFT_RECORDS: usize = 1_000_000;

/// MFT 레코드 100만 개(1GB)를 인덱스로 만드는 파싱 시간의 기준(로컬, 릴리스 빌드).
///
/// spec #22의 기준은 「항목 100만 개 볼륨의 첫 인덱스 10초 미만」이고, 그 안에 MFT 읽기·파싱·저장이
/// 모두 든다. MFT 100만 레코드는 약 1GB다. 이를 디스크에서 순서대로 읽는 데 느린 쪽(초당 150MB 안팎의
/// 하드 디스크)으로 약 7초, 인덱스 저장과 서비스 준비에 약 1초를 잡으면 파싱 몫은 2초가 남는다.
/// SSD에서는 읽기가 1~2초라 여유가 더 있다.
pub const MFT_PARSE_TARGET: Duration = Duration::from_millis(2000);

/// CI가 실패시키는 MFT 파싱 시간 상한: 로컬 기준 [`MFT_PARSE_TARGET`]의 10배.
/// [`LIST_LIMIT`]과 같은 까닭으로 넉넉하다(알고리즘이 한 단계 나빠지는 실수만 잡는다).
pub const MFT_PARSE_LIMIT: Duration = Duration::from_millis(20_000);

/// MFT 조각을 `records`개 레코드가 되도록 되풀이해 이어 붙인 MFT 덩어리.
///
/// 레코드마다 고정값이 맞으므로 모두 정상 레코드로 읽힌다. 되풀이한 레코드의 부모 참조는 첫 조각의
/// 폴더를 가리키므로 항목 대부분이 경로를 갖는다.
pub fn mft_image(records: usize) -> Vec<u8> {
    let len = records * MFT_RECORD_SIZE;
    MFT_FIXTURE.iter().copied().cycle().take(len).collect()
}

/// MFT 덩어리 `image`를 인덱스로 만드는 시간을 `runs`번 잰다. 재기 전에 한 번 돌려 데운다.
/// [`Measurement::item_count`]는 만든 인덱스의 항목 수다.
pub fn measure_mft(image: &[u8], runs: usize) -> Result<Measurement, String> {
    let parse = || {
        facet_index::Index::from_mft(SYNTHETIC_ROOT, MFT_RECORD_SIZE, image)
            .map_err(|e| e.to_string())
    };
    let (index, report) = parse()?;
    if report.skipped != 0 {
        return Err(format!(
            "MFT 조각에서 건너뛴 레코드가 있습니다: {}개",
            report.skipped
        ));
    }
    let mut item_count = index.len();
    drop(index);
    let mut times = Vec::with_capacity(runs);
    for _ in 0..runs {
        let start = Instant::now();
        let (index, _) = parse()?;
        times.push(start.elapsed());
        item_count = index.len();
    }
    Ok(Measurement { item_count, times })
}

/// MFT 파싱 중앙값이 `limit`을 넘으면 넘은 수치를 담은 메시지를 돌려준다.
pub fn check_mft_limit(m: &Measurement, records: usize, limit: Duration) -> Result<(), String> {
    let median = m.median();
    if median <= limit {
        Ok(())
    } else {
        Err(format!(
            "MFT 파싱 상한 초과: 레코드 {records}개 중앙값 {}ms > 상한 {}ms (최악 {}ms)",
            median.as_millis(),
            limit.as_millis(),
            m.max().as_millis()
        ))
    }
}
