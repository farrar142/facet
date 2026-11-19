//! #458: 설정의 단일 정의와 설정 파일(`settings.toml`) 읽기·검증·형식 보존 쓰기.
//!
//! 설정 항목마다 키·형·기본값·범위(또는 고를 수 있는 값)·분류를 [`definitions`] 한곳에 둔다. 표시 이름·설명은 화면
//! 문장이라 앱이 준다(ADR-0007). 파일은 `%APPDATA%\facet\settings.toml`이고 기본값과 다른 값만 담는다. 고쳐 쓸 때는
//! `toml_edit`으로 바뀐 키만 고쳐 주석·순서·facet이 모르는 키와 표를 그대로 두고, 같은 폴더의 임시 파일에 다 쓴 뒤
//! 바꿔치기한다.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::command::{CommandId, Shortcut};
use crate::keymap::Keymap;

/// #458: 설정 파일 이름.
pub const FILE_NAME: &str = "settings.toml";

/// #458: 설정 폴더 `%APPDATA%\facet`.
pub fn settings_folder() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .filter(|v| !v.is_empty())
        .map(|dir| PathBuf::from(dir).join("facet"))
}

/// #458: 설정 분류.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    General,
    View,
    Shortcuts,
    Search,
    Thumbnails,
    Network,
    Advanced,
}

/// #458: 설정 값.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
    Integer(i64),
    Choice(&'static str),
}

/// #458: 설정의 형과 범위.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bool,
    Integer { min: i64, max: i64 },
    Choice(&'static [&'static str]),
}

/// #458: 설정 항목 하나의 정의.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Definition {
    pub key: &'static str,
    pub category: Category,
    pub kind: Kind,
    pub default: Value,
}

/// #458: 모든 설정 정의.
pub fn definitions() -> &'static [Definition] {
    DEFINITIONS
}

/// 설정 정의 표. 키는 TOML의 `[표]`와 키를 점으로 이은 것이다(표 이름에는 점이 없다). 네트워크 분류의 항목은
/// S16이 더한다.
const DEFINITIONS: &[Definition] = &[
    Definition {
        key: "general.show_hidden",
        category: Category::General,
        kind: Kind::Bool,
        default: Value::Bool(false),
    },
    Definition {
        key: "view.theme",
        category: Category::View,
        kind: Kind::Choice(&["light", "dark", "system"]),
        default: Value::Choice("system"),
    },
    Definition {
        key: "view.font_size",
        category: Category::View,
        // 포인트.
        kind: Kind::Integer { min: 8, max: 20 },
        default: Value::Integer(9),
    },
    Definition {
        key: "view.row_height",
        category: Category::View,
        // 0은 글꼴 크기에 맞춤.
        kind: Kind::Integer { min: 0, max: 64 },
        default: Value::Integer(0),
    },
    Definition {
        key: "shortcuts.preset",
        category: Category::Shortcuts,
        kind: Kind::Choice(&["explorer", "total-commander"]),
        default: Value::Choice("explorer"),
    },
    Definition {
        key: "search.content_max_file_size_mb",
        category: Category::Search,
        // 10진 MB.
        kind: Kind::Integer { min: 1, max: 2000 },
        default: Value::Integer(50),
    },
    Definition {
        key: "thumbnails.cache_limit_mb",
        category: Category::Thumbnails,
        // MiB.
        kind: Kind::Integer {
            min: 64,
            max: 102_400,
        },
        default: Value::Integer(1024),
    },
    Definition {
        key: "advanced.shell_item_timeout_ms",
        category: Category::Advanced,
        kind: Kind::Integer {
            min: 500,
            max: 30_000,
        },
        default: Value::Integer(2000),
    },
    Definition {
        key: "advanced.shell_helper_idle_minutes",
        category: Category::Advanced,
        kind: Kind::Integer { min: 1, max: 120 },
        default: Value::Integer(5),
    },
];

/// #458: 키로 정의를 찾는다.
pub fn definition(key: &str) -> Option<&'static Definition> {
    DEFINITIONS.iter().find(|d| d.key == key)
}

fn index_of(key: &str) -> Option<usize> {
    DEFINITIONS.iter().position(|d| d.key == key)
}

/// #458: 키·앱이 준 (표시 이름, 설명)의 대소문자 무시 부분 일치, 정의 순서. 검색어는 앞뒤 공백을 걷고, 비면 모두다.
pub fn search<S: AsRef<str>>(
    query: &str,
    text: impl Fn(&Definition) -> (S, S),
) -> Vec<&'static Definition> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return DEFINITIONS.iter().collect();
    }
    DEFINITIONS
        .iter()
        .filter(|definition| {
            if definition.key.to_lowercase().contains(&query) {
                return true;
            }
            let (name, description) = text(definition);
            name.as_ref().to_lowercase().contains(&query)
                || description.as_ref().to_lowercase().contains(&query)
        })
        .collect()
}

