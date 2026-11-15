//! 복사·이동의 작업 기록과 실행취소·다시 실행을 임시 폴더의 실제 파일과 실제 휴지통으로 공개 인터페이스에서
//! 확인한다(Windows 전용).
//!
//! - 복사를 실행취소하면 만든 항목이 휴지통에 가고 대상이 작업 전과 같으며, 다시 실행하면 작업 뒤와 같다.
//! - 같은 볼륨 이동을 실행취소하면 같은 파일 ID로 원래 자리에 돌아오고, 다시 실행하면 다시 옮겨진다. 볼륨을 넘는
//!   이동은 두 번째 볼륨의 폴더(`FACET_TEST_OTHER_VOLUME_DIR`)가 있을 때만 돈다.
//! - 밖의 변경(고친 사본, 원래 자리의 같은 이름)은 그 항목만 건너뛰고 까닭을 보고하며 아무것도 덮어쓰지 않는다.
//! - 폴더 합치기 복사의 실행취소는 합친 폴더와 원래 있던 항목을 남긴다.
//! - 덮어쓰기(옛 파일을 먼저 휴지통으로)의 실행취소는 옛 파일을 되살리고 새 파일을 치우며(복사면 휴지통으로, 이동이면
//!   원래 자리로), 다시 실행은 다시 덮어쓴다. 휴지통에 넣을 수 없어 영구 덮어쓴 작업은 되돌릴 수 없다.
//!
//! 러너 계정이 휴지통을 쓸 수 없으면(볼륨에 휴지통이 없거나 셸이 휴지통에 넣지 않음) 건너뛰었다고 출력한다.
//! 테스트가 휴지통에 넣은 항목은 테스트가 끝날 때(실패해도) 휴지통에서 지운다.
#![cfg(windows)]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use facet_core::Location;
use facet_core::history::{NotUndoable, OperationKind, RevertSkip, Step, UndoError};
use facet_core::job::{ConflictAnswer, JobId, JobKind, JobQuestion, JobState, Resolution, Summary};
use facet_core::tab::ReadOutcome;
use facet_core::window::{App, TransferPlan};
use facet_win::file_ops;
use facet_win::recycle::{self, RecycledItem};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(20);
const OTHER_VOLUME_ENV: &str = "FACET_TEST_OTHER_VOLUME_DIR";

/// 테스트가 휴지통에 넣은 항목. 버릴 때 휴지통에 남아 있으면 내용(`$R...`)과 정보 파일(`$I...`)을 지운다.
#[derive(Default)]
struct Bin(Vec<RecycledItem>);

impl Bin {
    fn keep(&mut self, summary: &Summary) {
        self.0.extend(summary.recycled.iter().cloned());
    }
}

