//! 실제 폴더 권한으로 질의 결과를 거르는지 본다(Windows 전용). 시험 폴더에 「목록 읽기」 거부를
//! 걸고(`icacls`), 같은 프로세스에 띄운 파이프 서버에 일반 클라이언트처럼 붙어 질의한다. 서버는
//! 파이프 클라이언트(이 테스트 프로세스의 사용자)로 가장해 잡은 토큰으로 권한을 검사한다.
//! 관리자 권한이 필요 없다.
#![cfg(windows)]

use std::fs::{self, File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use facet_index::{ATTRIBUTE_DIRECTORY, Index, NewItem, USN_REASON_SECURITY_CHANGE};
use facet_indexer::caller::ClientToken;
use facet_indexer::pipe::{CLIENT_ACCESS, PipeServer, current_user_sid};
use facet_indexer::volumes::Volumes;
use facet_protocol::{
    Case, Condition, HitMessage, PROTOCOL_VERSION, PageMessage, QueryMessage, Request, Response,
    ResumeMessage, read_message, write_message,
};

const VOLUME: &str = "T:";
const ROOT: u64 = 5;
const DIR: u32 = ATTRIBUTE_DIRECTORY;
const FILE: u32 = 0x20;

/// 파일 수. `공개`·`비밀`·`비밀\안쪽`에 번갈아 둔다.
const FILES: usize = 12;

fn unique_name() -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        r"\\.\pipe\facet-permission-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn connect(name: &str) -> File {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match OpenOptions::new().access_mode(CLIENT_ACCESS).open(name) {
            Ok(f) => return f,
            // ERROR_PIPE_BUSY
            Err(e) if e.raw_os_error() == Some(231) && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(e) => panic!("{name}에 붙지 못함: {e}"),
        }
    }
}

fn ask(pipe: &mut File, request: &Request) -> Response {
    write_message(pipe, request).unwrap();
    read_message(pipe).unwrap()
}

fn page(r: Response) -> PageMessage {
    match r {
        Response::Page(p) => p,
        other => panic!("결과 조각이 아님: {other:?}"),
    }
}

/// `icacls`를 부른다.
fn icacls(path: &Path, args: &[&str]) {
    let out = Command::new("icacls")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "icacls {} {args:?} 실패: {}",
        path.display(),
        String::from_utf8_lossy(&out.stdout)
    );
}

/// 시험 폴더: 임시 폴더 아래 `공개`, `비밀`, `비밀\안쪽`과 그 안의 파일들. 버릴 때 건 거부를
/// 풀어 임시 폴더를 지울 수 있게 한다.
struct Folder {
    dir: tempfile::TempDir,
    sid: String,
}

impl Folder {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for sub in ["공개", r"비밀\안쪽"] {
            fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        for i in 0..FILES {
            fs::write(dir.path().join(Self::file(i)), b"x").unwrap();
        }
        Self {
            dir,
            sid: current_user_sid().unwrap(),
        }
    }

    fn file(i: usize) -> String {
        let folder = match i % 3 {
            0 => "공개",
            1 => "비밀",
            _ => r"비밀\안쪽",
        };
        format!(r"{folder}\파일 {i:02}.txt")
    }

    fn path(&self, sub: &str) -> PathBuf {
        self.dir.path().join(sub)
    }

    /// 이 사용자에게 `sub` 폴더의 목록 읽기(RD)를 거부한다(그 폴더에만, 상속 없이).
    fn deny_list(&self, sub: &str) {
        icacls(&self.path(sub), &["/deny", &format!("*{}:(RD)", self.sid)]);
    }

    /// 건 거부를 푼다.
    fn allow_list(&self, sub: &str) {
        icacls(&self.path(sub), &["/remove:d", &format!("*{}", self.sid)]);
    }

    /// 시험 폴더를 루트로 하는 인덱스. 항목은 실제 폴더·파일과 같다.
    fn index(&self) -> Index {
        let root = self.dir.path().to_str().unwrap();
        let mut index = Index::new(root, ROOT);
        let mut put = |number, parent, name: &str, attributes| {
            index
                .insert(NewItem {
                    number,
                    parent,
                    name,
                    size: 1,
                    modified: 99,
                    attributes,
                })
                .unwrap();
        };
        put(10, ROOT, "공개", DIR);
        put(11, ROOT, "비밀", DIR);
        put(12, 11, "안쪽", DIR);
        for i in 0..FILES {
            let parent = [10, 11, 12][i % 3];
            put(100 + i as u64, parent, &format!("파일 {i:02}.txt"), FILE);
        }
        index
    }

    fn expect(&self, files: impl Iterator<Item = usize>) -> Vec<String> {
        let mut want: Vec<String> = files
            .map(|i| self.path(&Self::file(i)).to_str().unwrap().to_owned())
            .collect();
        want.sort();
        want
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = Command::new("icacls")
            .arg(self.path("비밀"))
            .args(["/remove:d", &format!("*{}", self.sid)])
            .output();
    }
}

