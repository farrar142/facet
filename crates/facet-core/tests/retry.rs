//! 「다시 시도」(`App::retry`)가 셸이 아닌 실패한 읽기도 같은 위치로 다시 하는지.

use std::fs;
use std::time::Duration;

use facet_core::Location;
use facet_core::tab::{Navigation, ReadOutcome};
use facet_core::window::App;
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(10);

#[test]
fn retry_reads_a_failed_location_again_after_it_appears() {
    let dir = TempDir::new().unwrap();
    let start = Location::file_system(dir.path());
    let mut app = App::new(Some(start.clone()));
    assert_eq!(app.wait(WAIT).unwrap().1, ReadOutcome::Committed);
    let tab = app.active_tab_id();
    assert!(!app.retry(tab), "실패한 읽기가 없다");

    let later = dir.path().join("나중에 생김");
    assert!(app.navigate(Navigation::Go(Location::file_system(&later))));
    assert_eq!(app.wait(WAIT).unwrap().1, ReadOutcome::Failed);
    assert_eq!(app.active_tab().location(), &start);

    fs::create_dir(&later).unwrap();
    fs::write(later.join("a.txt"), "a").unwrap();
    assert!(app.retry(tab));
    assert_eq!(app.wait(WAIT).unwrap().1, ReadOutcome::Committed);
    assert_eq!(app.active_tab().location(), &Location::file_system(&later));
    assert_eq!(app.active_tab().len(), 1);
    assert_eq!(app.active_tab().back_history(), &[start][..]);
    assert!(!app.retry(tab), "성공한 뒤에는 다시 할 읽기가 없다");
}
