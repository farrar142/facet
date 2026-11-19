//! 세션: 끝낼 때 저장해 다음 실행에 되살리는 창·패널·탭 상태.
//!
//! 담는 것: 창 목록과 마지막 활성 창, 창마다 패널(하나나 둘)·활성 패널·분할 비율, 패널마다 탭·활성 탭, 탭마다
//! 위치·정렬, 창마다 화면 자리·크기·최대화([`crate::placement::Placement`], 파일의 `placement`). 뒤로·앞으로 기록,
//! 선택·스크롤, 닫은 탭 목록, 숨김 항목 보이기는 담지 않는다.
//!
//! 창 자리는 가상 화면 기준 물리 픽셀이다(좌표 단위는 [`crate::placement`]). 자리를 모르는 창(아직 화면에 띄우지
//! 않았다)은 `placement`가 `null`이고, 되살리면 Windows가 정한 자리에 뜬다. 형식 버전 1의 앞선 파일은 늘 `null`을
//! 썼으므로 버전을 올리지 않았다: 앞선 파일은 그대로 읽히고(창 자리만 없다), 앞선 facet은 새 파일의 `placement`를
//! 무시한다. `placement`가 형식에 맞지 않으면(값이 빠졌거나 크기가 0) 그 창의 자리만 버리고 나머지는 읽는다.
//!
//! 파일은 사람이 읽을 수 있는 JSON이고 형식 버전([`FORMAT_VERSION`])을 갖는다. 자리는 `%APPDATA%\facet\session.json`
//! ([`session_path`])이고, 환경 변수 [`ENV_PATH`]로 바꿀 수 있다. 측정 모드는 [`ENV_PATH`]가 없으면 세션 파일을
//! 읽지도 쓰지도 않는다.
//!
//! - 저장([`save`])은 옆의 임시 파일([`temp_path`])에 다 쓴 뒤 바꿔치기해, 도중에 끊겨도 반쯤 쓴 파일이 남지 않는다.
//! - 읽기([`load`]): 파일이 없으면 [`Loaded::Missing`](첫 실행). JSON이 깨졌거나 버전이 없거나 모르는 버전이면
//!   그 파일을 옆에 다른 이름(`session.broken-<시각>.json`)으로 남기고 [`Loaded::Broken`]이다. 둘 다 앱은 기본
//!   창으로 뜬다.
//! - 바뀔 때마다 저장하되 UI 스레드는 디스크를 기다리지 않는다([`Saver`]: 바뀐 뒤 [`SAVE_DELAY`]를 묶어 일꾼
//!   스레드에서 쓰고, 끝낼 때 한 번 더 쓴다).
//!
//! 상태 모델과의 변환은 [`crate::window::App::session`](저장할 것)과 [`crate::window::App::restore`](되살리기)가 한다.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::details::{Column, Direction, Sort};
use crate::placement::{Placement, Rect};
use crate::view::{GridSize, View, ViewMode};
use crate::{Location, VirtualFolder};

/// 세션 파일의 형식 버전. 형식을 바꾸면 올린다. 이 버전이 아닌 파일은 깨진 파일처럼 옆에 남긴다.
pub const FORMAT_VERSION: u64 = 1;

/// 세션 파일 자리를 바꾸는 환경 변수. 측정 모드에서도 이것이 있으면 이 자리를 쓴다.
pub const ENV_PATH: &str = "FACET_SESSION_PATH";

/// 바뀐 뒤 이만큼을 묶어 한 번에 저장한다([`Saver`]).
pub const SAVE_DELAY: Duration = Duration::from_secs(2);

/// 저장할 화면 상태: 창 목록과 마지막 활성 창.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub windows: Vec<WindowSession>,
    /// 마지막 활성 창이 `windows`의 몇 번째인가.
    pub active_window: usize,
    /// 최근 대상 폴더(가장 최근 것부터, 앱 전체에 하나). 앞선 파일에는 없고 그러면 비어 있다.
    pub recent_targets: Vec<PathBuf>,
}

/// 창 하나: 패널(왼쪽부터), 활성 패널, 분할 비율.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowSession {
    pub panels: Vec<PanelSession>,
    /// 활성 패널이 몇 번째인가(0이 왼쪽).
    pub active_panel: usize,
    /// 분할 비율(왼쪽 패널의 몫). 합친 창에도 남는다.
    pub split_ratio: f32,
    /// 창의 화면 자리·크기·최대화. 모르면(아직 띄우지 않은 창) `None`이다.
    pub placement: Option<Placement>,
    /// #339: 미리보기 영역(켜짐·폭).
    pub preview: crate::preview::PreviewPane,
}

