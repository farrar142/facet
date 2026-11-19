//! 새 zip으로 묶기 작업(#429): 고른 파일·폴더를 현재 폴더의 새 zip으로 묶는다.
//!
//! 작업 대기열의 작업이라 진행·일시정지·취소·항목별 오류 질문은 복사와 같은 엔진([`Copier`])을 쓴다. zip은 같은
//! 폴더의 임시 이름(`~facet-<pid>-<n>.tmp`)에 다 쓴 뒤 이름을 바꿔 놓으므로 취소·실패해도 반쯤 쓴 zip이 남지 않고,
//! 이름 자리가 겹치면 「이름 (2).zip」, 「(3)」 …으로 비켜 간다.
//!
//! 쓰기: 모든 항목에 UTF-8 표시(일반 목적 비트 11)와 확장 시각 필드(0x5455, 유닉스 초)를 붙이고, 내용이 있는 파일은
//! Deflate로(덩어리마다 흘려 써 파일 크기만큼 메모리를 쓰지 않는다), 빈 파일은 Stored로 쓴다. 빈 폴더도 폴더 기록
//! (`이름/`)으로 담는다. 항목이 6만5천 개를 넘거나 central directory·항목 크기·자리가 4GB 가까이 넘으면 zip64로 쓴다.
//! 파일 내용의 크기·CRC는 다 쓴 뒤 지역 머리글을 되돌아가 채운다.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Datelike, Local, Timelike};
use facet_win::file_ops::{self, FileOpErrorKind};
use miniz_oxide::MZFlush;
use miniz_oxide::MZStatus;
use miniz_oxide::deflate::core::{CompressorOxide, create_comp_flags_from_zip_params};
use miniz_oxide::deflate::stream::deflate;

use super::copy::{Copier, ErrorStep};
use super::{Context, CopyRejection, ItemFailure, JournalSink, Summary};
use crate::history::{Stamp, Step};
use crate::zip_archive::Crc32;

/// 일반 목적 비트 11: 이름이 UTF-8이다.
const FLAG_UTF8: u16 = 1 << 11;
/// 이 크기 이상인 파일은 zip64 머리글로 쓴다(압축하면 커질 수 있는 여유를 둔다).
const ZIP64_FILE_LIMIT: u64 = 0xF000_0000;
/// 4바이트 칸에 담을 수 없을 때 적는 값.
const MAX32: u32 = 0xFFFF_FFFF;
/// 파일을 읽는 덩어리 크기.
const CHUNK: usize = 256 * 1024;

/// 새 zip 작업.
pub(super) struct NewZipJob {
    sources: Vec<PathBuf>,
    folder: PathBuf,
    /// 확장자 `.zip`을 뺀 이름.
    base: String,
    /// 한 일을 적을 곳(작업 기록에 남길 작업만).
    pub(super) journal: Option<JournalSink>,
}

impl NewZipJob {
    pub(super) fn prepare(
        sources: Vec<PathBuf>,
        folder: PathBuf,
        base: String,
    ) -> Result<Self, CopyRejection> {
        if sources.is_empty() {
            return Err(CopyRejection::NoSources);
        }
        let metadata = fs::metadata(&folder).map_err(|_| CopyRejection::TargetMissing)?;
        if !metadata.is_dir() {
            return Err(CopyRejection::TargetNotFolder);
        }
        Ok(NewZipJob {
            sources,
            folder,
            base,
            journal: None,
        })
    }

    pub(super) fn root_count(&self) -> u64 {
        self.sources.len() as u64
    }

