//! 탭의 선택·커서, 글자로 이동, 상태 표시줄 수치를 공개 인터페이스로만 확인한다.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::details::{Column, Direction, Sort};
use facet_core::selection::{CursorMove, StatusSummary, TYPE_AHEAD_TIMEOUT};
use facet_core::tab::{Navigation, ReadOutcome, Tab};
use tempfile::TempDir;

fn go(tab: &mut Tab, navigation: Navigation) -> ReadOutcome {
    let request = tab.navigate(navigation).expect("이동할 수 있어야 한다");
    tab.finish(request.run())
}

fn tab_at(path: &Path) -> Tab {
    let mut tab = Tab::new(Location::file_system(path), Sort::default(), true);
    assert_eq!(go(&mut tab, Navigation::Refresh), ReadOutcome::Committed);
    tab
}

fn folder_with(files: &[(&str, usize)], folders: &[&str]) -> TempDir {
    let dir = TempDir::new().unwrap();
    for (name, size) in files {
        fs::write(dir.path().join(name), vec![b'x'; *size]).unwrap();
    }
    for name in folders {
        fs::create_dir(dir.path().join(name)).unwrap();
    }
    dir
}

fn selected(tab: &Tab) -> Vec<String> {
    tab.selected_items()
        .map(|i| i.name.to_string_lossy().into_owned())
        .collect()
}

fn name_at(tab: &Tab, index: usize) -> String {
    tab.get(index).unwrap().name.to_string_lossy().into_owned()
}

/// 폴더 `d`, 파일 `a.txt`(1) `b.txt`(10) `c.txt`(100) `e.txt`(1000). 정렬하면 d, a, b, c, e.
fn basic() -> TempDir {
    folder_with(
        &[("a.txt", 1), ("b.txt", 10), ("c.txt", 100), ("e.txt", 1000)],
        &["d"],
    )
}

#[test]
fn 단일_선택은_하나만_남기고_커서를_옮긴다() {
    let dir = basic();
    let mut tab = tab_at(dir.path());
    tab.select_only(1);
    tab.select_only(3);
    assert_eq!(selected(&tab), ["c.txt"]);
    assert_eq!(tab.cursor(), Some(3));
    assert!(tab.is_selected(3) && !tab.is_selected(1));
}

#[test]
fn shift_범위는_기준부터_클릭한_곳까지다() {
    let dir = basic();
    let mut tab = tab_at(dir.path());
    tab.select_only(1);
    tab.select_range(3);
    assert_eq!(selected(&tab), ["a.txt", "b.txt", "c.txt"]);
    // 기준은 그대로라 반대쪽으로 늘리면 범위가 바뀐다.
    tab.select_range(0);
    assert_eq!(selected(&tab), ["d", "a.txt"]);
    assert_eq!(tab.cursor(), Some(0));
}

#[test]
fn shift_범위는_선택_없이_커서에서_시작할_수_있다() {
    let dir = basic();
    let mut tab = tab_at(dir.path());
    for _ in 0..3 {
        tab.move_cursor(CursorMove::Down);
    }
    tab.select_range(4);
    assert_eq!(selected(&tab), ["b.txt", "c.txt", "e.txt"]);
}

#[test]
fn ctrl_토글은_선택을_더하고_뺀다() {
    let dir = basic();
    let mut tab = tab_at(dir.path());
    tab.select_only(0);
    tab.toggle_select(2);
    tab.toggle_select(4);
    assert_eq!(selected(&tab), ["d", "b.txt", "e.txt"]);
    tab.toggle_select(2);
    assert_eq!(selected(&tab), ["d", "e.txt"]);
    // Ctrl로 더한 곳이 새 범위 기준이다.
    tab.select_range(3);
    assert_eq!(selected(&tab), ["b.txt", "c.txt"]);
}

