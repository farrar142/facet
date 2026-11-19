//! `cargo xtask ci`가 무엇을 돌릴지 정하는 순수 판단: 인자 해석, 바뀐 파일로 고르는 벤치마크, 공장 스크립트
//! 시험을 돌릴지.
//!
//! 바뀐 파일은 부르는 쪽이 git으로 모아 넘긴다(기준과의 merge-base 대비, 작업 공간 루트 기준 `/` 구분 경로).

/// 벤치마크 이름. `cargo xtask bench`·`ci`가 이 차례로 돌린다(`facet-bench`의 하위 명령 이름과 같다).
pub const BENCHES: [&str; 20] = [
    "list",
    "index",
    "mft",
    "session",
    "clipboard",
    "menu",
    "batch-rename",
    "filter",
    "query-syntax",
    "search",
    "content",
    "copy",
    "delete",
    "batch-rename-run",
    "thumbnail-decode",
    "thumbnail-cache",
    "text-preview",
    "content-gate",
    "shell-helper",
    "shell-list",
];

/// 바뀌면 벤치마크를 모두 돌리는 공통 파일. `/`로 끝나면 그 폴더 아래 전부다.
const ALL_BENCHES: &[&str] = &[
    "Cargo.lock",
    "Cargo.toml",
    "rust-toolchain.toml",
    ".cargo/",
    "xtask/",
    "crates/facet-bench/",
    // 벤치마크가 기대는 크레이트의 공통 파일.
    "crates/facet-core/Cargo.toml",
    "crates/facet-core/src/lib.rs",
    "crates/facet-win/Cargo.toml",
    "crates/facet-win/build.rs",
    "crates/facet-win/src/lib.rs",
    "crates/facet-win/src/windows_impl.rs",
    "crates/facet-index/Cargo.toml",
    "crates/facet-indexer/Cargo.toml",
    "crates/facet-protocol/Cargo.toml",
];

/// 바뀌어도 벤치마크에 닿지 않는 곳. `/`로 끝나면 그 폴더 아래 전부다.
const NO_BENCH: &[&str] = &[
    // 벤치마크가 기대지 않는 크레이트.
    "crates/facet-app/",
    "crates/facet-query/",
];

