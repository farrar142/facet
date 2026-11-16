//! 끌어 놓기의 결과 결정: 놓은 순간 복사·이동·고르기 메뉴·놓을 수 없음 중 무엇을 할지 정하는 순수 함수.
//!
//! 화면도 OLE도 파일 시스템도 부르지 않는다(경로 글자만 본다). 화면 문장은 없다(ADR-0007): 놓을 수 없는 까닭은
//! [`Refusal`]로만 알린다. 뒤 티켓의 OLE 드롭 대상이 [`decide`]를 부르고, 정한 복사·이동은
//! [`crate::window::App::start_drop`]으로 작업 대기열에 넣는다.
//!
//! 정한 것:
//! - **기본 효과**: 끄는 항목이 모두 놓을 폴더와 같은 볼륨이면 이동, 하나라도 다른 볼륨이면 복사. 볼륨은 경로의
//!   볼륨 루트 글자로 비교한다(`C:`는 대소문자 무시, UNC는 서버·공유까지, `\\?\` 형식도 같은 볼륨으로 본다).
//!   루트를 알 수 없는 경로(상대 경로 등)는 볼륨을 모르므로 다른 볼륨으로 보아 복사한다(원본을 지우지 않는 쪽).
//! - **수식 키**: Ctrl은 복사, Shift는 이동. **Ctrl+Shift는 둘 다 없는 것으로 본다**(탐색기에서는 바로 가기 만들기인데
//!   이 spec 범위 밖이라, 어느 한쪽을 몰래 고르지 않고 기본 효과를 따른다).
//! - **원본 앱이 허용한 효과**([`AllowedEffects`]): 허용하지 않은 효과는 고르지 않는다. 고른 효과가 허용되지
//!   않으면 허용된 다른 효과로 바꾼다(탐색기와 같다). 둘 다 허용하지 않으면 놓을 수 없음.
//! - **오른쪽 버튼**: 수식 키와 상관없이 고르기 메뉴([`Decision::Menu`])로 간다. 메뉴에는 허용되고 실제로 할 수 있는
//!   항목만 든다(항목이 하나도 없으면 놓을 수 없음).
//! - **놓을 수 없음**: 파일 목록이 아닌 데이터, 놓을 자리가 폴더가 아님(파일 위 등), 끄는 항목 없음, 자기 자신이나
//!   자기 하위 폴더로 놓기(복사·이동 모두), 원본이 이미 그 폴더 바로 안에 있는 이동(아무 일 없음, 모두 그럴 때만).
//!   같은 폴더로의 **복사는 허용**한다(작업이 「- 복사본」을 만든다).
//! - 경로는 정규화한 절대 경로라고 본다(`..`을 풀지 않는다).

use std::path::{Component, Path, PathBuf, Prefix};

use crate::job::{is_within, same_path};

/// 놓을 자리.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropTarget {
    /// 폴더 위(또는 폴더 목록의 빈 곳이면 그 폴더).
    Folder(PathBuf),
    /// 파일 위처럼 놓을 수 없는 곳.
    NotAFolder,
}

/// 끌어 온 데이터의 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropData {
    /// 파일 목록(`CF_HDROP`).
    Files,
    /// 글자·그림·가상 파일 내용 등 파일 목록이 아닌 것.
    Other,
}

/// 놓는 순간 눌려 있던 수식 키.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    /// Ctrl.
    pub ctrl: bool,
    /// Shift.
    pub shift: bool,
}

/// 원본 앱이 허용한 효과(`DROPEFFECT_COPY`·`DROPEFFECT_MOVE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllowedEffects {
    /// 복사를 허용.
    pub copy: bool,
    /// 이동을 허용.
    pub mov: bool,
}

impl AllowedEffects {
    /// 둘 다 허용.
    pub const ALL: Self = Self {
        copy: true,
        mov: true,
    };
}