#[test]
fn 모두_선택과_해제() {
    let dir = basic();
    let mut tab = tab_at(dir.path());
    tab.select_all();
    assert_eq!(tab.selected_count(), 5);
    assert_eq!(tab.cursor(), None);
    tab.clear_selection();
    assert_eq!(tab.selected_count(), 0);
}

#[test]
fn 커서만_옮기면_선택은_그대로다() {
    let dir = basic();
    let mut tab = tab_at(dir.path());
    tab.select_only(1);
    assert_eq!(tab.move_cursor(CursorMove::Down), Some(2));
    assert_eq!(selected(&tab), ["a.txt"]);
    assert_eq!(tab.cursor_item().unwrap().name, "b.txt");
    tab.toggle_cursor_selection();
    assert_eq!(selected(&tab), ["a.txt", "b.txt"]);
    tab.toggle_cursor_selection();
    assert_eq!(selected(&tab), ["a.txt"]);
}

#[test]
fn 커서_이동은_끝에서_멈춘다() {
    let dir = basic();
    let mut tab = tab_at(dir.path());
    assert_eq!(tab.move_cursor(CursorMove::Down), Some(0));
    assert_eq!(tab.move_cursor(CursorMove::Up), Some(0));
    assert_eq!(tab.move_cursor(CursorMove::PageDown(2)), Some(2));
    assert_eq!(tab.move_cursor(CursorMove::PageDown(10)), Some(4));
    assert_eq!(tab.move_cursor(CursorMove::Down), Some(4));
    assert_eq!(tab.move_cursor(CursorMove::PageUp(3)), Some(1));
    assert_eq!(tab.move_cursor(CursorMove::PageUp(3)), Some(0));
    assert_eq!(tab.move_cursor(CursorMove::Last), Some(4));
    assert_eq!(tab.move_cursor(CursorMove::First), Some(0));
}

#[test]
fn 빈_목록에서는_커서가_없다() {
    let dir = TempDir::new().unwrap();
    let mut tab = tab_at(dir.path());
    assert_eq!(tab.move_cursor(CursorMove::Down), None);
    assert_eq!(tab.move_cursor(CursorMove::Last), None);
    tab.select_only(0);
    tab.select_all();
    assert_eq!(tab.status().selected_count, 0);
}

#[test]
fn 정렬을_바꿔도_선택과_커서가_이름으로_유지된다() {
    let dir = basic();
    let mut tab = tab_at(dir.path());
    tab.select_only(1);
    tab.toggle_select(3);
    tab.set_sort(Sort {
        column: Column::Name,
        direction: Direction::Descending,
    });
    // 내림차순: d, e, c, b, a — 폴더는 그대로 앞이다.
    assert_eq!(name_at(&tab, 1), "e.txt");
    assert_eq!(selected(&tab), ["c.txt", "a.txt"]);
    assert_eq!(tab.cursor(), Some(2));
    assert_eq!(tab.cursor_item().unwrap().name, "c.txt");
    assert!(tab.is_selected(2) && tab.is_selected(4));
}

#[test]
fn 새로_고치면_사라진_항목은_선택에서_빠진다() {
    let dir = basic();
    let mut tab = tab_at(dir.path());
    tab.select_only(1);
    tab.toggle_select(2);
    tab.toggle_select(3);
    fs::remove_file(dir.path().join("b.txt")).unwrap();
    fs::write(dir.path().join("f.txt"), b"f").unwrap();
    go(&mut tab, Navigation::Refresh);
    assert_eq!(selected(&tab), ["a.txt", "c.txt"]);
    assert_eq!(tab.selected_count(), 2);
}

#[test]
fn 커서_항목이_사라지면_그_자리_근처로_옮긴다() {
    let dir = basic();
    let mut tab = tab_at(dir.path());
    tab.select_only(4);
    fs::remove_file(dir.path().join("e.txt")).unwrap();
    go(&mut tab, Navigation::Refresh);
    assert_eq!(tab.cursor(), Some(3));
    assert_eq!(tab.cursor_item().unwrap().name, "c.txt");
}

