//! facet의 인덱스: 볼륨 하나에 있는 항목의 이름·부모·크기·날짜·속성 목록과 그 위의 질의.
//!
//! 화면도 Windows API도 쓰지 않는 순수 크레이트다. 인덱서 서비스가 MFT와 USN 저널에서
//! 읽은 항목을 여기에 넣고, 질의는 여기서 실행한다(spec #22의 크레이트 (1)).
//!
//! 공개 인터페이스는 「항목 넣기 → 질의 → 결과」다.
//!
//! ```
//! use facet_index::{ATTRIBUTE_DIRECTORY, Index, NewItem, Query};
//!
//! let mut index = Index::new(r"C:\", 5);
//! index.insert(NewItem { number: 40, parent: 5, name: "문서", size: 0, modified: 0,
//!                        attributes: ATTRIBUTE_DIRECTORY }).unwrap();
//! index.insert(NewItem { number: 41, parent: 40, name: "Report.txt", size: 10, modified: 0,
//!                        attributes: 0 }).unwrap();
//! let hits = index.query(&Query::name_contains("REPORT").limit(100));
//! assert_eq!(hits[0].path, r"C:\문서\Report.txt");
//! ```
//!
//! 결과를 정렬하고 조각으로 나눠 받으려면 [`Index::search`]와 [`Index::resume`]을 쓴다.
//! 응답([`Page`])은 결과 조각, 전체 개수, 이어 받기 표지([`Cursor`])를 담는다.
//!
//! ```
//! use facet_index::{Index, NewItem, Order, Query, ResumeError, SortKey};
//!
//! let mut index = Index::new(r"C:\", 5);
//! for (n, size) in [(40, 30), (41, 10), (42, 20)] {
//!     index.insert(NewItem { number: n, parent: 5, name: &format!("{n}.txt"), size,
//!                            modified: 0, attributes: 0 }).unwrap();
//! }
//! let page = index.search(&Query::name_contains("txt").sort(SortKey::Size, Order::Ascending).limit(2));
//! assert_eq!((page.hits[0].size, page.hits[1].size, page.total), (10, 20, 3));
//! let rest = index.resume(page.next.unwrap(), 2).unwrap();
//! assert_eq!(rest.hits[0].size, 30);
//!
//! // 인덱스가 바뀌면 앞선 표지는 무효다.
//! let page = index.search(&Query::name_contains("txt").limit(1));
//! index.insert(NewItem { number: 43, parent: 5, name: "new.txt", size: 0, modified: 0,
//!                        attributes: 0 }).unwrap();
//! assert_eq!(index.resume(page.next.unwrap(), 1).unwrap_err(), ResumeError::Stale);
//! ```
//!
//! # 메모리 배치
//!
//! 항목 100만 개를 150MB 미만에 담으려고(spec #22) 항목마다 힙 할당을 하지 않는다.
//!
//! - 이름은 하나의 큰 문자열 버퍼에 이어 붙인다. 버퍼에는 대소문자를 접은 이름을 `\0`으로
//!   구분해 담아, 부분 일치 질의가 버퍼 전체를 한 번에 훑는다. 접은 이름이 원래 이름과
//!   다를 때만 원래 이름을 따로 담는다.
//! - 항목은 고정 크기 레코드(이름 위치, 부모 번호, 크기, 날짜, 속성)로 넣은 순서대로 둔다.
//! - 경로 문자열은 결과를 낼 때만 부모 번호를 따라 만든다. 부모 번호로 찾는 표는 폴더만 담는다.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::mem::size_of;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use memchr::memmem;

mod usn;
pub use usn::{
    Freshness, JournalInfo, JournalState, RebuildReason, SecurityChange, USN_REASON_FILE_CREATE,
    USN_REASON_FILE_DELETE, USN_REASON_RENAME_NEW_NAME, USN_REASON_RENAME_OLD_NAME,
    USN_REASON_SECURITY_CHANGE, UsnError, UsnOutcome,
};

/// 폴더 속성 비트(Windows `FILE_ATTRIBUTE_DIRECTORY`). 이 비트가 있는 항목만 부모가 될 수 있다.
pub const ATTRIBUTE_DIRECTORY: u32 = 0x10;

/// 읽기 전용 속성 비트(`FILE_ATTRIBUTE_READONLY`).
pub const ATTRIBUTE_READONLY: u32 = 0x1;
/// 숨김 속성 비트(`FILE_ATTRIBUTE_HIDDEN`).
pub const ATTRIBUTE_HIDDEN: u32 = 0x2;
/// 시스템 속성 비트(`FILE_ATTRIBUTE_SYSTEM`).
pub const ATTRIBUTE_SYSTEM: u32 = 0x4;

/// 경로 구분자.
const SEPARATOR: char = '\\';

/// 접은 이름 버퍼에서 이름과 이름 사이에 두는 글자. 이름에 들어갈 수 없다.
const NAME_END: char = '\0';

