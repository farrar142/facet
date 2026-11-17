//! 일괄 이름 바꾸기의 규칙과 미리 보기(순수 함수, 화면·파일 시스템·시계를 부르지 않음, ADR-0007).
//!
//! S14a 규칙(`RenameRule`, `preview`)은 찾아 바꾸기 → 앞·뒤 붙이기 → 번호 매기기를 파일의 줄기(확장자 앞)에만 쓴다.
//! 규칙 목록(`Rule`, `preview_rules`)은 켜진 규칙을 차례로 이름 부분·확장자·전체에 쓴다(`RenameRule::to_rules`가 S14a
//! 규칙을 같은 결과의 목록으로 바꿈).
//! 겹침 검사는 폴더별 해시 집합이라 항목·이름 수에 거의 비례한다.

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::name_input::{NameError, validate_name};

/// 이름 규칙. 쓰는 차례: 찾아 바꾸기 → 붙이기·번호. 모든 필드가 비면(기본값) 아무 이름도 바꾸지 않는다.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenameRule {
    /// 찾을 글자. 비면 찾아 바꾸기를 하지 않는다.
    pub find: String,
    /// 바꿀 글자. 비면 찾은 글자를 지운다.
    pub replace: String,
    /// 앞에 붙일 글자.
    pub prefix: String,
    /// 뒤에(확장자 앞에) 붙일 글자.
    pub suffix: String,
    /// 번호 매기기. `None`이면 번호를 붙이지 않는다.
    pub numbering: Option<Numbering>,
}

/// 번호 매기기 설정.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Numbering {
    /// 첫 항목의 번호. 항목 목록의 i번째(0부터) 항목은 `start + i`다.
    pub start: u64,
    /// 최소 자릿수. 모자라면 앞을 0으로 채운다(0·1이면 채우지 않음, 넘치면 그대로 길어짐).
    pub digits: usize,
    /// 번호를 줄기의 앞에 둘지 뒤에 둘지.
    pub position: NumberPosition,
    /// 번호와 줄기 사이에 넣을 글자(비면 붙여 씀).
    pub separator: String,
}

/// 번호의 자리.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberPosition {
    Start,
    End,
}

/// 바꿀 항목 하나: 든 폴더, 원래 이름, 폴더인가.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchItem {
    pub folder: PathBuf,
    pub name: OsString,
    pub is_folder: bool,
}

/// 겹침을 볼 때 쓰는 폴더마다의 이름들(바꿀 항목이 아닌 다른 항목 포함, 숨김 항목 포함).
#[derive(Debug, Clone, Default)]
pub struct ExistingNames {
    folders: HashMap<String, HashSet<String>>,
}

impl ExistingNames {
    pub fn new() -> Self {
        Self::default()
    }

    /// `folder`에 `name` 항목이 있다고 적는다. 같은 이름(대소문자 무시)을 여러 번 넣어도 하나다.
    pub fn insert(&mut self, folder: &Path, name: &OsStr) {
        self.folders
            .entry(folder_key(folder))
            .or_default()
            .insert(name_key(&name.to_string_lossy()));
    }
}

/// 규칙을 쓴 결과(아무것도 바꾸지 않는다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    entries: Vec<PreviewEntry>,
    renames: usize,
    unchanged: usize,
    problems: usize,
    rule_problems: Vec<RuleProblem>,
}

impl Preview {
    /// 항목마다(넘긴 차례 그대로) 결과.
    pub fn entries(&self) -> &[PreviewEntry] {
        &self.entries
    }

    /// 상태가 `EntryStatus::Rename`인 항목 수.
    pub fn rename_count(&self) -> usize {
        self.renames
    }

    /// 상태가 `EntryStatus::Unchanged`인 항목 수(건너뛸 항목).
    pub fn unchanged_count(&self) -> usize {
        self.unchanged
    }

    /// 상태가 `EntryStatus::Problem`인 항목 수.
    pub fn problem_count(&self) -> usize {
        self.problems
    }

    /// 실행할 수 있는가: 문제가 하나도 없고 바꿀 항목이 하나 이상.
    pub fn can_execute(&self) -> bool {
        self.problems == 0 && self.renames > 0
    }
}

/// 항목 하나의 미리 보기.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewEntry {
    pub folder: PathBuf,
    pub original: OsString,
    /// 바뀔 이름(그대로면 원래 이름과 같다).
    pub new_name: String,
    pub status: EntryStatus,
}

