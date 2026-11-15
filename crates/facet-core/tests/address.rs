//! 주소창 글자 해석과 자동 완성을 공개 인터페이스로만 확인한다.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use facet_core::details::Sort;
use facet_core::tab::{Navigation, ReadOutcome, Tab};
use facet_core::{FileSystemProvider, Location, Provider, ResolveError};
use tempfile::TempDir;

fn loc(path: &Path) -> Location {
    Location::file_system(path)
}

/// `root/Alpha/inner`, `root/alpine`, `root/beta`, `root/알파`, `root/Alpha.txt`.
fn tree() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("Alpha").join("inner")).unwrap();
    fs::create_dir(root.join("alpine")).unwrap();
    fs::create_dir(root.join("beta")).unwrap();
    fs::create_dir(root.join("알파")).unwrap();
    fs::write(root.join("Alpha.txt"), b"x").unwrap();
    fs::write(root.join("Alpha").join("a.txt"), b"x").unwrap();
    dir
}

fn go(tab: &mut Tab, navigation: Navigation) -> ReadOutcome {
    let request = tab.navigate(navigation).unwrap();
    tab.finish(request.run())
}

fn tab_at(path: &Path) -> Tab {
    let mut tab = Tab::new(loc(path), Sort::default(), false);
    assert_eq!(go(&mut tab, Navigation::Refresh), ReadOutcome::Committed);
    tab
}

fn resolve(text: &str) -> Result<facet_core::Resolved, ResolveError> {
    FileSystemProvider.resolve(text)
}

fn sep() -> char {
    std::path::MAIN_SEPARATOR
}

#[test]
fn 폴더_경로는_그_폴더로_해석한다() {
    let dir = tree();
    let text = dir.path().join("Alpha");
    let resolved = resolve(text.to_str().unwrap()).unwrap();
    assert_eq!(resolved.location, loc(&text));
    assert_eq!(resolved.select, None);
}

#[test]
fn 끝_구분자와_앞뒤_공백_따옴표는_있어도_된다() {
    let dir = tree();
    let text = dir.path().join("Alpha");
    let want = loc(&text);
    for typed in [
        format!("{}{}", text.display(), sep()),
        format!("  {}  ", text.display()),
        format!("\"{}\"", text.display()),
    ] {
        assert_eq!(resolve(&typed).unwrap().location, want, "{typed:?}");
    }
}

#[test]
fn 환경_변수를_펼친다() {
    let name = if cfg!(windows) { "TEMP" } else { "HOME" };
    let value = std::env::var(name).unwrap();
    let resolved = resolve(&format!("%{name}%")).unwrap();
    let want: PathBuf = PathBuf::from(&value).components().collect();
    assert_eq!(resolved.location, loc(&want));
    assert!(matches!(
        resolve("%FACET_NO_SUCH_VARIABLE_51%"),
        Err(ResolveError::UndefinedVariable { name }) if name == "FACET_NO_SUCH_VARIABLE_51"
    ));
}

#[test]
fn 잘못된_글자는_이유가_담긴_오류다() {
    assert!(matches!(resolve(""), Err(ResolveError::Empty)));
    assert!(matches!(resolve("   "), Err(ResolveError::Empty)));
    let dir = tree();
    let bad = format!("{}{}a|b", dir.path().display(), sep());
    assert!(matches!(
        resolve(&bad),
        Err(ResolveError::InvalidCharacter { character: '|' })
    ));
    assert!(matches!(
        resolve("relative/path"),
        Err(ResolveError::NotAbsolute { .. })
    ));
}

#[test]
fn 없는_경로는_오류다() {
    let dir = tree();
    let missing = dir.path().join("없음");
    let error = resolve(missing.to_str().unwrap()).unwrap_err();
    assert!(matches!(&error, ResolveError::NotFound { location } if *location == loc(&missing)));
    assert!(error.to_string().contains("없음"));
}

#[test]
fn 파일_경로는_그_폴더와_파일_이름으로_해석한다() {
    let dir = tree();
    let file = dir.path().join("Alpha.txt");
    let resolved = resolve(file.to_str().unwrap()).unwrap();
    assert_eq!(resolved.location, loc(dir.path()));
    assert_eq!(resolved.select, Some(OsString::from("Alpha.txt")));
}

#[test]
fn 파일_경로로_가면_그_폴더에서_그_파일이_선택된다() {
    let dir = tree();
    let sub = dir.path().join("Alpha");
    let mut tab = tab_at(&sub);
    let file = dir.path().join("Alpha.txt");
    let request = tab.go_to_text(file.to_str().unwrap()).unwrap();
    assert_eq!(
        tab.location(),
        &loc(&sub),
        "읽기가 끝나기 전에는 옮기지 않는다"
    );
    assert_eq!(tab.finish(request.run()), ReadOutcome::Committed);
    assert_eq!(tab.location(), &loc(dir.path()));
    assert_eq!(tab.back_history(), [loc(&sub)]);
    assert_eq!(tab.selected_count(), 1);
    assert_eq!(tab.selected_items().next().unwrap().name, "Alpha.txt");
    assert_eq!(tab.cursor_item().unwrap().name, "Alpha.txt");
}

