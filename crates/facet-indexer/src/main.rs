//! 인덱서 서비스 실행 파일. 서비스 본체(`service`), 개발용 설치·제거(`install`·`uninstall`),
//! 개발용 하위 명령 `dump-mft`와 `serve-sample`이 있다.

use std::env;
use std::process::ExitCode;

use facet_indexer::cli::{self, Command, DumpArgs, ServeArgs};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match cli::parse(&args) {
        Ok(Command::Help) => {
            println!("{}", cli::USAGE);
            ExitCode::SUCCESS
        }
        Ok(Command::Install) => run_install(),
        Ok(Command::Uninstall) => run_uninstall(),
        Ok(Command::Service) => run_service(),
        Ok(Command::DumpMft(dump)) => run_dump(&dump),
        Ok(Command::ServeSample(serve)) => run_serve(&serve),
        Err(e) => {
            eprintln!("{e}\n\n{}", cli::USAGE);
            ExitCode::FAILURE
        }
    }
}

#[cfg(windows)]
fn run_install() -> ExitCode {
    use facet_indexer::SERVICE_NAME;

    let exe = match env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("install: 이 실행 파일의 경로를 알지 못함: {e}");
            return ExitCode::FAILURE;
        }
    };
    match facet_indexer::service::install(&exe) {
        Ok(()) => {
            println!(
                "서비스 {SERVICE_NAME}를 설치하고 시작했습니다({}). 인덱스는 뒤에서 만듭니다. \
                 facet-query status로 상태를 봅니다.",
                exe.display()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("install: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(windows)]
fn run_uninstall() -> ExitCode {
    use facet_indexer::SERVICE_NAME;
    use facet_indexer::service::Uninstalled;

    match facet_indexer::service::uninstall() {
        Ok(Uninstalled::Removed) => {
            println!("서비스 {SERVICE_NAME}를 멈추고 지웠습니다. 인덱스 폴더도 지웠습니다.");
            ExitCode::SUCCESS
        }
        Ok(Uninstalled::NotInstalled) => {
            println!(
                "서비스 {SERVICE_NAME}는 설치돼 있지 않습니다. 남은 인덱스 폴더가 있으면 지웠습니다."
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("uninstall: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(windows)]
fn run_service() -> ExitCode {
    match facet_indexer::service::run_service() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("service: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn run_install() -> ExitCode {
    eprintln!("install: Windows에서만 동작합니다(Windows 서비스)");
    ExitCode::FAILURE
}

#[cfg(not(windows))]
fn run_uninstall() -> ExitCode {
    eprintln!("uninstall: Windows에서만 동작합니다(Windows 서비스)");
    ExitCode::FAILURE
}

#[cfg(not(windows))]
fn run_service() -> ExitCode {
    eprintln!("service: Windows에서만 동작합니다(Windows 서비스)");
    ExitCode::FAILURE
}

#[cfg(windows)]
fn run_dump(args: &DumpArgs) -> ExitCode {
    match facet_indexer::volume::dump_to_file(args) {
        Ok(s) => {
            println!(
                "{}: MFT 레코드 {}..{} ({}개, 레코드당 {}바이트, MFT 전체 {}개)를 {}에 떠 냄",
                args.drive,
                s.start,
                s.start + s.records,
                s.records,
                s.bytes_per_record,
                s.total_records,
                args.output.display()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("dump-mft: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn run_dump(_: &DumpArgs) -> ExitCode {
    eprintln!("dump-mft: Windows에서만 동작합니다(NTFS 볼륨 핸들이 필요함)");
    ExitCode::FAILURE
}

#[cfg(windows)]
fn run_serve(args: &ServeArgs) -> ExitCode {
    use std::sync::Arc;

    use facet_indexer::pipe::PipeServer;
    use facet_indexer::sample::{SAMPLE_VOLUME, sample_index};
    use facet_indexer::volumes::Volumes;

    let backend = Arc::new(Volumes::with_index(
        SAMPLE_VOLUME,
        sample_index(args.reports),
    ));
    match PipeServer::start(&args.pipe, backend) {
        Ok(_server) => {
            println!(
                "{}에서 견본 인덱스({SAMPLE_VOLUME})로 질의를 받습니다. Ctrl+C로 끝냅니다.",
                args.pipe
            );
            loop {
                std::thread::park();
            }
        }
        Err(e) => {
            eprintln!("serve-sample: 파이프 서버를 시작하지 못했습니다: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn run_serve(_: &ServeArgs) -> ExitCode {
    eprintln!("serve-sample: Windows에서만 동작합니다(이름 있는 파이프)");
    ExitCode::FAILURE
}
