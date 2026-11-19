//! 꺼내기 작업: zip 항목을 실제 폴더로 푼다(#425).
//!
//! 꺼내기는 복사 작업 길이다. 원본이 zip 항목 경로(zip 파일 경로 + zip 안 경로, [`zip_item`])면
//! [`super::JobQueue::copy`]가 이 작업을 만들고, zip 전체는 [`super::JobQueue::extract_all`]이 만든다. 이름 충돌·항목별
//! 오류 질문·일시정지·취소·덮어쓰기(옛 파일은 휴지통으로)·작업 기록은 복사 엔진([`Copier`])을 그대로 쓰고, 파일
//! 내용만 zip 리더([`Archive::stream_entry`])로 풀어 쓴다(`Copier`의 `zip` 원본).
//!
//! 훑기는 zip의 central directory(이미 메모리에 있는 폴더 나무)만 보므로 따로 스레드를 띄우지 않고 시작할 때 한
//! 번에 센다. 폴더 기록이 없는 중간 폴더도 폴더로 만든다.
//!
//! 건너뛰는 항목(묻지 않음, [`Summary::zip_skipped`]): 풀 수 없는 압축 방식(Stored·Deflate가 아님), 심볼릭 링크,
//! 대상 폴더 밖을 가리키는 이름(`..`·절대 경로·드라이브 글자·UNC·`:` 스트림), 쓸 수 없는 이름(S4 `validate_name`). 하나라도 있으면 작업은 실패로 끝난다. CRC 불일치·선언 크기
//! 초과·깨진 압축 데이터는 쓰다 만 파일을 지우고 항목별 오류([`FileOpErrorKind::Corrupt`])로 묻는다.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::SystemTime;

use facet_win::file_ops::{self, FileOpErrorKind};

use super::copy::{Copier, Entry, FileOutcome, Replaced, Reporter, count};
use super::{
    ConflictItem, Context, Control, CopyRejection, ItemKind, JournalSink, Summary, ZipSkipReason,
    ZipSkipped,
};
use crate::name_input::validate_name;
use crate::zip_archive::{Archive, ArchiveEntry, EntryError, StreamError};

/// 압축 방식: 그대로 저장.
const METHOD_STORED: u16 = 0;
/// 압축 방식: Deflate.
const METHOD_DEFLATE: u16 = 8;

/// 경로가 zip 항목 경로면 (zip 파일, zip 안 경로(`\` 구분)). zip 파일 자신이나 zip과 상관없는 경로면 `None`.
pub(crate) fn zip_item(path: &Path) -> Option<(PathBuf, String)> {
    crate::zip_provider::split_archive(path).filter(|(_, inner)| !inner.is_empty())
}

/// 원본 가운데 zip 항목 경로가 있는가.
pub(super) fn involves_zip(sources: &[PathBuf]) -> bool {
    sources.iter().any(|source| zip_item(source).is_some())
}

/// 넣기 전에 확인을 마친 꺼내기 작업.
pub(super) struct ExtractJob {
    /// zip 파일.
    archive: PathBuf,
    /// 꺼낼 zip 안 경로(`/` 구분). 빈 글자는 zip 전체(루트 아래 항목을 대상 폴더 안에 바로 둔다).
    roots: Vec<String>,
    target: PathBuf,
    /// 한 일을 적을 곳(작업 기록에 남길 작업만).
    pub(super) journal: Option<JournalSink>,
}

impl ExtractJob {
    /// zip 항목들을 `target` 폴더 안으로 꺼내는 작업. 원본이 모두 같은 zip의 항목이어야 한다(아니면
    /// [`CopyRejection::Unsupported`]). zip은 아직 열지 않는다(작업 스레드에서 연다).
    pub(super) fn prepare(sources: Vec<PathBuf>, target: PathBuf) -> Result<Self, CopyRejection> {
        if sources.is_empty() {
            return Err(CopyRejection::NoSources);
        }
        check_target(&target)?;
        let mut archive: Option<PathBuf> = None;
        let mut roots = Vec::with_capacity(sources.len());
        for source in &sources {
            let (zip, inner) = zip_item(source).ok_or(CopyRejection::Unsupported)?;
            match &archive {
                Some(first) if !super::same_path(first, &zip) => {
                    return Err(CopyRejection::Unsupported);
                }
                Some(_) => {}
                None => archive = Some(zip),
            }
            roots.push(inner.replace('\\', "/"));
        }
        let archive = archive.ok_or(CopyRejection::NoSources)?;
        check_space(&archive, &roots, &target)?;
        Ok(ExtractJob {
            archive,
            roots,
            target,
            journal: None,
        })
    }

