//! 내용 검색: 시작점 아래 파일을 직접 읽어 본문에서 글자·정규식을 찾는 엔진(용어는 GLOSSARY의 **내용 검색**,
//! 인덱스를 쓰지 않는다). 탭·화면과는 아직 잇지 않는다.
//!
//! 훑기는 실시간 탐색([`crate::live_search`])과 같다: 일꾼 스레드 [`thread_count`]개가 폴더를 나눠 읽고, 폴더 심볼릭
//! 링크·정션은 따라 들어가지 않으며, 읽지 못한 폴더는 건너뛰어 세고, 취소는 표지 하나이며(손잡이를 버려도 같음,
//! 취소한 뒤에는 묶음이 더 오지 않음), 묶음 스레드가 [`BATCH_INTERVAL`]보다 자주 보내지 않는다. 화면 문장은 없다
//! (ADR-0007).
//!
//! - 입력([`ContentRequest`]): 시작점(실제 폴더)·찾을 글자·방식([`ContentMode`]: 대소문자 무시 글자 일치가
//!   기본, 정규식 선택)·파일 이름 조건(필터와 같은 [`crate::name_match`] 규칙, 비우면 모든 파일)·숨김 항목
//!   보이기·큰 파일 한도([`DEFAULT_MAX_FILE_SIZE`], 설정은 S17). 정규식은 선형 시간을 보장하는 `regex` 크레이트로
//!   컴파일하고 틀리면 **시작 전에** 까닭이 담긴 [`ContentError::Regex`]로 돌려준다. 정규식은 줄마다 `^`·`$`가 맞고
//!   (여러 줄 모드) 대소문자를 구분한다(무시하려면 `(?i)`).
//! - 건너뛰는 파일과 종류별 수([`Skipped`]): 온라인 전용 항목(파일 속성만 보고 판단해 열지 않는다,
//!   [`is_online_only`]), 큰 파일(한도 초과, 열지 않음), 읽지 못한 파일(잠김·권한 등), 바이너리로 보이는 파일(앞
//!   [`BINARY_SNIFF_LEN`]바이트에 NUL 바이트가 있음, UTF-16 BOM이 있으면 예외).
//! - 인코딩: BOM으로 UTF-8·UTF-16 LE/BE를 가린다. BOM이 없으면 UTF-8로 읽고 UTF-8로 맞지 않는 파일은 CP949(`EUC-KR`,
//!   웹 표준에서 CP949의 이름)로 한 번 더 읽는다.
//! - 파일은 다른 프로그램의 읽기·쓰기·삭제를 막지 않는 공유 모드로 연다(Windows).
//! - 결과([`ContentFound`]): 실시간 탐색 결과와 같은 항목 정보에 첫 일치 줄과 일치 수를 더한다.

use std::borrow::Cow;
use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use regex::{Regex, RegexBuilder};

pub use crate::live_search::{BATCH_INTERVAL, MAX_THREADS, thread_count};
use crate::name_match::NameMatcher;
use crate::{Item, ListError, Location};

/// 큰 파일 한도의 기본값: 50MB(10진). 이보다 큰 파일은 읽지 않고 건너뛴다. 본문이 이보다 큰 텍스트 파일은 로그·덤프
/// 같은 것이라 찾는 자리가 아니고, 파일 하나를 통째로 메모리에 올리므로 일꾼 수만큼 곱해진 메모리를 막는다. spec #21이
/// 정한 값이고 설정으로 바꾸는 것은 S17이다.
pub const DEFAULT_MAX_FILE_SIZE: u64 = 50_000_000;

/// 바이너리 판정에 보는 앞부분의 길이: 8KB(spec #21).
pub const BINARY_SNIFF_LEN: usize = 8 * 1024;

/// 첫 일치 줄의 길이 상한(글자 수, 줄임표 제외). 목록의 한 칸에 보일 만큼이고 한 줄짜리 거대한 파일(압축한 JS 등)이
/// 결과마다 큰 문자열을 만들지 않게 한다.
pub const MAX_LINE_CHARS: usize = 200;

/// 찾는 방식.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContentMode {
    /// 대소문자를 무시하는 글자 일치(기본).
    #[default]
    Literal,
    /// 정규식(`regex` 크레이트 문법, 선형 시간).
    Regex,
}

