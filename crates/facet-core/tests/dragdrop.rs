//! 끌어 놓기의 결과 결정과 드롭 작업 넣기를 공개 인터페이스에서 확인한다(Windows 전용).
//!
//! 결정 함수는 경로 글자만 보므로 가짜 경로로 모든 경우를 확인하고, 실제 경로의 볼륨 판단은 임시 폴더와 두 번째
//! 볼륨(`FACET_TEST_OTHER_VOLUME_DIR`, 없으면 건너뛰었다고 출력)으로 본다. 드롭으로 넣은 작업은 작업 영역에 보이고
//! 같은 폴더 복사는 「- 복사본」이며 이동 뒤 실행취소가 원래대로 돌린다.
#![cfg(windows)]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::dragdrop::{
    AllowedEffects, Decision, DropData, DropEffect, DropRequest, DropTarget, Modifiers, Refusal,
    decide, same_volume,
};
use facet_core::job::{JobId, JobKind, JobState, Summary};
use facet_core::tab::ReadOutcome;
use facet_core::window::App;
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(20);
const OTHER_VOLUME_ENV: &str = "FACET_TEST_OTHER_VOLUME_DIR";

struct Case {
    sources: Vec<PathBuf>,
    target: DropTarget,
    modifiers: Modifiers,
    right: bool,
    allowed: AllowedEffects,
    data: DropData,
}

impl Case {
    fn new(sources: &[&str], target: &str) -> Self {
        Self {
            sources: sources.iter().map(PathBuf::from).collect(),
            target: DropTarget::Folder(PathBuf::from(target)),
            modifiers: Modifiers::default(),
            right: false,
            allowed: AllowedEffects::ALL,
            data: DropData::Files,
        }
    }
    fn ctrl(mut self) -> Self {
        self.modifiers.ctrl = true;
        self
    }
    fn shift(mut self) -> Self {
        self.modifiers.shift = true;
        self
    }
    fn right(mut self) -> Self {
        self.right = true;
        self
    }
    fn allow(mut self, copy: bool, mov: bool) -> Self {
        self.allowed = AllowedEffects { copy, mov };
        self
    }
    fn decide(&self) -> Decision {
        decide(&DropRequest {
            sources: &self.sources,
            target: &self.target,
            modifiers: self.modifiers,
            right_button: self.right,
            allowed: self.allowed,
            data: self.data,
        })
    }
}

#[test]
fn 같은_볼륨은_이동_다른_볼륨은_복사() {
    assert_eq!(
        Case::new(&[r"C:\a\x.txt"], r"C:\b").decide(),
        Decision::Move
    );
    assert_eq!(
        Case::new(&[r"c:\a\x.txt"], r"C:\B").decide(),
        Decision::Move
    );
    assert_eq!(
        Case::new(&[r"C:\a\x.txt"], r"D:\b").decide(),
        Decision::Copy
    );
    // 하나라도 다른 볼륨이면 복사.
    assert_eq!(
        Case::new(&[r"C:\a\x", r"D:\a\y"], r"C:\b").decide(),
        Decision::Copy
    );
    assert_eq!(
        Case::new(&[r"\\srv\share\x"], r"\\SRV\Share\b").decide(),
        Decision::Move
    );
    assert_eq!(
        Case::new(&[r"\\srv\share\x"], r"\\srv\other\b").decide(),
        Decision::Copy
    );
}

#[test]
fn ctrl은_복사_shift는_이동_둘_다면_기본_효과() {
    assert_eq!(
        Case::new(&[r"C:\a\x"], r"C:\b").ctrl().decide(),
        Decision::Copy
    );
    assert_eq!(
        Case::new(&[r"C:\a\x"], r"D:\b").shift().decide(),
        Decision::Move
    );
    assert_eq!(
        Case::new(&[r"C:\a\x"], r"C:\b").ctrl().shift().decide(),
        Decision::Move
    );
    assert_eq!(
        Case::new(&[r"C:\a\x"], r"D:\b").ctrl().shift().decide(),
        Decision::Copy
    );
}

#[test]
fn 오른쪽_버튼은_수식_키와_상관없이_메뉴다() {
    let menu = Decision::Menu {
        copy: true,
        mov: true,
    };
    assert_eq!(Case::new(&[r"C:\a\x"], r"C:\b").right().decide(), menu);
    assert_eq!(
        Case::new(&[r"C:\a\x"], r"D:\b").right().ctrl().decide(),
        menu
    );
    // 허용되지 않은 효과는 메뉴에 없다.
    assert_eq!(
        Case::new(&[r"C:\a\x"], r"C:\b")
            .right()
            .allow(true, false)
            .decide(),
        Decision::Menu {
            copy: true,
            mov: false
        }
    );
    // 이미 그 폴더 안이면 이동 항목이 없고 복사만 있다.
    assert_eq!(
        Case::new(&[r"C:\a\x"], r"C:\a").right().decide(),
        Decision::Menu {
            copy: true,
            mov: false
        }
    );
    assert_eq!(
        Case::new(&[r"C:\a\x"], r"C:\a")
            .right()
            .allow(false, true)
            .decide(),
        Decision::Refuse(Refusal::AlreadyThere)
    );
}

