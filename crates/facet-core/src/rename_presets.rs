//! #282: 일괄 이름 바꾸기의 규칙 묶음 저장소(`rename-presets.toml`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::batch_rename::{CaseMode, Edge, Place, Rule, RuleKind, Target};
use toml::{Table, Value};

/// #282: 파일 형식 버전.
pub const FORMAT_VERSION: u64 = 1;

/// #282: 이름 붙인 규칙 묶음 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    pub name: String,
    pub rules: Vec<Rule>,
}

/// #282: 규칙 묶음 모음(묶음들·마지막 규칙·건너뛴 묶음).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Presets {
    presets: Vec<Preset>,
    last: Option<Vec<Rule>>,
    skipped: Vec<SkippedPreset>,
    /// 건너뛴 묶음의 원래 TOML 조각(`skipped`와 같은 차례). 다시 저장할 때 그대로 쓴다.
    skipped_raw: Vec<Table>,
}

fn key(name: &str) -> String {
    name.trim().to_lowercase()
}

impl Presets {
    /// #282: 빈 모음.
    pub fn new() -> Presets {
        Presets::default()
    }

    /// #282: 묶음들(파일·저장 차례).
    pub fn presets(&self) -> &[Preset] {
        &self.presets
    }

    /// #282: 이름으로 찾기(앞뒤 공백 걷고 대소문자 무시).
    pub fn find(&self, name: &str) -> Option<&Preset> {
        let k = key(name);
        self.presets.iter().find(|p| key(&p.name) == k)
    }

    /// #282: 이름 붙여 저장(같은 이름이면 규칙을 바꿈).
    pub fn save_preset(&mut self, name: &str, rules: Vec<Rule>) -> Result<(), PresetError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(PresetError::EmptyName);
        }
        let k = key(name);
        match self.presets.iter_mut().find(|p| key(&p.name) == k) {
            Some(p) => p.rules = rules,
            None => self.presets.push(Preset {
                name: name.to_owned(),
                rules,
            }),
        }
        Ok(())
    }

    /// #282: 묶음 이름 바꾸기.
    pub fn rename_preset(&mut self, from: &str, to: &str) -> Result<(), PresetError> {
        let fk = key(from);
        let Some(index) = self.presets.iter().position(|p| key(&p.name) == fk) else {
            return Err(PresetError::NotFound);
        };
        let to = to.trim();
        if to.is_empty() {
            return Err(PresetError::EmptyName);
        }
        let tk = key(to);
        if self
            .presets
            .iter()
            .enumerate()
            .any(|(i, p)| i != index && key(&p.name) == tk)
        {
            return Err(PresetError::NameTaken);
        }
        self.presets[index].name = to.to_owned();
        Ok(())
    }

    /// #282: 묶음 지우기.
    pub fn delete_preset(&mut self, name: &str) -> Result<(), PresetError> {
        let k = key(name);
        let Some(index) = self.presets.iter().position(|p| key(&p.name) == k) else {
            return Err(PresetError::NotFound);
        };
        self.presets.remove(index);
        Ok(())
    }

    /// #282: 마지막에 쓴 규칙.
    pub fn last(&self) -> Option<&[Rule]> {
        self.last.as_deref()
    }

    /// #282: 마지막에 쓴 규칙 정하기.
    pub fn set_last(&mut self, rules: Vec<Rule>) {
        self.last = Some(rules);
    }

    /// #282: 읽을 때 건너뛴 묶음(파일 차례).
    pub fn skipped(&self) -> &[SkippedPreset] {
        &self.skipped
    }

    /// #282: TOML 글자로.
    pub fn to_toml(&self) -> String {
        let mut root = Table::new();
        root.insert("version".to_owned(), Value::Integer(FORMAT_VERSION as i64));
        if let Some(last) = &self.last {
            let mut t = Table::new();
            if !last.is_empty() {
                t.insert(
                    "rule".to_owned(),
                    Value::Array(last.iter().map(rule_to_value).collect()),
                );
            }
            root.insert("last".to_owned(), Value::Table(t));
        }
        let mut entries: Vec<Value> = self
            .presets
            .iter()
            .map(|p| {
                let mut t = Table::new();
                t.insert("name".to_owned(), Value::String(p.name.clone()));
                if !p.rules.is_empty() {
                    t.insert(
                        "rule".to_owned(),
                        Value::Array(p.rules.iter().map(rule_to_value).collect()),
                    );
                }
                Value::Table(t)
            })
            .collect();
        entries.extend(self.skipped_raw.iter().cloned().map(Value::Table));
        if !entries.is_empty() {
            root.insert("preset".to_owned(), Value::Array(entries));
        }
        toml::to_string(&root).unwrap_or_default()
    }

    /// #282: TOML 글자에서.
    pub fn from_toml(text: &str) -> Result<Presets, PresetsError> {
        let root: Table = text
            .parse()
            .map_err(|e: toml::de::Error| PresetsError::Invalid(e.to_string()))?;
        match root.get("version") {
            None => return Err(PresetsError::MissingVersion),
            Some(Value::Integer(n)) if *n >= 0 && *n as u64 == FORMAT_VERSION => {}
            Some(other) => return Err(PresetsError::UnknownVersion(version_text(other))),
        }
        let mut out = Presets::new();
        match root.get("last") {
            None => {}
            Some(Value::Table(t)) => {
                // 틀린 규칙이 든 `[last]`는 없는 것으로 본다(묶음이 아니라 편의 값이다).
                out.last = parse_rules(t).ok();
            }
            Some(_) => return Err(PresetsError::Invalid("`last`는 표여야 한다".to_owned())),
        }
        match root.get("preset") {
            None => {}
            Some(Value::Array(entries)) => {
                for entry in entries {
                    let Value::Table(t) = entry else {
                        return Err(PresetsError::Invalid(
                            "`preset`의 항목은 표여야 한다".to_owned(),
                        ));
                    };
                    out.read_preset(t);
                }
            }
            Some(_) => {
                return Err(PresetsError::Invalid(
                    "`preset`은 표 배열이어야 한다".to_owned(),
                ));
            }
        }
        Ok(out)
    }

    fn read_preset(&mut self, t: &Table) {
        let name = match t.get("name") {
            Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_owned()),
            _ => None,
        };
        let result = match &name {
            None => Err("묶음 이름이 없거나 글자가 아니다".to_owned()),
            Some(n) if self.find(n).is_some() => Err("앞 묶음과 같은 이름이다".to_owned()),
            Some(_) => parse_rules(t),
        };
        match (result, name) {
            (Ok(rules), Some(name)) => self.presets.push(Preset { name, rules }),
            (Err(error), name) => {
                self.skipped.push(SkippedPreset { name, error });
                self.skipped_raw.push(t.clone());
            }
            (Ok(_), None) => unreachable!("이름이 없으면 위에서 오류가 된다"),
        }
    }
}

