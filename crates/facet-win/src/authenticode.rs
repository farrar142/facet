//! #474: Authenticode 서명 확인과 서명자 이름.

use std::path::Path;

/// #474: 서명 확인 실패.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureError {
    NotSigned,
    Invalid(u32),
    Unsupported,
}

/// #474: 유효한 서명의 서명자 CN.
pub fn signer(_path: &Path) -> Result<String, SignatureError> {
    todo!("#474")
}
