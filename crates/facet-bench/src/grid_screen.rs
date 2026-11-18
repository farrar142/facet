//! #331: 그리드 화면 수치 측정(`cargo xtask measure-grid`)의 상수·판정·실행.
//!
//! 사진 [`GRID_IMAGE_COUNT`]장 폴더(4000×3000 JPEG)를 그리드(보통 크기)로 열어 (1) 끝까지 자동 스크롤하는 프레임 사이
//! 시간(최악값이 한 프레임 시간의 두 배 이하)과 (2) 캐시가 찬 두 번째 열기에서 한 화면이 썸네일로 다 차기까지의
//! 시간(중앙값 [`GRID_FILL_LIMIT`] 미만)을 잰다. 기존 `cargo xtask measure`의 결과 형식은 바꾸지 않는다.

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use facet_core::measure::{
    ENV_GRID, ENV_INSTANT, ENV_MENU, ENV_OPEN, ENV_OUT, ENV_RENAME, ENV_SCROLL_ROWS, ENV_SEARCH,
    ENV_START_US, ENV_WINDOWS, ScreenReport,
};

use crate::screen::{
    ASSUMED_REFRESH_HZ, RUN_TIMEOUT, SystemSpec, json_string, median, run_to_report, worst,
};

/// #331: 측정에 쓰는 사진 수.
pub const GRID_IMAGE_COUNT: usize = 10_000;
/// #331: 캐시가 찬 두 번째 열기에서 한 화면 채우기 상한.
pub const GRID_FILL_LIMIT: Duration = Duration::from_millis(100);
/// 사진 한 장의 크기.
pub const PHOTO_WIDTH: u32 = 4000;
/// 사진 한 장의 크기.
pub const PHOTO_HEIGHT: u32 = 3000;
/// 서로 다른 무늬(색)의 원본 사진 수. 나머지 사진은 이 파일들의 하드 링크라 디스크를 거의 쓰지 않는다.
const BASE_PHOTOS: usize = 16;
/// 스크롤 한 걸음에 내리는 칸 줄 수.
pub const GRID_SCROLL_LINES: usize = 2;

/// #331: 그리드 측정의 판정.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridVerdict {
    pub fill_median: Duration,
    pub fill_pass: bool,
    pub frame_worst: Duration,
    pub frame_pass: bool,
}

impl GridVerdict {
    /// #331: 두 수치가 모두 통과했는가.
    pub fn pass(&self) -> bool {
        self.fill_pass && self.frame_pass
    }
}

/// #331: 채우기 시간들과 프레임 시간들을 주사율로 판정한다. 채우기 중앙값이 상한 미만이고 프레임 최악값이 한 프레임
/// 시간(1000/주사율)의 두 배 이하일 때만 통과하고, 잰 값이 없으면 실패한다.
pub fn judge(fills: &[Duration], frames: &[Duration], refresh_hz: f64) -> GridVerdict {
    let fill_median = median(fills);
    let frame_worst = worst(frames);
    let limit = Duration::from_secs_f64(2.0 / refresh_hz);
    GridVerdict {
        fill_median,
        fill_pass: !fills.is_empty() && fill_median < GRID_FILL_LIMIT,
        frame_worst,
        frame_pass: !frames.is_empty() && frame_worst <= limit,
    }
}

/// 사진 폴더 기본 자리: 임시 폴더 아래 `facet-test-folders/photos-N`.
pub fn default_photo_folder(count: usize) -> PathBuf {
    std::env::temp_dir()
        .join("facet-test-folders")
        .join(format!("photos-{count}"))
}

fn photo_name(index: usize) -> String {
    format!("photo-{index:05}.jpg")
}

