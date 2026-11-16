//! 휴지통으로 보내기·복원의 Windows API 호출([`crate::recycle`]). 이 모듈의 함수는 [`crate::recycle`]이 띄운 전용
//! 스레드에서만 불린다.

use std::cell::{Cell, RefCell};
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use windows::Win32::Foundation::{E_ABORT, E_FAIL};
use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumePathNameW};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Shell::{
    COPYENGINE_E_ACCESS_DENIED_DEST, COPYENGINE_E_ACCESS_DENIED_SRC,
    COPYENGINE_E_ACCESSDENIED_READONLY, COPYENGINE_E_DISK_FULL, COPYENGINE_E_PATH_NOT_FOUND_SRC,
    COPYENGINE_E_SHARING_VIOLATION_DEST, COPYENGINE_E_SHARING_VIOLATION_SRC, FOF_ALLOWUNDO,
    FOF_NO_UI, FOF_RENAMEONCOLLISION, FOFX_RECYCLEONDELETE, FileOperation, IFileOperation,
    IFileOperationProgressSink, IFileOperationProgressSink_Impl, IShellItem,
    SHCreateItemFromParsingName, SHCreateItemFromRelativeName, SICHINT_CANONICAL,
    SIGDN_FILESYSPATH, TSF_DELETE_RECYCLE_IF_POSSIBLE,
};
use windows::core::{HRESULT, PCWSTR, Ref, implement};

use crate::file_ops::{ERROR_FILE_EXISTS, FileOpError, FileOpErrorKind, FileOpResult};
use crate::recycle::{RecycleOutcome, RecycleState, RecycledItem, Watch, fault};
use crate::windows_file_ops::{extended, from_io, from_windows};
use crate::windows_impl::ComApartment;

/// `GetDriveTypeW`의 「고정 디스크」.
const DRIVE_FIXED: u32 = 3;

fn com_error(error: std::io::Error) -> FileOpError {
    let _ = error;
    FileOpError::new(FileOpErrorKind::Other, None)
}

/// 셸이 항목마다 알린 결과 코드를 오류로. 셸의 복사 엔진은 Win32 코드 대신 제 코드(`COPYENGINE_E_*`)를 주므로
/// 같은 뜻의 종류로 바꾼다(잠긴 파일은 공유 위반 `COPYENGINE_E_SHARING_VIOLATION_SRC`로 온다).
fn from_hresult(hresult: HRESULT) -> FileOpError {
    let kind = match hresult {
        COPYENGINE_E_SHARING_VIOLATION_SRC | COPYENGINE_E_SHARING_VIOLATION_DEST => {
            Some(FileOpErrorKind::Locked)
        }
        COPYENGINE_E_ACCESS_DENIED_SRC
        | COPYENGINE_E_ACCESS_DENIED_DEST
        | COPYENGINE_E_ACCESSDENIED_READONLY => Some(FileOpErrorKind::AccessDenied),
        COPYENGINE_E_PATH_NOT_FOUND_SRC => Some(FileOpErrorKind::SourceMissing),
        COPYENGINE_E_DISK_FULL => Some(FileOpErrorKind::DiskFull),
        _ => None,
    };
    match kind {
        Some(kind) => FileOpError::new(kind, Some(hresult.0)),
        None => from_windows(&windows::core::Error::from_hresult(hresult)),
    }
}

/// 셸이 경로 하나로 받는 최대 길이(UTF-16 단위, 끝의 NUL 제외). `MAX_PATH`(260)에서 NUL 하나를 뺀 값.
const SHELL_MAX_PATH_LEN: usize = 259;

