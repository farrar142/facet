//! USN 저널 따라가기와 스스로 다시 만들기([`follow::run`])를 실제 볼륨 없이 확인한다. 볼륨 읽기는
//! 저널 정보·레코드·인덱스를 테스트가 바꿔 넣는 가짜 [`Source`]다. 관리자 권한이 필요 없다.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use facet_index::{
    ATTRIBUTE_DIRECTORY, Index, NewItem, USN_REASON_FILE_CREATE, USN_REASON_FILE_DELETE,
    USN_REASON_RENAME_NEW_NAME, USN_REASON_RENAME_OLD_NAME, USN_REASON_SECURITY_CHANGE,
};
use facet_indexer::access::Caller;
use facet_indexer::follow::{self, IndexFile, JournalNow, Records, SavePolicy, Source, Stop};
use facet_indexer::session::Cancel;
use facet_indexer::volumes::Volumes;
use facet_protocol::{Case, Condition, ErrorKind, NoIndexReason, QueryMessage, VolumeState};

const VOLUME: &str = "T:";
const ROOT: u64 = 5;
const DIR: u32 = ATTRIBUTE_DIRECTORY;
const FILE: u32 = 0x20;
const POLL: Duration = Duration::from_millis(2);
const PATIENCE: Duration = Duration::from_secs(10);

/// 가짜 볼륨의 저널. 테스트가 레코드를 덧붙이고 저널 ID·가장 오래된 USN을 바꾼다.
#[derive(Debug)]
struct Journal {
    journal_id: u64,
    first_usn: i64,
    /// (USN, 레코드 바이트). USN 순서.
    records: Vec<(i64, Vec<u8>)>,
    next_usn: i64,
    /// 읽기를 실패하게 한다.
    broken: bool,
}

struct Fake {
    journal: Arc<Mutex<Journal>>,
    /// 인덱스를 만들 때마다 테스트가 보내 주는 결과를 기다린다.
    builds: Receiver<Result<Index, String>>,
    /// 인덱스를 만들기 시작했다고 테스트에 알린다.
    started: Sender<()>,
}

impl Source for Fake {
    fn journal(&mut self) -> Result<JournalNow, String> {
        let j = self.journal.lock().unwrap();
        Ok(JournalNow {
            journal_id: j.journal_id,
            first_usn: j.first_usn,
            next_usn: j.next_usn,
        })
    }

    fn read(&mut self, journal_id: u64, start: i64) -> Result<Records, String> {
        let j = self.journal.lock().unwrap();
        if j.broken {
            return Err("읽기 실패".into());
        }
        assert_eq!(journal_id, j.journal_id, "다른 저널을 읽음");
        // 한 번에 레코드 둘까지만 준다(묶음을 여러 번 읽는지 본다).
        let batch: Vec<&(i64, Vec<u8>)> = j
            .records
            .iter()
            .filter(|(usn, _)| *usn >= start)
            .take(2)
            .collect();
        let next_usn = match j.records.iter().find(|(usn, _)| *usn >= start).map(|_| ()) {
            None => j.next_usn.max(start),
            Some(()) => batch.last().map_or(start, |(usn, _)| usn + 1),
        };
        Ok(Records {
            next_usn,
            bytes: batch.iter().flat_map(|(_, b)| b.iter().copied()).collect(),
        })
    }

    fn build(&mut self) -> Result<Index, String> {
        let _ = self.started.send(());
        self.builds
            .recv()
            .unwrap_or_else(|_| Err("테스트가 끝남".into()))
    }
}

/// `USN_RECORD_V2` 하나의 바이트.
fn record(number: u64, parent: u64, usn: i64, reason: u32, attributes: u32, name: &str) -> Vec<u8> {
    let units: Vec<u16> = name.encode_utf16().collect();
    let length = (60 + units.len() * 2).next_multiple_of(8);
    let mut b = vec![0u8; length];
    b[0..4].copy_from_slice(&(length as u32).to_le_bytes());
    b[4..6].copy_from_slice(&2u16.to_le_bytes());
    b[8..16].copy_from_slice(&number.to_le_bytes());
    b[16..24].copy_from_slice(&parent.to_le_bytes());
    b[24..32].copy_from_slice(&usn.to_le_bytes());
    b[32..40].copy_from_slice(&1_234u64.to_le_bytes());
    b[40..44].copy_from_slice(&reason.to_le_bytes());
    b[52..56].copy_from_slice(&attributes.to_le_bytes());
    b[56..58].copy_from_slice(&((units.len() * 2) as u16).to_le_bytes());
    b[58..60].copy_from_slice(&60u16.to_le_bytes());
    for (i, u) in units.iter().enumerate() {
        b[60 + i * 2..62 + i * 2].copy_from_slice(&u.to_le_bytes());
    }
    b
}

