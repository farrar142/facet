//! 그 자리 이름바꾸기(F2, 새 폴더 직후)의 화면 상태: 무엇을 고치는지, 막힌 까닭, 확장자 바꿈 확인, 확정해 결과를
//! 기다리는 이름.
//!
//! 코어의 이름 확인([`validate_name`])·확장자 바꿈([`extension_changed`])·처음 선택 범위([`initial_selection`])를
//! 부를 뿐 문장은 만들지 않는다(ADR-0007, 문장은 [`crate::text`]). 실제 이름바꾸기는 화면이
//! [`facet_core::window::App::rename`]·[`facet_core::window::App::rename_new_folder`]로 부르고, 결과는 나중에
//! [`facet_core::window::Update::Edited`]로 온다.

use std::ffi::{OsStr, OsString};

use facet_core::edit::EditError;
use facet_core::name_input::{NameError, extension_changed, initial_selection, validate_name};

/// 입력란 곁에 보일 막힌 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// 입력 단계에서 막힌 이름.
    Name(NameError),
    /// 확정했으나 코어가 막았거나 실행 중에 실패했다.
    Edit(EditError),
    /// 여러 항목에 번호를 붙이는 일괄 이름 바꾸기가 막혔다.
    Batch(facet_core::window::BatchRenameError),
}

/// 확정(Enter·입력란 밖 클릭)을 어떻게 할지.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    /// 바꿀 것이 없다(원래 이름 그대로): 입력란을 닫는다.
    Close,
    /// 막혔다: 입력란과 까닭을 그대로 둔다.
    Stay,
    /// 확장자가 바뀐다: 한 번 묻는다(입력란은 그대로).
    AskExtension,
    /// 이 이름으로 코어 이름바꾸기를 부른다. 새 폴더 직후면 `after_new_folder`.
    Submit {
        name: String,
        after_new_folder: bool,
    },
}

/// 열린(또는 확정해 결과를 기다리는) 이름바꾸기 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rename {
    original_name: OsString,
    original: String,
    is_folder: bool,
    after_new_folder: bool,
    problem: Option<Problem>,
    asking_extension: bool,
    pending: Option<String>,
}

impl Rename {
    /// `original` 항목의 이름 입력란을 연다. `after_new_folder`면 방금 만든 새 폴더의 이름 입력란이다.
    pub fn open(original: impl AsRef<OsStr>, is_folder: bool, after_new_folder: bool) -> Self {
        let original = original.as_ref();
        Self {
            original_name: original.to_owned(),
            original: original.to_string_lossy().into_owned(),
            is_folder,
            after_new_folder,
            problem: None,
            asking_extension: false,
            pending: None,
        }
    }

    /// 고치는 항목의 원래 이름(입력란에 처음 채우는 글자).
    pub fn original(&self) -> &str {
        &self.original
    }

    /// 고치는 항목의 원래 이름 그대로(코어에 넘기고 목록에서 행을 찾는 데 쓴다).
    pub fn original_name(&self) -> &OsStr {
        &self.original_name
    }

    /// `text`로 입력란을 열 때 처음 선택할 범위(UTF-8 바이트): 파일은 확장자 앞까지, 폴더는 전체.
    pub fn selection(&self, text: &str) -> (usize, usize) {
        initial_selection(text, self.is_folder)
    }

    /// 입력란 곁에 보일 까닭.
    pub fn problem(&self) -> Option<Problem> {
        self.problem
    }

    /// 확장자 바꿈을 묻는 중인가.
    pub fn asking_extension(&self) -> bool {
        self.asking_extension
    }

    /// 확정해 결과를 기다리는 이름. 기다리는 동안 입력란은 닫혀 있다.
    pub fn pending(&self) -> Option<&str> {
        self.pending.as_deref()
    }

    /// 글자가 바뀌었다: 이름 확인을 다시 하고, 묻던 확장자 확인은 거둔다.
    pub fn edited(&mut self, text: &str) {
        self.asking_extension = false;
        self.problem = if text == self.original {
            None
        } else {
            validate_name(text).err().map(Problem::Name)
        };
    }

