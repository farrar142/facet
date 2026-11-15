//! 이진 형식의 기본 조각: 정수(리틀 엔디언), 문자열, 있음/없음, 목록.
//!
//! 읽는 쪽은 남은 바이트를 넘어 읽지 않고, 목록 개수나 문자열 길이를 믿고 메모리를 미리
//! 잡지 않는다. 틀린 바이트는 모두 [`DecodeError`]가 된다.

use std::fmt;

/// 조건 트리를 읽을 때 허용하는 가장 깊은 중첩. 이보다 깊으면 스택을 지키려고 거절한다.
pub const MAX_DEPTH: usize = 64;

/// 메시지 본문을 읽지 못한 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// 본문이 메시지 중간에서 끝났다(길이·개수가 남은 바이트보다 크다).
    UnexpectedEnd,
    /// 알 수 없는 종류 번호. `what`은 무엇의 종류인지다.
    UnknownTag { what: &'static str, tag: u8 },
    /// 문자열이 UTF-8이 아니다.
    InvalidUtf8,
    /// 조건 트리가 [`MAX_DEPTH`]보다 깊다.
    TooDeep,
    /// 메시지를 다 읽었는데 바이트가 남았다.
    TrailingBytes,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEnd => f.write_str("메시지가 중간에서 끝났습니다"),
            Self::UnknownTag { what, tag } => write!(f, "알 수 없는 {what} 종류 번호 {tag}"),
            Self::InvalidUtf8 => f.write_str("문자열이 UTF-8이 아닙니다"),
            Self::TooDeep => write!(f, "조건 트리가 {MAX_DEPTH}단계보다 깊습니다"),
            Self::TrailingBytes => f.write_str("메시지 뒤에 남은 바이트가 있습니다"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// 쓰는 쪽.
pub(crate) struct Writer {
    pub(crate) out: Vec<u8>,
}

impl Writer {
    pub(crate) fn new() -> Self {
        Self { out: Vec::new() }
    }

    pub(crate) fn u8(&mut self, v: u8) {
        self.out.push(v);
    }

    pub(crate) fn u16(&mut self, v: u16) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    pub(crate) fn u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    pub(crate) fn u64(&mut self, v: u64) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    pub(crate) fn bool(&mut self, v: bool) {
        self.u8(v as u8);
    }

    /// 길이를 32비트로 담는다. 4GiB를 넘는 길이는 프레임 최대 크기를 한참 넘으므로
    /// 잘라 담아도 프레임을 쓸 때 거절된다.
    fn len(&mut self, n: usize) {
        self.u32(u32::try_from(n).unwrap_or(u32::MAX));
    }

    pub(crate) fn str(&mut self, s: &str) {
        self.len(s.len());
        self.out.extend_from_slice(s.as_bytes());
    }

    pub(crate) fn opt_u64(&mut self, v: Option<u64>) {
        match v {
            None => self.u8(0),
            Some(v) => {
                self.u8(1);
                self.u64(v);
            }
        }
    }

    pub(crate) fn list<T>(&mut self, items: &[T], mut each: impl FnMut(&mut Self, &T)) {
        self.len(items.len());
        for item in items {
            each(self, item);
        }
    }
}

/// 읽는 쪽. 남은 바이트 조각을 앞에서부터 줄여 간다.
pub(crate) struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { rest: bytes }
    }

    /// 다 읽었는지 확인한다.
    pub(crate) fn finish(self) -> Result<(), DecodeError> {
        if self.rest.is_empty() {
            Ok(())
        } else {
            Err(DecodeError::TrailingBytes)
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if n > self.rest.len() {
            return Err(DecodeError::UnexpectedEnd);
        }
        let (head, tail) = self.rest.split_at(n);
        self.rest = tail;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let mut a = [0; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.array::<1>()?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub(crate) fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    pub(crate) fn bool(&mut self) -> Result<bool, DecodeError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            tag => Err(DecodeError::UnknownTag {
                what: "참/거짓",
                tag,
            }),
        }
    }

    pub(crate) fn str(&mut self) -> Result<String, DecodeError> {
        let n = self.u32()? as usize;
        let bytes = self.take(n)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| DecodeError::InvalidUtf8)
    }

    pub(crate) fn opt_u64(&mut self) -> Result<Option<u64>, DecodeError> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.u64()?)),
            tag => Err(DecodeError::UnknownTag {
                what: "있음/없음",
                tag,
            }),
        }
    }

    /// 목록을 읽는다. 개수만큼 미리 잡지 않고 하나씩 읽어 붙인다. 항목은 적어도 1바이트이므로
    /// 개수가 남은 바이트보다 크면 읽기 전에 거절한다.
    pub(crate) fn list<T>(
        &mut self,
        mut each: impl FnMut(&mut Self) -> Result<T, DecodeError>,
    ) -> Result<Vec<T>, DecodeError> {
        let n = self.u32()? as usize;
        if n > self.rest.len() {
            return Err(DecodeError::UnexpectedEnd);
        }
        let mut items = Vec::new();
        for _ in 0..n {
            items.push(each(self)?);
        }
        Ok(items)
    }
}
