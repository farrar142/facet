//! 시험용 질의 명령. 인덱서 서비스의 파이프에 붙어 질의하고 결과를 출력한다.

use std::env;
use std::io::{self, Write};
use std::process::ExitCode;

use facet_query::{Action, Failure, USAGE, connect, parse, run};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let invocation = match parse(&args) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("{e}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    if invocation.action == Action::Help {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let outcome = connect(&invocation.pipe).and_then(|mut pipe| {
        let stdout = io::stdout();
        let mut out = io::BufWriter::new(stdout.lock());
        run(&invocation.action, &mut pipe, &mut out)?;
        out.flush().map_err(Failure::Output)
    });
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("facet-query: {e}");
            // 서비스가 없는 것은 다른 실패와 갈라 2로 끝낸다.
            ExitCode::from(match e {
                Failure::NoService { .. } => 2,
                _ => 1,
            })
        }
    }
}
