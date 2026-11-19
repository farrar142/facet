//! [`crate::cloud_files`]의 Windows 구현(클라우드 파일 API, `cldapi`). `unsafe`는 이 파일 안에 가둔다.
//!
//! 시험용 공급자의 콜백은 클라우드 파일 필터의 스레드 풀에서 불린다. 콜백 맥락은 공급자 상태(`Shared`)의
//! `Arc`를 원시 포인터로 넘긴 것이고, 연결을 끊은 뒤에야 놓는다(끊은 뒤에는 콜백이 오지 않는다).

use std::collections::HashMap;
use std::ffi::c_void;
use std::fs::OpenOptions;
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use windows::Win32::Foundation::{HANDLE, NTSTATUS};
use windows::Win32::Storage::CloudFilters::{
    CF_CALLBACK_INFO, CF_CALLBACK_PARAMETERS, CF_CALLBACK_REGISTRATION,
    CF_CALLBACK_TYPE_FETCH_DATA, CF_CALLBACK_TYPE_FETCH_PLACEHOLDERS, CF_CALLBACK_TYPE_NONE,
    CF_CALLBACK_TYPE_NOTIFY_DELETE, CF_CALLBACK_TYPE_NOTIFY_RENAME, CF_CONNECT_FLAG_NONE,
    CF_CONNECTION_KEY, CF_CREATE_FLAG_NONE, CF_FS_METADATA, CF_HARDLINK_POLICY_NONE,
    CF_HYDRATION_POLICY, CF_HYDRATION_POLICY_FULL, CF_HYDRATION_POLICY_MODIFIER_NONE,
    CF_INSYNC_POLICY_NONE, CF_OPERATION_ACK_DELETE_FLAG_NONE, CF_OPERATION_ACK_RENAME_FLAG_NONE,
    CF_OPERATION_INFO, CF_OPERATION_PARAMETERS, CF_OPERATION_PARAMETERS_0,
    CF_OPERATION_PARAMETERS_0_0, CF_OPERATION_PARAMETERS_0_4, CF_OPERATION_PARAMETERS_0_6,
    CF_OPERATION_PARAMETERS_0_7, CF_OPERATION_TRANSFER_DATA_FLAG_NONE,
    CF_OPERATION_TRANSFER_PLACEHOLDERS_FLAG_DISABLE_ON_DEMAND_POPULATION, CF_OPERATION_TYPE,
    CF_OPERATION_TYPE_ACK_DELETE, CF_OPERATION_TYPE_ACK_RENAME, CF_OPERATION_TYPE_TRANSFER_DATA,
    CF_OPERATION_TYPE_TRANSFER_PLACEHOLDERS, CF_PIN_STATE_PINNED, CF_PIN_STATE_UNPINNED,
    CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC, CF_PLACEHOLDER_CREATE_FLAGS,
    CF_PLACEHOLDER_CREATE_INFO, CF_PLACEHOLDER_MANAGEMENT_POLICY_DEFAULT, CF_POPULATION_POLICY,
    CF_POPULATION_POLICY_FULL, CF_POPULATION_POLICY_MODIFIER_NONE,
    CF_REGISTER_FLAG_DISABLE_ON_DEMAND_POPULATION_ON_ROOT, CF_SET_PIN_FLAG_NONE,
    CF_SET_PIN_FLAG_RECURSE, CF_SYNC_POLICIES, CF_SYNC_REGISTRATION, CF_SYNC_ROOT_BASIC_INFO,
    CF_SYNC_ROOT_INFO_BASIC, CfConnectSyncRoot, CfCreatePlaceholders, CfDisconnectSyncRoot,
    CfExecute, CfGetPlatformInfo, CfGetSyncRootInfoByPath, CfRegisterSyncRoot, CfSetPinState,
    CfUnregisterSyncRoot,
};
use windows::Win32::Storage::FileSystem::FILE_BASIC_INFO;
use windows::core::{GUID, PCWSTR};

use crate::cloud_files::PinState;
use crate::cloud_files::testing::SyncRootError;
use crate::file_ops::{FileOpError, win32_from_hresult};
use crate::windows_file_ops::from_windows;

