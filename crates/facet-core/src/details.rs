//! 자세히 보기: 열(이름·수정한 날짜·유형·크기)로 정렬한 목록과 열에 보일 값.
//!
//! - 정렬은 이미 읽은 목록을 다시 늘어놓을 뿐 파일 시스템을 다시 읽지 않는다. [`DetailsList`]는
//!   `Send`이므로 UI 스레드 밖에서 만들고 정렬을 바꿀 수 있다.
//! - 폴더는 정렬 방향과 상관없이 항상 파일보다 앞이다.
//! - 이름은 탐색기와 같은 자연 정렬([`compare_names`])로 비교한다.
//! - 정렬 키가 같으면 이름 오름차순으로 가른다(이름 열 내림차순일 때만 이름도 내림차순).
//!
//! 유형 열은 화면 문장이 아니라 [`FileType`]으로 돌려준다(ADR-0007). 「파일 폴더」·「TXT 파일」
//! 같은 글자는 앱이 만든다. 크기와 날짜는 수와 단위만이라 여기서 글자로 만든다.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::Arc;

use std::path::PathBuf;

use crate::Item;
use crate::file_list::format_modified_local;
use crate::shell_provider::RecycledEntry;

/// 자세히 보기의 열.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Column {
    /// 이름.
    #[default]
    Name,
    /// 수정한 날짜.
    Modified,
    /// 유형.
    Type,
    /// 크기.
    Size,
    /// 위치: 항목이 든 폴더(검색 결과 위치에만 뜻이 있다). 폴더 목록에서는 모든 항목이 같은 폴더라 이름 순서다.
    Location,
    /// 일치: 내용 검색 결과의 일치 수(검색 결과 위치에만 뜻이 있다). 일치가 없는 항목은 0으로 본다.
    Matches,
    /// #351: 내 PC 드라이브의 전체 크기.
    TotalSize,
    /// #351: 내 PC 드라이브의 남은 공간.
    FreeSpace,
    /// 원래 위치: 휴지통 항목이 지우기 전에 든 폴더(휴지통에만 뜻이 있다). 휴지통 항목이 아니면 이름 순서다.
    OriginalLocation,
    /// 지운 날짜: 휴지통 항목을 지운 시각(휴지통에만 뜻이 있다). 휴지통 항목이 아니면 이름 순서다.
    DateDeleted,
}

/// 정렬 방향.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Direction {
    /// 오름차순.
    #[default]
    Ascending,
    /// 내림차순.
    Descending,
}

/// 정렬 기준: 어느 열을 어느 방향으로. 기본값은 이름 오름차순이다.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Sort {
    pub column: Column,
    pub direction: Direction,
}

/// 항목의 유형. 화면 글자로 바꾸는 일은 앱이 한다(ADR-0007).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FileType {
    /// 폴더.
    Folder,
    /// 파일. `extension`은 마지막 `.` 뒤를 대문자로 바꾼 것이고, 없거나 비면 `None`이다.
    File { extension: Option<String> },
}

/// 항목의 유형. 확장자는 이름의 마지막 `.` 뒤다(`.gitignore`는 `GITIGNORE`, `이름.`은 없음).
pub fn file_type(item: &Item) -> FileType {
    if item.is_folder {
        FileType::Folder
    } else {
        FileType::File {
            extension: extension_upper(&item.name.to_string_lossy()),
        }
    }
}

fn extension_upper(name: &str) -> Option<String> {
    let (_, extension) = name.rsplit_once('.')?;
    (!extension.is_empty()).then(|| extension.to_uppercase())
}

/// 크기 열의 글자: KB 단위로 올림하고 세 자리마다 쉼표를 찍는다(`1,206 KB`). 폴더는 빈 글자다.
pub fn size_text(item: &Item) -> String {
    if item.is_folder {
        return String::new();
    }
    let digits = item.size.div_ceil(1024).to_string();
    let mut text = String::with_capacity(digits.len() + digits.len() / 3 + 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            text.push(',');
        }
        text.push(digit);
    }
    text.push_str(" KB");
    text
}

/// 수정한 날짜 열의 글자: 이 컴퓨터의 시간대로 `yyyy-MM-dd HH:mm`.
pub fn modified_text(item: &Item) -> String {
    format_modified_local(item.modified)
}

