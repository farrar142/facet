// target: crates/facet-core/tests/spec_zip_archive.rs
// AC: #422: 항목 정보, 폴더 나무와 중간 폴더, 이름 인코딩(UTF-8·Unicode Path·CP949·CP437, zip 하나에 한 인코딩), 같은 경로는 뒤 기록, zip64, 오류 값과 손상 표시
//! zip 리더(`zip_archive::Archive`)가 임시 폴더의 실제 zip 파일에서 central directory를 읽어 내놓는 값을 확인한다:
//! 항목의 이름·크기·압축된 크기·시각·방식·암호화·링크, 폴더 나무와 폴더 기록 없는 중간 폴더, 이름 인코딩 판정,
//! 같은 경로의 뒤 기록, zip64(항목 7만 개, 5GB 선언), zip이 아닌 파일·잘린 파일·잘못된 오프셋·망가뜨린 바이트에서
//! 죽지 않음과 손상 표시. 시험 zip은 `spec_zip_support.rs`의 도우미로 만든다. 모든 플랫폼에서 돈다.

#[path = "spec_zip_support.rs"]
mod zip_support;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{Local, TimeZone};
use facet_core::zip_archive::{Archive, ArchiveEntry, NameEncoding};
use tempfile::TempDir;
use zip_support::{BZIP2, DEFLATE, Entry, STORED};

fn zip_at(dir: &TempDir, name: &str, entries: &[Entry]) -> PathBuf {
    let path = dir.path().join(name);
    zip_support::write_zip(&path, entries);
    path
}

fn open(path: &Path) -> Archive {
    Archive::open(path).unwrap_or_else(|e| panic!("{} 열기: {e:?}", path.display()))
}

fn names(entries: &[ArchiveEntry]) -> Vec<String> {
    let mut names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
    names.sort();
    names
}

fn child(archive: &Archive, folder: &str, name: &str) -> ArchiveEntry {
    archive
        .children(folder)
        .unwrap_or_else(|| panic!("{folder:?} 폴더가 있다"))
        .into_iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("{folder:?} 안에 {name:?}가 있다"))
}

