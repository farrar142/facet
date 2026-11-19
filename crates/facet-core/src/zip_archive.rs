//! zip 파일의 central directory를 읽는 자체 리더(ADR-0004의 압축 파일 공급자, #422).
//!
//! 열 때 파일 끝의 끝 기록(zip64 끝 기록 포함)을 찾아 central directory만 한 번 읽고, 항목들(차례 그대로)과
//! zip 안 폴더 나무를 메모리에 만든다. 폴더 기록이 없는 중간 폴더(`a/b/c.txt`만 있는 경우)는 폴더 항목으로 만든다.
//! 같은 경로가 둘 이상이면 나무에는 뒤에 나온 기록이 들고 가려진 수를 센다.
//!
//! 이름 인코딩: UTF-8 표시(일반 목적 비트 11)나 Info-ZIP Unicode Path 확장 필드(0x7075, 이름 바이트의 CRC가 맞을
//! 때)가 있는 항목은 UTF-8이다. 나머지는 zip 하나에 한 인코딩이다: 모두 CP949로 엄격하게 풀리면 CP949, 하나라도
//! 실패하면 모두 CP437.
//!
//! 오류는 값이다: zip이 아니거나 잘렸거나 위치가 틀린 파일은 [`ArchiveError`]이고, central directory 중간이
//! 깨졌으면 그 앞 항목만으로 목록을 만들고 [`Archive::damaged`]가 참이다. 어떤 바이트에서도 패닉하지 않는다.
//! 화면 문장은 없다(ADR-0007).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{Local, NaiveDate, TimeZone};

/// 끝 기록 서명.
const EOCD_SIGNATURE: u32 = 0x0605_4B50;
/// zip64 끝 기록 위치 표시 서명.
const ZIP64_LOCATOR_SIGNATURE: u32 = 0x0706_4B50;
/// zip64 끝 기록 서명.
const ZIP64_EOCD_SIGNATURE: u32 = 0x0606_4B50;
/// central directory 항목 서명.
const CENTRAL_SIGNATURE: u32 = 0x0201_4B50;
/// 끝 기록의 고정 길이.
const EOCD_LEN: u64 = 22;
/// zip64 끝 기록 위치 표시의 길이.
const ZIP64_LOCATOR_LEN: u64 = 20;
/// zip64 끝 기록의 고정 부분 길이.
const ZIP64_EOCD_LEN: usize = 56;
/// central directory 항목의 고정 부분 길이.
const CENTRAL_LEN: usize = 46;
/// 끝 기록 뒤 주석의 최대 길이.
const MAX_COMMENT: u64 = 0xFFFF;

/// 일반 목적 비트 0: 암호화.
const FLAG_ENCRYPTED: u16 = 1;
/// 일반 목적 비트 11: 이름이 UTF-8이다.
const FLAG_UTF8: u16 = 1 << 11;

/// 확장 필드: zip64 크기·자리.
const EXTRA_ZIP64: u16 = 0x0001;
/// 확장 필드: 확장 시각.
const EXTRA_TIMESTAMP: u16 = 0x5455;
/// 확장 필드: Info-ZIP Unicode Path.
const EXTRA_UNICODE_PATH: u16 = 0x7075;

/// 만든 시스템(version made by 위 바이트): 유닉스.
const HOST_UNIX: u16 = 3;
/// 유닉스 파일 종류 비트.
const UNIX_TYPE_MASK: u32 = 0o170000;
/// 유닉스 심볼릭 링크.
const UNIX_SYMLINK: u32 = 0o120000;
/// 유닉스 폴더.
const UNIX_FOLDER: u32 = 0o040000;
/// MS-DOS 폴더 속성.
const DOS_FOLDER: u32 = 0x10;

/// 연 zip 파일의 항목과 폴더 나무.
pub struct Archive {
    /// 연 zip 파일(항목을 풀 때 다시 연다).
    path: PathBuf,
    entries: Vec<ArchiveEntry>,
    /// 기록마다 꺼내기에 쓸 값(`entries`와 같은 차례).
    records: Vec<RecordInfo>,
    /// 경로 → 그 경로의 마지막 기록 번호(같은 경로는 뒤 기록, 폴더 나무와 같다).
    by_path: HashMap<String, usize>,
    /// 폴더 경로("" 루트) → 바로 아래 이름 → 항목(같은 경로는 뒤 기록).
    folders: HashMap<String, BTreeMap<String, ArchiveEntry>>,
    encoding: NameEncoding,
    duplicates: usize,
    damaged: bool,
    encrypted: bool,
}