/// 탐색기와 같은 자연 정렬로 두 이름을 비교한다.
///
/// - 대소문자를 가리지 않는다(대소문자만 다르면 같다).
/// - 숫자 덩어리(ASCII 숫자)는 값으로 비교한다(「파일2」 < 「파일10」). 길이에 한계가 없다.
///   값이 같으면 앞자리 0이 많은 쪽이 앞이다(「01」 < 「1」).
/// - 글자 사이는 기호·공백 < 숫자 < 문자 순이고, 같은 무리 안에서는 소문자로 바꾼 코드 포인트 순이다
///   (그래서 영문 < 한글).
///
/// 탐색기(`StrCmpLogicalW`)는 언어별 규칙으로 글자를 비교하므로 악센트 글자나 `-`·`'`가 든 이름 등에서는
/// 다를 수 있다. 숫자·앞자리 0·한글·대소문자는 같다(Windows 전용 테스트가 확인한다).
pub fn compare_names(a: &str, b: &str) -> Ordering {
    natural_key(a).cmp(&natural_key(b))
}

/// 숫자 덩어리 표지. 기호·공백의 부호보다 크고 문자의 부호보다 작다.
const DIGITS: u8 = 0x43;

/// ASCII 바이트의 부호: 기호·공백·제어 문자는 0x01..=0x42(바이트 순서대로), 영문자는 대소문자를 접어
/// 0x44..=0x5D. 숫자는 덩어리로 따로 부호화한다. ASCII가 아닌 바이트는 부호화하지 않고 그대로 쓴다(0x80 이상).
const ASCII_CODE: [u8; 128] = {
    let mut table = [0u8; 128];
    let mut rank = 1;
    let mut byte = 0;
    while byte < 128 {
        let b = byte as u8;
        if b.is_ascii_alphabetic() {
            table[byte] = DIGITS + 1 + (b.to_ascii_lowercase() - b'a');
        } else if !b.is_ascii_digit() {
            table[byte] = rank;
            rank += 1;
        }
        byte += 1;
    }
    table
};

/// 자연 정렬의 정렬 키: 이 바이트열을 그냥 바이트 순서로 비교하면 [`compare_names`]의 순서가 된다.
///
/// 한 번 만들어 두면 비교가 바이트열 비교(`memcmp`) 하나라서 정렬을 여러 번 바꿔도 빠르다.
/// - ASCII 글자는 [`ASCII_CODE`]로 바꾸어 기호·공백 < 숫자 < 문자 순서와 대소문자 접기를 넣는다.
/// - ASCII가 아닌 글자는 소문자로 바꾼 UTF-8 바이트 그대로다. UTF-8 바이트 순서는 코드 포인트 순서와 같다.
/// - 숫자 덩어리는 [`DIGITS`], 앞자리 0을 뺀 자릿수, 그 숫자들, 앞자리 0 개수의 역순으로 부호화한다.
///   그래서 자릿수가 적은 쪽, 같으면 값이 작은 쪽, 같으면 앞자리 0이 많은 쪽이 앞이다.
fn natural_key(name: &str) -> Vec<u8> {
    natural_key_and_extension(name).0
}

/// [`natural_key`]와, 키 안에서 확장자(마지막 `.` 뒤)의 키가 시작하는 위치. 확장자가 없거나 비면 `None`이다.
///
/// `.`은 한 바이트 부호이고 숫자 덩어리를 끊으므로, 확장자의 키는 이름 키의 그 위치부터 끝까지와 같다.
fn natural_key_and_extension(name: &str) -> (Vec<u8>, Option<usize>) {
    let bytes = name.as_bytes();
    let mut extension = None;
    let mut key = Vec::with_capacity(bytes.len() + 8);
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_digit() {
            let end = bytes[i..]
                .iter()
                .position(|d| !d.is_ascii_digit())
                .map_or(bytes.len(), |n| i + n);
            let run = &bytes[i..end];
            let zeros = run.iter().take_while(|&&d| d == b'0').count();
            let value = &run[zeros..];
            key.push(DIGITS);
            match u8::try_from(value.len()) {
                Ok(len) if len < u8::MAX => key.push(len),
                _ => {
                    key.push(u8::MAX);
                    let len = u32::try_from(value.len()).unwrap_or(u32::MAX);
                    key.extend_from_slice(&len.to_be_bytes());
                }
            }
            key.extend_from_slice(value);
            let zeros = u32::try_from(zeros).unwrap_or(u32::MAX);
            key.extend_from_slice(&(u32::MAX - zeros).to_be_bytes());
            i = end;
        } else if b.is_ascii() {
            key.push(ASCII_CODE[usize::from(b)]);
            i += 1;
            if b == b'.' {
                extension = Some(key.len());
            }
        } else {
            let c = name[i..].chars().next().expect("글자 경계");
            let len = c.len_utf8();
            if is_caseless(c) {
                key.extend_from_slice(&bytes[i..i + len]);
                i += len;
                continue;
            }
            for lower in c.to_lowercase() {
                if lower.is_ascii() {
                    key.push(ASCII_CODE[lower as usize]);
                } else {
                    let mut buffer = [0; 4];
                    key.extend_from_slice(lower.encode_utf8(&mut buffer).as_bytes());
                }
            }
            i += len;
        }
    }
    let extension = extension.filter(|&start| start < key.len());
    (key, extension)
}

