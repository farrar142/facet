//! USN 저널 레코드 적용과 「다시 만들기 필요」 판정.
//!
//! 인덱서 서비스는 볼륨의 USN 저널에서 읽은 레코드 바이트(`USN_RECORD_V2`를 이어 붙인 것,
//! `FSCTL_READ_USN_JOURNAL`이 돌려주는 앞 8바이트의 다음 USN은 뺀 것)를 [`Index::apply_usn`]에
//! 넘긴다. 인덱스는 만들기·지우기·이름바꾸기·이동을 반영하고, 보안 변경은 바뀐 폴더를
//! [`UsnOutcome::security_changes`]로 알린다.
//!
//! 저널 상태([`JournalState`])는 인덱스가 함께 들고 있다. 서비스가 다시 뜰 때 현재 저널 정보
//! ([`JournalInfo`])를 [`Index::check_journal`]에 주면 저널 ID가 바뀌었거나 저널이 넘쳐 저장한
//! USN이 사라졌는지 알려 준다.

use std::collections::HashMap;
use std::fmt;

use crate::{ATTRIBUTE_DIRECTORY, DEAD, Index, InsertError, NAME_END, NewItem, SEPARATOR};

/// 파일이나 폴더가 만들어졌다(`USN_REASON_FILE_CREATE`).
pub const USN_REASON_FILE_CREATE: u32 = 0x0000_0100;
/// 파일이나 폴더가 지워졌다(`USN_REASON_FILE_DELETE`).
pub const USN_REASON_FILE_DELETE: u32 = 0x0000_0200;
/// 보안 설명자가 바뀌었다(`USN_REASON_SECURITY_CHANGE`).
pub const USN_REASON_SECURITY_CHANGE: u32 = 0x0000_0800;
/// 이름바꾸기·이동 전의 이름(`USN_REASON_RENAME_OLD_NAME`).
pub const USN_REASON_RENAME_OLD_NAME: u32 = 0x0000_1000;
/// 이름바꾸기·이동 뒤의 이름(`USN_REASON_RENAME_NEW_NAME`).
pub const USN_REASON_RENAME_NEW_NAME: u32 = 0x0000_2000;

/// 파일 참조 번호에서 MFT 레코드 번호만 남기는 마스크(위 16비트는 재사용 순번).
const NUMBER_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
/// `USN_RECORD_V2`의 이름 앞 고정 부분 길이.
const HEADER_LEN: usize = 60;

/// 볼륨의 저널 상태: 어느 저널을 어디까지 처리했는가.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalState {
    /// 저널 ID(`USN_JOURNAL_DATA.UsnJournalID`). 저널을 지우고 다시 만들면 바뀐다.
    pub journal_id: u64,
    /// 마지막으로 처리한 레코드의 USN.
    pub usn: i64,
}

/// 서비스가 볼륨에서 읽은 현재 저널 정보.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalInfo {
    /// 현재 저널 ID.
    pub journal_id: u64,
    /// 저널에 남은 가장 오래된 USN(`USN_JOURNAL_DATA.FirstUsn`).
    pub first_usn: i64,
}

/// 인덱스를 다시 만들어야 하는 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebuildReason {
    /// 인덱스가 저널 상태를 모른다(처음 만든 인덱스).
    NoJournalState,
    /// 저널 ID가 바뀌었다. 저널이 지워지고 다시 만들어졌다.
    JournalIdChanged,
    /// 저장한 USN이 가장 오래 남은 USN보다 앞선다. 저널이 넘쳐 그 사이 변경을 잃었다.
    Overflowed,
}

/// 저널 상태 판정.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// 저장한 USN부터 저널을 읽어 따라잡을 수 있다.
    Current,
    /// 인덱스를 처음부터 다시 만들어야 한다.
    RebuildNeeded(RebuildReason),
}

/// 보안 변경 레코드가 알리는 값: 보안이 바뀐 폴더.
///
/// 폴더 자신의 보안이 바뀌었으면 그 폴더, 파일의 보안이 바뀌었으면 그 파일이 든 폴더다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityChange {
    /// 폴더의 항목 번호. 볼륨 루트면 루트 번호.
    pub folder: u64,
    /// 레코드를 모두 적용한 뒤 인덱스에서 본 폴더의 전체 경로. 볼륨 루트까지 닿지 않으면 `None`.
    pub path: Option<String>,
}

/// [`Index::apply_usn`]이 알리는 결과.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsnOutcome {
    /// 해석해 훑은 레코드 수.
    pub records: usize,
    /// 보안이 바뀐 폴더들(같은 폴더는 한 번만, 처음 나온 순서).
    pub security_changes: Vec<SecurityChange>,
}