    /// zip 파일 `archive` 전체를 `target` 폴더 안으로 꺼내는 작업.
    pub(super) fn prepare_all(archive: PathBuf, target: PathBuf) -> Result<Self, CopyRejection> {
        if !archive.is_file() {
            return Err(CopyRejection::NoSources);
        }
        check_target(&target)?;
        let roots = vec![String::new()];
        check_space(&archive, &roots, &target)?;
        Ok(ExtractJob {
            archive,
            roots,
            target,
            journal: None,
        })
    }

    /// 넘긴 원본 수.
    pub(super) fn root_count(&self) -> u64 {
        self.roots.len() as u64
    }

    /// 작업 스레드에서 끝까지(또는 취소될 때까지) 실행한다.
    pub(super) fn run(self, context: &Context<'_>) -> (Summary, bool) {
        let mut copier = Copier::new(context, false);
        copier.journal = self.journal;
        let totals = Arc::clone(&copier.reporter.totals);

        // zip을 연다. 열지 못하면 zip 파일을 항목별 오류로 묻는다(다시 시도면 다시 연다).
        let archive = loop {
            match crate::zip_provider::open_archive(&self.archive) {
                Ok(archive) => break archive,
                Err(error) => {
                    let kind = match error {
                        crate::zip_archive::ArchiveError::Io(error) => io_kind(&error),
                        _ => FileOpErrorKind::Corrupt,
                    };
                    totals
                        .items
                        .store(self.roots.len() as u64, Ordering::Release);
                    match copier.on_error(&self.archive, kind) {
                        super::copy::ErrorStep::Retry => {}
                        super::copy::ErrorStep::Skip => {
                            copier.reporter.items_done = self.roots.len() as u64;
                            totals.counted.store(true, Ordering::Release);
                            return copier.finish();
                        }
                        super::copy::ErrorStep::Cancel => return copier.finish(),
                    }
                }
            }
        };
        totals.items.store(0, Ordering::Release);

        let source = Arc::new(ZipSource {
            archive,
            root: self.archive.clone(),
        });
        let items = plan(&source, &self.roots, &self.target);
        for item in &items {
            match item {
                Item::Entry(entry) => count(&totals, entry),
                Item::Skip(_, size) => {
                    totals.items.fetch_add(1, Ordering::AcqRel);
                    totals.bytes.fetch_add(*size, Ordering::AcqRel);
                }
            }
        }
        totals.counted.store(true, Ordering::Release);
        copier.zip = Some(source);

        let control = context.control;
        for item in items {
            if !control.proceed() {
                copier.cancelled = true;
            }
            if copier.cancelled {
                break;
            }
            match item {
                Item::Entry(entry) => copier.handle(entry),
                Item::Skip(skipped, size) => {
                    // 건너뛰기로 답한 폴더 안이면 그 폴더와 함께 건너뛴 것이다(따로 알리지 않는다).
                    if !copier.skipping() {
                        copier.summary.zip_skipped.push(skipped);
                    }
                    copier.reporter.items_done += 1;
                    copier.reporter.bytes_done += size;
                    copier.reporter.maybe_emit(control);
                }
            }
        }
        copier.finish()
    }
}

/// 대상 폴더를 확인한다(복사와 같다).
fn check_target(target: &Path) -> Result<(), CopyRejection> {
    let metadata = fs::metadata(target).map_err(|_| CopyRejection::TargetMissing)?;
    if !metadata.is_dir() {
        return Err(CopyRejection::TargetNotFolder);
    }
    Ok(())
}