impl Drop for Bin {
    fn drop(&mut self) {
        for item in &self.0 {
            let recycled = item.recycled();
            if recycled.is_dir() {
                let _ = fs::remove_dir_all(recycled);
            } else {
                let _ = fs::remove_file(recycled);
            }
            let name = recycled.file_name().unwrap().to_string_lossy();
            let _ = fs::remove_file(recycled.with_file_name(name.replacen("$R", "$I", 1)));
        }
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

/// 폴더 아래 전부(상대 경로 순): 상대 경로, 내용(파일만), 수정 시각(파일만).
type Snapshot = Vec<(String, Option<String>, Option<SystemTime>)>;

fn snapshot(dir: &Path) -> Snapshot {
    let mut entries = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(folder) = stack.pop() {
        for entry in fs::read_dir(&folder).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let name = path
                .strip_prefix(dir)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let metadata = entry.metadata().unwrap();
            if metadata.is_dir() {
                stack.push(path);
                entries.push((name, None, None));
            } else {
                entries.push((
                    name,
                    Some(fs::read_to_string(&path).unwrap()),
                    Some(metadata.modified().unwrap()),
                ));
            }
        }
    }
    entries.sort();
    entries
}

fn app_at(dir: &Path) -> App {
    let mut app = App::new(Some(Location::file_system(dir)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

/// 작업이 끝날 때까지 작업 사건을 받고 결과를 돌려준다.
fn wait_job(app: &mut App, job: JobId) -> (JobState, Summary) {
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll_jobs();
        app.poll();
        if let Some(entry) = app.work_area().entry(job)
            && let Some(summary) = &entry.summary
        {
            return (entry.state, summary.clone());
        }
        assert!(
            Instant::now() < deadline,
            "작업이 시간 안에 끝나지 않았다: {:?}",
            app.work_area()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn transfer(app: &mut App, kind: JobKind, sources: &[PathBuf], target: &Path) -> Summary {
    let plan = TransferPlan {
        kind,
        sources: sources.to_vec(),
        target: target.to_path_buf(),
    };
    let job = app.start_transfer(&plan).expect("작업을 시작한다");
    let (state, summary) = wait_job(app, job);
    assert_eq!(state, JobState::Finished, "{summary:?}");
    summary
}

fn undo(app: &mut App) -> (JobState, Summary) {
    let job = app.undo().expect("실행취소를 시작한다");
    wait_job(app, job)
}

fn redo(app: &mut App) -> (JobState, Summary) {
    let job = app.redo().expect("다시 실행을 시작한다");
    wait_job(app, job)
}

/// 휴지통을 쓸 수 있는 환경인지 보고, 아니면 건너뛴다고 출력한다.
fn recycle_bin_usable(dir: &Path, test: &str) -> bool {
    if recycle::has_recycle_bin(dir) {
        return true;
    }
    println!("건너뜀({test}): 임시 폴더의 볼륨에 휴지통이 없습니다");
    false
}

/// 실행취소가 셸이 휴지통에 넣지 않아(러너 계정이 휴지통을 쓸 수 없음) 건너뛰었으면 그렇다고 출력하고 `true`.
fn bin_refused(summary: &Summary, test: &str) -> bool {
    if summary
        .not_reverted
        .iter()
        .any(|n| n.reason == RevertSkip::NotRecyclable)
    {
        println!("건너뜀({test}): 이 계정은 휴지통을 쓸 수 없습니다(셸이 영구 삭제하려 함)");
        return true;
    }
    false
}

/// `root/src`(파일 하나, 중첩 폴더), 빈 `root/dst`.
fn tree() -> TempDir {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(src.join("사진").join("2024")).unwrap();
    write_file(&src.join("보고서.txt"), "보고서");
    write_file(&src.join("사진").join("a.jpg"), "a");
    write_file(&src.join("사진").join("2024").join("b.jpg"), "bb");
    fs::create_dir(dir.path().join("dst")).unwrap();
    dir
}

fn sources(dir: &TempDir) -> Vec<PathBuf> {
    let src = dir.path().join("src");
    vec![src.join("보고서.txt"), src.join("사진")]
}

#[test]
fn 복사를_실행취소하면_대상이_작업_전과_같고_사본이_휴지통에_있으며_다시_실행하면_작업_뒤와_같다() {
    let dir = tree();
    if !recycle_bin_usable(dir.path(), "복사 실행취소") {
        return;
    }
    let mut bin = Bin::default();
    let dst = dir.path().join("dst");
    let src_before = snapshot(&dir.path().join("src"));
    let mut app = app_at(dir.path());
    let before = snapshot(&dst);

    transfer(&mut app, JobKind::Copy, &sources(&dir), &dst);
    let after = snapshot(&dst);
    assert_eq!(after.len(), 5);
    let record = app.history().undo_target().expect("복사가 기록된다");
    assert_eq!(record.kind(), OperationKind::Copy);
    assert!(record.is_undoable());
    assert_eq!(record.items_completed(), 5, "만든 항목마다 하나씩");
    assert_eq!(record.top_steps().len(), 2);
    for step in record.steps() {
        let Step::Copied {
            source,
            path,
            stamp,
        } = step
        else {
            panic!("복사의 항목이어야 한다: {step:?}");
        };
        assert_eq!(
            path.strip_prefix(&dst).unwrap(),
            source.strip_prefix(dir.path().join("src")).unwrap()
        );
        assert_eq!(stamp.is_folder, path.is_dir());
    }

    let (state, summary) = undo(&mut app);
    bin.keep(&summary);
    if bin_refused(&summary, "복사 실행취소") {
        return;
    }
    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert_eq!(summary.items_completed(), 5);
    assert_eq!(snapshot(&dst), before, "대상이 작업 전과 같다");
    assert_eq!(
        snapshot(&dir.path().join("src")),
        src_before,
        "원본은 그대로다"
    );
    // 맨 위 항목 둘(파일, 폴더 통째로)이 휴지통에 있다.
    assert_eq!(summary.recycled.len(), 2, "{:?}", summary.recycled);
    for item in &summary.recycled {
        assert!(item.recycled().exists(), "휴지통에 있다: {item:?}");
    }
    let folder = summary
        .recycled
        .iter()
        .find(|item| item.original() == dst.join("사진"))
        .expect("폴더는 통째로 휴지통에 간다");
    assert!(folder.recycled().join("2024").join("b.jpg").is_file());

    let (state, summary) = redo(&mut app);
    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert_eq!(
        snapshot(&dst),
        after,
        "이름·내용·수정 시각이 작업 뒤와 같다"
    );
    assert_eq!(app.history().records().len(), 1);
    assert!(app.history().redo_target().is_none());
}

#[test]
fn 같은_폴더_복사본과_둘_다_유지의_이름을_기록한다() {
    let dir = tree();
    if !recycle_bin_usable(dir.path(), "복사본 이름") {
        return;
    }
    let mut bin = Bin::default();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    write_file(&dst.join("보고서.txt"), "원래 있던 것");
    let mut app = app_at(dir.path());

    // 같은 폴더로 복사: 「- 복사본」.
    transfer(&mut app, JobKind::Copy, &[src.join("보고서.txt")], &src);
    let record = app.history().undo_target().unwrap();
    assert_eq!(
        record.steps().iter().map(Step::path).collect::<Vec<_>>(),
        vec![src.join("보고서 - 복사본.txt").as_path()]
    );

    // 이름 충돌에 둘 다 유지: 「이름 (2)」.
    let plan = TransferPlan {
        kind: JobKind::Copy,
        sources: vec![src.join("보고서.txt")],
        target: dst.clone(),
    };
    let job = app.start_transfer(&plan).unwrap();
    answer_conflict(&mut app, job, Resolution::KeepBoth);
    let (state, _) = wait_job(&mut app, job);
    assert_eq!(state, JobState::Finished);
    let record = app.history().undo_target().unwrap();
    assert!(record.is_undoable());
    assert_eq!(
        record.steps().iter().map(Step::path).collect::<Vec<_>>(),
        vec![dst.join("보고서 (2).txt").as_path()]
    );
    let (state, summary) = undo(&mut app);
    bin.keep(&summary);
    if bin_refused(&summary, "복사본 이름") {
        return;
    }
    assert_eq!(state, JobState::Finished);
    assert!(!dst.join("보고서 (2).txt").exists());
    assert_eq!(
        fs::read_to_string(dst.join("보고서.txt")).unwrap(),
        "원래 있던 것"
    );
}

/// 작업이 이름 충돌을 물을 때까지 기다려 답한다.
fn answer_conflict(app: &mut App, job: JobId, resolution: Resolution) {
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll_jobs();
        if let Some(JobQuestion::Conflict(_)) =
            app.work_area().entry(job).and_then(|e| e.question.as_ref())
        {
            break;
        }
        assert!(Instant::now() < deadline, "질문이 오지 않았다");
        std::thread::sleep(Duration::from_millis(10));
    }
    app.answer_conflict(job, ConflictAnswer::once(resolution))
        .unwrap();
}

#[test]
fn 복사_뒤_고친_사본만_남고_나머지는_되돌아가며_까닭이_보고된다() {
    let dir = tree();
    if !recycle_bin_usable(dir.path(), "고친 사본") {
        return;
    }
    let mut bin = Bin::default();
    let dst = dir.path().join("dst");
    let mut app = app_at(dir.path());
    transfer(&mut app, JobKind::Copy, &sources(&dir), &dst);
    // 사본 하나를 고친다(크기·수정 시각이 기록과 다르다).
    let changed = dst.join("사진").join("2024").join("b.jpg");
    fs::write(&changed, "고친 내용").unwrap();

    let (state, summary) = undo(&mut app);
    bin.keep(&summary);
    if bin_refused(&summary, "고친 사본") {
        return;
    }
    assert_eq!(
        state,
        JobState::Failed,
        "건너뛴 항목이 있으면 실패로 끝난다"
    );
    assert_eq!(
        snapshot(&dst)
            .into_iter()
            .map(|(name, content, _)| (name, content))
            .collect::<Vec<_>>(),
        vec![
            ("사진".to_string(), None),
            (r"사진\2024".to_string(), None),
            (
                r"사진\2024\b.jpg".to_string(),
                Some("고친 내용".to_string())
            ),
        ],
        "고친 사본과 그것을 담은 폴더만 남는다"
    );
    let reasons: Vec<_> = summary
        .not_reverted
        .iter()
        .map(|n| (n.path.clone(), n.reason))
        .collect();
    assert!(
        reasons.contains(&(changed.clone(), RevertSkip::Changed)),
        "{reasons:?}"
    );
    assert!(
        reasons.contains(&(dst.join("사진"), RevertSkip::NotEmpty)),
        "{reasons:?}"
    );
    assert!(
        reasons.contains(&(dst.join("사진").join("2024"), RevertSkip::NotEmpty)),
        "{reasons:?}"
    );
    // 되돌린 항목(보고서.txt, a.jpg)은 휴지통에, 되돌리지 못한 항목은 기록에 남는다.
    assert_eq!(summary.recycled.len(), 2, "{:?}", summary.recycled);
    let left = app
        .history()
        .undo_target()
        .expect("못 한 항목이 기록에 남는다");
    assert_eq!(left.items_completed(), 3);
    let redo_target = app
        .history()
        .redo_target()
        .expect("되돌린 항목은 다시 실행 목록에");
    assert_eq!(redo_target.items_completed(), 2);
    assert_eq!(
        fs::read_to_string(&changed).unwrap(),
        "고친 내용",
        "고친 사본은 그대로다"
    );
}

#[test]
fn 폴더_합치기_복사를_실행취소하면_합친_폴더와_원래_있던_항목은_남는다() {
    let dir = tree();
    if !recycle_bin_usable(dir.path(), "합치기 복사") {
        return;
    }
    let mut bin = Bin::default();
    let dst = dir.path().join("dst");
    fs::create_dir(dst.join("사진")).unwrap();
    write_file(&dst.join("사진").join("원래.jpg"), "원래");
    let before = snapshot(&dst);
    let mut app = app_at(dir.path());
    let summary = transfer(
        &mut app,
        JobKind::Copy,
        &[dir.path().join("src").join("사진")],
        &dst,
    );
    assert_eq!(summary.folders_merged, 1);
    let record = app.history().undo_target().unwrap();
    assert!(
        record
            .steps()
            .iter()
            .all(|step| step.path() != dst.join("사진")),
        "합친 폴더는 만든 것이 아니라 기록에 없다"
    );

    let (state, summary) = undo(&mut app);
    bin.keep(&summary);
    if bin_refused(&summary, "합치기 복사") {
        return;
    }
    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert_eq!(
        snapshot(&dst),
        before,
        "합친 폴더와 원래 있던 항목은 남는다"
    );
}

/// 다른 시각: 덮어쓸 옛 파일의 수정 시각(원본의 [`old_time`]과 달라야 되돌린 뒤 옛 파일인지 본다).
fn older_time() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000)
}

fn write_old_file(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    let file = fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_modified(older_time()).unwrap();
}

/// 이름 충돌에 덮어쓰기로 답하는 복사·이동 하나를 끝까지 하고 결과를 돌려준다.
fn overwrite(app: &mut App, kind: JobKind, source: &Path, target: &Path) -> Summary {
    let plan = TransferPlan {
        kind,
        sources: vec![source.to_path_buf()],
        target: target.to_path_buf(),
    };
    let job = app.start_transfer(&plan).unwrap();
    answer_conflict(app, job, Resolution::Overwrite);
    let (state, summary) = wait_job(app, job);
    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert_eq!(summary.files_overwritten, 1, "{summary:?}");
    summary
}

const READONLY: u32 = 0x1;

fn attributes(path: &Path) -> u32 {
    use std::os::windows::fs::MetadataExt;
    fs::metadata(path).unwrap().file_attributes()
}

fn set_readonly(path: &Path) {
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions).unwrap();
}

/// 덮어쓴 파일이 휴지통 항목(원래 경로가 `path`)으로 휴지통에 있고 내용이 `content`다.
fn in_bin<'a>(summary: &'a Summary, path: &Path, content: &str) -> &'a RecycledItem {
    let item = summary
        .recycled
        .iter()
        .find(|item| item.original() == path)
        .unwrap_or_else(|| panic!("{} 이 휴지통에 가야 한다: {summary:?}", path.display()));
    assert_eq!(fs::read_to_string(item.recycled()).unwrap(), content);
    item
}