/// 끌어 놓기 한 번의 입력.
#[derive(Debug, Clone, Copy)]
pub struct DropRequest<'a> {
    /// 끄는 항목의 경로들.
    pub sources: &'a [PathBuf],
    /// 놓을 자리.
    pub target: &'a DropTarget,
    /// 수식 키.
    pub modifiers: Modifiers,
    /// 오른쪽 버튼으로 끌었는가.
    pub right_button: bool,
    /// 원본 앱이 허용한 효과.
    pub allowed: AllowedEffects,
    /// 데이터 종류.
    pub data: DropData,
}

/// 실제로 할 효과.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropEffect {
    /// 복사.
    Copy,
    /// 이동.
    Move,
}

/// 놓을 수 없는 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// 파일 목록이 아닌 데이터.
    NotFiles,
    /// 끄는 항목이 없음.
    NoSources,
    /// 놓을 자리가 폴더가 아님.
    NotAFolder,
    /// 자기 자신이나 자기 하위 폴더로 놓음.
    IntoItself,
    /// 원본이 이미 그 폴더 바로 안에 있어 이동할 것이 없음.
    AlreadyThere,
    /// 원본 앱이 허용한 효과가 없음.
    NoEffectAllowed,
}

/// 결정.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// 복사한다.
    Copy,
    /// 이동한다.
    Move,
    /// 오른쪽 버튼: 사용자에게 고르게 한다. 담긴 효과만 메뉴 항목으로 보인다(적어도 하나).
    Menu { copy: bool, mov: bool },
    /// 놓을 수 없다(아무 일도 하지 않고 금지 커서).
    Refuse(Refusal),
}

/// 끌어 놓기의 결과를 정한다. 파일 시스템을 부르지 않는다.
pub fn decide(request: &DropRequest<'_>) -> Decision {
    if request.data != DropData::Files {
        return Decision::Refuse(Refusal::NotFiles);
    }
    if request.sources.is_empty() {
        return Decision::Refuse(Refusal::NoSources);
    }
    let DropTarget::Folder(target) = request.target else {
        return Decision::Refuse(Refusal::NotAFolder);
    };
    if request
        .sources
        .iter()
        .any(|source| is_within(target, source))
    {
        return Decision::Refuse(Refusal::IntoItself);
    }
    let allowed = request.allowed;
    if !allowed.copy && !allowed.mov {
        return Decision::Refuse(Refusal::NoEffectAllowed);
    }
    let already_there = request
        .sources
        .iter()
        .all(|source| source.parent().is_some_and(|p| same_path(p, target)));
    let can_move = allowed.mov && !already_there;

    if request.right_button {
        return if allowed.copy || can_move {
            Decision::Menu {
                copy: allowed.copy,
                mov: can_move,
            }
        } else {
            Decision::Refuse(Refusal::AlreadyThere)
        };
    }

    let m = request.modifiers;
    let wanted = match (m.ctrl, m.shift) {
        (true, false) => DropEffect::Copy,
        (false, true) => DropEffect::Move,
        _ => default_effect(request.sources, target),
    };
    match wanted {
        DropEffect::Copy if allowed.copy => Decision::Copy,
        DropEffect::Move if allowed.mov => {
            if already_there {
                Decision::Refuse(Refusal::AlreadyThere)
            } else {
                Decision::Move
            }
        }
        // 고른 효과가 허용되지 않았으면 허용된 다른 효과로 바꾼다.
        DropEffect::Copy => {
            if can_move {
                Decision::Move
            } else {
                Decision::Refuse(Refusal::AlreadyThere)
            }
        }
        DropEffect::Move => Decision::Copy,
    }
}

/// 끄는 동안 커서에 보일 효과. 고르기 메뉴(오른쪽 버튼)면 메뉴에 든 효과 가운데 기본 효과(없으면 다른 하나)를
/// 보인다. 놓을 수 없으면 `None`.
pub fn cursor_effect(decision: Decision, sources: &[PathBuf], target: &Path) -> Option<DropEffect> {
    match decision {
        Decision::Copy => Some(DropEffect::Copy),
        Decision::Move => Some(DropEffect::Move),
        Decision::Menu { copy, mov } => match (default_effect(sources, target), copy, mov) {
            (DropEffect::Move, _, true) | (DropEffect::Copy, false, true) => Some(DropEffect::Move),
            (_, true, _) => Some(DropEffect::Copy),
            _ => None,
        },
        Decision::Refuse(_) => None,
    }
}

