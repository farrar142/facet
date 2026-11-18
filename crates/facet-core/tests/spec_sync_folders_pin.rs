// target: crates/facet-core/tests/spec_sync_folders_pin.rs
// AC: #455: 두 명령이 ALL에 있고 단축키 없음, id keep-on-device·free-up-space
// AC: #455: 「이 장치에 항상 유지」는 PINNED·배지 항상 유지, 「공간 확보」는 UNPINNED, 폴더는 안쪽까지, 작업 기록이 늘지 않음
// AC: #455: 동기화 폴더가 아니면 꺼지고 까닭 NotSyncFolder·팔레트에 꺼진 채, 원본이 없으면 꺼지고 까닭 None
// AC: #455: 동기화 폴더의 항목 메뉴는 삭제·구분선·두 명령·구분선·속성으로 끝남, 일반 폴더는 19줄 그대로
//! 「이 장치에 항상 유지」·「공간 확보」(`CommandId::KeepOnDevice`·`FreeUpSpace`, `JobKind`의 같은 이름,
//! `App::disabled_reason`, 동기화 폴더의 오른쪽 메뉴)를 확인한다. 명령 목록은 모든 플랫폼에서, 나머지는 시험용 동기화
//! 공급자(`facet_win::cloud_files::testing`)의 폴더로 Windows에서만 돈다. 동기화 루트를 등록할 수 없는 환경이면
//! 건너뛰었다고 출력하고, 환경 변수 `FACET_REQUIRE_SYNC_ROOT`가 있으면 그 경우를 실패로 본다.

use facet_core::command::CommandId;

#[test]
#[ignore = "#455"]
fn 두_명령은_팔레트에_있고_단축키가_없다() {
    for (command, id) in [
        (CommandId::KeepOnDevice, "keep-on-device"),
        (CommandId::FreeUpSpace, "free-up-space"),
    ] {
        assert!(CommandId::ALL.contains(&command), "{command:?}");
        assert!(command.shortcuts().is_empty(), "{command:?}");
        assert_eq!(command.as_str(), id);
        assert_eq!(CommandId::from_id(id), Some(command));
    }
}

#[cfg(windows)]
mod real {
    use std::fs;
    use std::os::windows::fs::MetadataExt;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use facet_core::Location;
    use facet_core::command::{CommandId, DisabledReason};
    use facet_core::job::{JobId, JobKind, JobState};
    use facet_core::menu::{MenuEntry, MenuKind};
    use facet_core::sync_status::SyncState;
    use facet_core::tab::ReadOutcome;
    use facet_core::window::{App, Execution};
    use facet_win::cloud_files::testing::TestSyncRoot;
    use tempfile::TempDir;

    const WAIT: Duration = Duration::from_secs(30);
    const ENV_REQUIRE: &str = "FACET_REQUIRE_SYNC_ROOT";
    const PINNED: u32 = 0x8_0000;
    const UNPINNED: u32 = 0x10_0000;

    fn sync_root(dir: &Path, test: &str) -> Option<TestSyncRoot> {
        match TestSyncRoot::register(dir) {
            Ok(root) => Some(root),
            Err(error) => {
                assert!(
                    std::env::var_os(ENV_REQUIRE).is_none(),
                    "{ENV_REQUIRE}가 있는데 동기화 루트를 등록하지 못했다: {error}"
                );
                println!("{test}: 동기화 루트를 등록할 수 없어 건너뜀: {error}");
                None
            }
        }
    }

