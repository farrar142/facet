//! 작업 기록과 실행취소·다시 실행: 명령과 단축키(Ctrl+Z·Ctrl+Y·작업 기록 보기)와 실행 가능 여부, 기록 100개 상한,
//! 새 작업 뒤 다시 실행 목록 비우기, 되돌릴 수 없는 마지막 기록, 그리고 임시 폴더의 실제 파일로 이름바꾸기·새 폴더·
//! 새 폴더+이름바꾸기(작업 하나)를 되돌리고 다시 하는 것과 밖의 변경(같은 이름, 비어 있지 않은 새 폴더)을 공개
//! 인터페이스로 확인한다.
//!
//! 앱([`App`])은 일꾼 스레드에서 바꾸고 실행취소는 작업 대기열에서 돌므로 결과를 기다린다. CI 러너가 느려도
//! 흔들리지 않도록 기다리는 시간([`WAIT`])을 넉넉히 두고, 결과가 오는 즉시 끝낸다.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use facet_core::command::{CommandId, Key, Shortcut, find_by_shortcut};
use facet_core::history::{
    HISTORY_LIMIT, History, NotUndoable, OperationKind, Record, Stamp, Step, UndoError,
};

fn stamp() -> Stamp {
    Stamp {
        is_folder: false,
        size: 0,
        modified: SystemTime::UNIX_EPOCH,
    }
}

fn rename_record(n: usize) -> Record {
    Record::new(
        OperationKind::Rename,
        vec![Step::Renamed {
            from: PathBuf::from(format!("a{n}")),
            to: PathBuf::from(format!("b{n}")),
            stamp: stamp(),
        }],
    )
}

#[test]
fn 실행취소는_ctrl_z_다시_실행은_ctrl_y이고_작업_기록_보기는_단축키가_없다() {
    assert_eq!(
        find_by_shortcut(Shortcut::new(Key::Letter('Z')).ctrl()),
        Some(CommandId::Undo)
    );
    assert_eq!(
        find_by_shortcut(Shortcut::new(Key::Letter('Y')).ctrl()),
        Some(CommandId::Redo)
    );
    // 한글 모드에서도 같은 키다(Z 키는 「ㅋ」, Y 키는 「ㅛ」).
    assert_eq!(Key::from_char('ㅋ'), Some(Key::Letter('Z')));
    assert_eq!(Key::from_char('ㅛ'), Some(Key::Letter('Y')));
    assert!(CommandId::ShowHistory.shortcuts().is_empty());
    for command in [CommandId::Undo, CommandId::Redo, CommandId::ShowHistory] {
        assert!(CommandId::ALL.contains(&command));
        assert_eq!(CommandId::from_id(command.as_str()), Some(command));
    }
}

#[test]
fn 기록은_100개까지이고_101번째에서_가장_오래된_것이_빠진다() {
    let mut history = History::new();
    let first = history.record(rename_record(0));
    for n in 1..HISTORY_LIMIT {
        history.record(rename_record(n));
    }
    assert_eq!(history.records().len(), HISTORY_LIMIT);
    assert_eq!(history.records().next().map(Record::id), Some(first));
    let last = history.record(rename_record(HISTORY_LIMIT));
    assert_eq!(history.records().len(), HISTORY_LIMIT);
    assert_ne!(history.records().next().map(Record::id), Some(first));
    assert_eq!(
        history.records().next().map(|r| r.steps()[0].clone()),
        Some(rename_record(1).steps()[0].clone()),
        "가장 오래된 기록 하나만 빠진다"
    );
    assert_eq!(history.undo_target().map(Record::id), Some(last));
}