/// 대소문자가 없는 글자가 확실한 범위(한중일 기호·가나·한자, 한글 음절, 그림 문자).
/// 이 범위는 소문자 표를 찾지 않고 그대로 쓴다. 범위 밖이어도 결과는 같고 조금 느릴 뿐이다.
fn is_caseless(c: char) -> bool {
    matches!(c, '\u{3000}'..='\u{9FFF}' | '\u{AC00}'..='\u{D7A3}' | '\u{1F300}'..='\u{1FAFF}')
}

/// 검색 결과 항목이 든 폴더와 그 폴더 경로의 자연 정렬 키(위치 열 정렬). 같은 폴더의 항목끼리 나눠 쓴다.
#[derive(Debug, Clone)]
struct Folder {
    path: Arc<Path>,
    key: Arc<[u8]>,
}

/// 정렬에 쓰는 값을 미리 계산해 둔 항목.
#[derive(Debug, Clone)]
struct Entry {
    item: Item,
    /// 이름의 자연 정렬 키([`natural_key`]).
    key: Vec<u8>,
    /// 확장자의 키가 `key` 안에서 시작하는 위치. 폴더와 확장자 없는 파일은 `None`.
    extension: Option<usize>,
    /// 검색 결과 항목이면 든 폴더. 폴더 목록의 항목은 `None`이다(모두 탭 위치 안).
    folder: Option<Folder>,
    /// 내용 검색 결과 항목이면 첫 일치 줄과 일치 수.
    hit: Option<Hit>,
    /// 휴지통 항목이면 그 정보.
    recycled: Option<Box<Recycled>>,
}

/// 휴지통 항목의 정보와 정렬·선택에 쓰는 값.
#[derive(Debug, Clone)]
struct Recycled {
    entry: RecycledEntry,
    /// 휴지통 안 경로(`$R...`): 같은 이름의 휴지통 항목을 가르는 선택 열쇠.
    key: PathBuf,
    /// 원래 폴더 경로의 자연 정렬 키(원래 위치 열 정렬).
    folder_key: Vec<u8>,
}

/// 내용 검색 결과 항목의 일치: 첫 일치 줄과 일치 수.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// 첫 일치 줄(앞뒤 공백을 걷고 잘린 것).
    pub first_line: String,
    /// 겹치지 않는 일치 수.
    pub matches: u64,
}

impl Entry {
    fn new(item: Item) -> Self {
        Self::in_folder(item, None)
    }

    fn in_folder(item: Item, folder: Option<Folder>) -> Self {
        let (key, extension) = natural_key_and_extension(&item.name.to_string_lossy());
        let extension = extension.filter(|_| !item.is_folder);
        Entry {
            item,
            key,
            extension,
            folder,
            hit: None,
            recycled: None,
        }
    }

    fn recycled(item: Item, entry: RecycledEntry, key: PathBuf) -> Self {
        let folder_key = entry
            .original
            .parent()
            .map(|folder| natural_key(&folder.to_string_lossy()))
            .unwrap_or_default();
        let mut this = Self::new(item);
        this.recycled = Some(Box::new(Recycled {
            entry,
            key,
            folder_key,
        }));
        this
    }

    fn original_folder_key(&self) -> Option<&[u8]> {
        self.recycled.as_ref().map(|r| r.folder_key.as_slice())
    }