    /// 확정한다. 확장자를 묻는 중에 다시 확정하면(Enter) 「예」로 본다.
    pub fn confirm(&mut self, text: &str) -> Confirm {
        if text == self.original {
            return Confirm::Close;
        }
        if let Err(error) = validate_name(text) {
            self.asking_extension = false;
            self.problem = Some(Problem::Name(error));
            return Confirm::Stay;
        }
        if !self.asking_extension && extension_changed(&self.original, text, self.is_folder) {
            self.asking_extension = true;
            return Confirm::AskExtension;
        }
        self.asking_extension = false;
        Confirm::Submit {
            name: text.to_owned(),
            after_new_folder: self.after_new_folder,
        }
    }

    /// 확장자 바꿈 확인의 답. 「아니오」면 입력란으로 돌아간다(`Stay`).
    pub fn answer_extension(&mut self, yes: bool, text: &str) -> Confirm {
        if !self.asking_extension {
            return Confirm::Stay;
        }
        if yes {
            self.confirm(text)
        } else {
            self.asking_extension = false;
            Confirm::Stay
        }
    }

    /// Esc. 확장자를 묻는 중이면 「아니오」와 같이 입력란으로 돌아가고 `false`, 아니면 그만두고 `true`.
    pub fn cancel(&mut self) -> bool {
        if self.asking_extension {
            self.asking_extension = false;
            return false;
        }
        true
    }

    /// 코어에 넘겼다: 입력란을 닫고 결과를 기다린다.
    pub fn submitted(&mut self, name: &str) {
        self.problem = None;
        self.pending = Some(name.to_owned());
    }

    /// 번호 붙이기(여러 항목)가 막혔다: 까닭을 두고 입력란을 남긴다.
    pub fn failed_batch(&mut self, error: facet_core::window::BatchRenameError) {
        self.problem = Some(Problem::Batch(error));
        self.asking_extension = false;
    }

