//! 단일 실행: 주인 되기(뮤텍스)·받기(파이프 수신기)·넘기기를 실제 Windows 객체로 확인한다.
//!
//! 테스트마다 다른 이름([`InstanceName::new`])을 써서 서로, 그리고 떠 있는 facet과 섞이지 않는다.
#![cfg(windows)]

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use facet_win::single_instance::{
    Claim, ForwardError, InstanceName, Launch, Owner, claim, encode_launch, forward,
};

/// 이 테스트 프로세스와 테스트 안에서 겹치지 않는 시험용 이름.
fn test_name(label: &str) -> InstanceName {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    InstanceName::new(&format!("test-{label}-{}-{n}", std::process::id())).unwrap()
}

fn owner(name: &InstanceName) -> Owner {
    match claim(name).unwrap() {
        Claim::Owner(owner) => owner,
        Claim::Taken => panic!("주인이 되어야 한다"),
    }
}

fn listening(name: &InstanceName) -> (Owner, mpsc::Receiver<Launch>) {
    let mut owner = owner(name);
    let (sender, receiver) = mpsc::channel();
    owner
        .listen(move |launch| {
            let _ = sender.send(launch);
        })
        .unwrap();
    (owner, receiver)
}

fn sample_launch() -> Launch {
    Launch {
        working_dir: PathBuf::from(r"C:\작업 폴더\공백 있는 곳"),
        paths: vec![
            PathBuf::from(r"C:\Users\사용자\내 문서"),
            PathBuf::from(r"D:\Program Files\프로그램"),
            PathBuf::from(r"상대\경로 하나"),
            PathBuf::from(r"\\server\공유 폴더\😀"),
        ],
    }
}

const WAIT: Duration = Duration::from_secs(10);

#[test]
fn 두_번째_쪽이_넘긴_경로와_작업_폴더를_수신기가_그대로_받는다() {
    let name = test_name("forward");
    let (_owner, received) = listening(&name);
    assert!(matches!(claim(&name).unwrap(), Claim::Taken));

    let launch = sample_launch();
    forward(&name, &launch, WAIT).unwrap();
    assert_eq!(received.recv_timeout(WAIT).unwrap(), launch);

    // 인자 없는 실행도 넘긴다.
    let bare = Launch {
        working_dir: PathBuf::from(r"C:\"),
        paths: Vec::new(),
    };
    forward(&name, &bare, WAIT).unwrap();
    assert_eq!(received.recv_timeout(WAIT).unwrap(), bare);
}

#[test]
fn 여러_쪽이_한꺼번에_넘겨도_모두_받는다() {
    let name = test_name("many");
    let (_owner, received) = listening(&name);
    let senders: Vec<_> = (0..8)
        .map(|i| {
            let name = name.clone();
            thread::spawn(move || {
                let launch = Launch {
                    working_dir: PathBuf::from(format!(r"C:\폴더 {i}")),
                    paths: vec![PathBuf::from(format!("경로 {i}"))],
                };
                forward(&name, &launch, WAIT).unwrap();
            })
        })
        .collect();
    for sender in senders {
        sender.join().unwrap();
    }
    let mut dirs: Vec<_> = (0..8)
        .map(|_| received.recv_timeout(WAIT).unwrap().working_dir)
        .collect();
    dirs.sort();
    let mut expected: Vec<_> = (0..8)
        .map(|i| PathBuf::from(format!(r"C:\폴더 {i}")))
        .collect();
    expected.sort();
    assert_eq!(dirs, expected);
}

#[test]
fn 두_쪽이_동시에_주인_되기를_시도해도_주인은_하나다() {
    let name = test_name("race");
    let tries = 16;
    let barrier = Arc::new(Barrier::new(tries));
    let threads: Vec<_> = (0..tries)
        .map(|_| {
            let name = name.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                claim(&name).unwrap()
            })
        })
        .collect();
    // 모두 끝날 때까지 결과(주인 자리)를 쥐고 있어야 공정하다.
    let claims: Vec<Claim> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    let owners = claims
        .iter()
        .filter(|c| matches!(c, Claim::Owner(_)))
        .count();
    assert_eq!(owners, 1);
}

/// 자식 프로세스로 불릴 때만 일한다: 이름을 받아 주인 되기를 시도하고 결과를 찍은 뒤 잠깐 쥐고 있는다.
#[test]
fn 자식_프로세스_주인_되기() {
    let Ok(base) = std::env::var("FACET_TEST_CLAIM_NAME") else {
        return;
    };
    let start: u64 = std::env::var("FACET_TEST_CLAIM_AT")
        .unwrap()
        .parse()
        .unwrap();
    // 모든 자식이 같은 순간에 시도하도록 정한 시각(유닉스 밀리초)까지 기다린다.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    if start > now {
        thread::sleep(Duration::from_millis(start - now));
    }
    let name = InstanceName::new(&base).unwrap();
    let result = claim(&name).unwrap();
    let label = match result {
        Claim::Owner(_) => "owner",
        Claim::Taken => "taken",
    };
    println!("FACET-CLAIM={label}");
    thread::sleep(Duration::from_millis(1500));
    drop(result);
}

