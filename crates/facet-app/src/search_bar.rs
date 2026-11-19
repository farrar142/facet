//! 검색창의 순수 상태(화면을 부르지 않아 단위 테스트한다): 「이름」「내용」 전환, 칠 글자 다듬기, 탐색 상태 줄의
//! 종류와 값. 화면 글자는 [`crate::text`]가 만든다(ADR-0007).

use std::time::Duration;

use facet_core::content_search::Skipped;
use facet_core::live_search::SearchEnd;
use facet_core::search_results::SearchStatus;

/// 검색창의 전환: 무엇을 찾는가.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// 파일·폴더 이름(실시간 탐색).
    #[default]
    Name,
    /// 파일 내용(내용 검색). 찾을 글자·정규식 확인란·파일 이름 조건이 더해진다.
    Content,
}

impl Mode {
    /// 화면의 「내용」 전환 값에서.
    pub const fn from_content(content: bool) -> Mode {
        if content { Mode::Content } else { Mode::Name }
    }
}

/// 검색창이 한 번에 하는 일: 친 글자로 시작할 검색어. 이름 검색은 앞뒤 공백을 걷고 비면 `None`이다(Enter를 눌러도 아무 일도
/// 없다). 내용 검색은 찾을 글자 그대로(공백도 글자다) 두되 공백뿐이면 `None`이다.
pub fn query_to_start(mode: Mode, text: &str) -> Option<&str> {
    match mode {
        Mode::Name => {
            let query = text.trim();
            (!query.is_empty()).then_some(query)
        }
        Mode::Content => (!text.trim().is_empty()).then_some(text),
    }
}

/// 탐색 상태 줄의 내용.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    /// 탐색 중: 지금까지 찾은 수와 훑은 폴더 수. 내용 검색이면 종류별 건너뛴 파일 수도 있다.
    Running {
        found: usize,
        folders: usize,
        files_skipped: Option<Skipped>,
        /// 내려가지 않은 펼쳐지지 않은 온라인 폴더 수.
        online_folders: usize,
    },
    /// 끝났다: 찾은 수·건너뛴 폴더 수·걸린 시간(내용 검색이면 종류별 건너뛴 파일 수도).
    Finished {
        found: usize,
        skipped: usize,
        elapsed: Duration,
        files_skipped: Option<Skipped>,
        /// 내려가지 않은 펼쳐지지 않은 온라인 폴더 수.
        online_folders: usize,
    },
    /// 중지했다(결과는 남는다).
    Stopped {
        found: usize,
        skipped: usize,
        elapsed: Duration,
        files_skipped: Option<Skipped>,
        /// 내려가지 않은 펼쳐지지 않은 온라인 폴더 수.
        online_folders: usize,
    },
}

impl Line {
    /// 탐색 중이면 중지 단추를 보인다.
    pub const fn is_running(self) -> bool {
        matches!(self, Line::Running { .. })
    }
}

/// 탭의 검색 상태에서 상태 줄을 정한다. 검색 결과 위치가 아니면(`None`) 줄이 없다.
pub fn line_of(status: Option<&SearchStatus>) -> Option<Line> {
    let status = status?;
    let files_skipped = status.content.map(|content| content.skipped);
    let Some(summary) = status.summary.filter(|_| !status.running) else {
        return Some(Line::Running {
            found: status.progress.found,
            folders: status.progress.folders_scanned,
            files_skipped,
            online_folders: status.progress.online_folders_skipped,
        });
    };
    let (found, skipped, elapsed) = (summary.found, summary.folders_skipped, summary.elapsed);
    Some(match summary.end {
        SearchEnd::Finished => Line::Finished {
            found,
            skipped,
            elapsed,
            files_skipped,
            online_folders: summary.online_folders_skipped,
        },
        SearchEnd::Cancelled => Line::Stopped {
            found,
            skipped,
            elapsed,
            files_skipped,
            online_folders: summary.online_folders_skipped,
        },
    })
}

/// 「이름」 검색창에서 입력이 바뀌었을 때 즉시 검색에 넘길 글자. 이름 전환에서만 넘기고(앞뒤 공백은 질의 해석이 다룬다, 비면
/// 검색 끝내기), 한글 조합 중(`composing`)이면 확정 때까지 넘기지 않는다. 내용 전환은 Enter로만 시작하므로 `None`이다.
pub fn instant_text(mode: Mode, text: &str, composing: bool) -> Option<&str> {
    match mode {
        Mode::Name if !composing => Some(text),
        _ => None,
    }
}

