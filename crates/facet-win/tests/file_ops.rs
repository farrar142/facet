//! 파일 연산을 임시 폴더의 실제 파일로 확인한다(Windows 전용).
//!
//! 다른 볼륨 이동은 두 번째 볼륨의 쓸 수 있는 폴더를 환경 변수 `FACET_TEST_OTHER_VOLUME_DIR`로 받는다. 없으면
//! 건너뛰었다고 출력한다.
#![cfg(windows)]

use std::fs::{self, File, FileTimes, OpenOptions};
use std::io::Write;
use std::os::windows::fs::{FileTimesExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime};

use facet_win::file_ops::{
    self, CopyProgress, CopyStep, FileOpErrorKind, UNBUFFERED_COPY_THRESHOLD,
};
use tempfile::TempDir;

const READONLY: u32 = 0x1;
const HIDDEN: u32 = 0x2;

/// 두 번째 볼륨의 쓸 수 있는 폴더를 알리는 환경 변수.
const OTHER_VOLUME_ENV: &str = "FACET_TEST_OTHER_VOLUME_DIR";

fn continue_always(_: CopyProgress) -> CopyStep {
    CopyStep::Continue
}

/// 바이트가 위치마다 다른 시험 내용.
fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 + i / 4096) as u8).collect()
}

fn attrib(path: &Path, flags: &[&str]) {
    let status = Command::new("attrib")
        .args(flags)
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success(), "attrib {flags:?} {}", path.display());
}

fn attributes(path: &Path) -> u32 {
    fs::metadata(path).unwrap().file_attributes()
}

/// 시각을 과거의 정해진 값으로 맞춘다(복사가 지금 시각을 주면 드러나게).
fn set_old_times(path: &Path, folder: bool) -> (SystemTime, SystemTime) {
    let created = SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000);
    let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
    let mut options = OpenOptions::new();
    options.access_mode(0x100); // FILE_WRITE_ATTRIBUTES
    if folder {
        options.custom_flags(0x0200_0000); // FILE_FLAG_BACKUP_SEMANTICS
    }
    let file = options.open(path).unwrap();
    file.set_times(FileTimes::new().set_created(created).set_modified(modified))
        .unwrap();
    (created, modified)
}

fn times(path: &Path) -> (SystemTime, SystemTime) {
    let metadata = fs::metadata(path).unwrap();
    (metadata.created().unwrap(), metadata.modified().unwrap())
}

fn ads(path: &Path, stream: &str) -> PathBuf {
    let mut text = path.as_os_str().to_owned();
    text.push(":");
    text.push(stream);
    PathBuf::from(text)
}

#[test]
fn 복사한_파일의_내용_시각_속성_대체_데이터_스트림이_원본과_같다() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("원본 파일.txt");
    let target = dir.path().join("복사본.txt");
    let content = pattern(3 * 1024 * 1024 + 17);
    fs::write(&source, &content).unwrap();
    let zone = b"[ZoneTransfer]\r\nZoneId=3\r\n";
    fs::write(ads(&source, "Zone.Identifier"), zone).unwrap();
    fs::write(ads(&source, "메모"), "다른 스트림").unwrap();
    let expected_times = set_old_times(&source, false);
    attrib(&source, &["+h", "+r"]);
    let source_attributes = attributes(&source);
    assert_eq!(source_attributes & (READONLY | HIDDEN), READONLY | HIDDEN);

    file_ops::copy_file(&source, &target, &mut continue_always).unwrap();

    assert_eq!(fs::read(&target).unwrap(), content);
    assert_eq!(times(&target), expected_times);
    assert_eq!(attributes(&target), source_attributes);
    assert_eq!(fs::read(ads(&target, "Zone.Identifier")).unwrap(), zone);
    assert_eq!(
        fs::read_to_string(ads(&target, "메모")).unwrap(),
        "다른 스트림"
    );
    // 원본은 그대로다.
    assert_eq!(fs::read(&source).unwrap(), content);

    attrib(&source, &["-r", "-h"]);
    attrib(&target, &["-r", "-h"]);
}

