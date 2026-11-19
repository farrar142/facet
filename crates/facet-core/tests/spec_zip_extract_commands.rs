// target: crates/facet-core/tests/spec_zip_extract_commands.rs
// AC: #427: 세 명령이 팔레트에 들고 id를 가짐, 여기에 모두 꺼내기와 작업 기록·실행취소, zip 이름의 폴더로 꺼내기와 합치기, zip 위치의 여기에 모두 꺼내기, zip이 아니면 꺼짐, 꺼내기 위치…의 대상 해석과 꺼낼 내용
//! 꺼내기 명령 「여기에 모두 꺼내기」「zip 이름의 폴더로 꺼내기」「꺼내기 위치…」를 확인한다. 명령 목록·id·팔레트는
//! 모든 플랫폼에서, 임시 폴더의 실제 zip으로 명령을 실행해 꺼낸 결과·작업 기록·실행취소·켜짐/꺼짐·대상 해석을 보는
//! 것은 Windows에서만 돈다(파일 연산·휴지통이 Windows 전용).

#[path = "spec_zip_support.rs"]
mod zip_support;

#[cfg(windows)]
#[path = "common/bin.rs"]
mod bin;

use facet_core::command::CommandId;
use facet_core::window::App;

const COMMANDS: [(CommandId, &str); 3] = [
    (CommandId::ExtractAllHere, "extract-all-here"),
    (CommandId::ExtractToFolder, "extract-to-folder"),
    (CommandId::ExtractTo, "extract-to"),
];

#[test]
fn the_three_commands_are_in_the_palette_with_their_ids() {
    let app = App::new(None);
    let palette = app.search_commands("", |command| command.as_str());
    for (command, id) in COMMANDS {
        assert_eq!(command.as_str(), id);
        assert!(
            CommandId::ALL.contains(&command),
            "{command:?}는 명령 목록에 든다"
        );
        assert!(
            command.shortcuts().is_empty(),
            "{command:?}는 기본 단축키가 없다"
        );
        assert!(
            palette.iter().any(|entry| entry.command == command),
            "팔레트에 {command:?}가 있다"
        );
    }
}

#[cfg(windows)]
mod run {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use facet_core::Location;
    use facet_core::command::CommandId;
    use facet_core::history::OperationKind;
    use facet_core::job::{JobId, JobKind, JobState, Summary};
    use facet_core::tab::ReadOutcome;
    use facet_core::window::{App, Execution, TargetError};
    use tempfile::TempDir;

    use super::bin;
    use super::zip_support::{self, Entry};

    const WAIT: Duration = Duration::from_secs(120);

    struct Fixture {
        dir: TempDir,
        zip: PathBuf,
    }

    /// 임시 폴더: `자료.v2.zip`(top.txt, 문서/readme.txt), 옆 파일 `옆.txt`, 하위 폴더 `폴더`.
    fn fixture() -> Fixture {
        let dir = TempDir::new().unwrap();
        let zip = dir.path().join("자료.v2.zip");
        zip_support::write_zip(
            &zip,
            &[
                Entry::stored("top.txt", b"top"),
                Entry::deflated("문서/readme.txt", &zip_support::repeated_text(4)),
            ],
        );
        fs::write(dir.path().join("옆.txt"), "next").unwrap();
        fs::create_dir(dir.path().join("폴더")).unwrap();
        Fixture { dir, zip }
    }

    fn app_at(location: Location) -> App {
        let mut app = App::new(Some(location));
        let (_, outcome) = app.wait(WAIT).expect("첫 읽기");
        assert_eq!(outcome, ReadOutcome::Committed);
        app
    }

    fn select(app: &mut App, name: &str) {
        let tab = app.active_tab();
        let index = (0..tab.len())
            .find(|&i| tab.get(i).unwrap().name == name)
            .unwrap_or_else(|| panic!("{name}가 목록에 없다"));
        app.active_tab_mut().select_only(index);
    }

