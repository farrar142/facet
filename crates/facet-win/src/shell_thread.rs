//! 셸 호출용 전용 STA 스레드와 시간 제한(S12, ADR-0002·ADR-0006 「예외」).

use std::time::Duration;

/// #349: 셸 호출을 받아 도는 전용 COM STA 스레드.
pub struct ShellThread {}

impl ShellThread {
    /// #349: 스레드는 아직 만들지 않는다.
    #[allow(clippy::new_without_default)]
    pub fn new() -> ShellThread {
        todo!("#349")
    }

    /// #349: `job`을 전용 스레드에서 `timeout` 안에 돌려 결과를 받는다.
    pub fn call<T: Send + 'static>(
        &self,
        _timeout: Duration,
        _job: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, CallError> {
        todo!("#349")
    }
}

/// #349: 셸 호출이 결과를 내지 못한 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallError {
    TimedOut,
    Failed,
}

/// #349: 부른 스레드가 COM STA로 초기화돼 있는가.
pub fn apartment_is_sta() -> bool {
    todo!("#349")
}
