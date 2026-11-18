//! 셸 폴더 열거(`shell_folder`)를 실제 셸의 「내 PC」로 확인한다. Windows 전용.
#![cfg(windows)]

use std::path::PathBuf;
use std::time::Duration;

use facet_win::shell_folder::{self, ShellFolderError};
use facet_win::shell_thread::ShellThread;

/// 내 PC(`::{20D04FE0-3AEA-1069-A2D8-08002B30309D}`)의 PIDL: 바탕 화면 아래 CLSID 조각 하나와 끝.
const THIS_PC: [u8; 22] = [
    0x14, 0x00, 0x1F, 0x50, 0xE0, 0x4F, 0xD0, 0x20, 0xEA, 0x3A, 0x69, 0x10, 0xA2, 0xD8, 0x08, 0x00,
    0x2B, 0x30, 0x30, 0x9D, 0x00, 0x00,
];

const WAIT: Duration = Duration::from_secs(30);

#[test]
fn 내_pc를_열거하면_시스템_드라이브가_실제_경로를_가진_폴더다() {
    let shell = ShellThread::new();
    let (name, entries) = shell
        .call(WAIT, || {
            let name = shell_folder::display_name(&THIS_PC);
            let mut entries = Vec::new();
            let result = shell_folder::enumerate(&THIS_PC, 2, &mut |batch| {
                assert!(batch.len() <= 2);
                entries.extend(batch);
                true
            });
            (name, result.map(|()| entries))
        })
        .expect("셸 스레드");
    assert!(!name.expect("표시 이름").trim().is_empty());
    let entries = entries.expect("열거");
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let root = PathBuf::from(format!("{drive}\\"));
    let system = entries
        .iter()
        .find(|entry| entry.path.as_deref() == Some(root.as_path()))
        .unwrap_or_else(|| panic!("시스템 드라이브가 없다: {entries:?}"));
    assert!(system.is_folder);
    assert!(!system.name.is_empty());
    assert!(shell_folder::is_valid_id(&system.id));
    assert!(system.id.starts_with(&THIS_PC[..20]), "{:?}", system.id);
}

#[test]
fn 그만두면_첫_묶음_뒤로_더_오지_않는다() {
    let shell = ShellThread::new();
    let batches = shell
        .call(WAIT, || {
            let mut batches = 0;
            shell_folder::enumerate(&THIS_PC, 1, &mut |_| {
                batches += 1;
                false
            })
            .map(|()| batches)
        })
        .expect("셸 스레드");
    assert_eq!(batches, Ok(1));
}

#[test]
fn 틀린_식별자는_셸을_부르지_않고_거절한다() {
    assert_eq!(
        shell_folder::enumerate(&[1, 2, 3], 1, &mut |_| true),
        Err(ShellFolderError::InvalidId)
    );
    assert_eq!(
        shell_folder::display_name(&[]),
        Err(ShellFolderError::InvalidId)
    );
}
