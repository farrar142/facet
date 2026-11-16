//! 실시간 탐색: 인덱스가 없는 위치에서 실제 폴더 아래를 그 자리에서 훑어 이름이 맞는 항목을 찾는 대로 흘려
//! 보내는 엔진(ADR-0003의 강등 경로, 용어는 GLOSSARY의 **실시간 탐색**).
//!
//! - [`LiveSearch::start`]가 시작점(실제 폴더)·이름 조건([`crate::name_match`]의 규칙, 필터와 같음)·숨김 항목
//!   보이기 여부([`SearchRequest`])를 받아 그 아래를 훑는다. 시작점 자신은 결과에 넣지 않는다.
//! - 일꾼 스레드 [`thread_count`]개(논리 코어 수와 [`MAX_THREADS`] 중 작은 것)가 폴더 하나씩 나눠 읽는다.
//! - 찾은 항목([`Found`]: 든 폴더와 목록 항목과 같은 [`Item`])은 묶음 스레드가 모아 [`BATCH_INTERVAL`]보다 자주
//!   보내지 않는다([`SearchEvent::Batch`], 진행 [`SearchProgress`]를 함께). 끝나면 [`SearchEvent::Finished`]
//!   ([`SearchSummary`])가 마지막 사건이다.
//! - 폴더 심볼릭 링크와 정션은 따라 들어가지 않는다(순환 방지, 같은 항목이 두 번 나오지 않음). 링크 자체는 이름이
//!   맞으면 결과에 낸다.
//! - 숨김 항목을 감추면 숨김 속성 항목은 결과에 내지 않고, 숨김 폴더 안으로도 들어가지 않는다(목록에서 보이지 않는
//!   폴더 안의 항목도 보이지 않게).
//! - 읽지 못한 폴더(권한 없음, 그 사이 사라짐 등)는 건너뛰고 수를 센다([`SearchProgress::folders_skipped`]).
//! - 취소([`LiveSearch::cancel`], 손잡이를 버려도 같음)는 표지 하나다. 일꾼은 다음 폴더를 열기 전에 표지를 본다.
//!   취소한 뒤에는 묶음이 더 오지 않고(이미 통로에 든 묶음도 [`LiveSearch::poll`]·[`LiveSearch::wait_event`]가
//!   버린다) 요약은 [`SearchEnd::Cancelled`]다.
//!
//! 사건은 탭의 읽기 결과·작업 대기열 사건과 나란한 통로로 받는다: [`LiveSearch::start_with_waker`]의 깨우기가 사건을
//! 보낼 때마다 불리고(앱은 이벤트 루프만 깨운다) [`LiveSearch::poll`]로 받는다. 화면 문장은 없다(ADR-0007).

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use crate::name_match::NameMatcher;
use crate::{Item, ListError, Location};

/// 일꾼 스레드 수의 상한. 디스크 하나를 훑는 일이라 스레드를 늘려도 어느 수 위로는 빨라지지 않고(SSD의 큐 깊이,
/// 파일 시스템의 디렉터리 잠금), 탐색이 도는 동안 앱의 다른 일(목록 읽기·필터·복사)이 쓸 코어를 남긴다. spec #21이
/// 정한 값이다.
pub const MAX_THREADS: usize = 8;

/// 묶음 사이 가장 짧은 간격: 1/30초를 올림한 34ms. 결과를 초당 30번 넘게 보내도 화면이 더 빨리 보이지 않고(사람이
/// 흐름으로 읽는 빠르기), 목록을 다시 그리는 일만 늘어난다. spec #21의 「초당 30번 이하」다.
pub const BATCH_INTERVAL: Duration = Duration::from_millis(34);

/// 일꾼 스레드 수: 논리 코어 수와 [`MAX_THREADS`] 중 작은 것(코어 수를 모르면 1).
pub fn thread_count() -> usize {
    thread::available_parallelism()
        .map_or(1, usize::from)
        .clamp(1, MAX_THREADS)
}

/// 실시간 탐색 요청.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    /// 시작점. 실제 폴더여야 한다.
    pub root: Location,
    /// 이름 조건([`crate::name_match`]의 규칙: 부분 일치, `*`·`?`가 있으면 이름 전체 와일드카드, 대소문자 무시).
    pub pattern: String,
    /// 숨김 항목 보이기.
    pub show_hidden: bool,
}

/// 찾은 항목 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// 항목이 든 폴더(위치 열에 쓸 것). 같은 폴더의 항목끼리 나눠 쓴다.
    pub folder: Arc<Path>,
    /// 항목(이름·폴더 여부·크기·수정 시각·속성, 목록 항목과 같음).
    pub item: Item,
}