#[test]
fn 숨김_항목을_감추면_선택에서도_빠진다() {
    let dir = basic();
    let mut tab = tab_at(dir.path());
    tab.select_all();
    tab.set_show_hidden(false);
    // 숨김 속성은 시험 폴더에 없으므로 모두 남는다.
    assert_eq!(tab.selected_count(), 5);
}

#[test]
fn 뒤로_가면_선택과_스크롤이_되살아난다() {
    let dir = basic();
    let root = dir.path();
    let mut tab = tab_at(root);
    tab.select_only(2);
    tab.toggle_select(4);
    tab.set_scroll(3);
    go(&mut tab, Navigation::Open("d".into()));
    // 새 위치는 깨끗하게 시작한다.
    assert_eq!(tab.selected_count(), 0);
    assert_eq!(tab.cursor(), None);
    assert_eq!(tab.scroll(), 0);
    go(&mut tab, Navigation::Back);
    assert_eq!(selected(&tab), ["b.txt", "e.txt"]);
    assert_eq!(tab.cursor(), Some(4));
    assert_eq!(tab.scroll(), 3);
    // 앞으로 갔다가 다시 뒤로 와도 되살아난다.
    go(&mut tab, Navigation::Forward);
    go(&mut tab, Navigation::Back);
    assert_eq!(selected(&tab), ["b.txt", "e.txt"]);
    assert_eq!(tab.scroll(), 3);
}

#[test]
fn 앞으로_가면_떠날_때의_선택이_되살아난다() {
    let dir = basic();
    let mut tab = tab_at(dir.path());
    go(&mut tab, Navigation::Open("d".into()));
    fs::write(dir.path().join("d").join("x.txt"), b"x").unwrap();
    go(&mut tab, Navigation::Refresh);
    tab.select_only(0);
    tab.set_scroll(7);
    go(&mut tab, Navigation::Back);
    assert_eq!(tab.selected_count(), 0);
    go(&mut tab, Navigation::Forward);
    assert_eq!(selected(&tab), ["x.txt"]);
    assert_eq!(tab.scroll(), 7);
}

#[test]
fn 되살린_선택에서_사라진_항목은_빠진다() {
    let dir = basic();
    let mut tab = tab_at(dir.path());
    tab.select_only(1);
    tab.toggle_select(2);
    go(&mut tab, Navigation::Open("d".into()));
    fs::remove_file(dir.path().join("a.txt")).unwrap();
    go(&mut tab, Navigation::Back);
    assert_eq!(selected(&tab), ["b.txt"]);
}

#[test]
fn 위로_가면_방금_있던_폴더가_선택되고_커서가_간다() {
    let dir = folder_with(&[("z.txt", 1)], &["a", "b", "c"]);
    let mut tab = tab_at(dir.path());
    tab.select_only(0);
    go(&mut tab, Navigation::Open("b".into()));
    go(&mut tab, Navigation::Up);
    assert_eq!(selected(&tab), ["b"]);
    assert_eq!(tab.cursor(), Some(1));
    assert_eq!(tab.cursor_item().unwrap().name, "b");
}

#[test]
fn 읽기가_실패하면_선택은_그대로다() {
    let dir = basic();
    let mut tab = tab_at(dir.path());
    tab.select_only(2);
    let missing = Location::file_system(dir.path().join("없음"));
    assert_eq!(go(&mut tab, Navigation::Go(missing)), ReadOutcome::Failed);
    assert_eq!(selected(&tab), ["b.txt"]);
    assert_eq!(tab.cursor(), Some(2));
}

