//! 복사 작업: 훑기와 복사를 나란히 한다.
//!
//! 작업이 시작되면 훑기 스레드가 원본을 깊이 우선으로 훑어 항목을 하나씩 통로에 넣으며 전체 항목 수와 바이트를
//! 센다. 작업 일꾼 스레드는 통로에서 이미 센 항목부터 꺼내 복사한다. 그래서 큰 폴더도 다 세기를 기다리지 않고
//! 곧바로 복사가 시작되고, 다 셀 때까지 남은 시간은 「계산 중」이다.
//!
//! 훑기는 폴더마다 「폴더 시작」을 넣고 그 안의 항목을 넣은 뒤 「폴더 끝」을 넣는다. 복사는 폴더 시작에서 폴더를
//! 만들고, 폴더 끝에서(안의 항목을 다 쓴 뒤) 폴더 시각·속성을 원본과 맞춘다.
//!
//! 통로는 크기 제한이 없다: 훑기가 복사보다 앞서 나가야 전체를 셀 수 있기 때문이다. 항목 하나가 경로 둘만큼의
//! 메모리를 쓴다.

use std::ffi::{OsStr, OsString};
use std::fs::{self, FileType};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use facet_win::file_ops::{self, CopyStep, FileOpErrorKind};

use super::{
    Conflict, ConflictItem, Context, Control, CopyRejection, ItemFailure, ItemKind, JobEvent,
    JobId, Outbox, PROGRESS_INTERVAL, Progress, Remaining, Resolution, Summary, is_within,
    same_path,
};

/// 대상이 이미 있어 다시 확인하는 횟수의 한도. 확인과 쓰기 사이에 밖에서 같은 이름을 만들면 다시 묻는데, 그것이
/// 끝없이 되풀이되지 않게 한다.
const MAX_ATTEMPTS: u32 = 3;

/// 덮어쓰기 임시 파일 이름의 번호(프로세스 안에서 겹치지 않게).
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 같은 폴더로 복사할 때 이름 뒤에 붙이는 말(탐색기와 같다).
///
/// 파일 이름이라 번역하는 화면 문장이 아니다(ADR-0007의 「번역하지 않는 것: 파일·폴더 이름」). 원문 언어인
/// 한국어 탐색기와 같은 이름을 만든다.
const COPY_SUFFIX: &str = " - 복사본";

/// 넣기 전에 확인을 마친 복사 작업.
pub(super) struct CopyJob {
    roots: Vec<Root>,
    target: PathBuf,
}

/// 넘긴 원본 하나.
struct Root {
    source: PathBuf,
    /// 원본이 대상 폴더 바로 안에 있다(「- 복사본」 이름을 만든다).
    same_folder: bool,
}

impl CopyJob {
    /// 시작 전에 막을 경우를 확인한다.
    pub(super) fn prepare(sources: Vec<PathBuf>, target: PathBuf) -> Result<Self, CopyRejection> {
        if sources.is_empty() {
            return Err(CopyRejection::NoSources);
        }
        let metadata = fs::metadata(&target).map_err(|_| CopyRejection::TargetMissing)?;
        if !metadata.is_dir() {
            return Err(CopyRejection::TargetNotFolder);
        }
        let canonical_target =
            fs::canonicalize(&target).map_err(|_| CopyRejection::TargetMissing)?;
        let mut roots = Vec::with_capacity(sources.len());
        for source in sources {
            // 폴더(링크가 아닌)만 자기 자신·하위 복사가 될 수 있다. 원본이 없으면 여기서 막지 않고 작업이 항목
            // 오류로 센다(넣은 뒤 사라질 수도 있으므로 어차피 그때 다뤄야 한다).
            let is_folder = fs::symlink_metadata(&source)
                .map(|m| m.is_dir() && !m.file_type().is_symlink())
                .unwrap_or(false);
            if is_folder && let Ok(canonical_source) = fs::canonicalize(&source) {
                if same_path(&canonical_target, &canonical_source) {
                    return Err(CopyRejection::IntoItself(source));
                }
                if is_within(&canonical_target, &canonical_source) {
                    return Err(CopyRejection::TargetInsideSource(source));
                }
            }
            let same_folder = source
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .and_then(|parent| fs::canonicalize(parent).ok())
                .is_some_and(|parent| same_path(&parent, &canonical_target));
            roots.push(Root {
                source,
                same_folder,
            });
        }
        Ok(CopyJob { roots, target })
    }

