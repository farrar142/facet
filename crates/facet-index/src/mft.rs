//! MFT 바이트 → 인덱스: NTFS MFT 레코드 덩어리에서 항목을 뽑아 인덱스를 만든다(spec #22, 티켓 #43).
//!
//! 레코드마다 이름(`$FILE_NAME`), 부모 참조, 크기(이름 없는 `$DATA`), 수정 날짜·속성
//! (`$STANDARD_INFORMATION`)을 읽는다. 경로는 부모 참조로 이어진다.
//!
//! - 하드 링크는 이름마다 항목 하나다. 8.3 짧은 이름(DOS 이름공간)은 항목이 아니다.
//! - 대체 데이터 스트림(이름 있는 `$DATA`)은 항목이 아니고 크기에도 들지 않는다.
//! - 지운 레코드(사용 중 표시가 꺼진 레코드)와 한 번도 쓰지 않은 빈 레코드는 뺀다.
//! - `$ATTRIBUTE_LIST`로 확장된 레코드는 확장 레코드 머리의 기본 레코드 참조를 따라 기본 레코드와
//!   합친다.
//! - NTFS 메타데이터 파일(레코드 0..16의 `$MFT`·`$Extend` 등)과 그 아래 항목은 사용자 항목이
//!   아니므로 넣지 않는다. 볼륨 루트(레코드 5) 자체도 항목이 아니다([`Index::new`]).
//!
//! 깨진 레코드에서 죽지 않는다. 서명·고정값(fixup)·길이가 맞지 않는 레코드는 건너뛰고
//! [`MftReport::skipped`]에 센다. 범위를 넘는 읽기를 하지 않고 panic하지 않는다. 부모를 볼륨
//! 루트까지 따라갈 수 없는 항목은 빼고 [`MftReport::orphans`]에 센다.
//!
//! ```
//! use facet_index::{Index, MftParser};
//!
//! // 실제로는 볼륨에서 읽은 MFT 덩어리를 차례로 넣는다. 여기서는 빈 레코드 둘이다.
//! let mut parser = MftParser::new(1024).unwrap();
//! parser.add(0, &[0u8; 2048]);
//! let (index, report) = parser.finish(r"C:\");
//! assert_eq!((index.len(), report.skipped), (0, 0));
//! ```

use std::collections::HashMap;
use std::fmt;

use crate::{ATTRIBUTE_DIRECTORY, Index, NewItem};

/// NTFS 볼륨 루트 폴더의 레코드 번호.
pub const MFT_ROOT_RECORD: u64 = 5;

/// 사용자 항목이 시작하는 레코드 번호. 그 앞(루트 말고)은 NTFS 메타데이터 파일이다.
const FIRST_USER_RECORD: u64 = 16;

/// 고정값(update sequence) 간격. 섹터 크기와 상관없이 512바이트다.
const FIXUP_STRIDE: usize = 512;

/// 파일 참조에서 레코드 번호 부분(아래 48비트).
const RECORD_MASK: u64 = 0xFFFF_FFFF_FFFF;

const ATTRIBUTE_END: u32 = 0xFFFF_FFFF;
const STANDARD_INFORMATION: u32 = 0x10;
const FILE_NAME: u32 = 0x30;
const DATA: u32 = 0x80;

/// 레코드 머리의 사용 중 표시.
const IN_USE: u16 = 0x0001;
/// 레코드 머리의 폴더 표시(파일 이름 색인이 있음).
const DIRECTORY: u16 = 0x0002;

/// `$FILE_NAME`의 DOS(8.3 짧은 이름) 이름공간. 같은 레코드에 Win32 이름이 따로 있다.
const NAMESPACE_DOS: u8 = 2;

/// MFT 파서를 만들 수 없는 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MftError {
    /// 레코드 크기가 512 이상 65536 이하의 2의 거듭제곱이 아니다.
    RecordSize(usize),
}

impl fmt::Display for MftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RecordSize(n) => write!(
                f,
                "MFT 레코드 크기가 512 이상 65536 이하의 2의 거듭제곱이 아닙니다: {n}"
            ),
        }
    }
}

