//! 휴지통으로 보내기·복원(`facet_win::recycle`)을 임시 폴더의 실제 파일과 실제 휴지통으로 확인한다(Windows 전용).
//!
//! 러너 계정이 휴지통을 쓸 수 없으면(임시 폴더의 볼륨에 휴지통이 없거나 셸이 휴지통에 넣지 않음) 건너뛰었다고
//! 출력한다. 테스트가 휴지통에 넣은 항목은 테스트가 끝날 때(실패해도) 휴지통에서 지운다.
#![cfg(windows)]

use std::fs::{self, File, FileTimes};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use facet_win::file_ops::FileOpErrorKind;
use facet_win::recycle::{self, RecycleOutcome, RecycledItem};
use tempfile::TempDir;

/// 테스트가 휴지통에 넣은 항목. 버릴 때 휴지통에 남아 있으면 내용(`$R...`)과 정보 파일(`$I...`)을 지운다.
struct Bin(Vec<RecycledItem>);

impl Bin {
    fn new() -> Self {
        Bin(Vec::new())
    }

    fn keep(&mut self, item: &RecycledItem) {
        self.0.push(item.clone());
    }
}

impl Drop for Bin {
    fn drop(&mut self) {
        for item in &self.0 {
            purge(item);
        }
    }
}

/// 휴지통 항목을 휴지통에서 영구히 지운다(이미 없으면 아무것도 안 함).
fn purge(item: &RecycledItem) {
    let recycled = item.recycled();
    if recycled.is_dir() {
        let _ = fs::remove_dir_all(recycled);
    } else {
        let _ = fs::remove_file(recycled);
    }
    let _ = fs::remove_file(info_file(recycled));
}

/// 휴지통 항목의 정보 파일(`$R...` 옆의 `$I...`).
fn info_file(recycled: &Path) -> PathBuf {
    let name = recycled.file_name().unwrap().to_string_lossy();
    recycled.with_file_name(name.replacen("$R", "$I", 1))
}

/// 휴지통을 쓸 수 있는 환경인지 보고, 아니면 건너뛴다고 출력한다.
fn recycle_bin_usable(dir: &Path, test: &str) -> bool {
    if recycle::has_recycle_bin(dir) {
        return true;
    }
    println!("건너뜀({test}): 임시 폴더의 볼륨에 휴지통이 없습니다");
    false
}

/// 하나를 휴지통에 보낸다. 셸이 휴지통에 넣지 않으면(러너 계정이 휴지통을 쓸 수 없음) 건너뛴다고 출력하고 `None`.
fn recycle_one(path: &Path, bin: &mut Bin, test: &str) -> Option<RecycledItem> {
    let outcomes = recycle::recycle(&[path.to_path_buf()]);
    match &outcomes[..] {
        [RecycleOutcome::Recycled(item)] => {
            bin.keep(item);
            Some(item.clone())
        }
        [RecycleOutcome::NotRecyclable] => {
            println!("건너뜀({test}): 이 계정은 휴지통을 쓸 수 없습니다(셸이 영구 삭제하려 함)");
            None
        }
        other => panic!("휴지통으로 보내지 못했습니다: {other:?}"),
    }
}

/// 수정 시각을 정한 파일을 쓴다(되돌린 뒤 같은지 보려고 지금과 다른 시각).
fn write_with_time(path: &Path, contents: &[u8]) -> SystemTime {
    fs::write(path, contents).unwrap();
    let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified))
        .unwrap();
    modified
}

fn modified(path: &Path) -> SystemTime {
    fs::metadata(path).unwrap().modified().unwrap()
}

#[test]
fn 임시_폴더의_고정_디스크에는_휴지통이_있다() {
    let dir = TempDir::new().unwrap();
    assert!(recycle::has_recycle_bin(dir.path()));
    // 아직 없는 경로도 볼륨으로 판단한다.
    assert!(recycle::has_recycle_bin(
        &dir.path().join("없는 폴더").join("a.txt")
    ));
}