impl Definition {
    /// 값이 이 정의의 형·범위·고를 수 있는 값에 맞는지 본다. 맞으면 정의 표의 글자를 쓴 값을 돌려준다.
    fn check(&self, value: Value) -> Result<Value, ValueError> {
        match (self.kind, value) {
            (Kind::Bool, Value::Bool(_)) => Ok(value),
            (Kind::Integer { min, max }, Value::Integer(n)) => {
                if (min..=max).contains(&n) {
                    Ok(value)
                } else {
                    Err(ValueError::OutOfRange { min, max })
                }
            }
            (Kind::Choice(choices), Value::Choice(c)) => choose(choices, c),
            _ => Err(ValueError::WrongType),
        }
    }

    /// 파일의 TOML 값을 설정 값으로 바꾸고 검증한다.
    fn check_toml(&self, value: &toml_edit::Value) -> Result<Value, ValueError> {
        match (self.kind, value) {
            (Kind::Bool, toml_edit::Value::Boolean(b)) => Ok(Value::Bool(*b.value())),
            (Kind::Integer { .. }, toml_edit::Value::Integer(n)) => {
                self.check(Value::Integer(*n.value()))
            }
            (Kind::Choice(choices), toml_edit::Value::String(s)) => choose(choices, s.value()),
            _ => Err(ValueError::WrongType),
        }
    }

    /// (표 이름, 키 이름).
    fn split(&self) -> (&'static str, &'static str) {
        self.key.split_once('.').expect("설정 키는 「표.키」다")
    }
}

fn choose(choices: &'static [&'static str], c: &str) -> Result<Value, ValueError> {
    choices
        .iter()
        .find(|choice| **choice == c)
        .map(|choice| Value::Choice(choice))
        .ok_or(ValueError::NotAChoice)
}

fn to_toml(value: Value) -> toml_edit::Value {
    match value {
        Value::Bool(b) => b.into(),
        Value::Integer(n) => n.into(),
        Value::Choice(c) => c.into(),
    }
}

/// 바이트 자리의 (줄, 열), 둘 다 1부터. 열은 글자 수로 센다.
fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &text[..offset];
    let line = before.matches('\n').count() + 1;
    let start = before.rfind('\n').map_or(0, |i| i + 1);
    (line, before[start..].chars().count() + 1)
}

/// #458: 받지 않는 값의 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueError {
    UnknownKey,
    WrongType,
    OutOfRange { min: i64, max: i64 },
    NotAChoice,
    InvalidShortcut,
}

/// #458: 설정 파일의 문법 오류(줄·열은 1부터).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

/// #458: 설정 파일의 틀린 값.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub key: String,
    pub line: usize,
    pub column: usize,
    pub error: ValueError,
}

/// #458: 파일에 쓰지 못한 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveError {
    FileBroken,
    Write(std::io::ErrorKind),
}

/// #458: 바꾼 값을 파일에 썼는가.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Saved {
    Written,
    KeptInMemory(SaveError),
}

/// #464: 쓸 모양.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}

/// #464: 글꼴 크기(포인트)에 맞춘 행 높이(논리 픽셀).
pub fn auto_row_height(_font_size: u32) -> u32 {
    todo!("#464")
}