impl std::error::Error for MftError {}

/// MFT를 파싱하며 센 수.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MftReport {
    /// 읽은 사용 중 레코드 수(확장 레코드 포함, 건너뛴 레코드 제외).
    pub records: u64,
    /// 서명·고정값·길이가 맞지 않아 건너뛴 레코드 수. 덩어리 끝의 잘린 레코드도 센다.
    pub skipped: u64,
    /// 부모를 볼륨 루트까지 따라갈 수 없어 뺀 항목(이름) 수.
    pub orphans: u64,
    /// 인덱스가 받지 않은 이름(빈 이름, `\0`·`\`가 든 이름) 수.
    pub rejected: u64,
}

/// MFT 레코드 덩어리를 차례로 받아 인덱스를 만든다.
///
/// 덩어리는 어떤 순서로 넣어도 된다. 같은 레코드 번호가 두 번 오면 나중 것을 쓴다.
#[derive(Debug, Clone)]
pub struct MftParser {
    record_size: usize,
    /// 사용 중 레코드들(받은 순서).
    slots: Vec<Slot>,
    /// 모든 레코드의 이름(DOS 이름공간 제외).
    names: Vec<Name>,
    /// 이름 글자들을 이어 붙인 것.
    text: String,
    /// 고정값을 되돌릴 레코드 사본.
    scratch: Vec<u8>,
    skipped: u64,
}

/// 사용 중 레코드 하나에서 읽은 값.
#[derive(Debug, Clone, Copy)]
struct Slot {
    number: u64,
    sequence: u16,
    directory: bool,
    /// 확장 레코드면 기본 레코드 참조, 기본 레코드면 0.
    base: u64,
    /// 이름 없는 `$DATA`의 크기(첫 조각이 이 레코드에 있을 때).
    size: Option<u64>,
    /// `$STANDARD_INFORMATION`의 (수정 날짜, 속성).
    standard: Option<(u64, u32)>,
}

/// `$FILE_NAME` 하나.
#[derive(Debug, Clone, Copy)]
struct Name {
    /// 이 이름을 담은 레코드의 [`MftParser::slots`] 위치.
    slot: u32,
    /// 부모 폴더 참조(레코드 번호와 순번).
    parent: u64,
    start: u32,
    end: u32,
}

/// 레코드 하나가 깨졌다는 표지. 까닭은 세기만 하므로 담지 않는다.
struct Broken;

