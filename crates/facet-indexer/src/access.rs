//! 질의 결과의 폴더 권한 거르기. 플랫폼과 상관없는 코드다. 실제 권한 검사는 [`Caller`]를 구현한
//! 쪽(서비스는 파이프 클라이언트의 토큰으로 검사하는 [`crate::caller::PipeCaller`])이 한다.
//!
//! 인덱서 서비스는 권한 경계다(ADR-0003). 질의한 사용자가 목록을 읽을 수 없는 폴더 아래 항목은
//! 결과에 내보내지 않는다. 결과 항목의 조상 폴더(볼륨 루트부터 항목이 든 폴더까지)마다 「목록
//! 읽기」 권한을 본다. 파일 자체의 권한은 보지 않는다(폴더 목록을 읽을 수 있으면 이름은 이미 보인다).
//!
//! 검사 결과는 사용자(SID)마다 폴더 경로로 [`AccessCache`]에 기억한다. 폴더 보안이 바뀌거나(USN
//! 보안 변경 레코드) 어떤 경로에 다른 폴더가 놓이면(만들기·이름바꾸기·이동) 그 경로 아래 기억을
//! 버린다([`AccessCache::forget_under`]).

use std::collections::{HashMap, HashSet};
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

/// 경로 구분자.
const SEPARATOR: char = '\\';

/// 기억하는 폴더 수의 상한(모든 사용자를 합쳐). 넘으면 모두 버리고 다시 채운다. 버릴 때 훑는
/// 비용과 메모리를 묶어 두려는 값이다.
pub const MAX_CACHED_FOLDERS: usize = 200_000;

/// 질의한 사용자. 연결마다 하나다.
pub trait Caller: Send + Sync {
    /// 사용자를 가르는 열쇠(SID 문자열, 예: `S-1-5-21-...`). 검사 결과를 이 열쇠로 기억한다.
    /// 사용자를 알 수 없으면 `None`이고, 그때는 기억하지 않는다.
    fn user(&self) -> Option<String>;

    /// 폴더 `folder`(전체 경로, 볼륨 루트는 `C:\`처럼 구분자로 끝난다)의 목록을 읽을 수 있는가.
    /// 알 수 없으면(폴더가 사라짐, 보안 설명자를 읽지 못함 등) `false`다.
    fn can_list(&self, folder: &str) -> bool;
}

/// 모든 폴더를 읽을 수 있는 사용자. 권한을 거르지 않는 쪽(서비스 안, 시험)이 쓴다.
#[derive(Debug, Clone, Copy, Default)]
pub struct Everyone;

impl Caller for Everyone {
    fn user(&self) -> Option<String> {
        None
    }

    fn can_list(&self, _: &str) -> bool {
        true
    }
}

/// 항목 경로 `path`의 조상 폴더 경로들: 볼륨 루트(`C:\`)부터 항목이 든 폴더까지. 항목 자신은
/// 담지 않는다. 볼륨 루트는 구분자로 끝나고 나머지는 구분자 없이 끝난다.
pub fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    let mut first = true;
    path.match_indices(SEPARATOR).map(move |(at, _)| {
        if first {
            first = false;
            &path[..=at]
        } else {
            &path[..at]
        }
    })
}

/// 기억할 열쇠: 대소문자를 접은 경로(NTFS 경로는 대소문자를 가리지 않는다).
fn key(folder: &str) -> String {
    folder.chars().flat_map(char::to_lowercase).collect()
}

/// 사용자마다 폴더 권한 검사 결과를 기억한다. 여러 연결과 USN 따라가기가 함께 쓴다.
#[derive(Debug, Default)]
pub struct AccessCache {
    /// 사용자 → (접은 폴더 경로 → 목록을 읽을 수 있는가).
    users: RwLock<HashMap<String, HashMap<String, bool>>>,
}

impl AccessCache {
    /// 빈 기억.
    pub fn new() -> Self {
        Self::default()
    }

    fn read(&self) -> RwLockReadGuard<'_, HashMap<String, HashMap<String, bool>>> {
        // 값을 넣고 빼기만 하므로 잠근 채 패닉한 쪽이 있어도 기억은 온전하다.
        self.users.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, HashMap<String, HashMap<String, bool>>> {
        self.users.write().unwrap_or_else(|e| e.into_inner())
    }

    /// 기억하는 폴더 수(모든 사용자를 합쳐).
    pub fn len(&self) -> usize {
        self.read().values().map(HashMap::len).sum()
    }

    /// 기억하는 것이 없는가.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `caller`가 경로 `path`의 항목을 볼 수 있는가: 조상 폴더([`ancestors`])를 모두 목록 읽기 할
    /// 수 있는가. 기억에 없는 폴더만 `caller`에게 묻고 그 답을 기억한다.
    pub fn visible(&self, caller: &dyn Caller, path: &str) -> bool {
        let Some(user) = caller.user() else {
            return ancestors(path).all(|folder| caller.can_list(folder));
        };
        let mut unknown = Vec::new();
        {
            let users = self.read();
            let known = users.get(&user);
            for folder in ancestors(path) {
                let k = key(folder);
                match known.and_then(|m| m.get(&k)) {
                    Some(false) => return false,
                    Some(true) => {}
                    None => unknown.push((folder, k)),
                }
            }
        }
        if unknown.is_empty() {
            return true;
        }
        // 잠금 밖에서 묻는다(검사는 느릴 수 있다). 하나라도 막히면 거기서 그만 묻는다.
        let mut answers = Vec::with_capacity(unknown.len());
        let mut visible = true;
        for (folder, k) in unknown {
            let can = caller.can_list(folder);
            answers.push((k, can));
            if !can {
                visible = false;
                break;
            }
        }
        let mut users = self.write();
        let total: usize = users.values().map(HashMap::len).sum();
        if total + answers.len() > MAX_CACHED_FOLDERS {
            users.clear();
        }
        users.entry(user).or_default().extend(answers);
        visible
    }

    /// 폴더 `folder`와 그 아래 모든 폴더의 기억을 모든 사용자에게서 버린다.
    pub fn forget_under(&self, folder: &str) {
        self.forget_all_under([folder]);
    }

    /// 폴더들과 그 아래 모든 폴더의 기억을 모든 사용자에게서 한 번에 버린다.
    pub fn forget_all_under<'a>(&self, folders: impl IntoIterator<Item = &'a str>) {
        let gone: HashSet<String> = folders
            .into_iter()
            .map(|f| key(f.strip_suffix(SEPARATOR).unwrap_or(f)))
            .collect();
        if gone.is_empty() {
            return;
        }
        let mut users = self.write();
        for known in users.values_mut() {
            known.retain(|k, _| !is_under_any(k, &gone));
        }
        users.retain(|_, known| !known.is_empty());
    }

    /// 모든 기억을 버린다.
    pub fn clear(&self) {
        self.write().clear();
    }
}

/// 접은 경로 `k`가 `gone`의 어느 폴더(구분자 없이 끝나는 접은 경로)이거나 그 아래인가.
fn is_under_any(k: &str, gone: &HashSet<String>) -> bool {
    let k = k.strip_suffix(SEPARATOR).unwrap_or(k);
    if gone.contains(k) {
        return true;
    }
    k.match_indices(SEPARATOR)
        .any(|(at, _)| gone.contains(&k[..at]))
}