    pub(super) fn run(self, context: &Context<'_>) -> (Summary, bool) {
        let mut copier = Copier::new(context, false);
        let totals = Arc::clone(&copier.reporter.totals);
        let control = context.control;

        let (items, failures) = plan(&self.sources);
        copier.summary.failures.extend(failures);
        totals.items.store(items.len() as u64, Ordering::Release);
        totals
            .bytes
            .store(items.iter().map(|item| item.size).sum(), Ordering::Release);
        totals.counted.store(true, Ordering::Release);
        if items.is_empty() {
            return copier.finish();
        }

        let temp = temp_path(&self.folder);
        let file = match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => file,
            Err(error) => {
                copier.summary.failures.push(ItemFailure {
                    source: self.folder.clone(),
                    kind: io_kind(&error),
                });
                return copier.finish();
            }
        };
        let mut writer = ZipWriter::new(file);
        let mut aborted = false;
        let mut base_bytes = 0u64;
        'items: for item in &items {
            if !control.proceed() {
                copier.cancelled = true;
            }
            if copier.cancelled {
                break;
            }
            copier.reporter.current = Some(item.path.clone());
            if item.is_folder {
                if let Err(error) = writer.add_folder(&item.name, item.modified) {
                    copier.summary.failures.push(ItemFailure {
                        source: temp.clone(),
                        kind: io_kind(&error),
                    });
                    aborted = true;
                    break;
                }
            } else {
                loop {
                    let reporter = &mut copier.reporter;
                    let result = writer.add_file(
                        &item.name,
                        &item.path,
                        item.size,
                        item.modified,
                        &mut |read| {
                            reporter.bytes_done = base_bytes + read.min(item.size);
                            reporter.maybe_emit(control);
                            control.proceed()
                        },
                    );
                    match result {
                        Ok(()) => break,
                        Err(WriteError::Cancelled) => {
                            copier.cancelled = true;
                            break 'items;
                        }
                        Err(WriteError::Write(error)) => {
                            copier.summary.failures.push(ItemFailure {
                                source: temp.clone(),
                                kind: io_kind(&error),
                            });
                            aborted = true;
                            break 'items;
                        }
                        Err(WriteError::Read(error)) => {
                            match copier.on_error(&item.path, io_kind(&error)) {
                                ErrorStep::Retry => {}
                                ErrorStep::Skip => break,
                                ErrorStep::Cancel => break 'items,
                            }
                        }
                    }
                }
            }
            base_bytes += item.size;
            copier.reporter.bytes_done = base_bytes;
            copier.reporter.items_done += 1;
            copier.reporter.maybe_emit(control);
        }

        if copier.cancelled || aborted {
            drop(writer);
            let _ = fs::remove_file(&temp);
            return copier.finish();
        }
        if let Err(error) = writer.finish() {
            copier.summary.failures.push(ItemFailure {
                source: temp.clone(),
                kind: io_kind(&error),
            });
            let _ = fs::remove_file(&temp);
            return copier.finish();
        }
        match place(&temp, &self.folder, &self.base) {
            Ok(dest) => {
                if let (Some(journal), Some(stamp)) = (&self.journal, Stamp::read(&dest)) {
                    super::lock(journal).steps.push(Step::CreatedZip {
                        sources: self.sources.clone(),
                        path: dest,
                        stamp,
                    });
                }
            }
            Err(kind) => {
                copier.summary.failures.push(ItemFailure {
                    source: self.folder.clone(),
                    kind,
                });
                let _ = fs::remove_file(&temp);
            }
        }
        copier.finish()
    }
}

/// 임시 zip의 이름: 폴더 안 `~facet-<pid>-<n>.tmp`.
fn temp_path(folder: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    folder.join(format!("~facet-zip-{}-{n}.tmp", std::process::id()))
}

/// 임시 파일을 비어 있는 첫 이름(「이름.zip」, 「이름 (2).zip」 …)으로 옮겨 놓고 그 경로를 돌려준다.
fn place(temp: &Path, folder: &Path, base: &str) -> Result<PathBuf, FileOpErrorKind> {
    for number in 1..=10_000u32 {
        let name = if number == 1 {
            format!("{base}.zip")
        } else {
            format!("{base} ({number}).zip")
        };
        let dest = folder.join(name);
        match file_ops::rename(temp, &dest) {
            Ok(()) => return Ok(dest),
            Err(error) if error.kind() == FileOpErrorKind::TargetExists => {}
            Err(error) => return Err(error.kind()),
        }
    }
    Err(FileOpErrorKind::TargetExists)
}