/// 인덱스에 넣을 항목 하나.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewItem<'a> {
    /// 항목 번호(NTFS면 MFT 레코드 번호). 하드 링크는 같은 번호로 이름마다 하나씩 넣는다.
    pub number: u64,
    /// 부모 폴더의 항목 번호. 볼륨 루트 바로 아래면 [`Index::new`]에 준 루트 번호다.
    pub parent: u64,
    /// 항목 이름(경로가 아닌 마지막 조각). 비어 있거나 `\0`·`\`가 들어 있으면 넣지 않는다.
    pub name: &'a str,
    /// 바이트 크기. 폴더는 보통 0.
    pub size: u64,
    /// 수정 날짜. Windows `FILETIME` 값(1601-01-01 UTC부터 100ns 단위).
    pub modified: u64,
    /// Windows 파일 속성 비트. 폴더는 [`ATTRIBUTE_DIRECTORY`]를 가진다.
    pub attributes: u32,
}

/// 항목을 넣지 못한 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertError {
    /// 이름이 비었다.
    EmptyName,
    /// 이름에 `\0`이나 경로 구분자 `\`가 있다.
    InvalidName,
    /// 인덱스가 담을 수 있는 크기(항목 수나 이름 버퍼 4GiB)를 넘는다.
    Full,
}

impl fmt::Display for InsertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::EmptyName => "이름이 비었습니다",
            Self::InvalidName => "이름에 쓸 수 없는 글자(\\0, \\)가 있습니다",
            Self::Full => "인덱스가 담을 수 있는 크기를 넘었습니다",
        })
    }
}

impl std::error::Error for InsertError {}

/// 이름 조건의 대소문자 규칙.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Case {
    /// 대소문자를 무시한다(기본).
    #[default]
    Insensitive,
    /// 대소문자를 구별한다.
    Sensitive,
}

/// 조건을 만들 수 없는 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterError {
    /// 정규식이 잘못됐거나 너무 크다. 정규식 라이브러리의 설명을 담는다.
    InvalidRegex(String),
}

impl fmt::Display for FilterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRegex(why) => write!(f, "잘못된 정규식입니다: {why}"),
        }
    }
}

impl std::error::Error for FilterError {}

/// 항목에 거는 조건 트리. 조건 하나를 만들거나 [`Filter::and`]·[`Filter::or`]·[`Filter::not`]으로 묶는다.
///
/// 사람이 치는 질의 문법은 이 위에 따로 얹는다. 여기에는 글자 문법이 없다.
#[derive(Debug, Clone)]
pub struct Filter(Node);

#[derive(Debug, Clone)]
enum Node {
    /// 이름에 조각이 들어 있다. 대소문자 무시면 `part`는 접은 조각이다.
    Contains {
        part: String,
        case: Case,
    },
    /// 이름이 정규식(와일드카드는 정규식으로 바꾼 것)과 맞는다.
    Pattern(regex::Regex),
    /// 폴더 아래(하위 폴더 포함)에 있다. 접은 폴더 경로를 `\`로 끝나게 담는다.
    Under(String),
    Size(Option<u64>, Option<u64>),
    Modified(Option<u64>, Option<u64>),
    /// 주어진 속성 비트를 모두 가진다.
    Attributes(u32),
    /// 접은 확장자(점 없이).
    Extension(String),
    All(Vec<Filter>),
    Any(Vec<Filter>),
    Not(Box<Filter>),
}

impl Filter {
    /// 이름에 `part`가 들어 있는 항목. 빈 조각은 모든 항목과 맞는다.
    pub fn name_contains(part: &str, case: Case) -> Self {
        let part = match case {
            Case::Insensitive => fold(part),
            Case::Sensitive => part.to_owned(),
        };
        Self(Node::Contains { part, case })
    }

    /// 이름 전체가 와일드카드(`*` 0개 이상의 글자, `?` 글자 하나)와 맞는 항목.
    pub fn name_wildcard(pattern: &str, case: Case) -> Result<Self, FilterError> {
        let mut re = String::from("^");
        let mut literal = String::new();
        for c in pattern.chars() {
            if matches!(c, '*' | '?') {
                re.push_str(&regex::escape(&literal));
                literal.clear();
                re.push_str(if c == '*' { ".*" } else { "." });
            } else {
                literal.push(c);
            }
        }
        re.push_str(&regex::escape(&literal));
        re.push('$');
        Self::compile(&re, case)
    }

    /// 이름의 어딘가가 정규식과 맞는 항목. 잘못된 정규식은 오류 값이다.
    pub fn name_regex(pattern: &str, case: Case) -> Result<Self, FilterError> {
        Self::compile(pattern, case)
    }

    fn compile(pattern: &str, case: Case) -> Result<Self, FilterError> {
        regex::RegexBuilder::new(pattern)
            .case_insensitive(case == Case::Insensitive)
            .dot_matches_new_line(true)
            .build()
            .map(|re| Self(Node::Pattern(re)))
            .map_err(|e| FilterError::InvalidRegex(e.to_string()))
    }

