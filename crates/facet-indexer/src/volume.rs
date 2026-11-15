//! NTFS 볼륨 핸들, 드라이브 문자 목록, USN 저널 읽기(Windows 전용).

use std::ffi::c_void;
use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::mem::size_of;
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_JOURNAL_NOT_ACTIVE, GENERIC_READ, GENERIC_WRITE,
    HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, FlushFileBuffers, GetDriveTypeW,
    GetLogicalDrives, GetVolumeInformationW, OPEN_EXISTING, ReadFile,
};
use windows_sys::Win32::System::IO::{DeviceIoControl, OVERLAPPED};
use windows_sys::Win32::System::Ioctl::{
    CREATE_USN_JOURNAL_DATA, FSCTL_CREATE_USN_JOURNAL, FSCTL_GET_NTFS_VOLUME_DATA,
    FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_USN_JOURNAL, NTFS_VOLUME_DATA_BUFFER,
    READ_USN_JOURNAL_DATA_V0, USN_JOURNAL_DATA_V0,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use facet_index::{Index, MftReport};

use crate::build;
use crate::cli::DumpArgs;
use crate::dump::{self, Geometry, ReadAt, Summary};
use crate::follow;
use crate::volumes::{self, Plan};

/// 관리자 권한이 없을 때 알리는 말.
pub const NEED_ADMIN: &str =
    "관리자 권한이 필요합니다. 볼륨 핸들을 열어 MFT를 읽으려면 관리자 터미널에서 다시 실행하세요.";

/// 직접 읽기 버퍼의 정렬. 섹터 크기(512 또는 4096)의 배수다.
const ALIGN: usize = 4096;

/// 소유한 핸들. 버릴 때 닫는다.
pub(crate) struct Handle(pub(crate) HANDLE);

// SAFETY: 커널 객체 핸들은 프로세스 전체에서 유효한 번호라 어느 스레드로 옮겨도 된다.
unsafe impl Send for Handle {}
// SAFETY: &Handle로는 번호를 읽을 수만 있고, 닫기는 Drop에서 한 번만 한다. 같은 핸들에 여러
// 스레드가 동시에 입출력하는 것은 Windows가 허용한다(겹친 입출력은 호출마다 구조체를 따로 둔다).
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: self.0은 CreateFileW/OpenProcessToken이 돌려준 유효한 핸들이고 여기서 한 번만 닫는다.
        unsafe { CloseHandle(self.0) };
    }
}

/// 이 프로세스가 관리자 권한(상승된 토큰)으로 도는지.
pub fn is_elevated() -> bool {
    let mut token: HANDLE = null_mut();
    // SAFETY: GetCurrentProcess는 가짜 핸들을 돌려주고, token은 쓸 수 있는 지역 변수다.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return false;
    }
    let token = Handle(token);
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut returned = 0u32;
    // SAFETY: 버퍼는 TOKEN_ELEVATION 하나이고 그 크기를 함께 넘긴다.
    let ok = unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast::<c_void>(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    };
    ok != 0 && elevation.TokenIsElevated != 0
}

/// 읽기용으로 연 NTFS 볼륨.
pub struct Volume {
    handle: Handle,
    drive: char,
    /// 정렬을 맞추기 위해 여유를 두고 잡은 읽기 버퍼.
    scratch: Vec<u8>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn open_raw(drive: char, access: u32) -> io::Result<Handle> {
    let path = wide(&format!(r"\\.\{drive}:"));
    // SAFETY: path는 NUL로 끝나는 UTF-16 문자열이고, 보안 속성과 템플릿은 쓰지 않는다.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            0,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        Ok(Handle(handle))
    }
}

fn describe_open_error(drive: char, e: &io::Error) -> String {
    if e.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) {
        NEED_ADMIN.to_owned()
    } else {
        format!("볼륨 {drive}:를 열지 못함: {e}")
    }
}

impl Volume {
    /// 볼륨을 연다. 관리자가 아니면 그 이유를 담은 오류를 낸다.
    pub fn open(drive: char) -> Result<Self, String> {
        if !is_elevated() {
            return Err(NEED_ADMIN.to_owned());
        }
        let handle = open_raw(drive, GENERIC_READ).map_err(|e| describe_open_error(drive, &e))?;
        Ok(Self {
            handle,
            drive,
            scratch: Vec::new(),
        })
    }