/// 즉시 검색 상태 줄의 내용(화면 글자는 [`crate::text::instant_status_text`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstantLine {
    /// 결과 개수.
    pub count: InstantCount,
    /// 인덱스를 만드는 중인 볼륨(덜 찼을 수 있음).
    pub building: Vec<String>,
    /// 서비스에 닿지 못해 기다리는 중이면 까닭.
    pub waiting: Option<Waiting>,
    /// 실시간 탐색으로 강등했으면 그 까닭.
    pub demotion: Option<facet_core::instant_search::Demotion>,
}

/// 결과 개수.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstantCount {
    /// 아직 응답이 없다.
    Unknown,
    /// 전체 개수. `upper_bound`이면 권한으로 거르기 전의 상한이다.
    Total { total: u64, upper_bound: bool },
    /// 강등한 실시간 탐색이 지금까지 찾은 수.
    Found(usize),
}

/// 서비스를 기다리는 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waiting {
    /// 연결을 판정하는 중.
    Connecting,
    /// 연결이 끊겼거나 서비스가 오류로 답했다.
    NoAnswer,
}

/// 탭의 즉시 검색 상태에서 상태 줄을 만든다. `found`는 목록의 결과 수(강등했을 때 쓴다).
pub fn instant_line(
    status: &facet_core::instant_search::InstantStatus,
    found: usize,
) -> InstantLine {
    use facet_core::indexer_client::Connection;
    use facet_core::instant_search::Unavailable;
    let count = if status.demotion.is_some() {
        InstantCount::Found(found)
    } else {
        status
            .total
            .map_or(InstantCount::Unknown, |total| InstantCount::Total {
                total,
                upper_bound: status.total_is_upper_bound,
            })
    };
    let waiting = status.unavailable.as_ref().map(|why| match why {
        Unavailable::Connection(Connection::Connecting) => Waiting::Connecting,
        _ => Waiting::NoAnswer,
    });
    InstantLine {
        count,
        building: status.building.clone(),
        waiting,
        demotion: status.demotion,
    }
}

/// 질의 문법 오류에서 틀린 글자 조각(UTF-8 바이트 범위를 글자 경계로 맞춰 자름). 범위가 비면 `None`.
pub fn wrong_part<'a>(query: &'a str, range: &std::ops::Range<usize>) -> Option<&'a str> {
    let mut start = range.start.min(query.len());
    let mut end = range.end.min(query.len());
    while !query.is_char_boundary(start) {
        start -= 1;
    }
    while !query.is_char_boundary(end) {
        end += 1;
    }
    let part = &query[start..end];
    (!part.is_empty()).then_some(part)
}

/// 스크롤이 목록 끝 가까이인가: 보이는 맨 아래가 마지막 행에서 `rows_before_end`행 안이면 이어 받을 때다.
/// `scroll`은 위로 넘긴 거리, 모두 같은 단위(픽셀).
pub fn near_end(
    scroll: f32,
    view_height: f32,
    row_height: f32,
    rows: usize,
    rows_before_end: usize,
) -> bool {
    if row_height <= 0.0 {
        return false;
    }
    let total = rows as f32 * row_height;
    scroll + view_height + rows_before_end as f32 * row_height >= total
}

#[cfg(test)]
mod tests {
    use super::*;
    use facet_core::live_search::{SearchProgress, SearchSummary};
    use facet_core::search_results::ContentStatus;

    #[test]
    fn 내용_전환은_찾을_글자를_그대로_두고_공백뿐이면_시작하지_않는다() {
        assert_eq!(Mode::from_content(true), Mode::Content);
        assert_eq!(Mode::from_content(false), Mode::Name);
        assert_eq!(query_to_start(Mode::Content, " 보고서 "), Some(" 보고서 "));
        assert_eq!(query_to_start(Mode::Content, "   "), None);
        assert_eq!(query_to_start(Mode::Content, ""), None);
    }

    #[test]
    fn 검색어는_앞뒤_공백을_걷고_비면_시작하지_않는다() {
        assert_eq!(query_to_start(Mode::Name, "  보고서 "), Some("보고서"));
        assert_eq!(query_to_start(Mode::Name, "   "), None);
        assert_eq!(query_to_start(Mode::Name, ""), None);
    }

    fn summary(end: SearchEnd) -> SearchSummary {
        SearchSummary {
            found: 7,
            folders_scanned: 30,
            folders_skipped: 2,
            online_folders_skipped: 0,
            elapsed: Duration::from_millis(1500),
            end,
        }
    }