    /// `folder` 아래(하위 폴더 포함)에 있는 항목. 폴더 자신은 맞지 않는다. 경로의 대소문자는 무시한다.
    pub fn under(folder: &str) -> Self {
        let mut path = fold(folder.trim_end_matches(SEPARATOR));
        path.push(SEPARATOR);
        Self(Node::Under(path))
    }

    /// 크기가 `min` 이상 `max` 이하인 항목(`None`은 그쪽 끝이 열려 있다).
    pub fn size(min: Option<u64>, max: Option<u64>) -> Self {
        Self(Node::Size(min, max))
    }

    /// 수정 날짜(`FILETIME`)가 `min` 이상 `max` 이하인 항목.
    pub fn modified(min: Option<u64>, max: Option<u64>) -> Self {
        Self(Node::Modified(min, max))
    }

    /// `mask`의 속성 비트를 모두 가진 항목. 없는 항목은 [`Filter::not`]으로 고른다.
    pub fn attributes(mask: u32) -> Self {
        Self(Node::Attributes(mask))
    }

    /// 확장자가 `extension`인 항목(대소문자 무시, 앞의 점은 있어도 없어도 된다).
    /// 확장자는 이름의 마지막 `.` 뒤다.
    pub fn extension(extension: &str) -> Self {
        let e = extension.strip_prefix('.').unwrap_or(extension);
        Self(Node::Extension(fold(e)))
    }

    /// 모두 맞아야 맞는다. 비었으면 모든 항목과 맞는다.
    pub fn and(filters: impl IntoIterator<Item = Filter>) -> Self {
        Self(Node::All(filters.into_iter().collect()))
    }

    /// 하나라도 맞으면 맞는다. 비었으면 아무 항목과도 맞지 않는다.
    pub fn or(filters: impl IntoIterator<Item = Filter>) -> Self {
        Self(Node::Any(filters.into_iter().collect()))
    }

    /// 맞지 않아야 맞는다.
    #[allow(clippy::should_implement_trait)]
    pub fn not(filter: Filter) -> Self {
        Self(Node::Not(Box::new(filter)))
    }
}

/// 결과를 정렬하는 기준.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    /// 이름. 대소문자를 접은 이름의 글자(유니코드 코드 포인트) 순서다.
    Name,
    /// 전체 경로. 경로 조각(폴더 이름)마다 이름 순서로 비교하므로 폴더 안의 항목이 폴더 바로
    /// 뒤에 모인다.
    Path,
    /// 바이트 크기.
    Size,
    /// 수정 날짜.
    Modified,
}

/// 정렬 방향.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Order {
    /// 작은 것부터(기본).
    #[default]
    Ascending,
    /// 큰 것부터.
    Descending,
}

/// 인덱스에 보내는 질의: 조건 하나(또는 조건 트리)와 정렬, 결과 수 제한.
#[derive(Debug, Clone)]
pub struct Query {
    plan: Plan,
    sort: Option<(SortKey, Order)>,
    limit: Option<usize>,
}

#[derive(Debug, Clone)]
enum Plan {
    /// 대소문자를 무시하는 이름 부분 일치. 접은 이름 버퍼를 한 번에 훑는 빠른 길.
    Contains(String),
    Filter(Filter),
}

impl Query {
    /// 이름에 `part`가 들어 있는(대소문자 무시) 항목을 찾는 질의. 빈 조각은 모든 항목과 맞는다.
    pub fn name_contains(part: &str) -> Self {
        Self {
            plan: Plan::Contains(fold(part)),
            sort: None,
            limit: None,
        }
    }

    /// 조건 트리에 맞는 항목을 찾는 질의.
    pub fn filter(filter: Filter) -> Self {
        let plan = match filter.0 {
            Node::Contains {
                part,
                case: Case::Insensitive,
            } => Plan::Contains(part),
            node => Plan::Filter(Filter(node)),
        };
        Self {
            plan,
            sort: None,
            limit: None,
        }
    }

    /// 결과를 `key` 기준 `order` 방향으로 정렬한다. 기준 값이 같은 항목끼리는 인덱스에 넣은
    /// 순서다. 정렬을 주지 않으면 결과는 넣은 순서다.
    pub fn sort(mut self, key: SortKey, order: Order) -> Self {
        self.sort = Some((key, order));
        self
    }

    /// 결과를 많아야 `n`개만 받는다. 나머지는 [`Page::next`]로 이어 받는다.
    pub fn limit(mut self, n: usize) -> Self {
        self.limit = Some(n);
        self
    }
}

/// 질의나 이어 받기의 응답: 결과 조각, 전체 개수, 이어 받기 표지.
#[derive(Debug, Clone)]
pub struct Page {
    /// 이번 조각의 결과. 정렬을 주었으면 정렬 순서다.
    pub hits: Vec<Hit>,
    /// 질의에 맞는 결과 전체의 개수(결과 수 제한과 상관없다). 이어 받아도 처음 질의의 값이다.
    pub total: usize,
    /// 남은 결과가 있으면 그것을 받을 표지. 다 받았으면 `None`.
    pub next: Option<Cursor>,
}