/// 경로 → 그 경로가 바뀌면 돌릴 벤치마크. 빈 목록은 「벤치마크에 닿지 않음」이다. 벤치마크가 기대는 크레이트
/// (`facet-core`·`facet-win`·`facet-index`·`facet-indexer`·`facet-protocol`)의 소스가 이 표에 없으면 모두 돌린다.
/// 새 모듈을 더하면 여기에도 더한다(이 파일이 바뀌므로 그 PR에서는 어차피 모두 돈다).
const TABLE: &[(&str, &[&str])] = &[
    // facet-core: 목록 읽기·탭·창.
    (
        "crates/facet-core/src/details.rs",
        &["list", "menu", "batch-rename", "filter", "shell-list"],
    ),
    (
        "crates/facet-core/src/file_system.rs",
        &["list", "menu", "batch-rename", "filter"],
    ),
    (
        "crates/facet-core/src/provider.rs",
        &["list", "menu", "batch-rename", "filter", "shell-list"],
    ),
    (
        "crates/facet-core/src/location.rs",
        &[
            "list",
            "session",
            "menu",
            "batch-rename",
            "filter",
            "shell-list",
        ],
    ),
    (
        "crates/facet-core/src/tab.rs",
        &["menu", "batch-rename", "filter", "shell-list"],
    ),
    // facet-core: 셸 공급자(가짜 셸 폴더로 목록 전달을 잰다).
    ("crates/facet-core/src/shell_provider.rs", &["shell-list"]),
    (
        "crates/facet-core/src/selection.rs",
        &["menu", "batch-rename", "filter"],
    ),
    (
        "crates/facet-core/src/window.rs",
        &["menu", "batch-rename", "filter"],
    ),
    (
        "crates/facet-core/src/watch.rs",
        &["menu", "batch-rename", "filter"],
    ),
    ("crates/facet-core/src/command.rs", &["menu"]),
    ("crates/facet-core/src/menu.rs", &["menu"]),
    ("crates/facet-core/src/batch_rename.rs", &["batch-rename"]),
    ("crates/facet-core/src/name_input.rs", &["batch-rename"]),
    (
        "crates/facet-core/src/name_match.rs",
        &["filter", "search", "content"],
    ),
    ("crates/facet-core/src/session.rs", &["session"]),
    ("crates/facet-core/src/placement.rs", &["session"]),
    ("crates/facet-core/src/clipboard.rs", &["clipboard"]),
    ("crates/facet-core/src/thumbnail.rs", &["thumbnail-decode"]),
    (
        "crates/facet-core/src/thumbnail_cache.rs",
        &["thumbnail-cache"],
    ),
    ("crates/facet-core/src/preview.rs", &["text-preview"]),
    ("crates/facet-core/src/content_gate.rs", &["content-gate"]),
    ("crates/facet-core/src/thumbnail_queue.rs", &[]),
    ("crates/facet-core/src/query_syntax.rs", &["query-syntax"]),
    ("crates/facet-core/src/live_search.rs", &["search"]),
    ("crates/facet-core/src/condition_match.rs", &["search"]),
    ("crates/facet-core/src/content_search.rs", &["content"]),
    // facet-core: 작업 대기열. 영구 삭제는 복사의 훑기(`copy::pipeline`)를 쓴다.
    (
        "crates/facet-core/src/job.rs",
        &["copy", "delete", "batch-rename-run"],
    ),
    ("crates/facet-core/src/job/copy.rs", &["copy", "delete"]),
    ("crates/facet-core/src/job/delete.rs", &["delete"]),
    (
        "crates/facet-core/src/job/batch_rename.rs",
        &["batch-rename-run"],
    ),
    ("crates/facet-core/src/history.rs", &["delete"]),
    ("crates/facet-core/src/job/moving.rs", &[]),
    ("crates/facet-core/src/job/recycle.rs", &[]),
    ("crates/facet-core/src/job/restore.rs", &[]),
    ("crates/facet-core/src/job/device.rs", &[]),
    ("crates/facet-core/src/job/download.rs", &[]),
    ("crates/facet-core/src/job/revert.rs", &[]),
    // facet-core: 벤치마크가 재지 않는 것.
    ("crates/facet-core/src/dragdrop.rs", &[]),
    ("crates/facet-core/src/edit.rs", &[]),
    ("crates/facet-core/src/file_list.rs", &[]),
    ("crates/facet-core/src/icons.rs", &[]),
    ("crates/facet-core/src/indexer_client.rs", &[]),
    ("crates/facet-core/src/instant_search.rs", &[]),
    ("crates/facet-core/src/launch.rs", &[]),
    ("crates/facet-core/src/measure.rs", &[]),
    ("crates/facet-core/src/rename_presets.rs", &[]),
    ("crates/facet-core/src/search_results.rs", &[]),
    ("crates/facet-core/src/shell_helper.rs", &["shell-helper"]),
    ("crates/facet-core/src/work_area.rs", &[]),
    ("crates/facet-core/src/zip_archive.rs", &[]),
    // facet-win.
    (
        "crates/facet-win/src/file_ops.rs",
        &["copy", "delete", "batch-rename-run"],
    ),
    (
        "crates/facet-win/src/windows_file_ops.rs",
        &["copy", "delete", "batch-rename-run"],
    ),
    ("crates/facet-win/src/clipboard/format.rs", &["clipboard"]),
    ("crates/facet-win/src/clipboard.rs", &[]),
    ("crates/facet-win/src/windows_clipboard.rs", &[]),
    ("crates/facet-win/src/cloud_files.rs", &[]),
    ("crates/facet-win/src/windows_cloud_files.rs", &[]),
    ("crates/facet-win/src/drag_source.rs", &[]),
    ("crates/facet-win/src/windows_drag_source.rs", &[]),
    ("crates/facet-win/src/drop_target.rs", &[]),
    ("crates/facet-win/src/windows_drop_target.rs", &[]),
    ("crates/facet-win/src/devices.rs", &[]),
    ("crates/facet-win/src/windows_devices.rs", &[]),
    ("crates/facet-win/src/icon.rs", &[]),
    ("crates/facet-win/src/windows_icon.rs", &[]),
    ("crates/facet-win/src/pipe_client.rs", &[]),
    ("crates/facet-win/src/windows_pipe_client.rs", &[]),
    ("crates/facet-win/src/recycle.rs", &[]),
    ("crates/facet-win/src/windows_recycle.rs", &[]),
    ("crates/facet-win/src/shell_copy.rs", &[]),
    ("crates/facet-win/src/windows_shell_copy.rs", &[]),
    ("crates/facet-win/src/shell_dialog.rs", &[]),
    ("crates/facet-win/src/shell_thread.rs", &[]),
    ("crates/facet-win/src/shell_folder.rs", &[]),
    ("crates/facet-win/src/shell_menu.rs", &[]),
    ("crates/facet-win/src/windows_shell_menu.rs", &[]),
    ("crates/facet-win/src/windows_shell_folder.rs", &[]),
    ("crates/facet-win/src/windows_shell_thread.rs", &[]),
    ("crates/facet-win/src/shell_transfer.rs", &[]),
    ("crates/facet-win/src/windows_shell_transfer.rs", &[]),
    ("crates/facet-win/src/single_instance.rs", &[]),
    ("crates/facet-win/src/windows_instance.rs", &[]),
    (
        "crates/facet-win/src/watch.rs",
        &["menu", "batch-rename", "filter"],
    ),
    (
        "crates/facet-win/src/windows_watch.rs",
        &["menu", "batch-rename", "filter"],
    ),
    ("crates/facet-win/src/windows_screen.rs", &[]),
    ("crates/facet-win/src/preview.rs", &[]),
    ("crates/facet-win/src/preview_handler.rs", &[]),
    ("crates/facet-win/src/windows_preview_handler.rs", &[]),
    // facet-index: 인덱스 질의·저장, MFT 파싱.
    ("crates/facet-index/src/lib.rs", &["index", "mft"]),
    ("crates/facet-index/src/store.rs", &["index"]),
    ("crates/facet-index/src/mft.rs", &["mft"]),
    ("crates/facet-index/src/usn.rs", &[]),
    ("testdata/mft/", &["mft"]),
    // facet-indexer: 권한 거르기를 거친 질의(`Volumes::search_as`).
    ("crates/facet-indexer/src/lib.rs", &["index"]),
    ("crates/facet-indexer/src/volumes.rs", &["index"]),
    ("crates/facet-indexer/src/access.rs", &["index"]),
    ("crates/facet-indexer/src/build.rs", &[]),
    ("crates/facet-indexer/src/caller.rs", &[]),
    ("crates/facet-indexer/src/cli.rs", &[]),
    ("crates/facet-indexer/src/dump.rs", &[]),
    ("crates/facet-indexer/src/follow.rs", &[]),
    ("crates/facet-indexer/src/index_dir.rs", &[]),
    ("crates/facet-indexer/src/layout.rs", &[]),
    ("crates/facet-indexer/src/main.rs", &[]),
    ("crates/facet-indexer/src/mounts.rs", &[]),
    ("crates/facet-indexer/src/pipe.rs", &[]),
    ("crates/facet-indexer/src/sample.rs", &[]),
    ("crates/facet-indexer/src/service.rs", &[]),
    ("crates/facet-indexer/src/session.rs", &["index"]),
    ("crates/facet-indexer/src/volume.rs", &[]),
    // facet-protocol: 조건 트리와 인덱스 질의 변환.
    (
        "crates/facet-protocol/src/lib.rs",
        &["index", "query-syntax", "search"],
    ),
    (
        "crates/facet-protocol/src/message.rs",
        &["index", "query-syntax", "search"],
    ),
    ("crates/facet-protocol/src/convert.rs", &["index"]),
    ("crates/facet-protocol/src/frame.rs", &[]),
    ("crates/facet-protocol/src/wire.rs", &[]),
];

