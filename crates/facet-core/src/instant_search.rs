//! **즉시 검색**(GLOSSARY)을 **검색 결과 위치**에 넣는 코어 흐름의 값과 규칙.
//!
//! 서비스 연결([`crate::indexer_client::IndexerClient`])은 앱이 하나 갖고([`crate::window::App::enable_indexer`]), 탭은
//! 연결을 모른 채 보낼 질의([`crate::tab::Tab::instant_query`])와 받은 응답([`crate::tab::Tab::instant_reply`])만 다룬다.
//!
//! - **위치**: [`crate::SearchKind::Instant`]. 검색한 폴더(검색 전 폴더)와 질의 글자, 범위(모든 볼륨 / 그 폴더 아래만)를
//!   담는다. 세션에는 다른 검색 결과처럼 검색 전 폴더로 남는다.
//! - **입력마다 질의**: [`crate::window::App::set_instant_query`]가 글자를 질의 문법([`crate::query_syntax`])으로 해석해 질의를
//!   보낸다. 첫 글자에서 검색 결과 위치로 가 기록에 하나 남고, 뒤 글자는 그 위치를 제자리에서 고친다. 앞 결과는 새
//!   응답이 올 때까지 그대로 보인다(깜박이지 않음). 문법이 틀리면 결과도 위치도 그대로이고 까닭과 틀린 위치가
//!   [`crate::tab::Tab::instant_error`]에 있다.
//! - **첫 화면 분량과 이어 받기**: 첫 질의와 이어 받기는 [`PAGE_SIZE`]개씩 받는다.
//!   [`crate::window::App::load_more_results`]가 이어 받는다.
//! - **정렬**: 서비스가 아는 열(이름·위치(경로)·크기·수정 날짜, [`server_sort`])로 바꾸면 같은 질의를 그 정렬로 다시 보내고
//!   (결과는 처음부터 다시 받는다), 서비스가 모르는 열(유형·일치)은 받아 둔 결과만 정렬한다. 그래서 그 경우 정렬은
//!   받은 [`PAGE_SIZE`]의 배수만큼에만 걸린다.
//! - **질의는 하나씩**: 서비스는 연결 하나에 질의 하나만 이어 받게 두고 새 질의가 앞선 질의를 취소한다. 그래서 다른
//!   탭이 질의를 보내면 앞 탭의 이어 받기는 끊긴다([`crate::tab::Tab::has_more_results`]가 `false`가 되고 전체 개수와
//!   받은 수의 차이로 드러남).
//! - **강등**: 서비스를 쓸 수 없으면(연결하지 못함·버전 다름·범위의 볼륨에 인덱스 없음·검색 중 서비스가 멈춤) 같은 조건 트리로
//!   실시간 탐색([`crate::live_search`])을 띄워 같은 검색 결과 위치에 결과를 넣는다([`Demotion`], [`InstantStatus::demotion`]).
//!   범위가 모든 볼륨이면 탐색 범위는 검색 전 폴더 아래로 좁아진다(`narrowed_to_folder`). 강등 중 입력이 바뀌면 앞선 탐색은
//!   곧바로 멈추고 새 탐색은 [`DEMOTED_SEARCH_DELAY`] 뒤에 시작한다. 서비스가 다시 닿으면 다음 입력부터 즉시 검색이다.
//! - **인덱스 변경 따라가기**: 결과가 보이는 탭은 [`FOLLOW_INTERVAL`]마다 인덱스가 바뀌었는지 확인한다. 이어 받기 표지가
//!   있으면 결과 0개짜리 이어 받기(probe)를 보내 서비스가 표지를 무효(`StaleCursor`, 인덱스가 바뀜)로 답하는지 보고, 표지가
//!   없으면(결과를 다 받았거나 다른 탭의 질의가 표지를 지움) 같은 질의를 곧바로 다시 보낸다. 바뀌었으면 같은 질의를 조용히
//!   다시 보내(받은 만큼, [`refresh_limit`]) 결과를 바꾼다. 선택·커서·범위 기준은 전체 경로로 남고 사라진 항목은 빠진다.
//!   숨은 탭은 확인하지 않는다.
//! - 화면 문장은 없다(ADR-0007).

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use facet_protocol::{HitMessage, Order, SortKey, StatusMessage, VolumeState};

