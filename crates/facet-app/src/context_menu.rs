//! 오른쪽 메뉴의 화면 상태: 코어가 만든 **자체 항목**([`facet_core::menu::Menu`])과 그 아래 구분선 뒤의 **셸 항목**
//! 자리(코어 [`ShellMenu`], #322), 고른 줄, 열린 하위 메뉴, 놓일 자리. 무엇이 들어 있고 켜졌는지는 코어가 정한다.
//! 이 모듈은 ↑↓·→·←·Enter를 줄 고르기로 바꾸고, 창 밖으로 넘치지 않게 자리를 정한다(Slint도 같은 줄 높이·너비로
//! 그린다: `app.slint`의 `ContextMenuView`). 셸 항목 자리는 자체 항목 뒤에만 붙고 바뀌므로 자체 항목의 줄 번호와
//! 자리는 셸 응답이 와도 그대로다.

use facet_core::command::CommandId;
use facet_core::menu::{Menu, MenuCommand, MenuEntry};
use facet_core::shell_helper::{CrashedExtension, ShellMenu, ShellMenuId, ShellSection};
use facet_shell_protocol::{MenuItem, ShellCommand};

/// 명령 줄의 높이.
pub const ROW_HEIGHT: f32 = 28.0;
/// 구분선 줄의 높이.
pub const SEPARATOR_HEIGHT: f32 = 9.0;
/// 메뉴 위아래 안쪽 여백.
pub const PADDING: f32 = 4.0;
/// 메뉴 너비.
pub const WIDTH: f32 = 300.0;
/// 하위 메뉴 너비.
pub const SUB_WIDTH: f32 = 200.0;
/// 셸 하위 메뉴 너비(셸 항목 글자는 자체 항목보다 길다).
pub const SHELL_SUB_WIDTH: f32 = 280.0;

/// 메뉴 안에서 한 일.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// 아무 일도 없다(고른 줄이나 열린 하위 메뉴만 바뀌었을 수 있다).
    None,
    /// 이 명령을 실행하고 메뉴를 닫는다.
    Run(CommandId),
    /// 이 번호의 셸 항목을 도우미가 실행하게 하고 메뉴를 닫는다.
    Shell(u32),
}

/// 셸 항목 자리의 안내 줄(고를 수 없음). 글자는 `text.rs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// 도우미의 응답을 기다린다.
    Loading,
    /// 대기 시간을 넘겼거나 도우미가 죽었다.
    Failed,
    /// 도우미가 연달아 죽어 쉬는 중이다.
    Resting,
    /// 같은 확장으로 여러 번 죽었다: 그 확장 이름.
    Crashing(String),
}

/// 메뉴의 한 줄.
#[derive(Debug, Clone)]
pub enum Line {
    /// 자체 항목 줄(코어 메뉴 그대로).
    Own(MenuEntry),
    /// 자체 항목과 셸 항목 자리 사이의 구분선.
    ShellSeparator,
    /// 셸 항목 자리의 안내 줄.
    Notice(Notice),
    /// 셸 항목(명령·하위 메뉴·구분선).
    Shell(MenuItem),
}

/// 하위 메뉴의 한 줄.
#[derive(Debug, Clone)]
pub enum SubLine {
    /// 자체 하위 메뉴의 명령.
    Own(MenuCommand),
    /// 셸 하위 메뉴의 명령.
    Shell(ShellCommand),
    Separator,
}

/// 열려 있는 오른쪽 메뉴.
#[derive(Debug, Clone)]
pub struct ContextMenu {
    lines: Vec<Line>,
    /// 자체 항목 줄 수(셸 자리는 그 뒤).
    own: usize,
    /// 붙은 셸 메뉴의 번호.
    shell: Option<ShellMenuId>,
    /// 고른 줄(줄 번호).
    selected: Option<usize>,
    /// 열린 하위 메뉴가 있으면 그 줄(줄 번호)과 고른 하위 줄.
    sub: Option<(usize, Option<usize>)>,
}

/// 줄 하나가 고를 수 있는 줄인가.
fn selectable(line: &Line) -> bool {
    match line {
        Line::Own(MenuEntry::Command(c)) => c.enabled,
        Line::Own(MenuEntry::Submenu(s)) => s.enabled,
        Line::Shell(MenuItem::Command(c)) => c.enabled,
        Line::Shell(MenuItem::Submenu(s)) => s.enabled,
        _ => false,
    }
}