/// 이어 받기 표지. 앞선 질의에서 아직 내보내지 않은 결과를 들고 있다가
/// [`Index::resume`]으로 다음 조각을 낸다.
///
/// 표지를 만든 뒤 인덱스가 바뀌면(항목을 넣는 등) 표지는 무효가 되어
/// [`ResumeError::Stale`]을 낸다. 다른 인덱스에 내밀어도 무효다.
#[derive(Debug, Clone)]
pub struct Cursor {
    /// 표지를 만들 때의 인덱스 판 번호.
    stamp: u64,
    sort: Option<(SortKey, Order)>,
    total: usize,
    /// 아직 내보내지 않은 결과의 레코드 위치. 정렬이 없으면 넣은 순서이고, 정렬이 있으면
    /// 순서가 정해져 있지 않다(조각을 낼 때마다 앞쪽만 골라 정렬한다).
    rest: Vec<u32>,
}

/// 이어 받지 못한 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeError {
    /// 표지를 만든 뒤 인덱스가 바뀌었거나 다른 인덱스의 표지다. 다시 질의해야 한다.
    Stale,
}

impl fmt::Display for ResumeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Stale => "인덱스가 바뀌어 이어 받을 수 없습니다. 다시 질의하십시오",
        })
    }
}

impl std::error::Error for ResumeError {}

/// 인덱스 판 번호를 내는 프로세스 전체의 계수기. 인덱스가 만들어지거나 바뀔 때마다 새 값을
/// 받으므로, 서로 다른 인덱스나 바뀌기 전후의 같은 인덱스는 판 번호가 다르다.
static NEXT_STAMP: AtomicU64 = AtomicU64::new(1);

fn new_stamp() -> u64 {
    NEXT_STAMP.fetch_add(1, AtomicOrdering::Relaxed)
}

/// 질의 결과 항목 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// 볼륨 루트부터 만든 전체 경로(예: `C:\문서\보고서.txt`).
    pub path: String,
    /// 항목 이름(넣을 때의 대소문자 그대로).
    pub name: String,
    pub size: u64,
    /// 수정 날짜(`FILETIME`).
    pub modified: u64,
    pub attributes: u32,
}

/// 항목 하나의 고정 크기 레코드(40바이트).
#[derive(Debug, Clone, Copy)]
struct Record {
    /// 항목 번호. USN 레코드가 가리키는 항목을 찾는 데 쓴다.
    number: u64,
    size: u64,
    modified: u64,
    parent: u64,
    /// 접은 이름 버퍼에서 이 항목 이름이 시작하는 위치. 넣은 순서대로 커진다.
    name_start: u32,
    attributes: u32,
}

/// 볼륨 하나의 인덱스.
#[derive(Debug, Clone)]
pub struct Index {
    /// 볼륨 루트 경로. 늘 `\`로 끝난다.
    root: String,
    root_number: u64,
    records: Vec<Record>,
    /// 대소문자를 접은 이름들. 이름마다 뒤에 [`NAME_END`]가 붙는다.
    folded: String,
    /// 접은 이름과 다른 원래 이름들을 이어 붙인 것.
    originals: String,
    /// (레코드 위치, `originals`에서 시작 위치). 레코드 위치 순서다.
    original_starts: Vec<(u32, u32)>,
    /// 폴더의 항목 번호 → 레코드 위치. 같은 번호가 또 오면 처음 것을 쓴다.
    folders: HashMap<u64, u32>,
    /// 판 번호. 만들 때와 바뀔 때마다 [`new_stamp`]로 새로 받는다. 이어 받기 표지가 이 값으로
    /// 무효를 가린다. 복제본은 내용이 같으므로 같은 값을 갖는다.
    stamp: u64,
    /// 지운 레코드 수. 지운 레코드는 자리만 남고 부모가 [`DEAD`]라 어떤 질의에도 나오지 않는다.
    dead: usize,
    /// (항목 번호, 부모 번호) → 레코드 위치. USN 레코드를 처음 적용할 때 만든다.
    /// 같은 폴더에 하드 링크가 여럿이면 마지막 것만 가진다.
    locator: Option<HashMap<(u64, u64), u32>>,
    /// 이름바꾸기 옛 이름 레코드를 받고 새 이름 레코드를 기다리는 항목: 번호 → 옛 부모 번호.
    pending_renames: HashMap<u64, u64>,
    /// 볼륨 저널 상태. 아직 모르면 `None`.
    journal: Option<JournalState>,
}

/// 지운 레코드의 부모 번호. 이 번호의 폴더는 없으므로 지운 레코드는 루트에 닿지 않는다.
const DEAD: u64 = u64::MAX;

