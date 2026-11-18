//! 복사 작업: 훑기와 복사를 나란히 한다.
//!
//! 작업이 시작되면 훑기 스레드가 원본을 깊이 우선으로 훑어 항목을 하나씩 통로에 넣으며 전체 항목 수와 바이트를
//! 센다. 작업 일꾼 스레드는 통로에서 이미 센 항목부터 꺼내 복사한다. 그래서 큰 폴더도 다 세기를 기다리지 않고
//! 곧바로 복사가 시작되고, 다 셀 때까지 남은 시간은 「계산 중」이다.
//!
//! 훑기는 폴더마다 「폴더 시작」을 넣고 그 안의 항목을 넣은 뒤 「폴더 끝」을 넣는다. 복사는 폴더 시작에서 폴더를
//! 만들고, 폴더 끝에서(안의 항목을 다 쓴 뒤) 폴더 시각·속성을 원본과 맞춘다.
//!
//! 심볼릭 링크(파일·폴더)와 정션은 따라 들어가지 않고 링크 자체를 대상에 만든다(같은 종류, 같은 곳을 가리킴).
//! 그래서 자기 상위를 가리키는 링크가 있어도 복사가 끝난다. 링크는 항목 하나(바이트 0)로 센다.
//!
//! 이름 충돌을 덮어쓰기로 풀면 대상 폴더 안 임시 이름에 다 쓴 뒤, 바꿔치기 직전에 원래 대상을 휴지통으로 보내고
//! 임시 파일을 제 이름으로 놓는다(`Copier::place`). 휴지통이 없는 위치와 휴지통에 들어가지 않는 파일은 영구
//! 덮어쓰기다.
//!
//! 통로는 크기 제한이 없다: 훑기가 복사보다 앞서 나가야 전체를 셀 수 있기 때문이다. 항목 하나가 경로 둘만큼의
//! 메모리를 쓴다.

use std::cell::RefCell;
use std::ffi::{OsStr, OsString};
use std::fs::{self, FileType};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use facet_win::file_ops::{self, CopyStep, FileOpErrorKind, Link};
use facet_win::recycle::{self, RecycleOutcome, RecycledItem};

use super::{
    Conflict, ConflictItem, Context, Control, CopyRejection, ErrorAnswer, ItemFailure, ItemKind,
    JobEvent, JobId, JournalSink, Outbox, PROGRESS_INTERVAL, Progress, Remaining, Resolution,
    Summary, is_within, same_path,
};
use crate::history::{Stamp, Step};

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
    pub(super) roots: Vec<Root>,
    pub(super) target: PathBuf,
    /// 한 일을 적을 곳(작업 기록에 남길 작업만).
    pub(super) journal: Option<JournalSink>,
}