fn query(limit: u32) -> Request {
    Request::Query(QueryMessage {
        condition: Condition::NameContains {
            part: ".txt".into(),
            case: Case::Insensitive,
        },
        sort: None,
        limit: Some(limit),
    })
}

/// 질의하고 표지가 다할 때까지 이어 받는다. (모든 결과 경로를 정렬한 것, 첫 조각)
fn search_all(pipe: &mut File, limit: u32) -> (Vec<String>, PageMessage) {
    let first = page(ask(pipe, &query(limit)));
    let mut hits: Vec<HitMessage> = first.hits.clone();
    let mut next = first.next;
    while let Some(cursor) = next {
        let p = page(ask(pipe, &Request::Resume(ResumeMessage { cursor, limit })));
        assert!(p.total_is_upper_bound);
        assert!(p.hits.len() as u32 <= limit);
        hits.extend(p.hits);
        next = p.next;
    }
    let mut paths: Vec<String> = hits.into_iter().map(|h| h.path).collect();
    paths.sort();
    (paths, first)
}

#[test]
fn the_token_check_follows_the_folder_acl() {
    let folder = Folder::new();
    let me = ClientToken::current().unwrap();
    assert_eq!(me.sid(), folder.sid);
    let secret = folder.path("비밀");
    let secret = secret.to_str().unwrap();
    assert!(me.check_list(secret).unwrap());
    folder.deny_list("비밀");
    assert!(!me.check_list(secret).unwrap());
    assert!(
        me.check_list(folder.path("공개").to_str().unwrap())
            .unwrap()
    );
    // 사라진 폴더는 오류다(질의는 읽을 수 없는 것으로 친다).
    assert!(
        me.check_list(folder.path("없음").to_str().unwrap())
            .is_err()
    );
}

#[test]
fn items_under_a_folder_the_client_cannot_list_are_filtered_over_the_pipe() {
    let folder = Folder::new();
    folder.deny_list("비밀");

    let volumes = Arc::new(Volumes::with_index(VOLUME, folder.index()));
    let name = unique_name();
    let _server = PipeServer::start(&name, volumes.clone()).unwrap();
    let mut pipe = connect(&name);
    assert_eq!(
        ask(
            &mut pipe,
            &Request::Hello {
                version: PROTOCOL_VERSION
            }
        ),
        Response::HelloAccepted {
            version: PROTOCOL_VERSION
        }
    );

    // 결과 수 제한은 걸러진 만큼 뒤 후보로 채우고, 이어 받아도 걸러진다.
    let (got, first) = search_all(&mut pipe, 2);
    assert_eq!(first.hits.len(), 2);
    assert_eq!(first.total, FILES as u64);
    assert!(first.total_is_upper_bound);
    assert_eq!(got, folder.expect((0..FILES).filter(|i| i % 3 == 0)));

    // 거부를 풀어도 기억으로 답하다가, 그 폴더의 보안 변경 레코드가 오면 반영된다.
    folder.allow_list("비밀");
    let (got, _) = search_all(&mut pipe, 2);
    assert_eq!(got.len(), FILES / 3);
    let outcome = volumes
        .update(VOLUME, |index| {
            index.apply_usn(&security_change(11, ROOT, "비밀"))
        })
        .unwrap()
        .unwrap();
    volumes.forget_access(&outcome);
    let (got, _) = search_all(&mut pipe, 2);
    assert_eq!(got, folder.expect(0..FILES));
}

/// 폴더 보안 변경 `USN_RECORD_V2` 하나의 바이트.
fn security_change(number: u64, parent: u64, name: &str) -> Vec<u8> {
    let units: Vec<u16> = name.encode_utf16().collect();
    let length = (60 + units.len() * 2).next_multiple_of(8);
    let mut b = vec![0u8; length];
    b[0..4].copy_from_slice(&(length as u32).to_le_bytes());
    b[4..6].copy_from_slice(&2u16.to_le_bytes());
    b[8..16].copy_from_slice(&number.to_le_bytes());
    b[16..24].copy_from_slice(&parent.to_le_bytes());
    b[24..32].copy_from_slice(&1i64.to_le_bytes());
    b[40..44].copy_from_slice(&USN_REASON_SECURITY_CHANGE.to_le_bytes());
    b[52..56].copy_from_slice(&DIR.to_le_bytes());
    b[56..58].copy_from_slice(&((units.len() * 2) as u16).to_le_bytes());
    b[58..60].copy_from_slice(&60u16.to_le_bytes());
    for (i, u) in units.iter().enumerate() {
        b[60 + i * 2..62 + i * 2].copy_from_slice(&u.to_le_bytes());
    }
    b
}
