//! 실제 셸 모드 도우미(#323)의 항목 실행·아이콘 모양과, 셸 메뉴 코드가 UI 프로세스 쪽 크레이트에서 불리지 않는 경계.
//! 셸 메뉴를 부르는 시험은 Windows에서만 돈다.

use std::fs;
use std::path::Path;

/// 셸 메뉴 구성·실행(`facet_win::shell_menu`)은 셸 확장 도우미만 부른다: UI 프로세스(`facet`)를 이루는 크레이트의 소스는
/// 그 모듈을 부르지 않는다(부르지 않는 함수는 `facet` 실행 파일에 링크되지 않는다, ADR-0006).
#[test]
fn ui_crates_do_not_call_the_shell_menu_module() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for name in ["facet-app", "facet-core"] {
        let mut stack = vec![crates.join(name).join("src")];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = fs::read_to_string(&path).unwrap();
                    assert!(
                        !text.contains("shell_menu::") && !text.contains("record_crashes"),
                        "{}가 셸 메뉴 모듈을 부른다",
                        path.display()
                    );
                }
            }
        }
    }
}

#[cfg(windows)]
mod real {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use facet_core::shell_helper::{HelperEvent, HelperOptions, Reply, RequestId, ShellHelper};
    use facet_shell_protocol::{MenuItem, MenuTarget};
    use tempfile::TempDir;

    const WAIT: Duration = Duration::from_secs(60);

    fn real_helper() -> ShellHelper {
        let mut options =
            HelperOptions::new(PathBuf::from(env!("CARGO_BIN_EXE_facet-shell-helper")));
        options.crash_log = None;
        options.reply_timeout = Duration::from_secs(30);
        options.stall_timeout = Duration::from_secs(50);
        ShellHelper::start(options)
    }

    fn reply_for(helper: &ShellHelper, id: RequestId) -> Reply {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "{id:?}의 결과가 오지 않았다");
            if let Some(HelperEvent::Reply { id: got, result }) = helper.wait_event(left)
                && got == id
            {
                return result.unwrap_or_else(|e| panic!("{id:?} 실패: {e:?}"));
            }
        }
    }

    fn file(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, b"facet").unwrap();
        path
    }

    /// 글자(가속 글자 `&` 뺌, 소문자)가 조건에 맞는 첫 켜진 명령의 번호.
    fn find_command(items: &[MenuItem], wanted: &dyn Fn(&str) -> bool) -> Option<u32> {
        items.iter().find_map(|item| match item {
            MenuItem::Command(c)
                if c.enabled && wanted(&c.text.replace('&', "").to_lowercase()) =>
            {
                Some(c.id)
            }
            MenuItem::Submenu(sub) => find_command(&sub.items, wanted),
            _ => None,
        })
    }

    fn icons_are_whole(items: &[MenuItem]) -> bool {
        items.iter().all(|item| {
            let (icon, children) = match item {
                MenuItem::Command(c) => (&c.icon, None),
                MenuItem::Submenu(s) => (&s.icon, Some(&s.items)),
                MenuItem::Separator => return true,
            };
            let whole = icon.as_ref().is_none_or(|i| {
                i.width > 0 && i.height > 0 && i.rgba.len() == (i.width * i.height * 4) as usize
            });
            whole && children.is_none_or(|c| icons_are_whole(c))
        })
    }

    #[test]
    fn create_shortcut_runs_through_the_menu_object() {
        let dir = TempDir::new().unwrap();
        let a = file(dir.path(), "a.txt");
        let helper = real_helper();
        let target = MenuTarget::Items(vec![a]);
        let menu = helper.menu(target);
        let Reply::Menu(items) = reply_for(&helper, menu) else {
            panic!("메뉴가 와야 한다");
        };
        assert!(icons_are_whole(&items), "아이콘 그림 크기가 맞지 않다");
        let Some(command) = find_command(&items, &|text| {
            text.contains("create shortcut") || text.contains("바로 가기 만들기")
        }) else {
            println!("「바로 가기 만들기」가 없어 건너뜀");
            return;
        };

        let done = helper.invoke(menu, command);
        assert_eq!(reply_for(&helper, done), Reply::Done);
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let made = fs::read_dir(dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .any(|e| e.path().extension().is_some_and(|x| x == "lnk"));
            if made {
                break;
            }
            assert!(Instant::now() < deadline, "바로 가기가 만들어지지 않았다");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn an_unknown_command_fails_without_ending_the_helper() {
        let dir = TempDir::new().unwrap();
        let a = file(dir.path(), "a.txt");
        let helper = real_helper();
        let menu = helper.menu(MenuTarget::Items(vec![a]));
        reply_for(&helper, menu);
        let pid = helper.process_id();

        let id = helper.invoke(menu, 0x7FFF);
        let deadline = Instant::now() + WAIT;
        let result = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero());
            if let Some(HelperEvent::Reply { id: got, result }) = helper.wait_event(left)
                && got == id
            {
                break result;
            }
        };
        assert!(result.is_err(), "없는 항목은 실패한다: {result:?}");

        let again = helper.menu(MenuTarget::Background(dir.path().to_path_buf()));
        assert!(matches!(reply_for(&helper, again), Reply::Menu(_)));
        assert_eq!(helper.process_id(), pid, "같은 도우미가 답한다");
    }
}
