// target: crates/facet-core/tests/spec_zip_password.rs
// AC: #428: read_entry가 ZipCrypto·AES를 맞는 암호로 풀고 틀리면 WrongPassword·없으면 PasswordNeeded, 꺼내기의 암호 질문과 다시 묻기, 한 작업에 한 번만, 취소와 NotAsking, 앱이 zip을 보는 동안 암호를 기억
//! 암호 zip 꺼내기를 임시 폴더의 실제 zip 파일로 확인한다. 암호화는 `spec_zip_support.rs`의 도우미(ZipCrypto,
//! WinZip AES AE-2·AES-256)가 한다. 리더의 `Archive::read_entry`는 모든 플랫폼에서, 꺼내기 작업의 암호 질문과
//! 앱의 암호 기억은 Windows에서만 돈다(파일 연산이 Windows 전용).

#[path = "spec_zip_support.rs"]
mod zip_support;

use facet_core::zip_archive::{Archive, EntryError};
use tempfile::TempDir;
use zip_support::Entry;

const PASSWORD: &str = "secret-1234";

fn body(name: &str) -> Vec<u8> {
    format!("{name}: 암호 안의 내용입니다. ")
        .repeat(20)
        .into_bytes()
}

#[test]
#[ignore = "#428"]
fn read_entry_decrypts_zipcrypto_and_aes() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("암호.zip");
    zip_support::write_zip(
        &path,
        &[
            Entry::stored("zc-stored.txt", &body("zc-stored")).zipcrypto(PASSWORD),
            Entry::deflated("zc-deflate.txt", &body("zc-deflate")).zipcrypto(PASSWORD),
            Entry::stored("aes-stored.txt", &body("aes-stored")).aes(PASSWORD),
            Entry::deflated("aes-deflate.txt", &body("aes-deflate")).aes(PASSWORD),
        ],
    );
    let archive = Archive::open(&path).unwrap();
    for name in ["zc-stored", "zc-deflate", "aes-stored", "aes-deflate"] {
        let entry = format!("{name}.txt");
        assert_eq!(
            archive.read_entry(&entry, Some(PASSWORD)),
            Ok(body(name)),
            "{entry}를 맞는 암호로 푼다"
        );
        assert_eq!(
            archive.read_entry(&entry, Some("wrong-password")),
            Err(EntryError::WrongPassword),
            "{entry}의 틀린 암호"
        );
        assert_eq!(
            archive.read_entry(&entry, None),
            Err(EntryError::PasswordNeeded),
            "{entry}는 암호가 있어야 한다"
        );
    }
}

#[cfg(windows)]
mod jobs {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use facet_core::Location;
    use facet_core::job::{AnswerError, JobEvent, JobId, JobKind, JobQuestion, JobQueue, JobState};
    use facet_core::tab::{Navigation, ReadOutcome};
    use facet_core::window::{App, TransferPlan};
    use tempfile::TempDir;

    use super::zip_support::{self, Entry};
    use super::{PASSWORD, body};

    const WAIT: Duration = Duration::from_secs(120);

    struct Fixture {
        dir: TempDir,
        zip: PathBuf,
    }

    /// `암호.zip`: zc.txt(ZipCrypto), aes.txt(AES), plain.txt(암호 없음).
    fn fixture() -> Fixture {
        let dir = TempDir::new().unwrap();
        let zip = dir.path().join("암호.zip");
        zip_support::write_zip(
            &zip,
            &[
                Entry::deflated("zc.txt", &body("zc")).zipcrypto(PASSWORD),
                Entry::deflated("aes.txt", &body("aes")).aes(PASSWORD),
                Entry::stored("plain.txt", b"plain"),
            ],
        );
        Fixture { dir, zip }
    }

    fn target(f: &Fixture, name: &str) -> PathBuf {
        let path = f.dir.path().join(name);
        fs::create_dir(&path).unwrap();
        path
    }

    /// 다음 암호 질문이나 끝남.
    enum Next {
        Asked(PathBuf, bool),
        Finished,
    }

