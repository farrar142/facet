//! #469: 레지스트리 안전 래퍼(HKCU·HKU 아래 값 읽기·쓰기·지우기, 키 있음, 키 나무 지우기). Windows가 아니면 `Unsupported`.

/// #469: 하이브.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hive {
    CurrentUser,
    Users,
}

/// #469: `REG_SZ`.
pub const REG_SZ: u32 = 1;
/// #469: `REG_EXPAND_SZ`.
pub const REG_EXPAND_SZ: u32 = 2;
/// #469: `REG_BINARY`.
pub const REG_BINARY: u32 = 3;
/// #469: `REG_DWORD`.
pub const REG_DWORD: u32 = 4;

/// #469: 형식 번호와 바이트 그대로의 값.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegValue {
    pub kind: u32,
    pub data: Vec<u8>,
}

impl RegValue {
    /// #469: `REG_SZ`(UTF-16LE + 끝 NUL).
    pub fn string(_text: &str) -> RegValue {
        todo!("#469")
    }
}

/// #469: 레지스트리 오류.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    AccessDenied,
    Unsupported,
    Other(u32),
}

/// #469: 값 읽기(없으면 `None`).
pub fn read_value(_hive: Hive, _key: &str, _name: &str) -> Result<Option<RegValue>, RegistryError> {
    todo!("#469")
}

/// #469: 값 쓰기(키가 없으면 만듦).
pub fn write_value(
    _hive: Hive,
    _key: &str,
    _name: &str,
    _value: &RegValue,
) -> Result<(), RegistryError> {
    todo!("#469")
}

/// #469: 값 지우기(없으면 성공).
pub fn delete_value(_hive: Hive, _key: &str, _name: &str) -> Result<(), RegistryError> {
    todo!("#469")
}

/// #469: 키가 있는가.
pub fn key_exists(_hive: Hive, _key: &str) -> Result<bool, RegistryError> {
    todo!("#469")
}

/// #469: 키 나무 지우기(없으면 성공).
pub fn delete_tree(_hive: Hive, _key: &str) -> Result<(), RegistryError> {
    todo!("#469")
}