#[test]
fn 빈_파일도_복사한다() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("빈.txt");
    File::create(&source).unwrap();
    let target = dir.path().join("빈 복사본.txt");
    file_ops::copy_file(&source, &target, &mut continue_always).unwrap();
    assert_eq!(fs::metadata(&target).unwrap().len(), 0);
}

#[test]
fn 대상이_있으면_대상_있음이고_대상은_그대로다() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("a.txt");
    let target = dir.path().join("b.txt");
    fs::write(&source, "새 내용").unwrap();
    fs::write(&target, "원래 내용").unwrap();
    let error = file_ops::copy_file(&source, &target, &mut continue_always).unwrap_err();
    assert_eq!(error.kind(), FileOpErrorKind::TargetExists, "{error:?}");
    assert_eq!(fs::read_to_string(&target).unwrap(), "원래 내용");
}

#[test]
fn 진행_콜백은_바이트를_늘려_가며_오고_취소하면_대상이_남지_않는다() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("큰 파일.bin");
    let len = 32 * 1024 * 1024;
    fs::write(&source, pattern(len)).unwrap();

    let target = dir.path().join("끝까지.bin");
    let mut seen: Vec<CopyProgress> = Vec::new();
    file_ops::copy_file(&source, &target, &mut |progress| {
        seen.push(progress);
        CopyStep::Continue
    })
    .unwrap();
    assert!(seen.len() >= 2, "덩어리마다 와야 한다: {seen:?}");
    assert!(
        seen.windows(2).all(|w| w[0].copied < w[1].copied),
        "{seen:?}"
    );
    assert!(seen.iter().all(|p| p.total == len as u64), "{seen:?}");
    assert_eq!(seen.last().unwrap().copied, len as u64);

    let target = dir.path().join("취소.bin");
    let mut calls = 0;
    let error = file_ops::copy_file(&source, &target, &mut |_| {
        calls += 1;
        CopyStep::Cancel
    })
    .unwrap_err();
    assert_eq!(error.kind(), FileOpErrorKind::Cancelled, "{error:?}");
    assert_eq!(calls, 1);
    assert!(!target.exists(), "취소한 대상이 남았다");
}

#[test]
fn 콜백에서_멈추면_대상이_늘지_않고_풀면_끝까지_복사된다() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("멈출 파일.bin");
    let content = pattern(32 * 1024 * 1024);
    fs::write(&source, &content).unwrap();
    let target = dir.path().join("멈춘 복사본.bin");

    let (paused_sender, paused) = mpsc::channel::<u64>();
    let (resume, resume_receiver) = mpsc::channel::<()>();
    let copy = {
        let (source, target) = (source.clone(), target.clone());
        thread::spawn(move || {
            let mut first = true;
            file_ops::copy_file(&source, &target, &mut |progress| {
                if first {
                    first = false;
                    paused_sender.send(progress.copied).unwrap();
                    // 멈춤: 풀 때까지 콜백 안에서 기다린다.
                    resume_receiver.recv().unwrap();
                }
                CopyStep::Continue
            })
        })
    };
    let copied = paused.recv_timeout(Duration::from_secs(30)).unwrap();
    assert!(copied < content.len() as u64);
    let size = || fs::metadata(&target).unwrap().len();
    let before = size();
    thread::sleep(Duration::from_millis(500));
    assert_eq!(size(), before, "멈춘 동안 대상이 늘었다");
    assert!(!copy.is_finished());
    resume.send(()).unwrap();
    copy.join().unwrap().unwrap();
    assert_eq!(fs::read(&target).unwrap(), content);
}