impl Found {
    /// 항목의 전체 경로.
    pub fn path(&self) -> PathBuf {
        self.folder.join(&self.item.name)
    }

    /// 항목 이름.
    pub fn name(&self) -> &OsString {
        &self.item.name
    }
}

/// 진행 수치.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchProgress {
    /// 지금까지 보낸 묶음에 든 항목 수.
    pub found: usize,
    /// 목록을 읽은 폴더 수(시작점 포함).
    pub folders_scanned: usize,
    /// 읽지 못해 건너뛴 폴더 수(권한 없음 등).
    pub folders_skipped: usize,
}

/// 탐색이 어떻게 끝났는가.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchEnd {
    /// 끝까지 훑었다.
    Finished,
    /// 취소했다(손잡이를 버린 것 포함).
    Cancelled,
}

/// 끝난 탐색의 요약.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchSummary {
    /// 보낸 묶음에 든 항목 수(취소했으면 취소 전에 보낸 것까지).
    pub found: usize,
    /// 목록을 읽은 폴더 수(시작점 포함).
    pub folders_scanned: usize,
    /// 읽지 못해 건너뛴 폴더 수.
    pub folders_skipped: usize,
    /// 시작부터 끝까지 걸린 시간.
    pub elapsed: Duration,
    /// 끝남·취소.
    pub end: SearchEnd,
}

/// 탐색이 보내는 사건.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchEvent {
    /// 찾은 항목 묶음과 그때의 진행. 항목이 없고 진행만 바뀐 묶음도 있다. [`BATCH_INTERVAL`]보다 자주 오지 않는다.
    Batch {
        items: Vec<Found>,
        progress: SearchProgress,
    },
    /// 끝남. 마지막 사건이다.
    Finished(SearchSummary),
}

/// 깨우기.
type Waker = Arc<dyn Fn() + Send + Sync>;

/// 일꾼 스레드들이 나눠 쓰는 상태.
struct Shared {
    /// 취소 표지.
    cancelled: AtomicBool,
    /// 훑을 폴더와 지금 폴더를 읽는 일꾼 수.
    queue: Mutex<Queue>,
    /// 폴더가 더해졌거나 끝났거나 취소됨.
    changed: Condvar,
    folders_scanned: AtomicUsize,
    folders_skipped: AtomicUsize,
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
        // 잠금을 쥐고 알려야 기다리기 직전의 일꾼이 알림을 놓치지 않는다.
        let _queue = self.queue();
        self.changed.notify_all();
    }

    fn progress(&self, found: usize) -> SearchProgress {
        SearchProgress {
            found,
            folders_scanned: self.folders_scanned.load(Ordering::SeqCst),
            folders_skipped: self.folders_skipped.load(Ordering::SeqCst),
        }
    }
}

/// 도는 실시간 탐색의 손잡이. 버리면 탐색을 취소한다(스레드를 기다리지 않는다).
pub struct LiveSearch {
    shared: Arc<Shared>,
    receiver: Receiver<SearchEvent>,
}

impl LiveSearch {
    /// 탐색을 시작한다. 시작점이 없거나 폴더가 아니거나 실제 폴더 위치가 아니면 시작하지 않는다.
    pub fn start(request: SearchRequest) -> Result<LiveSearch, ListError> {
        Self::spawn(request, None)
    }

    /// [`LiveSearch::start`]와 같고, 사건을 보낼 때마다 `waker`를 부른다(묶음 스레드에서, 보낸 뒤). 깨우기 안에서
    /// 이 손잡이를 부르지 않는다(앱은 이벤트 루프만 깨운다).
    pub fn start_with_waker(
        request: SearchRequest,
        waker: impl Fn() + Send + Sync + 'static,
    ) -> Result<LiveSearch, ListError> {
        Self::spawn(request, Some(Arc::new(waker)))
    }

