//! 볼륨 붙이기·떼기([`Mounts::sync`])를 실제 볼륨 없이 확인한다. 볼륨 읽기는 테스트가 인덱스를
//! 넘겨 주는 가짜 [`Source`]다. 관리자 권한이 필요 없다.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use facet_index::{ATTRIBUTE_DIRECTORY, Index, NewItem};
use facet_indexer::follow::{JournalNow, Records, Source, Stop};
use facet_indexer::mounts::Mounts;
use facet_indexer::session::Cancel;
use facet_indexer::volumes::{Plan, Volumes};
use facet_protocol::{
    Case, Condition, ErrorKind, NoIndexReason, QueryMessage, VolumeState, VolumeStatus,
};

const ROOT: u64 = 5;
const POLL: Duration = Duration::from_millis(2);
const PATIENCE: Duration = Duration::from_secs(10);

/// 볼륨 `volume`의 루트 아래 파일 `name` 하나.
fn index(volume: &str, name: &str) -> Index {
    let mut index = Index::new(volume, ROOT);
    index
        .insert(NewItem {
            number: 40,
            parent: ROOT,
            name: "문서",
            size: 0,
            modified: 1,
            attributes: ATTRIBUTE_DIRECTORY,
        })
        .unwrap();
    index
        .insert(NewItem {
            number: 41,
            parent: 40,
            name,
            size: 10,
            modified: 1,
            attributes: 0x20,
        })
        .unwrap();
    index
}

/// 가짜 볼륨. 인덱스를 만들 때마다 테스트가 그 볼륨 앞으로 보내 주는 결과를 기다린다. 저널은
/// 늘 같고 레코드가 없다. `gone`이 서면 볼륨이 빠진 것처럼 읽기가 모두 실패한다.
struct Fake {
    name: String,
    builds: Receiver<Result<Index, String>>,
    gone: Arc<Mutex<bool>>,
    /// 인덱스를 만들기 시작한 볼륨 이름을 적는다.
    started: Arc<Mutex<Vec<String>>>,
}

impl Source for Fake {
    fn journal(&mut self) -> Result<JournalNow, String> {
        if *self.gone.lock().unwrap() {
            return Err("볼륨이 없음".into());
        }
        Ok(JournalNow {
            journal_id: 1,
            first_usn: 0,
            next_usn: 100,
        })
    }

    fn read(&mut self, _journal_id: u64, start: i64) -> Result<Records, String> {
        if *self.gone.lock().unwrap() {
            return Err("볼륨이 없음".into());
        }
        Ok(Records {
            next_usn: start,
            bytes: Vec::new(),
        })
    }

    fn build(&mut self) -> Result<Index, String> {
        if *self.gone.lock().unwrap() {
            return Err("볼륨이 없음".into());
        }
        self.started.lock().unwrap().push(self.name.clone());
        self.builds
            .recv()
            .unwrap_or_else(|_| Err("테스트가 끝남".into()))
    }
}

/// 인덱스 만들기 한 번의 결과.
type Build = Result<Index, String>;

/// 서비스처럼 볼륨 목록을 받아 맞추는 쪽과, 가짜 볼륨마다 인덱스 결과를 보낼 통로.
struct Harness {
    volumes: Arc<Volumes>,
    mounts: Option<Mounts>,
    stop: Stop,
    /// 볼륨 이름 → 그 볼륨을 마지막으로 연 가짜 볼륨에 인덱스 결과를 보낼 통로.
    builds: Arc<Mutex<BTreeMap<String, Sender<Build>>>>,
    /// 볼륨 이름 → 그 볼륨의 가짜 볼륨들이 빠졌는가.
    gone: BTreeMap<String, Arc<Mutex<bool>>>,
    /// 연 볼륨 이름들.
    opened: Arc<Mutex<Vec<String>>>,
    /// 인덱스를 만들기 시작한 볼륨 이름들.
    started: Arc<Mutex<Vec<String>>>,
}

