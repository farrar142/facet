//! 저장소의 MFT 조각(`testdata/mft/`)이 기대값 목록과 맞는지. 관리자 권한이 필요 없다.
//!
//! 조각이 잘리거나(줄 끝 변환 등) 기대값 목록과 따로 다시 만들어지면 여기서 걸린다.

use std::path::PathBuf;

use facet_indexer::layout;

const RECORD: usize = 1024;

fn testdata(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/mft")
        .join(name)
}

struct Line {
    record: usize,
    sequence: u16,
    kind: String,
    path: String,
}

fn expected() -> Vec<Line> {
    let text = std::fs::read_to_string(testdata("ntfs-basic.expected.tsv")).unwrap();
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| {
            let cols: Vec<&str> = l.split('\t').collect();
            assert_eq!(cols.len(), 6, "열이 6개가 아님: {l}");
            Line {
                record: cols[0].parse().unwrap(),
                sequence: cols[1].parse().unwrap(),
                kind: cols[2].to_owned(),
                path: cols[3].to_owned(),
            }
        })
        .collect()
}

fn record(bytes: &[u8], n: usize) -> Vec<u8> {
    let mut r = bytes[n * RECORD..(n + 1) * RECORD].to_vec();
    layout::apply_fixup(&mut r).unwrap_or_else(|e| panic!("레코드 {n}: {e}"));
    r
}

#[test]
fn fixture_records_match_expected_list() {
    let bytes = std::fs::read(testdata("ntfs-basic.mft")).unwrap();
    assert_eq!(bytes.len() % RECORD, 0);
    let lines = expected();

    // spec이 요구한 경우가 모두 목록에 있다.
    let has = |pred: &dyn Fn(&Line) -> bool| lines.iter().any(pred);
    assert!(has(&|l| l.path.contains("한글")));
    assert!(has(&|l| l.path.contains('😀')));
    assert!(has(&|l| l.path.rsplit('\\').next().unwrap().encode_utf16().count() >= 200));
    assert!(has(&|l| l.path.matches('\\').count() >= 5));
    assert!(has(&|l| l.kind == "지운 파일"));
    assert!(has(&|l| l.kind == "확장 레코드"));
    let hard_links = lines
        .iter()
        .filter(|l| l.kind == "파일")
        .filter(|l| lines.iter().filter(|o| o.record == l.record).count() > 1)
        .count();
    assert!(hard_links >= 3);

    for l in &lines {
        let r = record(&bytes, l.record);
        let in_use = r[0x16] & 1 != 0;
        let deleted = l.kind.starts_with("지운");
        assert_eq!(in_use, !deleted, "레코드 {}({})", l.record, l.path);
        let is_dir = r[0x16] & 2 != 0;
        if l.kind == "폴더" || l.kind == "파일" {
            assert_eq!(is_dir, l.kind == "폴더", "레코드 {}({})", l.record, l.path);
            assert_eq!(
                u16::from_le_bytes([r[0x10], r[0x11]]),
                l.sequence,
                "레코드 {}의 순번",
                l.record
            );
        }
    }

    // 하드 링크가 많은 파일의 기본 레코드에 $ATTRIBUTE_LIST(0x20)가 있다.
    let base = lines
        .iter()
        .find(|l| l.kind == "확장 레코드")
        .map(|l| {
            let r = record(&bytes, l.record);
            u64::from_le_bytes(r[0x20..0x28].try_into().unwrap()) & 0xFFFF_FFFF_FFFF
        })
        .unwrap();
    let types = layout::attribute_types(&record(&bytes, base as usize)).unwrap();
    assert!(types.contains(&0x20), "{types:x?}");
}
