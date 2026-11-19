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
    pub fn from_attributes(bits: u32) -> SyncState {
        const OFFLINE: u32 = 0x1000;
        const RECALL_ON_OPEN: u32 = 0x4_0000;
        const PINNED: u32 = 0x8_0000;
        const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
        if bits & PINNED != 0 {
            SyncState::AlwaysKeep
        } else if bits & (OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS) != 0 {
            SyncState::OnlineOnly
        } else {
            SyncState::OnDevice
        }
    }
}
