//! `cargo xtask mft-fixture`: MFT 파서 테스트에 쓰는 MFT 조각과 그 기대값 목록을 만든다.
//!
//! 시험 VHD를 붙이고, spec(#22)이 요구하는 항목을 만들고, `facet-indexer dump-mft`로 MFT를
//! 떠서 `testdata/mft/`에 쓴다. 관리자 권한이 필요하다. 형식과 손으로 따라 하는 방법은
//! `testdata/mft/README.md`에 있다.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use facet_indexer::layout;

use crate::vhd::{self, TestVhd};

/// 조각 파일 이름(확장자 제외).
pub const FIXTURE: &str = "ntfs-basic";
/// 레코드 크기. 시험 VHD는 기본값(1024)으로 포맷된다.
const RECORD: usize = 1024;
/// 속성 목록을 만들려고 한 파일에 거는 하드 링크 수.
const MANY_LINKS: usize = 40;
/// `$ATTRIBUTE_LIST` 속성 형식.
const ATTRIBUTE_LIST: u32 = 0x20;

/// 기대값 목록의 한 줄.
struct Entry {
    record: u64,
    sequence: u16,
    kind: &'static str,
    /// 볼륨 루트에서의 경로(`\`로 시작).
    path: String,
    size: Option<u64>,
    note: String,
}

/// 만든 항목과, 조각을 뜬 뒤 확인할 레코드 번호.
struct Made {
    entries: Vec<Entry>,
    attribute_list_record: u64,
    deleted_records: Vec<u64>,
}