impl Harness {
    fn new() -> Self {
        let volumes = Arc::new(Volumes::new());
        let stop = Stop::new();
        Self {
            mounts: Some(Mounts::new(volumes.clone(), stop.clone(), None, POLL)),
            volumes,
            stop,
            builds: Arc::default(),
            gone: BTreeMap::new(),
            opened: Arc::default(),
            started: Arc::default(),
        }
    }

    fn sync(&mut self, present: &[(&str, Plan)]) {
        let present: Vec<(String, Plan)> = present
            .iter()
            .map(|(n, p)| ((*n).to_owned(), p.clone()))
            .collect();
        for (name, _) in &present {
            self.gone.entry(name.clone()).or_default();
        }
        let builds = self.builds.clone();
        let gone = self.gone.clone();
        let opened = self.opened.clone();
        let started = self.started.clone();
        self.mounts.as_mut().unwrap().sync(&present, move |name| {
            let (tx, rx) = mpsc::channel();
            builds.lock().unwrap().insert(name.to_owned(), tx);
            opened.lock().unwrap().push(name.to_owned());
            Fake {
                name: name.to_owned(),
                builds: rx,
                gone: gone[name].clone(),
                started: started.clone(),
            }
        });
    }

    /// 볼륨 `volume`의 인덱스 만들기를 `result`로 끝낸다.
    fn finish_build(&self, volume: &str, result: Result<Index, String>) {
        let tx = self.builds.lock().unwrap()[volume].clone();
        // 그 볼륨의 스레드가 이미 끝났으면 받을 쪽이 없다.
        let _ = tx.send(result);
    }

    /// 볼륨 `volume`의 `n`번째 인덱스 만들기가 시작되기를 기다린다.
    fn wait_building(&self, volume: &str, n: usize) {
        self.wait_for(&format!("{volume} 만들기 {n}번째 시작"), |h| {
            h.started
                .lock()
                .unwrap()
                .iter()
                .filter(|v| *v == volume)
                .count()
                >= n
        });
    }

    /// 볼륨 `volume`이 빠진 것처럼 읽기를 모두 실패하게 한다.
    fn pull_out(&self, volume: &str) {
        *self.gone[volume].lock().unwrap() = true;
    }

    fn put_back(&self, volume: &str) {
        *self.gone[volume].lock().unwrap() = false;
    }

    fn status(&self) -> Vec<VolumeStatus> {
        self.volumes.status().volumes
    }

    fn state(&self, volume: &str) -> Option<VolumeState> {
        self.status()
            .into_iter()
            .find(|v| v.volume == volume)
            .map(|v| v.state)
    }