#[test]
fn 같은_폴더의_파일_경로면_기록을_늘리지_않고_고른다() {
    let dir = tree();
    let mut tab = tab_at(dir.path());
    let file = dir.path().join("Alpha.txt");
    let request = tab.go_to_text(file.to_str().unwrap()).unwrap();
    assert_eq!(tab.finish(request.run()), ReadOutcome::Committed);
    assert!(!tab.can_go_back());
    assert_eq!(tab.selected_items().next().unwrap().name, "Alpha.txt");
}

#[test]
fn 폴더_글자로_가면_선택은_없다() {
    let dir = tree();
    let mut tab = tab_at(dir.path());
    let sub = dir.path().join("Alpha");
    let request = tab
        .go_to_text(&format!("{}{}", sub.display(), sep()))
        .unwrap();
    assert_eq!(tab.finish(request.run()), ReadOutcome::Committed);
    assert_eq!(tab.location(), &loc(&sub));
    assert_eq!(tab.selected_count(), 0);
}

#[test]
fn 해석에_실패하면_탭은_원래_위치에_남고_기록은_그대로다() {
    let dir = tree();
    let sub = dir.path().join("Alpha");
    let mut tab = tab_at(dir.path());
    go(&mut tab, Navigation::Open("Alpha".into()));
    go(&mut tab, Navigation::Back);
    assert_eq!(tab.location(), &loc(dir.path()));
    let back = tab.back_history().to_vec();
    let forward = tab.forward_history().to_vec();
    assert_eq!(forward, [loc(&sub)]);
    let names: Vec<_> = tab.items().map(|i| i.name.clone()).collect();

    let missing = dir.path().join("없음").display().to_string();
    for text in ["", "relative", "a|b", missing.as_str()] {
        assert!(tab.go_to_text(text).is_none(), "{text:?}");
        assert!(tab.address_error().is_some(), "{text:?}");
        assert_eq!(tab.location(), &loc(dir.path()));
        assert_eq!(tab.back_history(), back);
        assert_eq!(tab.forward_history(), forward);
        assert!(tab.pending_location().is_none());
        assert_eq!(
            tab.items().map(|i| i.name.clone()).collect::<Vec<_>>(),
            names
        );
    }

    // 다음 이동을 시작하면 오류를 지운다.
    assert!(tab.go_to_text(sub.to_str().unwrap()).is_some());
    assert!(tab.address_error().is_none());
}

fn complete_names(text: &str) -> Vec<String> {
    FileSystemProvider
        .complete(text)
        .into_iter()
        .map(|c| c.name.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn 자동_완성은_하위_폴더만_대소문자_무시_접두어로_낸다() {
    let dir = tree();
    let root = dir.path().display();
    let s = sep();
    // 파일(Alpha.txt)은 빠지고, 대소문자를 가리지 않으며 이름순이다.
    assert_eq!(complete_names(&format!("{root}{s}al")), ["Alpha", "alpine"]);
    assert_eq!(complete_names(&format!("{root}{s}AL")), ["Alpha", "alpine"]);
    assert_eq!(complete_names(&format!("{root}{s}alph")), ["Alpha"]);
    assert_eq!(complete_names(&format!("{root}{s}알")), ["알파"]);
    assert!(complete_names(&format!("{root}{s}zzz")).is_empty());
    // 마지막 구분자 바로 뒤면 모든 하위 폴더.
    assert_eq!(
        complete_names(&format!("{root}{s}")),
        ["Alpha", "alpine", "beta", "알파"]
    );
    // 마지막 구분자 앞까지가 폴더다.
    assert_eq!(complete_names(&format!("{root}{s}Alpha{s}")), ["inner"]);
    // 폴더가 없거나 구분자가 없으면 후보가 없다.
    assert!(complete_names(&format!("{root}{s}없음{s}a")).is_empty());
    assert!(complete_names("al").is_empty());
}

#[test]
fn 자동_완성_후보는_위치를_담는다() {
    let dir = tree();
    let found = FileSystemProvider.complete(&format!("{}{}be", dir.path().display(), sep()));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].location, loc(&dir.path().join("beta")));
}

#[test]
fn 늦게_온_자동_완성_결과는_버린다() {
    let dir = tree();
    let mut tab = tab_at(dir.path());
    let base = format!("{}{}", dir.path().display(), sep());
    let first = tab.complete(&format!("{base}a"));
    let second = tab.complete(&format!("{base}b"));
    // 요청은 일꾼 스레드에서 읽을 수 있다.
    let (late, current) = std::thread::scope(|scope| {
        let a = scope.spawn(|| first.run());
        let b = scope.spawn(|| second.run());
        (a.join().unwrap(), b.join().unwrap())
    });
    assert!(tab.finish_completion(late).is_none());
    let names: Vec<_> = tab
        .finish_completion(current)
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(names, ["beta"]);
}