#[test]
fn 덮어쓰기_복사를_실행취소하면_옛_파일이_돌아오고_새_파일은_휴지통에_있으며_다시_실행하면_작업_뒤와_같다()
 {
    let dir = tree();
    if !recycle_bin_usable(dir.path(), "덮어쓰기 복사 실행취소") {
        return;
    }
    let mut bin = Bin::default();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    let target = dst.join("보고서.txt");
    write_old_file(&target, "원래 있던 것");
    let before = snapshot(&dst);
    let mut app = app_at(dir.path());

    let summary = overwrite(&mut app, JobKind::Copy, &src.join("보고서.txt"), &dst);
    bin.keep(&summary);
    // 덮어쓰기 전에 옛 파일이 휴지통으로 갔다.
    in_bin(&summary, &target, "원래 있던 것");
    let after = snapshot(&dst);
    assert_eq!(fs::read_to_string(&target).unwrap(), "보고서");
    assert_eq!(after.len(), 1, "임시 파일이 남지 않는다: {after:?}");
    let record = app.history().undo_target().expect("복사가 기록된다");
    assert_eq!(record.kind(), OperationKind::Copy);
    assert!(
        record.is_undoable(),
        "휴지통에 간 덮어쓰기는 되돌릴 수 있다"
    );
    assert_eq!(record.not_undoable_reason(), None);
    assert!(
        matches!(record.steps(), [Step::Overwrote { moved: false, .. }]),
        "{:?}",
        record.steps()
    );

    let (state, summary) = undo(&mut app);
    bin.keep(&summary);
    if bin_refused(&summary, "덮어쓰기 복사 실행취소") {
        return;
    }
    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert_eq!(
        snapshot(&dst),
        before,
        "옛 파일이 내용·수정 시각까지 돌아온다"
    );
    in_bin(&summary, &target, "보고서");

    let (state, summary) = redo(&mut app);
    bin.keep(&summary);
    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert_eq!(snapshot(&dst), after, "작업 뒤와 같다");
    in_bin(&summary, &target, "원래 있던 것");
    assert_eq!(app.history().records().len(), 1);
    assert!(app.history().undo_target().unwrap().is_undoable());

    // 다시 실행한 것도 다시 되돌린다.
    let (state, summary) = undo(&mut app);
    bin.keep(&summary);
    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert_eq!(snapshot(&dst), before);
}

