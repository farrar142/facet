//! 복사 벤치마크와 탐색기 비교 측정.
//!
//! - **복사 벤치마크**(CI가 지킴, `facet-bench copy`): 작업 대기열([`JobQueue`])의 복사 작업으로 (1) 큰 파일 하나
//!   ([`COPY_BENCH_BIG_SIZE`], 256MiB)의 처리량을 하한([`COPY_THROUGHPUT_LIMIT`])과, (2) 작은 파일
//!   ([`COPY_SMALL_SIZE`], 4KiB) [`COPY_BENCH_SMALL_COUNT`]개(2천 개)가 든 폴더의 복사 시간을 상한
//!   ([`COPY_SMALL_LIMIT`])과 견준다. 각각 한 번 데우고 [`COPY_BENCH_RUNS`]회(3회) 잰다. 시험 파일은 임시 폴더에
//!   만들고(`target/` 밑이 아님) 대상은 매번 지운다.
//! - **탐색기 비교 측정**(사람이 잼, `facet-bench compare-copy`): 같은 복사를 facet 엔진과 셸의 `IFileOperation`
//!   ([`facet_win::shell_copy`])으로 번갈아 해 [`CompareResult`]로 낸다. 크기는 spec #18대로 1GiB
//!   ([`COPY_BIG_SIZE`])와 1만 개([`COPY_SMALL_COUNT`])다. 통과 기준은 큰 파일 처리량이 탐색기의
//!   [`EXPLORER_THROUGHPUT_RATIO`](90%) 이상, 작은 파일 1만 개 시간이 탐색기보다 길지 않음이다(spec #18).

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use facet_core::job::{JobEvent, JobQueue};

use crate::Measurement;
use crate::screen::{SystemSpec, json_string};

/// 탐색기 비교 측정의 큰 파일 크기: 1GiB(spec #18).
pub const COPY_BIG_SIZE: u64 = 1 << 30;

/// 탐색기 비교 측정의 작은 파일 수: 1만 개(spec #18).
pub const COPY_SMALL_COUNT: usize = 10_000;

/// 복사 벤치마크(CI)의 큰 파일 크기: 256MiB.
///
/// 무버퍼 입출력 경로의 문턱(`facet_win::file_ops::UNBUFFERED_COPY_THRESHOLD`, 256MiB 이상)과 같아 1GiB와
/// 같은 경로를 탄다. 처리량만 보므로 1GiB일 까닭이 없고, 1GiB였을 때는 원본 만들기와 6번 복사로 이 단계가 CI를
/// 2~5분 늘렸다.
pub const COPY_BENCH_BIG_SIZE: u64 = 256 << 20;

/// 복사 벤치마크(CI)의 작은 파일 수: 2천 개. 상한은 이 수에 맞춘 [`COPY_SMALL_LIMIT`]이다.
pub const COPY_BENCH_SMALL_COUNT: usize = 2_000;

/// 복사 벤치마크(CI)가 재는 횟수(데우기 한 번은 따로). 중앙값으로 판정하므로 3회면 한 번 튄 값을 견딘다.
pub const COPY_BENCH_RUNS: usize = 3;

/// 작은 파일 벤치마크의 파일 하나 크기: 4KiB(NTFS 클러스터 하나).
pub const COPY_SMALL_SIZE: usize = 4096;

/// 큰 파일 하나 복사 처리량의 기준(로컬, 릴리스 빌드, 초당 바이트): 400MB/s.
///
/// 256MiB 이상 파일은 무버퍼 입출력으로 디스크를 직접 읽고 쓰므로(`UNBUFFERED_COPY_THRESHOLD`) 처리량은 디스크가
/// 정한다. 같은 SSD 안 복사는 읽기와 쓰기가 한 디스크를 나눠 쓰므로 SATA SSD(순차 약 500MB/s) 안에서는 그
/// 무렵, NVMe에서는 그보다 높다. 하드 디스크(약 150MB/s)에서는 못 맞추는 값이고, 하드 디스크와의 비교는 탐색기
/// 비교 측정이 같은 기계에서 한다. 이 기준은 「엔진이 SSD 속도를 낸다」를 적어 둔 것이다.
pub const COPY_THROUGHPUT_TARGET: u64 = 400_000_000;

