//! 견본 인덱스: 개발용 하위 명령 `serve-sample`과 테스트가 파이프 서버에 물리는 메모리 인덱스.

use facet_index::{ATTRIBUTE_DIRECTORY, Index, NewItem};
use facet_protocol::{ErrorMessage, QueryMessage, StatusMessage};

use crate::session::{Backend, Caller, Cancel, Cursor, Page};
use crate::volumes::Volumes;

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

/// 견본 인덱스 하나(볼륨 [`SAMPLE_VOLUME`])로 답하는 인덱스 쪽.
///
/// 견본의 경로(`S:\...`)는 실제 폴더가 아니어서 권한을 검사할 수 없으므로 **권한으로 거르지
/// 않는다**. 개발용 `serve-sample`과 파이프 테스트만 쓴다. 인덱서 서비스는
/// [`Volumes`]를 그대로 물려 늘 거른다.
#[derive(Debug)]
pub struct SampleBackend(pub Volumes);

impl SampleBackend {
    /// `reports`개의 보고서가 든 견본 인덱스([`sample_index`])로 답한다.
    pub fn new(reports: u64) -> Self {
        Self(Volumes::with_index(SAMPLE_VOLUME, sample_index(reports)))
    }
}

impl Backend for SampleBackend {
    fn search(
        &self,
        query: &QueryMessage,
        _: &dyn Caller,
        cancel: &Cancel,
    ) -> Result<Page, ErrorMessage> {
        self.0.search(query, cancel)
    }

    fn resume(&self, cursor: Cursor, limit: usize, _: &dyn Caller) -> Result<Page, ErrorMessage> {
        self.0.resume(cursor, limit)
    }

    fn status(&self) -> StatusMessage {
        self.0.status()
    }
}
