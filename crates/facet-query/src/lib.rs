//! 시험용 질의 명령(`facet-query`)의 라이브러리 쪽: 명령줄 해석, 파이프 연결, 질의 보내기와
//! 결과 출력.
//!
//! 화면이 생기기 전까지 인덱서 서비스를 쓰는 유일한 통로다. 일반 권한으로 서비스의 파이프에 붙어
//! 버전 인사 뒤 구조화된 질의나 상태 질의를 보내고 결과를 출력한다.
//!
//! Windows API를 직접 부르지 않는다. 메시지는 [`facet_protocol`]이 만들고 읽으며, 파이프는 std의
//! 파일 열기로 연다(이름 있는 파이프는 Windows에서 파일처럼 열린다).

use std::fmt;
use std::io::{self, Read, Write};

use facet_protocol::{
    Case, Condition, ErrorMessage, NoIndexReason, Order, PROTOCOL_VERSION, PageMessage,
    QueryMessage, ReadError, Request, Response, ResumeMessage, SortKey, StatusMessage, VolumeState,
    WriteError, read_message, write_message,
};

pub const USAGE: &str = "\
사용법: facet-query [--pipe 이름] <명령>

명령:
  status
      인덱서 서비스의 상태(볼륨마다 인덱스가 있는지, 만드는 중인지, 항목 수)를 출력한다.
  search [<이름 조각>] [--wildcard 패턴 | --regex 식] [--case-sensitive]
         [--sort name|path|size|modified] [--desc] [--limit N] [--pages N | --all]
      이름 조건에 맞는 항목을 질의해 한 줄에 하나씩 「경로<탭>크기」로 출력하고, 끝에 받은 수와
      전체 개수를 출력한다.
      <이름 조각>        이름에 이 글자가 들어 있다(부분 일치)
      --wildcard 패턴    이름 전체가 와일드카드(*, ?)와 맞는다
      --regex 식         이름의 어딘가가 정규식과 맞는다
      --case-sensitive   대소문자를 구별한다(기본은 무시)
      --sort 기준        정렬 기준(기본: 인덱스에 넣은 순서)
      --desc             큰 것부터 정렬한다
      --limit N          한 조각에 받을 결과 수(기본 100)
      --pages N          이어 받기로 조각을 N개까지 받는다(기본 1)
      --all              남은 결과를 모두 이어 받는다
  help
      이 도움말을 보인다

  --pipe 이름   붙을 파이프(기본: 인덱서 서비스의 파이프)";

/// 해석한 명령줄.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// 붙을 파이프 이름.
    pub pipe: String,
    pub action: Action,
}

/// 할 일.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Help,
    Status,
    Search(Search),
}

/// `search`의 인자.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Search {
    pub condition: Condition,
    pub sort: Option<(SortKey, Order)>,
    /// 한 조각의 결과 수.
    pub limit: u32,
    /// 받을 조각 수. `None`이면 모두.
    pub pages: Option<u32>,
}

/// 실행 파일 이름을 뺀 인자를 해석한다.
pub fn parse(args: &[String]) -> Result<Invocation, String> {
    let mut pipe = facet_protocol::PIPE_NAME.to_owned();
    let mut rest = args;
    if let [flag, value, tail @ ..] = rest
        && flag == "--pipe"
    {
        pipe = value.clone();
        rest = tail;
    } else if let [flag] = rest
        && flag == "--pipe"
    {
        return Err("--pipe 뒤에 값이 없습니다".to_owned());
    }
    let action = match rest.first().map(String::as_str) {
        None | Some("help" | "--help" | "-h") => Action::Help,
        Some("status") => match &rest[1..] {
            [] => Action::Status,
            [extra, ..] => return Err(format!("알 수 없는 인자: {extra}")),
        },
        Some("search") => Action::Search(parse_search(&rest[1..])?),
        Some(other) => return Err(format!("알 수 없는 명령: {other}")),
    };
    Ok(Invocation { pipe, action })
}