    fn date_deleted(&self) -> Option<std::time::SystemTime> {
        self.recycled.as_ref().map(|r| r.entry.deleted)
    }

    fn match_count(&self) -> u64 {
        self.hit.as_ref().map_or(0, |hit| hit.matches)
    }

    fn folder_key(&self) -> Option<&[u8]> {
        self.folder.as_ref().map(|folder| &*folder.key)
    }

    /// 선택·커서가 이 항목을 기억하는 열쇠: 폴더 목록이면 이름, 검색 결과면 전체 경로, 휴지통 항목이면 휴지통 안 경로.
    fn selection_key(&self) -> Cow<'_, OsStr> {
        if let Some(recycled) = &self.recycled {
            return Cow::Borrowed(recycled.key.as_os_str());
        }
        match &self.folder {
            None => Cow::Borrowed(self.item.name.as_os_str()),
            Some(folder) => Cow::Owned(folder.path.join(&self.item.name).into_os_string()),
        }
    }

    /// 열쇠가 이 항목의 것인가. 경로를 만들지 않고 비교한다.
    fn has_key(&self, key: &OsStr) -> bool {
        if let Some(recycled) = &self.recycled {
            return recycled.key.as_os_str() == key;
        }
        match &self.folder {
            None => self.item.name == key,
            Some(folder) => {
                let path = Path::new(key);
                path.file_name() == Some(self.item.name.as_os_str())
                    && path.parent() == Some(&*folder.path)
            }
        }
    }

    fn extension_key(&self) -> Option<&[u8]> {
        self.extension.map(|start| &self.key[start..])
    }

    fn compare_names(&self, other: &Self) -> Ordering {
        self.key
            .cmp(&other.key)
            .then_with(|| self.item.name.cmp(&other.item.name))
    }
}

fn compare_entries(a: &Entry, b: &Entry, sort: Sort) -> Ordering {
    let directed = |ordering: Ordering| match sort.direction {
        Direction::Ascending => ordering,
        Direction::Descending => ordering.reverse(),
    };
    // 폴더 먼저: 방향과 상관없다.
    let folders_first = b.item.is_folder.cmp(&a.item.is_folder);
    if folders_first != Ordering::Equal {
        return folders_first;
    }
    let key = match sort.column {
        Column::Name => return directed(a.compare_names(b)),
        Column::Modified => a.item.modified.cmp(&b.item.modified),
        // 확장자 없는 파일이 앞이다(`None` < `Some`).
        Column::Type => a.extension_key().cmp(&b.extension_key()),
        // 폴더는 크기를 보이지 않으므로 크기로 가르지 않는다.
        Column::Size if a.item.is_folder => Ordering::Equal,
        Column::Size => a.item.size.cmp(&b.item.size),
        Column::Location => a.folder_key().cmp(&b.folder_key()),
        Column::Matches => a.match_count().cmp(&b.match_count()),
        Column::TotalSize | Column::FreeSpace => todo!("#351"),
        Column::OriginalLocation => a.original_folder_key().cmp(&b.original_folder_key()),
        Column::DateDeleted => a.date_deleted().cmp(&b.date_deleted()),
    };
    directed(key).then_with(|| a.compare_names(b))
}

/// 자세히 보기의 목록: 이미 읽은 항목을 정렬 기준대로 늘어놓은 것.
///
/// 정렬을 바꾸면([`DetailsList::set_sort`]) 가진 항목을 다시 정렬할 뿐 파일 시스템을 읽지 않는다.
#[derive(Debug, Clone)]
pub struct DetailsList {
    entries: Vec<Entry>,
    sort: Sort,
}

impl DetailsList {
    /// `items`를 `sort`대로 정렬한 목록.
    pub fn new(items: Vec<Item>, sort: Sort) -> Self {
        let mut list = DetailsList {
            entries: items.into_iter().map(Entry::new).collect(),
            sort,
        };
        list.resort();
        list
    }