/// 항목의 상태.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryStatus {
    /// 이름이 바뀐다.
    Rename,
    /// 규칙이 이름을 바꾸지 않는다(바뀔 이름이 원래 이름과 글자 그대로 같음). 건너뛴다.
    Unchanged,
    /// 바꿀 수 없다. 하나라도 있으면 실행할 수 없다.
    Problem(BatchProblem),
}

/// 바꿀 수 없는 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchProblem {
    /// 쓸 수 없는 이름(`validate_name`의 까닭 그대로).
    Invalid(NameError),
    /// 같은 폴더의 다른 바뀌는 항목과 바뀔 이름이 같다(대소문자 무시). 겹치는 항목 모두에 붙는다.
    Duplicate,
    /// 바뀔 이름이 같은 폴더에 남아 있을 다른 항목의 이름과 같다(대소문자 무시).
    NameTaken,
    /// 원래 이름이 유니코드로 읽을 수 없는 이름이라 규칙을 쓸 수 없다.
    NotUnicode,
}

/// 소문자 한 글자로 바뀌면 그 글자, 아니면 그대로(`name_match`와 같은 단순 접기).
fn fold(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(one), None) => one,
        _ => c,
    }
}

fn name_key(name: &str) -> String {
    name.chars().map(fold).collect()
}

fn folder_key(folder: &Path) -> String {
    let text = folder.to_string_lossy();
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text.into_owned()
    }
}

/// 줄기와 확장자(점 포함)로 가른다. `name_input::extension_changed`와 같은 규칙.
fn split_stem(name: &str, is_folder: bool) -> (&str, &str) {
    if is_folder {
        return (name, "");
    }
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < name.len() => name.split_at(dot),
        _ => (name, ""),
    }
}