fn parse_search(rest: &[String]) -> Result<Search, String> {
    let mut part: Option<String> = None;
    let mut pattern: Option<(bool, String)> = None; // (정규식인가, 패턴)
    let mut case = Case::Insensitive;
    let mut key = None;
    let mut order = Order::Ascending;
    let mut limit = 100;
    let mut pages = Some(1);
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{flag} 뒤에 값이 없습니다"))
        };
        match arg.as_str() {
            "--wildcard" | "--regex" if pattern.is_some() => {
                return Err("--wildcard와 --regex는 하나만 줍니다".to_owned());
            }
            "--wildcard" => pattern = Some((false, value("--wildcard")?)),
            "--regex" => pattern = Some((true, value("--regex")?)),
            "--case-sensitive" => case = Case::Sensitive,
            "--sort" => {
                key = Some(match value("--sort")?.as_str() {
                    "name" => SortKey::Name,
                    "path" => SortKey::Path,
                    "size" => SortKey::Size,
                    "modified" => SortKey::Modified,
                    other => {
                        return Err(format!(
                            "정렬 기준은 name, path, size, modified 중 하나입니다: {other}"
                        ));
                    }
                });
            }
            "--desc" => order = Order::Descending,
            "--limit" => limit = positive(&value("--limit")?, "--limit")?,
            "--pages" => pages = Some(positive(&value("--pages")?, "--pages")?),
            "--all" => pages = None,
            _ if arg.starts_with("--") => return Err(format!("알 수 없는 인자: {arg}")),
            _ if part.is_some() => return Err(format!("알 수 없는 인자: {arg}")),
            _ => part = Some(arg.clone()),
        }
    }
    let condition = match (part, pattern) {
        (Some(_), Some(_)) => {
            return Err("이름 조각과 --wildcard/--regex는 함께 줄 수 없습니다".to_owned());
        }
        (Some(part), None) => Condition::NameContains { part, case },
        (None, Some((false, pattern))) => Condition::NameWildcard { pattern, case },
        (None, Some((true, pattern))) => Condition::NameRegex { pattern, case },
        (None, None) => {
            return Err("search에는 이름 조각이나 --wildcard/--regex가 필요합니다".to_owned());
        }
    };
    Ok(Search {
        condition,
        sort: key.map(|k| (k, order)),
        limit,
        pages,
    })
}

fn positive(v: &str, flag: &str) -> Result<u32, String> {
    match v.parse::<u32>() {
        Ok(n) if n > 0 => Ok(n),
        _ => Err(format!("{flag}의 값은 1 이상의 정수입니다: {v}")),
    }
}

/// 명령이 실패한 까닭.
#[derive(Debug)]
pub enum Failure {
    /// 서비스의 파이프가 없다(서비스가 돌지 않는다).
    NoService { pipe: String },
    /// 파이프에 붙지 못했다.
    Connect { pipe: String, error: io::Error },
    /// 서비스가 이 명령의 프로토콜 버전을 받지 않았다.
    Rejected {
        client_version: u16,
        min_version: u16,
        max_version: u16,
    },
    /// 서비스가 오류로 답했다.
    Service(ErrorMessage),
    /// 주고받기가 깨졌다(연결이 끊기거나 뜻밖의 응답).
    Protocol(String),
    /// 결과를 출력하지 못했다.
    Output(io::Error),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoService { pipe } => write!(
                f,
                "인덱서 서비스가 없습니다: 파이프 {pipe}를 찾지 못했습니다. 서비스가 돌고 있는지 확인하세요."
            ),
            Self::Connect { pipe, error } => {
                write!(f, "파이프 {pipe}에 붙지 못했습니다: {error}")
            }
            Self::Rejected {
                client_version,
                min_version,
                max_version,
            } => write!(
                f,
                "서비스가 프로토콜 버전 {client_version}을 받지 않았습니다(지원 범위 {min_version}~{max_version})"
            ),
            Self::Service(e) => write!(f, "서비스 오류({:?}): {}", e.kind, e.message),
            Self::Protocol(m) => write!(f, "서비스와 주고받지 못했습니다: {m}"),
            Self::Output(e) => write!(f, "출력하지 못했습니다: {e}"),
        }
    }
}

impl From<ReadError> for Failure {
    fn from(e: ReadError) -> Self {
        Self::Protocol(e.to_string())
    }
}

impl From<WriteError> for Failure {
    fn from(e: WriteError) -> Self {
        Self::Protocol(e.to_string())
    }
}

impl From<io::Error> for Failure {
    fn from(e: io::Error) -> Self {
        Self::Output(e)
    }
}

