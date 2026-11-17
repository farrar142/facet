//! 화면 수치 측정: 앱을 측정 모드로 띄워 콜드 스타트, 폴더 열기부터 첫 화면까지, 스크롤 프레임 시간을 잰다.
//!
//! 이 수치는 CI가 지키지 않는다. 사람이 자기 기계에서 `cargo xtask measure`로 재어 기록한다
//! (`docs/roadmap.md`의 「화면 수치는 사람이 잰다」). 앱과 주고받는 약속은
//! [`facet_core::measure`]에 있다.

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use facet_core::measure::{
    ENV_MENU, ENV_OPEN, ENV_OUT, ENV_RENAME, ENV_SCROLL_ROWS, ENV_SEARCH, ENV_START_US,
    ENV_WINDOWS, ScreenReport,
};

/// 콜드 스타트 통과 기준: 중앙값이 이보다 짧아야 한다.
pub const COLD_START_LIMIT: Duration = Duration::from_millis(200);

/// 「세션 복원 콜드 스타트」 통과 기준: 콜드 스타트와 같다. 세션을 되살려 모든 창의 목록이 든 첫 프레임까지다.
pub const SESSION_RESTORE_LIMIT: Duration = COLD_START_LIMIT;

/// 세션 복원 콜드 스타트를 잴 세션의 모양: 창 둘, 창마다 패널 둘, 패널마다 탭 다섯 개(창마다 탭 10개).
pub const SESSION_WINDOWS: usize = 2;
pub const SESSION_TABS_PER_PANEL: usize = 5;

/// 「폴더 열기부터 첫 화면까지」 통과 기준: 중앙값이 이보다 짧아야 한다(spec #16).
/// 이미 떠 있는 창에서 그 폴더로 이동을 시작한 때부터 목록이 든 첫 프레임이 나올 때까지다.
pub const OPEN_LIMIT: Duration = Duration::from_millis(150);

/// 「이름 확정부터 목록에 새 이름이 보이는 첫 프레임까지」 통과 기준: 중앙값이 한 프레임(16ms)보다 짧아야 한다.
/// 이름바꾸기를 확정(코어 `App::rename`)한 때부터 새 이름이 든 목록의 첫 프레임이 화면에 나갈 때까지다.
pub const RENAME_LIMIT: Duration = Duration::from_millis(16);

/// 「오른쪽 메뉴가 뜨는 시간」 통과 기준: 중앙값이 50ms보다 짧아야 한다. 시험 폴더 전체를 선택한 활성 탭에서 항목 메뉴를
/// 열 때(오른쪽 클릭과 같은 길)부터 메뉴가 그려진 첫 프레임이 화면에 나갈 때까지다. 근거: 사람은 입력 뒤 약 100ms
/// 안의 반응을 즉각으로 느끼고, 메뉴 구성(1ms 기준)과 한두 프레임 그리기에 쓰고도 절반이 남는 값이다.
pub const MENU_LIMIT: Duration = Duration::from_millis(50);

/// 「실시간 탐색 시작부터 첫 결과 화면까지」 통과 기준: 중앙값이 100ms보다 짧아야 한다(spec #21의 속도 기준).
/// 시험 폴더를 연 창에서 검색창 Enter와 같은 길로 실시간 탐색을 시작한 때부터 결과가 하나 이상 든 첫 프레임이 화면에
/// 나갈 때까지다.
pub const SEARCH_FIRST_LIMIT: Duration = Duration::from_millis(100);

/// 실시간 탐색 첫 결과를 잴 때 넘기는 이름 조각. 시험 폴더의 0번 항목(「보고서 000000.txt」)을 비롯해 열 개 중
/// 하나가 맞으므로 시험 폴더에 항목이 하나라도 있으면 반드시 결과가 있다(실시간 탐색 벤치마크와 같은 조각).
pub const SEARCH_QUERY: &str = "보고서";

/// #227: 「즉시 검색 글자 입력부터 결과 첫 화면까지」 통과 기준(중앙값 50ms 미만).
pub const INSTANT_FIRST_LIMIT: Duration = Duration::from_millis(50);

/// #227: 즉시 검색 첫 결과를 잴 때 넘기는 질의 글자(시험 폴더 0번 항목에 맞음).
pub const INSTANT_QUERY: &str = "보고서";

/// 프레임 시간 통과 기준: 최악값이 한 프레임 시간의 이 배수 이하여야 한다.
pub const FRAME_WORST_FACTOR: f64 = 2.0;

/// 화면 주사율을 알아내지 못했을 때 가정하는 값.
pub const ASSUMED_REFRESH_HZ: f64 = 60.0;

/// 앱 한 번 실행에 기다릴 최대 시간. 넘으면 앱을 끝내고 실패로 친다.
pub const RUN_TIMEOUT: Duration = Duration::from_secs(300);

/// 스크롤 한 걸음(프레임 하나)에 내리는 기본 행 수.
pub const DEFAULT_SCROLL_ROWS: usize = 50;

/// 중앙값(짝수 개면 가운데 둘 중 큰 쪽). 비었으면 0.
pub fn median(values: &[Duration]) -> Duration {
    let mut sorted = values.to_vec();
    sorted.sort();
    sorted.get(sorted.len() / 2).copied().unwrap_or_default()
}

