//! 단일 실행: 첫 facet 프로세스가 주인이 되고, 두 번째 실행은 인자를 주인에게 넘긴 뒤 끝난다.
//!
//! - **주인 정하기**([`claim`]): 사용자 로그온 세션 범위의 이름 있는 뮤텍스. 먼저 만든 쪽 하나만 주인이다.
//!   두 쪽이 동시에 만들어도 커널이 하나만 「새로 만듦」으로 돌려준다.
//! - **받기**([`Owner::listen`]): 주인이 같은 범위 이름의 파이프를 열고 자기 스레드에서 받아 콜백으로
//!   넘긴다. UI 스레드는 파이프를 기다리지 않는다.
//! - **넘기기**([`forward`]): 두 번째 실행이 인자(경로 목록)와 작업 폴더([`Launch`])를 파이프로 보낸다.
//!   주인이 막 뜨는 중이라 파이프가 아직 없으면 정한 시간까지 짧게 다시 시도하고, 끝내 넘기지 못하면
//!   [`ForwardError`]로 돌려주어 부르는 쪽이 스스로 주인이 될지 정한다.
//!
//! 이름은 「사용자 SID + 로그온 세션 + 무결성 수준」을 담는다([`InstanceName`]). 관리자 권한으로 실행한
//! facet은 로그온 세션(연결된 관리자 토큰은 따로다)과 무결성 수준이 달라 일반 facet과 뮤텍스·파이프를
//! 나누지 않고 따로 돈다. 파이프 보안 설명자는 Windows 쪽(`windows_instance`)의 문서에 있다.
//!
//! 메시지 형식([`encode_launch`]·[`decode_launch`])은 버전을 갖는다. 너무 크거나 깨진 메시지는 버린다.
//! Windows가 아닌 플랫폼에서는 [`claim`]이 늘 주인이 되고 [`forward`]는 늘 [`ForwardError::NoOwner`]다.

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

/// 메시지 머리의 매직(「facet launch」).
const MAGIC: [u8; 4] = *b"FCTL";

/// 지금 쓰는 메시지 형식 버전. 형식을 바꾸면 올린다. 모르는 버전의 메시지는 버린다.
pub const LAUNCH_MESSAGE_VERSION: u16 = 1;

/// 메시지 본문의 최대 길이(바이트). 넘는 메시지는 보내지도 받지도 않는다.
pub const MAX_LAUNCH_MESSAGE_LEN: usize = 1024 * 1024;

/// [`forward`]가 주인의 파이프를 기다리는 기본 시간.
pub const DEFAULT_FORWARD_TIMEOUT: Duration = Duration::from_secs(2);

/// 실행 하나가 주인에게 넘기는 것: 실행 인자의 경로 목록과 그 실행의 작업 폴더.
///
/// 상대 경로는 풀지 않고 그대로 넘긴다. 받는 쪽이 [`Launch::working_dir`] 기준으로 푼다.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Launch {
    /// 실행한 쪽의 작업 폴더.
    pub working_dir: PathBuf,
    /// 실행 인자로 받은 경로들(순서대로).
    pub paths: Vec<PathBuf>,
}

/// 단일 실행 이름. 뮤텍스와 파이프 이름의 바탕이다.
///
/// 실제 facet은 [`InstanceName::facet`]을 쓴다. 테스트는 [`InstanceName::new`]로 저마다 다른 이름을 주어
/// 서로, 그리고 떠 있는 facet과 섞이지 않게 한다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceName(String);

impl InstanceName {
    /// 이름 바탕의 최대 길이.
    pub const MAX_LEN: usize = 64;

    /// 실제 facet의 이름.
    pub fn facet() -> Self {
        InstanceName("facet".into())
    }

    /// 다른 이름. ASCII 영문·숫자·`-`·`_`만, 1자 이상 [`Self::MAX_LEN`]자 이하여야 한다.
    pub fn new(base: &str) -> Result<Self, InvalidInstanceName> {
        let valid = !base.is_empty()
            && base.len() <= Self::MAX_LEN
            && base
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if valid {
            Ok(InstanceName(base.into()))
        } else {
            Err(InvalidInstanceName)
        }
    }

    /// 이름 바탕.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 이 프로세스(사용자·로그온 세션·무결성 수준)에서 이 이름의 수신기 파이프 전체 이름
    /// (`\\.\pipe\facet-instance-...`). 진단과 테스트용이다. Windows에서만 있다.
    #[cfg(windows)]
    pub fn pipe_path(&self) -> io::Result<String> {
        crate::windows_instance::pipe_path(self)
    }
}