/// ERROR_FILE_NOT_FOUND: 그 이름의 파이프가 없다.
#[cfg(windows)]
const ERROR_FILE_NOT_FOUND: i32 = 2;
/// ERROR_PIPE_BUSY: 파이프는 있지만 지금 받을 인스턴스가 없다.
#[cfg(windows)]
const ERROR_PIPE_BUSY: i32 = 231;
/// SECURITY_IDENTIFICATION: 서비스가 클라이언트 신원을 확인만 하고 대신 행동하지는 못하게 한다.
#[cfg(windows)]
const SECURITY_IDENTIFICATION: u32 = 0x0001_0000;

/// 서비스의 파이프에 붙는다. 읽기와 데이터 쓰기 권한만 청한다
/// ([`facet_protocol::PIPE_CLIENT_ACCESS`]). 인스턴스가 모두 바쁘면 2초까지 다시 해 본다.
#[cfg(windows)]
pub fn connect(pipe: &str) -> Result<std::fs::File, Failure> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let opened = std::fs::OpenOptions::new()
            .access_mode(facet_protocol::PIPE_CLIENT_ACCESS)
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .open(pipe);
        match opened {
            Ok(f) => return Ok(f),
            Err(e) if e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND) => {
                return Err(Failure::NoService {
                    pipe: pipe.to_owned(),
                });
            }
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                return Err(Failure::Connect {
                    pipe: pipe.to_owned(),
                    error,
                });
            }
        }
    }
}

/// 이름 있는 파이프는 Windows에만 있다.
#[cfg(not(windows))]
pub fn connect(pipe: &str) -> Result<std::fs::File, Failure> {
    Err(Failure::Connect {
        pipe: pipe.to_owned(),
        error: io::Error::new(
            io::ErrorKind::Unsupported,
            "인덱서 서비스 파이프는 Windows에서만 쓸 수 있습니다",
        ),
    })
}

fn ask<S: Read + Write>(stream: &mut S, request: &Request) -> Result<Response, Failure> {
    write_message(stream, request)?;
    Ok(read_message(stream)?)
}

/// 붙은 스트림에서 버전 인사를 하고 `action`을 실행해 결과를 `out`에 출력한다.
pub fn run<S: Read + Write>(
    action: &Action,
    stream: &mut S,
    out: &mut impl Write,
) -> Result<(), Failure> {
    match ask(
        stream,
        &Request::Hello {
            version: PROTOCOL_VERSION,
        },
    )? {
        Response::HelloAccepted { .. } => {}
        Response::HelloRejected {
            client_version,
            min_version,
            max_version,
        } => {
            return Err(Failure::Rejected {
                client_version,
                min_version,
                max_version,
            });
        }
        other => return Err(unexpected(&other)),
    }
    match action {
        Action::Help => Ok(()),
        Action::Status => match ask(stream, &Request::Status)? {
            Response::Status(s) => Ok(print_status(&s, out)?),
            Response::Error(e) => Err(Failure::Service(e)),
            other => Err(unexpected(&other)),
        },
        Action::Search(search) => run_search(search, stream, out),
    }
}

fn unexpected(r: &Response) -> Failure {
    Failure::Protocol(format!("뜻밖의 응답: {r:?}"))
}

fn page_of(r: Response) -> Result<PageMessage, Failure> {
    match r {
        Response::Page(p) => Ok(p),
        Response::Error(e) => Err(Failure::Service(e)),
        other => Err(unexpected(&other)),
    }
}

fn run_search<S: Read + Write>(
    search: &Search,
    stream: &mut S,
    out: &mut impl Write,
) -> Result<(), Failure> {
    let query = Request::Query(QueryMessage {
        condition: search.condition.clone(),
        sort: search.sort,
        limit: Some(search.limit),
    });
    let mut page = page_of(ask(stream, &query)?)?;
    let total = page.total;
    let mut building = Vec::new();
    let mut received = 0u64;
    let mut pages = 1u32;
    loop {
        for hit in &page.hits {
            writeln!(out, "{}\t{}", hit.path, hit.size)?;
        }
        received += page.hits.len() as u64;
        for v in page.building.drain(..) {
            if !building.contains(&v) {
                building.push(v);
            }
        }
        let Some(cursor) = page.next else { break };
        if search.pages.is_some_and(|max| pages >= max) {
            break;
        }
        page = page_of(ask(
            stream,
            &Request::Resume(ResumeMessage {
                cursor,
                limit: search.limit,
            }),
        )?)?;
        pages += 1;
    }
    writeln!(out, "결과 {received}개 / 전체 {total}개")?;
    if received < total {
        writeln!(
            out,
            "남은 결과 {}개는 --pages N이나 --all로 이어 받습니다",
            total - received
        )?;
    }
    if !building.is_empty() {
        writeln!(
            out,
            "인덱스를 만드는 중인 볼륨: {} (결과가 빠졌거나 오래됐을 수 있습니다)",
            building.join(", ")
        )?;
    }
    Ok(())
}