use crate::details::{Column, Direction, Sort};
use crate::indexer_client::{Connection, RequestId};
use crate::live_search::Found;
use crate::{Attributes, Item};

/// 첫 질의와 이어 받기마다 받는 결과 수: 한 화면 분량의 두 배쯤.
///
/// 행 높이 24픽셀이면 1080p 창에 약 40행, 4K·큰 창에서도 80행 안팎이다. 100개면 첫 응답으로 화면을 채우고 스크롤해도 한
/// 번 더 받기 전에 끝에 닿지 않는다. 더 키우면 한 응답의 크기(경로 문자열 100개 ≈ 수 KB가 아니라 수십 KB)와 항목을 목록에
/// 합치는 시간이 길어져 「치는 즉시」가 느려지고, 줄이면 스크롤 중 이어 받기가 잦다.
pub const PAGE_SIZE: u32 = 100;

/// 강등 중 입력이 바뀐 뒤 실시간 탐색을 시작하기까지 기다리는 시간.
///
/// 실시간 탐색은 큰 폴더를 훑으면 디스크와 코어를 쓰므로(10만 항목에 약 1초) 글자마다 새로 훑으면 치는 동안 앞선 훑기를
/// 계속 버리고 다시 시작하게 된다. 보통 타자 간격은 100~200ms이고 잠깐 멈추면 그보다 길다. 250ms면 이어 치는 동안에는 훑지
/// 않고, 멈추면 한 번 기다린 정도(체감 지연이 한 박자)로 시작한다. 처음 강등되는 입력(결과 위치로 막 간 첫 글자)과 검색 중
/// 서비스가 멈춰 강등된 질의는 이어 치는 중이 아니므로 기다리지 않는다.
pub const DEMOTED_SEARCH_DELAY: Duration = Duration::from_millis(250);

/// 결과가 보이는 즉시 검색 탭이 인덱스가 바뀌었는지 확인하는 간격.
///
/// 확인 방법: 이어 받기 표지가 있으면 결과 0개짜리 이어 받기를 보낸다. 서비스는 인덱스가 바뀐 뒤(USN 변경 적용·인덱스
/// 바꿔 끼우기)의 표지를 무효(`StaleCursor`)로 답하므로, 결과를 옮기지 않고 서비스 쪽 질의도 돌리지 않은 채 바뀌었는지
/// 안다. 표지가 없으면(결과를 다 받음·다른 탭의 질의가 이 연결의 표지를 지움) 같은 질의를 다시 보내 결과를 견준다.
///
/// 간격: 인덱서 서비스가 USN 저널을 100ms마다 읽어 새 파일이 1초 안에 질의 결과에 나오므로 1초면 밖의 변경이 늦어도 2초
/// 안에 화면에 닿는다. 더 짧게 하면 바쁜 볼륨(시스템 드라이브는 늘 바뀜)에서 1초에도 여러 번 같은 질의(100만 항목 인덱스에
/// 수십 ms)를 다시 돌리게 되고, 더 길면 「밖에서 지운 파일이 결과에 남아 있다」가 눈에 띈다.
pub const FOLLOW_INTERVAL: Duration = Duration::from_secs(1);

/// 인덱스가 바뀌어 다시 받는 결과 수의 상한. 스크롤로 이어 받은 만큼([`PAGE_SIZE`]의 배수로 올림)을 한 번에 다시 받되 이
/// 수를 넘지 않는다.
///
/// 결과 하나는 경로·이름·크기·날짜·속성으로 대략 200바이트라 5천 개면 응답이 1MB쯤으로 프레임 상한(8MB) 안이고, 목록을 다시
/// 만드는 데도 한 프레임 안팎이다. 그보다 많이 스크롤한 결과는 앞 5천 개만 다시 받고 나머지는 다시 스크롤하면 이어 받는다.
pub const REFRESH_LIMIT_MAX: u32 = 5_000;

