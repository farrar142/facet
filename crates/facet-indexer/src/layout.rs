//! MFT가 볼륨의 어디에 있는지 알아내는 최소한의 해석.
//!
//! MFT 자신도 파일(`$MFT`, 레코드 0)이고 조각나 있을 수 있다. 그래서 레코드 0의 이름 없는 `$DATA`
//! 속성에서 데이터 런(run list)을 읽어 MFT의 각 부분이 볼륨의 어느 클러스터에 있는지 안다.
//! 인덱스용 MFT 파싱(이름·부모·크기를 뽑기)은 여기서 하지 않는다. 그것은 인덱스 크레이트의
//! 일이다(spec #22, 티켓 #43). 여기 있는 것은 떠 내기에 필요한 만큼이다.
//!
//! 모든 함수는 범위를 넘는 읽기를 하지 않고 패닉 대신 오류 값을 낸다.

/// NTFS의 고정값(update sequence) 간격. 섹터 크기와 상관없이 512바이트다.
const FIXUP_STRIDE: usize = 512;
/// 속성 목록의 끝 표지.
const ATTRIBUTE_END: u32 = 0xFFFF_FFFF;
/// `$DATA` 속성 형식.
const ATTRIBUTE_DATA: u32 = 0x80;
/// `$ATTRIBUTE_LIST` 속성 형식.
const ATTRIBUTE_LIST: u32 = 0x20;

/// MFT의 연속된 한 부분: 가상 클러스터 `vcn`부터 `clusters`개가 볼륨의 `lcn`부터 있다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extent {
    pub vcn: u64,
    pub lcn: u64,
    pub clusters: u64,
}

/// 볼륨에서 읽을 한 덩어리: 볼륨 시작부터 `offset` 바이트에서 `len` 바이트.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Read {
    pub offset: u64,
    pub len: usize,
}

fn u16_at(b: &[u8], at: usize) -> Result<u16, String> {
    b.get(at..at + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| format!("레코드가 짧음: {at}바이트 위치를 읽을 수 없음"))
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, String> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| format!("레코드가 짧음: {at}바이트 위치를 읽을 수 없음"))
}

fn u64_at(b: &[u8], at: usize) -> Result<u64, String> {
    let s = b
        .get(at..at + 8)
        .ok_or_else(|| format!("레코드가 짧음: {at}바이트 위치를 읽을 수 없음"))?;
    let mut a = [0; 8];
    a.copy_from_slice(s);
    Ok(u64::from_le_bytes(a))
}

/// 디스크에서 읽은 레코드 하나에 고정값을 되돌린다(제자리에서).
///
/// 각 512바이트 조각의 마지막 2바이트는 디스크에 쓸 때 갱신 순서 번호(USN)로 바뀌어 있다.
/// 그 자리가 USN과 같은지 확인하고 원래 값(갱신 순서 배열에 있음)으로 되돌린다.
pub fn apply_fixup(record: &mut [u8]) -> Result<(), String> {
    if record.get(0..4) != Some(b"FILE") {
        return Err("레코드 서명이 FILE이 아님".to_owned());
    }
    let usa_offset = usize::from(u16_at(record, 4)?);
    let usa_count = usize::from(u16_at(record, 6)?);
    if usa_count == 0 || (usa_count - 1) * FIXUP_STRIDE > record.len() {
        return Err(format!(
            "갱신 순서 배열의 크기가 레코드와 맞지 않음: {usa_count}"
        ));
    }
    let usn = u16_at(record, usa_offset)?;
    for i in 1..usa_count {
        let original = u16_at(record, usa_offset + 2 * i)?;
        let end = i * FIXUP_STRIDE - 2;
        if u16_at(record, end)? != usn {
            return Err(format!("{i}번째 조각의 고정값이 틀림"));
        }
        record[end..end + 2].copy_from_slice(&original.to_le_bytes());
    }
    Ok(())
}

