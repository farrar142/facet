// target: crates/facet-core/tests/spec_preview_requests.rs
// AC: #338: 50ms 기다림과 다시 기다림·선택 없음, 마지막 요청의 결과만과 취소 표지, 붙잡힌 앞선 요청과 늦은 결과 버림, 곧바로 돌아옴, 깨우기, 패닉은 실패, 같은 항목 다시 요청
//! 미리보기 요청기(`facet_core::preview::Previewer`)의 기다림·취소·늦은 결과 버림을 주입한 만드는 함수와 주입한 시계
//! (`Instant`을 넘김)로 확인한다. 파일을 읽지 않으며 모든 환경에서 돈다.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use facet_core::preview::{
    PREVIEW_DELAY, PreviewBody, PreviewDone, PreviewId, PreviewProblem, PreviewRequest, Previewer,
};
use facet_core::{Attributes, Item};

/// CI 러너가 느려도 흔들리지 않게 넉넉히 기다린다. 결과가 오면 곧바로 끝난다.
const WAIT: Duration = Duration::from_secs(10);
/// 더 오면 안 되는 결과를 기다려 보는 시간.
const QUIET: Duration = Duration::from_millis(300);

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn request(name: &str) -> PreviewRequest {
    PreviewRequest {
        path: format!(r"C:\시험\{name}").into(),
        item: Item {
            name: name.into(),
            is_folder: false,
            size: 10,
            modified: SystemTime::UNIX_EPOCH,
            attributes: Attributes::default(),
        },
        show_hidden: false,
        image_side: 256,
    }
}

fn name_of(request: &PreviewRequest) -> String {
    request.item.name.to_string_lossy().into_owned()
}

/// 이름을 글자로 담은 「정보만」이 아닌 결과를 바로 낸다(어느 요청의 결과인지 가르려고 이름을 넣는다).
fn echo(request: &PreviewRequest, _cancel: &AtomicBool) -> PreviewBody {
    PreviewBody::Unavailable(PreviewProblem::Io(match name_of(request).as_str() {
        "a" => std::io::ErrorKind::NotFound,
        "b" => std::io::ErrorKind::PermissionDenied,
        "c" => std::io::ErrorKind::AlreadyExists,
        _ => std::io::ErrorKind::Other,
    }))
}

fn echo_of(name: &str) -> PreviewBody {
    echo(&request(name), &AtomicBool::new(false))
}

/// 결과 하나를 기다린다.
fn next(previewer: &mut Previewer) -> PreviewDone {
    previewer.wait_event(WAIT).expect("결과가 와야 한다")
}

/// `QUIET` 동안 결과가 오지 않는가.
fn quiet(previewer: &mut Previewer) -> bool {
    let deadline = Instant::now() + QUIET;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        if previewer.wait_event(left).is_some() {
            return false;
        }
    }
}

/// 요청을 내고(기다림이 끝난 시각으로) 그 번호를 돌려준다.
fn fire(previewer: &mut Previewer, name: &str, now: Instant) -> PreviewId {
    previewer.request(Some(request(name)), now);
    previewer
        .tick(now + PREVIEW_DELAY)
        .expect("기다림이 끝나면 낸다")
}

#[test]
#[ignore = "#338"]
fn the_delay_is_fifty_milliseconds() {
    assert_eq!(PREVIEW_DELAY, ms(50));
}

#[test]
#[ignore = "#338"]
fn a_request_waits_for_the_delay_before_it_is_made() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut previewer = {
        let calls = Arc::clone(&calls);
        Previewer::new(move |r, c| {
            calls.fetch_add(1, Ordering::SeqCst);
            echo(r, c)
        })
    };
    let t0 = Instant::now();
    assert_eq!(previewer.due(), None, "요청이 없으면 낼 것도 없다");
    previewer.request(Some(request("a")), t0);
    assert_eq!(previewer.due(), Some(t0 + PREVIEW_DELAY));
    assert_eq!(previewer.tick(t0 + ms(49)), None, "50ms 전에는 내지 않는다");
    assert!(quiet(&mut previewer));
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let id = previewer.tick(t0 + ms(50)).expect("50ms가 되면 낸다");
    let done = next(&mut previewer);
    assert_eq!(done.id, id);
    assert_eq!(name_of(&done.request), "a");
    assert_eq!(done.body, echo_of("a"));
    assert_eq!(previewer.due(), None);
    assert_eq!(previewer.tick(t0 + ms(500)), None, "한 번만 낸다");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