/// 공장 스크립트 시험(`bash .github/factory/test.sh`)이 보는 곳.
const FACTORY: &[&str] = &[".github/factory/", ".github/workflows/factory-"];

/// 고른 벤치마크와 그 까닭.
#[derive(Debug, PartialEq, Eq)]
pub struct Selection {
    /// 고른 벤치마크, `BENCHES` 차례.
    pub benches: Vec<&'static str>,
    /// 바뀐 파일마다 `경로 → 벤치마크`(벤치마크에 닿는 파일만).
    pub reasons: Vec<String>,
}

/// `path`가 `pattern`(`/`로 끝나면 폴더)에 드는가.
fn matches(path: &str, pattern: &str) -> bool {
    if pattern.ends_with('/') || pattern.ends_with('-') {
        path.starts_with(pattern)
    } else {
        path == pattern
    }
}

/// 벤치마크가 기대는 크레이트의 소스인가(표에 없으면 모두 돌린다).
fn in_bench_crate(path: &str) -> bool {
    [
        "crates/facet-core/",
        "crates/facet-win/",
        "crates/facet-index/",
        "crates/facet-indexer/",
        "crates/facet-protocol/",
    ]
    .iter()
    .any(|c| path.starts_with(c))
}

/// 한 경로가 부르는 벤치마크. `None`은 「모두」다.
fn benches_for(path: &str) -> Option<&'static [&'static str]> {
    if ALL_BENCHES.iter().any(|p| matches(path, p)) {
        return None;
    }
    // 문서와 시험 파일은 릴리스 빌드의 벤치마크에 닿지 않는다.
    if path.ends_with(".md")
        || path.contains("/tests/")
        || NO_BENCH.iter().any(|p| matches(path, p))
    {
        return Some(&[]);
    }
    if let Some((_, benches)) = TABLE.iter().find(|(p, _)| matches(path, p)) {
        return Some(benches);
    }
    if in_bench_crate(path) {
        None
    } else {
        Some(&[])
    }
}

