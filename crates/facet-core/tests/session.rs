//! 세션 저장과 되살리기: 화면 상태를 세션으로 저장했다 읽어 되살리면 같은 상태인지, 깨진 파일·끊긴 저장·사라진
//! 위치·실행 인자를 어떻게 다루는지 공개 인터페이스로 확인한다.
//!
//! 앱([`App`])은 일꾼 스레드에서 읽으므로 결과를 기다린다. CI 러너가 느려도 흔들리지 않도록 기다리는 시간
//! ([`WAIT`])을 넉넉히 두고, 조건이 맞는 즉시 끝낸다.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use facet_core::Location;
use facet_core::details::{Column, Direction, Sort};
use facet_core::launch::resolve_launch;
use facet_core::placement::{Placement, Rect};
use facet_core::session::{
    self, ENV_PATH, Loaded, PanelSession, Saver, Session, SessionError, TabSession, WindowSession,
};
use facet_core::tab::{Relocation, Tab};
use facet_core::window::{App, home_location};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(10);

fn loc(path: &Path) -> Location {
    Location::file_system(path)
}

fn sort(column: Column, direction: Direction) -> Sort {
    Sort { column, direction }
}

/// 폴더 `a`, `b`, `c`, `d`, `e`가 있는 임시 폴더.
fn tree() -> TempDir {
    let dir = TempDir::new().unwrap();
    for name in ["a", "b", "c", "d", "e"] {
        fs::create_dir(dir.path().join(name)).unwrap();
    }
    dir
}

fn all_tabs(app: &App) -> impl Iterator<Item = &Tab> + '_ {
    app.windows()
        .iter()
        .flat_map(|w| w.panels())
        .flat_map(|p| p.tabs())
        .map(|(_, tab)| tab)
}

/// 기다리는 읽기가 없어질 때까지 앱이 받은 것을 탭에 넘긴다.
fn settle(app: &mut App) {
    let deadline = Instant::now() + WAIT;
    while all_tabs(app).any(|tab| tab.pending_location().is_some()) {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "읽기가 시간 안에 끝나지 않았다");
        app.wait_update(left);
    }
}

fn tab(path: &Path, sort: Sort) -> TabSession {
    TabSession {
        location: loc(path),
        sort,
    }
}

/// 창 둘, 첫 창은 패널 둘(탭 셋·둘), 둘째 창은 패널 하나(탭 둘). 정렬과 분할 비율이 서로 다르고, 활성 창은 첫 창,
/// 첫 창의 활성 패널은 오른쪽, 활성 탭은 첫 번째가 아니다.
fn sample(root: &Path) -> Session {
    Session {
        windows: vec![
            WindowSession {
                panels: vec![
                    PanelSession {
                        tabs: vec![
                            tab(&root.join("a"), sort(Column::Name, Direction::Ascending)),
                            tab(&root.join("b"), sort(Column::Size, Direction::Descending)),
                            tab(
                                &root.join("c"),
                                sort(Column::Modified, Direction::Ascending),
                            ),
                        ],
                        active_tab: 1,
                    },
                    PanelSession {
                        tabs: vec![
                            tab(&root.join("d"), sort(Column::Type, Direction::Descending)),
                            tab(root, sort(Column::Name, Direction::Descending)),
                        ],
                        active_tab: 1,
                    },
                ],
                active_panel: 1,
                split_ratio: 0.3,
                placement: Some(Placement {
                    normal: Rect::new(120, 80, 1280, 900),
                    maximized: true,
                }),
            },
            WindowSession {
                panels: vec![PanelSession {
                    tabs: vec![
                        tab(&root.join("e"), sort(Column::Size, Direction::Ascending)),
                        tab(
                            &root.join("a"),
                            sort(Column::Modified, Direction::Descending),
                        ),
                    ],
                    active_tab: 0,
                }],
                active_panel: 0,
                split_ratio: 0.7,
                placement: Some(Placement {
                    normal: Rect::new(-1500, -40, 900, 700),
                    maximized: false,
                }),
            },
        ],
        active_window: 0,
        recent_targets: Vec::new(),
    }
}

