// target: crates/facet-core/tests/spec_virtual_folders_device_conflict.rs
// AC: #357: 장치가 날짜를 주지 않는 충돌에서는 「더 새것만 덮어쓰기」가 꺼진다
//! 휴대폰(MTP)과 PC 사이 복사의 이름 충돌에서 「더 새것만 덮어쓰기」를 고를 수 있는지를 장치 없이 확인한다.
//! 고를 수 있는지는 충돌 질문(`job::Conflict`)이 값으로 알린다(`Conflict::can_overwrite_if_newer`): 두 쪽 모두
//! 수정 시각이 있고 덮어쓸 수 있을 때만 참이다. 장치가 날짜를 주지 않는 충돌은 질문 값을 직접 만들어 보고(모든
//! 플랫폼), 두 쪽 모두 PC 파일인 실제 복사의 충돌은 그대로 고를 수 있는지를 임시 폴더로 본다(Windows 전용).
//! 휴대폰과의 실제 복사는 러너에 MTP 장치가 없어 사람이 확인한다.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use facet_core::job::{Conflict, ConflictItem, ItemKind};

fn file(path: &str, modified: Option<SystemTime>) -> ConflictItem {
    ConflictItem {
        path: PathBuf::from(path),
        kind: ItemKind::File,
        size: Some(10),
        modified,
    }
}

fn conflict(
    source: Option<SystemTime>,
    target: Option<SystemTime>,
    can_overwrite: bool,
) -> Conflict {
    Conflict {
        source: file(r"휴대폰\DCIM\사진.jpg", source),
        target: file(r"C:\사진\사진.jpg", target),
        can_overwrite,
        overwrite_is_permanent: false,
    }
}

fn some_time() -> Option<SystemTime> {
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000))
}

#[test]
fn newer_only_is_offered_when_both_sides_have_a_date() {
    assert!(conflict(some_time(), some_time(), true).can_overwrite_if_newer());
}

#[test]
fn newer_only_is_off_when_the_device_gives_no_date() {
    assert!(
        !conflict(None, some_time(), true).can_overwrite_if_newer(),
        "원본(장치)에 날짜가 없다"
    );
    assert!(
        !conflict(some_time(), None, true).can_overwrite_if_newer(),
        "대상(장치)에 날짜가 없다"
    );
    assert!(
        !conflict(None, None, true).can_overwrite_if_newer(),
        "두 쪽 모두 날짜가 없다"
    );
    // 덮어쓰기 자체는 그대로 고를 수 있다.
    assert!(conflict(None, some_time(), true).can_overwrite);
}

#[test]
fn newer_only_is_off_when_overwriting_is_not_allowed() {
    assert!(!conflict(some_time(), some_time(), false).can_overwrite_if_newer());
}

#[cfg(windows)]
#[test]
fn a_conflict_between_pc_files_still_offers_newer_only() {
    use std::fs;
    use std::time::Instant;

    use facet_core::job::{ConflictAnswer, JobEvent, JobQueue, Resolution};
    use tempfile::TempDir;

    const TIMEOUT: Duration = Duration::from_secs(120);

    let dir = TempDir::new().unwrap();
    let source_dir = dir.path().join("원본");
    let target_dir = dir.path().join("대상");
    fs::create_dir_all(&source_dir).unwrap();
    fs::create_dir_all(&target_dir).unwrap();
    let source = source_dir.join("같은 이름.txt");
    fs::write(&source, "원본").unwrap();
    fs::write(target_dir.join("같은 이름.txt"), "대상").unwrap();

    let mut queue = JobQueue::new();
    let job = queue
        .copy(vec![source], target_dir.clone())
        .expect("복사 작업을 만든다");

    let deadline = Instant::now() + TIMEOUT;
    let found = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match queue.wait_event(left) {
            Some(JobEvent::Conflict { job: id, conflict }) if id == job => break conflict,
            Some(JobEvent::Finished { job: id, .. }) if id == job => {
                panic!("이름 충돌을 묻기 전에 작업이 끝났다")
            }
            Some(_) => {}
            None => panic!("이름 충돌 질문이 오지 않았다"),
        }
    };
    assert!(found.source.modified.is_some() && found.target.modified.is_some());
    assert!(
        found.can_overwrite_if_newer(),
        "PC 파일끼리의 충돌은 「더 새것만 덮어쓰기」를 고를 수 있다: {found:?}"
    );

    queue
        .answer_conflict(
            job,
            ConflictAnswer {
                resolution: Resolution::Skip,
                apply_to_rest: false,
            },
        )
        .expect("건너뛰기 답을 받는다");
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match queue.wait_event(left) {
            Some(JobEvent::Finished { job: id, .. }) if id == job => break,
            Some(_) => {}
            None => panic!("작업이 끝나지 않았다"),
        }
    }
    assert_eq!(
        fs::read_to_string(target_dir.join("같은 이름.txt")).unwrap(),
        "대상"
    );
}
