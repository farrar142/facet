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

use std::cmp::Ordering;

use crate::Item;
use crate::file_list::format_modified_local;

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
    compare_folded(a.to_lowercase().as_bytes(), b.to_lowercase().as_bytes())
}

/// 소문자로 바꾼 두 이름의 UTF-8 바이트를 자연 정렬로 비교한다.
///
/// ASCII가 아닌 글자는 모두 문자 무리이고, UTF-8 바이트 순서는 코드 포인트 순서와 같으므로
/// 바이트 단위로 비교해도 된다.
fn compare_folded(a: &[u8], b: &[u8]) -> Ordering {
    let (mut i, mut j) = (0, 0);
    loop {
        let (x, y) = match (a.get(i), b.get(j)) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(&x), Some(&y)) => (x, y),
        };
        if x.is_ascii_digit() && y.is_ascii_digit() {
            let end_a = digit_run_end(a, i);
            let end_b = digit_run_end(b, j);
            let ordering = compare_numbers(&a[i..end_a], &b[j..end_b]);
            if ordering != Ordering::Equal {
                return ordering;
            }
            i = end_a;
            j = end_b;
        } else {
            let ordering = (class(x), x).cmp(&(class(y), y));
            if ordering != Ordering::Equal {
                return ordering;
            }
            i += 1;
            j += 1;
        }
    }
}

fn digit_run_end(bytes: &[u8], start: usize) -> usize {
    bytes[start..]
        .iter()
        .position(|b| !b.is_ascii_digit())
        .map_or(bytes.len(), |n| start + n)
}

/// 숫자 덩어리 둘을 값으로, 값이 같으면 앞자리 0이 많은(긴) 쪽을 앞으로.
fn compare_numbers(a: &[u8], b: &[u8]) -> Ordering {
    let trim = |digits: &[u8]| -> usize {
        digits
            .iter()
            .position(|&d| d != b'0')
            .unwrap_or(digits.len())
    };
    let (va, vb) = (&a[trim(a)..], &b[trim(b)..]);
    va.len()
        .cmp(&vb.len())
        .then_with(|| va.cmp(vb))
        .then_with(|| b.len().cmp(&a.len()))
}

/// 글자 무리: 기호·공백(0) < 숫자(1) < 문자(2). ASCII가 아닌 바이트는 문자다.
fn class(byte: u8) -> u8 {
    if byte.is_ascii_digit() {
        1
    } else if byte.is_ascii_alphabetic() || !byte.is_ascii() {
        2
    } else {
        0
    }
}

/// 정렬에 쓰는 값을 미리 계산해 둔 항목.
#[derive(Debug, Clone)]
struct Entry {
    item: Item,
    /// 소문자로 바꾼 이름.
    folded: Box<str>,
    /// 대문자로 바꾼 확장자. 폴더와 확장자 없는 파일은 `None`.
    extension: Option<Box<str>>,
}

impl Entry {
    fn new(item: Item) -> Self {
        let name = item.name.to_string_lossy();
        let folded = name.to_lowercase().into_boxed_str();
        let extension = if item.is_folder {
            None
        } else {
            extension_upper(&name).map(String::into_boxed_str)
        };
        Entry {
            item,
            folded,
            extension,
        }
    }

    fn compare_names(&self, other: &Self) -> Ordering {
        compare_folded(self.folded.as_bytes(), other.folded.as_bytes())
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
        Column::Type => match (&a.extension, &b.extension) {
            (Some(x), Some(y)) => compare_folded(x.as_bytes(), y.as_bytes()),
            (x, y) => x.is_some().cmp(&y.is_some()),
        },
        // 폴더는 크기를 보이지 않으므로 크기로 가르지 않는다.
        Column::Size if a.item.is_folder => Ordering::Equal,
        Column::Size => a.item.size.cmp(&b.item.size),
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

    /// 지금 정렬 기준.
    pub fn sort(&self) -> Sort {
        self.sort
    }

    /// 정렬 기준을 바꾸고 다시 정렬한다.
    pub fn set_sort(&mut self, sort: Sort) {
        self.sort = sort;
        self.resort();
    }

    fn resort(&mut self) {
        let sort = self.sort;
        self.entries
            .sort_unstable_by(|a, b| compare_entries(a, b, sort));
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