/// central directory 기록 하나의 꺼내기용 값.
#[derive(Debug, Clone, Copy)]
struct RecordInfo {
    /// 로컬 머리의 자리.
    local_header: u64,
    /// 압축 해제한 내용의 CRC-32.
    crc: u32,
}

impl Archive {
    /// zip 파일을 열어 central directory를 읽는다.
    pub fn open(path: &Path) -> Result<Archive, ArchiveError> {
        let mut file = File::open(path).map_err(ArchiveError::Io)?;
        let len = file.metadata().map_err(ArchiveError::Io)?.len();
        let central = read_central_directory(&mut file, len)?;
        let parsed = parse_records(&central.bytes, central.count);
        if parsed.records.is_empty() && (central.count > 0 || parsed.damaged) {
            return Err(ArchiveError::Damaged);
        }
        Ok(Archive::from_records(
            path.to_path_buf(),
            parsed.records,
            parsed.damaged || central.damaged,
        ))
    }

    /// 읽을 수 있었던 central directory 항목(차례 그대로, 같은 경로의 앞 기록 포함).
    pub fn entries(&self) -> &[ArchiveEntry] {
        &self.entries
    }

    /// `folder`(zip 안 경로, `/` 구분, 끝 `/` 없음, 루트는 "")의 바로 아래 항목. 같은 경로는 뒤 기록 하나.
    /// 폴더 기록이 없는 중간 폴더도 폴더 항목으로 든다(크기 0, 시각 `None`). 없는 폴더·파일이면 `None`.
    pub fn children(&self, folder: &str) -> Option<Vec<ArchiveEntry>> {
        let folder = folder.trim_end_matches('/');
        if !folder.is_empty() {
            let (parent, name) = split_parent(folder);
            let entry = self.folders.get(parent)?.get(name)?;
            if !entry.is_folder {
                return None;
            }
        }
        Some(
            self.folders
                .get(folder)
                .map(|children| children.values().cloned().collect())
                .unwrap_or_default(),
        )
    }

    /// UTF-8 표시 없는 이름에 고른 인코딩(그런 이름이 없으면 `Utf8`).
    pub fn encoding(&self) -> NameEncoding {
        self.encoding
    }

    /// 같은 경로가 앞에 또 있어 가려진 기록 수.
    pub fn duplicates(&self) -> usize {
        self.duplicates
    }

    /// central directory 일부를 읽지 못했다.
    pub fn damaged(&self) -> bool {
        self.damaged
    }

    /// 암호화된 항목이 하나라도 있다.
    pub fn encrypted(&self) -> bool {
        self.encrypted
    }

    /// zip 안 파일 `path`(`/` 구분, `\`도 받음)를 풀어 내용을 돌려준다. 같은 경로가 둘 이상이면 뒤 기록(목록에 보이는
    /// 것)이다. 압축 방식은 Stored(0)와 Deflate(8)만 푼다.
    ///
    /// 오류는 값이다: 없는 경로 [`EntryError::NotFound`], 폴더(기록이 없는 중간 폴더 포함) [`EntryError::NotAFile`],
    /// 암호화된 항목 [`EntryError::PasswordNeeded`](암호 풀기는 #428이라 지금은 `password`를 주어도 같다), 그 밖의
    /// 압축 방식 [`EntryError::UnsupportedMethod`], CRC 불일치·실제로 풀린 크기가 선언 크기와 다름·깨진 압축
    /// 데이터·틀린 로컬 머리 [`EntryError::Corrupt`], 파일을 읽지 못함 [`EntryError::Io`].
    pub fn read_entry(&self, path: &str, password: Option<&str>) -> Result<Vec<u8>, EntryError> {
        let index = self.file_index(path)?;
        let mut out = Vec::new();
        let result = self.stream_entry(index, password, &mut |chunk| {
            out.extend_from_slice(chunk);
            Ok(true)
        });
        match result {
            Ok(()) => Ok(out),
            Err(StreamError::Entry(error)) => Err(error),
            Err(StreamError::Write(error)) => Err(EntryError::Io(error.kind())),
            Err(StreamError::Stopped) => Err(EntryError::Io(std::io::ErrorKind::Interrupted)),
        }
    }

    /// 파일 `path`(`/` 구분)의 기록 번호(같은 경로는 뒤 기록). 없으면 `NotFound`, 폴더면 `NotAFile`.
    pub(crate) fn file_index(&self, path: &str) -> Result<usize, EntryError> {
        let path = normalize(path);
        match self.by_path.get(&path) {
            Some(&index) if !self.entries[index].is_folder => Ok(index),
            Some(_) => Err(EntryError::NotAFile),
            None if path.is_empty() || self.folders.contains_key(&path) => {
                Err(EntryError::NotAFile)
            }
            None => Err(EntryError::NotFound),
        }
    }