/// 받은 결과 `loaded`개를 다시 받을 때의 결과 수: [`PAGE_SIZE`]의 배수로 올리고 [`PAGE_SIZE`]..=[`REFRESH_LIMIT_MAX`]로 자른다.
pub fn refresh_limit(loaded: usize) -> u32 {
    let loaded = u32::try_from(loaded).unwrap_or(u32::MAX);
    loaded
        .div_ceil(PAGE_SIZE)
        .saturating_mul(PAGE_SIZE)
        .clamp(PAGE_SIZE, REFRESH_LIMIT_MAX)
}

/// 인덱스가 바뀌었는지 확인하는 요청([`crate::tab::Tab::instant_follow`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Follow {
    /// 결과 0개짜리 이어 받기: 서비스가 표지를 무효로 답하면 인덱스가 바뀌었다.
    Probe(facet_protocol::ResumeMessage),
    /// 같은 질의를 조용히 다시 보낸다(이어 받기 표지가 없음).
    Refresh,
}

/// 즉시 검색을 서비스에 보내는 정렬. 서비스가 모르는 열이면 `None`이다(받아 둔 결과만 정렬).
pub fn server_sort(sort: Sort) -> Option<(SortKey, Order)> {
    let key = match sort.column {
        Column::Name => SortKey::Name,
        Column::Location => SortKey::Path,
        Column::Size => SortKey::Size,
        Column::Modified => SortKey::Modified,
        Column::Type | Column::Matches => return None,
    };
    let order = match sort.direction {
        Direction::Ascending => Order::Ascending,
        Direction::Descending => Order::Descending,
    };
    Some((key, order))
}

/// 서비스에 닿지 못한 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// 연결 상태(서비스 없음·버전 다름·아직 판정 중). 연결이 맺어지면 앱이 질의를 다시 보낸다.
    Connection(Connection),
    /// 답을 기다리다 연결이 끊겼다.
    Disconnected,
    /// 서비스가 오류로 답했다.
    Service(facet_protocol::ErrorMessage),
}

/// 즉시 검색을 실시간 탐색으로 강등한 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemotionReason {
    /// 서비스가 없다(연결하지 못했거나 검색 중 멈췄다).
    NoService,
    /// 서비스가 이 클라이언트의 버전을 받지 않는다.
    VersionMismatch,
    /// 범위의 볼륨에 인덱스가 없다(서비스 상태에서 「인덱스 없음」이거나 그 볼륨이 없음: 네트워크·exFAT USB 등).
    VolumeNotIndexed,
}

/// 강등 상태: 까닭과 범위를 좁혔는지.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Demotion {
    pub reason: DemotionReason,
    /// 범위가 모든 볼륨이었는데 검색 전 폴더 아래로 좁혔다.
    pub narrowed_to_folder: bool,
}

/// 탭이 읽는 즉시 검색 상태.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InstantStatus {
    /// 서비스가 준 전체 개수(받은 수가 아니라 질의에 맞는 결과 전체). 아직 응답이 없으면 `None`.
    pub total: Option<u64>,
    /// `total`이 권한 거르기 전의 상한인가(받는 결과가 `total`보다 적을 수 있다).
    pub total_is_upper_bound: bool,
    /// 인덱스를 만드는 중이라 덜 찼을 수 있는 볼륨들(예: `C:`). 비어 있지 않으면 「덜 찼을 수 있음」이다.
    pub building: Vec<String>,
    /// 서비스에 닿지 못해 기다리는 까닭(연결 판정 중 등). 서비스 없음·버전 다름은 기다리지 않고 [`Self::demotion`]이 된다.
    pub unavailable: Option<Unavailable>,
    /// 실시간 탐색으로 강등했으면 그 까닭. 강등 중에는 결과가 실시간 탐색([`crate::tab::Tab::search`]의 진행·요약)에서 온다.
    pub demotion: Option<Demotion>,
    /// 질의나 이어 받기의 응답을 기다리는 중인가.
    pub loading: bool,
    /// 남은 결과가 더 있는가(이어 받을 수 있다).
    pub has_more: bool,
}