    /// 상위 폴더의 열거 결과로 읽은 속성 값(파일을 열지 않는다).
    fn attributes_of(path: &Path) -> u32 {
        let name = path.file_name().unwrap();
        fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap())
            .find(|e| e.file_name() == name)
            .expect("목록에 있다")
            .metadata()
            .unwrap()
            .file_attributes()
    }

    fn app_at(path: &Path) -> App {
        let mut app = App::new(Some(Location::file_system(path)));
        let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
        assert_eq!(outcome, ReadOutcome::Committed);
        app
    }

    fn index_of(app: &App, name: &str) -> usize {
        app.active_tab()
            .items()
            .position(|item| item.name == name)
            .unwrap_or_else(|| panic!("{name}이 목록에 없다"))
    }

    fn select(app: &mut App, name: &str) {
        let index = index_of(app, name);
        app.active_tab_mut().select_only(index);
    }

    /// 명령을 실행해 그 종류의 새 작업이 끝날 때까지 기다린다.
    fn run(app: &mut App, command: CommandId, kind: JobKind) -> JobId {
        let before: Vec<JobId> = app.work_area().entries().iter().map(|e| e.id).collect();
        assert!(app.can_execute(command), "{command:?}");
        assert_eq!(app.execute(command), Execution::Done);
        let deadline = Instant::now() + WAIT;
        loop {
            app.poll_jobs();
            app.poll();
            let entry = app
                .work_area()
                .entries()
                .iter()
                .find(|e| !before.contains(&e.id));
            if let Some(entry) = entry {
                assert_eq!(entry.kind, kind);
                if entry.is_finished() {
                    assert_eq!(entry.state, JobState::Finished, "{:?}", entry.summary);
                    return entry.id;
                }
            }
            assert!(
                Instant::now() < deadline,
                "{command:?} 작업이 끝나지 않았다"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_state(app: &mut App, name: &str, expected: SyncState) {
        let deadline = Instant::now() + WAIT;
        loop {
            app.poll();
            let index = index_of(app, name);
            if app.active_tab().sync_state(index) == Some(expected) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{name}의 배지가 {expected:?}가 되지 않았다"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    #[ignore = "#455"]
    fn 항상_유지와_공간_확보가_고정_상태를_바꾸고_배지가_따라오며_기록에_남지_않는다() {
        let dir = TempDir::new().unwrap();
        let Some(root) = sync_root(dir.path(), "항상 유지·공간 확보") else {
            return;
        };
        let file = root.add_online_file("a.txt", b"a");
        let mut app = app_at(dir.path());
        let records = app.history().records().len();
        select(&mut app, "a.txt");

        run(&mut app, CommandId::KeepOnDevice, JobKind::KeepOnDevice);
        let bits = attributes_of(&file);
        assert_ne!(bits & PINNED, 0, "{bits:#x}");
        assert_eq!(bits & UNPINNED, 0, "{bits:#x}");
        wait_state(&mut app, "a.txt", SyncState::AlwaysKeep);

        select(&mut app, "a.txt");
        run(&mut app, CommandId::FreeUpSpace, JobKind::FreeUpSpace);
        let bits = attributes_of(&file);
        assert_ne!(bits & UNPINNED, 0, "{bits:#x}");
        assert_eq!(bits & PINNED, 0, "{bits:#x}");
        wait_state(&mut app, "a.txt", SyncState::OnlineOnly);

        assert_eq!(
            app.history().records().len(),
            records,
            "실행취소 대상이 아니다"
        );
    }

    #[test]
    #[ignore = "#455"]
    fn 폴더에_내리면_안의_항목에도_적용된다() {
        let dir = TempDir::new().unwrap();
        let Some(root) = sync_root(dir.path(), "폴더에 항상 유지") else {
            return;
        };
        let folder = root.add_online_folder("모음", &[("1.txt", b"1"), ("2.txt", b"2")]);
        // 펼쳐 둔다.
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 2);
        let mut app = app_at(dir.path());
        select(&mut app, "모음");
        run(&mut app, CommandId::KeepOnDevice, JobKind::KeepOnDevice);
        assert_ne!(attributes_of(&folder) & PINNED, 0);
        for name in ["1.txt", "2.txt"] {
            assert_ne!(attributes_of(&folder.join(name)) & PINNED, 0, "{name}");
        }
    }

    #[test]
    #[ignore = "#455"]
    fn 동기화_폴더가_아니면_까닭과_함께_꺼진다() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.txt"), b"a").unwrap();
        let mut app = app_at(dir.path());
        select(&mut app, "a.txt");
        for command in [CommandId::KeepOnDevice, CommandId::FreeUpSpace] {
            assert!(!app.can_execute(command), "{command:?}");
            assert_eq!(
                app.disabled_reason(command),
                Some(DisabledReason::NotSyncFolder)
            );
            let found = app.search_commands(command.as_str(), |c| c.as_str());
            let entry = found
                .iter()
                .find(|e| e.command == command)
                .expect("팔레트에 나온다");
            assert!(!entry.enabled, "{command:?}");
        }
        assert_eq!(app.disabled_reason(CommandId::Copy), None);
    }

    #[test]
    #[ignore = "#455"]
    fn 동기화_폴더에서_원본이_없으면_꺼지고_까닭은_없다() {
        let dir = TempDir::new().unwrap();
        let Some(_root) = sync_root(dir.path(), "원본 없음") else {
            return;
        };
        let app = app_at(dir.path());
        assert!(app.active_tab().is_sync_folder());
        assert_eq!(app.active_tab().len(), 0);
        for command in [CommandId::KeepOnDevice, CommandId::FreeUpSpace] {
            assert!(!app.can_execute(command), "{command:?}");
            assert_eq!(app.disabled_reason(command), None);
        }
    }

    fn tail(entries: &[MenuEntry], n: usize) -> Vec<Option<(CommandId, bool)>> {
        entries[entries.len() - n..]
            .iter()
            .map(|entry| match entry {
                MenuEntry::Command(c) => Some((c.id, c.enabled)),
                MenuEntry::Separator => None,
                MenuEntry::Submenu(s) => panic!("하위 메뉴가 없어야 한다: {s:?}"),
            })
            .collect()
    }

    #[test]
    #[ignore = "#455"]
    fn 동기화_폴더의_항목_메뉴에만_두_줄이_삭제_아래에_붙는다() {
        let dir = TempDir::new().unwrap();
        let Some(root) = sync_root(dir.path(), "메뉴") else {
            return;
        };
        root.add_online_file("a.txt", b"a");
        let mut app = app_at(dir.path());
        let index = index_of(&app, "a.txt");
        assert_eq!(app.context_click(0, Some(index)), Some(MenuKind::Item));
        let menu = app.context_menu(MenuKind::Item);
        assert_eq!(menu.entries.len(), 22);
        assert_eq!(
            tail(&menu.entries, 6),
            vec![
                Some((CommandId::Delete, true)),
                None,
                Some((CommandId::KeepOnDevice, true)),
                Some((CommandId::FreeUpSpace, true)),
                None,
                Some((CommandId::Properties, true)),
            ]
        );

        let plain = TempDir::new().unwrap();
        fs::write(plain.path().join("b.txt"), b"b").unwrap();
        let mut app = app_at(plain.path());
        let index = index_of(&app, "b.txt");
        assert_eq!(app.context_click(0, Some(index)), Some(MenuKind::Item));
        let menu = app.context_menu(MenuKind::Item);
        assert_eq!(menu.entries.len(), 19, "일반 폴더의 메뉴는 그대로다");
        assert!(
            !menu
                .commands()
                .any(|c| matches!(c.id, CommandId::KeepOnDevice | CommandId::FreeUpSpace))
        );
        assert_eq!(
            tail(&menu.entries, 3),
            vec![
                Some((CommandId::Delete, true)),
                None,
                Some((CommandId::Properties, true)),
            ]
        );
    }
}