    /// 기록 `index`의 항목.
    pub(crate) fn entry(&self, index: usize) -> &ArchiveEntry {
        &self.entries[index]
    }

    /// 경로 `path`(`/` 구분)의 항목(같은 경로는 뒤 기록, 기록이 없는 중간 폴더도 폴더 항목). 루트("")나 없으면
    /// `None`.
    pub(crate) fn find(&self, path: &str) -> Option<ArchiveEntry> {
        let path = normalize(path);
        if path.is_empty() {
            return None;
        }
        let (parent, name) = split_parent(&path);
        self.folders.get(parent)?.get(name).cloned()
    }

    /// 기록 `index`의 파일 내용을 덩어리마다 `sink`에 넘기며 푼다(메모리에 다 올리지 않는다). `sink`가 `Ok(false)`를
    /// 돌려주면 그만두고 [`StreamError::Stopped`]다. 넘긴 내용은 끝까지 맞는지(CRC·크기) 확인하기 전의 것이라,
    /// 오류로 끝나면 부르는 쪽이 받은 것을 버린다. 선언 크기를 넘는 순간 곧바로 멈춘다.
    pub(crate) fn stream_entry(
        &self,
        index: usize,
        _password: Option<&str>,
        sink: &mut dyn FnMut(&[u8]) -> std::io::Result<bool>,
    ) -> Result<(), StreamError> {
        let entry = &self.entries[index];
        let record = self.records[index];
        if entry.is_folder {
            return Err(EntryError::NotAFile.into());
        }
        if entry.encrypted {
            return Err(EntryError::PasswordNeeded.into());
        }
        if entry.method != METHOD_STORED && entry.method != METHOD_DEFLATE {
            return Err(EntryError::UnsupportedMethod(entry.method).into());
        }
        let io = |e: std::io::Error| StreamError::Entry(EntryError::Io(e.kind()));
        let mut file = File::open(&self.path).map_err(io)?;
        let len = file.metadata().map_err(io)?.len();
        // 로컬 머리: 서명, 이름·확장 필드 길이 뒤에 데이터가 온다(크기는 central directory 값을 쓴다).
        let header = read_at(&mut file, record.local_header, LOCAL_LEN).map_err(|_| corrupt())?;
        if le32(&header, 0) != Some(LOCAL_SIGNATURE) {
            return Err(corrupt());
        }
        let name_len = u64::from(le16(&header, 26).ok_or_else(corrupt)?);
        let extra_len = u64::from(le16(&header, 28).ok_or_else(corrupt)?);
        let start = record.local_header + LOCAL_LEN as u64 + name_len + extra_len;
        let fits = start
            .checked_add(entry.compressed_size)
            .is_some_and(|end| end <= len);
        if !fits {
            return Err(corrupt());
        }
        file.seek(SeekFrom::Start(start)).map_err(io)?;
        let mut input = Limited {
            file,
            left: entry.compressed_size,
        };
        let mut check = Check {
            crc: Crc32::new(),
            written: 0,
            size: entry.size,
        };
        if entry.method == METHOD_STORED {
            let mut buf = vec![0u8; CHUNK];
            loop {
                let n = input.read(&mut buf).map_err(read_error)?;
                if n == 0 {
                    break;
                }
                check.take(&buf[..n])?;
                if !sink(&buf[..n]).map_err(StreamError::Write)? {
                    return Err(StreamError::Stopped);
                }
            }
        } else {
            inflate(&mut input, &mut check, sink)?;
        }
        if check.written != entry.size || check.crc.finish() != record.crc {
            return Err(corrupt());
        }
        Ok(())
    }

