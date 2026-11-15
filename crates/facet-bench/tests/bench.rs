//! 시험 폴더 생성과 벤치마크 판정을 공개 인터페이스로만 확인한다.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::time::Duration;

use facet_bench::{
    LIST_SORT_LIMIT, LIST_SORT_TARGET, Measurement, RESORT_CYCLE, RESORT_LIMIT, RESORT_TARGET,
    TestFolderError, check_limit, check_median, make_test_folder, measure_list,
    measure_list_sorted, measure_resort, test_folder_entry,
};
use facet_core::{FileSystemProvider, Location, Provider};
use tempfile::TempDir;

/// 폴더를 이름 → (폴더 여부, 크기)로 읽는다.
fn snapshot(path: &std::path::Path) -> BTreeMap<String, (bool, u64)> {
    FileSystemProvider
        .list(&Location::file_system(path))
        .unwrap()
        .into_iter()
        .map(|i| {
            let size = if i.is_folder { 0 } else { i.size };
            (i.name.into_string().unwrap(), (i.is_folder, size))
        })
        .collect()
}

#[test]
fn 이름에_한글_이모지_긴_이름이_섞이고_폴더와_파일이_섞인다() {
    let entries: Vec<_> = (0..100).map(test_folder_entry).collect();
    let has_korean = |s: &str| s.chars().any(|c| ('가'..='힣').contains(&c));
    let has_emoji = |s: &str| s.chars().any(|c| c as u32 >= 0x1F300);

    assert!(entries.iter().any(|e| has_korean(&e.name)));
    assert!(entries.iter().any(|e| has_emoji(&e.name)));
    assert!(entries.iter().any(|e| e.name.chars().count() >= 100));
    assert!(entries.iter().any(|e| e.is_folder));
    assert!(entries.iter().any(|e| !e.is_folder));
    // Windows 이름 한 칸의 한계(UTF-16 255단위) 안이다.
    assert!(entries.iter().all(|e| e.name.encode_utf16().count() <= 255));
    // 이름이 겹치지 않는다(대소문자를 무시하는 파일 시스템에서도).
    let names: BTreeSet<_> = entries.iter().map(|e| e.name.to_lowercase()).collect();
    assert_eq!(names.len(), entries.len());
}

#[test]
fn 같은_번호는_늘_같은_항목() {
    assert_eq!(test_folder_entry(12345), test_folder_entry(12345));
}

#[test]
fn 지정한_수의_항목이_든_폴더를_만든다() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("시험 폴더");

    let report = make_test_folder(&target, 250).unwrap();
    assert_eq!(report.created, 250);

    let got = snapshot(&target);
    let want: BTreeMap<String, (bool, u64)> = (0..250)
        .map(test_folder_entry)
        .map(|e| (e.name, (e.is_folder, e.size)))
        .collect();
    assert_eq!(got, want);
}

#[test]
fn 다시_돌려도_결과가_같다() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("t");
    make_test_folder(&target, 120).unwrap();
    let first = snapshot(&target);

    let report = make_test_folder(&target, 120).unwrap();
    assert_eq!(report.created, 0);
    assert_eq!(snapshot(&target), first);
}

/// 항목 경로가 MAX_PATH(260)를 넘는 깊은 폴더. 8.3 짧은 이름이 있으면 그 표기(`LONGFO~1`)로 돌려준다.
fn deep_folder(dir: &TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
    let long = dir
        .path()
        .join("long folder name for short path")
        .join("깊은 폴더 이름으로 경로를 길게 만든다 - 그리고 조금 더 길게 이어 붙인 이름 (MAX_PATH 넘기기용)");
    fs::create_dir_all(&long).unwrap();
    let short = dir
        .path()
        .join("LONGFO~1")
        .join("깊은 폴더 이름으로 경로를 길게 만든다 - 그리고 조금 더 길게 이어 붙인 이름 (MAX_PATH 넘기기용)");
    let base = if short.exists() { short } else { long.clone() };
    (base.join("items"), long.join("items"))
}