/// 가장 큰 값. 비었으면 0.
pub fn worst(values: &[Duration]) -> Duration {
    values.iter().max().copied().unwrap_or_default()
}

/// 「폴더 열기」를 잴 때 앱을 처음 띄울 폴더: 시험 폴더의 상위. 사용자가 상위 폴더에서
/// 시험 폴더로 들어가는 것과 같다. 상위가 없으면(드라이브 루트) `None`.
pub fn open_start(folder: &Path) -> Option<PathBuf> {
    folder.parent().map(Path::to_path_buf)
}

/// 측정 기계의 사양. 알아내지 못한 항목은 `None`이다.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SystemSpec {
    pub cpu: Option<String>,
    pub memory_bytes: Option<u64>,
    pub disk: Option<String>,
    pub os: Option<String>,
    /// 화면 주사율(Hz).
    pub refresh_hz: Option<f64>,
}

const SPEC_SCRIPT: &str = "\
$ErrorActionPreference = 'SilentlyContinue'
'cpu=' + (Get-CimInstance Win32_Processor | Select-Object -First 1).Name
'memory_bytes=' + (Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory
$os = Get-CimInstance Win32_OperatingSystem
'os=' + $os.Caption + ' ' + $os.Version
'refresh_hz=' + (Get-CimInstance Win32_VideoController | Where-Object { $_.CurrentRefreshRate } | Select-Object -First 1).CurrentRefreshRate
";

/// `path`가 있는 드라이브 문자(대문자). `C:\…`·`\\?\C:\…`가 아니면(UNC 경로 등) `None`.
pub fn drive_letter(path: &Path) -> Option<char> {
    use std::path::{Component, Prefix};
    match path.components().next()? {
        Component::Prefix(p) => match p.kind() {
            Prefix::Disk(d) | Prefix::VerbatimDisk(d) => Some(char::from(d).to_ascii_uppercase()),
            _ => None,
        },
        _ => None,
    }
}

/// 사양을 읽는 PowerShell 스크립트. 디스크는 첫 번째 물리 디스크가 아니라
/// `folder`가 있는 드라이브의 디스크를 적는다(이름과 연결 방식, 예: `… SSD (NVMe)`).
pub fn spec_script(folder: &Path) -> String {
    let mut script = SPEC_SCRIPT.to_owned();
    if let Some(letter) = drive_letter(folder) {
        let _ = writeln!(
            script,
            "$d = Get-Partition -DriveLetter {letter} | Get-Disk\n\
             'disk=' + $d.FriendlyName + ' (' + $d.BusType + ')'"
        );
    }
    script
}

impl SystemSpec {
    /// PowerShell로 이 기계의 사양과 `folder`가 있는 디스크를 읽는다. 읽지 못한 항목은 비운다.
    pub fn collect(folder: &Path) -> Self {
        let folder = std::path::absolute(folder).unwrap_or_else(|_| folder.to_path_buf());
        Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &spec_script(&folder),
            ])
            .stdin(Stdio::null())
            .output()
            .ok()
            .map(|o| Self::parse(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or_default()
    }

    /// `이름=값` 줄들에서 사양을 읽는다. 빈 값과 모르는 줄은 무시한다.
    pub fn parse(text: &str) -> Self {
        let mut spec = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            match key.trim() {
                "cpu" => spec.cpu = Some(value.to_owned()),
                "memory_bytes" => spec.memory_bytes = value.parse().ok(),
                "disk" => spec.disk = Some(value.to_owned()),
                "os" => spec.os = Some(value.to_owned()),
                "refresh_hz" => spec.refresh_hz = value.parse().ok().filter(|hz: &f64| *hz > 1.0),
                _ => {}
            }
        }
        spec
    }
}

/// 한 프레임의 시간. 주사율을 모르면 [`ASSUMED_REFRESH_HZ`]를 가정한다.
pub fn frame_budget(refresh_hz: Option<f64>) -> Duration {
    Duration::from_secs_f64(1.0 / refresh_hz.unwrap_or(ASSUMED_REFRESH_HZ))
}

/// 측정 결과 전체.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenResult {
    pub folder: PathBuf,
    pub item_count: usize,
    pub scroll_rows: usize,
    pub cold_starts: Vec<Duration>,
    /// 세션 복원 콜드 스타트(회차마다 하나): 탭 10개짜리 창 둘의 세션으로 띄워 모든 창의 첫 프레임까지.
    pub session_starts: Vec<Duration>,
    /// 이미 떠 있는 창에서 폴더로 이동해 첫 화면이 나올 때까지(회차마다 하나).
    pub opens: Vec<Duration>,
    /// 이름바꾸기를 확정한 때부터 새 이름이 든 첫 프레임까지(회차마다 하나).
    pub renames: Vec<Duration>,
    /// 시험 폴더 전체를 선택하고 항목 메뉴를 열어 그 프레임까지(회차마다 하나).
    pub menus: Vec<Duration>,
    /// 실시간 탐색을 시작해 첫 결과가 든 프레임까지(회차마다 하나).
    pub searches: Vec<Duration>,
    /// #227: 즉시 검색 글자를 넘겨 첫 결과가 든 프레임까지(회차마다 하나).
    pub instant_searches: Vec<Duration>,
    /// 스크롤하는 동안의 프레임 시간들(스크롤 회차 전체).
    pub frames: Vec<Duration>,
    pub scroll_runs: usize,
    pub spec: SystemSpec,
    pub debug_build: bool,
}

