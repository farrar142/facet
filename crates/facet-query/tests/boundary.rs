//! 시험용 질의 명령은 Windows API를 직접 부르지 않는다(#48). 질의 프로토콜 크레이트만 쓰고
//! `unsafe`가 없어야 한다. 이 경계가 무너지면 여기서 실패한다.

use std::fs;
use std::path::Path;

fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// `Cargo.toml`에서 `[header]` 절의 키들(의존성 이름).
fn section_keys(toml: &str, header: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut inside = false;
    for line in toml.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == header;
            continue;
        }
        if inside
            && !line.is_empty()
            && !line.starts_with('#')
            && let Some((key, _)) = line.split_once('=')
        {
            keys.push(key.trim().to_owned());
        }
    }
    keys
}

#[test]
fn only_the_protocol_crate_is_a_dependency() {
    let toml = fs::read_to_string(manifest_dir().join("Cargo.toml")).unwrap();
    assert_eq!(
        section_keys(&toml, "[dependencies]"),
        ["facet-protocol.workspace"]
    );
    // 플랫폼별 의존성(예: windows-sys)도 두지 않는다.
    assert!(!toml.contains("[target."), "플랫폼별 의존성이 있다");
    // 작업 공간 lint(unsafe_code = forbid)를 따른다.
    assert!(
        toml.contains("[lints]\nworkspace = true") || toml.contains("[lints]\r\nworkspace = true")
    );
}

#[test]
fn sources_have_no_unsafe_or_foreign_calls() {
    for entry in fs::read_dir(manifest_dir().join("src")).unwrap() {
        let path = entry.unwrap().path();
        let text = fs::read_to_string(&path).unwrap();
        for word in [
            "unsafe",
            "extern \"",
            "windows_sys",
            "windows::Win32",
            "#[link",
        ] {
            assert!(!text.contains(word), "{}: {word}", path.display());
        }
    }
}