/// 고정값을 되돌린 `$MFT` 레코드(레코드 0)에서 MFT 데이터의 위치 목록을 읽는다.
pub fn mft_extents(record: &[u8]) -> Result<Vec<Extent>, String> {
    if record.get(0..4) != Some(b"FILE") {
        return Err("$MFT 레코드 서명이 FILE이 아님".to_owned());
    }
    let used = (u32_at(record, 0x18)? as usize).min(record.len());
    let mut at = usize::from(u16_at(record, 0x14)?);
    let mut has_attribute_list = false;
    while at + 8 <= used {
        let kind = u32_at(record, at)?;
        if kind == ATTRIBUTE_END {
            break;
        }
        let len = u32_at(record, at + 4)? as usize;
        if len < 0x18 || at + len > used {
            return Err(format!("$MFT 레코드의 속성 길이가 틀림: {len}"));
        }
        let attribute = &record[at..at + len];
        if kind == ATTRIBUTE_LIST {
            has_attribute_list = true;
        }
        let non_resident = attribute[8] != 0;
        let name_len = attribute[9];
        if kind == ATTRIBUTE_DATA && name_len == 0 {
            if !non_resident {
                return Err("$MFT의 $DATA가 상주 속성임".to_owned());
            }
            let start_vcn = u64_at(attribute, 0x10)?;
            let runs = usize::from(u16_at(attribute, 0x20)?);
            let runs = attribute
                .get(runs..)
                .ok_or("$MFT의 데이터 런 위치가 속성 밖임")?;
            return decode_runs(runs, start_vcn);
        }
        at += len;
    }
    if has_attribute_list {
        Err("$MFT의 $DATA가 확장 레코드에 있음(MFT가 매우 조각남). 지원하지 않음".to_owned())
    } else {
        Err("$MFT 레코드에 이름 없는 $DATA 속성이 없음".to_owned())
    }
}

/// 데이터 런을 풀어 위치 목록을 만든다.
fn decode_runs(runs: &[u8], start_vcn: u64) -> Result<Vec<Extent>, String> {
    let mut extents = Vec::new();
    let mut at = 0;
    let mut vcn = start_vcn;
    let mut lcn: i64 = 0;
    loop {
        let header = *runs.get(at).ok_or("데이터 런이 끝 표지 없이 끝남")?;
        if header == 0 {
            break;
        }
        let len_size = usize::from(header & 0x0F);
        let off_size = usize::from(header >> 4);
        if len_size == 0 || len_size > 8 || off_size > 8 {
            return Err(format!("데이터 런 머리가 틀림: {header:#04x}"));
        }
        let field = runs
            .get(at + 1..at + 1 + len_size + off_size)
            .ok_or("데이터 런이 중간에 끊김")?;
        let clusters = le_unsigned(&field[..len_size]);
        if off_size == 0 {
            return Err("$MFT에 빈(sparse) 런이 있음".to_owned());
        }
        lcn = lcn
            .checked_add(le_signed(&field[len_size..]))
            .filter(|l| *l >= 0)
            .ok_or("데이터 런의 클러스터 위치가 범위를 벗어남")?;
        extents.push(Extent {
            vcn,
            lcn: lcn as u64,
            clusters,
        });
        vcn = vcn
            .checked_add(clusters)
            .ok_or("데이터 런의 길이가 범위를 벗어남")?;
        at += 1 + len_size + off_size;
    }
    if extents.is_empty() {
        return Err("$MFT의 데이터 런이 비어 있음".to_owned());
    }
    Ok(extents)
}

fn le_unsigned(b: &[u8]) -> u64 {
    b.iter()
        .rev()
        .fold(0u64, |acc, byte| (acc << 8) | u64::from(*byte))
}

fn le_signed(b: &[u8]) -> i64 {
    let raw = le_unsigned(b);
    let bits = b.len() * 8;
    if bits == 0 || bits == 64 {
        raw as i64
    } else {
        // 부호 확장.
        let shift = 64 - bits;
        ((raw << shift) as i64) >> shift
    }
}

