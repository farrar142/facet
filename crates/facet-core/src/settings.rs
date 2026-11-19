//! #458: 설정의 단일 정의와 설정 파일(`settings.toml`) 읽기·검증·형식 보존 쓰기.

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
    todo!("#458")
}

/// #458: 키로 정의를 찾는다.
pub fn definition(_key: &str) -> Option<&'static Definition> {
    todo!("#458")
}
