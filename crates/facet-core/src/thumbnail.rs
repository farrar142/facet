//! #326: 썸네일 만들기(자체 디코더: JPEG·PNG·GIF·WebP·BMP).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use crate::Item;

/// #326: 썸네일 그림(RGBA8, 행 우선, 길이 `width * height * 4`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// #326: 썸네일을 만들지 못한 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailError {
    Unsupported,
    NotFound,
    AccessDenied,
    TooLarge,
    Corrupt,
    Io(std::io::ErrorKind),
    Cancelled,
    Panicked,
}

/// #326: 디코더 화소 수 상한.
pub const DEFAULT_MAX_PIXELS: u64 = 100_000_000;
/// #326: 디코더에 넣을 파일 크기 상한(보통 사진은 넉넉히 들어가고, 통째로 읽으므로 메모리가 이 값까지 든다).
pub const DEFAULT_MAX_FILE_SIZE: u64 = 256 * 1024 * 1024;

/// #326: 디코더 상한.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeLimits {
    pub max_pixels: u64,
    pub max_file_size: u64,
}

impl Default for DecodeLimits {
    fn default() -> Self {
        DecodeLimits {
            max_pixels: DEFAULT_MAX_PIXELS,
            max_file_size: DEFAULT_MAX_FILE_SIZE,
        }
    }
}

/// #326: 썸네일 키(전체 경로, 한 변, 수정 시각, 파일 크기).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ThumbnailKey {
    pub path: PathBuf,
    pub side: u32,
    pub modified: SystemTime,
    pub file_size: u64,
}

impl ThumbnailKey {
    /// #326: 파일의 수정 시각·크기를 읽어 키를 만든다.
    pub fn for_file(path: &Path, side: u32) -> std::io::Result<ThumbnailKey> {
        let meta = std::fs::metadata(path)?;
        Ok(ThumbnailKey {
            path: path.to_path_buf(),
            side,
            modified: meta.modified()?,
            file_size: meta.len(),
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Jpeg,
    Png,
    Gif,
    WebP,
    Bmp,
}

fn kind_of(name: &OsStr) -> Option<Kind> {
    let ext = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "jpg" | "jpeg" => Kind::Jpeg,
        "png" => Kind::Png,
        "gif" => Kind::Gif,
        "webp" => Kind::WebP,
        "bmp" => Kind::Bmp,
        _ => return None,
    })
}

/// #326: 자체 디코더 형식인가(확장자, 대소문자 무시).
pub fn is_decodable(name: &OsStr) -> bool {
    kind_of(name).is_some()
}

/// #326: 기본 상한으로 썸네일을 만든다.
pub fn make_thumbnail(path: &Path, side: u32) -> Result<Thumbnail, ThumbnailError> {
    make_thumbnail_with(
        path,
        side,
        &DecodeLimits::default(),
        &AtomicBool::new(false),
    )
}

fn io_error(e: &std::io::Error) -> ThumbnailError {
    match e.kind() {
        std::io::ErrorKind::NotFound => ThumbnailError::NotFound,
        std::io::ErrorKind::PermissionDenied => ThumbnailError::AccessDenied,
        k => ThumbnailError::Io(k),
    }
}

/// #326: 상한과 취소 표지를 받아 썸네일을 만든다.
pub fn make_thumbnail_with(
    path: &Path,
    side: u32,
    limits: &DecodeLimits,
    cancel: &AtomicBool,
) -> Result<Thumbnail, ThumbnailError> {
    let kind = path
        .file_name()
        .and_then(kind_of)
        .ok_or(ThumbnailError::Unsupported)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(ThumbnailError::Cancelled);
    }
    let side = side.max(1);
    let meta = std::fs::metadata(path).map_err(|e| io_error(&e))?;
    if meta.len() > limits.max_file_size {
        return Err(ThumbnailError::TooLarge);
    }
    let bytes = std::fs::read(path).map_err(|e| io_error(&e))?;
    if cancel.load(Ordering::Relaxed) {
        return Err(ThumbnailError::Cancelled);
    }
    std::panic::catch_unwind(|| decode(kind, &bytes, side, limits, true))
        .unwrap_or(Err(ThumbnailError::Panicked))
}

fn check_pixels(width: u64, height: u64, limits: &DecodeLimits) -> Result<(), ThumbnailError> {
    if width == 0 || height == 0 {
        return Err(ThumbnailError::Corrupt);
    }
    if width * height > limits.max_pixels {
        return Err(ThumbnailError::TooLarge);
    }
    Ok(())
}

/// 긴 변이 `side`(작은 그림은 원래 크기)가 되는 크기.
fn target_size(width: u32, height: u32, side: u32) -> (u32, u32) {
    let long = u64::from(width.max(height));
    if long <= u64::from(side) {
        return (width, height);
    }
    let scale = |v: u32| ((u64::from(v) * u64::from(side) + long / 2) / long).max(1) as u32;
    if width >= height {
        (side, scale(height))
    } else {
        (scale(width), side)
    }
}