/// CI가 실패시키는 큰 파일 복사 처리량의 하한: 로컬 기준 [`COPY_THROUGHPUT_TARGET`]의 1/40(10MB/s).
///
/// 다른 벤치마크의 「기준의 10배」보다 넉넉하다. GitHub 호스트 Windows 러너의 디스크 처리량은 실행마다 크게
/// 다르다: 2026-11-15 성공한 실행 16번의 1GiB 복사 중앙값은 61~167MB/s였고, 같은 코드가 31.9MB/s로 떨어진
/// 실행도 있었다(그때 하한 40MB/s로 실패). 10MB/s는 관측한 가장 느린 값보다 세 배 넘게 아래라 러너 편차로는
/// 떨어지지 않고, 엔진이 한 단계 나빠지는 실수(바이트나 작은 덩어리 단위 복사, 덩어리마다 동기 쓰기, 진행
/// 콜백마다 무거운 일 등)는 초당 수 MB 아래로 떨어지므로 잡는다. 미세한 회귀를 잡는 용도가 아니다
/// (`docs/roadmap.md`의 「속도는 두 군데서 지킨다」).
pub const COPY_THROUGHPUT_LIMIT: u64 = COPY_THROUGHPUT_TARGET / 40;

/// 4KiB 파일 2천 개([`COPY_BENCH_SMALL_COUNT`])가 든 폴더 복사 시간의 기준(로컬, 릴리스 빌드): 1초.
///
/// 작은 파일 복사는 바이트가 아니라 파일마다 드는 일(열기·만들기·시각과 속성 맞추기·닫기, 백신 검사)이 정한다.
/// 탐색기는 SSD 기계에서 파일 하나에 0.3~0.5ms 안팎이 들어 1만 개에 3~5초쯤이다. facet은 탐색기보다 느리지
/// 않아야 하므로(spec #18) 그 위쪽 끝인 파일당 0.5ms를 기준으로 두어 2천 개에 1초다. 실제 비교는 탐색기 비교
/// 측정이 한다.
pub const COPY_SMALL_TARGET: Duration = Duration::from_secs(1);

/// CI가 실패시키는 작은 파일 2천 개 복사 시간의 상한: 로컬 기준 [`COPY_SMALL_TARGET`]의 20배(20초, 파일당 10ms).
///
/// 러너에서 1만 개 복사는 파일당 0.6~1.5ms였는데(2026-11-15 실행 16번), 큰 파일 처리량이 네다섯 배 떨어지는
/// 러너도 있었으므로 [`COPY_THROUGHPUT_LIMIT`]과 같은 까닭으로 10배보다 넉넉히 둔다(파일마다 일이 한 단계
/// 늘어나는 실수만 잡는다).
pub const COPY_SMALL_LIMIT: Duration = Duration::from_secs(20);

/// 탐색기 비교 측정의 큰 파일 기준: facet 처리량이 탐색기 처리량의 이 비율 이상이어야 한다(spec #18의 90%).
pub const EXPLORER_THROUGHPUT_RATIO: f64 = 0.9;

/// 복사 하나를 기다리는 최대 시간. 넘으면 실패로 본다(엔진이 멈춘 것).
const COPY_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// 큰 파일을 쓰는 덩어리 크기.
const WRITE_CHUNK: usize = 1 << 20;

/// 복사하는 쪽.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// facet 엔진: 작업 대기열의 복사 작업.
    Facet,
    /// 셸의 `IFileOperation`(탐색기의 복사 엔진).
    Explorer,
}

impl Engine {
    /// 표에 쓰는 이름.
    pub fn label(self) -> &'static str {
        match self {
            Engine::Facet => "facet",
            Engine::Explorer => "탐색기(IFileOperation)",
        }
    }
}

