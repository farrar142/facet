//! 저장소 작업 명령. `cargo xtask <명령>`으로 부른다.
//!
//! - `ci`: 형식 검사 → lint(경고를 오류로) → 빌드 → 테스트 → 코어 벤치마크 → 인덱스 벤치마크.
//!   CI와 로컬이 같은 명령을 쓴다.
//! - `test-folder`: 시험 폴더를 만든다(`facet-bench make-folder`).
//! - `bench`: 벤치마크만 돌린다(`facet-bench list`·`facet-bench index`, 릴리스 빌드).
//!
//! - `measure [인자]`: 앱과 측정 도구를 릴리스 빌드해 화면 수치(콜드 스타트·스크롤 프레임)를 잰다.
//! - `dist <label>`: release 빌드한 facet을 portable zip과 체크섬으로 묶는다(`dist` 모듈).
//!
//! 벤치마크 상한과 그 근거는 `crates/facet-bench/src/lib.rs`의 `LIST_LIMIT`·`QUERY_LIMIT`·
//! `INDEX_MEMORY_LIMIT`에 있다.

mod dist;

use std::env;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

const USAGE: &str = "\
사용법: cargo xtask <명령>

명령:
  ci                             형식 검사, lint(경고를 오류로), 빌드, 테스트, 코어 벤치마크,
                                 인덱스 벤치마크를 차례로 돌린다
  test-folder [경로] [--count N]  N개(기본 100000) 항목이 든 시험 폴더를 만든다.
                                 경로를 빼면 임시 폴더의 facet-test-folders/items-N
  bench                          10만 항목 시험 폴더를 만들거나 확인하고 코어 목록 읽기를 잰 뒤,
                                 합성 인덱스 100만 개의 메모리와 부분 일치 질의를 잰다(릴리스 빌드)
  measure [--count N] [--runs N] [--scroll-runs N] [--scroll-rows N] [--refresh-hz N] [--json 파일] [경로]
                                 앱을 릴리스 빌드하고 시험 폴더(기본 10만 항목)를 열어 콜드 스타트 중앙값과
                                 스크롤 프레임 시간을 잰다. 표와 JSON(기본 target/measure.json)을 내고,
                                 통과 기준을 못 맞추면 실패한다. CI는 부르지 않는다(사람이 잰다)
  dist <label>                   facet을 릴리스 빌드해 target/dist/에 facet-<label>-windows-x64.zip과
                                 그 .sha256을 만든다. label은 nightly나 태그 이름(예: v0.1.0)이고
                                 영문자·숫자·`.`·`-`·`_`만 쓸 수 있다";

/// 벤치마크 폴더의 항목 수.
const BENCH_COUNT: &str = "100000";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("ci") => run_steps("ci", &ci_steps()),
        Some("bench") => run_steps("bench", &bench_steps()),
        Some("test-folder") => match test_folder_step(&args[1..]) {
            Ok(step) => run_steps("test-folder", &[step]),
            Err(e) => {
                eprintln!("{e}\n\n{USAGE}");
                ExitCode::FAILURE
            }
        },
        Some("measure") => run_measure(&args[1..]),
        Some("dist") => match dist_label(&args[1..]) {
            Ok(label) => run_dist(label),
            Err(e) => {
                eprintln!("{e}\n\n{USAGE}");
                ExitCode::FAILURE
            }
        },
        Some(other) => {
            eprintln!("알 수 없는 명령: {other}\n\n{USAGE}");
            ExitCode::FAILURE
        }
        None => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

/// 한 단계: 이름과 `cargo`에 넘길 인자.
struct Step {
    name: &'static str,
    args: Vec<String>,
}

fn step(name: &'static str, args: &[&str]) -> Step {
    Step {
        name,
        args: args.iter().map(|s| (*s).to_owned()).collect(),
    }
}

fn ci_steps() -> Vec<Step> {
    vec![
        step("형식 검사", &["fmt", "--all", "--check"]),
        step(
            "lint",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--locked",
                "--",
                "-D",
                "warnings",
            ],
        ),
        step(
            "빌드",
            &["build", "--workspace", "--all-targets", "--locked"],
        ),
        step("테스트", &["test", "--workspace", "--locked"]),
    ]
    .into_iter()
    .chain(bench_steps())
    .collect()
}

fn bench_steps() -> Vec<Step> {
    vec![bench_step(), index_bench_step()]
}