/// 패널 하나: 탭들과 활성 탭.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelSession {
    pub tabs: Vec<TabSession>,
    /// 활성 탭이 몇 번째인가.
    pub active_tab: usize,
}

/// 탭 하나: 위치와 정렬.
#[derive(Debug, Clone, PartialEq)]
pub struct TabSession {
    pub location: Location,
    pub sort: Sort,
    /// #327: 보기 방식과 칸 크기.
    pub view: View,
}

/// 세션 파일을 해석하지 못한 까닭.
#[derive(Debug)]
pub enum SessionError {
    /// 파일을 읽지 못했다(없는 경우는 [`Loaded::Missing`]이다).
    Io(io::Error),
    /// JSON이 깨졌거나 형식에 맞지 않는다. 값은 해석기의 설명(로그용, 화면 문장이 아니다).
    Invalid(String),
    /// 형식 버전이 없다.
    MissingVersion,
    /// 모르는 형식 버전이다. 값은 파일에 적힌 그대로다.
    UnknownVersion(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "세션 파일을 읽지 못함: {error}"),
            Self::Invalid(detail) => write!(f, "세션 파일이 깨짐: {detail}"),
            Self::MissingVersion => write!(f, "세션 파일에 형식 버전이 없음"),
            Self::UnknownVersion(version) => write!(f, "모르는 세션 형식 버전: {version}"),
        }
    }
}

impl std::error::Error for SessionError {}

/// 세션 파일을 읽은 결과([`load`]).
#[derive(Debug)]
pub enum Loaded {
    /// 파일이 없다(첫 실행). 조용히 기본 창으로 뜬다.
    Missing,
    /// 읽었다.
    Restored(Session),
    /// 파일을 해석하지 못해 옆에 다른 이름으로 남겼다. 앱은 기본 창으로 뜨고 이 사실과 남긴 파일 이름을 보인다.
    /// 남기지 못했으면(이름 바꾸기 실패) `kept`가 `None`이다.
    Broken {
        error: SessionError,
        kept: Option<PathBuf>,
    },
}

impl Loaded {
    /// 되살릴 세션. 없거나 깨졌으면 `None`이다(기본 창으로 뜬다).
    pub fn session(&self) -> Option<&Session> {
        match self {
            Self::Restored(session) => Some(session),
            Self::Missing | Self::Broken { .. } => None,
        }
    }

    /// 되살릴 세션을 꺼낸다.
    pub fn into_session(self) -> Option<Session> {
        match self {
            Self::Restored(session) => Some(session),
            Self::Missing | Self::Broken { .. } => None,
        }
    }
}

impl Session {
    /// 세션 파일에 쓸 JSON(사람이 읽기 좋게 들여 쓴다).
    pub fn to_json(&self) -> String {
        let file = FileV1::from_session(self);
        serde_json::to_string_pretty(&file).expect("세션은 늘 JSON으로 바꿀 수 있다")
    }

    /// 세션 파일의 JSON을 해석한다. 버전을 먼저 보고(없으면 [`SessionError::MissingVersion`], 모르면
    /// [`SessionError::UnknownVersion`]) 그 버전의 형식으로 읽는다.
    pub fn from_json(text: &str) -> Result<Session, SessionError> {
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| SessionError::Invalid(e.to_string()))?;
        let version = match value.get("version") {
            None | Some(serde_json::Value::Null) => return Err(SessionError::MissingVersion),
            Some(version) => version,
        };
        if version.as_u64() != Some(FORMAT_VERSION) {
            return Err(SessionError::UnknownVersion(version.to_string()));
        }
        let file: FileV1 =
            serde_json::from_value(value).map_err(|e| SessionError::Invalid(e.to_string()))?;
        Ok(file.into_session())
    }
}

/// 세션 파일 자리: [`ENV_PATH`]가 있으면 그것, 없으면 `%APPDATA%\facet\session.json`. 측정 모드(`measuring`)는
/// [`ENV_PATH`]가 없으면 `None`이다(사용자의 세션 파일을 읽지도 쓰지도 않는다). `%APPDATA%`를 모르면 `None`이다.
pub fn session_path(measuring: bool) -> Option<PathBuf> {
    session_path_with(measuring, |name| std::env::var_os(name))
}