/// 경로의 셸 항목.
///
/// 셸은 확장 길이 경로(`\\?\`)를 받지 않으므로 절대 경로를 그대로 넘긴다. 셸이 한 번에 받는 길이(260자)를 넘으면
/// 볼륨 루트의 셸 항목에서 시작해 이름 조각마다 하위 항목을 찾아 내려간다(조각 하나는 260자를 넘지 않는다).
fn shell_item(path: &Path) -> FileOpResult<IShellItem> {
    let absolute = std::path::absolute(path).map_err(from_io)?;
    let parse = |text: &std::ffi::OsStr| {
        let wide = crate::windows_impl::to_wide(text);
        // SAFETY: wide는 NUL로 끝나는 UTF-16 경로이고 호출이 끝날 때까지 살아 있다. 바인드 문맥은 주지 않는다.
        unsafe { SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(wide.as_ptr()), None) }
    };
    let result = if absolute.as_os_str().encode_wide().count() <= SHELL_MAX_PATH_LEN {
        parse(absolute.as_os_str())
    } else {
        let mut components = absolute.components();
        let mut root = std::ffi::OsString::new();
        for component in components.by_ref() {
            root.push(component.as_os_str());
            if matches!(component, std::path::Component::RootDir) {
                break;
            }
        }
        parse(&root).and_then(|root| {
            components.try_fold(root, |parent, component| {
                let name = crate::windows_impl::to_wide(component.as_os_str());
                // SAFETY: parent는 살아 있는 셸 항목이고 name은 NUL로 끝나는 UTF-16 이름으로 호출이 끝날 때까지 살아
                // 있다. 바인드 문맥은 주지 않는다.
                unsafe {
                    SHCreateItemFromRelativeName::<_, _, _, IShellItem>(
                        &parent,
                        PCWSTR(name.as_ptr()),
                        None,
                    )
                }
            })
        })
    };
    result.map_err(|error| refine(from_windows(&error), path))
}

/// 「찾을 수 없음」인데 경로가 있으면 셸이 경로를 받지 못한 것이라 「그 밖」이다.
fn refine(error: FileOpError, path: &Path) -> FileOpError {
    if error.kind() == FileOpErrorKind::SourceMissing && exists(path) {
        FileOpError::new(FileOpErrorKind::Other, error.code())
    } else {
        error
    }
}

/// 링크를 따라가지 않고 무언가 있는지 본다(260자를 넘어도).
fn exists(path: &Path) -> bool {
    match extended(path) {
        Ok(wide) => {
            let path = PathBuf::from(String::from_utf16_lossy(&wide[..wide.len() - 1]));
            fs::symlink_metadata(path).is_ok()
        }
        Err(_) => false,
    }
}

/// 확장 길이 경로 그대로의 `PathBuf`(std 파일 함수에 넘길 것).
fn long(path: &Path) -> FileOpResult<PathBuf> {
    let wide = extended(path)?;
    Ok(PathBuf::from(String::from_utf16_lossy(
        &wide[..wide.len() - 1],
    )))
}

/// 셸 항목의 파일 시스템 경로.
fn file_system_path(item: &IShellItem) -> Option<PathBuf> {
    // SAFETY: item은 살아 있는 셸 항목이다. 돌려받은 문자열은 CoTaskMemAlloc으로 할당되어 읽은 뒤 한 번 푼다.
    unsafe {
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(name.0 as *const _));
        path
    }
}

/// 두 셸 항목이 같은 것을 가리키는가.
fn same_item(a: &IShellItem, b: &IShellItem) -> bool {
    // SAFETY: 두 셸 항목 모두 살아 있다. 힌트는 상수다.
    unsafe { a.Compare(b, SICHINT_CANONICAL.0 as u32) }.is_ok_and(|order| order == 0)
}

fn new_operation() -> FileOpResult<IFileOperation> {
    // SAFETY: 이 스레드는 COM을 초기화했다(ComApartment). FileOperation은 IFileOperation을 구현하는 셸 클래스다.
    unsafe { CoCreateInstance(&FileOperation, None, CLSCTX_ALL) }.map_err(|e| from_windows(&e))
}