    /// 넘긴 원본 수.
    pub(super) fn root_count(&self) -> u64 {
        self.roots.len() as u64
    }

    /// 작업 스레드에서 끝까지(또는 취소될 때까지) 실행하고 결과 요약과 취소로 멈췄는지를 돌려준다.
    ///
    /// 일시정지는 항목 사이와 파일 복사의 진행 콜백에서 기다리고, 훑기도 항목마다 기다린다. 취소하면 복사하던
    /// 파일을 그만두고(만들다 만 대상은 `copy_file`이 지운다) 훑기를 멈춘다. 취소로 멈추면 마지막 진행 사건은
    /// 보내지 않는다(끝남 사건의 결과가 마지막 수치다).
    pub(super) fn run(self, context: &Context<'_>) -> (Summary, bool) {
        let (id, outbox, control) = (context.id, context.outbox, context.control);
        let plans = self.plan();
        let totals = Arc::new(Totals::default());
        let (sender, receiver) = mpsc::channel();
        let (mut summary, cancelled) = thread::scope(|scope| {
            let scanner = {
                let totals = Arc::clone(&totals);
                thread::Builder::new()
                    .name("facet-job-scan".into())
                    .spawn_scoped(scope, move || scan(plans, &sender, &totals, control))
                    .expect("훑기 스레드를 띄우지 못했다")
            };
            let mut copier = Copier {
                reporter: Reporter::new(id, outbox.clone(), Arc::clone(&totals)),
                summary: Summary::default(),
                skip_depth: 0,
                folders: Vec::new(),
                remembered: None,
                context,
                cancelled: false,
            };
            // 훑기가 끝나 통로를 닫을 때까지 받는다. 취소하면 통로를 버려 훑기도 곧 멈춘다.
            for entry in receiver {
                if !control.proceed() {
                    copier.cancelled = true;
                }
                if copier.cancelled {
                    break;
                }
                copier.handle(entry);
            }
            let _ = scanner.join();
            // 훑기는 취소됐을 때만 다 세지 않고 멈춘다. 복사 쪽이 취소를 보기 전에 훑기가 먼저 멈춰 통로가
            // 닫혔어도(예: 항목을 하나도 넣기 전에 일시정지한 채 취소) 취소로 끝낸다.
            if !totals.counted.load(Ordering::Acquire) {
                copier.cancelled = true;
            }
            if !copier.cancelled {
                copier.reporter.current = None;
                copier.reporter.finish();
            }
            copier.summary.items_done = copier.reporter.items_done;
            (copier.summary, copier.cancelled)
        });
        summary.all_counted = totals.counted.load(Ordering::Acquire);
        summary.items_remaining = totals
            .items
            .load(Ordering::Acquire)
            .saturating_sub(summary.items_done);
        (summary, cancelled)
    }

    /// 원본마다 대상 경로를 정한다. 같은 폴더 복사면 지금 있는 이름과 이 작업에서 이미 정한 이름을 피해
    /// 「- 복사본」 이름을 만든다.
    fn plan(&self) -> Vec<(PathBuf, Option<PathBuf>)> {
        let mut taken: Vec<PathBuf> = Vec::new();
        self.roots
            .iter()
            .map(|root| {
                let Some(name) = root.source.file_name() else {
                    return (root.source.clone(), None);
                };
                let target = if root.same_folder {
                    let is_file = fs::symlink_metadata(&root.source)
                        .map(|m| !m.is_dir())
                        .unwrap_or(true);
                    let mut n = 1;
                    loop {
                        let candidate = self.target.join(copy_name(name, is_file, n));
                        let used = fs::symlink_metadata(&candidate).is_ok()
                            || taken.iter().any(|t| same_path(t, &candidate));
                        if !used {
                            break candidate;
                        }
                        n += 1;
                    }
                } else {
                    self.target.join(name)
                };
                taken.push(target.clone());
                (root.source.clone(), Some(target))
            })
            .collect()
    }
}