/// [`session_path`]를 환경 변수 읽기 `var`로 정한다(테스트가 쓴다).
pub fn session_path_with(
    measuring: bool,
    var: impl Fn(&str) -> Option<OsString>,
) -> Option<PathBuf> {
    let present = |value: Option<OsString>| value.filter(|v| !v.is_empty());
    if let Some(path) = present(var(ENV_PATH)) {
        return Some(PathBuf::from(path));
    }
    if measuring {
        return None;
    }
    present(var("APPDATA")).map(|dir| PathBuf::from(dir).join("facet").join("session.json"))
}

/// 저장할 때 먼저 쓰는 임시 파일: 세션 파일 이름 뒤에 `.tmp`를 붙인 옆 파일.
pub fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(OsString::from).unwrap_or_default();
    name.push(".tmp");
    path.with_file_name(name)
}

/// 세션을 `path`에 저장한다. 폴더가 없으면 만들고, 임시 파일([`temp_path`])에 다 쓰고 디스크에 내린 뒤 바꿔치기한다.
/// 도중에 끊기면 원래 파일이 그대로 남는다.
pub fn save(path: &Path, session: &Session) -> io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let temp = temp_path(path);
    let written = (|| {
        let mut file = fs::File::create(&temp)?;
        file.write_all(session.to_json().as_bytes())?;
        file.sync_all()
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    fs::rename(&temp, path)
}

/// `path`의 세션 파일을 읽는다. 없으면 [`Loaded::Missing`]. 해석하지 못하면 그 파일을 옆에
/// `<이름>.broken-<시각>.json`으로 옮겨 남기고 [`Loaded::Broken`]이다(다음 저장이 덮어쓰지 않게).
pub fn load(path: &Path) -> Loaded {
    let parsed = match fs::read(path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Session::from_json(&text),
            Err(error) => Err(SessionError::Invalid(error.to_string())),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Loaded::Missing,
        Err(error) => Err(SessionError::Io(error)),
    };
    match parsed {
        Ok(session) => Loaded::Restored(session),
        Err(error) => Loaded::Broken {
            error,
            kept: keep_broken(path),
        },
    }
}

/// 깨진 세션 파일을 옆에 겹치지 않는 이름으로 옮긴다. 옮긴 자리를 돌려준다.
fn keep_broken(path: &Path) -> Option<PathBuf> {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "session".to_owned());
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    for n in 1..100 {
        let name = if n == 1 {
            format!("{stem}.broken-{stamp}.json")
        } else {
            format!("{stem}.broken-{stamp}-{n}.json")
        };
        let target = path.with_file_name(name);
        if target.exists() {
            continue;
        }
        return fs::rename(path, &target).ok().map(|()| target);
    }
    None
}

enum SaverMessage {
    Changed(Session),
    Finish(Option<Session>),
}

/// 세션을 일꾼 스레드에서 저장하는 것. UI 스레드는 바뀐 상태를 넘기기만 하고([`Saver::update`]) 디스크를 기다리지
/// 않는다. 일꾼 스레드는 처음 바뀐 뒤 `delay`(앱은 [`SAVE_DELAY`])를 기다려 그 사이 마지막 상태를 한 번 쓴다.
/// 끝낼 때([`Saver::finish`]) 마지막 상태를 한 번 더 쓴다. 저장이 실패해도 앱은 그대로 돈다(다음 변경에 다시 쓴다).
pub struct Saver {
    sender: Option<Sender<SaverMessage>>,
    thread: Option<JoinHandle<io::Result<()>>>,
    /// 마지막으로 넘긴 상태. 같은 상태는 다시 넘기지 않는다.
    last: Option<Session>,
}

impl Saver {
    /// `path`에 저장하는 일꾼 스레드를 띄운다. `initial`은 지금 파일에 든(또는 막 되살린) 상태로, 같은 상태는 다시
    /// 쓰지 않는다. 스레드를 만들지 못하면 저장하지 않는 `Saver`다.
    pub fn spawn(path: PathBuf, delay: Duration, initial: Option<Session>) -> Saver {
        let (sender, receiver) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("facet-session".into())
            .spawn(move || run_saver(&path, delay, &receiver))
            .ok();
        Saver {
            sender: thread.is_some().then_some(sender),
            thread,
            last: initial,
        }
    }

