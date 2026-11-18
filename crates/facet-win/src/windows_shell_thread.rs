//! 셸 호출용 전용 STA 스레드의 Windows 부분(`crate::shell_thread`). `unsafe`는 이 파일 안에 있다.

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Com::{
    APTTYPE, APTTYPE_MAINSTA, APTTYPE_STA, APTTYPEQUALIFIER, CoGetApartmentType,
};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE,
    PeekMessageW, QS_ALLINPUT, TranslateMessage, WM_QUIT,
};

use crate::windows_impl::ComApartment;

/// 자동으로 풀리는 이벤트. 버리면 닫는다.
pub(crate) struct Event(HANDLE);

// SAFETY: 이벤트 핸들은 프로세스 전체에서 유효한 커널 개체 핸들이고, SetEvent·대기는 어느 스레드에서 불러도 된다.
// 닫기는 Drop에서 한 번만 한다.
unsafe impl Send for Event {}
// SAFETY: 위와 같다. 공유하며 하는 일(SetEvent·대기)은 스레드에 안전한 커널 호출이다.
unsafe impl Sync for Event {}

impl Event {
    pub(crate) fn new() -> Option<Event> {
        // SAFETY: 보안 속성·이름 없이 자동 리셋·처음 꺼진 이벤트를 만든다. 돌려받은 핸들은 Event가 맡아 한 번 닫는다.
        let handle = unsafe { CreateEventW(None, false, false, None) }.ok()?;
        Some(Event(handle))
    }

    pub(crate) fn set(&self) {
        // SAFETY: self.0은 이 Event가 살아 있는 동안 유효한 이벤트 핸들이다.
        let _ = unsafe { SetEvent(self.0) };
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: new가 만든 핸들이고 여기서 한 번만 닫는다.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// 이 스레드를 COM STA로 초기화하고, 일 표지(`event`)와 창 메시지를 함께 기다리는 메시지 루프를 돈다.
///
/// 표지가 켜지거나 메시지가 오면 메시지를 모두 처리하고 `drain`을 부른다. `drain`이 `false`(일 통로가 닫힘)를
/// 돌려주거나 `WM_QUIT`가 오면 끝낸다. COM을 초기화하지 못해도 일은 돌린다([`apartment_is_sta`]가 거짓을 알림).
pub(crate) fn run(event: &Event, mut drain: impl FnMut() -> bool) {
    let _apartment = ComApartment::enter().ok();
    loop {
        if !drain() {
            return;
        }
        let handles = [event.0];
        // SAFETY: handles는 이 호출 동안 살아 있는 유효한 이벤트 핸들 하나다. 실패(WAIT_FAILED)해도 아래에서 메시지와
        // 일 통로를 다시 보므로 루프는 이어진다.
        let _ = unsafe {
            MsgWaitForMultipleObjectsEx(Some(&handles), INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
        };
        let mut message = MSG::default();
        // SAFETY: message는 이 스레드의 지역 변수이고, 창 핸들 None은 이 스레드의 모든 메시지를 뜻한다.
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            if message.message == WM_QUIT {
                return;
            }
            // SAFETY: PeekMessageW가 채운 메시지를 같은 스레드에서 넘긴다.
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}

/// 부른 스레드가 COM STA(주 STA 포함)로 초기화돼 있는가.
pub(crate) fn apartment_is_sta() -> bool {
    let mut kind = APTTYPE::default();
    let mut qualifier = APTTYPEQUALIFIER::default();
    // SAFETY: 두 출력 인자는 이 함수의 지역 변수를 가리킨다. COM이 초기화되지 않았으면 오류를 돌려줄 뿐이다.
    let result = unsafe { CoGetApartmentType(&mut kind, &mut qualifier) };
    result.is_ok() && (kind == APTTYPE_STA || kind == APTTYPE_MAINSTA)
}