impl InstantStatus {
    /// 인덱스를 만드는 중인 볼륨이 있어 결과가 덜 찼을 수 있는가.
    pub fn may_be_incomplete(&self) -> bool {
        !self.building.is_empty()
    }
}

/// 보낸 요청의 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flight {
    Query,
    Resume,
    /// 범위 볼륨의 인덱스 여부를 묻는 상태 질의.
    Status,
    /// 인덱스가 바뀌었는지 보는 결과 0개짜리 이어 받기.
    Probe,
    /// 인덱스가 바뀌어 같은 질의를 받은 만큼 조용히 다시 보냄.
    Refresh,
}

/// 탭의 즉시 검색 내부 상태(검색 결과 위치가 즉시 검색일 때).
#[derive(Debug, Default)]
pub(crate) struct InstantState {
    pub(crate) status: InstantStatus,
    /// 기다리는 요청과 그 종류. 이 번호가 아닌 응답은 버린다.
    pub(crate) in_flight: Option<(RequestId, Flight)>,
    /// 이어 받기 표지.
    pub(crate) next: Option<u64>,
    /// 마지막으로 보낸 질의의 정렬(`None`이면 정렬 없이 보냄). 아직 보내지 않았으면 `None`이고 `sent`가 거짓이다.
    pub(crate) sent_sort: Option<(SortKey, Order)>,
    /// 질의를 한 번이라도 보냈는가.
    pub(crate) sent: bool,
    /// 범위 볼륨의 인덱스를 확인했는가(위치마다 한 번 상태 질의로).
    pub(crate) volume_checked: bool,
    /// 한 번이라도 강등했는가(이어 치는 중인 입력에는 실시간 탐색 시작을 미룬다).
    pub(crate) was_demoted: bool,
    /// 강등한 실시간 탐색을 시작할 시각. 시작했으면 `None`.
    pub(crate) live_due: Option<Instant>,
    /// 다음에 보낼 질의가 인덱스 변경을 따라가는 조용한 다시 받기인가(받은 만큼, 앞 결과를 그대로 보이며).
    pub(crate) refresh: bool,
    /// 결과를 마지막으로 받았거나 인덱스 변경을 확인한 시각. 아직 결과가 없으면 `None`이다.
    pub(crate) checked_at: Option<Instant>,
}

/// 경로가 든 볼륨 이름(`C:`). 드라이브 문자가 없으면(UNC 등) `None`이다.
pub(crate) fn volume_of(path: &Path) -> Option<String> {
    use std::path::{Component, Prefix};
    match path.components().next()? {
        Component::Prefix(prefix) => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                Some(format!("{}:", (letter as char).to_ascii_uppercase()))
            }
            _ => None,
        },
        _ => None,
    }
}

/// 상태 질의의 응답에서 `folder`의 볼륨이 인덱스를 쓸 수 있는가(준비됨·만드는 중). 상태에 없거나 인덱스 없음이면 `false`.
pub(crate) fn volume_is_indexed(status: &StatusMessage, folder: &Path) -> bool {
    let Some(volume) = volume_of(folder) else {
        return false;
    };
    status
        .volumes
        .iter()
        .find(|v| v.volume.eq_ignore_ascii_case(&volume))
        .is_some_and(|v| !matches!(v.state, VolumeState::NotIndexed(_)))
}