/// 한 가지 색으로 채운 기준선 JPEG(허프만 표를 짧은 코드로 잡아 덩어리마다 2비트만 쓴다). `seed`마다 색이 다르다.
pub fn solid_jpeg(width: u32, height: u32, seed: u32) -> Vec<u8> {
    fn segment(out: &mut Vec<u8>, marker: u8, data: &[u8]) {
        out.extend_from_slice(&[0xFF, marker]);
        out.extend_from_slice(&((data.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(data);
    }
    let mut out = vec![0xFF, 0xD8];
    let mut dqt = vec![0u8];
    dqt.extend_from_slice(&[1; 64]);
    segment(&mut out, 0xDB, &dqt);
    let mut sof = vec![8];
    sof.extend_from_slice(&(height as u16).to_be_bytes());
    sof.extend_from_slice(&(width as u16).to_be_bytes());
    sof.push(3);
    for id in 1..=3u8 {
        sof.extend_from_slice(&[id, 0x11, 0]);
    }
    segment(&mut out, 0xC0, &sof);
    // DC 표: 크기 0은 1비트 코드, 크기 1..=11은 5비트 코드(16 + 크기 - 1).
    let mut dht = vec![0x00];
    let mut counts = [0u8; 16];
    counts[0] = 1;
    counts[4] = 11;
    dht.extend_from_slice(&counts);
    dht.extend(0..12u8);
    // AC 표: 덩어리 끝(0x00) 하나가 1비트 코드.
    dht.push(0x10);
    let mut counts = [0u8; 16];
    counts[0] = 1;
    dht.extend_from_slice(&counts);
    dht.push(0x00);
    segment(&mut out, 0xC4, &dht);
    segment(&mut out, 0xDA, &[3, 1, 0x00, 2, 0x00, 3, 0x00, 0, 63, 0]);

    let (mut acc, mut used) = (0u32, 0u32);
    let mut put = |out: &mut Vec<u8>, value: u32, bits: u32| {
        for i in (0..bits).rev() {
            acc = (acc << 1) | ((value >> i) & 1);
            used += 1;
            if used == 8 {
                out.push(acc as u8);
                if acc as u8 == 0xFF {
                    out.push(0);
                }
                acc = 0;
                used = 0;
            }
        }
        used
    };
    let blocks = (width / 8) as usize * (height / 8) as usize;
    for block in 0..blocks {
        for c in 0..3u32 {
            let diff = if block == 0 {
                ((seed.wrapping_mul(7 + c) % 25) as i32 - 12) * 40
            } else {
                0
            };
            if diff == 0 {
                put(&mut out, 0, 1);
            } else {
                let size = 32 - diff.unsigned_abs().leading_zeros();
                put(&mut out, 16 + size - 1, 5);
                let mag = if diff >= 0 {
                    diff as u32
                } else {
                    ((diff - 1) as u32) & ((1 << size) - 1)
                };
                put(&mut out, mag, size);
            }
            put(&mut out, 0, 1);
        }
    }
    let left = put(&mut out, 0, 0);
    if left > 0 {
        let pad = 8 - left;
        put(&mut out, (1 << pad) - 1, pad);
    }
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}

/// `folder`에 사진 `count`장을 채운다(이미 있는 것은 건너뛴다, 다시 돌려도 같다). 폴더에 사진 이름이 아닌 항목이
/// 있으면 아무것도 하지 않고 멈춘다. 서로 다른 원본 [`BASE_PHOTOS`]장을 만들고 나머지는 그 하드 링크로 둔다
/// (링크를 못 만들면 복사). 새로 만든 사진 수를 돌려준다.
pub fn make_photo_folder(folder: &Path, count: usize) -> io::Result<usize> {
    std::fs::create_dir_all(folder)?;
    let expected: std::collections::HashSet<String> = (0..count).map(photo_name).collect();
    for entry in std::fs::read_dir(folder)? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if !expected.contains(&name) {
            return Err(io::Error::other(format!(
                "사진이 아닌 항목이 있어 멈춥니다: {}",
                folder.join(name).display()
            )));
        }
    }
    // 원본은 사진 폴더 밖(옆 폴더)에 둔다: 사진 폴더에는 사진만 있어야 앱이 읽은 항목 수가 맞는다.
    let base_dir = folder.with_file_name(format!(
        "{}-bases",
        folder
            .file_name()
            .map_or_else(|| "photos".into(), |n| n.to_string_lossy())
    ));
    let mut made = 0;
    for index in 0..count {
        let path = folder.join(photo_name(index));
        if path.exists() {
            continue;
        }
        let base = base_dir.join(format!("base-{}.jpg", index % BASE_PHOTOS));
        if !base.exists() {
            std::fs::create_dir_all(&base_dir)?;
            std::fs::write(
                &base,
                solid_jpeg(PHOTO_WIDTH, PHOTO_HEIGHT, (index % BASE_PHOTOS) as u32),
            )?;
        }
        if std::fs::hard_link(&base, &path).is_err() {
            std::fs::copy(&base, &path)?;
        }
        made += 1;
    }
    Ok(made)
}

fn start_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64)
}

