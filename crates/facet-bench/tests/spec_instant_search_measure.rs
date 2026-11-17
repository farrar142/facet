// target: crates/facet-bench/tests/spec_instant_search_measure.rs
// AC: 227: `cargo xtask measure`가 즉시 검색 첫 결과 시간을 표와 JSON에 내고 기준으로 통과 여부를 가림
// AC: 227: 측정 도구 단위 테스트(보고 해석: 시간이 없거나 결과 0이거나 강등이면 실패)
// AC: 227: 측정 도구가 항목 100만 개 합성 인덱스에 답하는 파이프 서버를 시험용 파이프 이름으로 띄움(관리자 필요 없음)
//! 즉시 검색 첫 결과 측정(글자 입력부터 결과가 든 첫 프레임까지, 기준 중앙값 50ms 미만)의 측정 도구 몫을 확인한다:
//! 앱과 주고받는 보고(`ScreenReport`)의 즉시 검색 줄 왕복과 해석(시간 없음·결과 0·강등이면 실패), 결과 판정과 표·JSON,
//! 측정이 넘기는 질의 글자, 측정 도구가 띄우는 합성 인덱스 파이프 서버(Windows 전용: 같은 프로세스의 코어 앱이 그
//! 서버에 즉시 검색을 보내 강등 없이 시험 폴더 아래 결과를 받는다). 화면 프레임 자체는 사람이 `cargo xtask measure`로 잰다.

use std::path::PathBuf;
use std::time::Duration;

use facet_bench::screen::{
    INSTANT_FIRST_LIMIT, INSTANT_QUERY, ScreenResult, SystemSpec, instant_frame,
};
use facet_core::measure::{ENV_INSTANT, ENV_SEARCH, ScreenReport};
use facet_core::name_match::name_matches;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// 즉시 검색 말고는 모두 통과하는 측정 결과.
fn result(instant: &[u64]) -> ScreenResult {
    ScreenResult {
        folder: PathBuf::from(r"C:\시험 폴더"),
        item_count: 100_000,
        scroll_rows: 50,
        cold_starts: vec![ms(100)],
        session_starts: vec![ms(120)],
        opens: vec![ms(100)],
        renames: vec![ms(10)],
        menus: vec![ms(20)],
        searches: vec![ms(50)],
        instant_searches: instant.iter().map(|n| ms(*n)).collect(),
        frames: vec![ms(16)],
        scroll_runs: 1,
        spec: SystemSpec {
            cpu: Some("Test CPU".into()),
            memory_bytes: Some(16 * 1024 * 1024 * 1024),
            disk: Some("Test SSD".into()),
            os: Some("Windows 11 10.0.26100".into()),
            refresh_hz: Some(60.0),
        },
        debug_build: false,
    }
}

#[test]
fn 즉시_검색_첫_결과는_중앙값이_50ms_미만일_때만_통과한다() {
    assert_eq!(INSTANT_FIRST_LIMIT, ms(50));
    assert!(result(&[20, 45, 900]).verdict().instant_pass);
    assert!(!result(&[20, 55, 60]).verdict().instant_pass);
    assert!(!result(&[50]).verdict().instant_pass, "기준과 같으면 실패");
    assert!(!result(&[]).verdict().instant_pass, "잰 값이 없으면 실패");
    assert!(result(&[30]).verdict().pass());
    assert!(!result(&[80]).verdict().pass(), "전체 통과에도 든다");

    let v = result(&[30, 20, 70]).verdict();
    assert_eq!((v.instant_median, v.instant_worst), (ms(30), ms(70)));
}

#[test]
fn 즉시_검색_첫_결과가_표와_json에_통과_여부와_함께_나온다() {
    let r = result(&[30, 20, 70]);
    let table = r.to_table();
    assert!(table.contains("즉시 검색 첫 결과 중앙값"), "{table}");

    let json = r.to_json();
    let start = json
        .find("\"instant_search_first_result\"")
        .unwrap_or_else(|| panic!("즉시 검색 절이 없다: {json}"));
    let section = &json[start..];
    let section = &section[..section.find('}').expect("절이 닫힌다")];
    assert!(section.contains("\"limit_ms\": 50,"), "{section}");
    assert!(section.contains("\"median_ms\": 30"), "{section}");
    assert!(section.contains("\"worst_ms\": 70"), "{section}");
    assert!(section.contains("\"pass\": true"), "{section}");

    let failed = result(&[80]).to_json();
    let start = failed.find("\"instant_search_first_result\"").unwrap();
    let section = &failed[start..];
    let section = &section[..section.find('}').unwrap()];
    assert!(section.contains("\"pass\": false"), "{section}");
}

