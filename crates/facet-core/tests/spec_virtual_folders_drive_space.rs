// target: crates/facet-core/tests/spec_virtual_folders_drive_space.rs
// AC: #351: 시스템 드라이브의 전체 크기·남은 공간이 파일 시스템 API 값과 같다, 전체 크기·남은 공간 정렬(모르는 값은 뒤)
//! 「내 PC」의 드라이브 용량을 실제 셸과 실제 드라이브로 확인한다: 시스템 드라이브 항목에 채워지는 전체 크기와 남은
//! 공간(이 사용자가 쓸 수 있는 양)이 파일 시스템 API(.NET `DriveInfo`, 안에서 `GetDiskFreeSpaceExW`) 값과 같은지,
//! 「전체 크기」「남은 공간」 열로 오름·내림 정렬하면 아는 값이 그 순서로 늘어서고 모르는 값은 뒤에 오는지.
//! Windows 전용.
#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use facet_core::command::CommandId;
use facet_core::details::{Column, Direction, Sort};
use facet_core::shell_provider::DriveSpace;
use facet_core::tab::ReadOutcome;
use facet_core::window::{App, Execution};
use facet_core::{Location, VirtualFolder};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(30);

/// 남은 공간은 잴 때마다 조금 바뀐다(다른 테스트가 디스크를 쓴다).
const FREE_TOLERANCE: u64 = 1 << 30;

fn system_root() -> PathBuf {
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_owned());
    PathBuf::from(format!("{drive}\\"))
}

/// 파일 시스템 API가 주는 (전체 크기, 이 사용자가 쓸 수 있는 남은 공간).
fn api_space(root: &std::path::Path) -> (u64, u64) {
    let script = format!(
        "$d = New-Object System.IO.DriveInfo '{}'; $d.TotalSize; $d.AvailableFreeSpace",
        root.display()
    );
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .expect("powershell을 띄운다");
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8_lossy(&output.stdout);
    let numbers: Vec<u64> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.trim().parse().expect("숫자"))
        .collect();
    assert_eq!(numbers.len(), 2, "{text}");
    (numbers[0], numbers[1])
}

/// 내 PC를 열고 시스템 드라이브의 용량이 채워질 때까지 기다린다. 처음 연 임시 폴더도 함께 돌려준다(버리면 지워짐).
fn open_this_pc_with_space() -> (App, TempDir) {
    let dir = TempDir::new().unwrap();
    let mut app = App::new(Some(Location::file_system(dir.path())));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    assert_eq!(app.execute(CommandId::OpenThisPc), Execution::Done);

    let root = system_root();
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll();
        let tab = app.active_tab();
        if let Some(error) = tab.error() {
            panic!("읽기가 실패했다: {error}");
        }
        let at_this_pc = tab.location().virtual_folder() == Some(VirtualFolder::ThisPc)
            && tab.pending_location().is_none()
            && tab.loading().is_none();
        if at_this_pc && system_space(&app).is_some() {
            return (app, dir);
        }
        assert!(
            Instant::now() < deadline,
            "{}의 용량이 채워지지 않았다",
            root.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// 시스템 드라이브 항목의 용량. 둘 다 채워졌을 때만.
fn system_space(app: &App) -> Option<DriveSpace> {
    let tab = app.active_tab();
    let root = system_root();
    let index = (0..tab.len()).find(|&i| tab.item_path(i).as_deref() == Some(root.as_path()))?;
    let space = *tab.drive(index)?;
    (space.total.is_some() && space.free.is_some()).then_some(space)
}

/// 보이는 순서대로 각 항목의 열 값.
fn column_values(app: &App, column: Column) -> Vec<Option<u64>> {
    let tab = app.active_tab();
    (0..tab.len())
        .map(|i| {
            let space = tab.drive(i).copied()?;
            match column {
                Column::TotalSize => space.total,
                Column::FreeSpace => space.free,
                _ => unreachable!(),
            }
        })
        .collect()
}

/// 아는 값이 `direction` 순서로 늘어서고 모르는 값은 모두 그 뒤에 있는지 본다.
fn assert_sorted(values: &[Option<u64>], direction: Direction) {
    let known = values.iter().take_while(|value| value.is_some()).count();
    assert!(
        values[known..].iter().all(Option::is_none),
        "모르는 값은 아는 값 뒤에 온다: {values:?}"
    );
    let known: Vec<u64> = values[..known].iter().flatten().copied().collect();
    let ordered = known.windows(2).all(|pair| match direction {
        Direction::Ascending => pair[0] <= pair[1],
        Direction::Descending => pair[0] >= pair[1],
    });
    assert!(ordered, "{direction:?} 순서가 아니다: {known:?}");
}

#[test]
#[ignore = "#351"]
fn system_drive_space_matches_the_file_system_api() {
    let (app, _dir) = open_this_pc_with_space();
    let space = system_space(&app).unwrap();
    let (total, free) = api_space(&system_root());

    assert_eq!(space.total, Some(total), "전체 크기");
    let shown = space.free.unwrap();
    assert!(
        shown.abs_diff(free) <= FREE_TOLERANCE,
        "남은 공간: 보인 값 {shown}, API {free}"
    );
    assert!(shown <= total);
}

#[test]
#[ignore = "#351"]
fn sorting_by_total_size_and_free_space() {
    let (mut app, _dir) = open_this_pc_with_space();
    for column in [Column::TotalSize, Column::FreeSpace] {
        for direction in [Direction::Ascending, Direction::Descending] {
            app.set_sort(Sort { column, direction });
            assert_eq!(app.active_tab().sort(), Sort { column, direction });
            let values = column_values(&app, column);
            assert!(
                values.iter().any(Option::is_some),
                "시스템 드라이브의 값이 있다"
            );
            assert_sorted(&values, direction);
        }
    }
}
