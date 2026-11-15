//! 파이프 연결 하나의 대화: 버전 인사, 질의·이어 받기·상태 질의에 답하기, 새 질의로 앞선 질의
//! 취소하기. 바이트 스트림([`Read`]·[`Write`]) 위에서 동작하는 플랫폼과 상관없는 코드다. 파이프는
//! [`crate::pipe`]가 맡는다.
//!
//! 한 연결에서 읽기와 답하기를 두 스레드로 나눈다. 읽는 쪽은 질의가 도는 동안에도 다음 요청을
//! 읽어, 새 질의가 오면 앞선 질의의 취소 표시를 세운다. 답하는 쪽은 요청을 받은 순서대로 하나씩
//! 처리해 요청마다 응답을 꼭 하나 쓴다. 취소된 질의·이어 받기는 [`ErrorKind::Cancelled`]로 답한다.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;

use facet_protocol::{
    ErrorKind, ErrorMessage, HitMessage, PageMessage, QueryMessage, ReadError, Request, Response,
    StatusMessage, WriteError, answer_hello, read_message, write_message,
};

pub use crate::volumes::{Cursor, Page};

/// 질의 하나의 취소 표시. 같은 연결에 새 질의가 오거나 연결이 끊기면 선다.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    /// 서지 않은 새 표시.
    pub fn new() -> Self {
        Self::default()
    }

    /// 표시를 세운다.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// 취소됐는가.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// 파이프 서버가 질의에 답하는 데 쓰는 인덱스 쪽. 여러 연결이 함께 부른다. 인덱서 서비스는
/// 볼륨 여럿의 인덱스([`crate::volumes::Volumes`])로 답한다.
pub trait Backend: Send + Sync {
    /// 질의를 실행한다. 오래 걸리면 `cancel`을 보고 일찍 그만둬도 된다(그만둔 결과는 버린다).
    fn search(&self, query: &QueryMessage, cancel: &Cancel) -> Result<Page, ErrorMessage>;
    /// 이어 받기 표지로 다음 결과 조각을 받는다.
    fn resume(&self, cursor: Cursor, limit: usize) -> Result<Page, ErrorMessage>;
    /// 상태 질의에 답한다.
    fn status(&self) -> StatusMessage;
}

/// 연결이 끝난 까닭.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    /// 상대가 연결을 닫았거나 스트림이 끊겼다.
    Closed,
    /// 지원하지 않는 버전으로 인사해 거절하고 끊었다.
    Rejected,
    /// 최대 크기를 넘는 프레임을 보내 끊었다.
    TooLarge,
}

/// 답하는 쪽 스레드에 넘기는 일. 받은 순서대로 처리한다.
enum Job {
    /// 정해진 응답을 그대로 보낸다.
    Reply(Response),
    Query(QueryMessage, Cancel),
    Resume(facet_protocol::ResumeMessage, Cancel),
    Status,
}

