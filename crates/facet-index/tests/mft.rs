//! MFT 바이트 → 인덱스. 저장소의 MFT 조각(`testdata/mft/`)과 기대값 목록으로 본다.
//! 깨진 레코드는 조각 바이트를 고쳐 만든다.

use std::collections::BTreeMap;
use std::path::PathBuf;

use facet_index::{
    ATTRIBUTE_DIRECTORY, Hit, Index, MFT_ROOT_RECORD, MftError, MftParser, MftReport, Query,
};

const RECORD: usize = 1024;
const ROOT: &str = r"T:\";
const SYSTEM_VOLUME_INFORMATION: &str = r"\System Volume Information";

fn testdata(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/mft")
        .join(name)
}

fn fixture() -> Vec<u8> {
    std::fs::read(testdata("ntfs-basic.mft")).unwrap()
}

/// 기대값 목록의 한 줄.
#[derive(Debug, Clone)]
struct Line {
    record: usize,
    kind: String,
    /// `\`로 시작하는 볼륨 루트에서의 경로.
    path: String,
    size: Option<u64>,
    modified: Option<u64>,
    attributes: Option<u32>,
}

fn expected() -> Vec<Line> {
    let text = std::fs::read_to_string(testdata("ntfs-basic.expected.tsv")).unwrap();
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| {
            let c: Vec<&str> = l.split('\t').collect();
            assert_eq!(c.len(), 8, "열이 8개가 아님: {l}");
            let opt = |s: &str| (s != "-").then(|| s.to_owned());
            Line {
                record: c[0].parse().unwrap(),
                kind: c[2].to_owned(),
                path: c[3].to_owned(),
                size: opt(c[4]).map(|s| s.parse().unwrap()),
                modified: opt(c[5]).map(|s| s.parse().unwrap()),
                attributes: opt(c[6])
                    .map(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).unwrap()),
            }
        })
        .collect()
}

/// 인덱스의 항목이 되어야 하는 줄: 폴더와 파일(볼륨 루트 빼고).
fn items() -> Vec<Line> {
    expected()
        .into_iter()
        .filter(|l| (l.kind == "폴더" || l.kind == "파일") && l.path != "\\")
        .collect()
}

fn parse(bytes: &[u8]) -> (Index, MftReport) {
    Index::from_mft(ROOT, RECORD, bytes).unwrap()
}

/// 인덱스의 모든 항목: 볼륨 루트에서의 경로(`\`로 시작) → 결과.
fn all(index: &Index) -> BTreeMap<String, Hit> {
    let hits = index.query(&Query::name_contains(""));
    let n = hits.len();
    let map: BTreeMap<String, Hit> = hits
        .into_iter()
        .map(|h| (h.path[ROOT.len() - 1..].to_owned(), h))
        .collect();
    assert_eq!(map.len(), n, "같은 경로가 두 번 나옴");
    map
}

fn record_mut(bytes: &mut [u8], n: usize) -> &mut [u8] {
    &mut bytes[n * RECORD..(n + 1) * RECORD]
}

fn record_of(path: &str) -> usize {
    expected()
        .into_iter()
        .find(|l| l.path == path && (l.kind == "폴더" || l.kind == "파일"))
        .unwrap_or_else(|| panic!("기대값 목록에 없음: {path}"))
        .record
}

#[test]
fn fixture_index_matches_expected_list() {
    let (index, report) = parse(&fixture());
    assert_eq!(report.skipped, 0);
    assert_eq!(report.orphans, 0);
    assert_eq!(report.rejected, 0);

    let found = all(&index);
    let want = items();
    let want_paths: Vec<&str> = want.iter().map(|l| l.path.as_str()).collect();
    // 포맷 뒤 Windows가 스스로 만드는 `\System Volume Information`(숨김·시스템)도 사용자 항목처럼
    // 인덱스에 든다. 조각을 만든 도구가 만든 항목이 아니므로 기대값 목록에는 없다.
    let found_paths: Vec<&str> = found
        .keys()
        .map(String::as_str)
        .filter(|p| !p.starts_with(SYSTEM_VOLUME_INFORMATION))
        .collect();
    assert!(found.contains_key(SYSTEM_VOLUME_INFORMATION));
    let mut sorted = want_paths.clone();
    sorted.sort_unstable();
    assert_eq!(found_paths, sorted, "항목 경로 집합이 기대값 목록과 다름");

    for l in &want {
        let h = &found[&l.path];
        assert_eq!(h.path, format!("T:{}", l.path));
        assert_eq!(Some(h.name.as_str()), l.path.rsplit('\\').next());
        let folder = l.kind == "폴더";
        assert_eq!(h.size, l.size.unwrap_or(0), "{} 크기", l.path);
        assert_eq!(Some(h.modified), l.modified, "{} 수정 날짜", l.path);
        assert_eq!(Some(h.attributes), l.attributes, "{} 속성", l.path);
        assert_eq!(
            h.attributes & ATTRIBUTE_DIRECTORY != 0,
            folder,
            "{}",
            l.path
        );
    }

    // 한글·이모지·긴 이름이 담겼다.
    assert!(found.keys().any(|p| p.contains("한글")));
    assert!(found.keys().any(|p| p.contains("😀🎉")));
    assert!(found.values().any(|h| h.name.encode_utf16().count() == 250));
}

