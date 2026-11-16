//! 검색창의 순수 상태(화면을 부르지 않아 단위 테스트한다): 「이름」「내용」 전환, 칠 글자 다듬기, 탐색 상태 줄의
//! 종류와 값. 화면 글자는 [`crate::text`]가 만든다(ADR-0007).

use std::time::Duration;

use facet_core::live_search::SearchEnd;
use facet_core::search_results::SearchStatus;

/// 검색창의 전환: 무엇을 찾는가.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// 파일·폴더 이름(실시간 탐색).
    #[default]
    Name,
    /// 파일 내용. 내용 검색이 생길 때까지 고를 수 없다(화면은 아직 이 값을 만들지 않는다).
    #[allow(dead_code)]
    Content,
}

impl Mode {
    /// 고를 수 있는가. 「내용」은 내용 검색 티켓이 켤 때까지 꺼 둔다.
    pub const fn is_available(self) -> bool {
        match self {
            Mode::Name => true,
            Mode::Content => false,
        }
    }
}

/// 검색창이 한 번에 하는 일: 친 글자로 시작할 검색어. 앞뒤 공백을 걷고 비면 `None`이다(Enter를 눌러도 아무 일도 없다).
/// 고를 수 없는 전환이면 `None`.
pub fn query_to_start(mode: Mode, text: &str) -> Option<&str> {
    if !mode.is_available() {
        return None;
    }
    let query = text.trim();
    (!query.is_empty()).then_some(query)
}

/// 탐색 상태 줄의 내용.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    /// 탐색 중: 지금까지 찾은 수와 훑은 폴더 수.
    Running { found: usize, folders: usize },
    /// 끝났다: 찾은 수·건너뛴 폴더 수·걸린 시간.
    Finished {
        found: usize,
        skipped: usize,
        elapsed: Duration,
    },
    /// 중지했다(결과는 남는다).
    Stopped {
        found: usize,
        skipped: usize,
        elapsed: Duration,
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
    let Some(summary) = status.summary.filter(|_| !status.running) else {
        return Some(Line::Running {
            found: status.progress.found,
            folders: status.progress.folders_scanned,
        });
    };
    let (found, skipped, elapsed) = (summary.found, summary.folders_skipped, summary.elapsed);
    Some(match summary.end {
        SearchEnd::Finished => Line::Finished {
            found,
            skipped,
            elapsed,
        },
        SearchEnd::Cancelled => Line::Stopped {
            found,
            skipped,
            elapsed,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use facet_core::live_search::{SearchProgress, SearchSummary};

    #[test]
    fn 내용_전환은_꺼져_있어_검색어가_있어도_시작하지_않는다() {
        assert!(Mode::Name.is_available());
        assert!(!Mode::Content.is_available());
        assert_eq!(query_to_start(Mode::Content, "보고서"), None);
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
            },
            ..SearchStatus::default()
        };
        let line = line_of(Some(&running)).unwrap();
        assert_eq!(
            line,
            Line::Running {
                found: 3,
                folders: 9
            }
        );
        assert!(line.is_running());
        let done = SearchStatus {
            summary: Some(summary(SearchEnd::Finished)),
            ..SearchStatus::default()
        };
        assert_eq!(
            line_of(Some(&done)),
            Some(Line::Finished {
                found: 7,
                skipped: 2,
                elapsed: Duration::from_millis(1500)
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
}
