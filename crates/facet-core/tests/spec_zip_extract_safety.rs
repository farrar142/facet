// target: crates/facet-core/tests/spec_zip_extract_safety.rs
// AC: #426: 대상 밖을 가리키는 이름은 아무것도 쓰지 않고 OutsideTarget으로 나열, 정규화해도 안에 남는 이름은 꺼냄, 예약 이름·금지 글자·끝 점은 InvalidName, 심볼릭 링크는 Symlink, 빈 공간이 모자라면 NotEnoughSpace
//! 악성 zip을 꺼낼 때(`JobQueue::extract_all`·zip 항목 복사) 대상 폴더 밖에 쓰지 않는지를 임시 폴더의 실제 zip
//! 파일로 확인한다: `..`·절대 경로·드라이브 문자·UNC·대체 데이터 스트림 이름은 대상 안팎 어디에도 쓰이지 않고
//! 작업 결과에 기록된 이름과 까닭으로 나열되며 나머지는 꺼내진다. 예약 이름·금지 글자, 심볼릭 링크 항목, 선언 크기가
//! 빈 공간보다 큰 zip도 본다. 대상 밖에 썼는지 보려고 이 프로세스 번호가 든 이름을 쓴다. 파일 연산이 Windows
//! 전용이라 Windows에서만 돈다.

#![cfg(windows)]

#[path = "spec_zip_support.rs"]
mod zip_support;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use facet_core::job::{CopyRejection, JobEvent, JobId, JobQueue, JobState, Summary, ZipSkipReason};
use tempfile::TempDir;
use zip_support::Entry;

const WAIT: Duration = Duration::from_secs(120);

fn finish(queue: &mut JobQueue, job: JobId) -> (JobState, Summary) {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match queue.wait_event(left) {
            Some(JobEvent::Finished { job: id, summary }) if id == job => {
                return (queue.state(job).unwrap(), summary);
            }
            Some(JobEvent::Conflict { job: id, conflict }) if id == job => {
                panic!("묻지 않아야 한다: {conflict:?}")
            }
            Some(JobEvent::ItemError { job: id, error }) if id == job => {
                panic!("항목별 오류로 묻지 않고 거부 목록에 담는다: {error:?}")
            }
            Some(_) => {}
            None => panic!("작업이 시간 안에 끝나지 않았다"),
        }
    }
}

