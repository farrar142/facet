// target: crates/facet-core/tests/spec_zip_support.rs
// AC: 없음(시험 함수 없음): S13(#28) 압축 파일 탐색의 인수 테스트들이 함께 쓰는 시험 zip 만들기
//! S13 압축 파일 탐색의 인수 테스트(`spec_zip_*.rs`)가 `#[path = "spec_zip_support.rs"] mod zip_support;`로 넣어 쓰는
//! 시험 zip 만들기 코드다. 저장소에 고정 파일을 두지 않고 테스트 안에서 바이트를 만든다: 이름 바이트·일반 목적 비트·
//! 압축 방식·선언 크기·확장 필드·외부 속성을 마음대로 정하는 항목, Stored와 고정 허프만 Deflate(일치 포함), zip64
//! (항목 6만5천 개 초과·4GB 넘는 선언 크기), ZipCrypto와 WinZip AES(AE-2, AES-256) 암호화. 암호화에 쓰는 SHA-1·
//! HMAC·PBKDF2·AES는 만들 때마다 알려진 시험 벡터로 스스로 확인한다(틀리면 이 도우미의 잘못이라고 알리며 멈춘다).
//! 시험 함수는 없다. 이 파일 자체도 시험 실행 파일로 빌드되므로 쓰지 않는 항목 경고를 끈다. 모든 플랫폼에서 빌드된다.

#![allow(dead_code)]

use std::path::Path;

/// 일반 목적 비트 0: 암호화.
pub const FLAG_ENCRYPTED: u16 = 1;
/// 일반 목적 비트 11: 이름이 UTF-8이다.
pub const FLAG_UTF8: u16 = 1 << 11;
/// 압축 방식: Stored.
pub const STORED: u16 = 0;
/// 압축 방식: Deflate.
pub const DEFLATE: u16 = 8;
/// 압축 방식: bzip2(꺼내기에서 지원하지 않는 방식의 보기).
pub const BZIP2: u16 = 12;
/// 압축 방식: WinZip AES.
pub const AES_METHOD: u16 = 99;

/// 기본 시각(DOS): 2024-05-06 07:08:10(지역 시각).
pub const DOS_DATE: u16 = ((2024 - 1980) << 9) | (5 << 5) | 6;
pub const DOS_TIME: u16 = (7 << 11) | (8 << 5) | (10 / 2);

// ---- CRC-32 ----

fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    for (n, slot) in table.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *slot = c;
    }
    table
}

/// 바이트 하나로 CRC-32 상태를 갱신한다(뒤집힌 상태 그대로, ZipCrypto 키 갱신이 쓴다).
fn crc_byte(table: &[u32; 256], crc: u32, byte: u8) -> u32 {
    table[((crc ^ u32::from(byte)) & 0xFF) as usize] ^ (crc >> 8)
}

/// zip의 CRC-32.
pub fn crc32(data: &[u8]) -> u32 {
    let table = crc_table();
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = crc_byte(&table, crc, b);
    }
    !crc
}

// ---- 고정 허프만 Deflate ----

struct BitWriter {
    out: Vec<u8>,
    bit: u32,
    cur: u32,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            out: Vec::new(),
            bit: 0,
            cur: 0,
        }
    }

    /// 값의 아래 비트부터 `count`개를 쓴다(추가 비트·블록 머리).
    fn bits(&mut self, value: u32, count: u32) {
        for i in 0..count {
            self.cur |= ((value >> i) & 1) << self.bit;
            self.bit += 1;
            if self.bit == 8 {
                self.out.push(self.cur as u8);
                self.cur = 0;
                self.bit = 0;
            }
        }
    }

    /// 허프만 부호를 윗비트부터 쓴다.
    fn code(&mut self, code: u32, len: u32) {
        for i in (0..len).rev() {
            self.bits((code >> i) & 1, 1);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.bit > 0 {
            self.out.push(self.cur as u8);
        }
        self.out
    }
}

fn literal_length_code(w: &mut BitWriter, symbol: u32) {
    match symbol {
        0..=143 => w.code(0x30 + symbol, 8),
        144..=255 => w.code(0x190 + (symbol - 144), 9),
        256..=279 => w.code(symbol - 256, 7),
        _ => w.code(0xC0 + (symbol - 280), 8),
    }
}