fn finish(img: image::RgbaImage, side: u32) -> Thumbnail {
    let (w, h) = target_size(img.width(), img.height(), side);
    let img = if (w, h) == (img.width(), img.height()) {
        img
    } else {
        image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle)
    };
    Thumbnail {
        width: img.width(),
        height: img.height(),
        rgba: img.into_raw(),
    }
}

fn decode(
    kind: Kind,
    bytes: &[u8],
    side: u32,
    limits: &DecodeLimits,
    use_embedded: bool,
) -> Result<Thumbnail, ThumbnailError> {
    if kind == Kind::Jpeg {
        return decode_jpeg(bytes, side, limits, use_embedded);
    }
    use image::ImageDecoder;
    let format = match kind {
        Kind::Png => image::ImageFormat::Png,
        Kind::Gif => image::ImageFormat::Gif,
        Kind::WebP => image::ImageFormat::WebP,
        _ => image::ImageFormat::Bmp,
    };
    if kind == Kind::WebP
        && let Some((w, h)) = webp_dimensions(bytes)
    {
        check_pixels(u64::from(w), u64::from(h), limits)?;
    }
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let mut lim = image::Limits::default();
    lim.max_alloc = Some(limits.max_pixels.saturating_mul(8).max(1 << 20));
    reader.limits(lim);
    let decoder = reader.into_decoder().map_err(|_| ThumbnailError::Corrupt)?;
    let (w, h) = decoder.dimensions();
    check_pixels(u64::from(w), u64::from(h), limits)?;
    let img = image::DynamicImage::from_decoder(decoder).map_err(|_| ThumbnailError::Corrupt)?;
    Ok(finish(img.into_rgba8(), side))
}

/// WebP 머리(VP8X·VP8L·VP8)에서 화소 크기만 읽는다(디코더가 본 그림을 읽기 전에 상한을 견주려고).
fn webp_dimensions(b: &[u8]) -> Option<(u32, u32)> {
    if b.get(0..4)? != b"RIFF" || b.get(8..12)? != b"WEBP" {
        return None;
    }
    let le24 = |o: usize| -> Option<u32> {
        let v = b.get(o..o + 3)?;
        Some(u32::from(v[0]) | u32::from(v[1]) << 8 | u32::from(v[2]) << 16)
    };
    match b.get(12..16)? {
        b"VP8X" => Some((le24(24)? + 1, le24(27)? + 1)),
        b"VP8L" => {
            if *b.get(20)? != 0x2F {
                return None;
            }
            let v = u32::from_le_bytes(b.get(21..25)?.try_into().ok()?);
            Some(((v & 0x3FFF) + 1, ((v >> 14) & 0x3FFF) + 1))
        }
        b"VP8 " => {
            let w = u16::from_le_bytes(b.get(26..28)?.try_into().ok()?) & 0x3FFF;
            let h = u16::from_le_bytes(b.get(28..30)?.try_into().ok()?) & 0x3FFF;
            Some((u32::from(w), u32::from(h)))
        }
        _ => None,
    }
}

