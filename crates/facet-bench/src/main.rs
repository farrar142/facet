//! facet 측정 도구 실행 파일. 보통 `cargo xtask test-folder`·`cargo xtask bench`로 부른다.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use facet_bench::screen::{self, DEFAULT_SCROLL_ROWS, Plan};
use facet_bench::{
    BENCH_QUERY, BENCH_RESULT_LIMIT, BENCH_SORT, BENCH_SORTED_QUERY, BenchCaller, DEFAULT_COUNT,
    INDEX_COUNT, INDEX_MEMORY_LIMIT, LIST_LIMIT, LIST_SORT_LIMIT, LIST_SORT_TARGET, LIST_TARGET,
    MFT_PARSE_LIMIT, MFT_PARSE_TARGET, MFT_RECORDS, Measurement, QUERY_LIMIT, QUERY_TARGET,
    RESORT_LIMIT, RESORT_TARGET, SYNTHETIC_VOLUME, check_limit, check_median, check_memory,
    check_mft_limit, check_query_limit, make_test_folder, measure_filtered_query, measure_list,
    measure_list_sorted, measure_mft, measure_query, measure_resort, mft_image, synthetic_index,
};

const USAGE: &str = "\
사용법:
  facet-bench make-folder <경로> [--count N]
      N개(기본 100000) 항목이 든 시험 폴더를 만든다. 다시 돌려도 결과가 같다.
  facet-bench list <경로> [--count N] [--runs N] [--limit-ms N]
      시험 폴더를 만들거나 확인한 뒤 코어 목록 읽기를 N회(기본 5) 재고,
      중앙값이 상한(기본 1000ms)을 넘으면 실패한다. 이어서 「읽기+정렬」(상한 1500ms)과
      「정렬 바꾸기」(상한 500ms)도 N회씩 재어 중앙값이 상한을 넘으면 실패한다.
      --limit-ms는 목록 읽기의 상한만 바꾼다.
  facet-bench index [--count N] [--runs N] [--limit-ms N]
      항목 N개(기본 1000000)의 합성 인덱스를 만들어 메모리를 재고(150MB 이상이면 실패),
      부분 일치 질의와 결과 10만 개를 경로로 정렬하는 질의(둘 다 결과 100개 제한)를
      N회(기본 5)씩 재어 중앙값이 상한(기본 300ms)을 넘으면 실패한다.
      인덱스 파일 저장·읽기 시간도 출력한다(상한 없음).
  facet-bench mft [--count N] [--runs N] [--limit-ms N]
      저장소의 MFT 조각을 되풀이해 레코드 N개(기본 1000000)의 MFT 덩어리를 만들고, 그것을
      인덱스로 만드는 시간을 N회(기본 5) 재어 중앙값이 상한(기본 20000ms)을 넘으면 실패한다.
  facet-bench screen <경로> --app <facet 실행 파일> [--count N] [--runs N] [--scroll-runs N]
                     [--scroll-rows N] [--refresh-hz N] [--json <파일>]
      시험 폴더를 만들거나 확인한 뒤 앱을 측정 모드로 띄워 콜드 스타트(--runs회, 기본 5),
      시험 폴더의 상위로 띄운 창에서 시험 폴더로 이동한 폴더 열기 첫 화면(--runs회),
      스크롤 프레임 시간(--scroll-runs회, 기본 1)을 잰다. 표를 출력하고 --json이면 JSON도 쓴다.
      통과 기준을 못 맞추면 실패한다. 보통 `cargo xtask measure`로 부른다.";

const DEFAULT_RUNS: usize = 5;