/// 거리 `distance`(1..=32768)의 거리 부호와 추가 비트.
fn distance_code(w: &mut BitWriter, distance: u32) {
    // (부호, 바탕 거리, 추가 비트 수)
    let mut base = 1u32;
    for code in 0..30u32 {
        let extra = if code < 4 { 0 } else { (code / 2) - 1 };
        let span = 1u32 << extra;
        if distance < base + span {
            w.code(code, 5);
            w.bits(distance - base, extra);
            return;
        }
        base += span;
    }
    panic!("거리가 너무 크다: {distance}");
}

/// 고정 허프만 블록 하나로 압축한다. 앞 256바이트와 같은 258바이트가 이어지면 일치(길이 258, 거리 256)로 쓰고
/// 나머지는 글자로 쓴다. 그래서 같은 256바이트가 되풀이되는 내용은 실제로 작아진다.
pub fn fixed_deflate(data: &[u8]) -> Vec<u8> {
    const DISTANCE: usize = 256;
    const LENGTH: usize = 258;
    let mut w = BitWriter::new();
    w.bits(1, 1); // BFINAL
    w.bits(1, 2); // BTYPE = 01(고정 허프만)
    let mut pos = 0;
    while pos < data.len() {
        let matches = pos >= DISTANCE
            && pos + LENGTH <= data.len()
            && (0..LENGTH).all(|i| data[pos + i] == data[pos - DISTANCE + i]);
        if matches {
            literal_length_code(&mut w, 285); // 길이 258, 추가 비트 없음
            distance_code(&mut w, DISTANCE as u32);
            pos += LENGTH;
        } else {
            literal_length_code(&mut w, u32::from(data[pos]));
            pos += 1;
        }
    }
    literal_length_code(&mut w, 256);
    w.finish()
}

/// 압축 시험에 쓰는 본문: 같은 256바이트 묶음이 `blocks`번 되풀이된다(한글 포함).
pub fn repeated_text(blocks: usize) -> Vec<u8> {
    let mut unit = Vec::new();
    let mut i = 0;
    while unit.len() < 256 {
        unit.extend_from_slice(format!("facet 압축 시험 줄 {i:04}\n").as_bytes());
        i += 1;
    }
    unit.truncate(256);
    unit.repeat(blocks)
}