/// 같은 폴더 복사의 `n`번째 이름: 1이면 「이름 - 복사본」, 2부터 「이름 - 복사본 (n)」. 파일은 확장자를 뒤에 둔다.
fn copy_name(name: &OsStr, is_file: bool, n: u32) -> OsString {
    let mut suffix = COPY_SUFFIX.to_owned();
    if n > 1 {
        suffix.push_str(&format!(" ({n})"));
    }
    decorated_name(name, is_file, &suffix)
}

/// 둘 다 유지의 `n`번째(2부터) 이름: 「이름 (n)」. 파일은 확장자를 뒤에 둔다(탐색기와 같다).
fn numbered_name(name: &OsStr, is_file: bool, n: u32) -> OsString {
    decorated_name(name, is_file, &format!(" ({n})"))
}

/// 이름 뒤(파일은 확장자 앞)에 `suffix`를 붙인다.
fn decorated_name(name: &OsStr, is_file: bool, suffix: &str) -> OsString {
    let path = Path::new(name);
    let (stem, extension) = match (is_file, path.file_stem(), path.extension()) {
        (true, Some(stem), Some(extension)) => (stem, Some(extension)),
        _ => (name, None),
    };
    let mut result = stem.to_owned();
    result.push(suffix);
    if let Some(extension) = extension {
        result.push(".");
        result.push(extension);
    }
    result
}

/// 둘 다 유지에 쓸 이름: `target`과 같은 폴더에서 아직 없는 「이름 (2)」, 「이름 (3)」 ….
fn free_numbered(target: &Path, is_file: bool) -> PathBuf {
    let (Some(parent), Some(name)) = (target.parent(), target.file_name()) else {
        return target.to_path_buf();
    };
    (2..)
        .map(|n| parent.join(numbered_name(name, is_file, n)))
        .find(|candidate| fs::symlink_metadata(candidate).is_err())
        .expect("빈 이름은 언젠가 나온다")
}

/// 덮어쓰기 임시 파일 경로: 대상과 같은 폴더(바꿔치기가 같은 볼륨 안 이름 바꾸기가 되게)의 아직 없는 짧은 이름.
/// 원래 이름을 넣지 않아 긴 이름에서도 이름 길이 한도를 넘지 않는다.
fn temp_path(target: &Path) -> PathBuf {
    let parent = target.parent().unwrap_or(Path::new(""));
    loop {
        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!("~facet-{}-{n}.tmp", std::process::id()));
        if fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
    }
}

/// 이름 충돌의 한 쪽 항목 정보. 링크는 가리키는 쪽의 종류로 본다(읽지 못하면 링크 자신).
fn conflict_item(path: &Path) -> ConflictItem {
    let metadata = fs::metadata(path)
        .or_else(|_| fs::symlink_metadata(path))
        .ok();
    let is_folder = metadata.as_ref().is_some_and(fs::Metadata::is_dir);
    ConflictItem {
        path: path.to_path_buf(),
        kind: if is_folder {
            ItemKind::Folder
        } else {
            ItemKind::File
        },
        size: metadata
            .as_ref()
            .filter(|_| !is_folder)
            .map(fs::Metadata::len),
        modified: metadata.and_then(|m| m.modified().ok()),
    }
}

/// 훑기가 센 전체. 훑기 스레드가 늘리고 복사 쪽이 읽는다.
#[derive(Default)]
struct Totals {
    items: AtomicU64,
    bytes: AtomicU64,
    counted: AtomicBool,
}

/// 훑기가 통로에 넣는 항목.
enum Entry {
    File {
        source: PathBuf,
        target: PathBuf,
        size: u64,
    },
    FolderStart {
        source: PathBuf,
        target: PathBuf,
    },
    FolderEnd {
        source: PathBuf,
        target: PathBuf,
    },
    /// 따라 들어가지 않는 링크.
    Link,
    /// 훑지 못한 항목.
    Failed {
        source: PathBuf,
        kind: FileOpErrorKind,
    },
}

