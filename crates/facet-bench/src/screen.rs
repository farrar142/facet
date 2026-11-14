//! 화면 수치 측정: 앱을 측정 모드로 띄워 콜드 스타트와 스크롤 프레임 시간을 잰다.
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

use facet_core::measure::{ENV_OUT, ENV_SCROLL_ROWS, ENV_START_US, ScreenReport};

/// 콜드 스타트 통과 기준: 중앙값이 이보다 짧아야 한다.
pub const COLD_START_LIMIT: Duration = Duration::from_millis(200);

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
    pub cold_pass: bool,
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
        self.cold_pass && self.frame_pass
    }
}

impl ScreenResult {
    /// 통과 기준과 견준다. 프레임이 하나도 없으면 통과하지 못한다.
    pub fn verdict(&self) -> Verdict {
        let cold_median = median(&self.cold_starts);
        let frame_worst = worst(&self.frames);
        let frame_worst_limit = frame_budget(self.spec.refresh_hz).mul_f64(FRAME_WORST_FACTOR);
        Verdict {
            cold_median,
            cold_pass: !self.cold_starts.is_empty() && cold_median < COLD_START_LIMIT,
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
        let _ = writeln!(j, "    \"limit_ms\": {},", COLD_START_LIMIT.as_millis());
        let _ = writeln!(j, "    \"pass\": {}", v.cold_pass);
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
pub fn run_app(
    app: &Path,
    folder: &Path,
    scroll_rows: usize,
    timeout: Duration,
) -> io::Result<ScreenReport> {
    let out = std::env::temp_dir().join(format!("facet-measure-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&out);
    let start_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64);
    let mut child = Command::new(app)
        .arg(folder)
        .env(ENV_OUT, &out)
        .env(ENV_START_US, start_us.to_string())
        .env(ENV_SCROLL_ROWS, scroll_rows.to_string())
        .stdin(Stdio::null())
        .spawn()?;
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
    let text = std::fs::read_to_string(&out)?;
    let _ = std::fs::remove_file(&out);
    ScreenReport::parse(&text).map_err(io::Error::other)
}

/// 측정 계획.
pub struct Plan<'a> {
    pub app: &'a Path,
    pub folder: &'a Path,
    pub item_count: usize,
    /// 콜드 스타트를 잴 횟수.
    pub runs: usize,
    /// 스크롤을 잴 횟수.
    pub scroll_runs: usize,
    pub scroll_rows: usize,
    /// 지정하면 PowerShell로 알아낸 주사율 대신 이 값을 쓴다.
    pub refresh_hz: Option<f64>,
}

/// 앱을 `runs`번 띄워 콜드 스타트를, `scroll_runs`번 띄워 스크롤 프레임을 잰다.
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
        let r = run_app(plan.app, plan.folder, 0, RUN_TIMEOUT).map_err(|e| e.to_string())?;
        check_rows(&r)?;
        cold_starts.push(
            r.first_frame
                .ok_or("앱이 첫 화면 시각을 알리지 않았습니다")?,
        );
    }
    let mut frames = Vec::new();
    for n in 1..=plan.scroll_runs {
        eprintln!("스크롤 {n}/{}…", plan.scroll_runs);
        let r = run_app(plan.app, plan.folder, plan.scroll_rows, RUN_TIMEOUT)
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
        frames,
        scroll_runs: plan.scroll_runs,
        spec,
        debug_build: cfg!(debug_assertions),
    })
}