    fn next(queue: &mut JobQueue, job: JobId) -> Next {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match queue.wait_event(left) {
                Some(JobEvent::PasswordNeeded {
                    job: id,
                    archive,
                    retry,
                }) if id == job => return Next::Asked(archive, retry),
                Some(JobEvent::Finished { job: id, .. }) if id == job => return Next::Finished,
                Some(JobEvent::Conflict { job: id, .. })
                | Some(JobEvent::ItemError { job: id, .. })
                    if id == job =>
                {
                    panic!("암호 질문 말고는 묻지 않는다")
                }
                Some(_) => {}
                None => panic!("작업이 시간 안에 끝나지 않았다"),
            }
        }
    }

    fn asked(queue: &mut JobQueue, job: JobId) -> (PathBuf, bool) {
        match next(queue, job) {
            Next::Asked(archive, retry) => (archive, retry),
            Next::Finished => panic!("암호를 묻기 전에 끝났다"),
        }
    }

    fn finished(queue: &mut JobQueue, job: JobId) -> JobState {
        match next(queue, job) {
            Next::Finished => queue.state(job).unwrap(),
            Next::Asked(archive, retry) => {
                panic!("더 묻지 않아야 한다: {} (retry {retry})", archive.display())
            }
        }
    }

    #[test]
    #[ignore = "#428"]
    fn extraction_asks_again_until_the_password_is_right() {
        let f = fixture();
        let mut queue = JobQueue::new();
        for name in ["zc.txt", "aes.txt"] {
            let out = target(&f, &format!("대상 {name}"));
            let job = queue.copy(vec![f.zip.join(name)], out.clone()).unwrap();
            assert_eq!(asked(&mut queue, job), (f.zip.clone(), false), "{name}");
            assert_eq!(queue.state(job), Some(JobState::AwaitingAnswer));
            queue
                .answer_password(job, Some("wrong-password".into()))
                .unwrap();
            assert_eq!(
                asked(&mut queue, job),
                (f.zip.clone(), true),
                "{name}: 틀리면 다시 묻는다"
            );
            queue.answer_password(job, Some(PASSWORD.into())).unwrap();
            assert_eq!(finished(&mut queue, job), JobState::Finished);
            let stem = name.trim_end_matches(".txt");
            assert_eq!(fs::read(out.join(name)).unwrap(), body(stem));
        }
    }

    #[test]
    #[ignore = "#428"]
    fn one_job_asks_only_once_for_the_same_zip() {
        let f = fixture();
        let out = target(&f, "대상");
        let mut queue = JobQueue::new();
        let job = queue.extract_all(f.zip.clone(), out.clone()).unwrap();
        let _ = asked(&mut queue, job);
        queue.answer_password(job, Some(PASSWORD.into())).unwrap();
        assert_eq!(finished(&mut queue, job), JobState::Finished);
        assert_eq!(fs::read(out.join("zc.txt")).unwrap(), body("zc"));
        assert_eq!(fs::read(out.join("aes.txt")).unwrap(), body("aes"));
        assert_eq!(fs::read(out.join("plain.txt")).unwrap(), b"plain");
    }

    #[test]
    #[ignore = "#428"]
    fn no_password_cancels_and_late_answers_are_refused() {
        let f = fixture();
        let out = target(&f, "대상");
        let mut queue = JobQueue::new();
        let job = queue
            .copy(vec![f.zip.join("aes.txt")], out.clone())
            .unwrap();
        let _ = asked(&mut queue, job);
        queue.answer_password(job, None).unwrap();
        assert_eq!(finished(&mut queue, job), JobState::Cancelled);
        assert_eq!(
            fs::read_dir(&out).unwrap().count(),
            0,
            "아무것도 남지 않는다"
        );
        assert_eq!(
            queue.answer_password(job, Some(PASSWORD.into())),
            Err(AnswerError::NotAsking)
        );

        let plain = queue
            .copy(vec![f.zip.join("plain.txt")], out.clone())
            .unwrap();
        assert_eq!(finished(&mut queue, plain), JobState::Finished);
        assert_eq!(
            queue.answer_password(plain, Some(PASSWORD.into())),
            Err(AnswerError::NotAsking)
        );
    }

    fn app_at(location: Location) -> App {
        let mut app = App::new(Some(location));
        let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
        assert_eq!(outcome, ReadOutcome::Committed);
        app
    }

    fn plan(f: &Fixture, name: &str, out: &Path) -> TransferPlan {
        TransferPlan {
            kind: JobKind::Copy,
            sources: vec![f.zip.join(name)],
            target: out.to_path_buf(),
        }
    }

    /// 앱에서 작업이 끝나기를 기다린다. 암호 질문이 오면 `password`로 답하고(없으면 실패) 물었는지를 돌려준다.
    fn run(app: &mut App, job: JobId, password: Option<&str>) -> (JobState, bool) {
        let deadline = Instant::now() + WAIT;
        let mut was_asked = false;
        loop {
            app.poll_jobs();
            app.poll();
            let entry = app.work_area().entry(job).expect("작업 영역의 작업");
            if let Some(summary) = &entry.summary {
                assert!(summary.failures.is_empty(), "{summary:?}");
                return (entry.state, was_asked);
            }
            if let Some(JobQuestion::Password { archive, retry }) = entry.question.clone() {
                assert!(!retry);
                let password = password.unwrap_or_else(|| {
                    panic!("{}의 암호를 다시 묻지 않아야 한다", archive.display())
                });
                was_asked = true;
                app.answer_password(job, Some(password.to_string()))
                    .expect("답을 받는다");
            }
            assert!(Instant::now() < deadline, "작업이 시간 안에 끝나지 않았다");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    #[ignore = "#428"]
    fn the_app_remembers_the_password_while_the_zip_is_open() {
        let f = fixture();
        let mut app = app_at(Location::zip(&f.zip, ""));

        let first = target(&f, "첫째");
        let job = app.start_transfer(&plan(&f, "zc.txt", &first)).unwrap();
        assert_eq!(
            run(&mut app, job, Some(PASSWORD)),
            (JobState::Finished, true)
        );
        assert_eq!(fs::read(first.join("zc.txt")).unwrap(), body("zc"));

        // 같은 zip을 보는 동안에는 다시 묻지 않는다.
        let second = target(&f, "둘째");
        let job = app.start_transfer(&plan(&f, "aes.txt", &second)).unwrap();
        assert_eq!(run(&mut app, job, None), (JobState::Finished, false));
        assert_eq!(fs::read(second.join("aes.txt")).unwrap(), body("aes"));

        // zip을 떠나면 잊는다.
        assert!(app.navigate(Navigation::Go(Location::file_system(f.dir.path()))));
        let (_, outcome) = app.wait(WAIT).expect("읽기");
        assert_eq!(outcome, ReadOutcome::Committed);
        let third = target(&f, "셋째");
        let job = app.start_transfer(&plan(&f, "zc.txt", &third)).unwrap();
        assert_eq!(
            run(&mut app, job, Some(PASSWORD)),
            (JobState::Finished, true)
        );
        assert_eq!(fs::read(third.join("zc.txt")).unwrap(), body("zc"));
    }
}
