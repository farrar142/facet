//! facet 측정 도구 실행 파일. 보통 `cargo xtask test-folder`·`cargo xtask bench`로 부른다.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use facet_bench::{
    BENCH_QUERY, BENCH_RESULT_LIMIT, DEFAULT_COUNT, INDEX_COUNT, INDEX_MEMORY_LIMIT, LIST_LIMIT,
    LIST_TARGET, Measurement, QUERY_LIMIT, QUERY_TARGET, check_limit, check_memory,
    check_query_limit, make_test_folder, measure_list, measure_query, synthetic_index,
};

const USAGE: &str = "\
사용법:
  facet-bench make-folder <경로> [--count N]
      N개(기본 100000) 항목이 든 시험 폴더를 만든다. 다시 돌려도 결과가 같다.
  facet-bench list <경로> [--count N] [--runs N] [--limit-ms N]
      시험 폴더를 만들거나 확인한 뒤 코어 목록 읽기를 N회(기본 5) 재고,
      중앙값이 상한(기본 1000ms)을 넘으면 실패한다.
  facet-bench index [--count N] [--runs N] [--limit-ms N]
      항목 N개(기본 1000000)의 합성 인덱스를 만들어 메모리를 재고(150MB 이상이면 실패),
      부분 일치 질의(결과 100개 제한)를 N회(기본 5) 재어 중앙값이 상한(기본 300ms)을
      넘으면 실패한다.";

const DEFAULT_RUNS: usize = 5;

struct Args {
    command: String,
    path: Option<PathBuf>,
    count: Option<usize>,
    runs: usize,
    limit: Option<Duration>,
}

impl Args {
    fn path(&self) -> Result<&PathBuf, String> {
        self.path
            .as_ref()
            .ok_or_else(|| "경로가 없습니다".to_owned())
    }
}

fn parse(mut it: impl Iterator<Item = String>) -> Result<Args, String> {
    let command = it.next().ok_or("명령이 없습니다")?;
    let mut path = None;
    let mut count = None;
    let mut runs = DEFAULT_RUNS;
    let mut limit = None;
    while let Some(arg) = it.next() {
        let mut number = |name: &str| -> Result<u64, String> {
            let v = it.next().ok_or(format!("{name} 뒤에 값이 없습니다"))?;
            v.parse()
                .map_err(|_| format!("{name}의 값이 수가 아닙니다: {v}"))
        };
        match arg.as_str() {
            "--count" => count = Some(number("--count")? as usize),
            "--runs" => runs = (number("--runs")? as usize).max(1),
            "--limit-ms" => limit = Some(Duration::from_millis(number("--limit-ms")?)),
            _ if path.is_none() && !arg.starts_with("--") => path = Some(PathBuf::from(arg)),
            _ => return Err(format!("알 수 없는 인자: {arg}")),
        }
    }
    Ok(Args {
        command,
        path,
        count,
        runs,
        limit,
    })
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let result = match args.command.as_str() {
        "make-folder" => make_folder(&args),
        "list" => make_folder(&args).and_then(|()| list(&args)),
        "index" => index(&args),
        other => Err(format!("알 수 없는 명령: {other}\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("\n{e}");
            ExitCode::FAILURE
        }
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn warn_debug() {
    if cfg!(debug_assertions) {
        println!("경고: 디버그 빌드입니다. 수치는 --release 빌드에서 재야 의미가 있습니다.");
    }
}

fn print_times(m: &Measurement) {
    for (n, t) in m.times.iter().enumerate() {
        println!("  {}회: {:.1}ms", n + 1, ms(*t));
    }
}

fn make_folder(args: &Args) -> Result<(), String> {
    let path = args.path()?;
    let count = args.count.unwrap_or(DEFAULT_COUNT);
    println!("시험 폴더: {} ({}항목)", path.display(), count);
    let start = Instant::now();
    let report = make_test_folder(path, count).map_err(|e| e.to_string())?;
    println!(
        "  새로 만듦 {} / 이미 있음 {} ({:.1}s)",
        report.created,
        report.kept,
        start.elapsed().as_secs_f64()
    );
    Ok(())
}

fn list(args: &Args) -> Result<(), String> {
    warn_debug();
    let count = args.count.unwrap_or(DEFAULT_COUNT);
    let limit = args.limit.unwrap_or(LIST_LIMIT);
    let m = measure_list(args.path()?, args.runs).map_err(|e| e.to_string())?;
    if m.item_count != count {
        return Err(format!(
            "항목 수가 다릅니다: {}개를 기대했지만 {}개를 읽었습니다",
            count, m.item_count
        ));
    }
    println!(
        "코어 목록 읽기 ({}항목, {}회):",
        m.item_count,
        m.times.len()
    );
    print_times(&m);
    println!(
        "  중앙값 {:.1}ms, 최악 {:.1}ms / 로컬 기준 {}ms, CI 상한 {}ms",
        ms(m.median()),
        ms(m.max()),
        LIST_TARGET.as_millis(),
        limit.as_millis()
    );
    check_limit(&m, limit)?;
    println!("  상한 안: 통과");
    Ok(())
}

fn index(args: &Args) -> Result<(), String> {
    if args.path.is_some() {
        return Err(format!("index 명령은 경로를 받지 않습니다\n\n{USAGE}"));
    }
    warn_debug();
    let count = args.count.unwrap_or(INDEX_COUNT);
    let limit = args.limit.unwrap_or(QUERY_LIMIT);

    let start = Instant::now();
    let index = synthetic_index(count);
    let bytes = index.memory_bytes();
    println!(
        "합성 인덱스 ({}항목): 만들기 {:.1}ms, 메모리 {:.1}MB / 상한 {:.1}MB",
        index.len(),
        ms(start.elapsed()),
        bytes as f64 / 1e6,
        INDEX_MEMORY_LIMIT as f64 / 1e6
    );
    check_memory(bytes, INDEX_MEMORY_LIMIT)?;

    let query = facet_index::Query::name_contains(BENCH_QUERY).limit(BENCH_RESULT_LIMIT);
    let m = measure_query(&index, &query, args.runs);
    println!(
        "인덱스 부분 일치 질의 (\"{BENCH_QUERY}\", 결과 {}개 제한, {}개 나옴, {}회):",
        BENCH_RESULT_LIMIT,
        m.item_count,
        m.times.len()
    );
    print_times(&m);
    println!(
        "  중앙값 {:.1}ms, 최악 {:.1}ms / 로컬 기준 {}ms, CI 상한 {}ms",
        ms(m.median()),
        ms(m.max()),
        QUERY_TARGET.as_millis(),
        limit.as_millis()
    );
    check_query_limit(&m, limit)?;
    println!("  상한 안: 통과");
    Ok(())
}
