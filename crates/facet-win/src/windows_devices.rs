//! 드라이브 용량과 장치 알림의 Windows 구현. `unsafe`는 이 파일 안에 가둔다.

use std::ffi::c_void;
use std::io;
use std::path::Path;

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_NOTIFY_ACTION, CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL,
    CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL, CM_NOTIFY_EVENT_DATA, CM_NOTIFY_FILTER,
    CM_NOTIFY_FILTER_0, CM_NOTIFY_FILTER_0_0, CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE,
    CM_Register_Notification, CM_Unregister_Notification, CR_SUCCESS, HCMNOTIFICATION,
};
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::core::{GUID, HSTRING};

use crate::devices::DiskSpace;

/// 볼륨 장치 인터페이스(`GUID_DEVINTERFACE_VOLUME`).
const VOLUME_INTERFACE: GUID = GUID::from_u128(0x53f5630d_b6bf_11d0_94f2_00a0c91efb8b);
/// 휴대용 장치(MTP 휴대폰 등) 인터페이스(`GUID_DEVINTERFACE_WPD`).
const WPD_INTERFACE: GUID = GUID::from_u128(0x6ac27878_a6fa_4155_ba85_f98f491d4f33);

pub fn disk_space(root: &Path) -> Option<DiskSpace> {
    let name = HSTRING::from(root.as_os_str());
    let mut free = 0u64;
    let mut total = 0u64;
    // SAFETY: `name`은 이 호출 동안 살아 있는 널 끝 UTF-16 글자이고, 두 출력 포인터는 지역 변수를 가리킨다.
    unsafe { GetDiskFreeSpaceExW(&name, Some(&mut free), Some(&mut total), None) }.ok()?;
    Some(DiskSpace { total, free })
}

type Callback = Box<dyn Fn() + Send + Sync>;

/// 걸린 알림들과 콜백. 버리면 알림을 모두 푼 뒤 콜백을 버린다.
pub struct Registration {
    handles: Vec<HCMNOTIFICATION>,
    context: *mut Callback,
}

// SAFETY: 알림 핸들은 어느 스레드에서 풀어도 되고, 콜백은 `Send + Sync`다.
unsafe impl Send for Registration {}

impl Drop for Registration {
    fn drop(&mut self) {
        for handle in self.handles.drain(..) {
            // SAFETY: `register`가 받은 핸들이고 한 번만 푼다. 풀기는 진행 중인 콜백이 끝나기를 기다린다.
            unsafe {
                let _ = CM_Unregister_Notification(handle);
            }
        }
        // SAFETY: 알림을 모두 풀었으므로 더는 콜백이 `context`를 읽지 않는다. `register`가 `Box::into_raw`로 만든 것이다.
        drop(unsafe { Box::from_raw(self.context) });
    }
}

pub fn register(callback: Callback) -> io::Result<Registration> {
    let context = Box::into_raw(Box::new(callback));
    let mut registration = Registration {
        handles: Vec::new(),
        context,
    };
    for class in [VOLUME_INTERFACE, WPD_INTERFACE] {
        let filter = CM_NOTIFY_FILTER {
            cbSize: std::mem::size_of::<CM_NOTIFY_FILTER>() as u32,
            Flags: 0,
            FilterType: CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE,
            Reserved: 0,
            u: CM_NOTIFY_FILTER_0 {
                DeviceInterface: CM_NOTIFY_FILTER_0_0 { ClassGuid: class },
            },
        };
        let mut handle = HCMNOTIFICATION::default();
        // SAFETY: `filter`는 cbSize를 채운 지역 값이고, `context`는 `Registration`이 버려질 때(알림을 푼 뒤)까지 산다.
        let result = unsafe {
            CM_Register_Notification(
                &filter,
                Some(context as *const c_void),
                Some(on_notify),
                &mut handle,
            )
        };
        if result != CR_SUCCESS {
            // 걸었던 것은 `registration`을 버리며 푼다.
            return Err(io::Error::other(format!(
                "CM_Register_Notification: {}",
                result.0
            )));
        }
        registration.handles.push(handle);
    }
    Ok(registration)
}

unsafe extern "system" fn on_notify(
    _notify: HCMNOTIFICATION,
    context: *const c_void,
    action: CM_NOTIFY_ACTION,
    _data: *const CM_NOTIFY_EVENT_DATA,
    _size: u32,
) -> u32 {
    if action == CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL
        || action == CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL
    {
        // SAFETY: `register`가 넘긴 콜백 상자이고, 알림이 걸린 동안(이 콜백이 불릴 수 있는 동안) 산다.
        let callback = unsafe { &*(context as *const Callback) };
        // 패닉이 시스템 스레드 풀로 넘어가지 않게 한다.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback));
    }
    0
}