/// 줄기에서 `find`(이미 접은 글자들)를 왼쪽부터 겹치지 않게 `replace`로 바꾼다(`all`이 거짓이면 첫 번째만).
fn replace_folded(stem: &str, find: &[char], replace: &str, all: bool) -> String {
    let chars: Vec<char> = stem.chars().collect();
    let folded: Vec<char> = chars.iter().copied().map(fold).collect();
    let mut out = String::with_capacity(stem.len());
    let mut i = 0;
    let mut replaced = false;
    while i < chars.len() {
        if (all || !replaced) && i + find.len() <= chars.len() && folded[i..i + find.len()] == *find
        {
            out.push_str(replace);
            i += find.len();
            replaced = true;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn new_name_for(
    item_name: &str,
    is_folder: bool,
    index: usize,
    rule: &RenameRule,
    find: &[char],
) -> String {
    let (stem, ext) = split_stem(item_name, is_folder);
    let replaced;
    let stem = if find.is_empty() {
        stem
    } else {
        replaced = replace_folded(stem, find, &rule.replace, true);
        &replaced
    };
    let mut out = String::with_capacity(item_name.len() + rule.prefix.len() + rule.suffix.len());
    out.push_str(&rule.prefix);
    match &rule.numbering {
        None => out.push_str(stem),
        Some(n) => {
            let number = n.start.saturating_add(index as u64);
            let number = format!("{number:0>width$}", width = n.digits);
            match n.position {
                NumberPosition::Start => {
                    out.push_str(&number);
                    out.push_str(&n.separator);
                    out.push_str(stem);
                }
                NumberPosition::End => {
                    out.push_str(stem);
                    out.push_str(&n.separator);
                    out.push_str(&number);
                }
            }
        }
    }
    out.push_str(&rule.suffix);
    out.push_str(ext);
    out
}

/// 폴더마다: 이름을 비우는 항목, 그대로 남는 항목, 바뀔 이름의 개수.
#[derive(Default)]
struct FolderNames {
    removed: HashSet<String>,
    kept: HashSet<String>,
    targets: HashMap<String, usize>,
}

/// 규칙을 항목들에 써 본다(아무것도 바꾸지 않는다).
pub fn preview(items: &[BatchItem], rule: &RenameRule, existing: &ExistingNames) -> Preview {
    let find: Vec<char> = rule.find.chars().map(fold).collect();
    let entries = initial_entries(items, |original, is_folder, index| {
        new_name_for(original, is_folder, index, rule, &find)
    });
    check(entries, existing, Vec::new())
}

/// 항목마다 새 이름을 만들고 이름 하나만 보는 상태(그대로·쓸 수 없음·유니코드 아님)를 붙인다.
fn initial_entries(
    items: &[BatchItem],
    mut new_name: impl FnMut(&str, bool, usize) -> String,
) -> Vec<PreviewEntry> {
    let mut entries: Vec<PreviewEntry> = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let (new_name, status) = match item.name.to_str() {
            None => (
                item.name.to_string_lossy().into_owned(),
                EntryStatus::Problem(BatchProblem::NotUnicode),
            ),
            Some(original) => {
                let new_name = new_name(original, item.is_folder, index);
                let status = if new_name == original {
                    EntryStatus::Unchanged
                } else if let Err(reason) = validate_name(&new_name) {
                    EntryStatus::Problem(BatchProblem::Invalid(reason))
                } else {
                    EntryStatus::Rename
                };
                (new_name, status)
            }
        };
        entries.push(PreviewEntry {
            folder: item.folder.clone(),
            original: item.name.clone(),
            new_name,
            status,
        });
    }
    entries
}

/// 폴더마다 겹침·남는 이름을 보고 수치를 센다.
fn check(
    mut entries: Vec<PreviewEntry>,
    existing: &ExistingNames,
    rule_problems: Vec<RuleProblem>,
) -> Preview {
    let mut folders: HashMap<String, FolderNames> = HashMap::new();
    let mut keys: Vec<(String, String)> = Vec::with_capacity(entries.len());
    for entry in &entries {
        let fkey = folder_key(&entry.folder);
        let original = name_key(&entry.original.to_string_lossy());
        let folder = folders.entry(fkey.clone()).or_default();
        match entry.status {
            EntryStatus::Unchanged | EntryStatus::Problem(BatchProblem::NotUnicode) => {
                folder.kept.insert(original);
            }
            EntryStatus::Rename | EntryStatus::Problem(_) => {
                folder.removed.insert(original);
            }
        }
        let target = name_key(&entry.new_name);
        if entry.status == EntryStatus::Rename {
            *folder.targets.entry(target.clone()).or_insert(0) += 1;
        }
        keys.push((fkey, target));
    }

    let (mut renames, mut unchanged, mut problems) = (0, 0, 0);
    for (entry, (fkey, target)) in entries.iter_mut().zip(&keys) {
        if entry.status == EntryStatus::Rename {
            let folder = &folders[fkey];
            if folder.targets[target] > 1 {
                entry.status = EntryStatus::Problem(BatchProblem::Duplicate);
            } else {
                let existing_here = existing.folders.get(fkey).is_some_and(|names| {
                    names.contains(target) && !folder.removed.contains(target)
                });
                if existing_here || folder.kept.contains(target) {
                    entry.status = EntryStatus::Problem(BatchProblem::NameTaken);
                }
            }
        }
        match entry.status {
            EntryStatus::Rename => renames += 1,
            EntryStatus::Unchanged => unchanged += 1,
            EntryStatus::Problem(_) => problems += 1,
        }
    }

    Preview {
        entries,
        renames,
        unchanged,
        problems,
        rule_problems,
    }
}

/// #272: 규칙 목록의 규칙 하나(종류·켜짐·적용 대상).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub kind: RuleKind,
    pub enabled: bool,
    pub target: Target,
}

impl Rule {
    /// #272: 켜짐 `true`, 대상 `Target::Stem`인 규칙.
    pub fn new(kind: RuleKind) -> Rule {
        Rule {
            kind,
            enabled: true,
            target: Target::Stem,
        }
    }
}

/// #272: 규칙의 적용 대상(이름 부분·확장자·전체).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Target {
    #[default]
    Stem,
    Extension,
    Whole,
}

/// #272: 자리를 세는 끝.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Start,
    End,
}

/// #272: 번호·날짜를 넣을 자리.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    Start,
    End,
    At { from: Edge, at: usize },
}

/// #272: 대소문자 규칙의 방식.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseMode {
    Lower,
    Upper,
    Title,
    Sentence,
}

/// #272: 규칙 종류.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleKind {
    Replace {
        find: String,
        replace: String,
        regex: bool,
        case_sensitive: bool,
        all: bool,
    },
    Insert {
        text: String,
        from: Edge,
        at: usize,
    },
    Delete {
        from: Edge,
        at: usize,
        count: usize,
    },
    Number {
        start: u64,
        step: u64,
        digits: usize,
        place: Place,
        separator: String,
    },
    Case(CaseMode),
    Extension {
        new: Option<String>,
    },
    Date {
        format: String,
        place: Place,
        separator: String,
    },
}

/// #272: 항목 밖에서 오는 값(항목마다 수정 시각 `FILETIME`, UTC와의 차이 초).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleContext {
    pub modified: Vec<Option<u64>>,
    pub utc_offset_seconds: i64,
}