/// 연결 하나를 끝까지 처리한다. `reader`와 `writer`는 같은 연결의 읽는 쪽과 쓰는 쪽이고, 서로
/// 다른 스레드에서 동시에 쓰인다(한쪽이 막혀도 다른 쪽이 움직여야 한다).
pub fn serve<R, W, B>(mut reader: R, writer: W, backend: &B) -> Ended
where
    R: Read,
    W: Write + Send,
    B: Backend + ?Sized,
{
    let (jobs, inbox) = mpsc::channel::<Job>();
    thread::scope(|s| {
        s.spawn(move || answer(inbox, writer, backend));

        let mut greeted = false;
        let mut current = Cancel::new();
        let ended = loop {
            let request = match read_message::<Request>(&mut reader) {
                Ok(r) => r,
                Err(ReadError::TooLarge { .. }) => break Ended::TooLarge,
                Err(ReadError::Decode(e)) => {
                    // 프레임은 끝까지 읽었으므로 다음 프레임부터 계속 읽을 수 있다.
                    let _ = jobs.send(Job::Reply(Response::Error(e.into())));
                    continue;
                }
                Err(_) => break Ended::Closed,
            };
            let job = match request {
                Request::Hello { version } => {
                    let reply = answer_hello(version);
                    let accepted = matches!(reply, Response::HelloAccepted { .. });
                    let _ = jobs.send(Job::Reply(if greeted {
                        error(ErrorKind::BadMessage, "이미 버전 인사를 했습니다")
                    } else {
                        reply
                    }));
                    if !greeted && !accepted {
                        break Ended::Rejected;
                    }
                    greeted = true;
                    continue;
                }
                _ if !greeted => Job::Reply(error(
                    ErrorKind::NotGreeted,
                    "먼저 버전 인사(Hello)를 보내십시오",
                )),
                Request::Query(q) => {
                    current.cancel();
                    current = Cancel::new();
                    Job::Query(q, current.clone())
                }
                Request::Resume(r) => Job::Resume(r, current.clone()),
                Request::Status => Job::Status,
            };
            if jobs.send(job).is_err() {
                // 답하는 쪽이 쓰지 못해 끝났다(상대가 사라졌다).
                break Ended::Closed;
            }
        };
        // 연결이 끝나면 돌고 있는 질의도 쓸모가 없다.
        current.cancel();
        drop(jobs);
        ended
    })
}

fn error(kind: ErrorKind, message: &str) -> Response {
    Response::Error(ErrorMessage {
        kind,
        message: message.to_owned(),
    })
}

fn cancelled() -> Response {
    error(
        ErrorKind::Cancelled,
        "같은 연결에 새 질의가 와서 취소했습니다",
    )
}

/// 답하는 쪽: 일을 받은 순서대로 처리해 응답을 하나씩 쓴다.
fn answer<W: Write, B: Backend + ?Sized>(inbox: mpsc::Receiver<Job>, mut writer: W, backend: &B) {
    // 이 연결의 이어 받기 표지. 새 질의가 오면 모두 버린다.
    let mut cursors: HashMap<u64, Cursor> = HashMap::new();
    let mut next_cursor = 1u64;
    let mut keep = |cursors: &mut HashMap<u64, Cursor>, page: Page| {
        Response::Page(PageMessage {
            hits: page.hits.into_iter().map(HitMessage::from).collect(),
            total: page.total as u64,
            next: page.next.map(|c| {
                let id = next_cursor;
                next_cursor += 1;
                cursors.insert(id, c);
                id
            }),
            building: page.building,
        })
    };

    for job in inbox {
        let response = match job {
            Job::Reply(r) => r,
            Job::Query(q, cancel) => {
                cursors.clear();
                if cancel.is_cancelled() {
                    cancelled()
                } else {
                    match backend.search(&q, &cancel) {
                        _ if cancel.is_cancelled() => cancelled(),
                        Ok(page) => keep(&mut cursors, page),
                        Err(e) => Response::Error(e),
                    }
                }
            }
            Job::Resume(r, cancel) => {
                if cancel.is_cancelled() {
                    cancelled()
                } else {
                    match cursors.remove(&r.cursor) {
                        None => error(
                            ErrorKind::StaleCursor,
                            "모르는 이어 받기 표지입니다. 다시 질의하십시오",
                        ),
                        Some(c) => match backend.resume(c, r.limit()) {
                            Ok(page) => keep(&mut cursors, page),
                            Err(e) => Response::Error(e),
                        },
                    }
                }
            }
            Job::Status => Response::Status(backend.status()),
        };
        let written = match write_message(&mut writer, &response) {
            Err(WriteError::TooLarge { len }) => write_message(
                &mut writer,
                &error(
                    ErrorKind::Internal,
                    &format!(
                        "응답이 한 프레임에 너무 큽니다({len}바이트). 결과 수 제한을 주십시오"
                    ),
                ),
            ),
            other => other,
        };
        if written.is_err() {
            return;
        }
    }
}