#[test]
fn 콜백이_패닉하면_대상을_지우고_패닉을_잇는다() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("a.bin");
    fs::write(&source, pattern(4 * 1024 * 1024)).unwrap();
    let target = dir.path().join("b.bin");
    let result = std::panic::catch_unwind(|| {
        let _ = file_ops::copy_file(&source, &target, &mut |_| panic!("콜백 패닉"));
    });
    assert!(result.is_err());
    assert!(!target.exists());
}

#[test]
fn 무버퍼_경계는_수백_mb다() {
    // 큰 파일만 무버퍼 입출력을 쓴다(근거는 상수 문서). 흔한 파일이 무버퍼로 가지 않게 지킨다.
    const { assert!(UNBUFFERED_COPY_THRESHOLD >= 64 * 1024 * 1024) };
}

#[test]
fn 폴더를_만들고_시각과_속성을_맞춘다() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("원본 폴더");
    fs::create_dir(&source).unwrap();
    let target = dir.path().join("새 폴더");
    file_ops::create_folder(&target).unwrap();
    assert!(target.is_dir());
    let error = file_ops::create_folder(&target).unwrap_err();
    assert_eq!(error.kind(), FileOpErrorKind::TargetExists);

    // 안에 항목을 쓴 뒤 맞추므로 수정 시각이 바뀐 뒤여도 원본 것이 된다.
    fs::write(target.join("안쪽.txt"), "x").unwrap();
    let expected = set_old_times(&source, true);
    attrib(&source, &["+h", "+r"]);
    file_ops::copy_folder_metadata(&source, &target).unwrap();
    assert_eq!(times(&target), expected);
    assert_eq!(attributes(&target), attributes(&source));
    attrib(&source, &["-r", "-h"]);
    attrib(&target, &["-r", "-h"]);
}

#[test]
fn 같은_볼륨_이름_바꾸기는_파일_id를_지킨다() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("옛 이름.txt");
    fs::write(&file, "내용").unwrap();
    let sub = dir.path().join("하위");
    fs::create_dir(&sub).unwrap();
    let before = file_ops::file_id(&file).unwrap();
    let moved = sub.join("새 이름 🎉.txt");
    file_ops::rename(&file, &moved).unwrap();
    assert!(!file.exists());
    assert_eq!(file_ops::file_id(&moved).unwrap(), before);

    let folder = dir.path().join("폴더");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("안.txt"), "x").unwrap();
    let before = file_ops::file_id(&folder).unwrap();
    let renamed = dir.path().join("바꾼 폴더");
    file_ops::rename(&folder, &renamed).unwrap();
    assert_eq!(file_ops::file_id(&renamed).unwrap(), before);
    assert_ne!(
        file_ops::file_id(&renamed).unwrap(),
        file_ops::file_id(&moved).unwrap()
    );
}

#[test]
fn 이름_바꾸기는_대상이_있으면_대상_있음이다() {
    let dir = TempDir::new().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    fs::write(&a, "a").unwrap();
    fs::write(&b, "b").unwrap();
    let error = file_ops::rename(&a, &b).unwrap_err();
    assert_eq!(error.kind(), FileOpErrorKind::TargetExists, "{error:?}");
    assert_eq!(fs::read_to_string(&b).unwrap(), "b");
}

#[test]
fn 다른_볼륨_이동은_다른_볼륨이다() {
    let Some(other) = std::env::var_os(OTHER_VOLUME_ENV) else {
        println!("건너뜀: {OTHER_VOLUME_ENV}가 없어 다른 볼륨 이동을 확인하지 않았다");
        return;
    };
    let dir = TempDir::new().unwrap();
    let other = TempDir::new_in(PathBuf::from(other)).unwrap();
    let file = dir.path().join("옮길 파일.txt");
    fs::write(&file, "내용").unwrap();
    let target = other.path().join("옮긴 파일.txt");
    if file_ops::file_id(&file).unwrap().volume_serial
        == file_ops::file_id(other.path()).unwrap().volume_serial
    {
        panic!("{OTHER_VOLUME_ENV}가 임시 폴더와 같은 볼륨이다");
    }
    let error = file_ops::rename(&file, &target).unwrap_err();
    assert_eq!(error.kind(), FileOpErrorKind::CrossVolume, "{error:?}");
    assert!(file.exists());
    assert!(!target.exists());

    let folder = dir.path().join("옮길 폴더");
    fs::create_dir(&folder).unwrap();
    let error = file_ops::rename(&folder, &other.path().join("폴더")).unwrap_err();
    assert_eq!(error.kind(), FileOpErrorKind::CrossVolume, "{error:?}");
}

