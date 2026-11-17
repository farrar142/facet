//! 주소창 경로 조각 사이 › 펼침 메뉴의 화면 상태(#306): 어느 조각의 메뉴인가, 목록을 기다리는 중인가·받았는가·
//! 읽지 못했는가, 고른 줄, 보이게 넘기기, 창 밖으로 넘치지 않는 자리.
//!
//! 하위 폴더 목록은 코어가 일꾼 스레드에서 읽는다([`facet_core::window::App::request_subfolders`]). 여기서는 받은
//! 결과가 지금 기다리는 요청의 것인지 가리고, ↑↓·Enter를 줄 고르기로 바꾼다. 줄 높이는 오른쪽 메뉴와 같고
//! (`context_menu::ROW_HEIGHT`) Slint도 같은 값으로 그린다(`app.slint`의 `CrumbMenuView`). 화면 문장은 `text.rs`.

use facet_core::Candidate;
use facet_core::Location;
use facet_core::file_list::scroll_to_show;
use facet_core::window::{Subfolders, SubfoldersError, SubfoldersId};

pub use crate::context_menu::{PADDING, ROW_HEIGHT};

/// 메뉴 너비.
pub const WIDTH: f32 = 260.0;
/// 한 번에 보이는 가장 많은 줄 수. 더 길면 넘겨서 본다(`ListView`가 보이는 줄만 그린다).
pub const MAX_VISIBLE_ROWS: usize = 14;
/// › 단추의 너비(Slint의 `Segment` 뒤 단추와 같다).
pub const ARROW_WIDTH: f32 = 16.0;

/// 메뉴가 보이는 내용.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    /// 결과를 기다리는 중(「읽는 중」 줄 하나).
    Loading,
    /// 읽지 못했다(까닭 줄 하나).
    Failed(SubfoldersError),
    /// 하위 폴더가 없다(빈 목록 줄 하나).
    Empty,
    /// 하위 폴더들. `current`는 「다음 조각」 폴더의 번호(굵게).
    Folders {
        folders: Vec<Candidate>,
        current: Option<usize>,
    },
}

/// 열려 있는 › 메뉴 하나.
#[derive(Debug, Clone)]
pub struct CrumbMenu {
    /// 메뉴를 연 조각이 있는 패널과 조각 번호.
    panel: usize,
    segment: usize,
    /// 기다리는 요청. 이 요청의 결과만 받는다.
    waiting: Option<SubfoldersId>,
    content: Content,
    selected: Option<usize>,
}

impl CrumbMenu {
    /// 패널 `panel`의 `segment`번 조각 뒤 › 메뉴를 연다. 요청 `id`의 결과를 기다리는 「읽는 중」 상태다.
    pub fn loading(panel: usize, segment: usize, id: SubfoldersId) -> CrumbMenu {
        CrumbMenu {
            panel,
            segment,
            waiting: Some(id),
            content: Content::Loading,
            selected: None,
        }
    }

    pub fn panel(&self) -> usize {
        self.panel
    }

    pub fn segment(&self) -> usize {
        self.segment
    }

    pub fn content(&self) -> &Content {
        &self.content
    }

    /// 고른 줄(하위 폴더 번호). 폴더 줄이 아닌 상태에서는 늘 `None`이다.
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// 결과를 받는다. 기다리는 요청의 결과면 내용을 바꾸고 `true`다. 늦게 온 앞선 요청이나 코어가 버린 결과(`None`)
    /// 면 메뉴를 그대로 두고 `false`다.
    pub fn receive(
        &mut self,
        id: SubfoldersId,
        list: Option<Result<Subfolders, SubfoldersError>>,
    ) -> bool {
        if self.waiting != Some(id) {
            return false;
        }
        let Some(list) = list else {
            return false;
        };
        self.waiting = None;
        self.selected = None;
        self.content = match list {
            Err(error) => Content::Failed(error),
            Ok(list) if list.folders.is_empty() => Content::Empty,
            Ok(list) => Content::Folders {
                folders: list.folders,
                current: list.current,
            },
        };
        true
    }

    /// 보일 줄 수(폴더 줄 또는 안내 줄 하나).
    pub fn row_count(&self) -> usize {
        match &self.content {
            Content::Folders { folders, .. } => folders.len(),
            _ => 1,
        }
    }