impl Index {
    /// 빈 인덱스. `root`는 볼륨 루트 경로(`C:\` 또는 `C:`), `root_number`는 루트 폴더의
    /// 항목 번호(NTFS는 5)다. 루트 자체는 항목으로 넣지 않는다.
    pub fn new(root: &str, root_number: u64) -> Self {
        let mut root = root.to_owned();
        if !root.ends_with(SEPARATOR) {
            root.push(SEPARATOR);
        }
        Self {
            root,
            root_number,
            records: Vec::new(),
            folded: String::new(),
            originals: String::new(),
            original_starts: Vec::new(),
            folders: HashMap::new(),
            stamp: new_stamp(),
            dead: 0,
            locator: None,
            pending_renames: HashMap::new(),
            journal: None,
        }
    }

    /// 항목 하나를 넣는다. 부모는 아직 넣지 않았어도 된다(경로는 질의할 때 만든다).
    ///
    /// 질의할 때 부모를 볼륨 루트까지 따라갈 수 없는 항목(부모가 없거나 폴더가 아니거나
    /// 고리를 이룸)은 결과에 나오지 않는다.
    pub fn insert(&mut self, item: NewItem<'_>) -> Result<(), InsertError> {
        if item.name.is_empty() {
            return Err(InsertError::EmptyName);
        }
        if item.name.contains([NAME_END, SEPARATOR]) {
            return Err(InsertError::InvalidName);
        }
        let folded = fold(item.name);
        let differs = folded != item.name;

        let index = u32::try_from(self.records.len()).map_err(|_| InsertError::Full)?;
        let fits = |used: usize, more: usize| {
            used.checked_add(more)
                .is_some_and(|end| u32::try_from(end).is_ok())
        };
        if !fits(self.folded.len(), folded.len() + 1)
            || (differs && !fits(self.originals.len(), item.name.len()))
        {
            return Err(InsertError::Full);
        }

        let name_start = self.folded.len() as u32;
        self.folded.push_str(&folded);
        self.folded.push(NAME_END);
        if differs {
            self.original_starts
                .push((index, self.originals.len() as u32));
            self.originals.push_str(item.name);
        }
        self.records.push(Record {
            number: item.number,
            size: item.size,
            modified: item.modified,
            parent: item.parent,
            name_start,
            attributes: item.attributes,
        });
        if item.attributes & ATTRIBUTE_DIRECTORY != 0 {
            self.folders.entry(item.number).or_insert(index);
        }
        if let Some(locator) = &mut self.locator {
            locator.insert((item.number, item.parent), index);
        }
        self.changed();
        Ok(())
    }

    /// 인덱스가 바뀌었다고 표시해 앞서 낸 이어 받기 표지를 무효로 만든다.
    /// 인덱스를 바꾸는 공개 메서드는 모두 바꾼 뒤 이것을 부른다.
    fn changed(&mut self) {
        self.stamp = new_stamp();
    }

    /// 들어 있는 항목 수(경로를 만들 수 없는 항목도 센다, 지운 항목은 세지 않는다).
    pub fn len(&self) -> usize {
        self.records.len() - self.dead
    }

    /// 항목이 하나도 없는가.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 인덱스가 잡고 있는 힙 메모리의 바이트 수. 할당한 용량으로 세므로 실제보다 작지 않다.
    pub fn memory_bytes(&self) -> usize {
        // std `HashMap`은 버킷 수가 2의 거듭제곱이고 7/8까지 채운다. 버킷마다 값 하나와 제어 바이트 하나.
        let folder_buckets = match self.folders.capacity() {
            0 => 0,
            cap => (cap * 8).div_ceil(7).next_power_of_two(),
        };
        size_of::<Self>()
            + self.root.capacity()
            + self.records.capacity() * size_of::<Record>()
            + self.folded.capacity()
            + self.originals.capacity()
            + self.original_starts.capacity() * size_of::<(u32, u32)>()
            + folder_buckets * (size_of::<(u64, u32)>() + 1)
            + self.locator.as_ref().map_or(0, |l| {
                let buckets = match l.capacity() {
                    0 => 0,
                    cap => (cap * 8).div_ceil(7).next_power_of_two(),
                };
                buckets * (size_of::<((u64, u64), u32)>() + 1)
            })
    }

    /// 질의를 실행해 결과 조각만 돌려준다. [`Index::search`]의 `hits`와 같다.
    pub fn query(&self, query: &Query) -> Vec<Hit> {
        self.search(query).hits
    }

    /// 질의를 실행해 결과 조각, 전체 개수, 이어 받기 표지를 돌려준다.
    ///
    /// 맞는 항목을 모두 찾아 세지만, 정렬은 결과 수 제한만큼의 앞쪽만 골라서 하고 경로 문자열도
    /// 내보낼 결과에만 만든다. 그래서 결과가 많아도 제한이 작으면 빠르다.
    pub fn search(&self, query: &Query) -> Page {
        let rest = self.matching(&query.plan);
        let cursor = Cursor {
            stamp: self.stamp,
            sort: query.sort,
            total: rest.len(),
            rest,
        };
        self.page(cursor, query.limit.unwrap_or(usize::MAX))
    }

