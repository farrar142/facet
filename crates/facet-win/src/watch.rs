//! 폴더의 변경 감시(`ReadDirectoryChangesW`).
//!
//! [`DirectoryWatcher::watch`]가 폴더 하나(하위 폴더 안은 빼고)의 바뀐 항목 이름을 감시 스레드에서
//! 콜백으로 알린다. 값을 버리면 감시를 풀고 감시 스레드가 끝날 때까지 기다린다. 그 뒤로는 콜백을 부르지 않는다.

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::Path;

/// 감시가 알리는 변경.
#[derive(Debug)]
pub enum DirectoryChange {
    /// 이 이름의 항목이 생겼거나, 지워졌거나, 바뀌었거나, 이름 바꾸기의 옛 이름 또는 새 이름이다.
    /// 어느 쪽인지는 알리지 않는다. 받는 쪽이 그 이름을 다시 확인한다.
    Changed(OsString),
    /// 알림 버퍼가 넘쳐 변경을 잃었다. 폴더를 다시 읽어야 한다. 감시는 계속된다.
    Overflow,
    /// 감시가 스스로 끝났다(폴더가 지워짐, 네트워크 끊김 등). 이 뒤로는 알림이 없다.
    Ended(io::Error),
}

/// 폴더 하나의 변경 감시. 버리면 감시를 푼다.
pub struct DirectoryWatcher {
    #[cfg(windows)]
    _inner: crate::windows_watch::Watch,
}

impl fmt::Debug for DirectoryWatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DirectoryWatcher").finish_non_exhaustive()
    }
}

impl DirectoryWatcher {
    /// `path` 폴더의 감시를 건다. 변경은 감시 스레드에서 `on_change`로 알린다.
    ///
    /// 폴더를 열 수 없거나 그 파일 시스템이 감시를 받지 않으면 오류다. Windows가 아니면 늘
    /// [`io::ErrorKind::Unsupported`]다.
    pub fn watch(
        path: &Path,
        on_change: impl FnMut(DirectoryChange) + Send + 'static,
    ) -> io::Result<Self> {
        #[cfg(windows)]
        {
            Ok(DirectoryWatcher {
                _inner: crate::windows_watch::watch(path, Box::new(on_change))?,
            })
        }
        #[cfg(not(windows))]
        {
            let _ = (path, on_change);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "이 환경에서는 폴더 변경을 감시할 수 없습니다",
            ))
        }
    }
}

/// `FILE_NOTIFY_INFORMATION` 레코드들이 든 버퍼에서 이름들을 꺼낸다. 버퍼는 4바이트 단위다.
///
/// 레코드: 다음 레코드까지의 거리(바이트, 0이면 끝), 동작, 이름 길이(바이트), UTF-16 이름. 거리나 길이가
/// 버퍼를 벗어나면 거기서 멈춘다. 마지막 레코드는 4바이트 단위로 채워지지 않을 수 있어 `len_bytes`를 올림한다.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn parse_notifications(buffer: &[u32], len_bytes: usize) -> Vec<OsString> {
    let words = &buffer[..len_bytes.div_ceil(4).min(buffer.len())];
    let mut names = Vec::new();
    let mut at = 0usize;
    while at + 3 <= words.len() {
        let next = words[at] as usize;
        let name_len = words[at + 2] as usize;
        let start = at + 3;
        let units = name_len / 2;
        let name_words = units.div_ceil(2);
        if start + name_words > words.len() {
            break;
        }
        let name: Vec<u16> = words[start..start + name_words]
            .iter()
            .flat_map(|w| [(*w & 0xFFFF) as u16, (*w >> 16) as u16])
            .take(units)
            .collect();
        names.push(from_wide(&name));
        if next == 0 || !next.is_multiple_of(4) {
            break;
        }
        at += next / 4;
    }
    names
}

#[cfg(windows)]
fn from_wide(wide: &[u16]) -> OsString {
    use std::os::windows::ffi::OsStringExt;
    OsString::from_wide(wide)
}

#[cfg(not(windows))]
fn from_wide(wide: &[u16]) -> OsString {
    OsString::from(String::from_utf16_lossy(wide))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 레코드 하나를 4바이트 단위로 만든다. `last`가 아니면 다음 레코드까지의 거리를 채운다.
    fn record(action: u32, name: &str, last: bool) -> Vec<u32> {
        let wide: Vec<u16> = name.encode_utf16().collect();
        let mut words = vec![0, action, (wide.len() * 2) as u32];
        for pair in wide.chunks(2) {
            words.push(u32::from(pair[0]) | (u32::from(*pair.get(1).unwrap_or(&0)) << 16));
        }
        if !last {
            words[0] = (words.len() * 4) as u32;
        }
        words
    }

    #[test]
    fn 레코드_여럿의_이름을_차례로_꺼낸다() {
        let mut buffer = record(1, "새 파일.txt", false);
        buffer.extend(record(4, "옛이름", false));
        buffer.extend(record(5, "새이름🎉a", true));
        // 마지막 레코드는 채움 없이 끝난다: 머리 12바이트 + 이름 5글자(10바이트).
        let len = buffer.len() * 4 - 2;
        buffer.extend([0; 8]);
        let names = parse_notifications(&buffer, len);
        assert_eq!(names, ["새 파일.txt", "옛이름", "새이름🎉a"]);
    }

    #[test]
    fn 버퍼를_벗어나는_레코드에서_멈춘다() {
        let mut buffer = record(1, "가나다라마바사", true);
        let len = buffer.len() * 4;
        buffer[2] = 1000;
        assert!(parse_notifications(&buffer, len).is_empty());
        assert!(parse_notifications(&[], 0).is_empty());
    }
}
