// target: crates/facet-core/tests/spec_install_launch.rs
// AC: #480: 탐색기식 인자(/select,·/e,·/root,·/n·내 PC 식별자·값이 다음 인자)를 해석하고 보통 경로는 그대로, 탐색기 넘기기 명령
//! 폴더 열기 가로채기로 facet이 받는 탐색기식 실행 인자(`resolve_launch`)와 시작 실패 때 탐색기에 넘기는 명령
//! (`launch::explorer_fallback`)을 임시 폴더로 확인한다. `/`로 시작하는 인자가 스위치인 것은 Windows의 약속이라(다른
//! 플랫폼에서는 절대 경로) Windows 전용이다.

#![cfg(windows)]

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use facet_core::Location;
use facet_core::launch::{LaunchTarget, explorer_fallback, resolve_launch};
use facet_core::window::home_location;
use tempfile::TempDir;

/// `root/폴더 하나/보고서.txt`.
fn tree() -> TempDir {
    let dir = TempDir::new().unwrap();
    let sub = dir.path().join("폴더 하나");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join("보고서.txt"), b"x").unwrap();
    dir
}

fn sub(dir: &TempDir) -> PathBuf {
    dir.path().join("폴더 하나")
}

fn arg(text: String) -> PathBuf {
    PathBuf::from(text)
}

fn one(dir: &Path, args: &[PathBuf]) -> LaunchTarget {
    let mut targets = resolve_launch(dir, args);
    assert_eq!(targets.len(), 1, "탭 하나");
    let target = targets.remove(0);
    assert!(target.error.is_none(), "해석 오류: {:?}", target.error);
    target
}

#[test]
#[ignore = "#480"]
fn select는_그_폴더를_열고_그_항목을_고른다() {
    let dir = tree();
    let file = sub(&dir).join("보고서.txt");
    let target = one(dir.path(), &[arg(format!("/select,{}", file.display()))]);
    assert_eq!(target.location, Location::file_system(sub(&dir)));
    assert_eq!(target.select, Some(OsString::from("보고서.txt")));

    // 폴더를 고르면 그 상위에서 그 폴더를 고른다.
    let target = one(
        dir.path(),
        &[arg(format!("/select,{}", sub(&dir).display()))],
    );
    assert_eq!(target.location, Location::file_system(dir.path()));
    assert_eq!(target.select, Some(OsString::from("폴더 하나")));
}

#[test]
#[ignore = "#480"]
fn 스위치의_값이_비면_다음_인자가_값이다() {
    let dir = tree();
    let file = sub(&dir).join("보고서.txt");
    let target = one(dir.path(), &[arg("/select,".to_owned()), file]);
    assert_eq!(target.location, Location::file_system(sub(&dir)));
    assert_eq!(target.select, Some(OsString::from("보고서.txt")));
}

#[test]
#[ignore = "#480"]
fn e와_root는_그_폴더를_연다() {
    let dir = tree();
    for switch in ["/e,", "/root,", "/n,/e,", "/E,"] {
        let target = one(
            dir.path(),
            &[arg(format!("{switch}{}", sub(&dir).display()))],
        );
        assert_eq!(
            target.location,
            Location::file_system(sub(&dir)),
            "{switch}"
        );
        assert_eq!(target.select, None, "{switch}");
    }
}

#[test]
#[ignore = "#480"]
fn 경로_없는_스위치만이면_홈이다() {
    let dir = tree();
    for args in [vec!["/n"], vec!["/e"], vec!["/n", "/e"], vec!["/separate"]] {
        let args: Vec<PathBuf> = args.into_iter().map(PathBuf::from).collect();
        let target = one(dir.path(), &args);
        assert_eq!(target.location, home_location(), "{args:?}");
        assert_eq!(target.select, None);
    }
}

#[test]
#[ignore = "#480"]
fn 내_PC_식별자는_내_PC다() {
    let dir = tree();
    for text in [
        "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}",
        "/e,::{20D04FE0-3AEA-1069-A2D8-08002B30309D}",
        "::{20d04fe0-3aea-1069-a2d8-08002b30309d}",
    ] {
        let target = one(dir.path(), &[PathBuf::from(text)]);
        assert_eq!(target.location, Location::this_pc(), "{text}");
    }
}

#[test]
#[ignore = "#480"]
fn 보통_경로_인자는_그대로다() {
    let dir = tree();
    let target = one(dir.path(), &[sub(&dir)]);
    assert_eq!(target.location, Location::file_system(sub(&dir)));
    let target = one(dir.path(), &[sub(&dir).join("보고서.txt")]);
    assert_eq!(target.location, Location::file_system(sub(&dir)));
    assert_eq!(target.select, Some(OsString::from("보고서.txt")));
}

#[test]
#[ignore = "#480"]
fn 탐색기_넘기기는_받은_인자_그대로다() {
    let args = [
        OsString::from("/select,C:\\폴더 하나\\보고서.txt"),
        OsString::from("C:\\Windows"),
    ];
    let command = explorer_fallback(&args);
    let windir = PathBuf::from(std::env::var_os("WINDIR").expect("WINDIR"));
    assert_eq!(
        Path::new(command.get_program())
            .to_string_lossy()
            .to_ascii_lowercase(),
        windir
            .join("explorer.exe")
            .to_string_lossy()
            .to_ascii_lowercase()
    );
    let got: Vec<&std::ffi::OsStr> = command.get_args().collect();
    assert_eq!(got, [args[0].as_os_str(), args[1].as_os_str()]);
}