#[test]
fn 덮어쓰기_이동을_실행취소하면_원본_자리에_옮긴_파일이_대상_자리에_옛_파일이_돌아온다() {
    let dir = tree();
    if !recycle_bin_usable(dir.path(), "덮어쓰기 이동 실행취소") {
        return;
    }
    let mut bin = Bin::default();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    let source = src.join("보고서.txt");
    let target = dst.join("보고서.txt");
    write_old_file(&target, "원래 있던 것");
    let src_before = snapshot(&src);
    let dst_before = snapshot(&dst);
    let file_id = file_ops::file_id(&source).unwrap();
    let mut app = app_at(dir.path());

    let summary = overwrite(&mut app, JobKind::Move, &source, &dst);
    bin.keep(&summary);
    in_bin(&summary, &target, "원래 있던 것");
    assert!(!source.exists());
    assert_eq!(fs::read_to_string(&target).unwrap(), "보고서");
    let src_after = snapshot(&src);
    let dst_after = snapshot(&dst);
    let record = app.history().undo_target().unwrap();
    assert_eq!(record.kind(), OperationKind::Move);
    assert!(record.is_undoable());
    assert!(matches!(
        record.steps(),
        [Step::Overwrote { moved: true, .. }]
    ));

    let (state, summary) = undo(&mut app);
    bin.keep(&summary);
    if bin_refused(&summary, "덮어쓰기 이동 실행취소") {
        return;
    }
    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert_eq!(snapshot(&src), src_before, "옮긴 파일이 원본 자리로");
    assert_eq!(snapshot(&dst), dst_before, "옛 파일이 대상 자리로");
    assert_eq!(
        file_ops::file_id(&source).unwrap(),
        file_id,
        "같은 파일이 되옮겨진다"
    );

    let (state, summary) = redo(&mut app);
    bin.keep(&summary);
    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert_eq!(snapshot(&src), src_after);
    assert_eq!(snapshot(&dst), dst_after);
    assert_eq!(file_ops::file_id(&target).unwrap(), file_id);
    in_bin(&summary, &target, "원래 있던 것");
}