/// 선언된 압축 해제 크기의 합이 대상 볼륨의 빈 공간보다 크면 막는다(#426). zip을 열지 못하거나 빈 공간을 알 수
/// 없으면 막지 않고 작업 스레드가 다룬다.
fn check_space(archive: &Path, roots: &[String], target: &Path) -> Result<(), CopyRejection> {
    let Ok(opened) = crate::zip_provider::open_archive(archive) else {
        return Ok(());
    };
    let source = ZipSource {
        archive: opened,
        root: archive.to_path_buf(),
    };
    let needed: u64 = plan(&source, roots, target)
        .iter()
        .map(|item| match item {
            Item::Entry(Entry::File { size, .. }) => *size,
            _ => 0,
        })
        .fold(0, u64::saturating_add);
    if needed == 0 {
        return Ok(());
    }
    if let Some(space) = facet_win::devices::disk_space(target)
        && needed > space.free
    {
        return Err(CopyRejection::NotEnoughSpace {
            needed,
            available: space.free,
        });
    }
    Ok(())
}

/// 꺼낼 항목 하나: 복사 엔진이 다룰 항목이거나, 묻지 않고 건너뛸 항목(과 그 크기).
enum Item {
    Entry(Entry),
    Skip(ZipSkipped, u64),
}

/// 원본들을 깊이 우선으로 훑어 꺼낼 차례를 만든다(폴더 시작 → 안의 항목 → 폴더 끝). 깊은 zip에서 스택이 넘치지
/// 않게 되부르지 않는다.
fn plan(source: &ZipSource, roots: &[String], target: &Path) -> Vec<Item> {
    let mut items = Vec::new();
    for root in roots {
        if root.is_empty() {
            plan_all(source, target, &mut items);
            continue;
        }
        match source.archive.find(root) {
            Some(entry) => walk(source, vec![entry], target, &mut items),
            None => items.push(Item::Entry(Entry::Failed {
                source: source.path_of(root),
                target: None,
                kind: FileOpErrorKind::SourceMissing,
            })),
        }
    }
    items
}

/// `entries`(같은 폴더의 항목들)를 `target` 폴더 안으로 꺼낼 차례에 더한다.
fn walk(source: &ZipSource, entries: Vec<ArchiveEntry>, target: &Path, items: &mut Vec<Item>) {
    // (남은 항목들, 그 폴더의 원본 경로·대상 경로(폴더 끝을 넣을 것, 바깥은 `None`))
    type Frame = (std::vec::IntoIter<ArchiveEntry>, Option<(PathBuf, PathBuf)>);
    let mut stack: Vec<Frame> = vec![(entries.into_iter(), None)];
    let mut targets: Vec<PathBuf> = vec![target.to_path_buf()];
    while let Some((rest, _)) = stack.last_mut() {
        let Some(entry) = rest.next() else {
            let (_, folder) = stack.pop().expect("방금 본 폴더");
            targets.pop();
            if let Some((source, target)) = folder {
                items.push(Item::Entry(Entry::FolderEnd { source, target }));
            }
            continue;
        };
        let size = if entry.is_folder { 0 } else { entry.size };
        let skip = |reason| {
            Item::Skip(
                ZipSkipped {
                    entry: entry.path.clone(),
                    reason,
                },
                size,
            )
        };
        if !is_safe_name(&entry.name) {
            items.push(skip(ZipSkipReason::OutsideTarget));
            continue;
        }
        if entry.symlink {
            items.push(skip(ZipSkipReason::Symlink));
            continue;
        }
        if let Err(error) = validate_name(&entry.name) {
            items.push(skip(ZipSkipReason::InvalidName(error)));
            continue;
        }
        let parent = targets.last().expect("대상 폴더").clone();
        let item_target = parent.join(&entry.name);
        let item_source = source.path_of(&entry.path);
        if entry.is_folder {
            items.push(Item::Entry(Entry::FolderStart {
                source: item_source.clone(),
                target: item_target.clone(),
            }));
            let children = source.archive.children(&entry.path).unwrap_or_default();
            targets.push(item_target.clone());
            stack.push((children.into_iter(), Some((item_source, item_target))));
        } else if entry.method != METHOD_STORED && entry.method != METHOD_DEFLATE {
            items.push(skip(ZipSkipReason::UnsupportedMethod(entry.method)));
        } else {
            items.push(Item::Entry(Entry::File {
                source: item_source,
                target: item_target,
                size,
            }));
        }
    }
}