/// 입출력 오류를 파일 작업 오류 종류로.
fn io_kind(error: &io::Error) -> FileOpErrorKind {
    match error.kind() {
        io::ErrorKind::NotFound => FileOpErrorKind::SourceMissing,
        io::ErrorKind::PermissionDenied => FileOpErrorKind::AccessDenied,
        io::ErrorKind::StorageFull => FileOpErrorKind::DiskFull,
        io::ErrorKind::AlreadyExists => FileOpErrorKind::TargetExists,
        _ => FileOpErrorKind::Other,
    }
}

/// 묶을 항목 하나.
pub(crate) struct Planned {
    path: PathBuf,
    /// zip 안 이름(`/` 구분, 폴더는 끝의 `/` 없이).
    name: String,
    is_folder: bool,
    size: u64,
    modified: Option<SystemTime>,
}

/// 원본들을 깊이 우선(폴더가 먼저, 안의 항목은 이름순)으로 훑어 묶을 차례를 만든다. 폴더 링크·정션은 따라 들어가지
/// 않고(순환을 막는다) 묶지 못한 항목으로 알린다. 되부르지 않는다(깊은 폴더에서 스택이 넘치지 않게).
fn plan(sources: &[PathBuf]) -> (Vec<Planned>, Vec<ItemFailure>) {
    let mut items = Vec::new();
    let mut failures = Vec::new();
    let mut stack: Vec<(PathBuf, String)> = Vec::new();
    for source in sources.iter().rev() {
        let name = source
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        stack.push((source.clone(), name));
    }
    while let Some((path, name)) = stack.pop() {
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                failures.push(ItemFailure {
                    source: path,
                    kind: io_kind(&error),
                });
                continue;
            }
        };
        let is_dir = metadata.is_dir();
        let link_to_dir = metadata.file_type().is_symlink()
            && fs::metadata(&path).is_ok_and(|target| target.is_dir());
        if link_to_dir {
            failures.push(ItemFailure {
                source: path,
                kind: FileOpErrorKind::Unsupported,
            });
            continue;
        }
        let modified = metadata.modified().ok();
        if is_dir {
            let mut children: Vec<(PathBuf, String)> = match fs::read_dir(&path) {
                Ok(listing) => listing
                    .filter_map(Result::ok)
                    .map(|entry| {
                        (
                            entry.path(),
                            format!("{name}/{}", entry.file_name().to_string_lossy()),
                        )
                    })
                    .collect(),
                Err(error) => {
                    failures.push(ItemFailure {
                        source: path.clone(),
                        kind: io_kind(&error),
                    });
                    Vec::new()
                }
            };
            items.push(Planned {
                path,
                name,
                is_folder: true,
                size: 0,
                modified,
            });
            // 스택이라 거꾸로 넣어야 이름순으로 나온다.
            children.sort_by_key(|a| std::cmp::Reverse(a.1.to_lowercase()));
            stack.extend(children);
        } else {
            let size = fs::metadata(&path).map_or(metadata.len(), |m| m.len());
            items.push(Planned {
                path,
                name,
                is_folder: false,
                size,
                modified,
            });
        }
    }
    (items, failures)
}

/// 항목 하나를 쓰지 못한 까닭.
pub(crate) enum WriteError {
    /// 원본을 읽지 못했다(그 항목만 건너뛸 수 있다).
    Read(io::Error),
    /// zip을 쓰지 못했다(작업을 그만둔다).
    Write(io::Error),
    /// 취소했다.
    Cancelled,
}

/// 쓴 항목의 central directory 기록 재료.
struct Record<'a> {
    name: &'a str,
    method: u16,
    time: u16,
    date: u16,
    mtime: Option<u32>,
    crc: u32,
    compressed: u64,
    size: u64,
    offset: u64,
    folder: bool,
}

/// 스트리밍 zip 쓰기.
pub(crate) struct ZipWriter {
    out: BufWriter<File>,
    pos: u64,
    central: Vec<u8>,
    count: u64,
}