#[test]
fn 읽기_전용_대상을_덮어쓴_복사도_실행취소로_되돌아온다() {
    let dir = tree();
    if !recycle_bin_usable(dir.path(), "읽기 전용 덮어쓰기") {
        return;
    }
    let mut bin = Bin::default();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    let target = dst.join("보고서.txt");
    write_old_file(&target, "읽기 전용");
    set_readonly(&target);
    let before = snapshot(&dst);
    let mut app = app_at(dir.path());

    let summary = overwrite(&mut app, JobKind::Copy, &src.join("보고서.txt"), &dst);
    bin.keep(&summary);
    assert_eq!(fs::read_to_string(&target).unwrap(), "보고서");
    assert_eq!(attributes(&target) & READONLY, 0, "결과 속성은 원본 것");
    assert!(app.history().undo_target().unwrap().is_undoable());

    let (state, summary) = undo(&mut app);
    bin.keep(&summary);
    if bin_refused(&summary, "읽기 전용 덮어쓰기") {
        return;
    }
    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert_eq!(snapshot(&dst), before);
    assert_ne!(attributes(&target) & READONLY, 0, "읽기 전용도 돌아온다");
    let mut permissions = fs::metadata(&target).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    fs::set_permissions(&target, permissions).unwrap();
}

#[test]
fn 덮어쓴_뒤_새_파일을_고치면_실행취소가_그_항목을_건너뛰고_보고하며_아무것도_덮어쓰지_않는다() {
    let dir = tree();
    if !recycle_bin_usable(dir.path(), "고친 새 파일") {
        return;
    }
    let mut bin = Bin::default();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    let target = dst.join("보고서.txt");
    write_old_file(&target, "원래 있던 것");
    let mut app = app_at(dir.path());
    let summary = overwrite(&mut app, JobKind::Copy, &src.join("보고서.txt"), &dst);
    bin.keep(&summary);
    let old = in_bin(&summary, &target, "원래 있던 것").clone();

    fs::write(&target, "그 뒤 고친 내용").unwrap();
    let (state, summary) = undo(&mut app);
    bin.keep(&summary);
    assert_eq!(state, JobState::Failed, "{summary:?}");
    assert_eq!(summary.not_reverted.len(), 1, "{summary:?}");
    assert_eq!(summary.not_reverted[0].path, target);
    assert_eq!(summary.not_reverted[0].reason, RevertSkip::Changed);
    assert!(
        summary.recycled.is_empty(),
        "아무것도 휴지통에 보내지 않는다"
    );
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "그 뒤 고친 내용",
        "고친 파일은 그대로"
    );
    assert_eq!(
        fs::read_to_string(old.recycled()).unwrap(),
        "원래 있던 것",
        "옛 파일은 휴지통에 그대로"
    );
    // 되돌리지 못한 항목은 기록에 남는다.
    assert!(app.history().undo_target().is_some());
    assert!(app.history().redo_target().is_none());
}