/// 릴리스 빌드한 `facet-bench`를 `cargo run`으로 부르는 인자.
fn facet_bench(command: &str) -> Vec<String> {
    [
        "run",
        "--release",
        "--locked",
        "--quiet",
        "-p",
        "facet-bench",
        "--",
        command,
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

/// 시험 폴더의 기본 위치: 임시 폴더 아래 `facet-test-folders/items-N`.
///
/// `target/` 밑에 두지 않는다. CI의 rust-cache가 `target/`을 캐시하므로
/// 10만 개 파일이 캐시에 섞여 들어가 저장·복원이 느려진다.
fn default_folder(count: &str) -> PathBuf {
    env::temp_dir()
        .join("facet-test-folders")
        .join(format!("items-{count}"))
}

fn bench_step() -> Step {
    let mut args = facet_bench("list");
    args.push(default_folder(BENCH_COUNT).display().to_string());
    args.extend(["--count".to_owned(), BENCH_COUNT.to_owned()]);
    Step {
        name: "코어 벤치마크",
        args,
    }
}

/// 합성 인덱스 100만 개의 메모리와 부분 일치 질의를 잰다. 수와 상한은 `facet-bench`의 기본값이다.
fn index_bench_step() -> Step {
    Step {
        name: "인덱스 벤치마크",
        args: facet_bench("index"),
    }
}

fn test_folder_step(rest: &[String]) -> Result<Step, String> {
    let mut path = None;
    let mut count = BENCH_COUNT.to_owned();
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--count" => {
                let v = it.next().ok_or("--count 뒤에 값이 없습니다")?;
                v.parse::<usize>()
                    .map_err(|_| format!("--count의 값이 수가 아닙니다: {v}"))?;
                count = v.clone();
            }
            _ if path.is_none() && !arg.starts_with("--") => path = Some(PathBuf::from(arg)),
            _ => return Err(format!("알 수 없는 인자: {arg}")),
        }
    }
    // 상대 경로는 명령을 부른 폴더 기준이다(단계는 작업 공간 루트에서 돌므로 여기서 푼다).
    let path = match path {
        Some(p) if p.is_relative() => env::current_dir().map_err(|e| e.to_string())?.join(p),
        Some(p) => p,
        None => default_folder(&count),
    };
    let mut args = facet_bench("make-folder");
    args.push(path.display().to_string());
    args.extend(["--count".to_owned(), count]);
    Ok(Step {
        name: "시험 폴더 생성",
        args,
    })
}

/// `dist` 뒤의 인자에서 label 하나를 꺼내 검사한다.
fn dist_label(rest: &[String]) -> Result<&str, String> {
    match rest {
        [label] => dist::validate_label(label).map(|()| label.as_str()),
        [] => Err("label이 없습니다".to_owned()),
        [_, extra, ..] => Err(format!("알 수 없는 인자: {extra}")),
    }
}