/// 기준과 통과 여부.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Verdict {
    pub cold_median: Duration,
    pub cold_worst: Duration,
    pub cold_pass: bool,
    pub session_median: Duration,
    pub session_worst: Duration,
    pub session_pass: bool,
    pub open_median: Duration,
    pub open_worst: Duration,
    pub open_pass: bool,
    pub rename_median: Duration,
    pub rename_worst: Duration,
    pub rename_pass: bool,
    pub menu_median: Duration,
    pub menu_worst: Duration,
    pub menu_pass: bool,
    pub search_median: Duration,
    pub search_worst: Duration,
    pub search_pass: bool,
    /// #227: 즉시 검색 첫 결과 중앙값·최악값·통과 여부.
    pub instant_median: Duration,
    pub instant_worst: Duration,
    pub instant_pass: bool,
    pub frame_median: Duration,
    pub frame_worst: Duration,
    /// 프레임 최악값의 상한(한 프레임의 두 배).
    pub frame_worst_limit: Duration,
    pub frame_pass: bool,
    /// 상한을 넘은 프레임 수. 최악값 하나만 튄 것인지(기계의 순간 정지 등) 늘 끊기는 것인지 가른다.
    pub frames_over_limit: usize,
    /// 주사율을 알아내지 못해 [`ASSUMED_REFRESH_HZ`]를 가정했다.
    pub refresh_assumed: bool,
}

impl Verdict {
    pub fn pass(&self) -> bool {
        self.cold_pass
            && self.session_pass
            && self.open_pass
            && self.rename_pass
            && self.menu_pass
            && self.search_pass
            && self.frame_pass
    }
}

impl ScreenResult {
    /// 통과 기준과 견준다. 잰 값(콜드 스타트, 폴더 열기, 프레임)이 하나도 없으면 그 항목은 통과하지 못한다.
    pub fn verdict(&self) -> Verdict {
        let cold_median = median(&self.cold_starts);
        let open_median = median(&self.opens);
        let session_median = median(&self.session_starts);
        let rename_median = median(&self.renames);
        let menu_median = median(&self.menus);
        let search_median = median(&self.searches);
        let frame_worst = worst(&self.frames);
        let frame_worst_limit = frame_budget(self.spec.refresh_hz).mul_f64(FRAME_WORST_FACTOR);
        Verdict {
            cold_median,
            cold_worst: worst(&self.cold_starts),
            cold_pass: !self.cold_starts.is_empty() && cold_median < COLD_START_LIMIT,
            session_median,
            session_worst: worst(&self.session_starts),
            session_pass: !self.session_starts.is_empty() && session_median < SESSION_RESTORE_LIMIT,
            open_median,
            open_worst: worst(&self.opens),
            open_pass: !self.opens.is_empty() && open_median < OPEN_LIMIT,
            rename_median,
            rename_worst: worst(&self.renames),
            rename_pass: !self.renames.is_empty() && rename_median < RENAME_LIMIT,
            menu_median,
            menu_worst: worst(&self.menus),
            menu_pass: !self.menus.is_empty() && menu_median < MENU_LIMIT,
            search_median,
            search_worst: worst(&self.searches),
            search_pass: !self.searches.is_empty() && search_median < SEARCH_FIRST_LIMIT,
            instant_median: Duration::ZERO, // #227
            instant_worst: Duration::ZERO,  // #227
            instant_pass: false,            // #227
            frame_median: median(&self.frames),
            frame_worst,
            frame_worst_limit,
            frame_pass: !self.frames.is_empty() && frame_worst <= frame_worst_limit,
            frames_over_limit: self
                .frames
                .iter()
                .filter(|f| **f > frame_worst_limit)
                .count(),
            refresh_assumed: self.spec.refresh_hz.is_none(),
        }
    }