/// 진행 알림을 받는 동안 operation에 알림 객체를 걸어 둔다. 버리면 뗀다.
struct Advised<'a> {
    operation: &'a IFileOperation,
    cookie: u32,
}

impl<'a> Advised<'a> {
    fn new(operation: &'a IFileOperation, sink: &IFileOperationProgressSink) -> FileOpResult<Self> {
        // SAFETY: operation과 sink는 살아 있는 COM 객체다. 받은 쿠키는 Drop에서 한 번 뗀다.
        let cookie = unsafe { operation.Advise(sink) }.map_err(|e| from_windows(&e))?;
        Ok(Advised { operation, cookie })
    }
}

impl Drop for Advised<'_> {
    fn drop(&mut self) {
        // SAFETY: Advise가 돌려준 쿠키를 같은 operation에서 한 번 뗀다.
        let _ = unsafe { self.operation.Unadvise(self.cookie) };
    }
}

pub(crate) fn recycle(state: &Arc<RecycleState>) -> FileOpResult<()> {
    let _com = ComApartment::enter().map_err(com_error)?;
    // COM 객체는 _com보다 먼저 버려지도록 안쪽 함수에서 만든다.
    recycle_items(state)
}

fn recycle_items(state: &Arc<RecycleState>) -> FileOpResult<()> {
    let mut items = Vec::new();
    for (index, path) in state.paths.iter().enumerate() {
        match shell_item(path) {
            Ok(item) => items.push((index, item)),
            Err(error) => state.settle(index, RecycleOutcome::Failed(error)),
        }
    }
    if items.is_empty() {
        return Ok(());
    }
    let operation = new_operation()?;
    // SAFETY: operation은 살아 있는 COM 객체이고 플래그는 상수다. FOF_NO_UI: 진행 창·확인·오류 대화를 띄우지 않는다.
    // FOF_ALLOWUNDO와 FOFX_RECYCLEONDELETE: 지우기를 휴지통으로 보내기로 한다. 휴지통에 못 넣는 항목을 셸이 영구
    // 삭제로 바꾸는 것은 진행 알림(PreDeleteItem)에서 막는다.
    unsafe { operation.SetOperationFlags(FOF_NO_UI | FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE) }
        .map_err(|e| from_windows(&e))?;
    let sink: IFileOperationProgressSink = RecycleSink {
        state: Arc::clone(state),
        items: items.clone(),
        current: Cell::new(None),
    }
    .into();
    let _advised = Advised::new(&operation, &sink)?;
    for (_, item) in &items {
        // SAFETY: operation과 item은 살아 있는 COM 객체다. 항목별 알림 객체는 주지 않는다(작업 전체에 걸었다).
        unsafe { operation.DeleteItem(item, None::<&IFileOperationProgressSink>) }
            .map_err(|e| from_windows(&e))?;
    }
    // SAFETY: operation은 살아 있는 COM 객체이고, 넣은 작업을 이 스레드에서 끝까지 실행한다. 알림 객체가 작업을
    // 멈추게 하면 실패로 돌아오지만 항목마다 결과는 알림 객체가 이미 적었다.
    let _ = unsafe { operation.PerformOperations() };
    Ok(())
}

/// 휴지통으로 보내기의 진행 알림. 항목마다 결과를 적고, 휴지통 대신 영구 삭제하려는 항목에서 작업을 멈춘다.
#[implement(IFileOperationProgressSink)]
struct RecycleSink {
    state: Arc<RecycleState>,
    items: Vec<(usize, IShellItem)>,
    /// 지금 셸이 다루는 요청 항목. 셸이 폴더 안 항목을 따로 알릴 때 그 결과를 이 항목에 적는다.
    current: Cell<Option<usize>>,
}

