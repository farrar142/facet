//! 이름 있는 파이프의 클라이언트 쪽: 겹친 입출력(overlapped)으로 열어 읽기와 쓰기를 서로 다른 스레드에서 동시에 한다.
//!
//! std의 파일 열기로 연 파이프(동기 핸들)는 커널이 한 핸들의 입출력을 차례로 묶어, 한 스레드가 응답을 기다리며
//! 읽는 동안 다른 스레드가 새 요청을 쓰지 못한다. 코어의 인덱서 서비스 연결은 응답을 기다리는 동안 새 질의를 보내
//! 앞선 질의를 취소해야 하므로 겹친 입출력으로 연다(ADR-0002: `unsafe`는 이 크레이트에).
//!
//! - [`connect`]: 파이프에 붙는다. 그 이름의 파이프가 없으면 곧바로 [`ConnectError::NotFound`], 인스턴스가 모두
//!   바쁘면 정한 시간까지 기다리고 그래도 바쁘면 [`ConnectError::Busy`].
//! - [`PipeClient::split`]: 읽는 쪽 [`PipeReader`]·쓰는 쪽 [`PipeWriter`]·닫개 [`PipeCloser`]로 나눈다. 저마다 다른
//!   스레드로 보낼 수 있다.
//! - 읽기는 마감 시각([`PipeReader::set_deadline`])을 넘으면 [`io::ErrorKind::TimedOut`]이다(그 읽기는 취소되어 데이터를
//!   잃지 않는다. 다만 프레임 중간에서 끊겼으면 그 연결은 버린다).
//! - [`PipeCloser::close`]를 부르면 기다리던·앞으로의 읽기·쓰기가 [`io::ErrorKind::ConnectionAborted`]로 끝난다. 다른
//!   스레드에서 읽기를 기다리는 일꾼을 깨워 끝낼 때 쓴다.
//! - 상대가 끊으면 읽기는 0바이트(EOF)다.
//!
//! Windows가 아니면 [`connect`]는 늘 [`ConnectError::Other`]([`io::ErrorKind::Unsupported`])다.

use std::fmt;
use std::io;
use std::time::{Duration, Instant};

/// 파이프에 붙지 못한 까닭.
#[derive(Debug)]
pub enum ConnectError {
    /// 그 이름의 파이프가 없다(서버가 떠 있지 않다).
    NotFound,
    /// 파이프는 있지만 정한 시간 안에 받을 인스턴스가 나지 않았다.
    Busy,
    /// 그 밖의 오류(권한 없음, 지원하지 않는 환경 등).
    Other(io::Error),
}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConnectError::NotFound => f.write_str("파이프가 없습니다"),
            ConnectError::Busy => f.write_str("파이프가 바쁩니다"),
            ConnectError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ConnectError {}

/// 붙은 파이프 하나. [`PipeClient::split`]으로 나눠 쓴다.
pub struct PipeClient(imp::Client);

/// 파이프의 읽는 쪽.
pub struct PipeReader(imp::Reader);

/// 파이프의 쓰는 쪽.
pub struct PipeWriter(imp::Writer);

/// 파이프를 닫는 손잡이. 복제해 여러 곳에 둘 수 있다.
#[derive(Clone)]
pub struct PipeCloser(imp::Closer);

/// `name`(예: `\\.\pipe\facet-indexer`)의 파이프에 `access` 권한(`CreateFileW`의 접근 마스크)으로 붙는다.
///
/// 서버는 클라이언트를 확인만 하고 가장할 수 없다(`SECURITY_IDENTIFICATION`). 인스턴스가 모두 바쁘면 `timeout`까지
/// 기다린다. 부르는 스레드를 막으므로 UI 스레드에서 부르지 않는다.
pub fn connect(name: &str, access: u32, timeout: Duration) -> Result<PipeClient, ConnectError> {
    imp::connect(name, access, Instant::now() + timeout).map(PipeClient)
}

impl PipeClient {
    /// 읽는 쪽·쓰는 쪽·닫개로 나눈다.
    pub fn split(self) -> io::Result<(PipeReader, PipeWriter, PipeCloser)> {
        let (r, w, c) = imp::split(self.0)?;
        Ok((PipeReader(r), PipeWriter(w), PipeCloser(c)))
    }
}

impl PipeReader {
    /// 읽기의 마감 시각. `None`이면 끝없이 기다린다. 넘으면 읽기가 [`io::ErrorKind::TimedOut`]이다.
    pub fn set_deadline(&mut self, deadline: Option<Instant>) {
        imp::set_deadline(&mut self.0, deadline);
    }
}

impl io::Read for PipeReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        imp::read(&mut self.0, buf)
    }
}

