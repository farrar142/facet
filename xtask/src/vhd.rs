//! 시험 VHD: 실제 볼륨 테스트와 MFT 조각 만들기에 쓰는 작은 NTFS 볼륨.
//!
//! `diskpart`로 확장형 VHDX를 만들어 붙이고, 파티션 하나를 NTFS로 빠르게 포맷해 빈 드라이브
//! 문자에 붙인다. 버릴 때(`Drop`) 떼고 파일을 지운다. 관리자 권한이 필요하다.
//! 같은 일을 손으로 하는 방법은 `testdata/mft/README.md`에 있다.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 시험 VHD의 최대 크기(MB). NTFS로 포맷할 수 있는 작은 크기면 된다.
const SIZE_MB: u32 = 64;
/// 시험 VHD 볼륨의 이름표.
pub const LABEL: &str = "facet-test";

/// 이 프로세스가 관리자 권한으로 도는지. `net session`은 관리자일 때만 성공한다.
pub fn is_admin() -> bool {
    cfg!(windows)
        && Command::new("net")
            .arg("session")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
}

/// 붙어 있는 시험 VHD.
pub struct TestVhd {
    file: PathBuf,
    /// 대문자 드라이브 문자.
    pub letter: char,
}

impl TestVhd {
    /// 임시 폴더에 시험 VHD를 만들어 NTFS로 포맷하고 빈 드라이브 문자에 붙인다.
    pub fn create() -> Result<Self, String> {
        let dir = std::env::temp_dir().join("facet-test-vhd");
        fs::create_dir_all(&dir).map_err(|e| format!("{}를 만들지 못함: {e}", dir.display()))?;
        let dir = long_path(&dir)?;
        let file = dir.join(format!("test-{}.vhdx", std::process::id()));
        if file.exists() {
            fs::remove_file(&file)
                .map_err(|e| format!("남은 {}를 지우지 못함: {e}", file.display()))?;
        }
        let script = format!(
            "create vdisk file=\"{f}\" maximum={SIZE_MB} type=expandable\n\
             select vdisk file=\"{f}\"\n\
             attach vdisk\n\
             create partition primary\n\
             format fs=ntfs quick label={LABEL}\n",
            f = file.display()
        );
        let mut vhd = Self { file, letter: '?' };
        // 중간에 실패해도 Drop이 떼고 지운다.
        diskpart(&script)?;
        // 루트 경로가 없어도 드라이브 문자가 아직 잡혀 있을 수 있다(앞서 뗀 볼륨의 문자를 마운트
        // 관리자가 늦게 놓는 경우 등). diskpart가 「문자가 비어 있지 않음」으로 거절하면 다음 문자로
        // 해 본다.
        let mut refused = Vec::new();
        for letter in free_letters() {
            let assign = format!(
                "select vdisk file=\"{f}\"\nselect partition 1\nassign letter={letter}\n",
                f = vhd.file.display()
            );
            match diskpart(&assign) {
                Ok(()) => {
                    vhd.letter = letter;
                    if !Path::new(&vhd.root()).exists() {
                        return Err(format!("시험 VHD를 {letter}:에 붙였지만 보이지 않음"));
                    }
                    return Ok(vhd);
                }
                Err(e) => refused.push(format!("{letter}: {e}")),
            }
        }
        Err(format!(
            "시험 VHD에 드라이브 문자를 붙이지 못함:\n{}",
            refused.join("\n")
        ))
    }

    /// 볼륨 루트 경로(예: `T:\`).
    pub fn root(&self) -> String {
        format!("{}:\\", self.letter)
    }
}

impl Drop for TestVhd {
    fn drop(&mut self) {
        let script = format!(
            "select vdisk file=\"{f}\"\ndetach vdisk\n",
            f = self.file.display()
        );
        if let Err(e) = diskpart(&script) {
            eprintln!("xtask: 시험 VHD를 떼지 못함: {e}");
        }
        if self.file.exists()
            && let Err(e) = fs::remove_file(&self.file)
        {
            eprintln!("xtask: {}를 지우지 못함: {e}", self.file.display());
        }
    }
}

/// 8.3 짧은 이름(`RUNNER~1` 같은)을 푼 전체 경로. diskpart는 붙일 때 쓴 경로와 다른 표기로
/// VHD를 고르면 「이미 떼어짐」이라며 떼지 못하므로, 붙이기와 떼기에 같은 긴 경로를 쓴다.
fn long_path(path: &Path) -> Result<PathBuf, String> {
    let full = fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = full.display().to_string();
    Ok(text.strip_prefix(r"\\?\").map_or(full, PathBuf::from))
}

/// 비어 보이는(루트 경로가 없는) 드라이브 문자들(Z부터 거꾸로).
fn free_letters() -> impl Iterator<Item = char> {
    ('G'..='Z')
        .rev()
        .filter(|c| !Path::new(&format!("{c}:\\")).exists())
}

/// `diskpart /s`로 스크립트를 돌린다. 실패하면 diskpart의 출력을 담은 오류를 낸다.
fn diskpart(script: &str) -> Result<(), String> {
    let path = std::env::temp_dir().join(format!("facet-diskpart-{}.txt", std::process::id()));
    fs::write(&path, script).map_err(|e| format!("diskpart 스크립트를 쓰지 못함: {e}"))?;
    let output = Command::new("diskpart").arg("/s").arg(&path).output();
    let _ = fs::remove_file(&path);
    let output = output.map_err(|e| format!("diskpart를 실행하지 못함: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "diskpart 실패({}):\n{}\n--- 스크립트 ---\n{script}",
            output.status,
            String::from_utf8_lossy(&output.stdout)
        ))
    }
}