fn print_status(status: &StatusMessage, out: &mut impl Write) -> io::Result<()> {
    if status.volumes.is_empty() {
        writeln!(out, "인덱스 대상 볼륨이 없습니다")?;
    }
    for v in &status.volumes {
        let state = match &v.state {
            VolumeState::Ready { items } => format!("준비됨\t항목 {items}개"),
            VolumeState::Building { items: Some(items) } => {
                format!("만드는 중\t앞선 인덱스 항목 {items}개로 답함")
            }
            VolumeState::Building { items: None } => "만드는 중\t앞선 인덱스 없음".to_owned(),
            VolumeState::NotIndexed(NoIndexReason::NotNtfs { filesystem }) => {
                format!("인덱스 없음\tNTFS가 아님({filesystem})")
            }
            VolumeState::NotIndexed(NoIndexReason::Network) => {
                "인덱스 없음\t네트워크 볼륨".to_owned()
            }
            VolumeState::NotIndexed(NoIndexReason::Failed { message }) => {
                format!("인덱스 없음\t만들지 못함: {message}")
            }
        };
        writeln!(out, "{}\t{state}", v.volume)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    fn search(v: &[&str]) -> Search {
        let mut all = vec!["search"];
        all.extend_from_slice(v);
        match parse(&args(&all)).unwrap().action {
            Action::Search(s) => s,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn no_command_is_help_and_pipe_defaults_to_service() {
        let inv = parse(&[]).unwrap();
        assert_eq!(inv.action, Action::Help);
        assert_eq!(inv.pipe, facet_protocol::PIPE_NAME);
    }

    #[test]
    fn pipe_and_status() {
        assert_eq!(
            parse(&args(&["--pipe", "p", "status"])).unwrap(),
            Invocation {
                pipe: "p".into(),
                action: Action::Status
            }
        );
    }

    #[test]
    fn search_defaults() {
        assert_eq!(
            search(&["보고서"]),
            Search {
                condition: Condition::NameContains {
                    part: "보고서".into(),
                    case: Case::Insensitive
                },
                sort: None,
                limit: 100,
                pages: Some(1),
            }
        );
    }

    #[test]
    fn search_with_every_option() {
        assert_eq!(
            search(&[
                "--regex",
                "^a.*z$",
                "--case-sensitive",
                "--sort",
                "size",
                "--desc",
                "--limit",
                "5",
                "--all"
            ]),
            Search {
                condition: Condition::NameRegex {
                    pattern: "^a.*z$".into(),
                    case: Case::Sensitive
                },
                sort: Some((SortKey::Size, Order::Descending)),
                limit: 5,
                pages: None,
            }
        );
        assert_eq!(
            search(&["--wildcard", "*.txt", "--sort", "path", "--pages", "3"]),
            Search {
                condition: Condition::NameWildcard {
                    pattern: "*.txt".into(),
                    case: Case::Insensitive
                },
                sort: Some((SortKey::Path, Order::Ascending)),
                limit: 100,
                pages: Some(3),
            }
        );
    }

    #[test]
    fn bad_arguments_are_errors() {
        for bad in [
            &["--pipe"][..],
            &["status", "x"],
            &["search"],
            &["search", "a", "b"],
            &["search", "a", "--regex", "b"],
            &["search", "--regex", "a", "--wildcard", "b"],
            &["search", "a", "--sort", "colour"],
            &["search", "a", "--limit", "0"],
            &["search", "a", "--limit"],
            &["search", "a", "--pages", "-1"],
            &["search", "a", "--what"],
            &["mystery"],
        ] {
            assert!(parse(&args(bad)).is_err(), "{bad:?}");
        }
    }
}
