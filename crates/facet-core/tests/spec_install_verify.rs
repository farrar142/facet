// target: crates/facet-core/tests/spec_install_verify.rs
// AC: #474: 체크섬 불일치·읽을 수 없음은 거절하고 지움, 서명 없음은 거절하고 지움, 신뢰하는 서명과 같은 게시자는 통과, 게시자가 다르면 거절, 서명 뒤 바뀐 파일은 거절, 서명자 읽기
//! 받은 설치 파일 확인(`facet_core::update::verify_download`)과 서명자 읽기(`facet_win::authenticode::signer`)를 임시
//! 폴더의 실제 파일로 확인한다. 체크섬 시험은 모든 플랫폼에서 돈다. 서명 시험은 Windows 전용이다: 테스트용 자체 서명
//! 코드 서명 인증서를 PowerShell로 만들어 로컬 컴퓨터의 신뢰 루트·신뢰할 수 있는 게시자에 잠깐 넣고 이 테스트 실행
//! 파일의 사본에 서명한다(관리자 필요, CI 러너는 관리자). 넣을 수 없으면 건너뛰었다고 출력하고, 끝나면 인증서를 지운다.

use std::fs;
use std::path::{Path, PathBuf};

use facet_core::update::{self, VerifyError, verify_download};
use tempfile::TempDir;

/// `dir`에 받은 파일과 체크섬 파일을 쓴다(체크섬 줄은 `cargo xtask dist`와 같은 형식).
fn downloaded(dir: &Path, name: &str, bytes: &[u8], checksum_line: &str) -> (PathBuf, PathBuf) {
    let file = dir.join(name);
    let checksum = dir.join(format!("{name}.sha256"));
    fs::write(&file, bytes).unwrap();
    fs::write(&checksum, checksum_line).unwrap();
    (file, checksum)
}

fn assert_removed(file: &Path, checksum: &Path) {
    assert!(
        !file.exists(),
        "확인에 실패한 파일은 지운다: {}",
        file.display()
    );
    assert!(
        !checksum.exists(),
        "확인에 실패한 체크섬 파일도 지운다: {}",
        checksum.display()
    );
}

#[test]
#[ignore = "#474"]
fn 체크섬이_다르면_거절하고_두_파일을_지운다() {
    let dir = TempDir::new().unwrap();
    let wrong = format!("{}  facet.msi\n", "0".repeat(64));
    let (file, checksum) = downloaded(dir.path(), "facet.msi", b"not the real msi", &wrong);

    assert_eq!(
        verify_download(&file, &checksum, "facet"),
        Err(VerifyError::ChecksumMismatch)
    );
    assert_removed(&file, &checksum);
}

#[test]
#[ignore = "#474"]
fn 체크섬_파일을_읽을_수_없으면_거절하고_두_파일을_지운다() {
    let dir = TempDir::new().unwrap();
    let (file, checksum) = downloaded(dir.path(), "facet.msi", b"msi", "이건 해시가 아님\n");
    assert_eq!(
        verify_download(&file, &checksum, "facet"),
        Err(VerifyError::ChecksumUnreadable)
    );
    assert_removed(&file, &checksum);

    // 체크섬 파일이 아예 없어도 같다.
    let file = dir.path().join("other.msi");
    fs::write(&file, b"msi").unwrap();
    let missing = dir.path().join("other.msi.sha256");
    assert_eq!(
        verify_download(&file, &missing, "facet"),
        Err(VerifyError::ChecksumUnreadable)
    );
    assert!(!file.exists());
}

#[test]
#[ignore = "#474"]
fn 게시자_이름은_빌드_때_넣은_값이다() {
    let expected = option_env!("FACET_PUBLISHER").filter(|name| !name.is_empty());
    assert_eq!(update::publisher(), expected);
}

#[cfg(windows)]
mod signed {
    use std::process::Command;

    use facet_win::authenticode::{self, SignatureError};

    use super::*;