/// #272: 적용하지 못한 규칙(`rule`은 넘긴 목록의 0부터 센 자리)과 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleProblem {
    pub rule: usize,
    pub error: RuleError,
}

/// #272: 규칙 문제의 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleError {
    Regex(String),
}

impl Preview {
    /// #272: 적용하지 못한 규칙들.
    pub fn rule_problems(&self) -> &[RuleProblem] {
        &self.rule_problems
    }
}

impl RenameRule {
    /// #272: 같은 결과를 내는 규칙 목록: 찾아 바꾸기(글자, 대소문자 무시, 모두) → 번호(증가 1) →
    /// 앞에 붙이기(앞 0번째 자리 넣기) → 뒤에 붙이기(뒤 0번째 자리 넣기), 모두 이름 부분 대상.
    /// 빈 찾을 글자·빈 붙일 글자·번호 없음은 규칙을 만들지 않는다.
    pub fn to_rules(&self) -> Vec<Rule> {
        let mut rules = Vec::new();
        if !self.find.is_empty() {
            rules.push(Rule::new(RuleKind::Replace {
                find: self.find.clone(),
                replace: self.replace.clone(),
                regex: false,
                case_sensitive: false,
                all: true,
            }));
        }
        if let Some(n) = &self.numbering {
            rules.push(Rule::new(RuleKind::Number {
                start: n.start,
                step: 1,
                digits: n.digits,
                place: match n.position {
                    NumberPosition::Start => Place::Start,
                    NumberPosition::End => Place::End,
                },
                separator: n.separator.clone(),
            }));
        }
        if !self.prefix.is_empty() {
            rules.push(Rule::new(RuleKind::Insert {
                text: self.prefix.clone(),
                from: Edge::Start,
                at: 0,
            }));
        }
        if !self.suffix.is_empty() {
            rules.push(Rule::new(RuleKind::Insert {
                text: self.suffix.clone(),
                from: Edge::End,
                at: 0,
            }));
        }
        rules
    }
}

/// 미리 보기 한 번 동안 규칙마다 한 번만 준비한 것(정규식은 여기서 한 번 컴파일).
enum Prepared<'a> {
    /// 적용하지 않음(꺼짐, 빈 찾을 글자, 틀린 정규식, 아직 없는 종류).
    Skip,
    /// 글자 그대로, 대소문자 무시: 접은 찾을 글자.
    Folded {
        find: Vec<char>,
        replace: &'a str,
        all: bool,
    },
    /// 글자 그대로, 대소문자 구분.
    Exact {
        find: &'a str,
        replace: &'a str,
        all: bool,
    },
    Regex {
        regex: regex::Regex,
        replace: &'a str,
        all: bool,
    },
    Insert {
        text: &'a str,
        from: Edge,
        at: usize,
    },
    Number {
        start: u64,
        step: u64,
        digits: usize,
        place: &'a Place,
        separator: &'a str,
    },
    Delete {
        from: Edge,
        at: usize,
        count: usize,
    },
    Case(CaseMode),
    /// 새 확장자(점 뒤 글자). `None`이면 확장자를 지운다. 적용 대상은 무시한다.
    Extension(Option<&'a str>),
    Date {
        format: &'a str,
        place: &'a Place,
        separator: &'a str,
    },
}

fn prepare<'a>(index: usize, rule: &'a Rule, problems: &mut Vec<RuleProblem>) -> Prepared<'a> {
    if !rule.enabled {
        return Prepared::Skip;
    }
    match &rule.kind {
        RuleKind::Replace {
            find,
            replace,
            regex,
            case_sensitive,
            all,
        } => {
            if find.is_empty() {
                Prepared::Skip
            } else if *regex {
                match regex::RegexBuilder::new(find)
                    .case_insensitive(!case_sensitive)
                    .build()
                {
                    Ok(regex) => Prepared::Regex {
                        regex,
                        replace,
                        all: *all,
                    },
                    Err(error) => {
                        problems.push(RuleProblem {
                            rule: index,
                            error: RuleError::Regex(error.to_string()),
                        });
                        Prepared::Skip
                    }
                }
            } else if *case_sensitive {
                Prepared::Exact {
                    find,
                    replace,
                    all: *all,
                }
            } else {
                Prepared::Folded {
                    find: find.chars().map(fold).collect(),
                    replace,
                    all: *all,
                }
            }
        }
        RuleKind::Insert { text, from, at } => Prepared::Insert {
            text,
            from: *from,
            at: *at,
        },
        RuleKind::Number {
            start,
            step,
            digits,
            place,
            separator,
        } => Prepared::Number {
            start: *start,
            step: *step,
            digits: *digits,
            place,
            separator,
        },
        RuleKind::Delete { from, at, count } => Prepared::Delete {
            from: *from,
            at: *at,
            count: *count,
        },
        RuleKind::Case(mode) => Prepared::Case(*mode),
        RuleKind::Extension { new } => Prepared::Extension(
            new.as_deref()
                .map(|e| e.strip_prefix('.').unwrap_or(e))
                .filter(|e| !e.is_empty()),
        ),
        RuleKind::Date {
            format,
            place,
            separator,
        } => Prepared::Date {
            format,
            place,
            separator,
        },
    }
}

