//! #336: 미리보기(텍스트 읽기·종류 고르기·내용 만들기·요청기·미리보기 영역).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::Item;
use crate::thumbnail::Thumbnail;

/// #336: 텍스트 미리보기가 읽는 앞부분의 한도(256KB).
pub const TEXT_HEAD_LIMIT: usize = 256 * 1024;

/// #336: 추정한 텍스트 인코딩(BOM 유무는 가르지 않음).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    Cp949,
    Windows1252,
}

const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

fn units16(bytes: &[u8], little: bool) -> impl Iterator<Item = u16> + '_ {
    bytes.as_chunks::<2>().0.iter().map(move |c| {
        if little {
            u16::from_le_bytes([c[0], c[1]])
        } else {
            u16::from_be_bytes([c[0], c[1]])
        }
    })
}

/// 끝의 반쪽 글자 하나를 용서하고 UTF-16으로 맞는지 본다.
fn utf16_fits(bytes: &[u8], little: bool) -> bool {
    let mut pending_high = false;
    for unit in units16(bytes, little) {
        match (pending_high, unit) {
            (false, 0xD800..=0xDBFF) => pending_high = true,
            (false, 0xDC00..=0xDFFF) => return false,
            (true, 0xDC00..=0xDFFF) => pending_high = false,
            (true, _) => return false,
            _ => {}
        }
    }
    true
}

fn cp949_strict(bytes: &[u8]) -> Option<String> {
    encoding_rs::EUC_KR
        .decode_without_bom_handling_and_without_replacement(bytes)
        .map(|s| s.into_owned())
}

/// CP949로 맞는지 본다(끝의 반쪽 글자 한 바이트는 용서).
fn cp949_fits(bytes: &[u8]) -> bool {
    cp949_strict(bytes).is_some()
        || (bytes.last().is_some_and(|b| *b >= 0x81)
            && cp949_strict(&bytes[..bytes.len() - 1]).is_some())
}

/// #336: 바이트의 인코딩을 추정한다(`None`이면 텍스트가 아님).
///
/// 차례: BOM → NUL 모양(UTF-16, 그 밖의 NUL은 바이너리) → UTF-8 → CP949 → Windows-1252. NUL이 든 바이트는 UTF-8로
/// 맞춰 보지 않는다(ASCII만 담은 BOM 없는 UTF-16이 NUL 섞인 올바른 UTF-8이기도 하므로).
pub fn detect_encoding(bytes: &[u8]) -> Option<TextEncoding> {
    if bytes.is_empty() || bytes.starts_with(&UTF8_BOM) {
        return Some(TextEncoding::Utf8);
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return Some(TextEncoding::Utf16Le);
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return Some(TextEncoding::Utf16Be);
    }
    let (mut even, mut odd) = (0usize, 0usize);
    for (i, b) in bytes.iter().enumerate() {
        if *b == 0 {
            if i % 2 == 0 {
                even += 1;
            } else {
                odd += 1;
            }
        }
    }
    if even + odd > 0 {
        // 한쪽 자리에 몰린 NUL만 UTF-16 모양이다(LE는 홀수 자리, BE는 짝수 자리).
        return if odd >= 9 * even && utf16_fits(bytes, true) {
            Some(TextEncoding::Utf16Le)
        } else if even >= 9 * odd && utf16_fits(bytes, false) {
            Some(TextEncoding::Utf16Be)
        } else {
            None
        };
    }
    match std::str::from_utf8(bytes) {
        Ok(_) => return Some(TextEncoding::Utf8),
        Err(e) if e.error_len().is_none() => return Some(TextEncoding::Utf8),
        Err(_) => {}
    }
    if cp949_fits(bytes) {
        Some(TextEncoding::Cp949)
    } else {
        Some(TextEncoding::Windows1252)
    }
}

