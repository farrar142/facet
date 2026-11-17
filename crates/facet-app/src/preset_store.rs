//! 일괄 이름 바꾸기 규칙 묶음 저장소의 앱 쪽 상태(#283).
//!
//! 코어의 [`Presets`]를 메모리에 들고 있다가 바뀔 때마다 사본을 일꾼 스레드에 넘겨 파일에 쓴다. UI 스레드는 디스크
//! 쓰기를 기다리지 않는다. 쓰기가 실패하면 까닭을 콜백으로 알리고 앱은 계속 돈다. 읽기는 처음 창을 열 때 한 번이다.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use facet_core::batch_rename::Rule;
use facet_core::rename_presets::{self, Loaded, Presets};

/// 파일을 읽을 때 알릴 일.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoadNotice {
    /// 파일 전체가 깨져 옆에 남겼는가. 안쪽은 남긴 파일 이름(옮기지 못했으면 `None`).
    pub broken: Option<Option<String>>,
}

pub struct PresetStore {
    presets: Presets,
    notice: LoadNotice,
    writer: Option<Sender<Presets>>,
}

impl PresetStore {
    /// `path`의 파일을 읽어 연다. `path`가 `None`이면(측정 모드 등) 메모리에만 둔다. `on_error`는 쓰기가 실패할
    /// 때 일꾼 스레드에서 불린다.
    pub fn open(path: Option<PathBuf>, on_error: impl Fn(String) + Send + 'static) -> PresetStore {
        let Some(path) = path else {
            return PresetStore {
                presets: Presets::new(),
                notice: LoadNotice::default(),
                writer: None,
            };
        };
        let (presets, notice) = match rename_presets::load(&path) {
            Loaded::Missing => (Presets::new(), LoadNotice::default()),
            Loaded::Read(presets) => (presets, LoadNotice::default()),
            Loaded::Broken { kept, .. } => (
                Presets::new(),
                LoadNotice {
                    broken: Some(
                        kept.and_then(|k| k.file_name().map(|n| n.to_string_lossy().into_owned())),
                    ),
                },
            ),
        };
        let (sender, receiver) = channel();
        std::thread::Builder::new()
            .name("rename-presets-writer".to_owned())
            .spawn(move || write_loop(&path, &receiver, &on_error))
            .ok();
        PresetStore {
            presets,
            notice,
            writer: Some(sender),
        }
    }

    pub fn presets(&self) -> &Presets {
        &self.presets
    }

    pub fn notice(&self) -> &LoadNotice {
        &self.notice
    }

    /// 묶음을 고치고 성공하면 저장을 맡긴다.
    pub fn edit<E>(&mut self, f: impl FnOnce(&mut Presets) -> Result<(), E>) -> Result<(), E> {
        f(&mut self.presets)?;
        self.persist();
        Ok(())
    }

    /// 마지막에 쓴 규칙을 남기고 저장을 맡긴다.
    pub fn set_last(&mut self, rules: Vec<Rule>) {
        if self.presets.last() == Some(rules.as_slice()) {
            return;
        }
        self.presets.set_last(rules);
        self.persist();
    }

    fn persist(&self) {
        if let Some(writer) = &self.writer {
            let _ = writer.send(self.presets.clone());
        }
    }
}

/// 가장 새로 온 상태만 쓴다.
fn write_loop(path: &Path, receiver: &Receiver<Presets>, on_error: &dyn Fn(String)) {
    while let Ok(mut presets) = receiver.recv() {
        while let Ok(newer) = receiver.try_recv() {
            presets = newer;
        }
        if let Err(error) = rename_presets::save(path, &presets) {
            on_error(error.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use facet_core::batch_rename::{RenameRule, Rule};

    fn rules(find: &str) -> Vec<Rule> {
        RenameRule {
            find: find.to_owned(),
            ..RenameRule::default()
        }
        .to_rules()
    }

    #[test]
    fn 저장한_묶음과_마지막_규칙은_다시_열어도_남는다() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("p.toml");
        let mut store = PresetStore::open(Some(path.clone()), |_| {});
        store.edit(|p| p.save_preset("사진", rules("a"))).unwrap();
        store.set_last(rules("b"));
        drop(store); // 쓰기 스레드가 남은 것을 쓰고 끝난다.
        for _ in 0..200 {
            if let Loaded::Read(p) = rename_presets::load(&path)
                && p.last().is_some()
            {
                assert_eq!(p.find("사진").unwrap().rules, rules("a"));
                assert_eq!(p.last().unwrap(), rules("b").as_slice());
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        panic!("묶음 파일이 쓰이지 않았다");
    }

    #[test]
    fn 깨진_파일은_옆에_남기고_알린다() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("p.toml");
        std::fs::write(&path, "이건 TOML이 아니다 ===").unwrap();
        let store = PresetStore::open(Some(path), |_| {});
        let Some(Some(kept)) = &store.notice().broken else {
            panic!("깨짐을 알려야 한다");
        };
        assert!(kept.contains("broken"));
        assert!(store.presets().presets().is_empty());
    }

    #[test]
    fn 거부된_조작은_저장소를_바꾸지_않는다() {
        let mut store = PresetStore::open(None, |_| {});
        assert!(store.edit(|p| p.save_preset(" ", rules("a"))).is_err());
        assert!(store.presets().presets().is_empty());
    }
}