#[ignore = "#338"]
fn a_new_selection_restarts_the_wait_and_only_the_last_is_made() {
    let made = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut previewer = {
        let made = Arc::clone(&made);
        Previewer::new(move |r, c| {
            made.lock().unwrap().push(name_of(r));
            echo(r, c)
        })
    };
    let t0 = Instant::now();
    previewer.request(Some(request("a")), t0);
    previewer.request(Some(request("b")), t0 + ms(30));
    assert_eq!(
        previewer.due(),
        Some(t0 + ms(80)),
        "마지막 요청부터 다시 기다린다"
    );
    assert_eq!(previewer.tick(t0 + ms(60)), None);
    let id = previewer.tick(t0 + ms(80)).expect("낸다");
    let done = next(&mut previewer);
    assert_eq!((done.id, name_of(&done.request)), (id, "b".to_string()));
    assert!(quiet(&mut previewer));
    assert_eq!(*made.lock().unwrap(), ["b"], "기다리던 a는 만들지 않는다");
}

#[test]
#[ignore = "#338"]
fn clearing_the_selection_drops_the_waiting_request() {
    let mut previewer = Previewer::new(echo);
    let t0 = Instant::now();
    previewer.request(Some(request("a")), t0);
    previewer.request(None, t0 + ms(10));
    assert_eq!(previewer.due(), None);
    assert_eq!(previewer.tick(t0 + ms(200)), None);
    assert!(quiet(&mut previewer));
}

/// 표지가 서거나 `release`가 서면 끝나는 만드는 함수. 끝날 때 표지를 보았는지 적는다.
fn gated(
    release: Arc<AtomicBool>,
    seen: Arc<Mutex<HashMap<String, bool>>>,
) -> impl Fn(&PreviewRequest, &AtomicBool) -> PreviewBody + Send + Sync + 'static {
    move |r, cancel| {
        let deadline = Instant::now() + WAIT;
        while !cancel.load(Ordering::SeqCst)
            && !release.load(Ordering::SeqCst)
            && Instant::now() < deadline
        {
            std::thread::sleep(ms(2));
        }
        seen.lock()
            .unwrap()
            .insert(name_of(r), cancel.load(Ordering::SeqCst));
        echo(r, cancel)
    }
}

#[test]
#[ignore = "#338"]
fn only_the_last_of_rapid_requests_comes_back_and_earlier_ones_are_cancelled() {
    let release = Arc::new(AtomicBool::new(false));
    let seen = Arc::new(Mutex::new(HashMap::new()));
    let mut previewer = Previewer::new(gated(Arc::clone(&release), Arc::clone(&seen)));
    let t0 = Instant::now();
    let _a = fire(&mut previewer, "a", t0);
    let _b = fire(&mut previewer, "b", t0 + ms(100));
    let c = fire(&mut previewer, "c", t0 + ms(200));
    release.store(true, Ordering::SeqCst);

    let done = next(&mut previewer);
    assert_eq!(done.id, c, "마지막 요청의 결과만 온다");
    assert_eq!(done.body, echo_of("c"));
    assert!(quiet(&mut previewer), "앞선 요청의 결과는 오지 않는다");

    let deadline = Instant::now() + WAIT;
    while seen.lock().unwrap().len() < 3 && Instant::now() < deadline {
        std::thread::sleep(ms(10));
    }
    let seen = seen.lock().unwrap();
    assert_eq!(seen.get("a"), Some(&true), "a는 취소 표지를 받았다");
    assert_eq!(seen.get("b"), Some(&true), "b는 취소 표지를 받았다");
    assert_eq!(seen.get("c"), Some(&false), "c는 취소되지 않았다");
}