impl RecycleSink {
    /// 셸이 알린 항목이 요청 항목 가운데 무엇인가. 요청 항목이 아니면(폴더 안 항목) 지금 다루는 요청 항목이다.
    fn index_of(&self, item: Option<&IShellItem>) -> Option<usize> {
        let found = item.and_then(|item| {
            self.items
                .iter()
                .find(|(_, candidate)| same_item(candidate, item))
                .map(|(index, _)| *index)
        });
        match found {
            Some(index) => {
                self.current.set(Some(index));
                Some(index)
            }
            None => self.current.get(),
        }
    }

    /// 진행을 알리고, 부른 쪽이 기다림을 그만두었으면 작업을 멈춘다.
    fn progress(&self) -> windows::core::Result<()> {
        self.state.watch.touch();
        if self.state.watch.is_cancelled() {
            return Err(E_ABORT.into());
        }
        Ok(())
    }
}

impl IFileOperationProgressSink_Impl for RecycleSink_Impl {
    fn StartOperations(&self) -> windows::core::Result<()> {
        self.progress()
    }

    fn FinishOperations(&self, _hrresult: HRESULT) -> windows::core::Result<()> {
        self.state.watch.touch();
        Ok(())
    }

    fn PreRenameItem(
        &self,
        _dwflags: u32,
        _psiitem: Ref<IShellItem>,
        _psznewname: &PCWSTR,
    ) -> windows::core::Result<()> {
        self.progress()
    }

    fn PostRenameItem(
        &self,
        _dwflags: u32,
        _psiitem: Ref<IShellItem>,
        _psznewname: &PCWSTR,
        _hrrename: HRESULT,
        _psinewlycreated: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        self.progress()
    }

    fn PreMoveItem(
        &self,
        _dwflags: u32,
        _psiitem: Ref<IShellItem>,
        _psidestinationfolder: Ref<IShellItem>,
        _psznewname: &PCWSTR,
    ) -> windows::core::Result<()> {
        self.progress()
    }

    fn PostMoveItem(
        &self,
        _dwflags: u32,
        _psiitem: Ref<IShellItem>,
        _psidestinationfolder: Ref<IShellItem>,
        _psznewname: &PCWSTR,
        _hrmove: HRESULT,
        _psinewlycreated: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        self.progress()
    }

    fn PreCopyItem(
        &self,
        _dwflags: u32,
        _psiitem: Ref<IShellItem>,
        _psidestinationfolder: Ref<IShellItem>,
        _psznewname: &PCWSTR,
    ) -> windows::core::Result<()> {
        self.progress()
    }

    fn PostCopyItem(
        &self,
        _dwflags: u32,
        _psiitem: Ref<IShellItem>,
        _psidestinationfolder: Ref<IShellItem>,
        _psznewname: &PCWSTR,
        _hrcopy: HRESULT,
        _psinewlycreated: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        self.progress()
    }

    fn PreDeleteItem(&self, dwflags: u32, psiitem: Ref<IShellItem>) -> windows::core::Result<()> {
        self.progress()?;
        let Some(index) = self.index_of(psiitem.as_ref()) else {
            return Ok(());
        };
        let path = &self.state.paths[index];
        if let Some(stall) = fault::stall(path) {
            std::thread::sleep(stall);
            if self.state.watch.is_cancelled() {
                return Err(E_ABORT.into());
            }
        }
        // 셸은 휴지통에 넣을 수 있는 항목에만 TSF_DELETE_RECYCLE_IF_POSSIBLE을 준다. 없으면 이 항목을 영구 삭제하려는
        // 것이므로 지우지 않고 작업을 멈춘다(실패를 돌려주면 이 항목과 뒤 항목을 하지 않는다).
        let recyclable = dwflags & TSF_DELETE_RECYCLE_IF_POSSIBLE.0 as u32 != 0;
        if !recyclable || fault::refuses(path) {
            self.state.settle(index, RecycleOutcome::NotRecyclable);
            return Err(E_ABORT.into());
        }
        Ok(())
    }