    /// 지금 상태를 알린다. 마지막으로 알린 것과 다르면 일꾼 스레드에 넘긴다. `None`(창이 하나도 없음)은 넘기지
    /// 않는다: 마지막 창을 닫고 끝내면 그 창이 있던 세션이 남는다. 막지 않는다.
    pub fn update(&mut self, session: Option<Session>) {
        let Some(session) = session else { return };
        if self.last.as_ref() == Some(&session) {
            return;
        }
        if let Some(sender) = &self.sender {
            let _ = sender.send(SaverMessage::Changed(session.clone()));
        }
        self.last = Some(session);
    }

    /// 끝낸다: 마지막 상태를 한 번 더 쓰고 일꾼 스레드를 기다린다. 마지막 쓰기의 결과를 돌려준다.
    pub fn finish(mut self) -> io::Result<()> {
        let last = self.last.take();
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(SaverMessage::Finish(last));
        }
        match self.thread.take() {
            Some(thread) => thread
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("세션 저장 스레드가 멈춤"))),
            None => Err(io::Error::other("세션 저장 스레드가 없음")),
        }
    }
}

impl Drop for Saver {
    fn drop(&mut self) {
        // finish 없이 버리면 일꾼 스레드는 남은 상태를 쓰고 스스로 끝난다(기다리지 않는다).
        self.sender.take();
    }
}