/// #336: 바이트를 그 인코딩의 글자로 바꾼다(BOM은 걷고 끝의 반쪽 글자는 버림).
pub fn decode_text(bytes: &[u8], encoding: TextEncoding) -> String {
    match encoding {
        TextEncoding::Utf8 => {
            let bytes = bytes.strip_prefix(&UTF8_BOM[..]).unwrap_or(bytes);
            match std::str::from_utf8(bytes) {
                Ok(s) => s.to_owned(),
                Err(e) if e.error_len().is_none() => {
                    String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned()
                }
                Err(_) => String::from_utf8_lossy(bytes).into_owned(),
            }
        }
        TextEncoding::Utf16Le | TextEncoding::Utf16Be => {
            let little = encoding == TextEncoding::Utf16Le;
            let bom: [u8; 2] = if little { [0xFF, 0xFE] } else { [0xFE, 0xFF] };
            let bytes = bytes.strip_prefix(&bom[..]).unwrap_or(bytes);
            let mut units: Vec<u16> = units16(bytes, little).collect();
            if units.last().is_some_and(|u| (0xD800..=0xDBFF).contains(u)) {
                units.pop();
            }
            String::from_utf16_lossy(&units)
        }
        TextEncoding::Cp949 => cp949_strict(bytes)
            .or_else(|| {
                bytes
                    .len()
                    .checked_sub(1)
                    .and_then(|n| cp949_strict(&bytes[..n]))
            })
            .unwrap_or_else(|| {
                encoding_rs::EUC_KR
                    .decode_without_bom_handling(bytes)
                    .0
                    .into_owned()
            }),
        TextEncoding::Windows1252 => encoding_rs::WINDOWS_1252
            .decode_without_bom_handling(bytes)
            .0
            .into_owned(),
    }
}

/// #336: 텍스트 미리보기(앞부분의 글자, 인코딩, 잘렸는가).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextPreview {
    pub text: String,
    pub encoding: TextEncoding,
    pub truncated: bool,
}

fn problem_of(error: &std::io::Error) -> PreviewProblem {
    // 32: 공유 위반, 33: 잠금 위반.
    if matches!(error.raw_os_error(), Some(32 | 33)) {
        return PreviewProblem::Locked;
    }
    match error.kind() {
        std::io::ErrorKind::NotFound => PreviewProblem::Missing,
        std::io::ErrorKind::PermissionDenied => PreviewProblem::AccessDenied,
        kind => PreviewProblem::Io(kind),
    }
}

/// #336: 파일 앞부분을 한도까지 읽어 글자로 바꾼다.
///
/// 읽기·쓰기·삭제 공유를 모두 허락해 열고 읽자마자 닫는다. 바이너리는 `Err(NotText)`.
pub fn read_text_head(path: &Path, limit: usize) -> Result<TextPreview, PreviewProblem> {
    use std::io::Read;

    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
        options.share_mode(7);
    }
    let file = options.open(path).map_err(|e| problem_of(&e))?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| problem_of(&e))?;
    let truncated = bytes.len() > limit;
    bytes.truncate(limit);
    let encoding = detect_encoding(&bytes).ok_or(PreviewProblem::NotText)?;
    Ok(TextPreview {
        text: decode_text(&bytes, encoding),
        encoding,
        truncated,
    })
}

/// #336: 미리 보지 못한 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewProblem {
    OnlineOnly,
    NotText,
    Missing,
    AccessDenied,
    Locked,
    Corrupt,
    TooLarge,
    Unsupported,
    Failed,
    ShellFailed,
    Io(std::io::ErrorKind),
}

/// #337: 미리보기 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewKind {
    Folder,
    Image,
    Text,
    Shell,
    Info,
    Unavailable(PreviewProblem),
}

/// #337: 항목의 미리보기 종류를 고른다.
pub fn choose_kind(path: &Path, item: &Item, has_handler: &dyn Fn(&str) -> bool) -> PreviewKind {
    if item.is_folder {
        return PreviewKind::Folder;
    }
    if item.attributes.online_only {
        return PreviewKind::Unavailable(PreviewProblem::OnlineOnly);
    }
    if crate::thumbnail::is_decodable(&item.name) {
        return PreviewKind::Image;
    }
    let ext = extension_lower(&item.name);
    let text_ext = ext.as_deref().is_some_and(|e| TEXT_EXTENSIONS.contains(&e));
    if !text_ext
        && let Some(e) = ext.as_deref()
        && has_handler(e)
    {
        return PreviewKind::Shell;
    }
    match read_text_head(path, SNIFF_LIMIT) {
        Ok(_) => PreviewKind::Text,
        Err(PreviewProblem::NotText) => PreviewKind::Info,
        Err(problem) => PreviewKind::Unavailable(problem),
    }
}