    /// 볼륨의 쓰기 버퍼를 디스크로 내보낸다. 방금 만든 항목이 떠 낸 MFT에 들어가게 한다.
    ///
    /// Windows는 볼륨 핸들의 FlushFileBuffers에 쓰기 접근을 요구한다. 이 핸들로는 아무것도 쓰지
    /// 않고 버퍼만 비운다.
    pub fn flush(&self) -> Result<(), String> {
        let handle = open_raw(self.drive, GENERIC_READ | GENERIC_WRITE)
            .map_err(|e| describe_open_error(self.drive, &e))?;
        // SAFETY: handle은 방금 연 유효한 볼륨 핸들이다.
        if unsafe { FlushFileBuffers(handle.0) } == 0 {
            return Err(format!(
                "볼륨 {}:의 버퍼를 비우지 못함: {}",
                self.drive,
                io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    /// NTFS 기하 정보. NTFS가 아니면 오류다.
    pub fn geometry(&self) -> Result<Geometry, String> {
        // SAFETY: NTFS_VOLUME_DATA_BUFFER는 정수만 담은 C 구조체라 0으로 채워도 유효하다.
        let mut data: NTFS_VOLUME_DATA_BUFFER = unsafe { std::mem::zeroed() };
        let mut returned = 0u32;
        // SAFETY: 입력 버퍼는 없고, 출력 버퍼는 data 하나이며 그 크기를 함께 넘긴다. 동기 핸들이다.
        let ok = unsafe {
            DeviceIoControl(
                self.handle.0,
                FSCTL_GET_NTFS_VOLUME_DATA,
                null(),
                0,
                (&mut data as *mut NTFS_VOLUME_DATA_BUFFER).cast::<c_void>(),
                size_of::<NTFS_VOLUME_DATA_BUFFER>() as u32,
                &mut returned,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(format!(
                "볼륨 {}:의 NTFS 정보를 얻지 못함(NTFS 볼륨이 아닐 수 있음): {}",
                self.drive,
                io::Error::last_os_error()
            ));
        }
        let non_negative = |v: i64, what: &str| {
            u64::try_from(v).map_err(|_| format!("볼륨의 {what} 값이 음수임: {v}"))
        };
        Ok(Geometry {
            bytes_per_sector: data.BytesPerSector,
            bytes_per_cluster: data.BytesPerCluster,
            bytes_per_record: data.BytesPerFileRecordSegment,
            mft_start_lcn: non_negative(data.MftStartLcn, "MFT 시작 위치")?,
            mft_valid_data_length: non_negative(data.MftValidDataLength, "MFT 유효 길이")?,
        })
    }
}

impl ReadAt for Volume {
    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let len = u32::try_from(buf.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "읽기 덩어리가 너무 큼"))?;
        if self.scratch.len() < buf.len() + ALIGN {
            self.scratch.resize(buf.len() + ALIGN, 0);
        }
        let skip = self.scratch.as_ptr().align_offset(ALIGN);
        let aligned = &mut self.scratch[skip..skip + buf.len()];

        // SAFETY: OVERLAPPED는 정수와 핸들만 담은 C 구조체라 0으로 채워도 유효하다.
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.Anonymous.Anonymous.Offset = offset as u32;
        overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
        let mut read = 0u32;
        // SAFETY: aligned는 len 바이트를 쓸 수 있는 버퍼이고, 핸들은 동기 핸들이라 호출이 끝나면
        // overlapped와 버퍼를 더 쓰지 않는다.
        let ok = unsafe {
            ReadFile(
                self.handle.0,
                aligned.as_mut_ptr(),
                len,
                &mut read,
                &mut overlapped,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        if read != len {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("{len}바이트를 요청했지만 {read}바이트만 읽음"),
            ));
        }
        buf.copy_from_slice(aligned);
        Ok(())
    }
}

/// `dump-mft` 하위 명령: 볼륨을 열고, 버퍼를 비우고, MFT 레코드를 파일로 떠 낸다.
///
/// 실패하면 반쯤 쓴 출력 파일을 지운다.
pub fn dump_to_file(args: &DumpArgs) -> Result<Summary, String> {
    let mut volume = Volume::open(args.drive)?;
    let geometry = volume.geometry()?;
    if let Err(e) = volume.flush() {
        eprintln!("경고: {e}. 최근 변경이 떠 낸 MFT에 빠질 수 있습니다.");
    }
    let file = File::create(&args.output)
        .map_err(|e| format!("출력 파일 {}을 만들지 못함: {e}", args.output.display()))?;
    let mut out = BufWriter::new(file);
    let result = dump::dump_mft(&mut volume, &geometry, args.start, args.count, &mut out);
    drop(out);
    if result.is_err() {
        remove_quietly(&args.output);
    }
    result
}

fn remove_quietly(path: &Path) {
    let _ = fs::remove_file(path);
}

/// 드라이브 문자가 있는 볼륨들과 그 다룰 방법(드라이브 문자 순서). 볼륨 이름은 `C:` 꼴이다.
///
/// 볼륨이 붙어 있지 않거나 매체가 없는 드라이브(빈 카드 리더 등)는 빠진다. 네트워크 드라이브는
/// 「네트워크 볼륨」, NTFS가 아닌 로컬 볼륨은 「NTFS가 아님」으로 든다([`volumes::classify`]).
pub fn local_volumes() -> Vec<(String, Plan)> {
    // SAFETY: 인자 없는 호출이다.
    let mask = unsafe { GetLogicalDrives() };
    ('A'..='Z')
        .enumerate()
        .filter(|(bit, _)| mask & (1 << bit) != 0)
        .filter_map(|(_, letter)| {
            let root = wide(&format!(r"{letter}:\"));
            // SAFETY: root는 NUL로 끝나는 UTF-16 문자열이다.
            let kind = unsafe { GetDriveTypeW(root.as_ptr()) };
            let filesystem = (kind != volumes::drive_type::REMOTE)
                .then(|| filesystem_name(&root))
                .flatten();
            volumes::classify(kind, filesystem.as_deref()).map(|plan| (format!("{letter}:"), plan))
        })
        .collect()
}

/// 볼륨 루트(`C:\`, NUL로 끝나는 UTF-16)의 파일 시스템 이름. 읽지 못하면 `None`.
fn filesystem_name(root: &[u16]) -> Option<String> {
    let mut name = [0u16; 64];
    // SAFETY: root는 NUL로 끝나는 UTF-16 문자열이고, name은 그 길이를 함께 넘기는 버퍼다. 받지
    // 않는 값은 모두 널이다.
    let ok = unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            null_mut(),
            0,
            null_mut(),
            null_mut(),
            null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        )
    };
    if ok == 0 {
        return None;
    }
    let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    Some(String::from_utf16_lossy(&name[..len]))
}

/// 드라이브 `drive`(대문자)의 첫 인덱스를 만든다: 볼륨을 열고, 쓰기 버퍼를 비우고, MFT 전체를 큰
/// 덩어리로 순서대로 읽는다. 관리자(서비스는 LocalSystem) 권한이 필요하다.
pub fn build_volume(drive: char) -> Result<(Index, MftReport), String> {
    let mut volume = Volume::open(drive)?;
    let geometry = volume.geometry()?;
    // 방금 바뀐 항목이 디스크의 MFT에 들어가게 한다. 못 해도 인덱스는 만든다(뒤의 변경은 USN
    // 저널로 따라간다).
    let _ = volume.flush();
    build::build_index(&mut volume, &geometry, &format!("{drive}:"))
}

/// 저널 읽기 버퍼 크기. 한 번에 이만큼의 레코드를 읽는다.
const JOURNAL_BUFFER: usize = 64 * 1024;

/// 저널이 없는 볼륨에 만드는 저널의 최대 크기와 늘리는 단위(Windows의 기본값과 같은 크기다).
const NEW_JOURNAL_SIZE: u64 = 32 * 1024 * 1024;
const NEW_JOURNAL_DELTA: u64 = 8 * 1024 * 1024;

/// 실제 NTFS 볼륨(드라이브 문자 하나)을 읽는 [`follow::Source`]. 관리자(서비스는 LocalSystem)
/// 권한이 필요하다.
pub struct NtfsSource {
    drive: char,
    /// 저널을 읽는 볼륨 핸들. 처음 쓸 때 연다.
    handle: Option<Handle>,
    buffer: Vec<u8>,
}

impl NtfsSource {
    /// 드라이브 `drive`(대문자)의 볼륨.
    pub fn new(drive: char) -> Self {
        Self {
            drive,
            handle: None,
            buffer: vec![0; JOURNAL_BUFFER],
        }
    }

    fn handle(&mut self) -> Result<&Handle, String> {
        if self.handle.is_none() {
            let handle = open_raw(self.drive, GENERIC_READ)
                .map_err(|e| describe_open_error(self.drive, &e))?;
            self.handle = Some(handle);
        }
        Ok(self.handle.as_ref().expect("방금 열었다"))
    }

    /// `FSCTL_QUERY_USN_JOURNAL`. 실패하면 OS 오류.
    fn query(&mut self) -> Result<io::Result<USN_JOURNAL_DATA_V0>, String> {
        let handle = self.handle()?.0;
        let mut data = USN_JOURNAL_DATA_V0::default();
        let mut returned = 0u32;
        // SAFETY: 입력 버퍼는 없고, 출력 버퍼는 data 하나이며 그 크기를 함께 넘긴다. 동기 핸들이다.
        let ok = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_QUERY_USN_JOURNAL,
                null(),
                0,
                (&mut data as *mut USN_JOURNAL_DATA_V0).cast::<c_void>(),
                size_of::<USN_JOURNAL_DATA_V0>() as u32,
                &mut returned,
                null_mut(),
            )
        };
        Ok(if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(data)
        })
    }