/// 넘긴 원본 하나.
pub(super) struct Root {
    pub(super) source: PathBuf,
    /// 원본이 대상 폴더 바로 안에 있다(「- 복사본」 이름을 만든다).
    pub(super) same_folder: bool,
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
        Ok(CopyJob {
            roots,
            target,
            journal: None,
        })
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
        let plans = self.plan();
        let mut copier = Copier::new(context, false);
        copier.journal = self.journal;
        // 훑기는 취소됐을 때만 다 세지 않고 멈춘다. 복사 쪽이 취소를 보기 전에 훑기가 먼저 멈췄어도(예: 항목을
        // 하나도 넣기 전에 일시정지한 채 취소) 다 세지 않았으므로 `finish`가 취소로 끝낸다.
        if copier.run_pipeline(plans) {
            copier
                .reporter
                .totals
                .counted
                .store(true, Ordering::Release);
        }
        copier.finish()
    }

    /// 원본마다 대상 경로를 정한다. 같은 폴더 복사면 지금 있는 이름과 이 작업에서 이미 정한 이름을 피해
    /// 「- 복사본」 이름을 만든다.
    fn plan(&self) -> Vec<Plan> {
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
pub(super) fn free_numbered(target: &Path, is_file: bool) -> PathBuf {
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
pub(super) fn temp_path(target: &Path) -> PathBuf {
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
pub(super) fn conflict_item(path: &Path) -> ConflictItem {
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

/// 원본 하나와 그 대상 경로(정하지 못했으면 `None`: 항목별 오류로 묻는다).
pub(super) type Plan = (PathBuf, Option<PathBuf>);

/// 훑기가 센 전체. 훑기 스레드가 늘리고 복사 쪽이 읽는다. 이동 작업은 이름 바꾸기로 옮길 항목도 여기에 센다.
#[derive(Default)]
pub(super) struct Totals {
    pub(super) items: AtomicU64,
    pub(super) bytes: AtomicU64,
    pub(super) counted: AtomicBool,
}

/// 훑기가 통로에 넣는 항목.
pub(super) enum Entry {
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
    /// 따라 들어가지 않고 링크 자체를 만들 링크(파일·폴더 심볼릭 링크, 정션).
    Link {
        source: PathBuf,
        target: PathBuf,
    },
    /// 훑지 못한 항목. 다시 시도하면 `target`이 있을 때 그 항목을 다시 훑는다(없으면 다시 훑을 수 없어 다시 묻는다).
    Failed {
        source: PathBuf,
        target: Option<PathBuf>,
        kind: FileOpErrorKind,
    },
}

/// 항목을 센 것에 더한다. 넣기 전에 세어, 복사 쪽이 한 것이 센 것을 넘지 않는다.
pub(super) fn count(totals: &Totals, entry: &Entry) {
    match entry {
        // 폴더 끝은 항목이 아니라 표지다.
        Entry::FolderEnd { .. } => {}
        Entry::File { size, .. } => {
            totals.items.fetch_add(1, Ordering::AcqRel);
            totals.bytes.fetch_add(*size, Ordering::AcqRel);
        }
        _ => {
            totals.items.fetch_add(1, Ordering::AcqRel);
        }
    }
}

/// 원본을 깊이 우선으로 훑어 항목을 센 뒤 넣는다.
///
/// 항목마다 일시정지면 기다리고, 취소됐으면 다 세지 않은 채 멈춘다. 다 훑었으면 `true`.
fn scan(plans: Vec<Plan>, sender: &Sender<Entry>, totals: &Totals, control: &Control) -> bool {
    let send = |entry: Entry| {
        count(totals, &entry);
        // 받는 쪽이 없으면 더 넣을 곳이 없다.
        let _ = sender.send(entry);
    };
    for (source, target) in plans {
        if !control.proceed() {
            return false;
        }
        if !scan_one(source, target, &send, control) {
            return false;
        }
    }
    true
}

/// 원본들을 훑기 스레드가 훑는 동안 이미 센 항목부터 `handle`에 넘긴다(복사와 영구 삭제가 함께 쓴다). `handle`이
/// `false`를 돌려주면(취소) 더 받지 않고 통로를 버려 훑기도 곧 멈춘다. 다 훑었으면 `true`.
pub(super) fn pipeline(
    plans: Vec<Plan>,
    totals: &Totals,
    control: &Control,
    mut handle: impl FnMut(Entry) -> bool,
) -> bool {
    let (sender, receiver) = mpsc::channel();
    thread::scope(|scope| {
        let scanner = thread::Builder::new()
            .name("facet-job-scan".into())
            .spawn_scoped(scope, move || scan(plans, &sender, totals, control))
            .expect("훑기 스레드를 띄우지 못했다");
        // 훑기가 끝나 통로를 닫을 때까지 받는다.
        for entry in receiver {
            if !handle(entry) {
                break;
            }
        }
        scanner.join().unwrap_or(false)
    })
}

/// 원본 항목 하나(폴더면 그 아래 전부)를 훑어 넣는다. 취소돼 멈췄으면 `false`.
pub(super) fn scan_one(
    source: PathBuf,
    target: Option<PathBuf>,
    send: &dyn Fn(Entry),
    control: &Control,
) -> bool {
    let Some(target) = target else {
        send(Entry::Failed {
            source,
            target: None,
            kind: FileOpErrorKind::Other,
        });
        return true;
    };
    match fs::symlink_metadata(&source) {
        Err(error) => send(Entry::Failed {
            source,
            target: Some(target),
            kind: error_kind(&error),
        }),
        Ok(metadata) if metadata.file_type().is_symlink() => send(Entry::Link { source, target }),
        Ok(metadata) if metadata.is_dir() => return walk(source, target, send, control),
        Ok(metadata) => send(Entry::File {
            source,
            target,
            size: metadata.len(),
        }),
    }
    true
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
                // 목록을 다 읽지 못했다. 그 폴더는 여기까지로 닫는다(같은 오류가 되풀이되지 않게). 이미 넣은 안의
                // 항목과 겹치므로 그 폴더를 다시 훑지 않는다.
                send(Entry::Failed {
                    source: source.clone(),
                    target: None,
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
                        target: Some(child_target),
                        kind: error_kind(&error),
                    }),
                    Ok(kind) if is_link(&kind) => send(Entry::Link {
                        source: child_source,
                        target: child_target,
                    }),
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
                            target: Some(child_target),
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
            target: Some(target),
            kind: error_kind(&error),
        }),
    }
}

/// 심볼릭 링크·정션(Windows는 이름 대리 재분석 지점)인지. 따라 들어가지 않고 링크 자체를 복사한다.
fn is_link(kind: &FileType) -> bool {
    kind.is_symlink()
}

pub(super) fn error_kind(error: &io::Error) -> FileOpErrorKind {
    match error.kind() {
        io::ErrorKind::NotFound => FileOpErrorKind::SourceMissing,
        io::ErrorKind::PermissionDenied => FileOpErrorKind::AccessDenied,
        _ => FileOpErrorKind::Other,
    }
}

/// 훑은 항목을 받아 복사한다. 이동 작업의 복사(`moving`)면 다 복사한 파일의 원본을 지우고, 안의 항목을 다 옮긴
/// 원본 폴더를 빈 폴더일 때만 지운다.
pub(super) struct Copier<'a> {
    pub(super) reporter: Reporter,
    pub(super) summary: Summary,
    /// 0이 아니면 건너뛰는 폴더 안이다(그 폴더에서 열린 폴더 깊이).
    skip_depth: usize,
    /// 들어가 있는 폴더(바깥부터). 안의 항목의 대상 경로를 실제 대상 폴더 아래로 바꾸는 데 쓴다.
    folders: Vec<OpenFolder>,
    /// 「나머지에도 적용」으로 받은 답.
    remembered: Option<Resolution>,
    /// 다음 이름 충돌 하나에 묻지 않고 쓸 답(이동이 이미 물어 받은 답을 복사로 바꿔 이어 갈 때).
    pub(super) preset: Option<Resolution>,
    /// 「나머지 오류도 건너뛰기」로 답했다: 남은 항목별 오류는 묻지 않고 건너뛴다.
    skip_errors: bool,
    pub(super) context: &'a Context<'a>,
    /// 취소로 멈췄다.
    pub(super) cancelled: bool,
    /// 이동 작업의 복사(볼륨을 넘는 이동): 복사를 마친 원본을 지운다. 실행취소가 볼륨을 넘어 되옮길 때 잠시 켠다.
    pub(super) moving: bool,
    /// 한 일을 적을 곳(작업 기록에 남길 작업만). 실제로 만들거나 옮긴 항목만 적는다.
    pub(super) journal: Option<JournalSink>,
    /// 이름 충돌에 둘 다 유지로 답해 방금 쓴 실제 대상 경로(「이름 (2)」). 기록에 실제 이름을 남기는 데 쓴다.
    kept_as: Option<PathBuf>,
    /// 휴지통에 넣을 수 없을 때 영구 덮어쓰기를 해도 되는가. 복사·이동 작업은 `true`(S3와 같은 영구 덮어쓰기),
    /// 다시 실행은 `false`(영구히 지우지 않고 `refused`를 세운다).
    pub(super) permanent_overwrite: bool,
    /// 영구 덮어쓰기를 해도 되지 않아(`permanent_overwrite`가 `false`) 덮어쓰지 않은 항목이 있었다.
    pub(super) refused: bool,
    /// 마지막 덮어쓰기가 휴지통으로 보낸 원래 대상과 보내기 전 모습(`place`가 적는다).
    pub(super) last_replaced: Option<(RecycledItem, Stamp)>,
}

