//! 휴대폰(MTP)과 PC 사이의 복사·이동·삭제 작업(#357)을 장치 없이 확인한다. 러너에 휴대폰이 없어 임시 폴더의 셸 항목을
//! 휴대폰 항목 경로 표에 적어(`shell_provider::register_device_item`) 휴대폰 항목처럼 쓴다: 작업은 휴대폰과 같은 셸
//! 호출(스트림 가져오기·`IFileOperation` 넣기·영구 삭제)을 지난다. 실제 휴대폰은 사람이 확인한다. Windows 전용.
#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::history::NotUndoable;
use facet_core::job::{
    AnswerError, ConflictAnswer, JobEvent, JobId, JobKind, JobQueue, JobState, Resolution, Summary,
};
use facet_core::shell_provider::{self, DEVICE_PATH_ROOT, DeviceItem};
use facet_core::window::{App, TransferPlan};
use facet_win::shell_folder;
use facet_win::shell_thread::ShellThread;
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(120);

/// 셸 항목 식별자.
fn id_of(path: &Path) -> Vec<u8> {
    let text = path.display().to_string();
    ShellThread::new()
        .call(WAIT, move || shell_folder::id_from_parsing_name(&text))
        .expect("셸 스레드")
        .expect("셸 항목 식별자")
}

/// 이 시험만의 휴대폰 이름(시험끼리 표가 섞이지 않게).
fn phone_root(tag: &str) -> PathBuf {
    PathBuf::from(format!(
        r"{DEVICE_PATH_ROOT}\시험 폰 {tag} {}\내부 저장소",
        std::process::id()
    ))
}

/// `dir`을 휴대폰 폴더 `label`로, 안의 항목들도 그 아래 경로로 적는다. 파일은 날짜를 주지 않는 장치처럼 적는다.
fn register(label: &Path, dir: &Path) {
    shell_provider::register_device_item(
        label.to_path_buf(),
        DeviceItem {
            id: id_of(dir),
            is_folder: true,
            size: None,
            modified: None,
        },
    );
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let metadata = entry.metadata().unwrap();
        shell_provider::register_device_item(
            label.join(entry.file_name()),
            DeviceItem {
                id: id_of(&entry.path()),
                is_folder: metadata.is_dir(),
                size: (!metadata.is_dir()).then_some(metadata.len()),
                modified: None,
            },
        );
    }
}

struct Setup {
    _dir: TempDir,
    pc: PathBuf,
    phone: PathBuf,
    label: PathBuf,
}

fn setup(tag: &str) -> Setup {
    let dir = TempDir::new().unwrap();
    let pc = dir.path().join("PC");
    let phone = dir.path().join("휴대폰");
    fs::create_dir_all(&pc).unwrap();
    fs::create_dir_all(phone.join("앨범")).unwrap();
    fs::write(phone.join("사진 1.jpg"), vec![7u8; 300_000]).unwrap();
    fs::write(phone.join("앨범").join("사진 2.jpg"), b"two").unwrap();
    let label = phone_root(tag);
    register(&label, &phone);
    Setup {
        _dir: dir,
        pc,
        phone,
        label,
    }
}

fn finish(queue: &mut JobQueue, job: JobId) -> (JobState, Summary) {
    let deadline = Instant::now() + WAIT;
    let mut state = None;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match queue.wait_event(left) {
            Some(JobEvent::StateChanged { job: id, state: s }) if id == job => state = Some(s),
            Some(JobEvent::Finished { job: id, summary }) if id == job => {
                return (state.expect("마지막 상태"), summary);
            }
            Some(JobEvent::Conflict { .. } | JobEvent::ItemError { .. }) => {
                panic!("묻지 않을 작업이 물었다")
            }
            Some(_) => {}
            None => panic!("작업이 끝나지 않았다"),
        }
    }
}

#[test]
fn 휴대폰의_파일과_폴더를_pc로_가져온다() {
    let s = setup("가져오기");
    let mut queue = JobQueue::new();
    let job = queue
        .copy(
            vec![s.label.join("사진 1.jpg"), s.label.join("앨범")],
            s.pc.clone(),
        )
        .expect("작업");
    let (state, summary) = finish(&mut queue, job);
    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert_eq!(
        fs::read(s.pc.join("사진 1.jpg")).unwrap(),
        vec![7u8; 300_000]
    );
    assert_eq!(
        fs::read(s.pc.join("앨범").join("사진 2.jpg")).unwrap(),
        b"two"
    );
    assert_eq!(summary.files_copied, 2);
    assert_eq!(summary.folders_copied, 1);
    // 원본은 그대로다.
    assert!(s.phone.join("사진 1.jpg").exists());
}

