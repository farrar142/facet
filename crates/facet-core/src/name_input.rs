//! 이름 입력란: 고른 항목의 이름을 그 행 자리에서 고쳐 쓰는 입력란(F2).
//!
//! 지금은 한글 IME를 확인하는 데만 쓴다. 확정해도 입력값을 돌려줄 뿐 파일 이름은 바꾸지 않는다.
//! 실제 이름바꾸기는 작업으로 S4에서 만든다.

use crate::Item;

/// 열려 있는 이름 입력란.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameInput {
    index: usize,
    original: String,
}

/// 확정한 이름 입력란의 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnteredName {
    /// 입력란을 열 때의 이름.
    pub original: String,
    /// 확정한 입력값.
    pub entered: String,
}

impl NameInput {
    /// `items` 가운데 `selected` 항목의 이름 입력란을 연다. 현재 이름이 채워진다.
    ///
    /// 선택이 없거나 목록 밖이면 열지 않는다(`None`).
    pub fn open(items: &[Item], selected: Option<usize>) -> Option<Self> {
        let index = selected?;
        let item = items.get(index)?;
        Some(Self {
            index,
            original: item.name.to_string_lossy().into_owned(),
        })
    }

    /// 입력란이 놓인 행.
    pub fn index(&self) -> usize {
        self.index
    }

    /// 입력란에 처음 채운 현재 이름.
    pub fn original(&self) -> &str {
        &self.original
    }

    /// 입력을 확정한다. 입력값을 돌려줄 뿐 파일 시스템은 건드리지 않는다.
    pub fn confirm(self, entered: &str) -> EnteredName {
        EnteredName {
            original: self.original,
            entered: entered.to_owned(),
        }
    }

    /// 입력을 취소한다. 아무것도 남기지 않는다.
    pub fn cancel(self) {}
}

impl EnteredName {
    /// 확정한 입력값을 사용자에게 알리는 문구.
    pub fn message(&self) -> String {
        format!(
            "입력한 이름: 「{}」 (원래 이름 「{}」, 실제 파일 이름은 바꾸지 않았습니다)",
            self.entered, self.original
        )
    }
}