    /// 이어 받기 표지로 다음 결과 조각을 많아야 `limit`개 받는다.
    ///
    /// 표지를 만든 뒤 인덱스가 바뀌었거나 다른 인덱스의 표지면 [`ResumeError::Stale`]이다.
    /// 그때는 다시 질의한다.
    pub fn resume(&self, cursor: Cursor, limit: usize) -> Result<Page, ResumeError> {
        if cursor.stamp != self.stamp {
            return Err(ResumeError::Stale);
        }
        Ok(self.page(cursor, limit))
    }

    /// 표지가 든 결과에서 앞쪽 `limit`개를 꺼내 조각을 만든다.
    fn page(&self, mut cursor: Cursor, limit: usize) -> Page {
        let take = limit.min(cursor.rest.len());
        let front = match cursor.sort {
            None => cursor.rest.drain(..take).collect(),
            Some((SortKey::Path, order)) => {
                // 열쇠는 (부모 폴더의 조상 열, 항목 자신)이다. 조상 열은 볼륨 루트 바로 아래
                // 조상부터 부모까지의 레코드 위치로, 부모 폴더마다 한 번만 만들어 나눠 쓴다.
                // 경로 문자열은 만들지 않고, 비교할 때 조각 이름을 버퍼에서 바로 읽는다.
                let mut folder_keys: HashMap<u32, Rc<[u32]>> = HashMap::new();
                let at_root: Rc<[u32]> = Rc::from([]);
                let mut keyed: Vec<(Rc<[u32]>, u32)> = cursor
                    .rest
                    .iter()
                    .map(|&at| {
                        let parent = self.records[at as usize].parent;
                        let key = match self.folders.get(&parent) {
                            Some(&folder) if parent != self.root_number => folder_keys
                                .entry(folder)
                                .or_insert_with(|| self.path_key(folder as usize).into())
                                .clone(),
                            _ => at_root.clone(),
                        };
                        (key, at)
                    })
                    .collect();
                let front = take_smallest(&mut keyed, take, |a, b| {
                    let a_path = a.0.iter().chain([&a.1]);
                    let b_path = b.0.iter().chain([&b.1]);
                    directed(self.compare_paths(a_path, b_path), order).then(a.1.cmp(&b.1))
                });
                cursor.rest = keyed.into_iter().map(|(_, at)| at).collect();
                front.into_iter().map(|(_, at)| at).collect()
            }
            Some((key, order)) => take_smallest(&mut cursor.rest, take, |&a, &b| {
                directed(self.compare(key, a as usize, b as usize), order).then(a.cmp(&b))
            }),
        };
        let hits = front
            .into_iter()
            .filter_map(|at| self.hit(at as usize))
            .collect();
        let total = cursor.total;
        let next = (!cursor.rest.is_empty()).then_some(cursor);
        Page { hits, total, next }
    }

    /// 두 레코드를 `key`(경로 말고) 기준으로 비교한다.
    fn compare(&self, key: SortKey, a: usize, b: usize) -> Ordering {
        let (ra, rb) = (&self.records[a], &self.records[b]);
        match key {
            SortKey::Size => ra.size.cmp(&rb.size),
            SortKey::Modified => ra.modified.cmp(&rb.modified),
            SortKey::Name | SortKey::Path => self
                .folded_name(a)
                .cmp(self.folded_name(b))
                .then_with(|| self.name(a).cmp(self.name(b))),
        }
    }

    /// 경로 정렬 열쇠: 볼륨 루트 바로 아래 조상부터 항목 자신까지의 레코드 위치.
    fn path_key(&self, index: usize) -> Vec<u32> {
        let mut key: Vec<u32> = self
            .chain(index)
            .unwrap_or_default()
            .into_iter()
            .map(|at| at as u32)
            .collect();
        key.reverse();
        key
    }

