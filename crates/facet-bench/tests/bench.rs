//! 시험 폴더 생성과 벤치마크 판정을 공개 인터페이스로만 확인한다.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::time::Duration;

use facet_bench::{
    Measurement, TestFolderError, check_limit, make_test_folder, measure_list, test_folder_entry,
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

    use facet_bench::screen::{COLD_START_LIMIT, ScreenResult, SystemSpec};

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn result(cold: &[u64], frames: &[u64], hz: Option<f64>) -> ScreenResult {
        ScreenResult {
            folder: PathBuf::from(r#"C:\시험 "폴더""#),
            item_count: 100_000,
            scroll_rows: 50,
            cold_starts: cold.iter().map(|n| ms(*n)).collect(),
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