    /// 볼륨에 USN 저널을 만든다(`FSCTL_CREATE_USN_JOURNAL`). 이미 있으면 그대로 둔다.
    fn create_journal(&self) -> Result<(), String> {
        let handle = open_raw(self.drive, GENERIC_READ | GENERIC_WRITE)
            .map_err(|e| describe_open_error(self.drive, &e))?;
        let create = CREATE_USN_JOURNAL_DATA {
            MaximumSize: NEW_JOURNAL_SIZE,
            AllocationDelta: NEW_JOURNAL_DELTA,
        };
        let mut returned = 0u32;
        // SAFETY: 입력 버퍼는 create 하나이고 그 크기를 함께 넘긴다. 출력 버퍼는 없다. 동기 핸들이다.
        let ok = unsafe {
            DeviceIoControl(
                handle.0,
                FSCTL_CREATE_USN_JOURNAL,
                (&create as *const CREATE_USN_JOURNAL_DATA).cast::<c_void>(),
                size_of::<CREATE_USN_JOURNAL_DATA>() as u32,
                null_mut(),
                0,
                &mut returned,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(format!(
                "볼륨 {}:에 USN 저널을 만들지 못함: {}",
                self.drive,
                io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}

impl follow::Source for NtfsSource {
    fn journal(&mut self) -> Result<follow::JournalNow, String> {
        let data = match self.query()? {
            Ok(data) => data,
            Err(e) if e.raw_os_error() == Some(ERROR_JOURNAL_NOT_ACTIVE as i32) => {
                // 저널이 꺼진 볼륨이다. 변경을 따라가려면 저널이 있어야 하므로 만든다.
                self.create_journal()?;
                self.query()?
                    .map_err(|e| format!("볼륨 {}:의 USN 저널을 읽지 못함: {e}", self.drive))?
            }
            Err(e) => {
                // 핸들이 낡았을 수 있다(볼륨이 다시 붙음 등). 다음에는 새로 연다.
                self.handle = None;
                return Err(format!("볼륨 {}:의 USN 저널을 읽지 못함: {e}", self.drive));
            }
        };
        Ok(follow::JournalNow {
            journal_id: data.UsnJournalID,
            first_usn: data.FirstUsn,
            next_usn: data.NextUsn,
        })
    }

    fn read(&mut self, journal_id: u64, start: i64) -> Result<follow::Records, String> {
        let handle = self.handle()?.0;
        let input = READ_USN_JOURNAL_DATA_V0 {
            StartUsn: start,
            ReasonMask: u32::MAX,
            // 닫을 때만이 아니라 변경마다 받는다. 이름바꾸기의 옛 이름 레코드가 필요하다.
            ReturnOnlyOnClose: 0,
            // 기다리지 않는다.
            Timeout: 0,
            BytesToWaitFor: 0,
            UsnJournalID: journal_id,
        };
        let mut returned = 0u32;
        // SAFETY: 입력 버퍼는 input 하나, 출력 버퍼는 self.buffer이고 각 크기를 함께 넘긴다. 동기
        // 핸들이라 호출이 끝나면 버퍼를 더 쓰지 않는다.
        let ok = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_READ_USN_JOURNAL,
                (&input as *const READ_USN_JOURNAL_DATA_V0).cast::<c_void>(),
                size_of::<READ_USN_JOURNAL_DATA_V0>() as u32,
                self.buffer.as_mut_ptr().cast::<c_void>(),
                self.buffer.len() as u32,
                &mut returned,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(format!(
                "볼륨 {}:의 USN 저널 레코드를 읽지 못함: {}",
                self.drive,
                io::Error::last_os_error()
            ));
        }
        // 앞 8바이트는 다음에 읽기 시작할 USN이고 그 뒤가 레코드다.
        let got = &self.buffer[..(returned as usize).min(self.buffer.len())];
        let Some((next, records)) = got.split_first_chunk::<8>() else {
            return Err(format!(
                "볼륨 {}:의 USN 저널 응답이 너무 짧음({}바이트)",
                self.drive,
                got.len()
            ));
        };
        Ok(follow::Records {
            next_usn: i64::from_le_bytes(*next),
            bytes: records.to_vec(),
        })
    }

    fn build(&mut self) -> Result<Index, String> {
        build_volume(self.drive).map(|(index, _report)| index)
    }
}