#[test]
fn 화면_상태를_저장했다_읽으면_같은_상태다() {
    let root = tree();
    let file = TempDir::new().unwrap();
    let path = file.path().join("facet").join("session.json");

    // 상태 모델의 연산으로 창 둘, 패널 둘, 탭 여럿, 서로 다른 정렬과 분할 비율을 만든다.
    let mut app = App::new(Some(loc(&root.path().join("a"))));
    app.set_sort(sort(Column::Size, Direction::Descending));
    app.open_tab(loc(&root.path().join("b")), true);
    app.set_sort(sort(Column::Modified, Direction::Ascending));
    app.split_panel();
    app.navigate(facet_core::tab::Navigation::Go(loc(&root.path().join("c"))));
    app.active_window_mut().set_split_ratio(0.35);
    app.open_window(loc(&root.path().join("d")));
    app.set_sort(sort(Column::Type, Direction::Descending));
    app.open_tab(loc(&root.path().join("e")), true);
    app.switch_tab(facet_core::window::TabSwitch::Previous);
    let first = app.windows()[0].id();
    app.activate_window(first);
    app.activate_panel(0);
    // 앱이 알린 창 자리: 첫 창은 최대화(보통 자리를 남김), 둘째 창은 왼쪽 모니터(음수 좌표).
    let placed = Placement {
        normal: Rect::new(200, 100, 1200, 800),
        maximized: true,
    };
    app.window_mut(first).unwrap().set_placement(Some(placed));
    let second = app.windows()[1].id();
    let left = Placement {
        normal: Rect::new(-1700, 50, 1000, 700),
        maximized: false,
    };
    app.window_mut(second).unwrap().set_placement(Some(left));
    settle(&mut app);
    let saved = app.session().expect("창이 있다");
    assert_eq!(saved.windows[0].placement, Some(placed));
    assert_eq!(saved.windows[1].placement, Some(left));
    assert_eq!(saved.windows.len(), 2);
    assert_eq!(saved.windows[0].panels.len(), 2);

    session::save(&path, &saved).unwrap();
    let loaded = session::load(&path);
    assert_eq!(loaded.session(), Some(&saved));

    let restored = App::restore(loaded.into_session(), Vec::new());
    assert_eq!(restored.session(), Some(saved.clone()));
    assert_eq!(restored.active_window_id(), restored.windows()[0].id());
    assert_eq!(restored.windows()[0].active_panel_index(), 0);
    assert_eq!(restored.windows()[0].split_ratio(), 0.35);
    assert_eq!(restored.windows()[1].active_panel().active_index(), 0);
    assert_eq!(restored.windows()[0].placement(), Some(placed));
    assert_eq!(restored.windows()[1].placement(), Some(left));

    // 손으로 만든 세션도 같다.
    let sample = sample(root.path());
    session::save(&path, &sample).unwrap();
    let restored = App::restore(session::load(&path).into_session(), Vec::new());
    assert_eq!(restored.session(), Some(sample));
}

#[test]
fn 세션_파일은_버전을_가진_사람이_읽을_수_있는_json이다() {
    let root = tree();
    let json = sample(root.path()).to_json();
    assert!(json.contains("\"version\": 1"), "{json}");
    assert!(json.contains("\"split_ratio\": 0.3"), "{json}");
    assert!(json.contains("\"maximized\": true"), "{json}");
    assert!(json.contains("\"x\": -1500"), "{json}");
    assert!(json.contains("\"width\": 1280"), "{json}");
    assert!(json.contains("\"column\": \"size\""), "{json}");
    assert!(json.contains('\n'), "들여 쓴다");
    assert_eq!(Session::from_json(&json).unwrap(), sample(root.path()));
}

/// 기본 상태: 창 하나, 패널 하나, 사용자 홈 폴더 탭 하나.
fn assert_default(app: &App) {
    let session = app.session().unwrap();
    assert_eq!(session.windows.len(), 1);
    assert_eq!(session.windows[0].panels.len(), 1);
    assert_eq!(session.windows[0].panels[0].tabs.len(), 1);
    assert_eq!(app.active_tab().location(), &home_location());
}

