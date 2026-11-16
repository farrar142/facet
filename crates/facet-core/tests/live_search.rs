//! 실시간 탐색 엔진: 임시 폴더의 실제 파일로 결과가 빠짐없고 중복 없는지, 이름 조건·숨김 항목, 권한 없는 폴더
//! 건너뛰기, 순환하는 정션, 취소와 묶음 간격을 본다.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::live_search::{
    BATCH_INTERVAL, Found, LiveSearch, MAX_THREADS, SearchEnd, SearchEvent, SearchProgress,
    SearchRequest, SearchSummary, fault, thread_count,
};

const WAIT: Duration = Duration::from_secs(30);

fn request(root: &Path, pattern: &str, show_hidden: bool) -> SearchRequest {
    SearchRequest::name(Location::file_system(root), pattern, show_hidden)
}

struct Outcome {
    found: Vec<Found>,
    progress: Vec<SearchProgress>,
    summary: SearchSummary,
}

impl Outcome {
    /// 시작점 기준 상대 경로들(정렬, 중복이 있으면 실패).
    fn relative(&self, root: &Path) -> Vec<String> {
        let mut seen = BTreeSet::new();
        for found in &self.found {
            let path = found.path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            assert!(seen.insert(relative.clone()), "중복: {relative}");
        }
        seen.into_iter().collect()
    }
}

/// 끝날 때까지 사건을 받는다.
fn run(search: &mut LiveSearch) -> Outcome {
    let mut found = Vec::new();
    let mut progress = Vec::new();
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match search.wait_event(left).expect("탐색이 끝나지 않았다") {
            SearchEvent::Batch { items, progress: p } => {
                found.extend(items);
                progress.push(p);
            }
            SearchEvent::Finished(summary) => {
                assert!(
                    search.wait_event(Duration::from_millis(50)).is_none(),
                    "끝남 뒤에 사건이 왔다"
                );
                return Outcome {
                    found,
                    progress,
                    summary,
                };
            }
        }
    }
}

fn search(root: &Path, pattern: &str, show_hidden: bool) -> Outcome {
    let mut search = LiveSearch::start(request(root, pattern, show_hidden)).unwrap();
    run(&mut search)
}

/// 중첩 나무: 폴더 이름과 파일 이름(시작점 기준 상대 경로, `/`로).
const FOLDERS: &[&str] = &[
    "docs",
    "docs/2024",
    "docs/2024/보고서 묶음",
    "src",
    "src/core",
    "src/core/deep",
    "src/core/deep/deeper",
    "빈 폴더",
];
const FILES: &[&str] = &[
    "readme.md",
    "Report.TXT",
    "docs/report-final.txt",
    "docs/2024/분기 보고서.hwp",
    "docs/2024/보고서 묶음/1월 보고서.txt",
    "docs/2024/보고서 묶음/notes.md",
    "src/main.rs",
    "src/core/lib.rs",
    "src/core/deep/a.rs",
    "src/core/deep/deeper/b.txt",
    "src/core/deep/deeper/REPORT.rs",
];

fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for folder in FOLDERS {
        fs::create_dir_all(at(dir.path(), folder)).unwrap();
    }
    for file in FILES {
        fs::write(at(dir.path(), file), b"x").unwrap();
    }
    dir
}

/// 시작점 아래 상대 경로(`/`로 나눔)를 구분자를 맞춘 경로로. `mklink`·`icacls`는 `/`를 받지 않는다.
fn at(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |p, part| p.join(part))
}

fn sorted(names: &[&str]) -> Vec<String> {
    let set: BTreeSet<String> = names.iter().map(|s| (*s).to_owned()).collect();
    set.into_iter().collect()
}

#[test]
fn 스레드_수는_코어_수와_8_중_작은_것이다() {
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    assert_eq!(thread_count(), cores.min(MAX_THREADS));
    assert!(BATCH_INTERVAL >= Duration::from_secs(1) / 30);
}