    /// PowerShell 한 줄짜리 스크립트를 돌려 표준 출력을 돌려준다. 실패하면 `None`.
    fn powershell(script: &str) -> Option<String> {
        let output = Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                script,
            ])
            .output()
            .ok()?;
        if !output.status.success() {
            eprintln!(
                "PowerShell 실패: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            return None;
        }
        Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    /// 소문자 16진 SHA-256.
    fn sha256_of(path: &Path) -> String {
        powershell(&format!(
            "(Get-FileHash -Algorithm SHA256 -LiteralPath '{}').Hash.ToLower()",
            path.display()
        ))
        .expect("Get-FileHash")
    }

    fn checksum_line_for(path: &Path) -> String {
        let name = path.file_name().unwrap().to_string_lossy();
        format!("{}  {name}\n", sha256_of(path))
    }

    /// 서명할 PE 파일: 서명 없는 이 테스트 실행 파일의 사본.
    fn unsigned_pe(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::copy(std::env::current_exe().unwrap(), &path).unwrap();
        path
    }

    /// 로컬 컴퓨터가 잠깐 신뢰하는 테스트용 코드 서명 인증서. 버리면 모든 저장소에서 지운다.
    struct TestCert {
        thumbprint: String,
    }

    impl TestCert {
        fn create(subject: &str) -> Option<TestCert> {
            let script = format!(
                "$ErrorActionPreference = 'Stop'
                 $c = New-SelfSignedCertificate -Type CodeSigningCert -Subject 'CN={subject}' -CertStoreLocation Cert:\\CurrentUser\\My -NotAfter (Get-Date).AddDays(2)
                 try {{
                   $p = Join-Path ([IO.Path]::GetTempPath()) ($c.Thumbprint + '.cer')
                   Export-Certificate -Cert $c -FilePath $p | Out-Null
                   Import-Certificate -FilePath $p -CertStoreLocation Cert:\\LocalMachine\\Root | Out-Null
                   Import-Certificate -FilePath $p -CertStoreLocation Cert:\\LocalMachine\\TrustedPublisher | Out-Null
                   Remove-Item $p
                 }} catch {{
                   Remove-Item ('Cert:\\CurrentUser\\My\\' + $c.Thumbprint) -ErrorAction SilentlyContinue
                   throw
                 }}
                 Write-Output $c.Thumbprint"
            );
            let thumbprint = powershell(&script)?;
            Some(TestCert {
                thumbprint: thumbprint.lines().last()?.trim().to_owned(),
            })
        }

        fn sign(&self, file: &Path) {
            let script = format!(
                "$ErrorActionPreference = 'Stop'
                 $c = Get-Item 'Cert:\\CurrentUser\\My\\{}'
                 $r = Set-AuthenticodeSignature -LiteralPath '{}' -Certificate $c -HashAlgorithm SHA256
                 if ($r.Status -ne 'Valid') {{ throw $r.StatusMessage }}",
                self.thumbprint,
                file.display()
            );
            powershell(&script).expect("테스트 인증서로 서명");
        }
    }

    impl Drop for TestCert {
        fn drop(&mut self) {
            let t = &self.thumbprint;
            let _ = powershell(&format!(
                "Remove-Item 'Cert:\\LocalMachine\\Root\\{t}','Cert:\\LocalMachine\\TrustedPublisher\\{t}','Cert:\\CurrentUser\\My\\{t}' -ErrorAction SilentlyContinue"
            ));
        }
    }

    fn test_cert(subject: &str) -> Option<TestCert> {
        let cert = TestCert::create(subject);
        if cert.is_none() {
            eprintln!("테스트 인증서를 신뢰 루트에 넣지 못해(관리자 아님?) 서명 시험을 건너뛴다");
        }
        cert
    }

    #[test]
    #[ignore = "#474"]
    fn 서명_없는_파일은_체크섬이_맞아도_거절하고_지운다() {
        let dir = TempDir::new().unwrap();
        let file = unsigned_pe(dir.path(), "facet-unsigned.exe");
        assert_eq!(authenticode::signer(&file), Err(SignatureError::NotSigned));

        let checksum = dir.path().join("facet-unsigned.exe.sha256");
        fs::write(&checksum, checksum_line_for(&file)).unwrap();
        assert_eq!(
            verify_download(&file, &checksum, "facet"),
            Err(VerifyError::NotSigned)
        );
        assert_removed(&file, &checksum);
    }

    #[test]
    #[ignore = "#474"]
    fn 신뢰하는_서명과_같은_게시자는_통과하고_다른_게시자는_거절한다() {
        let subject = format!("facet test publisher {}", std::process::id());
        let Some(cert) = test_cert(&subject) else {
            return;
        };
        let dir = TempDir::new().unwrap();

        let file = unsigned_pe(dir.path(), "facet-signed.exe");
        cert.sign(&file);
        assert_eq!(authenticode::signer(&file).as_deref(), Ok(subject.as_str()));

        let checksum = dir.path().join("facet-signed.exe.sha256");
        fs::write(&checksum, checksum_line_for(&file)).unwrap();
        assert_eq!(verify_download(&file, &checksum, &subject), Ok(()));
        assert!(file.exists(), "확인을 통과한 파일은 남는다");
        assert!(checksum.exists());

        assert_eq!(
            verify_download(&file, &checksum, "Someone Else"),
            Err(VerifyError::PublisherMismatch {
                found: subject.clone()
            })
        );
        assert_removed(&file, &checksum);
    }

    #[test]
    #[ignore = "#474"]
    fn 서명한_뒤_바뀐_파일은_거절하고_지운다() {
        let subject = format!("facet test tamper {}", std::process::id());
        let Some(cert) = test_cert(&subject) else {
            return;
        };
        let dir = TempDir::new().unwrap();

        let file = unsigned_pe(dir.path(), "facet-tampered.exe");
        cert.sign(&file);
        // 머리글 뒤 첫 구역 안의 한 바이트를 바꾼다(서명이 덮는 내용).
        let mut bytes = fs::read(&file).unwrap();
        bytes[2048] ^= 0xFF;
        fs::write(&file, &bytes).unwrap();
        assert!(matches!(
            authenticode::signer(&file),
            Err(SignatureError::Invalid(_))
        ));

        // 체크섬은 바뀐 파일에 맞춘다: 바꿔치기한 사람이 체크섬도 바꾼 경우.
        let checksum = dir.path().join("facet-tampered.exe.sha256");
        fs::write(&checksum, checksum_line_for(&file)).unwrap();
        assert_eq!(
            verify_download(&file, &checksum, &subject),
            Err(VerifyError::InvalidSignature)
        );
        assert_removed(&file, &checksum);
    }
}