fn run_saver(path: &Path, delay: Duration, receiver: &Receiver<SaverMessage>) -> io::Result<()> {
    let mut result = Ok(());
    let mut pending: Option<(Session, Instant)> = None;
    loop {
        let message = match &pending {
            None => receiver.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some((_, deadline)) => {
                receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            }
        };
        match message {
            Ok(SaverMessage::Changed(session)) => {
                let deadline = pending
                    .take()
                    .map_or_else(|| Instant::now() + delay, |(_, deadline)| deadline);
                pending = Some((session, deadline));
            }
            Ok(SaverMessage::Finish(last)) => {
                let last = last.or(pending.map(|(session, _)| session));
                return match last {
                    Some(session) => save(path, &session),
                    None => result,
                };
            }
            Err(RecvTimeoutError::Timeout) => {
                if let Some((session, _)) = pending.take() {
                    result = save(path, &session);
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                return match pending {
                    Some((session, _)) => save(path, &session),
                    None => result,
                };
            }
        }
    }
}

// 파일 형식 1. 바꾸면 [`FORMAT_VERSION`]을 올리고 앞 버전을 읽는 길을 남길지 정한다.

#[derive(Serialize, Deserialize)]
struct FileV1 {
    version: u64,
    active_window: usize,
    /// 최근 대상 폴더. 이 필드를 더하기 전의 버전 1 파일에는 없으므로 비어 있는 것으로 읽는다(그래서 버전을
    /// 올리지 않았다). UTF-16 짝이 맞지 않는 경로는 대체 문자로 바꿔 쓴다.
    #[serde(default)]
    recent_targets: Vec<String>,
    windows: Vec<WindowV1>,
}

#[derive(Serialize, Deserialize)]
struct WindowV1 {
    /// 창의 화면 자리·크기·최대화([`PlacementV1`]). 모르면 `null`이다. 형식에 맞지 않으면 버린다(읽을 때 값으로
    /// 받아 따로 해석한다).
    #[serde(default)]
    placement: Option<serde_json::Value>,
    split_ratio: f32,
    active_panel: usize,
    /// #339: 미리보기 영역. 이 필드를 더하기 전의 버전 1 파일에는 없으므로 꺼짐·기본 폭으로 읽는다.
    #[serde(default)]
    preview: PreviewV1,
    panels: Vec<PanelV1>,
}

#[derive(Serialize, Deserialize)]
struct PreviewV1 {
    visible: bool,
    width: f32,
}

impl Default for PreviewV1 {
    fn default() -> Self {
        let pane = crate::preview::PreviewPane::default();
        PreviewV1 {
            visible: pane.visible,
            width: pane.width,
        }
    }
}

/// 창 자리: 바깥 테두리 왼쪽 위(`x`, `y`)와 안쪽 크기(`width`, `height`), 가상 화면 기준 물리 픽셀. 최대화한 창은
/// 최대화를 풀었을 때의 자리다.
#[derive(Serialize, Deserialize)]
struct PlacementV1 {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    maximized: bool,
}

impl PlacementV1 {
    fn from_placement(placement: &Placement) -> Self {
        PlacementV1 {
            x: placement.normal.x,
            y: placement.normal.y,
            width: placement.normal.width,
            height: placement.normal.height,
            maximized: placement.maximized,
        }
    }

    /// 파일의 `placement` 값. `null`이거나 형식에 맞지 않거나 크기가 0이면 `None`이다.
    fn read(value: Option<serde_json::Value>) -> Option<Placement> {
        let placement: PlacementV1 = serde_json::from_value(value?).ok()?;
        (placement.width > 0 && placement.height > 0).then_some(Placement {
            normal: Rect::new(placement.x, placement.y, placement.width, placement.height),
            maximized: placement.maximized,
        })
    }
}

#[derive(Serialize, Deserialize)]
struct PanelV1 {
    active_tab: usize,
    tabs: Vec<TabV1>,
}

#[derive(Serialize, Deserialize)]
struct TabV1 {
    location: LocationV1,
    sort: SortV1,
    /// #327: 앞선 버전 1 파일에는 없다(자세히 보기·보통).
    #[serde(default)]
    view: ViewV1,
}

#[derive(Serialize, Deserialize, Default)]
struct ViewV1 {
    #[serde(default)]
    mode: ViewModeV1,
    #[serde(default)]
    grid_size: GridSizeV1,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum ViewModeV1 {
    #[default]
    Details,
    Grid,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum GridSizeV1 {
    Small,
    #[default]
    Medium,
    Large,
    ExtraLarge,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LocationV1 {
    /// 실제 폴더의 경로. UTF-16 짝이 맞지 않는 경로는 대체 문자로 바꿔 쓴다(되살리면 사라진 위치처럼 상위로 열린다).
    FileSystem(String),
    /// 가상 폴더(내 PC 등): 종류와 셸 항목 식별자(PIDL) 바이트의 16진 글자. 형식 버전을 올리지 않고 더한 변형이라 앞선
    /// 파일도 그대로 읽힌다.
    Shell { folder: FolderV1, id: String },
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum FolderV1 {
    ThisPc,
    RecycleBin,
    Device,
}

#[derive(Serialize, Deserialize)]
struct SortV1 {
    column: ColumnV1,
    direction: DirectionV1,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ColumnV1 {
    Name,
    Modified,
    Type,
    Size,
    /// 휴지통의 원래 위치. 휴지통 탭을 되살리려고 더한 변형이라 형식 버전을 올리지 않았다(앞선 파일도 그대로 읽힌다).
    OriginalLocation,
    /// 휴지통의 지운 날짜(위와 같다).
    DateDeleted,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DirectionV1 {
    Ascending,
    Descending,
}

impl FileV1 {
    fn from_session(session: &Session) -> Self {
        FileV1 {
            version: FORMAT_VERSION,
            active_window: session.active_window,
            recent_targets: session
                .recent_targets
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            windows: session
                .windows
                .iter()
                .map(|window| WindowV1 {
                    placement: window.placement.as_ref().map(|placement| {
                        serde_json::to_value(PlacementV1::from_placement(placement))
                            .expect("창 자리는 늘 JSON으로 바꿀 수 있다")
                    }),
                    split_ratio: window.split_ratio,
                    active_panel: window.active_panel,
                    preview: PreviewV1 {
                        visible: window.preview.visible,
                        width: window.preview.width,
                    },
                    panels: window
                        .panels
                        .iter()
                        .map(|panel| PanelV1 {
                            active_tab: panel.active_tab,
                            tabs: panel.tabs.iter().map(TabV1::from_tab).collect(),
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    fn into_session(self) -> Session {
        Session {
            active_window: self.active_window,
            recent_targets: self.recent_targets.into_iter().map(PathBuf::from).collect(),
            windows: self
                .windows
                .into_iter()
                .map(|window| WindowSession {
                    preview: {
                        use crate::preview::{PREVIEW_WIDTH_MAX, PREVIEW_WIDTH_MIN, PreviewPane};
                        let width = window.preview.width;
                        PreviewPane {
                            visible: window.preview.visible,
                            width: if width.is_finite() {
                                width.clamp(PREVIEW_WIDTH_MIN, PREVIEW_WIDTH_MAX)
                            } else {
                                PreviewPane::default().width
                            },
                        }
                    },
                    placement: PlacementV1::read(window.placement),
                    split_ratio: window.split_ratio,
                    active_panel: window.active_panel,
                    panels: window
                        .panels
                        .into_iter()
                        .map(|panel| PanelSession {
                            active_tab: panel.active_tab,
                            tabs: panel.tabs.into_iter().map(TabV1::into_tab).collect(),
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

/// 16진 글자(두 글자가 한 바이트)를 바이트로. 형식이 틀리면 `None`이다.
fn parse_hex(text: &str) -> Option<Vec<u8>> {
    if !text.is_ascii() || !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).ok())
        .collect()
}

impl TabV1 {
    fn from_tab(tab: &TabSession) -> Self {
        // 검색 결과 위치는 검색한 폴더로 남긴다(결과는 탐색 시점의 것이라 남기지 않는다).
        let location = match tab.location.for_session() {
            Location::FileSystem(path) => {
                LocationV1::FileSystem(path.to_string_lossy().into_owned())
            }
            Location::SearchResults(scope) => {
                LocationV1::FileSystem(scope.folder.to_string_lossy().into_owned())
            }
            Location::Shell(shell) => LocationV1::Shell {
                folder: match shell.folder() {
                    VirtualFolder::ThisPc => FolderV1::ThisPc,
                    VirtualFolder::RecycleBin => FolderV1::RecycleBin,
                    VirtualFolder::Device => FolderV1::Device,
                },
                id: shell
                    .id()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
            },
            Location::Zip(_) => todo!("#423"),
        };
        TabV1 {
            location,
            view: ViewV1 {
                mode: match tab.view.mode {
                    ViewMode::Details => ViewModeV1::Details,
                    ViewMode::Grid => ViewModeV1::Grid,
                },
                grid_size: match tab.view.grid_size {
                    GridSize::Small => GridSizeV1::Small,
                    GridSize::Medium => GridSizeV1::Medium,
                    GridSize::Large => GridSizeV1::Large,
                    GridSize::ExtraLarge => GridSizeV1::ExtraLarge,
                },
            },
            sort: SortV1 {
                column: match tab.sort.column {
                    Column::Name => ColumnV1::Name,
                    Column::Modified => ColumnV1::Modified,
                    Column::Type => ColumnV1::Type,
                    Column::Size => ColumnV1::Size,
                    // 위치 열은 검색 결과에만 뜻이 있고, 검색 결과 위치는 검색한 폴더로 남으므로 이름으로 남긴다(형식
                    // 버전을 올리지 않으려고 열을 더하지 않는다).
                    Column::Location | Column::Matches => ColumnV1::Name,
                    // 내 PC의 용량 열은 형식 버전을 올리지 않으려고 이름으로 남긴다(되살리면 이름 순서).
                    Column::TotalSize | Column::FreeSpace => ColumnV1::Name,
                    Column::OriginalLocation => ColumnV1::OriginalLocation,
                    Column::DateDeleted => ColumnV1::DateDeleted,
                    Column::CompressedSize => todo!("#423"),
                },
                direction: match tab.sort.direction {
                    Direction::Ascending => DirectionV1::Ascending,
                    Direction::Descending => DirectionV1::Descending,
                },
            },
        }
    }

    fn into_tab(self) -> TabSession {
        let location = match self.location {
            LocationV1::FileSystem(path) => Location::file_system(path),
            LocationV1::Shell { folder, id } => {
                let folder = match folder {
                    FolderV1::ThisPc => VirtualFolder::ThisPc,
                    FolderV1::RecycleBin => VirtualFolder::RecycleBin,
                    FolderV1::Device => VirtualFolder::Device,
                };
                match parse_hex(&id).filter(|id| facet_win::shell_folder::is_valid_id(id)) {
                    Some(id) => Location::shell(folder, id),
                    // 식별자가 깨졌으면 그 탭은 사용자 홈 폴더로 연다.
                    None => crate::window::home_location(),
                }
            }
        };
        TabSession {
            location,
            sort: Sort {
                column: match self.sort.column {
                    ColumnV1::Name => Column::Name,
                    ColumnV1::Modified => Column::Modified,
                    ColumnV1::Type => Column::Type,
                    ColumnV1::Size => Column::Size,
                    ColumnV1::OriginalLocation => Column::OriginalLocation,
                    ColumnV1::DateDeleted => Column::DateDeleted,
                },
                direction: match self.sort.direction {
                    DirectionV1::Ascending => Direction::Ascending,
                    DirectionV1::Descending => Direction::Descending,
                },
            },
            view: View {
                mode: match self.view.mode {
                    ViewModeV1::Details => ViewMode::Details,
                    ViewModeV1::Grid => ViewMode::Grid,
                },
                grid_size: match self.view.grid_size {
                    GridSizeV1::Small => GridSize::Small,
                    GridSizeV1::Medium => GridSize::Medium,
                    GridSizeV1::Large => GridSize::Large,
                    GridSizeV1::ExtraLarge => GridSize::ExtraLarge,
                },
            },
        }
    }
}