#[test]
fn pc_파일과_폴더를_휴대폰에_넣는다() {
    let s = setup("넣기");
    fs::write(s.pc.join("노래.mp3"), b"music").unwrap();
    fs::create_dir_all(s.pc.join("음반")).unwrap();
    fs::write(s.pc.join("음반").join("1.mp3"), b"one").unwrap();
    let mut queue = JobQueue::new();
    let job = queue
        .copy(
            vec![s.pc.join("노래.mp3"), s.pc.join("음반")],
            s.label.clone(),
        )
        .expect("작업");
    let (state, summary) = finish(&mut queue, job);
    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert_eq!(fs::read(s.phone.join("노래.mp3")).unwrap(), b"music");
    assert_eq!(
        fs::read(s.phone.join("음반").join("1.mp3")).unwrap(),
        b"one"
    );
}

#[test]
fn 휴대폰에서_옮기면_휴대폰_원본이_지워진다() {
    let s = setup("옮기기");
    let mut queue = JobQueue::new();
    let job = queue
        .move_items(vec![s.label.join("사진 1.jpg")], s.pc.clone())
        .expect("작업");
    let (state, _) = finish(&mut queue, job);
    assert_eq!(state, JobState::Finished);
    assert!(s.pc.join("사진 1.jpg").exists());
    assert!(!s.phone.join("사진 1.jpg").exists());
}

#[test]
fn 휴대폰_항목을_영구_삭제한다() {
    let s = setup("삭제");
    let mut queue = JobQueue::new();
    let job = queue
        .delete_permanently(vec![s.label.join("사진 1.jpg"), s.label.join("앨범")])
        .expect("작업");
    let (state, summary) = finish(&mut queue, job);
    assert_eq!(state, JobState::Finished);
    assert_eq!(summary.items_deleted, 2);
    assert!(!s.phone.join("사진 1.jpg").exists());
    assert!(!s.phone.join("앨범").exists());
}

#[test]
fn 휴대폰끼리는_주고받지_않는다() {
    let s = setup("휴대폰끼리");
    let mut queue = JobQueue::new();
    assert_eq!(
        queue.copy(vec![s.label.join("사진 1.jpg")], s.label.join("앨범")),
        Err(facet_core::job::CopyRejection::Unsupported)
    );
}

#[test]
fn 날짜를_주지_않는_휴대폰_파일과의_충돌은_더_새것만_덮어쓰기를_고를_수_없다() {
    let s = setup("충돌");
    fs::write(s.pc.join("사진 1.jpg"), b"old").unwrap();
    let mut queue = JobQueue::new();
    let job = queue
        .copy(vec![s.label.join("사진 1.jpg")], s.pc.clone())
        .expect("작업");
    let deadline = Instant::now() + WAIT;
    let conflict = loop {
        match queue.wait_event(deadline.saturating_duration_since(Instant::now())) {
            Some(JobEvent::Conflict { job: id, conflict }) if id == job => break conflict,
            Some(JobEvent::Finished { .. }) => panic!("묻지 않고 끝났다"),
            Some(_) => {}
            None => panic!("충돌 질문이 오지 않았다"),
        }
    };
    assert!(conflict.can_overwrite);
    assert_eq!(conflict.source.modified, None);
    assert!(!conflict.can_overwrite_if_newer());
    assert_eq!(
        queue.answer_conflict(job, ConflictAnswer::once(Resolution::OverwriteIfNewer)),
        Err(AnswerError::OverwriteNotAllowed)
    );
    queue
        .answer_conflict(job, ConflictAnswer::once(Resolution::Overwrite))
        .expect("덮어쓰기");
    let (state, summary) = finish(&mut queue, job);
    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert_eq!(
        fs::read(s.pc.join("사진 1.jpg")).unwrap(),
        vec![7u8; 300_000]
    );
    // 덮어쓴 옛 파일은 PC 휴지통으로 갔다(되살릴 수 있다). 시험 뒤 휴지통에서 지운다.
    for item in &summary.recycled {
        let _ = fs::remove_file(item.recycled());
    }
}