    fn from_records(path: PathBuf, raw: Vec<RawRecord>, damaged: bool) -> Archive {
        // zip 하나에 한 인코딩: UTF-8이 아닌 이름이 모두 CP949로 풀리면 CP949, 아니면 CP437.
        let mut legacy = raw.iter().filter(|r| r.utf8_name.is_none()).peekable();
        let encoding = if legacy.peek().is_none() {
            NameEncoding::Utf8
        } else if legacy.all(|r| decode_cp949(&r.name).is_some()) {
            NameEncoding::Cp949
        } else {
            NameEncoding::Cp437
        };

        let mut entries = Vec::with_capacity(raw.len());
        let mut records = Vec::with_capacity(raw.len());
        let mut encrypted = false;
        for r in raw {
            let name = match r.utf8_name {
                Some(name) => name,
                None => match encoding {
                    NameEncoding::Cp949 => decode_cp949(&r.name).unwrap_or_default(),
                    _ => decode_cp437(&r.name),
                },
            };
            let unix_mode = if r.version_made_by >> 8 == HOST_UNIX {
                r.external_attrs >> 16
            } else {
                0
            };
            let symlink = unix_mode & UNIX_TYPE_MASK == UNIX_SYMLINK;
            let is_folder = !symlink
                && (name.ends_with('/')
                    || name.ends_with('\\')
                    || unix_mode & UNIX_TYPE_MASK == UNIX_FOLDER
                    || (r.external_attrs & DOS_FOLDER != 0 && r.size == 0));
            let path = normalize(&name);
            let last = path.rsplit('/').next().unwrap_or("").to_string();
            let is_encrypted = r.flags & FLAG_ENCRYPTED != 0;
            encrypted |= is_encrypted;
            entries.push(ArchiveEntry {
                raw_name: name,
                path,
                name: last,
                is_folder,
                size: r.size,
                compressed_size: r.compressed_size,
                modified: r.modified,
                method: r.method,
                encrypted: is_encrypted,
                symlink,
            });
            records.push(RecordInfo {
                local_header: r.local_header,
                crc: r.crc,
            });
        }

        let (folders, duplicates) = build_tree(&entries);
        let by_path = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.path.is_empty())
            .map(|(i, e)| (e.path.clone(), i))
            .collect();
        Archive {
            path,
            entries,
            records,
            by_path,
            folders,
            encoding,
            duplicates,
            damaged,
            encrypted,
        }
    }
}

/// central directory 항목 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    /// zip에 기록된 이름 그대로(`..`·앞 `/`·`\`·드라이브 글자 포함, 풀어 쓸 때 안전한지 가리는 데 쓴다).
    pub raw_name: String,
    /// zip 안 전체 경로, `/` 구분, 끝 `/` 없음.
    pub path: String,
    /// 마지막 조각.
    pub name: String,
    /// 폴더인가.
    pub is_folder: bool,
    /// 압축 해제 크기.
    pub size: u64,
    /// 압축된 크기.
    pub compressed_size: u64,
    /// 수정 시각(확장 시각 필드, 없으면 DOS 시각을 지역 시각으로). 모르면 `None`.
    pub modified: Option<SystemTime>,
    /// 압축 방식 번호.
    pub method: u16,
    /// 암호화되었는가.
    pub encrypted: bool,
    /// 심볼릭 링크인가.
    pub symlink: bool,
}

/// 이름 인코딩.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameEncoding {
    /// UTF-8.
    Utf8,
    /// CP949(한국어 Windows).
    Cp949,
    /// CP437(IBM PC).
    Cp437,
}

/// zip을 열지 못한 까닭.
#[derive(Debug)]
pub enum ArchiveError {
    /// 끝 기록이 없다(zip이 아니거나 잘렸다).
    NotZip,
    /// 끝 기록은 있으나 central directory를 읽을 수 없다.
    Damaged,
    /// 파일을 읽지 못했다.
    Io(std::io::Error),
}

/// zip 항목을 풀지 못한 까닭([`Archive::read_entry`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryError {
    /// 그 경로의 항목이 없다.
    NotFound,
    /// 파일이 아니다(폴더).
    NotAFile,
    /// 암호화된 항목이라 암호가 있어야 한다.
    PasswordNeeded,
    /// 암호가 틀렸다(#428).
    WrongPassword,
    /// 풀 수 없는 압축 방식(값은 방식 번호). Stored(0)·Deflate(8)만 푼다.
    UnsupportedMethod(u16),
    /// 데이터가 선언과 맞지 않는다: CRC 불일치, 실제로 풀린 크기가 선언 크기와 다름, 깨진 압축 데이터, 틀린 로컬 머리.
    Corrupt,
    /// zip 파일을 읽지 못했다.
    Io(std::io::ErrorKind),
}

/// 항목을 덩어리로 풀다 멈춘 까닭([`Archive::stream_entry`]).
#[derive(Debug)]
pub(crate) enum StreamError {
    /// 항목을 풀지 못했다.
    Entry(EntryError),
    /// 받는 쪽이 쓰지 못했다.
    Write(std::io::Error),
    /// 받는 쪽이 그만두게 했다(취소).
    Stopped,
}

impl From<EntryError> for StreamError {
    fn from(error: EntryError) -> Self {
        StreamError::Entry(error)
    }
}

fn corrupt() -> StreamError {
    StreamError::Entry(EntryError::Corrupt)
}

