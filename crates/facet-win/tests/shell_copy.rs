//! 측정 전용 셸 복사(`IFileOperation`)를 임시 폴더의 실제 파일로 확인한다(Windows 전용).
#![cfg(windows)]

use std::fs;

use facet_win::shell_copy::{self, ShellCopyError};
use tempfile::TempDir;

#[test]
fn 셸_복사는_파일과_폴더를_대상_폴더_안으로_대화_없이_복사한다() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("보고서 🎉.txt");
    fs::write(&file, b"hello").unwrap();
    let folder = dir.path().join("작은 파일");
    fs::create_dir(&folder).unwrap();
    for i in 0..3 {
        fs::write(folder.join(format!("{i}.bin")), vec![i as u8; 4096]).unwrap();
    }
    let target = dir.path().join("대상");
    fs::create_dir(&target).unwrap();

    shell_copy::copy_items(&[file, folder], &target).unwrap();

    assert_eq!(fs::read(target.join("보고서 🎉.txt")).unwrap(), b"hello");
    for i in 0..3 {
        assert_eq!(
            fs::read(target.join("작은 파일").join(format!("{i}.bin"))).unwrap(),
            vec![i as u8; 4096]
        );
    }
}

#[test]
fn 없는_원본은_실패한다() {
    let dir = TempDir::new().unwrap();
    let result = shell_copy::copy_items(&[dir.path().join("없음.txt")], dir.path());
    assert!(
        matches!(result, Err(ShellCopyError::Failed { .. })),
        "{result:?}"
    );
}