#[test]
fn 덮어쓴_옛_파일의_휴지통_항목이_없으면_실행취소가_건너뛰고_새_파일을_남긴다() {
    let dir = tree();
    if !recycle_bin_usable(dir.path(), "비운 휴지통") {
        return;
    }
    let mut bin = Bin::default();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    let target = dst.join("보고서.txt");
    write_old_file(&target, "원래 있던 것");
    let mut app = app_at(dir.path());
    let summary = overwrite(&mut app, JobKind::Copy, &src.join("보고서.txt"), &dst);
    // 휴지통을 비운 것처럼 옛 파일의 휴지통 항목을 지운다.
    bin.keep(&summary);
    drop(bin);
    let mut bin = Bin::default();

    let (state, summary) = undo(&mut app);
    bin.keep(&summary);
    assert_eq!(state, JobState::Failed, "{summary:?}");
    assert_eq!(summary.not_reverted.len(), 1, "{summary:?}");
    assert_eq!(summary.not_reverted[0].reason, RevertSkip::RecycledMissing);
    assert_eq!(fs::read_to_string(&target).unwrap(), "보고서");
    assert!(summary.recycled.is_empty());
}

#[test]
fn 휴지통에_넣을_수_없는_덮어쓰기는_영구_덮어쓰기이고_되돌릴_수_없다고_기록된다() {
    let dir = tree();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    write_file(&dst.join("보고서.txt"), "원래 있던 것");
    // 셸이 휴지통 대신 영구 삭제하려는 것으로 본다(휴지통보다 큰 파일, 휴지통을 끈 볼륨).
    let _refuse = recycle::fault::refuse_recycle_under(&dst);
    let mut app = app_at(dir.path());
    let summary = overwrite(&mut app, JobKind::Copy, &src.join("보고서.txt"), &dst);
    assert!(summary.recycled.is_empty(), "{summary:?}");
    assert_eq!(names(&dst), ["보고서.txt"], "임시 파일이 남지 않는다");
    let record = app.history().undo_target().unwrap();
    assert_eq!(record.kind(), OperationKind::Copy);
    assert_eq!(record.not_undoable_reason(), Some(NotUndoable::Overwritten));
    assert_eq!(
        app.undo(),
        Err(UndoError::NotUndoable(NotUndoable::Overwritten))
    );
    assert_eq!(
        fs::read_to_string(dst.join("보고서.txt")).unwrap(),
        "보고서"
    );

    // 이동도 같다.
    write_file(&src.join("옮길.txt"), "옮길 것");
    write_file(&dst.join("옮길.txt"), "원래 있던 것");
    overwrite(&mut app, JobKind::Move, &src.join("옮길.txt"), &dst);
    let record = app.history().undo_target().unwrap();
    assert_eq!(record.kind(), OperationKind::Move);
    assert_eq!(record.not_undoable_reason(), Some(NotUndoable::Overwritten));
    assert_eq!(app.history().records().len(), 2);
}