    fn spawn(request: SearchRequest, waker: Option<Waker>) -> Result<LiveSearch, ListError> {
        let root = request
            .root
            .file_system_path()
            .ok_or_else(|| ListError::NotAFolder {
                location: request.root.clone(),
            })?;
        let metadata =
            fs::metadata(root).map_err(|e| ListError::from_io(request.root.clone(), e))?;
        if !metadata.is_dir() {
            return Err(ListError::NotAFolder {
                location: request.root.clone(),
            });
        }
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
        });
        let matcher = Arc::new(NameMatcher::new(&request.pattern));
        let (found_sender, found_receiver) = mpsc::channel::<Vec<Found>>();
        for _ in 0..thread_count() {
            let shared = Arc::clone(&shared);
            let matcher = Arc::clone(&matcher);
            let sender = found_sender.clone();
            let show_hidden = request.show_hidden;
            thread::Builder::new()
                .name("facet-live-search".to_owned())
                .spawn(move || work(&shared, &matcher, show_hidden, &sender))
                .expect("실시간 탐색 일꾼 스레드를 띄우지 못했다");
        }
        drop(found_sender);
        let (sender, receiver) = mpsc::channel();
        {
            let shared = Arc::clone(&shared);
            thread::Builder::new()
                .name("facet-live-search-batch".to_owned())
                .spawn(move || batch(&shared, &found_receiver, &sender, waker.as_ref(), start))
                .expect("실시간 탐색 묶음 스레드를 띄우지 못했다");
        }
        Ok(LiveSearch { shared, receiver })
    }

    /// 취소한다. 일꾼은 다음 폴더를 열기 전에 멈추고, 이 뒤로 받는 사건에는 묶음이 없다. 끝난 뒤에 불러도 된다.
    pub fn cancel(&self) {
        self.shared.cancel();
    }

    /// 취소했는가.
    pub fn is_cancelled(&self) -> bool {
        self.shared.is_cancelled()
    }

    /// 쌓인 사건을 모두 받는다. 기다리지 않는다. 취소했으면 묶음은 버린다.
    pub fn poll(&mut self) -> Vec<SearchEvent> {
        let cancelled = self.is_cancelled();
        self.receiver
            .try_iter()
            .filter(|event| !(cancelled && matches!(event, SearchEvent::Batch { .. })))
            .collect()
    }

    /// 사건 하나를 `timeout`까지 기다려 받는다. 취소했으면 묶음은 버리고 다음 사건을 기다린다. 끝남을 이미 받았으면
    /// 곧바로 `None`이다.
    pub fn wait_event(&mut self, timeout: Duration) -> Option<SearchEvent> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let event = self.receiver.recv_timeout(left).ok()?;
            if self.is_cancelled() && matches!(event, SearchEvent::Batch { .. }) {
                continue;
            }
            return Some(event);
        }
    }
}

impl Drop for LiveSearch {
    fn drop(&mut self) {
        self.shared.cancel();
    }
}

/// 일꾼 하나: 대기열에서 폴더를 꺼내 읽고, 맞는 항목을 보내고, 하위 폴더를 대기열에 더한다. 대기열이 비고 읽는
/// 일꾼도 없거나 취소되면 끝난다.
fn work(shared: &Shared, matcher: &NameMatcher, show_hidden: bool, sender: &Sender<Vec<Found>>) {
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
                    // 다른 일꾼도 깨워 끝나게 한다.
                    shared.changed.notify_all();
                    return;
                }
                queue = shared
                    .changed
                    .wait(queue)
                    .unwrap_or_else(|e| e.into_inner());
            }
        };
        // 다음 폴더를 열기 전에 취소 표지를 본다.
        let (found, subfolders) = if shared.is_cancelled() {
            (Vec::new(), Vec::new())
        } else {
            scan(shared, &folder, matcher, show_hidden)
        };
        if !found.is_empty() {
            // 묶음 스레드가 없어졌으면(일어나지 않음) 버린다.
            let _ = sender.send(found);
        }
        let mut queue = shared.queue();
        queue.folders.extend(subfolders);
        queue.busy -= 1;
        shared.changed.notify_all();
    }
}

/// 폴더 하나를 읽어 맞는 항목과 들어갈 하위 폴더를 낸다. 읽지 못하면 건너뛴 수를 센다.
fn scan(
    shared: &Shared,
    folder: &Path,
    matcher: &NameMatcher,
    show_hidden: bool,
) -> (Vec<Found>, Vec<PathBuf>) {
    let skip = || {
        shared.folders_skipped.fetch_add(1, Ordering::SeqCst);
        (Vec::new(), Vec::new())
    };
    if fault::listing_denied(folder) {
        return skip();
    }
    let Ok(entries) = fs::read_dir(folder) else {
        return skip();
    };
    let shared_folder: Arc<Path> = Arc::from(folder);
    let mut found = Vec::new();
    let mut subfolders = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            // 목록을 읽다 실패했다: 그 폴더는 다 훑지 못했다.
            return skip();
        };
        // 항목 하나의 정보를 읽지 못하면(그 사이 사라짐) 그 항목만 뺀다.
        let (Ok(metadata), Ok(kind)) = (entry.metadata(), entry.file_type()) else {
            continue;
        };
        let Ok(item) =
            crate::file_system::make_item(entry.file_name(), &|| entry.path(), &metadata)
        else {
            continue;
        };
        if !show_hidden && item.attributes.hidden {
            continue;
        }
        // 폴더 심볼릭 링크·정션은 따라 들어가지 않는다(표준 라이브러리는 둘 다 링크로 본다).
        if item.is_folder && !kind.is_symlink() {
            subfolders.push(entry.path());
        }
        if matcher.is_match(&item.name) {
            found.push(Found {
                folder: Arc::clone(&shared_folder),
                item,
            });
        }
    }
    shared.folders_scanned.fetch_add(1, Ordering::SeqCst);
    (found, subfolders)
}