    /// 사람이 읽는 표.
    pub fn to_table(&self) -> String {
        let v = self.verdict();
        let ms = |d: Duration| format!("{:.1}ms", d.as_secs_f64() * 1000.0);
        let mark = |ok: bool| if ok { "통과" } else { "실패" };
        let mut t = String::new();
        if self.debug_build {
            t.push_str("경고: 디버그 빌드입니다. 수치는 릴리스 빌드에서 재야 의미가 있습니다.\n");
        }
        let _ = writeln!(
            t,
            "측정: {} ({}항목)",
            self.folder.display(),
            self.item_count
        );
        t.push_str("\n기계:\n");
        let unknown = || "알 수 없음".to_owned();
        let _ = writeln!(
            t,
            "  CPU     {}",
            self.spec.cpu.clone().unwrap_or_else(unknown)
        );
        let _ = writeln!(
            t,
            "  메모리  {}",
            self.spec.memory_bytes.map_or_else(unknown, |b| format!(
                "{:.1} GiB",
                b as f64 / 1_073_741_824.0
            ))
        );
        let _ = writeln!(
            t,
            "  디스크  {}",
            self.spec.disk.clone().unwrap_or_else(unknown)
        );
        let _ = writeln!(
            t,
            "  화면    {}",
            self.spec.refresh_hz.map_or_else(
                || format!("주사율을 알 수 없어 {ASSUMED_REFRESH_HZ}Hz로 가정"),
                |hz| format!("{hz}Hz")
            )
        );
        let _ = writeln!(
            t,
            "  Windows {}",
            self.spec.os.clone().unwrap_or_else(unknown)
        );
        t.push_str("\n항목                      측정값      기준                       결과\n");
        let _ = writeln!(
            t,
            "콜드 스타트 중앙값 ({}회)  {:<10}  < {}ms                   {}",
            self.cold_starts.len(),
            ms(v.cold_median),
            COLD_START_LIMIT.as_millis(),
            mark(v.cold_pass)
        );
        let _ = writeln!(
            t,
            "콜드 스타트 최악값        {:<10}  (참고)",
            ms(v.cold_worst)
        );
        let _ = writeln!(
            t,
            "세션 복원 콜드 스타트 중앙값 ({}회)  {:<10}  < {}ms                   {}",
            self.session_starts.len(),
            ms(v.session_median),
            SESSION_RESTORE_LIMIT.as_millis(),
            mark(v.session_pass)
        );
        let _ = writeln!(
            t,
            "세션 복원 콜드 스타트 최악값  {:<10}  (참고)",
            ms(v.session_worst)
        );
        let _ = writeln!(
            t,
            "폴더 열기 첫 화면 중앙값 ({}회)  {:<10}  < {}ms                   {}",
            self.opens.len(),
            ms(v.open_median),
            OPEN_LIMIT.as_millis(),
            mark(v.open_pass)
        );
        let _ = writeln!(
            t,
            "폴더 열기 첫 화면 최악값  {:<10}  (참고)",
            ms(v.open_worst)
        );
        let _ = writeln!(
            t,
            "이름 확정 프레임 중앙값 ({}회)  {:<10}  < {}ms                   {}",
            self.renames.len(),
            ms(v.rename_median),
            RENAME_LIMIT.as_millis(),
            mark(v.rename_pass)
        );
        let _ = writeln!(
            t,
            "이름 확정 프레임 최악값   {:<10}  (참고)",
            ms(v.rename_worst)
        );
        let _ = writeln!(
            t,
            "오른쪽 메뉴 중앙값 ({}회)  {:<10}  < {}ms                   {}",
            self.menus.len(),
            ms(v.menu_median),
            MENU_LIMIT.as_millis(),
            mark(v.menu_pass)
        );
        let _ = writeln!(
            t,
            "오른쪽 메뉴 최악값        {:<10}  (참고)",
            ms(v.menu_worst)
        );
        let _ = writeln!(
            t,
            "실시간 탐색 첫 결과 중앙값 ({}회)  {:<10}  < {}ms                   {}",
            self.searches.len(),
            ms(v.search_median),
            SEARCH_FIRST_LIMIT.as_millis(),
            mark(v.search_pass)
        );
        let _ = writeln!(
            t,
            "실시간 탐색 첫 결과 최악값  {:<10}  (참고)",
            ms(v.search_worst)
        );
        let _ = writeln!(
            t,
            "프레임 시간 중앙값        {:<10}  (참고: 한 프레임 {})",
            ms(v.frame_median),
            ms(frame_budget(self.spec.refresh_hz))
        );
        let _ = writeln!(
            t,
            "프레임 시간 최악값        {:<10}  <= {} (한 프레임의 {}배)  {}",
            ms(v.frame_worst),
            ms(v.frame_worst_limit),
            FRAME_WORST_FACTOR,
            mark(v.frame_pass)
        );
        let _ = writeln!(
            t,
            "\n스크롤: 프레임마다 {}행, {}회, 프레임 {}개 (상한을 넘은 프레임 {}개)",
            self.scroll_rows,
            self.scroll_runs,
            self.frames.len(),
            v.frames_over_limit
        );
        let _ = writeln!(t, "전체: {}", mark(v.pass()));
        t
    }

