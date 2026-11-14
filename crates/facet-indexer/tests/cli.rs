//! 실행 파일의 명령줄 동작. 관리자 권한이 필요 없다.

use std::process::Command;

fn indexer(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_facet-indexer"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn help_succeeds_and_lists_dump_mft() {
    let out = indexer(&["help"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("dump-mft"));
}

#[test]
fn bad_arguments_fail_with_a_message_not_a_panic() {
    for args in [&["dump-mft"][..], &["dump-mft", "12", "o"], &["nope"]] {
        let out = indexer(args);
        assert!(!out.status.success(), "{args:?}");
        // 패닉이면 종료 코드 101이다.
        assert_ne!(out.status.code(), Some(101), "{args:?}");
        assert!(!out.stderr.is_empty(), "{args:?}");
        assert!(
            !String::from_utf8_lossy(&out.stderr).contains("panicked"),
            "{args:?}"
        );
    }
}

/// 없는 드라이브는 관리자 여부와 상관없이 실패하되 패닉 없이 이유를 알리고, 출력 파일을 남기지 않는다.
#[test]
fn dump_of_missing_volume_fails_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let out_path = dir.path().join("x.mft");
    let missing = (b'D'..=b'Z')
        .rev()
        .map(char::from)
        .find(|c| !std::path::Path::new(&format!("{c}:\\")).exists())
        .expect("빈 드라이브 문자가 없음");
    let out = indexer(&[
        "dump-mft",
        &format!("{missing}:"),
        out_path.to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    assert_ne!(out.status.code(), Some(101));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("panicked"));
    assert!(!out_path.exists());
}
