//! **휴지통 삭제** 작업과 그 실행취소(복원)·다시 실행을 임시 폴더의 실제 파일과 실제 휴지통으로 공개 인터페이스에서
//! 확인한다(Windows 전용).
//!
//! - 파일·폴더를 휴지통 삭제한 뒤 실행취소하면 원래 자리에 내용·이름·수정 시각이 같게 돌아오고, 다시 실행하면 다시
//!   사라진다.
//! - 실행취소 전에 원래 자리에 같은 이름을 만들면 그 항목만 건너뛰고 까닭을 보고하며 아무것도 덮어쓰지 않는다.
//! - 휴지통에 들어가지 않는 항목(셸이 영구 삭제하려 함, `recycle::fault::refuse_recycle_under`로 주입)은 묻고,
//!   건너뛰면 남고, 영구 삭제로 답하면 지워지며 그 항목만 되돌릴 수 없다.
//! - 잠긴 파일은 항목별 오류로 묻고 답대로 한다.
//! - 위치에 휴지통이 있는지 미리 묻는 함수와 Delete가 휴지통이 없는 위치에서 영구 삭제 작업이 되는 것.
//!
//! 러너 계정이 휴지통을 쓸 수 없으면(볼륨에 휴지통이 없거나 셸이 휴지통에 넣지 않음) 건너뛰었다고 출력한다.
//! 테스트가 휴지통에 넣은 항목은 테스트가 끝날 때(실패해도) 휴지통에서 지운다.
#![cfg(windows)]

#[path = "common/bin.rs"]
mod bin;

use std::fs::{self, File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use facet_core::Location;
use facet_core::history::{NotUndoable, OperationKind, RevertSkip, Step, UndoError};
use facet_core::job::{
    self, AnswerError, DeleteRejection, ErrorAnswer, FileOpErrorKind, ItemKind, JobId, JobKind,
    JobQuestion, JobQueue, JobState, NotRecyclableAnswer, Summary,
};
use facet_core::tab::ReadOutcome;
use facet_core::window::App;
use facet_win::recycle::{self, RecycledItem};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(60);

/// 테스트가 휴지통에 넣은 항목. 버릴 때 휴지통에 남아 있으면 지운다.
#[derive(Default)]
struct Bin(Vec<RecycledItem>);

impl Bin {
    fn keep(&mut self, summary: &Summary) {
        self.0.extend(summary.recycled.iter().cloned());
    }
}

impl Drop for Bin {
    fn drop(&mut self) {
        bin::purge(&self.0);
    }
}

/// 지난 시각: 작업 전 수정 시각이 지금과 달라야 되돌린 뒤 같은지 볼 수 있다.
fn old_time() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000)
}

fn write_file(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    let file = fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_modified(old_time()).unwrap();
}

fn modified(path: &Path) -> SystemTime {
    fs::metadata(path).unwrap().modified().unwrap()
}

