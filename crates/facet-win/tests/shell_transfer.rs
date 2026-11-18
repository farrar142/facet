//! 셸 항목과 PC 사이 파일 전송(`shell_transfer`)을 임시 폴더의 셸 항목으로 확인한다(휴대폰과 같은 셸 호출을 쓰되
//! 러너에 휴대폰이 없어 파일 시스템 셸 항목으로 본다). Windows 전용.
#![cfg(windows)]

use std::fs;
use std::time::Duration;

use facet_win::file_ops::FileOpErrorKind;
use facet_win::shell_folder;
use facet_win::shell_thread::ShellThread;
use facet_win::shell_transfer;

const WAIT: Duration = Duration::from_secs(60);

fn id_of(shell: &ShellThread, path: &std::path::Path) -> Vec<u8> {
    let text = path.display().to_string();
    shell
        .call(WAIT, move || shell_folder::id_from_parsing_name(&text))
        .expect("셸 스레드")
        .expect("셸 항목 식별자")
}

#[test]
fn 넣고_열거하고_가져오고_지운다() {
    let dir = tempfile::tempdir().unwrap();
    let pc = dir.path().join("pc");
    let device = dir.path().join("장치");
    fs::create_dir_all(&pc).unwrap();
    fs::create_dir_all(&device).unwrap();
    let source = pc.join("사진 1.jpg");
    let body: Vec<u8> = (0..3_000_000u32).map(|n| (n % 251) as u8).collect();
    fs::write(&source, &body).unwrap();

    let shell = ShellThread::new();
    let folder = id_of(&shell, &device);

    // 넣기(원래 이름과 새 이름), 폴더 만들기.
    let (put, renamed, made) = {
        let folder = folder.clone();
        let source = source.clone();
        shell
            .call(WAIT, move || {
                (
                    shell_transfer::copy_into(&source, &folder, None),
                    shell_transfer::copy_into(&source, &folder, Some("사진 1 (2).jpg")),
                    shell_transfer::new_folder(&folder, "새 앨범"),
                )
            })
            .unwrap()
    };
    put.expect("넣기");
    renamed.expect("새 이름으로 넣기");
    made.expect("폴더 만들기");
    assert_eq!(fs::read(device.join("사진 1.jpg")).unwrap(), body);
    assert!(device.join("사진 1 (2).jpg").is_file());
    assert!(device.join("새 앨범").is_dir());

    // 열거: 확장자를 포함한 파일 이름·폴더 여부·크기.
    let children = {
        let folder = folder.clone();
        shell
            .call(WAIT, move || shell_transfer::children(&folder))
            .unwrap()
            .expect("열거")
    };
    let photo = children
        .iter()
        .find(|child| child.name == "사진 1.jpg")
        .unwrap_or_else(|| panic!("넣은 파일이 없다: {children:?}"));
    assert!(!photo.is_folder);
    assert_eq!(photo.size, Some(body.len() as u64));
    assert!(photo.modified.is_some());
    assert!(
        children
            .iter()
            .any(|child| child.name == "새 앨범" && child.is_folder)
    );

    // 가져오기: 덩어리마다 진행을 알리고 내용이 같다.
    let target = pc.join("가져온 사진.jpg");
    let (read, steps) = {
        let id = photo.id.clone();
        let target = target.clone();
        shell
            .call(WAIT, move || {
                let mut steps = Vec::new();
                let read = shell_transfer::read_to_file(&id, &target, &mut |done| {
                    steps.push(done);
                    true
                });
                (read, steps)
            })
            .unwrap()
    };
    assert_eq!(read.expect("가져오기"), body.len() as u64);
    assert_eq!(fs::read(&target).unwrap(), body);
    assert!(steps.len() >= 2, "덩어리마다 알린다: {steps:?}");

    // 이미 있는 이름에는 쓰지 않는다.
    let again = {
        let id = photo.id.clone();
        let target = target.clone();
        shell
            .call(WAIT, move || {
                shell_transfer::read_to_file(&id, &target, &mut |_| true)
            })
            .unwrap()
    };
    assert_eq!(again.unwrap_err().kind(), FileOpErrorKind::TargetExists);

    // 그만두면 만들다 만 파일이 남지 않는다.
    let stopped_at = pc.join("그만둔 사진.jpg");
    let stopped = {
        let id = photo.id.clone();
        let target = stopped_at.clone();
        shell
            .call(WAIT, move || {
                shell_transfer::read_to_file(&id, &target, &mut |_| false)
            })
            .unwrap()
    };
    assert_eq!(stopped.unwrap_err().kind(), FileOpErrorKind::Cancelled);
    assert!(!stopped_at.exists());

    // 영구 삭제.
    let deleted = {
        let id = photo.id.clone();
        shell
            .call(WAIT, move || shell_transfer::delete(&id))
            .unwrap()
    };
    deleted.expect("지우기");
    assert!(!device.join("사진 1.jpg").exists());
}