#[test]
fn 빈_조건이면_시작점_아래_모든_항목이_빠짐없이_한_번씩_나온다() {
    let dir = tree();
    let outcome = search(dir.path(), "", true);
    let mut expected: Vec<&str> = FOLDERS.to_vec();
    expected.extend(FILES);
    assert_eq!(outcome.relative(dir.path()), sorted(&expected));
    let summary = outcome.summary;
    assert_eq!(summary.end, SearchEnd::Finished);
    assert_eq!(summary.found, expected.len());
    assert_eq!(summary.folders_scanned, FOLDERS.len() + 1);
    assert_eq!(summary.folders_skipped, 0);
    // 진행은 늘기만 하고 마지막 진행이 요약과 같다.
    let last = *outcome.progress.last().unwrap();
    assert_eq!(last.found, summary.found);
    assert_eq!(last.folders_scanned, summary.folders_scanned);
    assert!(
        outcome
            .progress
            .windows(2)
            .all(|w| w[0].found <= w[1].found && w[0].folders_scanned <= w[1].folders_scanned)
    );
}

#[test]
fn 결과_항목은_든_폴더와_목록_항목_정보를_갖는다() {
    let dir = tree();
    fs::write(at(dir.path(), "src/core/deep/sized.bin"), vec![7u8; 1234]).unwrap();
    let outcome = search(dir.path(), "sized", true);
    let [found] = outcome.found.as_slice() else {
        panic!("{:?}", outcome.relative(dir.path()));
    };
    assert_eq!(&*found.folder, at(dir.path(), "src/core/deep").as_path());
    assert_eq!(found.name(), "sized.bin");
    assert!(!found.item.is_folder);
    assert_eq!(found.item.size, 1234);
    let modified = fs::metadata(found.path()).unwrap().modified().unwrap();
    assert_eq!(found.item.modified, modified);
    let folder = search(dir.path(), "deeper", true);
    assert!(folder.found[0].item.is_folder);
}

#[test]
fn 이름_조건은_부분_일치_와일드카드_한글을_따른다() {
    let dir = tree();
    assert_eq!(
        search(dir.path(), "report", true).relative(dir.path()),
        sorted(&[
            "Report.TXT",
            "docs/report-final.txt",
            "src/core/deep/deeper/REPORT.rs",
        ])
    );
    assert_eq!(
        search(dir.path(), "*.txt", true).relative(dir.path()),
        sorted(&[
            "Report.TXT",
            "docs/report-final.txt",
            "docs/2024/보고서 묶음/1월 보고서.txt",
            "src/core/deep/deeper/b.txt",
        ])
    );
    assert_eq!(
        search(dir.path(), "?.rs", true).relative(dir.path()),
        sorted(&["src/core/deep/a.rs"])
    );
    // 폴더도 맞추고, 맞은 폴더 안으로도 들어간다.
    assert_eq!(
        search(dir.path(), "보고서", true).relative(dir.path()),
        sorted(&[
            "docs/2024/분기 보고서.hwp",
            "docs/2024/보고서 묶음",
            "docs/2024/보고서 묶음/1월 보고서.txt",
        ])
    );
    let none = search(dir.path(), "없는 이름", true);
    assert!(none.found.is_empty());
    assert_eq!(none.summary.found, 0);
    assert_eq!(none.summary.end, SearchEnd::Finished);
}

#[cfg(windows)]
fn hide(path: &Path) {
    let status = std::process::Command::new("attrib")
        .arg("+h")
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
}

#[cfg(windows)]
#[test]
fn 숨김_항목은_보이기를_따르고_숨김_폴더_안으로는_들어가지_않는다() {
    let dir = tree();
    hide(&dir.path().join("readme.md"));
    hide(&at(dir.path(), "src/core"));
    let hidden = search(dir.path(), "", false).relative(dir.path());
    assert!(!hidden.contains(&"readme.md".to_owned()));
    assert!(
        !hidden.iter().any(|p| p.starts_with("src/core")),
        "{hidden:?}"
    );
    assert!(hidden.contains(&"src/main.rs".to_owned()));
    let shown = search(dir.path(), "", true).relative(dir.path());
    assert_eq!(shown.len(), FOLDERS.len() + FILES.len());
}

#[test]
fn 시작점이_폴더가_아니면_시작하지_않는다() {
    let dir = tree();
    assert!(LiveSearch::start(request(&dir.path().join("readme.md"), "", true)).is_err());
    assert!(LiveSearch::start(request(&dir.path().join("없음"), "", true)).is_err());
}