    fn wait_for(&self, what: &str, done: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + PATIENCE;
        while !done(self) {
            assert!(
                Instant::now() < deadline,
                "{what}: 기다려도 되지 않음(상태 {:?})",
                self.status()
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn wait_ready(&self, volume: &str) {
        self.wait_for(&format!("{volume} 준비됨"), |h| {
            matches!(h.state(volume), Some(VolumeState::Ready { .. }))
        });
    }

    fn paths(&self, part: &str) -> Vec<String> {
        let page = self.volumes.search(&query(part), &Cancel::new()).unwrap();
        let mut paths: Vec<String> = page.hits.into_iter().map(|h| h.path).collect();
        paths.sort();
        paths
    }

    fn opened(&self, volume: &str) -> usize {
        self.opened
            .lock()
            .unwrap()
            .iter()
            .filter(|v| *v == volume)
            .count()
    }

    fn stop(mut self) {
        self.stop.stop();
        for t in self.mounts.take().unwrap().into_threads() {
            t.join().unwrap();
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.stop.stop();
    }
}

fn query(part: &str) -> QueryMessage {
    QueryMessage {
        condition: Condition::NameContains {
            part: part.into(),
            case: Case::Insensitive,
        },
        sort: None,
        limit: None,
    }
}

fn not_ntfs(filesystem: &str) -> Plan {
    Plan::NotIndexed(NoIndexReason::NotNtfs {
        filesystem: filesystem.into(),
    })
}

#[test]
fn attached_ntfs_volume_is_indexed_and_shows_up_in_status_and_queries() {
    let mut h = Harness::new();
    h.sync(&[("C:", Plan::Index)]);
    h.finish_build("C:", Ok(index("C:", "c.txt")));
    h.wait_ready("C:");

    // USB 드라이브(NTFS)를 꽂았다.
    h.sync(&[("C:", Plan::Index), ("E:", Plan::Index)]);
    assert_eq!(h.state("E:"), Some(VolumeState::Building { items: None }));
    h.finish_build("E:", Ok(index("E:", "usb.txt")));
    h.wait_ready("E:");
    assert_eq!(h.paths(".txt"), [r"C:\문서\c.txt", r"E:\문서\usb.txt"]);
    h.stop();
}

#[test]
fn attached_non_ntfs_volume_shows_up_as_not_indexed() {
    let mut h = Harness::new();
    h.sync(&[("F:", not_ntfs("exFAT"))]);
    assert_eq!(
        h.state("F:"),
        Some(VolumeState::NotIndexed(NoIndexReason::NotNtfs {
            filesystem: "exFAT".into()
        }))
    );
    // 인덱스를 만들 볼륨이 아니므로 열지 않는다.
    assert_eq!(h.opened("F:"), 0);

    // 떼면 상태에서도 빠진다.
    h.sync(&[]);
    assert_eq!(h.status(), []);
    h.stop();
}

#[test]
fn detached_volume_leaves_status_and_queries_while_others_keep_working() {
    let mut h = Harness::new();
    h.sync(&[("C:", Plan::Index), ("E:", Plan::Index)]);
    h.finish_build("C:", Ok(index("C:", "c.txt")));
    h.finish_build("E:", Ok(index("E:", "usb.txt")));
    h.wait_ready("C:");
    h.wait_ready("E:");
    // 떼기 전에 받아 둔 이어 받기 표지.
    let page = h
        .volumes
        .search(
            &QueryMessage {
                limit: Some(1),
                ..query(".txt")
            },
            &Cancel::new(),
        )
        .unwrap();
    let cursor = page.next.expect("결과가 둘이라 남은 것이 있다");

    // USB 드라이브를 뺐다. 볼륨 목록에서 사라지기 전에 읽기가 먼저 실패할 수도 있다.
    h.pull_out("E:");
    h.sync(&[("C:", Plan::Index)]);
    assert_eq!(h.status().len(), 1);
    assert_eq!(h.status()[0].volume, "C:");
    assert_eq!(h.paths(".txt"), [r"C:\문서\c.txt"]);
    // 뗀 볼륨의 결과를 든 표지는 무효다.
    let err = h.volumes.resume(cursor, 10).unwrap_err();
    assert_eq!(err.kind, ErrorKind::StaleCursor);

    // 따라가던 스레드가 실패를 보고 다시 만들려 해도 뗀 볼륨이 되살아나지 않는다.
    thread::sleep(POLL * 50);
    assert_eq!(h.status().len(), 1);
    assert_eq!(h.paths("usb"), Vec::<String>::new());

    // 남은 볼륨은 계속 질의에 답한다.
    h.sync(&[("C:", Plan::Index)]);
    assert_eq!(h.paths("c.txt"), [r"C:\문서\c.txt"]);
    h.stop();
}

#[test]
fn volume_detached_while_building_is_not_revived_when_the_build_finishes() {
    let mut h = Harness::new();
    h.sync(&[("E:", Plan::Index)]);
    h.wait_building("E:", 1);
    // 인덱스를 만드는 도중에 뺐다.
    h.sync(&[]);
    assert_eq!(h.status(), []);
    // 그 뒤 만들기가 (실패든 성공이든) 끝나도 상태에 나오지 않는다.
    h.finish_build("E:", Ok(index("E:", "late.txt")));
    thread::sleep(POLL * 50);
    assert_eq!(h.status(), []);
    assert!(h.paths("late").is_empty());
    h.stop();
}

#[test]
fn volume_put_back_at_the_same_letter_is_indexed_again() {
    let mut h = Harness::new();
    h.sync(&[("E:", Plan::Index)]);
    h.finish_build("E:", Ok(index("E:", "first.txt")));
    h.wait_ready("E:");

    h.pull_out("E:");
    h.sync(&[]);
    assert_eq!(h.status(), []);

    // 다시 꽂았다(다른 드라이브일 수도 있다). 앞선 스레드가 끝난 뒤 새로 붙여 다시 만든다.
    h.put_back("E:");
    let deadline = Instant::now() + PATIENCE;
    while h.opened("E:") < 2 {
        assert!(Instant::now() < deadline, "다시 붙이지 않음");
        h.sync(&[("E:", Plan::Index)]);
        thread::sleep(POLL);
    }
    assert_eq!(h.state("E:"), Some(VolumeState::Building { items: None }));
    h.finish_build("E:", Ok(index("E:", "second.txt")));
    h.wait_ready("E:");
    assert_eq!(h.paths(".txt"), [r"E:\문서\second.txt"]);
    h.stop();
}

#[test]
fn reattach_waits_for_the_old_thread_still_building() {
    let mut h = Harness::new();
    h.sync(&[("E:", Plan::Index)]);
    h.wait_building("E:", 1);
    h.sync(&[]);
    // 옛 스레드가 아직 인덱스를 만드는 중이라 같은 문자의 볼륨은 아직 붙이지 않는다.
    h.sync(&[("E:", Plan::Index)]);
    assert_eq!(h.opened("E:"), 1);
    assert_eq!(h.status(), []);

    // 옛 스레드가 끝나면 다음 맞추기에서 붙인다.
    h.finish_build("E:", Ok(index("E:", "old.txt")));
    let deadline = Instant::now() + PATIENCE;
    while h.opened("E:") < 2 {
        assert!(Instant::now() < deadline, "다시 붙이지 않음");
        h.sync(&[("E:", Plan::Index)]);
        thread::sleep(POLL);
    }
    h.finish_build("E:", Ok(index("E:", "new.txt")));
    h.wait_ready("E:");
    assert_eq!(h.paths(".txt"), [r"E:\문서\new.txt"]);
    h.stop();
}

#[test]
fn reformatted_volume_at_the_same_letter_changes_plan() {
    let mut h = Harness::new();
    h.sync(&[("E:", Plan::Index)]);
    h.finish_build("E:", Ok(index("E:", "ntfs.txt")));
    h.wait_ready("E:");

    // 같은 문자에 exFAT 볼륨이 보인다(빼고 다른 드라이브를 꽂았거나 다시 포맷했다).
    let mut present = [("E:", not_ntfs("exFAT"))];
    h.sync(&present);
    // 옛 스레드가 끝나기 전이면 아직 붙이지 않을 수 있다.
    let deadline = Instant::now() + PATIENCE;
    while h.state("E:").is_none() {
        assert!(Instant::now() < deadline, "다시 붙이지 않음");
        thread::sleep(POLL);
        h.sync(&present);
    }
    assert_eq!(
        h.state("E:"),
        Some(VolumeState::NotIndexed(NoIndexReason::NotNtfs {
            filesystem: "exFAT".into()
        }))
    );
    assert!(h.paths("ntfs").is_empty());

    // 다시 NTFS로.
    present = [("E:", Plan::Index)];
    h.sync(&present);
    assert_eq!(h.state("E:"), Some(VolumeState::Building { items: None }));
    h.finish_build("E:", Ok(index("E:", "again.txt")));
    h.wait_ready("E:");
    assert_eq!(h.paths(".txt"), [r"E:\문서\again.txt"]);
    h.stop();
}