#[test]
fn 자기_자신_하위_파일_위_파일_아닌_데이터는_놓을_수_없다() {
    assert_eq!(
        Case::new(&[r"C:\a\dir"], r"C:\a\dir").decide(),
        Decision::Refuse(Refusal::IntoItself)
    );
    assert_eq!(
        Case::new(&[r"C:\a\dir"], r"c:\A\DIR\inner\deep")
            .ctrl()
            .decide(),
        Decision::Refuse(Refusal::IntoItself)
    );
    assert_eq!(
        Case::new(&[r"C:\a\dir", r"C:\z"], r"C:\a\dir\x")
            .right()
            .decide(),
        Decision::Refuse(Refusal::IntoItself)
    );
    // 이름이 앞부분만 같은 다른 폴더는 하위가 아니다.
    assert_eq!(
        Case::new(&[r"C:\a\dir"], r"C:\a\dir2").decide(),
        Decision::Move
    );
    let mut on_file = Case::new(&[r"C:\a\x"], r"C:\b");
    on_file.target = DropTarget::NotAFolder;
    assert_eq!(on_file.decide(), Decision::Refuse(Refusal::NotAFolder));
    let mut text = Case::new(&[r"C:\a\x"], r"C:\b");
    text.data = DropData::Other;
    assert_eq!(text.decide(), Decision::Refuse(Refusal::NotFiles));
    let none = Case::new(&[], r"C:\b");
    assert_eq!(none.decide(), Decision::Refuse(Refusal::NoSources));
}

#[test]
fn 이미_그_폴더_안의_이동은_없던_일이고_복사는_허용된다() {
    assert_eq!(
        Case::new(&[r"C:\a\x", r"C:\a\y"], r"C:\a").decide(),
        Decision::Refuse(Refusal::AlreadyThere)
    );
    assert_eq!(
        Case::new(&[r"C:\a\x"], r"C:\a").shift().decide(),
        Decision::Refuse(Refusal::AlreadyThere)
    );
    assert_eq!(
        Case::new(&[r"C:\a\x"], r"C:\a").ctrl().decide(),
        Decision::Copy
    );
    // 일부만 그 폴더 안이면 나머지가 옮겨지므로 이동이다.
    assert_eq!(
        Case::new(&[r"C:\a\x", r"C:\b\y"], r"C:\a").decide(),
        Decision::Move
    );
}

#[test]
fn 원본_앱이_허용한_효과만_고른다() {
    // 이동만 허용: 다른 볼륨 기본(복사)도 이동으로 바뀐다.
    assert_eq!(
        Case::new(&[r"C:\a\x"], r"D:\b").allow(false, true).decide(),
        Decision::Move
    );
    // 복사만 허용: 같은 볼륨 기본(이동)·Shift도 복사로 바뀐다.
    assert_eq!(
        Case::new(&[r"C:\a\x"], r"C:\b").allow(true, false).decide(),
        Decision::Copy
    );
    assert_eq!(
        Case::new(&[r"C:\a\x"], r"C:\b")
            .shift()
            .allow(true, false)
            .decide(),
        Decision::Copy
    );
    // 복사를 허용하지 않고 이미 그 폴더 안이면 할 것이 없다.
    assert_eq!(
        Case::new(&[r"C:\a\x"], r"C:\a")
            .ctrl()
            .allow(false, true)
            .decide(),
        Decision::Refuse(Refusal::AlreadyThere)
    );
    assert_eq!(
        Case::new(&[r"C:\a\x"], r"C:\b")
            .allow(false, false)
            .decide(),
        Decision::Refuse(Refusal::NoEffectAllowed)
    );
}