#[test]
fn 상태_표시줄은_항목_수와_선택_수와_파일_크기_합이다() {
    let dir = basic();
    let mut tab = tab_at(dir.path());
    assert_eq!(
        tab.status(),
        StatusSummary {
            item_count: 5,
            selected_count: 0,
            selected_size: 0
        }
    );
    tab.select_only(0); // 폴더만: 크기 합에 안 든다.
    assert_eq!(tab.status().selected_count, 1);
    assert_eq!(tab.status().selected_size, 0);
    tab.toggle_select(2);
    tab.toggle_select(4);
    assert_eq!(
        tab.status(),
        StatusSummary {
            item_count: 5,
            selected_count: 3,
            selected_size: 1010
        }
    );
    tab.select_all();
    assert_eq!(tab.status().selected_size, 1111);
}

// --- 글자로 이동 ---

fn type_dir() -> TempDir {
    folder_with(
        &[
            ("Apple.txt", 0),
            ("avocado.txt", 0),
            ("banana.txt", 0),
            ("Berry.txt", 0),
            ("bread.txt", 0),
            ("가방.txt", 0),
            ("가지.txt", 0),
            ("한글.txt", 0),
            ("한라산.txt", 0),
        ],
        &[],
    )
}

#[test]
fn 글자를_치면_그_글자로_시작하는_항목으로_간다() {
    let dir = type_dir();
    let mut tab = tab_at(dir.path());
    let t = Instant::now();
    let at = tab.type_to_find('b', t).unwrap();
    assert_eq!(name_at(&tab, at), "banana.txt");
    assert_eq!(tab.cursor(), Some(at));
    // 선택은 바뀌지 않는다.
    assert_eq!(tab.selected_count(), 0);
    assert_eq!(tab.type_to_find('q', t + TYPE_AHEAD_TIMEOUT * 2), None);
    assert_eq!(tab.cursor(), Some(at));
}

#[test]
fn 이어_친_글자는_접두어로_이어_붙인다() {
    let dir = type_dir();
    let mut tab = tab_at(dir.path());
    let t = Instant::now();
    tab.type_to_find('b', t);
    let at = tab
        .type_to_find('E', t + Duration::from_millis(400))
        .unwrap();
    assert_eq!(name_at(&tab, at), "Berry.txt");
    let at = tab
        .type_to_find('r', t + Duration::from_millis(800))
        .unwrap();
    assert_eq!(name_at(&tab, at), "Berry.txt");
    // 접두어에 맞는 항목이 없으면 커서는 그대로다.
    assert_eq!(tab.type_to_find('z', t + Duration::from_millis(1200)), None);
    assert_eq!(name_at(&tab, tab.cursor().unwrap()), "Berry.txt");
}

#[test]
fn 일_초가_지나면_새로_시작한다() {
    let dir = type_dir();
    let mut tab = tab_at(dir.path());
    let t = Instant::now();
    tab.type_to_find('b', t);
    // 정확히 1초는 이어 친 것이다.
    let at = tab.type_to_find('r', t + TYPE_AHEAD_TIMEOUT).unwrap();
    assert_eq!(name_at(&tab, at), "bread.txt");
    // 1초를 넘기면 "br"이 아니라 "a"로 새로 시작한다.
    let at = tab.type_to_find('a', t + TYPE_AHEAD_TIMEOUT * 3).unwrap();
    assert_eq!(name_at(&tab, at), "Apple.txt");
}

#[test]
fn 같은_글자를_거듭_치면_차례로_돈다() {
    let dir = type_dir();
    let mut tab = tab_at(dir.path());
    let t = Instant::now();
    let mut visited = Vec::new();
    for i in 0..4u32 {
        let at = tab
            .type_to_find('b', t + Duration::from_millis(100 * u64::from(i)))
            .unwrap();
        visited.push(name_at(&tab, at));
    }
    assert_eq!(
        visited,
        ["banana.txt", "Berry.txt", "bread.txt", "banana.txt"]
    );
}