impl ZipWriter {
    pub(crate) fn new(file: File) -> Self {
        ZipWriter {
            out: BufWriter::with_capacity(CHUNK, file),
            pos: 0,
            central: Vec::new(),
            count: 0,
        }
    }

    fn put(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.out.write_all(bytes)?;
        self.pos += bytes.len() as u64;
        Ok(())
    }

    /// 폴더 기록(`이름/`)을 쓴다.
    pub(crate) fn add_folder(
        &mut self,
        name: &str,
        modified: Option<SystemTime>,
    ) -> io::Result<()> {
        let full = format!("{name}/");
        let (time, date) = dos_time(modified);
        let mtime = unix_seconds(modified);
        let offset = self.pos;
        let record = Record {
            name: &full,
            method: 0,
            time,
            date,
            mtime,
            crc: 0,
            compressed: 0,
            size: 0,
            offset,
            folder: true,
        };
        let header = local_header(&record, false);
        self.put(&header)?;
        self.push_central(&record);
        Ok(())
    }

    /// 파일 하나를 쓴다. 실패하면 이 항목에서 쓴 것을 되돌려 놓는다(zip에는 앞 항목까지만 남는다).
    pub(crate) fn add_file(
        &mut self,
        name: &str,
        path: &Path,
        size: u64,
        modified: Option<SystemTime>,
        step: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), WriteError> {
        let start = (self.pos, self.central.len(), self.count);
        let result = self.add_file_inner(name, path, size, modified, step);
        if result.is_err() {
            let _ = self.rollback(start);
        }
        result
    }

    fn rollback(&mut self, (pos, central, count): (u64, usize, u64)) -> io::Result<()> {
        self.out.flush()?;
        self.out.get_mut().set_len(pos)?;
        self.out.seek(SeekFrom::Start(pos))?;
        self.pos = pos;
        self.central.truncate(central);
        self.count = count;
        Ok(())
    }

    fn add_file_inner(
        &mut self,
        name: &str,
        path: &Path,
        size: u64,
        modified: Option<SystemTime>,
        step: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), WriteError> {
        let mut file = File::open(path).map_err(WriteError::Read)?;
        let (time, date) = dos_time(modified);
        let mtime = unix_seconds(modified);
        let zip64 = size >= ZIP64_FILE_LIMIT;
        let method = if size == 0 { 0 } else { 8 };
        let offset = self.pos;
        let mut record = Record {
            name,
            method,
            time,
            date,
            mtime,
            crc: 0,
            compressed: 0,
            size: 0,
            offset,
            folder: false,
        };
        let header = local_header(&record, zip64);
        self.put(&header).map_err(WriteError::Write)?;
        if size == 0 {
            self.push_central(&record);
            return Ok(());
        }

        let mut crc = Crc32::new();
        let mut total = 0u64;
        let mut compressed = 0u64;
        let flags = create_comp_flags_from_zip_params(6, -15, 0);
        let mut compressor = Box::new(CompressorOxide::new(flags));
        let mut input = vec![0u8; CHUNK];
        let mut output = vec![0u8; 64 * 1024];
        loop {
            let n = file.read(&mut input).map_err(WriteError::Read)?;
            if n == 0 {
                break;
            }
            crc.update(&input[..n]);
            total += n as u64;
            if !step(total) {
                return Err(WriteError::Cancelled);
            }
            let mut rest = &input[..n];
            while !rest.is_empty() {
                let result = deflate(&mut compressor, rest, &mut output, MZFlush::None);
                if result.status.is_err()
                    || (result.bytes_consumed == 0 && result.bytes_written == 0)
                {
                    return Err(WriteError::Write(io::Error::other("deflate 실패")));
                }
                rest = &rest[result.bytes_consumed..];
                self.put(&output[..result.bytes_written])
                    .map_err(WriteError::Write)?;
                compressed += result.bytes_written as u64;
            }
        }
        loop {
            let result = deflate(&mut compressor, &[], &mut output, MZFlush::Finish);
            self.put(&output[..result.bytes_written])
                .map_err(WriteError::Write)?;
            compressed += result.bytes_written as u64;
            match result.status {
                Ok(MZStatus::StreamEnd) => break,
                Ok(_) => {}
                Err(_) => return Err(WriteError::Write(io::Error::other("deflate 실패"))),
            }
        }
        if !zip64 && (total >= u64::from(MAX32) || compressed >= u64::from(MAX32)) {
            return Err(WriteError::Write(io::Error::other("zip64가 필요한 크기")));
        }

        record.crc = crc.finish();
        record.compressed = compressed;
        record.size = total;
        self.patch_header(&record, zip64)
            .map_err(WriteError::Write)?;
        self.push_central(&record);
        Ok(())
    }

