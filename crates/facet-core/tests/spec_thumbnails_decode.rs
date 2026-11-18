// target: crates/facet-core/tests/spec_thumbnails_decode.rs
// AC: #326: 다섯 형식의 크기·위아래·색(BMP 아래→위 행 포함), JPEG EXIF 회전 여덟 가지, EXIF 내장 썸네일 쓰기·안 쓰기
// AC: #326: PNG·WebP 투명은 알파로, GIF·WebP 애니메이션은 첫 장면, 화소 수·파일 크기 상한은 TooLarge(바꿔 넘길 수 있음)
// AC: #326: 잘린·가짜·빈 파일은 패닉 없이 실패, 없는 파일 NotFound, 이미지가 아닌 확장자 Unsupported, 미리 취소 Cancelled
// AC: #326: 자체 디코더 형식은 확장자로 가림(대소문자 무시), 썸네일 키는 파일의 수정 시각·크기를 읽어 만듦
//! 코어의 썸네일 만들기(`thumbnail::make_thumbnail`·`make_thumbnail_with`)를 임시 폴더의 시험 그림으로 확인한다. 시험
//! 그림은 저장소에 넣지 않고 이 파일 안의 작은 인코더(압축하지 않는 PNG·LZW 지움 코드만 쓰는 GIF·단순 접두 부호 VP8L
//! WebP·24비트 BMP·8×8 덩어리마다 한 색인 기준선 JPEG와 EXIF)가 만든다. 화소 하나하나가 아니라 크기와 자리마다의 대략의
//! 색(가장 센 채널)·알파를 본다. 순수 Rust 디코더라 모든 플랫폼에서 돈다.

use std::ffi::OsStr;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use facet_core::thumbnail::{
    DEFAULT_MAX_FILE_SIZE, DEFAULT_MAX_PIXELS, DecodeLimits, Thumbnail, ThumbnailError,
    ThumbnailKey, is_decodable, make_thumbnail, make_thumbnail_with,
};
use tempfile::TempDir;

type Rgb = [u8; 3];
type Rgba = [u8; 4];

const RED: Rgb = [200, 40, 40];
const GREEN: Rgb = [40, 180, 40];
const BLUE: Rgb = [40, 40, 200];

fn opaque([r, g, b]: Rgb) -> Rgba {
    [r, g, b, 255]
}

fn write(dir: &TempDir, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.path().join(name);
    fs::write(&path, bytes).unwrap();
    path
}

fn decode(dir: &TempDir, name: &str, bytes: &[u8], side: u32) -> Thumbnail {
    let path = write(dir, name, bytes);
    make_thumbnail(&path, side).unwrap_or_else(|e| panic!("{name}: {e:?}"))
}

fn fails(dir: &TempDir, name: &str, bytes: &[u8]) -> ThumbnailError {
    let path = write(dir, name, bytes);
    match make_thumbnail(&path, 96) {
        Ok(t) => panic!(
            "{name}: 실패해야 하는데 {}×{} 그림이 나왔다",
            t.width, t.height
        ),
        Err(e) => e,
    }
}

// ---- 그림 살피기

/// 긴 변은 정확히, 짧은 변은 ±1화소. RGBA 길이도 맞아야 한다.
fn assert_size(t: &Thumbnail, width: u32, height: u32) {
    assert_eq!(
        t.rgba.len(),
        (t.width * t.height * 4) as usize,
        "RGBA 길이가 크기와 맞지 않는다"
    );
    let ok = if width >= height {
        t.width == width && t.height.abs_diff(height) <= 1
    } else {
        t.height == height && t.width.abs_diff(width) <= 1
    };
    assert!(ok, "크기 {}×{}, 기대 {width}×{height}", t.width, t.height);
}

/// (x, y) 둘레 3×3 화소의 평균.
fn around(t: &Thumbnail, x: u32, y: u32) -> Rgba {
    let mut sum = [0u32; 4];
    let mut n = 0;
    for yy in y.saturating_sub(1)..=(y + 1).min(t.height - 1) {
        for xx in x.saturating_sub(1)..=(x + 1).min(t.width - 1) {
            let i = ((yy * t.width + xx) * 4) as usize;
            for (c, s) in sum.iter_mut().enumerate() {
                *s += u32::from(t.rgba[i + c]);
            }
            n += 1;
        }
    }
    sum.map(|s| (s / n) as u8)
}