    /// 기계가 읽는 JSON.
    pub fn to_json(&self) -> String {
        let v = self.verdict();
        let ms = |d: Duration| format!("{:.3}", d.as_secs_f64() * 1000.0);
        let list = |ds: &[Duration]| ds.iter().map(|d| ms(*d)).collect::<Vec<_>>().join(", ");
        let opt_str = |s: &Option<String>| s.as_deref().map_or("null".to_owned(), json_string);
        let spec = &self.spec;
        let mut j = String::from("{\n");
        let _ = writeln!(
            j,
            "  \"folder\": {},",
            json_string(&self.folder.to_string_lossy())
        );
        let _ = writeln!(j, "  \"item_count\": {},", self.item_count);
        let _ = writeln!(j, "  \"debug_build\": {},", self.debug_build);
        let _ = writeln!(j, "  \"machine\": {{");
        let _ = writeln!(j, "    \"cpu\": {},", opt_str(&spec.cpu));
        let _ = writeln!(
            j,
            "    \"memory_bytes\": {},",
            spec.memory_bytes
                .map_or("null".to_owned(), |b| b.to_string())
        );
        let _ = writeln!(j, "    \"disk\": {},", opt_str(&spec.disk));
        let _ = writeln!(j, "    \"os\": {},", opt_str(&spec.os));
        let _ = writeln!(
            j,
            "    \"refresh_hz\": {},",
            spec.refresh_hz
                .map_or("null".to_owned(), |hz| hz.to_string())
        );
        let _ = writeln!(j, "    \"refresh_hz_assumed\": {}", v.refresh_assumed);
        let _ = writeln!(j, "  }},");
        let _ = writeln!(j, "  \"cold_start\": {{");
        let _ = writeln!(j, "    \"runs_ms\": [{}],", list(&self.cold_starts));
        let _ = writeln!(j, "    \"median_ms\": {},", ms(v.cold_median));
        let _ = writeln!(j, "    \"worst_ms\": {},", ms(v.cold_worst));
        let _ = writeln!(j, "    \"limit_ms\": {},", COLD_START_LIMIT.as_millis());
        let _ = writeln!(j, "    \"pass\": {}", v.cold_pass);
        let _ = writeln!(j, "  }},");
        let _ = writeln!(j, "  \"session_restore_cold_start\": {{");
        let _ = writeln!(j, "    \"windows\": {SESSION_WINDOWS},");
        let _ = writeln!(
            j,
            "    \"tabs_per_window\": {},",
            2 * SESSION_TABS_PER_PANEL
        );
        let _ = writeln!(j, "    \"runs_ms\": [{}],", list(&self.session_starts));
        let _ = writeln!(j, "    \"median_ms\": {},", ms(v.session_median));
        let _ = writeln!(j, "    \"worst_ms\": {},", ms(v.session_worst));
        let _ = writeln!(
            j,
            "    \"limit_ms\": {},",
            SESSION_RESTORE_LIMIT.as_millis()
        );
        let _ = writeln!(j, "    \"pass\": {}", v.session_pass);
        let _ = writeln!(j, "  }},");
        let _ = writeln!(j, "  \"folder_open\": {{");
        let _ = writeln!(j, "    \"runs_ms\": [{}],", list(&self.opens));
        let _ = writeln!(j, "    \"median_ms\": {},", ms(v.open_median));
        let _ = writeln!(j, "    \"worst_ms\": {},", ms(v.open_worst));
        let _ = writeln!(j, "    \"limit_ms\": {},", OPEN_LIMIT.as_millis());
        let _ = writeln!(j, "    \"pass\": {}", v.open_pass);
        let _ = writeln!(j, "  }},");
        let _ = writeln!(j, "  \"rename_commit\": {{");
        let _ = writeln!(j, "    \"runs_ms\": [{}],", list(&self.renames));
        let _ = writeln!(j, "    \"median_ms\": {},", ms(v.rename_median));
        let _ = writeln!(j, "    \"worst_ms\": {},", ms(v.rename_worst));
        let _ = writeln!(j, "    \"limit_ms\": {},", RENAME_LIMIT.as_millis());
        let _ = writeln!(j, "    \"pass\": {}", v.rename_pass);
        let _ = writeln!(j, "  }},");
        let _ = writeln!(j, "  \"context_menu\": {{");
        let _ = writeln!(j, "    \"runs_ms\": [{}],", list(&self.menus));
        let _ = writeln!(j, "    \"median_ms\": {},", ms(v.menu_median));
        let _ = writeln!(j, "    \"worst_ms\": {},", ms(v.menu_worst));
        let _ = writeln!(j, "    \"limit_ms\": {},", MENU_LIMIT.as_millis());
        let _ = writeln!(j, "    \"pass\": {}", v.menu_pass);
        let _ = writeln!(j, "  }},");
        let _ = writeln!(j, "  \"live_search_first_result\": {{");
        let _ = writeln!(j, "    \"query\": {},", json_string(SEARCH_QUERY));
        let _ = writeln!(j, "    \"runs_ms\": [{}],", list(&self.searches));
        let _ = writeln!(j, "    \"median_ms\": {},", ms(v.search_median));
        let _ = writeln!(j, "    \"worst_ms\": {},", ms(v.search_worst));
        let _ = writeln!(j, "    \"limit_ms\": {},", SEARCH_FIRST_LIMIT.as_millis());
        let _ = writeln!(j, "    \"pass\": {}", v.search_pass);
        let _ = writeln!(j, "  }},");
        let _ = writeln!(j, "  \"scroll\": {{");
        let _ = writeln!(j, "    \"rows_per_frame\": {},", self.scroll_rows);
        let _ = writeln!(j, "    \"runs\": {},", self.scroll_runs);
        let _ = writeln!(j, "    \"frame_count\": {},", self.frames.len());
        let _ = writeln!(j, "    \"frame_median_ms\": {},", ms(v.frame_median));
        let _ = writeln!(j, "    \"frame_worst_ms\": {},", ms(v.frame_worst));
        let _ = writeln!(
            j,
            "    \"frame_worst_limit_ms\": {},",
            ms(v.frame_worst_limit)
        );
        let _ = writeln!(j, "    \"frames_over_limit\": {},", v.frames_over_limit);
        let _ = writeln!(j, "    \"pass\": {}", v.frame_pass);
        let _ = writeln!(j, "  }},");
        let _ = writeln!(j, "  \"pass\": {}", v.pass());
        j.push_str("}\n");
        j
    }
}

