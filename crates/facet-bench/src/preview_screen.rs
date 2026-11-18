//! #344: 미리보기 화면 수치 측정(`cargo xtask measure-preview`)의 판정·실행.
//!
//! 사진(4000×3000 JPEG)과 텍스트 파일이 섞인 폴더를 앱으로 열어 (1) 미리보기 영역을 끈 채·켠 채 커서를 프레임마다 한
//! 행씩 끝까지 옮기는 프레임 시간, (2) 사진·텍스트 하나를 고른 때부터 내용이 든 첫 프레임까지의 시간을 잰다. 기존
//! `cargo xtask measure`의 결과 형식은 바꾸지 않는다.

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use facet_core::measure::{
    ENV_GRID, ENV_INSTANT, ENV_MENU, ENV_OPEN, ENV_OUT, ENV_PREVIEW, ENV_RENAME, ENV_SCROLL_ROWS,
    ENV_SEARCH, ENV_START_US, ENV_WINDOWS, ScreenReport,
};

use crate::grid_screen::solid_jpeg;
use crate::screen::{
    FRAME_WORST_FACTOR, RUN_TIMEOUT, SystemSpec, frame_budget, json_string, median, run_to_report,
    worst,
};

/// #344: 이미지 표시 기준(150ms).
pub const IMAGE_SHOW_LIMIT: Duration = Duration::from_millis(150);
/// #344: 텍스트 표시 기준(50ms).
pub const TEXT_SHOW_LIMIT: Duration = Duration::from_millis(50);
/// #344: 켠 때 화살표 프레임 중앙값이 끈 때의 몇 배까지인가(1.1).
pub const ARROW_SLOWDOWN_LIMIT: f64 = 1.1;

/// #344: 미리보기 화면 측정 결과.
#[derive(Debug, Clone)]
pub struct PreviewScreenResult {
    pub arrow_off: Vec<Duration>,
    pub arrow_on: Vec<Duration>,
    pub image_shows: Vec<Duration>,
    pub text_shows: Vec<Duration>,
    pub refresh_hz: Option<f64>,
}

impl PreviewScreenResult {
    /// #344: 판정.
    pub fn verdict(&self) -> PreviewVerdict {
        let shown =
            |values: &[Duration], limit: Duration| !values.is_empty() && median(values) < limit;
        let arrow_pass = !self.arrow_off.is_empty()
            && !self.arrow_on.is_empty()
            && median(&self.arrow_on).as_secs_f64()
                <= median(&self.arrow_off).as_secs_f64() * ARROW_SLOWDOWN_LIMIT
            && worst(&self.arrow_on) <= frame_budget(self.refresh_hz).mul_f64(FRAME_WORST_FACTOR);
        PreviewVerdict {
            arrow_pass,
            image_pass: shown(&self.image_shows, IMAGE_SHOW_LIMIT),
            text_pass: shown(&self.text_shows, TEXT_SHOW_LIMIT),
        }
    }
}

/// #344: 판정 결과.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewVerdict {
    pub arrow_pass: bool,
    pub image_pass: bool,
    pub text_pass: bool,
}

impl PreviewVerdict {
    /// #344: 모두 통과인가.
    pub fn pass(&self) -> bool {
        self.arrow_pass && self.image_pass && self.text_pass
    }
}

/// 시험 폴더의 사진(4000×3000 JPEG)과 텍스트 파일 수(각각).
pub const PREVIEW_FILES_EACH: usize = 200;
/// 표시 시간을 잴 사진 이름.
pub const IMAGE_NAME: &str = "photo-00000.jpg";
/// 표시 시간을 잴 텍스트 파일 이름.
pub const TEXT_NAME: &str = "note-00000.txt";
const BASE_PHOTOS: usize = 4;

/// 시험 폴더 기본 자리: 임시 폴더 아래 `facet-test-folders/preview-N`.
pub fn default_folder(each: usize) -> PathBuf {
    std::env::temp_dir()
        .join("facet-test-folders")
        .join(format!("preview-{each}"))
}

fn photo_name(i: usize) -> String {
    format!("photo-{i:05}.jpg")
}

fn note_name(i: usize) -> String {
    format!("note-{i:05}.txt")
}