fn version_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn parse_rules(t: &Table) -> Result<Vec<Rule>, String> {
    match t.get("rule") {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(i, v)| match v {
                Value::Table(r) => rule_from_table(r).map_err(|e| format!("규칙 {}: {e}", i + 1)),
                _ => Err(format!("규칙 {}: 표가 아니다", i + 1)),
            })
            .collect(),
        Some(_) => Err("`rule`은 표 배열이어야 한다".to_owned()),
    }
}

fn int(v: u64) -> Value {
    match i64::try_from(v) {
        Ok(n) => Value::Integer(n),
        Err(_) => Value::String(v.to_string()),
    }
}

fn edge_text(e: Edge) -> &'static str {
    match e {
        Edge::Start => "start",
        Edge::End => "end",
    }
}

fn place_to_value(p: &Place) -> Value {
    match p {
        Place::Start => Value::String("start".to_owned()),
        Place::End => Value::String("end".to_owned()),
        Place::At { from, at } => {
            let mut t = Table::new();
            t.insert(
                "from".to_owned(),
                Value::String(edge_text(*from).to_owned()),
            );
            t.insert("at".to_owned(), int(*at as u64));
            Value::Table(t)
        }
    }
}

fn rule_to_value(rule: &Rule) -> Value {
    let mut t = Table::new();
    let s = |x: &str| Value::String(x.to_owned());
    let kind = match &rule.kind {
        RuleKind::Replace {
            find,
            replace,
            regex,
            case_sensitive,
            all,
        } => {
            t.insert("find".to_owned(), s(find));
            t.insert("replace".to_owned(), s(replace));
            t.insert("regex".to_owned(), Value::Boolean(*regex));
            t.insert("case_sensitive".to_owned(), Value::Boolean(*case_sensitive));
            t.insert("all".to_owned(), Value::Boolean(*all));
            "replace"
        }
        RuleKind::Insert { text, from, at } => {
            t.insert("text".to_owned(), s(text));
            t.insert("from".to_owned(), s(edge_text(*from)));
            t.insert("at".to_owned(), int(*at as u64));
            "insert"
        }
        RuleKind::Delete { from, at, count } => {
            t.insert("from".to_owned(), s(edge_text(*from)));
            t.insert("at".to_owned(), int(*at as u64));
            t.insert("count".to_owned(), int(*count as u64));
            "delete"
        }
        RuleKind::Number {
            start,
            step,
            digits,
            place,
            separator,
        } => {
            t.insert("start".to_owned(), int(*start));
            t.insert("step".to_owned(), int(*step));
            t.insert("digits".to_owned(), int(*digits as u64));
            t.insert("place".to_owned(), place_to_value(place));
            t.insert("separator".to_owned(), s(separator));
            "number"
        }
        RuleKind::Case(mode) => {
            let m = match mode {
                CaseMode::Lower => "lower",
                CaseMode::Upper => "upper",
                CaseMode::Title => "title",
                CaseMode::Sentence => "sentence",
            };
            t.insert("mode".to_owned(), s(m));
            "case"
        }
        RuleKind::Extension { new } => {
            if let Some(new) = new {
                t.insert("new".to_owned(), s(new));
            }
            "extension"
        }
        RuleKind::Date {
            format,
            place,
            separator,
        } => {
            t.insert("format".to_owned(), s(format));
            t.insert("place".to_owned(), place_to_value(place));
            t.insert("separator".to_owned(), s(separator));
            "date"
        }
    };
    t.insert("kind".to_owned(), s(kind));
    t.insert("enabled".to_owned(), Value::Boolean(rule.enabled));
    let target = match rule.target {
        Target::Stem => "stem",
        Target::Extension => "extension",
        Target::Whole => "whole",
    };
    t.insert("target".to_owned(), s(target));
    Value::Table(t)
}