/// 원본을 깊이 우선으로 훑어 항목을 넣는다. 넣기 전에 센 것에 더해, 복사 쪽이 한 것이 센 것을 넘지 않는다.
///
/// 항목마다 일시정지면 기다리고, 취소됐으면 다 세지 않은 채(`counted`를 세우지 않고) 멈춘다.
fn scan(
    plans: Vec<(PathBuf, Option<PathBuf>)>,
    sender: &Sender<Entry>,
    totals: &Totals,
    control: &Control,
) {
    let send = |entry: Entry| {
        match entry {
            // 폴더 끝은 항목이 아니라 표지다.
            Entry::FolderEnd { .. } => {}
            Entry::File { size, .. } => {
                totals.items.fetch_add(1, Ordering::AcqRel);
                totals.bytes.fetch_add(size, Ordering::AcqRel);
            }
            _ => {
                totals.items.fetch_add(1, Ordering::AcqRel);
            }
        }
        // 받는 쪽이 없으면 더 넣을 곳이 없다.
        let _ = sender.send(entry);
    };
    for (source, target) in plans {
        if !control.proceed() {
            return;
        }
        let Some(target) = target else {
            send(Entry::Failed {
                source,
                kind: FileOpErrorKind::Other,
            });
            continue;
        };
        match fs::symlink_metadata(&source) {
            Err(error) => send(Entry::Failed {
                source,
                kind: error_kind(&error),
            }),
            Ok(metadata) if metadata.file_type().is_symlink() => send(Entry::Link),
            Ok(metadata) if metadata.is_dir() => {
                if !walk(source, target, &send, control) {
                    return;
                }
            }
            Ok(metadata) => send(Entry::File {
                source,
                target,
                size: metadata.len(),
            }),
        }
    }
    totals.counted.store(true, Ordering::Release);
}

/// 폴더 하나를 깊이 우선으로 훑는다. 깊은 폴더에서 스택이 넘치지 않게 되부르지 않고 스택을 쓴다. 취소돼 멈췄으면
/// `false`.
fn walk(source: PathBuf, target: PathBuf, send: &dyn Fn(Entry), control: &Control) -> bool {
    let mut stack = Vec::new();
    open_folder(source, target, &mut stack, send);
    while let Some((listing, source, target)) = stack.last_mut() {
        if !control.proceed() {
            return false;
        }
        match listing.next() {
            None => {
                let (_, source, target) = stack.pop().expect("방금 본 폴더");
                send(Entry::FolderEnd { source, target });
            }
            Some(Err(error)) => {
                // 목록을 다 읽지 못했다. 그 폴더는 여기까지로 닫는다(같은 오류가 되풀이되지 않게).
                send(Entry::Failed {
                    source: source.clone(),
                    kind: error_kind(&error),
                });
                let (_, source, target) = stack.pop().expect("방금 본 폴더");
                send(Entry::FolderEnd { source, target });
            }
            Some(Ok(entry)) => {
                let child_source = entry.path();
                let child_target = target.join(entry.file_name());
                match entry.file_type() {
                    Err(error) => send(Entry::Failed {
                        source: child_source,
                        kind: error_kind(&error),
                    }),
                    Ok(kind) if is_link(&kind) => send(Entry::Link),
                    Ok(kind) if kind.is_dir() => {
                        open_folder(child_source, child_target, &mut stack, send)
                    }
                    Ok(_) => match entry.metadata() {
                        Ok(metadata) => send(Entry::File {
                            source: child_source,
                            target: child_target,
                            size: metadata.len(),
                        }),
                        Err(error) => send(Entry::Failed {
                            source: child_source,
                            kind: error_kind(&error),
                        }),
                    },
                }
            }
        }
    }
    true
}

type Listing = (fs::ReadDir, PathBuf, PathBuf);

fn open_folder(source: PathBuf, target: PathBuf, stack: &mut Vec<Listing>, send: &dyn Fn(Entry)) {
    match fs::read_dir(&source) {
        Ok(listing) => {
            send(Entry::FolderStart {
                source: source.clone(),
                target: target.clone(),
            });
            stack.push((listing, source, target));
        }
        Err(error) => send(Entry::Failed {
            source,
            kind: error_kind(&error),
        }),
    }
}

/// 심볼릭 링크·정션(Windows는 이름 대리 재분석 지점)인지. 따라 들어가지 않는다.
fn is_link(kind: &FileType) -> bool {
    kind.is_symlink()
}