/// 불투명한 화소 전체의 평균.
fn average(t: &Thumbnail) -> Rgba {
    let mut sum = [0u64; 4];
    let mut n = 0u64;
    for p in t.rgba.as_chunks::<4>().0.iter().filter(|p| p[3] > 0) {
        for (c, s) in sum.iter_mut().enumerate() {
            *s += u64::from(p[c]);
        }
        n += 1;
    }
    assert!(n > 0, "보이는 화소가 없다");
    sum.map(|s| (s / n) as u8)
}

/// 가장 센 채널: 다른 두 채널보다 60 넘게 크면 그 채널.
fn dominant(c: Rgba) -> Option<char> {
    let [r, g, b, _] = c.map(i32::from);
    if r > g + 60 && r > b + 60 {
        Some('r')
    } else if g > r + 60 && g > b + 60 {
        Some('g')
    } else if b > r + 60 && b > g + 60 {
        Some('b')
    } else {
        None
    }
}

// ---- PNG(압축하지 않는 deflate)

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in bytes {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let chunks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        out.push(u8::from(i + 1 == chunks.len()));
        let len = chunk.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// PNG 서명과 IHDR(8비트 RGBA).
fn png_header(width: u32, height: u32) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    png_chunk(&mut out, b"IHDR", &ihdr);
    out
}

fn png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> Rgba) -> Vec<u8> {
    let mut raw = Vec::new();
    for y in 0..height {
        raw.push(0);
        for x in 0..width {
            raw.extend_from_slice(&pixel(x, y));
        }
    }
    let mut out = png_header(width, height);
    png_chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    png_chunk(&mut out, b"IEND", &[]);
    out
}

// ---- BMP(24비트, 아래 행부터)

fn bmp_header(width: u32, height: u32, data_len: u32) -> Vec<u8> {
    let mut out = b"BM".to_vec();
    out.extend_from_slice(&(54 + data_len).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&(height as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&[0; 8]);
    out
}

fn bmp(width: u32, height: u32, pixel: impl Fn(u32, u32) -> Rgb) -> Vec<u8> {
    let row = (width * 3).div_ceil(4) * 4;
    let mut out = bmp_header(width, height, row * height);
    for y in (0..height).rev() {
        let start = out.len();
        for x in 0..width {
            let [r, g, b] = pixel(x, y);
            out.extend_from_slice(&[b, g, r]);
        }
        out.resize(start + row as usize, 0);
    }
    out
}

// ---- GIF(코드 폭을 8비트로 묶어 두려고 100화소마다 지움 코드를 넣는 LZW)

/// 색표 앞자리: 0 초록, 1 빨강, 2 파랑(나머지는 검정, 128색).
const GIF_PALETTE: [Rgb; 3] = [GREEN, RED, BLUE];

fn gif_screen(out: &mut Vec<u8>, width: u16, height: u16) {
    out.extend_from_slice(b"GIF89a");
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&[0xF6, 0, 0]);
    for i in 0..128 {
        out.extend_from_slice(&GIF_PALETTE.get(i).copied().unwrap_or([0, 0, 0]));
    }
}

fn gif_image(out: &mut Vec<u8>, width: u16, height: u16, codes: &[u8]) {
    out.push(0x2C);
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.push(0);
    out.push(7);
    for block in codes.chunks(255) {
        out.push(block.len() as u8);
        out.extend_from_slice(block);
    }
    out.push(0);
}

/// 장면마다 한 가지 색(색표 번호). 장면이 둘 이상이면 애니메이션이다.
fn gif(width: u16, height: u16, frames: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    gif_screen(&mut out, width, height);
    if frames.len() > 1 {
        out.extend_from_slice(b"\x21\xFF\x0BNETSCAPE2.0\x03\x01\x00\x00\x00");
    }
    for &index in frames {
        out.extend_from_slice(&[0x21, 0xF9, 0x04, 0x04, 10, 0, 0, 0]);
        let mut codes = Vec::new();
        let mut left = usize::from(width) * usize::from(height);
        while left > 0 {
            let n = left.min(100);
            codes.push(128);
            codes.extend(std::iter::repeat_n(index, n));
            left -= n;
        }
        codes.push(129);
        gif_image(&mut out, width, height, &codes);
    }
    out.push(0x3B);
    out
}

// ---- WebP(무손실 VP8L, 채널마다 값이 두 가지 이하인 그림만)

#[derive(Default)]
struct LsbBits {
    out: Vec<u8>,
    acc: u64,
    used: u32,
}

impl LsbBits {
    fn put(&mut self, value: u32, bits: u32) {
        self.acc |= u64::from(value) << self.used;
        self.used += bits;
        while self.used >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.used -= 8;
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.used > 0 {
            self.out.push(self.acc as u8);
        }
        self.out
    }
}