/// [`InstanceName::new`]가 받지 않는 이름.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidInstanceName;

impl fmt::Display for InvalidInstanceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("단일 실행 이름은 영문·숫자·-·_로 된 1~64자여야 합니다")
    }
}

impl std::error::Error for InvalidInstanceName {}

/// 받은 메시지가 깨진 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchMessageError {
    /// 머리가 facet의 메시지가 아니다.
    BadMagic,
    /// 모르는 형식 버전.
    UnsupportedVersion(u16),
    /// 메시지가 중간에 끝났다.
    Truncated,
    /// 메시지 끝 뒤에 바이트가 더 있다.
    TrailingBytes,
    /// [`MAX_LAUNCH_MESSAGE_LEN`]을 넘는다.
    TooLarge,
}

impl fmt::Display for LaunchMessageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMagic => f.write_str("facet 실행 메시지가 아닙니다"),
            Self::UnsupportedVersion(v) => write!(f, "모르는 실행 메시지 버전입니다({v})"),
            Self::Truncated => f.write_str("실행 메시지가 중간에 끝났습니다"),
            Self::TrailingBytes => f.write_str("실행 메시지 뒤에 바이트가 더 있습니다"),
            Self::TooLarge => f.write_str("실행 메시지가 너무 큽니다"),
        }
    }
}

impl std::error::Error for LaunchMessageError {}

/// [`Launch`]를 메시지 본문으로 바꾼다.
///
/// 형식(모든 정수는 리틀 엔디언): 매직 `FCTL`, 버전 `u16`, 작업 폴더 문자열, 경로 수 `u32`, 경로 문자열들.
/// 문자열은 UTF-16 단위 수 `u32` 뒤에 그 단위들(`u16`). Windows 경로는 짝이 맞지 않는 서로게이트까지
/// 그대로 담는다. 본문이 [`MAX_LAUNCH_MESSAGE_LEN`]을 넘으면 오류다.
pub fn encode_launch(launch: &Launch) -> Result<Vec<u8>, LaunchMessageError> {
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&LAUNCH_MESSAGE_VERSION.to_le_bytes());
    put_string(&mut out, launch.working_dir.as_os_str())?;
    let count = u32::try_from(launch.paths.len()).map_err(|_| LaunchMessageError::TooLarge)?;
    out.extend_from_slice(&count.to_le_bytes());
    for path in &launch.paths {
        put_string(&mut out, path.as_os_str())?;
        if out.len() > MAX_LAUNCH_MESSAGE_LEN {
            return Err(LaunchMessageError::TooLarge);
        }
    }
    if out.len() > MAX_LAUNCH_MESSAGE_LEN {
        return Err(LaunchMessageError::TooLarge);
    }
    Ok(out)
}

/// 메시지 본문을 [`Launch`]로 읽는다. 깨졌거나 모르는 버전이거나 너무 크면 오류다.
pub fn decode_launch(bytes: &[u8]) -> Result<Launch, LaunchMessageError> {
    if bytes.len() > MAX_LAUNCH_MESSAGE_LEN {
        return Err(LaunchMessageError::TooLarge);
    }
    let mut reader = Reader(bytes);
    if reader.take(4)? != MAGIC {
        return Err(LaunchMessageError::BadMagic);
    }
    let version = u16::from_le_bytes(reader.array()?);
    if version != LAUNCH_MESSAGE_VERSION {
        return Err(LaunchMessageError::UnsupportedVersion(version));
    }
    let working_dir = PathBuf::from(reader.string()?);
    let count = u32::from_le_bytes(reader.array()?) as usize;
    // 문자열 하나는 4바이트 이상이므로 남은 길이로 개수를 미리 가린다(큰 개수로 메모리를 잡지 않게).
    if count > reader.0.len() / 4 {
        return Err(LaunchMessageError::Truncated);
    }
    let mut paths = Vec::with_capacity(count);
    for _ in 0..count {
        paths.push(PathBuf::from(reader.string()?));
    }
    if !reader.0.is_empty() {
        return Err(LaunchMessageError::TrailingBytes);
    }
    Ok(Launch { working_dir, paths })
}