    fn wait_job(app: &mut App, job: JobId) -> (JobState, Summary) {
        let deadline = Instant::now() + WAIT;
        loop {
            app.poll_jobs();
            app.poll();
            if let Some(entry) = app.work_area().entry(job)
                && let Some(summary) = &entry.summary
            {
                bin::purge(&summary.recycled);
                return (entry.state, summary.clone());
            }
            assert!(Instant::now() < deadline, "작업이 시간 안에 끝나지 않았다");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// 명령을 실행해 넣은 작업이 끝나기를 기다린다.
    fn run_command(app: &mut App, command: CommandId) -> (JobState, Summary) {
        let before = app.work_area().entries().len();
        assert!(app.can_execute(command), "{command:?}가 켜져 있다");
        assert_eq!(app.execute(command), Execution::Done);
        app.poll_jobs();
        let entries = app.work_area().entries();
        assert_eq!(entries.len(), before + 1, "작업을 하나 넣었다");
        let job = entries.last().unwrap().id;
        wait_job(app, job)
    }

    fn tree(root: &Path) -> BTreeSet<String> {
        fn walk(root: &Path, dir: &Path, out: &mut BTreeSet<String>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                out.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
                if path.is_dir() {
                    walk(root, &path, out);
                }
            }
        }
        let mut out = BTreeSet::new();
        walk(root, root, &mut out);
        out
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn extract_all_here_from_a_zip_file_and_undo() {
        let f = fixture();
        let mut app = app_at(Location::file_system(f.dir.path()));
        select(&mut app, "자료.v2.zip");
        let (state, _) = run_command(&mut app, CommandId::ExtractAllHere);
        assert_eq!(state, JobState::Finished);
        assert_eq!(
            tree(f.dir.path()),
            set(&[
                "top.txt",
                "문서",
                "문서/readme.txt",
                "옆.txt",
                "자료.v2.zip",
                "폴더"
            ])
        );
        assert_eq!(
            fs::read(f.dir.path().join("문서").join("readme.txt")).unwrap(),
            zip_support::repeated_text(4)
        );
        let record = app.history().records().next_back().expect("작업 기록");
        assert_eq!(record.kind(), OperationKind::Extract);

        let undo = app.undo().expect("실행취소");
        let (state, _) = wait_job(&mut app, undo);
        assert_eq!(state, JobState::Finished);
        assert_eq!(
            tree(f.dir.path()),
            set(&["옆.txt", "자료.v2.zip", "폴더"]),
            "꺼낸 항목이 사라졌다"
        );
    }

    #[test]
    fn extract_to_a_folder_named_after_the_zip_and_merge() {
        let f = fixture();
        let named = f.dir.path().join("자료.v2");
        let mut app = app_at(Location::file_system(f.dir.path()));
        select(&mut app, "자료.v2.zip");
        let (state, _) = run_command(&mut app, CommandId::ExtractToFolder);
        assert_eq!(state, JobState::Finished);
        assert_eq!(tree(&named), set(&["top.txt", "문서", "문서/readme.txt"]));

        // 같은 이름의 폴더가 이미 있으면 합친다.
        fs::remove_dir_all(&named).unwrap();
        fs::create_dir(&named).unwrap();
        fs::write(named.join("기존.txt"), "kept").unwrap();
        select(&mut app, "자료.v2.zip");
        let (state, _) = run_command(&mut app, CommandId::ExtractToFolder);
        assert_eq!(state, JobState::Finished);
        assert_eq!(
            tree(&named),
            set(&["top.txt", "기존.txt", "문서", "문서/readme.txt"])
        );
        assert_eq!(fs::read_to_string(named.join("기존.txt")).unwrap(), "kept");
    }

    #[test]
    fn extract_all_here_inside_the_zip_uses_the_zips_folder() {
        let f = fixture();
        let mut app = app_at(Location::zip(&f.zip, "문서"));
        let (state, _) = run_command(&mut app, CommandId::ExtractAllHere);
        assert_eq!(state, JobState::Finished);
        assert!(f.dir.path().join("top.txt").is_file(), "zip 전체를 꺼낸다");
        assert!(f.dir.path().join("문서").join("readme.txt").is_file());
    }

    #[test]
    fn the_commands_are_off_without_a_zip() {
        let f = fixture();
        let mut app = app_at(Location::file_system(f.dir.path()));
        for name in ["옆.txt", "폴더"] {
            select(&mut app, name);
            for command in [
                CommandId::ExtractAllHere,
                CommandId::ExtractToFolder,
                CommandId::ExtractTo,
            ] {
                assert!(!app.can_execute(command), "{name}에서 {command:?}는 꺼진다");
                assert_eq!(app.execute(command), Execution::Disabled, "{command:?}");
            }
        }
        let empty = f.dir.path().join("폴더");
        let app = app_at(Location::file_system(&empty));
        assert!(
            !app.can_execute(CommandId::ExtractAllHere),
            "커서 항목이 없다"
        );
        assert!(app.work_area().entries().is_empty(), "작업을 넣지 않았다");
    }

    #[test]
    fn extract_to_resolves_the_target_and_extracts() {
        let f = fixture();
        let other = f.dir.path().join("다른 곳");
        fs::create_dir(&other).unwrap();

        // 실제 폴더의 zip 파일: zip 전체.
        let mut app = app_at(Location::file_system(f.dir.path()));
        select(&mut app, "자료.v2.zip");
        assert_eq!(
            app.execute(CommandId::ExtractTo),
            Execution::ChooseExtractTarget
        );
        let plan = app
            .extract_plan_to(&other.to_string_lossy())
            .expect("대상 폴더를 해석한다");
        assert_eq!(plan.kind, JobKind::Copy);
        assert_eq!(plan.target, other);
        let mut sources = plan.sources.clone();
        sources.sort();
        let mut expected = vec![f.zip.join("top.txt"), f.zip.join("문서")];
        expected.sort();
        assert_eq!(sources, expected, "zip 루트의 항목들");
        let job = app.start_transfer(&plan).expect("꺼내기를 넣는다");
        assert_eq!(wait_job(&mut app, job).0, JobState::Finished);
        assert_eq!(tree(&other), set(&["top.txt", "문서", "문서/readme.txt"]));

        assert!(
            matches!(
                app.extract_plan_to(&f.dir.path().join("없는 폴더").to_string_lossy()),
                Err(TargetError::Resolve(_))
            ),
            "해석하지 못하면 오류"
        );

        // zip 위치에서 고른 항목만.
        let only = f.dir.path().join("하나만");
        fs::create_dir(&only).unwrap();
        let mut app = app_at(Location::zip(&f.zip, ""));
        select(&mut app, "top.txt");
        assert_eq!(
            app.execute(CommandId::ExtractTo),
            Execution::ChooseExtractTarget
        );
        let plan = app.extract_plan_to(&only.to_string_lossy()).unwrap();
        assert_eq!(plan.sources, [f.zip.join("top.txt")]);
        let job = app.start_transfer(&plan).unwrap();
        assert_eq!(wait_job(&mut app, job).0, JobState::Finished);
        assert_eq!(tree(&only), set(&["top.txt"]));
    }
}
