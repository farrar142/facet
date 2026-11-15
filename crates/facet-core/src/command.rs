//! 명령: 사용자가 실행할 수 있는 이름 붙은 동작. 단축키와 명령 팔레트는 명령을 실행하는 통로일 뿐이다.
//!
//! - 명령은 바뀌지 않는 ID([`CommandId`])로 구별한다. 코어는 화면 문장(명령 이름)을 갖지 않는다(ADR-0007).
//!   명령 이름은 앱이 주고, 명령 팔레트 검색([`search`])은 앱이 넘긴 이름에서 찾는다.
//! - 명령마다 기본 단축키가 있다([`CommandId::shortcuts`]). 키 조합으로 명령을 찾는 것은 [`find_by_shortcut`]이다.
//! - 지금 실행할 수 있는지는 탭 상태에서 나온다([`CommandId::is_enabled`]). 탭 명령(n번째 탭 등)은 패널의 탭
//!   수도 본다([`CommandId::is_enabled_in`]). 실행은 [`crate::window::App::execute`].
//!
//! 단축키는 입력한 글자가 아니라 키로 맞춘다. 한글 IME가 한글 모드여서 H 키가 「ㅗ」를 내도 Ctrl+H는 Ctrl+H다.
//! 앱은 키 이벤트의 글자를 [`Key::from_char`]로 키로 바꾼다. 두벌식 자모는 그 자모를 내는 영문 키로 바뀐다.

use crate::details::{Column, Direction, Sort};
use crate::tab::Tab;
use crate::window::Panel;

/// 명령의 ID. 이 이름과 [`CommandId::as_str`]의 글자는 바뀌지 않는다(설정·단축키 바꾸기가 이것으로 명령을 가리킨다).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandId {
    /// 열기: 커서 항목이 폴더면 그 안으로 가고, 파일이면 기본 프로그램으로 연다.
    Open,
    /// 뒤로.
    Back,
    /// 앞으로.
    Forward,
    /// 위로.
    Up,
    /// 새로 고침.
    Refresh,
    /// 모두 선택.
    SelectAll,
    /// 커서 항목 선택 토글.
    ToggleCursorSelection,
    /// 주소창을 글자로 고쳐 쓰는 모드로.
    EditAddress,
    /// 숨김 항목 보이기/감추기.
    ToggleHidden,
    /// 명령 팔레트 열기.
    CommandPalette,
    /// 이름 열로 정렬(같은 열이면 방향을 뒤집는다).
    SortByName,
    /// 수정한 날짜 열로 정렬.
    SortByModified,
    /// 유형 열로 정렬.
    SortByType,
    /// 크기 열로 정렬.
    SortBySize,
    /// 새 탭: 지금 위치의 탭을 활성 탭 바로 뒤에 열고 활성으로 한다.
    NewTab,
    /// 탭 닫기: 활성 탭을 닫는다. 패널의 마지막 탭이면 패널이, 창의 마지막 패널이면 창이 닫힌다.
    CloseTab,
    /// 다음 탭. 끝에서는 처음으로 돈다.
    NextTab,
    /// 이전 탭. 처음에서는 끝으로 돈다.
    PreviousTab,
    /// n번째 탭(1부터 8까지). 그 탭이 없으면 꺼진다.
    GoToTab(u8),
    /// 마지막 탭.
    LastTab,
    /// 새 탭에서 열기: 커서 항목인 폴더를 새 탭으로 활성 탭 바로 뒤에 연다. 지금 탭은 그대로 활성이다.
    OpenInNewTab,
}

