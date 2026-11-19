//! #423: zip 공급자: zip 파일 안을 위치로 연다(ADR-0004).

use std::ffi::OsStr;
use std::path::Path;

use crate::provider::{Candidate, PathSegment, Provider, ResolveError, Resolved};
use crate::watch::{Watch, WatchError, WatchEvent};
use crate::{Item, ListError, Location};

/// #423: 확장자 `.zip`과 zip 시그니처가 맞는 파일인가.
pub fn is_zip_file(_path: &Path) -> bool {
    todo!("#423")
}

/// #423: 같은 이름 수와 손상 여부.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ArchiveNotes {
    /// #423
    pub duplicates: usize,
    /// #423
    pub damaged: bool,
}

/// #423: zip 공급자.
pub struct ZipProvider;

impl Provider for ZipProvider {
    fn list(&self, _location: &Location) -> Result<Vec<Item>, ListError> {
        todo!("#423")
    }

    fn child(&self, _location: &Location, _name: &OsStr) -> Location {
        todo!("#423")
    }

    fn parent(&self, _location: &Location) -> Option<Location> {
        todo!("#423")
    }

    fn segments(&self, _location: &Location) -> Vec<PathSegment> {
        todo!("#423")
    }

    fn resolve(&self, _text: &str) -> Result<Resolved, ResolveError> {
        todo!("#423")
    }

    fn complete(&self, _text: &str) -> Vec<Candidate> {
        todo!("#423")
    }

    fn watch(
        &self,
        _location: &Location,
        _on_event: Box<dyn FnMut(WatchEvent) + Send>,
    ) -> Result<Watch, WatchError> {
        todo!("#423")
    }

    fn item(&self, _location: &Location, _name: &OsStr) -> Result<Option<Item>, ListError> {
        todo!("#423")
    }
}
