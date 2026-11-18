// target: crates/facet-core/tests/spec_sync_folders_gate.rs
// AC: #450: 배경 동작은 온라인 전용(RECALL_ON_DATA_ACCESS·OFFLINE)이면 OnlineOnly, RECALL_ON_OPEN만이면 NotExpanded, 둘 다면 OnlineOnly로 거절
// AC: #450: 배경 동작은 보통 파일을 허락하고 내려받기가 아님, 사용자 동작은 늘 허락하고 온라인 항목이면 내려받기, 허락 값은 누가 읽는지를 가짐
// AC: #450: is_user_action은 사용자 동작 넷만 참, 목록 읽기가 Attributes::bits에 원래 속성 값을 담음(기본값 0)
//! 내용 읽기 관문(`content_gate::check`)의 규칙을 속성 값과 동작 종류로 확인한다(순수 함수라 파일이 필요 없다). 목록
//! 항목의 원래 속성 값(`Attributes::bits`)은 임시 폴더의 실제 파일로 Windows에서 확인한다. 나머지는 모든 플랫폼에서 돈다.

use facet_core::Attributes;
use facet_core::content_gate::{Reader, Refusal, check};

const READONLY: u32 = 0x1;
const HIDDEN: u32 = 0x2;
const SYSTEM: u32 = 0x4;
const DIRECTORY: u32 = 0x10;
const ARCHIVE: u32 = 0x20;
const REPARSE_POINT: u32 = 0x400;
const OFFLINE: u32 = 0x1000;
const RECALL_ON_OPEN: u32 = 0x4_0000;
const PINNED: u32 = 0x8_0000;
const UNPINNED: u32 = 0x10_0000;
const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;

const BACKGROUND: [Reader; 6] = [
    Reader::Thumbnail,
    Reader::Preview,
    Reader::ContentSearch,
    Reader::FolderSize,
    Reader::TypeSniff,
    Reader::LiveSearchDescend,
];

const USER: [Reader; 4] = [
    Reader::Copy,
    Reader::CrossVolumeMove,
    Reader::Open,
    Reader::PreviewDownload,
];

/// 내용이 이 장치에 있는 항목의 속성 값들(동기화 폴더의 「이 장치에 있음」·「항상 유지」 포함).
const LOCAL: [u32; 8] = [
    0,
    ARCHIVE,
    READONLY | HIDDEN | SYSTEM,
    DIRECTORY,
    ARCHIVE | REPARSE_POINT,
    ARCHIVE | REPARSE_POINT | PINNED,
    ARCHIVE | REPARSE_POINT | UNPINNED,
    DIRECTORY | REPARSE_POINT | PINNED,
];

/// 내용이 이 장치에 없는 파일의 속성 값들.
const ONLINE: [u32; 5] = [
    RECALL_ON_DATA_ACCESS,
    OFFLINE,
    ARCHIVE | REPARSE_POINT | RECALL_ON_DATA_ACCESS,
    ARCHIVE | REPARSE_POINT | OFFLINE | RECALL_ON_DATA_ACCESS | UNPINNED,
    ARCHIVE | REPARSE_POINT | RECALL_ON_DATA_ACCESS | PINNED,
];

/// 아직 펼쳐지지 않은 온라인 폴더의 속성 값들.
const NOT_EXPANDED: [u32; 2] = [RECALL_ON_OPEN, DIRECTORY | REPARSE_POINT | RECALL_ON_OPEN];

#[test]
#[ignore = "#450"]
fn 배경_동작은_온라인_전용_항목을_온라인_전용으로_거절한다() {
    for reader in BACKGROUND {
        for bits in ONLINE {
            assert_eq!(
                check(bits, reader),
                Err(Refusal::OnlineOnly),
                "{reader:?} {bits:#x}"
            );
        }
    }
}

