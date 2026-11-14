//! 저장소 작업 명령. `cargo xtask <명령>`으로 부른다.
//!
//! - `ci`: 형식 검사 → lint(경고를 오류로) → 빌드 → 테스트. CI와 로컬이 같은 명령을 쓴다.

use std::env;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

const USAGE: &str = "사용법: cargo xtask <명령>\n\n명령:\n  ci    형식 검사, lint(경고를 오류로), 빌드, 테스트를 차례로 돌린다";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("ci") => ci(),
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
    args: &'static [&'static str],
}

const CI_STEPS: &[Step] = &[
    Step {
        name: "형식 검사",
        args: &["fmt", "--all", "--check"],
    },
    Step {
        name: "lint",
        args: &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--locked",
            "--",
            "-D",
            "warnings",
        ],
    },
    Step {
        name: "빌드",
        args: &["build", "--workspace", "--all-targets", "--locked"],
    },
    Step {
        name: "테스트",
        args: &["test", "--workspace", "--locked"],
    },
];

fn ci() -> ExitCode {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let root = workspace_root();

    for step in CI_STEPS {
        println!(
            "\n=== xtask ci: {} (cargo {}) ===",
            step.name,
            step.args.join(" ")
        );
        let status = Command::new(&cargo)
            .args(step.args)
            .current_dir(&root)
            .status();
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                eprintln!("\nxtask ci: {} 단계 실패 ({s})", step.name);
                return ExitCode::FAILURE;
            }
            Err(e) => {
                eprintln!("\nxtask ci: {} 단계를 실행하지 못함: {e}", step.name);
                return ExitCode::FAILURE;
            }
        }
    }

    println!("\nxtask ci: 모든 단계 통과");
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