/// 압축된 데이터를 읽다 난 오류: 선언한 압축된 크기보다 파일이 짧으면 손상이다.
fn read_error(error: std::io::Error) -> StreamError {
    match error.kind() {
        std::io::ErrorKind::UnexpectedEof => corrupt(),
        kind => StreamError::Entry(EntryError::Io(kind)),
    }
}

/// 로컬 머리 서명.
const LOCAL_SIGNATURE: u32 = 0x0403_4B50;
/// 로컬 머리의 고정 부분 길이.
const LOCAL_LEN: usize = 30;
/// 압축 방식: 그대로 저장.
const METHOD_STORED: u16 = 0;
/// 압축 방식: Deflate.
const METHOD_DEFLATE: u16 = 8;
/// 풀 때 한 번에 읽고 쓰는 덩어리 크기.
const CHUNK: usize = 256 * 1024;

/// 데이터 자리부터 압축된 크기만큼만 읽는다.
struct Limited {
    file: File,
    left: u64,
}

impl Read for Limited {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let want = usize::try_from(self.left)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        if want == 0 {
            return Ok(0);
        }
        let n = self.file.read(&mut buf[..want])?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        self.left -= n as u64;
        Ok(n)
    }
}

/// 풀린 내용의 CRC와 크기를 세고 선언 크기를 넘으면 곧바로 멈춘다.
struct Check {
    crc: Crc32,
    written: u64,
    size: u64,
}

impl Check {
    fn take(&mut self, chunk: &[u8]) -> Result<(), StreamError> {
        self.written += chunk.len() as u64;
        if self.written > self.size {
            return Err(corrupt());
        }
        self.crc.update(chunk);
        Ok(())
    }
}

/// Deflate 데이터를 덩어리로 푼다.
fn inflate(
    input: &mut Limited,
    check: &mut Check,
    sink: &mut dyn FnMut(&[u8]) -> std::io::Result<bool>,
) -> Result<(), StreamError> {
    use miniz_oxide::inflate::stream::{InflateState, inflate};
    use miniz_oxide::{DataFormat, MZError, MZFlush, MZStatus};

    let mut state = InflateState::new_boxed(DataFormat::Raw);
    let mut buf = vec![0u8; CHUNK];
    let mut out = vec![0u8; CHUNK];
    let (mut start, mut end) = (0usize, 0usize);
    loop {
        if start == end && input.left > 0 {
            start = 0;
            end = input.read(&mut buf).map_err(read_error)?;
        }
        let result = inflate(&mut state, &buf[start..end], &mut out, MZFlush::None);
        start += result.bytes_consumed;
        let written = &out[..result.bytes_written];
        if !written.is_empty() {
            check.take(written)?;
            if !sink(written).map_err(StreamError::Write)? {
                return Err(StreamError::Stopped);
            }
        }
        match result.status {
            Ok(MZStatus::StreamEnd) => return Ok(()),
            Ok(_) | Err(MZError::Buf) => {
                let stalled = result.bytes_consumed == 0 && result.bytes_written == 0;
                // 더 줄 입력이 없거나, 입력이 남았는데도 나아가지 않는다: 데이터가 끝나기 전에 잘렸거나 깨졌다.
                if stalled && (start < end || input.left == 0) {
                    return Err(corrupt());
                }
            }
            Err(_) => return Err(corrupt()),
        }
    }
}

// ---- 끝 기록과 central directory 읽기 ----

/// 읽은 central directory 바이트와 선언된 항목 수.
struct Central {
    bytes: Vec<u8>,
    count: u64,
    /// 선언된 크기만큼 읽지 못했다.
    damaged: bool,
}