/// 시험 볼륨: `문서` 폴더 아래 `files`.
fn index(files: &[&str]) -> Index {
    let mut index = Index::new(VOLUME, ROOT);
    let mut put = |number, parent, name: &str, attributes| {
        index
            .insert(NewItem {
                number,
                parent,
                name,
                size: 10,
                modified: 99,
                attributes,
            })
            .unwrap();
    };
    put(40, ROOT, "문서", DIR);
    put(41, ROOT, "사진", DIR);
    for (n, name) in files.iter().enumerate() {
        put(50 + n as u64, 40, name, FILE);
    }
    index
}

struct Harness {
    volumes: Arc<Volumes>,
    journal: Arc<Mutex<Journal>>,
    builds: Sender<Result<Index, String>>,
    started: Receiver<()>,
    stop: Stop,
    thread: Option<JoinHandle<()>>,
}

/// 저널 끝에 레코드 하나를 덧붙인다.
fn push_to(
    journal: &Mutex<Journal>,
    number: u64,
    parent: u64,
    reason: u32,
    attributes: u32,
    name: &str,
) {
    let mut j = journal.lock().unwrap();
    let usn = j.next_usn;
    j.records
        .push((usn, record(number, parent, usn, reason, attributes, name)));
    j.next_usn += 100;
}

fn new_journal() -> Arc<Mutex<Journal>> {
    Arc::new(Mutex::new(Journal {
        journal_id: 7,
        first_usn: 0,
        records: Vec::new(),
        next_usn: 1000,
        broken: false,
    }))
}

impl Harness {
    fn start() -> Self {
        Self::start_with(new_journal(), None)
    }

    /// 저널 `journal`(서비스가 꺼졌다 다시 떠도 볼륨의 저널은 그대로다)과 인덱스 파일 `file`로
    /// 따라가기를 시작한다.
    fn start_with(journal: Arc<Mutex<Journal>>, file: Option<IndexFile>) -> Self {
        let volumes = Arc::new(Volumes::new());
        let (builds, builds_rx) = mpsc::channel();
        let (started_tx, started) = mpsc::channel();
        let stop = Stop::new();
        let mut fake = Fake {
            journal: journal.clone(),
            builds: builds_rx,
            started: started_tx,
        };
        let thread = {
            let volumes = volumes.clone();
            let stop = stop.clone();
            thread::spawn(move || {
                follow::run(&volumes, VOLUME, &mut fake, &stop, POLL, file.as_ref())
            })
        };
        Self {
            volumes,
            journal,
            builds,
            started,
            stop,
            thread: Some(thread),
        }
    }

    /// 인덱스 만들기가 시작되기를 기다렸다가 `index`로 끝낸다.
    fn finish_build(&self, index: Result<Index, String>) {
        self.started
            .recv_timeout(PATIENCE)
            .expect("인덱스 만들기가 시작되지 않음");
        self.builds.send(index).unwrap();
    }

    /// 시험 볼륨의 상태. 따라가기 스레드가 아직 아무것도 하지 않았으면 `None`.
    fn state(&self) -> Option<VolumeState> {
        self.volumes
            .status()
            .volumes
            .first()
            .map(|v| v.state.clone())
    }