/// 덮어쓰기에서 원래 대상을 어떻게 치웠는가.
#[derive(Debug, Clone)]
pub(super) enum Replaced {
    /// 덮어쓰지 않았다(대상이 없던 이름에 썼다).
    Nothing,
    /// 원래 대상을 휴지통으로 보냈다: 휴지통 항목과 보내기 전 모습. 실행취소로 되살릴 수 있다.
    Recycled(RecycledItem, Stamp),
    /// 휴지통을 거치지 않고 덮어썼다(휴지통이 없는 위치, 휴지통에 들어가지 않는 파일). 되살릴 수 없다.
    Permanently,
}

impl Replaced {
    pub(super) fn overwrote(&self) -> bool {
        !matches!(self, Replaced::Nothing)
    }
}

/// 들어가 있는 폴더 하나.
struct OpenFolder {
    /// 훑기가 정한 대상 경로(안의 항목의 대상 경로가 이것으로 시작한다).
    scanned: PathBuf,
    /// 실제 대상 경로(둘 다 유지로 「이름 (2)」가 됐거나 바깥 폴더가 그랬으면 다르다).
    actual: PathBuf,
    /// 이 작업이 만들었다(합친 폴더면 `false`: 시각·속성을 건드리지 않는다).
    created: bool,
    /// 이동이면 들어가기 전에 읽어 둔 원본 폴더의 메타데이터(안의 항목을 지우기 전의 시각).
    source_metadata: Option<fs::Metadata>,
}

/// 파일 하나를 한 결과.
pub(super) enum FileOutcome {
    /// 복사했다(덮어썼으면 원래 대상을 어떻게 치웠는지 `replaced`).
    Copied { replaced: Replaced },
    /// 이름 충돌로 건너뛰었다.
    Skipped,
    /// 휴지통에 넣을 수 없어 덮어쓰지 않았다(영구 덮어쓰기를 하지 않는 다시 실행만). 대상은 그대로다.
    Refused,
    /// 대상이 쓰는 사이에 생겼다(다시 확인한다).
    Exists,
    /// 취소로 그만뒀다.
    Cancelled,
    /// 실패했다.
    Failed(FileOpErrorKind),
}

/// 폴더 하나를 시작한 결과.
enum FolderOutcome {
    /// 들어갔거나(만들었거나 합쳤음) 이름 충돌로 건너뛰었다.
    Done,
    /// 취소로 그만뒀다.
    Cancelled,
    /// 실패했다.
    Failed(FileOpErrorKind),
}

/// 항목별 오류에 대한 답을 따른 다음 걸음.
pub(super) enum ErrorStep {
    /// 그 항목을 처음부터 다시 한다.
    Retry,
    /// 그 항목을 건너뛴다(이미 [`Summary::failures`]에 담았다).
    Skip,
    /// 작업을 취소한다(이미 `cancelled`를 세웠다).
    Cancel,
}

impl<'a> Copier<'a> {
    pub(super) fn new(context: &'a Context<'a>, moving: bool) -> Self {
        Copier {
            reporter: Reporter::new(context.id, context.outbox.clone(), Arc::default()),
            summary: Summary::default(),
            skip_depth: 0,
            folders: Vec::new(),
            remembered: None,
            preset: None,
            skip_errors: false,
            context,
            cancelled: false,
            moving,
            journal: None,
            kept_as: None,
            permanent_overwrite: true,
            refused: false,
            last_replaced: None,
        }
    }

    /// 파일·링크 하나를 `source`에서 `target`으로 복사했다(이동이면 옮겼다). 작업 기록에 적는다. 원래 대상을
    /// 휴지통으로 보내고 덮어썼으면 [`Step::Overwrote`], 영구 덮어썼으면 그 작업을 되돌릴 수 없다고 적는다.
    pub(super) fn note_item(&self, source: &Path, target: &Path, replaced: Replaced) {
        let Some(journal) = &self.journal else {
            return;
        };
        let Some(stamp) = Stamp::read(target) else {
            return;
        };
        let permanent = matches!(replaced, Replaced::Permanently);
        let step = if let Replaced::Recycled(item, old) = replaced {
            Step::Overwrote {
                source: source.to_path_buf(),
                path: target.to_path_buf(),
                moved: self.moving,
                stamp,
                replaced: item,
                replaced_stamp: old,
            }
        } else if self.moving {
            Step::Moved {
                from: source.to_path_buf(),
                to: target.to_path_buf(),
                stamp,
            }
        } else {
            Step::Copied {
                source: source.to_path_buf(),
                path: target.to_path_buf(),
                stamp,
            }
        };
        let mut journal = super::lock(journal);
        journal.overwrote |= permanent;
        journal.steps.push(step);
    }

