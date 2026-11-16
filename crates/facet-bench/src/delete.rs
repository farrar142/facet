//! 삭제 벤치마크와 휴지통 보내기 비교 측정.
//!
//! - **영구 삭제 벤치마크**(CI가 지킴, `facet-bench delete`): 4KiB 파일 [`DELETE_BENCH_COUNT`]개(1만 개)가 든 폴더를
//!   작업 대기열의 영구 삭제 작업([`JobQueue::delete_permanently`], 끝남 사건까지)으로 지우는 시간을 상한
//!   ([`DELETE_LIMIT`])과 견준다. 한 번 데우고 [`DELETE_BENCH_RUNS`]회 재며 회차마다 폴더를 새로 만든다(재는 시간 밖).
//! - **실행취소 대상 찾기**(CI가 지킴, 같은 명령): 작업 기록([`History`])이 상한([`HISTORY_LIMIT`], 100개)까지 찼고
//!   되돌릴 수 없는 기록이 섞여 있을 때 실행취소 대상([`History::undo_target`])을 찾는 시간을 상한
//!   ([`UNDO_TARGET_LIMIT`])과 견준다.
//! - **휴지통 보내기 비교**(사람이 잼, `facet-bench compare-recycle`): 같은 파일 1만 개([`RECYCLE_COUNT`])를 facet의
//!   휴지통 삭제 작업과 탐색기의 엔진(셸 `IFileOperation`의 휴지통 보내기, [`facet_win::recycle::recycle`])으로
//!   번갈아 보내 시간을 견준다. 셸에 매이므로 상한은 없고 비교만 낸다. 휴지통에 들어간 항목은 끝나면(실패해도)
//!   휴지통에서 지운다.

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use facet_core::history::{
    HISTORY_LIMIT, History, NotUndoable, OperationKind, Record, Stamp, Step,
};
use facet_core::job::{JobEvent, JobQueue, Summary};
use facet_win::recycle::{self, RecycleOutcome, RecycledItem};

use crate::Measurement;
use crate::copy::COPY_SMALL_SIZE;
use crate::screen::{SystemSpec, json_string, median, worst};

/// 영구 삭제 벤치마크의 파일 수: 4KiB 파일 1만 개(spec).
pub const DELETE_BENCH_COUNT: usize = 10_000;

/// 영구 삭제 벤치마크가 재는 횟수(데우기 한 번은 따로).
pub const DELETE_BENCH_RUNS: usize = 5;

/// 4KiB 파일 1만 개가 든 폴더 영구 삭제 시간의 기준(로컬, 릴리스 빌드): 1초(파일당 0.1ms).
///
/// 지우기는 파일마다 드는 일(열기·읽기 전용 확인·지우기, 백신 검사)이 정한다. GitHub 호스트 Windows 러너에서
/// 재어 보니(2026-11-16, 릴리스 빌드, 5회) 중앙값 2.0초(1.95~2.14초)였다. 로컬 SSD는 러너보다 빠르므로 그 절반인
/// 1초를 기준으로 둔다.
pub const DELETE_TARGET: Duration = Duration::from_secs(1);

/// CI가 실패시키는 영구 삭제 시간의 상한: 로컬 기준 [`DELETE_TARGET`]의 10배(10초, 파일당 1ms).
///
/// 러너 중앙값(2.0초)의 다섯 배라 러너 디스크 편차(복사 벤치마크는 처리량이 네다섯 배 떨어진 실행도 있었다)를
/// 견디고, 파일마다 일이 한 단계 늘어나는 실수(파일마다 동기 쓰기·디스크 내보내기, 항목마다 목록 다시 읽기)는
/// 잡는다.
pub const DELETE_LIMIT: Duration = Duration::from_secs(10);

/// 실행취소 대상 찾기 기준(로컬): 1ms. 마지막 기록을 보는 일이라 마이크로초 아래가 정상이고, 기록 수에 비례해
/// 느려지는 실수(전체 훑기·복제)를 잡으려는 값이다.
pub const UNDO_TARGET_TARGET: Duration = Duration::from_millis(1);

/// CI가 실패시키는 실행취소 대상 찾기 상한: 기준의 10배(10ms).
pub const UNDO_TARGET_LIMIT: Duration = Duration::from_millis(10);

/// 실행취소 대상 찾기를 재는 횟수.
pub const UNDO_TARGET_RUNS: usize = 5;