    /// 두 경로 정렬 열쇠를 경로 조각마다 접은 이름으로 비교한다. 앞 조각이 모두 같으면
    /// 짧은 쪽(폴더 자신)이 앞이다.
    fn compare_paths<'k>(
        &self,
        mut a: impl Iterator<Item = &'k u32>,
        mut b: impl Iterator<Item = &'k u32>,
    ) -> Ordering {
        loop {
            match (a.next(), b.next()) {
                (Some(x), Some(y)) if x == y => {}
                (Some(&x), Some(&y)) => match self.compare(SortKey::Name, x as usize, y as usize) {
                    Ordering::Equal => {}
                    other => return other,
                },
                (Some(_), None) => return Ordering::Greater,
                (None, Some(_)) => return Ordering::Less,
                (None, None) => return Ordering::Equal,
            }
        }
    }

    /// 질의에 맞고 볼륨 루트까지 경로를 만들 수 있는 레코드 위치들(넣은 순서).
    fn matching(&self, plan: &Plan) -> Vec<u32> {
        let mut found = Vec::new();
        let mut reach = Reach::new(self);
        let folded = match plan {
            Plan::Contains(folded) => folded,
            Plan::Filter(filter) => {
                for index in 0..self.records.len() {
                    let mut item = Item::new(self, index);
                    if self.matches(filter, &mut item) && item.reachable(&mut reach) {
                        found.push(index as u32);
                    }
                }
                return found;
            }
        };
        if folded.contains(NAME_END) {
            return found;
        }

        if folded.is_empty() {
            found.extend(
                (0..self.records.len())
                    .filter(|&index| reach.reachable(index))
                    .map(|index| index as u32),
            );
            return found;
        }

        // 접은 이름 버퍼 전체를 한 번에 훑는다. 찾을 조각에 `\0`이 없으므로 이름 경계를 넘는
        // 일치는 생기지 않는다. 한 이름에서 맞으면 그 이름의 나머지는 건너뛴다.
        let finder = memmem::Finder::new(folded.as_bytes());
        let haystack = self.folded.as_bytes();
        let mut from = 0;
        while let Some(found_at) = finder.find(&haystack[from..]) {
            let index = self.record_at(from + found_at);
            if reach.reachable(index) {
                found.push(index as u32);
            }
            from = self.folded_name_end(index) + 1;
        }
        found
    }

    /// 조건 트리를 항목 하나에 평가한다.
    fn matches(&self, filter: &Filter, item: &mut Item<'_>) -> bool {
        let record = self.records[item.index];
        match &filter.0 {
            Node::Contains { part, case } => match case {
                Case::Insensitive => item.folded_name().contains(part.as_str()),
                Case::Sensitive => self.name(item.index).contains(part.as_str()),
            },
            Node::Pattern(re) => re.is_match(self.name(item.index)),
            Node::Under(prefix) => item
                .folded_path()
                .is_some_and(|p| p.len() > prefix.len() && p.starts_with(prefix.as_str())),
            Node::Size(min, max) => in_range(record.size, *min, *max),
            Node::Modified(min, max) => in_range(record.modified, *min, *max),
            Node::Attributes(mask) => record.attributes & mask == *mask,
            Node::Extension(ext) => {
                let name = item.folded_name();
                name.rfind('.').is_some_and(|dot| &name[dot + 1..] == ext)
            }
            Node::All(fs) => fs.iter().all(|f| self.matches(f, item)),
            Node::Any(fs) => fs.iter().any(|f| self.matches(f, item)),
            Node::Not(f) => !self.matches(f, item),
        }
    }

    /// 접은 이름 버퍼의 `offset`이 속한 레코드 위치.
    fn record_at(&self, offset: usize) -> usize {
        self.records
            .partition_point(|r| r.name_start as usize <= offset)
            - 1
    }

    /// `index`번 레코드의 접은 이름이 끝나는 위치(뒤따르는 [`NAME_END`]의 위치).
    fn folded_name_end(&self, index: usize) -> usize {
        match self.records.get(index + 1) {
            Some(next) => next.name_start as usize - 1,
            None => self.folded.len() - 1,
        }
    }

    /// `index`번 레코드의 원래 이름.
    fn name(&self, index: usize) -> &str {
        let key = index as u32;
        match self.original_starts.binary_search_by_key(&key, |&(i, _)| i) {
            Ok(at) => {
                let start = self.original_starts[at].1 as usize;
                let end = self
                    .original_starts
                    .get(at + 1)
                    .map_or(self.originals.len(), |&(_, s)| s as usize);
                &self.originals[start..end]
            }
            Err(_) => {
                let start = self.records[index].name_start as usize;
                &self.folded[start..self.folded_name_end(index)]
            }
        }
    }

    /// `index`번 레코드의 접은 이름.
    fn folded_name(&self, index: usize) -> &str {
        let start = self.records[index].name_start as usize;
        &self.folded[start..self.folded_name_end(index)]
    }

    /// `index`번 레코드부터 볼륨 루트 바로 아래 조상까지의 레코드 위치들.
    /// 볼륨 루트까지 부모를 따라갈 수 없으면 `None`.
    fn chain(&self, index: usize) -> Option<Vec<usize>> {
        let mut chain = vec![index];
        let mut parent = self.records[index].parent;
        while parent != self.root_number {
            // 폴더 수보다 깊으면 고리다.
            if chain.len() > self.folders.len() {
                return None;
            }
            let &at = self.folders.get(&parent)?;
            chain.push(at as usize);
            parent = self.records[at as usize].parent;
        }
        Some(chain)
    }

    /// `index`번 레코드의 전체 경로. 볼륨 루트까지 부모를 따라갈 수 없으면 `None`.
    fn path(&self, index: usize) -> Option<String> {
        let chain = self.chain(index)?;
        let mut path = self.root.clone();
        for (n, &at) in chain.iter().rev().enumerate() {
            if n > 0 {
                path.push(SEPARATOR);
            }
            path.push_str(self.name(at));
        }
        Some(path)
    }

    /// `index`번 레코드의 결과. 볼륨 루트까지 부모를 따라갈 수 없으면 `None`.
    fn hit(&self, index: usize) -> Option<Hit> {
        self.path(index).map(|path| self.hit_with(index, path))
    }

    fn hit_with(&self, index: usize, path: String) -> Hit {
        let record = self.records[index];
        Hit {
            path,
            name: self.name(index).to_owned(),
            size: record.size,
            modified: record.modified,
            attributes: record.attributes,
        }
    }
}