/// 내용 검색 요청.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentRequest {
    /// 시작점. 실제 폴더여야 한다.
    pub root: Location,
    /// 찾을 글자(방식이 정규식이면 정규식). 비어 있으면 시작하지 않는다.
    pub text: String,
    /// 찾는 방식.
    pub mode: ContentMode,
    /// 파일 이름 조건([`crate::name_match`]의 규칙). 비우면 모든 파일.
    pub name_pattern: String,
    /// 숨김 항목 보이기.
    pub show_hidden: bool,
    /// 큰 파일 한도(바이트). 이 값을 넘는 파일은 건너뛴다.
    pub max_file_size: u64,
}

impl ContentRequest {
    /// 기본값(글자 일치, 모든 파일, 숨김 항목 감춤, 한도 [`DEFAULT_MAX_FILE_SIZE`])의 요청.
    pub fn new(root: Location, text: impl Into<String>) -> Self {
        ContentRequest {
            root,
            text: text.into(),
            mode: ContentMode::Literal,
            name_pattern: String::new(),
            show_hidden: false,
            max_file_size: DEFAULT_MAX_FILE_SIZE,
        }
    }
}

/// 시작하지 못한 까닭.
#[derive(Debug)]
pub enum ContentError {
    /// 시작점을 읽지 못한다(없음·폴더 아님·실제 폴더가 아님 등).
    Location(ListError),
    /// 찾을 글자가 비었다.
    EmptyText,
    /// 정규식이 틀렸다. 까닭은 `regex` 크레이트의 설명이다(화면 문장은 앱 몫).
    Regex(String),
}

/// 찾은 파일 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentFound {
    /// 파일이 든 폴더.
    pub folder: Arc<Path>,
    /// 파일 항목(실시간 탐색 결과·목록과 같음).
    pub item: Item,
    /// 첫 일치 줄: 그 줄의 앞뒤 공백을 걷고 [`MAX_LINE_CHARS`]글자까지 자른 한 줄(잘리면 줄임표 `…`).
    pub first_line: String,
    /// 파일 안의 일치 수(겹치지 않는 일치).
    pub matches: usize,
}

impl ContentFound {
    /// 파일의 전체 경로.
    pub fn path(&self) -> PathBuf {
        self.folder.join(&self.item.name)
    }
}

/// 건너뛴 파일의 종류별 수.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Skipped {
    /// 바이너리로 보이는 파일.
    pub binary: usize,
    /// 온라인 전용 항목(열면 내려받게 되므로 열지 않음).
    pub online_only: usize,
    /// 한도를 넘는 큰 파일.
    pub too_large: usize,
    /// 읽지 못한 파일(잠김·권한 등).
    pub unreadable: usize,
}

impl Skipped {
    /// 모두 합한 수.
    pub fn total(&self) -> usize {
        self.binary + self.online_only + self.too_large + self.unreadable
    }
}

/// 진행 수치.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContentProgress {
    /// 지금까지 보낸 묶음에 든 파일 수(찾은 파일 수).
    pub found: usize,
    /// 본문을 읽은 파일 수.
    pub files_read: usize,
    /// 읽은 바이트 수.
    pub bytes_read: u64,
    /// 목록을 읽은 폴더 수(시작점 포함).
    pub folders_scanned: usize,
    /// 읽지 못해 건너뛴 폴더 수.
    pub folders_skipped: usize,
    /// 종류별 건너뛴 파일 수.
    pub skipped: Skipped,
}

/// 끝남·취소.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentEnd {
    /// 끝까지 훑었다.
    Finished,
    /// 취소했다(손잡이를 버린 것 포함).
    Cancelled,
}

/// 끝난 검색의 요약.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentSummary {
    /// 마지막 진행(찾은 파일 수는 취소했으면 취소 전에 보낸 것까지).
    pub progress: ContentProgress,
    /// 걸린 시간.
    pub elapsed: Duration,
    /// 끝남·취소.
    pub end: ContentEnd,
}

/// 검색이 보내는 사건.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentEvent {
    /// 찾은 파일 묶음과 그때의 진행. 파일 없이 진행만 바뀐 묶음도 있다. [`BATCH_INTERVAL`]보다 자주 오지 않는다.
    Batch {
        items: Vec<ContentFound>,
        progress: ContentProgress,
    },
    /// 끝남. 마지막 사건이다.
    Finished(ContentSummary),
}

const ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
const ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
const ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;

/// 온라인 전용 항목인가: Windows 파일 속성 값에 `OFFLINE`·`RECALL_ON_OPEN`·`RECALL_ON_DATA_ACCESS`가 하나라도
/// 있으면 열 때 내려받게 되는 클라우드·보관소 파일이다. 파일을 열지 않고 속성만 보므로 순수 함수다.
pub fn is_online_only(attributes: u32) -> bool {
    attributes & (ATTRIBUTE_OFFLINE | ATTRIBUTE_RECALL_ON_OPEN | ATTRIBUTE_RECALL_ON_DATA_ACCESS)
        != 0
}

#[cfg(windows)]
fn raw_attributes(metadata: &Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes()
}

#[cfg(not(windows))]
fn raw_attributes(_metadata: &Metadata) -> u32 {
    0
}

/// 파일을 다른 프로그램의 읽기·쓰기·삭제를 막지 않는 공유 모드로 연다(검색이 파일을 여는 방식. 시험이 같은 방식으로
/// 열어 둔 채 다른 핸들로 쓰고 지울 수 있는지 본다).
pub fn open_shared(path: &Path) -> io::Result<File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
        options.share_mode(0x1 | 0x2 | 0x4);
    }
    options.open(path)
}

/// 바이트가 UTF-16 BOM으로 시작하는가.
fn has_utf16_bom(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF])
}

/// 앞부분(최대 [`BINARY_SNIFF_LEN`]바이트)이 바이너리로 보이는가: NUL 바이트가 있고 UTF-16 BOM이 없다.
pub fn looks_binary(head: &[u8]) -> bool {
    let head = &head[..head.len().min(BINARY_SNIFF_LEN)];
    !has_utf16_bom(head) && head.contains(&0)
}

/// 파일 바이트를 글자로 바꾼다: BOM(UTF-8·UTF-16 LE/BE)이 있으면 그 인코딩, 없으면 UTF-8, 맞지 않으면 CP949.
pub fn decode(bytes: &[u8]) -> Cow<'_, str> {
    if let Some((encoding, bom_len)) = encoding_rs::Encoding::for_bom(bytes) {
        return encoding.decode_without_bom_handling(&bytes[bom_len..]).0;
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => Cow::Borrowed(text),
        Err(_) => encoding_rs::EUC_KR.decode_without_bom_handling(bytes).0,
    }
}

/// `text`에서 `at`(바이트 위치)이 든 줄을 다듬어 한 줄로 낸다.
fn line_at(text: &str, at: usize) -> String {
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    let line = &text[start..end];
    let trimmed_start = line.trim_start();
    let lead = line.len() - trimmed_start.len();
    let line = trimmed_start.trim_end();
    // 일치가 줄 앞의 공백 안이면 줄 처음부터 본다.
    let mut at_in_line = (at - start).saturating_sub(lead).min(line.len());
    while !line.is_char_boundary(at_in_line) {
        at_in_line -= 1;
    }
    let total = line.chars().count();
    if total <= MAX_LINE_CHARS {
        return line.to_owned();
    }
    // 일치가 보이게 일치 앞 글자를 조금(상한의 1/4) 남기고 자른다.
    let match_char = line[..at_in_line].chars().count();
    let first = match_char
        .saturating_sub(MAX_LINE_CHARS / 4)
        .min(total - MAX_LINE_CHARS);
    let mut out = String::new();
    if first > 0 {
        out.push('…');
    }
    out.extend(line.chars().skip(first).take(MAX_LINE_CHARS));
    if first + MAX_LINE_CHARS < total {
        out.push('…');
    }
    out
}

/// 본문에서 찾는다: (첫 일치 줄, 일치 수). 일치가 없으면 `None`.
fn find_in(text: &str, regex: &Regex) -> Option<(String, usize)> {
    let mut found = regex.find_iter(text);
    let first = found.next()?;
    let count = 1 + found.count();
    Some((line_at(text, first.start()), count))
}