#[test]
fn hard_links_are_one_item_per_name_and_streams_deleted_are_not_items() {
    let (index, _) = parse(&fixture());
    let found = all(&index);
    let lines = expected();

    // 하드 링크: 같은 레코드의 이름마다 항목 하나.
    let linked = record_of(r"\하드 링크 원본.txt");
    let names: Vec<&Line> = lines.iter().filter(|l| l.record == linked).collect();
    assert_eq!(names.len(), 3);
    for l in names {
        assert_eq!(found[&l.path].size, 6);
    }

    // 대체 데이터 스트림: 파일 하나뿐이고 크기는 이름 없는 $DATA(6바이트)다.
    let streams: Vec<&String> = found.keys().filter(|p| p.contains("대체 스트림")).collect();
    assert_eq!(streams, [r"\대체 스트림.txt"]);
    assert_eq!(found[r"\대체 스트림.txt"].size, 6);
    assert!(found.keys().all(|p| !p.contains(':')));

    // 지운 레코드는 항목이 아니다.
    for l in lines.iter().filter(|l| l.kind.starts_with("지운")) {
        assert!(!found.contains_key(&l.path), "지운 항목이 나옴: {}", l.path);
    }
    assert!(index.query(&Query::name_contains("지울")).is_empty());

    // 8.3 짧은 이름과 NTFS 메타데이터 파일도 항목이 아니다.
    assert!(found.keys().all(|p| !p.contains('~')));
    assert!(found.keys().all(|p| !p.contains('$')));
}

#[test]
fn attribute_list_records_merge_names_and_size() {
    let (index, _) = parse(&fixture());
    let found = all(&index);
    let lines = expected();
    let base = lines
        .iter()
        .find(|l| l.kind == "확장 레코드")
        .map(|l| record_of(&l.path))
        .unwrap();
    let names: Vec<&Line> = lines
        .iter()
        .filter(|l| l.record == base && l.kind == "파일")
        .collect();
    assert_eq!(names.len(), 40);
    for l in names {
        let h = found
            .get(&l.path)
            .unwrap_or_else(|| panic!("확장 레코드의 이름이 없음: {}", l.path));
        assert_eq!(h.size, 4);
        assert_eq!(Some(h.modified), l.modified);
    }
    assert_eq!(
        index.query(&Query::name_contains("속성 많은 파일")).len(),
        40
    );
}

#[test]
fn broken_records_are_skipped_and_counted() {
    let clean = all(&parse(&fixture()).0);
    let mut bytes = fixture();

    // 서명 틀림.
    let bad_signature = r"\한글 폴더\문서.txt";
    record_mut(&mut bytes, record_of(bad_signature))[..4].copy_from_slice(b"BAAD");
    // 길이 넘침: 사용 중 길이가 레코드 크기를 넘는다.
    let overflow = r"\빈 파일.txt";
    record_mut(&mut bytes, record_of(overflow))[0x18..0x1C]
        .copy_from_slice(&(RECORD as u32 + 8).to_le_bytes());
    // 고정값 틀림: 첫 섹터 끝 2바이트가 갱신 순서 번호와 다르다.
    let bad_fixup = r"\큰 파일.bin";
    let r = record_mut(&mut bytes, record_of(bad_fixup));
    r[510] ^= 0xFF;
    // 속성 길이 넘침: 첫 속성의 길이가 레코드 밖까지 간다.
    let attribute_overflow = r"\이모지 😀🎉 파일.txt";
    let r = record_mut(&mut bytes, record_of(attribute_overflow));
    let first = usize::from(u16::from_le_bytes([r[0x14], r[0x15]]));
    r[first + 4..first + 8].copy_from_slice(&0x1000u32.to_le_bytes());

    let (index, report) = parse(&bytes);
    assert_eq!(report.skipped, 4);
    assert_eq!(report.orphans, 0);
    let found = all(&index);
    let broken = [bad_signature, overflow, bad_fixup, attribute_overflow];
    for p in broken {
        assert!(!found.contains_key(p), "깨진 레코드가 나옴: {p}");
    }
    let rest: BTreeMap<_, _> = clean
        .into_iter()
        .filter(|(p, _)| !broken.contains(&p.as_str()))
        .collect();
    assert_eq!(found, rest, "나머지 항목이 그대로 나오지 않음");
}

