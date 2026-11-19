//! #451: 동기화 상태.

/// #451: 동기화 상태 다섯 가지(정렬 순서대로).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SyncState {
    OnlineOnly,
    OnDevice,
    AlwaysKeep,
    Syncing,
    Error,
}

impl SyncState {
    /// #451: 속성 값으로 온라인 전용·이 장치에 있음·항상 유지를 가른다.
    pub fn from_attributes(_bits: u32) -> SyncState {
        todo!("#451")
    }
}