fn error_kind(error: &io::Error) -> FileOpErrorKind {
    match error.kind() {
        io::ErrorKind::NotFound => FileOpErrorKind::SourceMissing,
        io::ErrorKind::PermissionDenied => FileOpErrorKind::AccessDenied,
        _ => FileOpErrorKind::Other,
    }
}

/// 훑은 항목을 받아 복사한다.
struct Copier<'a> {
    reporter: Reporter,
    summary: Summary,
    /// 0이 아니면 건너뛰는 폴더 안이다(그 폴더에서 열린 폴더 깊이).
    skip_depth: usize,
    /// 들어가 있는 폴더(바깥부터). 안의 항목의 대상 경로를 실제 대상 폴더 아래로 바꾸는 데 쓴다.
    folders: Vec<OpenFolder>,
    /// 「나머지에도 적용」으로 받은 답.
    remembered: Option<Resolution>,
    context: &'a Context<'a>,
    /// 취소로 멈췄다.
    cancelled: bool,
}

/// 들어가 있는 폴더 하나.
struct OpenFolder {
    /// 훑기가 정한 대상 경로(안의 항목의 대상 경로가 이것으로 시작한다).
    scanned: PathBuf,
    /// 실제 대상 경로(둘 다 유지로 「이름 (2)」가 됐거나 바깥 폴더가 그랬으면 다르다).
    actual: PathBuf,
    /// 이 작업이 만들었다(합친 폴더면 `false`: 시각·속성을 건드리지 않는다).
    created: bool,
}

/// 파일 하나를 한 결과.
enum FileOutcome {
    /// 복사했다(덮어썼으면 `overwritten`).
    Copied { overwritten: bool },
    /// 이름 충돌로 건너뛰었다.
    Skipped,
    /// 대상이 쓰는 사이에 생겼다(다시 확인한다).
    Exists,
    /// 취소로 그만뒀다.
    Cancelled,
    /// 실패했다.
    Failed(FileOpErrorKind),
}