/// 복사 벤치마크·비교 측정의 원본: 큰 파일 하나와 작은 파일들이 든 폴더 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyFixture {
    /// 원본을 담은 폴더(만든 것).
    pub root: PathBuf,
    /// 큰 파일.
    pub big: PathBuf,
    /// 큰 파일 크기.
    pub big_size: u64,
    /// 작은 파일들이 든 폴더.
    pub small: PathBuf,
    /// 작은 파일 수.
    pub small_count: usize,
    /// 작은 파일 하나 크기.
    pub small_size: usize,
}

impl CopyFixture {
    /// `root` 폴더(없어야 한다)를 만들고 그 안에 큰 파일(`big_size`바이트)과 작은 파일 `small_count`개
    /// (`small_size`바이트씩)가 든 폴더를 만든다. 내용은 0이 아닌 무늬라 압축·희소 파일로 줄지 않는다.
    /// 큰 파일은 다 쓴 뒤 디스크로 내보내(`sync_all`) 재는 동안 쓰기 캐시를 비우는 일이 덜 끼게 한다.
    pub fn create(
        root: &Path,
        big_size: u64,
        small_count: usize,
        small_size: usize,
    ) -> io::Result<Self> {
        fs::create_dir_all(root.parent().unwrap_or(root))?;
        fs::create_dir(root)?;
        let fixture = CopyFixture {
            root: root.to_path_buf(),
            big: root.join("큰 파일.bin"),
            big_size,
            small: root.join("작은 파일"),
            small_count,
            small_size,
        };
        let made = fixture.write();
        if made.is_err() {
            let _ = fs::remove_dir_all(root);
        }
        made.map(|()| fixture)
    }

    fn write(&self) -> io::Result<()> {
        let mut file = File::create(&self.big)?;
        let mut chunk = vec![0u8; WRITE_CHUNK];
        let mut written = 0u64;
        let mut n = 0u64;
        while written < self.big_size {
            fill_pattern(&mut chunk, n);
            let len = (self.big_size - written).min(WRITE_CHUNK as u64) as usize;
            file.write_all(&chunk[..len])?;
            written += len as u64;
            n += 1;
        }
        file.sync_all()?;
        drop(file);

        fs::create_dir(&self.small)?;
        let mut content = vec![0u8; self.small_size];
        for i in 0..self.small_count {
            fill_pattern(&mut content, i as u64);
            let mut file = File::create(self.small.join(format!("작은 파일 {i:05}.bin")))?;
            file.write_all(&content)?;
        }
        Ok(())
    }

    /// 큰 파일 복사의 항목 수(파일 하나).
    pub fn big_items(&self) -> u64 {
        1
    }

    /// 작은 파일 폴더 복사의 항목 수(폴더 자신과 파일들).
    pub fn small_items(&self) -> u64 {
        self.small_count as u64 + 1
    }

    /// 작은 파일 폴더 복사의 바이트.
    pub fn small_bytes(&self) -> u64 {
        (self.small_count * self.small_size) as u64
    }