/// 조건을 평가하는 동안의 항목 하나. 경로는 `Under`가 물을 때만 만든다.
struct Item<'a> {
    owner: &'a Index,
    index: usize,
    /// 아직 만들지 않았으면 바깥 `None`, 만들었으면 (경로, 접은 경로). 경로를 못 만들면 안쪽 `None`.
    path: Option<Option<(String, String)>>,
}

impl<'a> Item<'a> {
    fn new(owner: &'a Index, index: usize) -> Self {
        Self {
            owner,
            index,
            path: None,
        }
    }

    fn folded_name(&self) -> &'a str {
        self.owner.folded_name(self.index)
    }

    fn built_path(&mut self) -> &Option<(String, String)> {
        let (owner, index) = (self.owner, self.index);
        self.path
            .get_or_insert_with(|| owner.path(index).map(|p| (p.clone(), fold(&p))))
    }

    fn folded_path(&mut self) -> Option<&str> {
        self.built_path().as_ref().map(|(_, f)| f.as_str())
    }

    /// 볼륨 루트까지 경로를 만들 수 있는가. 경로를 이미 만들었으면 그 결과를 쓴다.
    fn reachable(&self, reach: &mut Reach<'_>) -> bool {
        match &self.path {
            Some(built) => built.is_some(),
            None => reach.reachable(self.index),
        }
    }
}

/// 질의 하나 동안 폴더마다 볼륨 루트까지 닿는지 기억해 두고 항목이 닿는지 가린다.
/// [`Index::chain`]과 같은 판정을, 같은 폴더를 두 번 따라가지 않고 한다.
struct Reach<'a> {
    owner: &'a Index,
    /// 레코드 위치마다 [`Reach::UNKNOWN`]·[`Reach::VISITING`]·[`Reach::YES`]·[`Reach::NO`].
    /// 폴더 레코드에만 쓴다.
    memo: Vec<u8>,
    /// 따라가는 중인 폴더들. 판정이 나면 모두 같은 값으로 적는다.
    walked: Vec<usize>,
}

impl<'a> Reach<'a> {
    const UNKNOWN: u8 = 0;
    const VISITING: u8 = 1;
    const YES: u8 = 2;
    const NO: u8 = 3;

    fn new(owner: &'a Index) -> Self {
        Self {
            owner,
            memo: vec![Self::UNKNOWN; owner.records.len()],
            walked: Vec::new(),
        }
    }

    /// `index`번 레코드가 부모를 따라 볼륨 루트까지 닿는가. 부모가 없거나 폴더가 아니거나
    /// 고리를 이루면 닿지 않는다.
    fn reachable(&mut self, index: usize) -> bool {
        let owner = self.owner;
        let mut parent = owner.records[index].parent;
        let reached = loop {
            if parent == owner.root_number {
                break true;
            }
            let Some(&at) = owner.folders.get(&parent) else {
                break false;
            };
            let at = at as usize;
            match self.memo[at] {
                Self::YES => break true,
                // 이번에 따라가는 중인 폴더를 다시 만나면 고리다.
                Self::NO | Self::VISITING => break false,
                _ => {}
            }
            self.memo[at] = Self::VISITING;
            self.walked.push(at);
            parent = owner.records[at].parent;
        };
        let mark = if reached { Self::YES } else { Self::NO };
        for at in self.walked.drain(..) {
            self.memo[at] = mark;
        }
        reached
    }
}

/// 비교 결과를 정렬 방향에 맞춘다.
fn directed(ordering: Ordering, order: Order) -> Ordering {
    match order {
        Order::Ascending => ordering,
        Order::Descending => ordering.reverse(),
    }
}

/// `items`에서 `cmp` 순서로 가장 작은 `n`개를 정렬해 꺼내고, 나머지는 순서 없이 남긴다.
/// 전체를 정렬하지 않고 고르기(`select_nth_unstable`)로 앞쪽만 가르므로 O(전체 + n log n)이다.
fn take_smallest<T>(
    items: &mut Vec<T>,
    n: usize,
    mut cmp: impl FnMut(&T, &T) -> Ordering,
) -> Vec<T> {
    let n = n.min(items.len());
    if n == 0 {
        return Vec::new();
    }
    if n < items.len() {
        items.select_nth_unstable_by(n - 1, &mut cmp);
    }
    let rest = items.split_off(n);
    let mut front = std::mem::replace(items, rest);
    front.sort_unstable_by(cmp);
    front
}

fn in_range(value: u64, min: Option<u64>, max: Option<u64>) -> bool {
    min.is_none_or(|m| value >= m) && max.is_none_or(|m| value <= m)
}

/// 대소문자를 접는다. 글자마다 따로 소문자로 바꾸므로 앞뒤 글자에 따라 결과가 달라지지 않는다.
fn fold(s: &str) -> String {
    s.chars().flat_map(char::to_lowercase).collect()
}