    #[test]
    fn 상태_줄은_탐색_중_끝남_중지를_가른다() {
        assert_eq!(line_of(None), None);
        let running = SearchStatus {
            running: true,
            progress: SearchProgress {
                found: 3,
                folders_scanned: 9,
                folders_skipped: 0,
                online_folders_skipped: 0,
            },
            ..SearchStatus::default()
        };
        let line = line_of(Some(&running)).unwrap();
        assert_eq!(
            line,
            Line::Running {
                found: 3,
                folders: 9,
                files_skipped: None,
                online_folders: 0
            }
        );
        assert!(line.is_running());
        let content = SearchStatus {
            running: true,
            content: Some(ContentStatus {
                skipped: Skipped {
                    binary: 2,
                    ..Skipped::default()
                },
                ..ContentStatus::default()
            }),
            ..SearchStatus::default()
        };
        assert!(matches!(
            line_of(Some(&content)),
            Some(Line::Running {
                files_skipped: Some(Skipped { binary: 2, .. }),
                ..
            })
        ));
        let done = SearchStatus {
            summary: Some(summary(SearchEnd::Finished)),
            ..SearchStatus::default()
        };
        assert_eq!(
            line_of(Some(&done)),
            Some(Line::Finished {
                found: 7,
                skipped: 2,
                elapsed: Duration::from_millis(1500),
                files_skipped: None,
                online_folders: 0
            })
        );
        let stopped = SearchStatus {
            summary: Some(summary(SearchEnd::Cancelled)),
            ..SearchStatus::default()
        };
        assert!(matches!(
            line_of(Some(&stopped)),
            Some(Line::Stopped { found: 7, .. })
        ));
        assert!(!line_of(Some(&stopped)).unwrap().is_running());
    }

    #[test]
    fn 즉시_검색은_이름_전환에서만_조합이_끝난_글자를_넘긴다() {
        assert_eq!(instant_text(Mode::Name, "보고", false), Some("보고"));
        assert_eq!(instant_text(Mode::Name, "", false), Some(""));
        assert_eq!(instant_text(Mode::Name, "보고", true), None);
        assert_eq!(instant_text(Mode::Content, "보고", false), None);
    }

    #[test]
    fn 즉시_검색_상태는_개수_덜_참_강등을_담는다() {
        use facet_core::instant_search::{Demotion, DemotionReason, InstantStatus, Unavailable};
        let status = InstantStatus {
            total: Some(1234),
            total_is_upper_bound: true,
            building: vec!["C:".to_owned()],
            ..InstantStatus::default()
        };
        let line = instant_line(&status, 100);
        assert_eq!(
            line.count,
            InstantCount::Total {
                total: 1234,
                upper_bound: true
            }
        );
        assert_eq!(line.building, vec!["C:".to_owned()]);
        assert_eq!(line.waiting, None);
        assert_eq!(
            instant_line(&InstantStatus::default(), 0).count,
            InstantCount::Unknown
        );

        let demoted = InstantStatus {
            total: Some(5),
            demotion: Some(Demotion {
                reason: DemotionReason::NoService,
                narrowed_to_folder: true,
            }),
            ..InstantStatus::default()
        };
        let line = instant_line(&demoted, 7);
        assert_eq!(line.count, InstantCount::Found(7));
        assert!(line.demotion.is_some());

        let waiting = InstantStatus {
            unavailable: Some(Unavailable::Disconnected),
            ..InstantStatus::default()
        };
        assert_eq!(instant_line(&waiting, 0).waiting, Some(Waiting::NoAnswer));
    }

    #[test]
    fn 틀린_범위는_글자_경계로_맞춰_자른다() {
        assert_eq!(wrong_part("보고서 <초안", &(10..11)), Some("<"));
        assert_eq!(wrong_part("보고서", &(1..4)), Some("보고"));
        assert_eq!(wrong_part("abc", &(5..9)), None);
        assert_eq!(wrong_part("abc", &(1..1)), None);
    }

    #[test]
    fn 목록_끝_가까이_스크롤하면_이어_받는다() {
        // 100행, 행 24, 보이는 높이 240.
        assert!(!near_end(0.0, 240.0, 24.0, 100, 10));
        assert!(near_end(2000.0, 240.0, 24.0, 100, 10));
        assert!(near_end(0.0, 240.0, 24.0, 5, 10));
        assert!(!near_end(0.0, 240.0, 0.0, 5, 10));
    }
}
