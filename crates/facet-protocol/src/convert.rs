//! 인덱스 크레이트와 오가는 변환: 질의 메시지 → 인덱스 질의, 인덱스 결과 → 응답 메시지.

use facet_index as index;

use crate::message::{
    Case, Condition, ErrorKind, ErrorMessage, HitMessage, Order, PageMessage, QueryMessage,
    ResumeMessage, SortKey,
};
use crate::wire::DecodeError;

impl From<Case> for index::Case {
    fn from(c: Case) -> Self {
        match c {
            Case::Insensitive => Self::Insensitive,
            Case::Sensitive => Self::Sensitive,
        }
    }
}

impl From<index::Case> for Case {
    fn from(c: index::Case) -> Self {
        match c {
            index::Case::Insensitive => Self::Insensitive,
            index::Case::Sensitive => Self::Sensitive,
        }
    }
}

impl From<SortKey> for index::SortKey {
    fn from(k: SortKey) -> Self {
        match k {
            SortKey::Name => Self::Name,
            SortKey::Path => Self::Path,
            SortKey::Size => Self::Size,
            SortKey::Modified => Self::Modified,
        }
    }
}

impl From<index::SortKey> for SortKey {
    fn from(k: index::SortKey) -> Self {
        match k {
            index::SortKey::Name => Self::Name,
            index::SortKey::Path => Self::Path,
            index::SortKey::Size => Self::Size,
            index::SortKey::Modified => Self::Modified,
        }
    }
}

impl From<Order> for index::Order {
    fn from(o: Order) -> Self {
        match o {
            Order::Ascending => Self::Ascending,
            Order::Descending => Self::Descending,
        }
    }
}

impl From<index::Order> for Order {
    fn from(o: index::Order) -> Self {
        match o {
            index::Order::Ascending => Self::Ascending,
            index::Order::Descending => Self::Descending,
        }
    }
}

impl Condition {
    /// 인덱스의 조건 트리로 바꾼다. 잘못된 정규식은 오류 값이다.
    pub fn to_filter(&self) -> Result<index::Filter, index::FilterError> {
        Ok(match self {
            Self::NameContains { part, case } => index::Filter::name_contains(part, (*case).into()),
            Self::NameWildcard { pattern, case } => {
                index::Filter::name_wildcard(pattern, (*case).into())?
            }
            Self::NameRegex { pattern, case } => {
                index::Filter::name_regex(pattern, (*case).into())?
            }
            Self::Under { folder } => index::Filter::under(folder),
            Self::Size { min, max } => index::Filter::size(*min, *max),
            Self::Modified { min, max } => index::Filter::modified(*min, *max),
            Self::Attributes { mask } => index::Filter::attributes(*mask),
            Self::Extension { extension } => index::Filter::extension(extension),
            Self::And(all) => index::Filter::and(
                all.iter()
                    .map(Self::to_filter)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Self::Or(any) => index::Filter::or(
                any.iter()
                    .map(Self::to_filter)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Self::Not(inner) => index::Filter::not(inner.to_filter()?),
        })
    }
}

impl QueryMessage {
    /// 인덱스 질의로 바꾼다. 잘못된 정규식은 오류 값이다.
    pub fn to_query(&self) -> Result<index::Query, index::FilterError> {
        let mut query = index::Query::filter(self.condition.to_filter()?);
        if let Some((key, order)) = self.sort {
            query = query.sort(key.into(), order.into());
        }
        if let Some(n) = self.limit {
            query = query.limit(n as usize);
        }
        Ok(query)
    }
}

impl ResumeMessage {
    /// [`facet_index::Index::resume`]에 줄 결과 수 상한.
    pub fn limit(&self) -> usize {
        self.limit as usize
    }
}

impl From<index::Hit> for HitMessage {
    fn from(h: index::Hit) -> Self {
        Self {
            path: h.path,
            name: h.name,
            size: h.size,
            modified: h.modified,
            attributes: h.attributes,
        }
    }
}

impl From<HitMessage> for index::Hit {
    fn from(h: HitMessage) -> Self {
        Self {
            path: h.path,
            name: h.name,
            size: h.size,
            modified: h.modified,
            attributes: h.attributes,
        }
    }
}

impl PageMessage {
    /// 인덱스의 결과 조각을 응답으로 바꾼다. 이어 받기 표지가 있으면 `keep`에 넘겨 보관하고,
    /// `keep`이 돌려준 번호를 응답의 표지로 쓴다. 만드는 중인 볼륨은 비워 두고, 권한으로 거르지
    /// 않았으므로 전체 개수는 상한이 아니다.
    pub fn from_page(page: index::Page, keep: impl FnOnce(index::Cursor) -> u64) -> Self {
        Self {
            hits: page.hits.into_iter().map(HitMessage::from).collect(),
            total: page.total as u64,
            total_is_upper_bound: false,
            next: page.next.map(keep),
            building: Vec::new(),
        }
    }
}

impl From<index::FilterError> for ErrorMessage {
    fn from(e: index::FilterError) -> Self {
        Self {
            kind: ErrorKind::InvalidQuery,
            message: e.to_string(),
        }
    }
}

impl From<index::ResumeError> for ErrorMessage {
    fn from(e: index::ResumeError) -> Self {
        Self {
            kind: ErrorKind::StaleCursor,
            message: e.to_string(),
        }
    }
}

impl From<DecodeError> for ErrorMessage {
    fn from(e: DecodeError) -> Self {
        Self {
            kind: ErrorKind::BadMessage,
            message: e.to_string(),
        }
    }
}
