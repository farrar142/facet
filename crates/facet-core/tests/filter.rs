//! 필터(탭마다 현재 폴더 목록을 이름으로 좁히기)를 공개 인터페이스로 확인한다. 임시 폴더의 실제 파일을 읽는다.

use std::fs;
use std::path::Path;
use std::time::Duration;

use facet_core::Location;
use facet_core::command::{CommandId, Key, find_by_shortcut};
use facet_core::details::{Column, Direction, Sort};
use facet_core::tab::{Navigation, ReadOutcome, Tab};
use facet_core::window::{App, Execution, Update};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(10);

fn go(tab: &mut Tab, navigation: Navigation) -> ReadOutcome {
    let request = tab.navigate(navigation).expect("이동할 수 있어야 한다");
    tab.finish(request.run())
}

fn tab_at(path: &Path) -> Tab {
    let mut tab = Tab::new(Location::file_system(path), Sort::default(), true);
    assert_eq!(go(&mut tab, Navigation::Refresh), ReadOutcome::Committed);
    tab
}

fn names(tab: &Tab) -> Vec<String> {
    tab.items()
        .map(|i| i.name.to_string_lossy().into_owned())
        .collect()
}

fn selected(tab: &Tab) -> Vec<String> {
    tab.selected_items()
        .map(|i| i.name.to_string_lossy().into_owned())
        .collect()
}

fn folder(files: &[&str], folders: &[&str]) -> TempDir {
    let dir = TempDir::new().unwrap();
    for name in files {
        fs::write(dir.path().join(name), name.as_bytes()).unwrap();
    }
    for name in folders {
        fs::create_dir(dir.path().join(name)).unwrap();
    }
    dir
}

const FILES: &[&str] = &[
    "보고서.txt",
    "Report.PDF",
    "report-final.pdf",
    "notes.md",
    "photo 🎉.jpg",
    ".hidden-rc",
];

fn sorted(mut names: Vec<String>) -> Vec<String> {
    names.sort();
    names
}

#[test]
fn 이름으로_좁히고_폴더도_같은_규칙으로_거른다() {
    let dir = folder(FILES, &["Reports", "기타"]);
    let mut tab = tab_at(dir.path());
    assert_eq!(tab.len(), 8);

    tab.set_filter("report");
    assert_eq!(tab.filter(), Some("report"));
    assert_eq!(
        sorted(names(&tab)),
        ["Report.PDF", "Reports", "report-final.pdf"]
    );

    tab.set_filter("*.pdf");
    assert_eq!(sorted(names(&tab)), ["Report.PDF", "report-final.pdf"]);

    tab.set_filter("보고");
    assert_eq!(names(&tab), ["보고서.txt"]);

    tab.clear_filter();
    assert_eq!(tab.filter(), None);
    assert_eq!(tab.len(), 8);
}

#[test]
fn 글자를_덧붙이면_앞_결과_안에서_거르고_결과는_처음부터_거른_것과_같다() {
    let dir = folder(FILES, &["Reports", "기타"]);
    let mut incremental = tab_at(dir.path());
    for text in ["r", "re", "rep", "repo", "report", "report-", "report-f"] {
        incremental.set_filter(text);
        let mut fresh = tab_at(dir.path());
        fresh.set_filter(text);
        assert_eq!(names(&incremental), names(&fresh), "{text}");
    }
    // 지우고 다른 글자로: 처음부터.
    incremental.set_filter("o");
    let mut fresh = tab_at(dir.path());
    fresh.set_filter("o");
    assert_eq!(names(&incremental), names(&fresh));
    // 와일드카드: 덧붙여도 좁아진다고 할 수 없는 경우(「?」 → 「?o」)도 처음부터 거른 것과 같다.
    for text in ["?", "?o", "?o*", "*", "*.", "*.p"] {
        incremental.set_filter(text);
        let mut fresh = tab_at(dir.path());
        fresh.set_filter(text);
        assert_eq!(names(&incremental), names(&fresh), "{text}");
    }
}