struct Args {
    command: String,
    path: Option<PathBuf>,
    count: Option<usize>,
    runs: usize,
    limit: Option<Duration>,
    app: Option<PathBuf>,
    scroll_runs: usize,
    scroll_rows: usize,
    refresh_hz: Option<f64>,
    json: Option<PathBuf>,
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
    let mut app = None;
    let mut scroll_runs = 1;
    let mut scroll_rows = DEFAULT_SCROLL_ROWS;
    let mut refresh_hz = None;
    let mut json = None;
    while let Some(arg) = it.next() {
        let mut text = |name: &str| it.next().ok_or(format!("{name} 뒤에 값이 없습니다"));
        match arg.as_str() {
            "--app" => {
                app = Some(PathBuf::from(text("--app")?));
                continue;
            }
            "--json" => {
                json = Some(PathBuf::from(text("--json")?));
                continue;
            }
            "--refresh-hz" => {
                let v = text("--refresh-hz")?;
                refresh_hz = Some(
                    v.parse::<f64>()
                        .ok()
                        .filter(|hz| *hz > 0.0)
                        .ok_or(format!("--refresh-hz의 값이 양수가 아닙니다: {v}"))?,
                );
                continue;
            }
            _ => {}
        }
        let mut number = |name: &str| -> Result<u64, String> {
            let v = it.next().ok_or(format!("{name} 뒤에 값이 없습니다"))?;
            v.parse()
                .map_err(|_| format!("{name}의 값이 수가 아닙니다: {v}"))
        };
        match arg.as_str() {
            "--count" => count = Some(number("--count")? as usize),
            "--runs" => runs = (number("--runs")? as usize).max(1),
            "--scroll-runs" => scroll_runs = number("--scroll-runs")? as usize,
            "--scroll-rows" => scroll_rows = (number("--scroll-rows")? as usize).max(1),
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
        app,
        scroll_runs,
        scroll_rows,
        refresh_hz,
        json,
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
        "mft" => mft(&args),
        "screen" => make_folder(&args).and_then(|()| screen(&args)),
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

    let path = args.path()?;
    let m = measure_list_sorted(path, args.runs).map_err(|e| e.to_string())?;
    println!(
        "코어 읽기+정렬 ({}항목, 이름 오름차순, {}회):",
        m.item_count,
        m.times.len()
    );
    report_median("읽기+정렬", &m, LIST_SORT_TARGET, LIST_SORT_LIMIT)?;

    let m = measure_resort(path, args.runs).map_err(|e| e.to_string())?;
    println!(
        "코어 정렬 바꾸기 ({}항목, 열·방향을 차례로 바꿈, {}회):",
        m.item_count,
        m.times.len()
    );
    report_median("정렬 바꾸기", &m, RESORT_TARGET, RESORT_LIMIT)
}

/// 측정값을 출력하고 중앙값이 `limit`을 넘으면 실패한다.
fn report_median(
    what: &str,
    m: &Measurement,
    target: Duration,
    limit: Duration,
) -> Result<(), String> {
    print_times(m);
    println!(
        "  중앙값 {:.1}ms, 최악 {:.1}ms / 로컬 기준 {}ms, CI 상한 {}ms",
        ms(m.median()),
        ms(m.max()),
        target.as_millis(),
        limit.as_millis()
    );
    check_median(what, m, limit)?;
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
    report_store(&index, args.runs)?;

    let query = facet_index::Query::name_contains(BENCH_QUERY).limit(BENCH_RESULT_LIMIT);
    let m = measure_query(&index, &query, args.runs);
    println!(
        "인덱스 부분 일치 질의 (\"{BENCH_QUERY}\", 결과 {}개 제한, {}개 나옴, {}회):",
        BENCH_RESULT_LIMIT,
        m.item_count,
        m.times.len()
    );
    report_query(&m, limit)?;

    let (key, order) = BENCH_SORT;
    let query = facet_index::Query::name_contains(BENCH_SORTED_QUERY)
        .sort(key, order)
        .limit(BENCH_RESULT_LIMIT);
    let total = index.search(&query).total;
    let m = measure_query(&index, &query, args.runs);
    println!(
        "인덱스 정렬 질의 (\"{BENCH_SORTED_QUERY}\", {key:?} {order:?}, 결과 {}개 제한, 전체 {total}개 중 {}개 나옴, {}회):",
        BENCH_RESULT_LIMIT,
        m.item_count,
        m.times.len()
    );
    report_query(&m, limit)?;
    report_filtered(index, args.runs, limit)
}

/// 폴더 권한으로 거르는 질의(인덱서 서비스가 파이프로 받는 질의)를 권한 검사 기억이 찬 상태로
/// 잰다. 상한은 거르지 않는 질의와 같다.
fn report_filtered(index: facet_index::Index, runs: usize, limit: Duration) -> Result<(), String> {
    use facet_protocol::{Case, Condition, QueryMessage};

    let volumes = facet_indexer::volumes::Volumes::with_index(SYNTHETIC_VOLUME, index);
    let caller = BenchCaller::default();
    let query = |part: &str, sort| QueryMessage {
        condition: Condition::NameContains {
            part: part.to_owned(),
            case: Case::Insensitive,
        },
        sort,
        limit: Some(BENCH_RESULT_LIMIT as u32),
    };
    let (key, order) = BENCH_SORT;
    let sort = Some((key.into(), order.into()));
    for (part, sort) in [(BENCH_QUERY, None), (BENCH_SORTED_QUERY, sort)] {
        let q = query(part, sort);
        let m = measure_filtered_query(&volumes, &caller, &q, runs)?;
        // 걸러진 결과가 있어야 「걸러진 만큼 뒤 후보로 채우기」까지 잰다.
        let cancel = facet_indexer::session::Cancel::new();
        let shown = |page: facet_indexer::volumes::Page| -> Vec<String> {
            page.hits.into_iter().map(|h| h.path).collect()
        };
        let unfiltered = shown(volumes.search(&q, &cancel).map_err(|e| e.message)?);
        let filtered = shown(
            volumes
                .search_as(&q, &caller, &cancel)
                .map_err(|e| e.message)?,
        );
        let hidden = unfiltered.iter().filter(|p| !filtered.contains(p)).count();
        if sort.is_some() && hidden == 0 {
            return Err(format!(
                "권한으로 거르는 질의 \"{part}\"의 앞쪽 결과에서 걸러진 것이 없습니다(채우기를 재지 못함)"
            ));
        }
        let sorted = match sort {
            Some(_) => format!(", {key:?} {order:?}"),
            None => String::new(),
        };
        println!(
            "권한으로 거르는 질의 (\"{part}\"{sorted}, 결과 {}개 제한, {}개 나옴, 거르지 않은 앞쪽에서 {hidden}개 걸러짐, 기억한 폴더 {}개, {}회):",
            BENCH_RESULT_LIMIT,
            m.item_count,
            volumes.access().len(),
            m.times.len()
        );
        report_query(&m, limit)?;
    }
    Ok(())
}

fn mft(args: &Args) -> Result<(), String> {
    if args.path.is_some() {
        return Err(format!(
            "mft 명령은 경로를 받지 않습니다

{USAGE}"
        ));
    }
    warn_debug();
    let records = args.count.unwrap_or(MFT_RECORDS);
    let limit = args.limit.unwrap_or(MFT_PARSE_LIMIT);
    let image = mft_image(records);
    let m = measure_mft(&image, args.runs)?;
    let mb = image.len() as f64 / 1e6;
    println!(
        "MFT 파싱 (조각을 되풀이한 레코드 {records}개, {mb:.1}MB → 항목 {}개, {}회):",
        m.item_count,
        m.times.len()
    );
    print_times(&m);
    let median = m.median();
    println!(
        "  중앙값 {:.1}ms ({:.0}MB/s, 레코드 100만 개당 {:.1}ms), 최악 {:.1}ms / 로컬 기준 {}ms, CI 상한 {}ms",
        ms(median),
        mb / median.as_secs_f64().max(1e-9),
        ms(median) * 1e6 / records.max(1) as f64,
        ms(m.max()),
        MFT_PARSE_TARGET.as_millis(),
        limit.as_millis()
    );
    check_mft_limit(&m, records, limit)?;
    println!("  상한 안: 통과");
    Ok(())
}

/// 인덱스 저장·읽기 시간을 잰다(재부팅 뒤 바로 검색의 근거). 시간 상한은 두지 않고 출력만 한다.
/// 읽은 인덱스가 같은 항목 수가 아니면 실패한다.
fn report_store(index: &facet_index::Index, runs: usize) -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("facet-bench-store-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("bench.idx");
    let result = (|| {
        let mut saves = Vec::new();
        let mut loads = Vec::new();
        for _ in 0..runs {
            let t = Instant::now();
            index.save(&path).map_err(|e| e.to_string())?;
            saves.push(t.elapsed());
            let t = Instant::now();
            let loaded = facet_index::Index::load(&path).map_err(|e| e.to_string())?;
            loads.push(t.elapsed());
            if loaded.len() != index.len() {
                return Err(format!(
                    "읽은 인덱스의 항목 수가 다릅니다: {} / {}",
                    loaded.len(),
                    index.len()
                ));
            }
        }
        let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
        let median = |mut v: Vec<Duration>| {
            v.sort();
            v[v.len() / 2]
        };
        println!(
            "인덱스 파일 ({:.1}MB, {}회): 저장 중앙값 {:.1}ms, 읽기 중앙값 {:.1}ms",
            size as f64 / 1e6,
            runs,
            ms(median(saves)),
            ms(median(loads))
        );
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// 질의 측정값을 출력하고 중앙값이 `limit`을 넘으면 실패한다.
fn report_query(m: &Measurement, limit: Duration) -> Result<(), String> {
    print_times(m);
    println!(
        "  중앙값 {:.1}ms, 최악 {:.1}ms / 로컬 기준 {}ms, CI 상한 {}ms",
        ms(m.median()),
        ms(m.max()),
        QUERY_TARGET.as_millis(),
        limit.as_millis()
    );
    check_query_limit(m, limit)?;
    println!("  상한 안: 통과");
    Ok(())
}

fn screen(args: &Args) -> Result<(), String> {
    let app = args.app.as_deref().ok_or("--app이 없습니다")?;
    let plan = Plan {
        app,
        folder: args.path()?,
        item_count: args.count.unwrap_or(DEFAULT_COUNT),
        runs: args.runs,
        scroll_runs: args.scroll_runs,
        scroll_rows: args.scroll_rows,
        refresh_hz: args.refresh_hz,
    };
    let result = screen::measure(&plan)?;
    println!(
        "
{}",
        result.to_table()
    );
    if let Some(json) = &args.json {
        std::fs::write(json, result.to_json())
            .map_err(|e| format!("{}를 쓰지 못함: {e}", json.display()))?;
        println!("JSON: {}", json.display());
    }
    if result.verdict().pass() {
        Ok(())
    } else {
        Err("통과 기준을 맞추지 못했습니다".to_owned())
    }
}