/// 폴더 아래 모든 항목의 상대 경로(`/` 구분).
fn tree(root: &Path) -> BTreeSet<String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeSet<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.insert(relative);
            if path.is_dir() {
                walk(root, &path, out);
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(root, root, &mut out);
    out
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn skipped_with(summary: &Summary, pick: impl Fn(&ZipSkipReason) -> bool) -> BTreeSet<String> {
    summary
        .zip_skipped
        .iter()
        .filter(|s| pick(&s.reason))
        .map(|s| s.entry.clone())
        .collect()
}

/// `dir/out/대상`을 대상으로 꺼낼 준비: 그 위 폴더에 무엇이 새로 생기는지 볼 수 있게 한 단계 더 둔다.
fn target_in(dir: &TempDir) -> (PathBuf, PathBuf) {
    let out = dir.path().join("out");
    let target = out.join("대상");
    fs::create_dir_all(&target).unwrap();
    (out, target)
}

#[test]
#[ignore = "#426"]
fn names_pointing_outside_the_target_are_refused_and_listed() {
    let pid = std::process::id();
    let evil: Vec<String> = (1..=7)
        .map(|n| format!("facet-evil-{pid}-{n}.txt"))
        .collect();
    let outside_names = vec![
        format!("../{}", evil[0]),
        format!("..\\{}", evil[1]),
        format!("/{}", evil[2]),
        format!("C:/{}", evil[3]),
        format!("C:{}", evil[4]),
        format!("//localhost/C$/{}", evil[5]),
        format!("ok/../../{}", evil[6]),
        "stream.txt:hidden".to_string(),
    ];
    let mut entries: Vec<Entry> = outside_names
        .iter()
        .map(|name| Entry::stored(name, b"evil"))
        .collect();
    entries.push(Entry::stored("good.txt", b"good"));
    entries.push(Entry::stored("dir/ok.txt", b"ok"));
    entries.push(Entry::stored("dir/../inside.txt", b"inside"));

    let dir = TempDir::new().unwrap();
    let zip = dir.path().join("악성.zip");
    zip_support::write_zip(&zip, &entries);
    let (out, target) = target_in(&dir);

    let mut queue = JobQueue::new();
    let job = queue.extract_all(zip.clone(), target.clone()).unwrap();
    let (state, summary) = finish(&mut queue, job);

    assert_eq!(state, JobState::Failed, "거부한 항목이 있다");
    assert_eq!(
        tree(&target),
        set(&["dir", "dir/ok.txt", "good.txt", "inside.txt"]),
        "대상 안에는 안전한 항목만 쓰인다(stream.txt도 생기지 않는다)"
    );
    assert_eq!(fs::read(target.join("inside.txt")).unwrap(), b"inside");
    assert_eq!(
        tree(&out),
        set(&[
            "대상",
            "대상/dir",
            "대상/dir/ok.txt",
            "대상/good.txt",
            "대상/inside.txt"
        ])
    );
    assert_eq!(
        tree(dir.path())
            .into_iter()
            .filter(|p| !p.starts_with("out"))
            .collect::<BTreeSet<_>>(),
        set(&["악성.zip"]),
        "대상 위 폴더에도 아무것도 쓰지 않았다"
    );
    let cwd = std::env::current_dir().unwrap();
    for name in &evil {
        for place in [
            PathBuf::from(r"C:\"),
            PathBuf::from(r"\"),
            cwd.clone(),
            std::env::temp_dir(),
        ] {
            assert!(
                !place.join(name).exists(),
                "대상 밖에 썼다: {}",
                place.join(name).display()
            );
        }
    }

    assert_eq!(
        skipped_with(&summary, |r| *r == ZipSkipReason::OutsideTarget),
        outside_names.iter().cloned().collect::<BTreeSet<_>>(),
        "거부한 항목을 기록된 이름 그대로 나열한다: {:?}",
        summary.zip_skipped
    );
}

#[test]
#[ignore = "#426"]
fn reserved_and_forbidden_names_are_refused_by_the_name_check() {
    let dir = TempDir::new().unwrap();
    let zip = dir.path().join("이름.zip");
    let bad = ["CON.txt", "a<b.txt", "끝점.", "sub/NUL", "물음표?.txt"];
    let mut entries: Vec<Entry> = bad.iter().map(|name| Entry::stored(name, b"x")).collect();
    entries.push(Entry::stored("좋음.txt", b"good"));
    zip_support::write_zip(&zip, &entries);
    let (_out, target) = target_in(&dir);

    let mut queue = JobQueue::new();
    let job = queue.extract_all(zip, target.clone()).unwrap();
    let (state, summary) = finish(&mut queue, job);
    assert_eq!(state, JobState::Failed);
    assert_eq!(
        skipped_with(&summary, |r| matches!(r, ZipSkipReason::InvalidName(_))),
        set(&bad),
        "{:?}",
        summary.zip_skipped
    );
    let written = tree(&target);
    assert!(written.contains("좋음.txt"), "{written:?}");
    assert!(
        written.iter().all(|p| p == "좋음.txt" || p == "sub"),
        "쓸 수 없는 이름은 쓰지 않는다: {written:?}"
    );
}

#[test]
#[ignore = "#426"]
fn symbolic_link_entries_are_skipped() {
    let dir = TempDir::new().unwrap();
    let zip = dir.path().join("링크.zip");
    zip_support::write_zip(
        &zip,
        &[
            Entry::stored("link", b"../../outside").symlink(),
            Entry::stored("좋음.txt", b"good"),
        ],
    );
    let (_out, target) = target_in(&dir);
    let mut queue = JobQueue::new();
    let job = queue.extract_all(zip, target.clone()).unwrap();
    let (state, summary) = finish(&mut queue, job);
    assert_eq!(state, JobState::Failed);
    assert_eq!(
        skipped_with(&summary, |r| *r == ZipSkipReason::Symlink),
        set(&["link"])
    );
    assert_eq!(
        tree(&target),
        set(&["좋음.txt"]),
        "링크도 파일도 만들지 않는다"
    );
}

#[test]
#[ignore = "#426"]
fn a_zip_larger_than_the_free_space_is_refused_before_starting() {
    let dir = TempDir::new().unwrap();
    let zip = dir.path().join("폭탄.zip");
    let declared = 1u64 << 60;
    zip_support::write(
        &zip,
        &zip_support::build_zip64(&[
            Entry::stored("작은.txt", b"s"),
            Entry {
                size: declared,
                ..Entry::stored("huge.bin", b"")
            },
        ]),
    );
    let (_out, target) = target_in(&dir);
    let mut queue = JobQueue::new();

    match queue.extract_all(zip.clone(), target.clone()) {
        Err(CopyRejection::NotEnoughSpace { needed, available }) => {
            assert!(needed >= declared, "선언 크기의 합: {needed}");
            assert!(available < needed, "남은 공간: {available}");
        }
        other => panic!("빈 공간이 모자라 시작하지 않아야 한다: {other:?}"),
    }
    assert!(
        matches!(
            queue.copy(vec![zip.join("huge.bin")], target.clone()),
            Err(CopyRejection::NotEnoughSpace { .. })
        ),
        "zip 항목 복사도 같다"
    );
    assert!(tree(&target).is_empty(), "아무것도 쓰지 않았다");
    assert!(!queue.has_unfinished(), "작업을 만들지 않았다");
}