#[test]
fn 가려진_항목은_선택과_커서에서_빠진다() {
    let dir = folder(FILES, &["기타"]);
    let mut tab = tab_at(dir.path());
    tab.select_all();
    assert_eq!(tab.selected_count(), 7);
    tab.select_only(0); // 폴더 「기타」
    assert_eq!(selected(&tab), ["기타"]);

    tab.set_filter("md");
    assert_eq!(names(&tab), ["notes.md"]);
    assert_eq!(tab.selected_count(), 0, "가려진 항목은 선택에서 빠진다");
    assert_eq!(selected(&tab), Vec::<String>::new());
    if let Some(cursor) = tab.cursor() {
        assert_eq!(tab.get(cursor).unwrap().name, "notes.md");
    }

    // 이미 읽은 목록만 거른다: 폴더에 새로 생긴 파일은 다시 읽기 전에는 필터 변경으로 나타나지 않는다.
    fs::write(dir.path().join("another.md"), b"x").unwrap();
    tab.set_filter("an");
    assert_eq!(names(&tab), Vec::<String>::new());
    tab.clear_filter();
    assert_eq!(tab.len(), 7);
}

#[test]
fn 필터에_맞는_선택은_필터를_바꿔도_남는다() {
    let dir = folder(&["a.log", "b.log", "c.txt"], &[]);
    let mut tab = tab_at(dir.path());
    tab.select_all();
    tab.set_filter("log");
    assert_eq!(selected(&tab), ["a.log", "b.log"]);
    tab.set_filter("a.");
    assert_eq!(selected(&tab), ["a.log"]);
}

#[test]
fn 정렬을_바꿔도_필터가_유지된다() {
    let dir = folder(&["b.log", "a.log", "c.txt"], &[]);
    let mut tab = tab_at(dir.path());
    tab.set_filter("log");
    assert_eq!(names(&tab), ["a.log", "b.log"]);
    tab.set_sort(Sort {
        column: Column::Name,
        direction: Direction::Descending,
    });
    assert_eq!(tab.filter(), Some("log"), "정렬을 바꿔도 필터가 유지된다");
    assert_eq!(names(&tab), ["b.log", "a.log"]);
    tab.set_sort(Sort {
        column: Column::Size,
        direction: Direction::Ascending,
    });
    assert_eq!(tab.len(), 2);
    assert_eq!(tab.status().total_count, Some(3));
}

#[cfg(windows)]
#[test]
fn 숨김_속성_항목은_보이기가_꺼져_있으면_필터에_맞아도_숨는다() {
    let dir = folder(&["visible.log", "secret.log"], &[]);
    let status = std::process::Command::new("attrib")
        .arg("+h")
        .arg(dir.path().join("secret.log"))
        .status()
        .unwrap();
    assert!(status.success());
    let mut tab = Tab::new(Location::file_system(dir.path()), Sort::default(), false);
    go(&mut tab, Navigation::Refresh);
    tab.set_filter("log");
    assert_eq!(names(&tab), ["visible.log"]);
    assert_eq!(tab.status().total_count, Some(1));
    tab.set_show_hidden(true);
    assert_eq!(names(&tab), ["secret.log", "visible.log"]);
    assert_eq!(tab.status().total_count, Some(2));
    tab.set_show_hidden(false);
    assert_eq!(names(&tab), ["visible.log"]);
}