/// `text`의 `char` 자리(`from` 끝에서 센 `at`번째, 길이보다 크면 반대쪽 끝)의 바이트 오프셋.
fn char_offset(text: &str, from: Edge, at: usize) -> usize {
    let len = text.chars().count();
    let at = at.min(len);
    let index = match from {
        Edge::Start => at,
        Edge::End => len - at,
    };
    text.char_indices()
        .nth(index)
        .map_or(text.len(), |(offset, _)| offset)
}

/// `from` 끝에서 센 `at`번째 `char` 자리부터 안쪽으로 `count`글자를 지운다(범위 밖은 무시).
fn delete_range(text: &str, from: Edge, at: usize, count: usize) -> String {
    let len = text.chars().count();
    let (start, end) = match from {
        Edge::Start => (at.min(len), at.saturating_add(count).min(len)),
        Edge::End => {
            if at >= len {
                return text.to_string();
            }
            let end = len - at;
            (end.saturating_sub(count), end)
        }
    };
    if start >= end {
        return text.to_string();
    }
    text.chars()
        .enumerate()
        .filter(|(i, _)| *i < start || *i >= end)
        .map(|(_, c)| c)
        .collect()
}

fn change_case(text: &str, mode: CaseMode) -> String {
    match mode {
        CaseMode::Lower => text.to_lowercase(),
        CaseMode::Upper => text.to_uppercase(),
        CaseMode::Title | CaseMode::Sentence => {
            let mut out = String::with_capacity(text.len());
            let mut capital = true;
            for c in text.chars() {
                if capital {
                    out.extend(c.to_uppercase());
                } else {
                    out.extend(c.to_lowercase());
                }
                capital = mode == CaseMode::Title && matches!(c, ' ' | '_' | '-' | '.');
            }
            out
        }
    }
}

fn insert_at(text: &str, from: Edge, at: usize, insert: &str) -> String {
    let offset = char_offset(text, from, at);
    let mut out = String::with_capacity(text.len() + insert.len());
    out.push_str(&text[..offset]);
    out.push_str(insert);
    out.push_str(&text[offset..]);
    out
}

/// 대상 글자 하나에 준비한 규칙 하나를 쓴다(`index`는 항목 목록의 0부터 센 자리).
fn apply_prepared(rule: &Prepared, text: &str, index: usize, time: Option<(u64, i64)>) -> String {
    match rule {
        Prepared::Skip | Prepared::Extension(_) => text.to_string(),
        Prepared::Date {
            format,
            place,
            separator,
        } => {
            // 수정 시각을 모르는 항목은 이 규칙을 건너뛴다.
            let Some((modified, offset)) = time else {
                return text.to_string();
            };
            let date = format_date(format, modified, offset);
            match place {
                Place::Start => format!("{date}{separator}{text}"),
                Place::End => format!("{text}{separator}{date}"),
                Place::At { from, at } => {
                    insert_at(text, *from, *at, &format!("{date}{separator}"))
                }
            }
        }
        Prepared::Delete { from, at, count } => delete_range(text, *from, *at, *count),
        Prepared::Case(mode) => change_case(text, *mode),
        Prepared::Folded { find, replace, all } => replace_folded(text, find, replace, *all),
        Prepared::Exact { find, replace, all } => {
            if *all {
                text.replace(find, replace)
            } else {
                text.replacen(find, replace, 1)
            }
        }
        Prepared::Regex {
            regex,
            replace,
            all,
        } => {
            if *all {
                regex.replace_all(text, *replace).into_owned()
            } else {
                regex.replace(text, *replace).into_owned()
            }
        }
        Prepared::Insert {
            text: insert,
            from,
            at,
        } => insert_at(text, *from, *at, insert),
        Prepared::Number {
            start,
            step,
            digits,
            place,
            separator,
        } => {
            let number = start.saturating_add(step.saturating_mul(index as u64));
            let number = format!("{number:0>digits$}");
            match place {
                Place::Start => format!("{number}{separator}{text}"),
                Place::End => format!("{text}{separator}{number}"),
                Place::At { from, at } => {
                    insert_at(text, *from, *at, &format!("{number}{separator}"))
                }
            }
        }
    }
}