#[test]
fn 실행취소_대상은_기록_100개에서도_곧바로_찾는다() {
    let mut history = History::new();
    for n in 0..HISTORY_LIMIT {
        history.record(rename_record(n));
    }
    let start = std::time::Instant::now();
    for _ in 0..10_000 {
        assert!(history.undo_target().is_some());
    }
    // 마지막 기록을 보는 것뿐이라 1만 번도 순간이다(넉넉한 상한, 벤치마크는 뒤 티켓).
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn 되돌릴_수_없는_기록은_까닭을_갖는다() {
    let record = Record::not_undoable(
        OperationKind::Rename,
        Vec::new(),
        NotUndoable::PermanentDelete,
    );
    assert!(!record.is_undoable());
    assert_eq!(
        record.not_undoable_reason(),
        Some(NotUndoable::PermanentDelete)
    );
    assert!(rename_record(0).is_undoable());
    let _ = UndoError::NotUndoable(NotUndoable::PermanentDelete);
}

#[cfg(windows)]
mod files {
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, Instant, SystemTime};

    use facet_core::Location;
    use facet_core::command::CommandId;
    use facet_core::edit::{EditError, EditKind, Edited};
    use facet_core::history::{
        NotReverted, NotUndoable, OperationKind, Record, RevertSkip, UndoError,
    };
    use facet_core::job::{JobId, JobKind, JobState, Summary};
    use facet_core::tab::ReadOutcome;
    use facet_core::window::{App, Execution, Update};
    use tempfile::TempDir;

    const WAIT: Duration = Duration::from_secs(10);

    /// 지난 시각(초 단위로 떨어지게): 작업 전 수정 시각이 지금과 달라야 되돌린 뒤 같은지 볼 수 있다.
    fn old_time() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000)
    }

    fn write_file(path: &Path, content: &str) {
        fs::write(path, content).unwrap();
        let file = fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(old_time()).unwrap();
    }

    fn app_at(dir: &Path) -> App {
        let mut app = App::new(Some(Location::file_system(dir)));
        let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
        assert_eq!(outcome, ReadOutcome::Committed);
        app
    }

    /// 이름바꾸기·새 폴더 결과가 올 때까지 받는다.
    fn wait_edited(app: &mut App) -> Edited {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "이름바꾸기·새 폴더 결과가 오지 않았다");
            if let Some((_, Update::Edited(result))) = app.wait_update(left) {
                return result.expect("이름바꾸기·새 폴더가 성공한다");
            }
        }
    }

    /// 작업이 끝날 때까지 작업 사건을 받고 결과를 돌려준다.
    fn wait_job(app: &mut App, job: JobId) -> (JobState, Summary) {
        let deadline = Instant::now() + WAIT;
        loop {
            app.poll_jobs();
            // 탭 쪽 결과(변경 감시)도 받아 둔다.
            app.poll();
            if let Some(entry) = app.work_area().entry(job)
                && let Some(summary) = &entry.summary
            {
                return (entry.state, summary.clone());
            }
            assert!(Instant::now() < deadline, "작업이 시간 안에 끝나지 않았다");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn rename(app: &mut App, from: &str, to: &str) -> Edited {
        app.rename(from.as_ref(), to)
            .expect("이름바꾸기를 시작한다");
        wait_edited(app)
    }

    fn undo(app: &mut App) -> (JobState, Summary) {
        let job = app.undo().expect("실행취소를 시작한다");
        wait_job(app, job)
    }

    fn redo(app: &mut App) -> (JobState, Summary) {
        let job = app.redo().expect("다시 실행을 시작한다");
        wait_job(app, job)
    }

    /// 폴더 안 항목: 이름(디스크 그대로), 내용(파일만), 수정 시각(파일만), 이름 순.
    fn snapshot(dir: &Path) -> Vec<(String, Option<String>, Option<SystemTime>)> {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let metadata = entry.metadata().unwrap();
                let name = entry.file_name().to_string_lossy().into_owned();
                if metadata.is_dir() {
                    (name, None, None)
                } else {
                    (
                        name,
                        Some(fs::read_to_string(entry.path()).unwrap()),
                        Some(metadata.modified().unwrap()),
                    )
                }
            })
            .collect();
        entries.sort();
        entries
    }

    #[test]
    fn 이름바꾸기를_실행취소하면_작업_전과_같고_다시_실행하면_작업_뒤와_같다() {
        let dir = TempDir::new().unwrap();
        write_file(&dir.path().join("보고서.txt"), "내용");
        let mut app = app_at(dir.path());
        assert!(!app.can_execute(CommandId::Undo));
        assert!(!app.can_execute(CommandId::Redo));
        assert!(!app.can_execute(CommandId::ShowHistory));
        let before = snapshot(dir.path());

        rename(&mut app, "보고서.txt", "결과.txt");
        let after = snapshot(dir.path());
        assert_ne!(before, after);
        let target = app.history().undo_target().expect("기록이 남는다");
        assert_eq!(target.kind(), OperationKind::Rename);
        assert!(app.can_execute(CommandId::Undo));
        assert!(!app.can_execute(CommandId::Redo));
        assert!(app.can_execute(CommandId::ShowHistory));
        assert_eq!(app.execute(CommandId::ShowHistory), Execution::ShowHistory);

        let (state, summary) = undo(&mut app);
        assert_eq!(state, JobState::Finished);
        assert_eq!(summary.items_completed(), 1);
        assert_eq!(
            snapshot(dir.path()),
            before,
            "이름·내용·수정 시각이 작업 전과 같다"
        );
        assert!(app.history().undo_target().is_none());
        assert!(app.can_execute(CommandId::Redo));
        assert!(!app.can_execute(CommandId::Undo));

        let (state, _) = redo(&mut app);
        assert_eq!(state, JobState::Finished);
        assert_eq!(snapshot(dir.path()), after, "다시 실행하면 작업 뒤와 같다");
        assert!(app.can_execute(CommandId::Undo));
        assert!(!app.can_execute(CommandId::Redo));
        assert_eq!(app.history().records().len(), 1);
    }

    #[test]
    fn 새_폴더를_실행취소하면_지워지고_다시_실행하면_다시_생긴다() {
        let dir = TempDir::new().unwrap();
        write_file(&dir.path().join("a.txt"), "a");
        let mut app = app_at(dir.path());
        let before = snapshot(dir.path());
        app.new_folder("새 폴더").unwrap();
        let edited = wait_edited(&mut app);
        assert_eq!(edited.kind, EditKind::NewFolder);
        let after = snapshot(dir.path());
        assert_eq!(
            app.history().undo_target().map(Record::kind),
            Some(OperationKind::NewFolder)
        );

        let (state, _) = undo(&mut app);
        assert_eq!(state, JobState::Finished);
        assert_eq!(snapshot(dir.path()), before);

        let (state, _) = redo(&mut app);
        assert_eq!(state, JobState::Finished);
        assert_eq!(snapshot(dir.path()), after);
        assert!(dir.path().join("새 폴더").is_dir());
    }

    #[test]
    fn 새_폴더_뒤_이름_입력란_확정은_작업_하나로_기록되고_함께_되돌아간다() {
        let dir = TempDir::new().unwrap();
        let mut app = app_at(dir.path());
        let before = snapshot(dir.path());
        app.new_folder("새 폴더").unwrap();
        wait_edited(&mut app);
        app.rename_new_folder("새 폴더".as_ref(), "사진").unwrap();
        wait_edited(&mut app);
        let after = snapshot(dir.path());
        assert_eq!(after, vec![("사진".to_string(), None, None)]);
        assert_eq!(app.history().records().len(), 1, "작업 하나로 남는다");
        assert_eq!(
            app.history().undo_target().map(Record::kind),
            Some(OperationKind::NewFolder)
        );

        let (state, _) = undo(&mut app);
        assert_eq!(state, JobState::Finished);
        assert_eq!(
            snapshot(dir.path()),
            before,
            "새 폴더도 이름바꾸기도 되돌아간다"
        );
        assert!(app.history().undo_target().is_none());

        let (state, _) = redo(&mut app);
        assert_eq!(state, JobState::Finished);
        assert_eq!(snapshot(dir.path()), after);
    }

    #[test]
    fn 새_폴더가_아닌_이름바꾸기는_묶지_않고_따로_기록한다() {
        let dir = TempDir::new().unwrap();
        write_file(&dir.path().join("a.txt"), "a");
        let mut app = app_at(dir.path());
        app.new_folder("새 폴더").unwrap();
        wait_edited(&mut app);
        app.rename_new_folder("a.txt".as_ref(), "b.txt").unwrap();
        wait_edited(&mut app);
        assert_eq!(app.history().records().len(), 2);
        assert_eq!(
            app.history().undo_target().map(Record::kind),
            Some(OperationKind::Rename)
        );
    }

    #[test]
    fn 원래_이름_자리에_같은_이름이_생기면_건너뛰고_까닭을_담고_덮어쓰지_않는다() {
        let dir = TempDir::new().unwrap();
        write_file(&dir.path().join("a.txt"), "원래");
        let mut app = app_at(dir.path());
        rename(&mut app, "a.txt", "b.txt");
        write_file(&dir.path().join("a.txt"), "새로 만든 것");
        let before = snapshot(dir.path());

        let (state, summary) = undo(&mut app);
        assert_eq!(state, JobState::Failed);
        assert_eq!(
            summary.not_reverted,
            vec![NotReverted {
                path: dir.path().join("b.txt"),
                reason: RevertSkip::NameTaken,
            }]
        );
        assert_eq!(summary.items_skipped(), 1);
        assert_eq!(snapshot(dir.path()), before, "아무것도 덮어쓰지 않는다");
        // 되돌리지 못한 기록은 남아 다시 해 볼 수 있고, 다시 실행할 것은 없다.
        assert!(app.history().undo_target().is_some());
        assert!(app.history().redo_target().is_none());

        // 막던 것을 치우면 다시 실행취소된다.
        fs::remove_file(dir.path().join("a.txt")).unwrap();
        let (state, _) = undo(&mut app);
        assert_eq!(state, JobState::Finished);
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "원래"
        );
    }

    #[test]
    fn 이름을_바꾼_파일이_그_뒤_바뀌었으면_되바꾸지_않는다() {
        let dir = TempDir::new().unwrap();
        write_file(&dir.path().join("a.txt"), "원래");
        let mut app = app_at(dir.path());
        rename(&mut app, "a.txt", "b.txt");
        fs::write(dir.path().join("b.txt"), "고쳐 쓴 내용").unwrap();
        let (state, summary) = undo(&mut app);
        assert_eq!(state, JobState::Failed);
        assert_eq!(summary.not_reverted[0].reason, RevertSkip::Changed);
        assert!(dir.path().join("b.txt").exists());
        assert!(!dir.path().join("a.txt").exists());
    }

    #[test]
    fn 파일을_넣은_새_폴더는_지우지_않고_보고한다() {
        let dir = TempDir::new().unwrap();
        let mut app = app_at(dir.path());
        app.new_folder("새 폴더").unwrap();
        wait_edited(&mut app);
        let folder = dir.path().join("새 폴더");
        write_file(&folder.join("안.txt"), "안");

        let (state, summary) = undo(&mut app);
        assert_eq!(state, JobState::Failed);
        assert_eq!(
            summary.not_reverted,
            vec![NotReverted {
                path: folder.clone(),
                reason: RevertSkip::NotEmpty,
            }]
        );
        assert_eq!(fs::read_to_string(folder.join("안.txt")).unwrap(), "안");
    }

    #[test]
    fn 마지막_기록이_되돌릴_수_없으면_까닭이_돌아오고_그_앞_작업은_되돌아가지_않는다() {
        let dir = TempDir::new().unwrap();
        write_file(&dir.path().join("a.txt"), "a");
        let mut app = app_at(dir.path());
        rename(&mut app, "a.txt", "b.txt");
        app.record_operation(Record::not_undoable(
            OperationKind::Rename,
            Vec::new(),
            NotUndoable::PermanentDelete,
        ));
        // 대상이 있으므로 명령은 켜져 있고, 실행하면 까닭을 돌려준다.
        assert!(app.can_execute(CommandId::Undo));
        let refused = UndoError::NotUndoable(NotUndoable::PermanentDelete);
        assert_eq!(
            app.execute(CommandId::Undo),
            Execution::UndoRefused(refused)
        );
        assert_eq!(app.undo(), Err(refused));
        assert!(app.work_area().is_empty(), "작업을 만들지 않는다");
        std::thread::sleep(Duration::from_millis(200));
        app.poll_jobs();
        assert!(dir.path().join("b.txt").exists());
        assert!(!dir.path().join("a.txt").exists());
        assert_eq!(app.history().records().len(), 2);
    }

    #[test]
    fn 새_작업이_기록되면_다시_실행_목록이_빈다() {
        let dir = TempDir::new().unwrap();
        write_file(&dir.path().join("a.txt"), "a");
        write_file(&dir.path().join("c.txt"), "c");
        let mut app = app_at(dir.path());
        rename(&mut app, "a.txt", "b.txt");
        undo(&mut app);
        assert!(app.history().redo_target().is_some());
        rename(&mut app, "c.txt", "d.txt");
        assert!(app.history().redo_target().is_none());
        assert!(!app.can_execute(CommandId::Redo));
        assert_eq!(app.redo(), Err(UndoError::Nothing));
        assert_eq!(app.execute(CommandId::Redo), Execution::Disabled);
    }

    #[test]
    fn 실행취소는_대기열의_작업으로_돌아_진행과_끝남이_온다() {
        let dir = TempDir::new().unwrap();
        write_file(&dir.path().join("a.txt"), "a");
        let mut app = app_at(dir.path());
        rename(&mut app, "a.txt", "b.txt");
        let job = app.undo().unwrap();
        // 도는 동안 다른 실행취소·다시 실행은 시작하지 않는다.
        if app.history().is_busy() {
            assert_eq!(app.undo(), Err(UndoError::Busy));
            assert!(!app.can_execute(CommandId::Undo));
        }
        let (state, summary) = wait_job(&mut app, job);
        let entry = app.work_area().entry(job).unwrap();
        assert_eq!(entry.kind, JobKind::Undo);
        assert_eq!(entry.target, dir.path());
        assert_eq!(state, JobState::Finished);
        let progress = entry.progress.as_ref().expect("진행 사건이 온다");
        assert_eq!(progress.items_done, 1);
        assert_eq!(progress.items_total, 1);
        assert!(progress.counted);
        assert_eq!(summary.items_done, 1);
        assert!(!app.history().is_busy());
        assert!(app.dismiss_job(job));

        let job = app.redo().unwrap();
        wait_job(&mut app, job);
        assert_eq!(app.work_area().entry(job).unwrap().kind, JobKind::Redo);
    }

    #[test]
    fn 실행취소의_파일_연산이_실패하면_항목별_오류로_묻는다() {
        use facet_core::job::{ErrorAnswer, JobQuestion};
        use std::os::windows::fs::OpenOptionsExt;

        let dir = TempDir::new().unwrap();
        write_file(&dir.path().join("a.txt"), "a");
        let mut app = app_at(dir.path());
        rename(&mut app, "a.txt", "b.txt");
        // 공유 없이 열어 이름을 바꿀 수 없게 한다(잠김).
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(dir.path().join("b.txt"))
            .unwrap();
        let job = app.undo().unwrap();
        let deadline = Instant::now() + WAIT;
        loop {
            app.poll_jobs();
            let entry = app.work_area().entry(job).unwrap();
            if let Some(JobQuestion::Error(failure)) = &entry.question {
                assert_eq!(failure.source, dir.path().join("b.txt"));
                break;
            }
            assert!(Instant::now() < deadline, "질문이 오지 않았다");
            std::thread::sleep(Duration::from_millis(10));
        }
        app.answer_error(job, ErrorAnswer::Skip).unwrap();
        let (state, summary) = wait_job(&mut app, job);
        drop(lock);
        assert_eq!(state, JobState::Failed);
        assert_eq!(summary.errors_skipped(), 1);
        assert!(dir.path().join("b.txt").exists());
        // 되돌리지 못한 기록은 남는다.
        assert!(app.history().undo_target().is_some());
    }

    #[test]
    fn 실패한_이름바꾸기는_기록하지_않는다() {
        let dir = TempDir::new().unwrap();
        write_file(&dir.path().join("a.txt"), "a");
        let mut app = app_at(dir.path());
        fs::remove_file(dir.path().join("a.txt")).unwrap();
        app.rename("a.txt".as_ref(), "b.txt").unwrap();
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero());
            if let Some((_, Update::Edited(result))) = app.wait_update(left) {
                assert!(matches!(result, Err(EditError::File(_))));
                break;
            }
        }
        assert!(app.history().is_empty());
    }
}