pub fn run(root: &Path) -> Result<(), String> {
    if !vhd::is_admin() {
        return Err(
            "관리자 권한이 필요합니다(시험 VHD를 만들고 볼륨 핸들을 엶). 관리자 터미널에서 다시 실행하세요"
                .to_owned(),
        );
    }
    let vhd = TestVhd::create()?;
    println!("xtask mft-fixture: 시험 VHD를 {}에 붙임", vhd.root());
    let volume = vhd.root();
    enable_short_names(vhd.letter)?;
    let made = populate(&volume)?;

    let out_dir = root.join("testdata").join("mft");
    fs::create_dir_all(&out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let mft_path = out_dir.join(format!("{FIXTURE}.mft"));
    dump(root, vhd.letter, &mft_path)?;
    let bytes = fs::read(&mft_path).map_err(|e| format!("{}: {e}", mft_path.display()))?;
    check(&bytes, &made)?;
    let mut entries = made.entries;
    entries.extend(extension_records(&bytes, &entries));

    let tsv_path = out_dir.join(format!("{FIXTURE}.expected.tsv"));
    fs::write(&tsv_path, expected_tsv(&entries, bytes.len() / RECORD))
        .map_err(|e| format!("{}: {e}", tsv_path.display()))?;
    println!(
        "xtask mft-fixture: {}와 {}를 씀",
        mft_path.display(),
        tsv_path.display()
    );
    Ok(())
}

/// 8.3 짧은 이름 만들기를 켠다. 실제 시스템 볼륨처럼 DOS 이름공간의 `$FILE_NAME`이 생긴다.
fn enable_short_names(letter: char) -> Result<(), String> {
    command(Command::new("fsutil").args(["8dot3name", "set", &format!("{letter}:"), "0"])).map(drop)
}

fn populate(volume: &str) -> Result<Made, String> {
    let at = |rel: &str| PathBuf::from(format!("{volume}{rel}"));
    let mut entries = Vec::new();
    let mut add = |rel: &str, kind: &'static str, note: &str| -> Result<(u64, u16), String> {
        let path = at(rel);
        let (record, sequence) = file_id(&path)?;
        let size = if kind == "파일" {
            Some(
                fs::metadata(&path)
                    .map_err(|e| format!("{}: {e}", path.display()))?
                    .len(),
            )
        } else {
            None
        };
        entries.push(Entry {
            record,
            sequence,
            kind,
            path: format!("\\{rel}"),
            size,
            note: note.to_owned(),
        });
        Ok((record, sequence))
    };
    let mkdir = |rel: &str| fs::create_dir_all(at(rel)).map_err(|e| format!("{rel}: {e}"));
    let write =
        |rel: &str, data: &[u8]| fs::write(at(rel), data).map_err(|e| format!("{rel}: {e}"));
    let link = |from: &str, to: &str| {
        fs::hard_link(at(from), at(to)).map_err(|e| format!("{from} → {to}: {e}"))
    };

    // 볼륨 루트(레코드 5).
    add("", "폴더", "볼륨 루트")?;

    // 한글 이름.
    mkdir("한글 폴더")?;
    write("한글 폴더\\문서.txt", "안녕하세요".as_bytes())?;
    add("한글 폴더", "폴더", "한글 이름")?;
    add("한글 폴더\\문서.txt", "파일", "한글 이름, 상주 $DATA")?;

    // 이모지(UTF-16 대리 쌍).
    write("이모지 😀🎉 파일.txt", "🎉".as_bytes())?;
    add(
        "이모지 😀🎉 파일.txt",
        "파일",
        "이모지 이름(UTF-16 대리 쌍)",
    )?;

    // 긴 이름: UTF-16 250자(한계 255자).
    let long = long_name(250);
    write(&long, b"long")?;
    add(&long, "파일", "긴 이름(UTF-16 250자), 8.3 짧은 이름 있음")?;

    // 여러 단계 폴더.
    let deep = "깊은 폴더\\1단계\\2단계\\3단계\\4단계";
    mkdir(deep)?;
    let mut prefix = String::new();
    for part in deep.split('\\') {
        if !prefix.is_empty() {
            prefix.push('\\');
        }
        prefix.push_str(part);
        add(&prefix, "폴더", "여러 단계 폴더")?;
    }
    write(&format!("{deep}\\끝.txt"), b"end")?;
    add(
        &format!("{deep}\\끝.txt"),
        "파일",
        "여러 단계 폴더의 맨 아래",
    )?;

    // 빈 파일과 비상주 $DATA.
    write("빈 파일.txt", b"")?;
    add("빈 파일.txt", "파일", "크기 0")?;
    let big: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    write("큰 파일.bin", &big)?;
    add("큰 파일.bin", "파일", "비상주 $DATA")?;

    // 하드 링크: 레코드 하나에 이름 셋(서로 다른 폴더).
    write("하드 링크 원본.txt", b"linked")?;
    link("하드 링크 원본.txt", "한글 폴더\\하드 링크 사본.txt")?;
    link("하드 링크 원본.txt", "깊은 폴더\\하드 링크 셋째.txt")?;
    for rel in [
        "하드 링크 원본.txt",
        "한글 폴더\\하드 링크 사본.txt",
        "깊은 폴더\\하드 링크 셋째.txt",
    ] {
        add(rel, "파일", "하드 링크(이름 3개)")?;
    }

    // 대체 데이터 스트림: 항목이 아니다. 본문 크기는 이름 없는 $DATA의 크기다.
    write("대체 스트림.txt", "본문".as_bytes())?;
    write("대체 스트림.txt:숨은 스트림", &[7u8; 3000])?;
    write(
        "대체 스트림.txt:Zone.Identifier",
        b"[ZoneTransfer]\r\nZoneId=3\r\n",
    )?;
    add(
        "대체 스트림.txt",
        "파일",
        "대체 데이터 스트림 2개(숨은 스트림 3000바이트 비상주, Zone.Identifier)",
    )?;

    // 속성이 많아 $ATTRIBUTE_LIST와 확장 레코드가 생기는 파일: 하드 링크마다 $FILE_NAME이 하나씩 붙는다.
    mkdir("링크 많음")?;
    let first = "링크 많음\\속성 많은 파일.txt";
    write(first, b"many")?;
    let (attribute_list_record, _) = add(first, "파일", "")?;
    let padding = "가".repeat(100);
    for n in 1..MANY_LINKS {
        let rel = format!("링크 많음\\속성 많은 파일 {n:02} {padding}.txt");
        link(first, &rel)?;
        add(&rel, "파일", "")?;
    }
    add("링크 많음", "폴더", "")?;
    let many_note = format!("$ATTRIBUTE_LIST와 확장 레코드(하드 링크 {MANY_LINKS}개)");
    for e in entries
        .iter_mut()
        .filter(|e| e.record == attribute_list_record)
    {
        e.note.clone_from(&many_note);
    }

    // 지운 파일과 지운 폴더: 레코드 번호를 얻은 뒤 지운다. 다른 항목을 다 만든 뒤라 레코드가 다시 쓰이지 않는다.
    write("지울 파일.txt", b"bye")?;
    mkdir("지울 폴더")?;
    write("지울 폴더\\안의 파일.txt", b"bye")?;
    let mut deleted = Vec::new();
    for (rel, kind) in [
        ("지울 파일.txt", "지운 파일"),
        ("지울 폴더\\안의 파일.txt", "지운 파일"),
        ("지울 폴더", "지운 폴더"),
    ] {
        let (record, sequence) = file_id(&at(rel))?;
        deleted.push(Entry {
            record,
            sequence,
            kind,
            path: format!("\\{rel}"),
            size: None,
            note: "레코드 사용 중 표시가 꺼짐".to_owned(),
        });
    }
    fs::remove_dir_all(at("지울 폴더")).map_err(|e| format!("지울 폴더: {e}"))?;
    fs::remove_file(at("지울 파일.txt")).map_err(|e| format!("지울 파일.txt: {e}"))?;
    let deleted_records = deleted.iter().map(|e| e.record).collect();
    entries.extend(deleted);

    Ok(Made {
        entries,
        attribute_list_record,
        deleted_records,
    })
}

/// UTF-16 `units`자 길이의 파일 이름(확장자 포함). 모두 기본 다국어 평면 글자다.
fn long_name(units: usize) -> String {
    let base: String = "긴 이름 "
        .chars()
        .chain("가나다라마바사아자차카타파하".chars().cycle())
        .take(units - 4)
        .collect();
    format!("{base}.txt")
}

