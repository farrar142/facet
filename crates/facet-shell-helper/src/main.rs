//! 셸 확장 도우미 실행 파일(spec #24, ADR-0006): 셸 확장을 UI 프로세스 대신 싣는다.
//!
//! 명령 줄: `facet-shell-helper --pipe <파이프 이름> [--parent <프로세스 번호>] [--idle-timeout-ms <ms>] [--fake]
//! [--fake-version <n>] [--crash-state <파일>]`. UI 프로세스(`facet_core::shell_helper`)가 만든 사적인 파이프에 붙어 버전
//! 인사에 답한 뒤, 요청을 받은 차례대로 하나씩 처리한다(셸의 STA 스레드처럼 한 번에 하나). 파이프가 끊기면(UI가
//! 끝나거나 도우미를 버림) 끝난다.
//!
//! # 수명(#320)
//!
//! - **부모 감시**: `--parent`의 프로세스(UI)가 끝나면 곧바로 끝난다(감시 스레드).
//! - **쉬면 끝내기**: 요청을 처리하고 있지 않은 채 `--idle-timeout-ms` 동안 요청이 없으면 스스로 끝난다(종료 코드 0,
//!   관리 쪽은 이것을 죽음으로 세지 않는다). 없으면 쉬어도 끝나지 않는다.
//! - **마지막으로 부른 확장**: `--crash-state <파일>`은 관리 쪽과 함께 보는 공유 상태다. 확장을 부르기 직전에 그
//!   확장의 CLSID·DLL 경로·이름을 한 줄씩 적어 두면, 도우미가 죽었을 때 관리 쪽이 읽어 기록한다.
//!
//! 실제 셸 메뉴는 아직 부르지 않는다(뒤 티켓): 시험용 모드(`--fake`)가 아니면 메뉴·실행 요청에 오류로 답한다.
//!
//! # 속성·연결 프로그램(#321)
//!
//! 속성 요청은 Windows 속성 대화상자를, 연결 프로그램 요청은 「연결 프로그램」 대화상자를 대화상자마다 띄운 스레드에서
//! 띄우고([`facet_win::shell_dialog`]) 곧바로 완료로 답한다(대화상자가 떠 있는 동안 요청을 붙잡지 않아 관리 쪽의 멈춤
//! 판정에 걸리지 않는다). 대화상자가 떠 있는 동안은 쉬면 끝내기를 미루고, 닫힌 때부터 쉬는 시간을 다시 센다.
//!
//! # 시험용 모드(`--fake`)
//!
//! 셸을 부르지 않고 정해진 대로 답한다(#319의 약속). 메뉴 요청은 항목 대상이면 첫 대상의 파일 이름으로 동작을 고른다:
//! `late-<ms>`면 그 밀리초 뒤에 답하고, `hang`이면 답하지 않고 붙잡혀 있고, `crash`면 가짜 확장을 「마지막으로 부른
//! 확장」으로 적어 두고(`--crash-state`가 있을 때 그 파일에) 곧바로 끝난다. 그 밖에는 시험용 트리로 답한다. 항목
//! 실행은 첫 대상이 든 폴더(폴더 배경이면 그 폴더)에 `facet-fake-invoked.txt`를 만든다. `--fake-version <n>`이면 버전
//! 인사에서 자기 버전을 `n`으로 말한다(`n`만 받는다).
//!
//! 속성 요청은 첫 경로가 든 폴더에 `facet-fake-properties.txt`를 만들어 줄마다 경로들의 이름을 쓰고 완료로 답하며,
//! 「대화상자」가 1.5초 동안 떠 있는 것으로 친다(그동안 쉬면 끝내기를 미룸). 연결 프로그램 요청은 그 파일이 든 폴더에
//! `facet-fake-open-with.txt`를 만들어 파일 이름을 쓰고 완료로 답한다.