/// USN 레코드 바이트를 적용하지 못한 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsnError {
    /// `offset`에서 시작하는 레코드가 바이트 끝을 넘는다.
    Truncated { offset: usize },
    /// `offset`의 레코드 길이가 말이 안 된다(고정 부분보다 짧거나 8의 배수가 아니다).
    BadLength { offset: usize },
    /// 지원하지 않는 레코드 판(`USN_RECORD_V2`만 안다).
    UnsupportedVersion { offset: usize, major: u16 },
    /// 이름 위치·길이가 레코드를 벗어나거나 이름이 비었거나 쓸 수 없는 글자가 있다.
    BadName { offset: usize },
    /// 인덱스가 담을 수 있는 크기를 넘었다. 앞선 레코드는 이미 적용됐다.
    Full,
}

impl fmt::Display for UsnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { offset } => write!(f, "{offset}바이트의 USN 레코드가 잘렸습니다"),
            Self::BadLength { offset } => {
                write!(f, "{offset}바이트의 USN 레코드 길이가 맞지 않습니다")
            }
            Self::UnsupportedVersion { offset, major } => {
                write!(
                    f,
                    "{offset}바이트의 USN 레코드 판 {major}은 지원하지 않습니다"
                )
            }
            Self::BadName { offset } => {
                write!(f, "{offset}바이트의 USN 레코드 이름이 맞지 않습니다")
            }
            Self::Full => f.write_str("인덱스가 담을 수 있는 크기를 넘었습니다"),
        }
    }
}

impl std::error::Error for UsnError {}

/// 해석한 USN 레코드 하나.
struct Record {
    number: u64,
    parent: u64,
    usn: i64,
    timestamp: u64,
    reason: u32,
    attributes: u32,
    name: String,
}

fn le<const N: usize>(bytes: &[u8], at: usize) -> Option<[u8; N]> {
    bytes.get(at..at.checked_add(N)?)?.try_into().ok()
}

/// 바이트 전체를 레코드 열로 해석한다. 하나라도 깨졌으면 오류다.
fn parse(bytes: &[u8]) -> Result<Vec<Record>, UsnError> {
    let mut records = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let rest = &bytes[offset..];
        let truncated = UsnError::Truncated { offset };
        let length = u32::from_le_bytes(le(rest, 0).ok_or(truncated)?) as usize;
        if length < HEADER_LEN || !length.is_multiple_of(8) {
            return Err(UsnError::BadLength { offset });
        }
        let record = rest.get(..length).ok_or(truncated)?;
        let major = u16::from_le_bytes(le(record, 4).ok_or(truncated)?);
        if major != 2 {
            return Err(UsnError::UnsupportedVersion { offset, major });
        }
        let field = |at| le::<8>(record, at).map(u64::from_le_bytes).ok_or(truncated);
        let word = |at| le::<4>(record, at).map(u32::from_le_bytes).ok_or(truncated);
        let name_len = usize::from(u16::from_le_bytes(le(record, 56).ok_or(truncated)?));
        let name_at = usize::from(u16::from_le_bytes(le(record, 58).ok_or(truncated)?));
        let bad_name = UsnError::BadName { offset };
        let raw = name_at
            .checked_add(name_len)
            .and_then(|end| record.get(name_at..end))
            .filter(|raw| raw.len() % 2 == 0)
            .ok_or(bad_name)?;
        let units: Vec<u16> = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        // NTFS 이름은 짝 없는 서로게이트도 담을 수 있다. 그런 글자는 대체 문자로 바꿔 받는다.
        let name = String::from_utf16_lossy(&units);
        if name.is_empty() || name.contains([NAME_END, SEPARATOR]) {
            return Err(bad_name);
        }
        records.push(Record {
            number: field(8)? & NUMBER_MASK,
            parent: field(16)? & NUMBER_MASK,
            usn: field(24)? as i64,
            timestamp: field(32)?,
            reason: word(40)?,
            attributes: word(52)?,
            name,
        });
        offset += length;
    }
    Ok(records)
}

impl Index {
    /// 인덱스가 들고 있는 볼륨 저널 상태. 아직 모르면 `None`.
    pub fn journal(&self) -> Option<JournalState> {
        self.journal
    }

    /// 저널 상태를 정한다. MFT로 처음 만든 직후 그 시점의 저널 ID와 USN을 넣는다.
    pub fn set_journal(&mut self, state: JournalState) {
        self.journal = Some(state);
    }

    /// 서비스가 읽은 현재 저널 정보와 저장한 상태를 견줘 저널을 따라잡을 수 있는지 알린다.
    ///
    /// 저널 ID가 바뀌었거나 저장한 USN이 가장 오래 남은 USN보다 앞서면(넘침) 다시 만들어야 한다.
    pub fn check_journal(&self, info: JournalInfo) -> Freshness {
        match self.journal {
            None => Freshness::RebuildNeeded(RebuildReason::NoJournalState),
            Some(state) if state.journal_id != info.journal_id => {
                Freshness::RebuildNeeded(RebuildReason::JournalIdChanged)
            }
            Some(state) if state.usn < info.first_usn => {
                Freshness::RebuildNeeded(RebuildReason::Overflowed)
            }
            Some(_) => Freshness::Current,
        }
    }