#[test]
fn 짧은_이름과_max_path를_넘는_경로에서도_다시_돌리면_결과가_같다() {
    let dir = TempDir::new().unwrap();
    let (target, long_target) = deep_folder(&dir);
    let longest = (0..30)
        .map(test_folder_entry)
        .map(|e| long_target.join(e.name).as_os_str().encode_wide_len())
        .max()
        .unwrap();
    assert!(
        longest > 260,
        "항목 경로가 MAX_PATH를 넘어야 한다: {longest}"
    );

    let report = make_test_folder(&target, 30).unwrap();
    assert_eq!(report.created, 30);
    let first = snapshot(&target);

    let report = make_test_folder(&target, 30).unwrap();
    assert_eq!((report.created, report.kept), (0, 30));
    assert_eq!(snapshot(&target), first);

    // 같은 폴더를 긴 표기로 불러도 같다.
    let report = make_test_folder(&long_target, 30).unwrap();
    assert_eq!((report.created, report.kept), (0, 30));
    assert_eq!(snapshot(&long_target), first);
}

#[test]
fn 같은_폴더를_동시에_만들어도_둘_다_성공하고_결과가_같다() {
    let dir = TempDir::new().unwrap();
    let (target, _) = deep_folder(&dir);
    let count = 3000;

    let reports: Vec<_> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..2)
            .map(|_| s.spawn(|| make_test_folder(&target, count)))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for report in reports {
        let report = report.unwrap();
        assert_eq!(report.created + report.kept, count);
    }

    let want: BTreeMap<String, (bool, u64)> = (0..count)
        .map(test_folder_entry)
        .map(|e| (e.name, (e.is_folder, e.size)))
        .collect();
    assert_eq!(snapshot(&target), want);
}

/// 경로의 UTF-16 길이(Windows가 MAX_PATH를 세는 단위).
trait EncodeWideLen {
    fn encode_wide_len(&self) -> usize;
}

impl EncodeWideLen for std::ffi::OsStr {
    fn encode_wide_len(&self) -> usize {
        self.to_string_lossy().encode_utf16().count()
    }
}

#[test]
fn 일부만_남은_폴더는_채우고_크기가_틀린_파일은_고친다() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("t");
    make_test_folder(&target, 50).unwrap();
    let want = snapshot(&target);

    let file = (0..50)
        .map(test_folder_entry)
        .find(|e| !e.is_folder)
        .unwrap();
    fs::write(target.join(&file.name), vec![0u8; file.size as usize + 10]).unwrap();
    let folder = (0..50)
        .map(test_folder_entry)
        .find(|e| e.is_folder)
        .unwrap();
    fs::remove_dir(target.join(&folder.name)).unwrap();

    let report = make_test_folder(&target, 50).unwrap();
    assert_eq!(report.created, 1);
    assert_eq!(snapshot(&target), want);
}

#[test]
fn 모르는_항목이_있으면_건드리지_않고_거절한다() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("t");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("내 파일.txt"), b"keep").unwrap();

    let err = make_test_folder(&target, 10).unwrap_err();
    assert!(matches!(err, TestFolderError::Unexpected { .. }), "{err}");
    assert_eq!(fs::read(target.join("내 파일.txt")).unwrap(), b"keep");
    assert_eq!(snapshot(&target).len(), 1);
}

#[test]
fn 경로가_파일이면_거절한다() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("f");
    fs::write(&target, b"").unwrap();
    assert!(make_test_folder(&target, 3).is_err());
}

#[test]
fn 측정은_항목_수와_회차별_시간을_돌려준다() {
    let dir = TempDir::new().unwrap();
    make_test_folder(dir.path(), 30).unwrap();
    let m = measure_list(dir.path(), 3).unwrap();
    assert_eq!(m.item_count, 30);
    assert_eq!(m.times.len(), 3);
    assert!(m.median() <= m.max());
}