#[test]
#[ignore = "#338"]
fn a_stuck_earlier_request_does_not_hold_up_the_next_one() {
    let release_a = Arc::new(AtomicBool::new(false));
    let mut previewer = {
        let release_a = Arc::clone(&release_a);
        Previewer::new(move |r, c| {
            if name_of(r) == "a" {
                // 표지를 무시하고 붙잡혀 있다(멈춘 디코더·처리기).
                let deadline = Instant::now() + WAIT;
                while !release_a.load(Ordering::SeqCst) && Instant::now() < deadline {
                    std::thread::sleep(ms(2));
                }
            }
            echo(r, c)
        })
    };
    let t0 = Instant::now();
    let _a = fire(&mut previewer, "a", t0);
    std::thread::sleep(ms(50));
    let b = fire(&mut previewer, "b", t0 + ms(100));
    let done = next(&mut previewer);
    assert_eq!(done.id, b, "a가 붙잡혀 있어도 b의 결과가 온다");
    assert_eq!(done.body, echo_of("b"));

    release_a.store(true, Ordering::SeqCst);
    assert!(quiet(&mut previewer), "뒤늦게 끝난 a의 결과는 버린다");
}

#[test]
#[ignore = "#338"]
fn requesting_and_ticking_return_at_once_even_when_making_is_slow() {
    let mut previewer = Previewer::new(|r, c| {
        std::thread::sleep(ms(1500));
        echo(r, c)
    });
    let t0 = Instant::now();
    let started = Instant::now();
    for (i, name) in ["a", "b", "c", "d"].into_iter().enumerate() {
        let now = t0 + ms(100) * i as u32;
        previewer.request(Some(request(name)), now);
        previewer.tick(now + PREVIEW_DELAY).expect("낸다");
    }
    assert!(
        started.elapsed() < ms(500),
        "부르는 스레드는 만들기를 기다리지 않는다: {:?}",
        started.elapsed()
    );
    let done = next(&mut previewer);
    assert_eq!(name_of(&done.request), "d");
}

#[test]
#[ignore = "#338"]
fn the_waker_is_called_when_a_result_is_ready() {
    let mut previewer = Previewer::new(echo);
    let woken = Arc::new(AtomicUsize::new(0));
    {
        let woken = Arc::clone(&woken);
        previewer.set_waker(move || {
            woken.fetch_add(1, Ordering::SeqCst);
        });
    }
    fire(&mut previewer, "a", Instant::now());
    let deadline = Instant::now() + WAIT;
    while woken.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "깨우기가 불리지 않았다");
        std::thread::sleep(ms(5));
    }
    let results = previewer.poll();
    assert_eq!(results.len(), 1);
    assert_eq!(name_of(&results[0].request), "a");
}

#[test]
#[ignore = "#338"]
fn a_panicking_maker_gives_a_failure_and_the_previewer_keeps_working() {
    let mut previewer = Previewer::new(|r, c| {
        if name_of(r) == "boom" {
            panic!("디코더가 터졌다");
        }
        echo(r, c)
    });
    let t0 = Instant::now();
    let boom = fire(&mut previewer, "boom", t0);
    let done = next(&mut previewer);
    assert_eq!(done.id, boom);
    assert_eq!(done.body, PreviewBody::Unavailable(PreviewProblem::Failed));

    let a = fire(&mut previewer, "a", t0 + ms(100));
    let done = next(&mut previewer);
    assert_eq!((done.id, done.body), (a, echo_of("a")));
}

#[test]
#[ignore = "#338"]
fn requesting_the_same_item_again_makes_it_again() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut previewer = {
        let calls = Arc::clone(&calls);
        Previewer::new(move |r, c| {
            calls.fetch_add(1, Ordering::SeqCst);
            echo(r, c)
        })
    };
    let t0 = Instant::now();
    let first = fire(&mut previewer, "a", t0);
    assert_eq!(next(&mut previewer).id, first);
    let second = fire(&mut previewer, "a", t0 + ms(100));
    assert_ne!(first, second, "새 요청 번호");
    let done = next(&mut previewer);
    assert_eq!((done.id, name_of(&done.request)), (second, "a".to_string()));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
