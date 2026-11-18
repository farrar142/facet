//! 드라이브 용량과 장치 붙임·뗌 알림. 내 PC(셸 공급자)가 쓴다.
//!
//! - [`disk_space`]: 볼륨 루트의 전체 크기와 이 사용자가 쓸 수 있는 남은 공간(`GetDiskFreeSpaceExW`, 탐색기·.NET
//!   `DriveInfo`와 같은 값). 끊긴 네트워크 드라이브는 오래 막을 수 있으므로 UI 스레드에서 부르지 않는다.
//! - [`watch_devices`]: 볼륨·휴대용 장치(WPD)의 장치 인터페이스가 붙거나 떨어지면 콜백을 부른다
//!   (`CM_Register_Notification`, 창 없이 시스템 스레드 풀에서 알림). 돌려받은 [`DeviceWatch`]를 버리면 풀린다.
//!
//! Windows가 아니면 용량은 `None`, 알림은 「지원하지 않음」이다.

use std::io;
use std::path::Path;

/// 볼륨의 용량(바이트).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskSpace {
    /// 전체 크기(이 사용자에게 할당량이 걸려 있으면 그 크기).
    pub total: u64,
    /// 이 사용자가 쓸 수 있는 남은 공간.
    pub free: u64,
}

/// `root`(드라이브 루트 등) 볼륨의 용량. 매체가 없거나(빈 카드 리더) 닿지 못하면 `None`이다. 끊긴 네트워크 드라이브는
/// 시간 제한까지 막을 수 있다.
pub fn disk_space(root: &Path) -> Option<DiskSpace> {
    #[cfg(windows)]
    {
        crate::windows_devices::disk_space(root)
    }
    #[cfg(not(windows))]
    {
        let _ = root;
        None
    }
}

/// 걸린 장치 알림. 버리면 풀리고, 버린 뒤에는 콜백이 불리지 않는다.
pub struct DeviceWatch {
    #[cfg(windows)]
    _inner: crate::windows_devices::Registration,
}

impl std::fmt::Debug for DeviceWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceWatch").finish_non_exhaustive()
    }
}

/// 볼륨이나 휴대용 장치(휴대폰)가 붙거나 떨어질 때마다 `on_change`를 부른다(시스템 스레드 풀에서, 여러 번 몰려 올 수
/// 있다). 콜백은 짧게 끝내야 한다.
pub fn watch_devices(on_change: impl Fn() + Send + Sync + 'static) -> io::Result<DeviceWatch> {
    #[cfg(windows)]
    {
        Ok(DeviceWatch {
            _inner: crate::windows_devices::register(Box::new(on_change))?,
        })
    }
    #[cfg(not(windows))]
    {
        let _ = on_change;
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}