fn get<'a>(t: &'a Table, k: &str) -> Result<&'a Value, String> {
    t.get(k).ok_or_else(|| format!("`{k}` 값이 없다"))
}

fn string(t: &Table, k: &str) -> Result<String, String> {
    match get(t, k)? {
        Value::String(s) => Ok(s.clone()),
        _ => Err(format!("`{k}`는 글자여야 한다")),
    }
}

fn boolean(t: &Table, k: &str) -> Result<bool, String> {
    match get(t, k)? {
        Value::Boolean(b) => Ok(*b),
        _ => Err(format!("`{k}`는 참·거짓이어야 한다")),
    }
}

fn uint(t: &Table, k: &str) -> Result<u64, String> {
    match get(t, k)? {
        Value::Integer(n) if *n >= 0 => Ok(*n as u64),
        Value::String(s) => s
            .parse::<u64>()
            .map_err(|_| format!("`{k}`는 0 이상의 수여야 한다")),
        _ => Err(format!("`{k}`는 0 이상의 수여야 한다")),
    }
}

fn size(t: &Table, k: &str) -> Result<usize, String> {
    usize::try_from(uint(t, k)?).map_err(|_| format!("`{k}`가 너무 크다"))
}

fn edge_from(v: &Value, k: &str) -> Result<Edge, String> {
    match v {
        Value::String(s) if s == "start" => Ok(Edge::Start),
        Value::String(s) if s == "end" => Ok(Edge::End),
        _ => Err(format!("`{k}`는 start나 end여야 한다")),
    }
}

fn place(t: &Table, k: &str) -> Result<Place, String> {
    match get(t, k)? {
        Value::String(s) if s == "start" => Ok(Place::Start),
        Value::String(s) if s == "end" => Ok(Place::End),
        Value::Table(p) => Ok(Place::At {
            from: edge_from(get(p, "from")?, "from")?,
            at: size(p, "at")?,
        }),
        _ => Err(format!("`{k}`는 start·end·표여야 한다")),
    }
}