/// JSON 문자열 리터럴로 만든다.
pub fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// 앱을 측정 모드로 한 번 띄워 결과를 받는다. `scroll_rows`가 0이면 첫 화면만 잰다.
/// `open`을 주면 첫 화면 뒤에 그 폴더로 이동해 첫 화면이 나올 때까지를 잰다(스크롤은 하지 않는다).
pub fn run_app(
    app: &Path,
    folder: &Path,
    open: Option<&Path>,
    scroll_rows: usize,
    timeout: Duration,
) -> io::Result<ScreenReport> {
    let out = std::env::temp_dir().join(format!("facet-measure-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&out);
    let start_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64);
    let mut command = Command::new(app);
    command
        .arg(folder)
        .env_remove(ENV_WINDOWS)
        .env(ENV_OUT, &out)
        .env(ENV_START_US, start_us.to_string())
        .env(ENV_SCROLL_ROWS, scroll_rows.to_string())
        .env_remove(ENV_OPEN)
        .env_remove(ENV_RENAME)
        .env_remove(ENV_MENU)
        .env_remove(ENV_SEARCH)
        .stdin(Stdio::null());
    if let Some(open) = open {
        command.env(ENV_OPEN, open);
    }
    run_to_report(command, &out, timeout)
}

/// 이름바꾸기 확정 프레임을 잰다: `folder`를 열어 첫 화면이 나오면 그 폴더의 `from` 항목을 `to`로 바꾸고(코어
/// 이름바꾸기) 확정부터 목록에 새 이름이 든 첫 프레임까지를 앱이 알린다.
pub fn run_app_rename(
    app: &Path,
    folder: &Path,
    from: &str,
    to: &str,
    timeout: Duration,
) -> io::Result<ScreenReport> {
    let out = std::env::temp_dir().join(format!("facet-measure-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&out);
    let start_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64);
    let mut command = Command::new(app);
    command
        .arg(folder)
        .env_remove(ENV_WINDOWS)
        .env(ENV_OUT, &out)
        .env(ENV_START_US, start_us.to_string())
        .env(ENV_SCROLL_ROWS, "0")
        .env_remove(ENV_OPEN)
        .env(ENV_RENAME, format!("{from}|{to}"))
        .env_remove(ENV_MENU)
        .env_remove(ENV_SEARCH)
        .stdin(Stdio::null());
    run_to_report(command, &out, timeout)
}

/// 오른쪽 메뉴를 잰다: `folder`를 열어 첫 화면이 나오면 그 폴더 전체를 선택하고 항목 메뉴를 열어, 여는 때부터 메뉴가
/// 그려진 첫 프레임까지를 앱이 알린다.
pub fn run_app_menu(app: &Path, folder: &Path, timeout: Duration) -> io::Result<ScreenReport> {
    let out = std::env::temp_dir().join(format!("facet-measure-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&out);
    let start_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64);
    let mut command = Command::new(app);
    command
        .arg(folder)
        .env_remove(ENV_WINDOWS)
        .env(ENV_OUT, &out)
        .env(ENV_START_US, start_us.to_string())
        .env(ENV_SCROLL_ROWS, "0")
        .env_remove(ENV_OPEN)
        .env_remove(ENV_RENAME)
        .env(ENV_MENU, "1")
        .env_remove(ENV_SEARCH)
        .stdin(Stdio::null());
    run_to_report(command, &out, timeout)
}

/// 실시간 탐색 첫 결과를 잰다: `folder`를 열어 첫 화면이 나오면 그 폴더 아래로 `query`의 실시간 탐색을 검색창과 같은
/// 길로 시작하고, 시작한 때부터 첫 결과가 든 프레임까지를 앱이 알린다.
pub fn run_app_search(
    app: &Path,
    folder: &Path,
    query: &str,
    timeout: Duration,
) -> io::Result<ScreenReport> {
    let out = std::env::temp_dir().join(format!("facet-measure-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&out);
    let start_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64);
    let mut command = Command::new(app);
    command
        .arg(folder)
        .env_remove(ENV_WINDOWS)
        .env(ENV_OUT, &out)
        .env(ENV_START_US, start_us.to_string())
        .env(ENV_SCROLL_ROWS, "0")
        .env_remove(ENV_OPEN)
        .env_remove(ENV_RENAME)
        .env_remove(ENV_MENU)
        .env(ENV_SEARCH, query)
        .stdin(Stdio::null());
    run_to_report(command, &out, timeout)
}

/// 실시간 탐색 첫 결과 측정의 보고에서 잰 시간을 꺼낸다. 시간이 없거나 그 프레임에 결과가 없으면(앱이 결과 없는
/// 프레임을 잘못 셌다) 오류다.
pub fn search_frame(report: &ScreenReport) -> Result<Duration, String> {
    let frame = report
        .search_frame
        .ok_or("앱이 실시간 탐색 첫 결과 프레임 시각을 알리지 않았습니다")?;
    if report.rows == 0 {
        return Err("실시간 탐색 첫 결과 프레임에 결과가 없습니다".to_owned());
    }
    Ok(frame)
}

/// #227: 즉시 검색 첫 결과 측정의 보고에서 잰 시간을 꺼낸다(시간 없음·결과 0·강등이면 오류).
pub fn instant_frame(_report: &ScreenReport) -> Result<Duration, String> {
    todo!("#227")
}

/// #227: `folder`가 든 볼륨의 합성 인덱스(`folder` 아래 항목 `count`개)를 거르지 않는 백엔드로 `pipe`에 띄운다.
#[cfg(windows)]
pub fn serve_instant_index(
    _pipe: &str,
    _folder: &Path,
    _count: usize,
) -> io::Result<facet_indexer::pipe::PipeServer> {
    todo!("#227")
}

/// 세션 복원 콜드 스타트를 잰다: 인자 없이 `session` 파일(`FACET_SESSION_PATH`)로 앱을 띄워 창 `windows`개의
/// 첫 프레임까지를 잰다. 사용자의 세션 파일은 건드리지 않는다(앱은 환경 변수가 가리키는 이 파일만 읽고 쓴다).
pub fn run_app_session(
    app: &Path,
    session: &Path,
    windows: usize,
    timeout: Duration,
) -> io::Result<ScreenReport> {
    let out = std::env::temp_dir().join(format!("facet-measure-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&out);
    let start_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64);
    let mut command = Command::new(app);
    command
        .env(ENV_OUT, &out)
        .env(ENV_START_US, start_us.to_string())
        .env(ENV_SCROLL_ROWS, "0")
        .env(ENV_WINDOWS, windows.to_string())
        .env(facet_core::session::ENV_PATH, session)
        .env_remove(ENV_OPEN)
        .env_remove(ENV_RENAME)
        .env_remove(ENV_MENU)
        .env_remove(ENV_SEARCH)
        .stdin(Stdio::null());
    run_to_report(command, &out, timeout)
}

/// 앱을 띄워 끝나기를 기다리고(`timeout`이 지나면 멈춘다) 앱이 `out`에 쓴 결과를 읽는다.
fn run_to_report(mut command: Command, out: &Path, timeout: Duration) -> io::Result<ScreenReport> {
    let mut child = command.spawn()?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("앱이 {}초 안에 끝나지 않아 멈췄습니다", timeout.as_secs()),
            ));
        }
        sleep(Duration::from_millis(20));
    };
    if !status.success() {
        return Err(io::Error::other(format!(
            "앱이 실패로 끝났습니다: {status}"
        )));
    }
    let text = std::fs::read_to_string(out)?;
    let _ = std::fs::remove_file(out);
    ScreenReport::parse(&text).map_err(io::Error::other)
}

/// 측정 계획.
pub struct Plan<'a> {
    pub app: &'a Path,
    pub folder: &'a Path,
    pub item_count: usize,
    /// 콜드 스타트와 폴더 열기를 각각 잴 횟수.
    pub runs: usize,
    /// 스크롤을 잴 횟수.
    pub scroll_runs: usize,
    pub scroll_rows: usize,
    /// 지정하면 PowerShell로 알아낸 주사율 대신 이 값을 쓴다.
    pub refresh_hz: Option<f64>,
}