/// #337: 텍스트로 보는 확장자(소문자).
const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "log", "md", "json", "csv", "xml", "ini", "rs", "toml", "yaml", "yml", "cfg", "conf",
    "html", "htm", "css", "js", "ts", "py", "c", "h", "cpp", "java", "sh", "bat", "ps1", "slint",
];

/// #337: 종류를 정할 때 앞부분을 보는 크기.
const SNIFF_LIMIT: usize = 8 * 1024;

fn extension_lower(name: &std::ffi::OsStr) -> Option<String> {
    let name = name.to_str()?;
    let (stem, ext) = name.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() {
        return None;
    }
    Some(ext.to_lowercase())
}

/// #337: 미리보기 내용 요청.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewRequest {
    pub path: PathBuf,
    pub item: Item,
    pub show_hidden: bool,
    pub image_side: u32,
}

/// #337: 도우미 미리보기의 상태.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellPreview {
    Loading,
    Showing,
}

/// #337: 미리보기 본문.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewBody {
    Loading,
    Folder {
        count: Option<u64>,
    },
    Image {
        image: Arc<Thumbnail>,
        placeholder: bool,
    },
    Text(TextPreview),
    Shell(ShellPreview),
    Info,
    Unavailable(PreviewProblem),
}

/// #337: 종류대로 미리보기 내용을 만든다.
pub fn load_preview(
    request: &PreviewRequest,
    cancel: &AtomicBool,
    has_handler: &dyn Fn(&str) -> bool,
) -> PreviewBody {
    let run = || match choose_kind(&request.path, &request.item, has_handler) {
        PreviewKind::Folder => PreviewBody::Folder {
            count: count_folder(&request.path, request.show_hidden),
        },
        PreviewKind::Image => {
            match crate::thumbnail::make_thumbnail_with(
                &request.path,
                request.image_side,
                &crate::thumbnail::DecodeLimits::default(),
                cancel,
            ) {
                Ok(image) => PreviewBody::Image {
                    image: Arc::new(image),
                    placeholder: false,
                },
                Err(error) => PreviewBody::Unavailable(match error {
                    crate::thumbnail::ThumbnailError::Unsupported => PreviewProblem::Unsupported,
                    crate::thumbnail::ThumbnailError::NotFound => PreviewProblem::Missing,
                    crate::thumbnail::ThumbnailError::AccessDenied => PreviewProblem::AccessDenied,
                    crate::thumbnail::ThumbnailError::TooLarge => PreviewProblem::TooLarge,
                    crate::thumbnail::ThumbnailError::Corrupt => PreviewProblem::Corrupt,
                    crate::thumbnail::ThumbnailError::Io(kind) => PreviewProblem::Io(kind),
                    crate::thumbnail::ThumbnailError::Cancelled
                    | crate::thumbnail::ThumbnailError::Panicked => PreviewProblem::Failed,
                }),
            }
        }
        PreviewKind::Text => match read_text_head(&request.path, TEXT_HEAD_LIMIT) {
            Ok(text) => PreviewBody::Text(text),
            Err(PreviewProblem::NotText) => PreviewBody::Info,
            Err(problem) => PreviewBody::Unavailable(problem),
        },
        PreviewKind::Shell => PreviewBody::Shell(ShellPreview::Loading),
        PreviewKind::Info => PreviewBody::Info,
        PreviewKind::Unavailable(problem) => PreviewBody::Unavailable(problem),
    };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(run))
        .unwrap_or(PreviewBody::Unavailable(PreviewProblem::Failed))
}

fn count_folder(path: &Path, show_hidden: bool) -> Option<u64> {
    let entries = std::fs::read_dir(path).ok()?;
    let mut count = 0u64;
    for entry in entries.flatten() {
        if !show_hidden && is_hidden(&entry) {
            continue;
        }
        count += 1;
    }
    Some(count)
}

#[cfg(windows)]
fn is_hidden(entry: &std::fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    // FILE_ATTRIBUTE_HIDDEN
    entry
        .metadata()
        .is_ok_and(|m| m.file_attributes() & 0x2 != 0)
}