    /// 만든 원본을 모두 지운다.
    pub fn remove(&self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// `n`번째 덩어리의 무늬: 덩어리마다·자리마다 다른 0이 아닌 바이트.
fn fill_pattern(buffer: &mut [u8], n: u64) {
    let mut x = n.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    for b in buffer.iter_mut() {
        // xorshift: 빠르고 압축되지 않는다.
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = (x as u8) | 1;
    }
}

/// 대상 폴더를 비운다(없으면 만든다). 재는 시간에 들지 않는다.
pub fn clear_dir(dir: &Path) -> io::Result<()> {
    match fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    fs::create_dir_all(dir)
}

/// `sources`를 폴더 `target` 안으로 `engine`으로 한 번 복사하고 걸린 시간을 돌려준다. facet 엔진이면 `queue`에
/// 복사 작업을 넣고 끝남 사건까지 기다리며, 건너뛴 항목이 있거나 처리한 항목 수가 `expected_items`가 아니면
/// 실패한다. 탐색기면 셸 복사가 끝날 때까지다.
pub fn copy_once(
    engine: Engine,
    queue: &mut JobQueue,
    sources: &[PathBuf],
    target: &Path,
    expected_items: u64,
) -> Result<Duration, String> {
    let start = Instant::now();
    match engine {
        Engine::Facet => {
            let id = queue
                .copy(sources.to_vec(), target.to_path_buf())
                .map_err(|e| format!("복사 작업을 만들지 못했습니다: {e:?}"))?;
            let deadline = start + COPY_TIMEOUT;
            let summary = loop {
                let left = deadline.saturating_duration_since(Instant::now());
                match queue.wait_event(left) {
                    Some(JobEvent::Finished { job, summary }) if job == id => break summary,
                    Some(JobEvent::Conflict { .. } | JobEvent::ItemError { .. }) => {
                        return Err("복사 작업이 답을 기다립니다(이름 충돌이나 항목별 오류)".into());
                    }
                    Some(_) => {}
                    None => return Err("복사 작업이 제때 끝나지 않았습니다".into()),
                }
            };
            let elapsed = start.elapsed();
            if !summary.failures.is_empty()
                || summary.items_skipped() != 0
                || summary.items_remaining != 0
                || summary.items_done != expected_items
            {
                return Err(format!(
                    "복사 작업이 다 복사하지 못했습니다(기대한 항목 {expected_items}개): {summary:?}"
                ));
            }
            Ok(elapsed)
        }
        Engine::Explorer => {
            facet_win::shell_copy::copy_items(sources, target).map_err(|e| e.to_string())?;
            Ok(start.elapsed())
        }
    }
}

/// `sources`를 `target` 안으로 복사하는 시간을 `runs`번 잰다. 재기 전에 한 번 복사해 데운다. 회차마다 먼저
/// 대상을 비운다(재는 시간 밖). 다 재면 대상을 지운다. [`Measurement::item_count`]는 `expected_items`다.
pub fn measure_copy(
    engine: Engine,
    sources: &[PathBuf],
    target: &Path,
    expected_items: u64,
    runs: usize,
) -> Result<Measurement, String> {
    let mut queue = JobQueue::new();
    let result = (|| {
        let mut once = || -> Result<Duration, String> {
            clear_dir(target).map_err(|e| format!("{}: {e}", target.display()))?;
            copy_once(engine, &mut queue, sources, target, expected_items)
        };
        once()?;
        let times = (0..runs).map(|_| once()).collect::<Result<Vec<_>, _>>()?;
        Ok(Measurement {
            item_count: expected_items as usize,
            times,
        })
    })();
    let _ = fs::remove_dir_all(target);
    result
}

/// 처리량(초당 바이트). 시간이 0이면 아주 큰 값.
pub fn throughput(bytes: u64, time: Duration) -> f64 {
    bytes as f64 / time.as_secs_f64().max(1e-9)
}

/// 큰 파일 복사 중앙값의 처리량이 `min_bytes_per_second`보다 낮으면 그 수치를 담은 메시지를 돌려준다.
pub fn check_throughput(
    m: &Measurement,
    bytes: u64,
    min_bytes_per_second: u64,
) -> Result<(), String> {
    let median = throughput(bytes, m.median());
    if median >= min_bytes_per_second as f64 {
        Ok(())
    } else {
        Err(format!(
            "복사 처리량 하한 미달: {:.1}MB 파일 중앙값 {:.1}MB/s < 하한 {:.1}MB/s (최악 {:.1}MB/s)",
            bytes as f64 / 1e6,
            median / 1e6,
            min_bytes_per_second as f64 / 1e6,
            throughput(bytes, m.max()) / 1e6
        ))
    }
}

/// 탐색기 비교 측정의 한 가지(큰 파일 또는 작은 파일들): 두 엔진의 회차별 시간.
#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    /// 복사한 바이트.
    pub bytes: u64,
    /// 복사한 항목 수(폴더 포함).
    pub items: u64,
    pub facet: Vec<Duration>,
    pub explorer: Vec<Duration>,
}

impl Comparison {
    fn median(times: &[Duration]) -> Duration {
        crate::screen::median(times)
    }