    /// `index`번 줄의 폴더 이름. 안내 줄이면 `None`이다.
    pub fn folder(&self, index: usize) -> Option<&Candidate> {
        match &self.content {
            Content::Folders { folders, .. } => folders.get(index),
            _ => None,
        }
    }

    /// `index`번 줄이 「다음 조각」 폴더(굵게)인가.
    pub fn is_current(&self, index: usize) -> bool {
        matches!(&self.content, Content::Folders { current: Some(c), .. } if *c == index)
    }

    /// 메뉴가 보이는 줄 수.
    pub fn visible_rows(&self) -> usize {
        self.row_count().min(MAX_VISIBLE_ROWS)
    }

    /// 메뉴 전체의 높이.
    pub fn height(&self) -> f32 {
        2.0 * PADDING + self.visible_rows() as f32 * ROW_HEIGHT
    }

    /// 줄이 보이는 영역의 높이(위아래 여백 뺀 것).
    pub fn view_height(&self) -> f32 {
        self.visible_rows() as f32 * ROW_HEIGHT
    }

    /// ↓(`delta` 1)·↑(-1): 다음·앞 폴더 줄을 고른다. 끝에서는 반대쪽 끝으로 돈다. 처음이면 ↓는 첫 줄, ↑는 마지막
    /// 줄이다. 폴더 줄이 없는 상태(읽는 중·까닭·빈 목록)에서는 아무 일도 없다.
    pub fn step(&mut self, delta: i32) {
        let len = match &self.content {
            Content::Folders { folders, .. } => folders.len(),
            _ => return,
        };
        let next = match (self.selected, delta >= 0) {
            (None, true) => 0,
            (None, false) => len - 1,
            (Some(i), true) => (i + 1) % len,
            (Some(i), false) => (i + len - 1) % len,
        };
        self.selected = Some(next);
    }

    /// 마우스가 `index`번 줄 위로 왔다. 폴더 줄이면 고른다.
    pub fn hover(&mut self, index: usize) {
        if index < self.folders_len() {
            self.selected = Some(index);
        }
    }

    fn folders_len(&self) -> usize {
        match &self.content {
            Content::Folders { folders, .. } => folders.len(),
            _ => 0,
        }
    }

    /// 고른 줄이 보이게 하는 새 스크롤 위치(목록 맨 위에서 보이는 맨 위까지의 거리, 0 이상). 고른 줄이 없으면
    /// `offset`을 그대로 돌려준다.
    pub fn scroll_to_selected(&self, offset: f32) -> f32 {
        match self.selected {
            Some(i) => scroll_to_show(i, ROW_HEIGHT, offset, self.view_height()),
            None => offset,
        }
    }

    /// 「다음 조각」 폴더가 보이게 하는 처음 스크롤 위치(없으면 0).
    pub fn initial_scroll(&self) -> f32 {
        match &self.content {
            Content::Folders {
                current: Some(c), ..
            } => scroll_to_show(*c, ROW_HEIGHT, 0.0, self.view_height()),
            _ => 0.0,
        }
    }

    /// `index`번 줄을 눌렀다(또는 Enter): 폴더 줄이면 그 위치. 안내 줄은 아무 일도 없다.
    pub fn activate(&self, index: usize) -> Option<Location> {
        self.folder(index).map(|c| c.location.clone())
    }

    /// Enter: 고른 줄의 위치.
    pub fn enter(&self) -> Option<Location> {
        self.selected.and_then(|i| self.activate(i))
    }
}

/// 메뉴를 놓는다: 조각 뒤 › 단추의 왼쪽 아래(`anchor`, 창 기준)에 `size` 크기로. 오른쪽·아래로 넘치면 안쪽으로 옮긴다
/// (`bounds`는 창 크기). 창보다 크면 왼쪽·위에 붙는다.
pub fn place(anchor: (f32, f32), size: (f32, f32), bounds: (f32, f32)) -> (f32, f32) {
    crate::context_menu::place(anchor, size, bounds)
}