/// `target/dist/`를 비우고, facet을 release로 빌드하고, zip과 체크섬을 만든다.
///
/// 폴더를 먼저 비우므로 빌드가 실패해도 앞선 산출물이나 반쪽 파일이 남지 않는다.
fn run_dist(label: &str) -> ExitCode {
    let root = workspace_root();
    let dist_dir = root.join("target").join("dist");
    if let Err(e) = dist::clean_dir(&dist_dir) {
        eprintln!("xtask dist: {}를 비우지 못함: {e}", dist_dir.display());
        return ExitCode::FAILURE;
    }

    let build = step(
        "release 빌드",
        &["build", "--release", "--locked", "-p", "facet-app"],
    );
    if run_steps("dist", &[build]) != ExitCode::SUCCESS {
        return ExitCode::FAILURE;
    }

    let target_dir = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .map(|p| if p.is_relative() { root.join(p) } else { p })
        .unwrap_or_else(|| root.join("target"));
    let entries = [
        dist::Entry {
            name: "facet.exe",
            source: target_dir
                .join("release")
                .join(format!("facet{}", env::consts::EXE_SUFFIX)),
        },
        dist::Entry {
            name: "LICENSE",
            source: root.join("LICENSE"),
        },
        dist::Entry {
            name: "NOTICE.txt",
            source: root.join("NOTICE.txt"),
        },
    ];
    match dist::package(&dist_dir, label, &entries) {
        Ok(zip) => {
            println!("xtask dist: {} 와 .sha256을 만듦", zip.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("xtask dist: 묶기 실패: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_steps(task: &str, steps: &[Step]) -> ExitCode {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let root = workspace_root();

    for step in steps {
        println!(
            "\n=== xtask {task}: {} (cargo {}) ===",
            step.name,
            step.args.join(" ")
        );
        let status = Command::new(&cargo)
            .args(&step.args)
            .current_dir(&root)
            .status();
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                eprintln!("\nxtask {task}: {} 단계 실패 ({s})", step.name);
                return ExitCode::FAILURE;
            }
            Err(e) => {
                eprintln!("\nxtask {task}: {} 단계를 실행하지 못함: {e}", step.name);
                return ExitCode::FAILURE;
            }
        }
    }

    println!("\nxtask {task}: 모든 단계 통과");
    ExitCode::SUCCESS
}

/// 작업 공간 루트. xtask 크레이트의 부모 폴더다.
fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .map(PathBuf::from)
        .unwrap_or(manifest_dir)
}

/// 앱과 측정 도구를 릴리스 빌드하고 `facet-bench screen`을 돌린다.
///
/// 받은 인자는 그대로 넘기되, 경로·`--count`·`--json`을 비우면 기본값(10만 항목 시험 폴더,
/// `target/measure.json`)을 채운다. 상대 경로는 명령을 부른 폴더 기준이다.
fn run_measure(rest: &[String]) -> ExitCode {
    let root = workspace_root();
    let build = step(
        "release 빌드",
        &[
            "build",
            "--release",
            "--locked",
            "-p",
            "facet-app",
            "-p",
            "facet-bench",
        ],
    );
    if run_steps("measure", &[build]) != ExitCode::SUCCESS {
        return ExitCode::FAILURE;
    }
    let args = match measure_args(rest, &root) {
        Ok(a) => a,
        Err(e) => {
            eprintln!(
                "{e}

{USAGE}"
            );
            return ExitCode::FAILURE;
        }
    };
    let target_dir = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .map(|p| if p.is_relative() { root.join(p) } else { p })
        .unwrap_or_else(|| root.join("target"));
    let release = target_dir.join("release");
    let status = Command::new(release.join("facet-bench.exe"))
        .arg("screen")
        .args(&args)
        .arg("--app")
        .arg(release.join("facet.exe"))
        .status();
    match status {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("xtask measure: facet-bench를 실행하지 못함: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `measure`의 인자에 빠진 기본값(경로, `--count`, `--json`)을 채운다.
fn measure_args(rest: &[String], root: &std::path::Path) -> Result<Vec<String>, String> {
    let cwd = env::current_dir().map_err(|e| e.to_string())?;
    let absolute = |p: &str| {
        let p = PathBuf::from(p);
        if p.is_relative() { cwd.join(p) } else { p }
    };
    let mut path = None;
    let mut count = BENCH_COUNT.to_owned();
    let mut json = None;
    let mut passthrough = Vec::new();
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--count" | "--json" | "--runs" | "--scroll-runs" | "--scroll-rows"
            | "--refresh-hz" => {
                let v = it.next().ok_or(format!("{arg} 뒤에 값이 없습니다"))?;
                match arg.as_str() {
                    "--count" => count = v.clone(),
                    "--json" => json = Some(absolute(v)),
                    _ => passthrough.extend([arg.clone(), v.clone()]),
                }
            }
            _ if path.is_none() && !arg.starts_with("--") => path = Some(absolute(arg)),
            _ => return Err(format!("알 수 없는 인자: {arg}")),
        }
    }
    let path = path.unwrap_or_else(|| default_folder(&count));
    let json = json.unwrap_or_else(|| root.join("target").join("measure.json"));
    let mut args = vec![path.display().to_string()];
    args.extend(["--count".to_owned(), count]);
    args.extend(["--json".to_owned(), json.display().to_string()]);
    args.extend(passthrough);
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::dist_label;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn dist_takes_exactly_one_safe_label() {
        assert_eq!(dist_label(&args(&["nightly"])), Ok("nightly"));
        assert_eq!(dist_label(&args(&["v0.1.0"])), Ok("v0.1.0"));
        assert!(dist_label(&args(&[])).is_err());
        assert!(dist_label(&args(&["a/b"])).is_err());
        assert!(dist_label(&args(&["a b"])).is_err());
        assert!(dist_label(&args(&["nightly", "extra"])).is_err());
    }
}