/// 시험 폴더를 만든다(빠진 것만 채움, 시험 항목이 아닌 것이 있으면 멈춤). 사진은 단색 JPEG 몇 장의 하드 링크라
/// 디스크를 거의 쓰지 않는다(링크를 못 만들면 복사).
pub fn make_folder(folder: &Path, each: usize) -> io::Result<()> {
    std::fs::create_dir_all(folder)?;
    let expected: std::collections::HashSet<String> = (0..each)
        .flat_map(|i| [photo_name(i), note_name(i)])
        .collect();
    for entry in std::fs::read_dir(folder)? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if !expected.contains(&name) {
            return Err(io::Error::other(format!(
                "시험 항목이 아닌 것이 있어 멈춥니다: {}",
                folder.join(name).display()
            )));
        }
    }
    let base_dir = folder.with_file_name(format!(
        "{}-bases",
        folder
            .file_name()
            .map_or_else(|| "preview".into(), |n| n.to_string_lossy())
    ));
    for i in 0..each {
        let path = folder.join(photo_name(i));
        if !path.exists() {
            let base = base_dir.join(format!("base-{}.jpg", i % BASE_PHOTOS));
            if !base.exists() {
                std::fs::create_dir_all(&base_dir)?;
                std::fs::write(&base, solid_jpeg(4000, 3000, (i % BASE_PHOTOS) as u32))?;
            }
            if std::fs::hard_link(&base, &path).is_err() {
                std::fs::copy(&base, &path)?;
            }
        }
        let note = folder.join(note_name(i));
        if !note.exists() {
            let text: String = (0..40)
                .map(|n| format!("메모 {i} 줄 {n}: 미리보기 측정용 글자 abc\n"))
                .collect();
            std::fs::write(note, text)?;
        }
    }
    Ok(())
}

fn start_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64)
}