#[test]
fn 깨졌거나_버전이_없거나_모르는_버전이면_기본_상태가_되고_원래_파일을_남긴다() {
    let root = tree();
    let good = sample(root.path()).to_json();
    let cases = [
        ("깨진 JSON", "{\"version\": 1, \"windows\": [".to_owned()),
        ("버전 없음", good.replace("\"version\": 1,", "")),
        (
            "모르는 버전",
            good.replace("\"version\": 1,", "\"version\": 99,"),
        ),
        (
            "버전이 수가 아님",
            good.replace("\"version\": 1,", "\"version\": \"1\","),
        ),
        (
            "형식에 맞지 않음",
            "{\"version\": 1, \"windows\": 3}".to_owned(),
        ),
    ];
    for (case, text) in cases {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("session.json");
        fs::write(&path, &text).unwrap();
        let loaded = session::load(&path);
        let Loaded::Broken { error, kept } = loaded else {
            panic!("{case}: 깨진 파일로 보지 않았다: {loaded:?}");
        };
        match case {
            "버전 없음" => assert!(matches!(error, SessionError::MissingVersion), "{case}"),
            "모르는 버전" => assert!(
                matches!(&error, SessionError::UnknownVersion(v) if v == "99"),
                "{case}: {error:?}"
            ),
            "버전이 수가 아님" => {
                assert!(matches!(error, SessionError::UnknownVersion(_)), "{case}")
            }
            _ => assert!(
                matches!(error, SessionError::Invalid(_)),
                "{case}: {error:?}"
            ),
        }
        let kept = kept.unwrap_or_else(|| panic!("{case}: 남긴 파일 이름이 없다"));
        assert_eq!(fs::read_to_string(&kept).unwrap(), text, "{case}");
        assert_eq!(kept.parent(), Some(dir.path()), "{case}: 옆에 남긴다");
        let name = kept.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("session.broken-") && name.ends_with(".json"),
            "{name}"
        );
        assert!(
            !path.exists(),
            "{case}: 원래 이름에는 남지 않는다(다음 저장이 덮어쓰지 않게)"
        );

        let app = App::restore(None, Vec::new());
        assert_default(&app);
    }
}

#[test]
fn 깨진_파일을_여러_번_남겨도_겹치지_않는다() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("session.json");
    let mut kept = Vec::new();
    for n in 0..3 {
        fs::write(&path, format!("깨짐 {n}")).unwrap();
        let Loaded::Broken {
            kept: Some(file), ..
        } = session::load(&path)
        else {
            panic!("깨진 파일로 보지 않았다");
        };
        kept.push(file);
    }
    for (n, file) in kept.iter().enumerate() {
        assert_eq!(fs::read_to_string(file).unwrap(), format!("깨짐 {n}"));
    }
}

#[test]
fn 파일이_없으면_조용히_기본_상태다() {
    let dir = TempDir::new().unwrap();
    let loaded = session::load(&dir.path().join("session.json"));
    assert!(matches!(loaded, Loaded::Missing));
    assert_default(&App::restore(loaded.into_session(), Vec::new()));
    assert_eq!(
        fs::read_dir(dir.path()).unwrap().count(),
        0,
        "아무 파일도 만들지 않는다"
    );
}

#[test]
fn 저장이_바꿔치기_전에_끊기면_원래_파일이_그대로_남는다() {
    let root = tree();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("session.json");
    let before = sample(root.path());
    session::save(&path, &before).unwrap();

    // 저장 도중 끊김: 임시 파일에 반쯤 쓰고 바꿔치기하지 못했다.
    let temp = session::temp_path(&path);
    assert_eq!(temp.parent(), path.parent(), "임시 파일은 옆에 둔다");
    let half = &before.to_json()[..40];
    fs::write(&temp, half).unwrap();
    assert_eq!(session::load(&path).session(), Some(&before));

    // 남은 임시 파일이 있어도 다음 저장은 된다.
    let mut after = before.clone();
    after.active_window = 1;
    session::save(&path, &after).unwrap();
    assert_eq!(session::load(&path).session(), Some(&after));
    assert!(!temp.exists(), "바꿔치기하면 임시 파일이 남지 않는다");
}