    /// 다 쓴 파일의 지역 머리글에 CRC와 크기를 채운다.
    fn patch_header(&mut self, record: &Record<'_>, zip64: bool) -> io::Result<()> {
        let end = self.pos;
        self.out.flush()?;
        self.out.seek(SeekFrom::Start(record.offset + 14))?;
        let (csize32, size32) = if zip64 {
            (MAX32, MAX32)
        } else {
            (record.compressed as u32, record.size as u32)
        };
        let mut fixed = Vec::with_capacity(12);
        fixed.extend_from_slice(&record.crc.to_le_bytes());
        fixed.extend_from_slice(&csize32.to_le_bytes());
        fixed.extend_from_slice(&size32.to_le_bytes());
        self.out.write_all(&fixed)?;
        if zip64 {
            // zip64 확장 필드는 이름 바로 뒤(그 필드 머리 4바이트 다음)에 있다.
            let at = record.offset + 30 + record.name.len() as u64 + 4;
            self.out.flush()?;
            self.out.seek(SeekFrom::Start(at))?;
            let mut big = Vec::with_capacity(16);
            big.extend_from_slice(&record.size.to_le_bytes());
            big.extend_from_slice(&record.compressed.to_le_bytes());
            self.out.write_all(&big)?;
        }
        self.out.flush()?;
        self.out.seek(SeekFrom::Start(end))?;
        Ok(())
    }

    fn push_central(&mut self, record: &Record<'_>) {
        let zip64_sizes = record.size >= u64::from(MAX32)
            || record.compressed >= u64::from(MAX32)
            || (!record.folder && record.method != 0 && record.size >= ZIP64_FILE_LIMIT);
        let zip64_offset = record.offset >= u64::from(MAX32);
        let mut extra = time_extra(record.mtime);
        if zip64_sizes || zip64_offset {
            let mut data = Vec::new();
            if zip64_sizes {
                data.extend_from_slice(&record.size.to_le_bytes());
                data.extend_from_slice(&record.compressed.to_le_bytes());
            }
            if zip64_offset {
                data.extend_from_slice(&record.offset.to_le_bytes());
            }
            extra.extend_from_slice(&1u16.to_le_bytes());
            extra.extend_from_slice(&(data.len() as u16).to_le_bytes());
            extra.extend_from_slice(&data);
        }
        let version: u16 = if zip64_sizes || zip64_offset {
            45
        } else if record.method == 8 {
            20
        } else {
            10
        };
        let c = &mut self.central;
        c.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        c.extend_from_slice(&version.to_le_bytes()); // 만든 버전
        c.extend_from_slice(&version.to_le_bytes()); // 풀 때 필요한 버전
        c.extend_from_slice(&FLAG_UTF8.to_le_bytes());
        c.extend_from_slice(&record.method.to_le_bytes());
        c.extend_from_slice(&record.time.to_le_bytes());
        c.extend_from_slice(&record.date.to_le_bytes());
        c.extend_from_slice(&record.crc.to_le_bytes());
        let (csize32, size32) = if zip64_sizes {
            (MAX32, MAX32)
        } else {
            (record.compressed as u32, record.size as u32)
        };
        c.extend_from_slice(&csize32.to_le_bytes());
        c.extend_from_slice(&size32.to_le_bytes());
        c.extend_from_slice(&(record.name.len() as u16).to_le_bytes());
        c.extend_from_slice(&(extra.len() as u16).to_le_bytes());
        c.extend_from_slice(&0u16.to_le_bytes()); // 주석 길이
        c.extend_from_slice(&0u16.to_le_bytes()); // 시작 디스크
        c.extend_from_slice(&0u16.to_le_bytes()); // 안쪽 속성
        let external: u32 = if record.folder { 0x10 } else { 0x20 };
        c.extend_from_slice(&external.to_le_bytes());
        let offset32 = if zip64_offset {
            MAX32
        } else {
            record.offset as u32
        };
        c.extend_from_slice(&offset32.to_le_bytes());
        c.extend_from_slice(record.name.as_bytes());
        c.extend_from_slice(&extra);
        self.count += 1;
    }