#[test]
fn 읽기_정렬_측정은_항목_수와_회차별_시간을_돌려준다() {
    let dir = TempDir::new().unwrap();
    make_test_folder(dir.path(), 30).unwrap();
    let m = measure_list_sorted(dir.path(), 3).unwrap();
    assert_eq!(m.item_count, 30);
    assert_eq!(m.times.len(), 3);
}

#[test]
fn 정렬_바꾸기_측정은_회차마다_다른_정렬로_바꾼다() {
    let dir = TempDir::new().unwrap();
    make_test_folder(dir.path(), 30).unwrap();
    let m = measure_resort(dir.path(), 7).unwrap();
    assert_eq!(m.item_count, 30);
    assert_eq!(m.times.len(), 7);
    for pair in RESORT_CYCLE.windows(2) {
        assert_ne!(pair[0], pair[1]);
    }
    assert_ne!(RESORT_CYCLE[0], RESORT_CYCLE[RESORT_CYCLE.len() - 1]);
    assert_ne!(RESORT_CYCLE[0], facet_core::details::Sort::default());
}

#[test]
fn 정렬_기준과_상한() {
    assert_eq!(RESORT_TARGET, Duration::from_millis(50));
    assert_eq!(RESORT_LIMIT, RESORT_TARGET * 10);
    assert_eq!(LIST_SORT_LIMIT, LIST_SORT_TARGET * 10);
}

#[test]
fn 이름을_붙인_상한_판정은_무엇이_넘었는지_알린다() {
    assert!(check_median("정렬 바꾸기", &measurement(&[40, 600, 45]), RESORT_LIMIT).is_ok());
    let msg =
        check_median("정렬 바꾸기", &measurement(&[600, 700, 550]), RESORT_LIMIT).unwrap_err();
    assert!(msg.contains("정렬 바꾸기"), "{msg}");
    assert!(msg.contains("600"), "{msg}");
    assert!(msg.contains("500"), "{msg}");
}

fn measurement(ms: &[u64]) -> Measurement {
    Measurement {
        item_count: 100_000,
        times: ms.iter().map(|&t| Duration::from_millis(t)).collect(),
    }
}

#[test]
fn 중앙값은_가운데_값() {
    assert_eq!(
        measurement(&[30, 10, 20]).median(),
        Duration::from_millis(20)
    );
    assert_eq!(measurement(&[30, 10, 20]).max(), Duration::from_millis(30));
}

#[test]
fn 상한_안이면_통과() {
    assert!(check_limit(&measurement(&[50, 900, 60]), Duration::from_millis(1000)).is_ok());
}

#[test]
fn 상한을_넘으면_넘은_수치를_알린다() {
    let msg = check_limit(
        &measurement(&[1500, 1200, 1300]),
        Duration::from_millis(1000),
    )
    .unwrap_err();
    assert!(msg.contains("1300"), "{msg}");
    assert!(msg.contains("1000"), "{msg}");
}

mod screen {
    use std::path::PathBuf;
    use std::time::Duration;

