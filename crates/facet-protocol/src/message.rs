//! 메시지 정의와 이진 형식.
//!
//! 메시지 본문의 첫 바이트는 메시지 종류 번호다. 요청은 `0x01`부터, 응답은 `0x81`부터 쓴다.
//! 버전 인사 세 가지(요청 [`Request::Hello`], 응답 [`Response::HelloAccepted`]·
//! [`Response::HelloRejected`])의 형식은 프로토콜 버전이 바뀌어도 바꾸지 않는다. 그래야 버전이
//! 다른 상대끼리도 거절을 읽는다.

use std::fmt;

use crate::wire::{DecodeError, MAX_DEPTH, Reader, Writer};

/// 클라이언트가 서비스에 보내는 메시지.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// 버전 인사. 연결하면 가장 먼저 보낸다. 클라이언트의 프로토콜 버전을 담는다.
    Hello { version: u16 },
    /// 질의. 같은 연결의 앞선 질의(와 그 이어 받기 표지)는 취소된다. 아직 답하지 않은 앞선
    /// 질의·이어 받기는 [`ErrorKind::Cancelled`] 오류로 답한다.
    Query(QueryMessage),
    /// 앞선 질의의 다음 결과 조각 받기.
    Resume(ResumeMessage),
    /// 상태 질의.
    Status,
}

/// 서비스가 클라이언트에 보내는 메시지.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    /// 버전 인사를 받아들였다. 이 연결에서 쓸 버전을 담는다.
    HelloAccepted { version: u16 },
    /// 버전 인사를 거절했다. 클라이언트가 보낸 버전과 서비스가 지원하는 버전 범위를 담는다.
    HelloRejected {
        client_version: u16,
        min_version: u16,
        max_version: u16,
    },
    /// 질의나 이어 받기의 응답.
    Page(PageMessage),
    /// 상태 질의의 응답.
    Status(StatusMessage),
    /// 요청을 처리하지 못했다.
    Error(ErrorMessage),
}

/// 이름 조건의 대소문자 규칙. 인덱스의 [`facet_index::Case`]와 같다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Case {
    /// 대소문자를 무시한다(기본).
    #[default]
    Insensitive,
    /// 대소문자를 구별한다.
    Sensitive,
}

/// 질의의 조건 트리. 조건 하나하나는 인덱스의 [`facet_index::Filter`] 조건과 같다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    /// 이름에 `part`가 들어 있다.
    NameContains { part: String, case: Case },
    /// 이름 전체가 와일드카드(`*`, `?`)와 맞는다.
    NameWildcard { pattern: String, case: Case },
    /// 이름의 어딘가가 정규식과 맞는다.
    NameRegex { pattern: String, case: Case },
    /// 전체 경로(볼륨 루트부터, `\` 구분)에 `part`가 들어 있다.
    PathContains { part: String, case: Case },
    /// 전체 경로 전체가 와일드카드(`*`, `?`)와 맞는다.
    PathWildcard { pattern: String, case: Case },
    /// 폴더 아래(하위 폴더 포함)에 있다.
    Under { folder: String },
    /// 크기가 `min` 이상 `max` 이하(`None`은 그쪽 끝이 열려 있다).
    Size { min: Option<u64>, max: Option<u64> },
    /// 수정 날짜(`FILETIME`)가 `min` 이상 `max` 이하.
    Modified { min: Option<u64>, max: Option<u64> },
    /// 속성 비트를 모두 가진다.
    Attributes { mask: u32 },
    /// 확장자가 같다(대소문자 무시).
    Extension { extension: String },
    /// 모두 맞는다(그리고).
    And(Vec<Condition>),
    /// 하나라도 맞는다(또는).
    Or(Vec<Condition>),
    /// 맞지 않는다(아님).
    Not(Box<Condition>),
}

/// 결과 정렬 기준. 인덱스의 [`facet_index::SortKey`]와 같다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Path,
    Size,
    Modified,
}

/// 정렬 방향. 인덱스의 [`facet_index::Order`]와 같다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Order {
    #[default]
    Ascending,
    Descending,
}