#[test]
fn 한글은_조합이_끝난_글자로_찾는다() {
    let dir = type_dir();
    let mut tab = tab_at(dir.path());
    let t = Instant::now();
    let at = tab.type_to_find('한', t).unwrap();
    assert_eq!(name_at(&tab, at), "한글.txt");
    let at = tab
        .type_to_find('라', t + Duration::from_millis(300))
        .unwrap();
    assert_eq!(name_at(&tab, at), "한라산.txt");
    let at = tab.type_to_find('가', t + TYPE_AHEAD_TIMEOUT * 5).unwrap();
    assert_eq!(name_at(&tab, at), "가방.txt");
    let at = tab
        .type_to_find(
            '가',
            t + TYPE_AHEAD_TIMEOUT * 5 + Duration::from_millis(100),
        )
        .unwrap();
    assert_eq!(name_at(&tab, at), "가지.txt");
}

// --- 큰 목록 ---

#[test]
fn 큰_목록에서도_모두_선택과_정렬_뒤_유지가_빠르다() {
    // 이름 찾기가 항목 수의 제곱이면 이 크기에서 수 초가 걸린다.
    const COUNT: usize = 20_000;
    let dir = TempDir::new().unwrap();
    for i in 0..COUNT {
        fs::write(dir.path().join(format!("항목 {i:06}.txt")), b"").unwrap();
    }
    let mut tab = tab_at(dir.path());
    assert_eq!(tab.len(), COUNT);

    let started = Instant::now();
    tab.select_all();
    tab.select_only(COUNT / 2);
    tab.select_range(COUNT - 1);
    tab.select_all();
    tab.set_sort(Sort {
        column: Column::Name,
        direction: Direction::Descending,
    });
    go(&mut tab, Navigation::Refresh);
    let status = tab.status();
    let elapsed = started.elapsed();

    assert_eq!(status.selected_count, COUNT);
    assert_eq!(status.item_count, COUNT);
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
}

// --- 수식 키에 따른 선택(앱이 클릭·키 입력을 그대로 넘긴다) ---

use facet_core::selection::Pick;

#[test]
fn 클릭의_수식_키() {
    assert_eq!(Pick::for_click(false, false), Pick::Only);
    assert_eq!(Pick::for_click(true, false), Pick::Toggle);
    assert_eq!(Pick::for_click(false, true), Pick::Range);
    // Shift가 Ctrl보다 앞선다.
    assert_eq!(Pick::for_click(true, true), Pick::Range);
}

#[test]
fn 커서_키의_수식_키() {
    assert_eq!(Pick::for_cursor_key(false, false), Pick::Only);
    assert_eq!(Pick::for_cursor_key(true, false), Pick::CursorOnly);
    assert_eq!(Pick::for_cursor_key(false, true), Pick::Range);
    assert_eq!(Pick::for_cursor_key(true, true), Pick::Range);
}

#[test]
fn 클릭은_수식_키대로_고른다() {
    let dir = basic();
    let mut tab = tab_at(dir.path());
    tab.pick(1, Pick::for_click(false, false));
    tab.pick(3, Pick::for_click(false, true));
    assert_eq!(selected(&tab), ["a.txt", "b.txt", "c.txt"]);
    tab.pick(2, Pick::for_click(true, false));
    assert_eq!(selected(&tab), ["a.txt", "c.txt"]);
    assert_eq!(tab.cursor(), Some(2));
    tab.pick(0, Pick::CursorOnly);
    assert_eq!(selected(&tab), ["a.txt", "c.txt"]);
    assert_eq!(tab.cursor(), Some(0));
    // 범위 밖은 아무것도 하지 않는다.
    tab.pick(99, Pick::Only);
    assert_eq!(selected(&tab), ["a.txt", "c.txt"]);
}