#[test]
fn 실제_경로의_볼륨_판단() {
    let here = TempDir::new().unwrap();
    let also = TempDir::new().unwrap();
    assert!(same_volume(here.path(), also.path()));
    let Some(other) = env::var_os(OTHER_VOLUME_ENV) else {
        println!("건너뜀: {OTHER_VOLUME_ENV}가 없어 다른 볼륨의 판단을 확인하지 않았다");
        return;
    };
    let there = TempDir::new_in(PathBuf::from(other)).unwrap();
    assert!(!same_volume(here.path(), there.path()));
    let source = vec![here.path().join("x.txt")];
    let request = |target: &Path| {
        decide(&DropRequest {
            sources: &source,
            target: &DropTarget::Folder(target.to_path_buf()),
            modifiers: Modifiers::default(),
            right_button: false,
            allowed: AllowedEffects::ALL,
            data: DropData::Files,
        })
    };
    assert_eq!(request(there.path()), Decision::Copy);
    assert_eq!(request(also.path()), Decision::Move);
}

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
        assert!(Instant::now() < deadline, "작업이 끝나지 않았다");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn app_at(dir: &Path) -> App {
    let mut app = App::new(Some(Location::file_system(dir)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn 같은_폴더로_놓은_복사는_복사본이_되고_작업_영역에_보인다() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("보고서.txt");
    fs::write(&file, "내용").unwrap();
    let mut app = app_at(dir.path());
    let sources = vec![file.clone()];
    let decision = decide(&DropRequest {
        sources: &sources,
        target: &DropTarget::Folder(dir.path().to_path_buf()),
        modifiers: Modifiers {
            ctrl: true,
            shift: false,
        },
        right_button: false,
        allowed: AllowedEffects::ALL,
        data: DropData::Files,
    });
    assert_eq!(decision, Decision::Copy);
    let job = app
        .start_drop(DropEffect::Copy, sources, dir.path().to_path_buf())
        .unwrap();
    let entry = app.work_area().entry(job).expect("작업 영역에 보인다");
    assert_eq!(entry.kind, JobKind::Copy);
    let (state, _) = wait_job(&mut app, job);
    assert_eq!(state, JobState::Finished);
    assert_eq!(names(dir.path()), ["보고서 - 복사본.txt", "보고서.txt"]);
    assert_eq!(app.recent_targets(), [dir.path().to_path_buf()]);
}

#[test]
fn 드롭으로_이동한_뒤_실행취소하면_원래대로_돌아온다() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(src.join("a.txt"), "a").unwrap();
    let mut app = app_at(dir.path());
    let job = app
        .start_drop(DropEffect::Move, vec![src.join("a.txt")], dst.clone())
        .unwrap();
    let (state, _) = wait_job(&mut app, job);
    assert_eq!(state, JobState::Finished);
    assert_eq!(names(&dst), ["a.txt"]);
    assert!(names(&src).is_empty());

    let undo = app.undo().expect("실행취소를 시작한다");
    let (state, summary) = wait_job(&mut app, undo);
    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert_eq!(names(&src), ["a.txt"]);
    assert!(names(&dst).is_empty());
}

/// facet 항목을 끄는 길: 앱의 끄는 항목(선택)으로 끌기 원본의 데이터 개체를 만들고, 드롭 대상(다른 앱에서 온 끌기와
/// 같은 OLE 길)이 그 데이터 개체에서 읽은 파일로 결정해 작업을 넣는다.
struct AppDrop {
    app: std::rc::Rc<std::cell::RefCell<App>>,
    target: PathBuf,
    started: std::rc::Rc<std::cell::RefCell<Option<JobId>>>,
}

impl AppDrop {
    fn decision(
        &self,
        data: &facet_win::drop_target::DragData,
        state: &facet_win::drop_target::DragState,
    ) -> (Decision, Vec<PathBuf>) {
        let sources = data.files.clone().unwrap_or_default();
        let decision = decide(&DropRequest {
            sources: &sources,
            target: &DropTarget::Folder(self.target.clone()),
            modifiers: Modifiers {
                ctrl: state.keys.ctrl,
                shift: state.keys.shift,
            },
            right_button: state.keys.right_button,
            allowed: AllowedEffects {
                copy: state.allowed.copy,
                mov: state.allowed.mov,
            },
            data: if data.files.is_some() {
                DropData::Files
            } else {
                DropData::Other
            },
        });
        (decision, sources)
    }
}

fn win_effect(decision: Decision) -> facet_win::drop_target::Effect {
    match decision {
        Decision::Copy => facet_win::drop_target::Effect::Copy,
        Decision::Move => facet_win::drop_target::Effect::Move,
        _ => facet_win::drop_target::Effect::None,
    }
}

impl facet_win::drop_target::DropHandler for AppDrop {
    fn enter(
        &self,
        data: &facet_win::drop_target::DragData,
        state: &facet_win::drop_target::DragState,
    ) -> facet_win::drop_target::Effect {
        win_effect(self.decision(data, state).0)
    }
    fn over(&self, _: &facet_win::drop_target::DragState) -> facet_win::drop_target::Effect {
        facet_win::drop_target::Effect::None
    }
    fn leave(&self) {}
    fn drop(
        &self,
        data: &facet_win::drop_target::DragData,
        state: &facet_win::drop_target::DragState,
    ) -> facet_win::drop_target::Effect {
        let (decision, sources) = self.decision(data, state);
        let effect = match decision {
            Decision::Copy => DropEffect::Copy,
            Decision::Move => DropEffect::Move,
            _ => return facet_win::drop_target::Effect::None,
        };
        let job = self
            .app
            .borrow_mut()
            .start_drop(effect, sources, self.target.clone())
            .expect("작업을 넣는다");
        *self.started.borrow_mut() = Some(job);
        win_effect(decision)
    }
}

#[test]
fn facet_항목을_끌어_폴더에_놓으면_같은_볼륨은_이동이고_실행취소된다() {
    use facet_win::drag_source::DragObject;
    use facet_win::drop_target::testing::TestTarget;
    use facet_win::drop_target::{Allowed, DragKeys, Effect};
    use std::cell::RefCell;
    use std::rc::Rc;

    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(src.join("a.txt"), "a").unwrap();
    fs::write(src.join("b.txt"), "b").unwrap();
    let mut app = app_at(&src);
    app.active_tab_mut().select_all();
    let sources = app.drag_sources().expect("끄는 항목");
    assert_eq!(sources, [src.join("a.txt"), src.join("b.txt")]);

    let app = Rc::new(RefCell::new(app));
    let started = Rc::new(RefCell::new(None));
    let target = TestTarget::new(Box::new(AppDrop {
        app: Rc::clone(&app),
        target: dst.clone(),
        started: Rc::clone(&started),
    }))
    .unwrap();
    let object = DragObject::new(
        &sources,
        Allowed {
            copy: true,
            mov: true,
        },
    )
    .unwrap();
    let data = object.as_test_data().unwrap();
    let keys = DragKeys::default();
    assert_eq!(
        target.enter(&data, keys, (0, 0), object.allowed()),
        Effect::Move
    );
    // Ctrl은 복사로 바꾼다(놓지 않고 커서만 본다).
    let ctrl = DragKeys { ctrl: true, ..keys };
    target.leave();
    assert_eq!(
        target.enter(&data, ctrl, (0, 0), object.allowed()),
        Effect::Copy
    );
    // 놓으면 facet이 이동을 직접 하므로 원본 앱(여기서는 facet의 끌기 원본)에는 「없음」이 돌아간다.
    assert_eq!(
        target.drop(&data, keys, (0, 0), object.allowed()),
        Effect::None
    );
    assert_eq!(data.performed_effect(), Some(0));

    let job = started.borrow().expect("작업이 들어갔다");
    let mut app = app.borrow_mut();
    assert_eq!(app.work_area().entry(job).unwrap().kind, JobKind::Move);
    let (state, _) = wait_job(&mut app, job);
    assert_eq!(state, JobState::Finished);
    assert_eq!(names(&dst), ["a.txt", "b.txt"]);
    assert!(names(&src).is_empty());

    let undo = app.undo().expect("실행취소");
    let (state, summary) = wait_job(&mut app, undo);
    assert_eq!(state, JobState::Finished, "{summary:?}");
    assert_eq!(names(&src), ["a.txt", "b.txt"]);
    assert!(names(&dst).is_empty());
}

#[test]
fn facet_폴더를_자기_자신에_끌면_놓을_수_없다() {
    use facet_win::drag_source::DragObject;
    use facet_win::drop_target::testing::TestTarget;
    use facet_win::drop_target::{Allowed, DragKeys, Effect};
    use std::cell::RefCell;
    use std::rc::Rc;

    let dir = TempDir::new().unwrap();
    let folder = dir.path().join("폴더");
    fs::create_dir_all(folder.join("하위")).unwrap();
    let app = Rc::new(RefCell::new(app_at(dir.path())));
    let started = Rc::new(RefCell::new(None));
    for target_folder in [folder.clone(), folder.join("하위")] {
        let target = TestTarget::new(Box::new(AppDrop {
            app: Rc::clone(&app),
            target: target_folder,
            started: Rc::clone(&started),
        }))
        .unwrap();
        let object = DragObject::new(
            std::slice::from_ref(&folder),
            Allowed {
                copy: true,
                mov: true,
            },
        )
        .unwrap();
        let data = object.as_test_data().unwrap();
        let keys = DragKeys::default();
        assert_eq!(
            target.enter(&data, keys, (0, 0), object.allowed()),
            Effect::None
        );
        assert_eq!(
            target.drop(&data, keys, (0, 0), object.allowed()),
            Effect::None
        );
    }
    assert!(started.borrow().is_none(), "아무 작업도 넣지 않는다");
}