    fn PostDeleteItem(
        &self,
        _dwflags: u32,
        psiitem: Ref<IShellItem>,
        hrdelete: HRESULT,
        psinewlycreated: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        self.state.watch.touch();
        let Some(index) = self.index_of(psiitem.as_ref()) else {
            return Ok(());
        };
        let outcome = if hrdelete.is_err() {
            RecycleOutcome::Failed(refine(from_hresult(hrdelete), &self.state.paths[index]))
        } else {
            match psinewlycreated.as_ref().and_then(file_system_path) {
                Some(recycled) => RecycleOutcome::Recycled(RecycledItem::new(
                    self.state.paths[index].clone(),
                    recycled,
                )),
                // 지웠는데 휴지통에 생긴 항목이 없다. PreDeleteItem이 영구 삭제를 막으므로 생기지 않아야 한다.
                None => {
                    RecycleOutcome::Failed(FileOpError::new(FileOpErrorKind::Other, Some(E_FAIL.0)))
                }
            }
        };
        self.state.settle(index, outcome);
        Ok(())
    }

    fn PreNewItem(
        &self,
        _dwflags: u32,
        _psidestinationfolder: Ref<IShellItem>,
        _psznewname: &PCWSTR,
    ) -> windows::core::Result<()> {
        self.progress()
    }

    fn PostNewItem(
        &self,
        _dwflags: u32,
        _psidestinationfolder: Ref<IShellItem>,
        _psznewname: &PCWSTR,
        _psztemplatename: &PCWSTR,
        _dwfileattributes: u32,
        _hrnew: HRESULT,
        _psinewitem: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        self.progress()
    }

    fn UpdateProgress(&self, _iworktotal: u32, _iworksofar: u32) -> windows::core::Result<()> {
        self.progress()
    }

    fn ResetTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn PauseTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn ResumeTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
}

pub(crate) fn restore(item: &RecycledItem, watch: &Arc<Watch>) -> FileOpResult<PathBuf> {
    // 휴지통을 비웠으면 휴지통 항목이 없다.
    if !exists(item.recycled()) {
        return Err(FileOpError::new(FileOpErrorKind::SourceMissing, None));
    }
    // 원래 자리에 같은 이름이 있으면 덮어쓰지 않는다.
    if exists(item.original()) {
        return Err(FileOpError::from_win32(ERROR_FILE_EXISTS));
    }
    let original = item.original();
    let (Some(folder), Some(name)) = (original.parent(), original.file_name()) else {
        return Err(FileOpError::new(FileOpErrorKind::Other, None));
    };
    // 원래 폴더가 없어졌으면 탐색기의 복원처럼 다시 만든다.
    fs::create_dir_all(long(folder)?).map_err(from_io)?;
    let _com = ComApartment::enter().map_err(com_error)?;
    restore_item(item, folder, name, watch)
}

fn restore_item(
    item: &RecycledItem,
    folder: &Path,
    name: &std::ffi::OsStr,
    watch: &Arc<Watch>,
) -> FileOpResult<PathBuf> {
    let source = shell_item(item.recycled())?;
    let destination = shell_item(folder)?;
    let operation = new_operation()?;
    // SAFETY: operation은 살아 있는 COM 객체이고 플래그는 상수다. FOF_NO_UI는 묻지 않고 「모두 예」로 답해 같은
    // 이름을 덮어쓰므로, 미리 본 뒤에 같은 이름이 생기는 경우에도 덮어쓰지 않게 FOF_RENAMEONCOLLISION을 함께 준다
    // (진행 알림 PreMoveItem도 같은 이름이 있으면 멈춘다).
    unsafe { operation.SetOperationFlags(FOF_NO_UI | FOF_RENAMEONCOLLISION) }
        .map_err(|e| from_windows(&e))?;
    let target = folder.join(name);
    let result = Rc::new(RefCell::new(None));
    let sink: IFileOperationProgressSink = RestoreSink {
        watch: Arc::clone(watch),
        target: target.clone(),
        result: Rc::clone(&result),
    }
    .into();
    let _advised = Advised::new(&operation, &sink)?;
    let name = crate::windows_impl::to_wide(name);
    // SAFETY: 두 셸 항목과 operation은 살아 있는 COM 객체이고, name은 NUL로 끝나는 UTF-16으로 호출이 끝날 때까지
    // 살아 있다.
    unsafe {
        operation.MoveItem(
            &source,
            &destination,
            PCWSTR(name.as_ptr()),
            None::<&IFileOperationProgressSink>,
        )
    }
    .map_err(|e| from_windows(&e))?;
    // SAFETY: operation은 살아 있는 COM 객체이고, 넣은 작업을 이 스레드에서 끝까지 실행한다.
    let performed = unsafe { operation.PerformOperations() };
    let result = result.borrow_mut().take();
    let restored = match result {
        Some(result) => result?,
        None => {
            return Err(match performed {
                Err(error) => from_windows(&error),
                Ok(()) => FileOpError::new(FileOpErrorKind::Other, None),
            });
        }
    };
    remove_recycle_info(item.recycled());
    Ok(restored)
}