#[cfg(not(windows))]
fn is_hidden(_entry: &std::fs::DirEntry) -> bool {
    false
}

/// #338: 선택이 바뀐 뒤 요청을 내기까지 기다리는 시간(50ms).
///
/// 화살표 키를 누르고 있으면 선택이 30ms 안팎마다 바뀐다(키 반복). 그보다 길어야 지나가는 항목마다 파일을 읽지
/// 않고, 사람이 멈춘 뒤 미리보기가 늦다고 느끼지 않을 만큼(100ms 아래) 짧다.
pub const PREVIEW_DELAY: Duration = Duration::from_millis(50);

/// #338: 미리보기 요청 번호.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PreviewId(u64);

/// #338: 끝난 미리보기 요청.
#[derive(Debug, Clone)]
pub struct PreviewDone {
    pub id: PreviewId,
    pub request: PreviewRequest,
    pub body: PreviewBody,
}

type Loader = dyn Fn(&PreviewRequest, &AtomicBool) -> PreviewBody + Send + Sync;
type Waker = Arc<dyn Fn() + Send + Sync>;

/// 일꾼들과 나누는 상태.
struct Shared {
    /// 마지막으로 낸 요청 번호(0은 없음). 일꾼은 제 번호가 아니면 결과를 버린다.
    latest: AtomicU64,
    state: Mutex<SharedState>,
    event: Condvar,
}

#[derive(Default)]
struct SharedState {
    results: VecDeque<PreviewDone>,
    waker: Option<Waker>,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, SharedState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// #338: 미리보기 요청기(기다림·취소·늦은 결과 버림).
///
/// 선택이 바뀌면(`request`) 앞선 요청에 취소 표지를 세우고 `PREVIEW_DELAY`만큼 기다렸다가(`tick`, 시각은 부르는 쪽이
/// 넘김) 낸다. 낸 요청마다 일꾼 스레드 하나에서 만든다: 앞선 요청이 표지를 무시하고 붙잡혀 있어도(멈춘 디코더·처리기)
/// 다음 요청이 그 뒤에 줄 서지 않게 하려는 것이다. 내기는 기다림 덕에 사람이 멈춘 뒤 한 번꼴이라 스레드 비용은 작다.
/// 결과는 마지막으로 낸 요청의 것만 받고(`poll`·`wait_event`), 그 밖의 결과는 일꾼이 보내지 않고 받는 쪽도 버린다.
pub struct Previewer {
    loader: Arc<Loader>,
    shared: Arc<Shared>,
    /// 기다리는 요청과 낼 시각.
    waiting: Option<(PreviewRequest, Instant)>,
    /// 진행 중일 수 있는 마지막으로 낸 요청의 취소 표지.
    running: Option<Arc<AtomicBool>>,
    next_id: u64,
}

impl Previewer {
    /// #338: 만드는 함수로 요청기를 만든다.
    pub fn new(
        loader: impl Fn(&PreviewRequest, &AtomicBool) -> PreviewBody + Send + Sync + 'static,
    ) -> Previewer {
        Previewer {
            loader: Arc::new(loader),
            shared: Arc::new(Shared {
                latest: AtomicU64::new(0),
                state: Mutex::new(SharedState::default()),
                event: Condvar::new(),
            }),
            waiting: None,
            running: None,
            next_id: 1,
        }
    }

    /// #338: 결과가 올 때마다 부를 깨우기.
    pub fn set_waker(&self, waker: impl Fn() + Send + Sync + 'static) {
        self.shared.lock().waker = Some(Arc::new(waker));
    }

    /// #338: 선택이 바뀌었다(`None`은 선택 없음).
    ///
    /// 앞선 요청에 취소 표지를 세우고 그 결과를 버리며, 새 요청은 `now`부터 다시 기다린다.
    pub fn request(&mut self, request: Option<PreviewRequest>, now: Instant) {
        self.cancel_running();
        self.waiting = request.map(|r| (r, now + PREVIEW_DELAY));
    }

    /// #338: 기다리는 요청을 낼 시각.
    pub fn due(&self) -> Option<Instant> {
        self.waiting.as_ref().map(|(_, due)| *due)
    }