    /// facet 중앙값.
    pub fn facet_median(&self) -> Duration {
        Self::median(&self.facet)
    }

    /// 탐색기 중앙값.
    pub fn explorer_median(&self) -> Duration {
        Self::median(&self.explorer)
    }

    /// 중앙값 처리량의 비(facet / 탐색기). 1보다 크면 facet이 빠르다.
    pub fn ratio(&self) -> f64 {
        self.explorer_median().as_secs_f64() / self.facet_median().as_secs_f64().max(1e-9)
    }
}

/// 탐색기 비교 측정 결과.
#[derive(Debug, Clone, PartialEq)]
pub struct CompareResult {
    /// 원본을 만든 폴더.
    pub source: PathBuf,
    /// 복사한 대상 폴더.
    pub target: PathBuf,
    /// 큰 파일 하나.
    pub big: Comparison,
    /// 작은 파일들이 든 폴더 하나.
    pub small: Comparison,
    /// 기계 사양(디스크는 원본 쪽).
    pub spec: SystemSpec,
    /// 대상 쪽 디스크.
    pub target_disk: Option<String>,
    pub debug_build: bool,
}

/// 비교 측정의 판정.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompareVerdict {
    /// 큰 파일: facet 처리량이 탐색기의 90% 이상.
    pub big_pass: bool,
    /// 작은 파일: facet 시간이 탐색기보다 길지 않음.
    pub small_pass: bool,
}

impl CompareVerdict {
    pub fn pass(&self) -> bool {
        self.big_pass && self.small_pass
    }
}

fn mark(pass: bool) -> &'static str {
    if pass { "통과" } else { "실패" }
}

impl CompareResult {
    /// 통과 기준과 견준다(중앙값끼리).
    pub fn verdict(&self) -> CompareVerdict {
        CompareVerdict {
            big_pass: self.big.ratio() >= EXPLORER_THROUGHPUT_RATIO,
            small_pass: self.small.facet_median() <= self.small.explorer_median(),
        }
    }

    /// 사람이 읽는 표.
    pub fn to_table(&self) -> String {
        let v = self.verdict();
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let mut t = String::new();
        if self.debug_build {
            let _ = writeln!(
                t,
                "경고: 디버그 빌드입니다. 수치는 --release 빌드에서 재야 의미가 있습니다."
            );
        }
        let spec = &self.spec;
        let or_unknown = |s: &Option<String>| s.clone().unwrap_or_else(|| "알 수 없음".into());
        let _ = writeln!(t, "CPU: {}", or_unknown(&spec.cpu));
        let _ = writeln!(
            t,
            "메모리: {}",
            spec.memory_bytes.map_or("알 수 없음".into(), |b| format!(
                "{:.1}GB",
                b as f64 / 1e9
            ))
        );
        let _ = writeln!(t, "Windows: {}", or_unknown(&spec.os));
        let _ = writeln!(
            t,
            "원본: {} (디스크 {})",
            self.source.display(),
            or_unknown(&spec.disk)
        );
        let _ = writeln!(
            t,
            "대상: {} (디스크 {})",
            self.target.display(),
            or_unknown(&self.target_disk)
        );
        let _ = writeln!(t);
        let _ = writeln!(
            t,
            "| 복사 | 엔진 | 회차(ms) | 중앙값(ms) | 최악(ms) | 처리량(MB/s) |"
        );
        let _ = writeln!(t, "|---|---|---|---|---|---|");
        for (name, c) in [
            (
                format!("큰 파일 하나 {:.0}MB", self.big.bytes as f64 / 1e6),
                &self.big,
            ),
            (
                format!("작은 파일 {}개", self.small.items.saturating_sub(1)),
                &self.small,
            ),
        ] {
            for (engine, times) in [(Engine::Facet, &c.facet), (Engine::Explorer, &c.explorer)] {
                let runs = times
                    .iter()
                    .map(|d| format!("{:.0}", ms(*d)))
                    .collect::<Vec<_>>()
                    .join(", ");
                let median = Comparison::median(times);
                let _ = writeln!(
                    t,
                    "| {name} | {} | {runs} | {:.1} | {:.1} | {:.1} |",
                    engine.label(),
                    ms(median),
                    ms(crate::screen::worst(times)),
                    throughput(c.bytes, median) / 1e6
                );
            }
        }
        let _ = writeln!(t);
        let _ = writeln!(
            t,
            "큰 파일: facet 처리량이 탐색기의 {:.1}% (기준 {:.0}% 이상): {}",
            self.big.ratio() * 100.0,
            EXPLORER_THROUGHPUT_RATIO * 100.0,
            mark(v.big_pass)
        );
        let _ = writeln!(
            t,
            "작은 파일: facet {:.1}ms / 탐색기 {:.1}ms (탐색기보다 느리지 않을 것): {}",
            ms(self.small.facet_median()),
            ms(self.small.explorer_median()),
            mark(v.small_pass)
        );
        let _ = writeln!(t, "전체: {}", mark(v.pass()));
        t
    }