    /// 휴지통의 목록: `items`와 같은 차례로 항목마다 휴지통 정보와 휴지통 안 경로(선택 열쇠, 모르면 `None`)를 받아
    /// `sort`대로 정렬한다. `recycled`가 짧으면 남은 항목은 휴지통 정보가 없다.
    pub(crate) fn with_recycled(
        items: Vec<Item>,
        recycled: Vec<Option<(RecycledEntry, PathBuf)>>,
        sort: Sort,
    ) -> Self {
        let mut recycled = recycled.into_iter();
        let mut list = DetailsList {
            entries: items
                .into_iter()
                .map(|item| match recycled.next().flatten() {
                    Some((entry, key)) => Entry::recycled(item, entry, key),
                    None => Entry::new(item),
                })
                .collect(),
            sort,
        };
        list.resort();
        list
    }

    /// `index`번째 항목의 휴지통 정보(원래 경로·지운 시각). 휴지통 항목이 아니면 `None`이다.
    pub fn recycled(&self, index: usize) -> Option<&RecycledEntry> {
        self.entries
            .get(index)
            .and_then(|e| e.recycled.as_deref())
            .map(|recycled| &recycled.entry)
    }

    /// `index`번째 휴지통 항목의 휴지통 안 경로(`$R...`). 휴지통 항목이 아니면 `None`이다.
    pub fn recycled_path(&self, index: usize) -> Option<&Path> {
        self.entries
            .get(index)
            .and_then(|e| e.recycled.as_deref())
            .map(|recycled| recycled.key.as_path())
    }

    /// 지금 정렬 기준.
    pub fn sort(&self) -> Sort {
        self.sort
    }

    /// 정렬 기준을 바꾸고 다시 정렬한다.
    pub fn set_sort(&mut self, sort: Sort) {
        self.sort = sort;
        self.resort();
    }

    /// 바뀐 항목만 반영한다: 이름이 같은 항목을 빼고, 다시 읽은 항목(`Some`)을 넣은 뒤 다시 정렬한다.
    /// `None`은 그 이름의 항목이 사라졌다는 뜻이다. 다른 항목은 다시 읽지 않는다.
    pub fn apply_changes(&mut self, changes: impl IntoIterator<Item = (OsString, Option<Item>)>) {
        let mut names = HashSet::new();
        let mut added = Vec::new();
        for (name, item) in changes {
            added.retain(|entry: &Entry| entry.item.name != name);
            if let Some(item) = item {
                added.push(Entry::new(item));
            }
            names.insert(name);
        }
        if names.is_empty() {
            return;
        }
        self.entries
            .retain(|entry| !names.contains(&entry.item.name));
        self.entries.extend(added);
        self.resort();
    }

    fn resort(&mut self) {
        let sort = self.sort;
        self.entries
            .sort_unstable_by(|a, b| compare_entries(a, b, sort));
    }

    /// 검색 결과 항목들(든 폴더와 항목)을 더한다. 더한 것만 정렬해 이미 정렬된 목록과 합치므로 결과가 많아도 묶음마다
    /// 다시 정렬하지 않는다. 같은 폴더의 정렬 키는 한 번만 만든다.
    pub fn extend_found(&mut self, found: impl IntoIterator<Item = (Arc<Path>, Item)>) {
        self.extend_hits(found.into_iter().map(|(path, item)| (path, item, None)));
    }

    /// [`DetailsList::extend_found`]와 같고 항목마다 내용 검색의 일치(첫 일치 줄·일치 수)를 함께 받는다.
    pub fn extend_hits(&mut self, found: impl IntoIterator<Item = (Arc<Path>, Item, Option<Hit>)>) {
        let mut keys: HashMap<Arc<Path>, Arc<[u8]>> = HashMap::new();
        let mut added: Vec<Entry> = found
            .into_iter()
            .map(|(path, item, hit)| {
                let key = keys
                    .entry(Arc::clone(&path))
                    .or_insert_with(|| natural_key(&path.to_string_lossy()).into())
                    .clone();
                let mut entry = Entry::in_folder(item, Some(Folder { path, key }));
                entry.hit = hit;
                entry
            })
            .collect();
        if added.is_empty() {
            return;
        }
        let sort = self.sort;
        added.sort_unstable_by(|a, b| compare_entries(a, b, sort));
        let old = std::mem::take(&mut self.entries);
        let mut merged = Vec::with_capacity(old.len() + added.len());
        let mut old = old.into_iter().peekable();
        let mut added = added.into_iter().peekable();
        loop {
            let take_old = match (old.peek(), added.peek()) {
                (Some(a), Some(b)) => compare_entries(a, b, sort) != Ordering::Greater,
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => break,
            };
            let next = if take_old { old.next() } else { added.next() };
            merged.extend(next);
        }
        self.entries = merged;
    }

