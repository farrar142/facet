//! 배포 산출물: portable zip과 그 체크섬 파일.
//!
//! `.github/workflows/release.yml`이 `cargo xtask dist <label>`을 부르고
//! `target/dist/*`를 통째로 릴리스에 올린다(ADR-0001 「빌드 산출물」).
//! 그래서 산출물 폴더에는 zip과 `.sha256` 두 파일만 남긴다.
//!
//! 이 모듈은 빌드를 모른다. 빌드한 `facet.exe`의 경로를 받아 묶기만 하므로
//! 테스트가 release 빌드 없이 실제 파일로 확인할 수 있다.

use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

/// zip에 넣을 파일 하나: zip 안의 이름과 원본 경로.
pub struct Entry {
    pub name: &'static str,
    pub source: PathBuf,
}

/// label을 파일 이름에 써도 되는지 본다.
///
/// 허용: ASCII 영문자·숫자와 `.`, `-`, `_`. 비어 있거나 `.`으로 시작하면 안 된다
/// (`.`·`..` 같은 경로 조각이나 숨김 파일 이름을 막는다).
pub fn validate_label(label: &str) -> Result<(), String> {
    if label.is_empty() {
        return Err("label이 비어 있습니다".to_owned());
    }
    if label.starts_with('.') {
        return Err(format!("label은 `.`으로 시작할 수 없습니다: {label}"));
    }
    if let Some(c) = label
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')))
    {
        return Err(format!(
            "label에 파일 이름에 쓸 수 없는 글자 {c:?}가 있습니다: {label} \
             (영문자·숫자·`.`·`-`·`_`만 쓸 수 있습니다)"
        ));
    }
    Ok(())
}

/// zip 파일 이름: `facet-<label>-windows-x64.zip`.
pub fn zip_name(label: &str) -> String {
    format!("facet-{label}-windows-x64.zip")
}

/// 산출물 폴더를 비운다(없으면 만든다).
pub fn clean_dir(dir: &Path) -> io::Result<()> {
    match fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    fs::create_dir_all(dir)
}

/// `dir`에 zip과 `.sha256`을 만들고 zip 경로를 돌려준다.
///
/// `dir`는 비어 있어야 한다([`clean_dir`]). 도중에 실패하면 `dir`를 다시 비워
/// 반쪽 파일을 남기지 않는다.
pub fn package(dir: &Path, label: &str, entries: &[Entry]) -> io::Result<PathBuf> {
    let result = write_artifacts(dir, label, entries);
    if result.is_err() {
        let _ = clean_dir(dir);
    }
    result
}

fn write_artifacts(dir: &Path, label: &str, entries: &[Entry]) -> io::Result<PathBuf> {
    let name = zip_name(label);
    let zip_path = dir.join(&name);
    write_zip(&zip_path, entries)?;
    let line = checksum_line(&sha256_hex(&zip_path)?, &name);
    fs::write(dir.join(format!("{name}.sha256")), line)?;
    Ok(zip_path)
}

/// 항목을 zip 최상위에 차례로 넣는다. 시각은 고정값(1980-01-01)이라
/// 같은 입력이면 같은 zip이 나온다.
fn write_zip(path: &Path, entries: &[Entry]) -> io::Result<()> {
    let mut zip = ZipWriter::new(BufWriter::new(File::create(path)?));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(DateTime::default());
    for entry in entries {
        let mut source = File::open(&entry.source)
            .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", entry.source.display())))?;
        zip.start_file(entry.name, options)
            .map_err(io::Error::other)?;
        io::copy(&mut source, &mut zip)?;
    }
    zip.finish().map_err(io::Error::other)?.flush()
}

