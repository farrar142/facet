//! 인덱서 서비스 실행 파일. 지금은 개발용 하위 명령 `dump-mft`와 `serve-sample`만 있다.

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
        Ok(Command::DumpMft(dump)) => run_dump(&dump),
        Ok(Command::ServeSample(serve)) => run_serve(&serve),
        Err(e) => {
            eprintln!("{e}\n\n{}", cli::USAGE);
            ExitCode::FAILURE
        }
    }
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
    use facet_indexer::session::MemoryBackend;

    let backend = Arc::new(MemoryBackend::new(
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