/// 바뀐 파일(작업 공간 루트 기준 경로)로 돌릴 벤치마크를 고른다. 공통 파일(`Cargo.lock`·`xtask/`·
/// `crates/facet-bench/` 등)이나 표에 없는 벤치마크 크레이트 소스가 하나라도 있으면 모두, 바뀐 Rust 코드가
/// 벤치마크에 닿지 않으면 하나도 고르지 않는다.
pub fn select_benches(changed: &[String]) -> Selection {
    let mut chosen = [false; BENCHES.len()];
    let mut reasons = Vec::new();
    for raw in changed {
        let path = raw.replace('\\', "/");
        match benches_for(&path) {
            None => {
                chosen = [true; BENCHES.len()];
                reasons.push(format!("{path} → 모두"));
            }
            Some([]) => {}
            Some(benches) => {
                for b in benches {
                    if let Some(i) = BENCHES.iter().position(|n| n == b) {
                        chosen[i] = true;
                    }
                }
                reasons.push(format!("{path} → {}", benches.join("·")));
            }
        }
    }
    Selection {
        benches: BENCHES
            .iter()
            .zip(chosen)
            .filter_map(|(b, c)| c.then_some(*b))
            .collect(),
        reasons,
    }
}

/// 바뀐 파일에 공장 스크립트나 공장 workflow가 있는가.
pub fn touches_factory(changed: &[String]) -> bool {
    changed.iter().any(|raw| {
        let path = raw.replace('\\', "/");
        FACTORY.iter().any(|p| matches(&path, p))
    })
}