impl Copier<'_> {
    fn handle(&mut self, entry: Entry) {
        if self.skip_depth > 0 {
            self.skip(entry);
            self.reporter.maybe_emit();
            return;
        }
        match entry {
            Entry::File {
                source,
                target,
                size,
            } => {
                let target = self.actual_target(&target);
                if self.copy_file(&source, &target, size) {
                    self.reporter.items_done += 1;
                }
            }
            Entry::FolderStart { source, target } => {
                self.reporter.current = Some(source.clone());
                let actual = self.actual_target(&target);
                if self.start_folder(source, target, actual) {
                    self.reporter.items_done += 1;
                }
            }
            Entry::FolderEnd { source, target } => {
                // 들어간 폴더만 끝이 온다(건너뛴 폴더의 끝은 `skip`이 먹는다).
                let folder = self.folders.pop();
                debug_assert!(folder.as_ref().is_some_and(|f| f.scanned == target));
                if let Some(folder) = folder.filter(|f| f.created)
                    && let Err(error) = file_ops::copy_folder_metadata(&source, &folder.actual)
                {
                    self.fail(source, error.kind());
                }
            }
            Entry::Link => {
                self.summary.links_skipped += 1;
                self.reporter.items_done += 1;
            }
            Entry::Failed { source, kind } => {
                self.fail(source, kind);
                self.reporter.items_done += 1;
            }
        }
        self.reporter.maybe_emit();
    }

    /// 훑기가 정한 대상 경로를 실제 대상 경로로 바꾼다(바깥 폴더가 「이름 (2)」가 됐으면 그 아래로).
    fn actual_target(&self, scanned: &Path) -> PathBuf {
        match self.folders.last() {
            Some(folder) => match scanned.strip_prefix(&folder.scanned) {
                Ok(rest) => folder.actual.join(rest),
                Err(_) => scanned.to_path_buf(),
            },
            None => scanned.to_path_buf(),
        }
    }

    /// 폴더를 만들거나(있으면 합치거나, 파일과 부딪히면 묻고) 들어간다. 취소로 그만뒀으면 `false`.
    fn start_folder(&mut self, source: PathBuf, scanned: PathBuf, target: PathBuf) -> bool {
        for _ in 0..MAX_ATTEMPTS {
            match fs::symlink_metadata(&target) {
                // 같은 이름의 폴더(링크가 아닌): 묻지 않고 합친다.
                Ok(metadata) if metadata.is_dir() => {
                    self.summary.folders_merged += 1;
                    self.enter(scanned, target, false);
                    return true;
                }
                // 같은 이름의 파일(또는 링크): 묻는다. 덮어쓰기는 고를 수 없다.
                Ok(_) => {
                    let Some(resolution) = self.resolve(&source, &target, false) else {
                        self.cancelled = true;
                        return false;
                    };
                    match resolution {
                        Resolution::KeepBoth => {
                            let renamed = free_numbered(&target, false);
                            return self.create_folder(source, scanned, renamed);
                        }
                        // 덮어쓰기 답은 `resolve`가 받지 않는다.
                        _ => {
                            self.summary.conflicts_skipped += 1;
                            self.skip_depth = 1;
                            return true;
                        }
                    }
                }
                Err(_) => match file_ops::create_folder(&target) {
                    Ok(()) => {
                        self.summary.folders_copied += 1;
                        self.enter(scanned, target, true);
                        return true;
                    }
                    // 확인한 뒤 밖에서 생겼다: 다시 본다.
                    Err(error) if error.kind() == FileOpErrorKind::TargetExists => {}
                    Err(error) => {
                        self.fail(source, error.kind());
                        self.skip_depth = 1;
                        return true;
                    }
                },
            }
        }
        self.fail(source, FileOpErrorKind::TargetExists);
        self.skip_depth = 1;
        true
    }

    /// 둘 다 유지로 정한 새 이름의 폴더를 만들고 들어간다.
    fn create_folder(&mut self, source: PathBuf, scanned: PathBuf, target: PathBuf) -> bool {
        match file_ops::create_folder(&target) {
            Ok(()) => {
                self.summary.folders_copied += 1;
                self.enter(scanned, target, true);
            }
            Err(error) => {
                self.fail(source, error.kind());
                self.skip_depth = 1;
            }
        }
        true
    }

    fn enter(&mut self, scanned: PathBuf, actual: PathBuf, created: bool) {
        self.folders.push(OpenFolder {
            scanned,
            actual,
            created,
        });
    }

    /// 이름 충돌의 답을 정한다: 「나머지에도 적용」한 답이 이 충돌에 쓸 수 있으면 그것, 아니면 묻는다. 취소되면
    /// `None`. 덮어쓸 수 없는 충돌에는 덮어쓰기 답이 나오지 않는다(대기열이 거부한다).
    fn resolve(&mut self, source: &Path, target: &Path, can_overwrite: bool) -> Option<Resolution> {
        if let Some(remembered) = self.remembered
            && (can_overwrite || !remembered.overwrites())
        {
            return Some(remembered);
        }
        let conflict = Conflict {
            source: conflict_item(source),
            target: conflict_item(target),
            can_overwrite,
            overwrite_is_permanent: can_overwrite,
        };
        let answer = self.context.ask(conflict)?;
        if answer.apply_to_rest {
            self.remembered = Some(answer.resolution);
        }
        Some(answer.resolution)
    }

    /// 건너뛰는 폴더 안의 항목: 하지 않고 한 것으로만 센다.
    fn skip(&mut self, entry: Entry) {
        match entry {
            Entry::FolderStart { .. } => {
                self.skip_depth += 1;
                self.reporter.items_done += 1;
            }
            Entry::FolderEnd { .. } => self.skip_depth -= 1,
            Entry::File { size, .. } => {
                self.reporter.items_done += 1;
                self.reporter.bytes_done += size;
            }
            Entry::Link | Entry::Failed { .. } => self.reporter.items_done += 1,
        }
    }

    /// 파일 하나를 복사한다(대상이 있으면 묻고 답대로). 취소로 그만뒀으면 `false`(그 파일은 하지 않은 것으로
    /// 남는다).
    fn copy_file(&mut self, source: &Path, target: &Path, size: u64) -> bool {
        self.reporter.current = Some(source.to_path_buf());
        let base = self.reporter.bytes_done;
        let mut outcome = FileOutcome::Exists;
        for _ in 0..MAX_ATTEMPTS {
            outcome = match fs::symlink_metadata(target) {
                Err(_) => self.write_file(source, target, size, base),
                Ok(_) => self.resolve_file(source, target, size, base),
            };
            if !matches!(outcome, FileOutcome::Exists) {
                break;
            }
            self.reporter.bytes_done = base;
        }
        match outcome {
            FileOutcome::Copied { overwritten } => {
                self.summary.files_copied += 1;
                self.summary.bytes_copied += size;
                if overwritten {
                    self.summary.files_overwritten += 1;
                }
            }
            FileOutcome::Skipped => self.summary.conflicts_skipped += 1,
            FileOutcome::Cancelled => {
                self.cancelled = true;
                self.reporter.bytes_done = base;
                return false;
            }
            FileOutcome::Exists => self.fail(source.to_path_buf(), FileOpErrorKind::TargetExists),
            FileOutcome::Failed(kind) => self.fail(source.to_path_buf(), kind),
        }
        self.reporter.bytes_done = base + size;
        true
    }

    /// 대상에 같은 이름이 있는 파일: 묻고 답대로 한다.
    fn resolve_file(&mut self, source: &Path, target: &Path, size: u64, base: u64) -> FileOutcome {
        let target_is_folder = fs::metadata(target).is_ok_and(|m| m.is_dir())
            || fs::symlink_metadata(target).is_ok_and(|m| m.is_dir());
        let Some(resolution) = self.resolve(source, target, !target_is_folder) else {
            return FileOutcome::Cancelled;
        };
        match resolution {
            Resolution::Skip => FileOutcome::Skipped,
            Resolution::Overwrite => self.overwrite(source, target, size, base),
            Resolution::OverwriteIfNewer => {
                let modified = |path: &Path| fs::metadata(path).and_then(|m| m.modified()).ok();
                match (modified(source), modified(target)) {
                    (Some(source_time), Some(target_time)) if source_time > target_time => {
                        self.overwrite(source, target, size, base)
                    }
                    _ => FileOutcome::Skipped,
                }
            }
            Resolution::KeepBoth => {
                let renamed = free_numbered(target, true);
                match self.write_file(source, &renamed, size, base) {
                    // 고른 번호를 밖에서 먼저 썼으면 실패로 둔다(같은 질문을 되풀이하지 않게).
                    FileOutcome::Exists => FileOutcome::Failed(FileOpErrorKind::TargetExists),
                    outcome => outcome,
                }
            }
        }
    }

    /// 덮어쓰기: 대상 폴더 안 임시 이름에 다 쓴 뒤 바꿔치기한다. 복사가 실패하거나 취소되면 `copy_file`이 임시
    /// 파일을 지우고 원래 파일은 그대로다. 바꿔치기가 실패하면 임시 파일을 지운다.
    fn overwrite(&mut self, source: &Path, target: &Path, size: u64, base: u64) -> FileOutcome {
        let temp = temp_path(target);
        match self.write_file(source, &temp, size, base) {
            FileOutcome::Copied { .. } => match file_ops::replace(&temp, target) {
                Ok(()) => FileOutcome::Copied { overwritten: true },
                Err(error) => {
                    let _ = file_ops::remove_file(&temp);
                    FileOutcome::Failed(error.kind())
                }
            },
            FileOutcome::Exists => FileOutcome::Failed(FileOpErrorKind::TargetExists),
            outcome => outcome,
        }
    }

    /// 파일 내용을 `target`(없는 이름)에 쓴다.
    fn write_file(&mut self, source: &Path, target: &Path, size: u64, base: u64) -> FileOutcome {
        let reporter = &mut self.reporter;
        let control = self.context.control;
        let result = file_ops::copy_file(source, target, &mut |progress| {
            // 훑은 뒤 파일이 커졌거나 대체 데이터 스트림이 있어도 센 크기를 넘지 않게 한다.
            reporter.bytes_done = base + progress.copied.min(size);
            reporter.maybe_emit();
            // 일시정지면 여기서(덩어리 사이) 기다린다.
            if control.proceed() {
                CopyStep::Continue
            } else {
                CopyStep::Cancel
            }
        });
        match result {
            Ok(()) => FileOutcome::Copied { overwritten: false },
            Err(error) => match error.kind() {
                FileOpErrorKind::TargetExists => FileOutcome::Exists,
                FileOpErrorKind::Cancelled => FileOutcome::Cancelled,
                kind => FileOutcome::Failed(kind),
            },
        }
    }

    fn fail(&mut self, source: PathBuf, kind: FileOpErrorKind) {
        self.summary.failures.push(ItemFailure { source, kind });
    }
}