/// 앱을 `runs`번 띄워 콜드 스타트를, 시험 폴더의 상위로 `runs`번 띄워 그 안의 시험 폴더로 이동하는
/// 폴더 열기를, `scroll_runs`번 띄워 스크롤 프레임을 잰다.
pub fn measure(plan: &Plan<'_>) -> Result<ScreenResult, String> {
    let check_rows = |r: &ScreenReport| {
        if r.rows == plan.item_count {
            Ok(())
        } else {
            Err(format!(
                "항목 수가 다릅니다: {}개를 기대했지만 앱이 {}개를 읽었습니다",
                plan.item_count, r.rows
            ))
        }
    };
    let mut cold_starts = Vec::new();
    for n in 1..=plan.runs {
        eprintln!("콜드 스타트 {n}/{}…", plan.runs);
        let r = run_app(plan.app, plan.folder, None, 0, RUN_TIMEOUT).map_err(|e| e.to_string())?;
        check_rows(&r)?;
        cold_starts.push(
            r.first_frame
                .ok_or("앱이 첫 화면 시각을 알리지 않았습니다")?,
        );
    }
    let session_starts = measure_session_restore(plan)?;
    let folder = std::path::absolute(plan.folder).map_err(|e| e.to_string())?;
    let start = open_start(&folder).ok_or_else(|| {
        format!(
            "{}의 상위 폴더가 없어 폴더 열기를 잴 수 없습니다",
            folder.display()
        )
    })?;
    let mut opens = Vec::new();
    for n in 1..=plan.runs {
        eprintln!("폴더 열기 {n}/{}…", plan.runs);
        let r =
            run_app(plan.app, &start, Some(&folder), 0, RUN_TIMEOUT).map_err(|e| e.to_string())?;
        check_rows(&r)?;
        opens.push(
            r.open_frame
                .ok_or("앱이 폴더 열기 첫 화면 시각을 알리지 않았습니다")?,
        );
    }
    let renames = measure_renames(plan)?;
    let mut menus = Vec::new();
    for n in 1..=plan.runs {
        eprintln!("오른쪽 메뉴 {n}/{}…", plan.runs);
        let r = run_app_menu(plan.app, plan.folder, RUN_TIMEOUT).map_err(|e| e.to_string())?;
        check_rows(&r)?;
        menus.push(
            r.menu_frame
                .ok_or("앱이 오른쪽 메뉴 프레임 시각을 알리지 않았습니다")?,
        );
    }
    let mut searches = Vec::new();
    for n in 1..=plan.runs {
        eprintln!("실시간 탐색 첫 결과 {n}/{}…", plan.runs);
        let r = run_app_search(plan.app, plan.folder, SEARCH_QUERY, RUN_TIMEOUT)
            .map_err(|e| e.to_string())?;
        searches.push(search_frame(&r)?);
    }
    let mut frames = Vec::new();
    for n in 1..=plan.scroll_runs {
        eprintln!("스크롤 {n}/{}…", plan.scroll_runs);
        let r = run_app(plan.app, plan.folder, None, plan.scroll_rows, RUN_TIMEOUT)
            .map_err(|e| e.to_string())?;
        check_rows(&r)?;
        frames.extend(r.frames);
    }
    let mut spec = SystemSpec::collect(plan.folder);
    if plan.refresh_hz.is_some() {
        spec.refresh_hz = plan.refresh_hz;
    }
    Ok(ScreenResult {
        folder: plan.folder.to_path_buf(),
        item_count: plan.item_count,
        scroll_rows: plan.scroll_rows,
        cold_starts,
        session_starts,
        opens,
        renames,
        menus,
        searches,
        instant_searches: Vec::new(), // #227
        frames,
        scroll_runs: plan.scroll_runs,
        spec,
        debug_build: cfg!(debug_assertions),
    })
}