fn decode_jpeg(
    bytes: &[u8],
    side: u32,
    limits: &DecodeLimits,
    use_embedded: bool,
) -> Result<Thumbnail, ThumbnailError> {
    let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(bytes));
    decoder.read_info().map_err(|_| ThumbnailError::Corrupt)?;
    let info = decoder.info().ok_or(ThumbnailError::Corrupt)?;
    check_pixels(u64::from(info.width), u64::from(info.height), limits)?;
    let exif = decoder.exif_data().map(parse_exif).unwrap_or_default();
    let orientation = exif.orientation.unwrap_or(1);
    if use_embedded
        && let Some(thumb) = exif
            .thumbnail
            .and_then(|(at, len)| decoder.exif_data()?.get(at..at.checked_add(len)?))
        && jpeg_long_side(thumb).is_some_and(|l| l >= side)
        && let Ok(t) = decode_jpeg(thumb, side, limits, false)
    {
        return Ok(orient(t, orientation));
    }
    let (tw, th) = target_size(u32::from(info.width), u32::from(info.height), side);
    decoder
        .scale(
            tw.min(u32::from(u16::MAX)) as u16,
            th.min(u32::from(u16::MAX)) as u16,
        )
        .map_err(|_| ThumbnailError::Corrupt)?;
    let pixels = decoder.decode().map_err(|_| ThumbnailError::Corrupt)?;
    let info = decoder.info().ok_or(ThumbnailError::Corrupt)?;
    let (w, h) = (u32::from(info.width), u32::from(info.height));
    let n = (w * h) as usize;
    let mut rgba = Vec::with_capacity(n * 4);
    match info.pixel_format {
        jpeg_decoder::PixelFormat::L8 => {
            for &v in pixels.iter().take(n) {
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        jpeg_decoder::PixelFormat::L16 => {
            for c in pixels.as_chunks::<2>().0.iter().take(n) {
                rgba.extend_from_slice(&[c[0], c[0], c[0], 255]);
            }
        }
        jpeg_decoder::PixelFormat::RGB24 => {
            for c in pixels.as_chunks::<3>().0.iter().take(n) {
                rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        jpeg_decoder::PixelFormat::CMYK32 => {
            for c in pixels.as_chunks::<4>().0.iter().take(n) {
                let k = u32::from(c[3]);
                let f = |v: u8| (u32::from(v) * k / 255) as u8;
                rgba.extend_from_slice(&[f(c[0]), f(c[1]), f(c[2]), 255]);
            }
        }
    }
    if rgba.len() != n * 4 {
        return Err(ThumbnailError::Corrupt);
    }
    let img = image::RgbaImage::from_raw(w, h, rgba).ok_or(ThumbnailError::Corrupt)?;
    Ok(orient(finish(img, side), orientation))
}

fn jpeg_long_side(bytes: &[u8]) -> Option<u32> {
    let mut d = jpeg_decoder::Decoder::new(std::io::Cursor::new(bytes));
    d.read_info().ok()?;
    let i = d.info()?;
    Some(u32::from(i.width.max(i.height)))
}

/// EXIF 회전 값(1~8)대로 그림을 바로 세운다.
fn orient(t: Thumbnail, orientation: u16) -> Thumbnail {
    use image::imageops as o;
    if !(2..=8).contains(&orientation) {
        return t;
    }
    let Some(img) = image::RgbaImage::from_raw(t.width, t.height, t.rgba) else {
        return Thumbnail {
            width: 0,
            height: 0,
            rgba: Vec::new(),
        };
    };
    let img = match orientation {
        2 => o::flip_horizontal(&img),
        3 => o::rotate180(&img),
        4 => o::flip_vertical(&img),
        5 => o::flip_horizontal(&o::rotate90(&img)),
        6 => o::rotate90(&img),
        7 => o::flip_horizontal(&o::rotate270(&img)),
        _ => o::rotate270(&img),
    };
    Thumbnail {
        width: img.width(),
        height: img.height(),
        rgba: img.into_raw(),
    }
}

#[derive(Default)]
struct Exif {
    orientation: Option<u16>,
    /// 내장 썸네일의 (EXIF 안 시작, 길이).
    thumbnail: Option<(usize, usize)>,
}

/// TIFF 머리부터 시작하는 EXIF 바이트에서 회전과 내장 썸네일 자리를 읽는다(범위를 넘으면 없는 것으로 본다).
fn parse_exif(t: &[u8]) -> Exif {
    let mut out = Exif::default();
    let le = match t.get(0..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return out,
    };
    let u16_at = |o: usize| -> Option<u16> {
        let b: [u8; 2] = t.get(o..o.checked_add(2)?)?.try_into().ok()?;
        Some(if le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let u32_at = |o: usize| -> Option<u32> {
        let b: [u8; 4] = t.get(o..o.checked_add(4)?)?.try_into().ok()?;
        Some(if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    type Ifd = (Vec<(u16, u16, usize)>, Option<usize>);
    let ifd = |start: usize| -> Option<Ifd> {
        let n = usize::from(u16_at(start)?);
        let mut entries = Vec::new();
        for i in 0..n {
            let e = start + 2 + i * 12;
            entries.push((u16_at(e)?, u16_at(e + 2)?, e + 8));
        }
        let next = u32_at(start + 2 + n * 12)
            .map(|v| v as usize)
            .filter(|&v| v != 0);
        Some((entries, next))
    };
    let Some(first) = u32_at(4) else { return out };
    let Some((entries, next)) = ifd(first as usize) else {
        return out;
    };
    for (tag, kind, at) in entries {
        if tag == 0x0112 && kind == 3 {
            out.orientation = u16_at(at).filter(|v| (1..=8).contains(v));
        }
    }
    if let Some((entries, _)) = next.and_then(ifd) {
        let (mut off, mut len) = (None, None);
        for (tag, _, at) in entries {
            match tag {
                0x0201 => off = u32_at(at),
                0x0202 => len = u32_at(at),
                _ => {}
            }
        }
        if let (Some(o), Some(l)) = (off, len) {
            out.thumbnail = Some((o as usize, l as usize));
        }
    }
    out
}

/// #330: 항목의 썸네일 계획.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailPlan {
    Decode,
    CacheOnly,
    IconOnly,
}

/// #330: 항목마다 계획을 정한다.
pub fn plan(item: &Item) -> ThumbnailPlan {
    if item.is_folder || !is_decodable(&item.name) {
        ThumbnailPlan::IconOnly
    } else if crate::content_gate::check_item(
        &item.attributes,
        crate::content_gate::Reader::Thumbnail,
    )
    .is_err()
    {
        // 내용을 읽으면 내려받기가 일어나므로 캐시에 있는 것만 쓴다.
        ThumbnailPlan::CacheOnly
    } else {
        ThumbnailPlan::Decode
    }
}
