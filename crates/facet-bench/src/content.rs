//! 내용 검색 벤치마크: 임시 폴더의 본문 128MB 시험 나무([`make_content_tree`])를 내용 검색으로 끝까지 읽는
//! 처리량을 잰다([`measure_content_search`]).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use facet_core::Location;

use crate::{Measurement, TestFolderError, io_err};

/// 시험 나무의 폴더 수.
pub const CONTENT_FOLDERS: usize = 4;

/// 폴더마다 든 파일 수.
pub const CONTENT_FILES_PER_FOLDER: usize = 8;

/// 파일 하나의 크기: 4MiB. 큰 파일 한도(50MB)보다 한참 작고 한 파일을 읽는 시간이 시스템 호출 몇 번의 비용보다
/// 충분히 길다.
pub const CONTENT_FILE_SIZE: usize = 4 * 1024 * 1024;

/// 벤치마크가 찾는 글자(대소문자 무시). 시험 나무 전체에서 마지막 파일의 마지막 줄에만 있어 모든 파일을 끝까지
/// 읽는다.
pub const CONTENT_NEEDLE: &str = "needle-facet-bench";

/// 처리량의 로컬 기준(릴리스 빌드, 파일이 운영체제 캐시에 있을 때): 400MB/s(10진). 대소문자 무시 글자 일치는
/// `regex`의 부분 문자열 찾기라 한 코어에서도 초당 GB 단위이고, 파일 읽기·UTF-8 검증·일꾼 스레드 여럿이 더해진다.
/// 2026-11-16 GitHub 호스트 Windows 러너(논리 코어 4개, 캐시에 든 128MB)에서 잰 중앙값은 약 1.6GB/s였고, 로컬 기준은
/// 캐시에 없는 SSD 읽기까지 감안해 그 1/4로 잡았다.
pub const CONTENT_THROUGHPUT_TARGET: u64 = 400_000_000;

/// CI가 실패시키는 처리량 하한: 로컬 기준 [`CONTENT_THROUGHPUT_TARGET`]의 1/10인 40MB/s. 공유 러너의 디스크·CPU
/// 편차를 견디면서 한 단계 나빠지는 실수(파일을 바이트·줄 단위로 읽기, 정규식을 파일마다 새로 컴파일, 결과마다 본문
/// 복사 등)만 잡는다(`docs/roadmap.md`의 「속도는 두 군데서 지킨다」, [`crate::LIST_LIMIT`]과 같은 까닭).
pub const CONTENT_THROUGHPUT_LIMIT: u64 = 40_000_000;

/// 시험 나무의 전체 바이트.
pub const CONTENT_TOTAL_BYTES: u64 =
    (CONTENT_FOLDERS * CONTENT_FILES_PER_FOLDER * CONTENT_FILE_SIZE) as u64;

/// 시험 나무의 자리: 임시 폴더 아래 `facet-test-folders/content-search`(`target/` 밑이 아님).
pub fn test_content_path() -> PathBuf {
    std::env::temp_dir()
        .join("facet-test-folders")
        .join("content-search")
}

/// 시험 파일 `index`번째의 본문: 한글과 영문이 섞인 줄을 [`CONTENT_FILE_SIZE`]바이트까지 채운다. 마지막 파일의
/// 마지막 줄만 [`CONTENT_NEEDLE`]이 든다.
fn content_file_bytes(index: usize, last: bool) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(CONTENT_FILE_SIZE);
    let mut line = 0usize;
    while bytes.len() + 128 < CONTENT_FILE_SIZE {
        let text = format!(
            "{index:03}-{line:07} 회의록 본문 한 줄 the quick brown fox jumps over the lazy dog 보고서 초안\n"
        );
        bytes.extend_from_slice(text.as_bytes());
        line += 1;
    }
    let tail = if last {
        format!("마지막 줄 {CONTENT_NEEDLE}\n")
    } else {
        "마지막 줄\n".to_owned()
    };
    bytes.extend_from_slice(tail.as_bytes());
    bytes.resize(CONTENT_FILE_SIZE, b' ');
    bytes
}