    /// central directory와 끝 기록(필요하면 zip64 끝 기록도)을 쓰고 파일을 닫는다.
    pub(crate) fn finish(mut self) -> io::Result<()> {
        let cd_offset = self.pos;
        let central = std::mem::take(&mut self.central);
        self.put(&central)?;
        let cd_size = central.len() as u64;
        let count = self.count;
        let big = count > 0xFFFF || cd_offset >= u64::from(MAX32) || cd_size >= u64::from(MAX32);
        if big {
            let eocd64_offset = self.pos;
            let mut z = Vec::with_capacity(76);
            z.extend_from_slice(&0x0606_4b50u32.to_le_bytes());
            z.extend_from_slice(&44u64.to_le_bytes());
            z.extend_from_slice(&45u16.to_le_bytes());
            z.extend_from_slice(&45u16.to_le_bytes());
            z.extend_from_slice(&0u32.to_le_bytes());
            z.extend_from_slice(&0u32.to_le_bytes());
            z.extend_from_slice(&count.to_le_bytes());
            z.extend_from_slice(&count.to_le_bytes());
            z.extend_from_slice(&cd_size.to_le_bytes());
            z.extend_from_slice(&cd_offset.to_le_bytes());
            z.extend_from_slice(&0x0706_4b50u32.to_le_bytes());
            z.extend_from_slice(&0u32.to_le_bytes());
            z.extend_from_slice(&eocd64_offset.to_le_bytes());
            z.extend_from_slice(&1u32.to_le_bytes());
            self.put(&z)?;
        }
        let mut e = Vec::with_capacity(22);
        e.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        e.extend_from_slice(&0u16.to_le_bytes());
        e.extend_from_slice(&0u16.to_le_bytes());
        let count16 = if big { 0xFFFFu16 } else { count as u16 };
        e.extend_from_slice(&count16.to_le_bytes());
        e.extend_from_slice(&count16.to_le_bytes());
        let (size32, offset32) = if big {
            (MAX32, MAX32)
        } else {
            (cd_size as u32, cd_offset as u32)
        };
        e.extend_from_slice(&size32.to_le_bytes());
        e.extend_from_slice(&offset32.to_le_bytes());
        e.extend_from_slice(&0u16.to_le_bytes());
        self.put(&e)?;
        self.out.flush()?;
        let file = self
            .out
            .into_inner()
            .map_err(io::IntoInnerError::into_error)?;
        file.sync_all().ok();
        Ok(())
    }
}