/// 꺼낼 기록 하나: 원본 항목과 대상 폴더 안의 경로(`..`을 정리한 뒤).
struct Node {
    entry: ArchiveEntry,
    /// 대상 폴더 안 경로(`/` 구분).
    resolved: String,
}

impl Node {
    /// 폴더 기록이 없는 중간 폴더.
    fn folder(path: &str, name: &str) -> Node {
        Node {
            entry: ArchiveEntry {
                raw_name: path.to_string(),
                path: path.to_string(),
                name: name.to_string(),
                is_folder: true,
                size: 0,
                compressed_size: 0,
                modified: None,
                method: 0,
                encrypted: false,
                symlink: false,
            },
            resolved: path.to_string(),
        }
    }
}

/// 기록된 이름을 대상 폴더 안의 경로 조각들로 정리한다: `.`·빈 조각은 빼고 `..`은 앞 조각을 지운다(`a/../b`는
/// `b`). 앞 `/`·`\`, 드라이브 글자·대체 데이터 스트림(`:`), 대상 밖으로 나가는 `..`은 [`ZipSkipReason::OutsideTarget`],
/// 심볼릭 링크는 [`ZipSkipReason::Symlink`], 쓸 수 없는 이름은 [`ZipSkipReason::InvalidName`]이다. 정리하고 남는
/// 조각이 없으면(`.`·`/`뿐) 빈 목록이다.
fn resolve(entry: &ArchiveEntry) -> Result<Vec<String>, ZipSkipReason> {
    let raw = entry.raw_name.as_str();
    if raw.starts_with(['/', '\\']) {
        return Err(ZipSkipReason::OutsideTarget);
    }
    let mut parts: Vec<String> = Vec::new();
    for part in raw.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(ZipSkipReason::OutsideTarget);
                }
            }
            _ if part.contains(':') => return Err(ZipSkipReason::OutsideTarget),
            _ => parts.push(part.to_string()),
        }
    }
    if entry.symlink {
        return Err(ZipSkipReason::Symlink);
    }
    for part in &parts {
        validate_name(part).map_err(ZipSkipReason::InvalidName)?;
    }
    if !entry.is_folder && entry.method != METHOD_STORED && entry.method != METHOD_DEFLATE {
        return Err(ZipSkipReason::UnsupportedMethod(entry.method));
    }
    Ok(parts)
}

