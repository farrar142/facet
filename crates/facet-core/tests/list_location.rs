//! 「위치의 항목 목록 읽기」를 공개 인터페이스로만 확인한다.
//! 임시 폴더에 실제 파일과 폴더를 만들고 결과를 본다.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File};
use std::path::Path;
use std::time::{Duration, SystemTime};

use facet_core::{Item, ListError, list_location};
use tempfile::TempDir;

fn by_name(items: Vec<Item>) -> BTreeMap<OsString, Item> {
    items.into_iter().map(|i| (i.name.clone(), i)).collect()
}

fn write_file(path: &Path, len: usize) {
    fs::write(path, vec![b'x'; len]).unwrap();
}

#[test]
fn 빈_폴더는_빈_목록() {
    let dir = TempDir::new().unwrap();
    assert_eq!(list_location(dir.path()).unwrap(), Vec::new());
}

#[test]
fn 파일과_폴더가_섞인_폴더() {
    let dir = TempDir::new().unwrap();
    write_file(&dir.path().join("a.txt"), 3);
    write_file(&dir.path().join("b"), 0);
    fs::create_dir(dir.path().join("c")).unwrap();
    fs::create_dir(dir.path().join("d.dir")).unwrap();
    write_file(&dir.path().join("d.dir").join("안쪽.txt"), 1);

    let items = by_name(list_location(dir.path()).unwrap());
    let folders: Vec<(&str, bool)> = items
        .iter()
        .map(|(n, i)| (n.to_str().unwrap(), i.is_folder))
        .collect();
    assert_eq!(
        folders,
        vec![("a.txt", false), ("b", false), ("c", true), ("d.dir", true)]
    );
}

#[test]
fn 특수한_이름이_그대로_돌아온다() {
    let dir = TempDir::new().unwrap();
    let long = format!("{}{}", "가".repeat(100), "a".repeat(150));
    let names = [
        "새 폴더 가나다.txt",
        "😀 이모지 🎉",
        "  앞뒤 공백 .txt",
        "a  b",
        long.as_str(),
    ];
    for name in names {
        write_file(&dir.path().join(name), 1);
    }
    fs::create_dir(dir.path().join("한글 폴더 📁")).unwrap();

    let items = by_name(list_location(dir.path()).unwrap());
    let mut expected: Vec<OsString> = names.iter().map(OsString::from).collect();
    expected.push(OsString::from("한글 폴더 📁"));
    expected.sort();
    assert_eq!(items.keys().cloned().collect::<Vec<_>>(), expected);
    assert!(items[&OsString::from("한글 폴더 📁")].is_folder);
}

#[test]
fn 숨김_항목도_포함된다() {
    let dir = TempDir::new().unwrap();
    write_file(&dir.path().join(".점으로_시작"), 1);
    fs::create_dir(dir.path().join(".숨김폴더")).unwrap();
    create_hidden_file(&dir.path().join("숨김속성.txt"));

    let items = by_name(list_location(dir.path()).unwrap());
    assert!(items.contains_key(&OsString::from(".점으로_시작")));
    assert!(items[&OsString::from(".숨김폴더")].is_folder);
    assert!(items.contains_key(&OsString::from("숨김속성.txt")));
    assert_eq!(items.len(), 3);
}

#[cfg(windows)]
fn create_hidden_file(path: &Path) {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .attributes(FILE_ATTRIBUTE_HIDDEN)
        .open(path)
        .unwrap();
    let attrs = {
        use std::os::windows::fs::MetadataExt;
        fs::metadata(path).unwrap().file_attributes()
    };
    assert_ne!(attrs & FILE_ATTRIBUTE_HIDDEN, 0, "숨김 속성이 붙지 않았다");
}

#[cfg(not(windows))]
fn create_hidden_file(path: &Path) {
    write_file(path, 0);
}

#[test]
fn 크기와_수정_시각이_실제_값과_같다() {
    let dir = TempDir::new().unwrap();
    let known = SystemTime::UNIX_EPOCH + Duration::from_secs(981_173_106); // 2001-02-03
    let file = dir.path().join("크기.bin");
    write_file(&file, 12_345);
    File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(known)
        .unwrap();
    write_file(&dir.path().join("빈.bin"), 0);
    fs::create_dir(dir.path().join("폴더")).unwrap();

    let items = by_name(list_location(dir.path()).unwrap());
    let item = &items[&OsString::from("크기.bin")];
    assert_eq!(item.size, 12_345);
    assert_eq!(item.modified, known);
    assert_eq!(items[&OsString::from("빈.bin")].size, 0);

    for (name, item) in &items {
        let actual = fs::metadata(dir.path().join(name)).unwrap();
        assert_eq!(item.modified, actual.modified().unwrap(), "{name:?}");
        if !item.is_folder {
            assert_eq!(item.size, actual.len(), "{name:?}");
        }
    }
}

#[test]
fn 없는_경로는_없음_오류() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("없는 폴더");
    let err = list_location(&missing).unwrap_err();
    assert!(matches!(err, ListError::NotFound { .. }), "{err:?}");
    assert_eq!(err.path(), missing);
}

#[test]
fn 파일_경로는_폴더_아님_오류() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("파일.txt");
    write_file(&file, 1);
    let err = list_location(&file).unwrap_err();
    assert!(matches!(err, ListError::NotAFolder { .. }), "{err:?}");
}

#[test]
fn 읽을_수_없는_경로는_권한_오류() {
    let dir = TempDir::new().unwrap();
    let locked = dir.path().join("잠긴 폴더");
    fs::create_dir(&locked).unwrap();
    write_file(&locked.join("안.txt"), 1);

    let Some(_guard) = Lock::new(&locked) else {
        eprintln!("이 환경에서는 읽기를 막을 수 없어 건너뛴다");
        return;
    };
    let err = list_location(&locked).unwrap_err();
    assert!(matches!(err, ListError::PermissionDenied { .. }), "{err:?}");
    assert_eq!(err.path(), locked);
}

/// 폴더 읽기를 막고, 끝나면 되돌린다.
struct Lock<'a>(&'a Path);

#[cfg(windows)]
impl<'a> Lock<'a> {
    /// 모든 사용자(Everyone, S-1-1-0)에게 폴더 내용 나열을 거부하는 ACE를 단다.
    /// 소유자는 그래도 권한 목록을 고칠 수 있어 되돌릴 수 있다.
    fn new(path: &'a Path) -> Option<Self> {
        let ok = icacls(path, &["/deny", "*S-1-1-0:(RD)"]);
        assert!(ok, "icacls로 읽기를 막지 못했다");
        Some(Self(path))
    }
}

#[cfg(windows)]
impl Drop for Lock<'_> {
    fn drop(&mut self) {
        icacls(self.0, &["/remove:d", "*S-1-1-0"]);
    }
}

#[cfg(windows)]
fn icacls(path: &Path, args: &[&str]) -> bool {
    std::process::Command::new("icacls")
        .arg(path)
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
}

#[cfg(unix)]
impl<'a> Lock<'a> {
    fn new(path: &'a Path) -> Option<Self> {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
        // root는 권한을 무시하므로 막히지 않는다.
        if fs::read_dir(path).is_ok() {
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            return None;
        }
        Some(Self(path))
    }
}

#[cfg(unix)]
impl Drop for Lock<'_> {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(self.0, fs::Permissions::from_mode(0o755));
    }
}