/// `path`에 시험 나무를 만든다. 크기가 맞는 파일은 그대로 두므로 다시 돌려도 결과가 같다. 파일 수를 돌려준다.
pub fn make_content_tree(path: &Path) -> Result<usize, TestFolderError> {
    let total = CONTENT_FOLDERS * CONTENT_FILES_PER_FOLDER;
    for n in 0..total {
        let folder = path.join(format!("폴더 {}", n / CONTENT_FILES_PER_FOLDER));
        fs::create_dir_all(&folder).map_err(io_err(&folder))?;
        let file = folder.join(format!("본문 {n:02}.txt"));
        let ok = fs::metadata(&file).is_ok_and(|m| m.len() == CONTENT_FILE_SIZE as u64);
        if !ok {
            fs::write(&file, content_file_bytes(n, n + 1 == total)).map_err(io_err(&file))?;
        }
    }
    Ok(total)
}

/// 내용 검색을 끝까지 돌려 (찾은 파일 수, 읽은 바이트)를 낸다.
fn run_content_search(root: &Path) -> Result<(usize, u64), String> {
    use facet_core::content_search::{
        ContentEnd, ContentEvent, ContentRequest, ContentSearch, Skipped,
    };
    let mut search = ContentSearch::start(ContentRequest::new(
        Location::file_system(root),
        CONTENT_NEEDLE.to_uppercase(),
    ))
    .map_err(|e| format!("{e:?}"))?;
    let mut found = 0;
    loop {
        match search.wait_event(Duration::from_secs(300)) {
            Some(ContentEvent::Batch { items, .. }) => found += items.len(),
            Some(ContentEvent::Finished(summary)) => {
                let p = summary.progress;
                if summary.end != ContentEnd::Finished
                    || p.skipped != Skipped::default()
                    || p.folders_skipped != 0
                {
                    return Err(format!("내용 검색 요약이 이상합니다: {summary:?}"));
                }
                return Ok((found, p.bytes_read));
            }
            None => return Err("내용 검색이 끝나지 않았습니다".to_owned()),
        }
    }
}

/// 시험 나무의 내용 검색 전체 시간(시작부터 끝남 사건까지)을 `runs`번 잰다(한 번 데운 뒤). 찾은 파일이 하나가
/// 아니거나 읽은 바이트가 [`CONTENT_TOTAL_BYTES`]와 다르거나 건너뛴 것이 있으면 실패한다.
/// [`Measurement::item_count`]는 파일 수다.
pub fn measure_content_search(path: &Path, runs: usize) -> Result<Measurement, String> {
    let files = make_content_tree(path).map_err(|e| e.to_string())?;
    let check = |(found, bytes): (usize, u64)| {
        if found != 1 || bytes != CONTENT_TOTAL_BYTES {
            return Err(format!(
                "내용 검색 결과가 다릅니다: 찾은 파일 {found}개(1개 기대), 읽은 바이트 {bytes}(기대 {CONTENT_TOTAL_BYTES})"
            ));
        }
        Ok(())
    };
    check(run_content_search(path)?)?;
    let mut times = Vec::with_capacity(runs);
    for _ in 0..runs {
        let start = Instant::now();
        let result = run_content_search(path)?;
        times.push(start.elapsed());
        check(result)?;
    }
    Ok(Measurement {
        item_count: files,
        times,
    })
}

/// 처리량(초당 바이트).
pub fn content_throughput(time: Duration) -> f64 {
    CONTENT_TOTAL_BYTES as f64 / time.as_secs_f64().max(1e-9)
}

/// 처리량 중앙값이 `min_bytes_per_second`보다 낮으면 수치를 담은 메시지를 돌려준다.
pub fn check_content_throughput(m: &Measurement, min_bytes_per_second: u64) -> Result<(), String> {
    let median = content_throughput(m.median());
    if median >= min_bytes_per_second as f64 {
        Ok(())
    } else {
        Err(format!(
            "내용 검색 처리량 하한 미달: 중앙값 {:.1}MB/s < 하한 {:.1}MB/s (최악 {:.1}MB/s)",
            median / 1e6,
            min_bytes_per_second as f64 / 1e6,
            content_throughput(m.max()) / 1e6
        ))
    }
}
