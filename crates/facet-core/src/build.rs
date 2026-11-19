//! #468: 빌드 정보(버전·채널·설치 종류).

use std::path::Path;

/// #468: 패키지 버전.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// #468: 설치 표지 파일 이름.
pub const INSTALL_MARKER: &str = "facet-installed";

/// #468: 채널.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Nightly,
}

impl Channel {
    /// #468: `"stable"`·`"nightly"`.
    pub fn name(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Nightly => "nightly",
        }
    }
}

/// #468: 설치 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    Installed,
    Portable,
}

impl InstallKind {
    /// #468: `"installed"`·`"portable"`.
    pub fn name(self) -> &'static str {
        match self {
            InstallKind::Installed => "installed",
            InstallKind::Portable => "portable",
        }
    }
}

/// #468: 빌드 때 `FACET_CHANNEL`로 정한 채널.
pub fn channel() -> Channel {
    match option_env!("FACET_CHANNEL") {
        Some("stable") => Channel::Stable,
        _ => Channel::Nightly,
    }
}

/// #468: 실행 파일 폴더의 설치 표지로 정한 설치 종류.
pub fn install_kind(exe_dir: &Path) -> InstallKind {
    if exe_dir.join(INSTALL_MARKER).is_file() {
        InstallKind::Installed
    } else {
        InstallKind::Portable
    }
}

/// #468: `facet <버전> (<채널>, <설치 종류>)`.
pub fn version_line(channel: Channel, kind: InstallKind) -> String {
    format!("facet {VERSION} ({}, {})", channel.name(), kind.name())
}
