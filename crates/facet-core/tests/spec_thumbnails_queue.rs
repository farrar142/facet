// target: crates/facet-core/tests/spec_thumbnails_queue.rs
// AC: #329: 보이는 요청이 근처 요청보다 먼저(같은 무리는 넘긴 차례), 빠진 요청은 시작 전이면 안 만들고 만드는 중이면 취소(결과 없음)
// AC: #329: 주인을 잊으면 그 주인만 모두 취소, 패닉은 Panicked 실패로 오고 계속 돎, 실패한 키는 다시 만들지 않고 같은 실패가 옴
// AC: #329: 취소된 키는 다시 만듦, 계속 원하는 동안 낸 결과는 다시 만들지 않음, 작업자 수만큼 나란히, 작업자 수 규칙, 깨우기, 원하기는 기다리지 않음
//! 썸네일 요청 대기열(`thumbnail_queue::ThumbnailQueue`)을 주입한 만드는 함수로 확인한다. 만드는 함수는 이름으로 동작을
//! 고른다: `gate`로 시작하면 풀어 줄 때까지(또는 취소 표지가 설 때까지) 기다리고, `panic`이면 패닉하고, `bad`면
//! 깨진 파일 실패, 그 밖은 1×1 그림. 파일을 읽지 않으므로 모든 플랫폼에서 돈다.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

use facet_core::thumbnail::{Thumbnail, ThumbnailError, ThumbnailKey};
use facet_core::thumbnail_queue::{MAX_WORKERS, ThumbnailDone, ThumbnailQueue, worker_count};

/// CI 러너가 느려도 흔들리지 않게 넉넉히 기다린다. 조건이 맞으면 곧바로 끝난다.
const WAIT: Duration = Duration::from_secs(20);
/// 결과가 「오지 않음」을 볼 때 기다리는 시간.
const QUIET: Duration = Duration::from_millis(300);

fn key(name: &str) -> ThumbnailKey {
    ThumbnailKey {
        path: PathBuf::from(name),
        side: 96,
        modified: UNIX_EPOCH,
        file_size: 1,
    }
}

fn name(key: &ThumbnailKey) -> String {
    key.path.to_string_lossy().into_owned()
}

fn tiny() -> Thumbnail {
    Thumbnail {
        width: 1,
        height: 1,
        rgba: vec![1, 2, 3, 255],
    }
}

/// 만드는 함수가 남기는 기록.
#[derive(Default)]
struct Log {
    started: Mutex<Vec<String>>,
    cancelled: Mutex<Vec<String>>,
    running: AtomicUsize,
    peak: AtomicUsize,
    release: AtomicBool,
}

impl Log {
    fn started(&self) -> Vec<String> {
        self.started.lock().unwrap().clone()
    }

    fn cancelled(&self) -> Vec<String> {
        self.cancelled.lock().unwrap().clone()
    }

    fn starts_of(&self, name: &str) -> usize {
        self.started().iter().filter(|n| *n == name).count()
    }
}

fn make_queue(workers: usize) -> (ThumbnailQueue, Arc<Log>) {
    let log = Arc::new(Log::default());
    let shared = Arc::clone(&log);
    let queue = ThumbnailQueue::new(workers, move |key: &ThumbnailKey, cancel: &AtomicBool| {
        let n = name(key);
        shared.started.lock().unwrap().push(n.clone());
        let now = shared.running.fetch_add(1, Ordering::SeqCst) + 1;
        shared.peak.fetch_max(now, Ordering::SeqCst);
        let result = if n.starts_with("gate") {
            loop {
                if cancel.load(Ordering::SeqCst) {
                    shared.cancelled.lock().unwrap().push(n.clone());
                    break Err(ThumbnailError::Cancelled);
                }
                if shared.release.load(Ordering::SeqCst) {
                    break Ok(tiny());
                }
                thread::sleep(Duration::from_millis(2));
            }
        } else if n.starts_with("panic") {
            shared.running.fetch_sub(1, Ordering::SeqCst);
            panic!("시험용 패닉: {n}");
        } else if n.starts_with("bad") {
            Err(ThumbnailError::Corrupt)
        } else {
            Ok(tiny())
        };
        shared.running.fetch_sub(1, Ordering::SeqCst);
        result
    });
    (queue, log)
}

fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !condition() {
        assert!(Instant::now() < deadline, "{what}: 시간 안에 되지 않았다");
        thread::sleep(Duration::from_millis(2));
    }
}

/// 결과를 `count`개 받는다.
fn collect(queue: &ThumbnailQueue, count: usize) -> Vec<ThumbnailDone> {
    let deadline = Instant::now() + WAIT;
    let mut out = Vec::new();
    while out.len() < count {
        let left = deadline.saturating_duration_since(Instant::now());
        match queue.wait_event(left) {
            Some(done) => out.push(done),
            None => panic!("결과 {count}개 가운데 {}개만 왔다", out.len()),
        }
    }
    out
}