/// 진행을 세고 [`PROGRESS_INTERVAL`]마다 한 번까지 진행 사건을 보낸다.
struct Reporter {
    id: JobId,
    outbox: Outbox,
    totals: Arc<Totals>,
    start: Instant,
    last: Option<Instant>,
    items_done: u64,
    bytes_done: u64,
    current: Option<PathBuf>,
}

impl Reporter {
    fn new(id: JobId, outbox: Outbox, totals: Arc<Totals>) -> Self {
        Reporter {
            id,
            outbox,
            totals,
            start: Instant::now(),
            last: None,
            items_done: 0,
            bytes_done: 0,
            current: None,
        }
    }

    /// 앞 사건에서 간격이 지났으면 보낸다. 첫 사건은 바이트를 처음 옮긴 때(빠르면 시작하자마자 복사가 보이게)나
    /// 시작에서 간격이 지난 때 보낸다. 폴더만 만든 뒤 곧바로 보내면 다음 사건까지 간격을 기다려야 한다.
    fn maybe_emit(&mut self) {
        let now = Instant::now();
        let due = match self.last {
            Some(last) => now.duration_since(last) >= PROGRESS_INTERVAL,
            None => self.bytes_done > 0 || now.duration_since(self.start) >= PROGRESS_INTERVAL,
        };
        if due {
            self.emit(now);
        }
    }