/// 휴지통 보내기 비교의 파일 수: 1만 개.
pub const RECYCLE_COUNT: usize = 10_000;

/// 작업 하나를 기다리는 최대 시간. 넘으면 실패로 본다(엔진이 멈춘 것).
const JOB_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// `dir`(없어야 한다)을 만들고 그 안에 4KiB 파일 `count`개를 만든다. 내용은 0이 아닌 무늬다.
pub fn create_files(dir: &Path, count: usize) -> io::Result<Vec<PathBuf>> {
    fs::create_dir_all(dir)?;
    let content = vec![0xA5u8; COPY_SMALL_SIZE];
    let mut paths = Vec::with_capacity(count);
    for i in 0..count {
        let path = dir.join(format!("삭제 시험 {i:05}.bin"));
        File::create(&path)?.write_all(&content)?;
        paths.push(path);
    }
    Ok(paths)
}

/// 영구 삭제 작업 하나를 넣어 끝남 사건까지 기다린다.
fn run_delete(
    queue: &mut JobQueue,
    folder: &Path,
    expected_items: u64,
) -> Result<Duration, String> {
    let start = Instant::now();
    let id = queue
        .delete_permanently(vec![folder.to_path_buf()])
        .map_err(|e| format!("삭제 작업을 만들지 못했습니다: {e:?}"))?;
    let deadline = start + JOB_TIMEOUT;
    let summary = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match queue.wait_event(left) {
            Some(JobEvent::Finished { job, summary }) if job == id => break summary,
            Some(JobEvent::ItemError { .. }) => {
                return Err("삭제 작업이 항목별 오류로 답을 기다립니다".into());
            }
            Some(_) => {}
            None => return Err("삭제 작업이 제때 끝나지 않았습니다".into()),
        }
    };
    let elapsed = start.elapsed();
    if !summary.failures.is_empty() || summary.items_deleted != expected_items || folder.exists() {
        return Err(format!(
            "삭제 작업이 다 지우지 못했습니다(기대한 항목 {expected_items}개): {summary:?}"
        ));
    }
    Ok(elapsed)
}

/// `root` 아래에 4KiB 파일 `count`개가 든 폴더를 만들어 영구 삭제 작업으로 지우는 시간을 `runs`번 잰다. 재기 전에
/// 한 번 해 데운다. 회차마다 폴더를 새로 만든다(재는 시간 밖). [`Measurement::item_count`]는 지운 항목 수(폴더
/// 포함)다.
pub fn measure_permanent_delete(
    root: &Path,
    count: usize,
    runs: usize,
) -> Result<Measurement, String> {
    let mut queue = JobQueue::new();
    let folder = root.join("삭제할 폴더");
    let expected = count as u64 + 1;
    let result = (|| {
        let mut once = || -> Result<Duration, String> {
            let _ = fs::remove_dir_all(&folder);
            create_files(&folder, count).map_err(|e| format!("{}: {e}", folder.display()))?;
            run_delete(&mut queue, &folder, expected)
        };
        once()?;
        let times = (0..runs).map(|_| once()).collect::<Result<Vec<_>, _>>()?;
        Ok(Measurement {
            item_count: expected as usize,
            times,
        })
    })();
    let _ = fs::remove_dir_all(&folder);
    result
}

/// 되돌릴 수 있는 기록과 없는 기록이 섞여 [`HISTORY_LIMIT`]개까지 찬 작업 기록. 마지막 기록은 되돌릴 수 없는
/// 것이고(대상 찾기가 까닭까지 본다), 앞에는 되돌릴 수 있는 기록이 섞여 있다.
pub fn full_history() -> History {
    let stamp = Stamp {
        is_folder: true,
        size: 0,
        modified: SystemTime::UNIX_EPOCH,
    };
    let mut history = History::new();
    // 한도를 넘겨 채워 가장 오래된 것이 빠지는 것까지 지나게 한다. 마지막 기록이 되돌릴 수 없는 것이 되게
    // 번호를 맞춘다.
    let total = HISTORY_LIMIT + 20;
    for i in 0..total {
        let path = PathBuf::from(format!(r"C:\시험\항목 {i:03}"));
        let record = if i % 3 == 2 || i + 1 == total {
            Record::not_undoable(
                OperationKind::Delete,
                vec![Step::Deleted { path }],
                NotUndoable::PermanentDelete,
            )
        } else {
            Record::new(
                OperationKind::NewFolder,
                vec![Step::CreatedFolder { path, stamp }],
            )
        };
        history.record(record);
    }
    history
}

