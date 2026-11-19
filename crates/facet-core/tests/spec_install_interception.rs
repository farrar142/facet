// target: crates/facet-core/tests/spec_install_interception.rs
// AC: #469: 래퍼가 여러 형식의 값을 바이트 그대로 쓰고 읽음, 켜기→끄기 뒤 바이트 단위 복구·없던 값과 키는 다시 없음, 백업을 쓸 수 없으면 레지스트리 그대로, 켜짐·풀림·꺼짐, 풀린 뒤 다시 켜면 새 원래 값, 백업 없음·깨진 백업, 다른 루트에서 되돌리기, for_facet의 값
// AC: #479: 프로필마다 되돌림·백업 없음·실패(깨진 백업은 그대로, 다른 프로필은 되돌림), 결과는 넘긴 차례
//! 폴더 열기 가로채기의 백업·복구(`facet_core::interception`)와 레지스트리 안전 래퍼(`facet_win::registry`)를 실제
//! 레지스트리로 확인한다. 실제 폴더 열기 키는 건드리지 않고 HKCU의 `Software\facet-test\...` 아래 시험마다 새 루트 키를
//! 만들어 그 아래에서 켜고 끄며, 끝나면 지운다. 백업 파일은 임시 폴더에 둔다. Windows 전용.

#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use facet_core::interception::{
    self, BACKUP_FILE_NAME, Change, Interception, InterceptionError, InterceptionStatus, Profile,
    ProfileOutcome,
};
use facet_win::registry::{self, Hive, REG_BINARY, REG_DWORD, REG_EXPAND_SZ, REG_SZ, RegValue};
use tempfile::TempDir;

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// 시험마다 새로 만드는 HKCU 아래 루트 키. 버리면 지운다.
struct TestRoot(String);

impl TestRoot {
    fn new() -> TestRoot {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = format!(
            r"Software\facet-test\interception-{}-{n}",
            std::process::id()
        );
        registry::delete_tree(Hive::CurrentUser, &root).unwrap();
        TestRoot(root)
    }

    fn key(&self, relative: &str) -> String {
        format!(r"{}\{relative}", self.0)
    }

    fn read(&self, relative: &str, name: &str) -> Option<RegValue> {
        registry::read_value(Hive::CurrentUser, &self.key(relative), name).unwrap()
    }

    fn write(&self, relative: &str, name: &str, value: &RegValue) {
        registry::write_value(Hive::CurrentUser, &self.key(relative), name, value).unwrap();
    }

    fn exists(&self, relative: &str) -> bool {
        registry::key_exists(Hive::CurrentUser, &self.key(relative)).unwrap()
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = registry::delete_tree(Hive::CurrentUser, &self.0);
    }
}

const OPEN: &str = r"Folder\shell\open\command";
const WIN_E: &str = r"CLSID\{test}\shell\opennewwindow\command";

fn raw(kind: u32, data: &[u8]) -> RegValue {
    RegValue {
        kind,
        data: data.to_vec(),
    }
}

fn expand(text: &str) -> RegValue {
    let mut value = RegValue::string(text);
    value.kind = REG_EXPAND_SZ;
    value
}