#[test]
fn 앱_보고의_즉시_검색_줄을_그대로_읽는다() {
    assert!(ENV_INSTANT.starts_with("FACET_MEASURE_"));
    assert_ne!(ENV_INSTANT, ENV_SEARCH);

    let report = ScreenReport {
        rows: 100,
        instant_frame: Some(ms(30)),
        instant_demoted: true,
        ..ScreenReport::default()
    };
    assert_eq!(ScreenReport::parse(&report.to_text()), Ok(report));

    let plain = ScreenReport::parse("rows 3\n").unwrap();
    assert_eq!(plain.instant_frame, None);
    assert!(!plain.instant_demoted);
}

#[test]
fn 즉시_검색_보고는_시간과_결과가_있고_강등되지_않아야_한다() {
    let report = |rows, frame, demoted| ScreenReport {
        rows,
        instant_frame: frame,
        instant_demoted: demoted,
        ..ScreenReport::default()
    };
    assert_eq!(instant_frame(&report(100, Some(ms(30)), false)), Ok(ms(30)));
    assert!(
        instant_frame(&report(100, None, false)).is_err(),
        "시간 없음"
    );
    assert!(
        instant_frame(&report(0, Some(ms(30)), false)).is_err(),
        "결과 0"
    );
    assert!(
        instant_frame(&report(100, Some(ms(30)), true)).is_err(),
        "강등"
    );
}

#[test]
fn 측정이_넘기는_질의는_시험_폴더_항목에_맞는다() {
    assert!(!INSTANT_QUERY.trim().is_empty());
    assert!(name_matches(
        INSTANT_QUERY,
        std::ffi::OsStr::new(&facet_bench::test_folder_entry(0).name)
    ));
}

#[cfg(windows)]
mod server {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    use facet_bench::screen::{INSTANT_QUERY, serve_instant_index};
    use facet_core::Location;
    use facet_core::indexer_client::{CONNECT_TIMEOUT, ClientOptions};
    use facet_core::name_match::name_matches;
    use facet_core::tab::ReadOutcome;
    use facet_core::window::App;
    use tempfile::TempDir;

    const WAIT: Duration = Duration::from_secs(60);
    /// CI에서 빨리 돌도록 100만 개 대신 쓰는 항목 수(같은 함수가 측정 때는 100만 개를 만든다).
    const COUNT: usize = 2_000;

    fn unique_pipe() -> String {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        format!(
            r"\\.\pipe\facet-bench-instant-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// 활성 탭의 이동이 확정될 때까지 기다린다.
    fn wait_read(app: &mut App) {
        loop {
            let (tab, outcome) = app.wait(WAIT).expect("읽기 결과");
            if tab == app.active_tab_id() {
                assert_eq!(outcome, ReadOutcome::Committed);
                return;
            }
        }
    }

    #[test]
    fn 측정_도구의_합성_인덱스_서버에_즉시_검색하면_강등_없이_시험_폴더_아래_결과가_온다() {
        let dir = TempDir::new().unwrap();
        let pipe = unique_pipe();
        let _server = serve_instant_index(&pipe, dir.path(), COUNT).expect("서버를 띄운다");

        let mut app = App::new(Some(Location::file_system(dir.path())));
        wait_read(&mut app);
        app.enable_indexer(ClientOptions {
            pipe: pipe.clone(),
            connect_timeout: CONNECT_TIMEOUT,
            reconnect_interval: Duration::from_millis(100),
        });
        app.set_instant_query(INSTANT_QUERY);
        wait_read(&mut app);

        let deadline = Instant::now() + WAIT;
        loop {
            app.poll();
            let status = app.active_tab().instant().expect("즉시 검색 상태");
            assert_eq!(status.demotion, None, "측정 서버로는 강등되지 않는다");
            if !status.loading && status.total.is_some() {
                break;
            }
            assert!(Instant::now() < deadline, "응답이 오지 않았다");
            std::thread::sleep(Duration::from_millis(5));
        }

        let tab = app.active_tab();
        let status = tab.instant().unwrap();
        let expected = (0..COUNT)
            .filter(|i| {
                name_matches(
                    INSTANT_QUERY,
                    std::ffi::OsStr::new(&facet_bench::test_folder_entry(*i).name),
                )
            })
            .count() as u64;
        assert!(expected > 0);
        assert_eq!(status.total, Some(expected));
        assert!(!status.may_be_incomplete());
        assert!(!tab.is_empty(), "첫 화면에 결과가 있다");
        for i in 0..tab.len() {
            let folder = tab.folder_of(i).expect("결과 항목의 폴더");
            assert!(
                folder.starts_with(dir.path()),
                "결과는 시험 폴더 아래에 있다: {}",
                folder.display()
            );
            let name = &tab.get(i).unwrap().name;
            assert!(name_matches(INSTANT_QUERY, name), "{name:?}");
        }
    }
}
