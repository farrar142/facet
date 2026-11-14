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
//! # 메모리 배치
//!
//! 항목 100만 개를 150MB 미만에 담으려고(spec #22) 항목마다 힙 할당을 하지 않는다.
//!
//! - 이름은 하나의 큰 문자열 버퍼에 이어 붙인다. 버퍼에는 대소문자를 접은 이름을 `\0`으로
//!   구분해 담아, 부분 일치 질의가 버퍼 전체를 한 번에 훑는다. 접은 이름이 원래 이름과
//!   다를 때만 원래 이름을 따로 담는다.
//! - 항목은 고정 크기 레코드(이름 위치, 부모 번호, 크기, 날짜, 속성)로 넣은 순서대로 둔다.
//! - 경로 문자열은 결과를 낼 때만 부모 번호를 따라 만든다. 부모 번호로 찾는 표는 폴더만 담는다.

use std::collections::HashMap;
use std::fmt;
use std::mem::size_of;

use memchr::memmem;

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

/// 인덱스에 보내는 질의: 조건 하나(또는 조건 트리)와 결과 수 제한.
#[derive(Debug, Clone)]
pub struct Query {
    plan: Plan,
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
        Self { plan, limit: None }
    }

    /// 결과를 많아야 `n`개만 받는다.
    pub fn limit(mut self, n: usize) -> Self {
        self.limit = Some(n);
        self
    }
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

/// 항목 하나의 고정 크기 레코드(32바이트).
#[derive(Debug, Clone, Copy)]
struct Record {
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
}

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
            size: item.size,
            modified: item.modified,
            parent: item.parent,
            name_start,
            attributes: item.attributes,
        });
        if item.attributes & ATTRIBUTE_DIRECTORY != 0 {
            self.folders.entry(item.number).or_insert(index);
        }
        Ok(())
    }

    /// 넣은 항목 수(경로를 만들 수 없는 항목도 센다).
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// 항목이 하나도 없는가.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
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
    }

    /// 질의를 실행해 결과를 넣은 순서대로 돌려준다.
    pub fn query(&self, query: &Query) -> Vec<Hit> {
        let limit = query.limit.unwrap_or(usize::MAX);
        let mut hits = Vec::new();
        if limit == 0 {
            return hits;
        }
        let folded = match &query.plan {
            Plan::Contains(folded) => folded,
            Plan::Filter(filter) => {
                for index in 0..self.records.len() {
                    let mut item = Item::new(self, index);
                    if self.matches(filter, &mut item)
                        && let Some(hit) = item.into_hit()
                    {
                        hits.push(hit);
                        if hits.len() >= limit {
                            break;
                        }
                    }
                }
                return hits;
            }
        };
        if folded.contains(NAME_END) {
            return hits;
        }

        if folded.is_empty() {
            for index in 0..self.records.len() {
                if let Some(hit) = self.hit(index) {
                    hits.push(hit);
                    if hits.len() >= limit {
                        break;
                    }
                }
            }
            return hits;
        }

        // 접은 이름 버퍼 전체를 한 번에 훑는다. 찾을 조각에 `\0`이 없으므로 이름 경계를 넘는
        // 일치는 생기지 않는다. 한 이름에서 맞으면 그 이름의 나머지는 건너뛴다.
        let finder = memmem::Finder::new(folded.as_bytes());
        let haystack = self.folded.as_bytes();
        let mut from = 0;
        while let Some(found) = finder.find(&haystack[from..]) {
            let index = self.record_at(from + found);
            if let Some(hit) = self.hit(index) {
                hits.push(hit);
                if hits.len() >= limit {
                    break;
                }
            }
            from = self.folded_name_end(index) + 1;
        }
        hits
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

    /// `index`번 레코드의 전체 경로. 볼륨 루트까지 부모를 따라갈 수 없으면 `None`.
    fn path(&self, index: usize) -> Option<String> {
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
        let owner = self.owner;
        let start = owner.records[self.index].name_start as usize;
        &owner.folded[start..owner.folded_name_end(self.index)]
    }

    fn built_path(&mut self) -> &Option<(String, String)> {
        let (owner, index) = (self.owner, self.index);
        self.path
            .get_or_insert_with(|| owner.path(index).map(|p| (p.clone(), fold(&p))))
    }

    fn folded_path(&mut self) -> Option<&str> {
        self.built_path().as_ref().map(|(_, f)| f.as_str())
    }

    fn into_hit(mut self) -> Option<Hit> {
        self.built_path();
        let (owner, index) = (self.owner, self.index);
        self.path.flatten().map(|(p, _)| owner.hit_with(index, p))
    }
}

fn in_range(value: u64, min: Option<u64>, max: Option<u64>) -> bool {
    min.is_none_or(|m| value >= m) && max.is_none_or(|m| value <= m)
}

/// 대소문자를 접는다. 글자마다 따로 소문자로 바꾸므로 앞뒤 글자에 따라 결과가 달라지지 않는다.
fn fold(s: &str) -> String {
    s.chars().flat_map(char::to_lowercase).collect()
}