/// 앱을 그리드 측정 모드로 한 번 띄운다. `fill`이면 한 화면 채우기, 아니면 끝까지 스크롤하는 프레임 시간을 잰다.
pub fn run_app_grid(
    app: &Path,
    folder: &Path,
    fill: bool,
    scroll_lines: usize,
) -> io::Result<ScreenReport> {
    let out = std::env::temp_dir().join(format!("facet-measure-grid-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&out);
    let mut command = Command::new(app);
    command
        .arg(folder)
        .env_remove(ENV_WINDOWS)
        .env(ENV_OUT, &out)
        .env(ENV_START_US, start_us().to_string())
        .env(ENV_SCROLL_ROWS, scroll_lines.to_string())
        .env(ENV_GRID, if fill { "fill" } else { "scroll" })
        .env_remove(ENV_OPEN)
        .env_remove(ENV_RENAME)
        .env_remove(ENV_MENU)
        .env_remove(ENV_SEARCH)
        .env_remove(ENV_INSTANT)
        .stdin(Stdio::null());
    run_to_report(command, &out, RUN_TIMEOUT)
}

/// 측정 계획.
pub struct GridPlan<'a> {
    pub app: &'a Path,
    pub folder: &'a Path,
    pub count: usize,
    pub runs: usize,
    pub refresh_hz: Option<f64>,
}

/// 측정 결과.
#[derive(Debug, Clone)]
pub struct GridResult {
    pub folder: PathBuf,
    pub count: usize,
    pub fills: Vec<Duration>,
    pub frames: Vec<Duration>,
    pub spec: SystemSpec,
    pub debug_build: bool,
}

impl GridResult {
    fn refresh_hz(&self) -> f64 {
        self.spec.refresh_hz.unwrap_or(ASSUMED_REFRESH_HZ)
    }

    /// 판정.
    pub fn verdict(&self) -> GridVerdict {
        judge(&self.fills, &self.frames, self.refresh_hz())
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
            "측정: {} (사진 {}장, 그리드 보통 크기)",
            self.folder.display(),
            self.count
        );
        let unknown = || "알 수 없음".to_owned();
        let _ = writeln!(
            t,
            "  CPU     {}",
            self.spec.cpu.clone().unwrap_or_else(unknown)
        );
        let _ = writeln!(
            t,
            "  디스크  {}",
            self.spec.disk.clone().unwrap_or_else(unknown)
        );
        let _ = writeln!(
            t,
            "  주사율  {:.0}Hz{}",
            self.refresh_hz(),
            if self.spec.refresh_hz.is_none() {
                " (읽지 못해 가정)"
            } else {
                ""
            }
        );
        let _ = writeln!(
            t,
            "\n한 화면 채우기(캐시 찬 두 번째 열기, {}회): 중앙값 {} / 최악 {}  (기준 {} 미만)  {}",
            self.fills.len(),
            ms(v.fill_median),
            ms(worst(&self.fills)),
            ms(GRID_FILL_LIMIT),
            mark(v.fill_pass)
        );
        let limit = Duration::from_secs_f64(2.0 / self.refresh_hz());
        let _ = writeln!(
            t,
            "스크롤 프레임({}개): 중앙값 {} / 최악 {}  (기준 {} 이하, 한 프레임의 두 배)  {}",
            self.frames.len(),
            ms(median(&self.frames)),
            ms(v.frame_worst),
            ms(limit),
            mark(v.frame_pass)
        );
        let _ = writeln!(t, "\n종합: {}", mark(v.pass()));
        t
    }

    /// JSON.
    pub fn to_json(&self) -> String {
        let v = self.verdict();
        let ms = |d: Duration| format!("{:.3}", d.as_secs_f64() * 1000.0);
        let mut j = String::from("{\n");
        let _ = writeln!(
            j,
            "  \"folder\": {},",
            json_string(&self.folder.display().to_string())
        );
        let _ = writeln!(j, "  \"image_count\": {},", self.count);
        let _ = writeln!(j, "  \"debug_build\": {},", self.debug_build);
        let _ = writeln!(j, "  \"refresh_hz\": {},", self.refresh_hz());
        let _ = writeln!(
            j,
            "  \"refresh_assumed\": {},",
            self.spec.refresh_hz.is_none()
        );
        let _ = writeln!(j, "  \"grid_fill\": {{");
        let _ = writeln!(j, "    \"runs\": {},", self.fills.len());
        let _ = writeln!(j, "    \"median_ms\": {},", ms(v.fill_median));
        let _ = writeln!(j, "    \"worst_ms\": {},", ms(worst(&self.fills)));
        let _ = writeln!(j, "    \"limit_ms\": {},", GRID_FILL_LIMIT.as_millis());
        let _ = writeln!(j, "    \"pass\": {}", v.fill_pass);
        let _ = writeln!(j, "  }},");
        let _ = writeln!(j, "  \"scroll\": {{");
        let _ = writeln!(j, "    \"frame_count\": {},", self.frames.len());
        let _ = writeln!(j, "    \"frame_median_ms\": {},", ms(median(&self.frames)));
        let _ = writeln!(j, "    \"frame_worst_ms\": {},", ms(v.frame_worst));
        let _ = writeln!(
            j,
            "    \"frame_worst_limit_ms\": {},",
            ms(Duration::from_secs_f64(2.0 / self.refresh_hz()))
        );
        let _ = writeln!(j, "    \"pass\": {}", v.frame_pass);
        let _ = writeln!(j, "  }},");
        let _ = writeln!(j, "  \"pass\": {}", v.pass());
        j.push_str("}\n");
        j
    }
}

