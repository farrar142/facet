//! NTFS 볼륨 핸들(Windows 전용). 이 크레이트의 `unsafe`는 모두 여기 있다.

use std::ffi::c_void;
use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::mem::size_of;
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, FlushFileBuffers, OPEN_EXISTING, ReadFile,
};
use windows_sys::Win32::System::IO::{DeviceIoControl, OVERLAPPED};
use windows_sys::Win32::System::Ioctl::{FSCTL_GET_NTFS_VOLUME_DATA, NTFS_VOLUME_DATA_BUFFER};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::cli::DumpArgs;
use crate::dump::{self, Geometry, ReadAt, Summary};

/// 관리자 권한이 없을 때 알리는 말.
pub const NEED_ADMIN: &str =
    "관리자 권한이 필요합니다. 볼륨 핸들을 열어 MFT를 읽으려면 관리자 터미널에서 다시 실행하세요.";

/// 직접 읽기 버퍼의 정렬. 섹터 크기(512 또는 4096)의 배수다.
const ALIGN: usize = 4096;

/// 소유한 핸들. 버릴 때 닫는다.
struct Handle(HANDLE);

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
