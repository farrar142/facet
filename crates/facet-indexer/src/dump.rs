//! MFT 읽기와 떠 내기의 절차. 볼륨 읽기를 [`ReadAt`]로 받으므로 플랫폼과 상관없이 테스트한다.
//! 실제 볼륨은 [`crate::volume`]이 이 트레이트를 구현한다.

use std::io::{self, Write};

use crate::layout::{self, Extent};

/// 한 번에 읽는 최대 바이트 수. MFT를 레코드 하나씩이 아니라 덩어리째 읽는다.
pub const CHUNK: usize = 1 << 20;

/// 볼륨의 지정한 위치에서 정확히 `buf.len()` 바이트를 읽는다.
pub trait ReadAt {
    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()>;
}

/// 떠 내기에 필요한 NTFS 볼륨의 기하 정보(`FSCTL_GET_NTFS_VOLUME_DATA`에서 얻는다).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub bytes_per_sector: u32,
    pub bytes_per_cluster: u32,
    pub bytes_per_record: u32,
    pub mft_start_lcn: u64,
    /// MFT에서 실제로 쓰인 데이터의 길이. 이 뒤의 레코드는 떠 내지 않는다.
    pub mft_valid_data_length: u64,
}

/// 떠 낸 결과.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub start: u64,
    pub records: u64,
    pub bytes_per_record: u32,
    /// MFT의 유효 데이터 안에 있는 레코드 수.
    pub total_records: u64,
}

/// 레코드 `start`부터 `count`개(없으면 끝까지)를 디스크 그대로 `out`에 쓴다.
pub fn dump_mft(
    volume: &mut impl ReadAt,
    geometry: &Geometry,
    start: u64,
    count: Option<u64>,
    out: &mut impl Write,
) -> Result<Summary, String> {
    check_geometry(geometry)?;
    let rs = u64::from(geometry.bytes_per_record);
    let extents = read_extents(volume, geometry)?;

    let total_records = geometry.mft_valid_data_length / rs;
    if start > total_records {
        return Err(format!(
            "--start {start}가 MFT의 레코드 수 {total_records}보다 큼"
        ));
    }
    let records = match count {
        Some(n) if n > total_records - start => {
            return Err(format!(
                "레코드 {start}부터 {n}개를 요청했지만 MFT에는 {total_records}개뿐임"
            ));
        }
        Some(n) => n,
        None => total_records - start,
    };

    let mut write = |bytes: &[u8]| {
        out.write_all(bytes)
            .map_err(|e| format!("출력 파일에 쓰지 못함: {e}"))
    };
    read_records(
        volume,
        geometry,
        &extents,
        start * rs,
        (start + records) * rs,
        &mut write,
    )?;
    out.flush()
        .map_err(|e| format!("출력 파일에 쓰지 못함: {e}"))?;
    Ok(Summary {
        start,
        records,
        bytes_per_record: geometry.bytes_per_record,
        total_records,
    })
}

/// MFT의 유효 데이터 전체를 처음부터 순서대로 [`CHUNK`] 크기 이하의 덩어리로 읽어 `sink`에 넘긴다.
/// 덩어리는 MFT 순서로 이어지고, 다 이으면 레코드 0부터 유효 데이터 끝까지의 바이트다.
/// 레코드 경계와 덩어리 경계는 맞지 않을 수 있다(클러스터가 레코드보다 작을 때).
///
/// 돌려주는 값은 읽은 레코드 수다.
pub fn read_mft(
    volume: &mut impl ReadAt,
    geometry: &Geometry,
    sink: &mut dyn FnMut(&[u8]) -> Result<(), String>,
) -> Result<u64, String> {
    check_geometry(geometry)?;
    let extents = read_extents(volume, geometry)?;
    let rs = u64::from(geometry.bytes_per_record);
    let records = geometry.mft_valid_data_length / rs;
    read_records(volume, geometry, &extents, 0, records * rs, sink)?;
    Ok(records)
}

fn check_geometry(geometry: &Geometry) -> Result<(), String> {
    let rs = u64::from(geometry.bytes_per_record);
    let cs = u64::from(geometry.bytes_per_cluster);
    let ss = u64::from(geometry.bytes_per_sector);
    if rs == 0 || cs == 0 || ss == 0 || !rs.is_multiple_of(ss) {
        return Err(format!("지원하지 않는 볼륨 기하 정보: {geometry:?}"));
    }
    Ok(())
}

/// MFT 안의 바이트 범위 `[from, to)`를 덩어리로 읽어 차례로 `sink`에 넘긴다.
fn read_records(
    volume: &mut impl ReadAt,
    geometry: &Geometry,
    extents: &[Extent],
    from: u64,
    to: u64,
    sink: &mut dyn FnMut(&[u8]) -> Result<(), String>,
) -> Result<(), String> {
    let cs = u64::from(geometry.bytes_per_cluster);
    let ss = u64::from(geometry.bytes_per_sector);
    let reads = layout::plan_reads(extents, cs, from, to, CHUNK)?;
    let mut buf = vec![0u8; CHUNK];
    for read in reads {
        if !read.offset.is_multiple_of(ss) || !(read.len as u64).is_multiple_of(ss) {
            return Err(format!("섹터에 맞지 않는 읽기: {read:?}"));
        }
        let chunk = &mut buf[..read.len];
        volume
            .read_exact_at(read.offset, chunk)
            .map_err(|e| format!("볼륨의 {}바이트 위치를 읽지 못함: {e}", read.offset))?;
        sink(chunk)?;
    }
    Ok(())
}

