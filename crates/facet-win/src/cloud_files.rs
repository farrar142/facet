//! 클라우드 파일 기능(Windows Cloud Filter API): 동기화 폴더 판정·고정 상태 바꾸기와 시험용 동기화 공급자(#449).
//!
//! 실제 내려받기·비우기는 동기화 프로그램(OneDrive 등)의 몫이고 여기서는 고정 상태만 바꾼다. Windows 구현은
//! `windows_cloud_files.rs`에 있다. Windows가 아니면 동기화 폴더가 없고 고정 상태 바꾸기는 `Unsupported`다.

use std::path::Path;

use crate::file_ops::FileOpError;

/// 경로가 클라우드 동기화 루트 아래인가(루트 자신 포함). 부르는 쪽(코어)이 위치마다 캐시한다.
///
/// 파일 내용을 읽지 않으므로 온라인 전용 항목을 내려받지 않는다. 경로가 없거나 판정하지 못하면 `false`.
pub fn is_in_sync_root(path: &Path) -> bool {
    #[cfg(windows)]
    {
        crate::windows_cloud_files::is_in_sync_root(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

/// 고정 상태(항상 유지 = pinned, 공간 확보 = unpinned).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinState {
    /// 「항상 유지」: 내용을 이 기기에 둔다(`FILE_ATTRIBUTE_PINNED`).
    Pinned,
    /// 「공간 확보」: 내용을 비워도 된다(`FILE_ATTRIBUTE_UNPINNED`).
    Unpinned,
}

/// 항목의 고정 상태를 바꾼다(폴더는 `recursive`면 안쪽까지). 동기화 폴더가 아닌 곳에서는 실패한다.
pub fn set_pin_state(path: &Path, state: PinState, recursive: bool) -> Result<(), FileOpError> {
    #[cfg(windows)]
    {
        crate::windows_cloud_files::set_pin_state(path, state, recursive)
    }
    #[cfg(not(windows))]
    {
        let _ = (path, state, recursive);
        Err(FileOpError::unsupported())
    }
}

/// 시험용 동기화 공급자. 앱은 부르지 않는다(`drop_target::testing`과 같은 자리).
///
/// 임시 폴더를 동기화 루트로 등록하고 이 프로세스에서 연결해 진짜 온라인 전용 항목(파일 자리표시와 아직 펼쳐지지
/// 않은 온라인 폴더)을 만든다. 내용 요청(FETCH_DATA)과 목록 요청(FETCH_PLACEHOLDERS)을 세고, 멈춤·내려받기
/// 실패·연결 끊기를 흉내 낸다. 지우기·이름바꾸기 알림은 받아들인다. 클라우드 파일 필터는 연결한 프로세스 자신의
/// 폴더 열거에는 목록 요청을 보내지 않으므로, 펼쳐지지 않은 폴더를 펼치려면 다른 프로세스에서 열거한다.
pub mod testing {
    use std::fmt;
    use std::path::{Path, PathBuf};

    /// 동기화 루트를 등록하지 못한 까닭.
    #[derive(Debug)]
    pub enum SyncRootError {
        /// 이 환경에서는 동기화 루트를 쓸 수 없다(클라우드 파일 필터 없음, Windows가 아님 등).
        Unsupported(String),
        /// 등록·연결이 실패했다.
        Failed(String),
    }

    impl fmt::Display for SyncRootError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                SyncRootError::Unsupported(why) => {
                    write!(f, "동기화 루트를 쓸 수 없는 환경: {why}")
                }
                SyncRootError::Failed(why) => write!(f, "동기화 루트 등록 실패: {why}"),
            }
        }
    }

    impl std::error::Error for SyncRootError {}

    /// 임시 폴더를 동기화 루트로 등록하고 연결한 시험용 공급자. 버리면 연결을 끊고 등록을 지운다.
    pub struct TestSyncRoot {
        #[cfg(windows)]
        inner: crate::windows_cloud_files::TestSyncRoot,
        #[cfg(not(windows))]
        root: PathBuf,
    }

    impl TestSyncRoot {
        /// 등록하고 연결한다.
        pub fn register(root: &Path) -> Result<TestSyncRoot, SyncRootError> {
            #[cfg(windows)]
            {
                crate::windows_cloud_files::TestSyncRoot::register(root)
                    .map(|inner| TestSyncRoot { inner })
            }
            #[cfg(not(windows))]
            {
                let _ = root;
                Err(SyncRootError::Unsupported("Windows가 아님".to_owned()))
            }
        }

        /// 등록한 루트.
        pub fn root(&self) -> &Path {
            #[cfg(windows)]
            {
                self.inner.root()
            }
            #[cfg(not(windows))]
            {
                &self.root
            }
        }

        /// 온라인 전용 파일 자리표시(내용은 FETCH_DATA 때 줌). 만든 경로를 돌려준다. 상위 폴더는 이미 있어야
        /// 한다. 만들지 못하면 패닉한다(시험 도우미).
        pub fn add_online_file(&self, relative: &str, content: &[u8]) -> PathBuf {
            #[cfg(windows)]
            {
                self.inner.add_online_file(relative, content)
            }
            #[cfg(not(windows))]
            {
                let _ = content;
                self.root.join(relative)
            }
        }

        /// 아직 펼쳐지지 않은 온라인 폴더(안의 파일은 FETCH_PLACEHOLDERS 때 온라인 전용 파일로 줌).
        pub fn add_online_folder(&self, relative: &str, files: &[(&str, &[u8])]) -> PathBuf {
            #[cfg(windows)]
            {
                self.inner.add_online_folder(relative, files)
            }
            #[cfg(not(windows))]
            {
                let _ = files;
                self.root.join(relative)
            }
        }

        /// 내용 요청(FETCH_DATA) 수.
        pub fn fetch_data_count(&self) -> usize {
            #[cfg(windows)]
            {
                self.inner.fetch_data_count()
            }
            #[cfg(not(windows))]
            {
                0
            }
        }

        /// 목록 요청(FETCH_PLACEHOLDERS) 수.
        pub fn fetch_placeholders_count(&self) -> usize {
            #[cfg(windows)]
            {
                self.inner.fetch_placeholders_count()
            }
            #[cfg(not(windows))]
            {
                0
            }
        }

        /// 참이면 내용 요청에 답하지 않는다(멈춤). 거짓으로 바꾸면 기다리던 요청에 답한다.
        pub fn set_stall(&self, stall: bool) {
            #[cfg(windows)]
            self.inner.set_stall(stall);
            #[cfg(not(windows))]
            let _ = stall;
        }

        /// 참이면 내용 요청을 실패로 답한다(네트워크 끊김 흉내).
        pub fn set_fail(&self, fail: bool) {
            #[cfg(windows)]
            self.inner.set_fail(fail);
            #[cfg(not(windows))]
            let _ = fail;
        }

        /// 연결을 끊는다(동기화 프로그램 꺼짐). 등록은 남는다.
        pub fn disconnect(&self) {
            #[cfg(windows)]
            self.inner.disconnect();
        }
    }
}