/// `ci`의 부분. 인자 없이 부르면 모두 돈다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parts {
    /// 형식 검사와 lint.
    pub lint: bool,
    /// 빌드, 테스트, 실제 볼륨 테스트.
    pub test: bool,
    /// 공장 스크립트 시험(바뀌었을 때만, `--full`이면 늘).
    pub factory: bool,
    /// 벤치마크(바뀐 곳만, `--full`이면 모두).
    pub bench: bool,
}

/// `cargo xtask ci`의 인자.
#[derive(Debug, PartialEq, Eq)]
pub struct CiOptions {
    /// 벤치마크를 모두, 공장 스크립트 시험을 늘 돌린다.
    pub full: bool,
    /// 바뀐 파일과 상관없이 더 돌릴 벤치마크.
    pub benches: Vec<&'static str>,
    /// 바뀐 파일을 잴 기준(이것과 `HEAD`의 merge-base).
    pub base: String,
    /// 돌릴 부분.
    pub parts: Parts,
}

/// 바뀐 파일을 잴 기본 기준.
pub const DEFAULT_BASE: &str = "origin/main";

impl CiOptions {
    /// `ci` 뒤의 인자를 읽는다: `--full`, `--bench 이름,…`(되풀이 가능), `--base 기준`,
    /// `--only lint,test,factory,bench`.
    pub fn parse(rest: &[String]) -> Result<Self, String> {
        let mut options = CiOptions {
            full: false,
            benches: Vec::new(),
            base: DEFAULT_BASE.to_owned(),
            parts: Parts {
                lint: true,
                test: true,
                factory: true,
                bench: true,
            },
        };
        let mut it = rest.iter();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--full" => options.full = true,
                "--bench" => {
                    let v = it.next().ok_or("--bench 뒤에 벤치마크 이름이 없습니다")?;
                    for name in v.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                        let known = BENCHES.iter().find(|b| **b == name).ok_or_else(|| {
                            format!(
                                "알 수 없는 벤치마크: {name} (있는 것: {})",
                                BENCHES.join(", ")
                            )
                        })?;
                        if !options.benches.contains(known) {
                            options.benches.push(known);
                        }
                    }
                }
                "--base" => {
                    options.base = it.next().ok_or("--base 뒤에 기준이 없습니다")?.clone();
                }
                "--only" => {
                    let v = it.next().ok_or("--only 뒤에 부분 이름이 없습니다")?;
                    let mut parts = Parts {
                        lint: false,
                        test: false,
                        factory: false,
                        bench: false,
                    };
                    for name in v.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                        match name {
                            "lint" => parts.lint = true,
                            "test" => parts.test = true,
                            "factory" => parts.factory = true,
                            "bench" => parts.bench = true,
                            _ => {
                                return Err(format!(
                                    "알 수 없는 부분: {name} (lint, test, factory, bench)"
                                ));
                            }
                        }
                    }
                    options.parts = parts;
                }
                _ => return Err(format!("알 수 없는 인자: {arg}")),
            }
        }
        Ok(options)
    }
}

#[cfg(test)]
mod tests {
    use super::{BENCHES, CiOptions, DEFAULT_BASE, select_benches, touches_factory};

    fn files(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    fn picked(v: &[&str]) -> Vec<&'static str> {
        select_benches(&files(v)).benches
    }

    #[test]
    fn a_tab_change_runs_only_the_benches_that_use_tabs() {
        assert_eq!(
            picked(&["crates/facet-core/src/tab.rs"]),
            ["menu", "batch-rename", "filter", "shell-list"]
        );
    }

    #[test]
    fn engines_map_to_their_own_benches() {
        assert_eq!(
            picked(&["crates/facet-core/src/live_search.rs"]),
            ["search"]
        );
        assert_eq!(
            picked(&["crates/facet-core/src/content_search.rs"]),
            ["content"]
        );
        assert_eq!(
            picked(&["crates/facet-core/src/job/copy.rs"]),
            ["copy", "delete"]
        );
        assert_eq!(picked(&["testdata/mft/ntfs-basic.mft"]), ["mft"]);
        // 차례는 늘 `BENCHES`의 차례다.
        assert_eq!(
            picked(&[
                "crates/facet-core/src/content_search.rs",
                "crates/facet-core/src/session.rs",
            ]),
            ["session", "content"]
        );
    }