fn u16_at(b: &[u8], at: usize) -> Result<u16, Broken> {
    let s = b.get(at..at.checked_add(2).ok_or(Broken)?).ok_or(Broken)?;
    Ok(u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, Broken> {
    let s = b.get(at..at.checked_add(4).ok_or(Broken)?).ok_or(Broken)?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn u64_at(b: &[u8], at: usize) -> Result<u64, Broken> {
    let s = b.get(at..at.checked_add(8).ok_or(Broken)?).ok_or(Broken)?;
    let mut a = [0; 8];
    a.copy_from_slice(s);
    Ok(u64::from_le_bytes(a))
}

fn u8_at(b: &[u8], at: usize) -> Result<u8, Broken> {
    b.get(at).copied().ok_or(Broken)
}

/// `b[at..at + len]`. 범위를 넘으면 [`Broken`].
fn slice(b: &[u8], at: usize, len: usize) -> Result<&[u8], Broken> {
    b.get(at..at.checked_add(len).ok_or(Broken)?).ok_or(Broken)
}

/// 레코드에 고정값을 되돌린다(제자리에서). 각 512바이트 조각의 마지막 2바이트가 갱신 순서
/// 번호와 같아야 하고, 갱신 순서 배열의 원래 값으로 바꾼다.
fn apply_fixup(record: &mut [u8]) -> Result<(), Broken> {
    let usa_offset = usize::from(u16_at(record, 4)?);
    let usa_count = usize::from(u16_at(record, 6)?);
    if usa_count != record.len() / FIXUP_STRIDE + 1 {
        return Err(Broken);
    }
    let usn = u16_at(record, usa_offset)?;
    for i in 1..usa_count {
        let original = u16_at(record, usa_offset + 2 * i)?;
        let end = i * FIXUP_STRIDE - 2;
        if u16_at(record, end)? != usn {
            return Err(Broken);
        }
        record[end..end + 2].copy_from_slice(&original.to_le_bytes());
    }
    Ok(())
}

impl MftParser {
    /// 레코드 크기 `record_size`(보통 1024, 4K 섹터 볼륨은 4096)의 MFT를 읽는 파서.
    pub fn new(record_size: usize) -> Result<Self, MftError> {
        if !record_size.is_power_of_two() || !(FIXUP_STRIDE..=65536).contains(&record_size) {
            return Err(MftError::RecordSize(record_size));
        }
        Ok(Self {
            record_size,
            slots: Vec::new(),
            names: Vec::new(),
            text: String::new(),
            scratch: vec![0; record_size],
            skipped: 0,
        })
    }

    /// 레코드 번호 `first_record`부터 이어지는 레코드들을 담은 덩어리를 읽는다.
    ///
    /// 덩어리 길이가 레코드 크기의 배수가 아니면 끝의 잘린 레코드를 건너뛰고 센다.
    pub fn add(&mut self, first_record: u64, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(self.record_size);
        for (n, raw) in chunks.by_ref().enumerate() {
            let Some(number) = first_record.checked_add(n as u64) else {
                self.skipped += 1;
                continue;
            };
            self.record(number, raw);
        }
        if !chunks.remainder().is_empty() {
            self.skipped += 1;
        }
    }

    /// 레코드 하나를 읽는다. 깨졌으면 앞서 넣은 이름을 되돌리고 센다.
    fn record(&mut self, number: u64, raw: &[u8]) {
        if raw[..4] == [0; 4] {
            // 한 번도 쓰지 않은 레코드.
            return;
        }
        let (names, text) = (self.names.len(), self.text.len());
        match self.parse(number, raw) {
            Ok(()) => {}
            Err(Broken) => {
                self.names.truncate(names);
                self.text.truncate(text);
                self.skipped += 1;
            }
        }
    }

    fn parse(&mut self, number: u64, raw: &[u8]) -> Result<(), Broken> {
        if &raw[..4] != b"FILE" {
            return Err(Broken);
        }
        let mut record = std::mem::take(&mut self.scratch);
        record.copy_from_slice(raw);
        let result = apply_fixup(&mut record).and_then(|()| self.attributes(number, &record));
        self.scratch = record;
        result
    }

    /// 고정값을 되돌린 레코드의 머리와 속성을 읽어 사용 중이면 담는다.
    fn attributes(&mut self, number: u64, r: &[u8]) -> Result<(), Broken> {
        let sequence = u16_at(r, 0x10)?;
        let first = usize::from(u16_at(r, 0x14)?);
        let flags = u16_at(r, 0x16)?;
        let used = u32_at(r, 0x18)? as usize;
        let allocated = u32_at(r, 0x1C)? as usize;
        let base = u64_at(r, 0x20)?;
        if allocated != r.len() || used > r.len() || first < 0x18 || first >= used {
            return Err(Broken);
        }
        if flags & IN_USE == 0 {
            return Ok(());
        }
        let slot = u32::try_from(self.slots.len()).map_err(|_| Broken)?;
        let mut size = None;
        let mut standard = None;
        let mut at = first;
        loop {
            if at + 4 > used {
                return Err(Broken);
            }
            let kind = u32_at(r, at)?;
            if kind == ATTRIBUTE_END {
                break;
            }
            if at + 8 > used {
                return Err(Broken);
            }
            let len = u32_at(r, at + 4)? as usize;
            if len < 0x10 || len > used - at {
                return Err(Broken);
            }
            let a = &r[at..at + len];
            let non_resident = u8_at(a, 8)? != 0;
            let name_len = u8_at(a, 9)?;
            let value = || -> Result<&[u8], Broken> {
                if non_resident {
                    return Err(Broken);
                }
                let value_len = u32_at(a, 0x10)? as usize;
                let value_at = usize::from(u16_at(a, 0x14)?);
                slice(a, value_at, value_len)
            };
            match kind {
                STANDARD_INFORMATION => {
                    let v = value()?;
                    standard = Some((u64_at(v, 0x08)?, u32_at(v, 0x20)?));
                }
                FILE_NAME => {
                    let v = value()?;
                    let parent = u64_at(v, 0)?;
                    let chars = usize::from(u8_at(v, 0x40)?);
                    let namespace = u8_at(v, 0x41)?;
                    let utf16 = slice(v, 0x42, chars * 2)?;
                    if namespace != NAMESPACE_DOS {
                        self.push_name(slot, parent, utf16)?;
                    }
                }
                DATA if name_len == 0 => {
                    if non_resident {
                        // 비상주 속성은 여러 레코드로 나뉠 수 있다. 실제 크기는 첫 조각(VCN 0)에 있다.
                        if u64_at(a, 0x10)? == 0 {
                            size = Some(u64_at(a, 0x30)?);
                        }
                    } else {
                        size = Some(value()?.len() as u64);
                    }
                }
                _ => {}
            }
            at += len;
        }
        self.slots.push(Slot {
            number,
            sequence,
            directory: flags & DIRECTORY != 0,
            base,
            size,
            standard,
        });
        Ok(())
    }

    fn push_name(&mut self, slot: u32, parent: u64, utf16: &[u8]) -> Result<(), Broken> {
        let start = u32::try_from(self.text.len()).map_err(|_| Broken)?;
        let units = utf16
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c));
        self.text
            .extend(char::decode_utf16(units).map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER)));
        let end = u32::try_from(self.text.len()).map_err(|_| Broken)?;
        self.names.push(Name {
            slot,
            parent,
            start,
            end,
        });
        Ok(())
    }

    /// 읽은 레코드로 볼륨 루트 경로가 `root`(`C:\` 등)인 인덱스를 만든다.
    pub fn finish(self, root: &str) -> (Index, MftReport) {
        let Self {
            mut slots,
            names,
            text,
            skipped,
            ..
        } = self;
        let mut report = MftReport {
            records: slots.len() as u64,
            skipped,
            ..MftReport::default()
        };

        // 레코드 번호 → 기본 레코드 위치. 같은 번호가 또 오면 나중 것.
        let by_number: HashMap<u64, u32> = slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.base == 0)
            .map(|(i, s)| (s.number, i as u32))
            .collect();
        // 참조(번호와 순번)가 가리키는 사용 중 기본 레코드. 순번 0은 확인하지 않는다.
        let resolve = |slots: &[Slot], reference: u64| -> Option<u32> {
            let &at = by_number.get(&(reference & RECORD_MASK))?;
            let sequence = (reference >> 48) as u16;
            (sequence == 0 || slots[at as usize].sequence == sequence).then_some(at)
        };

        // 확장 레코드의 크기·날짜를 기본 레코드에 합친다.
        for i in 0..slots.len() {
            let ext = slots[i];
            if ext.base == 0 {
                continue;
            }
            if let Some(at) = resolve(&slots, ext.base) {
                let base = &mut slots[at as usize];
                base.size = base.size.or(ext.size);
                base.standard = base.standard.or(ext.standard);
            }
        }

        // 이름마다 기본 레코드를 정하고, 폴더는 첫 이름의 부모를 기억한다.
        let owner = |slots: &[Slot], name: &Name| -> Option<u32> {
            let slot = &slots[name.slot as usize];
            if slot.base == 0 {
                Some(name.slot)
            } else {
                resolve(slots, slot.base)
            }
        };
        let mut folder_parent: HashMap<u32, u64> = HashMap::new();
        for name in &names {
            if let Some(at) = owner(&slots, name)
                && slots[at as usize].directory
            {
                folder_parent.entry(at).or_insert(name.parent);
            }
        }

        let mut reach = Reach {
            slots: &slots,
            resolve: &resolve,
            folder_parent: &folder_parent,
            memo: HashMap::new(),
        };
        let mut index = Index::new(root, MFT_ROOT_RECORD);
        for name in &names {
            // 확장 레코드의 기본 레코드가 없으면(지워졌거나 다시 쓰임) 이름도 버린다.
            let Some(at) = owner(&slots, name) else {
                continue;
            };
            let slot = slots[at as usize];
            if slot.number < FIRST_USER_RECORD {
                continue;
            }
            match reach.parent(name.parent) {
                Place::Reachable => {}
                Place::System => continue,
                Place::Orphan => {
                    report.orphans += 1;
                    continue;
                }
            }
            let (modified, attributes) = slot.standard.unwrap_or((0, 0));
            let item = NewItem {
                number: slot.number,
                parent: name.parent & RECORD_MASK,
                name: &text[name.start as usize..name.end as usize],
                size: if slot.directory {
                    0
                } else {
                    slot.size.unwrap_or(0)
                },
                modified,
                attributes: if slot.directory {
                    attributes | ATTRIBUTE_DIRECTORY
                } else {
                    attributes
                },
            };
            if index.insert(item).is_err() {
                report.rejected += 1;
            }
        }
        (index, report)
    }
}

