//! 명령줄 해석. 플랫폼과 상관없는 순수 코드다.

use std::path::PathBuf;

pub const USAGE: &str = "\
사용법: facet-indexer <명령>

명령:
  dump-mft <볼륨> <출력 파일> [--start N] [--count N]
      (개발용, 관리자 권한) NTFS 볼륨의 MFT 원시 레코드를 덩어리째 읽어 파일로 떠 낸다.
      <볼륨>은 드라이브 문자(예: D: 또는 D). 레코드는 고정값(fixup)을 되돌리지 않은 디스크 그대로다.
      --start  첫 레코드 번호(기본 0)
      --count  레코드 수(기본: start부터 MFT의 유효 데이터 끝까지)
  serve-sample [--pipe 이름] [--reports N]
      (개발용) 견본 인덱스(메모리, 볼륨 S:)로 질의 파이프 서버를 띄운다. Ctrl+C로 끝낸다.
      시험용 질의 명령(facet-query)으로 질의해 본다.
      --pipe     파이프 이름(기본: 인덱서 서비스의 파이프 이름)
      --reports  견본 「보고서 N.txt」 파일 수(기본 1000)
  help
      이 도움말을 보인다";

/// 해석한 명령.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    DumpMft(DumpArgs),
    ServeSample(ServeArgs),
}

/// `serve-sample`의 인자.
#[derive(Debug, PartialEq, Eq)]
pub struct ServeArgs {
    pub pipe: String,
    pub reports: u64,
}

/// `dump-mft`의 인자.
#[derive(Debug, PartialEq, Eq)]
pub struct DumpArgs {
    /// 대문자 드라이브 문자.
    pub drive: char,
    pub output: PathBuf,
    pub start: u64,
    pub count: Option<u64>,
}

/// 실행 파일 이름을 뺀 인자를 해석한다.
pub fn parse(args: &[String]) -> Result<Command, String> {
    match args.first().map(String::as_str) {
        None | Some("help" | "--help" | "-h") => Ok(Command::Help),
        Some("dump-mft") => parse_dump(&args[1..]).map(Command::DumpMft),
        Some("serve-sample") => parse_serve(&args[1..]).map(Command::ServeSample),
        Some(other) => Err(format!("알 수 없는 명령: {other}")),
    }
}

fn parse_dump(rest: &[String]) -> Result<DumpArgs, String> {
    let mut positional = Vec::new();
    let mut start = 0;
    let mut count = None;
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--start" => start = number(it.next(), "--start")?,
            "--count" => count = Some(number(it.next(), "--count")?),
            _ if arg.starts_with("--") => return Err(format!("알 수 없는 인자: {arg}")),
            _ => positional.push(arg),
        }
    }
    match positional.as_slice() {
        [volume, output] => Ok(DumpArgs {
            drive: parse_drive(volume)?,
            output: PathBuf::from(output),
            start,
            count,
        }),
        [] | [_] => Err("dump-mft에는 <볼륨>과 <출력 파일>이 필요합니다".to_owned()),
        [_, _, extra, ..] => Err(format!("알 수 없는 인자: {extra}")),
    }
}

fn parse_serve(rest: &[String]) -> Result<ServeArgs, String> {
    let mut serve = ServeArgs {
        pipe: facet_protocol::PIPE_NAME.to_owned(),
        reports: 1000,
    };
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--pipe" => {
                serve.pipe = it.next().ok_or("--pipe 뒤에 값이 없습니다")?.clone();
            }
            "--reports" => serve.reports = number(it.next(), "--reports")?,
            _ => return Err(format!("알 수 없는 인자: {arg}")),
        }
    }
    Ok(serve)
}

fn number(value: Option<&String>, flag: &str) -> Result<u64, String> {
    let v = value.ok_or_else(|| format!("{flag} 뒤에 값이 없습니다"))?;
    v.parse()
        .map_err(|_| format!("{flag}의 값이 0 이상의 정수가 아닙니다: {v}"))
}

/// `D`, `d:`, `D:\` 같은 표기에서 드라이브 문자를 꺼낸다.
pub fn parse_drive(volume: &str) -> Result<char, String> {
    let trimmed = volume.trim_end_matches(['\\', '/']);
    let trimmed = trimmed.strip_suffix(':').unwrap_or(trimmed);
    let mut chars = trimmed.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => Ok(c.to_ascii_uppercase()),
        _ => Err(format!("볼륨은 드라이브 문자로 적습니다(예: D:): {volume}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn no_command_or_help_is_help() {
        assert_eq!(parse(&args(&[])), Ok(Command::Help));
        assert_eq!(parse(&args(&["help"])), Ok(Command::Help));
    }

    #[test]
    fn dump_mft_takes_volume_output_and_range() {
        assert_eq!(
            parse(&args(&[
                "dump-mft", "d:", "out.mft", "--start", "16", "--count", "8"
            ])),
            Ok(Command::DumpMft(DumpArgs {
                drive: 'D',
                output: PathBuf::from("out.mft"),
                start: 16,
                count: Some(8),
            }))
        );
        assert_eq!(
            parse(&args(&["dump-mft", "E:\\", "o"])),
            Ok(Command::DumpMft(DumpArgs {
                drive: 'E',
                output: PathBuf::from("o"),
                start: 0,
                count: None,
            }))
        );
    }

    #[test]
    fn serve_sample_takes_pipe_and_reports() {
        assert_eq!(
            parse(&args(&["serve-sample"])),
            Ok(Command::ServeSample(ServeArgs {
                pipe: facet_protocol::PIPE_NAME.to_owned(),
                reports: 1000,
            }))
        );
        assert_eq!(
            parse(&args(&[
                "serve-sample",
                "--pipe",
                "my-pipe",
                "--reports",
                "5"
            ])),
            Ok(Command::ServeSample(ServeArgs {
                pipe: "my-pipe".to_owned(),
                reports: 5,
            }))
        );
        assert!(parse(&args(&["serve-sample", "--pipe"])).is_err());
        assert!(parse(&args(&["serve-sample", "x"])).is_err());
    }

    #[test]
    fn bad_dump_arguments_are_errors() {
        for bad in [
            &["dump-mft"][..],
            &["dump-mft", "D:"],
            &["dump-mft", "D:", "o", "x"],
            &["dump-mft", "DD:", "o"],
            &["dump-mft", "1:", "o"],
            &["dump-mft", "D:", "o", "--count"],
            &["dump-mft", "D:", "o", "--count", "-1"],
            &["dump-mft", "D:", "o", "--what"],
            &["mystery"],
        ] {
            assert!(parse(&args(bad)).is_err(), "{bad:?}");
        }
    }
}