impl CommandId {
    /// 모든 명령. 명령 팔레트는 검색어가 없으면 이 순서로 보인다.
    pub const ALL: [CommandId; 28] = [
        CommandId::Open,
        CommandId::Back,
        CommandId::Forward,
        CommandId::Up,
        CommandId::Refresh,
        CommandId::SelectAll,
        CommandId::ToggleCursorSelection,
        CommandId::EditAddress,
        CommandId::ToggleHidden,
        CommandId::CommandPalette,
        CommandId::SortByName,
        CommandId::SortByModified,
        CommandId::SortByType,
        CommandId::SortBySize,
        CommandId::NewTab,
        CommandId::CloseTab,
        CommandId::NextTab,
        CommandId::PreviousTab,
        CommandId::GoToTab(1),
        CommandId::GoToTab(2),
        CommandId::GoToTab(3),
        CommandId::GoToTab(4),
        CommandId::GoToTab(5),
        CommandId::GoToTab(6),
        CommandId::GoToTab(7),
        CommandId::GoToTab(8),
        CommandId::LastTab,
        CommandId::OpenInNewTab,
    ];

    /// 바뀌지 않는 ID 글자. 화면 문장이 아니다.
    pub const fn as_str(self) -> &'static str {
        match self {
            CommandId::Open => "open",
            CommandId::Back => "back",
            CommandId::Forward => "forward",
            CommandId::Up => "up",
            CommandId::Refresh => "refresh",
            CommandId::SelectAll => "select-all",
            CommandId::ToggleCursorSelection => "toggle-cursor-selection",
            CommandId::EditAddress => "edit-address",
            CommandId::ToggleHidden => "toggle-hidden",
            CommandId::CommandPalette => "command-palette",
            CommandId::SortByName => "sort-by-name",
            CommandId::SortByModified => "sort-by-modified",
            CommandId::SortByType => "sort-by-type",
            CommandId::SortBySize => "sort-by-size",
            CommandId::NewTab => "new-tab",
            CommandId::CloseTab => "close-tab",
            CommandId::NextTab => "next-tab",
            CommandId::PreviousTab => "previous-tab",
            CommandId::GoToTab(1) => "go-to-tab-1",
            CommandId::GoToTab(2) => "go-to-tab-2",
            CommandId::GoToTab(3) => "go-to-tab-3",
            CommandId::GoToTab(4) => "go-to-tab-4",
            CommandId::GoToTab(5) => "go-to-tab-5",
            CommandId::GoToTab(6) => "go-to-tab-6",
            CommandId::GoToTab(7) => "go-to-tab-7",
            CommandId::GoToTab(8) => "go-to-tab-8",
            // 1..=8 밖의 번호는 명령 목록에 없다.
            CommandId::GoToTab(_) => "go-to-tab",
            CommandId::LastTab => "last-tab",
            CommandId::OpenInNewTab => "open-in-new-tab",
        }
    }

    /// ID 글자로 명령을 찾는다. [`CommandId::as_str`]의 반대다.
    pub fn from_id(id: &str) -> Option<CommandId> {
        CommandId::ALL.into_iter().find(|c| c.as_str() == id)
    }

    /// 그 열로 정렬하는 명령.
    pub const fn sort_by(column: Column) -> CommandId {
        match column {
            Column::Name => CommandId::SortByName,
            Column::Modified => CommandId::SortByModified,
            Column::Type => CommandId::SortByType,
            Column::Size => CommandId::SortBySize,
        }
    }

    /// 정렬 명령이면 그 열.
    pub const fn sort_column(self) -> Option<Column> {
        match self {
            CommandId::SortByName => Some(Column::Name),
            CommandId::SortByModified => Some(Column::Modified),
            CommandId::SortByType => Some(Column::Type),
            CommandId::SortBySize => Some(Column::Size),
            _ => None,
        }
    }

    /// 기본 단축키. 여럿이면 앞의 것이 대표다(명령 팔레트에 먼저 보인다). 없으면 비어 있다.
    pub const fn shortcuts(self) -> &'static [Shortcut] {
        const fn key(key: Key) -> Shortcut {
            Shortcut::new(key)
        }
        const fn letter(c: char) -> Key {
            Key::Letter(c)
        }
        match self {
            CommandId::Open => const { &[key(Key::Enter)] },
            CommandId::Back => const { &[key(Key::Backspace), key(Key::Left).alt()] },
            CommandId::Forward => const { &[key(Key::Right).alt()] },
            CommandId::Up => const { &[key(Key::Up).alt()] },
            CommandId::Refresh => const { &[key(Key::Function(5))] },
            CommandId::SelectAll => const { &[key(letter('A')).ctrl()] },
            CommandId::ToggleCursorSelection => const { &[key(Key::Space).ctrl()] },
            CommandId::EditAddress => {
                const {
                    &[
                        key(letter('D')).alt(),
                        key(letter('L')).ctrl(),
                        key(Key::Function(4)),
                    ]
                }
            }
            CommandId::ToggleHidden => const { &[key(letter('H')).ctrl()] },
            CommandId::CommandPalette => const { &[key(letter('P')).ctrl().shift()] },
            CommandId::SortByName
            | CommandId::SortByModified
            | CommandId::SortByType
            | CommandId::SortBySize
            | CommandId::OpenInNewTab => &[],
            CommandId::NewTab => const { &[key(letter('T')).ctrl()] },
            CommandId::CloseTab => const { &[key(letter('W')).ctrl()] },
            CommandId::NextTab => const { &[key(Key::Tab).ctrl()] },
            CommandId::PreviousTab => const { &[key(Key::Tab).ctrl().shift()] },
            CommandId::GoToTab(1) => const { &[key(Key::Digit(1)).ctrl()] },
            CommandId::GoToTab(2) => const { &[key(Key::Digit(2)).ctrl()] },
            CommandId::GoToTab(3) => const { &[key(Key::Digit(3)).ctrl()] },
            CommandId::GoToTab(4) => const { &[key(Key::Digit(4)).ctrl()] },
            CommandId::GoToTab(5) => const { &[key(Key::Digit(5)).ctrl()] },
            CommandId::GoToTab(6) => const { &[key(Key::Digit(6)).ctrl()] },
            CommandId::GoToTab(7) => const { &[key(Key::Digit(7)).ctrl()] },
            CommandId::GoToTab(8) => const { &[key(Key::Digit(8)).ctrl()] },
            CommandId::GoToTab(_) => &[],
            CommandId::LastTab => const { &[key(Key::Digit(9)).ctrl()] },
        }
    }

    /// 탭 상태에서 지금 실행할 수 있는가.
    ///
    /// - 열기·커서 항목 선택 토글: 커서 항목이 있을 때.
    /// - 뒤로·앞으로: 그쪽 기록이 있을 때. 위로: 상위 위치가 있을 때(드라이브 루트에서 꺼진다).
    /// - 모두 선택: 보이는 항목이 있을 때.
    /// - 새 탭에서 열기: 커서 항목이 폴더일 때.
    /// - 나머지는 늘 켜져 있다. 다음·이전·n번째 탭은 탭 하나만 보아서는 모르므로 켜져 있다고 하고, 패널의 탭
    ///   수는 [`CommandId::is_enabled_in`]이 본다.
    pub fn is_enabled(self, tab: &Tab) -> bool {
        match self {
            CommandId::Open | CommandId::ToggleCursorSelection => tab.cursor_item().is_some(),
            CommandId::OpenInNewTab => tab.cursor_item().is_some_and(|item| item.is_folder),
            CommandId::Back => tab.can_go_back(),
            CommandId::Forward => tab.can_go_forward(),
            CommandId::Up => tab.can_go_up(),
            CommandId::SelectAll => !tab.is_empty(),
            CommandId::Refresh
            | CommandId::EditAddress
            | CommandId::ToggleHidden
            | CommandId::CommandPalette
            | CommandId::SortByName
            | CommandId::SortByModified
            | CommandId::SortByType
            | CommandId::SortBySize
            | CommandId::NewTab
            | CommandId::CloseTab
            | CommandId::NextTab
            | CommandId::PreviousTab
            | CommandId::GoToTab(_)
            | CommandId::LastTab => true,
        }
    }

    /// 패널 상태에서 지금 실행할 수 있는가. 탭 명령은 패널의 탭 수를 보고, 나머지는 활성 탭을 따른다
    /// ([`CommandId::is_enabled`]).
    ///
    /// - 다음·이전 탭: 다른 탭이 있을 때(탭이 둘 이상).
    /// - n번째 탭: 그 탭이 있을 때.
    pub fn is_enabled_in(self, panel: &Panel) -> bool {
        match self {
            CommandId::NextTab | CommandId::PreviousTab => panel.len() > 1,
            CommandId::GoToTab(n) => n >= 1 && usize::from(n) <= panel.len(),
            command => command.is_enabled(panel.active_tab()),
        }
    }
}

