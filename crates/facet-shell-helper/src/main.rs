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
//! # 시험용 모드(`--fake`)
//!
//! 셸을 부르지 않고 정해진 대로 답한다(#319의 약속). 메뉴 요청은 항목 대상이면 첫 대상의 파일 이름으로 동작을 고른다:
//! `late-<ms>`면 그 밀리초 뒤에 답하고, `hang`이면 답하지 않고 붙잡혀 있고, `crash`면 가짜 확장을 「마지막으로 부른
//! 확장」으로 적어 두고(`--crash-state`가 있을 때 그 파일에) 곧바로 끝난다. 그 밖에는 시험용 트리로 답한다. 항목
//! 실행은 첫 대상이 든 폴더(폴더 배경이면 그 폴더)에 `facet-fake-invoked.txt`를 만든다. `--fake-version <n>`이면 버전
//! 인사에서 자기 버전을 `n`으로 말한다(`n`만 받는다).

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
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
            let response = handle(&args, request);
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
        /// `started`부터 마지막 활동까지의 밀리초.
        last_ms: AtomicU64,
    }

    impl Activity {
        fn new() -> Arc<Activity> {
            Arc::new(Activity {
                started: Instant::now(),
                busy: AtomicBool::new(false),
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

        /// 처리 중이 아니고 마지막 활동 뒤 `idle`이 지났는가.
        fn idle_for(&self, idle: Duration) -> bool {
            if self.busy.load(Ordering::SeqCst) {
                return false;
            }
            let since = self
                .now_ms()
                .saturating_sub(self.last_ms.load(Ordering::SeqCst));
            Duration::from_millis(since) >= idle
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
    fn handle(args: &Args, request: Request) -> Option<Response> {
        match request {
            Request::Hello { version } => Some(hello(args, version)),
            Request::Menu { id, target } if args.fake => fake_menu(args, id, &target),
            Request::Execute {
                id,
                target,
                command,
            } if args.fake => Some(fake_execute(id, &target, command)),
            Request::Menu { id, .. }
            | Request::Execute { id, .. }
            | Request::DropMenu { id, .. }
            | Request::Properties { id, .. }
            | Request::OpenWith { id, .. } => Some(Response::Error {
                id,
                kind: ErrorKind::Failed,
                message: "not supported yet".into(),
            }),
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