/// 구조화된 질의: 조건 트리, 정렬, 결과 수 제한.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryMessage {
    pub condition: Condition,
    /// 정렬 기준과 방향. `None`이면 인덱스에 넣은 순서다.
    pub sort: Option<(SortKey, Order)>,
    /// 첫 조각에 받을 결과 수의 상한. `None`이면 모두 받는다.
    pub limit: Option<u32>,
}

/// 이어 받기 요청: 앞선 응답의 이어 받기 표지와 이번에 받을 결과 수의 상한.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResumeMessage {
    pub cursor: u64,
    pub limit: u32,
}

/// 결과 항목 하나. 인덱스의 [`facet_index::Hit`]과 같다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HitMessage {
    /// 볼륨 루트부터 만든 전체 경로.
    pub path: String,
    pub name: String,
    pub size: u64,
    /// 수정 날짜(`FILETIME`).
    pub modified: u64,
    pub attributes: u32,
}

/// 질의나 이어 받기의 응답: 결과 조각, 전체 개수, 이어 받기 표지, 만드는 중인 볼륨.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageMessage {
    pub hits: Vec<HitMessage>,
    /// 질의에 맞는 결과 전체의 개수. `total_is_upper_bound`면 권한 거르기 전의 개수다.
    pub total: u64,
    /// `total`이 「걸러지기 전 상한」인가. 인덱서 서비스는 질의한 사용자가 목록을 읽을 수 없는
    /// 폴더 아래 결과를 내보내지 않는데, 전체 개수는 거르기 전에 센다. 그래서 받는 결과는
    /// `total`보다 적을 수 있다.
    pub total_is_upper_bound: bool,
    /// 남은 결과를 받을 이어 받기 표지. 다 받았으면 `None`.
    pub next: Option<u64>,
    /// 인덱스를 (다시) 만드는 중이어서 앞선 인덱스로(또는 결과 없이) 답한 볼륨들(예: `C:`).
    pub building: Vec<String>,
}

/// 상태 질의의 응답: 볼륨마다 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusMessage {
    pub volumes: Vec<VolumeStatus>,
}

/// 볼륨 하나의 상태.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeStatus {
    /// 볼륨 이름(예: `C:`).
    pub volume: String,
    pub state: VolumeState,
}

/// 볼륨의 인덱스 상태.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VolumeState {
    /// 인덱스가 있고 최신이다.
    Ready { items: u64 },
    /// 인덱스를 (다시) 만드는 중이다. 앞선 인덱스가 있으면 그 항목 수, 없으면 `None`.
    Building { items: Option<u64> },
    /// 인덱스가 없다.
    NotIndexed(NoIndexReason),
}

/// 볼륨에 인덱스가 없는 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoIndexReason {
    /// NTFS가 아니다. 파일 시스템 이름(예: `FAT32`, `exFAT`, `ReFS`)을 담는다.
    NotNtfs { filesystem: String },
    /// 네트워크 볼륨이다.
    Network,
    /// 인덱스를 만들지 못했다. 사람이 읽는 설명을 담는다.
    Failed { message: String },
}

/// 오류 응답.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorMessage {
    pub kind: ErrorKind,
    /// 사람이 읽는 설명.
    pub message: String,
}

/// 오류의 종류. 클라이언트는 이것으로 다음 동작을 정한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// 요청을 메시지로 읽지 못했다.
    BadMessage,
    /// 버전 인사 전에 다른 요청을 보냈다.
    NotGreeted,
    /// 질의를 실행할 수 없다(잘못된 정규식 등).
    InvalidQuery,
    /// 이어 받기 표지가 무효다(인덱스가 바뀌었거나 새 질의로 취소됐거나 모르는 표지). 다시 질의한다.
    StaleCursor,
    /// 서비스 안의 오류.
    Internal,
    /// 같은 연결에 새 질의가 와서 이 질의(또는 그 이어 받기)를 취소했다. 새 질의의 응답은 이
    /// 응답 뒤에 온다.
    Cancelled,
}

impl fmt::Display for ErrorMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ErrorMessage {}

// ---- 종류 번호 ----