/// zip 전체를 기록된 이름대로 훑어 꺼낼 차례를 만든다. 이름은 [`resolve`]로 정리해 대상 폴더 안에 놓고, 거부할
/// 기록은 기록된 이름 그대로 건너뛸 항목에 담는다.
fn plan_all(source: &ZipSource, target: &Path, items: &mut Vec<Item>) {
    use std::collections::{BTreeMap, HashMap};
    // 폴더 경로("" 루트) → 바로 아래 이름 → 기록(같은 경로는 뒤 기록).
    let mut folders: HashMap<String, BTreeMap<String, Node>> = HashMap::new();
    folders.insert(String::new(), BTreeMap::new());
    for entry in source.archive.entries() {
        let parts = match resolve(entry) {
            Ok(parts) => parts,
            Err(reason) => {
                let size = if entry.is_folder { 0 } else { entry.size };
                items.push(Item::Skip(
                    ZipSkipped {
                        entry: entry.raw_name.clone(),
                        reason,
                    },
                    size,
                ));
                continue;
            }
        };
        if parts.is_empty() {
            continue;
        }
        // 위 폴더들이 나무에 있게 한다.
        let mut folder = String::new();
        for part in &parts[..parts.len() - 1] {
            let path = if folder.is_empty() {
                part.clone()
            } else {
                format!("{folder}/{part}")
            };
            let children = folders.entry(folder.clone()).or_default();
            if !children.get(part).is_some_and(|node| node.entry.is_folder) {
                children.insert(part.clone(), Node::folder(&path, part));
            }
            folders.entry(path.clone()).or_default();
            folder = path;
        }
        let name = parts[parts.len() - 1].clone();
        let resolved = parts.join("/");
        let mut node = Node {
            entry: entry.clone(),
            resolved: resolved.clone(),
        };
        node.entry.name = name.clone();
        if entry.is_folder {
            folders.entry(resolved).or_default();
        }
        folders.entry(folder).or_default().insert(name, node);
    }

    type Frame = (std::vec::IntoIter<Node>, Option<(PathBuf, PathBuf)>);
    let take = |folders: &mut HashMap<String, BTreeMap<String, Node>>, path: &str| {
        folders
            .remove(path)
            .map(|children| children.into_values().collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
    };
    let mut stack: Vec<Frame> = vec![(take(&mut folders, ""), None)];
    let mut targets: Vec<PathBuf> = vec![target.to_path_buf()];
    while let Some((rest, _)) = stack.last_mut() {
        let Some(node) = rest.next() else {
            let (_, folder) = stack.pop().expect("방금 본 폴더");
            targets.pop();
            if let Some((source, target)) = folder {
                items.push(Item::Entry(Entry::FolderEnd { source, target }));
            }
            continue;
        };
        let entry = &node.entry;
        let parent = targets.last().expect("대상 폴더").clone();
        let item_target = parent.join(&entry.name);
        let item_source = source.path_of(&entry.path);
        if entry.is_folder {
            items.push(Item::Entry(Entry::FolderStart {
                source: item_source.clone(),
                target: item_target.clone(),
            }));
            targets.push(item_target.clone());
            stack.push((
                take(&mut folders, &node.resolved),
                Some((item_source, item_target)),
            ));
        } else {
            items.push(Item::Entry(Entry::File {
                source: item_source,
                target: item_target,
                size: entry.size,
            }));
        }
    }
}

/// 대상 폴더 안의 이름 하나로 쓸 수 있는가: `..`이나 드라이브·스트림 구분자(`:`)가 든 이름은 대상 폴더 밖이나
/// 다른 곳을 가리킬 수 있어 쓰지 않는다(경로 구분자는 리더가 이미 나눴다).
fn is_safe_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(':')
}

/// 꺼내기의 원본: 연 zip과 그 파일 경로. 복사 엔진([`Copier`])이 원본 경로(zip 항목 경로)를 zip 안 항목으로 바꿔
/// 쓴다.
pub(super) struct ZipSource {
    archive: Arc<Archive>,
    root: PathBuf,
}

impl ZipSource {
    /// zip 안 경로(`/` 구분)의 원본 경로(zip 항목 경로).
    fn path_of(&self, inner: &str) -> PathBuf {
        let mut path = self.root.clone();
        for part in inner.split('/').filter(|part| !part.is_empty()) {
            path.push(part);
        }
        path
    }

    /// 원본 경로의 zip 안 경로(`/` 구분).
    fn inner_of(&self, source: &Path) -> Option<String> {
        let rest = source.strip_prefix(&self.root).ok()?;
        let parts: Vec<String> = rest
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect();
        Some(parts.join("/"))
    }

    fn entry_of(&self, source: &Path) -> Option<ArchiveEntry> {
        self.archive.find(&self.inner_of(source)?)
    }

    /// 원본 항목의 수정 시각(zip 항목의 시각).
    pub(super) fn modified(&self, source: &Path) -> Option<SystemTime> {
        self.entry_of(source)?.modified
    }