/// 명령의 실행 가능 여부를 정하는 상태: 탭 하나([`CommandId::is_enabled`])나 패널([`CommandId::is_enabled_in`]).
pub trait CommandState {
    /// `command`를 지금 실행할 수 있는가.
    fn is_enabled(&self, command: CommandId) -> bool;
}

impl CommandState for Tab {
    fn is_enabled(&self, command: CommandId) -> bool {
        command.is_enabled(self)
    }
}

impl CommandState for Panel {
    fn is_enabled(&self, command: CommandId) -> bool {
        command.is_enabled_in(self)
    }
}

/// 열 머리글을 누른 것처럼 정렬을 바꾼다: 같은 열이면 방향을 뒤집고, 다른 열이면 그 열 오름차순.
pub fn sort_toggled(sort: Sort, column: Column) -> Sort {
    let direction = match (sort.column == column, sort.direction) {
        (true, Direction::Ascending) => Direction::Descending,
        (true, Direction::Descending) | (false, _) => Direction::Ascending,
    };
    Sort { column, direction }
}

/// 키보드의 키. 글자가 아니라 키다: 「a」와 「A」, 두벌식의 「ㅁ」은 모두 [`Key::Letter`]`('A')`다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// 영문 키. 대문자 `'A'..='Z'`로 쓴다. 소문자로 만들어도 [`Shortcut::new`]가 대문자로 맞춘다.
    Letter(char),
    /// 숫자 키 `0..=9`.
    Digit(u8),
    /// 기능 키 F1..F24.
    Function(u8),
    Enter,
    Backspace,
    Space,
    Tab,
    Escape,
    Delete,
    Insert,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
}