    fn wait_for(&self, what: &str, done: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + PATIENCE;
        while !done(self) {
            assert!(
                Instant::now() < deadline,
                "{what}: 기다려도 되지 않음(상태 {:?})",
                self.state()
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn wait_ready(&self) {
        self.wait_for("준비됨", |h| {
            matches!(h.state(), Some(VolumeState::Ready { .. }))
        });
    }

    /// 저널 끝에 레코드 하나를 덧붙인다.
    fn push(&self, number: u64, parent: u64, reason: u32, attributes: u32, name: &str) {
        push_to(&self.journal, number, parent, reason, attributes, name);
    }

    fn paths(&self, part: &str) -> Vec<String> {
        let page = self
            .volumes
            .search(&query(part, None), &Cancel::new())
            .unwrap();
        let mut paths: Vec<String> = page.hits.into_iter().map(|h| h.path).collect();
        paths.sort();
        paths
    }

    fn wait_paths(&self, part: &str, want: &[&str]) {
        self.wait_for(&format!("{part} → {want:?}"), |h| h.paths(part) == want);
    }

    fn stop(mut self) {
        self.stop.stop();
        self.thread.take().unwrap().join().unwrap();
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.stop.stop();
    }
}

fn query(part: &str, limit: Option<u32>) -> QueryMessage {
    QueryMessage {
        condition: Condition::NameContains {
            part: part.into(),
            case: Case::Insensitive,
        },
        sort: None,
        limit,
    }
}

#[test]
fn first_index_is_building_then_ready() {
    let h = Harness::start();
    h.wait_for("만드는 중", |h| {
        h.state() == Some(VolumeState::Building { items: None })
    });
    h.finish_build(Ok(index(&["a.txt"])));
    h.wait_ready();
    assert_eq!(h.paths("a.txt"), [r"T:\문서\a.txt"]);
    h.stop();
}

#[test]
fn create_rename_delete_and_folder_move_show_up_in_queries() {
    let h = Harness::start();
    h.finish_build(Ok(index(&["report.txt"])));
    h.wait_ready();

    h.push(60, 41, USN_REASON_FILE_CREATE, FILE, "새 파일.txt");
    h.wait_paths("새 파일", &[r"T:\사진\새 파일.txt"]);

    h.push(60, 41, USN_REASON_RENAME_OLD_NAME, FILE, "새 파일.txt");
    h.push(60, 41, USN_REASON_RENAME_NEW_NAME, FILE, "바꾼 이름.txt");
    h.wait_paths("바꾼 이름", &[r"T:\사진\바꾼 이름.txt"]);
    assert!(h.paths("새 파일").is_empty());

    h.push(60, 41, USN_REASON_FILE_DELETE, FILE, "바꾼 이름.txt");
    h.wait_paths("바꾼 이름", &[]);

    // 폴더 이동: 「문서」를 「사진」 아래로 옮기면 그 아래 항목의 경로도 바뀐다.
    h.push(40, ROOT, USN_REASON_RENAME_OLD_NAME, DIR, "문서");
    h.push(40, 41, USN_REASON_RENAME_NEW_NAME, DIR, "문서");
    h.wait_paths("report", &[r"T:\사진\문서\report.txt"]);
    h.stop();
}

#[test]
fn changes_made_while_the_first_index_is_built_are_applied() {
    let h = Harness::start();
    h.started.recv_timeout(PATIENCE).unwrap();
    // 저널 정보는 이미 읽었고 MFT를 읽는 중이다. 그 사이에 파일이 생긴다.
    h.push(60, 40, USN_REASON_FILE_CREATE, FILE, "그 사이.txt");
    h.builds.send(Ok(index(&[]))).unwrap();
    h.wait_paths("그 사이", &[r"T:\문서\그 사이.txt"]);
    h.stop();
}

#[test]
fn journal_id_change_rebuilds_while_answering_from_the_previous_index() {
    let h = Harness::start();
    h.finish_build(Ok(index(&["old.txt", "other.txt"])));
    h.wait_ready();
    let cursor = h
        .volumes
        .search(&query(".txt", Some(1)), &Cancel::new())
        .unwrap()
        .next
        .unwrap();

    {
        let mut j = h.journal.lock().unwrap();
        j.journal_id = 8;
        j.records.clear();
    }
    // 다시 만드는 동안: 「만드는 중」(앞선 인덱스의 항목 수)이고 앞선 인덱스로 답한다.
    h.started.recv_timeout(PATIENCE).unwrap();
    assert_eq!(h.state(), Some(VolumeState::Building { items: Some(4) }));
    let page = h
        .volumes
        .search(&query("old", None), &Cancel::new())
        .unwrap();
    assert_eq!(page.hits[0].path, r"T:\문서\old.txt");
    assert_eq!(page.building, [VOLUME]);

    h.builds.send(Ok(index(&["new.txt"]))).unwrap();
    h.wait_ready();
    assert_eq!(h.state(), Some(VolumeState::Ready { items: 3 }));
    assert!(h.paths("old").is_empty());
    assert_eq!(h.paths("new"), [r"T:\문서\new.txt"]);
    let page = h
        .volumes
        .search(&query("new", None), &Cancel::new())
        .unwrap();
    assert!(page.building.is_empty());
    // 바꿔 끼운 볼륨의 앞선 이어 받기 표지는 무효다.
    let err = h.volumes.resume(cursor, 10).unwrap_err();
    assert_eq!(err.kind, ErrorKind::StaleCursor);

    // 새 저널을 따라간다.
    h.push(70, 40, USN_REASON_FILE_CREATE, FILE, "after.txt");
    h.wait_paths("after", &[r"T:\문서\after.txt"]);
    h.stop();
}

#[test]
fn journal_overflow_rebuilds() {
    let h = Harness::start();
    h.finish_build(Ok(index(&["old.txt"])));
    h.wait_ready();
    {
        // 따라가기가 읽기 전에 레코드가 밀려났다.
        let mut j = h.journal.lock().unwrap();
        j.next_usn = 5000;
        j.first_usn = 4000;
    }
    h.finish_build(Ok(index(&["fresh.txt"])));
    h.wait_paths("fresh", &[r"T:\문서\fresh.txt"]);
    h.wait_ready();
    h.stop();
}

#[test]
fn unreadable_journal_or_broken_records_rebuild() {
    let h = Harness::start();
    h.finish_build(Ok(index(&["one.txt"])));
    h.wait_ready();
    h.journal.lock().unwrap().broken = true;
    h.started.recv_timeout(PATIENCE).unwrap();
    h.journal.lock().unwrap().broken = false;
    h.builds.send(Ok(index(&["two.txt"]))).unwrap();
    h.wait_paths("two", &[r"T:\문서\two.txt"]);

    {
        let mut j = h.journal.lock().unwrap();
        let usn = j.next_usn;
        j.records.push((usn, vec![0xFF; 16]));
        j.next_usn += 100;
    }
    h.finish_build(Ok(index(&["three.txt"])));
    h.wait_paths("three", &[r"T:\문서\three.txt"]);
    h.stop();
}

#[test]
fn failed_build_marks_the_volume_not_indexed_and_ends() {
    let mut h = Harness::start();
    h.finish_build(Err("MFT를 읽지 못함".into()));
    h.thread.take().unwrap().join().unwrap();
    assert_eq!(
        h.state(),
        Some(VolumeState::NotIndexed(NoIndexReason::Failed {
            message: "MFT를 읽지 못함".into()
        }))
    );
}

#[test]
fn idle_polls_keep_cursors_alive() {
    let h = Harness::start();
    h.finish_build(Ok(index(&["a.txt", "b.txt"])));
    h.wait_ready();
    {
        // 적용할 레코드 없이 저널만 앞으로 간다.
        let mut j = h.journal.lock().unwrap();
        j.next_usn += 10_000;
    }
    let cursor = h
        .volumes
        .search(&query(".txt", Some(1)), &Cancel::new())
        .unwrap()
        .next
        .unwrap();
    thread::sleep(POLL * 20);
    let page = h.volumes.resume(cursor, 10).unwrap();
    assert_eq!(page.hits.len(), 1);
    h.stop();
}

// --- 인덱스 파일: 저장과 다시 뜬 뒤 따라잡기 ---

/// 저장 주기가 테스트 동안 오지 않는 정책(멈출 때와 만든 직후에만 저장한다).
const NEVER: SavePolicy = SavePolicy {
    changes: usize::MAX,
    interval: Duration::from_secs(3600),
};

fn index_file(dir: &tempfile::TempDir, policy: SavePolicy) -> IndexFile {
    IndexFile::in_dir(dir.path(), VOLUME, policy)
}

/// 파일에 저장된 인덱스에서 이름에 `part`가 든 항목의 경로들(정렬). 읽지 못하면 `None`.
fn saved_paths(file: &IndexFile, part: &str) -> Option<Vec<String>> {
    let index = Index::load(&file.path).ok()?;
    let volumes = Volumes::with_index(VOLUME, index);
    let page = volumes.search(&query(part, None), &Cancel::new()).ok()?;
    let mut paths: Vec<String> = page.hits.into_iter().map(|h| h.path).collect();
    paths.sort();
    Some(paths)
}

fn wait_saved(file: &IndexFile, part: &str, want: &[&str]) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let got = saved_paths(file, part);
        if got.as_deref().is_some_and(|g| g == want) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "저장된 인덱스에서 {part} → {want:?}가 되지 않음(지금 {got:?})"
        );
        thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn file_name_is_per_volume() {
    assert_eq!(follow::file_name("C:"), "C.facetidx");
    assert_eq!(follow::file_name(r"D:\"), "D.facetidx");
    assert_ne!(follow::file_name("C:"), follow::file_name("D:"));
}

#[test]
fn a_fresh_index_is_saved_right_after_it_is_built() {
    let dir = tempfile::tempdir().unwrap();
    let file = index_file(&dir, NEVER);
    let h = Harness::start_with(new_journal(), Some(file.clone()));
    h.finish_build(Ok(index(&["a.txt"])));
    h.wait_ready();
    wait_saved(&file, "a.txt", &[r"T:\문서\a.txt"]);
    let saved = Index::load(&file.path).unwrap();
    assert_eq!(saved.journal().unwrap().journal_id, 7);
    h.stop();
}

#[test]
fn restart_uses_the_saved_index_and_catches_up_on_changes_made_while_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let file = index_file(&dir, NEVER);
    let journal = new_journal();

    let first = Harness::start_with(journal.clone(), Some(file.clone()));
    first.finish_build(Ok(index(&["a.txt"])));
    first.wait_ready();
    // 따라가는 동안의 변경도 멈출 때 저장된다.
    first.push(60, 41, USN_REASON_FILE_CREATE, FILE, "켜진 동안.txt");
    first.wait_paths("켜진 동안", &[r"T:\사진\켜진 동안.txt"]);
    first.stop();
    wait_saved(&file, "켜진 동안", &[r"T:\사진\켜진 동안.txt"]);

    // 서비스가 꺼져 있는 동안의 변경.
    push_to(
        &journal,
        61,
        40,
        USN_REASON_FILE_CREATE,
        FILE,
        "꺼진 동안.txt",
    );
    push_to(&journal, 50, 40, USN_REASON_FILE_DELETE, FILE, "a.txt");

    let second = Harness::start_with(journal, Some(file));
    second.wait_ready();
    second.wait_paths("꺼진 동안", &[r"T:\문서\꺼진 동안.txt"]);
    second.wait_paths("a.txt", &[]);
    assert_eq!(second.paths("켜진 동안"), [r"T:\사진\켜진 동안.txt"]);
    // MFT로 다시 만들지 않았다.
    assert!(
        second.started.try_recv().is_err(),
        "저장한 인덱스를 두고 다시 만듦"
    );
    second.stop();
}

#[test]
fn journal_changed_or_overflowed_while_stopped_rebuilds_answering_from_the_saved_index() {
    for overflow in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let file = index_file(&dir, NEVER);
        let journal = new_journal();
        let first = Harness::start_with(journal.clone(), Some(file.clone()));
        first.finish_build(Ok(index(&["old.txt"])));
        first.wait_ready();
        first.stop();
        wait_saved(&file, "old", &[r"T:\문서\old.txt"]);

        {
            let mut j = journal.lock().unwrap();
            if overflow {
                // 저장한 USN(1000) 뒤의 레코드가 밀려났다.
                j.next_usn = 9000;
                j.first_usn = 8000;
            } else {
                j.journal_id = 8;
            }
        }
        let second = Harness::start_with(journal, Some(file.clone()));
        second
            .started
            .recv_timeout(PATIENCE)
            .unwrap_or_else(|_| panic!("다시 만들지 않음(overflow={overflow})"));
        // 다시 만드는 동안 저장한 인덱스로 답한다.
        assert_eq!(
            second.state(),
            Some(VolumeState::Building { items: Some(3) })
        );
        assert_eq!(second.paths("old"), [r"T:\문서\old.txt"]);
        second.builds.send(Ok(index(&["new.txt"]))).unwrap();
        second.wait_ready();
        assert!(second.paths("old").is_empty());
        // 새로 만든 인덱스를 저장했다.
        wait_saved(&file, "new", &[r"T:\문서\new.txt"]);
        second.stop();
    }
}

#[test]
fn broken_or_other_version_index_files_are_discarded_and_rebuilt() {
    let dir = tempfile::tempdir().unwrap();
    let file = index_file(&dir, NEVER);
    index(&["good.txt"]).save(&file.path).unwrap();
    let good = std::fs::read(&file.path).unwrap();

    let mut checksum = good.clone();
    let last = checksum.len() - 1;
    checksum[last] ^= 0xFF;
    let mut version = good.clone();
    version[8..12].copy_from_slice(&(facet_index::FORMAT_VERSION + 1).to_le_bytes());
    let cases: [(&str, Vec<u8>); 5] = [
        ("검사 값 틀림", checksum),
        ("본문 잘림", good[..good.len() - 10].to_vec()),
        ("머리 잘림", good[..10].to_vec()),
        ("형식 버전 다름", version),
        ("인덱스 파일이 아님", b"not an index file at all".to_vec()),
    ];
    for (what, bytes) in cases {
        std::fs::write(&file.path, &bytes).unwrap();
        let h = Harness::start_with(new_journal(), Some(file.clone()));
        h.started
            .recv_timeout(PATIENCE)
            .unwrap_or_else(|_| panic!("{what}: 다시 만들지 않음"));
        // 깨진 파일은 쓰지 않는다: 만드는 동안 답할 앞선 인덱스가 없다.
        assert_eq!(
            h.state(),
            Some(VolumeState::Building { items: None }),
            "{what}"
        );
        h.builds.send(Ok(index(&["rebuilt.txt"]))).unwrap();
        h.wait_ready();
        wait_saved(&file, "rebuilt", &[r"T:\문서\rebuilt.txt"]);
        h.stop();
    }
}

#[test]
fn saves_after_enough_changes() {
    let dir = tempfile::tempdir().unwrap();
    let file = index_file(
        &dir,
        SavePolicy {
            changes: 2,
            interval: Duration::from_secs(3600),
        },
    );
    let h = Harness::start_with(new_journal(), Some(file.clone()));
    h.finish_build(Ok(index(&[])));
    h.wait_ready();
    wait_saved(&file, ".txt", &[]);

    h.push(60, 40, USN_REASON_FILE_CREATE, FILE, "one.txt");
    h.wait_paths("one", &[r"T:\문서\one.txt"]);
    // 하나로는 저장하지 않는다.
    thread::sleep(POLL * 20);
    assert_eq!(saved_paths(&file, ".txt").unwrap(), Vec::<String>::new());

    h.push(61, 40, USN_REASON_FILE_CREATE, FILE, "two.txt");
    wait_saved(&file, ".txt", &[r"T:\문서\one.txt", r"T:\문서\two.txt"]);
    h.stop();
}

#[test]
fn saves_after_the_interval_when_something_changed() {
    let dir = tempfile::tempdir().unwrap();
    let file = index_file(
        &dir,
        SavePolicy {
            changes: usize::MAX,
            interval: Duration::from_millis(50),
        },
    );
    let h = Harness::start_with(new_journal(), Some(file.clone()));
    h.finish_build(Ok(index(&[])));
    h.wait_ready();
    h.push(60, 40, USN_REASON_FILE_CREATE, FILE, "later.txt");
    wait_saved(&file, "later", &[r"T:\문서\later.txt"]);
    h.stop();
}

/// 막힌 폴더 목록을 테스트가 바꾸는 가짜 사용자.
struct Denying(Mutex<Vec<String>>);

impl Caller for Denying {
    fn user(&self) -> Option<String> {
        Some("S-1-5-21-1".into())
    }

    fn can_list(&self, folder: &str) -> bool {
        !self.0.lock().unwrap().iter().any(|d| d == folder)
    }
}

#[test]
fn a_security_change_record_makes_the_changed_permission_show_up() {
    let h = Harness::start();
    h.finish_build(Ok(index(&["report.txt"])));
    h.wait_ready();
    let caller = Denying(Mutex::new(vec![r"T:\문서".into()]));
    let visible = |h: &Harness| {
        h.volumes
            .search_as(&query("report", None), &caller, &Cancel::new())
            .unwrap()
            .hits
            .len()
    };
    assert_eq!(visible(&h), 0);

    // 권한이 풀려도 기억으로 답하다가, 보안 변경 레코드를 적용하면 반영된다.
    caller.0.lock().unwrap().clear();
    assert_eq!(visible(&h), 0);
    h.push(40, ROOT, USN_REASON_SECURITY_CHANGE, DIR, "문서");
    h.wait_for("보안 변경 반영", |h| visible(h) == 1);
    h.stop();
}