/// 세션 복원 콜드 스타트용 세션 파일을 `dir` 아래에 만든다: 창 [`SESSION_WINDOWS`]개, 창마다 패널 둘, 패널마다
/// 탭 [`SESSION_TABS_PER_PANEL`]개. 탭마다 `dir` 아래 작은 폴더(파일 몇 개)를 연다. 세션 파일 자리를 돌려준다.
pub fn write_session_fixture(dir: &Path) -> io::Result<PathBuf> {
    let root = dir.join("folders");
    let location = |w: usize, p: usize, t: usize| {
        let folder = root
            .join(format!("창{w}"))
            .join(format!("패널{p}"))
            .join(format!("탭 {t}"));
        facet_core::Location::file_system(folder)
    };
    let session = crate::session_fixture(SESSION_WINDOWS, SESSION_TABS_PER_PANEL, location);
    for window in &session.windows {
        for tab in window.panels.iter().flat_map(|p| &p.tabs) {
            if let Some(folder) = tab.location.file_system_path() {
                std::fs::create_dir_all(folder)?;
                for n in 0..20 {
                    std::fs::write(folder.join(format!("파일 {n:02}.txt")), b"facet")?;
                }
            }
        }
    }
    let path = dir.join("session.json");
    std::fs::write(&path, session.to_json())?;
    Ok(path)
}

/// 세션 복원 콜드 스타트를 `plan.runs`번 잰다. 세션 파일은 임시 폴더에 만들어 `FACET_SESSION_PATH`로 앱에
/// 알리므로 사용자의 세션 파일(`%APPDATA%\facet\session.json`)은 읽지도 바꾸지도 않는다. 앱이 세션을 닫을 때
/// 되쓰므로 회차마다 세션 파일을 새로 만든다.
fn measure_session_restore(plan: &Plan<'_>) -> Result<Vec<Duration>, String> {
    let dir = std::env::temp_dir().join(format!("facet-measure-session-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let result = (|| {
        let mut starts = Vec::new();
        for n in 1..=plan.runs {
            eprintln!("세션 복원 콜드 스타트 {n}/{}…", plan.runs);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let session = write_session_fixture(&dir).map_err(|e| e.to_string())?;
            let r = run_app_session(plan.app, &session, SESSION_WINDOWS, RUN_TIMEOUT)
                .map_err(|e| e.to_string())?;
            if r.windows != SESSION_WINDOWS {
                return Err(format!(
                    "창 {SESSION_WINDOWS}개를 기대했지만 앱이 첫 프레임을 낸 창은 {}개입니다",
                    r.windows
                ));
            }
            starts.push(
                r.first_frame
                    .ok_or("앱이 세션 복원 첫 화면 시각을 알리지 않았습니다")?,
            );
        }
        Ok(starts)
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// 이름 확정 프레임을 `plan.runs`번 잰다. 시험 폴더의 첫 항목([`crate::test_folder_entry`]`(0)`)을 새 이름으로
/// 바꾸게 하고, 회차가 끝날 때마다(실패해도) 원래 이름으로 되돌려 시험 폴더가 그대로 남게 한다.
fn measure_renames(plan: &Plan<'_>) -> Result<Vec<Duration>, String> {
    let from = crate::test_folder_entry(0).name;
    let to = "이름 바꾼 항목.txt";
    let before = plan.folder.join(&from);
    let after = plan.folder.join(to);
    let mut renames = Vec::new();
    for n in 1..=plan.runs {
        eprintln!("이름 확정 프레임 {n}/{}…", plan.runs);
        let r = run_app_rename(plan.app, plan.folder, &from, to, RUN_TIMEOUT);
        // 앱이 바꿨든 아니든 원래 이름으로 돌려 놓는다.
        let restored = if after.exists() {
            std::fs::rename(&after, &before)
        } else {
            Ok(())
        };
        let r = r.map_err(|e| e.to_string())?;
        restored.map_err(|e| format!("{}를 되돌리지 못함: {e}", after.display()))?;
        renames.push(r.rename_frame.ok_or(
            "앱이 이름 확정 프레임 시각을 알리지 않았습니다(항목을 바꾸지 못했을 수 있음)",
        )?);
    }
    Ok(renames)
}
