// target: crates/facet-core/tests/spec_install_update_check.rs
// AC: #475: 첫 지연 뒤 첫 확인과 알림·UpdateChecked, GET·User-Agent·사용자 정보 없음, 주기마다 확인, 주기를 끄면 명령만, nightly는 요청 없음·명령 꺼짐, 응답 없음은 막지 않고 TimedOut, HTTP 500은 결과로만·다음 주기 재시도, 이 버전 건너뛰기, 명령 id·단축키 없음·확인 중 꺼짐
// AC: #476: updates.check 설정 정의, 값 → 주기 대응, 주기를 켜면 곧바로 확인·끄면 멈춤
//! 앱의 업데이트 확인(`App::enable_updates`·`CommandId::CheckForUpdates`·`App::skip_update`)을 같은 프로세스에 띄운
//! 작은 HTTP 서버(`127.0.0.1`, GitHub Releases API 목록 JSON을 흉내)로 확인한다: 첫 지연·주기·끄기, nightly 채널, 서버가
//! 답하지 않거나 오류를 낼 때 UI가 기다리지 않고 결과로만 남는 것, 요청에 사용자 정보가 없는 것, 건너뛴 버전이 상태
//! 폴더에 남아 다음 실행에도 알리지 않는 것. 설정 정의(#476)와 값 → 주기 대응은 모든 플랫폼, 서버를 쓰는 시험은 Windows
//! 전용이다.

use std::time::Duration;

use facet_core::command::CommandId;
use facet_core::settings::{self, Category, Kind, Value};
use facet_core::update::{self, interval_for};

#[test]
#[ignore = "#475"]
fn check_for_updates_command_is_listed_without_a_shortcut() {
    assert!(CommandId::ALL.contains(&CommandId::CheckForUpdates));
    assert_eq!(CommandId::CheckForUpdates.as_str(), "check-for-updates");
    assert_eq!(
        CommandId::from_id("check-for-updates"),
        Some(CommandId::CheckForUpdates)
    );
    assert!(CommandId::CheckForUpdates.shortcuts().is_empty());
}

#[test]
#[ignore = "#475"]
fn default_options_are_30s_delay_daily_10s_timeout() {
    let dir = tempfile::TempDir::new().unwrap();
    let options = update::UpdateOptions::new(dir.path());
    assert_eq!(options.releases_url, update::RELEASES_URL);
    assert_eq!(
        update::RELEASES_URL,
        "https://api.github.com/repos/farrar142/facet/releases"
    );
    assert_eq!(options.first_delay, Duration::from_secs(30));
    assert_eq!(options.first_delay, update::FIRST_CHECK_DELAY);
    assert_eq!(options.interval, Some(Duration::from_secs(24 * 3600)));
    assert_eq!(update::DEFAULT_INTERVAL, Duration::from_secs(24 * 3600));
    assert_eq!(options.timeout, Duration::from_secs(10));
    assert_eq!(update::REQUEST_TIMEOUT, Duration::from_secs(10));
    assert_eq!(options.state_folder, dir.path());
    assert_eq!(options.channel, facet_core::build::channel());
    assert_eq!(
        options.current,
        update::Version::parse(facet_core::build::VERSION).unwrap()
    );
}

#[test]
#[ignore = "#476"]
fn update_check_interval_setting_is_defined() {
    let definition = settings::definition("updates.check").expect("updates.check 정의");
    assert_eq!(definition.category, Category::General);
    assert_eq!(definition.kind, Kind::Choice(&["off", "daily", "weekly"]));
    assert_eq!(definition.default, Value::Choice("daily"));
    assert!(
        settings::definitions()
            .iter()
            .any(|d| d.key == "updates.check")
    );
}

#[test]
#[ignore = "#476"]
fn setting_values_map_to_intervals() {
    assert_eq!(interval_for("off"), None);
    assert_eq!(interval_for("daily"), Some(Duration::from_secs(24 * 3600)));
    assert_eq!(
        interval_for("weekly"),
        Some(Duration::from_secs(7 * 24 * 3600))
    );
    assert_eq!(interval_for("hourly"), None, "모르는 값은 끔");
}

#[cfg(windows)]
mod with_server {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::path::Path;
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use facet_core::Location;
    use facet_core::build::Channel;
    use facet_core::command::CommandId;
    use facet_core::tab::ReadOutcome;
    use facet_core::update::{UpdateCheckError, UpdateDecision, UpdateOptions, Version};
    use facet_core::window::{App, Update};
    use tempfile::TempDir;

    const WAIT: Duration = Duration::from_secs(15);