/// 작업 기록 100개에서 실행취소 대상을 찾는 시간을 `runs`번 잰다(대상과 되돌릴 수 없는 까닭까지 읽는다).
/// [`Measurement::item_count`]는 기록 수다.
pub fn measure_undo_target(runs: usize) -> Result<Measurement, String> {
    let history = full_history();
    if history.records().len() != HISTORY_LIMIT {
        return Err(format!(
            "작업 기록이 {HISTORY_LIMIT}개가 아닙니다: {}개",
            history.records().len()
        ));
    }
    let find = |history: &History| {
        std::hint::black_box(history)
            .undo_target()
            .map(|record| (record.id(), record.not_undoable_reason()))
    };
    // 데운다.
    let target = find(&history).ok_or("실행취소 대상이 없습니다")?;
    if target.1.is_none() {
        return Err("마지막 기록은 되돌릴 수 없는 기록이어야 합니다".into());
    }
    let times = (0..runs)
        .map(|_| {
            let start = Instant::now();
            let found = find(&history);
            let elapsed = start.elapsed();
            std::hint::black_box(found);
            elapsed
        })
        .collect();
    Ok(Measurement {
        item_count: history.records().len(),
        times,
    })
}

/// 휴지통에서 항목을 영구히 지운다: 내용(`$R...`)과 정보 파일(`$I...`). 이미 없으면 그대로 둔다.
pub fn purge(items: &[RecycledItem]) {
    for item in items {
        let recycled = item.recycled();
        if recycled.is_dir() {
            let _ = fs::remove_dir_all(recycled);
        } else {
            let _ = fs::remove_file(recycled);
        }
        if let Some(name) = recycled.file_name() {
            let name = name.to_string_lossy().replacen("$R", "$I", 1);
            let _ = fs::remove_file(recycled.with_file_name(name));
        }
    }
}

/// 휴지통으로 보내는 쪽.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecycleEngine {
    /// facet 엔진: 작업 대기열의 휴지통 삭제 작업.
    Facet,
    /// 셸의 `IFileOperation`(탐색기의 휴지통 보내기).
    Explorer,
}

impl RecycleEngine {
    pub fn label(self) -> &'static str {
        match self {
            RecycleEngine::Facet => "facet",
            RecycleEngine::Explorer => "탐색기(IFileOperation)",
        }
    }
}

/// `paths`를 `engine`으로 휴지통에 한 번 보내고 걸린 시간을 돌려준다. 하나라도 휴지통에 들어가지 못하면 실패한다.
/// 휴지통에 들어간 항목은 성공·실패와 상관없이 `items`에 담아 부른 쪽이 치우게 한다.
fn recycle_once(
    engine: RecycleEngine,
    queue: &mut JobQueue,
    paths: &[PathBuf],
    items: &mut Vec<RecycledItem>,
) -> Result<Duration, String> {
    let start = Instant::now();
    match engine {
        RecycleEngine::Facet => {
            let id = queue
                .recycle(paths.to_vec())
                .map_err(|e| format!("휴지통 삭제 작업을 만들지 못했습니다: {e:?}"))?;
            let deadline = start + JOB_TIMEOUT;
            let mut finished: Option<Summary> = None;
            let mut problem = None;
            while finished.is_none() && problem.is_none() {
                let left = deadline.saturating_duration_since(Instant::now());
                match queue.wait_event(left) {
                    Some(JobEvent::Finished { job, summary }) if job == id => {
                        finished = Some(summary);
                    }
                    Some(JobEvent::NotRecyclable { .. }) => {
                        problem = Some("이 계정은 휴지통을 쓸 수 없습니다(셸이 영구 삭제하려 함)");
                    }
                    Some(JobEvent::ItemError { .. }) => {
                        problem = Some("휴지통 삭제 작업이 항목별 오류로 답을 기다립니다");
                    }
                    Some(_) => {}
                    None => problem = Some("휴지통 삭제 작업이 제때 끝나지 않았습니다"),
                }
            }
            if let Some(problem) = problem {
                // 멈춘 작업은 취소하고, 취소 뒤 끝남 사건이 담은 항목은 치우게 모은다.
                let _ = queue.cancel(id);
                while let Some(event) = queue.wait_event(Duration::from_secs(30)) {
                    if let JobEvent::Finished { job, summary } = event
                        && job == id
                    {
                        items.extend(summary.recycled.iter().cloned());
                        break;
                    }
                }
                return Err(problem.into());
            }
            let summary = finished.expect("끝남 사건");
            let elapsed = start.elapsed();
            items.extend(summary.recycled.iter().cloned());
            if !summary.failures.is_empty() || summary.recycled.len() != paths.len() {
                return Err(format!(
                    "휴지통 삭제 작업이 다 보내지 못했습니다(기대한 항목 {}개): {summary:?}",
                    paths.len()
                ));
            }
            Ok(elapsed)
        }
        RecycleEngine::Explorer => {
            let outcomes = recycle::recycle(paths);
            let elapsed = start.elapsed();
            let mut bad = 0;
            for outcome in outcomes {
                match outcome {
                    RecycleOutcome::Recycled(item) => items.push(item),
                    _ => bad += 1,
                }
            }
            if bad != 0 {
                return Err(format!(
                    "탐색기 엔진이 {bad}개를 휴지통으로 보내지 못했습니다(이 계정은 휴지통을 쓸 수 없을 수 있음)"
                ));
            }
            Ok(elapsed)
        }
    }
}

