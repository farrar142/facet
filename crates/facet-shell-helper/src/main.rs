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
//!
//! # 실제 셸 메뉴(#323)
//!
//! 시험용 모드가 아니면 메뉴 요청은 COM STA 셸 스레드([`facet_win::shell_thread::ShellThread`])에서 대상의 셸 메뉴
//! ([`facet_win::shell_menu::ShellMenu`])를 만들어 항목 트리로 답하고, 그 메뉴 객체를 그 스레드에 들고 있는다(다음 메뉴
//! 요청이 오면 놓는다). 실행 요청은 들고 있던 메뉴(대상이 다르면 새로 만든 메뉴)로 그 항목을 실행한다: 셸 스레드를 그
//! 실행에 넘기고(실행이 모달 창을 띄워도 다음 요청은 새 셸 스레드가 받음) [`INVOKE_ANSWER_WAIT`] 안에 실패하면 실패로,
//! 아니면 완료로 답한다. 실행이 끝나고 그것이 띄운 창이 닫히면 메뉴 객체를 놓고 그 스레드는 끝난다. 대상이 없거나
//! 권한이 없으면 죽지 않고 실패로 답한다. `--crash-state`가 있으면 처리되지 않은 예외로 죽을 때 그 주소가 든 확장 DLL을
//! 적는다([`facet_win::shell_menu::record_crashes`]).
//!
//! # 실제 미리보기(#343)
//!
//! 미리보기 요청은 메뉴와 따로 둔 미리보기 전용 COM STA 셸 스레드에서 그 파일의 셸 미리보기 처리기를 싣고
//! ([`facet_win::preview_handler::PreviewHandler`], 초기화는 스트림을 먼저) 처리기 창을 UI 창(요청의 `host.parent`)의
//! 자식 창으로 그 자리에 붙인다. 처리기가 없으면 「처리기 없음」, 싣거나 그리지 못하면 실패로 답한다. 자리 바꾸기는 그
//! 창을 옮기고, 내리기·다음 미리보기 요청은 처리기를 내려(`Unload`) 잡고 있던 파일을 놓는다. 그 셸 스레드가 요청 사이에
//! 메시지를 돌려 처리기 창이 그려진다. 미리보기가 떠 있는 동안은 쉬어도 끝나지 않는다(끝나면 붙인 창이 사라진다).
//! 자리 바꾸기·내리기가 [`PREVIEW_CALL_TIMEOUT`] 안에 끝나지 않으면(처리기가 멈춤) 완료로 답한 뒤 스스로 끝난다: 멈춘
//! 처리기의 창과 잡힌 파일은 프로세스가 끝나야 놓이기 때문이다.
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
//! 미리보기 요청(#342)은 파일 이름의 줄기(마지막 점 앞)로 고른다: `hang`이면 답하지 않고 붙잡혀 있고, `crash`면
//! 곧바로 죽고, `late-<ms>`로 시작하면 그 밀리초 뒤에 「보이기 시작함」, `nohandler`면 「처리기 없음」, `lock-`으로
//! 시작하면 그 파일을 공유 없이 열어 둔 채 「보이기 시작함」으로 답하고 미리보기를 내리거나 다음 미리보기 요청이 올 때
//! 닫는다. 그 밖에는 곧바로 「보이기 시작함」. 자리 바꾸기·내리기는 완료로 답한다.
//!
//! 시험용 모드가 아니면 실제 처리기를 싣는다(#343, 아래 「실제 미리보기」).
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
mod real_menu;
#[cfg(windows)]
mod real_preview;

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
    use facet_win::shell_thread::{CallError, ShellThread};

    use super::{real_menu, real_preview};

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
        if !args.fake
            && let Some(state) = &args.crash_state
        {
            facet_win::shell_menu::record_crashes(state);
        }
        let shell = Shell::new();
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
        let mut preview = Preview::default();
        loop {
            let Ok(request) = read_message::<Request>(&mut reader) else {
                return 0; // 끊김: UI가 끝났거나 도우미를 버렸다.
            };
            activity.begin();
            let response = handle(&args, &shell, &activity, &mut preview, request);
            let written = response
                .as_ref()
                .is_none_or(|response| write_message(&mut writer, response).is_ok());
            activity.end();
            if !written || preview.exit_after_reply {
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

    /// 떠 있는 미리보기(#342). 시험용 모드에서는 `lock-` 파일을 공유 없이 열어 둔 것이다.
    #[derive(Default)]
    struct Preview {
        held: Option<fs::File>,
        /// 실제 처리기가 떠 있는 동안 쉬어도 끝나지 않게 잡는 표지(#343).
        shown: Option<DialogOpen>,
        /// 답을 쓴 뒤 스스로 끝낸다(멈춘 처리기를 놓으려고, #343).
        exit_after_reply: bool,
    }

    /// 요청 하나에 답한다. `None`이면 답하지 않는다.
    fn handle(
        args: &Args,
        shell: &Shell,
        activity: &Arc<Activity>,
        preview: &mut Preview,
        request: Request,
    ) -> Option<Response> {
        match request {
            Request::Preview { id, path, host } => {
                // 앞선 미리보기를 내린다.
                preview.held = None;
                if args.fake {
                    fake_preview(args, preview, id, &path)
                } else {
                    Some(shell.preview(activity, preview, id, path, host))
                }
            }
            Request::MovePreview { id, host } => {
                if !args.fake {
                    shell.move_preview(preview, host);
                }
                Some(Response::Done { id })
            }
            Request::UnloadPreview { id } => {
                preview.held = None;
                if !args.fake {
                    shell.unload_preview(preview);
                }
                Some(Response::Done { id })
            }
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
            Request::Menu { id, target } => Some(shell.menu(id, target)),
            Request::Execute {
                id,
                target,
                command,
            } => Some(shell.execute(activity, id, target, command)),
            Request::DropMenu { id, .. } => Some(Response::Error {
                id,
                kind: ErrorKind::Failed,
                message: "not supported yet".into(),
            }),
        }
    }

    /// 셸 메뉴를 만들 때 기다리는 가장 긴 시간.
    ///
    /// 근거: 보통 수십~수백 ms지만 처음 셸을 초기화하고 확장을 처음 실을 때(느린 CI 러너 포함) 몇 초 걸릴 수 있다. 정말로
    /// 멈춘 확장은 관리 쪽의 멈춤 판정(`STALL_TIMEOUT`, 10초)이 이 도우미를 끝내므로 이 값은 그보다 넉넉한 안전망이다.
    const BUILD_TIMEOUT: Duration = Duration::from_secs(60);
    /// 셸 항목 실행이 실패를 알리기를 기다리는 시간. 넘으면(모달 창을 띄우는 등 아직 실행 중) 완료로 답한다.
    ///
    /// 근거: 관리 쪽 응답 시간 제한(`REPLY_TIMEOUT`, 2초)보다 짧아야 하고, 곧바로 실패하는 셸 항목(없는 대상 등)은 수십 ms
    /// 안에 돌아온다.
    pub(super) const INVOKE_ANSWER_WAIT: Duration = Duration::from_secs(1);

    /// 처리기를 싣고 그리기 시작하기를 기다리는 가장 긴 시간(#343).
    ///
    /// 근거: 관리 쪽이 미리보기 요청을 3초(`SHELL_PREVIEW_TIMEOUT`)에 끊고 이 도우미를 끝내므로 실제로는 그쪽이 먼저다. 이
    /// 값은 관리 쪽이 없을 때(시험) 붙잡혀 있지 않게 하는 안전망이다.
    const PREVIEW_LOAD_TIMEOUT: Duration = Duration::from_secs(10);
    /// 자리 바꾸기·내리기를 기다리는 가장 긴 시간(#343). 넘으면 처리기가 멈춘 것으로 보고 답한 뒤 끝난다.
    ///
    /// 근거: 창 옮기기와 `Unload`는 보통 수 ms다. 관리 쪽 응답 시간 제한(`REPLY_TIMEOUT`, 2초) 안에 답해야 한다.
    pub(super) const PREVIEW_CALL_TIMEOUT: Duration = Duration::from_millis(1500);

    /// 실제 셸 메뉴를 다루는 셸 스레드(COM STA). 만든 메뉴 객체는 그 스레드에 있다([`real_menu`]). 미리보기 처리기는
    /// 따로 둔 셸 스레드에 있다([`real_preview`], 처리기가 멈춰도 메뉴는 돈다).
    struct Shell {
        thread: std::sync::Mutex<ShellThread>,
        preview: ShellThread,
    }

    impl Shell {
        fn new() -> Shell {
            Shell {
                thread: std::sync::Mutex::new(ShellThread::new()),
                preview: ShellThread::new(),
            }
        }

        /// 실제 미리보기(#343): 앞선 처리기를 내리고 `path`의 처리기를 `host`에 붙인다.
        fn preview(
            &self,
            activity: &Arc<Activity>,
            preview: &mut Preview,
            id: u64,
            path: PathBuf,
            host: facet_shell_protocol::PreviewHost,
        ) -> Response {
            use facet_win::preview_handler::LoadError;
            preview.shown = None;
            match self.preview.call(PREVIEW_LOAD_TIMEOUT, move || {
                real_preview::show(&path, host)
            }) {
                Ok(Ok(())) => {
                    preview.shown = Some(activity.dialog());
                    Response::PreviewShowing { id }
                }
                Ok(Err(LoadError::NoHandler)) => Response::NoPreviewHandler { id },
                Ok(Err(LoadError::Failed(message))) => Response::Error {
                    id,
                    kind: ErrorKind::Failed,
                    message,
                },
                Err(error) => call_failed(id, error),
            }
        }

        fn move_preview(&self, preview: &mut Preview, host: facet_shell_protocol::PreviewHost) {
            if preview.shown.is_none() {
                return;
            }
            let moved = self
                .preview
                .call(PREVIEW_CALL_TIMEOUT, move || real_preview::move_to(host));
            if moved.is_err() {
                preview.exit_after_reply = true;
            }
        }

        fn unload_preview(&self, preview: &mut Preview) {
            if preview.shown.take().is_none() {
                return;
            }
            if self
                .preview
                .call(PREVIEW_CALL_TIMEOUT, real_preview::unload)
                .is_err()
            {
                preview.exit_after_reply = true;
            }
        }

        fn menu(&self, id: u64, target: MenuTarget) -> Response {
            let thread = self.thread.lock().unwrap_or_else(|p| p.into_inner());
            match thread.call(BUILD_TIMEOUT, move || real_menu::build(&target)) {
                Ok(Ok(items)) => Response::Menu { id, items },
                Ok(Err(error)) => menu_error(id, error),
                Err(error) => call_failed(id, error),
            }
        }

        fn execute(
            &self,
            activity: &Arc<Activity>,
            id: u64,
            target: MenuTarget,
            command: u32,
        ) -> Response {
            // 실행이 띄운 창이 떠 있는 동안은 쉬어도 끝나지 않는다.
            let open = activity.dialog();
            let (answer, answered) = std::sync::mpsc::channel();
            let job = move || {
                let _open = open;
                real_menu::invoke(&target, command, &answer);
            };
            {
                let mut thread = self.thread.lock().unwrap_or_else(|p| p.into_inner());
                // 이 셸 스레드(들고 있던 메뉴 객체가 있는 곳)를 실행에 넘긴다: 기다리지 않고 맡긴 뒤 버린다(일이
                // 끝나면 스레드도 끝남). 다음 요청은 새 셸 스레드가 받는다.
                let _ = thread.call(Duration::ZERO, job);
                *thread = ShellThread::new();
            }
            match answered.recv_timeout(INVOKE_ANSWER_WAIT) {
                Ok(Ok(())) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    Response::Done { id }
                }
                Ok(Err(error)) => menu_error(id, error),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    call_failed(id, CallError::Failed)
                }
            }
        }
    }

    fn menu_error(id: u64, error: facet_win::shell_menu::MenuError) -> Response {
        use facet_win::shell_menu::MenuError;
        let kind = match &error {
            MenuError::NotFound(_) => ErrorKind::NotFound,
            MenuError::UnknownCommand => ErrorKind::UnknownCommand,
            MenuError::Failed(_) => ErrorKind::Failed,
        };
        Response::Error {
            id,
            kind,
            message: error.to_string(),
        }
    }

    fn call_failed(id: u64, error: CallError) -> Response {
        let message = match error {
            CallError::TimedOut => "shell timed out",
            CallError::Failed => "shell call failed",
        };
        Response::Error {
            id,
            kind: ErrorKind::Failed,
            message: message.into(),
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

    /// 시험용 미리보기: 파일 이름의 줄기로 동작을 고른다(#342의 약속).
    fn fake_preview(args: &Args, preview: &mut Preview, id: u64, path: &Path) -> Option<Response> {
        let name = file_name(path);
        let stem = name
            .rsplit_once('.')
            .map_or(name.as_str(), |(stem, _)| stem);
        if stem == "hang" {
            loop {
                thread::sleep(Duration::from_secs(3600));
            }
        }
        if stem == "crash" {
            crash(args);
        }
        if stem == "nohandler" {
            return Some(Response::NoPreviewHandler { id });
        }
        if let Some(ms) = stem
            .strip_prefix("late-")
            .and_then(|ms| ms.parse::<u64>().ok())
        {
            thread::sleep(Duration::from_millis(ms));
        }
        if stem.starts_with("lock-") {
            use std::os::windows::fs::OpenOptionsExt;
            match fs::OpenOptions::new().read(true).share_mode(0).open(path) {
                Ok(file) => preview.held = Some(file),
                Err(e) => {
                    return Some(Response::Error {
                        id,
                        kind: ErrorKind::Failed,
                        message: e.to_string(),
                    });
                }
            }
        }
        Some(Response::PreviewShowing { id })
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