    /// 폴더 `target`에 들어갔다(`created`면 만들었고, 아니면 같은 이름의 폴더와 합쳤다). 작업 기록에 적는다. 복사가
    /// 합친 폴더는 만든 것이 아니므로 적지 않는다(실행취소가 지우지 않게).
    pub(super) fn note_folder(&self, source: &Path, target: &Path, created: bool) {
        let Some(journal) = &self.journal else {
            return;
        };
        if !self.moving && !created {
            return;
        }
        let Some(stamp) = Stamp::read(target) else {
            return;
        };
        let step = if self.moving {
            Step::MovedFolder {
                from: source.to_path_buf(),
                to: target.to_path_buf(),
                created,
                stamp,
            }
        } else {
            Step::Copied {
                source: source.to_path_buf(),
                path: target.to_path_buf(),
                stamp,
            }
        };
        super::lock(journal).steps.push(step);
    }

    /// 원본들을 훑기 스레드가 훑는 동안 이미 센 항목부터 복사한다. 다 훑었으면 `true`(취소로 훑기가 멈췄으면
    /// `false`이고 `cancelled`를 세운다). 다 셌는지(`counted`)는 부르는 쪽이 정한다.
    pub(super) fn run_pipeline(&mut self, plans: Vec<Plan>) -> bool {
        let control = self.context.control;
        let totals = Arc::clone(&self.reporter.totals);
        let scanned = pipeline(plans, &totals, control, |entry| {
            if !control.proceed() {
                self.cancelled = true;
            }
            if self.cancelled {
                return false;
            }
            self.handle(entry);
            true
        });
        if !scanned {
            self.cancelled = true;
        }
        scanned
    }

    /// 결과 요약과 취소로 멈췄는지를 낸다. 다 세지 않았으면 취소로 끝난 것이다. 취소로 멈추지 않았으면 마지막
    /// 진행 사건을 보낸다(취소면 보내지 않는다: 끝남 사건의 결과가 마지막 수치다).
    pub(super) fn finish(mut self) -> (Summary, bool) {
        let totals = Arc::clone(&self.reporter.totals);
        let counted = totals.counted.load(Ordering::Acquire);
        if !counted {
            self.cancelled = true;
        }
        if !self.cancelled {
            self.reporter.current = None;
            self.reporter.finish();
        }
        let mut summary = self.summary;
        summary.items_done = self.reporter.items_done;
        summary.all_counted = counted;
        summary.items_remaining = totals
            .items
            .load(Ordering::Acquire)
            .saturating_sub(summary.items_done);
        (summary, self.cancelled)
    }