    /// 기계가 읽는 JSON.
    pub fn to_json(&self) -> String {
        let v = self.verdict();
        let ms = |d: Duration| format!("{:.3}", d.as_secs_f64() * 1000.0);
        let list = |ds: &[Duration]| ds.iter().map(|d| ms(*d)).collect::<Vec<_>>().join(", ");
        let opt_str = |s: &Option<String>| s.as_deref().map_or("null".to_owned(), json_string);
        let spec = &self.spec;
        let mut j = String::from("{\n");
        let _ = writeln!(
            j,
            "  \"source\": {},",
            json_string(&self.source.to_string_lossy())
        );
        let _ = writeln!(
            j,
            "  \"target\": {},",
            json_string(&self.target.to_string_lossy())
        );
        let _ = writeln!(j, "  \"debug_build\": {},", self.debug_build);
        let _ = writeln!(j, "  \"machine\": {{");
        let _ = writeln!(j, "    \"cpu\": {},", opt_str(&spec.cpu));
        let _ = writeln!(
            j,
            "    \"memory_bytes\": {},",
            spec.memory_bytes
                .map_or("null".to_owned(), |b| b.to_string())
        );
        let _ = writeln!(j, "    \"os\": {},", opt_str(&spec.os));
        let _ = writeln!(j, "    \"source_disk\": {},", opt_str(&spec.disk));
        let _ = writeln!(j, "    \"target_disk\": {}", opt_str(&self.target_disk));
        let _ = writeln!(j, "  }},");
        for (key, c, pass) in [
            ("big_file", &self.big, v.big_pass),
            ("small_files", &self.small, v.small_pass),
        ] {
            let _ = writeln!(j, "  \"{key}\": {{");
            let _ = writeln!(j, "    \"bytes\": {},", c.bytes);
            let _ = writeln!(j, "    \"items\": {},", c.items);
            for (name, times) in [("facet", &c.facet), ("explorer", &c.explorer)] {
                let median = Comparison::median(times);
                let _ = writeln!(j, "    \"{name}\": {{");
                let _ = writeln!(j, "      \"runs_ms\": [{}],", list(times));
                let _ = writeln!(j, "      \"median_ms\": {},", ms(median));
                let _ = writeln!(
                    j,
                    "      \"worst_ms\": {},",
                    ms(crate::screen::worst(times))
                );
                let _ = writeln!(
                    j,
                    "      \"median_bytes_per_second\": {:.0}",
                    throughput(c.bytes, median)
                );
                let _ = writeln!(j, "    }},");
            }
            let _ = writeln!(j, "    \"facet_to_explorer_speed\": {:.4},", c.ratio());
            let _ = writeln!(j, "    \"pass\": {pass}");
            let _ = writeln!(j, "  }},");
        }
        let _ = writeln!(j, "  \"pass\": {}", v.pass());
        j.push_str("}\n");
        j
    }
}