const FILE_READ_ATTRIBUTES: u32 = 0x80;
const FILE_WRITE_ATTRIBUTES: u32 = 0x100;
const FILE_SHARE_ALL: u32 = 0x1 | 0x2 | 0x4;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
/// 내려받기 실패 모드의 답(`STATUS_UNSUCCESSFUL`).
const STATUS_UNSUCCESSFUL: NTSTATUS = NTSTATUS(0xC000_0001_u32 as i32);
const STATUS_SUCCESS: NTSTATUS = NTSTATUS(0);

/// 이 환경에 클라우드 파일 기능이 없음을 뜻하는 Win32 코드들.
const UNSUPPORTED_CODES: [u32; 5] = [
    1,     // ERROR_INVALID_FUNCTION: 필터가 없음
    50,    // ERROR_NOT_SUPPORTED
    120,   // ERROR_CALL_NOT_IMPLEMENTED
    665,   // ERROR_FILE_SYSTEM_LIMITATION
    0x17C, // ERROR_CLOUD_FILE_NOT_SUPPORTED
];

fn wide(path: &Path) -> Vec<u16> {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    absolute
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn wide_str(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(crate) fn is_in_sync_root(path: &Path) -> bool {
    let path = wide(path);
    let mut info = CF_SYNC_ROOT_BASIC_INFO::default();
    // SAFETY: 경로는 NUL로 끝나는 UTF-16이고 호출 동안 산다. 버퍼는 `info`이고 크기를 그대로 넘긴다.
    unsafe {
        CfGetSyncRootInfoByPath(
            PCWSTR(path.as_ptr()),
            CF_SYNC_ROOT_INFO_BASIC,
            (&raw mut info).cast::<c_void>(),
            size_of::<CF_SYNC_ROOT_BASIC_INFO>() as u32,
            None,
        )
    }
    .is_ok()
}

/// 동기화 루트 밖(`ERROR_CLOUD_FILE_NOT_IN_SYNC_ROOT`).
const ERROR_CLOUD_FILE_NOT_IN_SYNC_ROOT: u32 = 390;
/// 등록 정보 저장소가 다른 등록·지우기와 겹쳐 항목을 놓침(`ERROR_NOT_FOUND`).
const ERROR_NOT_FOUND: u32 = 1168;
/// 겹친 등록이 실패하면 다시 해 볼 횟수.
const REGISTER_ATTEMPTS: usize = 5;

/// 동기화 루트 등록·지우기를 프로세스 안에서 하나씩 한다. 같은 공급자 이름으로 나란히 등록·지우면 셸의 등록 정보
/// 저장소가 서로의 항목을 놓쳐 `ERROR_NOT_FOUND`(0x80070490)로 실패한다(시험이 나란히 돈다).
static REGISTRY: Mutex<()> = Mutex::new(());

pub(crate) fn set_pin_state(
    path: &Path,
    state: PinState,
    recursive: bool,
) -> Result<(), FileOpError> {
    // 동기화 루트 밖의 보통 파일에도 `CfSetPinState`가 성공하고 속성을 켜므로 먼저 막는다.
    if !is_in_sync_root(path) {
        return Err(if path.exists() {
            FileOpError::from_win32(ERROR_CLOUD_FILE_NOT_IN_SYNC_ROOT)
        } else {
            FileOpError::from_win32(crate::file_ops::ERROR_FILE_NOT_FOUND)
        });
    }
    let file = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES)
        .share_mode(FILE_SHARE_ALL)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .map_err(crate::windows_file_ops::from_io)?;
    let pin = match state {
        PinState::Pinned => CF_PIN_STATE_PINNED,
        PinState::Unpinned => CF_PIN_STATE_UNPINNED,
    };
    let flags = if recursive {
        CF_SET_PIN_FLAG_RECURSE
    } else {
        CF_SET_PIN_FLAG_NONE
    };
    // SAFETY: 핸들은 `file`이 쥔 열린 핸들이고 호출 동안 산다. 겹친 입출력은 쓰지 않는다(None).
    unsafe { CfSetPinState(HANDLE(file.as_raw_handle()), pin, flags, None) }
        .map_err(|error| from_windows(&error))
}

/// 시험용 공급자가 아는 항목(키는 루트 기준 상대 경로, `/` 구분).
enum Item {
    File(Arc<[u8]>),
    /// 펼쳐지지 않은 폴더: 안의 파일 이름들(내용은 그 키의 `File`).
    Folder(Vec<String>),
}

#[derive(Default)]
struct Shared {
    items: Mutex<HashMap<String, Item>>,
    fetch_data: AtomicUsize,
    fetch_placeholders: AtomicUsize,
    stall: Mutex<bool>,
    resume: Condvar,
    fail: AtomicBool,
}

impl Shared {
    fn wait_while_stalled(&self) {
        let mut stalled = self.stall.lock().unwrap_or_else(|e| e.into_inner());
        while *stalled {
            stalled = self.resume.wait(stalled).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn set_stall(&self, stall: bool) {
        *self.stall.lock().unwrap_or_else(|e| e.into_inner()) = stall;
        self.resume.notify_all();
    }
}

pub(crate) struct TestSyncRoot {
    root: PathBuf,
    shared: Arc<Shared>,
    /// 콜백 맥락으로 넘긴 `Arc<Shared>`의 원시 포인터(주소). 연결을 끊은 뒤 [`Drop`]에서 놓는다.
    context: usize,
    connection: Mutex<Option<CF_CONNECTION_KEY>>,
}

fn classify_register(error: &windows::core::Error, what: &str) -> SyncRootError {
    let hresult = error.code().0;
    let text = format!("{what}: {error} ({hresult:#010x})");
    match win32_from_hresult(hresult) {
        Some(code) if UNSUPPORTED_CODES.contains(&code) => SyncRootError::Unsupported(text),
        _ => SyncRootError::Failed(text),
    }
}

fn now_filetime() -> i64 {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (since.as_secs() as i64 + 11_644_473_600) * 10_000_000 + i64::from(since.subsec_nanos() / 100)
}

fn metadata(attributes: u32, size: usize) -> CF_FS_METADATA {
    let time = now_filetime();
    CF_FS_METADATA {
        BasicInfo: FILE_BASIC_INFO {
            CreationTime: time,
            LastAccessTime: time,
            LastWriteTime: time,
            ChangeTime: time,
            FileAttributes: attributes,
        },
        FileSize: size as i64,
    }
}

/// 자리표시 하나를 만들 재료(이름·키는 호출 동안 살아 있어야 한다).
struct Placeholder {
    name: Vec<u16>,
    key: Vec<u8>,
    folder: bool,
    size: usize,
    flags: CF_PLACEHOLDER_CREATE_FLAGS,
}

impl Placeholder {
    fn info(&self) -> CF_PLACEHOLDER_CREATE_INFO {
        let attributes = if self.folder {
            FILE_ATTRIBUTE_DIRECTORY
        } else {
            FILE_ATTRIBUTE_NORMAL
        };
        CF_PLACEHOLDER_CREATE_INFO {
            RelativeFileName: PCWSTR(self.name.as_ptr()),
            FsMetadata: metadata(attributes, self.size),
            FileIdentity: self.key.as_ptr().cast(),
            FileIdentityLength: self.key.len() as u32,
            Flags: self.flags,
            ..Default::default()
        }
    }
}

fn file_name(relative: &str) -> &str {
    relative.rsplit(['/', '\\']).next().unwrap_or(relative)
}

fn normalize(relative: &str) -> String {
    relative.replace('\\', "/").trim_matches('/').to_owned()
}

impl TestSyncRoot {
    pub(crate) fn register(root: &Path) -> Result<TestSyncRoot, SyncRootError> {
        // SAFETY: 인자 없는 질의다.
        unsafe { CfGetPlatformInfo() }
            .map_err(|error| SyncRootError::Unsupported(format!("CfGetPlatformInfo: {error}")))?;
        let path = wide(root);
        let provider_id = GUID::new().map_err(|error| SyncRootError::Failed(error.to_string()))?;
        let name = wide_str("facet-test");
        let version = wide_str("1");
        let identity = format!("{provider_id:?}").into_bytes();
        let registration = CF_SYNC_REGISTRATION {
            StructSize: size_of::<CF_SYNC_REGISTRATION>() as u32,
            ProviderName: PCWSTR(name.as_ptr()),
            ProviderVersion: PCWSTR(version.as_ptr()),
            SyncRootIdentity: identity.as_ptr().cast(),
            SyncRootIdentityLength: identity.len() as u32,
            FileIdentity: std::ptr::null(),
            FileIdentityLength: 0,
            ProviderId: provider_id,
        };
        let policies = CF_SYNC_POLICIES {
            StructSize: size_of::<CF_SYNC_POLICIES>() as u32,
            Hydration: CF_HYDRATION_POLICY {
                Primary: CF_HYDRATION_POLICY_FULL,
                Modifier: CF_HYDRATION_POLICY_MODIFIER_NONE,
            },
            Population: CF_POPULATION_POLICY {
                Primary: CF_POPULATION_POLICY_FULL,
                Modifier: CF_POPULATION_POLICY_MODIFIER_NONE,
            },
            InSync: CF_INSYNC_POLICY_NONE,
            HardLink: CF_HARDLINK_POLICY_NONE,
            PlaceholderManagement: CF_PLACEHOLDER_MANAGEMENT_POLICY_DEFAULT,
        };
        let mut attempt = 0;
        loop {
            let registered = {
                let _registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
                // SAFETY: 경로·등록 정보·정책은 모두 이 함수의 지역 값이고 호출 동안 산다.
                unsafe {
                    CfRegisterSyncRoot(
                        PCWSTR(path.as_ptr()),
                        &registration,
                        &policies,
                        CF_REGISTER_FLAG_DISABLE_ON_DEMAND_POPULATION_ON_ROOT,
                    )
                }
            };
            match registered {
                Ok(()) => break,
                Err(error)
                    if attempt + 1 < REGISTER_ATTEMPTS
                        && win32_from_hresult(error.code().0) == Some(ERROR_NOT_FOUND) =>
                {
                    attempt += 1;
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(error) => return Err(classify_register(&error, "CfRegisterSyncRoot")),
            }
        }

        let shared = Arc::new(Shared::default());
        let context = Arc::into_raw(Arc::clone(&shared));
        let callbacks = [
            CF_CALLBACK_REGISTRATION {
                Type: CF_CALLBACK_TYPE_FETCH_DATA,
                Callback: Some(on_fetch_data),
            },
            CF_CALLBACK_REGISTRATION {
                Type: CF_CALLBACK_TYPE_FETCH_PLACEHOLDERS,
                Callback: Some(on_fetch_placeholders),
            },
            CF_CALLBACK_REGISTRATION {
                Type: CF_CALLBACK_TYPE_NOTIFY_DELETE,
                Callback: Some(on_notify_delete),
            },
            CF_CALLBACK_REGISTRATION {
                Type: CF_CALLBACK_TYPE_NOTIFY_RENAME,
                Callback: Some(on_notify_rename),
            },
            CF_CALLBACK_REGISTRATION {
                Type: CF_CALLBACK_TYPE_NONE,
                Callback: None,
            },
        ];
        // SAFETY: 콜백 표는 `CF_CALLBACK_TYPE_NONE`으로 끝나고 호출 동안 산다(필터가 복사함). 맥락 포인터는
        // `Arc<Shared>` 하나를 쥔 것이고 연결을 끊은 뒤에야 놓는다.
        let connected = unsafe {
            CfConnectSyncRoot(
                PCWSTR(path.as_ptr()),
                callbacks.as_ptr(),
                Some(context.cast::<c_void>()),
                CF_CONNECT_FLAG_NONE,
            )
        };
        match connected {
            Ok(key) => Ok(TestSyncRoot {
                root: root.to_path_buf(),
                shared,
                context: context as usize,
                connection: Mutex::new(Some(key)),
            }),
            Err(error) => {
                // SAFETY: 연결하지 못했으므로 콜백은 이 맥락을 쓰지 않는다. 위에서 만든 `Arc` 하나를 되돌린다.
                drop(unsafe { Arc::from_raw(context) });
                unregister(&path);
                Err(classify_register(&error, "CfConnectSyncRoot"))
            }
        }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    fn create(&self, relative: &str, placeholder: &Placeholder) -> PathBuf {
        let target = self.root.join(relative);
        let parent = target.parent().unwrap_or(&self.root).to_path_buf();
        let base = wide(&parent);
        let mut infos = [placeholder.info()];
        // SAFETY: 기준 폴더 경로와 자리표시 정보(이름·키는 `placeholder`가 쥠)는 호출 동안 산다.
        let created = unsafe {
            CfCreatePlaceholders(PCWSTR(base.as_ptr()), &mut infos, CF_CREATE_FLAG_NONE, None)
        };
        if let Err(error) = created {
            panic!("자리표시를 만들지 못했다({}): {error}", target.display());
        }
        target
    }

    pub(crate) fn add_online_file(&self, relative: &str, content: &[u8]) -> PathBuf {
        let key = normalize(relative);
        self.lock_items()
            .insert(key.clone(), Item::File(Arc::from(content)));
        let placeholder = Placeholder {
            name: wide_str(file_name(&key)),
            key: key.clone().into_bytes(),
            folder: false,
            size: content.len(),
            flags: CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC,
        };
        self.create(&key, &placeholder)
    }

    pub(crate) fn add_online_folder(&self, relative: &str, files: &[(&str, &[u8])]) -> PathBuf {
        let key = normalize(relative);
        {
            let mut items = self.lock_items();
            for (name, content) in files {
                items.insert(format!("{key}/{name}"), Item::File(Arc::from(*content)));
            }
            items.insert(
                key.clone(),
                Item::Folder(files.iter().map(|(name, _)| (*name).to_owned()).collect()),
            );
        }
        let placeholder = Placeholder {
            name: wide_str(file_name(&key)),
            key: key.clone().into_bytes(),
            folder: true,
            size: 0,
            // 채우기를 끄지 않아 펼쳐지지 않은 폴더로 남는다.
            flags: CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC,
        };
        self.create(&key, &placeholder)
    }

    fn lock_items(&self) -> std::sync::MutexGuard<'_, HashMap<String, Item>> {
        self.shared.items.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn fetch_data_count(&self) -> usize {
        self.shared.fetch_data.load(Ordering::SeqCst)
    }

    pub(crate) fn fetch_placeholders_count(&self) -> usize {
        self.shared.fetch_placeholders.load(Ordering::SeqCst)
    }

    pub(crate) fn set_stall(&self, stall: bool) {
        self.shared.set_stall(stall);
    }

    pub(crate) fn set_fail(&self, fail: bool) {
        self.shared.fail.store(fail, Ordering::SeqCst);
    }

    pub(crate) fn disconnect(&self) {
        // 멈춘 콜백이 끊기를 붙잡지 않게 먼저 푼다.
        self.shared.set_stall(false);
        let key = self
            .connection
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(key) = key {
            // SAFETY: `CfConnectSyncRoot`가 준 연결 열쇠이고 한 번만 끊는다(`take`).
            let _ = unsafe { CfDisconnectSyncRoot(key) };
        }
    }
}

/// 등록을 지운다(`REGISTRY` 잠금 안에서).
fn unregister(path: &[u16]) {
    let _registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: 경로는 NUL로 끝나는 UTF-16이고 호출 동안 산다.
    let _ = unsafe { CfUnregisterSyncRoot(PCWSTR(path.as_ptr())) };
}

impl Drop for TestSyncRoot {
    fn drop(&mut self) {
        self.disconnect();
        unregister(&wide(&self.root));
        // SAFETY: `register`가 `Arc::into_raw`로 넘긴 포인터이고, 연결을 끊었으므로 더는 콜백이 쓰지 않는다.
        drop(unsafe { Arc::from_raw(self.context as *const Shared) });
    }
}

/// 콜백 맥락과 항목 키(자리표시의 파일 정체성).
///
/// # Safety
/// `info`는 필터가 넘긴 유효한 콜백 정보이고 맥락은 [`TestSyncRoot::register`]가 넘긴 `Arc<Shared>`다.
unsafe fn context_of(info: &CF_CALLBACK_INFO) -> (&Shared, String) {
    // SAFETY: 맥락은 연결이 끊길 때까지 살아 있는 `Shared`를 가리킨다(부르는 쪽의 약속).
    let shared = unsafe { &*info.CallbackContext.cast::<Shared>() };
    let key = if info.FileIdentity.is_null() || info.FileIdentityLength == 0 {
        String::new()
    } else {
        // SAFETY: 필터가 정체성 바이트와 그 길이를 함께 넘긴다.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                info.FileIdentity.cast::<u8>(),
                info.FileIdentityLength as usize,
            )
        };
        String::from_utf8_lossy(bytes).into_owned()
    };
    (shared, key)
}

fn operation(info: &CF_CALLBACK_INFO, kind: CF_OPERATION_TYPE) -> CF_OPERATION_INFO {
    CF_OPERATION_INFO {
        StructSize: size_of::<CF_OPERATION_INFO>() as u32,
        Type: kind,
        ConnectionKey: info.ConnectionKey,
        TransferKey: info.TransferKey,
        CorrelationVector: info.CorrelationVector,
        SyncStatus: std::ptr::null(),
        RequestKey: info.RequestKey,
    }
}

fn execute(op: &CF_OPERATION_INFO, anonymous: CF_OPERATION_PARAMETERS_0, field_size: usize) {
    let mut params = CF_OPERATION_PARAMETERS {
        ParamSize: (offset_of!(CF_OPERATION_PARAMETERS, Anonymous) + field_size) as u32,
        Anonymous: anonymous,
    };
    // SAFETY: 작업 정보와 매개변수는 호출 동안 사는 지역 값이고, 매개변수가 가리키는 버퍼는 부르는 쪽이 쥔다.
    let _ = unsafe { CfExecute(op, &mut params) };
}

fn fetch_data(info: &CF_CALLBACK_INFO, params: &CF_CALLBACK_PARAMETERS) {
    // SAFETY: 이 콜백은 등록한 연결에서만 오고 맥락은 그 연결 동안 산다.
    let (shared, key) = unsafe { context_of(info) };
    shared.fetch_data.fetch_add(1, Ordering::SeqCst);
    shared.wait_while_stalled();
    // SAFETY: FETCH_DATA 콜백의 매개변수는 `FetchData` 변형이다.
    let request = unsafe { params.Anonymous.FetchData };
    let content = match shared
        .items
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
    {
        Some(Item::File(content)) => Some(Arc::clone(content)),
        _ => None,
    };
    let offset = request.RequiredFileOffset.max(0) as usize;
    let op = operation(info, CF_OPERATION_TYPE_TRANSFER_DATA);
    let content = content.filter(|c| !shared.fail.load(Ordering::SeqCst) && offset <= c.len());
    // 버퍼(`content`)는 `CfExecute`가 돌아온 뒤에 놓인다.
    let transfer = match &content {
        Some(content) => {
            let rest = &content[offset..];
            CF_OPERATION_PARAMETERS_0_0 {
                Flags: CF_OPERATION_TRANSFER_DATA_FLAG_NONE,
                CompletionStatus: STATUS_SUCCESS,
                Buffer: rest.as_ptr().cast(),
                Offset: offset as i64,
                Length: rest.len() as i64,
            }
        }
        None => CF_OPERATION_PARAMETERS_0_0 {
            Flags: CF_OPERATION_TRANSFER_DATA_FLAG_NONE,
            CompletionStatus: STATUS_UNSUCCESSFUL,
            Buffer: std::ptr::null(),
            Offset: request.RequiredFileOffset,
            Length: request.RequiredLength,
        },
    };
    execute(
        &op,
        CF_OPERATION_PARAMETERS_0 {
            TransferData: transfer,
        },
        size_of::<CF_OPERATION_PARAMETERS_0_0>(),
    );
}

fn fetch_placeholders(info: &CF_CALLBACK_INFO) {
    // SAFETY: 이 콜백은 등록한 연결에서만 오고 맥락은 그 연결 동안 산다.
    let (shared, key) = unsafe { context_of(info) };
    shared.fetch_placeholders.fetch_add(1, Ordering::SeqCst);
    let children = match shared
        .items
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
    {
        Some(Item::Folder(children)) => children.clone(),
        _ => Vec::new(),
    };
    let sizes: Vec<usize> = {
        let items = shared.items.lock().unwrap_or_else(|e| e.into_inner());
        children
            .iter()
            .map(|name| match items.get(&format!("{key}/{name}")) {
                Some(Item::File(content)) => content.len(),
                _ => 0,
            })
            .collect()
    };
    let placeholders: Vec<Placeholder> = children
        .iter()
        .zip(&sizes)
        .map(|(name, size)| Placeholder {
            name: wide_str(name),
            key: format!("{key}/{name}").into_bytes(),
            folder: false,
            size: *size,
            flags: CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC,
        })
        .collect();
    let mut infos: Vec<CF_PLACEHOLDER_CREATE_INFO> =
        placeholders.iter().map(Placeholder::info).collect();
    let op = operation(info, CF_OPERATION_TYPE_TRANSFER_PLACEHOLDERS);
    execute(
        &op,
        CF_OPERATION_PARAMETERS_0 {
            TransferPlaceholders: CF_OPERATION_PARAMETERS_0_4 {
                Flags: CF_OPERATION_TRANSFER_PLACEHOLDERS_FLAG_DISABLE_ON_DEMAND_POPULATION,
                CompletionStatus: STATUS_SUCCESS,
                PlaceholderTotalCount: infos.len() as i64,
                PlaceholderArray: if infos.is_empty() {
                    std::ptr::null_mut()
                } else {
                    infos.as_mut_ptr()
                },
                PlaceholderCount: infos.len() as u32,
                EntriesProcessed: 0,
            },
        },
        size_of::<CF_OPERATION_PARAMETERS_0_4>(),
    );
    drop(placeholders);
}

fn ack_delete(info: &CF_CALLBACK_INFO) {
    let op = operation(info, CF_OPERATION_TYPE_ACK_DELETE);
    execute(
        &op,
        CF_OPERATION_PARAMETERS_0 {
            AckDelete: CF_OPERATION_PARAMETERS_0_7 {
                Flags: CF_OPERATION_ACK_DELETE_FLAG_NONE,
                CompletionStatus: STATUS_SUCCESS,
            },
        },
        size_of::<CF_OPERATION_PARAMETERS_0_7>(),
    );
}

fn ack_rename(info: &CF_CALLBACK_INFO) {
    let op = operation(info, CF_OPERATION_TYPE_ACK_RENAME);
    execute(
        &op,
        CF_OPERATION_PARAMETERS_0 {
            AckRename: CF_OPERATION_PARAMETERS_0_6 {
                Flags: CF_OPERATION_ACK_RENAME_FLAG_NONE,
                CompletionStatus: STATUS_SUCCESS,
            },
        },
        size_of::<CF_OPERATION_PARAMETERS_0_6>(),
    );
}

/// 콜백 몸체를 패닉이 FFI 경계를 넘지 않게 부른다.
fn guarded(body: impl FnOnce()) {
    let _ = panic::catch_unwind(AssertUnwindSafe(body));
}

unsafe extern "system" fn on_fetch_data(
    info: *const CF_CALLBACK_INFO,
    params: *const CF_CALLBACK_PARAMETERS,
) {
    // SAFETY: 필터는 콜백 동안 유효한 정보와 매개변수를 넘긴다.
    let (info, params) = unsafe { (&*info, &*params) };
    guarded(|| fetch_data(info, params));
}

unsafe extern "system" fn on_fetch_placeholders(
    info: *const CF_CALLBACK_INFO,
    _params: *const CF_CALLBACK_PARAMETERS,
) {
    // SAFETY: 필터는 콜백 동안 유효한 정보를 넘긴다.
    let info = unsafe { &*info };
    guarded(|| fetch_placeholders(info));
}

unsafe extern "system" fn on_notify_delete(
    info: *const CF_CALLBACK_INFO,
    _params: *const CF_CALLBACK_PARAMETERS,
) {
    // SAFETY: 필터는 콜백 동안 유효한 정보를 넘긴다.
    let info = unsafe { &*info };
    guarded(|| ack_delete(info));
}

unsafe extern "system" fn on_notify_rename(
    info: *const CF_CALLBACK_INFO,
    _params: *const CF_CALLBACK_PARAMETERS,
) {
    // SAFETY: 필터는 콜백 동안 유효한 정보를 넘긴다.
    let info = unsafe { &*info };
    guarded(|| ack_rename(info));
}
