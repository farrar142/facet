//! 첫 인덱스 만들기: 볼륨의 MFT 전체를 큰 덩어리로 순서대로 읽어 인덱스를 만든다. 볼륨 읽기를
//! [`ReadAt`]로 받으므로 플랫폼과 상관없이 테스트한다. 실제 볼륨은 [`crate::volume`]이 연다.

use facet_index::{Index, MftParser, MftReport};

use crate::dump::{self, Geometry, ReadAt};

/// 볼륨 루트 경로가 `root`(예: `C:`)인 인덱스를 MFT 전체에서 만든다.
///
/// MFT를 [`dump::CHUNK`] 크기 덩어리로 처음부터 차례로 읽어 파서에 넘긴다. 덩어리 경계가
/// 레코드 중간에 걸리면(클러스터가 레코드보다 작은 볼륨) 남은 조각을 다음 덩어리와 이어 붙인다.
/// 깨진 레코드는 파서가 건너뛰고 [`MftReport::skipped`]에 센다.
pub fn build_index(
    volume: &mut impl ReadAt,
    geometry: &Geometry,
    root: &str,
) -> Result<(Index, MftReport), String> {
    let record = geometry.bytes_per_record as usize;
    let mut parser = MftParser::new(record).map_err(|e| e.to_string())?;
    let mut carry: Vec<u8> = Vec::new();
    let mut next_record = 0u64;
    let mut feed = |chunk: &[u8]| {
        let bytes: &[u8] = if carry.is_empty() {
            chunk
        } else {
            carry.extend_from_slice(chunk);
            &carry
        };
        let whole = bytes.len() / record * record;
        parser.add(next_record, &bytes[..whole]);
        next_record += (whole / record) as u64;
        let rest = bytes[whole..].to_vec();
        carry = rest;
        Ok(())
    };
    dump::read_mft(volume, geometry, &mut feed)?;
    drop(feed);
    if !carry.is_empty() {
        // 유효 데이터 길이가 레코드 크기의 배수가 아니었다. 잘린 레코드로 센다.
        parser.add(next_record, &carry);
    }
    Ok(parser.finish(root))
}