#[test]
fn 위치를_바꾸면_필터가_풀린다() {
    let dir = folder(&["a.txt", "b.txt"], &["sub"]);
    fs::write(dir.path().join("sub").join("x.txt"), b"x").unwrap();
    let mut tab = tab_at(dir.path());

    tab.set_filter("a");
    go(&mut tab, Navigation::Open("sub".into()));
    assert_eq!(tab.filter(), None, "폴더로 들어가면 풀린다");
    assert_eq!(names(&tab), ["x.txt"]);

    tab.set_filter("x");
    go(&mut tab, Navigation::Back);
    assert_eq!(tab.filter(), None, "뒤로");
    assert_eq!(tab.len(), 3);

    go(&mut tab, Navigation::Forward);
    tab.set_filter("x");
    go(&mut tab, Navigation::Up);
    assert_eq!(tab.filter(), None, "위로");

    tab.set_filter("a");
    go(
        &mut tab,
        Navigation::Go(Location::file_system(dir.path().join("sub"))),
    );
    assert_eq!(tab.filter(), None, "주소창");

    // 같은 위치를 다시 읽으면(F5) 필터는 그대로고 새 파일은 필터 조건에 맞을 때만 보인다.
    go(&mut tab, Navigation::Up);
    tab.set_filter("txt");
    fs::write(dir.path().join("new.txt"), b"x").unwrap();
    fs::write(dir.path().join("new.md"), b"x").unwrap();
    go(&mut tab, Navigation::Refresh);
    assert_eq!(tab.filter(), Some("txt"));
    assert_eq!(names(&tab), ["a.txt", "b.txt", "new.txt"]);
}

#[test]
fn 상태_수치에_보이는_수와_전체_수가_있다() {
    let dir = folder(FILES, &[]);
    let mut tab = tab_at(dir.path());
    assert_eq!(tab.status().total_count, None);
    tab.set_filter("report");
    let status = tab.status();
    assert_eq!(status.item_count, 2);
    assert_eq!(status.total_count, Some(6));
    tab.clear_filter();
    assert_eq!(tab.status().total_count, None);
    assert_eq!(tab.status().item_count, 6);
}

#[test]
fn 늦게_온_앞선_거르기_결과는_버린다() {
    let dir = folder(FILES, &[]);
    let mut tab = tab_at(dir.path());
    let first = tab.request_filter("report").unwrap();
    let second = tab.request_filter("notes").unwrap();
    assert_ne!(first.id(), second.id());
    // 뒤 요청의 결과가 먼저 오고 앞 요청의 결과가 늦게 와도 마지막 요청만 반영한다.
    let late = first.run();
    assert!(tab.finish_filter(second.run()));
    assert!(!tab.finish_filter(late), "늦게 온 앞선 결과는 버린다");
    assert_eq!(names(&tab), ["notes.md"]);

    // 지운 뒤 늦게 온 결과도 버린다.
    let pending = tab.request_filter("pdf").unwrap();
    tab.clear_filter();
    assert!(!tab.finish_filter(pending.run()));
    assert_eq!(tab.filter(), None);
    assert_eq!(tab.len(), 6);

    // 다른 위치로 간 뒤 늦게 온 결과도 버린다.
    let pending = tab.request_filter("pdf").unwrap();
    let other = folder(&["z.pdf"], &[]);
    go(
        &mut tab,
        Navigation::Go(Location::file_system(other.path())),
    );
    assert!(!tab.finish_filter(pending.run()));
    assert_eq!(tab.filter(), None);
}

#[test]
fn 거르는_동안_목록이_바뀌면_새_목록을_다시_거른다() {
    let dir = folder(&["a.log", "b.log"], &[]);
    let mut tab = tab_at(dir.path());
    let pending = tab.request_filter("log").unwrap();
    // 그 사이 목록을 다시 읽었다(새 파일이 생김).
    fs::write(dir.path().join("c.log"), b"x").unwrap();
    go(&mut tab, Navigation::Refresh);
    assert!(tab.finish_filter(pending.run()));
    assert_eq!(names(&tab), ["a.log", "b.log", "c.log"]);
}

#[test]
fn 후보가_많아_여러_스레드로_거를_때도_결과와_순서가_같다() {
    let dir = TempDir::new().unwrap();
    for i in 0..25_000 {
        fs::write(dir.path().join(format!("file{i:05}.dat")), b"").unwrap();
    }
    let mut tab = tab_at(dir.path());
    tab.set_filter("0000");
    // file00000..file00009 (「0000」이 든 이름): 앞 네 자리가 0인 것.
    let found = names(&tab);
    let expected: Vec<String> = (0..25_000)
        .map(|i| format!("file{i:05}.dat"))
        .filter(|n| n.contains("0000"))
        .collect();
    assert_eq!(sorted(found), sorted(expected));
    assert_eq!(tab.status().total_count, Some(25_000));
}