/// 결과 항목 하나로 목록 항목을 만든다(디스크를 다시 읽지 않는다). 경로가 폴더 없이 이름뿐이면 빈 폴더다.
pub(crate) fn found_from_hit(hit: &HitMessage) -> Found {
    let full = Path::new(&hit.path);
    let folder: Arc<Path> = Arc::from(full.parent().unwrap_or_else(|| Path::new("")));
    const READ_ONLY: u32 = 0x1;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    const DIRECTORY: u32 = 0x10;
    Found {
        folder,
        item: Item {
            name: hit.name.clone().into(),
            is_folder: hit.attributes & DIRECTORY != 0,
            size: hit.size,
            modified: system_time(hit.modified),
            attributes: Attributes {
                hidden: hit.attributes & HIDDEN != 0,
                system: hit.attributes & SYSTEM != 0,
                read_only: hit.attributes & READ_ONLY != 0,
            },
        },
    }
}

/// `FILETIME`(1601-01-01 UTC부터 100ns)을 시각으로.
fn system_time(filetime: u64) -> SystemTime {
    const EPOCH_GAP_SECS: u64 = 11_644_473_600;
    let secs = filetime / 10_000_000;
    let nanos = (filetime % 10_000_000) as u32 * 100;
    if secs >= EPOCH_GAP_SECS {
        SystemTime::UNIX_EPOCH + Duration::new(secs - EPOCH_GAP_SECS, nanos)
    } else {
        SystemTime::UNIX_EPOCH - Duration::new(EPOCH_GAP_SECS - secs, 0) + Duration::new(0, nanos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 파일_시각을_시각으로_바꾼다() {
        let unix = system_time(11_644_473_600 * 10_000_000);
        assert_eq!(unix, SystemTime::UNIX_EPOCH);
        let later = system_time((11_644_473_600 + 5) * 10_000_000 + 10);
        assert_eq!(later, SystemTime::UNIX_EPOCH + Duration::new(5, 1_000));
    }

    #[test]
    fn 경로의_볼륨_이름을_드라이브_문자에서_읽는다() {
        let bs = '\u{5c}';
        let verbatim = format!("{bs}{bs}?{bs}D:{bs}a");
        let unc = format!("{bs}{bs}server{bs}share{bs}a");
        assert_eq!(volume_of(Path::new(&verbatim)), Some("D:".into()));
        assert_eq!(volume_of(Path::new(&unc)), None);
        assert_eq!(volume_of(Path::new("c:/a/b")), Some("C:".into()));
        assert_eq!(volume_of(Path::new(r"\server\share\a")), None);
    }

    #[test]
    fn 다시_받는_수는_받은_만큼을_한_화면_분량의_배수로_올리고_상한에서_자른다() {
        assert_eq!(refresh_limit(0), PAGE_SIZE);
        assert_eq!(refresh_limit(1), PAGE_SIZE);
        assert_eq!(refresh_limit(100), 100);
        assert_eq!(refresh_limit(101), 200);
        assert_eq!(refresh_limit(1_000_000), REFRESH_LIMIT_MAX);
    }

    #[test]
    fn 정렬은_서비스가_아는_열만_보낸다() {
        let by = |column| Sort {
            column,
            direction: Direction::Descending,
        };
        assert_eq!(
            server_sort(by(Column::Location)),
            Some((SortKey::Path, Order::Descending))
        );
        assert_eq!(server_sort(by(Column::Type)), None);
        assert_eq!(server_sort(by(Column::Matches)), None);
    }

    #[test]
    fn 결과_항목은_경로의_폴더와_속성으로_만든다() {
        let found = found_from_hit(&HitMessage {
            path: r"C:\a\b.txt".into(),
            name: "b.txt".into(),
            size: 7,
            modified: 0,
            attributes: 0x1 | 0x2,
        });
        assert_eq!(&*found.folder, Path::new(r"C:\a"));
        assert!(!found.item.is_folder);
        assert!(found.item.attributes.hidden && found.item.attributes.read_only);
    }
}