/// 일꾼들이 나눠 쓰는 상태.
struct Shared {
    cancelled: AtomicBool,
    queue: Mutex<Queue>,
    changed: Condvar,
    folders_scanned: AtomicUsize,
    folders_skipped: AtomicUsize,
    files_read: AtomicUsize,
    bytes_read: AtomicU64,
    binary: AtomicUsize,
    online_only: AtomicUsize,
    too_large: AtomicUsize,
    unreadable: AtomicUsize,
}

#[derive(Default)]
struct Queue {
    folders: Vec<PathBuf>,
    busy: usize,
}

impl Shared {
    fn queue(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let _queue = self.queue();
        self.changed.notify_all();
    }

    fn progress(&self, found: usize) -> ContentProgress {
        ContentProgress {
            found,
            files_read: self.files_read.load(Ordering::SeqCst),
            bytes_read: self.bytes_read.load(Ordering::SeqCst),
            folders_scanned: self.folders_scanned.load(Ordering::SeqCst),
            folders_skipped: self.folders_skipped.load(Ordering::SeqCst),
            skipped: Skipped {
                binary: self.binary.load(Ordering::SeqCst),
                online_only: self.online_only.load(Ordering::SeqCst),
                too_large: self.too_large.load(Ordering::SeqCst),
                unreadable: self.unreadable.load(Ordering::SeqCst),
            },
        }
    }
}

/// 일꾼들이 읽기만 하는 검색 조건.
struct Spec {
    regex: Regex,
    names: NameMatcher,
    show_hidden: bool,
    max_file_size: u64,
}

type Waker = Arc<dyn Fn() + Send + Sync>;

/// 도는 내용 검색의 손잡이. 버리면 검색을 취소한다(스레드를 기다리지 않는다).
pub struct ContentSearch {
    shared: Arc<Shared>,
    receiver: Receiver<ContentEvent>,
}

impl ContentSearch {
    /// 검색을 시작한다. 시작점이 실제 폴더가 아니거나, 글자가 비었거나, 정규식이 틀리면 시작하지 않는다.
    pub fn start(request: ContentRequest) -> Result<ContentSearch, ContentError> {
        Self::spawn(request, None)
    }

    /// [`ContentSearch::start`]와 같고 사건을 보낼 때마다 `waker`를 부른다(묶음 스레드에서, 보낸 뒤).
    pub fn start_with_waker(
        request: ContentRequest,
        waker: impl Fn() + Send + Sync + 'static,
    ) -> Result<ContentSearch, ContentError> {
        Self::spawn(request, Some(Arc::new(waker)))
    }

    fn spawn(request: ContentRequest, waker: Option<Waker>) -> Result<ContentSearch, ContentError> {
        let not_a_folder = || {
            ContentError::Location(ListError::NotAFolder {
                location: request.root.clone(),
            })
        };
        let root = request.root.file_system_path().ok_or_else(not_a_folder)?;
        let metadata = fs::metadata(root)
            .map_err(|e| ContentError::Location(ListError::from_io(request.root.clone(), e)))?;
        if !metadata.is_dir() {
            return Err(not_a_folder());
        }
        if request.text.is_empty() {
            return Err(ContentError::EmptyText);
        }
        let regex = match request.mode {
            ContentMode::Literal => RegexBuilder::new(&regex::escape(&request.text))
                .case_insensitive(true)
                .build(),
            ContentMode::Regex => RegexBuilder::new(&request.text).multi_line(true).build(),
        }
        .map_err(|e| ContentError::Regex(e.to_string()))?;
        let spec = Arc::new(Spec {
            regex,
            names: NameMatcher::new(&request.name_pattern),
            show_hidden: request.show_hidden,
            max_file_size: request.max_file_size,
        });
        let start = Instant::now();
        let shared = Arc::new(Shared {
            cancelled: AtomicBool::new(false),
            queue: Mutex::new(Queue {
                folders: vec![root.to_path_buf()],
                busy: 0,
            }),
            changed: Condvar::new(),
            folders_scanned: AtomicUsize::new(0),
            folders_skipped: AtomicUsize::new(0),
            files_read: AtomicUsize::new(0),
            bytes_read: AtomicU64::new(0),
            binary: AtomicUsize::new(0),
            online_only: AtomicUsize::new(0),
            too_large: AtomicUsize::new(0),
            unreadable: AtomicUsize::new(0),
        });
        let (found_sender, found_receiver) = mpsc::channel::<Vec<ContentFound>>();
        for _ in 0..thread_count() {
            let shared = Arc::clone(&shared);
            let spec = Arc::clone(&spec);
            let sender = found_sender.clone();
            thread::Builder::new()
                .name("facet-content-search".to_owned())
                .spawn(move || work(&shared, &spec, &sender))
                .expect("내용 검색 일꾼 스레드를 띄우지 못했다");
        }
        drop(found_sender);
        let (sender, receiver) = mpsc::channel();
        {
            let shared = Arc::clone(&shared);
            thread::Builder::new()
                .name("facet-content-search-batch".to_owned())
                .spawn(move || batch(&shared, &found_receiver, &sender, waker.as_ref(), start))
                .expect("내용 검색 묶음 스레드를 띄우지 못했다");
        }
        Ok(ContentSearch { shared, receiver })
    }