    /// 서버가 다음 요청에 줄 답.
    #[derive(Clone)]
    enum Reply {
        Json(String),
        Status(u16),
        /// 받고 답하지 않는다(연결은 붙든다).
        Hang,
    }

    /// 같은 프로세스의 작은 HTTP/1.1 서버. 받은 요청 머리(첫 줄과 머리들)를 차례로 넘긴다.
    struct Server {
        addr: SocketAddr,
        reply: Arc<Mutex<Reply>>,
        requests: Receiver<String>,
    }

    impl Server {
        fn start(reply: Reply) -> Server {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let reply = Arc::new(Mutex::new(reply));
            let (sender, requests) = mpsc::channel();
            let shared = Arc::clone(&reply);
            thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { return };
                    let reply = shared.lock().unwrap().clone();
                    let sender = sender.clone();
                    thread::spawn(move || answer(stream, reply, sender));
                }
            });
            Server {
                addr,
                reply,
                requests,
            }
        }

        fn url(&self) -> String {
            format!("http://{}/repos/farrar142/facet/releases", self.addr)
        }

        fn set(&self, reply: Reply) {
            *self.reply.lock().unwrap() = reply;
        }

        /// 지금까지 온 요청을 모두 꺼낸다.
        fn drain(&self) -> Vec<String> {
            self.requests.try_iter().collect()
        }
    }

    fn answer(mut stream: TcpStream, reply: Reply, sender: Sender<String>) {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            match stream.read(&mut byte) {
                Ok(1) => head.push(byte[0]),
                _ => return,
            }
        }
        let _ = sender.send(String::from_utf8_lossy(&head).into_owned());
        match reply {
            Reply::Json(body) => {
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
            Reply::Status(code) => {
                let _ = write!(
                    stream,
                    "HTTP/1.1 {code} Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
            }
            Reply::Hang => thread::sleep(Duration::from_secs(30)),
        }
    }

    /// 정식 릴리스 목록 JSON(태그마다 하나, 최근 것부터).
    fn releases_json(tags: &[&str]) -> String {
        let items: Vec<String> = tags
            .iter()
            .map(|tag| {
                format!(
                    r#"{{"tag_name":"{tag}","html_url":"https://github.com/farrar142/facet/releases/tag/{tag}","draft":false,"prerelease":false,"assets":[]}}"#
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    fn options(server: &Server, state: &Path, channel: Channel) -> UpdateOptions {
        let mut options = UpdateOptions::new(state);
        options.releases_url = server.url();
        options.current = Version::parse("0.1.0").unwrap();
        options.channel = channel;
        options.first_delay = Duration::from_millis(300);
        options.interval = None;
        options.timeout = Duration::from_secs(5);
        options
    }

    fn app_at(path: &Path) -> App {
        let mut app = App::new(Some(Location::file_system(path)));
        let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
        assert_eq!(outcome, ReadOutcome::Committed);
        app
    }

    /// `duration` 동안 앱을 돌리며 온 요청을 모은다.
    fn pump_for(app: &mut App, server: &Server, duration: Duration) -> Vec<String> {
        let deadline = Instant::now() + duration;
        let mut got = Vec::new();
        while Instant::now() < deadline {
            app.wait_update(Duration::from_millis(20));
            got.extend(server.drain());
        }
        got.extend(server.drain());
        got
    }

    /// 요청이 `count`개 올 때까지 앱을 돌린다.
    fn pump_until_requests(app: &mut App, server: &Server, count: usize) -> Vec<String> {
        let deadline = Instant::now() + WAIT;
        let mut got = Vec::new();
        while got.len() < count {
            assert!(
                Instant::now() < deadline,
                "요청 {count}개를 기다렸으나 {}개만 왔다",
                got.len()
            );
            app.wait_update(Duration::from_millis(20));
            got.extend(server.drain());
        }
        got
    }

    /// 확인이 하나 끝날 때까지(`Update::UpdateChecked`) 기다린다.
    fn wait_checked(app: &mut App) {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "업데이트 확인 결과가 오지 않았다");
            if let Some((_, Update::UpdateChecked)) = app.wait_update(left) {
                return;
            }
        }
    }

    fn available_tag(app: &App) -> Option<String> {
        match app.update_check() {
            Some(Ok(UpdateDecision::Available(release))) => Some(release.tag.clone()),
            _ => None,
        }
    }

    #[test]
    #[ignore = "#475"]
    fn first_check_waits_for_the_delay_and_announces_a_new_version() {
        let dir = TempDir::new().unwrap();
        let server = Server::start(Reply::Json(releases_json(&["v9.0.0", "v0.1.0"])));
        let mut app = app_at(dir.path());
        let mut options = options(&server, &dir.path().join("state"), Channel::Stable);
        options.first_delay = Duration::from_millis(600);
        options.interval = Some(Duration::from_secs(3600));

        let started = Instant::now();
        app.enable_updates(options);
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "켜기는 곧바로 돌아온다"
        );
        assert!(app.update_notice().is_none());
        assert!(
            pump_for(&mut app, &server, Duration::from_millis(300)).is_empty(),
            "첫 지연 전에는 요청이 없다"
        );

        let requests = pump_until_requests(&mut app, &server, 1);
        assert!(started.elapsed() >= Duration::from_millis(550));
        let head = requests[0].to_ascii_lowercase();
        assert!(
            head.starts_with("get /repos/farrar142/facet/releases"),
            "GET 하나: {head}"
        );
        assert!(head.contains("\r\nuser-agent:"), "User-Agent가 있다");
        assert!(
            !head.contains("\r\nauthorization:"),
            "인증 정보를 보내지 않는다"
        );
        assert!(!head.contains("\r\ncookie:"), "쿠키를 보내지 않는다");

        wait_checked(&mut app);
        assert_eq!(available_tag(&app).as_deref(), Some("v9.0.0"));
        assert_eq!(app.update_notice().map(|r| r.tag.as_str()), Some("v9.0.0"));
    }

    #[test]
    #[ignore = "#475"]
    fn checks_again_every_interval() {
        let dir = TempDir::new().unwrap();
        let server = Server::start(Reply::Json(releases_json(&["v0.1.0"])));
        let mut app = app_at(dir.path());
        let mut options = options(&server, &dir.path().join("state"), Channel::Stable);
        options.first_delay = Duration::from_millis(100);
        options.interval = Some(Duration::from_millis(300));
        app.enable_updates(options);

        assert!(pump_until_requests(&mut app, &server, 3).len() >= 3);
        assert_eq!(
            app.update_check().cloned(),
            Some(Ok(UpdateDecision::UpToDate))
        );
        assert!(app.update_notice().is_none());
    }

    #[test]
    #[ignore = "#475"]
    fn no_interval_means_no_automatic_check_but_the_command_checks() {
        let dir = TempDir::new().unwrap();
        let server = Server::start(Reply::Json(releases_json(&["v2.0.0"])));
        let mut app = app_at(dir.path());
        let mut options = options(&server, &dir.path().join("state"), Channel::Stable);
        options.first_delay = Duration::from_millis(100);
        options.interval = None;
        app.enable_updates(options);

        assert!(
            pump_for(&mut app, &server, Duration::from_millis(800)).is_empty(),
            "주기를 끄면 첫 확인도 없다"
        );
        assert!(app.can_execute(CommandId::CheckForUpdates));
        app.execute(CommandId::CheckForUpdates);
        assert_eq!(pump_until_requests(&mut app, &server, 1).len(), 1);
        wait_checked(&mut app);
        assert_eq!(available_tag(&app).as_deref(), Some("v2.0.0"));
    }

    #[test]
    #[ignore = "#475"]
    fn nightly_channel_sends_no_request_and_disables_the_command() {
        let dir = TempDir::new().unwrap();
        let server = Server::start(Reply::Json(releases_json(&["v9.0.0"])));
        let mut app = app_at(dir.path());
        assert!(
            !app.can_execute(CommandId::CheckForUpdates),
            "켜기 전에는 꺼져 있다"
        );
        let mut options = options(&server, &dir.path().join("state"), Channel::Nightly);
        options.first_delay = Duration::from_millis(50);
        options.interval = Some(Duration::from_millis(200));
        app.enable_updates(options);

        assert!(!app.can_execute(CommandId::CheckForUpdates));
        app.execute(CommandId::CheckForUpdates);
        assert!(
            pump_for(&mut app, &server, Duration::from_secs(1)).is_empty(),
            "nightly는 요청을 보내지 않는다"
        );
        assert!(app.update_notice().is_none());
        assert!(app.update_check().is_none());
    }

    #[test]
    #[ignore = "#475"]
    fn silent_server_does_not_block_and_times_out() {
        let dir = TempDir::new().unwrap();
        let server = Server::start(Reply::Hang);
        let mut app = app_at(dir.path());
        let mut options = options(&server, &dir.path().join("state"), Channel::Stable);
        options.timeout = Duration::from_millis(500);

        let started = Instant::now();
        app.enable_updates(options);
        app.execute(CommandId::CheckForUpdates);
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "켜기와 명령은 곧바로 돌아온다: {:?}",
            started.elapsed()
        );
        assert!(
            !app.can_execute(CommandId::CheckForUpdates),
            "확인 중에는 명령이 꺼진다"
        );

        wait_checked(&mut app);
        let took = started.elapsed();
        assert_eq!(
            app.update_check().cloned(),
            Some(Err(UpdateCheckError::TimedOut))
        );
        assert!(
            took >= Duration::from_millis(400),
            "시간 제한을 지킨다: {took:?}"
        );
        assert!(
            took < Duration::from_secs(5),
            "시간 제한 뒤 끝난다: {took:?}"
        );
        assert!(app.update_notice().is_none());
        assert!(
            app.can_execute(CommandId::CheckForUpdates),
            "끝나면 다시 켜진다"
        );
    }

    #[test]
    #[ignore = "#475"]
    fn server_error_is_only_a_result_and_is_retried_next_interval() {
        let dir = TempDir::new().unwrap();
        let server = Server::start(Reply::Status(500));
        let mut app = app_at(dir.path());
        let mut options = options(&server, &dir.path().join("state"), Channel::Stable);
        options.first_delay = Duration::from_millis(100);
        options.interval = Some(Duration::from_millis(500));
        app.enable_updates(options);

        wait_checked(&mut app);
        assert_eq!(
            app.update_check().cloned(),
            Some(Err(UpdateCheckError::Http(500)))
        );
        assert!(app.update_notice().is_none());

        server.set(Reply::Json(releases_json(&["v3.0.0"])));
        let deadline = Instant::now() + WAIT;
        while available_tag(&app).is_none() {
            assert!(
                Instant::now() < deadline,
                "다음 주기에 다시 확인하지 않았다"
            );
            wait_checked(&mut app);
        }
        assert_eq!(available_tag(&app).as_deref(), Some("v3.0.0"));
    }

    #[test]
    #[ignore = "#475"]
    fn skipped_version_stays_skipped_next_run_but_newer_is_announced() {
        let dir = TempDir::new().unwrap();
        let state = dir.path().join("state");
        let server = Server::start(Reply::Json(releases_json(&["v9.0.0"])));

        let mut app = app_at(dir.path());
        assert!(!app.skip_update(), "알릴 버전이 없으면 건너뛸 것도 없다");
        app.enable_updates(options(&server, &state, Channel::Stable));
        app.execute(CommandId::CheckForUpdates);
        wait_checked(&mut app);
        assert!(app.update_notice().is_some());
        assert!(app.skip_update());
        assert!(app.update_notice().is_none(), "건너뛰면 알림이 사라진다");
        drop(app);

        // 같은 상태 폴더로 다시 켠 앱(다음 실행).
        let mut app = app_at(dir.path());
        app.enable_updates(options(&server, &state, Channel::Stable));
        app.execute(CommandId::CheckForUpdates);
        wait_checked(&mut app);
        match app.update_check() {
            Some(Ok(UpdateDecision::Skipped(release))) => assert_eq!(release.tag, "v9.0.0"),
            other => panic!("건너뛴 버전이어야 한다: {other:?}"),
        }
        assert!(app.update_notice().is_none());

        server.set(Reply::Json(releases_json(&["v9.1.0", "v9.0.0"])));
        app.execute(CommandId::CheckForUpdates);
        wait_checked(&mut app);
        assert_eq!(app.update_notice().map(|r| r.tag.as_str()), Some("v9.1.0"));
    }

    #[test]
    #[ignore = "#476"]
    fn setting_an_interval_starts_checking_and_clearing_it_stops() {
        let dir = TempDir::new().unwrap();
        let server = Server::start(Reply::Json(releases_json(&["v0.1.0"])));
        let mut app = app_at(dir.path());
        let mut options = options(&server, &dir.path().join("state"), Channel::Stable);
        options.first_delay = Duration::from_millis(100);
        options.interval = None;
        app.enable_updates(options);
        assert!(pump_for(&mut app, &server, Duration::from_millis(500)).is_empty());

        app.set_update_interval(Some(Duration::from_millis(300)));
        assert!(pump_until_requests(&mut app, &server, 2).len() >= 2);

        app.set_update_interval(None);
        // 이미 떠난 요청이 있을 수 있어 잠깐 기다린 뒤부터 센다.
        pump_for(&mut app, &server, Duration::from_millis(400));
        assert!(
            pump_for(&mut app, &server, Duration::from_millis(1000)).is_empty(),
            "끄면 더 확인하지 않는다"
        );
    }
}
