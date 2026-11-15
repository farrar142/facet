//! 견본 인덱스: 개발용 하위 명령 `serve-sample`과 테스트가 파이프 서버에 물리는 메모리 인덱스.

use facet_index::{ATTRIBUTE_DIRECTORY, Index, NewItem};

/// 견본 인덱스의 볼륨 이름.
pub const SAMPLE_VOLUME: &str = "S:";

/// NTFS 루트 폴더의 항목 번호.
const ROOT: u64 = 5;

/// 견본 인덱스. 루트 아래 `문서`·`사진` 폴더와 `보고서 N.txt`(N = 1..=`reports`) 파일들, 그 밖에
/// 몇 개의 파일이 든다. 크기는 보고서 N이 N×100바이트다.
pub fn sample_index(reports: u64) -> Index {
    let mut index = Index::new(SAMPLE_VOLUME, ROOT);
    let mut put = |number, parent, name: &str, size, attributes| {
        index
            .insert(NewItem {
                number,
                parent,
                name,
                size,
                modified: 133_000_000_000_000_000 + number,
                attributes,
            })
            .expect("견본 이름은 올바르다");
    };
    put(100, ROOT, "문서", 0, ATTRIBUTE_DIRECTORY);
    put(101, ROOT, "사진", 0, ATTRIBUTE_DIRECTORY);
    put(102, 101, "여름 바다.jpg", 2_000_000, 0x20);
    put(103, 101, "README.md", 1_000, 0x20);
    for n in 1..=reports {
        put(1000 + n, 100, &format!("보고서 {n}.txt"), n * 100, 0x20);
    }
    index
}