/// 사진 폴더를 채우고 앱을 띄워 잰다: 채우기 한 번(캐시를 채우는 데우기, 재지 않음) → 채우기 `runs`회 → 스크롤 한 번.
pub fn measure(plan: &GridPlan<'_>) -> Result<GridResult, String> {
    eprintln!("사진 폴더 확인 중: {}…", plan.folder.display());
    make_photo_folder(plan.folder, plan.count).map_err(|e| e.to_string())?;
    let check = |r: &ScreenReport| {
        if r.rows == plan.count {
            Ok(())
        } else {
            Err(format!(
                "항목 수가 다릅니다: {}개를 기대했지만 앱이 {}개를 읽었습니다",
                plan.count, r.rows
            ))
        }
    };
    eprintln!("썸네일 캐시 채우는 중(재지 않음)…");
    run_app_grid(plan.app, plan.folder, true, 0).map_err(|e| e.to_string())?;
    let mut fills = Vec::new();
    for n in 1..=plan.runs {
        eprintln!("한 화면 채우기 {n}/{}…", plan.runs);
        let r = run_app_grid(plan.app, plan.folder, true, 0).map_err(|e| e.to_string())?;
        check(&r)?;
        fills.push(
            r.grid_fill
                .ok_or("앱이 한 화면 채우기 시각을 알리지 않았습니다")?,
        );
    }
    eprintln!("스크롤…");
    let r =
        run_app_grid(plan.app, plan.folder, false, GRID_SCROLL_LINES).map_err(|e| e.to_string())?;
    check(&r)?;
    let mut spec = SystemSpec::collect(plan.folder);
    if plan.refresh_hz.is_some() {
        spec.refresh_hz = plan.refresh_hz;
    }
    Ok(GridResult {
        folder: plan.folder.to_path_buf(),
        count: plan.count,
        fills,
        frames: r.frames,
        spec,
        debug_build: cfg!(debug_assertions),
    })
}
