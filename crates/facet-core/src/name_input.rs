//! 이름 입력란: 고른 항목의 이름을 그 행 자리에서 고쳐 쓰는 입력란(F2).
//!
//! 지금은 한글 IME를 확인하는 데만 쓴다. 확정해도 입력값을 돌려줄 뿐 파일 이름은 바꾸지 않는다.
//! 실제 이름바꾸기는 작업으로 S4에서 만든다. 화면에 낼 문장은 만들지 않는다(ADR-0007).

use crate::Item;

/// 열려 있는 이름 입력란.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameInput {
    index: usize,
    original: String,
}

/// 확정한 이름 입력란의 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnteredName {
    /// 입력란을 열 때의 이름.
    pub original: String,
    /// 확정한 입력값.
    pub entered: String,
}

impl NameInput {
    /// `items` 가운데 `selected` 항목의 이름 입력란을 연다. 현재 이름이 채워진다.
    ///
    /// 선택이 없거나 목록 밖이면 열지 않는다(`None`).
    pub fn open(items: &[Item], selected: Option<usize>) -> Option<Self> {
        let index = selected?;
        let item = items.get(index)?;
        Some(Self {
            index,
            original: item.name.to_string_lossy().into_owned(),
        })
    }

    /// 입력란이 놓인 행.
    pub fn index(&self) -> usize {
        self.index
    }

    /// 입력란에 처음 채운 현재 이름.
    pub fn original(&self) -> &str {
        &self.original
    }

    /// 입력을 확정한다. 입력값을 돌려줄 뿐 파일 시스템은 건드리지 않는다.
    pub fn confirm(self, entered: &str) -> EnteredName {
        EnteredName {
            original: self.original,
            entered: entered.to_owned(),
        }
    }

    /// 입력을 취소한다. 아무것도 남기지 않는다.
    ///
    /// 사용자에게 알릴 문장은 앱이 만든다(ADR-0007).
    pub fn cancel(self) {}
}

/// 이름이 Windows에서 쓸 수 없는 까닭. 화면 문장은 앱이 만든다(ADR-0007).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    /// 이름이 비어 있다.
    Empty,
    /// 금지 글자(`\` `/` `:` `*` `?` `"` `<` `>` `|`)가 들어 있다. 어떤 글자인지 함께.
    ForbiddenChar(char),
    /// 제어 문자(U+0000..=U+001F)가 들어 있다. 어떤 글자인지 함께.
    ControlChar(char),
    /// 예약 이름(`CON` `PRN` `AUX` `NUL` `COM1`~`COM9` `LPT1`~`LPT9`)이다. 확장자가 붙어도 그렇다.
    Reserved,
    /// 이름이 점으로 끝난다.
    TrailingDot,
    /// 이름이 공백으로 끝난다.
    TrailingSpace,
    /// UTF-16 단위가 `MAX_NAME_UNITS`를 넘는다. 센 단위 수를 함께.
    TooLong { units: usize },
}

/// 이름 하나의 최대 길이(UTF-16 단위).
pub const MAX_NAME_UNITS: usize = 255;

const FORBIDDEN: [char; 9] = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];

/// 이름 하나가 Windows에서 쓸 수 있는지 확인한다. 막는 까닭이 없으면 `Ok`.
///
/// 입력할 때마다 부르므로 이름을 한 번 훑는 것 말고는 하지 않는다.
pub fn validate_name(name: &str) -> Result<(), NameError> {
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    for c in name.chars() {
        if FORBIDDEN.contains(&c) {
            return Err(NameError::ForbiddenChar(c));
        }
        if (c as u32) < 0x20 {
            return Err(NameError::ControlChar(c));
        }
    }
    if is_reserved(name) {
        return Err(NameError::Reserved);
    }
    if name.ends_with('.') {
        return Err(NameError::TrailingDot);
    }
    if name.ends_with(' ') {
        return Err(NameError::TrailingSpace);
    }
    let units = name.encode_utf16().count();
    if units > MAX_NAME_UNITS {
        return Err(NameError::TooLong { units });
    }
    Ok(())
}

/// 첫 점 앞(끝 공백 뺀) 부분이 예약 이름인지. Windows는 `CON.txt`·`CON .txt`도 예약으로 본다.
fn is_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    let upper = stem.to_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    let mut chars = upper.chars();
    let (a, b, c, d) = (chars.next(), chars.next(), chars.next(), chars.next());
    if chars.next().is_some() {
        return false;
    }
    let device = matches!(
        (a, b, c),
        (Some('C'), Some('O'), Some('M')) | (Some('L'), Some('P'), Some('T'))
    );
    device && matches!(d, Some('1'..='9' | '¹' | '²' | '³'))
}

/// 이름의 마지막 점 위치(바이트). 확장자가 없으면 `None`.
/// 점으로 시작하는 이름(`.gitignore`)이나 점으로 끝나는 이름은 확장자가 없다.
fn extension_dot(name: &str) -> Option<usize> {
    let dot = name.rfind('.')?;
    (dot > 0 && dot + 1 < name.len()).then_some(dot)
}

fn extension(name: &str) -> &str {
    extension_dot(name).map_or("", |dot| &name[dot + 1..])
}

/// 이름을 바꾸면 확장자가 달라지는지. 폴더는 확장자를 보지 않아 늘 `false`.
///
/// 대문자·소문자만 다른 확장자(`a.TXT` ← `a.txt`)는 바뀐 것으로 보지 않는다:
/// Windows는 확장자를 대소문자 없이 같게 보아 같은 프로그램이 열기 때문이다.
pub fn extension_changed(original: &str, new: &str, is_folder: bool) -> bool {
    if is_folder {
        return false;
    }
    let (old, new) = (extension(original), extension(new));
    old != new && old.to_lowercase() != new.to_lowercase()
}

/// 이름 입력란을 열 때 처음 선택할 범위 `(시작, 끝)`.
///
/// 단위는 **UTF-8 바이트 오프셋**이다. Slint `TextInput.set-selection-offsets`가
/// 그 단위를 쓰므로 앱이 그대로 넘기면 된다(한글·이모지 이름도 맞다).
/// 파일은 확장자를 뺀 부분, 폴더와 확장자 없는 파일(`.gitignore` 포함)은 전체다.
pub fn initial_selection(name: &str, is_folder: bool) -> (usize, usize) {
    let end = if is_folder {
        name.len()
    } else {
        extension_dot(name).unwrap_or(name.len())
    };
    (0, end)
}