/// 폴더 내용 나열을 모두에게(Everyone) 거부하고, 끝나면 되돌린다.
#[cfg(windows)]
struct Lock(PathBuf);

#[cfg(windows)]
impl Lock {
    /// 백업 권한이 켜진 프로세스는 거부 ACE를 무시하므로 막은 뒤에도 읽히면 `None`.
    fn new(path: &Path) -> Option<Lock> {
        assert!(icacls(path, &["/deny", "*S-1-1-0:(RD)"]), "icacls 실패");
        let lock = Lock(path.to_path_buf());
        if fs::read_dir(path).is_ok() {
            return None;
        }
        Some(lock)
    }
}

#[cfg(windows)]
impl Drop for Lock {
    fn drop(&mut self) {
        icacls(&self.0, &["/remove:d", "*S-1-1-0"]);
    }
}

#[cfg(windows)]
fn icacls(path: &Path, args: &[&str]) -> bool {
    std::process::Command::new("icacls")
        .arg(path)
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
}

#[cfg(windows)]
#[test]
fn 목록_읽기를_막은_폴더는_건너뛰고_수를_센다() {
    let dir = tree();
    let locks: Vec<Lock> = ["docs/2024", "src/core/deep/deeper"]
        .iter()
        .filter_map(|f| Lock::new(&at(dir.path(), f)))
        .collect();
    if locks.len() != 2 {
        println!("건너뜀: 이 프로세스는 폴더 읽기 거부를 무시한다(백업 권한)");
        return;
    }
    let outcome = search(dir.path(), "", true);
    let found = outcome.relative(dir.path());
    // 막은 폴더 자신은 위 폴더 목록에 있으니 나오고, 그 안은 나오지 않는다.
    assert!(found.contains(&"docs/2024".to_owned()));
    assert!(found.contains(&"src/core/deep/deeper".to_owned()));
    assert!(
        !found.iter().any(|p| p.starts_with("docs/2024/")),
        "{found:?}"
    );
    assert!(
        !found.iter().any(|p| p.starts_with("src/core/deep/deeper/")),
        "{found:?}"
    );
    assert!(found.contains(&"docs/report-final.txt".to_owned()));
    let summary = outcome.summary;
    assert_eq!(summary.end, SearchEnd::Finished);
    assert_eq!(summary.folders_skipped, 2);
    // 막은 둘과 그 안의 「보고서 묶음」은 읽지 않았다.
    assert_eq!(summary.folders_scanned, FOLDERS.len() + 1 - 3);
    drop(locks);
}

#[test]
fn 읽지_못한_폴더는_건너뛰고_수를_센다_주입() {
    // CI 러너처럼 백업 권한이 켜져 icacls 거부가 듣지 않는 곳에서도 건너뛰기와 수를 본다.
    let dir = tree();
    let _denied = [
        fault::deny_listing(&at(dir.path(), "docs/2024")),
        fault::deny_listing(&at(dir.path(), "src/core/deep/deeper")),
    ];
    let outcome = search(dir.path(), "", true);
    let found = outcome.relative(dir.path());
    assert!(found.contains(&"docs/2024".to_owned()));
    assert!(
        !found.iter().any(|p| p.starts_with("docs/2024/")),
        "{found:?}"
    );
    assert!(
        !found.iter().any(|p| p.starts_with("src/core/deep/deeper/")),
        "{found:?}"
    );
    assert_eq!(outcome.summary.folders_skipped, 2);
    assert_eq!(outcome.summary.folders_scanned, FOLDERS.len() + 1 - 3);
    assert_eq!(outcome.progress.last().unwrap().folders_skipped, 2);
}

#[cfg(windows)]
fn junction(link: &Path, target: &Path) -> bool {
    std::process::Command::new("cmd")
        .arg("/c")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .output()
        .is_ok_and(|o| o.status.success())
}