fn names(folder: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn 아무것도_하지_않은_작업은_기록하지_않는다() {
    let dir = tree();
    let src = dir.path().join("src");
    let mut app = app_at(dir.path());
    // 같은 폴더로 이동은 아무것도 하지 않는다.
    transfer(&mut app, JobKind::Move, &[src.join("보고서.txt")], &src);
    assert_eq!(app.history().records().len(), 0);
}

#[test]
fn 같은_볼륨_이동을_실행취소하면_같은_파일_id로_돌아오고_다시_실행하면_다시_옮겨진다() {
    let dir = tree();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    let file_id = file_ops::file_id(&src.join("보고서.txt")).unwrap();
    let folder_id = file_ops::file_id(&src.join("사진")).unwrap();
    let inner_id = file_ops::file_id(&src.join("사진").join("a.jpg")).unwrap();
    let src_before = snapshot(&src);
    let dst_before = snapshot(&dst);
    let mut app = app_at(dir.path());

    let summary = transfer(&mut app, JobKind::Move, &sources(&dir), &dst);
    assert_eq!(summary.items_renamed, 2);
    let src_after = snapshot(&src);
    let dst_after = snapshot(&dst);
    let record = app.history().undo_target().unwrap();
    assert_eq!(record.kind(), OperationKind::Move);
    assert!(record.is_undoable());
    assert_eq!(record.steps().len(), 2, "이름 바꾸기 한 번이면 항목 하나다");
    assert!(
        record
            .steps()
            .iter()
            .all(|s| matches!(s, Step::Moved { .. }))
    );

    let (state, summary) = undo(&mut app);
    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert_eq!(snapshot(&src), src_before);
    assert_eq!(snapshot(&dst), dst_before);
    assert_eq!(file_ops::file_id(&src.join("보고서.txt")).unwrap(), file_id);
    assert_eq!(file_ops::file_id(&src.join("사진")).unwrap(), folder_id);
    assert_eq!(
        file_ops::file_id(&src.join("사진").join("a.jpg")).unwrap(),
        inner_id
    );

    let (state, summary) = redo(&mut app);
    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert_eq!(snapshot(&src), src_after);
    assert_eq!(snapshot(&dst), dst_after);
    assert_eq!(file_ops::file_id(&dst.join("보고서.txt")).unwrap(), file_id);
    assert_eq!(file_ops::file_id(&dst.join("사진")).unwrap(), folder_id);
}

#[test]
fn 합친_폴더_이동을_실행취소하면_원래_폴더를_다시_만들어_되옮긴다() {
    let dir = tree();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    fs::create_dir(dst.join("사진")).unwrap();
    write_file(&dst.join("사진").join("원래.jpg"), "원래");
    let src_before = snapshot(&src);
    let dst_before = snapshot(&dst);
    let mut app = app_at(dir.path());
    let summary = transfer(&mut app, JobKind::Move, &[src.join("사진")], &dst);
    assert_eq!(summary.folders_merged, 1);
    assert!(!src.join("사진").exists(), "비운 원래 폴더는 지워졌다");
    let dst_after = snapshot(&dst);

    let (state, summary) = undo(&mut app);
    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert_eq!(
        snapshot(&src)
            .into_iter()
            .map(|(name, content, _)| (name, content))
            .collect::<Vec<_>>(),
        src_before
            .iter()
            .cloned()
            .map(|(name, content, _)| (name, content))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        snapshot(&dst),
        dst_before,
        "합친 폴더와 원래 있던 항목은 남는다"
    );

    let (state, summary) = redo(&mut app);
    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert_eq!(snapshot(&dst), dst_after);
    assert!(!src.join("사진").exists());
}

#[test]
fn 이동_뒤_원래_자리에_같은_이름이_생기면_그_항목만_건너뛰고_덮어쓰지_않는다() {
    let dir = tree();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    let mut app = app_at(dir.path());
    transfer(&mut app, JobKind::Move, &sources(&dir), &dst);
    write_file(&src.join("보고서.txt"), "새로 만든 것");

    let (state, summary) = undo(&mut app);
    assert_eq!(state, JobState::Failed);
    assert_eq!(summary.not_reverted.len(), 1, "{summary:?}");
    assert_eq!(summary.not_reverted[0].path, dst.join("보고서.txt"));
    assert_eq!(summary.not_reverted[0].reason, RevertSkip::NameTaken);
    assert_eq!(
        fs::read_to_string(src.join("보고서.txt")).unwrap(),
        "새로 만든 것",
        "덮어쓰지 않는다"
    );
    assert_eq!(
        fs::read_to_string(dst.join("보고서.txt")).unwrap(),
        "보고서"
    );
    assert!(
        src.join("사진").join("2024").join("b.jpg").is_file(),
        "나머지는 되돌아간다"
    );
    assert!(!dst.join("사진").exists());
}

/// 두 번째 볼륨의 빈 시험 폴더. 환경 변수가 없으면 건너뛴다고 출력하고 `None`.
fn other_volume(test: &str) -> Option<TempDir> {
    let Some(other) = env::var_os(OTHER_VOLUME_ENV) else {
        println!("건너뜀: {OTHER_VOLUME_ENV}가 없어 볼륨을 넘는 이동({test})을 확인하지 않았다");
        return None;
    };
    let dir = TempDir::new_in(PathBuf::from(other)).unwrap();
    let here = file_ops::file_id(&env::temp_dir()).unwrap();
    let there = file_ops::file_id(dir.path()).unwrap();
    assert_ne!(
        here.volume_serial, there.volume_serial,
        "{OTHER_VOLUME_ENV}가 임시 폴더와 같은 볼륨이다"
    );
    Some(dir)
}

#[test]
fn 볼륨을_넘는_이동을_실행취소하면_원래_자리로_돌아오고_다시_실행하면_다시_옮겨진다() {
    let Some(other) = other_volume("실행취소") else {
        return;
    };
    let dir = tree();
    let src = dir.path().join("src");
    let dst = other.path();
    let src_before = snapshot(&src);
    let mut app = app_at(dir.path());
    let summary = transfer(&mut app, JobKind::Move, &sources(&dir), dst);
    assert_eq!(
        summary.items_renamed, 0,
        "볼륨을 넘으면 복사 뒤 원본 삭제다"
    );
    assert!(!src.join("사진").exists());
    let dst_after = snapshot(dst);

    let (state, summary) = undo(&mut app);
    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert_eq!(
        snapshot(&src),
        src_before,
        "이름·내용·수정 시각이 작업 전과 같다"
    );
    assert!(
        snapshot(dst).is_empty(),
        "이동이 만든 대상 폴더는 비어 지워진다"
    );

    let (state, summary) = redo(&mut app);
    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert_eq!(snapshot(dst), dst_after);
    assert!(!src.join("사진").exists());
    assert!(!src.join("보고서.txt").exists());
}
