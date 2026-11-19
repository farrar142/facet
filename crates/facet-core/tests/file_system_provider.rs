//! 파일 시스템 공급자의 「위치의 항목 목록 읽기」·상위 위치·경로 조각을 공개 인터페이스로만 확인한다.
//! 임시 폴더에 실제 파일과 폴더를 만들고 결과를 본다.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File};
use std::path::Path;
use std::time::{Duration, SystemTime};

use facet_core::{Attributes, FileSystemProvider, Item, ListError, Location, Provider};
use tempfile::TempDir;

fn by_name(items: Vec<Item>) -> BTreeMap<OsString, Item> {
    items.into_iter().map(|i| (i.name.clone(), i)).collect()
}

/// 파일 시스템 공급자로 실제 폴더 `path`의 목록을 읽는다.
fn list(path: &Path) -> Result<Vec<Item>, ListError> {
    FileSystemProvider.list(&Location::file_system(path))
}

fn write_file(path: &Path, len: usize) {
    fs::write(path, vec![b'x'; len]).unwrap();
}

#[test]
fn 빈_폴더는_빈_목록() {
    let dir = TempDir::new().unwrap();
    assert_eq!(list(dir.path()).unwrap(), Vec::new());
}

#[test]
fn 파일과_폴더가_섞인_폴더() {
    let dir = TempDir::new().unwrap();
    write_file(&dir.path().join("a.txt"), 3);
    write_file(&dir.path().join("b"), 0);
    fs::create_dir(dir.path().join("c")).unwrap();
    fs::create_dir(dir.path().join("d.dir")).unwrap();
    write_file(&dir.path().join("d.dir").join("안쪽.txt"), 1);

    let items = by_name(list(dir.path()).unwrap());
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

    let items = by_name(list(dir.path()).unwrap());
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

    let items = by_name(list(dir.path()).unwrap());
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

    let items = by_name(list(dir.path()).unwrap());
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
    let err = list(&missing).unwrap_err();
    assert!(matches!(err, ListError::NotFound { .. }), "{err:?}");
    assert_eq!(err.location(), &Location::file_system(&missing));
}

#[test]
fn 파일_경로는_폴더_아님_오류() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("파일.txt");
    write_file(&file, 1);
    let err = list(&file).unwrap_err();
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
    let err = list(&locked).unwrap_err();
    assert!(matches!(err, ListError::PermissionDenied { .. }), "{err:?}");
    assert_eq!(err.location(), &Location::file_system(&locked));
}

/// 폴더 읽기를 막고, 끝나면 되돌린다.
struct Lock<'a>(&'a Path);