fn put_string(out: &mut Vec<u8>, text: &std::ffi::OsStr) -> Result<(), LaunchMessageError> {
    let units = to_units(text);
    if units.len() > MAX_LAUNCH_MESSAGE_LEN / 2 {
        return Err(LaunchMessageError::TooLarge);
    }
    out.extend_from_slice(&(units.len() as u32).to_le_bytes());
    for unit in units {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    Ok(())
}

#[cfg(windows)]
fn to_units(text: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    text.encode_wide().collect()
}

#[cfg(not(windows))]
fn to_units(text: &std::ffi::OsStr) -> Vec<u16> {
    text.to_string_lossy().encode_utf16().collect()
}

#[cfg(windows)]
fn from_units(units: &[u16]) -> OsString {
    use std::os::windows::ffi::OsStringExt;
    OsString::from_wide(units)
}

#[cfg(not(windows))]
fn from_units(units: &[u16]) -> OsString {
    String::from_utf16_lossy(units).into()
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], LaunchMessageError> {
        if self.0.len() < len {
            return Err(LaunchMessageError::Truncated);
        }
        let (head, rest) = self.0.split_at(len);
        self.0 = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], LaunchMessageError> {
        let mut out = [0; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    fn string(&mut self) -> Result<OsString, LaunchMessageError> {
        let len = u32::from_le_bytes(self.array()?) as usize;
        let bytes = self.take(len.checked_mul(2).ok_or(LaunchMessageError::Truncated)?)?;
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        Ok(from_units(&units))
    }
}

/// [`forward`]가 넘기지 못한 까닭.
#[derive(Debug)]
pub enum ForwardError {
    /// 정한 시간 안에 주인의 파이프를 찾지 못했다(주인이 없거나 아직 받을 준비가 안 됐다).
    NoOwner,
    /// 파이프는 있지만 이 사용자가 만든 것이 아니다. 넘기지 않는다.
    NotOurs,
    /// 주인이 메시지를 받지 않았다고 답했다.
    Rejected,
    /// 넘길 것이 [`MAX_LAUNCH_MESSAGE_LEN`]을 넘는다.
    TooLarge,
    /// 그 밖의 입출력 오류.
    Io(io::Error),
}

impl fmt::Display for ForwardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoOwner => f.write_str("떠 있는 facet을 찾지 못했습니다"),
            Self::NotOurs => f.write_str("파이프가 이 사용자의 facet이 만든 것이 아닙니다"),
            Self::Rejected => f.write_str("떠 있는 facet이 실행 인자를 받지 않았습니다"),
            Self::TooLarge => f.write_str("실행 인자가 너무 깁니다"),
            Self::Io(error) => write!(f, "실행 인자를 넘기지 못했습니다: {error}"),
        }
    }
}

impl std::error::Error for ForwardError {}

/// 주인 되기의 결과.
pub enum Claim {
    /// 이 프로세스가 주인이다. 값을 버리면 주인 자리를 내놓는다.
    Owner(Owner),
    /// 이미 주인이 있다. [`forward`]로 넘긴다.
    Taken,
}

impl fmt::Debug for Claim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Claim::Owner(_) => f.write_str("Claim::Owner"),
            Claim::Taken => f.write_str("Claim::Taken"),
        }
    }
}

/// 주인 자리. 뮤텍스와 (열었으면) 수신기를 갖는다. 버리면 수신기를 멈추고 뮤텍스를 놓는다.
pub struct Owner {
    #[cfg(windows)]
    inner: crate::windows_instance::Owner,
}

impl Owner {
    /// 수신기를 연다. 받은 실행마다 수신기 스레드에서 `on_launch`를 부른다(UI 스레드가 아니다).
    /// 깨진 메시지는 버리고 부르지 않는다. 이미 열었으면 오류다.
    pub fn listen(&mut self, on_launch: impl FnMut(Launch) + Send + 'static) -> io::Result<()> {
        #[cfg(windows)]
        {
            self.inner.listen(Box::new(on_launch))
        }
        #[cfg(not(windows))]
        {
            let _ = on_launch;
            Ok(())
        }
    }
}

/// 주인이 되려 한다. 같은 이름의 주인이 이미 있으면 [`Claim::Taken`]이다.
///
/// 이름의 범위는 이 사용자·이 로그온 세션·이 무결성 수준이다(모듈 문서).
pub fn claim(name: &InstanceName) -> io::Result<Claim> {
    #[cfg(windows)]
    {
        Ok(match crate::windows_instance::claim(name)? {
            Some(inner) => Claim::Owner(Owner { inner }),
            None => Claim::Taken,
        })
    }
    #[cfg(not(windows))]
    {
        let _ = name;
        Ok(Claim::Owner(Owner {}))
    }
}

