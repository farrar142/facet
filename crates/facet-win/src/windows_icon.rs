//! 시스템 아이콘을 얻는 Windows API 호출([`crate::icon`]).
//!
//! 셸의 시스템 아이콘 목록(`SHGetFileInfoW` + `SHGetImageList`)에서 아이콘 번호를 찾아 그 아이콘을 요청 크기의
//! 32비트 DIB에 두 번(검은 바탕·흰 바탕) 그리고, 두 결과의 차이로 투명도를 되살려 RGBA로 바꾼다. 이렇게 하면
//! 아이콘이 알파 채널을 가졌든 마스크만 가졌든 같은 길로 RGBA가 된다.

use std::ffi::OsStr;
use std::path::Path;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLACK_BRUSH, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, FillRect, GdiFlush, GetStockObject, HBRUSH, HDC,
    HGDIOBJ, SelectObject, WHITE_BRUSH,
};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES,
};
use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT};
use windows::Win32::UI::Shell::{
    SHFILEINFOW, SHGFI_FLAGS, SHGFI_PIDL, SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES,
    SHGetFileInfoW, SHGetImageList, SHIL_EXTRALARGE, SHIL_JUMBO, SHIL_LARGE, SHIL_SMALL,
};
use windows::Win32::UI::WindowsAndMessaging::{DI_NORMAL, DestroyIcon, DrawIconEx, HICON};
use windows::core::PCWSTR;

use crate::icon::IconImage;
use crate::windows_impl::{ComApartment, to_wide};

/// 아이콘 얻기의 차례(잡혀 있으면 참).
static TURN: Mutex<bool> = Mutex::new(false);
/// 차례가 풀렸다는 알림.
static TURN_FREE: Condvar = Condvar::new();

/// 차례를 기다리는 가장 긴 시간. 가상 폴더 항목(끊긴 네트워크 드라이브·휴대폰)의 아이콘 얻기는 셸 안에서 오래 멈출 수
/// 있고, 그 호출은 시간 제한을 넘으면 버려진 채 차례를 쥐고 있다. 그래도 다른 아이콘이 영영 멈추지 않게 이만큼
/// 기다린 뒤에는 차례 없이 부른다(드물게 동시 호출로 실패하면 그 아이콘만 없다). 보통 아이콘 하나는 수 ms라 넉넉하다.
const TURN_WAIT: Duration = Duration::from_secs(2);

/// 잡은 차례. 버리면 푼다(잡지 못했으면 아무것도 하지 않음).
struct Turn(bool);

