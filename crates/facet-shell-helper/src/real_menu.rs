//! 실제 셸 메뉴(#323): 셸 스레드(COM STA)에서 메뉴를 만들어 들고 있다가 고른 항목을 실행한다. 이 모듈의 함수는 모두
//! 셸 스레드에서 돈다(메뉴 객체는 만든 스레드에서만 쓴다).

use std::cell::RefCell;
use std::mem::ManuallyDrop;
use std::sync::mpsc::Sender;

use facet_shell_protocol::{Icon, MenuItem, MenuTarget, ShellCommand, ShellSubmenu};
use facet_win::shell_menu::{self, Entry, IconImage, MenuError, ShellMenu};

/// 들고 있는 메뉴(이 셸 스레드에 하나).
///
/// 메뉴 객체는 [`ManuallyDrop`]에 둔다: 정상 길에서는 꺼내 버리고(COM이 살아 있는 동안 놓음), 스레드가 끝나며 남은
/// 것은(셸 스레드를 버린 경우) COM을 정리한 뒤라 놓지 않고 흘린다(이미 내린 DLL을 부르지 않게).
struct Held {
    target: MenuTarget,
    menu: ManuallyDrop<ShellMenu>,
}

thread_local! {
    static HELD: RefCell<Option<Held>> = const { RefCell::new(None) };
}

/// `target`의 셸 메뉴를 만들어 항목 트리로 돌려주고 들고 있는다. 앞서 들고 있던 메뉴는 놓는다.
pub fn build(target: &MenuTarget) -> Result<Vec<MenuItem>, MenuError> {
    release();
    let menu = ShellMenu::build(&to_shell(target))?;
    let items = menu.entries().into_iter().map(item).collect();
    HELD.with(|held| {
        *held.borrow_mut() = Some(Held {
            target: target.clone(),
            menu: ManuallyDrop::new(menu),
        });
    });
    Ok(items)
}

/// 들고 있던 `target`의 메뉴(없거나 대상이 다르면 새로 만든 메뉴)로 `command`번 항목을 실행한다. 실행이 돌아오면 그
/// 결과를 `answer`로 보내고, 실행이 띄운 창이 닫힐 때까지 메시지를 돌린 뒤 메뉴 객체를 놓는다.
pub fn invoke(target: &MenuTarget, command: u32, answer: &Sender<Result<(), MenuError>>) {
    let held = HELD.with(|held| held.borrow_mut().take());
    let menu = match held {
        Some(held) if held.target == *target => ManuallyDrop::into_inner(held.menu),
        other => {
            if let Some(old) = other {
                drop(ManuallyDrop::into_inner(old.menu));
            }
            match ShellMenu::build(&to_shell(target)) {
                Ok(menu) => {
                    // 펼칠 때 채우는 하위 메뉴(「보내기」 등)의 항목 번호도 생기게 한 번 읽는다.
                    let _ = menu.entries();
                    menu
                }
                Err(error) => {
                    let _ = answer.send(Err(error));
                    return;
                }
            }
        }
    };
    let result = menu.invoke(command);
    let invoked = result.is_ok();
    let _ = answer.send(result);
    if invoked {
        shell_menu::pump_while_windows(facet_win::shell_dialog::APPEAR_TIMEOUT);
    }
    drop(menu);
}

/// 들고 있던 메뉴를 놓는다.
fn release() {
    if let Some(held) = HELD.with(|held| held.borrow_mut().take()) {
        drop(ManuallyDrop::into_inner(held.menu));
    }
}

fn to_shell(target: &MenuTarget) -> shell_menu::MenuTarget {
    match target {
        MenuTarget::Items(paths) => shell_menu::MenuTarget::Items(paths.clone()),
        MenuTarget::Background(folder) => shell_menu::MenuTarget::Background(folder.clone()),
    }
}

fn item(entry: Entry) -> MenuItem {
    match entry {
        Entry::Command(command) => MenuItem::Command(ShellCommand {
            id: command.id,
            text: command.text,
            icon: command.icon.map(icon),
            enabled: command.enabled,
        }),
        Entry::Submenu(submenu) => MenuItem::Submenu(ShellSubmenu {
            text: submenu.text,
            icon: submenu.icon.map(icon),
            enabled: submenu.enabled,
            items: submenu.items.into_iter().map(item).collect(),
        }),
        Entry::Separator => MenuItem::Separator,
    }
}

fn icon(image: IconImage) -> Icon {
    Icon {
        width: image.width,
        height: image.height,
        rgba: image.rgba,
    }
}