fn vp8l_header(bits: &mut LsbBits, width: u32, height: u32, alpha: bool) {
    bits.put(0x2F, 8);
    bits.put(width - 1, 14);
    bits.put(height - 1, 14);
    bits.put(u32::from(alpha), 1);
    bits.put(0, 3);
}

/// 단순 접두 부호: 기호 하나(0비트)나 둘(1비트, 작은 기호가 0).
fn simple_code(bits: &mut LsbBits, symbols: &[u8]) {
    bits.put(1, 1);
    bits.put(symbols.len() as u32 - 1, 1);
    bits.put(1, 1);
    for &s in symbols {
        bits.put(u32::from(s), 8);
    }
}

fn vp8l(width: u32, height: u32, pixels: &[Rgba]) -> Vec<u8> {
    let mut bits = LsbBits::default();
    vp8l_header(&mut bits, width, height, pixels.iter().any(|p| p[3] != 255));
    bits.put(0, 1); // 변환 없음
    bits.put(0, 1); // 색 캐시 없음
    bits.put(0, 1); // 메타 접두 부호 없음
    // 접두 부호 차례는 초록·빨강·파랑·알파·거리, RGBA 안의 자리로는 1·0·2·3.
    let order = [1usize, 0, 2, 3];
    let symbols: Vec<Vec<u8>> = order
        .iter()
        .map(|&c| {
            let mut values: Vec<u8> = pixels.iter().map(|p| p[c]).collect();
            values.sort_unstable();
            values.dedup();
            assert!(values.len() <= 2, "채널마다 값은 두 가지까지");
            values
        })
        .collect();
    for s in &symbols {
        simple_code(&mut bits, s);
    }
    simple_code(&mut bits, &[0]);
    for p in pixels {
        for (k, &c) in order.iter().enumerate() {
            if symbols[k].len() == 2 {
                bits.put(u32::from(p[c] == symbols[k][1]), 1);
            }
        }
    }
    bits.finish()
}

fn riff_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(kind);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    if !data.len().is_multiple_of(2) {
        out.push(0);
    }
}

fn riff_webp(chunks: &[u8]) -> Vec<u8> {
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&((chunks.len() + 4) as u32).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(chunks);
    out
}

fn pixels(width: u32, height: u32, pixel: impl Fn(u32, u32) -> Rgba) -> Vec<Rgba> {
    let mut out = Vec::new();
    for y in 0..height {
        for x in 0..width {
            out.push(pixel(x, y));
        }
    }
    out
}

fn webp(width: u32, height: u32, pixel: impl Fn(u32, u32) -> Rgba) -> Vec<u8> {
    let mut chunks = Vec::new();
    riff_chunk(
        &mut chunks,
        b"VP8L",
        &vp8l(width, height, &pixels(width, height, pixel)),
    );
    riff_webp(&chunks)
}

fn u24(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes()[..3]);
}

/// 장면마다 한 가지 색인 애니메이션 WebP(VP8X + ANIM + ANMF).
fn animated_webp(width: u32, height: u32, frames: &[Rgb]) -> Vec<u8> {
    let mut chunks = Vec::new();
    let mut vp8x = vec![0x02, 0, 0, 0];
    u24(&mut vp8x, width - 1);
    u24(&mut vp8x, height - 1);
    riff_chunk(&mut chunks, b"VP8X", &vp8x);
    riff_chunk(&mut chunks, b"ANIM", &[255, 255, 255, 255, 0, 0]);
    for &color in frames {
        let mut anmf = Vec::new();
        u24(&mut anmf, 0);
        u24(&mut anmf, 0);
        u24(&mut anmf, width - 1);
        u24(&mut anmf, height - 1);
        u24(&mut anmf, 100);
        anmf.push(0x02);
        let frame = vec![opaque(color); (width * height) as usize];
        riff_chunk(&mut anmf, b"VP8L", &vp8l(width, height, &frame));
        riff_chunk(&mut chunks, b"ANMF", &anmf);
    }
    riff_webp(&chunks)
}

// ---- JPEG(기준선, 8×8 덩어리마다 한 색: DC 계수만 쓰고 AC는 모두 0)

#[derive(Default)]
struct MsbBits {
    out: Vec<u8>,
    acc: u32,
    used: u32,
}

