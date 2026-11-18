//! 셸 미리보기 처리기 조회(#342): 그 확장자에 미리보기 처리기(`IPreviewHandler`)가 등록돼 있는지 레지스트리만 읽어
//! 본다. 처리기를 싣지 않으므로 UI 프로세스에서 불러도 셸 확장이 들어오지 않는다(ADR-0006).
//!
//! 조회는 셸의 연결 규칙대로 `AssocQueryStringW(ASSOCSTR_SHELLEXTENSION)`에 미리보기 처리기의 인터페이스 번호를 넘긴다
//! (확장자 키, 그 ProgID, `SystemFileAssociations`를 셸과 같은 차례로 본다). 모든 파일(`*`)에 걸린 처리기는 보지 않는다.

/// 미리보기 처리기의 셸 확장 자리 이름(`IPreviewHandler`의 인터페이스 번호).
#[cfg(windows)]
const PREVIEW_HANDLER_IID: &str = "{8895b1c6-b41f-4c1c-a562-0d564250836f}";

/// 그 확장자(점 없음)에 셸 미리보기 처리기가 등록돼 있는가. 빈 확장자, 점·경로 글자가 든 확장자, Windows가 아니면
/// `false`다.
pub fn has_preview_handler(extension: &str) -> bool {
    if extension.is_empty() || extension.contains(['.', '\\', '/', '\0', '*']) {
        return false;
    }
    #[cfg(windows)]
    {
        imp::has_preview_handler(extension)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
mod imp {
    use windows::Win32::UI::Shell::{
        ASSOCF_NOTRUNCATE, ASSOCSTR_SHELLEXTENSION, AssocQueryStringW,
    };
    use windows::core::{HSTRING, PWSTR};

    pub fn has_preview_handler(extension: &str) -> bool {
        let assoc = HSTRING::from(format!(".{extension}"));
        let extra = HSTRING::from(super::PREVIEW_HANDLER_IID);
        // CLSID 글자(중괄호 포함 38글자)와 끝 널이 들어가는 넉넉한 칸.
        let mut out = [0u16; 128];
        let mut len = out.len() as u32;
        // SAFETY: 두 글자는 이 함수가 사는 동안 살아 있는 널로 끝나는 UTF-16이고, `out`은 `len` 단위의 쓸 수 있는
        // 칸이며 `len`은 그 크기를 담은 지역 변수다. 함수는 레지스트리만 읽고 처리기를 싣지 않는다.
        let result = unsafe {
            AssocQueryStringW(
                ASSOCF_NOTRUNCATE,
                ASSOCSTR_SHELLEXTENSION,
                &assoc,
                &extra,
                Some(PWSTR(out.as_mut_ptr())),
                &mut len,
            )
        };
        result.is_ok() && out.first().is_some_and(|c| *c != 0)
    }
}