    /// `index`번째 항목의 내용 검색 일치(첫 일치 줄·일치 수). 내용 검색 결과가 아니면 `None`이다.
    pub fn hit(&self, index: usize) -> Option<&Hit> {
        self.entries.get(index).and_then(|e| e.hit.as_ref())
    }

    /// `index`번째 항목이 든 폴더(검색 결과 항목). 폴더 목록의 항목은 `None`이다.
    pub fn folder(&self, index: usize) -> Option<&Arc<Path>> {
        self.entries
            .get(index)
            .and_then(|e| e.folder.as_ref())
            .map(|folder| &folder.path)
    }

    /// `index`번째 항목의 선택 열쇠: 폴더 목록이면 이름, 검색 결과면 전체 경로.
    pub(crate) fn selection_key(&self, index: usize) -> Option<Cow<'_, OsStr>> {
        self.entries.get(index).map(Entry::selection_key)
    }

    /// `index`번째 항목이 열쇠 `key`의 것인가.
    pub(crate) fn has_key(&self, index: usize, key: &OsStr) -> bool {
        self.entries.get(index).is_some_and(|e| e.has_key(key))
    }

    /// 검색 결과 항목에 facet이 한 작업을 반영한다: `keep`이 `false`인 항목은 빼고, `rename`이 새 폴더·새 이름을
    /// 주는 항목은 바꾼다(폴더 경로, 항목 이름). 바뀐 것이 있으면 다시 정렬하고 `true`다.
    pub(crate) fn apply_path_changes(
        &mut self,
        mut keep: impl FnMut(&Path, &Item) -> bool,
        mut rename: impl FnMut(&Path, &Item) -> Option<(Arc<Path>, OsString)>,
    ) -> bool {
        let before = self.entries.len();
        self.entries.retain(|entry| match &entry.folder {
            Some(folder) => keep(&folder.path, &entry.item),
            None => true,
        });
        let mut changed = self.entries.len() != before;
        let mut keys: HashMap<Arc<Path>, Arc<[u8]>> = HashMap::new();
        for entry in &mut self.entries {
            let Some(folder) = &entry.folder else {
                continue;
            };
            let Some((path, name)) = rename(&folder.path, &entry.item) else {
                continue;
            };
            let key = keys
                .entry(Arc::clone(&path))
                .or_insert_with(|| natural_key(&path.to_string_lossy()).into())
                .clone();
            let mut item = entry.item.clone();
            item.name = name;
            let hit = entry.hit.take();
            *entry = Entry::in_folder(item, Some(Folder { path, key }));
            entry.hit = hit;
            changed = true;
        }
        if changed {
            self.resort();
        }
        changed
    }

    /// 항목 수.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 항목이 없는가.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 정렬된 순서로 `index`번째 항목.
    pub fn get(&self, index: usize) -> Option<&Item> {
        self.entries.get(index).map(|e| &e.item)
    }

    /// 정렬된 순서로 모든 항목.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Item> + '_ {
        self.entries.iter().map(|e| &e.item)
    }

    /// 정렬된 순서의 항목들.
    pub fn into_items(self) -> Vec<Item> {
        self.entries.into_iter().map(|e| e.item).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 확장자의_키는_이름_키의_꼬리와_같다() {
        for name in [
            "a.txt",
            "사진 01.JPEG",
            "v1.2.mp3",
            "archive.tar.gz",
            ".gitignore",
            "x.7z",
            "x.007",
            "데이터.파일",
        ] {
            let (key, start) = natural_key_and_extension(name);
            let (_, extension) = name.rsplit_once('.').unwrap();
            assert_eq!(&key[start.unwrap()..], natural_key(extension), "{name}");
        }
        for name in ["README", "끝에 점.", "12"] {
            assert_eq!(natural_key_and_extension(name).1, None, "{name}");
        }
    }

    #[test]
    fn 대소문자가_없는_범위도_소문자_바꾸기와_같다() {
        for c in ['가', '힣', '漢', 'あ', '🎉', '🦀'] {
            assert!(is_caseless(c));
            assert_eq!(c.to_lowercase().collect::<String>(), c.to_string());
        }
    }
}
