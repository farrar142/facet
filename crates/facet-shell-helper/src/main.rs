//! 셸 확장 도우미 실행 파일(spec #24, ADR-0006): 셸 확장을 UI 프로세스 대신 싣는다.
//!
//! 명령 줄: `facet-shell-helper --pipe <파이프 이름> [--parent <프로세스 번호>] [--idle-ms <밀리초>] [--fake]
//! [--fake-version <n>] [--crash-state <파일>]`. UI 프로세스(`facet_core::shell_helper`)가 만든 사적인 파이프에 붙어 버전
//! 인사에 답한 뒤, 요청을 받은 차례대로 하나씩 처리한다(셸의 STA 스레드처럼 한 번에 하나). 파이프가 끊기면(UI가 끝나거나
//! 도우미를 버림) 끝난다. `--idle-ms`가 있으면 마지막 요청(또는 대화상자가 닫힌 때) 뒤 그만큼 요청이 없으면 스스로
//! 끝난다. 대화상자(속성·「연결 프로그램」)가 떠 있는 동안은 끝나지 않는다.
//!
//! 속성·「연결 프로그램」 요청은 Windows 대화상자를 띄우고(`facet_win::shell_dialog`) 곧바로 완료로 답한다. 실제 셸 메뉴는
//! 아직 부르지 않는다(뒤 티켓): 시험용 모드(`--fake`)가 아니면 메뉴·실행 요청에 오류로 답한다.
//!
//! # 시험용 모드(`--fake`)
//!
//! 셸을 부르지 않고 정해진 대로 답한다(#319의 약속). 메뉴 요청은 항목 대상이면 첫 대상의 파일 이름으로 동작을 고른다:
//! `late-<ms>`면 그 밀리초 뒤에 답하고, `hang`이면 답하지 않고 붙잡혀 있고, `crash`면 가짜 확장을 「마지막으로 부른
//! 확장」으로 적어 두고(`--crash-state`가 있을 때 그 파일에) 곧바로 끝난다. 그 밖에는 시험용 트리로 답한다. 항목
//! 실행은 첫 대상이 든 폴더(폴더 배경이면 그 폴더)에 `facet-fake-invoked.txt`를 만든다. `--fake-version <n>`이면 버전
//! 인사에서 자기 버전을 `n`으로 말한다(`n`만 받는다).
//!
//! 속성 요청은 첫 경로가 든 폴더에 `facet-fake-properties.txt`를 만들어 줄마다 경로들의 이름을 쓴 뒤 완료로 답하고,
//! 「대화상자」가 1.5초 떠 있는 것으로 친다(그동안 쉬면 끝내기를 미룸). 「연결 프로그램」 요청은 그 파일이 든 폴더에
//! `facet-fake-open-with.txt`를 만들어 파일 이름을 쓴 뒤 완료로 답한다(#321의 약속).

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
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::thread;
    use std::time::{Duration, Instant};

    use facet_shell_protocol::{
        ErrorKind, Icon, MenuItem, MenuTarget, Request, Response, ShellCommand, ShellSubmenu,
        answer_hello, read_message, write_message,
    };
    use facet_win::{pipe_client, shell_dialog};

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
    /// 속성 요청이 만드는 표지 파일.
    const PROPERTIES_FILE: &str = "facet-fake-properties.txt";
    /// 「연결 프로그램」 요청이 만드는 표지 파일.
    const OPEN_WITH_FILE: &str = "facet-fake-open-with.txt";
    /// 시험용 모드의 속성 「대화상자」가 떠 있는 시간.
    const FAKE_DIALOG: Duration = Duration::from_millis(1500);
    /// 대화상자가 떠 있는 동안 닫혔는지 다시 보는 간격.
    const DIALOG_POLL: Duration = Duration::from_millis(100);

    struct Args {
        pipe: OsString,
        idle: Option<Duration>,
        fake: bool,
        fake_version: Option<u16>,
        crash_state: Option<PathBuf>,
    }

    /// 요청 사이의 상태: 시험용 모드의 「대화상자」가 닫히는 때.
    #[derive(Default)]
    struct State {
        fake_dialog_until: Option<Instant>,
    }

    impl State {
        /// 대화상자가 떠 있는가.
        fn dialog_open(&self) -> bool {
            shell_dialog::dialogs_open()
                || self.fake_dialog_until.is_some_and(|t| Instant::now() < t)
        }
    }

    fn parse(args: Vec<OsString>) -> Option<Args> {
        let mut pipe = None;
        let mut idle = None;
        let mut fake = false;
        let mut fake_version = None;
        let mut crash_state = None;
        let mut it = args.into_iter();
        while let Some(arg) = it.next() {
            match arg.to_str() {
                Some("--pipe") => pipe = Some(it.next()?),
                // 부모 감시는 「도우미 수명」 티켓(#320)이 한다. 지금은 파이프가 끊기면 끝난다.
                Some("--parent") => {
                    it.next()?;
                }
                Some("--idle-ms") => {
                    idle = Some(Duration::from_millis(it.next()?.to_str()?.parse().ok()?));
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
            idle,
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
        let Ok(client) = pipe_client::connect(pipe, PIPE_ACCESS, CONNECT_WAIT) else {
            return 3;
        };
        let Ok((mut reader, mut writer, _closer)) = client.split() else {
            return 3;
        };
        // 요청은 읽는 스레드가 받아 넘긴다: 이 스레드가 쉬면 끝내기 시간을 재며 기다릴 수 있게(읽기에 마감을 걸면 프레임
        // 중간에서 끊길 수 있다).
        let (requests, inbox) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("facet-shell-helper-reader".into())
            .spawn(move || {
                while let Ok(request) = read_message::<Request>(&mut reader) {
                    if requests.send(request).is_err() {
                        return;
                    }
                }
            });
        if spawned.is_err() {
            return 3;
        }
        let mut state = State::default();
        // 쉬기 시작한 때: 마지막 요청을 받은 때. 대화상자가 떠 있는 동안은 계속 지금으로 미룬다.
        let mut idle_since = Instant::now();
        loop {
            let request = match args.idle {
                None => match inbox.recv() {
                    Ok(request) => request,
                    Err(_) => return 0, // 끊김: UI가 끝났거나 도우미를 버렸다.
                },
                Some(idle) => {
                    let wait = if state.dialog_open() {
                        idle_since = Instant::now();
                        DIALOG_POLL
                    } else {
                        let left = (idle_since + idle).saturating_duration_since(Instant::now());
                        if left.is_zero() {
                            return 0; // 쉬면 끝내기.
                        }
                        left
                    };
                    match inbox.recv_timeout(wait) {
                        Ok(request) => request,
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => return 0,
                    }
                }
            };
            let response = handle(&args, &mut state, request);
            idle_since = Instant::now();
            let Some(response) = response else {
                continue;
            };
            if write_message(&mut writer, &response).is_err() {
                return 0;
            }
        }
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
    fn handle(args: &Args, state: &mut State, request: Request) -> Option<Response> {
        match request {
            Request::Hello { version } => Some(hello(args, version)),
            Request::Properties { id, paths } if args.fake => {
                let response = fake_properties(id, &paths);
                if matches!(response, Response::Done { .. }) {
                    state.fake_dialog_until = Some(Instant::now() + FAKE_DIALOG);
                }
                Some(response)
            }
            Request::OpenWith { id, path } if args.fake => Some(fake_open_with(id, &path)),
            Request::Properties { id, paths } => Some(dialog(id, shell_dialog::properties(paths))),
            Request::OpenWith { id, path } => Some(dialog(id, shell_dialog::open_with(path))),
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

    /// 대화상자를 띄운 결과의 답.
    fn dialog(id: u64, shown: Result<(), shell_dialog::DialogError>) -> Response {
        match shown {
            Ok(()) => Response::Done { id },
            Err(shell_dialog::DialogError::NoTarget) => Response::Error {
                id,
                kind: ErrorKind::NotFound,
                message: "no target".into(),
            },
            Err(e) => Response::Error {
                id,
                kind: ErrorKind::Failed,
                message: format!("{e:?}"),
            },
        }
    }

    /// 표지 파일 `folder`/`file`에 줄마다 `names`를 쓰고 완료로 답한다.
    fn write_marker(id: u64, folder: Option<&Path>, file: &str, names: &[String]) -> Response {
        let Some(folder) = folder else {
            return Response::Error {
                id,
                kind: ErrorKind::NotFound,
                message: "no target".into(),
            };
        };
        let mut text = String::new();
        for name in names {
            text.push_str(name);
            text.push('\n');
        }
        match fs::write(folder.join(file), text) {
            Ok(()) => Response::Done { id },
            Err(e) => Response::Error {
                id,
                kind: ErrorKind::Failed,
                message: e.to_string(),
            },
        }
    }

    fn fake_properties(id: u64, paths: &[PathBuf]) -> Response {
        let names: Vec<String> = paths.iter().map(|p| file_name(p)).collect();
        write_marker(
            id,
            paths.first().and_then(|p| p.parent()),
            PROPERTIES_FILE,
            &names,
        )
    }

    fn fake_open_with(id: u64, path: &Path) -> Response {
        write_marker(id, path.parent(), OPEN_WITH_FILE, &[file_name(path)])
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