// ---- SHA-1 · HMAC · PBKDF2 ----

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        let mut i = 16;
        while i < 80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
            i += 1;
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn hmac_sha1(key: &[u8], data: &[u8]) -> [u8; 20] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..20].copy_from_slice(&sha1(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    inner.extend_from_slice(data);
    let inner_hash = sha1(&inner);
    let mut outer: Vec<u8> = k.iter().map(|b| b ^ 0x5C).collect();
    outer.extend_from_slice(&inner_hash);
    sha1(&outer)
}

fn pbkdf2_sha1(password: &[u8], salt: &[u8], iterations: u32, len: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut block = 1u32;
    while out.len() < len {
        let mut s = salt.to_vec();
        s.extend_from_slice(&block.to_be_bytes());
        let mut u = hmac_sha1(password, &s);
        let mut t = u;
        for _ in 1..iterations {
            u = hmac_sha1(password, &u);
            for (x, y) in t.iter_mut().zip(u.iter()) {
                *x ^= y;
            }
        }
        out.extend_from_slice(&t);
        block += 1;
    }
    out.truncate(len);
    out
}

// ---- AES-256(암호화만) ----

fn gf_mul(mut a: u8, mut b: u8) -> u8 {
    let mut p = 0u8;
    while b != 0 {
        if b & 1 != 0 {
            p ^= a;
        }
        let high = a & 0x80;
        a <<= 1;
        if high != 0 {
            a ^= 0x1B;
        }
        b >>= 1;
    }
    p
}

fn sbox() -> [u8; 256] {
    let mut table = [0u8; 256];
    for (x, slot) in table.iter_mut().enumerate() {
        // 곱셈 역원(0의 역원은 0).
        let inv = if x == 0 {
            0
        } else {
            (1..=255u8)
                .find(|&y| gf_mul(x as u8, y) == 1)
                .expect("역원")
        };
        let mut s = inv;
        let mut r = inv;
        for _ in 0..4 {
            r = r.rotate_left(1);
            s ^= r;
        }
        *slot = s ^ 0x63;
    }
    table
}

struct Aes256 {
    round_keys: Vec<[u8; 16]>,
    sbox: [u8; 256],
}

impl Aes256 {
    fn new(key: &[u8]) -> Self {
        assert_eq!(key.len(), 32);
        let sbox = sbox();
        let mut words: Vec<[u8; 4]> = key.chunks(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
        let mut rcon = 1u8;
        for i in 8..60 {
            let mut temp = words[i - 1];
            if i % 8 == 0 {
                temp = [temp[1], temp[2], temp[3], temp[0]];
                for t in temp.iter_mut() {
                    *t = sbox[*t as usize];
                }
                temp[0] ^= rcon;
                rcon = gf_mul(rcon, 2);
            } else if i % 8 == 4 {
                for t in temp.iter_mut() {
                    *t = sbox[*t as usize];
                }
            }
            let prev = words[i - 8];
            words.push([
                prev[0] ^ temp[0],
                prev[1] ^ temp[1],
                prev[2] ^ temp[2],
                prev[3] ^ temp[3],
            ]);
        }
        let round_keys = words
            .chunks(4)
            .map(|w| {
                let mut k = [0u8; 16];
                for (j, word) in w.iter().enumerate() {
                    k[j * 4..j * 4 + 4].copy_from_slice(word);
                }
                k
            })
            .collect();
        Self { round_keys, sbox }
    }

    fn encrypt(&self, block: &[u8; 16]) -> [u8; 16] {
        let mut s = *block;
        xor16(&mut s, &self.round_keys[0]);
        for (round, round_key) in self.round_keys.iter().enumerate().skip(1) {
            for b in s.iter_mut() {
                *b = self.sbox[*b as usize];
            }
            // 행 옮기기(열 우선 배치: s[행 + 4*열]).
            let t = s;
            for col in 0..4 {
                for row in 0..4 {
                    s[row + 4 * col] = t[row + 4 * ((col + row) % 4)];
                }
            }
            if round != 14 {
                let t = s;
                for col in 0..4 {
                    let c = &t[4 * col..4 * col + 4];
                    s[4 * col] = gf_mul(c[0], 2) ^ gf_mul(c[1], 3) ^ c[2] ^ c[3];
                    s[4 * col + 1] = c[0] ^ gf_mul(c[1], 2) ^ gf_mul(c[2], 3) ^ c[3];
                    s[4 * col + 2] = c[0] ^ c[1] ^ gf_mul(c[2], 2) ^ gf_mul(c[3], 3);
                    s[4 * col + 3] = gf_mul(c[0], 3) ^ c[1] ^ c[2] ^ gf_mul(c[3], 2);
                }
            }
            xor16(&mut s, round_key);
        }
        s
    }
}

fn xor16(a: &mut [u8; 16], b: &[u8; 16]) {
    for (x, y) in a.iter_mut().zip(b.iter()) {
        *x ^= y;
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 이 도우미의 암호 기본 연산을 알려진 시험 벡터로 확인한다. 틀리면 도우미의 잘못이다.
fn check_crypto_helpers() {
    assert_eq!(
        hex(&sha1(b"abc")),
        "a9993e364706816aba3e25717850c26c9cd0d89d",
        "시험 도우미의 SHA-1이 틀렸다"
    );
    assert_eq!(
        hex(&pbkdf2_sha1(b"password", b"salt", 2, 20)),
        "ea6c014dc72d6f8ccd1ed92ace1d41f0d8de8957",
        "시험 도우미의 PBKDF2-HMAC-SHA1이 틀렸다"
    );
    let key: Vec<u8> = (0u8..32).collect();
    let plain: [u8; 16] = [
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        0xff,
    ];
    assert_eq!(
        hex(&Aes256::new(&key).encrypt(&plain)),
        "8ea2b7ca516745bfeafc49904b496089",
        "시험 도우미의 AES-256이 틀렸다"
    );
}

// ---- 항목과 zip ----

/// zip 항목 하나. 필드를 바로 고쳐 깨진·악성 zip도 만든다.
#[derive(Debug, Clone)]
pub struct Entry {
    /// 이름 바이트(디스크에 쓰이는 그대로).
    pub name: Vec<u8>,
    pub flags: u16,
    pub method: u16,
    /// 로컬 머리 뒤에 오는 바이트(압축·암호화된 그대로). 압축된 크기는 이 길이다.
    pub data: Vec<u8>,
    pub crc: u32,
    /// 선언한 압축 해제 크기.
    pub size: u64,
    pub dos_date: u16,
    pub dos_time: u16,
    /// 로컬 머리와 central directory 둘 다에 쓰는 확장 필드.
    pub extra: Vec<u8>,
    pub external_attrs: u32,
    pub version_made_by: u16,
}

impl Entry {
    /// UTF-8 표시가 붙은 Stored 파일.
    pub fn stored(name: &str, content: &[u8]) -> Entry {
        Entry {
            name: name.as_bytes().to_vec(),
            flags: FLAG_UTF8,
            method: STORED,
            data: content.to_vec(),
            crc: crc32(content),
            size: content.len() as u64,
            dos_date: DOS_DATE,
            dos_time: DOS_TIME,
            extra: Vec::new(),
            external_attrs: 0,
            version_made_by: 20,
        }
    }

    /// UTF-8 표시가 붙은 Deflate 파일(고정 허프만).
    pub fn deflated(name: &str, content: &[u8]) -> Entry {
        Entry {
            method: DEFLATE,
            data: fixed_deflate(content),
            ..Entry::stored(name, content)
        }
    }

    /// 폴더 기록(이름 끝 `/`, 크기 0, MS-DOS 폴더 속성).
    pub fn folder(name: &str) -> Entry {
        let name = if name.ends_with('/') {
            name.to_string()
        } else {
            format!("{name}/")
        };
        Entry {
            external_attrs: 0x10,
            ..Entry::stored(&name, b"")
        }
    }

    /// UTF-8 표시 없는 Stored 파일: 이름 바이트를 그대로 쓴다(CP949·CP437 이름).
    pub fn raw(name: &[u8], content: &[u8]) -> Entry {
        Entry {
            name: name.to_vec(),
            flags: 0,
            ..Entry::stored("x", content)
        }
    }

    /// 확장 시각 필드(0x5455, 수정 시각만, 유닉스 초 UTC)를 더한다.
    pub fn modified_unix(mut self, secs: u32) -> Entry {
        self.extra.extend_from_slice(&0x5455u16.to_le_bytes());
        self.extra.extend_from_slice(&5u16.to_le_bytes());
        self.extra.push(1);
        self.extra.extend_from_slice(&secs.to_le_bytes());
        self
    }

    /// Info-ZIP Unicode Path 확장 필드(0x7075)를 더한다. 이름 바이트는 그대로 두고 UTF-8 이름을 확장 필드에 담는다.
    pub fn unicode_path(mut self, utf8_name: &str) -> Entry {
        let mut body = vec![1u8];
        body.extend_from_slice(&crc32(&self.name).to_le_bytes());
        body.extend_from_slice(utf8_name.as_bytes());
        self.extra.extend_from_slice(&0x7075u16.to_le_bytes());
        self.extra
            .extend_from_slice(&(body.len() as u16).to_le_bytes());
        self.extra.extend_from_slice(&body);
        self
    }

    /// 유닉스에서 만든 심볼릭 링크 항목으로 바꾼다(내용은 가리키는 곳).
    pub fn symlink(mut self) -> Entry {
        self.version_made_by = (3 << 8) | 20;
        self.external_attrs = 0o120777 << 16;
        self
    }

    /// ZipCrypto(전통 PKWARE 암호화)로 암호화한다. 압축 방식은 그대로다.
    pub fn zipcrypto(mut self, password: &str) -> Entry {
        let table = crc_table();
        let mut keys = [0x1234_5678u32, 0x2345_6789, 0x3456_7890];
        let update = |keys: &mut [u32; 3], byte: u8| {
            keys[0] = crc_byte(&table, keys[0], byte);
            keys[1] = keys[1]
                .wrapping_add(keys[0] & 0xFF)
                .wrapping_mul(134_775_813)
                .wrapping_add(1);
            keys[2] = crc_byte(&table, keys[2], (keys[1] >> 24) as u8);
        };
        for &b in password.as_bytes() {
            update(&mut keys, b);
        }
        let mut header: Vec<u8> = (0u8..11)
            .map(|i| i.wrapping_mul(37).wrapping_add(5))
            .collect();
        header.push((self.crc >> 24) as u8);
        let mut out = Vec::with_capacity(12 + self.data.len());
        for &plain in header.iter().chain(self.data.iter()) {
            let temp = (keys[2] | 2) as u16;
            let key_byte = (temp.wrapping_mul(temp ^ 1) >> 8) as u8;
            out.push(plain ^ key_byte);
            update(&mut keys, plain);
        }
        self.data = out;
        self.flags |= FLAG_ENCRYPTED;
        self
    }

    /// WinZip AES(AE-2, AES-256)로 암호화한다. 원래 압축 방식은 확장 필드에 담고 방식은 99가 된다.
    pub fn aes(mut self, password: &str) -> Entry {
        check_crypto_helpers();
        let salt: Vec<u8> = (0u8..16)
            .map(|i| i.wrapping_mul(29).wrapping_add(3))
            .collect();
        let derived = pbkdf2_sha1(password.as_bytes(), &salt, 1000, 66);
        let (enc_key, rest) = derived.split_at(32);
        let (auth_key, verifier) = rest.split_at(32);
        let aes = Aes256::new(enc_key);
        let mut encrypted = Vec::with_capacity(self.data.len());
        for (n, chunk) in self.data.chunks(16).enumerate() {
            let mut counter = [0u8; 16];
            counter[..8].copy_from_slice(&((n as u64) + 1).to_le_bytes());
            let stream = aes.encrypt(&counter);
            encrypted.extend(chunk.iter().zip(stream.iter()).map(|(a, b)| a ^ b));
        }
        let auth = hmac_sha1(auth_key, &encrypted);
        let mut data = salt;
        data.extend_from_slice(verifier);
        data.extend_from_slice(&encrypted);
        data.extend_from_slice(&auth[..10]);
        self.extra.extend_from_slice(&0x9901u16.to_le_bytes());
        self.extra.extend_from_slice(&7u16.to_le_bytes());
        self.extra.extend_from_slice(&2u16.to_le_bytes()); // AE-2
        self.extra.extend_from_slice(b"AE");
        self.extra.push(3); // AES-256
        self.extra.extend_from_slice(&self.method.to_le_bytes());
        self.method = AES_METHOD;
        self.crc = 0; // AE-2는 CRC를 쓰지 않는다
        self.data = data;
        self.flags |= FLAG_ENCRYPTED;
        self
    }
}

const U32_MAX: u64 = 0xFFFF_FFFF;

/// 항목들로 zip 바이트를 만든다. 항목이 65535개를 넘거나 크기·자리가 32비트를 넘으면 zip64 기록을 쓴다.
pub fn build(entries: &[Entry]) -> Vec<u8> {
    build_with(entries, false)
}

/// [`build`]와 같되 항목 수와 상관없이 zip64 끝 기록을 쓴다.
pub fn build_zip64(entries: &[Entry]) -> Vec<u8> {
    build_with(entries, true)
}

fn push16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn push32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn push64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn build_with(entries: &[Entry], force_zip64: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let mut offsets = Vec::with_capacity(entries.len());
    for e in entries {
        let offset = out.len() as u64;
        offsets.push(offset);
        let compressed = e.data.len() as u64;
        let big = e.size >= U32_MAX || compressed >= U32_MAX;
        let mut extra = e.extra.clone();
        if big {
            push16(&mut extra, 0x0001);
            push16(&mut extra, 16);
            push64(&mut extra, e.size);
            push64(&mut extra, compressed);
        }
        push32(&mut out, 0x0403_4B50);
        push16(&mut out, if big { 45 } else { 20 });
        push16(&mut out, e.flags);
        push16(&mut out, e.method);
        push16(&mut out, e.dos_time);
        push16(&mut out, e.dos_date);
        push32(&mut out, e.crc);
        push32(
            &mut out,
            if big {
                U32_MAX as u32
            } else {
                compressed as u32
            },
        );
        push32(&mut out, if big { U32_MAX as u32 } else { e.size as u32 });
        push16(&mut out, e.name.len() as u16);
        push16(&mut out, extra.len() as u16);
        out.extend_from_slice(&e.name);
        out.extend_from_slice(&extra);
        out.extend_from_slice(&e.data);
    }
    let cd_offset = out.len() as u64;
    for (e, &offset) in entries.iter().zip(offsets.iter()) {
        let compressed = e.data.len() as u64;
        let mut zip64 = Vec::new();
        if e.size >= U32_MAX {
            push64(&mut zip64, e.size);
        }
        if compressed >= U32_MAX {
            push64(&mut zip64, compressed);
        }
        if offset >= U32_MAX {
            push64(&mut zip64, offset);
        }
        let mut extra = e.extra.clone();
        if !zip64.is_empty() {
            push16(&mut extra, 0x0001);
            push16(&mut extra, zip64.len() as u16);
            extra.extend_from_slice(&zip64);
        }
        let clamp = |v: u64| {
            if v >= U32_MAX {
                U32_MAX as u32
            } else {
                v as u32
            }
        };
        push32(&mut out, 0x0201_4B50);
        push16(&mut out, e.version_made_by);
        push16(&mut out, if zip64.is_empty() { 20 } else { 45 });
        push16(&mut out, e.flags);
        push16(&mut out, e.method);
        push16(&mut out, e.dos_time);
        push16(&mut out, e.dos_date);
        push32(&mut out, e.crc);
        push32(&mut out, clamp(compressed));
        push32(&mut out, clamp(e.size));
        push16(&mut out, e.name.len() as u16);
        push16(&mut out, extra.len() as u16);
        push16(&mut out, 0); // 주석 길이
        push16(&mut out, 0); // 시작 디스크
        push16(&mut out, 0); // 내부 속성
        push32(&mut out, e.external_attrs);
        push32(&mut out, clamp(offset));
        out.extend_from_slice(&e.name);
        out.extend_from_slice(&extra);
    }
    let cd_size = out.len() as u64 - cd_offset;
    let count = entries.len() as u64;
    let zip64 = force_zip64 || count >= 0xFFFF || cd_offset >= U32_MAX || cd_size >= U32_MAX;
    if zip64 {
        let record = out.len() as u64;
        push32(&mut out, 0x0606_4B50);
        push64(&mut out, 44);
        push16(&mut out, 45);
        push16(&mut out, 45);
        push32(&mut out, 0);
        push32(&mut out, 0);
        push64(&mut out, count);
        push64(&mut out, count);
        push64(&mut out, cd_size);
        push64(&mut out, cd_offset);
        push32(&mut out, 0x0706_4B50);
        push32(&mut out, 0);
        push64(&mut out, record);
        push32(&mut out, 1);
    }
    push32(&mut out, 0x0605_4B50);
    push16(&mut out, 0);
    push16(&mut out, 0);
    let small_count = if zip64 { 0xFFFF } else { count as u16 };
    push16(&mut out, small_count);
    push16(&mut out, small_count);
    push32(
        &mut out,
        if zip64 {
            U32_MAX as u32
        } else {
            cd_size as u32
        },
    );
    push32(
        &mut out,
        if zip64 {
            U32_MAX as u32
        } else {
            cd_offset as u32
        },
    );
    push16(&mut out, 0);
    out
}

/// zip 바이트를 파일로 쓴다(폴더가 없으면 만든다).
pub fn write(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, bytes).unwrap();
}

/// `path`에 항목들로 zip을 만든다.
pub fn write_zip(path: &Path, entries: &[Entry]) {
    write(path, &build(entries));
}

/// zip 바이트의 central directory 항목마다 (이름 바이트, 일반 목적 비트, 압축 방식)을 읽는다. 끝 기록(zip64 포함)을
/// 따라가는 간단한 읽기로, facet이 쓴 zip이 UTF-8 표시·Deflate로 쓰였는지 볼 때 쓴다.
pub fn central_entries(bytes: &[u8]) -> Vec<(Vec<u8>, u16, u16)> {
    let le16 = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let le32 = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let le64 = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
    let eocd = (0..=bytes.len() - 22)
        .rev()
        .find(|&i| le32(i) == 0x0605_4B50)
        .expect("끝 기록이 있다");
    let mut count = u64::from(le16(eocd + 10));
    let mut offset = u64::from(le32(eocd + 16));
    if count == 0xFFFF || offset == U32_MAX {
        let locator = eocd - 20;
        assert_eq!(le32(locator), 0x0706_4B50, "zip64 끝 기록 위치 표시");
        let record = le64(locator + 8) as usize;
        count = le64(record + 32);
        offset = le64(record + 48);
    }
    let mut at = offset as usize;
    let mut found = Vec::new();
    for _ in 0..count {
        assert_eq!(le32(at), 0x0201_4B50, "central directory 항목");
        let flags = le16(at + 8);
        let method = le16(at + 10);
        let name_len = le16(at + 28) as usize;
        let extra_len = le16(at + 30) as usize;
        let comment_len = le16(at + 32) as usize;
        found.push((bytes[at + 46..at + 46 + name_len].to_vec(), flags, method));
        at += 46 + name_len + extra_len + comment_len;
    }
    found
}