    /// 취소한다. 일꾼은 다음 폴더·파일을 열기 전에 멈추고, 이 뒤로 받는 사건에는 묶음이 없다.
    pub fn cancel(&self) {
        self.shared.cancel();
    }

    /// 취소했는가.
    pub fn is_cancelled(&self) -> bool {
        self.shared.is_cancelled()
    }

    /// 쌓인 사건을 모두 받는다. 기다리지 않는다. 취소했으면 묶음은 버린다.
    pub fn poll(&mut self) -> Vec<ContentEvent> {
        let cancelled = self.is_cancelled();
        self.receiver
            .try_iter()
            .filter(|event| !(cancelled && matches!(event, ContentEvent::Batch { .. })))
            .collect()
    }

    /// 사건 하나를 `timeout`까지 기다려 받는다. 취소했으면 묶음은 버리고 다음 사건을 기다린다.
    pub fn wait_event(&mut self, timeout: Duration) -> Option<ContentEvent> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let event = self.receiver.recv_timeout(left).ok()?;
            if self.is_cancelled() && matches!(event, ContentEvent::Batch { .. }) {
                continue;
            }
            return Some(event);
        }
    }
}

impl Drop for ContentSearch {
    fn drop(&mut self) {
        self.shared.cancel();
    }
}

fn work(shared: &Shared, spec: &Spec, sender: &Sender<Vec<ContentFound>>) {
    loop {
        let folder = {
            let mut queue = shared.queue();
            loop {
                if shared.is_cancelled() {
                    return;
                }
                if let Some(folder) = queue.folders.pop() {
                    queue.busy += 1;
                    break folder;
                }
                if queue.busy == 0 {
                    shared.changed.notify_all();
                    return;
                }
                queue = shared
                    .changed
                    .wait(queue)
                    .unwrap_or_else(|e| e.into_inner());
            }
        };
        let (found, subfolders) = scan(shared, &folder, spec);
        if !found.is_empty() {
            let _ = sender.send(found);
        }
        let mut queue = shared.queue();
        queue.folders.extend(subfolders);
        queue.busy -= 1;
        shared.changed.notify_all();
    }
}

/// 폴더 하나를 읽어 이름이 맞는 파일의 본문을 찾고 들어갈 하위 폴더를 낸다.
fn scan(shared: &Shared, folder: &Path, spec: &Spec) -> (Vec<ContentFound>, Vec<PathBuf>) {
    let skip = || {
        shared.folders_skipped.fetch_add(1, Ordering::SeqCst);
        (Vec::new(), Vec::new())
    };
    let Ok(entries) = fs::read_dir(folder) else {
        return skip();
    };
    let shared_folder: Arc<Path> = Arc::from(folder);
    let mut found = Vec::new();
    let mut subfolders = Vec::new();
    for entry in entries {
        if shared.is_cancelled() {
            return (Vec::new(), Vec::new());
        }
        let Ok(entry) = entry else {
            return skip();
        };
        let (Ok(metadata), Ok(kind)) = (entry.metadata(), entry.file_type()) else {
            continue;
        };
        let Ok(item) =
            crate::file_system::make_item(entry.file_name(), &|| entry.path(), &metadata)
        else {
            continue;
        };
        if !spec.show_hidden && item.attributes.hidden {
            continue;
        }
        if item.is_folder {
            if !kind.is_symlink() {
                subfolders.push(entry.path());
            }
            continue;
        }
        if !spec.names.is_match(&item.name) {
            continue;
        }
        if let Some((first_line, matches)) = search_file(shared, spec, &entry.path(), &metadata) {
            found.push(ContentFound {
                folder: Arc::clone(&shared_folder),
                item,
                first_line,
                matches,
            });
        }
    }
    shared.folders_scanned.fetch_add(1, Ordering::SeqCst);
    (found, subfolders)
}