/// 주인에게 `launch`를 넘긴다. 파이프가 아직 없거나 바쁘면 `timeout`까지 짧게 다시 시도한다.
/// 주인이 받았다고 답하면 `Ok`다.
pub fn forward(
    name: &InstanceName,
    launch: &Launch,
    timeout: Duration,
) -> Result<(), ForwardError> {
    let body = encode_launch(launch).map_err(|_| ForwardError::TooLarge)?;
    #[cfg(windows)]
    {
        crate::windows_instance::forward(name, &body, timeout)
    }
    #[cfg(not(windows))]
    {
        let _ = (name, body, timeout);
        Err(ForwardError::NoOwner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Launch {
        Launch {
            working_dir: PathBuf::from(r"C:\작업 폴더"),
            paths: vec![
                PathBuf::from(r"C:\Users\사용자\문서 모음"),
                PathBuf::from("상대 경로"),
                PathBuf::from(r"D:\a b\c"),
            ],
        }
    }

    #[test]
    fn 메시지는_되읽으면_같다() {
        let launch = sample();
        assert_eq!(decode_launch(&encode_launch(&launch).unwrap()), Ok(launch));
        let empty = Launch::default();
        assert_eq!(decode_launch(&encode_launch(&empty).unwrap()), Ok(empty));
    }

    #[cfg(windows)]
    #[test]
    fn 짝이_맞지_않는_서로게이트도_그대로_간다() {
        use std::os::windows::ffi::OsStringExt;
        let odd = PathBuf::from(OsString::from_wide(&[0x0041, 0xD800, 0x0042]));
        let launch = Launch {
            working_dir: odd.clone(),
            paths: vec![odd],
        };
        assert_eq!(decode_launch(&encode_launch(&launch).unwrap()), Ok(launch));
    }

    #[test]
    fn 깨진_메시지는_버린다() {
        let bytes = encode_launch(&sample()).unwrap();
        for len in 0..bytes.len() {
            assert!(
                decode_launch(&bytes[..len]).is_err(),
                "{len}바이트에서 끊김"
            );
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(
            decode_launch(&trailing),
            Err(LaunchMessageError::TrailingBytes)
        );
        let mut magic = bytes.clone();
        magic[0] = b'X';
        assert_eq!(decode_launch(&magic), Err(LaunchMessageError::BadMagic));
        let mut version = bytes;
        version[4..6].copy_from_slice(&2u16.to_le_bytes());
        assert_eq!(
            decode_launch(&version),
            Err(LaunchMessageError::UnsupportedVersion(2))
        );
    }

    #[test]
    fn 경로_수가_남은_길이보다_크면_잘린_것이다() {
        let mut bytes = encode_launch(&Launch::default()).unwrap();
        let at = bytes.len() - 4;
        bytes[at..].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode_launch(&bytes), Err(LaunchMessageError::Truncated));
    }

    #[test]
    fn 너무_큰_메시지는_만들지도_받지도_않는다() {
        let long = "가".repeat(MAX_LAUNCH_MESSAGE_LEN / 2);
        let launch = Launch {
            working_dir: PathBuf::from("."),
            paths: vec![PathBuf::from(long)],
        };
        assert_eq!(encode_launch(&launch), Err(LaunchMessageError::TooLarge));
        let many = Launch {
            working_dir: PathBuf::from("."),
            paths: vec![PathBuf::from("가".repeat(1000)); 600],
        };
        assert_eq!(encode_launch(&many), Err(LaunchMessageError::TooLarge));
        assert_eq!(
            decode_launch(&vec![0; MAX_LAUNCH_MESSAGE_LEN + 1]),
            Err(LaunchMessageError::TooLarge)
        );
    }

    #[test]
    fn 이름은_영문_숫자_줄표_밑줄만_받는다() {
        assert!(InstanceName::new("facet-test_1").is_ok());
        assert!(InstanceName::new("").is_err());
        assert!(InstanceName::new("a\\b").is_err());
        assert!(InstanceName::new("한글").is_err());
        assert!(InstanceName::new(&"a".repeat(65)).is_err());
        assert_eq!(InstanceName::facet().as_str(), "facet");
    }
}