impl Key {
    /// 키 이벤트가 낸 글자 하나에서 키를 찾는다. 영문자(대소문자), 숫자, 공백, 그리고 한글 IME가 한글 모드일 때
    /// 영문 키가 내는 두벌식 자모(Shift 쪽 쌍자음·ㅒ·ㅖ 포함)를 키로 바꾼다. 그 밖의 글자는 `None`이다.
    ///
    /// Enter·화살표·기능 키처럼 글자가 없는 키는 앱이 자기 키 이벤트에서 직접 [`Key`]로 바꾼다.
    pub fn from_char(c: char) -> Option<Key> {
        match c {
            'a'..='z' | 'A'..='Z' => Some(Key::Letter(c.to_ascii_uppercase())),
            '0'..='9' => Some(Key::Digit(c as u8 - b'0')),
            ' ' => Some(Key::Space),
            _ => dubeolsik_key(c).map(Key::Letter),
        }
    }

    /// 영문자는 대문자로 맞춘다.
    const fn normalized(self) -> Key {
        match self {
            Key::Letter(c) => Key::Letter(c.to_ascii_uppercase()),
            key => key,
        }
    }
}

/// 두벌식 자판에서 자모 `c`를 내는 영문 키(대문자).
fn dubeolsik_key(c: char) -> Option<char> {
    const LAYOUT: [(char, char); 33] = [
        ('ㅂ', 'Q'),
        ('ㅈ', 'W'),
        ('ㄷ', 'E'),
        ('ㄱ', 'R'),
        ('ㅅ', 'T'),
        ('ㅛ', 'Y'),
        ('ㅕ', 'U'),
        ('ㅑ', 'I'),
        ('ㅐ', 'O'),
        ('ㅔ', 'P'),
        ('ㅁ', 'A'),
        ('ㄴ', 'S'),
        ('ㅇ', 'D'),
        ('ㄹ', 'F'),
        ('ㅎ', 'G'),
        ('ㅗ', 'H'),
        ('ㅓ', 'J'),
        ('ㅏ', 'K'),
        ('ㅣ', 'L'),
        ('ㅋ', 'Z'),
        ('ㅌ', 'X'),
        ('ㅊ', 'C'),
        ('ㅍ', 'V'),
        ('ㅠ', 'B'),
        ('ㅜ', 'N'),
        ('ㅡ', 'M'),
        // Shift 쪽.
        ('ㅃ', 'Q'),
        ('ㅉ', 'W'),
        ('ㄸ', 'E'),
        ('ㄲ', 'R'),
        ('ㅆ', 'T'),
        ('ㅒ', 'O'),
        ('ㅖ', 'P'),
    ];
    LAYOUT
        .iter()
        .find(|(jamo, _)| *jamo == c)
        .map(|(_, key)| *key)
}