#[test]
fn 화살표는_커서를_옮기며_고른다() {
    let dir = basic();
    let mut tab = tab_at(dir.path());
    assert_eq!(
        tab.move_cursor_picking(CursorMove::Down, Pick::Only),
        Some(0)
    );
    assert_eq!(selected(&tab), ["d"]);
    assert_eq!(
        tab.move_cursor_picking(CursorMove::Down, Pick::Only),
        Some(1)
    );
    assert_eq!(selected(&tab), ["a.txt"]);
    // Ctrl: 선택은 그대로 두고 커서만.
    assert_eq!(
        tab.move_cursor_picking(CursorMove::Down, Pick::CursorOnly),
        Some(2)
    );
    assert_eq!(selected(&tab), ["a.txt"]);
    // Shift: 기준(a.txt)부터 범위.
    assert_eq!(
        tab.move_cursor_picking(CursorMove::Down, Pick::Range),
        Some(3)
    );
    assert_eq!(selected(&tab), ["a.txt", "b.txt", "c.txt"]);
    assert_eq!(
        tab.move_cursor_picking(CursorMove::Last, Pick::Only),
        Some(4)
    );
    assert_eq!(selected(&tab), ["e.txt"]);
}

#[test]
fn 빈_목록에서_화살표는_아무것도_하지_않는다() {
    let dir = folder_with(&[], &[]);
    let mut tab = tab_at(dir.path());
    assert_eq!(tab.move_cursor_picking(CursorMove::Down, Pick::Only), None);
    assert_eq!(tab.selected_count(), 0);
}

#[test]
fn 글자로_이동하면_그_항목을_고른다() {
    let dir = folder_with(&[("가나.txt", 1), ("나비.txt", 1), ("다리.txt", 1)], &[]);
    let mut tab = tab_at(dir.path());
    let now = Instant::now();
    assert_eq!(tab.type_to_select('나', now), Some(1));
    assert_eq!(selected(&tab), ["나비.txt"]);
    // 맞는 항목이 없으면 선택도 커서도 그대로다.
    let later = now + TYPE_AHEAD_TIMEOUT + Duration::from_millis(1);
    assert_eq!(tab.type_to_select('z', later), None);
    assert_eq!(selected(&tab), ["나비.txt"]);
    assert_eq!(tab.cursor(), Some(1));
}

// 사각형 선택(빈 곳에서 시작한 끌기). 화면은 사각형이 걸친 행 범위만 넘긴다. 정렬하면 d, a, b, c, e.

#[test]
fn 사각형_선택은_걸친_행으로_선택을_바꾼다() {
    let dir = basic();
    let mut tab = tab_at(dir.path());
    tab.select_only(4);
    tab.begin_marquee(false);
    assert!(tab.is_marquee_active());
    tab.marquee_select(Some((1, 2)));
    assert_eq!(selected(&tab), ["a.txt", "b.txt"]);
    // 끄는 동안 사각형이 줄면 빠진 행은 선택에서 빠진다.
    tab.marquee_select(Some((1, 1)));
    assert_eq!(selected(&tab), ["a.txt"]);
    // 위로 끌어도(끄는 점 쪽이 앞) 같은 범위다.
    tab.marquee_select(Some((3, 1)));
    assert_eq!(selected(&tab), ["a.txt", "b.txt", "c.txt"]);
    tab.end_marquee();
    assert!(!tab.is_marquee_active());
    assert_eq!(selected(&tab), ["a.txt", "b.txt", "c.txt"]);
}

#[test]
fn 사각형_선택의_커서와_기준은_끄는_점_쪽_끝_행이다() {
    let dir = basic();
    let mut tab = tab_at(dir.path());
    tab.begin_marquee(false);
    tab.marquee_select(Some((1, 3)));
    tab.end_marquee();
    assert_eq!(tab.cursor(), Some(3));
    // 기준이 거기 있어 Shift 클릭은 그 행부터다.
    tab.select_range(4);
    assert_eq!(selected(&tab), ["c.txt", "e.txt"]);

    tab.begin_marquee(false);
    tab.marquee_select(Some((4, 2)));
    assert_eq!(tab.cursor(), Some(2));
}

#[test]
fn 걸친_행이_없으면_선택이_비고_커서는_그대로다() {
    let dir = basic();
    let mut tab = tab_at(dir.path());
    tab.select_only(2);
    tab.begin_marquee(false);
    tab.marquee_select(None);
    assert!(selected(&tab).is_empty());
    assert_eq!(tab.cursor(), Some(2));
}