#[test]
#[ignore = "#450"]
fn 배경_동작은_펼쳐지지_않은_온라인_폴더를_펼쳐지지_않음으로_거절한다() {
    for reader in BACKGROUND {
        for bits in NOT_EXPANDED {
            assert_eq!(
                check(bits, reader),
                Err(Refusal::NotExpanded),
                "{reader:?} {bits:#x}"
            );
        }
        // 둘 다 있으면 온라인 전용이다.
        assert_eq!(
            check(RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS, reader),
            Err(Refusal::OnlineOnly),
            "{reader:?}"
        );
        assert_eq!(
            check(RECALL_ON_OPEN | OFFLINE, reader),
            Err(Refusal::OnlineOnly),
            "{reader:?}"
        );
    }
}

#[test]
#[ignore = "#450"]
fn 배경_동작은_이_장치에_있는_항목을_허락하고_내려받기가_아니다() {
    for reader in BACKGROUND {
        for bits in LOCAL {
            let permit = check(bits, reader)
                .unwrap_or_else(|refusal| panic!("{reader:?} {bits:#x}: {refusal:?}"));
            assert!(!permit.downloads(), "{reader:?} {bits:#x}");
            assert_eq!(permit.reader(), reader);
        }
    }
}

#[test]
#[ignore = "#450"]
fn 사용자_동작은_늘_허락하고_온라인_항목이면_내려받기가_일어난다() {
    for reader in USER {
        for bits in LOCAL {
            let permit = check(bits, reader).expect("사용자 동작은 허락");
            assert!(!permit.downloads(), "{reader:?} {bits:#x}");
            assert_eq!(permit.reader(), reader);
        }
        for bits in ONLINE.into_iter().chain(NOT_EXPANDED) {
            let permit = check(bits, reader).expect("사용자 동작은 허락");
            assert!(permit.downloads(), "{reader:?} {bits:#x}");
            assert_eq!(permit.reader(), reader);
        }
    }
}

#[test]
#[ignore = "#450"]
fn 사용자_동작은_넷뿐이다() {
    for reader in USER {
        assert!(reader.is_user_action(), "{reader:?}");
    }
    for reader in BACKGROUND {
        assert!(!reader.is_user_action(), "{reader:?}");
    }
}

#[test]
#[ignore = "#450"]
fn 속성_값의_기본값은_0이다() {
    assert_eq!(Attributes::default().bits, 0);
}

#[cfg(windows)]
#[test]
#[ignore = "#450"]
fn 목록_읽기가_항목의_원래_속성_값을_담는다() {
    use std::fs;
    use std::time::Duration;

    use facet_core::Location;
    use facet_core::tab::ReadOutcome;
    use facet_core::window::App;

    let dir = tempfile::TempDir::new().unwrap();
    fs::write(dir.path().join("보통.txt"), b"x").unwrap();
    let hidden = dir.path().join("숨김.txt");
    fs::write(&hidden, b"x").unwrap();
    let status = std::process::Command::new("attrib")
        .args(["+h", "+r"])
        .arg(&hidden)
        .status()
        .unwrap();
    assert!(status.success());
    fs::create_dir(dir.path().join("폴더")).unwrap();

    let mut app = App::new(Some(Location::file_system(dir.path())));
    let (_, outcome) = app.wait(Duration::from_secs(10)).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app.active_tab_mut().set_show_hidden(true);
    let bits_of = |name: &str| {
        app.active_tab()
            .items()
            .find(|item| item.name == name)
            .unwrap_or_else(|| panic!("{name}이 목록에 없다"))
            .attributes
            .bits
    };
    let plain = bits_of("보통.txt");
    assert_eq!(plain & (HIDDEN | READONLY | DIRECTORY), 0, "{plain:#x}");
    let both = bits_of("숨김.txt");
    assert_eq!(both & (HIDDEN | READONLY), HIDDEN | READONLY, "{both:#x}");
    let folder = bits_of("폴더");
    assert_ne!(folder & DIRECTORY, 0, "{folder:#x}");
    // 읽기 전용을 풀어 임시 폴더를 지울 수 있게 한다.
    let mut permissions = fs::metadata(&hidden).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    fs::set_permissions(&hidden, permissions).unwrap();
}