impl Turn {
    fn take() -> Turn {
        let deadline = Instant::now() + TURN_WAIT;
        let mut busy = TURN.lock().unwrap_or_else(|e| e.into_inner());
        while *busy {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Turn(false);
            }
            busy = TURN_FREE
                .wait_timeout(busy, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        *busy = true;
        Turn(true)
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        if self.0 {
            *TURN.lock().unwrap_or_else(|e| e.into_inner()) = false;
            TURN_FREE.notify_one();
        }
    }
}

/// 그릴 수 있는 가장 큰 크기(시스템 아이콘 목록의 가장 큰 그림 256).
const MAX_SIZE: u32 = 256;

pub(crate) fn extension_icon(extension: &str, size: u32) -> Option<IconImage> {
    let name = format!("x.{extension}");
    icon_of(
        OsStr::new(&name),
        FILE_ATTRIBUTE_NORMAL,
        SHGFI_USEFILEATTRIBUTES,
        size,
    )
}

pub(crate) fn folder_icon(size: u32) -> Option<IconImage> {
    icon_of(
        OsStr::new("folder"),
        FILE_ATTRIBUTE_DIRECTORY,
        SHGFI_USEFILEATTRIBUTES,
        size,
    )
}

pub(crate) fn file_icon(path: &Path, size: u32) -> Option<IconImage> {
    icon_of(
        path.as_os_str(),
        FILE_FLAGS_AND_ATTRIBUTES(0),
        SHGFI_FLAGS(0),
        size,
    )
}

/// 셸 항목(절대 PIDL 바이트, 형식은 부른 쪽이 확인함)의 아이콘.
pub(crate) fn shell_item_icon(id: &[u8], size: u32) -> Option<IconImage> {
    if size == 0 || size > MAX_SIZE {
        return None;
    }
    // PIDL을 8바이트 정렬된 메모리에 옮긴다.
    let mut aligned = vec![0u64; id.len().div_ceil(8)];
    // SAFETY: aligned는 id.len()바이트 이상이고 id와 겹치지 않는다.
    unsafe {
        std::ptr::copy_nonoverlapping(id.as_ptr(), aligned.as_mut_ptr().cast::<u8>(), id.len())
    };
    let _turn = Turn::take();
    let _com = ComApartment::enter().ok();
    let mut info = SHFILEINFOW::default();
    // SAFETY: SHGFI_PIDL이면 첫 인자는 PIDL 주소다. aligned는 호출 동안 살아 있는 올바른 PIDL이고 info는 호출 동안
    // 살아 있는 SHFILEINFOW다. SHGFI_SYSICONINDEX는 아이콘 핸들을 만들지 않으므로 풀 것이 없다.
    let found = unsafe {
        SHGetFileInfoW(
            PCWSTR(aligned.as_ptr().cast::<u16>()),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_PIDL | SHGFI_SYSICONINDEX,
        )
    };
    if found == 0 {
        return None;
    }
    let icon = OwnedIcon(list_icon(info.iIcon, size)?);
    render(icon.0, size)
}

fn icon_of(
    name: &OsStr,
    attributes: FILE_FLAGS_AND_ATTRIBUTES,
    flags: SHGFI_FLAGS,
    size: u32,
) -> Option<IconImage> {
    if size == 0 || size > MAX_SIZE {
        return None;
    }
    // 시스템 아이콘 목록을 여러 스레드가 동시에 처음 만들거나 읽으면 셸이 실패를 돌려준다(여러 스레드에서 부른
    // 시험에서 봄). 프로세스 안의 아이콘 얻기를 하나씩 한다. 앱은 아이콘 일꾼 스레드 하나에서만 부르므로 늦어지지 않는다.
    let _turn = Turn::take();
    // 셸 호출은 COM이 초기화된 스레드를 바란다. 이미 다른 아파트로 초기화된 스레드면 그대로 부른다.
    let _com = ComApartment::enter().ok();
    let index = icon_index(name, attributes, flags)?;
    let icon = OwnedIcon(list_icon(index, size)?);
    render(icon.0, size)
}

/// 시스템 아이콘 목록에서의 아이콘 번호.
fn icon_index(
    name: &OsStr,
    attributes: FILE_FLAGS_AND_ATTRIBUTES,
    flags: SHGFI_FLAGS,
) -> Option<i32> {
    let wide = to_wide(name);
    let mut info = SHFILEINFOW::default();
    // SAFETY: wide는 NUL로 끝나는 UTF-16 문자열이고 info는 호출 동안 살아 있는 SHFILEINFOW다. 크기는 그 구조체의
    // 크기다. SHGFI_SYSICONINDEX는 아이콘 핸들을 만들지 않으므로 풀 것이 없다.
    let found = unsafe {
        SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            attributes,
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            flags | SHGFI_SYSICONINDEX,
        )
    };
    (found != 0).then_some(info.iIcon)
}

/// 요청 크기 이상인 가장 작은 시스템 아이콘 목록에서 아이콘 핸들을 얻는다(크기는 그릴 때 맞춘다).
fn list_icon(index: i32, size: u32) -> Option<HICON> {
    let list = match size {
        0..=16 => SHIL_SMALL,
        17..=32 => SHIL_LARGE,
        33..=48 => SHIL_EXTRALARGE,
        _ => SHIL_JUMBO,
    };
    // SAFETY: 목록 종류는 상수이고, 돌려받은 COM 객체는 이 함수 안에서만 쓴다.
    let images: IImageList = unsafe { SHGetImageList(list as i32) }.ok()?;
    // SAFETY: images는 살아 있는 COM 객체이고 index는 셸이 준 그 목록의 번호다. 받은 핸들은 부른 쪽이 푼다.
    unsafe { images.GetIcon(index, ILD_TRANSPARENT.0) }.ok()
}

/// 버리면 푸는 아이콘 핸들.
struct OwnedIcon(HICON);

impl Drop for OwnedIcon {
    fn drop(&mut self) {
        // SAFETY: 핸들은 GetIcon이 만든 것이고 여기서 한 번만 푼다.
        let _ = unsafe { DestroyIcon(self.0) };
    }
}

/// 메모리 DC와 거기 고른 32비트 DIB. 버리면 풀어 준다.
struct Canvas {
    dc: HDC,
    bitmap: HGDIOBJ,
    previous: HGDIOBJ,
    bits: *mut u8,
    size: u32,
}

