// target: crates/facet-core/tests/spec_sync_folders_badges.rs
// AC: #451: SyncState::from_attributes(고정이 먼저, 온라인 속성 셋, 나머지는 이 장치에 있음)
// AC: #451: 시험용 동기화 공급자의 폴더를 연 탭은 동기화 폴더이고 배지 셋이 맞으며 목록 읽기의 내용·목록 요청이 0
// AC: #451: 동기화 폴더가 아니면 배지 없음, 상태 열 정렬 순서와 방향, 밖에서 고정·내려받기하면 새로 고침 없이 배지가 바뀜
//! 동기화 상태 배지(`sync_status::SyncState`, `Tab::is_sync_folder`·`sync_state`, `Column::SyncState`)를 확인한다.
//! 속성 값 판정은 순수 함수라 모든 플랫폼에서, 나머지는 시험용 동기화 공급자(`facet_win::cloud_files::testing`)로
//! 만든 진짜 온라인 전용 항목으로 Windows에서만 돈다. 동기화 루트를 등록할 수 없는 환경이면 건너뛰었다고 출력하고,
//! 환경 변수 `FACET_REQUIRE_SYNC_ROOT`가 있으면 그 경우를 실패로 본다.

use facet_core::sync_status::SyncState;

const ARCHIVE: u32 = 0x20;
const REPARSE_POINT: u32 = 0x400;
const OFFLINE: u32 = 0x1000;
const RECALL_ON_OPEN: u32 = 0x4_0000;
const PINNED: u32 = 0x8_0000;
const UNPINNED: u32 = 0x10_0000;
const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;

#[test]
fn attributes_decide_the_three_badges() {
    let cases = [
        (0, SyncState::OnDevice),
        (ARCHIVE | REPARSE_POINT, SyncState::OnDevice),
        (ARCHIVE | REPARSE_POINT | UNPINNED, SyncState::OnDevice),
        (RECALL_ON_DATA_ACCESS, SyncState::OnlineOnly),
        (OFFLINE, SyncState::OnlineOnly),
        (RECALL_ON_OPEN, SyncState::OnlineOnly),
        (
            ARCHIVE | REPARSE_POINT | RECALL_ON_DATA_ACCESS | UNPINNED,
            SyncState::OnlineOnly,
        ),
        (ARCHIVE | REPARSE_POINT | PINNED, SyncState::AlwaysKeep),
        // 고정했지만 아직 내려받지 않았어도 항상 유지다.
        (
            ARCHIVE | REPARSE_POINT | PINNED | RECALL_ON_DATA_ACCESS,
            SyncState::AlwaysKeep,
        ),
    ];
    for (bits, expected) in cases {
        assert_eq!(SyncState::from_attributes(bits), expected, "{bits:#x}");
    }
}

#[test]
fn state_order_runs_from_online_only_to_error() {
    // 속성으로 가른 셋이 그 순서이고, 셸 속성으로 받는 둘이 그 뒤다.
    let online = SyncState::from_attributes(RECALL_ON_DATA_ACCESS);
    let on_device = SyncState::from_attributes(ARCHIVE);
    let always_keep = SyncState::from_attributes(PINNED);
    assert_eq!(online, SyncState::OnlineOnly);
    assert_eq!(on_device, SyncState::OnDevice);
    assert_eq!(always_keep, SyncState::AlwaysKeep);
    assert!(online < on_device);
    assert!(on_device < always_keep);
    assert!(always_keep < SyncState::Syncing);
    assert!(SyncState::Syncing < SyncState::Error);
}

#[cfg(windows)]
mod real {
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use facet_core::Location;
    use facet_core::details::{Column, Direction, Sort};
    use facet_core::sync_status::SyncState;
    use facet_core::tab::ReadOutcome;
    use facet_core::window::App;
    use facet_win::cloud_files::testing::TestSyncRoot;
    use facet_win::cloud_files::{PinState, set_pin_state};
    use tempfile::TempDir;

    const WAIT: Duration = Duration::from_secs(20);
    const ENV_REQUIRE: &str = "FACET_REQUIRE_SYNC_ROOT";

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

    /// 온라인 전용 파일 둘, 펼쳐지지 않은 온라인 폴더, 이 장치에 있는 파일, 고정한 파일.
    fn populate(root: &TestSyncRoot) {
        root.add_online_file("온라인 1.txt", b"one");
        root.add_online_file("온라인 2.txt", b"two");
        root.add_online_folder("온라인 폴더", &[("안.txt", b"in")]);
        fs::write(root.root().join("로컬.txt"), b"local").unwrap();
        let pinned = root.add_online_file("고정.txt", b"pin");
        set_pin_state(&pinned, PinState::Pinned, false).unwrap();
    }