/// 앱을 미리보기 측정 모드(`mode`: `arrow-off`·`arrow-on`·`show:<이름>`)로 한 번 띄운다.
pub fn run_app(app: &Path, folder: &Path, mode: &str) -> io::Result<ScreenReport> {
    let out =
        std::env::temp_dir().join(format!("facet-measure-preview-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&out);
    let mut command = Command::new(app);
    command
        .arg(folder)
        .env_remove(ENV_WINDOWS)
        .env(ENV_OUT, &out)
        .env(ENV_START_US, start_us().to_string())
        .env(ENV_SCROLL_ROWS, "0")
        .env(ENV_PREVIEW, mode)
        .env_remove(ENV_GRID)
        .env_remove(ENV_OPEN)
        .env_remove(ENV_RENAME)
        .env_remove(ENV_MENU)
        .env_remove(ENV_SEARCH)
        .env_remove(ENV_INSTANT)
        .stdin(Stdio::null());
    run_to_report(command, &out, RUN_TIMEOUT)
}

/// 측정 계획.
pub struct PreviewPlan<'a> {
    pub app: &'a Path,
    pub folder: &'a Path,
    pub each: usize,
    pub runs: usize,
    pub refresh_hz: Option<f64>,
}

/// 측정 결과와 기계 정보.
#[derive(Debug, Clone)]
pub struct PreviewRun {
    pub folder: PathBuf,
    pub result: PreviewScreenResult,
    pub spec: SystemSpec,
    pub debug_build: bool,
}

/// 폴더를 만들고 앱을 띄워 잰다: 화살표 끔·켬(각 한 번), 이미지·텍스트 표시(각 `runs`회).
pub fn measure(plan: &PreviewPlan<'_>) -> Result<PreviewRun, String> {
    eprintln!("시험 폴더 확인 중: {}…", plan.folder.display());
    make_folder(plan.folder, plan.each).map_err(|e| e.to_string())?;
    let rows = 2 * plan.each;
    let arrow = |mode: &str| -> Result<Vec<Duration>, String> {
        eprintln!("화살표 이동({mode})…");
        let r = run_app(plan.app, plan.folder, mode).map_err(|e| e.to_string())?;
        if r.rows != rows {
            return Err(format!(
                "항목 수가 다릅니다: {rows}개를 기대했지만 {}개",
                r.rows
            ));
        }
        Ok(r.frames)
    };
    let arrow_off = arrow("arrow-off")?;
    let arrow_on = arrow("arrow-on")?;
    let shows = |name: &str, what: &str| -> Result<Vec<Duration>, String> {
        let mut out = Vec::new();
        for n in 1..=plan.runs {
            eprintln!("{what} 표시 {n}/{}…", plan.runs);
            let r = run_app(plan.app, plan.folder, &format!("show:{name}"))
                .map_err(|e| e.to_string())?;
            out.push(
                r.open_frame
                    .ok_or_else(|| format!("앱이 {what} 표시 시각을 알리지 않았습니다"))?,
            );
        }
        Ok(out)
    };
    let image_shows = shows(IMAGE_NAME, "이미지")?;
    let text_shows = shows(TEXT_NAME, "텍스트")?;
    let mut spec = SystemSpec::collect(plan.folder);
    if plan.refresh_hz.is_some() {
        spec.refresh_hz = plan.refresh_hz;
    }
    Ok(PreviewRun {
        folder: plan.folder.to_path_buf(),
        result: PreviewScreenResult {
            arrow_off,
            arrow_on,
            image_shows,
            text_shows,
            refresh_hz: spec.refresh_hz,
        },
        spec,
        debug_build: cfg!(debug_assertions),
    })
}

fn hz(refresh_hz: Option<f64>) -> f64 {
    1.0 / frame_budget(refresh_hz).as_secs_f64()
}

impl PreviewRun {
    /// 사람이 읽는 표.
    pub fn to_table(&self) -> String {
        let r = &self.result;
        let v = r.verdict();
        let ms = |d: Duration| format!("{:.1}ms", d.as_secs_f64() * 1000.0);
        let mark = |ok: bool| if ok { "통과" } else { "실패" };
        let unknown = || "알 수 없음".to_owned();
        let mut t = String::new();
        if self.debug_build {
            t.push_str("경고: 디버그 빌드입니다. 수치는 릴리스 빌드에서 재야 의미가 있습니다.\n");
        }
        let _ = writeln!(t, "측정: {} (미리보기 영역)", self.folder.display());
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
            hz(r.refresh_hz),
            if r.refresh_hz.is_none() {
                " (읽지 못해 가정)"
            } else {
                ""
            }
        );
        for (name, frames) in [("끔", &r.arrow_off), ("켬", &r.arrow_on)] {
            let _ = writeln!(
                t,
                "\n화살표 이동, 영역 {name}({}프레임): 중앙값 {} / 최악 {}",
                frames.len(),
                ms(median(frames)),
                ms(worst(frames))
            );
        }
        let limit = frame_budget(r.refresh_hz).mul_f64(FRAME_WORST_FACTOR);
        let _ = writeln!(
            t,
            "  켠 때 중앙값이 끈 때의 {ARROW_SLOWDOWN_LIMIT}배 이하, 최악값이 {} 이하  {}",
            ms(limit),
            mark(v.arrow_pass)
        );
        for (what, shows, limit, ok) in [
            (
                "이미지 표시",
                &r.image_shows,
                IMAGE_SHOW_LIMIT,
                v.image_pass,
            ),
            ("텍스트 표시", &r.text_shows, TEXT_SHOW_LIMIT, v.text_pass),
        ] {
            let _ = writeln!(
                t,
                "\n{what}({}회): 중앙값 {} / 최악 {}  (기준 {} 미만)  {}",
                shows.len(),
                ms(median(shows)),
                ms(worst(shows)),
                ms(limit),
                mark(ok)
            );
        }
        let _ = writeln!(t, "\n종합: {}", mark(v.pass()));
        t
    }

    /// JSON.
    pub fn to_json(&self) -> String {
        let r = &self.result;
        let v = r.verdict();
        let ms = |d: Duration| format!("{:.3}", d.as_secs_f64() * 1000.0);
        let mut j = String::from("{\n");
        let _ = writeln!(
            j,
            "  \"folder\": {},",
            json_string(&self.folder.display().to_string())
        );
        let _ = writeln!(j, "  \"debug_build\": {},", self.debug_build);
        let _ = writeln!(j, "  \"refresh_hz\": {},", hz(r.refresh_hz));
        let _ = writeln!(j, "  \"refresh_assumed\": {},", r.refresh_hz.is_none());
        let _ = writeln!(j, "  \"arrow\": {{");
        let _ = writeln!(j, "    \"off_frames\": {},", r.arrow_off.len());
        let _ = writeln!(j, "    \"off_median_ms\": {},", ms(median(&r.arrow_off)));
        let _ = writeln!(j, "    \"off_worst_ms\": {},", ms(worst(&r.arrow_off)));
        let _ = writeln!(j, "    \"on_frames\": {},", r.arrow_on.len());
        let _ = writeln!(j, "    \"on_median_ms\": {},", ms(median(&r.arrow_on)));
        let _ = writeln!(j, "    \"on_worst_ms\": {},", ms(worst(&r.arrow_on)));
        let _ = writeln!(j, "    \"slowdown_limit\": {ARROW_SLOWDOWN_LIMIT},");
        let _ = writeln!(
            j,
            "    \"worst_limit_ms\": {},",
            ms(frame_budget(r.refresh_hz).mul_f64(FRAME_WORST_FACTOR))
        );
        let _ = writeln!(j, "    \"pass\": {}", v.arrow_pass);
        let _ = writeln!(j, "  }},");
        for (key, shows, limit, ok) in [
            ("image_show", &r.image_shows, IMAGE_SHOW_LIMIT, v.image_pass),
            ("text_show", &r.text_shows, TEXT_SHOW_LIMIT, v.text_pass),
        ] {
            let _ = writeln!(j, "  \"{key}\": {{");
            let _ = writeln!(j, "    \"runs\": {},", shows.len());
            let _ = writeln!(j, "    \"median_ms\": {},", ms(median(shows)));
            let _ = writeln!(j, "    \"worst_ms\": {},", ms(worst(shows)));
            let _ = writeln!(j, "    \"limit_ms\": {},", limit.as_millis());
            let _ = writeln!(j, "    \"pass\": {ok}");
            let _ = writeln!(j, "  }},");
        }
        let _ = writeln!(j, "  \"pass\": {}", v.pass());
        j.push_str("}\n");
        j
    }
}