/// 잠시 기다린 뒤 그사이 온 결과.
fn quiet(queue: &ThumbnailQueue) -> Vec<ThumbnailDone> {
    thread::sleep(QUIET);
    queue.poll()
}

fn names(done: &[ThumbnailDone]) -> Vec<String> {
    done.iter().map(|d| name(&d.key)).collect()
}

#[test]
#[ignore = "#329"]
fn 보이는_요청이_먼저_들어온_근처_요청보다_먼저_시작한다() {
    let (queue, log) = make_queue(1);
    queue.want(1, vec![key("gate")], vec![key("n1"), key("n2")]);
    wait_until("gate 시작", || log.started() == ["gate"]);
    // 스크롤: v1이 화면에 들어왔다. n1·n2는 이미 기다리던 근처 요청이다.
    queue.want(1, vec![key("gate"), key("v1")], vec![key("n1"), key("n2")]);
    log.release.store(true, Ordering::SeqCst);
    let done = collect(&queue, 4);
    assert_eq!(log.started(), ["gate", "v1", "n1", "n2"]);
    assert!(
        log.cancelled().is_empty(),
        "계속 원한 gate는 취소되지 않는다"
    );
    for d in &done {
        assert_eq!(d.owner, 1);
        assert_eq!(*d.result.as_ref().expect("만들었다").as_ref(), tiny());
    }
}

#[test]
#[ignore = "#329"]
fn 같은_무리_안에서는_넘긴_차례로_만든다() {
    let (queue, log) = make_queue(1);
    queue.want(1, vec![key("gate")], vec![]);
    wait_until("gate 시작", || log.started() == ["gate"]);
    queue.want(
        1,
        vec![key("gate"), key("v1"), key("v2"), key("v3")],
        vec![key("n1"), key("n2"), key("n3")],
    );
    log.release.store(true, Ordering::SeqCst);
    collect(&queue, 7);
    assert_eq!(log.started(), ["gate", "v1", "v2", "v3", "n1", "n2", "n3"]);
}

#[test]
#[ignore = "#329"]
fn 빠진_요청은_시작_전이면_만들지_않고_만드는_중이면_취소되며_결과를_내지_않는다() {
    let (queue, log) = make_queue(1);
    queue.want(1, vec![key("gate")], vec![key("p1"), key("p2")]);
    wait_until("gate 시작", || log.started() == ["gate"]);
    // 스크롤로 모두 화면 밖으로 나가고 x만 보인다.
    queue.want(1, vec![key("x")], vec![]);
    wait_until("gate 취소 표지", || log.cancelled() == ["gate"]);
    let done = collect(&queue, 1);
    assert_eq!(names(&done), ["x"]);
    assert!(quiet(&queue).is_empty(), "취소·제외된 요청은 결과가 없다");
    assert_eq!(log.started(), ["gate", "x"], "p1·p2는 만들지 않았다");
}

#[test]
#[ignore = "#329"]
fn 주인을_잊으면_그_주인의_요청만_모두_취소된다() {
    let (queue, log) = make_queue(1);
    queue.want(1, vec![key("gate")], vec![key("p1"), key("p2")]);
    wait_until("gate 시작", || log.started() == ["gate"]);
    queue.want(2, vec![key("o1")], vec![key("o2")]);
    queue.forget(1);
    wait_until("gate 취소 표지", || log.cancelled() == ["gate"]);
    let done = collect(&queue, 2);
    assert_eq!(names(&done), ["o1", "o2"]);
    assert!(
        done.iter().all(|d| d.owner == 2),
        "다른 주인의 요청은 그대로"
    );
    assert!(quiet(&queue).is_empty());
    assert_eq!(
        log.started(),
        ["gate", "o1", "o2"],
        "잊은 주인의 남은 요청은 만들지 않았다"
    );
}

#[test]
#[ignore = "#329"]
fn 만드는_함수가_패닉하면_panicked_실패로_오고_뒤_요청도_만든다() {
    let (queue, log) = make_queue(1);
    queue.want(1, vec![key("panic1"), key("after1")], vec![key("after2")]);
    let done = collect(&queue, 3);
    let panicked = done.iter().find(|d| name(&d.key) == "panic1").unwrap();
    assert_eq!(
        panicked.result.as_ref().err(),
        Some(&ThumbnailError::Panicked)
    );
    for after in ["after1", "after2"] {
        let d = done.iter().find(|d| name(&d.key) == after).unwrap();
        assert!(d.result.is_ok(), "{after}");
    }
    // 작업자는 계속 돈다.
    queue.want(1, vec![key("later")], vec![]);
    assert_eq!(names(&collect(&queue, 1)), ["later"]);
    assert_eq!(log.starts_of("panic1"), 1);
}