/// 파일 하나의 본문에서 찾는다. 건너뛰면 그 종류를 세고 `None`이다.
fn search_file(
    shared: &Shared,
    spec: &Spec,
    path: &Path,
    metadata: &Metadata,
) -> Option<(String, usize)> {
    if is_online_only(raw_attributes(metadata)) {
        shared.online_only.fetch_add(1, Ordering::SeqCst);
        return None;
    }
    if metadata.len() > spec.max_file_size {
        shared.too_large.fetch_add(1, Ordering::SeqCst);
        return None;
    }
    let unreadable = || {
        shared.unreadable.fetch_add(1, Ordering::SeqCst);
        None
    };
    let Ok(mut file) = open_shared(path) else {
        return unreadable();
    };
    // 앞부분만 먼저 읽어 바이너리면 나머지를 읽지 않는다.
    let mut bytes = Vec::new();
    if (&mut file)
        .take(BINARY_SNIFF_LEN as u64)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return unreadable();
    }
    if looks_binary(&bytes) {
        shared.binary.fetch_add(1, Ordering::SeqCst);
        return None;
    }
    // 읽는 동안 커진 파일도 한도를 넘어 읽지 않게 한도까지만 읽는다.
    let rest = spec.max_file_size.saturating_sub(bytes.len() as u64);
    if file.take(rest).read_to_end(&mut bytes).is_err() {
        return unreadable();
    }
    shared.files_read.fetch_add(1, Ordering::SeqCst);
    shared
        .bytes_read
        .fetch_add(bytes.len() as u64, Ordering::SeqCst);
    find_in(&decode(&bytes), &spec.regex)
}

fn batch(
    shared: &Shared,
    found: &Receiver<Vec<ContentFound>>,
    sender: &Sender<ContentEvent>,
    waker: Option<&Waker>,
    start: Instant,
) {
    let send = |event: ContentEvent| {
        if sender.send(event).is_ok()
            && let Some(waker) = waker
        {
            waker();
        }
    };
    let mut pending: Vec<ContentFound> = Vec::new();
    let mut sent = 0usize;
    let mut last_progress = ContentProgress::default();
    let mut last_emit: Option<Instant> = None;
    let mut emit = |pending: &mut Vec<ContentFound>, last_emit: &mut Option<Instant>| {
        let progress = shared.progress(sent + pending.len());
        if pending.is_empty() && progress == last_progress {
            return;
        }
        sent = progress.found;
        last_progress = progress;
        send(ContentEvent::Batch {
            items: std::mem::take(pending),
            progress,
        });
        *last_emit = Some(Instant::now());
    };
    loop {
        let wait = last_emit.map_or(BATCH_INTERVAL, |at| {
            (at + BATCH_INTERVAL).saturating_duration_since(Instant::now())
        });
        match found.recv_timeout(wait) {
            Ok(items) => pending.extend(items),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if shared.is_cancelled() {
            pending.clear();
            continue;
        }
        if last_emit.is_none_or(|at| at.elapsed() >= BATCH_INTERVAL) {
            emit(&mut pending, &mut last_emit);
        }
    }
    if !shared.is_cancelled() {
        if let Some(at) = last_emit {
            thread::sleep((at + BATCH_INTERVAL).saturating_duration_since(Instant::now()));
        }
        if !shared.is_cancelled() {
            emit(&mut pending, &mut last_emit);
        }
    }
    let end = if shared.is_cancelled() {
        ContentEnd::Cancelled
    } else {
        ContentEnd::Finished
    };
    send(ContentEvent::Finished(ContentSummary {
        progress: shared.progress(sent),
        elapsed: start.elapsed(),
        end,
    }));
}