#[test]
fn 프로세스_여럿이_동시에_시도해도_주인은_하나다() {
    let name = test_name("procs");
    let exe = std::env::current_exe().unwrap();
    let start = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 1500;
    let children: Vec<_> = (0..4)
        .map(|_| {
            Command::new(&exe)
                .args([
                    "--exact",
                    "자식_프로세스_주인_되기",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("FACET_TEST_CLAIM_NAME", name.as_str())
                .env("FACET_TEST_CLAIM_AT", start.to_string())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let mut owners = 0;
    let mut taken = 0;
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        owners += stdout.matches("FACET-CLAIM=owner").count();
        taken += stdout.matches("FACET-CLAIM=taken").count();
    }
    assert_eq!((owners, taken), (1, 3));
}

#[test]
fn 주인이_막_뜨는_중이면_넘기는_쪽이_다시_시도해_닿는다() {
    let name = test_name("late");
    let sender = {
        let name = name.clone();
        thread::spawn(move || forward(&name, &sample_launch(), WAIT))
    };
    thread::sleep(Duration::from_millis(300));
    let (_owner, received) = listening(&name);
    sender.join().unwrap().unwrap();
    assert_eq!(received.recv_timeout(WAIT).unwrap(), sample_launch());
}

#[test]
fn 주인이_없으면_정한_시간_뒤에_넘기지_못했다고_돌려준다() {
    let name = test_name("none");
    let started = Instant::now();
    let result = forward(&name, &sample_launch(), Duration::from_millis(200));
    assert!(matches!(result, Err(ForwardError::NoOwner)), "{result:?}");
    assert!(started.elapsed() >= Duration::from_millis(200));
    assert!(started.elapsed() < WAIT);

    // 주인 자리만 있고 수신기가 아직 없어도 같다. 부르는 쪽이 스스로 주인이 될지 정한다.
    let _owner = owner(&name);
    let result = forward(&name, &sample_launch(), Duration::from_millis(100));
    assert!(matches!(result, Err(ForwardError::NoOwner)), "{result:?}");
}

fn raw_exchange(name: &InstanceName, frame: &[u8]) -> Vec<u8> {
    let mut pipe = OpenOptions::new()
        .read(true)
        .write(true)
        .open(name.pipe_path().unwrap())
        .unwrap();
    pipe.write_all(frame).unwrap();
    let mut reply = Vec::new();
    let mut byte = [0u8; 1];
    if let Ok(1) = pipe.read(&mut byte) {
        reply.push(byte[0]);
    }
    reply
}

#[test]
fn 깨지거나_너무_큰_메시지는_버리고_수신기는_계속_받는다() {
    let name = test_name("broken");
    let (_owner, received) = listening(&name);

    // 매직이 틀림.
    let garbage = b"NOPE-not-a-launch";
    let mut frame = (garbage.len() as u32).to_le_bytes().to_vec();
    frame.extend_from_slice(garbage);
    assert_eq!(raw_exchange(&name, &frame), vec![0]);

    // 모르는 버전.
    let mut body = encode_launch(&sample_launch()).unwrap();
    body[4..6].copy_from_slice(&99u16.to_le_bytes());
    let mut frame = (body.len() as u32).to_le_bytes().to_vec();
    frame.extend_from_slice(&body);
    assert_eq!(raw_exchange(&name, &frame), vec![0]);

    // 너무 큰 길이: 본문을 읽지 않고 버린다.
    assert_eq!(raw_exchange(&name, &u32::MAX.to_le_bytes()), vec![0]);

    // 길이만 보내고 끊음.
    {
        let mut pipe = OpenOptions::new()
            .write(true)
            .read(true)
            .open(name.pipe_path().unwrap())
            .unwrap();
        pipe.write_all(&100u32.to_le_bytes()).unwrap();
    }

    assert!(received.recv_timeout(Duration::from_millis(200)).is_err());
    forward(&name, &sample_launch(), WAIT).unwrap();
    assert_eq!(received.recv_timeout(WAIT).unwrap(), sample_launch());
    assert!(received.try_recv().is_err(), "깨진 메시지는 넘기지 않는다");
}

#[test]
fn 주인_자리를_놓으면_다음_쪽이_주인이_된다() {
    let name = test_name("release");
    let (first, _received) = listening(&name);
    assert!(matches!(claim(&name).unwrap(), Claim::Taken));
    drop(first);
    let (_second, received) = listening(&name);
    forward(&name, &sample_launch(), WAIT).unwrap();
    assert_eq!(received.recv_timeout(WAIT).unwrap(), sample_launch());
}

#[test]
fn 이름이_다르면_서로_섞이지_않는다() {
    let a = test_name("mix-a");
    let b = test_name("mix-b");
    let (_a, received_a) = listening(&a);
    let (_b, received_b) = listening(&b);
    assert_ne!(a.pipe_path().unwrap(), b.pipe_path().unwrap());
    forward(&b, &sample_launch(), WAIT).unwrap();
    assert_eq!(received_b.recv_timeout(WAIT).unwrap(), sample_launch());
    assert!(received_a.recv_timeout(Duration::from_millis(200)).is_err());
    // 실제 facet의 이름과도 다르다.
    assert_ne!(
        a.pipe_path().unwrap(),
        InstanceName::facet().pipe_path().unwrap()
    );
}

#[test]
fn 수신기는_한_번만_연다() {
    let name = test_name("twice");
    let (mut owner, _received) = listening(&name);
    assert!(owner.listen(|_| {}).is_err());
}
