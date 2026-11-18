//! 내 PC의 용량 열(#351): 사용량 몫, 정렬 명령, 내 PC 감시(장치 붙임·뗌 알림으로 다시 읽기).
//! 인수 시험(시스템 드라이브 값·정렬 순서)은 `spec_virtual_folders_drive_space.rs`에 있다.

use facet_core::command::CommandId;
use facet_core::details::Column;
use facet_core::shell_provider::DriveSpace;

#[test]
fn 사용량_몫은_쓴_공간의_비율이고_모르면_없다() {
    let space = |total, free| DriveSpace { total, free };
    assert_eq!(space(Some(100), Some(25)).used_fraction(), Some(0.75));
    assert_eq!(space(Some(100), Some(100)).used_fraction(), Some(0.0));
    // 할당량 등으로 남은 공간이 전체보다 크게 와도 0 아래로 내려가지 않는다.
    assert_eq!(space(Some(100), Some(200)).used_fraction(), Some(0.0));
    assert_eq!(space(Some(0), Some(0)).used_fraction(), None);
    assert_eq!(space(None, Some(1)).used_fraction(), None);
    assert_eq!(DriveSpace::UNKNOWN.used_fraction(), None);
}

#[test]
fn 용량_열마다_정렬_명령이_있고_팔레트에는_없다() {
    for column in [Column::TotalSize, Column::FreeSpace] {
        let command = CommandId::sort_by(column);
        assert_eq!(command.sort_column(), Some(column));
        assert!(command.shortcuts().is_empty());
        assert!(!CommandId::ALL.contains(&command), "{command:?}");
        assert_eq!(CommandId::from_id(command.as_str()), None);
    }
}

#[cfg(windows)]
mod windows {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{Duration, Instant};

    use facet_core::command::CommandId;
    use facet_core::details::{Column, Direction, Sort};
    use facet_core::tab::{AutoRefresh, ReadOutcome};
    use facet_core::window::{App, Execution};
    use facet_core::{Location, VirtualFolder};
    use tempfile::TempDir;

    const WAIT: Duration = Duration::from_secs(30);

    #[test]
    fn 용량_정렬_명령은_내_pc에서만_켜지고_머리글처럼_방향을_바꾼다() {
        let dir = TempDir::new().unwrap();
        let mut app = App::new(Some(Location::file_system(dir.path())));
        assert_eq!(app.wait(WAIT).map(|(_, o)| o), Some(ReadOutcome::Committed));
        assert!(!app.can_execute(CommandId::SortByTotalSize));
        assert!(!app.can_execute(CommandId::SortByFreeSpace));

        assert_eq!(app.execute(CommandId::OpenThisPc), Execution::Done);
        assert_eq!(app.wait(WAIT).map(|(_, o)| o), Some(ReadOutcome::Committed));
        assert_eq!(
            app.active_tab().location().virtual_folder(),
            Some(VirtualFolder::ThisPc)
        );
        assert!(app.can_execute(CommandId::SortByTotalSize));
        assert!(app.can_execute(CommandId::SortByFreeSpace));

        assert_eq!(app.execute(CommandId::SortByFreeSpace), Execution::Done);
        assert_eq!(
            app.active_tab().sort(),
            Sort {
                column: Column::FreeSpace,
                direction: Direction::Ascending
            }
        );
        assert_eq!(app.execute(CommandId::SortByFreeSpace), Execution::Done);
        assert_eq!(app.active_tab().sort().direction, Direction::Descending);
    }

    #[test]
    fn 내_pc는_장치_붙임_뗌을_감시한다() {
        let mut app = App::new(Some(Location::this_pc()));
        assert_eq!(app.wait(WAIT).map(|(_, o)| o), Some(ReadOutcome::Committed));
        assert!(
            matches!(app.active_tab().auto_refresh(), AutoRefresh::On),
            "{:?}",
            app.active_tab().auto_refresh()
        );
    }

    fn has_drive(app: &App, root: &Path) -> bool {
        let tab = app.active_tab();
        (0..tab.len()).any(|i| tab.item_path(i).as_deref() == Some(root))
    }

    /// `subst`로 드라이브 문자를 붙이거나 뗀다. 관리자 권한은 필요 없다.
    fn subst(args: &[&str]) -> bool {
        Command::new("subst")
            .args(args)
            .status()
            .is_ok_and(|status| status.success())
    }

    /// 새로 고침 없이 `present`가 될 때까지 기다린다.
    fn wait_for(app: &mut App, root: &Path, present: bool) -> bool {
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline {
            app.wait_update(Duration::from_millis(100));
            if has_drive(app, root) == present && app.active_tab().pending_location().is_none() {
                return true;
            }
        }
        false
    }

    #[test]
    fn 드라이브_문자가_붙거나_떨어지면_내_pc_목록이_스스로_바뀐다() {
        let used = facet_win::drive_letters();
        let Some(letter) = ('M'..='Y').rev().find(|c| !used.contains(c)) else {
            println!("빈 드라이브 문자가 없어 건너뛴다");
            return;
        };
        let dir = TempDir::new().unwrap();
        let drive = format!("{letter}:");
        let root = PathBuf::from(format!("{letter}:\\"));

        let mut app = App::new(Some(Location::this_pc()));
        assert_eq!(app.wait(WAIT).map(|(_, o)| o), Some(ReadOutcome::Committed));
        assert!(!has_drive(&app, &root));

        if !subst(&[&drive, &dir.path().display().to_string()]) {
            println!("subst로 드라이브를 붙이지 못해 건너뛴다");
            return;
        }
        let appeared = wait_for(&mut app, &root, true);
        let removed = subst(&[&drive, "/d"]);
        assert!(appeared, "붙인 {drive}가 내 PC에 나타나지 않았다");
        assert!(removed, "subst {drive} /d");
        assert!(
            wait_for(&mut app, &root, false),
            "뗀 {drive}가 내 PC에서 사라지지 않았다"
        );
    }
}