    fn handle(&mut self, entry: Entry) {
        if self.skip_depth > 0 {
            self.skip(entry);
            self.reporter.maybe_emit(self.context.control);
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
                // 이동이면 안의 항목을 옮기며 지우느라 원본 폴더의 수정 시각이 바뀌므로, 들어가기 전에 읽어 둔다.
                let snapshot = if self.moving {
                    fs::metadata(&source).ok()
                } else {
                    None
                };
                let depth = self.folders.len();
                if self.start_folder(source, target, actual) {
                    self.reporter.items_done += 1;
                }
                if self.folders.len() > depth
                    && let Some(folder) = self.folders.last_mut()
                {
                    folder.source_metadata = snapshot;
                }
            }
            Entry::FolderEnd { source, target } => {
                // 들어간 폴더만 끝이 온다(건너뛴 폴더의 끝은 `skip`이 먹는다).
                let folder = self.folders.pop();
                debug_assert!(folder.as_ref().is_some_and(|f| f.scanned == target));
                if let Some(folder) = folder.as_ref().filter(|f| f.created) {
                    self.finish_folder(&source, &folder.actual, folder.source_metadata.as_ref());
                }
                // 안의 항목을 다 옮긴 원본 폴더는 비었다. 건너뛰거나 실패한 항목이 남았으면 비지 않아 지워지지
                // 않는다(빈 폴더만 지우므로 남은 항목을 잃지 않는다).
                if self.moving && folder.is_some() && !self.cancelled {
                    let _ = file_ops::remove_empty_folder(&source);
                }
            }
            Entry::Link { source, target } => {
                let target = self.actual_target(&target);
                if self.copy_link(&source, &target) {
                    self.reporter.items_done += 1;
                }
            }
            Entry::Failed {
                source,
                target,
                kind,
            } => self.scan_failed(source, target, kind),
        }
        self.reporter.maybe_emit(self.context.control);
    }

    /// 안의 항목을 다 쓴 폴더의 시각·속성을 원본과 맞춘다(들어가기 전에 읽어 둔 `snapshot`이 있으면 그 값으로).
    /// 실패하면 묻는다.
    pub(super) fn finish_folder(
        &mut self,
        source: &Path,
        target: &Path,
        snapshot: Option<&fs::Metadata>,
    ) {
        let apply = |target: &Path| match snapshot {
            Some(metadata) => file_ops::apply_folder_metadata(metadata, target),
            None => file_ops::copy_folder_metadata(source, target),
        };
        while let Err(error) = apply(target) {
            match self.on_error(source, error.kind()) {
                ErrorStep::Retry => {}
                ErrorStep::Skip | ErrorStep::Cancel => return,
            }
        }
    }

    /// 훑지 못한 항목: 묻는다. 다시 시도면 그 항목을 지금 다시 훑어 그 결과를 차례로 다룬다(다시 실패하면 다시
    /// 묻는다).
    fn scan_failed(&mut self, source: PathBuf, target: Option<PathBuf>, kind: FileOpErrorKind) {
        loop {
            match self.on_error(&source, kind) {
                ErrorStep::Skip => {
                    self.reporter.items_done += 1;
                    return;
                }
                ErrorStep::Cancel => return,
                ErrorStep::Retry => {
                    if let Some(target) = target {
                        self.rescan(source, target);
                        return;
                    }
                }
            }
        }
    }

    /// 훑지 못했던 항목 하나를 다시 훑어 다룬다. 앞서 그 항목을 하나로 셌으므로 빼고 다시 센다.
    fn rescan(&mut self, source: PathBuf, target: PathBuf) {
        let totals = Arc::clone(&self.reporter.totals);
        totals.items.fetch_sub(1, Ordering::AcqRel);
        let control = self.context.control;
        let entries = RefCell::new(Vec::new());
        let send = |entry: Entry| {
            count(&totals, &entry);
            entries.borrow_mut().push(entry);
        };
        let finished = scan_one(source, Some(target), &send, control);
        for entry in entries.into_inner() {
            if !control.proceed() {
                self.cancelled = true;
            }
            if self.cancelled {
                return;
            }
            self.handle(entry);
        }
        if !finished {
            self.cancelled = true;
        }
    }

    /// 항목별 오류를 묻고(「나머지 오류도 건너뛰기」 뒤면 묻지 않고 건너뛴다) 답을 따른다. 건너뛰면
    /// [`Summary::failures`]에 담고, 취소면 `cancelled`를 세운다.
    pub(super) fn on_error(&mut self, source: &Path, kind: FileOpErrorKind) -> ErrorStep {
        let failure = ItemFailure {
            source: source.to_path_buf(),
            kind,
        };
        let answer = if self.skip_errors {
            ErrorAnswer::Skip
        } else {
            match self.context.ask_error(failure.clone()) {
                Some(answer) => answer,
                None => {
                    self.cancelled = true;
                    return ErrorStep::Cancel;
                }
            }
        };
        match answer {
            ErrorAnswer::Retry => ErrorStep::Retry,
            ErrorAnswer::Skip | ErrorAnswer::SkipAll => {
                if answer == ErrorAnswer::SkipAll {
                    self.skip_errors = true;
                }
                self.summary.failures.push(failure);
                ErrorStep::Skip
            }
            ErrorAnswer::CancelJob => {
                self.cancelled = true;
                ErrorStep::Cancel
            }
        }
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

    /// 폴더를 만들거나(있으면 합치거나, 파일과 부딪히면 묻고) 들어간다. 실패하면 항목별 오류로 묻고, 건너뛰면 그
    /// 아래 전부를 건너뛴다. 취소로 그만뒀으면 `false`.
    fn start_folder(&mut self, source: PathBuf, scanned: PathBuf, target: PathBuf) -> bool {
        loop {
            let kind = match self.try_start_folder(&source, &scanned, &target) {
                FolderOutcome::Done => return true,
                FolderOutcome::Cancelled => {
                    self.cancelled = true;
                    return false;
                }
                FolderOutcome::Failed(kind) => kind,
            };
            match self.on_error(&source, kind) {
                ErrorStep::Retry => {}
                ErrorStep::Skip => {
                    self.skip_depth = 1;
                    return true;
                }
                ErrorStep::Cancel => return false,
            }
        }
    }

    fn try_start_folder(&mut self, source: &Path, scanned: &Path, target: &Path) -> FolderOutcome {
        for _ in 0..MAX_ATTEMPTS {
            match fs::symlink_metadata(target) {
                // 같은 이름의 폴더(링크가 아닌): 묻지 않고 합친다.
                Ok(metadata) if metadata.is_dir() => {
                    self.summary.folders_merged += 1;
                    self.enter(scanned, target, false);
                    self.note_folder(source, target, false);
                    return FolderOutcome::Done;
                }
                // 같은 이름의 파일(또는 링크): 묻는다. 덮어쓰기는 고를 수 없다.
                Ok(_) => {
                    let Some(resolution) = self.resolve(source, target, false) else {
                        return FolderOutcome::Cancelled;
                    };
                    match resolution {
                        Resolution::KeepBoth => {
                            let renamed = free_numbered(target, false);
                            return self.create_folder(source, scanned, &renamed);
                        }
                        // 덮어쓰기 답은 `resolve`가 받지 않는다.
                        _ => {
                            self.summary.conflicts_skipped += 1;
                            self.skip_depth = 1;
                            return FolderOutcome::Done;
                        }
                    }
                }
                Err(_) => match self.create_folder(source, scanned, target) {
                    // 확인한 뒤 밖에서 생겼다: 다시 본다.
                    FolderOutcome::Failed(FileOpErrorKind::TargetExists) => {}
                    outcome => return outcome,
                },
            }
        }
        FolderOutcome::Failed(FileOpErrorKind::TargetExists)
    }

    /// 폴더를 만들고 들어간다.
    fn create_folder(&mut self, source: &Path, scanned: &Path, target: &Path) -> FolderOutcome {
        match file_ops::create_folder(target) {
            Ok(()) => {
                self.summary.folders_copied += 1;
                self.enter(scanned, target, true);
                self.note_folder(source, target, true);
                FolderOutcome::Done
            }
            Err(error) => FolderOutcome::Failed(error.kind()),
        }
    }

    fn enter(&mut self, scanned: &Path, actual: &Path, created: bool) {
        self.folders.push(OpenFolder {
            scanned: scanned.to_path_buf(),
            actual: actual.to_path_buf(),
            created,
            source_metadata: None,
        });
    }

    /// 이름 충돌의 답을 정한다: 「나머지에도 적용」한 답이 이 충돌에 쓸 수 있으면 그것, 아니면 묻는다. 취소되면
    /// `None`. 덮어쓸 수 없는 충돌에는 덮어쓰기 답이 나오지 않는다(대기열이 거부한다).
    pub(super) fn resolve(
        &mut self,
        source: &Path,
        target: &Path,
        can_overwrite: bool,
    ) -> Option<Resolution> {
        let conflict = Conflict {
            source: conflict_item(source),
            target: conflict_item(target),
            can_overwrite,
            // 휴지통이 있는 위치면 원래 대상을 휴지통으로 보낸 뒤 덮어쓰므로 되살릴 수 있다.
            overwrite_is_permanent: can_overwrite && !recycle::has_recycle_bin(target),
        };
        self.answer_conflict(conflict)
    }

    /// 이름 충돌 `conflict`의 답: 미리 받은 답·「나머지에도 적용」한 답이 이 충돌에 쓸 수 있으면
    /// ([`Conflict::allows`]) 그것, 아니면 묻는다. 취소되면 `None`.
    pub(super) fn answer_conflict(&mut self, conflict: Conflict) -> Option<Resolution> {
        if let Some(preset) = self.preset.take()
            && conflict.allows(preset)
        {
            return Some(preset);
        }
        if let Some(remembered) = self.remembered
            && conflict.allows(remembered)
        {
            return Some(remembered);
        }
        let answer = self.context.ask_conflict(conflict)?;
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
            Entry::Link { .. } | Entry::Failed { .. } => self.reporter.items_done += 1,
        }
    }

    /// 파일 하나를 복사한다(대상이 있으면 묻고 답대로). 실패하면(만들다 만 파일은 `copy_file`이 이미 지웠다)
    /// 항목별 오류로 묻고, 다시 시도면 그 파일을 처음부터 다시 한다. 취소로 그만뒀으면 `false`(그 파일은 하지 않은
    /// 것으로 남는다).
    ///
    /// 이동 작업의 복사면 복사를 마친 뒤 원본을 지운다(지우지 못하면 항목별 오류로 묻는다).
    pub(super) fn copy_file(&mut self, source: &Path, target: &Path, size: u64) -> bool {
        self.reporter.current = Some(source.to_path_buf());
        self.kept_as = None;
        self.last_replaced = None;
        let base = self.reporter.bytes_done;
        loop {
            let outcome = self.attempt_file(source, target, size, base);
            self.reporter.bytes_done = base;
            let kind = match outcome {
                FileOutcome::Copied { replaced } => {
                    self.summary.files_copied += 1;
                    self.summary.bytes_copied += size;
                    if replaced.overwrote() {
                        self.summary.files_overwritten += 1;
                    }
                    let written = self.kept_as.take();
                    self.note_item(source, written.as_deref().unwrap_or(target), replaced);
                    if self.moving && !self.remove_source(source, file_ops::remove_file) {
                        return false;
                    }
                    break;
                }
                FileOutcome::Skipped => {
                    self.summary.conflicts_skipped += 1;
                    break;
                }
                FileOutcome::Refused => {
                    self.refused = true;
                    break;
                }
                FileOutcome::Cancelled => {
                    self.cancelled = true;
                    return false;
                }
                FileOutcome::Exists => FileOpErrorKind::TargetExists,
                FileOutcome::Failed(kind) => kind,
            };
            match self.on_error(source, kind) {
                ErrorStep::Retry => {}
                ErrorStep::Skip => break,
                ErrorStep::Cancel => return false,
            }
        }
        self.reporter.bytes_done = base + size;
        true
    }

    /// 다 복사한 원본 파일을 지운다. 실패하면 묻고(다시 시도면 지우기만 다시 한다), 건너뛰면 원본과 복사본이 둘 다
    /// 남는다. 취소로 그만뒀으면 `false`.
    fn remove_source(
        &mut self,
        source: &Path,
        remove: fn(&Path) -> file_ops::FileOpResult<()>,
    ) -> bool {
        loop {
            let Err(error) = remove(source) else {
                return true;
            };
            match self.on_error(source, error.kind()) {
                ErrorStep::Retry => {}
                ErrorStep::Skip => return true,
                ErrorStep::Cancel => return false,
            }
        }
    }

    /// 링크 하나를 링크 자체로 복사한다(대상이 있으면 묻고 답대로). 링크를 읽지 못하거나 만들지 못하면(심볼릭
    /// 링크를 만들 권한이 없으면 접근 거부) 항목별 오류로 묻는다. 취소로 그만뒀으면 `false`.
    ///
    /// 이동 작업의 복사면 링크를 만든 뒤 원본 링크를 지운다(가리키는 곳은 건드리지 않는다).
    pub(super) fn copy_link(&mut self, source: &Path, target: &Path) -> bool {
        self.reporter.current = Some(source.to_path_buf());
        self.kept_as = None;
        self.last_replaced = None;
        loop {
            let kind = match self.attempt_link(source, target) {
                FileOutcome::Copied { replaced } => {
                    self.summary.links_copied += 1;
                    if replaced.overwrote() {
                        self.summary.files_overwritten += 1;
                    }
                    let written = self.kept_as.take();
                    self.note_item(source, written.as_deref().unwrap_or(target), replaced);
                    return !self.moving || self.remove_source(source, file_ops::remove_link);
                }
                FileOutcome::Skipped => {
                    self.summary.conflicts_skipped += 1;
                    return true;
                }
                FileOutcome::Refused => {
                    self.refused = true;
                    return true;
                }
                FileOutcome::Cancelled => {
                    self.cancelled = true;
                    return false;
                }
                FileOutcome::Exists => FileOpErrorKind::TargetExists,
                FileOutcome::Failed(kind) => kind,
            };
            match self.on_error(source, kind) {
                ErrorStep::Retry => {}
                ErrorStep::Skip => return true,
                ErrorStep::Cancel => return false,
            }
        }
    }

    /// 링크 하나를 한 번 해 본다. 대상이 만드는 사이에 밖에서 생기면 [`MAX_ATTEMPTS`]까지 다시 확인한다.
    fn attempt_link(&mut self, source: &Path, target: &Path) -> FileOutcome {
        let link = match file_ops::read_link(source) {
            Ok(link) => link,
            Err(error) => return FileOutcome::Failed(error.kind()),
        };
        let mut outcome = FileOutcome::Exists;
        for _ in 0..MAX_ATTEMPTS {
            outcome = match fs::symlink_metadata(target) {
                Err(_) => write_link(&link, target),
                Ok(_) => self.resolve_link(source, &link, target),
            };
            if !matches!(outcome, FileOutcome::Exists) {
                break;
            }
        }
        outcome
    }

    /// 대상에 같은 이름이 있는 링크: 묻고 답대로 한다. 폴더 링크·정션은 폴더처럼 덮어쓸 수 없고, 파일 링크는
    /// 대상이 폴더가 아니면 덮어쓸 수 있다(임시 이름에 링크를 만든 뒤 바꿔치기).
    fn resolve_link(&mut self, source: &Path, link: &Link, target: &Path) -> FileOutcome {
        let target_is_folder = fs::metadata(target).is_ok_and(|m| m.is_dir())
            || fs::symlink_metadata(target).is_ok_and(|m| m.is_dir());
        let can_overwrite = !link.kind.is_folder() && !target_is_folder;
        let Some(resolution) = self.resolve(source, target, can_overwrite) else {
            return FileOutcome::Cancelled;
        };
        match resolution {
            Resolution::Skip => FileOutcome::Skipped,
            Resolution::Overwrite => self.overwrite_link(link, target),
            Resolution::OverwriteIfNewer => {
                let modified = |path: &Path| fs::metadata(path).and_then(|m| m.modified()).ok();
                match (modified(source), modified(target)) {
                    (Some(source_time), Some(target_time)) if source_time > target_time => {
                        self.overwrite_link(link, target)
                    }
                    _ => FileOutcome::Skipped,
                }
            }
            Resolution::KeepBoth => {
                let renamed = free_numbered(target, !link.kind.is_folder());
                match write_link(link, &renamed) {
                    FileOutcome::Exists => FileOutcome::Failed(FileOpErrorKind::TargetExists),
                    outcome @ FileOutcome::Copied { .. } => {
                        self.kept_as = Some(renamed);
                        outcome
                    }
                    outcome => outcome,
                }
            }
        }
    }

    /// 파일 하나를 한 번 해 본다. 대상이 쓰는 사이에 밖에서 생기면 [`MAX_ATTEMPTS`]까지 다시 확인한다.
    fn attempt_file(&mut self, source: &Path, target: &Path, size: u64, base: u64) -> FileOutcome {
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
        outcome
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
                    outcome @ FileOutcome::Copied { .. } => {
                        self.kept_as = Some(renamed);
                        outcome
                    }
                    outcome => outcome,
                }
            }
        }
    }

    /// 덮어쓰기: 대상 폴더 안 임시 이름에 다 쓴 뒤 제 이름으로 놓는다(`place`: 원래 대상은 먼저 휴지통으로). 복사가
    /// 실패하거나 취소되면 `copy_file`이 임시 파일을 지우고 원래 파일은 그대로다. 놓지 못하면 임시 파일을 지운다.
    fn overwrite(&mut self, source: &Path, target: &Path, size: u64, base: u64) -> FileOutcome {
        let temp = temp_path(target);
        match self.write_file(source, &temp, size, base) {
            FileOutcome::Copied { .. } => {
                let placed = self.place(&temp, target);
                if !matches!(placed, FileOutcome::Copied { .. }) {
                    let _ = file_ops::remove_file(&temp);
                }
                placed
            }
            FileOutcome::Exists => FileOutcome::Failed(FileOpErrorKind::TargetExists),
            outcome => outcome,
        }
    }

    /// 파일 링크로 덮어쓰기: 대상 폴더 안 임시 이름에 링크를 만든 뒤 제 이름으로 놓는다(`place`). 놓지 못하면 임시
    /// 링크를 지우고 원래 파일은 그대로다.
    fn overwrite_link(&mut self, link: &Link, target: &Path) -> FileOutcome {
        let temp = temp_path(target);
        match write_link(link, &temp) {
            FileOutcome::Copied { .. } => {
                let placed = self.place(&temp, target);
                if !matches!(placed, FileOutcome::Copied { .. }) {
                    let _ = file_ops::remove_link(&temp);
                }
                placed
            }
            FileOutcome::Exists => FileOutcome::Failed(FileOpErrorKind::TargetExists),
            outcome => outcome,
        }
    }

    /// 같은 볼륨 이동의 덮어쓰기: 원본을 대상 폴더 안 임시 이름으로 옮긴 뒤 제 이름으로 놓는다(`place`). 놓지
    /// 못하면 원본을 제자리로 되옮긴다(못 되옮기면 임시 이름으로라도 남는다, 지우지 않는다). 다른 볼륨이면 아무것도
    /// 하지 않고 `Failed(CrossVolume)`이다. 결과 요약의 수와 작업 기록은 부르는 쪽이 적는다.
    pub(super) fn move_over(&mut self, source: &Path, target: &Path) -> FileOutcome {
        self.last_replaced = None;
        let temp = temp_path(target);
        if let Err(error) = file_ops::rename(source, &temp) {
            return FileOutcome::Failed(error.kind());
        }
        let placed = self.place(&temp, target);
        if !matches!(placed, FileOutcome::Copied { .. }) {
            let _ = file_ops::rename(&temp, source);
        }
        placed
    }

    /// 덮어쓰기의 마지막 걸음: 다 쓴 `temp`(대상 폴더 안 임시 이름)를 `target` 자리에 놓는다. 놓지 못하면 `temp`는
    /// 그대로 두고(부르는 쪽이 지우거나 되옮긴다) 원래 대상은 제자리에 있다(아래의 드문 경우는 휴지통에).
    ///
    /// - 휴지통이 있는 위치([`recycle::has_recycle_bin`])면 원래 대상을 **먼저 휴지통으로 보내고**(그 휴지통 항목은
    ///   [`Summary::recycled`]와 `last_replaced`에) `temp`의 이름을 `target`으로 바꾼다. 휴지통 보내기가 실패하면
    ///   (잠김·접근 거부·시간 초과 등) 아무것도 바꾸지 않고 실패로 돌려줘 항목별 오류로 묻는다.
    /// - 휴지통 보내기와 이름 바꾸기 사이에 실패하면(그 사이 밖에서 같은 이름이 생김 등) 원래 대상을 휴지통에서
    ///   제자리로 복원하려 한다. 복원도 실패하면(그 자리를 다른 항목이 차지함, 휴지통을 비움) 원래 대상은 **휴지통에
    ///   남는다**(영구히 지우지 않으므로 사용자가 휴지통에서 되살릴 수 있다). 어느 쪽이든 `temp`는 부르는 쪽이
    ///   지우거나(복사: 원본이 그대로라 잃는 것이 없다) 원본 자리로 되옮긴다(이동). 그래서 남는 상태는 「원래 대상이
    ///   제자리 또는 휴지통에, 원본이 제자리에」이고 새 파일이 반쯤 놓인 상태는 없다.
    /// - 휴지통이 없는 위치이거나 셸이 휴지통에 넣지 않으려 하면(휴지통보다 큰 파일, 휴지통을 끈 볼륨) S3처럼
    ///   바꿔치기(`file_ops::replace`, 읽기 전용 대상도)로 영구 덮어쓴다([`Replaced::Permanently`]).
    ///   `permanent_overwrite`가 꺼져 있으면(다시 실행) 덮어쓰지 않고 [`FileOutcome::Refused`]다.
    pub(super) fn place(&mut self, temp: &Path, target: &Path) -> FileOutcome {
        if recycle::has_recycle_bin(target) {
            let Some(old) = Stamp::read(target) else {
                // 그 사이 원래 대상이 사라졌다: 덮어쓸 것 없이 놓는다.
                return match file_ops::rename(temp, target) {
                    Ok(()) => FileOutcome::Copied {
                        replaced: Replaced::Nothing,
                    },
                    Err(error) => FileOutcome::Failed(error.kind()),
                };
            };
            match crate::shell_provider::recycle(&[target.to_path_buf()]).pop() {
                Some(RecycleOutcome::Recycled(item)) => {
                    return match file_ops::rename(temp, target) {
                        Ok(()) => {
                            self.summary.recycled.push(item.clone());
                            self.last_replaced = Some((item.clone(), old));
                            FileOutcome::Copied {
                                replaced: Replaced::Recycled(item, old),
                            }
                        }
                        Err(error) => {
                            // 원래 대상을 제자리로 되살린다. 못 하면 휴지통에 남는다(잃지 않는다).
                            let _ = recycle::restore(&item);
                            FileOutcome::Failed(error.kind())
                        }
                    };
                }
                // 휴지통에 넣을 수 없다(원래 대상은 그대로): 아래의 영구 덮어쓰기로.
                Some(RecycleOutcome::NotRecyclable) => {}
                Some(RecycleOutcome::Failed(error)) => return FileOutcome::Failed(error.kind()),
                Some(RecycleOutcome::NotAttempted) | None => {
                    return FileOutcome::Failed(FileOpErrorKind::Other);
                }
            }
        }
        if !self.permanent_overwrite {
            return FileOutcome::Refused;
        }
        match file_ops::replace(temp, target) {
            Ok(()) => FileOutcome::Copied {
                replaced: Replaced::Permanently,
            },
            Err(error) => FileOutcome::Failed(error.kind()),
        }
    }

    /// 파일 내용을 `target`(없는 이름)에 쓴다.
    fn write_file(&mut self, source: &Path, target: &Path, size: u64, base: u64) -> FileOutcome {
        let reporter = &mut self.reporter;
        let control = self.context.control;
        let result = file_ops::copy_file(source, target, &mut |progress| {
            // 훑은 뒤 파일이 커졌거나 대체 데이터 스트림이 있어도 센 크기를 넘지 않게 한다.
            reporter.bytes_done = base + progress.copied.min(size);
            reporter.maybe_emit(control);
            // 일시정지면 여기서(덩어리 사이) 기다린다.
            if control.proceed() {
                CopyStep::Continue
            } else {
                CopyStep::Cancel
            }
        });
        match result {
            Ok(()) => FileOutcome::Copied {
                replaced: Replaced::Nothing,
            },
            Err(error) => match error.kind() {
                FileOpErrorKind::TargetExists => FileOutcome::Exists,
                FileOpErrorKind::Cancelled => FileOutcome::Cancelled,
                kind => FileOutcome::Failed(kind),
            },
        }
    }
}