#[test]
fn items_whose_parent_cannot_be_found_are_dropped_and_counted() {
    let clean = all(&parse(&fixture()).0);
    let mut bytes = fixture();

    // `\깊은 폴더\1단계` 레코드를 지운 것으로 만든다: 그 아래 2단계·3단계·4단계·끝.txt는 부모를
    // 볼륨 루트까지 따라갈 수 없다.
    let lost = r"\깊은 폴더\1단계";
    record_mut(&mut bytes, record_of(lost))[0x16] &= !1;
    // 다른 파일 하나는 부모 참조를 쓰지 않은 레코드(200번)로 바꾼다.
    let moved = r"\빈 파일.txt";
    let r = record_mut(&mut bytes, record_of(moved));
    let fname = file_name_value(r);
    r[fname..fname + 8].copy_from_slice(&(200u64 | (1 << 48)).to_le_bytes());

    let (index, report) = parse(&bytes);
    assert_eq!(report.skipped, 0);
    assert_eq!(report.orphans, 5);
    let found = all(&index);
    let gone: Vec<&String> = clean.keys().filter(|p| !found.contains_key(*p)).collect();
    assert_eq!(
        gone,
        [
            r"\깊은 폴더\1단계",
            r"\깊은 폴더\1단계\2단계",
            r"\깊은 폴더\1단계\2단계\3단계",
            r"\깊은 폴더\1단계\2단계\3단계\4단계",
            r"\깊은 폴더\1단계\2단계\3단계\4단계\끝.txt",
            r"\빈 파일.txt",
        ]
    );
}

/// 고정값을 되돌리지 않은 레코드에서 첫 `$FILE_NAME` 값의 위치. 시험 레코드는 첫 섹터 안에 있다.
fn file_name_value(r: &[u8]) -> usize {
    let mut at = usize::from(u16::from_le_bytes([r[0x14], r[0x15]]));
    loop {
        let kind = u32::from_le_bytes(r[at..at + 4].try_into().unwrap());
        let len = u32::from_le_bytes(r[at + 4..at + 8].try_into().unwrap()) as usize;
        if kind == 0x30 {
            let value = usize::from(u16::from_le_bytes([r[at + 0x14], r[at + 0x15]]));
            assert!(at + value + 8 < 510);
            return at + value;
        }
        at += len;
    }
}

#[test]
fn chunks_in_any_order_build_the_same_index() {
    let bytes = fixture();
    let whole = all(&parse(&bytes).0);
    let mut parser = MftParser::new(RECORD).unwrap();
    let split = 60 * RECORD;
    parser.add(60, &bytes[split..]);
    parser.add(0, &bytes[..split]);
    let (index, report) = parser.finish(ROOT);
    assert_eq!(report.skipped, 0);
    assert_eq!(all(&index), whole);
}

#[test]
fn truncated_chunk_counts_the_cut_record() {
    let bytes = fixture();
    let (index, report) = parse(&bytes[..bytes.len() - 100]);
    assert_eq!(report.skipped, 1);
    assert_eq!(all(&index), all(&parse(&bytes).0));
}

#[test]
fn record_size_must_be_a_sane_power_of_two() {
    for bad in [0, 1, 256, 1000, 1536, 131072] {
        assert_eq!(
            MftParser::new(bad).unwrap_err(),
            MftError::RecordSize(bad),
            "{bad}"
        );
    }
    for good in [512, 1024, 4096] {
        assert!(MftParser::new(good).is_ok());
    }
    assert_eq!(MFT_ROOT_RECORD, 5);
}

/// 작은 의사 난수(xorshift). 같은 씨앗이면 늘 같은 열이다.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[test]
fn arbitrary_bytes_do_not_panic() {
    let bytes = fixture();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);

    // 잘린 덩어리: 여러 길이로 자른다.
    for _ in 0..200 {
        let cut = rng.below(bytes.len());
        let _ = parse(&bytes[..cut]);
        let _ = parse(&bytes[cut..]);
    }

    // 무작위 값: 서명과 고정값이 맞는 레코드도 섞이도록 앞부분은 "FILE"로 시작하게 한다.
    for size in [512, 1024, 4096] {
        for _ in 0..200 {
            let len = rng.below(size * 4 + 1);
            let mut junk: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            if len >= 4 && rng.below(2) == 0 {
                junk[..4].copy_from_slice(b"FILE");
            }
            let _ = Index::from_mft(ROOT, size, &junk).unwrap();
        }
    }

    // 조각의 바이트를 무작위로 바꾼다. 고정값이 맞는 레코드의 속성 안쪽이 바뀌는 경우가 대부분이다.
    for _ in 0..300 {
        let mut b = bytes.clone();
        for _ in 0..1 + rng.below(64) {
            let at = rng.below(b.len());
            b[at] = rng.next() as u8;
        }
        let (index, report) = parse(&b);
        let _ = (index.len(), report);
        let _ = index.query(&Query::name_contains(""));
    }
}