/// 수식 키가 없을 때의 효과: 모두 같은 볼륨이면 이동, 아니면 복사.
pub fn default_effect(sources: &[PathBuf], target: &Path) -> DropEffect {
    match volume_key(target) {
        Some(volume)
            if sources
                .iter()
                .all(|s| volume_key(s).as_ref() == Some(&volume)) =>
        {
            DropEffect::Move
        }
        _ => DropEffect::Copy,
    }
}

/// 두 경로가 같은 볼륨에 있는지(루트 글자 비교, 모르면 `false`).
pub fn same_volume(a: &Path, b: &Path) -> bool {
    match (volume_key(a), volume_key(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// 볼륨 루트를 가리키는 소문자 키. 드라이브는 문자, UNC는 서버·공유까지. 루트를 모르면 `None`.
fn volume_key(path: &Path) -> Option<String> {
    match path.components().next()? {
        Component::Prefix(prefix) => Some(match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                format!("disk:{}", (letter as char).to_ascii_lowercase())
            }
            Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => format!(
                "unc:{}\\{}",
                server.to_string_lossy().to_lowercase(),
                share.to_string_lossy().to_lowercase()
            ),
            Prefix::Verbatim(name) => format!("verbatim:{}", name.to_string_lossy().to_lowercase()),
            Prefix::DeviceNS(name) => format!("device:{}", name.to_string_lossy().to_lowercase()),
        }),
        Component::RootDir => Some("root".to_owned()),
        _ => None,
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn paths(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn 볼륨_키는_대소문자와_verbatim을_무시하고_unc는_공유까지_본다() {
        assert!(same_volume(Path::new(r"C:\a"), Path::new(r"c:\B\c")));
        assert!(same_volume(Path::new(r"\\?\C:\a"), Path::new(r"C:\b")));
        assert!(!same_volume(Path::new(r"C:\a"), Path::new(r"D:\a")));
        assert!(same_volume(
            Path::new(r"\\Srv\Share\a"),
            Path::new(r"\\srv\share\b")
        ));
        assert!(!same_volume(
            Path::new(r"\\srv\share\a"),
            Path::new(r"\\srv\other\a")
        ));
        assert!(!same_volume(Path::new("rel"), Path::new("rel")));
    }

    #[test]
    fn 커서_효과는_메뉴면_기본_효과를_따른다() {
        let same = paths(&[r"C:\a\x"]);
        let other = paths(&[r"D:\a\x"]);
        let t = Path::new(r"C:\t");
        let menu = |copy, mov| Decision::Menu { copy, mov };
        assert_eq!(
            cursor_effect(menu(true, true), &same, t),
            Some(DropEffect::Move)
        );
        assert_eq!(
            cursor_effect(menu(true, true), &other, t),
            Some(DropEffect::Copy)
        );
        assert_eq!(
            cursor_effect(menu(true, false), &same, t),
            Some(DropEffect::Copy)
        );
        assert_eq!(
            cursor_effect(menu(false, true), &other, t),
            Some(DropEffect::Move)
        );
        assert_eq!(
            cursor_effect(Decision::Copy, &same, t),
            Some(DropEffect::Copy)
        );
        assert_eq!(
            cursor_effect(Decision::Refuse(Refusal::IntoItself), &same, t),
            None
        );
    }

    #[test]
    fn 섞인_볼륨은_복사다() {
        let s = paths(&[r"C:\a\x", r"D:\a\y"]);
        assert_eq!(default_effect(&s, Path::new(r"C:\t")), DropEffect::Copy);
    }
}