impl io::Write for PipeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        imp::write(&mut self.0, buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl PipeCloser {
    /// 파이프를 닫는다. 기다리던·앞으로의 읽기·쓰기가 [`io::ErrorKind::ConnectionAborted`]로 끝난다. 여러 번 불러도 된다.
    pub fn close(&self) {
        imp::close(&self.0);
    }
}

/// 이 프로세스가 띄운 자식 하나만 붙게 하는 사적인 파이프의 서버 쪽(셸 확장 도우미 연결이 쓴다).
///
/// 인스턴스는 하나뿐이고 첫 인스턴스로 만든다(같은 이름이 이미 있으면 [`io::ErrorKind::AddrInUse`]). 보안 설명자는
/// 소유자·유일한 허용 항목이 현재 사용자 SID이고 NETWORK를 거부하며 무결성 수준 강제 레이블(위로 쓰기 금지)을 건다.
/// 원격 클라이언트는 거부한다. 받은 연결의 서버 쪽 끝은 [`PipeClient`]라 [`PipeClient::split`]으로 나눠 쓴다.
/// Windows가 아니면 만들 수 없다([`io::ErrorKind::Unsupported`]).
pub mod private {
    use std::io;
    use std::time::Duration;

    use super::PipeClient;

    /// 연결 하나를 받을 파이프. 버리면(받기 전이면) 그 이름의 파이프가 사라진다.
    pub struct PrivatePipe(imp::PrivatePipe);

    impl PrivatePipe {
        /// `name`(예: `\\.\pipe\...`)의 파이프를 만든다.
        pub fn create(name: &str) -> io::Result<Self> {
            imp::create(name).map(PrivatePipe)
        }

        /// 연결 하나를 `timeout`까지 기다린다. 오지 않으면 `None`(다시 기다릴 수 있다). 받은 뒤 다시 부르면 오류다.
        pub fn accept(&self, timeout: Duration) -> io::Result<Option<PipeClient>> {
            Ok(imp::accept(&self.0, timeout)?.map(PipeClient))
        }
    }

    #[cfg(windows)]
    use crate::windows_pipe_client::private as imp;

    #[cfg(not(windows))]
    mod imp {
        use std::io;
        use std::time::Duration;

        pub enum PrivatePipe {}

        pub fn create(_: &str) -> io::Result<PrivatePipe> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "이름 있는 파이프는 Windows에서만 쓸 수 있습니다",
            ))
        }
        pub fn accept(
            p: &PrivatePipe,
            _: Duration,
        ) -> io::Result<Option<super::super::imp::Client>> {
            match *p {}
        }
    }
}

/// 시험 도우미: 같은 프로세스에 가짜 파이프 서버를 띄운다. 앱은 부르지 않는다.
///
/// 받은 연결의 서버 쪽 끝도 [`PipeClient`]라 [`PipeClient::split`]으로 나눠 읽고 쓰며, 그 읽는 쪽·쓰는 쪽을 모두
/// 버리면 연결이 끊긴다(상대의 읽기가 EOF). 보안 설명자는 기본값(만든 사용자)이다.
#[cfg(windows)]
pub mod testing {
    use std::io;
    use std::time::Duration;

    use super::PipeClient;
    use crate::windows_pipe_client::testing as imp;

    /// 이름 있는 파이프의 받는 쪽. 받을 인스턴스를 늘 하나 만들어 두어 클라이언트가 「파이프 없음」을 보지 않는다.
    /// 버리면 기다리던 인스턴스가 닫혀 그 이름의 파이프가 사라진다(이미 받은 연결은 그대로).
    pub struct Listener(imp::Listener);

    impl Listener {
        /// `name`의 파이프를 만든다.
        pub fn bind(name: &str) -> io::Result<Self> {
            imp::bind(name).map(Listener)
        }

        /// 연결 하나를 `timeout`까지 기다린다. 오지 않으면 `None`.
        pub fn accept(&self, timeout: Duration) -> io::Result<Option<PipeClient>> {
            Ok(imp::accept(&self.0, timeout)?.map(PipeClient))
        }
    }
}

#[cfg(windows)]
use crate::windows_pipe_client as imp;

#[cfg(not(windows))]
mod imp {
    use std::io;
    use std::time::Instant;

    use super::ConnectError;

    pub enum Never {}
    pub type Client = Never;
    pub type Reader = Never;
    pub type Writer = Never;
    #[derive(Clone)]
    pub enum Closer {}

    pub fn connect(_: &str, _: u32, _: Instant) -> Result<Client, ConnectError> {
        Err(ConnectError::Other(io::Error::new(
            io::ErrorKind::Unsupported,
            "이름 있는 파이프는 Windows에서만 쓸 수 있습니다",
        )))
    }
    pub fn split(c: Client) -> io::Result<(Reader, Writer, Closer)> {
        match c {}
    }
    pub fn set_deadline(r: &mut Reader, _: Option<Instant>) {
        match *r {}
    }
    pub fn read(r: &mut Reader, _: &mut [u8]) -> io::Result<usize> {
        match *r {}
    }
    pub fn write(w: &mut Writer, _: &[u8]) -> io::Result<usize> {
        match *w {}
    }
    pub fn close(c: &Closer) {
        match *c {}
    }
}