    fn app_at(path: &Path) -> App {
        let mut app = App::new(Some(Location::file_system(path)));
        let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
        assert_eq!(outcome, ReadOutcome::Committed);
        app
    }

    fn state_of(app: &App, name: &str) -> Option<SyncState> {
        let tab = app.active_tab();
        let index = tab
            .items()
            .position(|item| item.name == name)
            .unwrap_or_else(|| panic!("{name}이 목록에 없다"));
        tab.sync_state(index)
    }

    /// 그 항목의 배지가 `expected`가 될 때까지 받는다(새로 고침은 하지 않는다).
    fn wait_state(app: &mut App, name: &str, expected: SyncState) {
        let deadline = Instant::now() + WAIT;
        loop {
            app.poll();
            if state_of(app, name) == Some(expected) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{name}의 배지가 {expected:?}가 되지 않았다: {:?}",
                state_of(app, name)
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn sync_folder_items_get_badges_without_fetching() {
        let dir = TempDir::new().unwrap();
        let Some(root) = sync_root(dir.path(), "배지 셋") else {
            return;
        };
        populate(&root);
        let app = app_at(dir.path());
        assert!(app.active_tab().is_sync_folder());
        assert_eq!(app.active_tab().len(), 5);
        assert_eq!(state_of(&app, "온라인 1.txt"), Some(SyncState::OnlineOnly));
        assert_eq!(state_of(&app, "온라인 2.txt"), Some(SyncState::OnlineOnly));
        assert_eq!(state_of(&app, "온라인 폴더"), Some(SyncState::OnlineOnly));
        assert_eq!(state_of(&app, "로컬.txt"), Some(SyncState::OnDevice));
        assert_eq!(state_of(&app, "고정.txt"), Some(SyncState::AlwaysKeep));
        assert_eq!(
            root.fetch_data_count(),
            0,
            "목록 읽기는 내용을 요청하지 않는다"
        );
        assert_eq!(
            root.fetch_placeholders_count(),
            0,
            "하위 온라인 폴더를 펼치지 않는다"
        );
    }

    #[test]
    fn no_badges_outside_sync_folders() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.txt"), b"a").unwrap();
        fs::create_dir(dir.path().join("폴더")).unwrap();
        let app = app_at(dir.path());
        let tab = app.active_tab();
        assert!(!tab.is_sync_folder());
        assert_eq!(tab.len(), 2);
        for index in 0..tab.len() {
            assert_eq!(tab.sync_state(index), None);
        }
    }

    #[test]
    fn sorting_by_state_groups_and_reverses() {
        let dir = TempDir::new().unwrap();
        let Some(root) = sync_root(dir.path(), "상태 열 정렬") else {
            return;
        };
        populate(&root);
        let mut app = app_at(dir.path());
        let states = |app: &App| -> Vec<SyncState> {
            let tab = app.active_tab();
            (0..tab.len())
                .map(|i| tab.sync_state(i).expect("동기화 폴더의 배지"))
                .collect()
        };
        app.set_sort(Sort {
            column: Column::SyncState,
            direction: Direction::Ascending,
        });
        let ascending = states(&app);
        assert_eq!(
            ascending,
            vec![
                SyncState::OnlineOnly,
                SyncState::OnlineOnly,
                SyncState::OnlineOnly,
                SyncState::OnDevice,
                SyncState::AlwaysKeep,
            ]
        );
        app.set_sort(Sort {
            column: Column::SyncState,
            direction: Direction::Descending,
        });
        let mut descending = states(&app);
        descending.reverse();
        assert_eq!(descending, ascending);
    }

    #[test]
    fn badges_follow_outside_pin_and_download_without_refresh() {
        let dir = TempDir::new().unwrap();
        let Some(root) = sync_root(dir.path(), "배지 따라가기") else {
            return;
        };
        populate(&root);
        let mut app = app_at(dir.path());
        assert_eq!(state_of(&app, "온라인 1.txt"), Some(SyncState::OnlineOnly));
        set_pin_state(&dir.path().join("온라인 1.txt"), PinState::Pinned, false).unwrap();
        wait_state(&mut app, "온라인 1.txt", SyncState::AlwaysKeep);

        assert_eq!(state_of(&app, "온라인 2.txt"), Some(SyncState::OnlineOnly));
        assert_eq!(fs::read(dir.path().join("온라인 2.txt")).unwrap(), b"two");
        wait_state(&mut app, "온라인 2.txt", SyncState::OnDevice);

        set_pin_state(&dir.path().join("고정.txt"), PinState::Unpinned, false).unwrap();
        wait_state(&mut app, "고정.txt", SyncState::OnlineOnly);
    }
}