#[test]
fn 앱은_일꾼_스레드에서_거르고_결과를_받아_반영한다() {
    let dir = folder(FILES, &[]);
    let mut app = App::new(Some(Location::file_system(dir.path())));
    app.wait(WAIT).expect("첫 읽기");
    app.set_show_hidden(true);

    app.set_filter("report");
    let (_, update) = app.wait_update(WAIT).expect("거르기 결과");
    assert_eq!(update, Update::Filtered(true));
    assert_eq!(
        sorted(names(app.active_tab())),
        ["Report.PDF", "report-final.pdf"]
    );

    // 연달아 쳐도 마지막 글자의 결과만 남는다.
    app.set_filter("n");
    app.set_filter("no");
    app.set_filter("not");
    let mut applied = 0;
    for _ in 0..3 {
        if let Some((_, Update::Filtered(true))) = app.wait_update(WAIT) {
            applied += 1;
        }
    }
    assert!(applied >= 1, "마지막 요청의 결과는 반영된다");
    assert_eq!(names(app.active_tab()), ["notes.md"]);

    app.clear_filter();
    assert_eq!(app.active_tab().filter(), None);
    assert_eq!(app.active_tab().len(), 6);
}

#[test]
fn 필터_명령은_ctrl_shift_f이고_한글_모드에서도_같다() {
    let shortcuts = CommandId::Filter.shortcuts();
    assert_eq!(shortcuts.len(), 1);
    let dir = folder(&["a"], &[]);
    let mut app = App::new(Some(Location::file_system(dir.path())));
    app.wait(WAIT).expect("첫 읽기");
    assert!(app.can_execute(CommandId::Filter));
    assert_eq!(app.execute(CommandId::Filter), Execution::Filter);
    assert_eq!(find_by_shortcut(shortcuts[0]), Some(CommandId::Filter));
    assert_eq!(CommandId::from_id("filter"), Some(CommandId::Filter));
    // 두벌식에서 F 자리의 「ㄹ」도 같은 키다.
    assert_eq!(Key::from_char('ㄹ'), Key::from_char('f'));
}

#[cfg(windows)]
#[test]
fn 필터_중_파일을_만들면_조건에_맞을_때만_보이고_지운_항목은_빠진다() {
    use std::time::Instant;
    let dir = folder(&["a.log", "b.txt"], &[]);
    let mut app = App::new(Some(Location::file_system(dir.path())));
    app.wait(WAIT).expect("첫 읽기");
    app.set_filter("log");
    let (_, update) = app.wait_update(WAIT).expect("거르기 결과");
    assert_eq!(update, Update::Filtered(true));
    assert_eq!(names(app.active_tab()), ["a.log"]);

    let wait_until = |app: &mut App, what: &str, done: &dyn Fn(&Tab) -> bool| {
        let deadline = Instant::now() + WAIT;
        while !done(app.active_tab()) {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                !left.is_zero(),
                "{what}: 시간 안에 맞지 않았다: {:?}",
                names(app.active_tab())
            );
            app.wait_update(left);
        }
    };

    // 조건에 맞지 않는 새 파일은 보이지 않고, 맞는 새 파일은 보인다.
    fs::write(dir.path().join("c.md"), b"x").unwrap();
    fs::write(dir.path().join("d.log"), b"x").unwrap();
    wait_until(&mut app, "새 파일", &|tab| {
        names(tab) == ["a.log", "d.log"]
    });
    assert_eq!(app.active_tab().status().total_count, Some(4));

    fs::remove_file(dir.path().join("a.log")).unwrap();
    wait_until(&mut app, "지운 항목", &|tab| names(tab) == ["d.log"]);
    assert_eq!(app.active_tab().filter(), Some("log"));
}