    /// USN 레코드 바이트를 적용한다.
    ///
    /// 바이트가 깨졌으면 아무것도 바꾸지 않고 오류를 돌려준다. 저널 상태가 있으면 마지막 레코드의
    /// USN으로 앞으로 옮긴다. 이미 적용한 레코드를 다시 적용해도 결과는 같다(만들기는 있으면
    /// 넘어가고, 지우기는 없으면 넘어간다).
    ///
    /// 항목은 (항목 번호, 부모 폴더)로 가리킨다. 한 폴더에 이름이 둘인 하드 링크는 구별하지 못한다.
    pub fn apply_usn(&mut self, bytes: &[u8]) -> Result<UsnOutcome, UsnError> {
        let records = parse(bytes)?;
        if self.locator.is_none() {
            self.locator = Some(self.build_locator());
        }
        let mut folders: Vec<u64> = Vec::new();
        for record in &records {
            self.apply_one(record)?;
            if record.reason & USN_REASON_SECURITY_CHANGE != 0
                && record.reason & USN_REASON_FILE_DELETE == 0
            {
                let is_folder = record.attributes & ATTRIBUTE_DIRECTORY != 0;
                let folder = if is_folder {
                    record.number
                } else {
                    record.parent
                };
                if !folders.contains(&folder) {
                    folders.push(folder);
                }
            }
        }
        if let (Some(state), Some(last)) = (&mut self.journal, records.last()) {
            state.usn = state.usn.max(last.usn);
        }
        self.changed();
        let security_changes = folders
            .into_iter()
            .map(|folder| SecurityChange {
                folder,
                path: self.folder_path(folder),
            })
            .collect();
        Ok(UsnOutcome {
            records: records.len(),
            security_changes,
        })
    }

    fn build_locator(&self) -> HashMap<(u64, u64), u32> {
        self.records
            .iter()
            .enumerate()
            .filter(|(_, r)| r.parent != DEAD)
            .map(|(at, r)| ((r.number, r.parent), at as u32))
            .collect()
    }

    fn locate(&self, number: u64, parent: u64) -> Option<usize> {
        let locator = self.locator.as_ref()?;
        locator.get(&(number, parent)).map(|&at| at as usize)
    }

    fn apply_one(&mut self, r: &Record) -> Result<(), UsnError> {
        if r.reason & USN_REASON_FILE_DELETE != 0 {
            if let Some(at) = self.locate(r.number, r.parent) {
                self.kill(at);
            }
            self.pending_renames.remove(&r.number);
        } else if r.reason & USN_REASON_RENAME_NEW_NAME != 0 {
            let old = self
                .pending_renames
                .remove(&r.number)
                .and_then(|parent| self.locate(r.number, parent));
            let (size, modified) = old.map_or((0, r.timestamp), |at| {
                (self.records[at].size, self.records[at].modified)
            });
            if let Some(at) = old {
                self.kill(at);
            }
            // 옛 이름 레코드를 못 받았어도 새 자리에 같은 항목이 있으면 그것을 새 이름으로 바꾼다.
            if let Some(at) = self.locate(r.number, r.parent) {
                self.kill(at);
            }
            self.add(r, size, modified)?;
        } else if r.reason & USN_REASON_RENAME_OLD_NAME != 0 {
            self.pending_renames.insert(r.number, r.parent);
        } else if r.reason & USN_REASON_FILE_CREATE != 0
            && self.locate(r.number, r.parent).is_none()
        {
            self.add(r, 0, r.timestamp)?;
        }
        Ok(())
    }

    fn add(&mut self, r: &Record, size: u64, modified: u64) -> Result<(), UsnError> {
        self.insert(NewItem {
            number: r.number,
            parent: r.parent,
            name: &r.name,
            size,
            modified,
            attributes: r.attributes,
        })
        .map_err(|e| match e {
            InsertError::Full => UsnError::Full,
            // 해석할 때 걸러 낸 이름이다.
            InsertError::EmptyName | InsertError::InvalidName => UsnError::BadName { offset: 0 },
        })
    }

    /// 레코드를 지운 것으로 표시한다. 자리는 남지만 어떤 질의에도 나오지 않는다.
    fn kill(&mut self, at: usize) {
        let record = &mut self.records[at];
        if record.parent == DEAD {
            return;
        }
        let (number, parent) = (record.number, record.parent);
        record.parent = DEAD;
        self.dead += 1;
        if self.folders.get(&number) == Some(&(at as u32)) {
            self.folders.remove(&number);
        }
        if let Some(locator) = &mut self.locator
            && locator.get(&(number, parent)) == Some(&(at as u32))
        {
            locator.remove(&(number, parent));
        }
    }

    /// 폴더의 전체 경로. 볼륨 루트면 루트 경로, 닿지 않으면 `None`.
    fn folder_path(&self, folder: u64) -> Option<String> {
        if folder == self.root_number {
            return Some(self.root.clone());
        }
        let &at = self.folders.get(&folder)?;
        self.path(at as usize)
    }
}