/// 레코드 0(`$MFT`)을 읽어 MFT의 위치 목록을 얻는다.
fn read_extents(volume: &mut impl ReadAt, geometry: &Geometry) -> Result<Vec<Extent>, String> {
    let offset = geometry
        .mft_start_lcn
        .checked_mul(u64::from(geometry.bytes_per_cluster))
        .ok_or("MFT 시작 위치가 범위를 벗어남")?;
    let mut record = vec![0u8; geometry.bytes_per_record as usize];
    volume
        .read_exact_at(offset, &mut record)
        .map_err(|e| format!("$MFT 레코드를 읽지 못함: {e}"))?;
    layout::apply_fixup(&mut record).map_err(|e| format!("$MFT 레코드가 깨짐: {e}"))?;
    layout::mft_extents(&record)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 메모리 위의 볼륨.
    struct Memory(Vec<u8>);

    impl ReadAt for Memory {
        fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
            let at = offset as usize;
            let src = self
                .0
                .get(at..at + buf.len())
                .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
            buf.copy_from_slice(src);
            Ok(())
        }
    }

    const CLUSTER: usize = 4096;
    const RECORD: usize = 1024;

    /// 레코드 `n`: 서명과 고정값을 갖추고, 본문 자리에 번호를 적는다.
    fn record(n: u32, data_runs: Option<&[u8]>) -> Vec<u8> {
        let mut r = vec![0u8; RECORD];
        r[0..4].copy_from_slice(b"FILE");
        r[4..6].copy_from_slice(&0x30u16.to_le_bytes());
        r[6..8].copy_from_slice(&3u16.to_le_bytes());
        r[0x14..0x16].copy_from_slice(&0x38u16.to_le_bytes());
        r[0x18..0x1C].copy_from_slice(&(RECORD as u32).to_le_bytes());
        r[0x2C..0x30].copy_from_slice(&n.to_le_bytes());
        let mut at = 0x38;
        if let Some(runs) = data_runs {
            let len = (0x40 + runs.len()).next_multiple_of(8);
            r[at..at + 4].copy_from_slice(&0x80u32.to_le_bytes());
            r[at + 4..at + 8].copy_from_slice(&(len as u32).to_le_bytes());
            r[at + 8] = 1;
            r[at + 0x20..at + 0x22].copy_from_slice(&0x40u16.to_le_bytes());
            r[at + 0x40..at + 0x40 + runs.len()].copy_from_slice(runs);
            at += len;
        }
        r[at..at + 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        // 고정값: 각 512바이트 끝의 원래 값을 배열에 옮기고 USN을 적는다.
        r[0x30..0x32].copy_from_slice(&7u16.to_le_bytes());
        for i in 1..3 {
            let end = i * 512 - 2;
            let original = [r[end], r[end + 1]];
            r[0x30 + 2 * i..0x32 + 2 * i].copy_from_slice(&original);
            r[end..end + 2].copy_from_slice(&7u16.to_le_bytes());
        }
        r
    }

    /// MFT가 두 조각(클러스터 10..12, 클러스터 3..5)에 나뉜 볼륨. 레코드는 16개.
    fn fragmented_volume() -> (Memory, Geometry, Vec<Vec<u8>>) {
        // 2 클러스터를 10에, 다음 2 클러스터를 3(= 10 - 7)에.
        let runs = [0x11, 0x02, 0x0A, 0x11, 0x02, 0xF9, 0x00];
        let records: Vec<Vec<u8>> = (0..16)
            .map(|n| record(n, (n == 0).then_some(&runs[..])))
            .collect();
        let mut disk = vec![0u8; 16 * CLUSTER];
        let per_cluster = CLUSTER / RECORD;
        for (n, r) in records.iter().enumerate() {
            let lcn = if n < 2 * per_cluster { 10 } else { 3 - 2 };
            let at = (lcn * CLUSTER) + n * RECORD;
            disk[at..at + RECORD].copy_from_slice(r);
        }
        let geometry = Geometry {
            bytes_per_sector: 512,
            bytes_per_cluster: CLUSTER as u32,
            bytes_per_record: RECORD as u32,
            mft_start_lcn: 10,
            mft_valid_data_length: (16 * RECORD) as u64,
        };
        (Memory(disk), geometry, records)
    }

    #[test]
    fn dumps_records_in_mft_order_across_fragments() {
        let (mut volume, geometry, records) = fragmented_volume();
        let mut out = Vec::new();
        let summary = dump_mft(&mut volume, &geometry, 0, None, &mut out).unwrap();
        assert_eq!(summary.records, 16);
        assert_eq!(summary.total_records, 16);
        assert_eq!(out, records.concat());

        let mut part = Vec::new();
        dump_mft(&mut volume, &geometry, 6, Some(4), &mut part).unwrap();
        assert_eq!(part, records[6..10].concat());
    }

    #[test]
    fn out_of_range_requests_and_broken_mft_record_are_errors() {
        let (mut volume, geometry, _) = fragmented_volume();
        let mut out = Vec::new();
        assert!(dump_mft(&mut volume, &geometry, 17, None, &mut out).is_err());
        assert!(dump_mft(&mut volume, &geometry, 10, Some(7), &mut out).is_err());

        let (Memory(mut disk), geometry, _) = fragmented_volume();
        disk[10 * CLUSTER] = b'B';
        assert!(dump_mft(&mut Memory(disk), &geometry, 0, None, &mut out).is_err());
    }
}
