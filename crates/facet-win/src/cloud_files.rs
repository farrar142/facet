//! #449: 클라우드 파일 기능(동기화 폴더 판정·고정 상태 바꾸기)과 시험용 동기화 공급자.

use std::path::Path;

use crate::file_ops::FileOpError;

/// #449: 경로가 클라우드 동기화 루트 아래인가(루트 자신 포함).
pub fn is_in_sync_root(_path: &Path) -> bool {
    todo!("#449")
}

/// #449: 고정 상태(항상 유지 = pinned, 공간 확보 = unpinned).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinState {
    Pinned,
    Unpinned,
}

/// #449: 항목의 고정 상태를 바꾼다(폴더는 `recursive`면 안쪽까지).
pub fn set_pin_state(_path: &Path, _state: PinState, _recursive: bool) -> Result<(), FileOpError> {
    todo!("#449")
}

/// #449: 시험용 동기화 공급자. 앱은 부르지 않는다.
pub mod testing {
    use std::fmt;
    use std::path::{Path, PathBuf};

    /// #449: 동기화 루트를 등록하지 못한 까닭.
    #[derive(Debug)]
    pub enum SyncRootError {
        Unsupported(String),
        Failed(String),
    }

    impl fmt::Display for SyncRootError {
        fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
            todo!("#449")
        }
    }

    /// #449: 임시 폴더를 동기화 루트로 등록하고 연결한 시험용 공급자.
    pub struct TestSyncRoot {
        _private: (),
    }

    impl Drop for TestSyncRoot {
        /// #449: 연결을 끊고 등록을 지운다.
        fn drop(&mut self) {} // #449
    }

    impl TestSyncRoot {
        /// #449: 등록하고 연결한다.
        pub fn register(_root: &Path) -> Result<TestSyncRoot, SyncRootError> {
            todo!("#449")
        }

        /// #449: 등록한 루트.
        pub fn root(&self) -> &Path {
            todo!("#449")
        }

        /// #449: 온라인 전용 파일 자리표시를 만든다.
        pub fn add_online_file(&self, _relative: &str, _content: &[u8]) -> PathBuf {
            todo!("#449")
        }

        /// #449: 아직 펼쳐지지 않은 온라인 폴더를 만든다.
        pub fn add_online_folder(&self, _relative: &str, _files: &[(&str, &[u8])]) -> PathBuf {
            todo!("#449")
        }

        /// #449: 내용 요청(FETCH_DATA) 수.
        pub fn fetch_data_count(&self) -> usize {
            todo!("#449")
        }

        /// #449: 목록 요청(FETCH_PLACEHOLDERS) 수.
        pub fn fetch_placeholders_count(&self) -> usize {
            todo!("#449")
        }

        /// #449: 응답하지 않게(멈춤) 한다.
        pub fn set_stall(&self, _stall: bool) {
            todo!("#449")
        }

        /// #449: 내려받기를 실패시킨다.
        pub fn set_fail(&self, _fail: bool) {
            todo!("#449")
        }

        /// #449: 연결을 끊는다(동기화 프로그램 꺼짐).
        pub fn disconnect(&self) {
            todo!("#449")
        }
    }
}