    #[test]
    fn common_files_run_every_bench() {
        for path in [
            "Cargo.lock",
            "Cargo.toml",
            "rust-toolchain.toml",
            "xtask/src/main.rs",
            "crates/facet-bench/src/lib.rs",
            "crates/facet-core/src/lib.rs",
            "crates/facet-core/Cargo.toml",
        ] {
            assert_eq!(picked(&[path]), BENCHES, "{path}");
        }
    }

    #[test]
    fn a_bench_crate_source_missing_from_the_table_runs_every_bench() {
        assert_eq!(picked(&["crates/facet-core/src/new_module.rs"]), BENCHES);
        assert_eq!(picked(&["crates/facet-win/src/windows_new.rs"]), BENCHES);
    }

    #[test]
    fn changes_that_do_not_reach_the_benches_run_none() {
        assert!(picked(&[]).is_empty());
        assert!(
            picked(&[
                "AGENTS.md",
                "docs/roadmap.md",
                ".github/workflows/ci.yml",
                ".github/factory/verify.sh",
                "crates/facet-app/src/main.rs",
                "crates/facet-app/ui/app.slint",
                "crates/facet-query/src/main.rs",
                "crates/facet-core/tests/tabs.rs",
                "crates/facet-core/src/edit.rs",
            ])
            .is_empty()
        );
    }

    #[test]
    fn backslash_paths_are_the_same_files() {
        assert_eq!(picked(&[r"crates\facet-core\src\menu.rs"]), ["menu"]);
    }

    #[test]
    fn reasons_name_each_file_that_reaches_a_bench() {
        let s = select_benches(&files(&[
            "crates/facet-core/src/menu.rs",
            "docs/x.md",
            "Cargo.lock",
        ]));
        assert_eq!(
            s.reasons,
            ["crates/facet-core/src/menu.rs → menu", "Cargo.lock → 모두"]
        );
    }

    #[test]
    fn every_table_entry_names_known_benches() {
        for (path, benches) in super::TABLE {
            for b in *benches {
                assert!(BENCHES.contains(b), "{path}: {b}");
            }
        }
    }

    #[test]
    fn factory_tests_follow_factory_paths() {
        assert!(touches_factory(&files(&[".github/factory/ledger.sh"])));
        assert!(touches_factory(&files(&[
            ".github/workflows/factory-verify.yml"
        ])));
        assert!(!touches_factory(&files(&[
            ".github/workflows/ci.yml",
            "crates/facet-core/src/tab.rs",
        ])));
    }

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn ci_options_default_to_everything_against_origin_main() {
        let o = CiOptions::parse(&[]).unwrap();
        assert!(!o.full);
        assert!(o.benches.is_empty());
        assert_eq!(o.base, DEFAULT_BASE);
        assert!(o.parts.lint && o.parts.test && o.parts.factory && o.parts.bench);
    }

    #[test]
    fn ci_options_read_full_bench_base_and_only() {
        let o = CiOptions::parse(&args(&[
            "--full",
            "--bench",
            "search,filter",
            "--bench",
            "search",
            "--base",
            "HEAD^1",
            "--only",
            "lint,bench",
        ]))
        .unwrap();
        assert!(o.full);
        assert_eq!(o.benches, ["search", "filter"]);
        assert_eq!(o.base, "HEAD^1");
        assert!(o.parts.lint && o.parts.bench && !o.parts.test && !o.parts.factory);
        assert!(CiOptions::parse(&args(&["--bench", "nope"])).is_err());
        assert!(CiOptions::parse(&args(&["--only", "docs"])).is_err());
        assert!(CiOptions::parse(&args(&["--bench"])).is_err());
        assert!(CiOptions::parse(&args(&["--fast"])).is_err());
    }
}