    /// #338: 기다림이 끝났으면 일꾼에 내고 그 번호.
    pub fn tick(&mut self, now: Instant) -> Option<PreviewId> {
        if self.due().is_none_or(|due| now < due) {
            return None;
        }
        let (request, _) = self.waiting.take()?;
        self.cancel_running();
        let id = self.next_id;
        self.next_id += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.running = Some(Arc::clone(&cancel));
        self.shared.latest.store(id, Ordering::SeqCst);

        let loader = Arc::clone(&self.loader);
        let shared = Arc::clone(&self.shared);
        let spawned = std::thread::Builder::new()
            .name("facet-preview".into())
            .spawn(move || {
                let body = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    loader(&request, &cancel)
                }))
                .unwrap_or(PreviewBody::Unavailable(PreviewProblem::Failed));
                deliver(
                    &shared,
                    PreviewDone {
                        id: PreviewId(id),
                        request,
                        body,
                    },
                );
            });
        if spawned.is_err() {
            // 스레드를 띄우지 못했다: 만들지 못한 것으로 알린다(요청은 스레드로 옮겨 갔으므로 받지 못함).
            self.shared.latest.store(0, Ordering::SeqCst);
            self.running = None;
            return None;
        }
        Some(PreviewId(id))
    }

    /// #338: 마지막으로 낸 요청의 결과.
    pub fn poll(&mut self) -> Vec<PreviewDone> {
        let latest = self.shared.latest.load(Ordering::SeqCst);
        let mut state = self.shared.lock();
        state
            .results
            .drain(..)
            .filter(|d| d.id.0 == latest)
            .collect()
    }

    /// #338: 결과 하나를 기다린다.
    pub fn wait_event(&mut self, timeout: Duration) -> Option<PreviewDone> {
        let deadline = Instant::now() + timeout;
        let mut state = self.shared.lock();
        loop {
            while let Some(done) = state.results.pop_front() {
                if done.id.0 == self.shared.latest.load(Ordering::SeqCst) {
                    return Some(done);
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            state = self
                .shared
                .event
                .wait_timeout(state, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// 진행 중인 요청에 취소 표지를 세우고 그 결과를 버리게 한다.
    fn cancel_running(&mut self) {
        if let Some(cancel) = self.running.take() {
            cancel.store(true, Ordering::SeqCst);
        }
        self.shared.latest.store(0, Ordering::SeqCst);
    }
}

impl Drop for Previewer {
    fn drop(&mut self) {
        self.cancel_running();
    }
}

/// 일꾼이 끝낸 결과를 넣고 깨운다(그 사이 더 뒤 요청이 났으면 버림).
fn deliver(shared: &Shared, done: PreviewDone) {
    let waker = {
        let mut state = shared.lock();
        if shared.latest.load(Ordering::SeqCst) != done.id.0 {
            return;
        }
        state.results.push_back(done);
        state.waker.clone()
    };
    shared.event.notify_all();
    if let Some(waker) = waker {
        waker();
    }
}

/// #339: 미리보기 영역의 기본 폭(논리 픽셀).
pub const PREVIEW_WIDTH_DEFAULT: f32 = 320.0;
/// #339: 미리보기 영역의 가장 좁은 폭.
pub const PREVIEW_WIDTH_MIN: f32 = 160.0;
/// #339: 미리보기 영역의 가장 넓은 폭.
pub const PREVIEW_WIDTH_MAX: f32 = 1600.0;

/// #339: 창의 미리보기 영역(켜짐·폭).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreviewPane {
    pub visible: bool,
    pub width: f32,
}

impl Default for PreviewPane {
    fn default() -> Self {
        PreviewPane {
            visible: false,
            width: PREVIEW_WIDTH_DEFAULT,
        }
    }
}

/// #339: 미리보기 영역의 내용.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewContent {
    Nothing,
    Many {
        count: usize,
        total_size: u64,
    },
    Item {
        path: PathBuf,
        item: Item,
        body: PreviewBody,
    },
}

/// #342: 도우미 미리보기의 대기 시간(3초).
pub const SHELL_PREVIEW_TIMEOUT: Duration = Duration::ZERO; // #342

/// #342: 처리기 창을 붙일 자리(창 핸들 값, 화면 물리 픽셀).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewHost {
    pub parent: u64,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
