// target: crates/facet-core/tests/spec_sync_folders_download.rs
// AC: #453: 온라인 전용 파일 열기는 곧바로 돌아오고 내려받기 작업(「내려받는 중」) 뒤 내용 요청·온라인 전용 풀림·열기 결과
// AC: #453: 이 장치에 있는 파일은 내려받기 작업 없이 엶, 멈춘 내려받기를 취소하면 열지 않음
// AC: #453: 무응답 시간 제한 뒤 TimedOut 질문과 그동안 다른 작업이 끝남·건너뛰면 열지 않음, 실패 뒤 다시 시도하면 열림
// AC: #453: 연결을 끊으면 곧바로 돌아오고 SyncProviderNotRunning 질문, 시간 제한 기본 30초, 작업 기록에 남지 않음
//! 온라인 전용 파일 열기(`App::execute(Open)` → `JobKind::Download` → 기본 프로그램으로 열기)를 시험용 동기화
//! 공급자(`facet_win::cloud_files::testing`)로 만든 진짜 온라인 전용 파일로 확인한다. 실제 프로그램이 뜨지 않게
//! 연결 프로그램이 없는 확장자를 쓰고, 열기를 시도한 것은 「연결 프로그램 없음」 열기 결과로 본다. Windows에서만
//! 돈다. 동기화 루트를 등록할 수 없는 환경이면 건너뛰었다고 출력하고, 환경 변수 `FACET_REQUIRE_SYNC_ROOT`가 있으면
//! 그 경우를 실패로 본다.
#![cfg(windows)]

use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::Path;
use std::time::{Duration, Instant};

use facet_core::command::CommandId;
use facet_core::job::{
    DOWNLOAD_STALL_TIMEOUT, ErrorAnswer, FileOpErrorKind, ItemFailure, JobId, JobKind, JobQuestion,
    JobState,
};
use facet_core::tab::ReadOutcome;
use facet_core::window::{App, Execution, Update};
use facet_core::{Location, OpenError};
use facet_win::cloud_files::testing::TestSyncRoot;
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(60);
/// 결과가 「오지 않음」을 볼 때 기다리는 시간.
const QUIET: Duration = Duration::from_secs(1);
const ENV_REQUIRE: &str = "FACET_REQUIRE_SYNC_ROOT";
/// 연결된 프로그램이 없는 확장자.
const NAME: &str = "문서.facetnoassoc7f3a";
const ONLINE_BITS: u32 = 0x1000 | 0x40_0000;

fn sync_root(dir: &Path, test: &str) -> Option<TestSyncRoot> {
    match TestSyncRoot::register(dir) {
        Ok(root) => Some(root),
        Err(error) => {
            assert!(
                std::env::var_os(ENV_REQUIRE).is_none(),
                "{ENV_REQUIRE}가 있는데 동기화 루트를 등록하지 못했다: {error}"
            );
            println!("{test}: 동기화 루트를 등록할 수 없어 건너뜀: {error}");
            None
        }
    }
}

fn content() -> Vec<u8> {
    (0..256 * 1024).map(|i| (i % 251) as u8).collect()
}

/// 상위 폴더의 열거 결과로 읽은 속성 값(파일을 열지 않는다).
fn attributes_of(path: &Path) -> u32 {
    let name = path.file_name().unwrap();
    fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap())
        .find(|e| e.file_name() == name)
        .expect("목록에 있다")
        .metadata()
        .unwrap()
        .file_attributes()
}

fn app_at(path: &Path) -> App {
    let mut app = App::new(Some(Location::file_system(path)));
    let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
    assert_eq!(outcome, ReadOutcome::Committed);
    app
}

/// 그 이름의 항목을 커서로 두고 연다. `execute`가 돌아오기까지 걸린 시간을 함께 돌려준다.
fn open(app: &mut App, name: &str) -> Duration {
    let index = app
        .active_tab()
        .items()
        .position(|item| item.name == name)
        .unwrap_or_else(|| panic!("{name}이 목록에 없다"));
    app.active_tab_mut().select_only(index);
    let started = Instant::now();
    assert_eq!(app.execute(CommandId::Open), Execution::Done);
    started.elapsed()
}

/// 앱의 결과를 받으며 열기 결과가 왔는지 센다.
struct Watch {
    opened: Vec<bool>,
}

impl Watch {
    fn new() -> Self {
        Watch { opened: Vec::new() }
    }