/// facet이 쓸 값들: 있던 값 바꾸기(형식도 바뀜), 없던 값 더하기, 없던 키 만들기.
fn changes() -> Vec<Change> {
    let change = |key: &str, name: &str, value: RegValue| Change {
        key: key.to_owned(),
        name: name.to_owned(),
        value,
    };
    vec![
        change(OPEN, "", RegValue::string(r#""C:\facet\facet.exe" "%1""#)),
        change(OPEN, "DelegateExecute", RegValue::string("")),
        change(OPEN, "Flags", raw(REG_DWORD, &1u32.to_le_bytes())),
        change(OPEN, "Odd", RegValue::string("facet")),
        change(WIN_E, "", RegValue::string(r#""C:\facet\facet.exe""#)),
    ]
}

/// 켜기 전 원래 상태: `OPEN` 키에 기본값(REG_EXPAND_SZ)·Flags(REG_BINARY 홀수 바이트)·Odd(NUL 없는 REG_SZ)와 facet이
/// 건드리지 않는 값 하나, `DelegateExecute`는 없음, `WIN_E` 키는 상위 키까지 없음.
fn originals(root: &TestRoot) {
    root.write(
        OPEN,
        "",
        &expand(r"%SystemRoot%\explorer.exe /idlist,%I,%L"),
    );
    root.write(OPEN, "Flags", &raw(REG_BINARY, &[1, 2, 3]));
    root.write(OPEN, "Odd", &raw(REG_SZ, &[0x41, 0x00, 0x42]));
    root.write(OPEN, "Untouched", &raw(REG_DWORD, &7u32.to_le_bytes()));
}

fn assert_originals(root: &TestRoot) {
    assert_eq!(
        root.read(OPEN, ""),
        Some(expand(r"%SystemRoot%\explorer.exe /idlist,%I,%L"))
    );
    assert_eq!(root.read(OPEN, "Flags"), Some(raw(REG_BINARY, &[1, 2, 3])));
    assert_eq!(
        root.read(OPEN, "Odd"),
        Some(raw(REG_SZ, &[0x41, 0x00, 0x42])),
        "NUL 없는 홀수 바이트도 그대로"
    );
    assert_eq!(
        root.read(OPEN, "Untouched"),
        Some(raw(REG_DWORD, &7u32.to_le_bytes()))
    );
    assert_eq!(
        root.read(OPEN, "DelegateExecute"),
        None,
        "없던 값은 다시 없다"
    );
    assert!(!root.exists(WIN_E), "facet이 만든 키는 남지 않는다");
    assert!(!root.exists("CLSID"), "facet이 만든 상위 키도 남지 않는다");
}

fn assert_facet_values(root: &TestRoot) {
    for change in changes() {
        assert_eq!(
            root.read(&change.key, &change.name),
            Some(change.value),
            "{}\\{}",
            change.key,
            change.name
        );
    }
}

fn interception_at(root: &TestRoot, backup: &Path) -> Interception {
    Interception::new(Hive::CurrentUser, root.0.clone(), changes(), backup)
}

#[test]
#[ignore = "#469"]
fn 래퍼는_여러_형식의_값을_바이트_그대로_쓰고_읽는다() {
    let root = TestRoot::new();
    assert!(!root.exists("a"));
    assert_eq!(root.read("a", "x"), None, "없는 키의 값은 없음");

    let values = [
        ("sz", RegValue::string("한글 ✓ C:\\facet")),
        ("expand", expand("%TEMP%\\x")),
        ("dword", raw(REG_DWORD, &0xDEAD_BEEFu32.to_le_bytes())),
        ("binary", raw(REG_BINARY, &[0, 255, 1, 254, 2])),
        ("odd", raw(REG_SZ, &[0x41, 0x00, 0x42])),
        ("", RegValue::string("기본값")),
    ];
    for (name, value) in &values {
        root.write(r"a\b", name, value);
    }
    assert!(root.exists("a") && root.exists(r"a\b"));
    for (name, value) in &values {
        assert_eq!(root.read(r"a\b", name).as_ref(), Some(value), "{name}");
    }
    assert_eq!(
        RegValue::string("ab").data,
        [0x61, 0, 0x62, 0, 0, 0],
        "UTF-16LE와 끝 NUL"
    );
    assert_eq!(RegValue::string("ab").kind, REG_SZ);

    registry::delete_value(Hive::CurrentUser, &root.key(r"a\b"), "sz").unwrap();
    assert_eq!(root.read(r"a\b", "sz"), None);
    registry::delete_value(Hive::CurrentUser, &root.key(r"a\b"), "없는 값").unwrap();
    registry::delete_tree(Hive::CurrentUser, &root.key("a")).unwrap();
    assert!(!root.exists("a"));
    registry::delete_tree(Hive::CurrentUser, &root.key("a")).unwrap();
}

#[test]
#[ignore = "#469"]
fn 켜고_끄면_원래_값이_바이트까지_같고_없던_값과_키는_다시_없다() {
    let root = TestRoot::new();
    originals(&root);
    let dir = TempDir::new().unwrap();
    let backup = dir.path().join(BACKUP_FILE_NAME);
    let interception = interception_at(&root, &backup);
    assert_eq!(interception.status(), Ok(InterceptionStatus::Off));

    interception.enable().expect("켜기");
    assert!(backup.is_file(), "켜면 백업이 남는다");
    assert_facet_values(&root);
    assert_eq!(interception.status(), Ok(InterceptionStatus::On));

    interception.disable().expect("끄기");
    assert_originals(&root);
    assert!(!backup.exists(), "끄면 백업을 지운다");
    assert_eq!(interception.status(), Ok(InterceptionStatus::Off));
}

#[test]
#[ignore = "#469"]
fn 백업을_쓸_수_없으면_레지스트리를_바꾸지_않는다() {
    let root = TestRoot::new();
    originals(&root);
    let dir = TempDir::new().unwrap();
    // 상위가 파일이라 백업을 만들 수 없는 자리.
    let blocker = dir.path().join("파일");
    fs::write(&blocker, b"not a folder").unwrap();
    let backup = blocker.join(BACKUP_FILE_NAME);
    let interception = interception_at(&root, &backup);

    assert!(matches!(
        interception.enable(),
        Err(InterceptionError::Backup(_))
    ));
    assert_originals(&root);
    assert_eq!(interception.status(), Ok(InterceptionStatus::Off));
}

#[test]
#[ignore = "#469"]
fn 밖에서_바꾸면_풀림이고_다시_켜면_바뀐_값이_새_원래_값이다() {
    let root = TestRoot::new();
    originals(&root);
    let dir = TempDir::new().unwrap();
    let backup = dir.path().join(BACKUP_FILE_NAME);
    let interception = interception_at(&root, &backup);
    interception.enable().unwrap();

    // Windows 업데이트가 기본값을 새 값으로 돌려놓은 경우.
    let reset = expand(r"%SystemRoot%\explorer.exe /new");
    root.write(OPEN, "", &reset);
    assert_eq!(interception.status(), Ok(InterceptionStatus::Lost));

    interception.enable().expect("다시 켜기");
    assert_eq!(interception.status(), Ok(InterceptionStatus::On));
    assert_facet_values(&root);

    interception.disable().unwrap();
    assert_eq!(root.read(OPEN, ""), Some(reset), "바뀐 값이 새 원래 값이다");
    // 다시 켤 때 아직 facet 값이던 것은 처음 백업대로 돌아온다.
    assert_eq!(root.read(OPEN, "DelegateExecute"), None);
    assert_eq!(root.read(OPEN, "Flags"), Some(raw(REG_BINARY, &[1, 2, 3])));
    assert!(!root.exists(WIN_E));
    assert!(!backup.exists());
}

#[test]
#[ignore = "#469"]
fn 백업이_없거나_깨졌으면_끄기가_아무것도_바꾸지_않는다() {
    let root = TestRoot::new();
    originals(&root);
    let dir = TempDir::new().unwrap();
    let backup = dir.path().join(BACKUP_FILE_NAME);
    let interception = interception_at(&root, &backup);
    assert_eq!(interception.disable(), Err(InterceptionError::NotEnabled));
    assert_originals(&root);

    interception.enable().unwrap();
    fs::write(&backup, b"{ \xEB\xB0\xB1\xEC\x97\x85 ").unwrap();
    assert_eq!(interception.disable(), Err(InterceptionError::BackupBroken));
    assert_facet_values(&root);
    assert!(backup.is_file(), "깨진 백업은 지우지 않는다");
}

#[test]
#[ignore = "#469"]
fn 같은_백업을_다른_루트에서_되돌린다() {
    let first = TestRoot::new();
    let second = TestRoot::new();
    originals(&first);
    let dir = TempDir::new().unwrap();
    let backup = dir.path().join(BACKUP_FILE_NAME);
    interception_at(&first, &backup).enable().unwrap();

    // 같은 사용자의 하이브를 다른 자리(HKU\<SID>)에서 본 것처럼 두 번째 루트에 지금 값을 옮겨 둔다.
    for change in changes() {
        second.write(&change.key, &change.name, &change.value);
    }
    second.write(OPEN, "Untouched", &raw(REG_DWORD, &7u32.to_le_bytes()));

    interception::restore_from_backup(Hive::CurrentUser, &second.0, &backup).expect("되돌리기");
    assert_originals(&second);
    assert!(!backup.exists());
    assert_facet_values(&first);
}

#[test]
#[ignore = "#469"]
fn facet_값은_사용자_클래스_아래에서_facet_실행_파일을_가리킨다() {
    let exe = Path::new(r"C:\Program Files\facet\facet.exe");
    let backup = PathBuf::from(r"C:\Users\someone\AppData\Roaming\facet").join(BACKUP_FILE_NAME);
    let interception = Interception::for_facet(exe, &backup);
    assert_eq!(interception.hive(), Hive::CurrentUser);
    assert_eq!(interception.root(), r"Software\Classes");
    assert_eq!(interception.backup_path(), backup);
    assert!(!interception.changes().is_empty());

    let keys: Vec<String> = interception
        .changes()
        .iter()
        .map(|c| c.key.to_ascii_lowercase())
        .collect();
    assert!(
        keys.iter().any(|k| k.starts_with(r"folder\shell\open")),
        "폴더 열기 동작: {keys:?}"
    );
    assert!(
        keys.iter()
            .any(|k| k.contains("{52205fd8-5dfb-447d-801a-d0b52f2e83e1}")),
        "Win+E 대상: {keys:?}"
    );
    for change in interception.changes() {
        if change.name.is_empty() {
            let units: Vec<u16> = change
                .value
                .data
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect();
            let text = String::from_utf16_lossy(&units);
            assert!(
                text.contains(r"C:\Program Files\facet\facet.exe"),
                "{} 명령이 facet을 가리키지 않는다: {text}",
                change.key
            );
        }
    }
}

#[test]
#[ignore = "#479"]
fn 프로필마다_되돌리고_백업_없음과_실패를_가른다() {
    let broken_root = TestRoot::new();
    let enabled_root = TestRoot::new();
    let plain_root = TestRoot::new();
    let dir = TempDir::new().unwrap();
    let backup_of = |name: &str| {
        let folder = dir.path().join(name);
        fs::create_dir_all(&folder).unwrap();
        folder.join(BACKUP_FILE_NAME)
    };

    // 깨진 백업이 든 계정: facet 값이 그대로 남아야 한다.
    originals(&broken_root);
    let broken_backup = backup_of("broken");
    interception_at(&broken_root, &broken_backup)
        .enable()
        .unwrap();
    fs::write(&broken_backup, b"not json").unwrap();

    // 가로채기를 켠 계정.
    originals(&enabled_root);
    let enabled_backup = backup_of("enabled");
    interception_at(&enabled_root, &enabled_backup)
        .enable()
        .unwrap();

    // 켠 적 없는 계정.
    originals(&plain_root);
    let plain_backup = backup_of("plain");

    let profile = |name: &str, root: &TestRoot, backup: &Path| Profile {
        name: name.to_owned(),
        hive: Hive::CurrentUser,
        root: root.0.clone(),
        backup_path: backup.to_path_buf(),
    };
    let outcomes = interception::restore_profiles(&[
        profile("broken", &broken_root, &broken_backup),
        profile("enabled", &enabled_root, &enabled_backup),
        profile("plain", &plain_root, &plain_backup),
    ]);

    let names: Vec<&str> = outcomes.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["broken", "enabled", "plain"], "넘긴 차례대로");
    assert!(matches!(outcomes[0].1, ProfileOutcome::Failed(_)));
    assert_eq!(outcomes[1].1, ProfileOutcome::Restored);
    assert_eq!(outcomes[2].1, ProfileOutcome::NoBackup);

    assert_facet_values(&broken_root);
    assert!(broken_backup.is_file(), "실패한 계정의 백업은 남는다");
    assert_originals(&enabled_root);
    assert!(!enabled_backup.exists());
    assert_originals(&plain_root);
}
