//! 시험용 질의 명령 실행 파일을 같은 프로세스의 파이프 서버에 붙여 본다. 관리자 권한이 필요 없다.

use std::process::{Command, Output};

fn query(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_facet-query"))
        .args(args)
        .output()
        .unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn assert_no_panic(out: &Output) {
    // 패닉이면 종료 코드 101이다.
    assert_ne!(out.status.code(), Some(101));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("panicked"));
}

#[test]
fn help_lists_commands() {
    let out = query(&["help"]);
    assert!(out.status.success());
    let text = stdout(&out);
    for word in ["status", "search", "--sort", "--limit", "--pages", "--all"] {
        assert!(text.contains(word), "{word}");
    }
}

#[test]
fn bad_arguments_fail_with_a_message_not_a_panic() {
    for args in [&["search"][..], &["search", "a", "--limit", "x"], &["nope"]] {
        let out = query(args);
        assert!(!out.status.success(), "{args:?}");
        assert_no_panic(&out);
        assert!(!out.stderr.is_empty(), "{args:?}");
    }
}

#[test]
fn missing_service_is_reported_without_panic() {
    let out = query(&[
        "--pipe",
        &format!(r"\\.\pipe\facet-test-nobody-{}", std::process::id()),
        "status",
    ]);
    assert!(!out.status.success());
    assert_no_panic(&out);
    if cfg!(windows) {
        assert_eq!(out.status.code(), Some(2));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("인덱서 서비스가 없습니다"),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[cfg(windows)]
mod with_server {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use facet_indexer::pipe::PipeServer;
    use facet_indexer::sample::{SAMPLE_VOLUME, SampleBackend};

    use super::*;

    const REPORTS: u64 = 30;

    fn server() -> (PipeServer, String) {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!(
            r"\\.\pipe\facet-query-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let backend = Arc::new(SampleBackend::new(REPORTS));
        (PipeServer::start(&name, backend).unwrap(), name)
    }

    fn lines(out: &Output) -> Vec<String> {
        stdout(out).lines().map(str::to_owned).collect()
    }

    #[test]
    fn status_prints_volume_state_and_item_count() {
        let (_s, pipe) = server();
        let out = query(&["--pipe", &pipe, "status"]);
        assert!(out.status.success(), "{out:?}");
        assert_eq!(
            lines(&out),
            [format!("{SAMPLE_VOLUME}\t준비됨\t항목 {}개", REPORTS + 4)]
        );
    }

    #[test]
    fn search_prints_sorted_limited_hits_and_total() {
        let (_s, pipe) = server();
        let out = query(&[
            "--pipe",
            &pipe,
            "search",
            "보고서",
            "--sort",
            "size",
            "--desc",
            "--limit",
            "3",
        ]);
        assert!(out.status.success(), "{out:?}");
        let lines = lines(&out);
        assert_eq!(
            lines[..3],
            [
                format!(r"{SAMPLE_VOLUME}\문서\보고서 30.txt	3000"),
                format!(r"{SAMPLE_VOLUME}\문서\보고서 29.txt	2900"),
                format!(r"{SAMPLE_VOLUME}\문서\보고서 28.txt	2800"),
            ]
        );
        assert_eq!(lines[3], format!("결과 3개 / 전체 {REPORTS}개"));
        assert!(lines[4].contains("남은 결과 27개"), "{}", lines[4]);
    }

    #[test]
    fn search_resumes_pages() {
        let (_s, pipe) = server();
        let out = query(&[
            "--pipe",
            &pipe,
            "search",
            "보고서",
            "--sort",
            "name",
            "--limit",
            "4",
            "--pages",
            "2",
        ]);
        assert!(out.status.success(), "{out:?}");
        let first = lines(&out);
        assert_eq!(first.len(), 8 + 2);
        assert_eq!(first[8], format!("결과 8개 / 전체 {REPORTS}개"));

        let out = query(&["--pipe", &pipe, "search", "보고서", "--limit", "7", "--all"]);
        assert!(out.status.success(), "{out:?}");
        let lines = lines(&out);
        assert_eq!(lines.len() as u64, REPORTS + 1);
        assert_eq!(
            lines.last().unwrap(),
            &format!("결과 {REPORTS}개 / 전체 {REPORTS}개")
        );
    }

    #[test]
    fn wildcard_and_regex_conditions() {
        let (_s, pipe) = server();
        let out = query(&["--pipe", &pipe, "search", "--wildcard", "*.jpg"]);
        assert!(out.status.success(), "{out:?}");
        assert_eq!(
            lines(&out),
            [
                format!(r"{SAMPLE_VOLUME}\사진\여름 바다.jpg	2000000"),
                "결과 1개 / 전체 1개".to_owned()
            ]
        );

        let out = query(&[
            "--pipe",
            &pipe,
            "search",
            "--regex",
            "^readme",
            "--case-sensitive",
        ]);
        assert!(out.status.success(), "{out:?}");
        assert_eq!(lines(&out), ["결과 0개 / 전체 0개"]);
    }

    #[test]
    fn service_error_is_reported_without_panic() {
        let (_s, pipe) = server();
        let out = query(&["--pipe", &pipe, "search", "--regex", "("]);
        assert_eq!(out.status.code(), Some(1));
        assert_no_panic(&out);
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("InvalidQuery"),
            "{out:?}"
        );
    }
}
