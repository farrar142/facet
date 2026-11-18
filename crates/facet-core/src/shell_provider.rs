//! 셸 공급자: 가상 폴더(내 PC·휴지통·휴대폰)를 셸 네임스페이스로 읽는다(ADR-0004, S12).

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// #355: 셸 호출의 기본 시간 제한.
pub const CALL_TIMEOUT: Duration = Duration::ZERO; // #355

/// #351: 내 PC의 드라이브 용량(바이트, 모르면 `None`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriveSpace {
    pub total: Option<u64>,
    pub free: Option<u64>,
}

/// #352: 휴지통 항목의 원래 전체 경로와 지운 시각.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecycledEntry {
    pub original: PathBuf,
    pub deleted: SystemTime,
}

/// #355: 프로세스 전체의 셸 호출 시간 제한을 바꾼다(시험용).
#[doc(hidden)]
pub fn set_call_timeout(_timeout: Duration) {
    todo!("#355")
}

/// #355: 시험용 셸 호출 오류 주입.
#[doc(hidden)]
pub mod fault {
    use std::time::Duration;

    use crate::Location;

    /// #355: 살아 있는 동안 주입이 걸려 있다.
    pub struct Injected {}

    impl Drop for Injected {
        fn drop(&mut self) {} // #355
    }

    /// #355: `location`의 열거가 `stall`만큼 응답하지 않게 한다.
    pub fn stall_listing(_location: &Location, _stall: Duration) -> Injected {
        todo!("#355")
    }
}