/// 휴지통 보내기 비교 결과.
#[derive(Debug, Clone, PartialEq)]
pub struct RecycleComparison {
    /// 파일을 만든 폴더.
    pub folder: PathBuf,
    /// 보낸 파일 수.
    pub count: usize,
    pub facet: Vec<Duration>,
    pub explorer: Vec<Duration>,
    pub spec: SystemSpec,
    pub debug_build: bool,
}

impl RecycleComparison {
    pub fn facet_median(&self) -> Duration {
        median(&self.facet)
    }

    pub fn explorer_median(&self) -> Duration {
        median(&self.explorer)
    }

    /// 중앙값의 비(탐색기 / facet). 1보다 크면 facet이 빠르다.
    pub fn facet_speedup(&self) -> f64 {
        self.explorer_median().as_secs_f64() / self.facet_median().as_secs_f64().max(1e-9)
    }

    /// 사람이 읽는 표. 상한이 없어 통과 여부는 없고 비교만 낸다.
    pub fn to_table(&self) -> String {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let or_unknown = |s: &Option<String>| s.clone().unwrap_or_else(|| "알 수 없음".into());
        let mut t = String::new();
        if self.debug_build {
            let _ = writeln!(
                t,
                "경고: 디버그 빌드입니다. 수치는 --release 빌드에서 재야 의미가 있습니다."
            );
        }
        let _ = writeln!(t, "CPU: {}", or_unknown(&self.spec.cpu));
        let _ = writeln!(
            t,
            "메모리: {}",
            self.spec
                .memory_bytes
                .map_or("알 수 없음".into(), |b| format!(
                    "{:.1}GB",
                    b as f64 / 1e9
                ))
        );
        let _ = writeln!(t, "Windows: {}", or_unknown(&self.spec.os));
        let _ = writeln!(
            t,
            "폴더: {} (디스크 {})",
            self.folder.display(),
            or_unknown(&self.spec.disk)
        );
        let _ = writeln!(t);
        let _ = writeln!(
            t,
            "| 휴지통 보내기 {}개 | 회차(ms) | 중앙값(ms) | 최악(ms) | 항목당(ms) |",
            self.count
        );
        let _ = writeln!(t, "|---|---|---|---|---|");
        for (engine, times) in [
            (RecycleEngine::Facet, &self.facet),
            (RecycleEngine::Explorer, &self.explorer),
        ] {
            let runs = times
                .iter()
                .map(|d| format!("{:.0}", ms(*d)))
                .collect::<Vec<_>>()
                .join(", ");
            let med = median(times);
            let _ = writeln!(
                t,
                "| {} | {runs} | {:.1} | {:.1} | {:.3} |",
                engine.label(),
                ms(med),
                ms(worst(times)),
                ms(med) / self.count.max(1) as f64
            );
        }
        let _ = writeln!(t);
        let _ = writeln!(
            t,
            "facet 중앙값은 탐색기의 {:.2}배 속도(상한 없음, 비교만)",
            self.facet_speedup()
        );
        t
    }