/// 지역 머리글(이름·확장 시각·필요하면 zip64 자리 표시). CRC·크기는 0(파일은 다 쓴 뒤 채운다).
fn local_header(record: &Record<'_>, zip64: bool) -> Vec<u8> {
    let mut extra = Vec::new();
    if zip64 {
        extra.extend_from_slice(&1u16.to_le_bytes());
        extra.extend_from_slice(&16u16.to_le_bytes());
        extra.extend_from_slice(&[0u8; 16]);
    }
    extra.extend_from_slice(&time_extra(record.mtime));
    let version: u16 = if zip64 {
        45
    } else if record.method == 8 {
        20
    } else {
        10
    };
    let mut h = Vec::with_capacity(30 + record.name.len() + extra.len());
    h.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
    h.extend_from_slice(&version.to_le_bytes());
    h.extend_from_slice(&FLAG_UTF8.to_le_bytes());
    h.extend_from_slice(&record.method.to_le_bytes());
    h.extend_from_slice(&record.time.to_le_bytes());
    h.extend_from_slice(&record.date.to_le_bytes());
    h.extend_from_slice(&0u32.to_le_bytes());
    let size32 = if zip64 { MAX32 } else { 0 };
    h.extend_from_slice(&size32.to_le_bytes());
    h.extend_from_slice(&size32.to_le_bytes());
    h.extend_from_slice(&(record.name.len() as u16).to_le_bytes());
    h.extend_from_slice(&(extra.len() as u16).to_le_bytes());
    h.extend_from_slice(record.name.as_bytes());
    h.extend_from_slice(&extra);
    h
}

/// 확장 시각 필드(0x5455): 수정 시각의 유닉스 초.
fn time_extra(mtime: Option<u32>) -> Vec<u8> {
    let Some(secs) = mtime else {
        return Vec::new();
    };
    let mut e = Vec::with_capacity(9);
    e.extend_from_slice(&0x5455u16.to_le_bytes());
    e.extend_from_slice(&5u16.to_le_bytes());
    e.push(1);
    e.extend_from_slice(&secs.to_le_bytes());
    e
}

fn unix_seconds(modified: Option<SystemTime>) -> Option<u32> {
    let since = modified?.duration_since(UNIX_EPOCH).ok()?;
    u32::try_from(since.as_secs()).ok()
}

/// 수정 시각의 DOS 시각·날짜(지역 시각, 1980~2107년으로 자름). 모르면 1980-01-01.
fn dos_time(modified: Option<SystemTime>) -> (u16, u16) {
    let Some(modified) = modified else {
        return (0, (1 << 5) | 1);
    };
    let local: DateTime<Local> = modified.into();
    let year = local.year().clamp(1980, 2107) as u16;
    let date = ((year - 1980) << 9) | ((local.month() as u16) << 5) | local.day() as u16;
    let time = ((local.hour() as u16) << 11)
        | ((local.minute() as u16) << 5)
        | (local.second() as u16 / 2);
    (time, date)
}

/// 실행취소한 새 zip을 다시 실행으로 다시 만든다: 같은 원본으로 `dest`에 쓴다(있으면 `TargetExists`). 반쯤 쓴 파일은
/// 남기지 않는다. 취소·질문은 없다(실행취소·다시 실행 작업 안에서 한 번에 한다).
pub(crate) fn rewrite(sources: &[PathBuf], dest: &Path) -> Result<(), FileOpErrorKind> {
    if fs::symlink_metadata(dest).is_ok() {
        return Err(FileOpErrorKind::TargetExists);
    }
    let folder = dest.parent().ok_or(FileOpErrorKind::Other)?;
    let (items, failures) = plan(sources);
    if let Some(failure) = failures.into_iter().next() {
        return Err(failure.kind);
    }
    let temp = temp_path(folder);
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| io_kind(&error))?;
    let mut writer = ZipWriter::new(file);
    let mut result = Ok(());
    for item in &items {
        let one = if item.is_folder {
            writer
                .add_folder(&item.name, item.modified)
                .map_err(|error| io_kind(&error))
        } else {
            writer
                .add_file(
                    &item.name,
                    &item.path,
                    item.size,
                    item.modified,
                    &mut |_| true,
                )
                .map_err(|error| match error {
                    WriteError::Read(error) | WriteError::Write(error) => io_kind(&error),
                    WriteError::Cancelled => FileOpErrorKind::Other,
                })
        };
        if let Err(kind) = one {
            result = Err(kind);
            break;
        }
    }
    let result = result.and_then(|()| writer.finish().map_err(|error| io_kind(&error)));
    let result = result.and_then(|()| file_ops::rename(&temp, dest).map_err(|error| error.kind()));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