fn le16(bytes: &[u8], at: usize) -> Option<u16> {
    let b = bytes.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

fn le32(bytes: &[u8], at: usize) -> Option<u32> {
    let b = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn le64(bytes: &[u8], at: usize) -> Option<u64> {
    let b = bytes.get(at..at.checked_add(8)?)?;
    let mut a = [0u8; 8];
    a.copy_from_slice(b);
    Some(u64::from_le_bytes(a))
}

fn read_at(file: &mut File, offset: u64, len: usize) -> Result<Vec<u8>, ArchiveError> {
    file.seek(SeekFrom::Start(offset))
        .map_err(ArchiveError::Io)?;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf).map_err(ArchiveError::Io)?;
    Ok(buf)
}

fn read_central_directory(file: &mut File, len: u64) -> Result<Central, ArchiveError> {
    if len < EOCD_LEN {
        return Err(ArchiveError::NotZip);
    }
    // 끝 기록은 파일 끝에서 22바이트 + 주석(최대 65535) 안에 있다.
    let tail_len = len.min(EOCD_LEN + MAX_COMMENT);
    let tail_start = len - tail_len;
    let tail = read_at(file, tail_start, tail_len as usize)?;
    let eocd = find_eocd(&tail).ok_or(ArchiveError::NotZip)?;
    let eocd_pos = tail_start + eocd as u64;

    let mut count = u64::from(le16(&tail, eocd + 10).ok_or(ArchiveError::NotZip)?);
    let mut size = u64::from(le32(&tail, eocd + 12).ok_or(ArchiveError::NotZip)?);
    let mut offset = u64::from(le32(&tail, eocd + 16).ok_or(ArchiveError::NotZip)?);
    // central directory가 끝날 수 있는 가장 뒤 자리(끝 기록이나 zip64 끝 기록 앞).
    let mut end = eocd_pos;

    // zip64 끝 기록 위치 표시가 끝 기록 바로 앞에 있으면 zip64 값을 쓴다.
    if eocd_pos >= ZIP64_LOCATOR_LEN {
        let locator_pos = eocd_pos - ZIP64_LOCATOR_LEN;
        let locator = read_at(file, locator_pos, ZIP64_LOCATOR_LEN as usize)?;
        if le32(&locator, 0) == Some(ZIP64_LOCATOR_SIGNATURE) {
            let record_pos = le64(&locator, 8).ok_or(ArchiveError::Damaged)?;
            let fits = record_pos
                .checked_add(ZIP64_EOCD_LEN as u64)
                .is_some_and(|e| e <= locator_pos);
            if !fits {
                return Err(ArchiveError::Damaged);
            }
            let record = read_at(file, record_pos, ZIP64_EOCD_LEN)?;
            if le32(&record, 0) != Some(ZIP64_EOCD_SIGNATURE) {
                return Err(ArchiveError::Damaged);
            }
            count = le64(&record, 32).ok_or(ArchiveError::Damaged)?;
            size = le64(&record, 40).ok_or(ArchiveError::Damaged)?;
            offset = le64(&record, 48).ok_or(ArchiveError::Damaged)?;
            end = record_pos;
        }
    }

    if offset > end {
        return Err(ArchiveError::Damaged);
    }
    // 선언된 크기가 끝 기록을 넘으면 읽을 수 있는 데까지만 읽고 손상으로 친다.
    let available = end - offset;
    let damaged = size > available;
    let read_len = size.min(available);
    let read_len = usize::try_from(read_len).map_err(|_| ArchiveError::Damaged)?;
    let bytes = read_at(file, offset, read_len)?;
    Ok(Central {
        bytes,
        count,
        damaged,
    })
}

/// 꼬리 바이트에서 끝 기록의 자리를 찾는다. 주석 길이가 파일 끝과 맞는 것을 먼저, 없으면 가장 뒤의 서명.
fn find_eocd(tail: &[u8]) -> Option<usize> {
    let last = tail.len().checked_sub(EOCD_LEN as usize)?;
    let candidates = (0..=last)
        .rev()
        .filter(|&i| le32(tail, i) == Some(EOCD_SIGNATURE));
    let mut fallback = None;
    for i in candidates {
        let comment = usize::from(le16(tail, i + 20)?);
        if i + EOCD_LEN as usize + comment == tail.len() {
            return Some(i);
        }
        fallback.get_or_insert(i);
    }
    fallback
}

/// central directory 기록 하나의 날 값(이름 인코딩을 고르기 전).
struct RawRecord {
    version_made_by: u16,
    flags: u16,
    method: u16,
    crc: u32,
    compressed_size: u64,
    size: u64,
    local_header: u64,
    external_attrs: u32,
    /// 이름 바이트.
    name: Vec<u8>,
    /// UTF-8 표시나 Unicode Path 확장 필드로 정해진 이름.
    utf8_name: Option<String>,
    modified: Option<SystemTime>,
}

struct Parsed {
    records: Vec<RawRecord>,
    damaged: bool,
}

/// central directory 바이트에서 기록을 차례로 읽는다. 깨진 기록을 만나면 거기서 멈추고 손상으로 친다.
fn parse_records(bytes: &[u8], count: u64) -> Parsed {
    let mut records = Vec::new();
    let mut at = 0usize;
    let mut damaged = false;
    while (records.len() as u64) < count {
        match parse_record(bytes, at) {
            Some((record, next)) => {
                records.push(record);
                at = next;
            }
            None => {
                damaged = true;
                break;
            }
        }
    }
    Parsed { records, damaged }
}

fn parse_record(bytes: &[u8], at: usize) -> Option<(RawRecord, usize)> {
    if le32(bytes, at)? != CENTRAL_SIGNATURE {
        return None;
    }
    let version_made_by = le16(bytes, at + 4)?;
    let flags = le16(bytes, at + 8)?;
    let method = le16(bytes, at + 10)?;
    let dos_time = le16(bytes, at + 12)?;
    let dos_date = le16(bytes, at + 14)?;
    let crc = le32(bytes, at + 16)?;
    let mut compressed_size = u64::from(le32(bytes, at + 20)?);
    let mut size = u64::from(le32(bytes, at + 24)?);
    let name_len = usize::from(le16(bytes, at + 28)?);
    let extra_len = usize::from(le16(bytes, at + 30)?);
    let comment_len = usize::from(le16(bytes, at + 32)?);
    let external_attrs = le32(bytes, at + 38)?;
    let mut local_header = u64::from(le32(bytes, at + 42)?);

    let name_start = at + CENTRAL_LEN;
    let extra_start = name_start + name_len;
    let next = extra_start + extra_len + comment_len;
    let name = bytes.get(name_start..extra_start)?.to_vec();
    let extra = bytes.get(extra_start..extra_start + extra_len)?;
    bytes.get(extra_start + extra_len..next)?;

    let mut utf8_name = if flags & FLAG_UTF8 != 0 {
        Some(String::from_utf8_lossy(&name).into_owned())
    } else {
        None
    };
    let mut modified = None;
    for (id, body) in extra_fields(extra) {
        match id {
            EXTRA_ZIP64 => {
                // 32비트 칸이 꽉 찬 값만 차례로 담긴다.
                let mut pos = 0;
                let mut take = |slot: &mut u64, full: bool| {
                    if full {
                        if let Some(v) = le64(body, pos) {
                            *slot = v;
                        }
                        pos += 8;
                    }
                };
                let size_full = size == 0xFFFF_FFFF;
                let compressed_full = compressed_size == 0xFFFF_FFFF;
                let offset_full = local_header == 0xFFFF_FFFF;
                take(&mut size, size_full);
                take(&mut compressed_size, compressed_full);
                take(&mut local_header, offset_full);
            }
            EXTRA_TIMESTAMP => {
                // 첫 바이트가 표시, 비트 0이면 수정 시각(유닉스 초, UTC)이 뒤따른다.
                if body.first().is_some_and(|f| f & 1 != 0)
                    && let Some(secs) = le32(body, 1)
                {
                    modified = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(u64::from(secs)));
                }
            }
            EXTRA_UNICODE_PATH => {
                // 판 1, 이름 바이트의 CRC-32, UTF-8 이름. CRC가 맞을 때만 쓴다.
                if utf8_name.is_none()
                    && body.first() == Some(&1)
                    && le32(body, 1) == Some(crc32(&name))
                    && let Some(text) = body.get(5..)
                    && let Ok(text) = std::str::from_utf8(text)
                {
                    utf8_name = Some(text.to_string());
                }
            }
            _ => {}
        }
    }
    if modified.is_none() {
        modified = dos_time_to_system(dos_date, dos_time);
    }

    Some((
        RawRecord {
            version_made_by,
            flags,
            method,
            crc,
            compressed_size,
            size,
            local_header,
            external_attrs,
            name,
            utf8_name,
            modified,
        },
        next,
    ))
}