#[test]
fn 목록_밖_번호는_끝_행으로_자른다() {
    let dir = basic();
    let mut tab = tab_at(dir.path());
    tab.begin_marquee(false);
    tab.marquee_select(Some((3, 99)));
    assert_eq!(selected(&tab), ["c.txt", "e.txt"]);
    assert_eq!(tab.cursor(), Some(4));
}

#[test]
fn ctrl_사각형_선택은_끌기_전_선택에_더한다() {
    let dir = basic();
    let mut tab = tab_at(dir.path());
    tab.select_only(0);
    tab.toggle_select(4);
    tab.begin_marquee(true);
    tab.marquee_select(Some((1, 2)));
    assert_eq!(selected(&tab), ["d", "a.txt", "b.txt", "e.txt"]);
    // 사각형이 줄어도 끌기 전 선택은 남는다.
    tab.marquee_select(Some((2, 2)));
    assert_eq!(selected(&tab), ["d", "b.txt", "e.txt"]);
    // 끌기 전에 고른 행을 사각형이 지나가도 그대로 선택이다(뒤집지 않는다).
    tab.marquee_select(Some((3, 4)));
    assert_eq!(selected(&tab), ["d", "c.txt", "e.txt"]);
    tab.marquee_select(None);
    assert_eq!(selected(&tab), ["d", "e.txt"]);
}

#[test]
fn 사각형_선택은_정렬을_바꿔도_이름으로_유지된다() {
    let dir = basic();
    let mut tab = tab_at(dir.path());
    tab.select_only(4);
    tab.begin_marquee(true);
    tab.marquee_select(Some((1, 2)));
    // 끄는 중에 정렬이 바뀌어도 이미 고른 이름과 끌기 전 선택은 그대로다.
    tab.set_sort(Sort {
        column: Column::Name,
        direction: Direction::Descending,
    });
    // 내림차순: d, e, c, b, a.
    assert_eq!(selected(&tab), ["e.txt", "b.txt", "a.txt"]);
    tab.marquee_select(Some((2, 2)));
    assert_eq!(selected(&tab), ["e.txt", "c.txt"]);
    tab.end_marquee();
    tab.set_sort(Sort::default());
    assert_eq!(selected(&tab), ["c.txt", "e.txt"]);
    assert_eq!(tab.cursor_item().unwrap().name, "c.txt");
}

#[test]
fn 사각형_선택은_새로_고쳐도_이름으로_유지되고_사라진_항목은_빠진다() {
    let dir = basic();
    let mut tab = tab_at(dir.path());
    tab.select_only(4);
    tab.begin_marquee(true);
    tab.marquee_select(Some((1, 2)));
    fs::remove_file(dir.path().join("e.txt")).unwrap();
    fs::write(dir.path().join("0.txt"), b"0").unwrap();
    go(&mut tab, Navigation::Refresh);
    // d, 0, a, b, c: 행 번호가 밀려도 이름으로 남고, 사라진 e는 끌기 전 선택에서도 빠진다.
    assert_eq!(selected(&tab), ["a.txt", "b.txt"]);
    tab.marquee_select(Some((4, 4)));
    assert_eq!(selected(&tab), ["c.txt"]);
    assert_eq!(tab.selected_count(), 1);
    tab.end_marquee();
    go(&mut tab, Navigation::Refresh);
    assert_eq!(selected(&tab), ["c.txt"]);
}

#[test]
fn 다른_위치로_가면_사각형_선택이_끝난다() {
    let dir = folder_with(&[("a.txt", 1)], &["sub"]);
    let mut tab = tab_at(dir.path());
    tab.begin_marquee(true);
    tab.marquee_select(Some((0, 1)));
    go(
        &mut tab,
        Navigation::Go(Location::file_system(dir.path().join("sub"))),
    );
    assert!(!tab.is_marquee_active());
}