const REQ_HELLO: u8 = 0x01;
const REQ_QUERY: u8 = 0x02;
const REQ_RESUME: u8 = 0x03;
const REQ_STATUS: u8 = 0x04;

const RES_HELLO_ACCEPTED: u8 = 0x81;
const RES_HELLO_REJECTED: u8 = 0x82;
const RES_PAGE: u8 = 0x83;
const RES_STATUS: u8 = 0x84;
const RES_ERROR: u8 = 0x85;

/// 이진 형식으로 쓰고 읽을 수 있는 메시지([`Request`], [`Response`]).
pub trait Message: Sized {
    /// 메시지 본문(프레임 머리 없이)을 만든다.
    fn to_bytes(&self) -> Vec<u8>;
    /// 메시지 본문을 읽는다. 본문 전체가 메시지 하나여야 한다.
    fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError>;
}

impl Message for Request {
    fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        match self {
            Self::Hello { version } => {
                w.u8(REQ_HELLO);
                w.u16(*version);
            }
            Self::Query(q) => {
                w.u8(REQ_QUERY);
                write_condition(&mut w, &q.condition);
                match q.sort {
                    None => w.u8(0),
                    Some((key, order)) => {
                        w.u8(1);
                        w.u8(match key {
                            SortKey::Name => 0,
                            SortKey::Path => 1,
                            SortKey::Size => 2,
                            SortKey::Modified => 3,
                        });
                        w.u8(match order {
                            Order::Ascending => 0,
                            Order::Descending => 1,
                        });
                    }
                }
                match q.limit {
                    None => w.u8(0),
                    Some(n) => {
                        w.u8(1);
                        w.u32(n);
                    }
                }
            }
            Self::Resume(r) => {
                w.u8(REQ_RESUME);
                w.u64(r.cursor);
                w.u32(r.limit);
            }
            Self::Status => w.u8(REQ_STATUS),
        }
        w.out
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        let message = match r.u8()? {
            REQ_HELLO => Self::Hello { version: r.u16()? },
            REQ_QUERY => {
                let condition = read_condition(&mut r, 0)?;
                let sort = match r.u8()? {
                    0 => None,
                    1 => {
                        let key = match r.u8()? {
                            0 => SortKey::Name,
                            1 => SortKey::Path,
                            2 => SortKey::Size,
                            3 => SortKey::Modified,
                            tag => return Err(unknown("정렬 기준", tag)),
                        };
                        let order = match r.u8()? {
                            0 => Order::Ascending,
                            1 => Order::Descending,
                            tag => return Err(unknown("정렬 방향", tag)),
                        };
                        Some((key, order))
                    }
                    tag => return Err(unknown("있음/없음", tag)),
                };
                let limit = match r.u8()? {
                    0 => None,
                    1 => Some(r.u32()?),
                    tag => return Err(unknown("있음/없음", tag)),
                };
                Self::Query(QueryMessage {
                    condition,
                    sort,
                    limit,
                })
            }
            REQ_RESUME => Self::Resume(ResumeMessage {
                cursor: r.u64()?,
                limit: r.u32()?,
            }),
            REQ_STATUS => Self::Status,
            tag => return Err(unknown("요청", tag)),
        };
        r.finish()?;
        Ok(message)
    }
}

