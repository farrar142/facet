//! 주소창의 고쳐 쓰는 모드 상태: 기다리는 자동 완성 요청, 받은 후보, 고른 후보.
//!
//! 후보 읽기는 코어가 일꾼 스레드에서 한다([`facet_core::window::App::complete_address`]). 여기서는 받은 결과가
//! 지금 기다리는 요청의 것인지 가리고, Tab·화살표로 후보를 차례로 고른다.

use facet_core::Candidate;
use facet_core::tab::CompletionId;

/// 고쳐 쓰는 모드 하나의 상태. 모드를 열 때 새로 만들고 닫을 때 버린다.
#[derive(Debug, Default)]
pub struct AddressEdit {
    /// 마지막으로 보낸 자동 완성 요청. 이 요청의 결과만 받는다.
    waiting: Option<CompletionId>,
    candidates: Vec<Candidate>,
    /// 고른 후보의 번호. 아직 고르지 않았으면 `None`이다.
    highlight: Option<usize>,
}

impl AddressEdit {
    /// 글자가 바뀌어 새 요청을 보냈다. 앞선 후보는 지금 글자와 맞지 않으므로 버린다.
    pub fn request(&mut self, id: CompletionId) {
        self.waiting = Some(id);
        self.candidates.clear();
        self.highlight = None;
    }

    /// 자동 완성 결과를 받는다. 기다리는 요청의 결과면 후보를 바꾸고 `true`다. 늦게 온 앞선 요청이나 다른
    /// 고쳐 쓰기의 결과면 버리고 `false`다.
    pub fn receive(&mut self, id: CompletionId, candidates: Option<Vec<Candidate>>) -> bool {
        if self.waiting != Some(id) {
            return false;
        }
        let Some(candidates) = candidates else {
            return false;
        };
        self.waiting = None;
        self.candidates = candidates;
        self.highlight = None;
        true
    }

    /// 후보들.
    pub fn candidates(&self) -> &[Candidate] {
        &self.candidates
    }

    /// 고른 후보의 번호.
    pub fn highlight(&self) -> Option<usize> {
        self.highlight
    }

    /// Tab·↓(`forward`)나 Shift+Tab·↑로 다음·앞 후보를 고른다. 끝에서는 반대쪽 끝으로 돈다. 처음이면 앞으로는
    /// 첫 후보, 뒤로는 마지막 후보다. 후보가 없으면 `None`이다.
    pub fn step(&mut self, forward: bool) -> Option<&Candidate> {
        let len = self.candidates.len();
        if len == 0 {
            return None;
        }
        let next = match (self.highlight, forward) {
            (None, true) => 0,
            (None, false) => len - 1,
            (Some(i), true) => (i + 1) % len,
            (Some(i), false) => (i + len - 1) % len,
        };
        self.highlight = Some(next);
        self.candidates.get(next)
    }

    /// `index`번 후보.
    pub fn candidate(&self, index: usize) -> Option<&Candidate> {
        self.candidates.get(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use facet_core::Location;
    use facet_core::details::Sort;
    use facet_core::tab::Tab;

    fn candidates(names: &[&str]) -> Vec<Candidate> {
        names
            .iter()
            .map(|name| Candidate {
                name: (*name).into(),
                location: Location::file_system(format!(r"C:\{name}")),
            })
            .collect()
    }

    /// 요청 번호는 코어의 탭만 만든다.
    fn ids(n: usize) -> Vec<CompletionId> {
        let mut tab = Tab::new(Location::file_system("."), Sort::default(), false);
        (0..n).map(|_| tab.complete("x").id()).collect()
    }

    fn name(c: Option<&Candidate>) -> Option<String> {
        c.map(|c| c.name.to_string_lossy().into_owned())
    }

    #[test]
    fn 기다리는_요청의_결과만_받는다() {
        let id = ids(2);
        let mut edit = AddressEdit::default();
        assert!(!edit.receive(id[0], Some(candidates(&["a"]))), "요청 전");
        edit.request(id[0]);
        edit.request(id[1]);
        assert!(!edit.receive(id[0], Some(candidates(&["a"]))), "앞선 요청");
        assert!(!edit.receive(id[1], None), "코어가 버린 결과");
        assert!(edit.candidates().is_empty());
        assert!(edit.receive(id[1], Some(candidates(&["b"]))));
        assert_eq!(name(edit.candidate(0)).as_deref(), Some("b"));
    }

    #[test]
    fn 글자가_바뀌면_후보와_고른_것을_버린다() {
        let id = ids(2);
        let mut edit = AddressEdit::default();
        edit.request(id[0]);
        edit.receive(id[0], Some(candidates(&["a", "b"])));
        edit.step(true);
        edit.request(id[1]);
        assert!(edit.candidates().is_empty());
        assert_eq!(edit.highlight(), None);
        assert_eq!(edit.step(true), None, "후보가 없으면 고를 것이 없다");
    }

    #[test]
    fn tab과_화살표로_후보를_돌며_고른다() {
        let id = ids(1);
        let mut edit = AddressEdit::default();
        edit.request(id[0]);
        edit.receive(id[0], Some(candidates(&["a", "b", "c"])));
        assert_eq!(name(edit.step(true)).as_deref(), Some("a"));
        assert_eq!(name(edit.step(true)).as_deref(), Some("b"));
        assert_eq!(name(edit.step(true)).as_deref(), Some("c"));
        assert_eq!(name(edit.step(true)).as_deref(), Some("a"), "끝에서 돈다");
        assert_eq!(name(edit.step(false)).as_deref(), Some("c"), "뒤로도 돈다");
        assert_eq!(edit.highlight(), Some(2));

        let mut edit = AddressEdit::default();
        edit.request(id[0]);
        edit.receive(id[0], Some(candidates(&["a", "b", "c"])));
        assert_eq!(
            name(edit.step(false)).as_deref(),
            Some("c"),
            "처음에 ↑면 마지막 후보"
        );
    }
}