/// 확장 필드들을 (ID, 내용)으로 나눈다. 길이가 넘치는 필드에서 멈춘다.
fn extra_fields(extra: &[u8]) -> impl Iterator<Item = (u16, &[u8])> {
    let mut at = 0usize;
    std::iter::from_fn(move || {
        let id = le16(extra, at)?;
        let len = usize::from(le16(extra, at + 2)?);
        let body = extra.get(at + 4..at + 4 + len)?;
        at += 4 + len;
        Some((id, body))
    })
}

/// DOS 날짜·시각(지역 시각)을 시각으로 바꾼다. 틀린 값이면 `None`.
fn dos_time_to_system(date: u16, time: u16) -> Option<SystemTime> {
    let year = 1980 + i32::from(date >> 9);
    let month = u32::from((date >> 5) & 0x0F);
    let day = u32::from(date & 0x1F);
    let hour = u32::from(time >> 11);
    let minute = u32::from((time >> 5) & 0x3F);
    let second = u32::from(time & 0x1F) * 2;
    let naive = NaiveDate::from_ymd_opt(year, month, day)?.and_hms_opt(hour, minute, second)?;
    let local = Local.from_local_datetime(&naive).earliest()?;
    Some(local.into())
}

/// zip의 CRC-32.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = Crc32::new();
    crc.update(data);
    crc.finish()
}