/// 비교 측정 계획.
#[derive(Debug, Clone)]
pub struct ComparePlan<'a> {
    /// 원본을 만들 폴더(그 안에 측정 폴더를 하나 만들고 끝나면 지운다).
    pub source: &'a Path,
    /// 복사할 대상 폴더(그 안에 측정 폴더를 하나 만들고 끝나면 지운다).
    pub target: &'a Path,
    pub big_size: u64,
    pub small_count: usize,
    pub small_size: usize,
    pub runs: usize,
}

/// 같은 복사를 facet 엔진과 탐색기로 번갈아(facet, 탐색기, facet, 탐색기, …) 각각 `runs`번 재고, 재기 전에 둘 다
/// 한 번씩 데운다. 회차마다 대상을 비운다. 만든 원본과 대상은 끝나면(실패해도) 지운다.
///
/// 늘 한 엔진 뒤에 다른 엔진이 오게 한다. 탐색기는 원본을 캐시를 거쳐 읽어 원본이 캐시에 남고, facet은 큰 파일을
/// 무버퍼로 읽어 그 원본의 캐시를 비운다. 같은 엔진이 잇달아 돌면 탐색기만 캐시에 남은 원본을 읽게 되어(측정에서
/// 1GiB가 0.3초) 공정하지 않다.
pub fn compare(plan: &ComparePlan<'_>) -> Result<CompareResult, String> {
    if !cfg!(windows) {
        return Err("탐색기 비교 측정은 Windows에서만 잴 수 있습니다(IFileOperation)".into());
    }
    let name = format!("facet-copy-compare-{}", std::process::id());
    let source_root = plan.source.join(&name);
    let target_root = plan.target.join(&name);
    if source_root == target_root {
        return Err("원본 폴더와 대상 폴더가 같습니다".into());
    }
    println!(
        "원본 만드는 중: {} (큰 파일 {:.0}MB, 작은 파일 {}개)",
        source_root.display(),
        plan.big_size as f64 / 1e6,
        plan.small_count
    );
    let fixture = CopyFixture::create(
        &source_root,
        plan.big_size,
        plan.small_count,
        plan.small_size,
    )
    .map_err(|e| format!("{}: {e}", source_root.display()))?;
    let result = (|| {
        let big = compare_one(
            std::slice::from_ref(&fixture.big),
            &target_root,
            fixture.big_size,
            fixture.big_items(),
            plan.runs,
        )?;
        let small = compare_one(
            std::slice::from_ref(&fixture.small),
            &target_root,
            fixture.small_bytes(),
            fixture.small_items(),
            plan.runs,
        )?;
        Ok::<_, String>((big, small))
    })();
    fixture.remove();
    let _ = fs::remove_dir_all(&target_root);
    let (big, small) = result?;
    let spec = SystemSpec::collect(plan.source);
    let target_disk = SystemSpec::collect(plan.target).disk;
    Ok(CompareResult {
        source: plan.source.to_path_buf(),
        target: plan.target.to_path_buf(),
        big,
        small,
        spec,
        target_disk,
        debug_build: cfg!(debug_assertions),
    })
}

fn compare_one(
    sources: &[PathBuf],
    target: &Path,
    bytes: u64,
    items: u64,
    runs: usize,
) -> Result<Comparison, String> {
    let mut queue = JobQueue::new();
    let mut once = |engine| -> Result<Duration, String> {
        clear_dir(target).map_err(|e| format!("{}: {e}", target.display()))?;
        let time = copy_once(engine, &mut queue, sources, target, items)?;
        println!("  {}: {:.1}ms", engine.label(), time.as_secs_f64() * 1000.0);
        Ok(time)
    };
    println!("{} 복사 데우기", sources[0].display());
    once(Engine::Facet)?;
    once(Engine::Explorer)?;
    let mut comparison = Comparison {
        bytes,
        items,
        facet: Vec::with_capacity(runs),
        explorer: Vec::with_capacity(runs),
    };
    for run in 0..runs {
        println!("{}회", run + 1);
        for engine in [Engine::Facet, Engine::Explorer] {
            let time = once(engine)?;
            match engine {
                Engine::Facet => comparison.facet.push(time),
                Engine::Explorer => comparison.explorer.push(time),
            }
        }
    }
    Ok(comparison)
}