#[test]
fn 되살린_직후_보이는_탭만_읽고_보이지_않는_탭은_처음_볼_때_읽는다() {
    let root = tree();
    let mut app = App::restore(Some(sample(root.path())), Vec::new());
    let mut reading = Vec::new();
    for window in app.windows() {
        for panel in window.panels() {
            for (index, (_, tab)) in panel.tabs().enumerate() {
                let visible = index == panel.active_index();
                assert_eq!(tab.is_shown(), visible);
                assert_eq!(
                    tab.pending_location().is_some(),
                    visible,
                    "{}",
                    tab.location()
                );
                if visible {
                    reading.push(tab.location().clone());
                }
            }
        }
    }
    assert_eq!(
        reading,
        [
            loc(&root.path().join("b")),
            loc(root.path()),
            loc(&root.path().join("e"))
        ]
    );
    settle(&mut app);
    // 보이지 않던 탭은 아직 목록이 없다.
    let hidden = app.windows()[0].panels()[0].tabs().next().unwrap();
    assert!(hidden.1.is_empty());
    assert!(hidden.1.pending_location().is_none());
    let hidden = hidden.0;

    app.activate_tab(hidden);
    assert_eq!(
        app.active_tab().pending_location(),
        Some(&loc(&root.path().join("a")))
    );
    settle(&mut app);
    assert!(app.active_tab().is_shown());
}

#[test]
fn 사라진_위치의_탭은_처음_볼_때_가장_가까운_남은_상위로_열리고_원래_위치를_알린다() {
    let root = tree();
    let deep = root.path().join("a").join("깊은").join("폴더");
    fs::create_dir_all(&deep).unwrap();
    let saved = Session {
        windows: vec![WindowSession {
            panels: vec![PanelSession {
                tabs: vec![
                    tab(&root.path().join("b"), Sort::default()),
                    tab(&deep, sort(Column::Size, Direction::Descending)),
                ],
                active_tab: 0,
            }],
            active_panel: 0,
            split_ratio: 0.5,
            placement: None,
        }],
        active_window: 0,
        recent_targets: Vec::new(),
    };
    let file = TempDir::new().unwrap();
    let path = file.path().join("session.json");
    session::save(&path, &saved).unwrap();
    fs::remove_dir_all(root.path().join("a").join("깊은")).unwrap();

    let mut app = App::restore(session::load(&path).into_session(), Vec::new());
    settle(&mut app);
    // 보지 않은 탭은 아직 확인하지 않았다.
    let (hidden, tab_before) = app.windows()[0].panels()[0].tabs().nth(1).unwrap();
    assert_eq!(tab_before.location(), &loc(&deep));
    assert!(tab_before.relocation().is_none());

    app.activate_tab(hidden);
    settle(&mut app);
    let tab = app.active_tab();
    assert_eq!(tab.location(), &loc(&root.path().join("a")));
    assert_eq!(
        tab.relocation(),
        Some(&Relocation::ToParent {
            vanished: loc(&deep)
        })
    );
    assert_eq!(tab.sort(), sort(Column::Size, Direction::Descending));
}

#[test]
fn 보이는_탭의_위치가_사라졌으면_곧바로_상위로_열린다() {
    let root = tree();
    let gone = root.path().join("b").join("없음");
    let saved = Session {
        windows: vec![WindowSession {
            panels: vec![PanelSession {
                tabs: vec![tab(&gone, Sort::default())],
                active_tab: 0,
            }],
            active_panel: 0,
            split_ratio: 0.5,
            placement: None,
        }],
        active_window: 0,
        recent_targets: Vec::new(),
    };
    let mut app = App::restore(Some(saved), Vec::new());
    settle(&mut app);
    assert_eq!(app.active_tab().location(), &loc(&root.path().join("b")));
    assert_eq!(
        app.active_tab().relocation(),
        Some(&Relocation::ToParent {
            vanished: loc(&gone)
        })
    );
}