    use facet_bench::screen::{COLD_START_LIMIT, OPEN_LIMIT, ScreenResult, SystemSpec};

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn result(cold: &[u64], frames: &[u64], hz: Option<f64>) -> ScreenResult {
        ScreenResult {
            folder: PathBuf::from(r#"C:\시험 "폴더""#),
            item_count: 100_000,
            scroll_rows: 50,
            cold_starts: cold.iter().map(|n| ms(*n)).collect(),
            session_starts: vec![ms(120)],
            opens: vec![ms(100)],
            frames: frames.iter().map(|n| ms(*n)).collect(),
            scroll_runs: 1,
            spec: SystemSpec {
                cpu: Some("Test CPU".into()),
                memory_bytes: Some(16 * 1024 * 1024 * 1024),
                disk: Some("Test SSD".into()),
                os: Some("Windows 11 10.0.26100".into()),
                refresh_hz: hz,
            },
            debug_build: false,
        }
    }

    #[test]
    fn 콜드_스타트는_중앙값이_200ms_미만일_때만_통과한다() {
        assert!(
            result(&[150, 190, 900], &[16], Some(60.0))
                .verdict()
                .cold_pass
        );
        assert!(
            !result(&[150, 250, 260], &[16], Some(60.0))
                .verdict()
                .cold_pass
        );
        assert!(!result(&[200], &[16], Some(60.0)).verdict().cold_pass);
        assert_eq!(COLD_START_LIMIT, ms(200));
    }

    fn with_opens(mut r: ScreenResult, opens: &[u64]) -> ScreenResult {
        r.opens = opens.iter().map(|n| ms(*n)).collect();
        r
    }

    #[test]
    fn 폴더_열기_첫_화면은_중앙값이_150ms_미만일_때만_통과한다() {
        let base = || result(&[100], &[16], Some(60.0));
        assert!(with_opens(base(), &[90, 140, 600]).verdict().open_pass);
        assert!(!with_opens(base(), &[90, 160, 170]).verdict().open_pass);
        assert!(!with_opens(base(), &[150]).verdict().open_pass);
        assert!(!with_opens(base(), &[]).verdict().open_pass);
        assert!(!with_opens(base(), &[160]).verdict().pass());
        assert_eq!(OPEN_LIMIT, ms(150));
    }

    fn with_sessions(mut r: ScreenResult, sessions: &[u64]) -> ScreenResult {
        r.session_starts = sessions.iter().map(|n| ms(*n)).collect();
        r
    }

    #[test]
    fn 세션_복원_콜드_스타트는_중앙값이_200ms_미만일_때만_통과한다() {
        use facet_bench::screen::SESSION_RESTORE_LIMIT;
        let base = || result(&[100], &[16], Some(60.0));
        assert!(
            with_sessions(base(), &[150, 190, 900])
                .verdict()
                .session_pass
        );
        assert!(
            !with_sessions(base(), &[150, 250, 260])
                .verdict()
                .session_pass
        );
        assert!(!with_sessions(base(), &[200]).verdict().session_pass);
        assert!(!with_sessions(base(), &[]).verdict().session_pass);
        assert!(!with_sessions(base(), &[250]).verdict().pass());
        assert_eq!(SESSION_RESTORE_LIMIT, ms(200));
        let r = with_sessions(base(), &[120, 100, 180]);
        let v = r.verdict();
        assert_eq!((v.session_median, v.session_worst), (ms(120), ms(180)));
        let t = r.to_table();
        assert!(t.contains("세션 복원 콜드 스타트 중앙값"), "{t}");
        assert!(t.contains("세션 복원 콜드 스타트 최악값"), "{t}");
        let j = r.to_json();
        for want in [
            "\"session_restore_cold_start\": {",
            "\"runs_ms\": [120.000, 100.000, 180.000]",
            "\"windows\": 2",
            "\"tabs_per_window\": 10",
        ] {
            assert!(
                j.contains(want),
                "{want}가 없다:
{j}"
            );
        }
    }

    #[test]
    fn 세션_복원_측정_세션은_탭_10개짜리_창_둘이고_폴더가_있다() {
        use facet_bench::screen::write_session_fixture;
        use facet_core::session::{self, Loaded};
        let dir = tempfile::tempdir().unwrap();
        let path = write_session_fixture(dir.path()).unwrap();
        let Loaded::Restored(s) = session::load(&path) else {
            panic!("세션을 읽지 못함");
        };
        assert_eq!(s.windows.len(), 2);
        for w in &s.windows {
            assert_eq!(w.panels.len(), 2);
            assert_eq!(w.panels.iter().map(|p| p.tabs.len()).sum::<usize>(), 10);
            for tab in w.panels.iter().flat_map(|p| &p.tabs) {
                assert!(tab.location.file_system_path().unwrap().is_dir());
            }
        }
    }

    #[test]
    fn 콜드_스타트와_폴더_열기의_중앙값과_최악값이_나온다() {
        let r = with_opens(result(&[120, 100, 180], &[16], Some(60.0)), &[40, 70, 50]);
        let v = r.verdict();
        assert_eq!(v.cold_median, ms(120));
        assert_eq!(v.cold_worst, ms(180));
        assert_eq!(v.open_median, ms(50));
        assert_eq!(v.open_worst, ms(70));
        let t = r.to_table();
        for want in [
            "콜드 스타트 중앙값",
            "콜드 스타트 최악값",
            "180.0ms",
            "폴더 열기 첫 화면 중앙값",
            "폴더 열기 첫 화면 최악값",
            "50.0ms",
            "70.0ms",
            "150ms",
        ] {
            assert!(
                t.contains(want),
                "{want}가 없다:
{t}"
            );
        }
        let j = r.to_json();
        for want in [
            "\"worst_ms\": 180.000",
            "\"folder_open\": {",
            "\"runs_ms\": [40.000, 70.000, 50.000]",
            "\"median_ms\": 50.000",
            "\"worst_ms\": 70.000",
            "\"limit_ms\": 150",
        ] {
            assert!(
                j.contains(want),
                "{want}가 없다:
{j}"
            );
        }
    }

    #[test]
    fn 폴더_열기는_시험_폴더의_상위에서_시작한다() {
        use facet_bench::screen::open_start;
        use std::path::Path;
        assert_eq!(
            open_start(Path::new(r"C:\Temp\facet-test-folders\items-100000")),
            Some(PathBuf::from(r"C:\Temp\facet-test-folders"))
        );
        assert_eq!(open_start(Path::new(r"C:\")), None);
    }

    #[test]
    fn 프레임_최악값은_한_프레임의_두_배_이하일_때_통과한다() {
        // 60Hz: 한 프레임 16.67ms, 상한 33.3ms
        assert!(
            result(&[100], &[16, 17, 33], Some(60.0))
                .verdict()
                .frame_pass
        );
        assert!(
            !result(&[100], &[16, 17, 34], Some(60.0))
                .verdict()
                .frame_pass
        );
        // 144Hz: 상한 13.9ms
        assert!(result(&[100], &[7, 13], Some(144.0)).verdict().frame_pass);
        assert!(!result(&[100], &[7, 14], Some(144.0)).verdict().frame_pass);
    }

    #[test]
    fn 상한을_넘은_프레임_수를_센다() {
        // 60Hz 상한 33.3ms: 34와 2835만 넘는다.
        let r = result(&[100], &[6, 33, 34, 7, 2835], Some(60.0));
        assert_eq!(r.verdict().frames_over_limit, 2);
        assert!(r.to_table().contains("상한을 넘은 프레임 2개"));
        assert!(r.to_json().contains("\"frames_over_limit\": 2"));
    }

    #[test]
    fn 디스크는_폴더가_있는_드라이브에서_찾는다() {
        use facet_bench::screen::{drive_letter, spec_script};
        use std::path::Path;
        assert_eq!(drive_letter(Path::new(r"d:\시험\items")), Some('D'));
        assert_eq!(drive_letter(Path::new(r"\\?\C:\Temp")), Some('C'));
        assert_eq!(drive_letter(Path::new(r"\\server\share\x")), None);
        assert_eq!(drive_letter(Path::new("relative")), None);
        assert!(spec_script(Path::new(r"E:\x")).contains("Get-Partition -DriveLetter E"));
        assert!(!spec_script(Path::new(r"\\server\share\x")).contains("disk="));
    }

    #[test]
    fn 프레임이_하나도_없으면_통과하지_못한다() {
        assert!(!result(&[100], &[], Some(60.0)).verdict().frame_pass);
    }

    #[test]
    fn 프레임_중앙값과_최악값이_나온다() {
        let v = result(&[100], &[10, 30, 20], Some(60.0)).verdict();
        assert_eq!(v.frame_median, ms(20));
        assert_eq!(v.frame_worst, ms(30));
    }

    #[test]
    fn 주사율을_모르면_육십헤르츠로_가정하고_표시한다() {
        let r = result(&[100], &[16], None);
        assert!(r.verdict().refresh_assumed);
        assert!(r.to_table().contains("60"));
        assert!(r.to_json().contains("\"refresh_hz_assumed\": true"));
    }

    #[test]
    fn 표에_사양과_기준과_통과_여부가_나온다() {
        let t = result(&[150], &[16], Some(144.0)).to_table();
        for want in [
            "Test CPU",
            "16.0 GiB",
            "Test SSD",
            "Windows 11",
            "144Hz",
            "200ms",
            "통과",
        ] {
            assert!(t.contains(want), "{want}가 없다:\n{t}");
        }
        assert!(
            result(&[250], &[16], Some(60.0))
                .to_table()
                .contains("실패")
        );
    }

    #[test]
    fn json에_수치와_사양과_기준이_들어가고_문자열은_이스케이프된다() {
        let j = result(&[100, 150, 120], &[10, 20], Some(60.0)).to_json();
        for want in [
            "\"median_ms\": 120.000",
            "\"limit_ms\": 200",
            "\"frame_median_ms\": 20.000",
            "\"frame_worst_ms\": 20.000",
            "\"cpu\": \"Test CPU\"",
            "\"refresh_hz\": 60",
            "\"pass\": true",
            r#""folder": "C:\\시험 \"폴더\"""#,
        ] {
            assert!(j.contains(want), "{want}가 없다:\n{j}");
        }
    }

    #[test]
    fn 사양_출력을_읽고_빈_값은_모름으로_둔다() {
        let spec = SystemSpec::parse(
            "cpu=Some CPU\nmemory_bytes=17179869184\nos=Windows 11 10.0.1\ndisk=\nrefresh_hz=165\n잡음\n",
        );
        assert_eq!(spec.cpu.as_deref(), Some("Some CPU"));
        assert_eq!(spec.memory_bytes, Some(17_179_869_184));
        assert_eq!(spec.disk, None);
        assert_eq!(spec.refresh_hz, Some(165.0));
        assert_eq!(SystemSpec::parse("refresh_hz=0").refresh_hz, None);
    }
}

mod session_bench {
    use std::time::Duration;

    use facet_bench::{
        Measurement, SESSION_TABS, check_session_limit, measure_session, session_fixture,
    };
    use facet_core::Location;

    #[test]
    fn 탭_100개_세션을_읽고_해석해_횟수만큼_잰다() {
        let m = measure_session(3).unwrap();
        assert_eq!(m.item_count, SESSION_TABS);
        assert_eq!(m.times.len(), 3);
    }

    #[test]
    fn 세션_모양은_창과_패널과_탭_수대로다() {
        let s = session_fixture(3, 4, |w, p, t| {
            Location::file_system(format!(r"C:\{w}\{p}\{t}"))
        });
        assert_eq!(s.windows.len(), 3);
        for w in &s.windows {
            assert_eq!(w.panels.len(), 2);
            assert!(
                w.panels
                    .iter()
                    .all(|p| p.tabs.len() == 4 && p.active_tab == 3)
            );
        }
    }

    #[test]
    fn 중앙값이_상한을_넘으면_실패한다() {
        let slow = Measurement {
            item_count: 100,
            times: vec![Duration::from_millis(60); 3],
        };
        let err = check_session_limit(&slow, Duration::from_millis(50)).unwrap_err();
        assert!(err.contains("상한 초과"), "{err}");
        let fast = Measurement {
            item_count: 100,
            times: vec![Duration::from_millis(1); 3],
        };
        assert!(check_session_limit(&fast, Duration::from_millis(50)).is_ok());
    }
}