/// 줄 하나가 하위 메뉴 줄인가.
fn is_submenu(line: &Line) -> bool {
    matches!(
        line,
        Line::Own(MenuEntry::Submenu(_)) | Line::Shell(MenuItem::Submenu(_))
    )
}

/// 줄 하나가 구분선인가(높이가 다르다).
pub fn is_separator(line: &Line) -> bool {
    matches!(
        line,
        Line::Own(MenuEntry::Separator) | Line::ShellSeparator | Line::Shell(MenuItem::Separator)
    )
}

/// 셸 하위 메뉴의 줄들을 한 단계로 편다: 하위 메뉴 안의 하위 메뉴는 앞뒤 구분선과 함께 그 자리에 풀어 놓는다(화면은
/// 하위 메뉴를 한 단계만 연다). 꺼진 안쪽 하위 메뉴의 명령은 꺼진 채로 둔다.
fn flatten(items: &[MenuItem], enabled: bool, out: &mut Vec<SubLine>) {
    for item in items {
        match item {
            MenuItem::Command(c) => {
                let mut c = c.clone();
                c.enabled &= enabled;
                out.push(SubLine::Shell(c));
            }
            MenuItem::Separator => out.push(SubLine::Separator),
            MenuItem::Submenu(s) => {
                out.push(SubLine::Separator);
                flatten(&s.items, enabled && s.enabled, out);
                out.push(SubLine::Separator);
            }
        }
    }
}

/// 겹친 구분선과 양 끝 구분선을 걷는다.
fn tidy_sub(lines: Vec<SubLine>) -> Vec<SubLine> {
    let mut out: Vec<SubLine> = Vec::with_capacity(lines.len());
    for line in lines {
        let separator = matches!(line, SubLine::Separator);
        if separator && out.last().is_none_or(|l| matches!(l, SubLine::Separator)) {
            continue;
        }
        out.push(line);
    }
    if matches!(out.last(), Some(SubLine::Separator)) {
        out.pop();
    }
    out
}

