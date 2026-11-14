//! 인덱서 서비스 실행 파일. 지금은 개발용 하위 명령 `dump-mft`만 있다.

use std::env;
use std::process::ExitCode;

use facet_indexer::cli::{self, Command, DumpArgs};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match cli::parse(&args) {
        Ok(Command::Help) => {
            println!("{}", cli::USAGE);
            ExitCode::SUCCESS
        }
        Ok(Command::DumpMft(dump)) => run_dump(&dump),
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
