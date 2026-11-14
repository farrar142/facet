//! 인덱스의 「항목 넣기 → 질의 → 결과」를 공개 인터페이스로만 확인한다.

use facet_index::{ATTRIBUTE_DIRECTORY, Hit, Index, InsertError, NewItem, Query};

const ROOT: u64 = 5;

fn folder(number: u64, parent: u64, name: &str) -> NewItem<'_> {
    NewItem {
        number,
        parent,
        name,
        size: 0,
        modified: 0,
        attributes: ATTRIBUTE_DIRECTORY,
    }
}

fn file(number: u64, parent: u64, name: &str, size: u64) -> NewItem<'_> {
    NewItem {
        number,
        parent,
        name,
        size,
        modified: 133_000_000_000_000_000 + number,
        attributes: 0x20,
    }
}

/// C:\ 아래 작은 나무:
///
/// ```text
/// C:\문서\보고서 2026.txt
/// C:\문서\사진\Photo 🎉.JPG
/// C:\문서\사진\ÄRGER.txt
/// C:\README.md
/// C:\Program Files\
/// ```
fn sample() -> Index {
    let mut index = Index::new(r"C:\", ROOT);
    // 자식을 부모보다 먼저 넣어도 된다(MFT 순서는 부모가 먼저라는 보장이 없다).
    index.insert(file(30, 20, "Photo 🎉.JPG", 300)).unwrap();
    index.insert(folder(10, ROOT, "문서")).unwrap();
    index.insert(folder(20, 10, "사진")).unwrap();
    index.insert(file(11, 10, "보고서 2026.txt", 110)).unwrap();
    index.insert(file(31, 20, "ÄRGER.txt", 310)).unwrap();
    index.insert(file(40, ROOT, "README.md", 40)).unwrap();
    index.insert(folder(50, ROOT, "Program Files")).unwrap();
    index
}

fn paths(hits: &[Hit]) -> Vec<&str> {
    let mut v: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
    v.sort();
    v
}

#[test]
fn 일치하는_항목만_볼륨_루트부터_만든_경로와_함께_돌아온다() {
    let index = sample();
    let hits = index.query(&Query::name_contains("o"));
    assert_eq!(
        paths(&hits),
        vec![r"C:\Program Files", r"C:\문서\사진\Photo 🎉.JPG"]
    );
}

#[test]
fn 결과는_이름_크기_날짜_속성을_함께_돌려준다() {
    let index = sample();
    let hits = index.query(&Query::name_contains("보고서"));
    assert_eq!(
        hits,
        vec![Hit {
            path: r"C:\문서\보고서 2026.txt".to_owned(),
            name: "보고서 2026.txt".to_owned(),
            size: 110,
            modified: 133_000_000_000_000_011,
            attributes: 0x20,
        }]
    );
}

#[test]
fn 폴더도_결과에_나온다() {
    let index = sample();
    let hits = index.query(&Query::name_contains("사진"));
    assert_eq!(paths(&hits), vec![r"C:\문서\사진"]);
    assert_eq!(
        hits[0].attributes & ATTRIBUTE_DIRECTORY,
        ATTRIBUTE_DIRECTORY
    );
}

#[test]
fn 대소문자를_무시한다() {
    let index = sample();
    assert_eq!(
        paths(&index.query(&Query::name_contains("readme"))),
        vec![r"C:\README.md"]
    );
    assert_eq!(
        paths(&index.query(&Query::name_contains("PHOTO 🎉.jpg"))),
        vec![r"C:\문서\사진\Photo 🎉.JPG"]
    );
    // ASCII 밖의 대소문자도 같다.
    assert_eq!(
        paths(&index.query(&Query::name_contains("ärger"))),
        vec![r"C:\문서\사진\ÄRGER.txt"]
    );
    assert_eq!(
        paths(&index.query(&Query::name_contains("ÄRG"))),
        vec![r"C:\문서\사진\ÄRGER.txt"]
    );
}

#[test]
fn 한글과_이모지_조각으로_찾는다() {
    let index = sample();
    assert_eq!(
        paths(&index.query(&Query::name_contains("고서 20"))),
        vec![r"C:\문서\보고서 2026.txt"]
    );
    assert_eq!(
        paths(&index.query(&Query::name_contains("🎉"))),
        vec![r"C:\문서\사진\Photo 🎉.JPG"]
    );
    assert!(index.query(&Query::name_contains("🦀")).is_empty());
}

#[test]
fn 이름_경계를_넘어서는_일치하지_않는다() {
    let index = sample();
    // 「문서」 다음에 넣은 「사진」을 이어 붙인 글자로는 찾지 않는다.
    assert!(index.query(&Query::name_contains("문서사진")).is_empty());
    assert!(index.query(&Query::name_contains("서사")).is_empty());
    // 경로 구분자도 이름의 일부가 아니다.
    assert!(index.query(&Query::name_contains(r"문서\")).is_empty());
}

#[test]
fn 일치하는_것이_없으면_빈_결과() {
    let index = sample();
    assert!(index.query(&Query::name_contains("없는 이름")).is_empty());
    assert!(index.query(&Query::name_contains("a\0b")).is_empty());
}

#[test]
fn 빈_조각은_모든_항목과_일치한다() {
    let index = sample();
    assert_eq!(index.query(&Query::name_contains("")).len(), 7);
}

#[test]
fn 결과_수_제한을_주면_그_수_이하로_돌아온다() {
    let mut index = Index::new(r"D:\", ROOT);
    for n in 0..50 {
        index
            .insert(file(100 + n, ROOT, &format!("파일 {n}.txt"), n))
            .unwrap();
    }
    assert_eq!(index.query(&Query::name_contains("파일")).len(), 50);
    assert_eq!(
        index.query(&Query::name_contains("파일").limit(10)).len(),
        10
    );
    assert_eq!(index.query(&Query::name_contains("파일").limit(0)).len(), 0);
    assert_eq!(
        index.query(&Query::name_contains("파일").limit(80)).len(),
        50
    );
    // 제한 안의 결과도 모두 일치하는 항목이다.
    for hit in index.query(&Query::name_contains("1").limit(5)) {
        assert!(hit.name.contains('1'), "{}", hit.name);
    }
}

#[test]
fn 하드_링크는_이름마다_항목_하나다() {
    let mut index = sample();
    // 같은 파일(번호 11)이 다른 폴더에 다른 이름으로 하나 더 있다.
    index.insert(file(11, 50, "보고서 링크.txt", 110)).unwrap();
    assert_eq!(
        paths(&index.query(&Query::name_contains("보고서"))),
        vec![
            r"C:\Program Files\보고서 링크.txt",
            r"C:\문서\보고서 2026.txt"
        ]
    );
}

#[test]
fn 부모를_찾을_수_없는_항목은_결과에_나오지_않는다() {
    let mut index = sample();
    index.insert(file(60, 999, "고아.txt", 1)).unwrap();
    // 부모가 있지만 폴더가 아니다.
    index.insert(file(61, 40, "파일 아래.txt", 1)).unwrap();
    // 부모가 서로를 가리킨다.
    index.insert(folder(70, 71, "고리 가")).unwrap();
    index.insert(folder(71, 70, "고리 나")).unwrap();
    assert!(index.query(&Query::name_contains("고아")).is_empty());
    assert!(index.query(&Query::name_contains("파일 아래")).is_empty());
    assert!(index.query(&Query::name_contains("고리")).is_empty());
    // 걸러진 항목은 결과 수 제한을 차지하지 않는다.
    index.insert(folder(80, ROOT, "고리 다")).unwrap();
    assert_eq!(
        paths(&index.query(&Query::name_contains("고리").limit(1))),
        vec![r"C:\고리 다"]
    );
}

#[test]
fn 볼륨_루트는_끝의_구분자를_하나만_쓴다() {
    for root in [r"E:\", "E:"] {
        let mut index = Index::new(root, ROOT);
        index.insert(folder(10, ROOT, "가")).unwrap();
        index.insert(file(11, 10, "나.txt", 0)).unwrap();
        assert_eq!(
            paths(&index.query(&Query::name_contains("나"))),
            vec![r"E:\가\나.txt"]
        );
    }
}

#[test]
fn 쓸_수_없는_이름은_넣지_않는다() {
    let mut index = sample();
    assert_eq!(
        index.insert(file(90, ROOT, "", 0)),
        Err(InsertError::EmptyName)
    );
    assert_eq!(
        index.insert(file(91, ROOT, "a\0b", 0)),
        Err(InsertError::InvalidName)
    );
    assert_eq!(
        index.insert(file(92, ROOT, r"a\b", 0)),
        Err(InsertError::InvalidName)
    );
    assert_eq!(index.len(), 7);
    assert_eq!(index.query(&Query::name_contains("")).len(), 7);
}

#[test]
fn 항목_수와_메모리_사용을_알려준다() {
    let empty = Index::new(r"C:\", ROOT);
    assert!(empty.is_empty());
    let index = sample();
    assert_eq!(index.len(), 7);
    assert!(index.memory_bytes() > 0);
}
