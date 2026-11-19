//! #460: 단축키 모델(프리셋·사용자 변경·문맥·충돌).

use crate::command::{CommandId, Shortcut};

/// #460: 단축키 프리셋.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Preset {
    Explorer,
    TotalCommander,
}

impl Preset {
    /// #460: 설정 파일의 id(`explorer`·`total-commander`).
    pub fn as_str(self) -> &'static str {
        todo!("#460")
    }

    /// #460: id로 프리셋을 찾는다.
    pub fn from_id(_id: &str) -> Option<Preset> {
        todo!("#460")
    }
}

/// #460: 프리셋의 명령 단축키.
pub fn preset_shortcuts(_preset: Preset, _command: CommandId) -> Vec<Shortcut> {
    todo!("#460")
}

/// #460: 단축키 문맥.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Context {
    Global,
    List,
    Input,
}

/// #460: 명령의 문맥.
pub fn context_of(_command: CommandId) -> Context {
    todo!("#460")
}

/// #460: 한 키를 문맥이 겹치는 둘 이상의 명령이 쓰는 충돌.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub shortcut: Shortcut,
    pub commands: Vec<CommandId>,
    pub winner: CommandId,
}

/// #460: 프리셋 위에 사용자 변경을 덧씌운 단축키 모델.
#[derive(Debug, Clone)]
pub struct Keymap {}

impl Keymap {
    /// #460: 사용자 변경 없는 프리셋.
    pub fn new(_preset: Preset) -> Keymap {
        todo!("#460")
    }

    /// #460: 지금 프리셋.
    pub fn preset(&self) -> Preset {
        todo!("#460")
    }

    /// #460: 프리셋을 바꾼다(사용자 변경은 남음).
    pub fn set_preset(&mut self, _preset: Preset) {
        todo!("#460")
    }

    /// #460: 명령의 단축키(변경이 있으면 변경).
    pub fn shortcuts(&self, _command: CommandId) -> Vec<Shortcut> {
        todo!("#460")
    }

    /// #460: 명령의 단축키를 바꾼다.
    pub fn set_shortcuts(&mut self, _command: CommandId, _shortcuts: Vec<Shortcut>) {
        todo!("#460")
    }

    /// #460: 명령의 변경을 지운다.
    pub fn reset(&mut self, _command: CommandId) {
        todo!("#460")
    }

    /// #460: 모든 변경을 지운다.
    pub fn reset_all(&mut self) {
        todo!("#460")
    }

    /// #460: 명령에 사용자 변경이 있는가.
    pub fn is_overridden(&self, _command: CommandId) -> bool {
        todo!("#460")
    }

    /// #460: 포커스 문맥에서 키 하나를 명령으로.
    pub fn resolve(&self, _shortcut: Shortcut, _focus: Context) -> Option<CommandId> {
        todo!("#460")
    }

    /// #460: 모든 충돌.
    pub fn conflicts(&self) -> Vec<Conflict> {
        todo!("#460")
    }

    /// #460: 그 키를 그 명령에 주면 겹칠 다른 명령들.
    pub fn conflicts_with(&self, _command: CommandId, _shortcut: Shortcut) -> Vec<CommandId> {
        todo!("#460")
    }
}