/// 폴더가 볼륨 루트에서 어디에 있는가.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// 볼륨 루트까지 따라갈 수 있다.
    Reachable,
    /// NTFS 메타데이터 폴더(`$Extend` 등) 아래다. 항목으로 넣지 않고 세지도 않는다.
    System,
    /// 부모가 없거나 폴더가 아니거나 고리를 이룬다.
    Orphan,
}

/// 폴더마다 [`Place`]를 기억해 두고 부모 참조가 어디에 닿는지 가린다.
struct Reach<'a, F> {
    slots: &'a [Slot],
    resolve: &'a F,
    folder_parent: &'a HashMap<u32, u64>,
    /// 기본 레코드 위치 → 판정. 판정 중인 폴더는 `None`(고리 찾기).
    memo: HashMap<u32, Option<Place>>,
}

impl<F: Fn(&[Slot], u64) -> Option<u32>> Reach<'_, F> {
    /// 부모 참조 `reference`가 가리키는 폴더의 자리.
    fn parent(&mut self, reference: u64) -> Place {
        let mut walked = Vec::new();
        let mut reference = reference;
        let place = loop {
            let Some(at) = (self.resolve)(self.slots, reference) else {
                break Place::Orphan;
            };
            let slot = &self.slots[at as usize];
            if !slot.directory {
                break Place::Orphan;
            }
            if slot.number == MFT_ROOT_RECORD {
                break Place::Reachable;
            }
            if slot.number < FIRST_USER_RECORD {
                break Place::System;
            }
            match self.memo.get(&at) {
                Some(Some(place)) => break *place,
                // 이번에 따라가는 중인 폴더를 다시 만났다: 고리.
                Some(None) => break Place::Orphan,
                None => {}
            }
            let Some(&up) = self.folder_parent.get(&at) else {
                break Place::Orphan;
            };
            self.memo.insert(at, None);
            walked.push(at);
            reference = up;
        };
        for at in walked {
            self.memo.insert(at, Some(place));
        }
        place
    }
}

impl Index {
    /// 레코드 크기 `record_size`의 MFT 레코드 0번부터 담은 `bytes`로 볼륨 루트가 `root`인
    /// 인덱스를 만든다. [`MftParser`]에 덩어리 하나를 넣는 것과 같다.
    pub fn from_mft(
        root: &str,
        record_size: usize,
        bytes: &[u8],
    ) -> Result<(Index, MftReport), MftError> {
        let mut parser = MftParser::new(record_size)?;
        parser.add(0, bytes);
        Ok(parser.finish(root))
    }
}