fn unix(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

fn cp949(text: &str) -> Vec<u8> {
    let (bytes, _, unmappable) = encoding_rs::EUC_KR.encode(text);
    assert!(!unmappable, "시험 글자는 CP949로 쓸 수 있다");
    bytes.into_owned()
}

#[test]
fn reads_entry_details() {
    let dir = TempDir::new().unwrap();
    let text = zip_support::repeated_text(40);
    let path = zip_at(
        &dir,
        "자료.zip",
        &[
            Entry::stored("보고서.txt", b"stored body").modified_unix(1_700_000_000),
            Entry::deflated("압축.txt", &text),
            Entry::stored("다른 방식.bin", b"xx"),
            Entry::stored("link", b"target.txt").symlink(),
            Entry::stored("암호.txt", b"secret").zipcrypto("pw"),
        ],
    );
    // 지원하지 않는 방식은 목록에 그대로 보인다.
    let bzip_path = zip_at(
        &dir,
        "bzip.zip",
        &[Entry {
            method: BZIP2,
            ..Entry::stored("bzip.bin", b"not really bzip2")
        }],
    );

    let archive = open(&path);
    let stored = child(&archive, "", "보고서.txt");
    assert_eq!(stored.path, "보고서.txt");
    assert!(!stored.is_folder);
    assert_eq!(stored.size, 11);
    assert_eq!(stored.compressed_size, 11);
    assert_eq!(stored.method, STORED);
    assert_eq!(
        stored.modified,
        Some(unix(1_700_000_000)),
        "확장 시각 필드의 시각"
    );
    assert!(!stored.encrypted && !stored.symlink);

    let deflated = child(&archive, "", "압축.txt");
    assert_eq!(deflated.method, DEFLATE);
    assert_eq!(deflated.size, text.len() as u64);
    assert_eq!(
        deflated.compressed_size,
        zip_support::fixed_deflate(&text).len() as u64
    );
    assert!(deflated.compressed_size < deflated.size, "실제로 작아졌다");

    // 확장 시각 필드가 없으면 DOS 시각(지역 시각) 2024-05-06 07:08:10.
    let dos = child(&archive, "", "다른 방식.bin");
    let expected: SystemTime = Local
        .with_ymd_and_hms(2024, 5, 6, 7, 8, 10)
        .single()
        .expect("지역 시각")
        .into();
    assert_eq!(
        dos.modified,
        Some(expected),
        "DOS 시각을 지역 시각으로 읽는다"
    );

    assert!(
        child(&archive, "", "link").symlink,
        "유닉스 심볼릭 링크 항목"
    );
    let secret = child(&archive, "", "암호.txt");
    assert!(secret.encrypted);
    assert!(archive.encrypted(), "암호 항목이 하나라도 있다");

    let bzip = open(&bzip_path);
    assert_eq!(child(&bzip, "", "bzip.bin").method, BZIP2);
    assert!(!bzip.encrypted());
}

#[test]
fn builds_the_folder_tree_with_intermediate_folders() {
    let dir = TempDir::new().unwrap();
    let path = zip_at(
        &dir,
        "나무.zip",
        &[
            Entry::stored("문서/2024/보고서.txt", b"r"),
            Entry::stored("문서/메모.txt", b"m"),
            Entry::folder("빈 폴더"),
            Entry::folder("기록 있음"),
            Entry::stored("기록 있음/안.txt", b"i"),
            Entry::stored("맨 위.txt", b"t"),
        ],
    );
    let archive = open(&path);

    let root = archive.children("").expect("루트");
    assert_eq!(names(&root), ["기록 있음", "맨 위.txt", "문서", "빈 폴더"]);
    let docs = child(&archive, "", "문서");
    assert!(docs.is_folder, "폴더 기록 없는 중간 폴더도 폴더다");
    assert_eq!(docs.path, "문서");
    assert_eq!(docs.size, 0);

    let inside = archive.children("문서").expect("문서 폴더");
    assert_eq!(names(&inside), ["2024", "메모.txt"]);
    assert!(child(&archive, "문서", "2024").is_folder);
    let report = child(&archive, "문서/2024", "보고서.txt");
    assert_eq!(report.path, "문서/2024/보고서.txt");

    assert_eq!(
        archive.children("빈 폴더").map(|c| c.len()),
        Some(0),
        "빈 폴더는 빈 목록"
    );
    assert!(child(&archive, "", "기록 있음").is_folder);
    assert_eq!(names(&archive.children("기록 있음").unwrap()), ["안.txt"]);
    assert!(archive.children("없음").is_none(), "없는 폴더");
    assert!(
        archive.children("맨 위.txt").is_none(),
        "파일은 폴더가 아니다"
    );
    assert!(!archive.damaged());
    assert_eq!(archive.duplicates(), 0);
}

#[test]
fn reads_utf8_flag_and_unicode_path_names() {
    let dir = TempDir::new().unwrap();
    let path = zip_at(
        &dir,
        "utf8.zip",
        &[
            Entry::stored("한글 이름 🎉.txt", b"a"),
            Entry::raw(b"old-name.txt", b"b").unicode_path("유니코드 경로.txt"),
        ],
    );
    let archive = open(&path);
    assert_eq!(
        names(&archive.children("").unwrap()),
        ["유니코드 경로.txt", "한글 이름 🎉.txt"]
    );
    assert_eq!(archive.encoding(), NameEncoding::Utf8);
}

#[test]
fn reads_cp949_names_from_korean_windows() {
    let dir = TempDir::new().unwrap();
    let path = zip_at(
        &dir,
        "cp949.zip",
        &[
            Entry::raw(&cp949("보고서/2024 요약.txt"), b"a"),
            Entry::raw(&cp949("보고서/사진.jpg"), b"b"),
            Entry::raw(&cp949("읽어 주세요.txt"), b"c"),
        ],
    );
    let archive = open(&path);
    assert_eq!(archive.encoding(), NameEncoding::Cp949);
    assert_eq!(
        names(&archive.children("").unwrap()),
        ["보고서", "읽어 주세요.txt"]
    );
    assert_eq!(
        names(&archive.children("보고서").unwrap()),
        ["2024 요약.txt", "사진.jpg"]
    );
}

#[test]
fn utf8_flagged_names_stay_utf8_next_to_cp949_names() {
    let dir = TempDir::new().unwrap();
    let path = zip_at(
        &dir,
        "섞임.zip",
        &[
            Entry::stored("가나다.txt", b"a"),
            Entry::raw(&cp949("라마바.txt"), b"b"),
        ],
    );
    let archive = open(&path);
    assert_eq!(archive.encoding(), NameEncoding::Cp949);
    assert_eq!(
        names(&archive.children("").unwrap()),
        ["가나다.txt", "라마바.txt"]
    );
}

#[test]
fn falls_back_to_cp437_for_the_whole_zip() {
    let dir = TempDir::new().unwrap();
    // 0x82는 CP949의 앞 바이트인데 뒤에 '.'가 와서 CP949로 풀리지 않는다. CP437로는 é다.
    let path = zip_at(
        &dir,
        "cp437.zip",
        &[
            Entry::raw(b"caf\x82.txt", b"a"),
            Entry::raw(b"r\x82sum\x82.txt", b"b"),
        ],
    );
    let archive = open(&path);
    assert_eq!(archive.encoding(), NameEncoding::Cp437);
    assert_eq!(
        names(&archive.children("").unwrap()),
        ["café.txt", "résumé.txt"]
    );

    // 한 이름이라도 CP949로 풀리지 않으면 zip 전체가 CP437이다(항목마다 따로 고르지 않는다).
    let mixed = zip_at(
        &dir,
        "mixed.zip",
        &[
            Entry::raw(&cp949("한글.txt"), b"a"),
            Entry::raw(b"caf\x82.txt", b"b"),
        ],
    );
    let archive = open(&mixed);
    assert_eq!(archive.encoding(), NameEncoding::Cp437);
    let listed = names(&archive.children("").unwrap());
    assert!(listed.contains(&"café.txt".to_string()), "{listed:?}");
    assert!(
        !listed.contains(&"한글.txt".to_string()),
        "CP949 이름도 CP437로 읽힌다: {listed:?}"
    );
}

#[test]
fn the_later_record_wins_for_a_duplicate_path() {
    let dir = TempDir::new().unwrap();
    let path = zip_at(
        &dir,
        "겹침.zip",
        &[
            Entry::stored("같은.txt", b"first"),
            Entry::stored("다른.txt", b"x"),
            Entry::stored("같은.txt", b"second, longer"),
        ],
    );
    let archive = open(&path);
    let root = archive.children("").unwrap();
    assert_eq!(
        names(&root),
        ["같은.txt", "다른.txt"],
        "같은 경로는 하나만 보인다"
    );
    assert_eq!(child(&archive, "", "같은.txt").size, 14, "뒤에 나온 기록");
    assert_eq!(archive.duplicates(), 1);
    assert_eq!(archive.entries().len(), 3, "기록은 셋 다 읽었다");
}

#[test]
fn reads_zip64_with_seventy_thousand_entries() {
    let dir = TempDir::new().unwrap();
    let entries: Vec<Entry> = (0..70_000)
        .map(|i| Entry::stored(&format!("{i:05}.txt"), b""))
        .collect();
    let path = zip_at(&dir, "많음.zip", &entries);
    let archive = open(&path);
    assert_eq!(archive.entries().len(), 70_000);
    let root = archive.children("").unwrap();
    assert_eq!(root.len(), 70_000);
    assert!(root.iter().any(|e| e.name == "69999.txt"));
}

#[test]
fn reads_a_size_declared_beyond_four_gigabytes() {
    let dir = TempDir::new().unwrap();
    let big = Entry {
        size: 5_000_000_000,
        ..Entry::stored("백업.bin", b"")
    };
    let path = dir.path().join("큰.zip");
    zip_support::write(
        &path,
        &zip_support::build_zip64(&[Entry::stored("작은.txt", b"s"), big]),
    );
    let archive = open(&path);
    assert_eq!(child(&archive, "", "백업.bin").size, 5_000_000_000);
    assert_eq!(child(&archive, "", "작은.txt").size, 1);
}

#[test]
fn files_that_are_not_zip_or_truncated_are_error_values() {
    let dir = TempDir::new().unwrap();
    let text = dir.path().join("글.zip");
    std::fs::write(&text, "zip이 아닌 글자 파일입니다").unwrap();
    assert!(Archive::open(&text).is_err(), "zip이 아닌 파일");

    let empty = dir.path().join("빈.zip");
    std::fs::write(&empty, b"").unwrap();
    assert!(Archive::open(&empty).is_err(), "빈 파일");

    let bytes = zip_support::build(&[
        Entry::stored("a.txt", b"aaaa"),
        Entry::stored("b.txt", b"bbbb"),
    ]);
    let truncated = dir.path().join("잘림.zip");
    std::fs::write(&truncated, &bytes[..bytes.len() / 2]).unwrap();
    assert!(Archive::open(&truncated).is_err(), "끝 기록이 잘린 파일");

    assert!(
        Archive::open(&dir.path().join("없음.zip")).is_err(),
        "없는 파일"
    );
}

#[test]
fn a_wrong_central_directory_offset_does_not_panic() {
    let dir = TempDir::new().unwrap();
    let mut bytes = zip_support::build(&[Entry::stored("a.txt", b"aaaa")]);
    let len = bytes.len();
    // 끝 기록의 central directory 위치(끝에서 6바이트 앞의 4바이트)를 파일 밖으로.
    bytes[len - 6..len - 2].copy_from_slice(&0x7FFF_FFF0u32.to_le_bytes());
    let path = dir.path().join("오프셋.zip");
    std::fs::write(&path, &bytes).unwrap();
    match Archive::open(&path) {
        Err(_) => {}
        Ok(archive) => assert!(archive.damaged(), "열었다면 손상을 알린다"),
    }
}

#[test]
fn a_broken_tail_of_the_central_directory_keeps_the_readable_entries() {
    let dir = TempDir::new().unwrap();
    let mut bytes = zip_support::build(&[
        Entry::stored("첫째.txt", b"1"),
        Entry::stored("둘째.txt", b"2"),
        Entry::stored("셋째.txt", b"3"),
    ]);
    // 셋째 central directory 항목의 서명을 망가뜨린다.
    let signature = 0x0201_4B50u32.to_le_bytes();
    let positions: Vec<usize> = (0..bytes.len() - 4)
        .filter(|&i| bytes[i..i + 4] == signature)
        .collect();
    assert_eq!(positions.len(), 3);
    bytes[positions[2]] = b'X';
    let path = dir.path().join("깨짐.zip");
    std::fs::write(&path, &bytes).unwrap();

    let archive = Archive::open(&path).expect("앞 항목으로 목록을 만든다");
    assert!(archive.damaged(), "손상을 알린다");
    let listed = names(&archive.children("").unwrap());
    assert!(listed.contains(&"첫째.txt".to_string()), "{listed:?}");
    assert!(listed.contains(&"둘째.txt".to_string()), "{listed:?}");
}

#[test]
fn mangled_bytes_never_panic() {
    let dir = TempDir::new().unwrap();
    let original = zip_support::build_zip64(&[
        Entry::folder("폴더"),
        Entry::stored("폴더/a.txt", b"aaaa").modified_unix(1_600_000_000),
        Entry::deflated("b.txt", &zip_support::repeated_text(3)),
        Entry::raw(b"caf\x82.txt", b"c").unicode_path("카페.txt"),
    ]);
    let path = dir.path().join("망가짐.zip");
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    for _ in 0..300 {
        let mut bytes = original.clone();
        for _ in 0..4 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let at = (seed % bytes.len() as u64) as usize;
            bytes[at] = (seed >> 32) as u8;
        }
        std::fs::write(&path, &bytes).unwrap();
        if let Ok(archive) = Archive::open(&path) {
            // 열렸으면 나무를 훑어도 죽지 않는다.
            let _ = archive.children("");
            let _ = archive.children("폴더");
            let _ = archive.entries().len();
        }
    }
}