/// 이 기계에 없는 드라이브 문자. 모두 있으면 `None`.
#[cfg(windows)]
fn missing_drive() -> Option<PathBuf> {
    ('D'..='Z')
        .rev()
        .map(|letter| PathBuf::from(format!(r"{letter}:\")))
        .find(|root| !root.exists())
}

#[cfg(windows)]
#[test]
fn 사라진_위치의_상위가_하나도_없으면_홈_폴더로_열린다() {
    let Some(drive) = missing_drive() else {
        eprintln!("없는 드라이브 문자가 없어 건너뛴다");
        return;
    };
    let root = tree();
    let gone = drive.join("빠진 드라이브").join("폴더");
    let saved = Session {
        windows: vec![WindowSession {
            panels: vec![PanelSession {
                tabs: vec![
                    tab(root.path(), Sort::default()),
                    tab(&gone, Sort::default()),
                ],
                active_tab: 0,
            }],
            active_panel: 0,
            split_ratio: 0.5,
            placement: None,
        }],
        active_window: 0,
        recent_targets: Vec::new(),
    };
    let mut app = App::restore(Some(saved), Vec::new());
    settle(&mut app);
    let (hidden, _) = app.windows()[0].panels()[0].tabs().nth(1).unwrap();
    app.activate_tab(hidden);
    settle(&mut app);
    assert_eq!(app.active_tab().location(), &home_location());
    assert_eq!(
        app.active_tab().relocation(),
        Some(&Relocation::ToHome {
            vanished: loc(&gone)
        })
    );
}

#[test]
fn 첫_실행의_경로_인자는_마지막_활성_창의_활성_패널에_활성_탭으로_붙는다() {
    let root = tree();
    let mut saved = sample(root.path());
    // 마지막 활성 창은 둘째 창이고, 그 창의 활성 탭은 첫 번째(`e`)다.
    saved.active_window = 1;
    let targets = resolve_launch(root.path(), &[PathBuf::from("c"), PathBuf::from("d")]);
    let mut app = App::restore(Some(saved), targets);

    let second = app.windows()[1].id();
    assert_eq!(app.active_window_id(), second);
    let panel = app.active_window().active_panel();
    let locations: Vec<_> = panel.tabs().map(|(_, t)| t.location().clone()).collect();
    assert_eq!(
        locations,
        [
            loc(&root.path().join("e")),
            loc(&root.path().join("c")),
            loc(&root.path().join("d")),
            loc(&root.path().join("a")),
        ]
    );
    assert_eq!(panel.active_index(), 1);
    let tabs: Vec<_> = panel.tabs().map(|(_, t)| t).collect();
    assert!(!tabs[0].is_shown(), "앞서 보이던 탭은 숨는다");
    assert!(tabs[1].is_shown());
    assert!(!tabs[2].is_shown(), "나머지 인자 탭은 처음 볼 때 읽는다");
    assert!(tabs[2].pending_location().is_none());
    // 다른 창은 그대로다.
    assert_eq!(app.windows()[0].panels()[0].len(), 3);
    settle(&mut app);
    assert_eq!(app.active_tab().location(), &loc(&root.path().join("c")));
}

#[test]
fn 세션이_없으면_경로_인자로_창을_연다() {
    let root = tree();
    let targets = resolve_launch(root.path(), &[PathBuf::from("c")]);
    let app = App::restore(None, targets);
    assert_eq!(app.windows().len(), 1);
    assert_eq!(app.active_tab().location(), &loc(&root.path().join("c")));
}

#[test]
fn 세션의_범위_밖_번호와_빈_패널은_다듬어_되살린다() {
    let root = tree();
    let saved = Session {
        windows: vec![
            WindowSession {
                panels: vec![],
                active_panel: 0,
                split_ratio: 0.5,
                placement: None,
            },
            WindowSession {
                panels: vec![
                    PanelSession {
                        tabs: vec![],
                        active_tab: 0,
                    },
                    PanelSession {
                        tabs: vec![tab(&root.path().join("a"), Sort::default())],
                        active_tab: 7,
                    },
                ],
                active_panel: 5,
                split_ratio: 9.0,
                placement: None,
            },
        ],
        active_window: 3,
        recent_targets: Vec::new(),
    };
    let app = App::restore(Some(saved), Vec::new());
    assert_eq!(app.windows().len(), 1);
    let window = app.active_window();
    assert_eq!(window.panels().len(), 1);
    assert_eq!(window.active_panel().active_index(), 0);
    assert_eq!(app.active_tab().location(), &loc(&root.path().join("a")));

    let empty = Session {
        windows: vec![],
        active_window: 0,
        recent_targets: Vec::new(),
    };
    assert_default(&App::restore(Some(empty), Vec::new()));
}

/// `path`에 `expected`가 쓰일 때까지 기다린다.
fn wait_for_file(path: &Path, expected: &Session) {
    let deadline = Instant::now() + WAIT;
    loop {
        if session::load(path).session() == Some(expected) {
            return;
        }
        assert!(Instant::now() < deadline, "시간 안에 저장하지 않았다");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn 바뀐_상태를_묶어_일꾼_스레드에서_저장하고_끝낼_때_한_번_더_쓴다() {
    let root = tree();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("facet").join("session.json");
    let delay = Duration::from_millis(300);
    let mut saver = Saver::spawn(path.clone(), delay, None);

    let first = sample(root.path());
    let mut second = first.clone();
    second.active_window = 1;
    let started = Instant::now();
    saver.update(Some(first));
    saver.update(Some(second.clone()));
    // 막지 않고, 묶는 동안은 쓰지 않는다.
    assert!(started.elapsed() < delay);
    assert!(!path.exists() || started.elapsed() >= delay);
    wait_for_file(&path, &second);
    assert!(started.elapsed() >= delay, "묶는 시간보다 먼저 썼다");

    // 창이 하나도 없는 상태(끝남)는 넘기지 않는다: 마지막 창이 있던 세션이 남는다.
    saver.update(None);
    let mut third = second.clone();
    third.windows[0].split_ratio = 0.6;
    saver.update(Some(third.clone()));
    // 끝내면 묶는 시간을 기다리지 않고 마지막 상태를 쓴다.
    let finishing = Instant::now();
    saver.finish().unwrap();
    assert!(finishing.elapsed() < delay);
    assert_eq!(session::load(&path).session(), Some(&third));
}

#[test]
fn 같은_상태는_다시_쓰지_않는다() {
    let root = tree();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("session.json");
    let saved = sample(root.path());
    let mut saver = Saver::spawn(path.clone(), Duration::from_millis(50), Some(saved.clone()));
    saver.update(Some(saved));
    std::thread::sleep(Duration::from_millis(200));
    assert!(!path.exists());
}

#[test]
fn 저장에_실패해도_막히지_않는다() {
    let root = tree();
    let dir = TempDir::new().unwrap();
    // 폴더가 있어야 할 자리에 파일이 있어 저장할 수 없다.
    let blocker = dir.path().join("파일");
    fs::write(&blocker, b"x").unwrap();
    let path = blocker.join("session.json");
    let mut saver = Saver::spawn(path.clone(), Duration::from_millis(50), None);
    let started = Instant::now();
    saver.update(Some(sample(root.path())));
    std::thread::sleep(Duration::from_millis(200));
    let mut next = sample(root.path());
    next.active_window = 1;
    saver.update(Some(next));
    assert!(saver.finish().is_err());
    assert!(started.elapsed() < WAIT);
}

#[test]
fn 세션_파일_자리는_appdata_아래이고_측정_모드는_바꾼_자리만_쓴다() {
    let env = |vars: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    };
    let appdata = &[("APPDATA", r"C:\Users\u\AppData\Roaming")];
    assert_eq!(
        session::session_path_with(false, env(appdata)),
        Some(
            Path::new(r"C:\Users\u\AppData\Roaming")
                .join("facet")
                .join("session.json")
        )
    );
    assert_eq!(session::session_path_with(true, env(appdata)), None);
    let both = &[
        ("APPDATA", r"C:\Users\u\AppData\Roaming"),
        (ENV_PATH, r"D:\측정\session.json"),
    ];
    assert_eq!(
        session::session_path_with(true, env(both)),
        Some(PathBuf::from(r"D:\측정\session.json"))
    );
    assert_eq!(
        session::session_path_with(false, env(both)),
        Some(PathBuf::from(r"D:\측정\session.json"))
    );
    assert_eq!(session::session_path_with(false, env(&[])), None);
}

/// 형식 버전 1의 앞선 파일(창 자리를 담기 전, `placement`가 늘 `null`).
const OLDER_FILE: &str = r#"{
  "version": 1,
  "active_window": 0,
  "windows": [
    {
      "placement": null,
      "split_ratio": 0.5,
      "active_panel": 0,
      "panels": [
        { "active_tab": 0, "tabs": [ { "location": { "file_system": "C:\\" }, "sort": { "column": "name", "direction": "ascending" } } ] }
      ]
    }
  ]
}"#;

#[test]
fn 창_자리가_없는_앞선_파일은_그대로_읽고_창_자리만_없다() {
    let session = Session::from_json(OLDER_FILE).unwrap();
    assert_eq!(session.windows.len(), 1);
    assert_eq!(session.windows[0].placement, None);
    assert_eq!(session.windows[0].panels[0].tabs.len(), 1);
    // `placement` 열쇠가 아예 없어도 같다.
    let without_key = OLDER_FILE.replace("\"placement\": null,", "");
    assert_eq!(Session::from_json(&without_key).unwrap(), session);
    // 다시 쓰면 `null`로 남는다(앞선 facet도 읽는다).
    assert!(session.to_json().contains("\"placement\": null"));
}

#[test]
fn 형식에_맞지_않는_창_자리는_그_창의_자리만_버린다() {
    for placement in [
        r#""placement": { "x": 10, "y": 10, "width": 800 },"#,
        r#""placement": { "x": 10, "y": 10, "width": 0, "height": 600, "maximized": false },"#,
        r#""placement": { "x": 10, "y": 10, "width": -5, "height": 600, "maximized": false },"#,
        r#""placement": "왼쪽 위","#,
    ] {
        let text = OLDER_FILE.replace("\"placement\": null,", placement);
        let session = Session::from_json(&text).unwrap_or_else(|e| panic!("{placement}: {e}"));
        assert_eq!(session.windows[0].placement, None, "{placement}");
        assert_eq!(session.windows[0].panels[0].tabs.len(), 1, "{placement}");
    }
    let text = OLDER_FILE.replace(
        "\"placement\": null,",
        r#""placement": { "x": -10, "y": 20, "width": 800, "height": 600, "maximized": true },"#,
    );
    assert_eq!(
        Session::from_json(&text).unwrap().windows[0].placement,
        Some(Placement {
            normal: Rect::new(-10, 20, 800, 600),
            maximized: true
        })
    );
}

#[test]
fn 창_자리를_모르는_새_창은_세션에_자리가_없다() {
    let root = tree();
    let mut app = App::new(Some(loc(root.path())));
    app.open_window(loc(&root.path().join("a")));
    settle(&mut app);
    let session = app.session().unwrap();
    assert!(session.windows.iter().all(|w| w.placement.is_none()));
}

#[test]
fn 최근_대상이_없는_앞선_파일도_읽히고_최근_대상은_저장했다_읽으면_그대로다() {
    // 최근 대상을 담기 전의 버전 1 파일: 필드가 없어도 읽고 비어 있다(버전은 올리지 않았다).
    let older = Session::from_json(OLDER_FILE).unwrap();
    assert!(older.recent_targets.is_empty());

    let root = tree();
    let mut app = App::new(Some(loc(&root.path().join("a"))));
    settle(&mut app);
    let mut saved = app.session().unwrap();
    assert!(saved.recent_targets.is_empty());
    saved.recent_targets = vec![root.path().join("c"), root.path().join("b")];

    let json = saved.to_json();
    assert!(json.contains("\"version\": 1"), "{json}");
    let read = Session::from_json(&json).unwrap();
    assert_eq!(read, saved);

    // 앱으로 되살리면 앱의 최근 대상이 되고, 다시 저장하면 그대로 나온다.
    let restored = App::restore(Some(read), Vec::new());
    assert_eq!(
        restored.recent_targets(),
        [root.path().join("c"), root.path().join("b")].as_slice()
    );
    assert_eq!(restored.session().unwrap().recent_targets, saved.recent_targets);
}

#[test]
fn 되살린_최근_대상은_열_개까지만_같은_폴더는_한_번만_남는다() {
    let root = tree();
    let mut app = App::new(Some(loc(root.path())));
    settle(&mut app);
    let mut saved = app.session().unwrap();
    saved.recent_targets = (0..14).map(|n| root.path().join(format!("t{n}"))).collect();
    saved.recent_targets.insert(3, root.path().join("t0"));
    let restored = App::restore(Some(saved), Vec::new());
    let recent = restored.recent_targets();
    assert_eq!(recent.len(), 10);
    // 앞쪽(가장 최근) 순서를 지키고 중복은 첫 번째(가장 최근) 자리에 남는다.
    assert_eq!(recent[0], root.path().join("t0"));
    assert_eq!(recent[1], root.path().join("t1"));
    assert_eq!(recent[2], root.path().join("t2"));
    assert_eq!(recent[3], root.path().join("t3"));
}