#[test]
#[ignore = "#329"]
fn 실패한_키는_다시_만들지_않고_같은_실패가_곧바로_온다() {
    let (queue, log) = make_queue(1);
    queue.want(1, vec![key("bad1")], vec![]);
    let first = collect(&queue, 1);
    assert_eq!(
        first[0].result.as_ref().err(),
        Some(&ThumbnailError::Corrupt)
    );

    queue.want(1, vec![], vec![]);
    queue.want(1, vec![key("bad1")], vec![]);
    let again = collect(&queue, 1);
    assert_eq!(
        again[0].result.as_ref().err(),
        Some(&ThumbnailError::Corrupt)
    );

    queue.want(2, vec![key("bad1")], vec![]);
    let other = collect(&queue, 1);
    assert_eq!(other[0].owner, 2);
    assert_eq!(
        other[0].result.as_ref().err(),
        Some(&ThumbnailError::Corrupt)
    );
    assert_eq!(log.starts_of("bad1"), 1, "실패는 그 키로 기억한다");
}

#[test]
#[ignore = "#329"]
fn 취소된_키는_다시_원하면_다시_만든다() {
    let (queue, log) = make_queue(1);
    queue.want(1, vec![key("gate")], vec![]);
    wait_until("gate 시작", || log.started() == ["gate"]);
    queue.want(1, vec![], vec![]);
    wait_until("gate 취소 표지", || log.cancelled() == ["gate"]);
    log.release.store(true, Ordering::SeqCst);
    queue.want(1, vec![key("gate")], vec![]);
    let done = collect(&queue, 1);
    assert!(done[0].result.is_ok(), "취소는 실패로 기억하지 않는다");
    assert_eq!(log.starts_of("gate"), 2);
}

#[test]
#[ignore = "#329"]
fn 계속_원하는_동안_이미_낸_결과는_다시_만들지_않는다() {
    let (queue, log) = make_queue(1);
    queue.want(1, vec![key("a")], vec![key("b")]);
    collect(&queue, 2);
    // 같은 화면에서 다시 원한다(스크롤 없이 다시 그림 등).
    queue.want(1, vec![key("a")], vec![key("b")]);
    queue.want(1, vec![key("b")], vec![key("a")]);
    assert!(quiet(&queue).is_empty(), "결과를 다시 내지 않는다");
    assert_eq!(log.started(), ["a", "b"]);
}

#[test]
#[ignore = "#329"]
fn 작업자_수만큼_나란히_만든다() {
    let (queue, log) = make_queue(2);
    queue.want(1, vec![key("gate1"), key("gate2"), key("gate3")], vec![]);
    wait_until("둘이 나란히", || {
        log.running.load(Ordering::SeqCst) == 2
    });
    thread::sleep(QUIET);
    assert_eq!(
        log.running.load(Ordering::SeqCst),
        2,
        "작업자 둘을 넘지 않는다"
    );
    log.release.store(true, Ordering::SeqCst);
    collect(&queue, 3);
    assert_eq!(log.peak.load(Ordering::SeqCst), 2);

    let (queue, log) = make_queue(3);
    queue.want(1, vec![key("gate1"), key("gate2"), key("gate3")], vec![]);
    wait_until("셋이 나란히", || {
        log.running.load(Ordering::SeqCst) == 3
    });
    log.release.store(true, Ordering::SeqCst);
    collect(&queue, 3);
}

#[test]
#[ignore = "#329"]
fn 작업자_수는_코어_수_빼기_하나이고_1에서_8_사이다() {
    assert_eq!(MAX_WORKERS, 8);
    for (cores, workers) in [(1, 1), (2, 1), (4, 3), (8, 7), (9, 8), (16, 8), (64, 8)] {
        assert_eq!(worker_count(cores), workers, "코어 {cores}개");
    }
}

#[test]
#[ignore = "#329"]
fn 결과가_오면_깨우기를_부른다() {
    let (queue, _log) = make_queue(1);
    let woken = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&woken);
    queue.set_waker(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    queue.want(7, vec![key("a")], vec![]);
    wait_until("깨우기", || woken.load(Ordering::SeqCst) > 0);
    let done = queue.poll();
    assert_eq!(names(&done), ["a"]);
    assert_eq!(done[0].owner, 7);
}

#[test]
#[ignore = "#329"]
fn 원하기와_잊기는_부르는_스레드를_기다리게_하지_않는다() {
    let (queue, log) = make_queue(1);
    queue.want(1, vec![key("gate")], vec![]);
    wait_until("gate 시작", || log.started() == ["gate"]);
    let many: Vec<ThumbnailKey> = (0..2_000).map(|i| key(&format!("{i}.jpg"))).collect();
    let started = Instant::now();
    queue.want(1, many[..100].to_vec(), many[100..].to_vec());
    queue.want(1, many[1_000..1_100].to_vec(), many[..1_000].to_vec());
    queue.forget(1);
    let took = started.elapsed();
    assert!(
        took < Duration::from_millis(200),
        "작업자가 막혀 있어도 곧바로 돌아온다: {took:?}"
    );
    wait_until("gate 취소 표지", || log.cancelled() == ["gate"]);
}