#[test]
fn 바꿔치기로_읽기_전용_파일도_덮어쓴다() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("기존.txt");
    let temp = dir.path().join("기존.txt.facet-tmp");
    fs::write(&target, "옛 내용").unwrap();
    attrib(&target, &["+r"]);
    fs::write(&temp, "새 내용").unwrap();
    file_ops::replace(&temp, &target).unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), "새 내용");
    assert!(!temp.exists());
    assert_eq!(attributes(&target) & READONLY, 0);
}

#[test]
fn 바꿔치기가_실패하면_읽기_전용을_되돌린다() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("기존.txt");
    fs::write(&target, "옛 내용").unwrap();
    attrib(&target, &["+r"]);
    let error = file_ops::replace(&dir.path().join("없는 임시.txt"), &target).unwrap_err();
    assert_eq!(error.kind(), FileOpErrorKind::SourceMissing);
    assert_eq!(attributes(&target) & READONLY, READONLY);
    assert_eq!(fs::read_to_string(&target).unwrap(), "옛 내용");
    attrib(&target, &["-r"]);
}

#[test]
fn 읽기_전용_파일과_빈_폴더를_지운다() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("읽기 전용.txt");
    fs::write(&file, "x").unwrap();
    attrib(&file, &["+r"]);
    file_ops::remove_file(&file).unwrap();
    assert!(!file.exists());

    let folder = dir.path().join("빈 폴더");
    fs::create_dir(&folder).unwrap();
    attrib(&folder, &["+r"]);
    file_ops::remove_empty_folder(&folder).unwrap();
    assert!(!folder.exists());

    let full = dir.path().join("찬 폴더");
    fs::create_dir(&full).unwrap();
    fs::write(full.join("a"), "x").unwrap();
    let error = file_ops::remove_empty_folder(&full).unwrap_err();
    assert_eq!(error.kind(), FileOpErrorKind::Other);
    assert!(error.code().is_some());

    let error = file_ops::remove_file(&dir.path().join("없음")).unwrap_err();
    assert_eq!(error.kind(), FileOpErrorKind::SourceMissing);
}

#[test]
fn 공유_없이_연_파일_복사는_잠김이다() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("잠긴 파일.txt");
    fs::write(&source, "내용").unwrap();
    let _held = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&source)
        .unwrap();
    let target = dir.path().join("대상.txt");
    let error = file_ops::copy_file(&source, &target, &mut continue_always).unwrap_err();
    assert_eq!(error.kind(), FileOpErrorKind::Locked, "{error:?}");
    assert!(!target.exists());
}