#[test]
fn 휴대폰에_넣는_충돌의_덮어쓰기는_되살릴_수_없다() {
    let s = setup("넣기 충돌");
    fs::write(s.pc.join("사진 1.jpg"), b"new").unwrap();
    let mut queue = JobQueue::new();
    let job = queue
        .copy(vec![s.pc.join("사진 1.jpg")], s.label.clone())
        .expect("작업");
    let deadline = Instant::now() + WAIT;
    let conflict = loop {
        match queue.wait_event(deadline.saturating_duration_since(Instant::now())) {
            Some(JobEvent::Conflict { job: id, conflict }) if id == job => break conflict,
            Some(JobEvent::Finished { .. }) => panic!("묻지 않고 끝났다"),
            Some(_) => {}
            None => panic!("충돌 질문이 오지 않았다"),
        }
    };
    assert!(conflict.overwrite_is_permanent, "휴대폰에는 휴지통이 없다");
    queue
        .answer_conflict(job, ConflictAnswer::once(Resolution::KeepBoth))
        .expect("둘 다 유지");
    let (state, _) = finish(&mut queue, job);
    assert_eq!(state, JobState::Finished);
    assert_eq!(fs::read(s.phone.join("사진 1 (2).jpg")).unwrap(), b"new");
    assert_eq!(fs::read(s.phone.join("사진 1.jpg")).unwrap().len(), 300_000);
}

/// 앱에서 작업을 넣고 끝날 때까지 받는다.
fn run_in_app(app: &mut App, plan: TransferPlan) {
    app.start_transfer(&plan).expect("작업");
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll_jobs();
        if !app.jobs().has_unfinished() {
            app.poll_jobs();
            return;
        }
        assert!(Instant::now() < deadline, "작업이 끝나지 않았다");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn 가져온_복사는_되돌릴_수_있고_넣은_복사는_되돌릴_수_없다고_기록한다() {
    let s = setup("기록");
    let mut app = App::new(Some(Location::file_system(&s.pc)));
    let _ = app.wait(WAIT);
    run_in_app(
        &mut app,
        TransferPlan {
            kind: JobKind::Copy,
            sources: vec![s.label.join("사진 1.jpg")],
            target: s.pc.clone(),
        },
    );
    let pulled = app.history().records().last().expect("기록").clone();
    assert_eq!(
        pulled.not_undoable_reason(),
        None,
        "가져온 복사는 되돌릴 수 있다"
    );

    fs::write(s.pc.join("메모.txt"), b"memo").unwrap();
    run_in_app(
        &mut app,
        TransferPlan {
            kind: JobKind::Copy,
            sources: vec![s.pc.join("메모.txt")],
            target: s.label.clone(),
        },
    );
    let pushed = app.history().records().last().expect("기록").clone();
    assert_eq!(
        pushed.not_undoable_reason(),
        Some(NotUndoable::NotUndoableThere),
        "휴대폰 공급자는 실행취소 가능을 알리지 않는다"
    );
    assert!(s.phone.join("메모.txt").exists());
}

#[test]
fn 휴대폰_위치에서는_pc와_주고받고_지우기는_영구_삭제를_확인받는다() {
    use facet_core::command::CommandId;
    use facet_core::shell_provider::{ShellEntry, fake};
    use facet_core::window::DeleteWarning;

    let folder = fake::serve(vec![ShellEntry {
        name: "사진.jpg".into(),
        path: None,
        parsing_name: String::new(),
        is_folder: false,
        id: vec![6, 0, 1, 2, 3, 4, 0, 0],
        size: Some(10),
        modified: None,
    }]);
    let mut app = App::new(Some(folder.location().clone()));
    let _ = app.wait(WAIT);
    let tab = app.active_tab_mut();
    assert_eq!(tab.len(), 1);
    tab.select_only(0);
    assert!(app.can_execute(CommandId::CopyTo));
    assert!(app.can_execute(CommandId::MoveTo));
    assert!(app.can_execute(CommandId::DeletePermanently));
    // 휴대폰 항목은 클립보드·끌기로 다른 앱에 줄 수 없고, 이름 바꾸기·새 폴더는 spec #27 밖이다.
    assert!(!app.can_execute(CommandId::Copy));
    assert!(!app.can_execute(CommandId::Rename));
    assert!(!app.can_execute(CommandId::NewFolder));
    assert_eq!(app.drag_sources(), None);
    let plan = app.delete_plan(CommandId::Delete).expect("삭제할 내용");
    assert_eq!(plan.warning, Some(DeleteWarning::Permanent));
    assert!(shell_provider::is_device_path(&plan.sources[0]));
}
