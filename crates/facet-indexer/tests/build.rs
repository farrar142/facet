//! 첫 인덱스 만들기([`build_index`]): 볼륨에서 MFT 전체를 덩어리로 읽어 만든 인덱스가 MFT
//! 바이트를 그대로 파서에 넣은 것과 같은지. 볼륨은 저장소의 MFT 조각(`testdata/mft/`)을 레코드
//! 0의 위치 목록대로 흩어 놓은 메모리 볼륨이다. 관리자 권한이 필요 없다.

use std::io;
use std::path::PathBuf;

use facet_index::{Index, Query};
use facet_indexer::build::build_index;
use facet_indexer::dump::{Geometry, ReadAt};
use facet_indexer::layout;

const RECORD: usize = 1024;

fn fixture() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/mft/ntfs-basic.mft");
    std::fs::read(path).unwrap()
}

/// 드문드문 채운 볼륨: (위치, 바이트) 조각들. 조각 밖은 0이다.
struct Sparse(Vec<(u64, Vec<u8>)>);

impl ReadAt for Sparse {
    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        buf.fill(0);
        let end = offset + buf.len() as u64;
        for (at, bytes) in &self.0 {
            let piece_end = at + bytes.len() as u64;
            let from = offset.max(*at);
            let to = end.min(piece_end);
            if from < to {
                let src = &bytes[(from - at) as usize..(to - at) as usize];
                buf[(from - offset) as usize..(to - offset) as usize].copy_from_slice(src);
            }
        }
        Ok(())
    }
}

/// MFT 조각을 레코드 0의 데이터 런이 가리키는 클러스터에 놓은 볼륨과 그 기하 정보.
fn volume(mft: &[u8], cluster: u32) -> (Sparse, Geometry) {
    let mut record0 = mft[..RECORD].to_vec();
    layout::apply_fixup(&mut record0).unwrap();
    let extents = layout::mft_extents(&record0).unwrap();
    let cs = u64::from(cluster);
    let mut pieces = Vec::new();
    for e in &extents {
        let from = (e.vcn * cs) as usize;
        if from >= mft.len() {
            continue;
        }
        let to = (((e.vcn + e.clusters) * cs) as usize).min(mft.len());
        pieces.push((e.lcn * cs, mft[from..to].to_vec()));
    }
    let geometry = Geometry {
        bytes_per_sector: 512,
        bytes_per_cluster: cluster,
        bytes_per_record: RECORD as u32,
        mft_start_lcn: extents[0].lcn,
        mft_valid_data_length: mft.len() as u64,
    };
    (Sparse(pieces), geometry)
}

fn everything(index: &Index) -> Vec<(String, u64, u64, u32)> {
    let mut all: Vec<_> = index
        .query(&Query::name_contains(""))
        .into_iter()
        .map(|h| (h.path, h.size, h.modified, h.attributes))
        .collect();
    all.sort();
    all
}

#[test]
fn index_built_from_volume_matches_parsing_the_mft_bytes() {
    let mft = fixture();
    let (expected, expected_report) = Index::from_mft("T:", RECORD, &mft).unwrap();
    // 시험 VHD(64MB NTFS)의 클러스터는 4096바이트다.
    let (mut disk, geometry) = volume(&mft, 4096);
    let (built, report) = build_index(&mut disk, &geometry, "T:").unwrap();
    assert_eq!(report, expected_report);
    assert!(built.len() > 50, "항목이 {}개뿐임", built.len());
    assert_eq!(everything(&built), everything(&expected));
    assert!(
        everything(&built)
            .iter()
            .any(|(p, ..)| p == r"T:\한글 폴더\문서.txt")
    );
}

#[test]
fn broken_mft_record_zero_is_an_error_not_a_panic() {
    // 위치 목록은 깨지기 전 레코드로 정하고, 볼륨에 놓은 레코드 0의 서명만 깨뜨린다.
    let (mut disk, geometry) = volume(&fixture(), 4096);
    for (at, bytes) in &mut disk.0 {
        if *at == geometry.mft_start_lcn * 4096 {
            bytes[0] = b'X';
        }
    }
    assert!(build_index(&mut disk, &geometry, "T:").is_err());
}

#[test]
fn unreadable_volume_is_an_error() {
    struct Failing;
    impl ReadAt for Failing {
        fn read_exact_at(&mut self, _: u64, _: &mut [u8]) -> io::Result<()> {
            Err(io::Error::other("디스크 오류"))
        }
    }
    let geometry = Geometry {
        bytes_per_sector: 512,
        bytes_per_cluster: 4096,
        bytes_per_record: RECORD as u32,
        mft_start_lcn: 4,
        mft_valid_data_length: 64 * RECORD as u64,
    };
    let err = build_index(&mut Failing, &geometry, "T:").unwrap_err();
    assert!(err.contains("디스크 오류"), "{err}");
}