impl Message for Response {
    fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        match self {
            Self::HelloAccepted { version } => {
                w.u8(RES_HELLO_ACCEPTED);
                w.u16(*version);
            }
            Self::HelloRejected {
                client_version,
                min_version,
                max_version,
            } => {
                w.u8(RES_HELLO_REJECTED);
                w.u16(*client_version);
                w.u16(*min_version);
                w.u16(*max_version);
            }
            Self::Page(p) => {
                w.u8(RES_PAGE);
                w.list(&p.hits, |w, h| {
                    w.str(&h.path);
                    w.str(&h.name);
                    w.u64(h.size);
                    w.u64(h.modified);
                    w.u32(h.attributes);
                });
                w.u64(p.total);
                w.bool(p.total_is_upper_bound);
                w.opt_u64(p.next);
                w.list(&p.building, |w, v| w.str(v));
            }
            Self::Status(s) => {
                w.u8(RES_STATUS);
                w.list(&s.volumes, |w, v| {
                    w.str(&v.volume);
                    match &v.state {
                        VolumeState::Ready { items } => {
                            w.u8(0);
                            w.u64(*items);
                        }
                        VolumeState::Building { items } => {
                            w.u8(1);
                            w.opt_u64(*items);
                        }
                        VolumeState::NotIndexed(reason) => {
                            w.u8(2);
                            match reason {
                                NoIndexReason::NotNtfs { filesystem } => {
                                    w.u8(0);
                                    w.str(filesystem);
                                }
                                NoIndexReason::Network => w.u8(1),
                                NoIndexReason::Failed { message } => {
                                    w.u8(2);
                                    w.str(message);
                                }
                            }
                        }
                    }
                });
            }
            Self::Error(e) => {
                w.u8(RES_ERROR);
                w.u8(match e.kind {
                    ErrorKind::BadMessage => 0,
                    ErrorKind::NotGreeted => 1,
                    ErrorKind::InvalidQuery => 2,
                    ErrorKind::StaleCursor => 3,
                    ErrorKind::Internal => 4,
                    ErrorKind::Cancelled => 5,
                });
                w.str(&e.message);
            }
        }
        w.out
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        let message = match r.u8()? {
            RES_HELLO_ACCEPTED => Self::HelloAccepted { version: r.u16()? },
            RES_HELLO_REJECTED => Self::HelloRejected {
                client_version: r.u16()?,
                min_version: r.u16()?,
                max_version: r.u16()?,
            },
            RES_PAGE => {
                let hits = r.list(|r| {
                    Ok(HitMessage {
                        path: r.str()?,
                        name: r.str()?,
                        size: r.u64()?,
                        modified: r.u64()?,
                        attributes: r.u32()?,
                    })
                })?;
                Self::Page(PageMessage {
                    hits,
                    total: r.u64()?,
                    total_is_upper_bound: r.bool()?,
                    next: r.opt_u64()?,
                    building: r.list(Reader::str)?,
                })
            }
            RES_STATUS => {
                let volumes = r.list(|r| {
                    let volume = r.str()?;
                    let state = match r.u8()? {
                        0 => VolumeState::Ready { items: r.u64()? },
                        1 => VolumeState::Building {
                            items: r.opt_u64()?,
                        },
                        2 => VolumeState::NotIndexed(match r.u8()? {
                            0 => NoIndexReason::NotNtfs {
                                filesystem: r.str()?,
                            },
                            1 => NoIndexReason::Network,
                            2 => NoIndexReason::Failed { message: r.str()? },
                            tag => return Err(unknown("인덱스 없음 이유", tag)),
                        }),
                        tag => return Err(unknown("볼륨 상태", tag)),
                    };
                    Ok(VolumeStatus { volume, state })
                })?;
                Self::Status(StatusMessage { volumes })
            }
            RES_ERROR => {
                let kind = match r.u8()? {
                    0 => ErrorKind::BadMessage,
                    1 => ErrorKind::NotGreeted,
                    2 => ErrorKind::InvalidQuery,
                    3 => ErrorKind::StaleCursor,
                    4 => ErrorKind::Internal,
                    5 => ErrorKind::Cancelled,
                    tag => return Err(unknown("오류", tag)),
                };
                Self::Error(ErrorMessage {
                    kind,
                    message: r.str()?,
                })
            }
            tag => return Err(unknown("응답", tag)),
        };
        r.finish()?;
        Ok(message)
    }
}

fn unknown(what: &'static str, tag: u8) -> DecodeError {
    DecodeError::UnknownTag { what, tag }
}

const COND_CONTAINS: u8 = 0;
const COND_WILDCARD: u8 = 1;
const COND_REGEX: u8 = 2;
const COND_UNDER: u8 = 3;
const COND_SIZE: u8 = 4;
const COND_MODIFIED: u8 = 5;
const COND_ATTRIBUTES: u8 = 6;
const COND_EXTENSION: u8 = 7;
const COND_AND: u8 = 8;
const COND_OR: u8 = 9;
const COND_NOT: u8 = 10;
const COND_PATH_CONTAINS: u8 = 11;
const COND_PATH_WILDCARD: u8 = 12;