/// #272: 켜진 규칙을 차례로 적용한 미리 보기.
///
/// 이름 부분과 확장자는 처음 이름에서 한 번 가르고(마지막 `.`, `split_stem`) 규칙을 지나는 동안 그 경계를
/// 지킨다: 이름 부분 대상 규칙은 이름 부분만, 확장자 대상 규칙은 점 뒤 글자만 바꾸므로 규칙이 이름 부분에
/// 점을 넣어도 확장자가 바뀌지 않는다. 전체 대상 규칙은 이름 전체를 바꾸고 그 결과를 다시 가른다.
/// 그 밖의 검사(쓸 수 없는 이름·겹침·남는 이름·그대로)는 `preview`와 같다.
pub fn preview_rules(
    items: &[BatchItem],
    rules: &[Rule],
    context: &RuleContext,
    existing: &ExistingNames,
) -> Preview {
    let mut problems = Vec::new();
    let prepared: Vec<(Target, Prepared)> = rules
        .iter()
        .enumerate()
        .map(|(index, rule)| (rule.target, prepare(index, rule, &mut problems)))
        .filter(|(_, rule)| !matches!(rule, Prepared::Skip))
        .collect();

    let entries = initial_entries(items, |original, is_folder, index| {
        let (stem, ext) = split_stem(original, is_folder);
        let mut stem = stem.to_string();
        // 점 뒤 글자. `None`이면 확장자가 없다.
        let mut ext = ext.strip_prefix('.').map(str::to_string);
        let time = context
            .modified
            .get(index)
            .copied()
            .flatten()
            .map(|modified| (modified, context.utc_offset_seconds));
        for (target, rule) in &prepared {
            if let Prepared::Extension(new) = rule {
                if !is_folder {
                    ext = new.map(str::to_string);
                }
                continue;
            }
            match target {
                Target::Stem => stem = apply_prepared(rule, &stem, index, time),
                Target::Extension => {
                    if let Some(ext) = ext.as_mut() {
                        *ext = apply_prepared(rule, ext, index, time);
                    }
                }
                Target::Whole => {
                    let whole = join(&stem, ext.as_deref());
                    let whole = apply_prepared(rule, &whole, index, time);
                    let (new_stem, new_ext) = split_stem(&whole, is_folder);
                    stem = new_stem.to_string();
                    ext = new_ext.strip_prefix('.').map(str::to_string);
                }
            }
        }
        join(&stem, ext.as_deref())
    });
    check(entries, existing, problems)
}

/// `FILETIME`을 UTC와의 차이 초만큼 옮긴 지역 시각으로 `format`의 낱말(`yyyy` `yy` `MM` `dd` `HH` `mm` `ss`)을 바꾼다.
fn format_date(format: &str, filetime: u64, offset: i64) -> String {
    const EPOCH_GAP: i64 = 11_644_473_600;
    let seconds = (filetime / 10_000_000) as i64 - EPOCH_GAP + offset;
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    // 1970-01-01 기준 날수 → 그레고리력 날짜(Hinnant의 civil_from_days).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let words: [(&str, String); 7] = [
        ("yyyy", format!("{year:04}")),
        ("yy", format!("{:02}", year.rem_euclid(100))),
        ("MM", format!("{month:02}")),
        ("dd", format!("{day:02}")),
        ("HH", format!("{:02}", rest / 3600)),
        ("mm", format!("{:02}", rest % 3600 / 60)),
        ("ss", format!("{:02}", rest % 60)),
    ];
    let mut out = String::new();
    let mut tail = format;
    'outer: while !tail.is_empty() {
        for (word, value) in &words {
            if let Some(next) = tail.strip_prefix(word) {
                out.push_str(value);
                tail = next;
                continue 'outer;
            }
        }
        let c = tail.chars().next().unwrap();
        out.push(c);
        tail = &tail[c.len_utf8()..];
    }
    out
}

fn join(stem: &str, ext: Option<&str>) -> String {
    match ext {
        None => stem.to_string(),
        Some(ext) => format!("{stem}.{ext}"),
    }
}