/// 점 `(x, y)`가 `rect`(`left, top, width, height`) 안인가.
pub fn contains(rect: (f32, f32, f32, f32), x: f32, y: f32) -> bool {
    x >= rect.0 && x < rect.0 + rect.2 && y >= rect.1 && y < rect.1 + rect.3
}

#[cfg(test)]
mod tests {
    use super::*;
    use facet_core::window::App;

    /// 요청 번호는 코어만 만든다: 활성 탭이 실제 폴더인 앱에서 요청해 번호를 얻는다.
    fn ids(n: usize) -> Vec<SubfoldersId> {
        let dir = std::env::temp_dir();
        let mut app = App::new(Some(Location::file_system(&dir)));
        (0..n)
            .map(|_| {
                app.request_subfolders(&Location::file_system(&dir))
                    .unwrap()
            })
            .collect()
    }

    fn folders(names: &[&str]) -> Vec<Candidate> {
        names
            .iter()
            .map(|n| Candidate {
                name: (*n).into(),
                location: Location::file_system(format!(r"C:\x\{n}")),
            })
            .collect()
    }

    fn list(names: &[&str], current: Option<usize>) -> Subfolders {
        Subfolders {
            folders: folders(names),
            current,
            home: None,
        }
    }

    fn name(location: Option<Location>) -> Option<String> {
        location.and_then(|l| {
            l.file_system_path()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
        })
    }

    #[test]
    fn 기다리는_동안은_읽는_중_한_줄이고_고를_줄이_없다() {
        let id = ids(1);
        let mut menu = CrumbMenu::loading(0, 2, id[0]);
        assert_eq!(menu.content(), &Content::Loading);
        assert_eq!(menu.row_count(), 1);
        menu.step(1);
        menu.step(-1);
        menu.hover(0);
        assert_eq!(menu.selected(), None);
        assert_eq!(menu.enter(), None);
        assert_eq!(menu.activate(0), None);
        assert_eq!(menu.height(), 2.0 * PADDING + ROW_HEIGHT);
        assert_eq!((menu.panel(), menu.segment()), (0, 2));
    }

    #[test]
    fn 기다리는_요청의_결과만_받고_늦은_결과는_메뉴를_바꾸지_않는다() {
        let id = ids(2);
        let mut menu = CrumbMenu::loading(0, 0, id[1]);
        assert!(
            !menu.receive(id[0], Some(Ok(list(&["a"], None)))),
            "앞선 요청"
        );
        assert!(!menu.receive(id[1], None), "코어가 버린 결과");
        assert_eq!(menu.content(), &Content::Loading);
        assert!(menu.receive(id[1], Some(Ok(list(&["a", "b"], Some(1))))));
        assert_eq!(menu.row_count(), 2);
        assert!(menu.is_current(1) && !menu.is_current(0));
        assert!(
            !menu.receive(id[1], Some(Ok(list(&["z"], None)))),
            "이미 받았으면 다시 받지 않는다"
        );
        assert_eq!(menu.row_count(), 2);
    }

    #[test]
    fn 읽지_못했거나_비었으면_안내_줄_하나다() {
        let id = ids(2);
        let mut menu = CrumbMenu::loading(0, 0, id[0]);
        menu.receive(id[0], Some(Err(SubfoldersError::PermissionDenied)));
        assert_eq!(
            menu.content(),
            &Content::Failed(SubfoldersError::PermissionDenied)
        );
        assert_eq!(menu.row_count(), 1);
        menu.step(1);
        assert_eq!(menu.selected(), None);
        assert_eq!(menu.enter(), None);

        let mut menu = CrumbMenu::loading(0, 0, id[1]);
        menu.receive(id[1], Some(Ok(list(&[], None))));
        assert_eq!(menu.content(), &Content::Empty);
        assert_eq!(menu.row_count(), 1);
        menu.step(-1);
        assert_eq!(menu.selected(), None);
        assert_eq!(menu.enter(), None);
    }