/// 묶음 스레드: 일꾼이 보낸 항목을 모아 [`BATCH_INTERVAL`]마다 한 번까지 보내고, 일꾼이 모두 끝나면 남은 것과
/// 요약을 보낸다.
fn batch(
    shared: &Shared,
    found: &Receiver<Vec<Found>>,
    sender: &Sender<SearchEvent>,
    waker: Option<&Waker>,
    start: Instant,
) {
    let send = |event: SearchEvent| {
        if sender.send(event).is_ok()
            && let Some(waker) = waker
        {
            waker();
        }
    };
    let mut pending: Vec<Found> = Vec::new();
    let mut sent = 0usize;
    let mut last_progress = SearchProgress::default();
    // 마지막으로 묶음을 보낸(깨우기까지 부른) 뒤의 시각. 첫 묶음은 기다리지 않는다.
    let mut last_emit: Option<Instant> = None;
    let mut emit = |pending: &mut Vec<Found>, last_emit: &mut Option<Instant>| {
        let progress = shared.progress(sent + pending.len());
        if pending.is_empty() && progress == last_progress {
            return;
        }
        sent = progress.found;
        last_progress = progress;
        send(SearchEvent::Batch {
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
    // 일꾼이 모두 끝났다. 취소하지 않았으면 남은 것을 간격을 지켜 보낸다.
    if !shared.is_cancelled() {
        if let Some(at) = last_emit {
            thread::sleep((at + BATCH_INTERVAL).saturating_duration_since(Instant::now()));
        }
        if !shared.is_cancelled() {
            emit(&mut pending, &mut last_emit);
        }
    }
    let progress = shared.progress(sent);
    let end = if shared.is_cancelled() {
        SearchEnd::Cancelled
    } else {
        SearchEnd::Finished
    };
    send(SearchEvent::Finished(SearchSummary {
        found: progress.found,
        folders_scanned: progress.folders_scanned,
        folders_skipped: progress.folders_skipped,
        elapsed: start.elapsed(),
        end,
    }));
}

/// 시험용 오류 주입. 앱은 부르지 않는다.
///
/// CI 러너는 백업 권한이 켜진 관리자라 `icacls`로 목록 읽기를 거부해도 읽히므로, 건너뛰기와 그 수를 확인하려면
/// 읽기 실패를 주입한다(`facet_win::file_ops::fault`와 같은 방식).
pub mod fault {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, MutexGuard};

    static DENIED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    /// 주입한 폴더 수. 0이면 잠금 없이 지나간다(평소 탐색은 폴더마다 잠그지 않는다).
    static COUNT: AtomicUsize = AtomicUsize::new(0);

    /// 돌려준 값이 살아 있는 동안 실시간 탐색은 `folder`(그 자신만, 하위는 아님)의 목록을 읽지 못한 것으로 본다.
    #[must_use = "버리면 곧바로 주입이 풀린다"]
    pub fn deny_listing(folder: &Path) -> ListingDenied {
        let folder = folder.to_path_buf();
        lock().push(folder.clone());
        COUNT.fetch_add(1, Ordering::SeqCst);
        ListingDenied(folder)
    }

    /// [`deny_listing`]의 주입. 버리면 풀린다.
    pub struct ListingDenied(PathBuf);

    impl Drop for ListingDenied {
        fn drop(&mut self) {
            let mut folders = lock();
            if let Some(index) = folders.iter().position(|f| *f == self.0) {
                folders.remove(index);
                COUNT.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }

    fn lock() -> MutexGuard<'static, Vec<PathBuf>> {
        DENIED.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn listing_denied(folder: &Path) -> bool {
        COUNT.load(Ordering::SeqCst) > 0 && lock().iter().any(|f| f == folder)
    }
}