/// 셸 항목 글자의 메뉴 가속 표지를 걷는다: `&열기` → `열기`, `&&` → `&`(Win32 메뉴 글자의 약속).
pub fn shell_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&' {
            if chars.peek() == Some(&'&') {
                chars.next();
                out.push('&');
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// 죽게 만든 확장을 부를 이름: 확장 이름, 없으면 DLL 파일 이름, 그것도 없으면 CLSID.
pub fn extension_name(ext: &CrashedExtension) -> String {
    ext.name
        .clone()
        .filter(|name| !name.is_empty())
        .or_else(|| {
            ext.dll
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| ext.clsid.clone())
}

impl ContextMenu {
    pub fn new(menu: Menu) -> ContextMenu {
        let lines: Vec<Line> = menu.entries.into_iter().map(Line::Own).collect();
        ContextMenu {
            own: lines.len(),
            lines,
            shell: None,
            selected: None,
            sub: None,
        }
    }

    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    /// 붙은 셸 메뉴의 번호.
    pub fn shell_id(&self) -> Option<ShellMenuId> {
        self.shell
    }

    /// 셸 항목 자리를 코어의 셸 메뉴 `shell`로 바꾼다(`None`이면 셸 자리가 없다). 자체 항목 줄은 그대로이고, 셸 자리에
    /// 있던 고른 줄·열린 하위 메뉴는 풀린다.
    pub fn set_shell(&mut self, shell: Option<&ShellMenu>) {
        self.lines.truncate(self.own);
        self.shell = shell.map(|menu| menu.id);
        if self.selected.is_some_and(|row| row >= self.own) {
            self.selected = None;
        }
        if self.sub.is_some_and(|(row, _)| row >= self.own) {
            self.sub = None;
        }
        let Some(shell) = shell else {
            return;
        };
        let mut lines = vec![Line::ShellSeparator];
        match &shell.section {
            ShellSection::Loading => lines.push(Line::Notice(Notice::Loading)),
            ShellSection::Failed => lines.push(Line::Notice(Notice::Failed)),
            ShellSection::Resting => lines.push(Line::Notice(Notice::Resting)),
            ShellSection::Items(items) => lines.extend(items.iter().cloned().map(Line::Shell)),
        }
        if let Some(ext) = &shell.crashing_extension {
            lines.push(Line::Notice(Notice::Crashing(extension_name(ext))));
        }
        // 셸 메뉴의 겹친·끝 구분선은 걷는다(자체 항목은 건드리지 않는다).
        for line in lines {
            if is_separator(&line) && self.lines.last().is_none_or(is_separator) {
                continue;
            }
            self.lines.push(line);
        }
        while self.lines.len() > self.own && self.lines.last().is_some_and(is_separator) {
            self.lines.pop();
        }
    }

    /// 고른 줄(줄 번호).
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// 열린 하위 메뉴의 줄 번호.
    pub fn open_submenu(&self) -> Option<usize> {
        self.sub.map(|(row, _)| row)
    }

    /// 고른 하위 줄.
    pub fn sub_selected(&self) -> Option<usize> {
        self.sub.and_then(|(_, selected)| selected)
    }

    /// 줄 `row`가 하위 메뉴면 그 줄들(셸 하위 메뉴는 한 단계로 편 것).
    pub fn sub_lines_of(&self, row: usize) -> Vec<SubLine> {
        match self.lines.get(row) {
            Some(Line::Own(MenuEntry::Submenu(s))) => {
                s.entries.iter().cloned().map(SubLine::Own).collect()
            }
            Some(Line::Shell(MenuItem::Submenu(s))) => {
                let mut out = Vec::new();
                flatten(&s.items, s.enabled, &mut out);
                tidy_sub(out)
            }
            _ => Vec::new(),
        }
    }

    /// 열린 하위 메뉴의 줄들.
    pub fn sub_lines(&self) -> Vec<SubLine> {
        self.sub
            .map(|(row, _)| self.sub_lines_of(row))
            .unwrap_or_default()
    }

    /// 열린 하위 메뉴가 셸 하위 메뉴인가.
    pub fn sub_is_shell(&self) -> bool {
        self.sub
            .is_some_and(|(row, _)| matches!(self.lines.get(row), Some(Line::Shell(_))))
    }

    fn sub_enabled(lines: &[SubLine], i: usize) -> bool {
        match lines.get(i) {
            Some(SubLine::Own(c)) => c.enabled,
            Some(SubLine::Shell(c)) => c.enabled,
            _ => false,
        }
    }

    /// 한 줄 아래(`delta` 1)나 위(-1)의 고를 수 있는 줄로 간다. 하위 메뉴가 열려 있으면 그 안에서, 끝에서는 반대편
    /// 끝으로 돈다. 꺼진 줄·안내 줄·구분선은 건너뛴다.
    pub fn step(&mut self, delta: i32) {
        if let Some((row, selected)) = self.sub {
            let lines = self.sub_lines();
            let next = cycle(lines.len(), selected, delta, |i| {
                Self::sub_enabled(&lines, i)
            });
            self.sub = Some((row, next.or(selected)));
        } else {
            let len = self.lines.len();
            let next = cycle(len, self.selected, delta, |i| selectable(&self.lines[i]));
            self.selected = next.or(self.selected);
        }
    }

    /// 하위 메뉴를 연다: 고른 줄이 하위 메뉴면 열고 첫 줄을 고른다. 열었으면 `true`.
    pub fn open_sub(&mut self) -> bool {
        let Some(row) = self.selected else {
            return false;
        };
        if self.sub.is_some() {
            return false;
        }
        if self
            .lines
            .get(row)
            .is_some_and(|line| is_submenu(line) && selectable(line))
        {
            let lines = self.sub_lines_of(row);
            let first = (0..lines.len()).find(|i| Self::sub_enabled(&lines, *i));
            self.sub = Some((row, first));
            return true;
        }
        false
    }

    /// ←: 하위 메뉴가 열려 있으면 닫는다(그 줄이 고른 줄로 남는다). 닫았으면 `true`.
    pub fn close_sub(&mut self) -> bool {
        self.sub.take().is_some()
    }

    /// 메뉴 줄 위로 마우스가 왔다. 고를 수 없는 줄이면 아무 일도 없다. 하위 메뉴 줄이면 열고, 다른 줄이면 연 하위
    /// 메뉴를 닫는다.
    pub fn hover(&mut self, row: usize) {
        let Some(line) = self.lines.get(row) else {
            return;
        };
        if !selectable(line) {
            return;
        }
        let submenu = is_submenu(line);
        if self.selected != Some(row) {
            self.sub = None;
        }
        self.selected = Some(row);
        if submenu && self.sub.is_none() {
            self.open_sub();
        }
    }

    /// 하위 메뉴 줄 위로 마우스가 왔다.
    pub fn hover_sub(&mut self, i: usize) {
        if let Some((row, _)) = self.sub
            && Self::sub_enabled(&self.sub_lines(), i)
        {
            self.sub = Some((row, Some(i)));
        }
    }

    /// 메뉴 줄을 눌렀다(또는 Enter): 명령이면 실행하고, 하위 메뉴면 연다. 꺼진 줄·안내 줄·구분선은 아무 일도 없다.
    pub fn activate(&mut self, row: usize) -> Outcome {
        match self.lines.get(row) {
            Some(Line::Own(MenuEntry::Command(c))) if c.enabled => Outcome::Run(c.id),
            Some(Line::Shell(MenuItem::Command(c))) if c.enabled => Outcome::Shell(c.id),
            Some(line) if is_submenu(line) && selectable(line) => {
                self.selected = Some(row);
                if self.sub.is_none() {
                    self.open_sub();
                }
                Outcome::None
            }
            _ => Outcome::None,
        }
    }

    /// 하위 메뉴 줄을 눌렀다(또는 Enter).
    pub fn activate_sub(&self, i: usize) -> Outcome {
        match self.sub_lines().get(i) {
            Some(SubLine::Own(c)) if c.enabled => Outcome::Run(c.id),
            Some(SubLine::Shell(c)) if c.enabled => Outcome::Shell(c.id),
            _ => Outcome::None,
        }
    }

    /// Enter: 고른 줄을 누른 것으로 한다.
    pub fn enter(&mut self) -> Outcome {
        if let Some((_, Some(i))) = self.sub {
            return self.activate_sub(i);
        }
        match self.selected {
            Some(row) => self.activate(row),
            None => Outcome::None,
        }
    }

    /// 메뉴 전체의 높이.
    pub fn height(&self) -> f32 {
        height_of(self.lines.iter().map(is_separator))
    }

    /// `row`번 줄의 위쪽 y(메뉴 위쪽 기준).
    pub fn row_top(&self, row: usize) -> f32 {
        PADDING
            + self
                .lines
                .iter()
                .take(row)
                .map(|line| {
                    if is_separator(line) {
                        SEPARATOR_HEIGHT
                    } else {
                        ROW_HEIGHT
                    }
                })
                .sum::<f32>()
    }

    /// 열린 하위 메뉴의 높이. 열려 있지 않으면 0.
    pub fn sub_height(&self) -> f32 {
        if self.sub.is_none() {
            return 0.0;
        }
        height_of(
            self.sub_lines()
                .iter()
                .map(|l| matches!(l, SubLine::Separator)),
        )
    }
}

fn height_of(separators: impl Iterator<Item = bool>) -> f32 {
    2.0 * PADDING
        + separators
            .map(|s| if s { SEPARATOR_HEIGHT } else { ROW_HEIGHT })
            .sum::<f32>()
}

/// `len`개 줄에서 `from`의 다음(`delta`가 양수)·이전 줄 가운데 `ok`인 줄. 끝에서 돈다. 없으면 `None`.
fn cycle(len: usize, from: Option<usize>, delta: i32, ok: impl Fn(usize) -> bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let forward = delta >= 0;
    let start = match (from, forward) {
        (Some(at), _) => at,
        (None, true) => len - 1,
        (None, false) => 0,
    };
    (1..=len)
        .map(|step| {
            if forward {
                (start + step) % len
            } else {
                (start + len - step % len) % len
            }
        })
        .find(|i| ok(*i))
}

/// 점 `at`에 크기 `size`의 메뉴를 놓는다: 오른쪽·아래로 넘치면 안쪽으로 옮긴다(`bounds`는 창 크기). 창보다 크면
/// 왼쪽·위에 붙는다.
pub fn place(at: (f32, f32), size: (f32, f32), bounds: (f32, f32)) -> (f32, f32) {
    let x = at.0.min(bounds.0 - size.0).max(0.0);
    let y = at.1.min(bounds.1 - size.1).max(0.0);
    (x, y)
}

/// 하위 메뉴를 놓는다: 메뉴 오른쪽에 `row_y`(그 줄의 위쪽, 창 기준)부터. 오른쪽에 자리가 없으면 메뉴 왼쪽에 놓고
/// 아래로 넘치면 위로 올린다.
pub fn place_sub(
    menu_x: f32,
    menu_width: f32,
    row_y: f32,
    size: (f32, f32),
    bounds: (f32, f32),
) -> (f32, f32) {
    let right = menu_x + menu_width;
    let x = if right + size.0 <= bounds.0 {
        right
    } else {
        (menu_x - size.0).max(0.0)
    };
    let y = row_y.min(bounds.1 - size.1).max(0.0);
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use facet_core::details::{Column, Direction, Sort};
    use facet_core::menu::{MenuContext, MenuKind, build};
    use facet_shell_protocol::ShellSubmenu;

    fn context(kind: MenuKind) -> MenuContext {
        MenuContext {
            kind,
            selected_count: 1,
            has_cursor: true,
            cursor_is_folder: false,
            single_folder_selected: false,
            location_is_folder: true,
            location_is_results: false,
            panel_count: 1,
            opposite_is_folder: false,
            clipboard_has_files: false,
            sort: Sort {
                column: Column::Name,
                direction: Direction::Ascending,
            },
            show_hidden: false,
            can_go_back: false,
            can_go_forward: false,
            can_go_up: false,
            can_undo: false,
            location_is_sync_folder: false,
        }
    }

    fn item_menu() -> ContextMenu {
        ContextMenu::new(build(&context(MenuKind::Item)))
    }

    #[test]
    fn arrow_keys_skip_disabled_rows_and_separators() {
        let mut menu = item_menu();
        // 파일 하나: 열기(켜짐), 연결 프로그램(켜짐), 새 탭에서 열기(꺼짐), 구분선, 잘라내기(켜짐)… 끝은 속성(켜짐).
        menu.step(1);
        assert_eq!(menu.selected(), Some(0));
        menu.step(1);
        assert_eq!(menu.selected(), Some(1));
        menu.step(1);
        assert_eq!(menu.selected(), Some(4), "꺼진 줄과 구분선을 건너뛴다");
        menu.step(-1);
        assert_eq!(menu.selected(), Some(1));
        menu.step(-1);
        assert_eq!(menu.selected(), Some(0));
        menu.step(-1);
        assert_eq!(menu.selected(), Some(18), "처음에서 위로 가면 끝으로 돈다");
    }

    #[test]
    fn enter_runs_the_selected_command_but_not_disabled_ones() {
        let mut menu = item_menu();
        menu.step(1);
        assert_eq!(menu.enter(), Outcome::Run(CommandId::Open));
        // 꺼진 줄(새 탭에서 열기)은 눌러도 아무 일이 없다.
        assert_eq!(menu.activate(2), Outcome::None);
        menu.hover(2);
        assert_eq!(menu.selected(), Some(0), "꺼진 줄 위로는 고르지 않는다");
    }

    #[test]
    fn submenu_opens_with_right_and_closes_with_left() {
        let mut menu = ContextMenu::new(build(&context(MenuKind::Background)));
        // 꺼진 뒤로·앞으로·위로·붙여넣기는 건너뛰고 새 폴더 → (꺼진 실행취소 건너뜀) 정렬 기준(하위 메뉴).
        menu.step(1);
        assert_eq!(menu.selected(), Some(5));
        menu.step(1);
        assert_eq!(menu.selected(), Some(8));
        assert!(menu.open_sub());
        assert_eq!(menu.open_submenu(), Some(8));
        assert_eq!(menu.sub_selected(), Some(0));
        menu.step(1);
        assert_eq!(menu.sub_selected(), Some(1));
        assert_eq!(menu.enter(), Outcome::Run(CommandId::SortByModified));
        assert!(menu.close_sub());
        assert_eq!(menu.selected(), Some(8));
        assert!(!menu.close_sub());
    }

    #[test]
    fn enter_on_a_submenu_row_opens_it() {
        let mut menu = ContextMenu::new(build(&context(MenuKind::Background)));
        menu.hover(8);
        assert_eq!(menu.open_submenu(), Some(8), "마우스를 올리면 열린다");
        menu.hover(9);
        assert_eq!(menu.open_submenu(), None, "다른 줄로 가면 닫힌다");
        menu.hover(8);
        menu.close_sub();
        assert_eq!(menu.enter(), Outcome::None);
        assert_eq!(menu.open_submenu(), Some(8));
    }

    #[test]
    fn placement_keeps_the_menu_inside_the_window() {
        let size = (300.0, 400.0);
        let bounds = (1000.0, 700.0);
        assert_eq!(place((100.0, 100.0), size, bounds), (100.0, 100.0));
        assert_eq!(place((900.0, 100.0), size, bounds), (700.0, 100.0));
        assert_eq!(place((100.0, 600.0), size, bounds), (100.0, 300.0));
        assert_eq!(place((50.0, 50.0), (2000.0, 2000.0), bounds), (0.0, 0.0));
        // 하위 메뉴: 오른쪽에 자리가 없으면 왼쪽.
        assert_eq!(
            place_sub(100.0, 300.0, 50.0, (200.0, 120.0), bounds),
            (400.0, 50.0)
        );
        assert_eq!(
            place_sub(700.0, 300.0, 50.0, (200.0, 120.0), bounds),
            (500.0, 50.0)
        );
        assert_eq!(
            place_sub(100.0, 300.0, 650.0, (200.0, 120.0), bounds),
            (400.0, 580.0)
        );
    }

    #[test]
    fn heights_follow_the_rows() {
        let menu = item_menu();
        // 열다섯 줄과 구분선 넷.
        assert_eq!(
            menu.height(),
            8.0 + 15.0 * ROW_HEIGHT + 4.0 * SEPARATOR_HEIGHT
        );
        assert_eq!(menu.row_top(0), PADDING);
        assert_eq!(
            menu.row_top(4),
            PADDING + 3.0 * ROW_HEIGHT + SEPARATOR_HEIGHT
        );
    }

    // --- 셸 항목 자리(#322) ---

    fn command(id: u32, text: &str, enabled: bool) -> MenuItem {
        MenuItem::Command(ShellCommand {
            id,
            text: text.into(),
            icon: None,
            enabled,
        })
    }

    fn shell(section: ShellSection) -> ShellMenu {
        // 코어의 셸 메뉴 번호는 코어만 만든다: 시험 앱에서 하나 얻는다.
        ShellMenu {
            id: menu_id(),
            kind: MenuKind::Item,
            section,
            crashing_extension: None,
        }
    }

    fn menu_id() -> ShellMenuId {
        use facet_core::shell_helper::HelperOptions;
        let dir = std::env::temp_dir();
        let mut app = facet_core::window::App::new(Some(facet_core::Location::file_system(&dir)));
        app.enable_shell_helper(HelperOptions::new(dir.join("없는-도우미.exe")));
        app.open_shell_menu(MenuKind::Background)
            .expect("실제 폴더의 빈 곳 메뉴")
    }

    fn sample_items() -> Vec<MenuItem> {
        vec![
            command(1, "&Open with fake", true),
            MenuItem::Separator,
            MenuItem::Submenu(ShellSubmenu {
                text: "보내기".into(),
                icon: None,
                enabled: true,
                items: vec![
                    command(2, "압축 폴더", true),
                    MenuItem::Submenu(ShellSubmenu {
                        text: "더".into(),
                        icon: None,
                        enabled: false,
                        items: vec![command(3, "깊은 항목", true)],
                    }),
                ],
            }),
            command(4, "꺼진 항목", false),
        ]
    }

    #[test]
    fn loading_row_follows_own_rows_and_items_replace_it_without_moving_them() {
        let mut menu = item_menu();
        let own_height = menu.height();
        let own_tops: Vec<f32> = (0..19).map(|row| menu.row_top(row)).collect();
        let mut loading = shell(ShellSection::Loading);
        menu.set_shell(Some(&loading));
        assert_eq!(menu.lines().len(), 21);
        assert!(matches!(menu.lines()[19], Line::ShellSeparator));
        assert!(matches!(menu.lines()[20], Line::Notice(Notice::Loading)));
        assert_eq!(
            menu.height(),
            own_height + SEPARATOR_HEIGHT + ROW_HEIGHT,
            "구분선과 「불러오는 중」 한 줄"
        );
        // 「불러오는 중」은 고르지 못한다: 끝에서 아래로 가면 첫 줄로 돈다.
        menu.hover(18);
        menu.step(1);
        assert_eq!(menu.selected(), Some(0));
        loading.section = ShellSection::Items(sample_items());
        menu.set_shell(Some(&loading));
        let tops: Vec<f32> = (0..19).map(|row| menu.row_top(row)).collect();
        assert_eq!(tops, own_tops, "자체 항목의 자리는 그대로");
        assert_eq!(menu.shell_id(), Some(loading.id));
        // 셸 명령은 고를 수 있고 Enter면 그 번호를 실행한다.
        menu.hover(20);
        assert_eq!(menu.enter(), Outcome::Shell(1));
        // 꺼진 셸 항목은 고르지도 실행하지도 못한다.
        let last = menu.lines().len() - 1;
        assert_eq!(menu.activate(last), Outcome::None);
        menu.hover(last);
        assert_eq!(menu.selected(), Some(20));
    }

    #[test]
    fn shell_submenu_opens_flattened_and_runs_its_items() {
        let mut menu = item_menu();
        let mut items = shell(ShellSection::Items(sample_items()));
        items.crashing_extension = None;
        menu.set_shell(Some(&items));
        // 19 자체 + 구분선 + 명령 + 구분선 + 하위 메뉴(22) + 꺼진 명령.
        assert!(matches!(
            menu.lines()[22],
            Line::Shell(MenuItem::Submenu(_))
        ));
        menu.hover(22);
        assert_eq!(menu.open_submenu(), Some(22));
        assert!(menu.sub_is_shell());
        let sub = menu.sub_lines();
        assert_eq!(sub.len(), 3, "압축 폴더, 구분선, 깊은 항목(꺼짐)");
        assert!(matches!(&sub[2], SubLine::Shell(c) if c.id == 3 && !c.enabled));
        assert_eq!(menu.sub_selected(), Some(0));
        menu.step(1);
        assert_eq!(menu.sub_selected(), Some(0), "꺼진 줄은 건너뛰어 제자리");
        assert_eq!(menu.enter(), Outcome::Shell(2));
        assert_eq!(menu.activate_sub(2), Outcome::None);
        assert!(menu.close_sub());
        assert_eq!(menu.selected(), Some(22));
    }

    #[test]
    fn failed_resting_and_crashing_extension_are_notice_rows() {
        let mut menu = item_menu();
        menu.set_shell(Some(&shell(ShellSection::Failed)));
        assert!(matches!(
            menu.lines().last(),
            Some(Line::Notice(Notice::Failed))
        ));
        let mut resting = shell(ShellSection::Resting);
        resting.crashing_extension = Some(CrashedExtension {
            clsid: "{0000}".into(),
            dll: "C:\\x\\bad-ext.dll".into(),
            name: None,
            crashes: 3,
        });
        menu.set_shell(Some(&resting));
        let tail: Vec<_> = menu.lines()[19..].to_vec();
        assert_eq!(tail.len(), 3);
        assert!(matches!(tail[1], Line::Notice(Notice::Resting)));
        assert!(matches!(&tail[2], Line::Notice(Notice::Crashing(name)) if name == "bad-ext.dll"));
        menu.set_shell(None);
        assert_eq!(menu.lines().len(), 19, "셸 자리가 없으면 자체 항목만");
    }

    #[test]
    fn shell_text_drops_accelerator_marks() {
        assert_eq!(shell_text("&Open"), "Open");
        assert_eq!(shell_text("Save && Close"), "Save & Close");
        assert_eq!(shell_text("7-Zip(&Z)"), "7-Zip(Z)");
    }
}