/// 키 조합: 키 하나와 Ctrl·Alt·Shift.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Shortcut {
    pub key: Key,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Shortcut {
    /// 수식 키 없는 키 하나.
    pub const fn new(key: Key) -> Shortcut {
        Shortcut {
            key: key.normalized(),
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    /// Ctrl을 더한다.
    pub const fn ctrl(mut self) -> Shortcut {
        self.ctrl = true;
        self
    }

    /// Alt를 더한다.
    pub const fn alt(mut self) -> Shortcut {
        self.alt = true;
        self
    }

    /// Shift를 더한다.
    pub const fn shift(mut self) -> Shortcut {
        self.shift = true;
        self
    }
}

/// 키 조합에 묶인 명령. 없으면 `None`이다.
pub fn find_by_shortcut(shortcut: Shortcut) -> Option<CommandId> {
    let shortcut = Shortcut {
        key: shortcut.key.normalized(),
        ..shortcut
    };
    CommandId::ALL
        .into_iter()
        .find(|command| command.shortcuts().contains(&shortcut))
}

/// 명령 팔레트 검색 결과 한 줄.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaletteEntry {
    pub command: CommandId,
    /// 기본 단축키. 비어 있을 수 있다.
    pub shortcuts: &'static [Shortcut],
    /// 지금 실행할 수 있는가. 아니면 팔레트에 꺼진 채로 보인다.
    pub enabled: bool,
}

/// 명령 팔레트 검색: 앱이 주는 명령 이름 `name`에 `query`가 부분 일치하는 명령을 찾는다.
///
/// - 대소문자를 가리지 않는다. 앞뒤 공백은 무시하고, 비면 모든 명령을 [`CommandId::ALL`] 순서로 준다.
/// - 이름이 검색어로 시작하는 명령이 먼저다. 그 안에서, 그리고 나머지는 [`CommandId::ALL`] 순서다.
/// - 실행 가능 여부는 `state`(탭이나 패널)로 정한다([`CommandState`]). 꺼진 명령도 결과에 든다.
pub fn search<S: AsRef<str>>(
    query: &str,
    name: impl Fn(CommandId) -> S,
    state: &(impl CommandState + ?Sized),
) -> Vec<PaletteEntry> {
    let query = query.trim().to_lowercase();
    let mut found: Vec<(bool, PaletteEntry)> = CommandId::ALL
        .into_iter()
        .filter_map(|command| {
            let at = name(command).as_ref().to_lowercase().find(&query)?;
            Some((
                at != 0,
                PaletteEntry {
                    command,
                    shortcuts: command.shortcuts(),
                    enabled: state.is_enabled(command),
                },
            ))
        })
        .collect();
    // 안정 정렬이라 무리 안에서는 ALL 순서가 남는다.
    found.sort_by_key(|(at, _)| *at);
    found.into_iter().map(|(_, entry)| entry).collect()
}