/// 파일의 SHA-256을 소문자 16진수로.
pub fn sha256_hex(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// `sha256sum` 형식 한 줄: `<해시>  <파일 이름>\n`.
pub fn checksum_line(hex: &str, file_name: &str) -> String {
    format!("{hex}  {file_name}\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::ZipArchive;

    fn file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    /// 가짜 `facet.exe`와 저장소 루트의 `LICENSE`·`NOTICE.txt`.
    fn entries(src: &Path) -> Vec<Entry> {
        let exe = src.join("facet.exe");
        fs::write(&exe, b"MZ fake facet binary").unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        vec![
            Entry {
                name: "facet.exe",
                source: exe,
            },
            Entry {
                name: "LICENSE",
                source: root.join("LICENSE"),
            },
            Entry {
                name: "NOTICE.txt",
                source: root.join("NOTICE.txt"),
            },
        ]
    }

    #[test]
    fn label_accepts_nightly_and_version_tags() {
        for label in ["nightly", "v0.1.0", "v1.2.3-rc.1", "build_7"] {
            assert_eq!(validate_label(label), Ok(()), "{label}");
        }
    }

    #[test]
    fn label_rejects_empty_and_unsafe_names() {
        for label in [
            "", "a/b", "a\\b", "a b", ".", "..", ".hidden", "a:b", "a*b", "한글", "v1\n",
        ] {
            assert!(validate_label(label).is_err(), "{label:?}");
        }
    }

    #[test]
    fn package_writes_exactly_zip_and_checksum() {
        let tmp = tempfile::tempdir().unwrap();
        let dist = tmp.path().join("dist");
        let entries = entries(tmp.path());

        clean_dir(&dist).unwrap();
        package(&dist, "nightly", &entries).unwrap();
        assert_eq!(
            file_names(&dist),
            [
                "facet-nightly-windows-x64.zip",
                "facet-nightly-windows-x64.zip.sha256"
            ]
        );

        // 다시 실행하면 앞선 파일은 지워진다.
        clean_dir(&dist).unwrap();
        package(&dist, "v0.1.0", &entries).unwrap();
        assert_eq!(
            file_names(&dist),
            [
                "facet-v0.1.0-windows-x64.zip",
                "facet-v0.1.0-windows-x64.zip.sha256"
            ]
        );
    }

    #[test]
    fn zip_holds_exactly_three_files_with_same_contents() {
        let tmp = tempfile::tempdir().unwrap();
        let dist = tmp.path().join("dist");
        let entries = entries(tmp.path());
        clean_dir(&dist).unwrap();
        let zip_path = package(&dist, "v0.1.0", &entries).unwrap();

        let mut archive = ZipArchive::new(File::open(&zip_path).unwrap()).unwrap();
        let mut names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().unwrap().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["LICENSE", "NOTICE.txt", "facet.exe"]);

        for entry in &entries {
            let mut inside = Vec::new();
            archive
                .by_name(entry.name)
                .unwrap()
                .read_to_end(&mut inside)
                .unwrap();
            assert_eq!(inside, fs::read(&entry.source).unwrap(), "{}", entry.name);
        }
    }

    #[test]
    fn checksum_matches_zip_in_sha256sum_format() {
        let tmp = tempfile::tempdir().unwrap();
        let dist = tmp.path().join("dist");
        let entries = entries(tmp.path());
        clean_dir(&dist).unwrap();
        let zip_path = package(&dist, "nightly", &entries).unwrap();

        // 스트리밍 계산과 따로, 파일 전체를 한 번에 해시해 비교한다.
        let actual: String = Sha256::digest(fs::read(&zip_path).unwrap())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let line = fs::read_to_string(dist.join("facet-nightly-windows-x64.zip.sha256")).unwrap();
        assert_eq!(line, format!("{actual}  facet-nightly-windows-x64.zip\n"));
        assert_eq!(actual.len(), 64);
        assert!(actual.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
    }

    #[test]
    fn known_sha256() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("abc");
        fs::write(&path, b"abc").unwrap();
        assert_eq!(
            sha256_hex(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn failure_leaves_no_partial_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dist = tmp.path().join("dist");
        let mut entries = entries(tmp.path());
        entries[2].source = tmp.path().join("missing.txt");
        clean_dir(&dist).unwrap();

        assert!(package(&dist, "nightly", &entries).is_err());
        assert!(file_names(&dist).is_empty());
    }
}