impl Canvas {
    fn new(size: u32) -> Option<Canvas> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size as i32,
                // 음수 높이: 위에서 아래로 놓인 행.
                biHeight: -(size as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: 화면과 호환되는 메모리 DC를 새로 만든다. 실패하면 null이다.
        let dc = unsafe { CreateCompatibleDC(None) };
        if dc.is_invalid() {
            return None;
        }
        let mut bits = std::ptr::null_mut();
        // SAFETY: info는 32비트 위에서 아래 DIB를 그리는 올바른 머리이고 bits는 그림 메모리 주소를 받는다.
        let bitmap = match unsafe {
            CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
        } {
            Ok(bitmap) => HGDIOBJ(bitmap.0),
            Err(_) => {
                // SAFETY: 위에서 만든 DC를 한 번만 지운다.
                let _ = unsafe { DeleteDC(dc) };
                return None;
            }
        };
        // SAFETY: dc와 bitmap은 위에서 만든 살아 있는 객체다. 앞서 골라져 있던 객체는 Drop에서 되돌린다.
        let previous = unsafe { SelectObject(dc, bitmap) };
        Some(Canvas {
            dc,
            bitmap,
            previous,
            bits: bits.cast(),
            size,
        })
    }

    /// 바탕을 칠하고 아이콘을 그린 뒤 BGRA 화소를 복사해 돌려준다.
    fn draw(&self, icon: HICON, background: HBRUSH) -> Option<Vec<u8>> {
        let side = self.size as i32;
        let rect = RECT {
            left: 0,
            top: 0,
            right: side,
            bottom: side,
        };
        // SAFETY: dc에는 크기 side의 DIB가 골라져 있고 붓은 시스템 붓이다.
        unsafe { FillRect(self.dc, &rect, background) };
        // SAFETY: icon은 살아 있는 아이콘 핸들이고 그리는 곳은 이 DC의 DIB 안이다.
        unsafe { DrawIconEx(self.dc, 0, 0, icon, side, side, 0, None, DI_NORMAL) }.ok()?;
        // SAFETY: GDI가 미뤄 둔 그리기를 DIB 메모리에 반영한다.
        let _ = unsafe { GdiFlush() };
        let len = (self.size * self.size * 4) as usize;
        // SAFETY: bits는 CreateDIBSection이 준 size*size 32비트 화소 메모리이고 Canvas가 살아 있는 동안 유효하다.
        Some(unsafe { std::slice::from_raw_parts(self.bits, len) }.to_vec())
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        // SAFETY: 새로 만들 때 골라 둔 앞 객체를 되돌리고, 만든 DIB와 DC를 한 번씩 지운다.
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap);
            let _ = DeleteDC(self.dc);
        }
    }
}

/// 아이콘을 `size`×`size` RGBA로 그린다.
fn render(icon: HICON, size: u32) -> Option<IconImage> {
    let canvas = Canvas::new(size)?;
    // SAFETY: 시스템 붓은 지우지 않아도 되는 공용 객체다.
    let black = HBRUSH(unsafe { GetStockObject(BLACK_BRUSH) }.0);
    // SAFETY: 위와 같다.
    let white = HBRUSH(unsafe { GetStockObject(WHITE_BRUSH) }.0);
    let on_black = canvas.draw(icon, black)?;
    let on_white = canvas.draw(icon, white)?;
    Some(IconImage {
        width: size,
        height: size,
        rgba: unblend(&on_black, &on_white),
    })
}

/// 검은 바탕·흰 바탕에 그린 BGRA 두 벌에서 곧은(premultiplied 아닌) RGBA를 되살린다.
///
/// 검은 바탕 값은 `a·c`, 흰 바탕 값은 `a·c + (1 - a)·255`이므로 `a = 255 - (흰 - 검)`, `c = 검 / a`다.
pub(crate) fn unblend(on_black: &[u8], on_white: &[u8]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(on_black.len());
    for (b, w) in on_black
        .as_chunks::<4>()
        .0
        .iter()
        .zip(on_white.as_chunks::<4>().0)
    {
        // 채널마다 구한 투명도 가운데 가장 큰 값(반올림 오차에 덜 흔들림).
        let alpha = (0..3)
            .map(|i| 255 - (i32::from(w[i]) - i32::from(b[i])).clamp(0, 255))
            .max()
            .unwrap_or(0);
        let channel = |i: usize| -> u8 {
            if alpha == 0 {
                0
            } else {
                ((i32::from(b[i]) * 255 + alpha / 2) / alpha).clamp(0, 255) as u8
            }
        };
        // BGRA → RGBA.
        rgba.extend_from_slice(&[channel(2), channel(1), channel(0), alpha as u8]);
    }
    rgba
}
