//! 드라이브 용량과 장치 알림(Windows 전용).
#![cfg(windows)]

use std::path::PathBuf;

use facet_win::devices::{disk_space, watch_devices};

fn system_root() -> PathBuf {
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_owned());
    PathBuf::from(format!("{drive}\\"))
}

#[test]
fn 시스템_드라이브의_용량을_얻는다() {
    let space = disk_space(&system_root()).expect("시스템 드라이브 용량");
    assert!(space.total > 0);
    assert!(space.free <= space.total);
}

#[test]
fn 없는_드라이브는_용량이_없다() {
    let used = facet_win::drive_letters();
    let Some(letter) = ('A'..='Z').rev().find(|c| !used.contains(c)) else {
        return;
    };
    assert_eq!(disk_space(&PathBuf::from(format!("{letter}:\\"))), None);
}

#[test]
fn 장치_알림을_걸고_풀_수_있다() {
    let watch = watch_devices(|| {}).expect("장치 알림");
    drop(watch);
    // 다시 걸어도 된다.
    drop(watch_devices(|| {}).expect("장치 알림"));
}