    /// 마지막 진행: 간격을 지키도록 기다렸다가 보낸다.
    fn finish(&mut self) {
        let now = loop {
            let now = Instant::now();
            match self.last {
                Some(last) if now.duration_since(last) < PROGRESS_INTERVAL => {
                    thread::sleep(PROGRESS_INTERVAL - now.duration_since(last));
                }
                _ => break now,
            }
        };
        self.emit(now);
    }

    fn emit(&mut self, now: Instant) {
        let counted = self.totals.counted.load(Ordering::Acquire);
        let items_total = self.totals.items.load(Ordering::Acquire);
        let bytes_total = self.totals.bytes.load(Ordering::Acquire);
        let elapsed = now.duration_since(self.start);
        let seconds = elapsed.as_secs_f64();
        let bytes_per_second = if seconds > 0.0 {
            (self.bytes_done as f64 / seconds) as u64
        } else {
            0
        };
        let remaining = if !counted {
            Remaining::Calculating
        } else if self.bytes_done >= bytes_total {
            Remaining::Known(Duration::ZERO)
        } else if bytes_per_second == 0 {
            Remaining::Calculating
        } else {
            Remaining::Known(Duration::from_secs_f64(
                (bytes_total - self.bytes_done) as f64 / bytes_per_second as f64,
            ))
        };
        self.last = Some(now);
        self.outbox.send(JobEvent::Progress {
            job: self.id,
            progress: Progress {
                items_done: self.items_done,
                items_total,
                bytes_done: self.bytes_done,
                bytes_total,
                counted,
                bytes_per_second,
                remaining,
                current: self.current.clone(),
                elapsed,
            },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str, is_file: bool, n: u32) -> String {
        copy_name(OsStr::new(text), is_file, n)
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn 복사본_이름은_확장자를_뒤에_둔다() {
        assert_eq!(name("보고서.txt", true, 1), "보고서 - 복사본.txt");
        assert_eq!(name("보고서.txt", true, 2), "보고서 - 복사본 (2).txt");
        assert_eq!(name("a.tar.gz", true, 3), "a.tar - 복사본 (3).gz");
        assert_eq!(name("Makefile", true, 1), "Makefile - 복사본");
        assert_eq!(name(".gitignore", true, 1), ".gitignore - 복사본");
    }

    #[test]
    fn 폴더는_점이_있어도_이름_전체_뒤에_붙인다() {
        assert_eq!(name("사진.2024", false, 1), "사진.2024 - 복사본");
        assert_eq!(name("사진.2024", false, 2), "사진.2024 - 복사본 (2)");
    }
}