/// 복원한 휴지통 항목의 정보 파일(`$I...`)을 지운다.
///
/// 휴지통은 항목마다 내용(`$R<이름>`)과 원래 경로·지운 시각을 담은 정보 파일(`$I<이름>`)을 나란히 둔다. 휴지통 안의
/// 파일 시스템 경로에서 내용을 옮기면 셸은 정보 파일을 지우지 않아, 휴지통에 「원래 자리」만 남은 빈 항목이 보인다.
/// 그래서 내용을 되돌린 뒤 같은 이름의 정보 파일을 지운다. 지우지 못해도 복원은 끝났으므로 실패로 돌리지 않는다.
fn remove_recycle_info(recycled: &Path) {
    let (Some(folder), Some(name)) = (recycled.parent(), recycled.file_name()) else {
        return;
    };
    let name = name.to_string_lossy();
    let Some(rest) = name.strip_prefix("$R") else {
        return;
    };
    if let Ok(info) = long(&folder.join(format!("$I{rest}"))) {
        let _ = fs::remove_file(info);
    }
}

/// 복원의 진행 알림. 같은 이름이 있으면 멈추고, 결과(되돌린 경로)를 적는다.
#[implement(IFileOperationProgressSink)]
struct RestoreSink {
    watch: Arc<Watch>,
    target: PathBuf,
    result: Rc<RefCell<Option<FileOpResult<PathBuf>>>>,
}

impl RestoreSink {
    fn progress(&self) -> windows::core::Result<()> {
        self.watch.touch();
        if self.watch.is_cancelled() {
            return Err(E_ABORT.into());
        }
        Ok(())
    }

    fn settle(&self, result: FileOpResult<PathBuf>) {
        let mut slot = self.result.borrow_mut();
        if slot.is_none() {
            *slot = Some(result);
        }
    }
}

impl IFileOperationProgressSink_Impl for RestoreSink_Impl {
    fn StartOperations(&self) -> windows::core::Result<()> {
        self.progress()
    }

    fn FinishOperations(&self, _hrresult: HRESULT) -> windows::core::Result<()> {
        self.watch.touch();
        Ok(())
    }

    fn PreRenameItem(
        &self,
        _dwflags: u32,
        _psiitem: Ref<IShellItem>,
        _psznewname: &PCWSTR,
    ) -> windows::core::Result<()> {
        self.progress()
    }

    fn PostRenameItem(
        &self,
        _dwflags: u32,
        _psiitem: Ref<IShellItem>,
        _psznewname: &PCWSTR,
        _hrrename: HRESULT,
        _psinewlycreated: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        self.progress()
    }

    fn PreMoveItem(
        &self,
        _dwflags: u32,
        _psiitem: Ref<IShellItem>,
        _psidestinationfolder: Ref<IShellItem>,
        _psznewname: &PCWSTR,
    ) -> windows::core::Result<()> {
        self.progress()?;
        if exists(&self.target) {
            let error = FileOpError::from_win32(ERROR_FILE_EXISTS);
            self.settle(Err(error));
            return Err(HRESULT::from_win32(ERROR_FILE_EXISTS).into());
        }
        Ok(())
    }