#[test]
fn 파일과_폴더를_휴지통에_보내고_원래_자리로_복원한다() {
    let dir = TempDir::new().unwrap();
    if !recycle_bin_usable(dir.path(), "보내고 복원") {
        return;
    }
    let mut bin = Bin::new();
    let file = dir.path().join("보고서 🎉.txt");
    let file_time = write_with_time(&file, b"hello");
    let folder = dir.path().join("사진 폴더");
    fs::create_dir(&folder).unwrap();
    let inner_time = write_with_time(&folder.join("안.bin"), &[7u8; 4096]);
    let missing = dir.path().join("없음.txt");

    let outcomes = recycle::recycle(&[file.clone(), missing, folder.clone()]);

    if outcomes[0] == RecycleOutcome::NotRecyclable {
        println!("건너뜀(보내고 복원): 이 계정은 휴지통을 쓸 수 없습니다(셸이 영구 삭제하려 함)");
        return;
    }
    let items: Vec<RecycledItem> = outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            RecycleOutcome::Recycled(item) => Some(item.clone()),
            _ => None,
        })
        .collect();
    for item in &items {
        bin.keep(item);
    }
    assert!(
        matches!(&outcomes[1], RecycleOutcome::Failed(error) if error.kind() == FileOpErrorKind::SourceMissing),
        "{outcomes:?}"
    );
    assert_eq!(items.len(), 2, "{outcomes:?}");
    assert_eq!(items[0].original(), file);
    assert_eq!(items[1].original(), folder);
    assert!(
        !file.exists() && !folder.exists(),
        "원래 자리에서 사라져야 한다"
    );
    for item in &items {
        assert!(
            item.recycled().exists(),
            "휴지통 항목이 있어야 한다: {item:?}"
        );
    }

    for item in &items {
        let restored = recycle::restore(item).unwrap();
        assert_eq!(restored, item.original());
        assert!(!item.recycled().exists());
        assert!(
            !info_file(item.recycled()).exists(),
            "휴지통에 빈 정보 파일이 남으면 안 된다"
        );
    }
    assert_eq!(fs::read(&file).unwrap(), b"hello");
    assert_eq!(modified(&file), file_time);
    assert_eq!(fs::read(folder.join("안.bin")).unwrap(), vec![7u8; 4096]);
    assert_eq!(modified(&folder.join("안.bin")), inner_time);
}

#[test]
fn 원래_폴더가_없어졌으면_다시_만들어_복원한다() {
    let dir = TempDir::new().unwrap();
    if !recycle_bin_usable(dir.path(), "원래 폴더 다시 만들기") {
        return;
    }
    let mut bin = Bin::new();
    let parent = dir.path().join("상위");
    fs::create_dir(&parent).unwrap();
    let file = parent.join("a.txt");
    fs::write(&file, b"a").unwrap();
    let Some(item) = recycle_one(&file, &mut bin, "원래 폴더 다시 만들기") else {
        return;
    };
    fs::remove_dir(&parent).unwrap();

    recycle::restore(&item).unwrap();

    assert_eq!(fs::read(&file).unwrap(), b"a");
}

#[test]
fn 복원할_자리에_같은_이름이_있으면_덮어쓰지_않고_실패한다() {
    let dir = TempDir::new().unwrap();
    if !recycle_bin_usable(dir.path(), "같은 이름") {
        return;
    }
    let mut bin = Bin::new();
    let file = dir.path().join("같은 이름.txt");
    fs::write(&file, b"old").unwrap();
    let Some(item) = recycle_one(&file, &mut bin, "같은 이름") else {
        return;
    };
    fs::write(&file, b"new").unwrap();

    let error = recycle::restore(&item).unwrap_err();

    assert_eq!(error.kind(), FileOpErrorKind::TargetExists);
    assert_eq!(fs::read(&file).unwrap(), b"new");
    assert!(item.recycled().exists(), "휴지통 항목은 그대로 남는다");
}

#[test]
fn 휴지통_항목이_이미_지워졌으면_원본_없음으로_실패한다() {
    let dir = TempDir::new().unwrap();
    if !recycle_bin_usable(dir.path(), "비운 휴지통") {
        return;
    }
    let mut bin = Bin::new();
    let file = dir.path().join("비울 것.txt");
    fs::write(&file, b"x").unwrap();
    let Some(item) = recycle_one(&file, &mut bin, "비운 휴지통") else {
        return;
    };
    purge(&item);

    let error = recycle::restore(&item).unwrap_err();

    assert_eq!(error.kind(), FileOpErrorKind::SourceMissing);
    assert!(!file.exists());
}

