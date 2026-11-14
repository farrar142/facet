//! 인덱스 디스크 형식: 인덱스와 저널 상태를 파일 하나로 저장하고 다시 읽는다.
//!
//! 파일은 머리와 본문이다. 머리는 24바이트(모두 리틀 엔디언)다.
//!
//! | 위치 | 크기 | 내용 |
//! |---|---|---|
//! | 0 | 8 | 매직 `FACETIDX` |
//! | 8 | 4 | 형식 버전 ([`FORMAT_VERSION`]) |
//! | 12 | 4 | 예약(0) |
//! | 16 | 8 | 본문의 검사 값 |
//!
//! 머리 뒤가 본문이고, 본문 길이는 파일 길이에서 머리를 뺀 값이다. 길이를 따로 적지 않고
//! 본문이 잘리면 검사 값이 틀려 거절된다. 잘려서 머리조차 없는 파일은 [`StoreError::Truncated`]다.
//!
//! 저장은 같은 폴더의 임시 파일에 쓰고 디스크에 내린 뒤 이름을 바꿔치기한다. 저장 도중 꺼져도
//! 앞선 파일이 남는다.

use std::collections::HashMap;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::{DEAD, Index, JournalState, Record, new_stamp};

/// 파일 머리의 매직.
const MAGIC: &[u8; 8] = b"FACETIDX";
/// 지금 읽고 쓰는 형식 버전. 형식이 바뀌면 올린다. 다른 버전의 파일은 거절한다.
pub const FORMAT_VERSION: u32 = 1;
const HEADER_LEN: usize = 24;

/// 저장하거나 읽지 못한 까닭. 읽기 오류는 서비스가 인덱스를 다시 만들 신호다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// 파일을 열거나 읽거나 쓰지 못했다(없는 파일 포함). 운영체제 오류 종류와 설명.
    Io(io::ErrorKind, String),
    /// 파일이 머리보다 짧다.
    Truncated,
    /// 인덱스 파일이 아니다(매직이 다르다).
    BadMagic,
    /// 형식 버전이 이 판과 다르다.
    UnsupportedVersion { found: u32 },
    /// 본문이 검사 값과 맞지 않는다(내용이 깨졌거나 본문이 잘렸다).
    ChecksumMismatch,
    /// 검사 값은 맞지만 내용이 앞뒤가 맞지 않는다.
    Corrupt(&'static str),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(_, why) => write!(f, "인덱스 파일 입출력 오류: {why}"),
            Self::Truncated => f.write_str("인덱스 파일이 잘렸습니다"),
            Self::BadMagic => f.write_str("인덱스 파일이 아닙니다"),
            Self::UnsupportedVersion { found } => write!(
                f,
                "인덱스 파일 형식 버전이 다릅니다(파일 {found}, 이 판 {FORMAT_VERSION})"
            ),
            Self::ChecksumMismatch => f.write_str("인덱스 파일의 검사 값이 틀립니다"),
            Self::Corrupt(why) => write!(f, "인덱스 파일 내용이 깨졌습니다: {why}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        Self::Io(e.kind(), e.to_string())
    }
}

/// 본문 검사 값: 8바이트씩 섞는 64비트 해시. 우연한 깨짐을 잡는 용도이고 보안용이 아니다.
fn checksum(bytes: &[u8]) -> u64 {
    const K: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut h = 0xCBF2_9CE4_8422_2325_u64 ^ bytes.len() as u64;
    let (chunks, remainder) = bytes.as_chunks::<8>();
    for c in chunks {
        h = (h ^ u64::from_le_bytes(*c)).wrapping_mul(K).rotate_left(29);
    }
    let mut tail = [0u8; 8];
    tail[..remainder.len()].copy_from_slice(remainder);
    h = (h ^ u64::from_le_bytes(tail)).wrapping_mul(K);
    h ^ (h >> 32)
}

