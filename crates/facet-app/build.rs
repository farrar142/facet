//! `ui/app.slint`를 Rust 코드로 옮기고, 정보 화면에 넣을 빌드 정보(커밋·날짜·크레이트 라이선스 목록)를 만든다.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    slint_build::compile("ui/app.slint").expect("ui/app.slint를 컴파일하지 못했다");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    std::fs::write(out.join("build_commit.txt"), commit()).unwrap();
    std::fs::write(out.join("build_date.txt"), build_date()).unwrap();
    std::fs::write(out.join("crate_licenses.txt"), crate_licenses()).unwrap();
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
}

fn commit() -> String {
    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// UTC 날짜(`YYYY-MM-DD`). 시계 크레이트 없이 일 수에서 센다.
fn build_date() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// 의존성 그래프 전체의 「이름 버전 라이선스」 줄(`cargo metadata`, 의존성이 바뀌면 함께 바뀜).
fn crate_licenses() -> String {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--filter-platform",
            "x86_64-pc-windows-msvc",
        ])
        .current_dir(&dir)
        .output();
    let Ok(output) = output else {
        return String::new();
    };
    if !output.status.success() {
        return String::new();
    }
    let Ok(json) = String::from_utf8(output.stdout) else {
        return String::new();
    };
    let mut lines = Vec::new();
    let packages = json.split("\"packages\":[").nth(1).unwrap_or("");
    // 간단한 필드 추출: 패키지마다 name·version·license·source를 찾는다.
    for chunk in packages.split("{\"name\":\"").skip(1) {
        let name = chunk.split('"').next().unwrap_or("");
        let version = field(chunk, "\"version\":\"");
        let license = field(chunk, "\"license\":\"");
        let source = chunk.contains("\"source\":\"") && !version.is_empty() && license != "?";
        if !source {
            continue; // 작업 공간 크레이트는 facet 자신
        }
        let license = if license.is_empty() { "?" } else { license };
        lines.push(format!("{name} {version} {license}"));
    }
    lines.sort();
    lines.dedup();
    lines.join("\n")
}

fn field<'a>(chunk: &'a str, key: &str) -> &'a str {
    chunk
        .split(key)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or("")
}