    /// 이름 충돌의 원본 쪽 항목 정보.
    pub(super) fn conflict_item(&self, source: &Path) -> ConflictItem {
        let entry = self.entry_of(source);
        let is_folder = entry.as_ref().is_some_and(|e| e.is_folder);
        ConflictItem {
            path: source.to_path_buf(),
            kind: if is_folder {
                ItemKind::Folder
            } else {
                ItemKind::File
            },
            size: entry.as_ref().filter(|_| !is_folder).map(|e| e.size),
            modified: entry.and_then(|e| e.modified),
        }
    }

    /// zip 항목 `source`를 `target`(없는 이름)에 푼다. 진행은 `reporter`에 알리고 덩어리마다 일시정지면 기다린다.
    /// 다 풀면 수정 시각을 zip 항목의 시각으로 맞춘다. 실패하거나(CRC·크기가 맞지 않음 포함) 취소하면 쓰다 만 파일을
    /// 지운다.
    pub(super) fn write(
        &self,
        source: &Path,
        target: &Path,
        size: u64,
        base: u64,
        reporter: &mut Reporter,
        control: &Control,
    ) -> FileOutcome {
        let Some(inner) = self.inner_of(source) else {
            return FileOutcome::Failed(FileOpErrorKind::SourceMissing);
        };
        let index = match self.archive.file_index(&inner) {
            Ok(index) => index,
            Err(error) => return FileOutcome::Failed(entry_kind(&error)),
        };
        let mut file = match OpenOptions::new().write(true).create_new(true).open(target) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return FileOutcome::Exists;
            }
            Err(error) => return FileOutcome::Failed(io_kind(&error)),
        };
        let mut written = 0u64;
        let result = self.archive.stream_entry(index, None, &mut |chunk| {
            file.write_all(chunk)?;
            written += chunk.len() as u64;
            reporter.bytes_done = base + written.min(size);
            reporter.maybe_emit(control);
            Ok(control.proceed())
        });
        let result = result.map_err(|error| match error {
            StreamError::Stopped => FileOutcome::Cancelled,
            StreamError::Write(error) => FileOutcome::Failed(io_kind(&error)),
            StreamError::Entry(error) => FileOutcome::Failed(entry_kind(&error)),
        });
        let result = result.and_then(|()| {
            if let Some(modified) = self.archive.entry(index).modified {
                file.set_modified(modified)
                    .map_err(|error| FileOutcome::Failed(io_kind(&error)))?;
            }
            Ok(())
        });
        drop(file);
        match result {
            Ok(()) => FileOutcome::Copied {
                replaced: Replaced::Nothing,
            },
            Err(outcome) => {
                let _ = file_ops::remove_file(target);
                outcome
            }
        }
    }
}

/// zip 항목을 풀지 못한 까닭을 파일 작업 오류 종류로.
fn entry_kind(error: &EntryError) -> FileOpErrorKind {
    match error {
        EntryError::NotFound | EntryError::NotAFile => FileOpErrorKind::SourceMissing,
        // 암호 걸린 항목은 #428 전까지 풀 수 없다.
        EntryError::PasswordNeeded | EntryError::WrongPassword => FileOpErrorKind::AccessDenied,
        EntryError::UnsupportedMethod(_) => FileOpErrorKind::Unsupported,
        EntryError::Corrupt => FileOpErrorKind::Corrupt,
        EntryError::Io(kind) => io_kind(&io::Error::from(*kind)),
    }
}

/// 입출력 오류를 파일 작업 오류 종류로.
fn io_kind(error: &io::Error) -> FileOpErrorKind {
    match error.kind() {
        io::ErrorKind::NotFound => FileOpErrorKind::SourceMissing,
        io::ErrorKind::PermissionDenied => FileOpErrorKind::AccessDenied,
        io::ErrorKind::StorageFull => FileOpErrorKind::DiskFull,
        io::ErrorKind::AlreadyExists => FileOpErrorKind::TargetExists,
        _ => FileOpErrorKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_that_leave_the_target_are_not_safe() {
        assert!(is_safe_name("보고서.txt"));
        assert!(!is_safe_name(".."));
        assert!(!is_safe_name("C:"));
        assert!(!is_safe_name("a:stream"));
        assert!(!is_safe_name(""));
    }
}