/// MFT 안의 바이트 범위 `[start, end)`를 볼륨에서 읽을 덩어리 목록으로 바꾼다.
///
/// 덩어리는 위치 하나 안에 머물고 `max_chunk` 바이트를 넘지 않는다. 범위의 어느 부분이
/// 위치 목록에 없으면 오류다.
pub fn plan_reads(
    extents: &[Extent],
    cluster_size: u64,
    start: u64,
    end: u64,
    max_chunk: usize,
) -> Result<Vec<Read>, String> {
    if max_chunk == 0 || cluster_size == 0 {
        return Err("덩어리 크기와 클러스터 크기는 0일 수 없음".to_owned());
    }
    let mut reads = Vec::new();
    let mut pos = start;
    while pos < end {
        let extent = extents
            .iter()
            .find(|e| {
                let from = e.vcn.saturating_mul(cluster_size);
                let to = from.saturating_add(e.clusters.saturating_mul(cluster_size));
                (from..to).contains(&pos)
            })
            .ok_or_else(|| format!("MFT의 {pos}바이트 위치가 데이터 런에 없음"))?;
        let extent_start = extent.vcn * cluster_size;
        let extent_end = extent_start + extent.clusters * cluster_size;
        let len = (extent_end.min(end) - pos).min(max_chunk as u64);
        reads.push(Read {
            offset: extent.lcn * cluster_size + (pos - extent_start),
            len: len as usize,
        });
        pos += len;
    }
    Ok(reads)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_decode_with_relative_signed_offsets() {
        // 길이 1바이트·위치 2바이트: 0x10 클러스터를 0x1000에, 다음 0x08 클러스터를 0x1000-0x100에.
        let runs = [0x21, 0x10, 0x00, 0x10, 0x21, 0x08, 0x00, 0xFF, 0x00];
        assert_eq!(
            decode_runs(&runs, 0),
            Ok(vec![
                Extent {
                    vcn: 0,
                    lcn: 0x1000,
                    clusters: 0x10
                },
                Extent {
                    vcn: 0x10,
                    lcn: 0x0F00,
                    clusters: 0x08
                },
            ])
        );
    }

    #[test]
    fn broken_runs_are_errors_not_panics() {
        assert!(decode_runs(&[], 0).is_err());
        assert!(decode_runs(&[0x00], 0).is_err());
        assert!(decode_runs(&[0x21, 0x10], 0).is_err());
        assert!(decode_runs(&[0x01, 0x10, 0x00], 0).is_err()); // sparse
        assert!(decode_runs(&[0x11, 0x10, 0x80, 0x00], 0).is_err()); // 음수 클러스터
        assert!(decode_runs(&[0x9F], 0).is_err());
    }

    #[test]
    fn plan_reads_follows_extents_and_splits_chunks() {
        let extents = [
            Extent {
                vcn: 0,
                lcn: 100,
                clusters: 2,
            },
            Extent {
                vcn: 2,
                lcn: 10,
                clusters: 2,
            },
        ];
        // 클러스터 4096, MFT 1024..14336 범위, 덩어리 4096.
        let reads = plan_reads(&extents, 4096, 1024, 14336, 4096).unwrap();
        assert_eq!(
            reads,
            vec![
                Read {
                    offset: 100 * 4096 + 1024,
                    len: 4096
                },
                Read {
                    offset: 100 * 4096 + 5120,
                    len: 3072
                },
                Read {
                    offset: 10 * 4096,
                    len: 4096
                },
                Read {
                    offset: 11 * 4096,
                    len: 2048
                },
            ]
        );
        assert!(plan_reads(&extents, 4096, 0, 4 * 4096 + 1, 4096).is_err());
        assert_eq!(plan_reads(&extents, 4096, 0, 0, 4096), Ok(vec![]));
    }

    #[test]
    fn fixup_restores_sector_ends_and_rejects_mismatch() {
        let mut record = vec![0u8; 1024];
        record[0..4].copy_from_slice(b"FILE");
        record[4..6].copy_from_slice(&0x30u16.to_le_bytes());
        record[6..8].copy_from_slice(&3u16.to_le_bytes());
        record[0x30..0x32].copy_from_slice(&0xABCDu16.to_le_bytes());
        record[0x32..0x34].copy_from_slice(&0x1111u16.to_le_bytes());
        record[0x34..0x36].copy_from_slice(&0x2222u16.to_le_bytes());
        record[510..512].copy_from_slice(&0xABCDu16.to_le_bytes());
        record[1022..1024].copy_from_slice(&0xABCDu16.to_le_bytes());
        let mut fixed = record.clone();
        apply_fixup(&mut fixed).unwrap();
        assert_eq!(&fixed[510..512], &0x1111u16.to_le_bytes());
        assert_eq!(&fixed[1022..1024], &0x2222u16.to_le_bytes());

        record[1022] = 0;
        assert!(apply_fixup(&mut record).is_err());
        assert!(apply_fixup(&mut [0u8; 1024]).is_err());
        let mut short = b"FILE\x30\x00\x03\x00".to_vec();
        assert!(apply_fixup(&mut short).is_err());
    }
}