    fn PostMoveItem(
        &self,
        _dwflags: u32,
        _psiitem: Ref<IShellItem>,
        _psidestinationfolder: Ref<IShellItem>,
        _psznewname: &PCWSTR,
        hrmove: HRESULT,
        psinewlycreated: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        self.watch.touch();
        if hrmove.is_err() {
            self.settle(Err(from_hresult(hrmove)));
        } else {
            // 셸이 알린 경로는 짧은 이름(8.3)일 수 있어 원래 경로 모양으로 돌려준다. 미리 본 뒤 같은 이름이 생겨
            // 셸이 이름을 바꿔 되돌렸으면(FOF_RENAMEONCOLLISION) 그 이름을 원래 폴더에 붙여 돌려준다.
            let created = psinewlycreated
                .as_ref()
                .and_then(file_system_path)
                .and_then(|path| path.file_name().map(|name| name.to_os_string()));
            let restored = match (created, self.target.parent()) {
                (Some(name), Some(folder)) if Some(name.as_os_str()) != self.target.file_name() => {
                    folder.join(name)
                }
                _ => self.target.clone(),
            };
            self.settle(Ok(restored));
        }
        Ok(())
    }

    fn PreCopyItem(
        &self,
        _dwflags: u32,
        _psiitem: Ref<IShellItem>,
        _psidestinationfolder: Ref<IShellItem>,
        _psznewname: &PCWSTR,
    ) -> windows::core::Result<()> {
        self.progress()
    }

    fn PostCopyItem(
        &self,
        _dwflags: u32,
        _psiitem: Ref<IShellItem>,
        _psidestinationfolder: Ref<IShellItem>,
        _psznewname: &PCWSTR,
        _hrcopy: HRESULT,
        _psinewlycreated: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        self.progress()
    }

    fn PreDeleteItem(&self, _dwflags: u32, _psiitem: Ref<IShellItem>) -> windows::core::Result<()> {
        self.progress()
    }

    fn PostDeleteItem(
        &self,
        _dwflags: u32,
        _psiitem: Ref<IShellItem>,
        _hrdelete: HRESULT,
        _psinewlycreated: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        self.progress()
    }

    fn PreNewItem(
        &self,
        _dwflags: u32,
        _psidestinationfolder: Ref<IShellItem>,
        _psznewname: &PCWSTR,
    ) -> windows::core::Result<()> {
        self.progress()
    }

    fn PostNewItem(
        &self,
        _dwflags: u32,
        _psidestinationfolder: Ref<IShellItem>,
        _psznewname: &PCWSTR,
        _psztemplatename: &PCWSTR,
        _dwfileattributes: u32,
        _hrnew: HRESULT,
        _psinewitem: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        self.progress()
    }

    fn UpdateProgress(&self, _iworktotal: u32, _iworksofar: u32) -> windows::core::Result<()> {
        self.progress()
    }

    fn ResetTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn PauseTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn ResumeTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
}

pub(crate) fn has_recycle_bin(path: &Path) -> bool {
    let Ok(wide) = extended(path) else {
        return false;
    };
    let mut root = vec![0u16; wide.len() + 1];
    // SAFETY: wide는 NUL로 끝나는 UTF-16 경로이고, root는 경로보다 긴 버퍼다(볼륨 경로는 경로보다 길지 않다).
    if unsafe { GetVolumePathNameW(PCWSTR(wide.as_ptr()), &mut root) }.is_err() {
        return false;
    }
    // SAFETY: root는 GetVolumePathNameW가 채운 NUL로 끝나는 볼륨 경로다.
    let drive_type = unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) };
    drive_type == DRIVE_FIXED
}
