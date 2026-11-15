//! 창 자리 고치기: 모니터 목록과 창 자리를 주면 보이는 자리는 그대로 두고, 보이지 않는 자리는 주 모니터 안으로
//! 옮기고(크면 줄여), 최대화한 창은 최대화를 풀었을 때의 자리를 남기는지 화면 없이 확인한다.

use facet_core::placement::{Monitor, Placement, Rect, WindowState, fit};

/// 주 모니터(1920x1080, 아래 40픽셀 작업 표시줄)와 그 오른쪽의 배율 150% 4K 모니터(물리 픽셀).
fn two_monitors() -> Vec<Monitor> {
    vec![
        Monitor {
            work_area: Rect::new(0, 0, 1920, 1040),
            primary: true,
        },
        Monitor {
            work_area: Rect::new(1920, -200, 3840, 2100),
            primary: false,
        },
    ]
}

/// 주 모니터 하나만 남은 상태(오른쪽 모니터를 뺐다).
fn primary_only() -> Vec<Monitor> {
    vec![two_monitors()[0]]
}

#[test]
fn 모니터_안의_창은_그대로다() {
    let rect = Rect::new(100, 100, 800, 600);
    assert_eq!(fit(rect, &two_monitors()), rect);
    let on_second = Rect::new(2500, 300, 1600, 1200);
    assert_eq!(fit(on_second, &two_monitors()), on_second);
}

#[test]
fn 두_모니터에_걸친_창과_조금_밖으로_나간_창도_보이면_그대로다() {
    let across = Rect::new(1500, 100, 1000, 700);
    assert_eq!(fit(across, &two_monitors()), across);
    // 왼쪽 위로 조금 나갔어도(보이지 않는 테두리 등) 제목 줄을 잡을 수 있다.
    let edge = Rect::new(-7, 0, 800, 600);
    assert_eq!(fit(edge, &primary_only()), edge);
    // 오른쪽으로 많이 나갔어도 제목 줄이 100픽셀 넘게 보이면 그대로다.
    let mostly_out = Rect::new(1700, 200, 900, 600);
    assert_eq!(fit(mostly_out, &primary_only()), mostly_out);
}

#[test]
fn 없는_모니터에_있던_창은_주_모니터_가운데로_옮긴다() {
    let on_second = Rect::new(2500, 300, 800, 600);
    assert_eq!(
        fit(on_second, &primary_only()),
        Rect::new((1920 - 800) / 2, (1040 - 600) / 2, 800, 600)
    );
    // 왼쪽(음수 좌표)에 있던 모니터도 같다.
    let on_left = Rect::new(-1500, 100, 800, 600);
    assert_eq!(fit(on_left, &primary_only()), Rect::new(560, 220, 800, 600));
}

#[test]
fn 주_모니터보다_큰_창은_줄여_넣는다() {
    let huge = Rect::new(2000, 0, 3000, 1800);
    assert_eq!(fit(huge, &primary_only()), Rect::new(0, 0, 1920, 1040));
    let wide = Rect::new(5000, 5000, 2500, 500);
    assert_eq!(fit(wide, &primary_only()), Rect::new(0, 270, 1920, 500));
}

#[test]
fn 주_모니터와_조금만_겹친_창은_가까운_안쪽으로_민다() {
    // 제목 줄이 작업 영역 위로 나가 잡을 수 없다.
    let above = Rect::new(300, -500, 800, 600);
    assert_eq!(fit(above, &primary_only()), Rect::new(300, 0, 800, 600));
    // 오른쪽 끝에 20픽셀만 걸쳤다.
    let right = Rect::new(1900, 400, 800, 600);
    assert_eq!(fit(right, &primary_only()), Rect::new(1120, 400, 800, 600));
    // 작업 표시줄 뒤로 들어갔다.
    let below = Rect::new(100, 1030, 800, 600);
    assert_eq!(fit(below, &primary_only()), Rect::new(100, 440, 800, 600));
}

#[test]
fn 주_모니터가_원점에_있지_않아도_그_작업_영역_안으로_옮긴다() {
    // 작업 표시줄이 위·왼쪽에 있는 주 모니터, 주 모니터 표시가 없으면 첫 모니터.
    let monitors = [Monitor {
        work_area: Rect::new(60, 48, 1860, 1032),
        primary: false,
    }];
    let gone = Rect::new(-3000, -3000, 400, 300);
    assert_eq!(
        fit(gone, &monitors),
        Rect::new(60 + (1860 - 400) / 2, 48 + (1032 - 300) / 2, 400, 300)
    );
}

#[test]
fn 모니터를_모르면_그대로다() {
    let rect = Rect::new(-5000, 9000, 800, 600);
    assert_eq!(fit(rect, &[]), rect);
}

#[test]
fn 최대화한_창은_최대화를_풀었을_때의_자리를_남긴다() {
    let normal = Rect::new(100, 100, 800, 600);
    let placed = Placement::observe(None, normal, WindowState::Normal).unwrap();
    assert_eq!(
        placed,
        Placement {
            normal,
            maximized: false
        }
    );
    // 최대화하면 창은 모니터 전체지만 보통 자리는 그대로다.
    let full = Rect::new(-8, -8, 1920, 1009);
    let maximized = Placement::observe(Some(placed), full, WindowState::Maximized).unwrap();
    assert_eq!(
        maximized,
        Placement {
            normal,
            maximized: true
        }
    );
    // 최소화한 자리(-32000)는 남기지 않는다.
    let minimized = Rect::new(-32000, -32000, 160, 28);
    assert_eq!(
        Placement::observe(Some(maximized), minimized, WindowState::Minimized),
        Some(maximized)
    );
    // 최대화를 풀면 다시 보통 자리를 따른다.
    let moved = Rect::new(300, 200, 800, 600);
    assert_eq!(
        Placement::observe(Some(maximized), moved, WindowState::Normal),
        Some(Placement {
            normal: moved,
            maximized: false
        })
    );
}

#[test]
fn 처음부터_최대화한_창은_지금_자리를_남기고_크기가_없는_자리는_버린다() {
    let full = Rect::new(-8, -8, 1920, 1009);
    assert_eq!(
        Placement::observe(None, full, WindowState::Maximized),
        Some(Placement {
            normal: full,
            maximized: true
        })
    );
    let empty = Rect::new(0, 0, 0, 0);
    assert_eq!(Placement::observe(None, empty, WindowState::Normal), None);
    assert_eq!(
        Placement::observe(None, empty, WindowState::Minimized),
        None
    );
}