/// 링크를 `target`(없는 이름)에 만든다.
fn write_link(link: &Link, target: &Path) -> FileOutcome {
    match file_ops::create_link(target, link) {
        Ok(()) => FileOutcome::Copied {
            replaced: Replaced::Nothing,
        },
        Err(error) if error.kind() == FileOpErrorKind::TargetExists => FileOutcome::Exists,
        Err(error) => FileOutcome::Failed(error.kind()),
    }
}

/// 진행을 세고 [`PROGRESS_INTERVAL`]마다 한 번까지 진행 사건을 보낸다.
pub(super) struct Reporter {
    id: JobId,
    outbox: Outbox,
    pub(super) totals: Arc<Totals>,
    start: Instant,
    last: Option<Instant>,
    pub(super) items_done: u64,
    pub(super) bytes_done: u64,
    pub(super) current: Option<PathBuf>,
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
    ///
    /// 일시정지 중이면 보내지 않는다(`control`의 표지 잠금 안에서 보아, 일시정지 사건 뒤에 진행 사건이 오지 않는다).
    /// 이어 가면 다음 부름에서 보낸다.
    pub(super) fn maybe_emit(&mut self, control: &Control) {
        let now = Instant::now();
        let due = match self.last {
            Some(last) => now.duration_since(last) >= PROGRESS_INTERVAL,
            None => self.bytes_done > 0 || now.duration_since(self.start) >= PROGRESS_INTERVAL,
        };
        if due {
            control.unless_paused(|| self.emit(now));
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