#[cfg(windows)]
#[test]
fn 자기_상위를_가리키는_정션이_있어도_끝나고_중복이_없다() {
    let dir = tree();
    let made = junction(&at(dir.path(), "src/core/deep/위로"), dir.path())
        && junction(&at(dir.path(), "docs/나"), &dir.path().join("docs"));
    if !made {
        println!("건너뜀: 정션을 만들 수 없는 환경이다");
        return;
    }
    let outcome = search(dir.path(), "", true);
    let mut expected: Vec<&str> = FOLDERS.to_vec();
    expected.extend(FILES);
    // 링크 자체는 나오고 그 안으로는 들어가지 않는다.
    expected.extend(["src/core/deep/위로", "docs/나"]);
    assert_eq!(outcome.relative(dir.path()), sorted(&expected));
    assert_eq!(outcome.summary.end, SearchEnd::Finished);
    assert_eq!(outcome.summary.folders_scanned, FOLDERS.len() + 1);
    let link = search(dir.path(), "위로", true);
    assert_eq!(link.found.len(), 1);
    assert!(link.found[0].item.is_folder);
}

/// 폴더 `folders`개에 파일 `files`개씩 든 넓은 나무.
fn wide_tree(folders: usize, files: usize) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for f in 0..folders {
        let folder = dir.path().join(format!("폴더 {f:03}")).join("안");
        fs::create_dir_all(&folder).unwrap();
        for i in 0..files {
            fs::write(folder.join(format!("파일 {i:04}.txt")), b"").unwrap();
        }
    }
    dir
}

#[test]
fn 곧바로_취소하면_묶음이_오지_않고_취소로_끝난다() {
    let dir = wide_tree(40, 20);
    let mut search = LiveSearch::start(request(dir.path(), "", true)).unwrap();
    search.cancel();
    assert!(search.is_cancelled());
    let outcome = run(&mut search);
    assert!(outcome.found.is_empty());
    assert!(outcome.progress.is_empty());
    assert_eq!(outcome.summary.end, SearchEnd::Cancelled);
}

#[test]
fn 첫_묶음_뒤에_취소하면_그_뒤로_묶음이_오지_않는다() {
    let dir = wide_tree(60, 40);
    let mut search = LiveSearch::start(request(dir.path(), "", true)).unwrap();
    let first = search.wait_event(WAIT).unwrap();
    let SearchEvent::Batch { items, .. } = first else {
        // 끝까지 한 번에 끝났다면(아주 빠른 기계) 취소할 것이 없다.
        return;
    };
    search.cancel();
    let rest = run(&mut search);
    assert!(rest.found.is_empty(), "취소 뒤 묶음: {}", rest.found.len());
    assert!(rest.progress.is_empty());
    assert_eq!(rest.summary.end, SearchEnd::Cancelled);
    assert!(rest.summary.found >= items.len());
    assert!(rest.summary.found < 60 * 41 + 60, "취소했는데 다 찾았다");
}

#[test]
fn 손잡이를_버리면_더_보내지_않는다() {
    let dir = wide_tree(40, 20);
    let calls = Arc::new(Mutex::new(0usize));
    let counter = Arc::clone(&calls);
    let search = LiveSearch::start_with_waker(request(dir.path(), "", true), move || {
        *counter.lock().unwrap() += 1;
    })
    .unwrap();
    drop(search);
    let at_drop = *calls.lock().unwrap();
    std::thread::sleep(BATCH_INTERVAL * 5);
    // 받을 곳이 없으니 깨우지 않는다(버린 순간 보내던 사건 하나는 있을 수 있다).
    assert!(*calls.lock().unwrap() <= at_drop + 1);
}

#[test]
fn 묶음_사이_간격은_상한을_지키고_깨우기가_사건마다_불린다() {
    let dir = wide_tree(80, 40);
    let times = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&times);
    let mut search = LiveSearch::start_with_waker(request(dir.path(), "", true), move || {
        recorder.lock().unwrap().push(Instant::now());
    })
    .unwrap();
    let outcome = run(&mut search);
    assert_eq!(outcome.summary.end, SearchEnd::Finished);
    assert_eq!(outcome.found.len(), 80 * 2 + 80 * 40);
    let times = times.lock().unwrap();
    // 묶음마다 하나와 끝남 하나.
    assert_eq!(times.len(), outcome.progress.len() + 1);
    let batches = &times[..outcome.progress.len()];
    for pair in batches.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(
            gap >= BATCH_INTERVAL,
            "묶음 간격 {gap:?} < {BATCH_INTERVAL:?}"
        );
    }
}