#[test]
fn 셸이_영구_삭제하려는_항목은_지우지_않고_멈춘다() {
    let dir = TempDir::new().unwrap();
    let refused = dir.path().join("휴지통에 안 들어감");
    fs::create_dir(&refused).unwrap();
    let big = refused.join("큰 파일.bin");
    fs::write(&big, b"big").unwrap();
    let after = dir.path().join("뒤 항목.txt");
    fs::write(&after, b"after").unwrap();
    let _refuse = recycle::fault::refuse_recycle_under(&refused);
    let mut bin = Bin::new();

    let outcomes = recycle::recycle(&[big.clone(), after.clone()]);

    for outcome in &outcomes {
        if let RecycleOutcome::Recycled(item) = outcome {
            bin.keep(item);
        }
    }
    assert_eq!(
        outcomes,
        vec![RecycleOutcome::NotRecyclable, RecycleOutcome::NotAttempted]
    );
    assert_eq!(fs::read(&big).unwrap(), b"big");
    assert_eq!(fs::read(&after).unwrap(), b"after");
}

#[test]
fn 휴지통이_없는_네트워크_위치의_항목은_지우지_않고_멈춘다() {
    // 관리 공유(\\localhost\C$)로 본 임시 폴더는 네트워크 위치라 휴지통이 없다. 셸은 이것을 영구 삭제로 바꾼다.
    let dir = TempDir::new().unwrap();
    let local = dir.path().join("공유로 본 파일.txt");
    fs::write(&local, b"keep").unwrap();
    let absolute = std::path::absolute(&local).unwrap();
    let text = absolute.to_string_lossy();
    let Some((drive, rest)) = text.split_once(":\\") else {
        println!("건너뜀(네트워크 위치): 임시 폴더가 드라이브 문자 경로가 아닙니다");
        return;
    };
    let shared = PathBuf::from(format!(r"\\localhost\{drive}$\{rest}"));
    if !shared.exists() {
        println!(
            "건너뜀(네트워크 위치): 관리 공유 {} 에 닿을 수 없습니다",
            shared.display()
        );
        return;
    }
    assert!(!recycle::has_recycle_bin(&shared));
    let mut bin = Bin::new();

    let outcomes = recycle::recycle(&[shared]);

    for outcome in &outcomes {
        if let RecycleOutcome::Recycled(item) = outcome {
            bin.keep(item);
        }
    }
    assert_eq!(outcomes, vec![RecycleOutcome::NotRecyclable]);
    assert_eq!(fs::read(&local).unwrap(), b"keep");
}

#[test]
fn 셸이_멈추면_시간_초과로_돌려주고_항목을_지우지_않는다() {
    let dir = TempDir::new().unwrap();
    let stalled = dir.path().join("멈춤");
    fs::create_dir(&stalled).unwrap();
    let file = stalled.join("a.txt");
    fs::write(&file, b"a").unwrap();
    let stall = Duration::from_secs(2);
    let _stall = recycle::fault::stall_shell_under(&stalled, stall);
    let mut bin = Bin::new();

    let started = Instant::now();
    let outcomes =
        recycle::recycle_with_timeout(std::slice::from_ref(&file), Duration::from_millis(300));
    let waited = started.elapsed();

    for outcome in &outcomes {
        if let RecycleOutcome::Recycled(item) = outcome {
            bin.keep(item);
        }
    }
    assert!(
        matches!(&outcomes[..], [RecycleOutcome::Failed(error)] if error.kind() == FileOpErrorKind::TimedOut),
        "{outcomes:?}"
    );
    assert!(waited < stall, "멈춘 셸을 기다리지 않아야 한다: {waited:?}");
    // 멈춘 셸 스레드는 깨어나 그만두라는 신호를 보고 지우지 않는다.
    std::thread::sleep(stall + Duration::from_secs(1));
    assert_eq!(fs::read(&file).unwrap(), b"a");
}

#[test]
fn 경로가_260자를_넘어도_보내고_복원한다() {
    let dir = TempDir::new().unwrap();
    if !recycle_bin_usable(dir.path(), "긴 경로") {
        return;
    }
    let mut folder = dir.path().to_path_buf();
    for i in 0..5 {
        folder.push(format!("{i}-{}", "가".repeat(60)));
    }
    fs::create_dir_all(&folder).unwrap();
    let file = folder.join("긴 경로 파일.txt");
    fs::write(&file, b"long").unwrap();
    assert!(file.as_os_str().encode_wide_len() > 260);
    let mut bin = Bin::new();

    let Some(item) = recycle_one(&file, &mut bin, "긴 경로") else {
        return;
    };
    assert!(!file.exists());

    assert_eq!(recycle::restore(&item).unwrap(), file);
    assert_eq!(fs::read(&file).unwrap(), b"long");
}

trait WideLen {
    fn encode_wide_len(&self) -> usize;
}

impl WideLen for std::ffi::OsStr {
    fn encode_wide_len(&self) -> usize {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().count()
    }
}