#[cfg(windows)]
impl<'a> Lock<'a> {
    /// 모든 사용자(Everyone, S-1-1-0)에게 폴더 내용 나열을 거부하는 ACE를 단다.
    /// 소유자는 그래도 권한 목록을 고칠 수 있어 되돌릴 수 있다.
    ///
    /// 백업 권한(SeBackupPrivilege)이 켜진 프로세스는 거부 ACE를 무시하므로,
    /// 막은 뒤에도 읽히면 막을 수 없는 환경으로 보고 `None`을 돌려준다.
    fn new(path: &'a Path) -> Option<Self> {
        let ok = icacls(path, &["/deny", "*S-1-1-0:(RD)"]);
        assert!(ok, "icacls로 읽기를 막지 못했다");
        let lock = Self(path);
        if fs::read_dir(path).is_ok() {
            return None;
        }
        Some(lock)
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

#[test]
fn 위치에_맞는_공급자가_목록을_읽는다() {
    let dir = TempDir::new().unwrap();
    write_file(&dir.path().join("a.txt"), 1);
    let location = Location::file_system(dir.path());
    let items = facet_core::provider_for(&location).list(&location).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(location.file_system_path(), Some(dir.path()));
}

#[test]
fn 속성이_없는_항목은_모두_꺼져_있다() {
    let dir = TempDir::new().unwrap();
    write_file(&dir.path().join("보통.txt"), 1);
    fs::create_dir(dir.path().join("보통 폴더")).unwrap();
    for item in list(dir.path()).unwrap() {
        // `bits`는 열거 결과의 원래 속성 값(폴더 0x10, 보관 0x20 등)이라 견주지 않는다.
        let flags = Attributes {
            bits: 0,
            ..item.attributes
        };
        assert_eq!(flags, Attributes::default(), "{:?}", item.name);
    }
}

#[cfg(windows)]
#[test]
fn 숨김_시스템_읽기_전용_속성을_읽는다() {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    const READONLY: u32 = 0x1;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;

    let dir = TempDir::new().unwrap();
    let cases = [
        ("숨김.txt", HIDDEN),
        ("시스템.txt", SYSTEM),
        ("읽기 전용.txt", READONLY),
        ("셋 다.txt", HIDDEN | SYSTEM | READONLY),
        ("보통.txt", 0),
    ];
    for (name, attrs) in cases {
        let path = dir.path().join(name);
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .attributes(attrs)
            .open(&path)
            .unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().file_attributes() & (HIDDEN | SYSTEM | READONLY),
            attrs,
            "{name}: 속성이 붙지 않았다"
        );
    }
    let folder = dir.path().join("숨김 시스템 폴더");
    fs::create_dir(&folder).unwrap();
    let ok = std::process::Command::new("attrib")
        .args(["+h", "+s"])
        .arg(&folder)
        .status()
        .unwrap()
        .success();
    assert!(ok, "attrib로 폴더 속성을 붙이지 못했다");

    let items = by_name(list(dir.path()).unwrap());
    for (name, attrs) in cases {
        let a = items[&OsString::from(name)].attributes;
        assert_eq!(a.hidden, attrs & HIDDEN != 0, "{name}");
        assert_eq!(a.system, attrs & SYSTEM != 0, "{name}");
        assert_eq!(a.read_only, attrs & READONLY != 0, "{name}");
    }
    let f = &items[&OsString::from("숨김 시스템 폴더")];
    assert!(f.is_folder);
    assert!(f.attributes.hidden && f.attributes.system && !f.attributes.read_only);

    // 임시 폴더를 지울 수 있게 읽기 전용을 푼다.
    for name in ["읽기 전용.txt", "셋 다.txt"] {
        let path = dir.path().join(name);
        let mut perm = fs::metadata(&path).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        fs::set_permissions(&path, perm).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn 점으로_시작하는_이름은_숨김이고_쓰기_권한이_없으면_읽기_전용이다() {
    let dir = TempDir::new().unwrap();
    write_file(&dir.path().join(".숨김"), 1);
    let ro = dir.path().join("읽기 전용");
    write_file(&ro, 1);
    let mut perm = fs::metadata(&ro).unwrap().permissions();
    perm.set_readonly(true);
    fs::set_permissions(&ro, perm).unwrap();

    let items = by_name(list(dir.path()).unwrap());
    assert!(items[&OsString::from(".숨김")].attributes.hidden);
    assert!(items[&OsString::from("읽기 전용")].attributes.read_only);
}

#[test]
fn 경로가_260자를_넘는_폴더를_읽는다() {
    let dir = TempDir::new().unwrap();
    let mut deep = dir.path().to_path_buf();
    for i in 0..6 {
        deep.push(format!("{i}-{}", "긴폴더이름".repeat(10)));
    }
    fs::create_dir_all(&deep).unwrap();
    let path_len = wide_len(deep.as_os_str());
    assert!(path_len > 260, "경로가 260자 이하다: {path_len}");
    let file_name = format!("{}.txt", "파일".repeat(20));
    write_file(&deep.join(&file_name), 7);
    fs::create_dir(deep.join("안쪽")).unwrap();

    let items = by_name(list(&deep).unwrap());
    assert_eq!(items.len(), 2);
    assert_eq!(items[&OsString::from(file_name)].size, 7);
    assert!(items[&OsString::from("안쪽")].is_folder);

    // 긴 경로에서도 상위 위치와 경로 조각이 이어진다.
    let location = Location::file_system(&deep);
    let parent = FileSystemProvider.parent(&location).unwrap();
    assert_eq!(parent.file_system_path(), deep.parent());
    assert_eq!(FileSystemProvider.list(&parent).unwrap().len(), 1);
    let segments = FileSystemProvider.segments(&location);
    assert_eq!(segments.last().unwrap().location, location);
}

/// 경로 길이를 Windows가 세는 단위(UTF-16)로 센다. 다른 운영 체제에서는 바이트 수다.
#[cfg(windows)]
fn wide_len(s: &std::ffi::OsStr) -> usize {
    use std::os::windows::ffi::OsStrExt;
    s.encode_wide().count()
}

#[cfg(not(windows))]
fn wide_len(s: &std::ffi::OsStr) -> usize {
    s.len()
}

fn parent_of(path: &str) -> Option<String> {
    FileSystemProvider
        .parent(&Location::file_system(path))
        .map(|l| l.file_system_path().unwrap().to_str().unwrap().to_owned())
}

/// 경로 조각을 (보이는 이름, 그 조각의 위치 경로)로 펼친다.
fn segments_of(path: &str) -> Vec<(String, String)> {
    FileSystemProvider
        .segments(&Location::file_system(path))
        .into_iter()
        .map(|s| {
            let p = s.location.file_system_path().unwrap().to_str().unwrap();
            (s.label, p.to_owned())
        })
        .collect()
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

#[test]
fn 임시_폴더_안_폴더의_상위는_한_단계_위_폴더() {
    let dir = TempDir::new().unwrap();
    let child = dir.path().join("하위");
    fs::create_dir(&child).unwrap();
    let parent = FileSystemProvider
        .parent(&Location::file_system(&child))
        .unwrap();
    assert_eq!(parent, Location::file_system(dir.path()));
}

#[cfg(windows)]
#[test]
fn 상위_위치() {
    assert_eq!(parent_of(r"C:\Users\me").as_deref(), Some(r"C:\Users"));
    assert_eq!(parent_of(r"C:\Users").as_deref(), Some(r"C:\"));
    assert_eq!(parent_of(r"C:\Users\").as_deref(), Some(r"C:\"));
    assert_eq!(parent_of(r"\\?\C:\긴\경로").as_deref(), Some(r"\\?\C:\긴"));
    assert_eq!(
        parent_of(r"\\server\share\폴더").as_deref(),
        Some(r"\\server\share\")
    );
}

#[cfg(windows)]
#[test]
fn 드라이브_루트에는_상위가_없다() {
    assert_eq!(parent_of(r"C:\"), None);
    assert_eq!(parent_of(r"D:\"), None);
    assert_eq!(parent_of(r"\\?\C:\"), None);
    assert_eq!(parent_of(r"\\server\share\"), None);
    assert_eq!(parent_of(r"\\server\share"), None);
    assert_eq!(parent_of(""), None);
}

#[cfg(unix)]
#[test]
fn 루트에는_상위가_없다() {
    assert_eq!(parent_of("/home").as_deref(), Some("/"));
    assert_eq!(parent_of("/"), None);
    assert_eq!(parent_of(""), None);
}

#[cfg(windows)]
#[test]
fn 경로_조각() {
    assert_eq!(
        segments_of(r"C:\Users\me"),
        pairs(&[
            ("C:", r"C:\"),
            ("Users", r"C:\Users"),
            ("me", r"C:\Users\me")
        ])
    );
    assert_eq!(segments_of(r"C:\"), pairs(&[("C:", r"C:\")]));
    assert_eq!(
        segments_of(r"C:\한글 폴더\"),
        pairs(&[("C:", r"C:\"), ("한글 폴더", r"C:\한글 폴더")])
    );
    // 긴 경로 접두어는 보이는 이름에서 빼고 위치에는 남긴다.
    assert_eq!(
        segments_of(r"\\?\C:\a"),
        pairs(&[("C:", r"\\?\C:\"), ("a", r"\\?\C:\a")])
    );
    assert_eq!(
        segments_of(r"\\server\share\폴더"),
        pairs(&[
            (r"\\server\share", r"\\server\share\"),
            ("폴더", r"\\server\share\폴더")
        ])
    );
    assert_eq!(
        segments_of(r"\\?\UNC\server\share\폴더"),
        pairs(&[
            (r"\\server\share", r"\\?\UNC\server\share\"),
            ("폴더", r"\\?\UNC\server\share\폴더")
        ])
    );
    assert_eq!(segments_of(""), Vec::new());
}

#[cfg(unix)]
#[test]
fn 경로_조각() {
    assert_eq!(
        segments_of("/home/me"),
        pairs(&[("/", "/"), ("home", "/home"), ("me", "/home/me")])
    );
    assert_eq!(segments_of("/"), pairs(&[("/", "/")]));
}

#[test]
fn 경로_조각을_따라가면_상위_위치와_같다() {
    let dir = TempDir::new().unwrap();
    let location = Location::file_system(dir.path().join("a").join("b"));
    let segments = FileSystemProvider.segments(&location);
    let n = segments.len();
    assert!(n >= 3);
    assert_eq!(segments[n - 1].location, location);
    assert_eq!(
        Some(segments[n - 2].location.clone()),
        FileSystemProvider.parent(&location)
    );
}