/// 파일 ID에서 (레코드 번호, 순번)을 얻는다. NTFS 파일 ID의 아래 48비트가 레코드 번호다.
fn file_id(path: &Path) -> Result<(u64, u16), String> {
    let out = command(
        Command::new("fsutil")
            .args(["file", "queryfileid"])
            .arg(path),
    )?;
    let hex = out
        .split("0x")
        .nth(1)
        .map(str::trim)
        .ok_or_else(|| format!("fsutil 출력을 해석하지 못함: {out}"))?;
    let low = &hex[hex.len().saturating_sub(16)..];
    let id = u64::from_str_radix(low, 16).map_err(|e| format!("파일 ID {hex}: {e}"))?;
    Ok((id & 0xFFFF_FFFF_FFFF, (id >> 48) as u16))
}

fn command(cmd: &mut Command) -> Result<String, String> {
    let out = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        Ok(stdout)
    } else {
        Err(format!("{cmd:?} 실패({}): {stdout}", out.status))
    }
}

fn dump(root: &Path, letter: char, out: &Path) -> Result<(), String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let status = Command::new(cargo)
        .args(["run", "--locked", "--quiet", "-p", "facet-indexer", "--"])
        .args(["dump-mft", &format!("{letter}:")])
        .arg(out)
        .current_dir(root)
        .status()
        .map_err(|e| format!("facet-indexer를 실행하지 못함: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("dump-mft 실패({status})"))
    }
}

/// 떠 낸 조각에 의도한 경우가 실제로 들어갔는지 확인한다.
fn check(bytes: &[u8], made: &Made) -> Result<(), String> {
    let record = |n: u64| -> Result<Vec<u8>, String> {
        let at = n as usize * RECORD;
        let mut r = bytes
            .get(at..at + RECORD)
            .ok_or_else(|| format!("레코드 {n}이 조각 밖에 있음"))?
            .to_vec();
        layout::apply_fixup(&mut r).map_err(|e| format!("레코드 {n}: {e}"))?;
        Ok(r)
    };
    let in_use = |r: &[u8]| r[0x16] & 1 != 0;

    for e in &made.entries {
        let r = record(e.record)?;
        let deleted = made.deleted_records.contains(&e.record);
        if in_use(&r) == deleted {
            return Err(format!(
                "레코드 {}({})의 사용 중 표시가 기대와 다름",
                e.record, e.path
            ));
        }
    }
    let types = layout::attribute_types(&record(made.attribute_list_record)?)?;
    if !types.contains(&ATTRIBUTE_LIST) {
        return Err(format!(
            "레코드 {}에 $ATTRIBUTE_LIST가 생기지 않음(속성: {types:x?})",
            made.attribute_list_record
        ));
    }
    Ok(())
}

/// 조각에서 사용 중인 확장 레코드(기본 레코드 참조가 0이 아닌 레코드)를 찾아 기대값 줄로 만든다.
/// 경로는 기본 레코드의 첫 이름이다.
fn extension_records(bytes: &[u8], entries: &[Entry]) -> Vec<Entry> {
    let mut found = Vec::new();
    for (n, chunk) in bytes.as_chunks::<RECORD>().0.iter().enumerate() {
        let mut r = chunk.to_vec();
        if layout::apply_fixup(&mut r).is_err() || r[0x16] & 1 == 0 {
            continue;
        }
        let mut base = [0u8; 8];
        base.copy_from_slice(&r[0x20..0x28]);
        let base = u64::from_le_bytes(base);
        let (base_record, base_sequence) = (base & 0xFFFF_FFFF_FFFF, (base >> 48) as u16);
        if base_record == 0 {
            continue;
        }
        let path = entries
            .iter()
            .filter(|e| e.record == base_record)
            .map(|e| e.path.clone())
            .min()
            .unwrap_or_default();
        found.push(Entry {
            record: n as u64,
            sequence: u16::from_le_bytes([r[0x10], r[0x11]]),
            kind: "확장 레코드",
            path,
            size: None,
            note: format!("기본 레코드 {base_record}(순번 {base_sequence})"),
        });
    }
    found
}

fn expected_tsv(entries: &[Entry], records: usize) -> String {
    let mut sorted: Vec<&Entry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.record.cmp(&b.record).then_with(|| a.path.cmp(&b.path)));
    let mut s = format!(
        "# {FIXTURE}.mft의 기대값 목록. `cargo xtask mft-fixture`가 만든다. 형식은 README.md.\n\
         # 조각: 레코드 0..{records}, 레코드당 {RECORD}바이트, 디스크 그대로(고정값 미적용).\n\
         # 레코드\t순번\t종류\t경로\t크기\t비고\n"
    );
    for e in sorted {
        let size = e.size.map_or_else(|| "-".to_owned(), |s| s.to_string());
        let _ = writeln!(
            s,
            "{}\t{}\t{}\t{}\t{}\t{}",
            e.record, e.sequence, e.kind, e.path, size, e.note
        );
    }
    s
}
