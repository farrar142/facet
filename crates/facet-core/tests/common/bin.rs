//! 테스트가 휴지통에 넣은 항목 치우기. 덮어쓰기·실행취소가 실제 휴지통에 넣은 항목을 테스트가 끝날 때 지워 휴지통에
//! 시험 찌꺼기가 쌓이지 않게 한다. 쓰는 테스트 파일은 `#[path = "common/bin.rs"] mod bin;`으로 넣는다.

use std::fs;

use facet_win::recycle::RecycledItem;

/// 휴지통 항목들을 휴지통에서 지운다: 내용(`$R...`)과 정보 파일(`$I...`). 이미 없으면(복원함) 그대로 둔다.
pub fn purge<'a>(items: impl IntoIterator<Item = &'a RecycledItem>) {
    for item in items {
        let recycled = item.recycled();
        if recycled.is_dir() {
            let _ = fs::remove_dir_all(recycled);
        } else {
            let _ = fs::remove_file(recycled);
        }
        if let Some(name) = recycled.file_name() {
            let name = name.to_string_lossy().replacen("$R", "$I", 1);
            let _ = fs::remove_file(recycled.with_file_name(name));
        }
    }
}