impl MsbBits {
    fn put(&mut self, value: u32, bits: u32) {
        for i in (0..bits).rev() {
            self.acc = (self.acc << 1) | ((value >> i) & 1);
            self.used += 1;
            if self.used == 8 {
                let byte = self.acc as u8;
                self.out.push(byte);
                if byte == 0xFF {
                    self.out.push(0);
                }
                self.acc = 0;
                self.used = 0;
            }
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.used > 0 {
            let pad = 8 - self.used;
            self.put((1 << pad) - 1, pad);
        }
        self.out
    }
}

fn segment(out: &mut Vec<u8>, marker: u8, data: &[u8]) {
    out.extend_from_slice(&[0xFF, marker]);
    out.extend_from_slice(&((data.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(data);
}

/// SOI부터 SOS까지. 양자화 표는 모두 1, 허프만 표는 이 시험의 작은 표(DC 범주 0..=11은 범주 번호 그대로 4비트, AC는
/// 블록 끝 하나가 1비트 「0」).
fn jpeg_header(width: u32, height: u32, exif: Option<&[u8]>) -> Vec<u8> {
    let mut out = vec![0xFF, 0xD8];
    segment(&mut out, 0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
    if let Some(exif) = exif {
        segment(&mut out, 0xE1, exif);
    }
    let mut dqt = vec![0u8];
    dqt.extend_from_slice(&[1; 64]);
    segment(&mut out, 0xDB, &dqt);
    let mut sof = vec![8];
    sof.extend_from_slice(&(height as u16).to_be_bytes());
    sof.extend_from_slice(&(width as u16).to_be_bytes());
    sof.push(3);
    for id in 1..=3u8 {
        sof.extend_from_slice(&[id, 0x11, 0]);
    }
    segment(&mut out, 0xC0, &sof);
    let mut dht = vec![0x00];
    let mut counts = [0u8; 16];
    counts[3] = 12;
    dht.extend_from_slice(&counts);
    dht.extend(0..12u8);
    dht.push(0x10);
    let mut counts = [0u8; 16];
    counts[0] = 1;
    dht.extend_from_slice(&counts);
    dht.push(0x00);
    segment(&mut out, 0xC4, &dht);
    segment(&mut out, 0xDA, &[3, 1, 0x00, 2, 0x00, 3, 0x00, 0, 63, 0]);
    out
}

fn category(value: i32) -> u32 {
    32 - value.unsigned_abs().leading_zeros()
}

fn magnitude(value: i32, size: u32) -> u32 {
    if value >= 0 {
        value as u32
    } else {
        ((value - 1) as u32) & ((1 << size) - 1)
    }
}

/// `block(bx, by)`가 8×8 덩어리마다의 색을 준다.
fn jpeg(width: u32, height: u32, exif: Option<&[u8]>, block: impl Fn(u32, u32) -> Rgb) -> Vec<u8> {
    assert!(width.is_multiple_of(8) && height.is_multiple_of(8));
    let mut out = jpeg_header(width, height, exif);
    let mut bits = MsbBits::default();
    let mut previous = [0i32; 3];
    for by in 0..height / 8 {
        for bx in 0..width / 8 {
            let [r, g, b] = block(bx, by).map(f64::from);
            let ycc = [
                0.299 * r + 0.587 * g + 0.114 * b,
                128.0 - 0.168_736 * r - 0.331_264 * g + 0.5 * b,
                128.0 + 0.5 * r - 0.418_688 * g - 0.081_312 * b,
            ];
            for (c, value) in ycc.into_iter().enumerate() {
                let dc = (8.0 * (value - 128.0)).round() as i32;
                let diff = dc - previous[c];
                previous[c] = dc;
                let size = category(diff);
                bits.put(size, 4);
                if size > 0 {
                    bits.put(magnitude(diff, size), size);
                }
                bits.put(0, 1);
            }
        }
    }
    out.extend(bits.finish());
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}

fn ifd_entry(out: &mut Vec<u8>, tag: u16, kind: u16, value: u32) {
    out.extend_from_slice(&tag.to_le_bytes());
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    if kind == 3 {
        out.extend_from_slice(&(value as u16).to_le_bytes());
        out.extend_from_slice(&[0, 0]);
    } else {
        out.extend_from_slice(&value.to_le_bytes());
    }
}

/// APP1 EXIF 본문: IFD0에 회전, 내장 썸네일이 있으면 IFD1에 그 JPEG.
fn exif(orientation: u16, thumbnail: Option<&[u8]>) -> Vec<u8> {
    let mut tiff = b"II\x2A\x00\x08\x00\x00\x00".to_vec();
    tiff.extend_from_slice(&1u16.to_le_bytes());
    ifd_entry(&mut tiff, 0x0112, 3, u32::from(orientation));
    let ifd1: u32 = 8 + 2 + 12 + 4;
    let next = if thumbnail.is_some() { ifd1 } else { 0 };
    tiff.extend_from_slice(&next.to_le_bytes());
    if let Some(thumb) = thumbnail {
        let data = ifd1 + 2 + 3 * 12 + 4;
        tiff.extend_from_slice(&3u16.to_le_bytes());
        ifd_entry(&mut tiff, 0x0103, 3, 6);
        ifd_entry(&mut tiff, 0x0201, 4, data);
        ifd_entry(&mut tiff, 0x0202, 4, thumb.len() as u32);
        tiff.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(tiff.len(), data as usize);
        tiff.extend_from_slice(thumb);
    }
    let mut out = b"Exif\0\0".to_vec();
    out.extend(tiff);
    out
}

// ---- 크기·위아래·색

#[test]
#[ignore = "#326"]
fn png는_긴_변을_요청_크기로_줄이고_위아래_색이_맞다() {
    let dir = TempDir::new().unwrap();
    let bytes = png(300, 200, |_, y| opaque(if y < 100 { RED } else { BLUE }));
    let t = decode(&dir, "그림.png", &bytes, 100);
    assert_size(&t, 100, 67);
    assert_eq!(dominant(around(&t, 50, 15)), Some('r'), "위쪽은 빨강");
    assert_eq!(dominant(around(&t, 50, 52)), Some('b'), "아래쪽은 파랑");
}

#[test]
#[ignore = "#326"]
fn 요청보다_작은_그림은_키우지_않는다() {
    let dir = TempDir::new().unwrap();
    let t = decode(&dir, "작은.png", &png(40, 30, |_, _| opaque(GREEN)), 256);
    assert_size(&t, 40, 30);
    assert_eq!(dominant(average(&t)), Some('g'));
}

#[test]
#[ignore = "#326"]
fn bmp는_아래_행부터_저장해도_위아래가_맞고_줄일_수_있다() {
    let dir = TempDir::new().unwrap();
    let bytes = bmp(48, 32, |_, y| if y < 16 { RED } else { BLUE });
    let t = decode(&dir, "그림.bmp", &bytes, 256);
    assert_size(&t, 48, 32);
    assert_eq!(dominant(around(&t, 24, 6)), Some('r'), "위쪽은 빨강");
    assert_eq!(dominant(around(&t, 24, 26)), Some('b'), "아래쪽은 파랑");

    let small = decode(&dir, "그림2.bmp", &bytes, 24);
    assert_size(&small, 24, 16);
}

#[test]
#[ignore = "#326"]
fn gif는_색과_크기가_맞다() {
    let dir = TempDir::new().unwrap();
    let t = decode(&dir, "파랑.gif", &gif(40, 30, &[2]), 256);
    assert_size(&t, 40, 30);
    assert_eq!(dominant(average(&t)), Some('b'));

    let small = decode(&dir, "파랑2.gif", &gif(40, 30, &[2]), 20);
    assert_size(&small, 20, 15);
}

#[test]
#[ignore = "#326"]
fn webp는_위아래_색과_크기가_맞다() {
    let dir = TempDir::new().unwrap();
    let bytes = webp(40, 30, |_, y| opaque(if y < 15 { RED } else { BLUE }));
    let t = decode(&dir, "그림.webp", &bytes, 256);
    assert_size(&t, 40, 30);
    assert_eq!(dominant(around(&t, 20, 4)), Some('r'), "위쪽은 빨강");
    assert_eq!(dominant(around(&t, 20, 25)), Some('b'), "아래쪽은 파랑");

    let small = decode(&dir, "그림2.webp", &bytes, 20);
    assert_size(&small, 20, 15);
}

#[test]
#[ignore = "#326"]
fn jpeg는_긴_변을_요청_크기로_줄이고_색이_맞다() {
    let dir = TempDir::new().unwrap();
    let bytes = jpeg(640, 480, None, |_, by| if by < 30 { RED } else { BLUE });
    let t = decode(&dir, "사진.jpg", &bytes, 96);
    assert_size(&t, 96, 72);
    assert_eq!(dominant(around(&t, 48, 10)), Some('r'), "위쪽은 빨강");
    assert_eq!(dominant(around(&t, 48, 62)), Some('b'), "아래쪽은 파랑");

    let full = decode(&dir, "사진2.jpeg", &bytes, 1024);
    assert_size(&full, 640, 480);
}

// ---- EXIF

/// 저장한 그림의 왼쪽 위 사분면(가로 64 × 세로 32 중 32×16)만 빨강이다. 회전 값대로 바로 세우면 빨강이 가는 자리
/// (열, 행: 0은 왼쪽·위, 1은 오른쪽·아래).
const ORIENTATIONS: [(u16, (u32, u32)); 8] = [
    (1, (0, 0)),
    (2, (1, 0)),
    (3, (1, 1)),
    (4, (0, 1)),
    (5, (0, 0)),
    (6, (1, 0)),
    (7, (1, 1)),
    (8, (0, 1)),
];

#[test]
#[ignore = "#326"]
fn jpeg_exif_회전_여덟_가지가_모두_바로_선다() {
    let dir = TempDir::new().unwrap();
    for (orientation, red_at) in ORIENTATIONS {
        let bytes = jpeg(64, 32, Some(&exif(orientation, None)), |bx, by| {
            if bx < 4 && by < 2 { RED } else { BLUE }
        });
        let t = decode(&dir, &format!("회전{orientation}.jpg"), &bytes, 256);
        if orientation <= 4 {
            assert_size(&t, 64, 32);
        } else {
            assert_size(&t, 32, 64);
        }
        for column in 0..2 {
            for row in 0..2 {
                let x = t.width / 4 + column * t.width / 2;
                let y = t.height / 4 + row * t.height / 2;
                let expected = if (column, row) == red_at { 'r' } else { 'b' };
                assert_eq!(
                    dominant(around(&t, x, y)),
                    Some(expected),
                    "회전 {orientation}: 사분면 ({column}, {row})"
                );
            }
        }
    }
}

/// 본 그림은 빨강 640×480, 내장 썸네일은 초록 160×120.
fn photo_with_embedded_thumbnail() -> Vec<u8> {
    let thumb = jpeg(160, 120, None, |_, _| GREEN);
    jpeg(640, 480, Some(&exif(1, Some(&thumb))), |_, _| RED)
}

#[test]
#[ignore = "#326"]
fn 내장_썸네일이_요청_크기_이상이면_그것을_쓴다() {
    let dir = TempDir::new().unwrap();
    let bytes = photo_with_embedded_thumbnail();
    let t = decode(&dir, "내장.jpg", &bytes, 96);
    assert_size(&t, 96, 72);
    assert_eq!(dominant(average(&t)), Some('g'), "내장 썸네일(초록)을 썼다");

    let same = decode(&dir, "내장2.jpg", &bytes, 160);
    assert_size(&same, 160, 120);
    assert_eq!(
        dominant(average(&same)),
        Some('g'),
        "같은 크기도 내장 썸네일"
    );
}

#[test]
#[ignore = "#326"]
fn 내장_썸네일이_요청보다_작거나_없으면_본_그림을_쓴다() {
    let dir = TempDir::new().unwrap();
    let t = decode(&dir, "내장.jpg", &photo_with_embedded_thumbnail(), 400);
    assert_size(&t, 400, 300);
    assert_eq!(dominant(average(&t)), Some('r'), "본 그림(빨강)을 읽었다");

    let plain = jpeg(640, 480, Some(&exif(1, None)), |_, _| RED);
    let t = decode(&dir, "내장없음.jpg", &plain, 96);
    assert_size(&t, 96, 72);
    assert_eq!(dominant(average(&t)), Some('r'));
}

// ---- 투명·애니메이션

fn half_transparent(x: u32, _y: u32) -> Rgba {
    if x < 16 { opaque(RED) } else { [0, 0, 0, 0] }
}

fn assert_half_transparent(t: &Thumbnail) {
    assert_size(t, 32, 32);
    let left = around(t, 6, 16);
    assert!(left[3] >= 240, "왼쪽은 불투명: {left:?}");
    assert_eq!(dominant(left), Some('r'));
    let right = around(t, 26, 16);
    assert!(right[3] <= 15, "오른쪽은 투명: {right:?}");
}

#[test]
#[ignore = "#326"]
fn png의_투명_부분은_알파로_남는다() {
    let dir = TempDir::new().unwrap();
    let t = decode(&dir, "투명.png", &png(32, 32, half_transparent), 256);
    assert_half_transparent(&t);
}

#[test]
#[ignore = "#326"]
fn webp의_투명_부분은_알파로_남는다() {
    let dir = TempDir::new().unwrap();
    let t = decode(&dir, "투명.webp", &webp(32, 32, half_transparent), 256);
    assert_half_transparent(&t);
}

#[test]
#[ignore = "#326"]
fn 움직이는_gif는_첫_장면이다() {
    let dir = TempDir::new().unwrap();
    let t = decode(&dir, "움직임.gif", &gif(40, 30, &[0, 1]), 256);
    assert_size(&t, 40, 30);
    assert_eq!(dominant(average(&t)), Some('g'), "첫 장면은 초록");
}

#[test]
#[ignore = "#326"]
fn 움직이는_webp는_첫_장면이다() {
    let dir = TempDir::new().unwrap();
    let t = decode(
        &dir,
        "움직임.webp",
        &animated_webp(40, 30, &[GREEN, RED]),
        256,
    );
    assert_size(&t, 40, 30);
    assert_eq!(dominant(average(&t)), Some('g'), "첫 장면은 초록");
}

// ---- 상한

fn png_bomb() -> Vec<u8> {
    let mut out = png_header(20_000, 20_000);
    png_chunk(&mut out, b"IDAT", &zlib_stored(&[0; 100]));
    png_chunk(&mut out, b"IEND", &[]);
    out
}

fn jpeg_bomb() -> Vec<u8> {
    let mut out = jpeg_header(20_000, 20_000, None);
    out.extend_from_slice(&[0; 16]);
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}

fn gif_bomb() -> Vec<u8> {
    let mut out = Vec::new();
    gif_screen(&mut out, u16::MAX, u16::MAX);
    gif_image(&mut out, u16::MAX, u16::MAX, &[128, 0, 129]);
    out.push(0x3B);
    out
}

fn webp_bomb() -> Vec<u8> {
    let mut bits = LsbBits::default();
    vp8l_header(&mut bits, 16_384, 16_384, false);
    bits.put(0, 3);
    let mut payload = bits.finish();
    payload.extend_from_slice(&[0; 32]);
    let mut chunks = Vec::new();
    riff_chunk(&mut chunks, b"VP8L", &payload);
    riff_webp(&chunks)
}

fn bmp_bomb() -> Vec<u8> {
    let mut out = bmp_header(20_000, 20_000, 60_000 * 20_000);
    out.extend_from_slice(&[0; 64]);
    out
}

#[test]
#[ignore = "#326"]
fn 화소_수가_상한을_넘는_머리는_디코딩하지_않고_too_large다() {
    assert_eq!(DEFAULT_MAX_PIXELS, 100_000_000);
    let dir = TempDir::new().unwrap();
    for (name, bytes) in [
        ("폭탄.png", png_bomb()),
        ("폭탄.jpg", jpeg_bomb()),
        ("폭탄.gif", gif_bomb()),
        ("폭탄.webp", webp_bomb()),
        ("폭탄.bmp", bmp_bomb()),
    ] {
        assert_eq!(
            fails(&dir, name, &bytes),
            ThumbnailError::TooLarge,
            "{name}"
        );
    }
}

#[test]
#[ignore = "#326"]
fn 상한은_바꿔_넘길_수_있고_파일_크기_상한도_있다() {
    assert_eq!(
        DecodeLimits::default(),
        DecodeLimits {
            max_pixels: DEFAULT_MAX_PIXELS,
            max_file_size: DEFAULT_MAX_FILE_SIZE,
        }
    );
    assert!(
        (50 * 1024 * 1024..).contains(&DEFAULT_MAX_FILE_SIZE),
        "보통 사진은 넉넉히 들어간다"
    );
    let dir = TempDir::new().unwrap();
    let path = write(&dir, "그림.png", &png(64, 32, |_, _| opaque(GREEN)));
    let go = AtomicBool::new(false);
    assert!(make_thumbnail_with(&path, 96, &DecodeLimits::default(), &go).is_ok());

    let few_pixels = DecodeLimits {
        max_pixels: 1_000,
        ..DecodeLimits::default()
    };
    assert_eq!(
        make_thumbnail_with(&path, 96, &few_pixels, &go),
        Err(ThumbnailError::TooLarge),
        "64×32 = 2048화소 > 1000"
    );
    let small_file = DecodeLimits {
        max_file_size: 16,
        ..DecodeLimits::default()
    };
    assert_eq!(
        make_thumbnail_with(&path, 96, &small_file, &go),
        Err(ThumbnailError::TooLarge),
        "파일 크기 상한"
    );
}

// ---- 실패

#[test]
#[ignore = "#326"]
fn 잘린_파일은_패닉_없이_실패한다() {
    let dir = TempDir::new().unwrap();
    let jpg = jpeg(64, 32, None, |_, _| RED);
    let sos = jpg.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
    let png_bytes = png(300, 200, |_, _| opaque(RED));
    let gif_bytes = gif(40, 30, &[0]);
    let webp_bytes = webp(40, 30, |x, _| opaque(if x < 20 { RED } else { BLUE }));
    let bmp_bytes = bmp(48, 32, |_, _| RED);
    for (name, bytes) in [
        ("잘림.jpg", &jpg[..sos]),
        ("잘림.png", &png_bytes[..png_bytes.len() / 2]),
        ("잘림.gif", &gif_bytes[..400]),
        ("잘림.webp", &webp_bytes[..webp_bytes.len() / 2]),
        ("잘림.bmp", &bmp_bytes[..60]),
    ] {
        fails(&dir, name, bytes);
    }
}

#[test]
#[ignore = "#326"]
fn 머리만_맞는_가짜_파일과_빈_파일은_패닉_없이_실패한다() {
    let dir = TempDir::new().unwrap();
    let garbage = [0x5Au8; 1000];
    let with = |head: &[u8]| {
        let mut v = head.to_vec();
        v.extend_from_slice(&garbage);
        v
    };
    for (name, bytes) in [
        ("가짜.jpg", with(b"\xFF\xD8\xFF\xE0")),
        ("가짜.png", with(b"\x89PNG\r\n\x1a\n")),
        ("가짜.gif", with(b"GIF89a")),
        ("가짜.webp", with(b"RIFF\x00\x10\x00\x00WEBPVP8L")),
        ("가짜.bmp", with(b"BM")),
    ] {
        fails(&dir, name, &bytes);
    }
    for name in ["빈.jpg", "빈.png", "빈.gif", "빈.webp", "빈.bmp"] {
        fails(&dir, name, &[]);
    }
}

#[test]
#[ignore = "#326"]
fn 없는_파일은_not_found다() {
    let dir = TempDir::new().unwrap();
    assert_eq!(
        make_thumbnail(&dir.path().join("없음.jpg"), 96),
        Err(ThumbnailError::NotFound)
    );
}

#[test]
#[ignore = "#326"]
fn 이미지가_아닌_확장자는_unsupported다() {
    let dir = TempDir::new().unwrap();
    assert_eq!(
        fails(&dir, "메모.txt", "그림이 아니다".as_bytes()),
        ThumbnailError::Unsupported
    );
    // 도우미가 맡을 형식(이 spec 밖)도 자체 디코더는 만들지 않는다.
    assert_eq!(
        fails(&dir, "그림.psd", b"8BPS\x00\x01"),
        ThumbnailError::Unsupported
    );
}

#[test]
#[ignore = "#326"]
fn 미리_취소한_요청은_읽지_않고_cancelled다() {
    let dir = TempDir::new().unwrap();
    let path = write(&dir, "그림.png", &png(40, 30, |_, _| opaque(GREEN)));
    let cancel = AtomicBool::new(true);
    assert_eq!(
        make_thumbnail_with(&path, 96, &DecodeLimits::default(), &cancel),
        Err(ThumbnailError::Cancelled)
    );
}

#[test]
#[ignore = "#326"]
fn 자체_디코더_형식은_확장자로_가린다() {
    for name in [
        "a.jpg",
        "b.JPG",
        "c.jpeg",
        "d.Jpeg",
        "e.png",
        "f.PNG",
        "g.gif",
        "h.webp",
        "i.WebP",
        "j.bmp",
        "사진.jpg",
    ] {
        assert!(is_decodable(OsStr::new(name)), "{name}");
    }
    for name in [
        "a.txt",
        "b.psd",
        "c.heic",
        "d.mp4",
        "e.pdf",
        "f.jpg.txt",
        "이름없음",
        "g.png.zip",
    ] {
        assert!(!is_decodable(OsStr::new(name)), "{name}");
    }
}

#[test]
#[ignore = "#326"]
fn 썸네일_키는_파일의_수정_시각과_크기를_읽는다() {
    let dir = TempDir::new().unwrap();
    let path = write(&dir, "그림.png", b"12345");
    let key = ThumbnailKey::for_file(&path, 96).unwrap();
    let meta = fs::metadata(&path).unwrap();
    assert_eq!(key.path, path);
    assert_eq!(key.side, 96);
    assert_eq!(key.file_size, 5);
    assert_eq!(key.modified, meta.modified().unwrap());
    assert!(ThumbnailKey::for_file(&dir.path().join("없음.png"), 96).is_err());
}