    #[test]
    fn 화살표로_돌며_고르고_enter가_그_폴더로_간다() {
        let id = ids(1);
        let mut menu = CrumbMenu::loading(0, 0, id[0]);
        menu.receive(id[0], Some(Ok(list(&["a", "b", "c"], None))));
        assert_eq!(menu.enter(), None, "고르기 전");
        menu.step(1);
        assert_eq!(menu.selected(), Some(0));
        menu.step(-1);
        assert_eq!(menu.selected(), Some(2), "처음에서 위로 가면 끝으로 돈다");
        menu.step(1);
        assert_eq!(menu.selected(), Some(0), "끝에서 아래로 가면 처음으로 돈다");
        menu.hover(1);
        assert_eq!(name(menu.enter()).as_deref(), Some("b"));
        assert_eq!(name(menu.activate(2)).as_deref(), Some("c"));
        menu.hover(9);
        assert_eq!(menu.selected(), Some(1), "없는 줄 위로는 고르지 않는다");

        let mut menu = CrumbMenu::loading(0, 0, id[0]);
        menu.receive(id[0], Some(Ok(list(&["a", "b"], None))));
        menu.step(-1);
        assert_eq!(menu.selected(), Some(1), "처음에 ↑면 마지막 줄");
    }

    #[test]
    fn 긴_목록은_보이는_줄만_높이에_넣고_고른_줄이_보이게_넘긴다() {
        let id = ids(1);
        let names: Vec<String> = (0..1000).map(|i| format!("f{i}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut menu = CrumbMenu::loading(0, 0, id[0]);
        menu.receive(id[0], Some(Ok(list(&refs, Some(500)))));
        assert_eq!(menu.visible_rows(), MAX_VISIBLE_ROWS);
        assert_eq!(
            menu.height(),
            2.0 * PADDING + MAX_VISIBLE_ROWS as f32 * ROW_HEIGHT
        );
        // 굵은 줄(500번)이 보이는 처음 위치.
        let start = menu.initial_scroll();
        assert_eq!(start, 501.0 * ROW_HEIGHT - menu.view_height());
        // 아래로 벗어난 줄을 고르면 그 줄이 맨 아래에 온다.
        menu.hover(520);
        assert_eq!(
            menu.scroll_to_selected(start),
            521.0 * ROW_HEIGHT - menu.view_height()
        );
        // 이미 보이면 그대로, 위로 벗어났으면 맨 위.
        menu.hover(495);
        assert_eq!(menu.scroll_to_selected(start), start);
        menu.hover(10);
        assert_eq!(menu.scroll_to_selected(start), 10.0 * ROW_HEIGHT);
        // 끝에서 아래로 돌면 첫 줄이 보이게 맨 위로 간다.
        menu.hover(999);
        menu.step(1);
        assert_eq!(menu.selected(), Some(0));
        assert_eq!(menu.scroll_to_selected(900.0 * ROW_HEIGHT), 0.0);
    }

    #[test]
    fn 짧은_목록은_줄_수만큼_높고_넘기지_않는다() {
        let id = ids(1);
        let mut menu = CrumbMenu::loading(0, 0, id[0]);
        menu.receive(id[0], Some(Ok(list(&["a", "b", "c"], None))));
        assert_eq!(menu.height(), 2.0 * PADDING + 3.0 * ROW_HEIGHT);
        menu.step(1);
        menu.step(1);
        assert_eq!(menu.scroll_to_selected(0.0), 0.0);
        assert_eq!(menu.initial_scroll(), 0.0);
    }

    #[test]
    fn 창_밖으로_넘치지_않게_놓는다() {
        let size = (WIDTH, 300.0);
        let bounds = (1000.0, 700.0);
        assert_eq!(place((100.0, 100.0), size, bounds), (100.0, 100.0));
        assert_eq!(
            place((900.0, 100.0), size, bounds),
            (1000.0 - WIDTH, 100.0),
            "오른쪽으로 넘치면 안쪽으로"
        );
        assert_eq!(place((100.0, 600.0), size, bounds), (100.0, 400.0));
        assert_eq!(place((50.0, 50.0), (5000.0, 5000.0), bounds), (0.0, 0.0));
    }

    #[test]
    fn 점이_단추_안인지_가른다() {
        let rect = (10.0, 20.0, ARROW_WIDTH, 24.0);
        assert!(contains(rect, 10.0, 20.0));
        assert!(contains(rect, 25.9, 43.9));
        assert!(!contains(rect, 26.0, 30.0));
        assert!(!contains(rect, 15.0, 44.0));
        assert!(!contains(rect, 9.9, 30.0));
    }
}