/// 바이트 하나씩의 CRC-32 표.
const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
};

/// 덩어리마다 이어 세는 CRC-32.
pub(crate) struct Crc32(u32);

impl Crc32 {
    pub(crate) fn new() -> Self {
        Crc32(0xFFFF_FFFF)
    }

    pub(crate) fn update(&mut self, data: &[u8]) {
        let mut crc = self.0;
        for &b in data {
            crc = CRC_TABLE[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8);
        }
        self.0 = crc;
    }

    pub(crate) fn finish(&self) -> u32 {
        !self.0
    }
}

// ---- 이름 인코딩 ----

/// CP949로 엄격하게 푼다(잘못된 바이트가 하나라도 있으면 `None`).
fn decode_cp949(bytes: &[u8]) -> Option<String> {
    encoding_rs::EUC_KR
        .decode_without_bom_handling_and_without_replacement(bytes)
        .map(|s| s.into_owned())
}

/// CP437 위쪽 반(0x80..=0xFF)의 글자.
const CP437_HIGH: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', //
    'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ', //
    'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»', //
    '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐', //
    '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧', //
    '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐', '▀', //
    'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩', //
    '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{A0}',
];

/// CP437로 푼다(모든 바이트가 글자다).
fn decode_cp437(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| {
            if b < 0x80 {
                char::from(b)
            } else {
                CP437_HIGH[usize::from(b - 0x80)]
            }
        })
        .collect()
}

// ---- 폴더 나무 ----

/// zip 안 이름을 `/` 구분, 앞뒤 `/` 없는 경로로 바꾼다(`\`는 `/`로, 빈 조각과 `.`은 뺌).
fn normalize(name: &str) -> String {
    name.split(['/', '\\'])
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>()
        .join("/")
}

/// 경로를 (상위 폴더, 마지막 조각)으로 나눈다. 루트 바로 아래면 상위는 "".
fn split_parent(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => ("", path),
    }
}

/// 기록들로 폴더 나무를 만들고 가려진 기록 수를 센다.
fn build_tree(
    entries: &[ArchiveEntry],
) -> (HashMap<String, BTreeMap<String, ArchiveEntry>>, usize) {
    let mut folders: HashMap<String, BTreeMap<String, ArchiveEntry>> = HashMap::new();
    folders.insert(String::new(), BTreeMap::new());
    let mut seen: HashSet<&str> = HashSet::new();
    let mut duplicates = 0;
    for entry in entries {
        if entry.path.is_empty() {
            continue;
        }
        if !seen.insert(entry.path.as_str()) {
            duplicates += 1;
        }
        ensure_ancestors(&mut folders, &entry.path);
        let (parent, name) = split_parent(&entry.path);
        if let Some(children) = folders.get_mut(parent) {
            children.insert(name.to_string(), entry.clone());
        }
        if entry.is_folder {
            folders.entry(entry.path.clone()).or_default();
        }
    }
    (folders, duplicates)
}

/// `path`의 모든 상위 폴더가 나무에 폴더로 있게 한다(기록이 없으면 만든다).
fn ensure_ancestors(folders: &mut HashMap<String, BTreeMap<String, ArchiveEntry>>, path: &str) {
    let mut end = 0;
    while let Some(i) = path[end..].find('/') {
        let folder = &path[..end + i];
        let (parent, name) = split_parent(folder);
        let children = folders.entry(parent.to_string()).or_default();
        let is_folder = children.get(name).is_some_and(|e| e.is_folder);
        if !is_folder {
            children.insert(
                name.to_string(),
                ArchiveEntry {
                    raw_name: folder.to_string(),
                    path: folder.to_string(),
                    name: name.to_string(),
                    is_folder: true,
                    size: 0,
                    compressed_size: 0,
                    modified: None,
                    method: 0,
                    encrypted: false,
                    symlink: false,
                },
            );
        }
        folders.entry(folder.to_string()).or_default();
        end += i + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cp437_decodes_the_high_half() {
        assert_eq!(decode_cp437(b"caf\x82"), "café");
        assert_eq!(decode_cp437(b"\xFF"), "\u{A0}");
    }

    #[test]
    fn crc32_matches_the_known_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn normalize_drops_empty_and_dot_parts() {
        assert_eq!(normalize("/a//./b\\c/"), "a/b/c");
        assert_eq!(normalize("/"), "");
    }

    #[test]
    fn invalid_dos_dates_have_no_time() {
        assert_eq!(dos_time_to_system(0, 0), None, "달 0");
    }
}