fn write_case(w: &mut Writer, case: Case) {
    w.bool(case == Case::Sensitive);
}

fn read_case(r: &mut Reader<'_>) -> Result<Case, DecodeError> {
    Ok(if r.bool()? {
        Case::Sensitive
    } else {
        Case::Insensitive
    })
}

fn write_condition(w: &mut Writer, c: &Condition) {
    match c {
        Condition::NameContains { part, case } => {
            w.u8(COND_CONTAINS);
            w.str(part);
            write_case(w, *case);
        }
        Condition::NameWildcard { pattern, case } => {
            w.u8(COND_WILDCARD);
            w.str(pattern);
            write_case(w, *case);
        }
        Condition::NameRegex { pattern, case } => {
            w.u8(COND_REGEX);
            w.str(pattern);
            write_case(w, *case);
        }
        Condition::PathContains { part, case } => {
            w.u8(COND_PATH_CONTAINS);
            w.str(part);
            write_case(w, *case);
        }
        Condition::PathWildcard { pattern, case } => {
            w.u8(COND_PATH_WILDCARD);
            w.str(pattern);
            write_case(w, *case);
        }
        Condition::Under { folder } => {
            w.u8(COND_UNDER);
            w.str(folder);
        }
        Condition::Size { min, max } => {
            w.u8(COND_SIZE);
            w.opt_u64(*min);
            w.opt_u64(*max);
        }
        Condition::Modified { min, max } => {
            w.u8(COND_MODIFIED);
            w.opt_u64(*min);
            w.opt_u64(*max);
        }
        Condition::Attributes { mask } => {
            w.u8(COND_ATTRIBUTES);
            w.u32(*mask);
        }
        Condition::Extension { extension } => {
            w.u8(COND_EXTENSION);
            w.str(extension);
        }
        Condition::And(all) => {
            w.u8(COND_AND);
            w.list(all, write_condition);
        }
        Condition::Or(any) => {
            w.u8(COND_OR);
            w.list(any, write_condition);
        }
        Condition::Not(inner) => {
            w.u8(COND_NOT);
            write_condition(w, inner);
        }
    }
}

/// 조건 하나를 읽는다. `depth`는 이 조건을 감싼 조건 수다.
fn read_condition(r: &mut Reader<'_>, depth: usize) -> Result<Condition, DecodeError> {
    if depth >= MAX_DEPTH {
        return Err(DecodeError::TooDeep);
    }
    Ok(match r.u8()? {
        COND_CONTAINS => Condition::NameContains {
            part: r.str()?,
            case: read_case(r)?,
        },
        COND_WILDCARD => Condition::NameWildcard {
            pattern: r.str()?,
            case: read_case(r)?,
        },
        COND_REGEX => Condition::NameRegex {
            pattern: r.str()?,
            case: read_case(r)?,
        },
        COND_PATH_CONTAINS => Condition::PathContains {
            part: r.str()?,
            case: read_case(r)?,
        },
        COND_PATH_WILDCARD => Condition::PathWildcard {
            pattern: r.str()?,
            case: read_case(r)?,
        },
        COND_UNDER => Condition::Under { folder: r.str()? },
        COND_SIZE => Condition::Size {
            min: r.opt_u64()?,
            max: r.opt_u64()?,
        },
        COND_MODIFIED => Condition::Modified {
            min: r.opt_u64()?,
            max: r.opt_u64()?,
        },
        COND_ATTRIBUTES => Condition::Attributes { mask: r.u32()? },
        COND_EXTENSION => Condition::Extension {
            extension: r.str()?,
        },
        COND_AND => Condition::And(r.list(|r| read_condition(r, depth + 1))?),
        COND_OR => Condition::Or(r.list(|r| read_condition(r, depth + 1))?),
        COND_NOT => Condition::Not(Box::new(read_condition(r, depth + 1)?)),
        tag => return Err(unknown("조건", tag)),
    })
}