fn main() {
    #[cfg(windows)]
    {
        std::process::exit(helper::run(std::env::args_os().skip(1).collect()));
    }
    #[cfg(not(windows))]
    {
        eprintln!("facet-shell-helper는 Windows에서만 돕니다");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod helper {
    use std::ffi::OsString;
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    use facet_shell_protocol::{
        ErrorKind, Icon, MenuItem, MenuTarget, Request, Response, ShellCommand, ShellSubmenu,
        answer_hello, read_message, write_message,
    };
    use facet_win::pipe_client;

    /// 파이프에 청하는 권한: 읽기(`FILE_GENERIC_READ`)와 데이터 쓰기(`FILE_WRITE_DATA`).
    const PIPE_ACCESS: u32 = 0x0012_0089 | 0x0002;
    /// 파이프가 바쁠 때 기다리는 시간(인스턴스는 하나뿐이고 이 도우미를 위해 만든 것이라 보통 곧바로 붙는다).
    const CONNECT_WAIT: Duration = Duration::from_secs(5);

    /// 시험용 모드의 가짜 확장(「마지막으로 부른 확장」).
    const FAKE_CLSID: &str = "{FACE0000-0000-0000-0000-000000000001}";
    const FAKE_DLL: &str = r"C:\fake\fake-extension.dll";
    const FAKE_NAME: &str = "Fake Extension";
    /// 항목 실행이 만드는 표지 파일.
    const INVOKED_FILE: &str = "facet-fake-invoked.txt";
    /// 시험용 속성 요청이 만드는 표지 파일.
    const PROPERTIES_FILE: &str = "facet-fake-properties.txt";
    /// 시험용 연결 프로그램 요청이 만드는 표지 파일.
    const OPEN_WITH_FILE: &str = "facet-fake-open-with.txt";
    /// 시험용 속성 「대화상자」가 떠 있는 시간(#321의 약속).
    const FAKE_DIALOG_TIME: Duration = Duration::from_millis(1500);

    /// 쉬면 끝내기로 스스로 끝날 때의 종료 코드(관리 쪽은 깨끗한 끝으로 본다).
    const IDLE_EXIT_CODE: i32 = 0;
    /// 부모가 끝나 따라 끝날 때의 종료 코드.
    const PARENT_EXIT_CODE: i32 = 0;
    /// 쉬는 시간을 확인하는 가장 긴 간격.
    const IDLE_POLL_MAX: Duration = Duration::from_millis(500);

    struct Args {
        pipe: OsString,
        parent: Option<u32>,
        idle_timeout: Option<Duration>,
        fake: bool,
        fake_version: Option<u16>,
        crash_state: Option<PathBuf>,
    }

    fn parse(args: Vec<OsString>) -> Option<Args> {
        let mut pipe = None;
        let mut parent = None;
        let mut idle_timeout = None;
        let mut fake = false;
        let mut fake_version = None;
        let mut crash_state = None;
        let mut it = args.into_iter();
        while let Some(arg) = it.next() {
            match arg.to_str() {
                Some("--pipe") => pipe = Some(it.next()?),
                Some("--parent") => parent = Some(it.next()?.to_str()?.parse().ok()?),
                Some("--idle-timeout-ms") => {
                    let ms: u64 = it.next()?.to_str()?.parse().ok()?;
                    idle_timeout = Some(Duration::from_millis(ms));
                }
                Some("--fake") => fake = true,
                Some("--fake-version") => {
                    fake_version = Some(it.next()?.to_str()?.parse().ok()?);
                }
                Some("--crash-state") => crash_state = Some(PathBuf::from(it.next()?)),
                // 모르는 인자는 건너뛴다(뒤 티켓이 더하는 인자를 옛 도우미가 받아도 돌게).
                _ => {}
            }
        }
        Some(Args {
            pipe: pipe?,
            parent,
            idle_timeout,
            fake,
            fake_version,
            crash_state,
        })
    }

    pub fn run(args: Vec<OsString>) -> i32 {
        let Some(args) = parse(args) else {
            eprintln!("사용법: facet-shell-helper --pipe <파이프 이름> [--fake]");
            return 2;
        };
        let Some(pipe) = args.pipe.to_str() else {
            return 2;
        };
        if let Some(parent) = args.parent {
            watch_parent(parent);
        }
        let activity = Activity::new();
        if let Some(idle) = args.idle_timeout {
            watch_idle(Arc::clone(&activity), idle);
        }
        let Ok(client) = pipe_client::connect(pipe, PIPE_ACCESS, CONNECT_WAIT) else {
            return 3;
        };
        let Ok((mut reader, mut writer, _closer)) = client.split() else {
            return 3;
        };
        loop {
            let Ok(request) = read_message::<Request>(&mut reader) else {
                return 0; // 끊김: UI가 끝났거나 도우미를 버렸다.
            };
            activity.begin();
            let response = handle(&args, &activity, request);
            let written = response
                .as_ref()
                .is_none_or(|response| write_message(&mut writer, response).is_ok());
            activity.end();
            if !written {
                return 0;
            }
        }
    }

    /// 부모 프로세스가 끝나면 이 프로세스를 끝내는 감시 스레드를 띄운다. 부모가 이미 없으면 곧바로 끝난다.
    fn watch_parent(parent: u32) {
        let spawned = thread::Builder::new()
            .name("facet-shell-helper-parent".into())
            .spawn(move || {
                // 열지 못한 것(이미 끝남)도 끝난 것으로 본다.
                let _ = facet_win::process::wait_for_exit(parent);
                std::process::exit(PARENT_EXIT_CODE);
            });
        if spawned.is_err() {
            // 감시하지 못하면 부모 없이 남을 수 있으므로 돌지 않는다.
            std::process::exit(PARENT_EXIT_CODE);
        }
    }

    /// 요청 처리 중인지와 마지막으로 처리를 마친 때.
    struct Activity {
        started: Instant,
        busy: AtomicBool,
        /// 떠 있는 대화상자 수(#321). 하나라도 있으면 쉬지 않는다.
        dialogs: AtomicUsize,
        /// `started`부터 마지막 활동까지의 밀리초.
        last_ms: AtomicU64,
    }

    impl Activity {
        fn new() -> Arc<Activity> {
            Arc::new(Activity {
                started: Instant::now(),
                busy: AtomicBool::new(false),
                dialogs: AtomicUsize::new(0),
                last_ms: AtomicU64::new(0),
            })
        }

        fn now_ms(&self) -> u64 {
            u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
        }

        fn begin(&self) {
            self.busy.store(true, Ordering::SeqCst);
            self.last_ms.store(self.now_ms(), Ordering::SeqCst);
        }

        fn end(&self) {
            self.last_ms.store(self.now_ms(), Ordering::SeqCst);
            self.busy.store(false, Ordering::SeqCst);
        }

        /// 대화상자 하나가 떴다. 돌려받은 표지를 버리면 닫힌 것이고 그때부터 쉬는 시간을 다시 센다.
        fn dialog(self: &Arc<Self>) -> DialogOpen {
            self.dialogs.fetch_add(1, Ordering::SeqCst);
            DialogOpen(Arc::clone(self))
        }

        /// 처리 중이 아니고 떠 있는 대화상자도 없으며 마지막 활동 뒤 `idle`이 지났는가.
        fn idle_for(&self, idle: Duration) -> bool {
            if self.busy.load(Ordering::SeqCst) || self.dialogs.load(Ordering::SeqCst) > 0 {
                return false;
            }
            let since = self
                .now_ms()
                .saturating_sub(self.last_ms.load(Ordering::SeqCst));
            Duration::from_millis(since) >= idle
        }
    }

    /// 떠 있는 대화상자 하나(#321). 버리면 닫힌 것이다.
    struct DialogOpen(Arc<Activity>);

    impl Drop for DialogOpen {
        fn drop(&mut self) {
            self.0.last_ms.store(self.0.now_ms(), Ordering::SeqCst);
            self.0.dialogs.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// 쉬면 끝내기 감시 스레드를 띄운다.
    fn watch_idle(activity: Arc<Activity>, idle: Duration) {
        let poll = (idle / 4).clamp(Duration::from_millis(10), IDLE_POLL_MAX);
        let _ = thread::Builder::new()
            .name("facet-shell-helper-idle".into())
            .spawn(move || {
                loop {
                    thread::sleep(poll);
                    if activity.idle_for(idle) {
                        std::process::exit(IDLE_EXIT_CODE);
                    }
                }
            });
    }

    fn hello(args: &Args, version: u16) -> Response {
        match args.fake_version {
            Some(own) if version == own => Response::HelloAccepted { version },
            Some(own) => Response::HelloRejected {
                min_version: own,
                max_version: own,
            },
            None => answer_hello(version),
        }
    }

    /// 요청 하나에 답한다. `None`이면 답하지 않는다.
    fn handle(args: &Args, activity: &Arc<Activity>, request: Request) -> Option<Response> {
        match request {
            Request::Properties { id, paths } if args.fake => {
                Some(fake_properties(activity, id, &paths))
            }
            Request::OpenWith { id, path } if args.fake => Some(fake_open_with(id, &path)),
            Request::Properties { id, paths } => Some(dialog(activity, id, move || {
                facet_win::shell_dialog::show_properties(&paths)
            })),
            Request::OpenWith { id, path } => Some(dialog(activity, id, move || {
                facet_win::shell_dialog::show_open_with(&path)
            })),
            Request::Hello { version } => Some(hello(args, version)),
            Request::Menu { id, target } if args.fake => fake_menu(args, id, &target),
            Request::Execute {
                id,
                target,
                command,
            } if args.fake => Some(fake_execute(id, &target, command)),
            Request::Menu { id, .. }
            | Request::Execute { id, .. }
            | Request::DropMenu { id, .. } => Some(Response::Error {
                id,
                kind: ErrorKind::Failed,
                message: "not supported yet".into(),
            }),
        }
    }

    /// 대화상자를 따로 스레드에서 띄우고 곧바로 완료로 답한다. 스레드가 끝날 때(대화상자가 닫힐 때)까지 쉬지 않는다.
    fn dialog(
        activity: &Arc<Activity>,
        id: u64,
        show: impl FnOnce() -> std::io::Result<()> + Send + 'static,
    ) -> Response {
        let open = activity.dialog();
        let spawned = thread::Builder::new()
            .name("facet-shell-helper-dialog".into())
            .spawn(move || {
                let _open = open;
                let _ = show();
            });
        match spawned {
            Ok(_) => Response::Done { id },
            Err(e) => Response::Error {
                id,
                kind: ErrorKind::Failed,
                message: e.to_string(),
            },
        }
    }

    /// 시험용 속성: 첫 경로가 든 폴더에 표지 파일을 쓰고, 「대화상자」를 1.5초 띄운 것으로 친다.
    fn fake_properties(activity: &Arc<Activity>, id: u64, paths: &[PathBuf]) -> Response {
        let Some(folder) = paths.first().and_then(|p| p.parent()) else {
            return no_target(id);
        };
        let names: Vec<String> = paths.iter().map(|p| file_name(p)).collect();
        let response = write_marker(id, &folder.join(PROPERTIES_FILE), &names);
        if matches!(response, Response::Done { .. }) {
            let open = activity.dialog();
            let _ = thread::Builder::new()
                .name("facet-shell-helper-fake-dialog".into())
                .spawn(move || {
                    let _open = open;
                    thread::sleep(FAKE_DIALOG_TIME);
                });
        }
        response
    }

    /// 시험용 연결 프로그램: 그 파일이 든 폴더에 표지 파일을 쓴다.
    fn fake_open_with(id: u64, path: &Path) -> Response {
        let Some(folder) = path.parent() else {
            return no_target(id);
        };
        write_marker(id, &folder.join(OPEN_WITH_FILE), &[file_name(path)])
    }

    fn no_target(id: u64) -> Response {
        Response::Error {
            id,
            kind: ErrorKind::NotFound,
            message: "no target".into(),
        }
    }

    /// 표지 파일에 줄마다 이름을 쓴다.
    fn write_marker(id: u64, file: &Path, names: &[String]) -> Response {
        let mut text = String::new();
        for name in names {
            text.push_str(name);
            text.push('\n');
        }
        match fs::write(file, text) {
            Ok(()) => Response::Done { id },
            Err(e) => Response::Error {
                id,
                kind: ErrorKind::Failed,
                message: e.to_string(),
            },
        }
    }

    fn file_name(path: &Path) -> String {
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned())
    }

    fn fake_menu(args: &Args, id: u64, target: &MenuTarget) -> Option<Response> {
        let first_text = match target {
            MenuTarget::Items(paths) => {
                let action = paths.first().map(|p| file_name(p)).unwrap_or_default();
                if action == "hang" {
                    loop {
                        thread::sleep(Duration::from_secs(3600));
                    }
                }
                if action == "crash" {
                    crash(args);
                }
                if let Some(ms) = action
                    .strip_prefix("late-")
                    .and_then(|ms| ms.parse::<u64>().ok())
                {
                    thread::sleep(Duration::from_millis(ms));
                }
                let names: Vec<String> = paths.iter().map(|p| file_name(p)).collect();
                format!("fake: {}", names.join("|"))
            }
            MenuTarget::Background(folder) => format!("fake-background: {}", file_name(folder)),
        };
        Some(Response::Menu {
            id,
            items: fake_tree(first_text),
        })
    }

    fn fake_tree(first_text: String) -> Vec<MenuItem> {
        vec![
            MenuItem::Command(ShellCommand {
                id: 1,
                text: first_text,
                icon: Some(fake_icon()),
                enabled: true,
            }),
            MenuItem::Separator,
            MenuItem::Submenu(ShellSubmenu {
                text: "fake-submenu".into(),
                icon: None,
                enabled: true,
                items: vec![MenuItem::Command(ShellCommand {
                    id: 2,
                    text: "fake-child".into(),
                    icon: None,
                    enabled: true,
                })],
            }),
            MenuItem::Command(ShellCommand {
                id: 3,
                text: "fake-disabled".into(),
                icon: None,
                enabled: false,
            }),
        ]
    }

    /// 16×16 한 색 그림.
    fn fake_icon() -> Icon {
        Icon {
            width: 16,
            height: 16,
            rgba: [0x30, 0x80, 0xE0, 0xFF].repeat(16 * 16),
        }
    }

    /// 가짜 확장을 「마지막으로 부른 확장」으로 적고 곧바로 끝난다.
    fn crash(args: &Args) -> ! {
        if let Some(path) = &args.crash_state {
            let _ = fs::write(path, format!("{FAKE_CLSID}\n{FAKE_DLL}\n{FAKE_NAME}\n"));
        }
        // 정리 없이 곧바로 끝난다(오류 보고 창을 띄우지 않도록 abort가 아니라 종료 코드로).
        std::process::exit(101);
    }

    fn fake_execute(id: u64, target: &MenuTarget, command: u32) -> Response {
        let (folder, names) = match target {
            MenuTarget::Items(paths) => (
                paths
                    .first()
                    .and_then(|p| p.parent())
                    .map(Path::to_path_buf),
                paths.iter().map(|p| file_name(p)).collect::<Vec<_>>(),
            ),
            MenuTarget::Background(folder) => (Some(folder.clone()), vec![file_name(folder)]),
        };
        let Some(folder) = folder else {
            return Response::Error {
                id,
                kind: ErrorKind::NotFound,
                message: "no target".into(),
            };
        };
        let mut text = format!("{command}\n");
        for name in names {
            text.push_str(&name);
            text.push('\n');
        }
        let written = fs::File::create(folder.join(INVOKED_FILE))
            .and_then(|mut file| file.write_all(text.as_bytes()));
        match written {
            Ok(()) => Response::Done { id },
            Err(e) => Response::Error {
                id,
                kind: ErrorKind::Failed,
                message: e.to_string(),
            },
        }
    }
}