    fn pump(&mut self, app: &mut App) {
        app.poll_jobs();
        for (_, update) in app.poll() {
            if let Update::Opened(opened) = update {
                self.opened.push(opened);
            }
        }
    }
}

fn download_job(app: &App) -> Option<JobId> {
    app.work_area()
        .entries()
        .iter()
        .find(|entry| entry.kind == JobKind::Download)
        .map(|entry| entry.id)
}

fn wait_download_job(app: &mut App, watch: &mut Watch) -> JobId {
    let deadline = Instant::now() + WAIT;
    loop {
        watch.pump(app);
        if let Some(id) = download_job(app) {
            return id;
        }
        assert!(Instant::now() < deadline, "내려받기 작업이 생기지 않았다");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_question(app: &mut App, watch: &mut Watch, job: JobId) -> ItemFailure {
    let deadline = Instant::now() + WAIT;
    loop {
        watch.pump(app);
        let entry = app.work_area().entry(job).expect("작업 영역의 작업");
        if let Some(question) = &entry.question {
            match question {
                JobQuestion::Error(failure) => return failure.clone(),
                other => panic!("항목별 오류 질문이어야 한다: {other:?}"),
            }
        }
        assert!(
            entry.summary.is_none(),
            "질문 전에 끝났다: {:?}",
            entry.state
        );
        assert!(Instant::now() < deadline, "질문이 오지 않았다");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_finished(app: &mut App, watch: &mut Watch, job: JobId) -> JobState {
    let deadline = Instant::now() + WAIT;
    loop {
        watch.pump(app);
        let entry = app.work_area().entry(job).expect("작업 영역의 작업");
        if entry.is_finished() {
            return entry.state;
        }
        assert!(
            entry.question.is_none(),
            "묻지 않아야 한다: {:?}",
            entry.question
        );
        assert!(Instant::now() < deadline, "작업이 끝나지 않았다");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_opened(app: &mut App, watch: &mut Watch) -> bool {
    let deadline = Instant::now() + WAIT;
    loop {
        watch.pump(app);
        if let Some(opened) = watch.opened.first() {
            return *opened;
        }
        assert!(Instant::now() < deadline, "열기 결과가 오지 않았다");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn quiet(app: &mut App, watch: &mut Watch) {
    let until = Instant::now() + QUIET;
    while Instant::now() < until {
        watch.pump(app);
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn opening_online_only_file_downloads_then_opens() {
    let dir = TempDir::new().unwrap();
    let Some(root) = sync_root(dir.path(), "내려받은 뒤 열기") else {
        return;
    };
    let file = root.add_online_file(NAME, &content());
    let mut app = app_at(dir.path());
    let mut watch = Watch::new();
    let records_before = app.history().records().len();

    let took = open(&mut app, NAME);
    assert!(took < Duration::from_secs(1), "곧바로 돌아온다: {took:?}");
    let job = wait_download_job(&mut app, &mut watch);
    let state = wait_finished(&mut app, &mut watch, job);
    assert_eq!(state, JobState::Finished);
    let entry = app.work_area().entry(job).unwrap();
    assert!(
        entry.progress.as_ref().expect("진행이 왔다").downloading,
        "진행에 「내려받는 중」이 켜져 있다"
    );

    assert!(!wait_opened(&mut app, &mut watch), "연결 프로그램이 없다");
    assert_eq!(
        app.active_tab().open_error(),
        Some(&OpenError::NoAssociation)
    );
    assert!(root.fetch_data_count() >= 1);
    assert_eq!(
        attributes_of(&file) & ONLINE_BITS,
        0,
        "내려받은 뒤에는 온라인 전용이 아니다"
    );
    assert_eq!(fs::read(&file).unwrap(), content());
    assert_eq!(
        app.history().records().len(),
        records_before,
        "작업 기록에 남지 않는다"
    );
}

#[test]
fn local_file_opens_without_download_job() {
    let dir = TempDir::new().unwrap();
    let Some(_root) = sync_root(dir.path(), "로컬 파일 열기") else {
        return;
    };
    fs::write(dir.path().join(NAME), b"local").unwrap();
    let mut app = app_at(dir.path());
    let mut watch = Watch::new();
    open(&mut app, NAME);
    assert!(!wait_opened(&mut app, &mut watch));
    assert_eq!(download_job(&app), None);
}

#[test]
fn cancelled_stalled_download_does_not_open() {
    let dir = TempDir::new().unwrap();
    let Some(root) = sync_root(dir.path(), "내려받기 취소") else {
        return;
    };
    root.add_online_file(NAME, &content());
    root.set_stall(true);
    let mut app = app_at(dir.path());
    let mut watch = Watch::new();
    open(&mut app, NAME);
    let job = wait_download_job(&mut app, &mut watch);
    assert!(app.jobs_mut().cancel(job));
    let state = wait_finished(&mut app, &mut watch, job);
    root.set_stall(false);
    assert_eq!(state, JobState::Cancelled);
    quiet(&mut app, &mut watch);
    assert!(watch.opened.is_empty(), "취소하면 열지 않는다");
}

#[test]
fn stalled_provider_times_out_into_item_error_while_other_jobs_run() {
    let dir = TempDir::new().unwrap();
    let other = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    let Some(root) = sync_root(dir.path(), "무응답 시간 제한") else {
        return;
    };
    root.add_online_file(NAME, &content());
    let local = other.path().join("다른 작업.txt");
    fs::write(&local, b"other job").unwrap();
    root.set_stall(true);

    let mut app = app_at(dir.path());
    app.jobs_mut().set_stall_timeout(Duration::from_secs(2));
    let mut watch = Watch::new();
    open(&mut app, NAME);
    let job = wait_download_job(&mut app, &mut watch);
    app.jobs_mut()
        .copy(vec![local], target.path().to_path_buf())
        .expect("복사 작업");

    let failure = wait_question(&mut app, &mut watch, job);
    assert_eq!(failure.kind, FileOpErrorKind::TimedOut);
    // 답을 기다리는 동안 뒤의 작업이 끝난다.
    let copied = target.path().join("다른 작업.txt");
    let deadline = Instant::now() + WAIT;
    while fs::read(&copied).ok().as_deref() != Some(&b"other job"[..]) {
        watch.pump(&mut app);
        assert!(Instant::now() < deadline, "다른 작업이 끝나지 않았다");
        std::thread::sleep(Duration::from_millis(10));
    }

    app.answer_error(job, ErrorAnswer::Skip).unwrap();
    let state = wait_finished(&mut app, &mut watch, job);
    root.set_stall(false);
    assert_eq!(state, JobState::Failed);
    quiet(&mut app, &mut watch);
    assert!(watch.opened.is_empty(), "건너뛰면 열지 않는다");
}

#[test]
fn failed_download_asks_and_retry_opens() {
    let dir = TempDir::new().unwrap();
    let Some(root) = sync_root(dir.path(), "내려받기 실패") else {
        return;
    };
    let file = root.add_online_file(NAME, &content());
    root.set_fail(true);
    let mut app = app_at(dir.path());
    let mut watch = Watch::new();
    open(&mut app, NAME);
    let job = wait_download_job(&mut app, &mut watch);
    let failure = wait_question(&mut app, &mut watch, job);
    assert_eq!(failure.source, file);
    assert!(watch.opened.is_empty(), "답하기 전에는 열지 않는다");

    root.set_fail(false);
    app.answer_error(job, ErrorAnswer::Retry).unwrap();
    let state = wait_finished(&mut app, &mut watch, job);
    assert_eq!(state, JobState::Finished);
    assert!(!wait_opened(&mut app, &mut watch));
    assert_eq!(fs::read(&file).unwrap(), content());
}

#[test]
fn provider_not_running_asks_without_hanging() {
    let dir = TempDir::new().unwrap();
    let Some(root) = sync_root(dir.path(), "동기화 프로그램 꺼짐") else {
        return;
    };
    root.add_online_file(NAME, &content());
    let mut app = app_at(dir.path());
    root.disconnect();
    let mut watch = Watch::new();
    let took = open(&mut app, NAME);
    assert!(took < Duration::from_secs(1), "곧바로 돌아온다: {took:?}");
    let job = wait_download_job(&mut app, &mut watch);
    let failure = wait_question(&mut app, &mut watch, job);
    assert_eq!(failure.kind, FileOpErrorKind::SyncProviderNotRunning);
    app.answer_error(job, ErrorAnswer::CancelJob).unwrap();
    assert_eq!(
        wait_finished(&mut app, &mut watch, job),
        JobState::Cancelled
    );
    assert!(watch.opened.is_empty());
}

#[test]
fn download_stall_timeout_defaults_to_30_seconds() {
    assert_eq!(DOWNLOAD_STALL_TIMEOUT, Duration::from_secs(30));
}