fn rule_from_table(t: &Table) -> Result<Rule, String> {
    let kind_name = string(t, "kind")?;
    let enabled = boolean(t, "enabled")?;
    let target = match string(t, "target")?.as_str() {
        "stem" => Target::Stem,
        "extension" => Target::Extension,
        "whole" => Target::Whole,
        other => return Err(format!("모르는 적용 대상 `{other}`")),
    };
    let kind = match kind_name.as_str() {
        "replace" => RuleKind::Replace {
            find: string(t, "find")?,
            replace: string(t, "replace")?,
            regex: boolean(t, "regex")?,
            case_sensitive: boolean(t, "case_sensitive")?,
            all: boolean(t, "all")?,
        },
        "insert" => RuleKind::Insert {
            text: string(t, "text")?,
            from: edge_from(get(t, "from")?, "from")?,
            at: size(t, "at")?,
        },
        "delete" => RuleKind::Delete {
            from: edge_from(get(t, "from")?, "from")?,
            at: size(t, "at")?,
            count: size(t, "count")?,
        },
        "number" => RuleKind::Number {
            start: uint(t, "start")?,
            step: uint(t, "step")?,
            digits: size(t, "digits")?,
            place: place(t, "place")?,
            separator: string(t, "separator")?,
        },
        "case" => RuleKind::Case(match string(t, "mode")?.as_str() {
            "lower" => CaseMode::Lower,
            "upper" => CaseMode::Upper,
            "title" => CaseMode::Title,
            "sentence" => CaseMode::Sentence,
            other => return Err(format!("모르는 대소문자 방식 `{other}`")),
        }),
        "extension" => RuleKind::Extension {
            new: match t.get("new") {
                None => None,
                Some(Value::String(s)) => Some(s.clone()),
                Some(_) => return Err("`new`는 글자여야 한다".to_owned()),
            },
        },
        "date" => RuleKind::Date {
            format: string(t, "format")?,
            place: place(t, "place")?,
            separator: string(t, "separator")?,
        },
        other => return Err(format!("모르는 규칙 종류 `{other}`")),
    };
    Ok(Rule {
        kind,
        enabled,
        target,
    })
}

/// #282: 묶음 조작의 거부 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetError {
    EmptyName,
    NameTaken,
    NotFound,
}

/// #282: 건너뛴 묶음(이름이 있으면 이름, 해석기 설명).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedPreset {
    pub name: Option<String>,
    pub error: String,
}

/// #282: 파일 전체를 읽지 못한 까닭.
#[derive(Debug)]
pub enum PresetsError {
    Io(std::io::Error),
    Invalid(String),
    MissingVersion,
    UnknownVersion(String),
}

/// #282: 파일을 읽은 결과.
#[derive(Debug)]
pub enum Loaded {
    Missing,
    Read(Presets),
    Broken {
        error: PresetsError,
        kept: Option<PathBuf>,
    },
}

/// #282: `%APPDATA%\facet\rename-presets.toml`.
pub fn presets_path() -> Option<PathBuf> {
    presets_path_with(|name| std::env::var_os(name))
}

/// #282: 환경 변수를 읽는 함수로 정한 파일 자리.
pub fn presets_path_with(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    var("APPDATA")
        .filter(|v| !v.is_empty())
        .map(|dir| PathBuf::from(dir).join("facet").join("rename-presets.toml"))
}

/// #282: 옆 임시 파일 자리(이름 뒤에 `.tmp`).
pub fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(OsString::from).unwrap_or_default();
    name.push(".tmp");
    path.with_file_name(name)
}

/// #282: 임시 파일에 쓰고 바꿔치기.
pub fn save(path: &Path, presets: &Presets) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let temp = temp_path(path);
    let result =
        std::fs::write(&temp, presets.to_toml()).and_then(|()| std::fs::rename(&temp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// #282: 파일 읽기.
pub fn load(path: &Path) -> Loaded {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Loaded::Missing,
        Err(e) => {
            return Loaded::Broken {
                error: PresetsError::Io(e),
                kept: None,
            };
        }
    };
    match Presets::from_toml(&text) {
        Ok(presets) => Loaded::Read(presets),
        Err(error) => Loaded::Broken {
            error,
            kept: keep_broken(path),
        },
    }
}

/// 깨진 파일을 옆에 겹치지 않는 이름으로 옮긴다. 옮긴 자리를 돌려준다.
fn keep_broken(path: &Path) -> Option<PathBuf> {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "rename-presets".to_owned());
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    for n in 1..100 {
        let name = if n == 1 {
            format!("{stem}.broken-{stamp}.toml")
        } else {
            format!("{stem}.broken-{stamp}-{n}.toml")
        };
        let target = path.with_file_name(name);
        if target.exists() {
            continue;
        }
        return std::fs::rename(path, &target).ok().map(|()| target);
    }
    None
}