    /// 기계가 읽는 JSON.
    pub fn to_json(&self) -> String {
        let ms = |d: Duration| format!("{:.3}", d.as_secs_f64() * 1000.0);
        let list = |ds: &[Duration]| ds.iter().map(|d| ms(*d)).collect::<Vec<_>>().join(", ");
        let opt = |s: &Option<String>| s.as_deref().map_or("null".to_owned(), json_string);
        let mut j = String::from("{\n");
        let _ = writeln!(
            j,
            "  \"folder\": {},",
            json_string(&self.folder.to_string_lossy())
        );
        let _ = writeln!(j, "  \"count\": {},", self.count);
        let _ = writeln!(j, "  \"debug_build\": {},", self.debug_build);
        let _ = writeln!(j, "  \"machine\": {{");
        let _ = writeln!(j, "    \"cpu\": {},", opt(&self.spec.cpu));
        let _ = writeln!(
            j,
            "    \"memory_bytes\": {},",
            self.spec
                .memory_bytes
                .map_or("null".to_owned(), |b| b.to_string())
        );
        let _ = writeln!(j, "    \"os\": {},", opt(&self.spec.os));
        let _ = writeln!(j, "    \"disk\": {}", opt(&self.spec.disk));
        let _ = writeln!(j, "  }},");
        for (name, times) in [("facet", &self.facet), ("explorer", &self.explorer)] {
            let _ = writeln!(j, "  \"{name}\": {{");
            let _ = writeln!(j, "    \"runs_ms\": [{}],", list(times));
            let _ = writeln!(j, "    \"median_ms\": {},", ms(median(times)));
            let _ = writeln!(j, "    \"worst_ms\": {}", ms(worst(times)));
            let _ = writeln!(j, "  }},");
        }
        let _ = writeln!(
            j,
            "  \"facet_speedup_over_explorer\": {:.4}",
            self.facet_speedup()
        );
        j.push_str("}\n");
        j
    }
}

/// `root` 안에 파일 `count`개를 만들어 facet 휴지통 삭제 작업과 탐색기 엔진으로 번갈아(facet, 탐색기, …) 각각
/// `runs`번 휴지통에 보내 견준다. 재기 전에 둘 다 한 번씩 데운다. 회차마다 파일을 새로 만들고(재는 시간 밖), 휴지통에
/// 들어간 항목은 회차가 끝날 때마다(실패해도) 휴지통에서 지운다. 만든 폴더도 끝나면 지운다.
pub fn compare_recycle(
    root: &Path,
    count: usize,
    runs: usize,
) -> Result<RecycleComparison, String> {
    if !cfg!(windows) {
        return Err("휴지통 보내기 비교는 Windows에서만 잴 수 있습니다(IFileOperation)".into());
    }
    let folder = root.join(format!("facet-recycle-compare-{}", std::process::id()));
    fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    if !recycle::has_recycle_bin(&folder) {
        let _ = fs::remove_dir_all(&folder);
        return Err(format!(
            "{}의 볼륨에는 휴지통이 없습니다. 휴지통이 있는(고정 디스크) 폴더로 재세요",
            folder.display()
        ));
    }
    let mut queue = JobQueue::new();
    let result = (|| {
        let mut once = |engine: RecycleEngine| -> Result<Duration, String> {
            let dir = folder.join("파일");
            let _ = fs::remove_dir_all(&dir);
            let paths = create_files(&dir, count).map_err(|e| format!("{}: {e}", dir.display()))?;
            let mut items = Vec::new();
            let time = recycle_once(engine, &mut queue, &paths, &mut items);
            // 휴지통에 들어간 항목은 성공·실패와 상관없이 휴지통에서 지운다.
            purge(&items);
            let _ = fs::remove_dir_all(&dir);
            let time = time?;
            println!("  {}: {:.1}ms", engine.label(), time.as_secs_f64() * 1000.0);
            Ok(time)
        };
        println!("휴지통 보내기 {count}개 데우기");
        once(RecycleEngine::Facet)?;
        once(RecycleEngine::Explorer)?;
        let mut facet = Vec::with_capacity(runs);
        let mut explorer = Vec::with_capacity(runs);
        for run in 0..runs {
            println!("{}회", run + 1);
            facet.push(once(RecycleEngine::Facet)?);
            explorer.push(once(RecycleEngine::Explorer)?);
        }
        Ok::<_, String>((facet, explorer))
    })();
    let _ = fs::remove_dir_all(&folder);
    let (facet, explorer) = result?;
    Ok(RecycleComparison {
        folder: root.to_path_buf(),
        count,
        facet,
        explorer,
        spec: SystemSpec::collect(root),
        debug_build: cfg!(debug_assertions),
    })
}