/// 임시 파일에 `write`로 쓰고 디스크에 내린 뒤 `path`로 바꿔치기한다. 어느 단계든 실패하면
/// 임시 파일을 지우고 `path`는 건드리지 않는다.
fn write_atomic(
    path: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> Result<(), StoreError> {
    let mut tmp_name = path
        .file_name()
        .ok_or(StoreError::Io(
            io::ErrorKind::InvalidInput,
            "파일 이름이 없는 경로입니다".to_owned(),
        ))?
        .to_owned();
    tmp_name.push(".tmp");
    let tmp: PathBuf = path.with_file_name(tmp_name);
    let result = (|| {
        let mut file = File::create(&tmp)?;
        write(&mut file)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], StoreError> {
        if n > self.0.len() {
            return Err(StoreError::Corrupt("본문이 모자랍니다"));
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }
    fn u8(&mut self) -> Result<u8, StoreError> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, StoreError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, StoreError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    /// 개수를 읽는다. 항목 하나가 적어도 `unit`바이트이므로 남은 본문보다 많으면 깨진 것이다.
    fn count(&mut self, unit: usize) -> Result<usize, StoreError> {
        let n =
            usize::try_from(self.u64()?).map_err(|_| StoreError::Corrupt("개수가 너무 큽니다"))?;
        if n.checked_mul(unit).is_none_or(|bytes| bytes > self.0.len()) {
            return Err(StoreError::Corrupt("개수가 본문보다 큽니다"));
        }
        Ok(n)
    }
    fn string(&mut self) -> Result<String, StoreError> {
        let n = self.count(1)?;
        String::from_utf8(self.take(n)?.to_vec())
            .map_err(|_| StoreError::Corrupt("글자가 UTF-8이 아닙니다"))
    }
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_str(out: &mut Vec<u8>, s: &str) {
    put_u64(out, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

impl Index {
    /// 인덱스와 저널 상태를 `path` 파일 하나로 저장한다. 임시 파일에 쓰고 바꿔치기하므로
    /// 저장 도중 실패하거나 꺼져도 앞선 `path` 파일이 그대로 남는다.
    pub fn save(&self, path: &Path) -> Result<(), StoreError> {
        let body = self.encode();
        let mut head = Vec::with_capacity(HEADER_LEN);
        head.extend_from_slice(MAGIC);
        put_u32(&mut head, FORMAT_VERSION);
        put_u32(&mut head, 0);
        put_u64(&mut head, checksum(&body));
        write_atomic(path, |file| {
            file.write_all(&head)?;
            file.write_all(&body)
        })
    }

    /// [`Index::save`]가 쓴 파일을 읽는다. 버전이 다르거나 검사 값이 틀리거나 잘리거나 깨진
    /// 파일은 [`StoreError`]로 거절한다. 서비스는 그 오류를 받으면 인덱스를 다시 만든다.
    pub fn load(path: &Path) -> Result<Self, StoreError> {
        let bytes = fs::read(path)?;
        if bytes.len() < HEADER_LEN {
            return Err(StoreError::Truncated);
        }
        let (head, body) = bytes.split_at(HEADER_LEN);
        if &head[..8] != MAGIC {
            return Err(StoreError::BadMagic);
        }
        let version = u32::from_le_bytes(head[8..12].try_into().unwrap());
        if version != FORMAT_VERSION {
            return Err(StoreError::UnsupportedVersion { found: version });
        }
        if checksum(body) != u64::from_le_bytes(head[16..24].try_into().unwrap()) {
            return Err(StoreError::ChecksumMismatch);
        }
        Self::decode(body)
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            self.records.len() * 40 + self.folded.len() + self.originals.len() + 4096,
        );
        put_str(&mut out, &self.root);
        put_u64(&mut out, self.root_number);
        match self.journal {
            Some(j) => {
                out.push(1);
                put_u64(&mut out, j.journal_id);
                put_u64(&mut out, j.usn as u64);
            }
            None => out.push(0),
        }
        put_u64(&mut out, self.dead as u64);
        put_str(&mut out, &self.folded);
        put_str(&mut out, &self.originals);
        put_u64(&mut out, self.records.len() as u64);
        for r in &self.records {
            put_u64(&mut out, r.number);
            put_u64(&mut out, r.size);
            put_u64(&mut out, r.modified);
            put_u64(&mut out, r.parent);
            put_u32(&mut out, r.name_start);
            put_u32(&mut out, r.attributes);
        }
        put_u64(&mut out, self.original_starts.len() as u64);
        for &(at, start) in &self.original_starts {
            put_u32(&mut out, at);
            put_u32(&mut out, start);
        }
        put_u64(&mut out, self.folders.len() as u64);
        for (&number, &at) in &self.folders {
            put_u64(&mut out, number);
            put_u32(&mut out, at);
        }
        put_u64(&mut out, self.pending_renames.len() as u64);
        for (&number, &parent) in &self.pending_renames {
            put_u64(&mut out, number);
            put_u64(&mut out, parent);
        }
        out
    }

    fn decode(body: &[u8]) -> Result<Self, StoreError> {
        let bad = StoreError::Corrupt;
        let mut r = Reader(body);
        let root = r.string()?;
        if !root.ends_with('\\') {
            return Err(bad("볼륨 루트 경로가 `\\`로 끝나지 않습니다"));
        }
        let root_number = r.u64()?;
        let journal = match r.u8()? {
            0 => None,
            1 => Some(JournalState {
                journal_id: r.u64()?,
                usn: r.u64()? as i64,
            }),
            _ => return Err(bad("저널 표지가 잘못됐습니다")),
        };
        let dead = usize::try_from(r.u64()?).map_err(|_| bad("지운 레코드 수가 너무 큽니다"))?;
        let folded = r.string()?;
        let originals = r.string()?;

        let n = r.count(40)?;
        let mut records = Vec::with_capacity(n);
        for _ in 0..n {
            records.push(Record {
                number: r.u64()?,
                size: r.u64()?,
                modified: r.u64()?,
                parent: r.u64()?,
                name_start: r.u32()?,
                attributes: r.u32()?,
            });
        }
        if records.iter().filter(|rec| rec.parent == DEAD).count() != dead {
            return Err(bad("지운 레코드 수가 맞지 않습니다"));
        }
        let mut previous = 0;
        for rec in &records {
            let start = rec.name_start as usize;
            if start < previous || start >= folded.len() || !folded.is_char_boundary(start) {
                return Err(bad("이름 위치가 잘못됐습니다"));
            }
            previous = start;
        }
        if !records.is_empty() && !folded.ends_with('\0') {
            return Err(bad("접은 이름 버퍼가 끝나지 않았습니다"));
        }

        let n = r.count(8)?;
        let mut original_starts = Vec::with_capacity(n);
        for _ in 0..n {
            let (at, start) = (r.u32()?, r.u32()?);
            if at as usize >= records.len()
                || start as usize > originals.len()
                || !originals.is_char_boundary(start as usize)
            {
                return Err(bad("원래 이름 위치가 잘못됐습니다"));
            }
            original_starts.push((at, start));
        }
        if original_starts
            .windows(2)
            .any(|w| w[0].0 >= w[1].0 || w[0].1 > w[1].1)
        {
            return Err(bad("원래 이름 위치가 순서에 맞지 않습니다"));
        }

        let n = r.count(12)?;
        let mut folders = HashMap::with_capacity(n);
        for _ in 0..n {
            let (number, at) = (r.u64()?, r.u32()?);
            if at as usize >= records.len() {
                return Err(bad("폴더 위치가 잘못됐습니다"));
            }
            folders.insert(number, at);
        }
        let n = r.count(16)?;
        let mut pending_renames = HashMap::with_capacity(n);
        for _ in 0..n {
            pending_renames.insert(r.u64()?, r.u64()?);
        }
        if !r.0.is_empty() {
            return Err(bad("본문 뒤에 남는 바이트가 있습니다"));
        }

        Ok(Self {
            root,
            root_number,
            records,
            folded,
            originals,
            original_starts,
            folders,
            stamp: new_stamp(),
            dead,
            locator: None,
            pending_renames,
            journal,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_write_keeps_previous_file_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.idx");
        fs::write(&path, b"old").unwrap();
        let err = write_atomic(&path, |f| {
            f.write_all(b"new but half")?;
            Err(io::Error::other("디스크가 찼습니다"))
        })
        .unwrap_err();
        assert!(matches!(err, StoreError::Io(..)));
        assert_eq!(fs::read(&path).unwrap(), b"old");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