    /// 코어가 곧바로 막았거나 실행이 실패했다: 까닭을 두고 입력란을 다시 연다. 다시 열 글자(친 이름)를 돌려준다.
    pub fn failed(&mut self, error: EditError, entered: &str) -> String {
        self.problem = Some(Problem::Edit(error));
        self.asking_extension = false;
        self.pending.take().unwrap_or_else(|| entered.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use facet_win::file_ops::FileOpErrorKind;

    #[test]
    fn 파일은_확장자_앞까지_폴더는_전체를_고른다() {
        let file = Rename::open("보고서.txt", false, false);
        assert_eq!(file.selection("보고서.txt"), (0, "보고서".len()));
        let folder = Rename::open("새 폴더", true, true);
        assert_eq!(folder.selection("새 폴더"), (0, "새 폴더".len()));
        let dotfile = Rename::open(".gitignore", false, false);
        assert_eq!(dotfile.selection(".gitignore"), (0, ".gitignore".len()));
    }

    #[test]
    fn 칠_때마다_까닭이_바뀐다() {
        let mut rename = Rename::open("a.txt", false, false);
        rename.edited("a?.txt");
        assert_eq!(
            rename.problem(),
            Some(Problem::Name(NameError::ForbiddenChar('?')))
        );
        rename.edited("CON.txt");
        assert_eq!(rename.problem(), Some(Problem::Name(NameError::Reserved)));
        rename.edited("b.txt");
        assert_eq!(rename.problem(), None);
        rename.edited("");
        assert_eq!(rename.problem(), Some(Problem::Name(NameError::Empty)));
        rename.edited("a.txt");
        assert_eq!(rename.problem(), None);
    }

    #[test]
    fn 막힌_이름은_확정하지_않는다() {
        let mut rename = Rename::open("a.txt", false, false);
        assert_eq!(rename.confirm("a:b.txt"), Confirm::Stay);
        assert_eq!(
            rename.problem(),
            Some(Problem::Name(NameError::ForbiddenChar(':')))
        );
        assert_eq!(rename.confirm("끝점."), Confirm::Stay);
        assert_eq!(rename.confirm("끝공백 "), Confirm::Stay);
        assert_eq!(rename.pending(), None);
    }

    #[test]
    fn 원래_이름_그대로면_닫는다() {
        let mut rename = Rename::open("새 폴더", true, true);
        assert_eq!(rename.confirm("새 폴더"), Confirm::Close);
    }

    #[test]
    fn 확장자를_바꾸면_한_번_묻는다() {
        let mut rename = Rename::open("a.txt", false, false);
        assert_eq!(rename.confirm("a.md"), Confirm::AskExtension);
        assert!(rename.asking_extension());
        // 아니오: 입력란으로 돌아간다.
        assert_eq!(rename.answer_extension(false, "a.md"), Confirm::Stay);
        assert!(!rename.asking_extension());
        // 다시 확정하면 다시 묻고, Enter(다시 확정)는 「예」다.
        assert_eq!(rename.confirm("a.md"), Confirm::AskExtension);
        assert_eq!(
            rename.confirm("a.md"),
            Confirm::Submit {
                name: "a.md".into(),
                after_new_folder: false
            }
        );
    }

    #[test]
    fn 확장자_확인의_예는_넘기고_esc는_입력란으로() {
        let mut rename = Rename::open("a.txt", false, false);
        assert_eq!(rename.confirm("a"), Confirm::AskExtension);
        assert!(!rename.cancel());
        assert!(!rename.asking_extension());
        assert_eq!(rename.confirm("a"), Confirm::AskExtension);
        assert!(matches!(
            rename.answer_extension(true, "a"),
            Confirm::Submit { .. }
        ));
        // 묻지 않을 때의 Esc는 그만둔다.
        assert!(rename.cancel());
    }

    #[test]
    fn 고쳐_치면_확장자_확인을_거둔다() {
        let mut rename = Rename::open("a.txt", false, false);
        assert_eq!(rename.confirm("a.md"), Confirm::AskExtension);
        rename.edited("a.mdx");
        assert!(!rename.asking_extension());
        assert_eq!(rename.confirm("a.mdx"), Confirm::AskExtension);
    }

    #[test]
    fn 대소문자만_다른_확장자와_폴더는_묻지_않는다() {
        let mut file = Rename::open("a.txt", false, false);
        assert!(matches!(file.confirm("b.TXT"), Confirm::Submit { .. }));
        let mut folder = Rename::open("사진.2024", true, false);
        assert!(matches!(
            folder.confirm("사진.2025"),
            Confirm::Submit { .. }
        ));
    }

    #[test]
    fn 새_폴더_직후의_확정은_그렇다고_알린다() {
        let mut rename = Rename::open("새 폴더 (2)", true, true);
        assert_eq!(
            rename.confirm("자료"),
            Confirm::Submit {
                name: "자료".into(),
                after_new_folder: true
            }
        );
    }

    #[test]
    fn 실패하면_친_이름과_까닭으로_다시_연다() {
        let mut rename = Rename::open("a.txt", false, false);
        rename.submitted("b.txt");
        assert_eq!(rename.pending(), Some("b.txt"));
        let text = rename.failed(EditError::File(FileOpErrorKind::AccessDenied), "");
        assert_eq!(text, "b.txt");
        assert_eq!(rename.pending(), None);
        assert_eq!(
            rename.problem(),
            Some(Problem::Edit(EditError::File(
                FileOpErrorKind::AccessDenied
            )))
        );
        // 곧바로 막힌 것(같은 이름)은 기다리지 않았으므로 친 글자로 다시 연다.
        let text = rename.failed(EditError::NameTaken, "c.txt");
        assert_eq!(text, "c.txt");
        assert_eq!(rename.problem(), Some(Problem::Edit(EditError::NameTaken)));
        // 다시 치면 까닭이 바뀐다.
        rename.edited("d.txt");
        assert_eq!(rename.problem(), None);
    }
}