fn app_at(dir: &Path) -> App {
    let mut app = App::new(Some(Location::file_system(dir)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

/// 작업이 끝나거나 질문할 때까지 작업 사건을 받는다. 질문이면 그 질문을, 끝났으면 `None`을 돌려준다.
fn wait_question(app: &mut App, job: JobId) -> Option<JobQuestion> {
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll_jobs();
        app.poll();
        let entry = app.work_area().entry(job).expect("작업 영역의 작업");
        if let Some(question) = &entry.question {
            assert_eq!(entry.state, JobState::AwaitingAnswer);
            return Some(question.clone());
        }
        if entry.summary.is_some() {
            return None;
        }
        assert!(
            Instant::now() < deadline,
            "작업이 시간 안에 끝나지 않았다: {:?}",
            app.work_area()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// 작업이 끝날 때까지 기다린다. 그 사이 질문이 오면 실패한다.
fn wait_job(app: &mut App, job: JobId) -> (JobState, Summary) {
    if let Some(question) = wait_question(app, job) {
        panic!("묻지 않아야 한다: {question:?}");
    }
    let entry = app.work_area().entry(job).unwrap();
    (entry.state, entry.summary.clone().unwrap())
}

/// 휴지통을 쓸 수 있는 환경인지 보고, 아니면 건너뛴다고 출력한다.
fn recycle_bin_usable(dir: &Path, test: &str) -> bool {
    if job::has_recycle_bin(dir) {
        return true;
    }
    println!("건너뜀({test}): 임시 폴더의 볼륨에 휴지통이 없습니다");
    false
}

/// 휴지통 삭제를 넣고 끝날 때까지 기다린다. 셸이 휴지통에 넣지 않으려 하면(러너 계정이 휴지통을 쓸 수 없음)
/// 작업을 취소하고 건너뛴다고 출력하고 `None`.
fn recycle_all(app: &mut App, sources: Vec<PathBuf>, bin: &mut Bin, test: &str) -> Option<Summary> {
    let job = app.recycle(sources).expect("작업을 시작한다");
    match wait_question(app, job) {
        None => {}
        Some(JobQuestion::NotRecyclable(_)) => {
            app.answer_not_recyclable(job, NotRecyclableAnswer::CancelJob)
                .unwrap();
            let (_, summary) = wait_job(app, job);
            bin.keep(&summary);
            println!("건너뜀({test}): 이 계정은 휴지통을 쓸 수 없습니다(셸이 영구 삭제하려 함)");
            return None;
        }
        Some(other) => panic!("묻지 않아야 한다: {other:?}"),
    }
    let (state, summary) = wait_job(app, job);
    bin.keep(&summary);
    assert_eq!(state, JobState::Finished, "{summary:?}");
    Some(summary)
}

fn undo(app: &mut App, bin: &mut Bin) -> (JobState, Summary) {
    let job = app.undo().expect("실행취소를 시작한다");
    let (state, summary) = wait_job(app, job);
    bin.keep(&summary);
    (state, summary)
}

fn redo(app: &mut App, bin: &mut Bin) -> (JobState, Summary) {
    let job = app.redo().expect("다시 실행을 시작한다");
    let (state, summary) = wait_job(app, job);
    bin.keep(&summary);
    (state, summary)
}

/// 폴더 안 항목 이름(이름 순).
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// 다른 프로그램이 공유 없이 연 것처럼 파일을 잠근다. 돌려준 것을 버리면 풀린다.
fn lock(path: &Path) -> File {
    OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .unwrap()
}

/// 임시 폴더를 관리 공유(`\\localhost\C$`)로 본 경로: 네트워크 위치라 휴지통이 없다. 닿을 수 없으면 `None`.
fn admin_share(local: &Path) -> Option<PathBuf> {
    let absolute = std::path::absolute(local).unwrap();
    let text = absolute.to_string_lossy();
    let (drive, rest) = text.split_once(":\\")?;
    let shared = PathBuf::from(format!(r"\\localhost\{drive}$\{rest}"));
    shared.exists().then_some(shared)
}

#[test]
fn 원본이_없으면_작업을_만들지_않는다() {
    let mut queue = JobQueue::new();
    assert_eq!(queue.recycle(Vec::new()), Err(DeleteRejection::NoSources));
    assert!(queue.poll().is_empty(), "거부된 작업은 사건이 없다");
    assert!(!queue.has_unfinished());
}

#[test]
fn 위치에_휴지통이_있는지_미리_안다() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("a.txt");
    fs::write(&file, "a").unwrap();
    assert!(job::has_recycle_bin(dir.path()));
    assert!(job::has_recycle_bin(
        &dir.path().join("없는 폴더").join("b.txt")
    ));
    assert!(
        !job::deletes_permanently(&[]),
        "원본이 없으면 아무것도 지우지 않는다"
    );
    assert!(!job::deletes_permanently(std::slice::from_ref(&file)));

    let Some(shared) = admin_share(&file) else {
        println!("건너뜀(네트워크 위치): 관리 공유에 닿을 수 없습니다");
        return;
    };
    assert!(
        !job::has_recycle_bin(&shared),
        "네트워크 위치에는 휴지통이 없다"
    );
    assert!(job::deletes_permanently(std::slice::from_ref(&shared)));
    assert!(
        !job::deletes_permanently(&[shared, file]),
        "섞였으면 휴지통 삭제가 항목마다 묻는다"
    );
}

#[test]
fn 휴지통이_없는_위치에서_삭제하면_영구_삭제_작업이_된다() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("공유로 지울 파일.txt");
    fs::write(&file, "x").unwrap();
    let Some(shared) = admin_share(&file) else {
        println!("건너뜀(네트워크 위치 삭제): 관리 공유에 닿을 수 없습니다");
        return;
    };
    let mut app = app_at(dir.path());

    let job = app.delete(vec![shared]).unwrap();

    assert_eq!(app.work_area().entry(job).unwrap().kind, JobKind::Delete);
    let (state, summary) = wait_job(&mut app, job);
    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert!(!file.exists());
    assert_eq!(
        app.undo(),
        Err(UndoError::NotUndoable(NotUndoable::PermanentDelete))
    );
}

#[test]
fn 파일과_폴더를_휴지통_삭제한_뒤_실행취소하면_그대로_돌아오고_다시_실행하면_다시_사라진다() {
    let test = "휴지통 삭제 실행취소";
    let dir = TempDir::new().unwrap();
    if !recycle_bin_usable(dir.path(), test) {
        return;
    }
    let file = dir.path().join("보고서 🎉.txt");
    write_file(&file, "보고서");
    let folder = dir.path().join("사진");
    fs::create_dir_all(folder.join("2024")).unwrap();
    write_file(&folder.join("a.jpg"), "a");
    write_file(&folder.join("2024").join("b.jpg"), "bb");
    write_file(&dir.path().join("남는 파일.txt"), "남음");
    let mut app = app_at(dir.path());
    let mut bin = Bin::default();

    // Delete: 휴지통이 있는 위치라 휴지통 삭제 작업이다.
    let job = app.delete(vec![file.clone(), folder.clone()]).unwrap();
    assert_eq!(app.work_area().entry(job).unwrap().kind, JobKind::Recycle);
    let summary = match wait_question(&mut app, job) {
        None => {
            let (state, summary) = wait_job(&mut app, job);
            bin.keep(&summary);
            assert_eq!(state, JobState::Finished, "{summary:?}");
            summary
        }
        Some(JobQuestion::NotRecyclable(_)) => {
            app.answer_not_recyclable(job, NotRecyclableAnswer::CancelJob)
                .unwrap();
            let (_, summary) = wait_job(&mut app, job);
            bin.keep(&summary);
            println!("건너뜀({test}): 이 계정은 휴지통을 쓸 수 없습니다");
            return;
        }
        Some(other) => panic!("묻지 않아야 한다: {other:?}"),
    };

    assert_eq!(names(dir.path()), ["남는 파일.txt"]);
    assert_eq!(summary.recycled.len(), 2, "넘긴 원본마다 휴지통 항목 하나");
    assert_eq!(summary.items_done, 2);
    assert_eq!(summary.items_completed(), 2);
    assert_eq!(summary.items_remaining, 0);
    for item in &summary.recycled {
        assert!(item.recycled().exists(), "휴지통에 있다: {item:?}");
    }
    let record = app.history().undo_target().expect("작업 기록");
    assert_eq!(record.kind(), OperationKind::Recycle);
    assert!(record.is_undoable());
    assert_eq!(record.items_completed(), 2);
    assert!(
        matches!(&record.steps()[0], Step::Recycled { path, item, .. }
            if *path == file && item.original() == file)
    );

    let (state, undone) = undo(&mut app, &mut bin);

    assert_eq!(state, JobState::Finished, "{undone:?}");
    assert_eq!(
        names(dir.path()),
        ["남는 파일.txt", "보고서 🎉.txt", "사진"]
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "보고서");
    assert_eq!(modified(&file), old_time());
    assert_eq!(fs::read_to_string(folder.join("a.jpg")).unwrap(), "a");
    assert_eq!(modified(&folder.join("a.jpg")), old_time());
    assert_eq!(
        fs::read_to_string(folder.join("2024").join("b.jpg")).unwrap(),
        "bb"
    );
    assert_eq!(modified(&folder.join("2024").join("b.jpg")), old_time());
    for item in &summary.recycled {
        assert!(!item.recycled().exists(), "휴지통에서 나왔다: {item:?}");
    }
    assert!(app.history().undo_target().is_none());

    let (state, redone) = redo(&mut app, &mut bin);

    assert_eq!(state, JobState::Finished, "{redone:?}");
    assert_eq!(names(dir.path()), ["남는 파일.txt"]);
    assert_eq!(redone.recycled.len(), 2);
    let record = app.history().undo_target().expect("다시 기록으로");
    assert_eq!(record.kind(), OperationKind::Recycle);

    // 다시 실행한 것도 다시 되돌아온다(새 휴지통 항목에서).
    let (state, _) = undo(&mut app, &mut bin);
    assert_eq!(state, JobState::Finished);
    assert_eq!(fs::read_to_string(&file).unwrap(), "보고서");
}

#[test]
fn 실행취소_전에_원래_자리에_같은_이름이_생기면_그_항목만_건너뛰고_덮어쓰지_않는다() {
    let test = "원래 자리의 같은 이름";
    let dir = TempDir::new().unwrap();
    if !recycle_bin_usable(dir.path(), test) {
        return;
    }
    let taken = dir.path().join("같은 이름.txt");
    write_file(&taken, "옛 내용");
    let other = dir.path().join("다른 파일.txt");
    write_file(&other, "다른");
    let mut app = app_at(dir.path());
    let mut bin = Bin::default();
    let Some(summary) = recycle_all(&mut app, vec![taken.clone(), other.clone()], &mut bin, test)
    else {
        return;
    };
    let taken_item = summary
        .recycled
        .iter()
        .find(|item| item.original() == taken)
        .unwrap()
        .clone();
    fs::write(&taken, "새 내용").unwrap();

    let (state, undone) = undo(&mut app, &mut bin);

    assert_eq!(state, JobState::Failed);
    assert_eq!(undone.not_reverted.len(), 1, "{undone:?}");
    assert_eq!(undone.not_reverted[0].path, taken);
    assert_eq!(undone.not_reverted[0].reason, RevertSkip::NameTaken);
    assert_eq!(
        fs::read_to_string(&taken).unwrap(),
        "새 내용",
        "덮어쓰지 않는다"
    );
    assert!(taken_item.recycled().exists(), "휴지통 항목은 그대로다");
    assert_eq!(fs::read_to_string(&other).unwrap(), "다른");
    // 되돌리지 못한 항목은 기록에 남아 고친 뒤 다시 해 볼 수 있다.
    let record = app.history().undo_target().expect("남은 기록");
    assert_eq!(record.steps().len(), 1);
    assert_eq!(record.steps()[0].path(), taken);

    fs::remove_file(&taken).unwrap();
    let (state, _) = undo(&mut app, &mut bin);
    assert_eq!(state, JobState::Finished);
    assert_eq!(fs::read_to_string(&taken).unwrap(), "옛 내용");
}

#[test]
fn 휴지통_항목이_비워졌으면_실행취소가_그_항목을_건너뛰고_보고한다() {
    let test = "비운 휴지통";
    let dir = TempDir::new().unwrap();
    if !recycle_bin_usable(dir.path(), test) {
        return;
    }
    let file = dir.path().join("비울 것.txt");
    write_file(&file, "x");
    let mut app = app_at(dir.path());
    let mut bin = Bin::default();
    let Some(summary) = recycle_all(&mut app, vec![file.clone()], &mut bin, test) else {
        return;
    };
    bin::purge(&summary.recycled);

    let (state, undone) = undo(&mut app, &mut bin);

    assert_eq!(state, JobState::Failed);
    assert_eq!(undone.not_reverted.len(), 1);
    assert_eq!(undone.not_reverted[0].reason, RevertSkip::RecycledMissing);
    assert!(!file.exists());
}

/// 휴지통에 들어가지 않는 항목(주입)과 보통 항목.
fn refused_tree(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let refused = dir.join("휴지통에 안 들어감");
    fs::create_dir(&refused).unwrap();
    let big = refused.join("큰 파일.bin");
    write_file(&big, "big");
    let normal = dir.join("보통 파일.txt");
    write_file(&normal, "보통");
    (refused, big, normal)
}

#[test]
fn 휴지통에_들어가지_않는_항목은_묻고_건너뛰면_남는다() {
    let test = "휴지통에 안 들어감 건너뛰기";
    let dir = TempDir::new().unwrap();
    let (refused, big, normal) = refused_tree(dir.path());
    let _refuse = recycle::fault::refuse_recycle_under(&refused);
    let mut app = app_at(dir.path());
    let mut bin = Bin::default();

    let job = app.recycle(vec![big.clone(), normal.clone()]).unwrap();

    let Some(JobQuestion::NotRecyclable(item)) = wait_question(&mut app, job) else {
        panic!("휴지통에 들어가지 않는 항목을 묻는다");
    };
    assert_eq!(item.path, big);
    assert_eq!(item.kind, ItemKind::File);
    assert_eq!(item.size, Some(3));
    assert!(big.exists(), "묻는 동안 지우지 않는다");
    assert_eq!(
        app.answer_error(job, ErrorAnswer::Skip),
        Err(AnswerError::NotAsking),
        "다른 종류의 답은 받지 않는다"
    );
    app.answer_not_recyclable(job, NotRecyclableAnswer::Skip)
        .unwrap();
    assert_eq!(
        app.answer_not_recyclable(job, NotRecyclableAnswer::Skip),
        Err(AnswerError::NotAsking),
        "한 질문에는 한 번만 답한다"
    );
    if let Some(JobQuestion::NotRecyclable(item)) = wait_question(&mut app, job) {
        assert_eq!(item.path, normal);
        app.answer_not_recyclable(job, NotRecyclableAnswer::CancelJob)
            .unwrap();
        bin.keep(&wait_job(&mut app, job).1);
        println!("건너뜀({test}): 이 계정은 휴지통을 쓸 수 없습니다");
        return;
    }
    let (state, summary) = wait_job(&mut app, job);
    bin.keep(&summary);

    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert_eq!(
        fs::read_to_string(&big).unwrap(),
        "big",
        "건너뛴 항목은 남는다"
    );
    assert!(!normal.exists(), "뒤 항목은 휴지통으로 간다");
    assert_eq!(summary.not_recycled, vec![big.clone()]);
    assert_eq!(summary.items_skipped(), 1);
    assert_eq!(summary.items_completed(), 1);
    assert_eq!(summary.recycled.len(), 1);
    assert_eq!(summary.items_deleted, 0);
    let record = app.history().undo_target().unwrap();
    assert_eq!(record.kind(), OperationKind::Recycle);
    assert_eq!(record.items_skipped(), 1);
}

#[test]
fn 휴지통에_들어가지_않는_항목을_영구_삭제로_답하면_지워지고_그_항목만_되돌릴_수_없다() {
    let test = "휴지통에 안 들어감 영구 삭제";
    let dir = TempDir::new().unwrap();
    let (refused, _big, normal) = refused_tree(dir.path());
    fs::create_dir(refused.join("안 폴더")).unwrap();
    write_file(&refused.join("안 폴더").join("c.txt"), "c");
    let _refuse = recycle::fault::refuse_recycle_under(&refused);
    let mut app = app_at(dir.path());
    let mut bin = Bin::default();

    let job = app.recycle(vec![refused.clone(), normal.clone()]).unwrap();

    let Some(JobQuestion::NotRecyclable(item)) = wait_question(&mut app, job) else {
        panic!("휴지통에 들어가지 않는 항목을 묻는다");
    };
    assert_eq!(item.path, refused);
    assert_eq!(item.kind, ItemKind::Folder);
    app.answer_not_recyclable(job, NotRecyclableAnswer::DeletePermanently)
        .unwrap();
    if let Some(JobQuestion::NotRecyclable(_)) = wait_question(&mut app, job) {
        app.answer_not_recyclable(job, NotRecyclableAnswer::CancelJob)
            .unwrap();
        bin.keep(&wait_job(&mut app, job).1);
        println!("건너뜀({test}): 이 계정은 휴지통을 쓸 수 없습니다");
        return;
    }
    let (state, summary) = wait_job(&mut app, job);
    bin.keep(&summary);

    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert!(!refused.exists(), "영구 삭제로 지워진다");
    assert!(!normal.exists());
    assert!(dir.path().read_dir().unwrap().next().is_none());
    // 폴더 둘(휴지통에 안 들어감·안 폴더) + 파일 둘.
    assert_eq!(summary.items_deleted, 4);
    assert_eq!(summary.recycled.len(), 1);
    assert_eq!(summary.items_done, 2, "넘긴 원본 수로 센다");
    assert_eq!(summary.items_remaining, 0);
    let record = app.history().undo_target().unwrap();
    assert_eq!(record.kind(), OperationKind::Recycle);
    assert!(record.is_undoable(), "휴지통에 간 항목은 되돌아가야 한다");
    assert!(
        record
            .steps()
            .iter()
            .any(|step| matches!(step, Step::Deleted { path } if *path == refused))
    );

    let (state, undone) = undo(&mut app, &mut bin);

    assert_eq!(
        state,
        JobState::Failed,
        "영구 삭제한 항목은 되돌리지 못했다"
    );
    assert_eq!(fs::read_to_string(&normal).unwrap(), "보통");
    assert!(!refused.exists());
    assert_eq!(undone.not_reverted.len(), 1);
    assert_eq!(undone.not_reverted[0].path, refused);
    assert_eq!(
        undone.not_reverted[0].reason,
        RevertSkip::PermanentlyDeleted
    );
    assert!(
        app.history().undo_target().is_none(),
        "기록은 통째로 다시 실행 목록으로 간다"
    );

    let (state, _) = redo(&mut app, &mut bin);
    assert_eq!(state, JobState::Finished);
    assert!(!normal.exists());
}

#[test]
fn 모두_영구_삭제로_진행한_휴지통_삭제는_되돌릴_수_없는_기록이다() {
    let dir = TempDir::new().unwrap();
    let refused = dir.path().join("휴지통에 안 들어감");
    fs::create_dir(&refused).unwrap();
    let a = refused.join("a.txt");
    let b = refused.join("b.txt");
    write_file(&a, "a");
    write_file(&b, "b");
    let _refuse = recycle::fault::refuse_recycle_under(&refused);
    let mut app = app_at(dir.path());

    let job = app.recycle(vec![a.clone(), b.clone()]).unwrap();

    let Some(JobQuestion::NotRecyclable(item)) = wait_question(&mut app, job) else {
        panic!("묻는다");
    };
    assert_eq!(item.path, a);
    app.answer_not_recyclable(job, NotRecyclableAnswer::DeleteAllPermanently)
        .unwrap();
    let (state, summary) = wait_job(&mut app, job);
    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert!(
        !a.exists() && !b.exists(),
        "나머지에도 적용: 다시 묻지 않고 지운다"
    );
    assert_eq!(summary.items_deleted, 2);
    let record = app.history().undo_target().unwrap();
    assert_eq!(record.kind(), OperationKind::Delete);
    assert_eq!(
        record.not_undoable_reason(),
        Some(NotUndoable::PermanentDelete)
    );
    assert_eq!(
        app.undo(),
        Err(UndoError::NotUndoable(NotUndoable::PermanentDelete))
    );
}

#[test]
fn 휴지통에_들어가지_않는_항목에서_작업을_취소하면_그_항목과_뒤_항목은_남는다() {
    let dir = TempDir::new().unwrap();
    let (refused, big, normal) = refused_tree(dir.path());
    let _refuse = recycle::fault::refuse_recycle_under(&refused);
    let mut app = app_at(dir.path());

    let job = app.recycle(vec![big.clone(), normal.clone()]).unwrap();
    assert!(matches!(
        wait_question(&mut app, job),
        Some(JobQuestion::NotRecyclable(_))
    ));
    app.answer_not_recyclable(job, NotRecyclableAnswer::CancelJob)
        .unwrap();
    let (state, summary) = wait_job(&mut app, job);

    assert_eq!(state, JobState::Cancelled);
    assert!(big.exists() && normal.exists());
    assert_eq!(summary.items_remaining, 2);
    assert!(
        app.history().undo_target().is_none(),
        "한 것이 없으면 기록하지 않는다"
    );
}

#[test]
fn 잠긴_파일은_묻고_다시_시도하면_휴지통으로_가고_건너뛰면_남는다() {
    let test = "잠긴 파일";
    let dir = TempDir::new().unwrap();
    if !recycle_bin_usable(dir.path(), test) {
        return;
    }
    let locked = dir.path().join("잠긴 파일.txt");
    write_file(&locked, "잠김");
    let free = dir.path().join("자유 파일.txt");
    write_file(&free, "자유");
    let mut app = app_at(dir.path());
    let mut bin = Bin::default();

    // 다시 시도: 잠금을 푼 뒤 다시 시도하면 휴지통으로 간다.
    let guard = lock(&locked);
    let job = app.recycle(vec![locked.clone(), free.clone()]).unwrap();
    let failure = match wait_question(&mut app, job) {
        Some(JobQuestion::Error(failure)) => failure,
        Some(JobQuestion::NotRecyclable(_)) => {
            app.answer_not_recyclable(job, NotRecyclableAnswer::CancelJob)
                .unwrap();
            bin.keep(&wait_job(&mut app, job).1);
            println!("건너뜀({test}): 이 계정은 휴지통을 쓸 수 없습니다");
            return;
        }
        other => panic!("잠긴 파일은 항목별 오류로 묻는다: {other:?}"),
    };
    assert_eq!(failure.source, locked);
    assert_eq!(failure.kind, FileOpErrorKind::Locked);
    assert!(locked.exists(), "묻는 동안 그대로다");
    drop(guard);
    app.answer_error(job, ErrorAnswer::Retry).unwrap();
    let (state, summary) = wait_job(&mut app, job);
    bin.keep(&summary);
    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert!(!locked.exists() && !free.exists());
    assert_eq!(summary.recycled.len(), 2);
    let (state, _) = undo(&mut app, &mut bin);
    assert_eq!(state, JobState::Finished);
    assert_eq!(fs::read_to_string(&locked).unwrap(), "잠김");
    assert_eq!(fs::read_to_string(&free).unwrap(), "자유");

    // 건너뛰기: 잠긴 파일은 남고 작업은 실패로 끝난다.
    let guard = lock(&locked);
    let job = app.recycle(vec![locked.clone(), free.clone()]).unwrap();
    let Some(JobQuestion::Error(failure)) = wait_question(&mut app, job) else {
        panic!("다시 묻는다");
    };
    assert_eq!(failure.source, locked);
    app.answer_error(job, ErrorAnswer::Skip).unwrap();
    let (state, summary) = wait_job(&mut app, job);
    bin.keep(&summary);
    drop(guard);
    assert_eq!(state, JobState::Failed);
    assert_eq!(fs::read_to_string(&locked).unwrap(), "잠김");
    assert!(!free.exists());
    assert_eq!(summary.failures.len(), 1);
    assert_eq!(summary.failures[0].source, locked);
    assert_eq!(summary.items_skipped(), 1);

    // 작업 취소: 아무것도 지우지 않는다.
    write_file(&free, "자유");
    let guard = lock(&locked);
    let job = app.recycle(vec![locked.clone(), free.clone()]).unwrap();
    assert!(matches!(
        wait_question(&mut app, job),
        Some(JobQuestion::Error(_))
    ));
    app.answer_error(job, ErrorAnswer::CancelJob).unwrap();
    let (state, summary) = wait_job(&mut app, job);
    bin.keep(&summary);
    drop(guard);
    assert_eq!(state, JobState::Cancelled);
    assert!(locked.exists());
    // 셸은 한 번에 넘긴 뒤 항목을 이미 보냈을 수 있다(휴지통에 있다, 영구히 지우지 않는다).
    assert_eq!(free.exists(), summary.recycled.is_empty());
}