/// 같은 폴더의 임시 파일에 다 쓰고 바꿔치기한다. 폴더가 없으면 만든다.
fn write_replacing(folder: &Path, path: &Path, text: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(folder)?;
    let temp = folder.join(format!("{FILE_NAME}.tmp"));
    let result = std::fs::write(&temp, text).and_then(|()| std::fs::rename(&temp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// #458: 적용 중인 설정과 설정 파일.
#[derive(Debug, Clone)]
pub struct Settings {
    /// 적용 중인 값, [`definitions`]와 같은 차례.
    values: Vec<Value>,
    /// 설정 폴더. [`Settings::defaults`]면 `None`이라 쓰지 않는다.
    folder: Option<PathBuf>,
    /// 읽은 파일의 형식 보존 문서(파일이 없으면 빈 문서).
    document: toml_edit::DocumentMut,
    /// 파일에 쓰지 않는 까닭(문법 오류·읽지 못함). 있으면 바꾼 값은 이번 실행에만 적용한다.
    blocked: Option<SaveError>,
    syntax_error: Option<SyntaxError>,
    problems: Vec<Problem>,
}

impl Settings {
    /// #458: 파일 없는 기본값. 값을 바꿔도 파일에 쓰지 않는다.
    pub fn defaults() -> Settings {
        Settings {
            values: DEFINITIONS.iter().map(|d| d.default).collect(),
            folder: None,
            document: toml_edit::DocumentMut::new(),
            blocked: None,
            syntax_error: None,
            problems: Vec::new(),
        }
    }

    /// #458: 설정 폴더의 설정 파일을 읽는다. 실패하지 않는다: 파일이 없으면 기본값(아무것도 만들지 않음), 문법 오류면
    /// 파일 전체를 무시한 기본값과 [`Settings::syntax_error`], 틀린 값은 그 키만 기본값과 [`Settings::problems`]다.
    pub fn load(folder: &Path) -> Settings {
        let mut settings = Settings::defaults();
        settings.folder = Some(folder.to_path_buf());
        let text = match std::fs::read_to_string(folder.join(FILE_NAME)) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return settings,
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                settings.blocked = Some(SaveError::FileBroken);
                settings.syntax_error = Some(SyntaxError {
                    line: 1,
                    column: 1,
                    message: "file is not valid UTF-8".to_owned(),
                });
                return settings;
            }
            Err(e) => {
                // 읽지 못한 파일을 덮어쓰지 않는다.
                settings.blocked = Some(SaveError::Write(e.kind()));
                return settings;
            }
        };
        let parsed = match toml_edit::Document::parse(text.as_str()) {
            Ok(parsed) => parsed,
            Err(error) => {
                let (line, column) = line_column(&text, error.span().map_or(0, |s| s.start));
                settings.blocked = Some(SaveError::FileBroken);
                settings.syntax_error = Some(SyntaxError {
                    line,
                    column,
                    message: error.message().trim().to_owned(),
                });
                return settings;
            }
        };
        for (i, definition) in DEFINITIONS.iter().enumerate() {
            let (table, name) = definition.split();
            let Some(table) = parsed.get(table).and_then(|t| t.as_table_like()) else {
                continue;
            };
            let Some((key, item)) = table.get_key_value(name) else {
                continue;
            };
            let result = match item.as_value() {
                Some(value) => definition.check_toml(value),
                None => Err(ValueError::WrongType),
            };
            match result {
                Ok(value) => settings.values[i] = value,
                Err(error) => {
                    let start = item
                        .span()
                        .or_else(|| key.span())
                        .map_or(0, |span| span.start);
                    let (line, column) = line_column(&text, start);
                    settings.problems.push(Problem {
                        key: definition.key.to_owned(),
                        line,
                        column,
                        error,
                    });
                }
            }
        }
        settings.document = parsed.into_mut();
        settings
    }

    /// #458: 적용 중인 값.
    pub fn get(&self, key: &str) -> Option<Value> {
        index_of(key).map(|i| self.values[i])
    }

    /// #458: 기본값과 다른가.
    pub fn is_modified(&self, key: &str) -> bool {
        index_of(key).is_some_and(|i| self.values[i] != DEFINITIONS[i].default)
    }

    /// #458: 값을 바꾼다. 정의에 맞지 않으면 아무것도 바꾸지 않고 까닭(범위)을 돌려준다. 받으면 적용하고 파일에는
    /// 기본값과 다를 때만 그 키를 쓰고, 기본값이면 파일에서 뺀다. 쓰지 못하면 값은 메모리에만 남는다.
    pub fn set(&mut self, key: &str, value: Value) -> Result<Saved, ValueError> {
        let i = index_of(key).ok_or(ValueError::UnknownKey)?;
        let value = DEFINITIONS[i].check(value)?;
        self.values[i] = value;
        Ok(self.save(&DEFINITIONS[i], value))
    }

    /// #458: 기본값으로 돌린다(파일에서 그 키를 뺀다).
    pub fn reset(&mut self, key: &str) -> Result<Saved, ValueError> {
        let i = index_of(key).ok_or(ValueError::UnknownKey)?;
        let definition = &DEFINITIONS[i];
        self.values[i] = definition.default;
        Ok(self.save(definition, definition.default))
    }

    /// 바뀐 키 하나만 문서에서 고쳐 파일에 쓴다. 쓰지 못하면 문서는 그대로다.
    fn save(&mut self, definition: &Definition, value: Value) -> Saved {
        if let Some(blocked) = &self.blocked {
            return Saved::KeptInMemory(blocked.clone());
        }
        let Some(folder) = self.folder.clone() else {
            return Saved::KeptInMemory(SaveError::Write(std::io::ErrorKind::Unsupported));
        };
        let mut document = self.document.clone();
        let (table_name, name) = definition.split();
        if value == definition.default {
            if let Some(table) = document
                .get_mut(table_name)
                .and_then(|t| t.as_table_like_mut())
            {
                table.remove(name);
            }
        } else {
            let root = document.as_table_mut();
            if !root
                .get(table_name)
                .is_some_and(|item| item.is_table_like())
            {
                root.insert(table_name, toml_edit::Item::Table(toml_edit::Table::new()));
            }
            let table = root
                .get_mut(table_name)
                .and_then(|t| t.as_table_like_mut())
                .expect("방금 표를 넣었다");
            let mut new = to_toml(value);
            match table.get_mut(name).and_then(|item| item.as_value_mut()) {
                Some(old) => {
                    // 줄 끝 주석과 둘레 공백을 그대로 둔다.
                    *new.decor_mut() = old.decor().clone();
                    *old = new;
                }
                None => {
                    table.insert(name, toml_edit::Item::Value(new));
                }
            }
        }
        let path = folder.join(FILE_NAME);
        let text = document.to_string();
        if text.is_empty() && !path.exists() {
            // 쓸 것이 없으면 파일을 만들지 않는다.
            self.document = document;
            return Saved::Written;
        }
        match write_replacing(&folder, &path, &text) {
            Ok(()) => {
                self.document = document;
                Saved::Written
            }
            Err(e) => Saved::KeptInMemory(SaveError::Write(e.kind())),
        }
    }

    /// #458: 파일의 문법 오류.
    pub fn syntax_error(&self) -> Option<&SyntaxError> {
        self.syntax_error.as_ref()
    }

    /// #458: 파일의 틀린 값.
    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }

    /// #458: 읽은 설정 파일 경로(`load`한 폴더의 [`FILE_NAME`], `defaults`면 `None`).
    pub fn file_path(&self) -> Option<PathBuf> {
        self.folder.as_ref().map(|folder| folder.join(FILE_NAME))
    }

    /// #460: 단축키 모델.
    pub fn keymap(&self) -> &Keymap {
        todo!("#460")
    }

    /// #460: 명령의 단축키를 바꾼다.
    pub fn set_shortcuts(&mut self, _command: CommandId, _shortcuts: Vec<Shortcut>) -> Saved {
        todo!("#460")
    }

    /// #460: 명령의 단축키를 프리셋 기본으로.
    pub fn reset_shortcuts(&mut self, _command: CommandId) -> Saved {
        todo!("#460")
    }

    /// #460: 모든 단축키를 프리셋 기본으로.
    pub fn reset_all_shortcuts(&mut self) -> Saved {
        todo!("#460")
    }

    /// #464: 쓸 모양.
    pub fn appearance(&self, _system_dark: bool) -> Appearance {
        todo!("#464")
    }

    /// #464: 글꼴 크기(포인트).
    pub fn font_size(&self) -> u32 {
        todo!("#464")
    }

    /// #464: 행 높이(논리 픽셀).
    pub fn row_height(&self) -> u32 {
        todo!("#464")
    }

    /// #465: 내용 검색 큰 파일 한도(바이트).
    pub fn content_max_file_size(&self) -> u64 {
        todo!("#465")
    }

    /// #465: 썸네일 디스크 캐시 상한(바이트).
    pub fn thumbnail_disk_limit(&self) -> u64 {
        todo!("#465")
    }

    /// #465: 셸 확장 도우미 응답 대기 시간.
    pub fn shell_reply_timeout(&self) -> Duration {
        todo!("#465")
    }

    /// #465: 셸 확장 도우미 유휴 종료 시간.
    pub fn shell_idle_timeout(&self) -> Duration {
        todo!("#465")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_column_counts_from_one_in_characters() {
        let text = "a = 1\n가나 = x\n";
        assert_eq!(line_column(text, 0), (1, 1));
        assert_eq!(line_column(text, 6), (2, 1));
        // 「가나 」 뒤의 「=」는 둘째 줄 넷째 글자다(바이트가 아니라 글자로 셈).
        assert_eq!(line_column(text, text.rfind('=').unwrap()), (2, 4));
    }

    #[test]
    fn defaults_keep_changes_in_memory_only() {
        let mut settings = Settings::defaults();
        assert_eq!(
            settings.set("view.font_size", Value::Integer(10)),
            Ok(Saved::KeptInMemory(SaveError::Write(
                std::io::ErrorKind::Unsupported
            )))
        );
        assert!(settings.is_modified("view.font_size"));
    }
}
