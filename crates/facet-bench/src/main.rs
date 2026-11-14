//! facet 측정 도구 실행 파일. 보통 `cargo xtask test-folder`·`cargo xtask bench`로 부른다.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use facet_bench::screen::{self, DEFAULT_SCROLL_ROWS, Plan};
use facet_bench::{
    DEFAULT_COUNT, LIST_LIMIT, LIST_TARGET, check_limit, make_test_folder, measure_list,
};

const USAGE: &str = "\
사용법:
  facet-bench make-folder <경로> [--count N]
      N개(기본 100000) 항목이 든 시험 폴더를 만든다. 다시 돌려도 결과가 같다.
  facet-bench list <경로> [--count N] [--runs N] [--limit-ms N]
      시험 폴더를 만들거나 확인한 뒤 코어 목록 읽기를 N회(기본 5) 재고,
      중앙값이 상한(기본 1000ms)을 넘으면 실패한다.
  facet-bench screen <경로> --app <facet 실행 파일> [--count N] [--runs N] [--scroll-runs N]
                     [--scroll-rows N] [--refresh-hz N] [--json <파일>]
      시험 폴더를 만들거나 확인한 뒤 앱을 측정 모드로 띄워 콜드 스타트(--runs회, 기본 5)와
      스크롤 프레임 시간(--scroll-runs회, 기본 1)을 잰다. 표를 출력하고 --json이면 JSON도 쓴다.
      통과 기준을 못 맞추면 실패한다. 보통 `cargo xtask measure`로 부른다.";

const DEFAULT_RUNS: usize = 5;

struct Args {
    command: String,
    path: PathBuf,
    count: usize,
    runs: usize,
    limit: Duration,
    app: Option<PathBuf>,
    scroll_runs: usize,
    scroll_rows: usize,
    refresh_hz: Option<f64>,
    json: Option<PathBuf>,
}

fn parse(mut it: impl Iterator<Item = String>) -> Result<Args, String> {
    let command = it.next().ok_or("명령이 없습니다")?;
    let mut path = None;
    let mut count = DEFAULT_COUNT;
    let mut runs = DEFAULT_RUNS;
    let mut limit = LIST_LIMIT;
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
            "--count" => count = number("--count")? as usize,
            "--runs" => runs = (number("--runs")? as usize).max(1),
            "--scroll-runs" => scroll_runs = number("--scroll-runs")? as usize,
            "--scroll-rows" => scroll_rows = (number("--scroll-rows")? as usize).max(1),
            "--limit-ms" => limit = Duration::from_millis(number("--limit-ms")?),
            _ if path.is_none() && !arg.starts_with("--") => path = Some(PathBuf::from(arg)),
            _ => return Err(format!("알 수 없는 인자: {arg}")),
        }
    }
    let path = path.ok_or("경로가 없습니다")?;
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

fn make_folder(args: &Args) -> Result<(), String> {
    println!("시험 폴더: {} ({}항목)", args.path.display(), args.count);
    let start = Instant::now();
    let report = make_test_folder(&args.path, args.count).map_err(|e| e.to_string())?;
    println!(
        "  새로 만듦 {} / 이미 있음 {} ({:.1}s)",
        report.created,
        report.kept,
        start.elapsed().as_secs_f64()
    );
    Ok(())
}

fn list(args: &Args) -> Result<(), String> {
    if cfg!(debug_assertions) {
        println!("경고: 디버그 빌드입니다. 수치는 --release 빌드에서 재야 의미가 있습니다.");
    }
    let m = measure_list(&args.path, args.runs).map_err(|e| e.to_string())?;
    if m.item_count != args.count {
        return Err(format!(
            "항목 수가 다릅니다: {}개를 기대했지만 {}개를 읽었습니다",
            args.count, m.item_count
        ));
    }
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    println!(
        "코어 목록 읽기 ({}항목, {}회):",
        m.item_count,
        m.times.len()
    );
    for (n, t) in m.times.iter().enumerate() {
        println!("  {}회: {:.1}ms", n + 1, ms(*t));
    }
    println!(
        "  중앙값 {:.1}ms, 최악 {:.1}ms / 로컬 기준 {}ms, CI 상한 {}ms",
        ms(m.median()),
        ms(m.max()),
        LIST_TARGET.as_millis(),
        args.limit.as_millis()
    );
    check_limit(&m, args.limit)?;
    println!("  상한 안: 통과");
    Ok(())
}

fn screen(args: &Args) -> Result<(), String> {
    let app = args.app.as_deref().ok_or("--app이 없습니다")?;
    let plan = Plan {
        app,
        folder: &args.path,
        item_count: args.count,
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