/// 지금 사용자의 SID(`whoami /user`).
fn current_user_sid() -> String {
    let out = Command::new("whoami")
        .args(["/user", "/fo", "csv", "/nh"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let sid = text.trim().rsplit(',').next().unwrap().trim_matches('"');
    assert!(sid.starts_with("S-1-"), "{text}");
    sid.to_owned()
}

/// 읽기 거부를 건 파일. 버릴 때 거부를 푼다.
struct DeniedRead {
    path: PathBuf,
    sid: String,
}

impl Drop for DeniedRead {
    fn drop(&mut self) {
        let _ = Command::new("icacls")
            .arg(&self.path)
            .args(["/remove:d", &format!("*{}", self.sid)])
            .output();
    }
}

#[test]
fn 읽기를_막은_파일_복사는_접근_거부다() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("막은 파일.txt");
    fs::write(&source, "비밀").unwrap();
    let denied = DeniedRead {
        path: source.clone(),
        sid: current_user_sid(),
    };
    let out = Command::new("icacls")
        .arg(&source)
        .args(["/deny", &format!("*{}:(R)", denied.sid)])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let target = dir.path().join("대상.txt");
    let error = file_ops::copy_file(&source, &target, &mut continue_always).unwrap_err();
    assert_eq!(error.kind(), FileOpErrorKind::AccessDenied, "{error:?}");
    assert!(!target.exists());
    drop(denied);
}

#[test]
fn 없는_원본은_원본_없음이다() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("없는 파일.txt");
    let target = dir.path().join("대상.txt");
    let error = file_ops::copy_file(&missing, &target, &mut continue_always).unwrap_err();
    assert_eq!(error.kind(), FileOpErrorKind::SourceMissing, "{error:?}");
    let error = file_ops::rename(&missing, &target).unwrap_err();
    assert_eq!(error.kind(), FileOpErrorKind::SourceMissing, "{error:?}");
    assert!(file_ops::file_id(&missing).is_err());
}

#[test]
fn 대상_폴더가_없으면_원본_없음이_아니다() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("있는 파일.txt");
    fs::write(&source, "x").unwrap();
    let target = dir.path().join("없는 폴더").join("대상.txt");
    let error = file_ops::copy_file(&source, &target, &mut continue_always).unwrap_err();
    assert_eq!(error.kind(), FileOpErrorKind::Other, "{error:?}");
    let error = file_ops::rename(&source, &target).unwrap_err();
    assert_eq!(error.kind(), FileOpErrorKind::Other, "{error:?}");
}

#[test]
fn 긴_경로와_한글_이모지_이름을_복사하고_이름_바꾼다() {
    let dir = TempDir::new().unwrap();
    let mut deep = dir.path().to_path_buf();
    for i in 0..6 {
        deep.push(format!(
            "아주 긴 폴더 이름 {i} 📁 가나다라마바사아자차카타파하 abcdefghij"
        ));
    }
    fs::create_dir_all(&deep).unwrap();
    let source = deep.join("원본 🎉 파일.txt");
    fs::write(&source, "긴 경로 내용").unwrap();
    assert!(source.as_os_str().len() > 260);
    let content = b"\xEA\xB0\x80 long".to_vec();
    File::create(&source).unwrap().write_all(&content).unwrap();
    set_old_times(&source, false);

    let copied = deep.join("복사본 😀 파일.txt");
    file_ops::copy_file(&source, &copied, &mut continue_always).unwrap();
    assert_eq!(fs::read(&copied).unwrap(), content);
    assert_eq!(times(&copied), times(&source));

    let before = file_ops::file_id(&copied).unwrap();
    let renamed = deep.join("바꾼 이름 🚀 파일.txt");
    file_ops::rename(&copied, &renamed).unwrap();
    assert_eq!(file_ops::file_id(&renamed).unwrap(), before);

    let temp = deep.join("임시 ✨.tmp");
    fs::write(&temp, "새 내용").unwrap();
    attrib_long_readonly(&renamed);
    file_ops::replace(&temp, &renamed).unwrap();
    assert_eq!(fs::read_to_string(&renamed).unwrap(), "새 내용");

    let folder = deep.join("새 폴더 🌱");
    file_ops::create_folder(&folder).unwrap();
    file_ops::copy_folder_metadata(&deep, &folder).unwrap();
    file_ops::remove_empty_folder(&folder).unwrap();
    file_ops::remove_file(&renamed).unwrap();
    assert!(!renamed.exists());
}

/// 긴 경로는 `attrib`이 받지 못하므로 std로 읽기 전용을 건다.
fn attrib_long_readonly(path: &Path) {
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions).unwrap();
}
