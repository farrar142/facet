//! 질의 조건 트리를 합성 인덱스에 대한 공개 인터페이스로만 확인한다.

use facet_index::{
    ATTRIBUTE_DIRECTORY, ATTRIBUTE_HIDDEN, ATTRIBUTE_SYSTEM, Case, Filter, FilterError, Index,
    NewItem, Query,
};

const ROOT: u64 = 5;

fn add(
    index: &mut Index,
    number: u64,
    parent: u64,
    name: &str,
    size: u64,
    modified: u64,
    attributes: u32,
) {
    index
        .insert(NewItem {
            number,
            parent,
            name,
            size,
            modified,
            attributes,
        })
        .unwrap();
}

/// ```text
/// C:\Docs\Report.TXT      size 100 date 1000
/// C:\Docs\notes.txt       size 200 date 2000
/// C:\Docs\Sub\deep.md     size 300 date 3000 (숨김)
/// C:\Docs\Sub\Archive.tar.gz size 400 date 4000
/// C:\Docs2\other.txt      size 500 date 5000
/// C:\pagefile.sys         size 600 date 6000 (숨김+시스템)
/// C:\noext                size 0 date 0
/// ```
fn sample() -> Index {
    let mut i = Index::new(r"C:\", ROOT);
    add(&mut i, 10, ROOT, "Docs", 0, 0, ATTRIBUTE_DIRECTORY);
    add(&mut i, 11, 10, "Sub", 0, 0, ATTRIBUTE_DIRECTORY);
    add(&mut i, 12, ROOT, "Docs2", 0, 0, ATTRIBUTE_DIRECTORY);
    add(&mut i, 20, 10, "Report.TXT", 100, 1000, 0x20);
    add(&mut i, 21, 10, "notes.txt", 200, 2000, 0x20);
    add(
        &mut i,
        22,
        11,
        "deep.md",
        300,
        3000,
        0x20 | ATTRIBUTE_HIDDEN,
    );
    add(&mut i, 23, 11, "Archive.tar.gz", 400, 4000, 0x20);
    add(&mut i, 24, 12, "other.txt", 500, 5000, 0x20);
    add(
        &mut i,
        25,
        ROOT,
        "pagefile.sys",
        600,
        6000,
        ATTRIBUTE_HIDDEN | ATTRIBUTE_SYSTEM,
    );
    add(&mut i, 26, ROOT, "noext", 0, 0, 0x20);
    i
}

fn run(f: Filter) -> Vec<String> {
    let mut v: Vec<String> = sample()
        .query(&Query::filter(f))
        .into_iter()
        .map(|h| h.path)
        .collect();
    // 구분자를 가장 작은 글자로 보고 정렬한다(`Docs\x`가 `Docs2` 앞).
    v.sort_by_key(|p| p.replace('\\', "\0"));
    v
}

#[test]
fn 와일드카드는_이름_전체와_맞는다() {
    let f = Filter::name_wildcard("*.txt", Case::Insensitive).unwrap();
    assert_eq!(
        run(f),
        [
            r"C:\Docs\Report.TXT",
            r"C:\Docs\notes.txt",
            r"C:\Docs2\other.txt"
        ]
    );
    let f = Filter::name_wildcard("note?.txt", Case::Insensitive).unwrap();
    assert_eq!(run(f), [r"C:\Docs\notes.txt"]);
    // 이름 전체와 맞아야 하므로 앞부분만 맞는 것은 아니다.
    let f = Filter::name_wildcard("note", Case::Insensitive).unwrap();
    assert!(run(f).is_empty());
    // 와일드카드 아닌 글자는 글자 그대로다(`.`이 아무 글자가 아니다).
    let f = Filter::name_wildcard("deep.md", Case::Insensitive).unwrap();
    assert_eq!(run(f), [r"C:\Docs\Sub\deep.md"]);
    let f = Filter::name_wildcard("deepxmd", Case::Insensitive).unwrap();
    assert!(run(f).is_empty());
}

#[test]
fn 정규식_이름_조건() {
    let f = Filter::name_regex(r"^(notes|deep)\.", Case::Insensitive).unwrap();
    assert_eq!(run(f), [r"C:\Docs\Sub\deep.md", r"C:\Docs\notes.txt"]);
}

#[test]
fn 잘못된_정규식은_panic_없이_오류_값이다() {
    let err = Filter::name_regex("(", Case::Insensitive).unwrap_err();
    assert!(matches!(err, FilterError::InvalidRegex(_)));
    assert!(!err.to_string().is_empty());
    assert!(Filter::name_regex("[a-", Case::Sensitive).is_err());
}

#[test]
fn 대소문자는_기본으로_무시하고_고르면_구별한다() {
    assert_eq!(Case::default(), Case::Insensitive);
    for make in [
        |c| Filter::name_contains("report.txt", c),
        |c| Filter::name_wildcard("report.*", c).unwrap(),
        |c| Filter::name_regex("^report", c).unwrap(),
    ] {
        assert_eq!(run(make(Case::Insensitive)), [r"C:\Docs\Report.TXT"]);
        assert!(run(make(Case::Sensitive)).is_empty());
    }
    assert_eq!(
        run(Filter::name_contains("Report.TXT", Case::Sensitive)),
        [r"C:\Docs\Report.TXT"]
    );
}

#[test]
fn 경로_범위는_하위_폴더까지_포함하고_폴더_자신과_이름만_비슷한_폴더는_뺀다() {
    assert_eq!(
        run(Filter::under(r"C:\Docs")),
        [
            r"C:\Docs\Report.TXT",
            r"C:\Docs\Sub",
            r"C:\Docs\Sub\Archive.tar.gz",
            r"C:\Docs\Sub\deep.md",
            r"C:\Docs\notes.txt"
        ]
    );
    // 끝의 구분자와 대소문자는 상관없다.
    assert_eq!(
        run(Filter::under(r"c:\docs\sub\")),
        [r"C:\Docs\Sub\Archive.tar.gz", r"C:\Docs\Sub\deep.md"]
    );
    assert_eq!(run(Filter::under(r"C:\Docs2")), [r"C:\Docs2\other.txt"]);
}

#[test]
fn 크기_범위() {
    assert_eq!(
        run(Filter::size(Some(200), Some(300))),
        [r"C:\Docs\Sub\deep.md", r"C:\Docs\notes.txt"]
    );
    assert_eq!(
        run(Filter::size(Some(500), None)),
        [r"C:\Docs2\other.txt", r"C:\pagefile.sys"]
    );
    assert_eq!(run(Filter::size(None, Some(0))).len(), 4); // 폴더 3 + noext
}

#[test]
fn 수정_날짜_범위() {
    assert_eq!(
        run(Filter::modified(Some(1000), Some(2000))),
        [r"C:\Docs\Report.TXT", r"C:\Docs\notes.txt"]
    );
    assert_eq!(
        run(Filter::modified(Some(6000), None)),
        [r"C:\pagefile.sys"]
    );
}

#[test]
fn 속성_조건() {
    assert_eq!(
        run(Filter::attributes(ATTRIBUTE_DIRECTORY)),
        [r"C:\Docs", r"C:\Docs\Sub", r"C:\Docs2"]
    );
    assert_eq!(
        run(Filter::attributes(ATTRIBUTE_HIDDEN)),
        [r"C:\Docs\Sub\deep.md", r"C:\pagefile.sys"]
    );
    // 비트를 모두 가져야 한다.
    assert_eq!(
        run(Filter::attributes(ATTRIBUTE_HIDDEN | ATTRIBUTE_SYSTEM)),
        [r"C:\pagefile.sys"]
    );
}

#[test]
fn 확장자_조건() {
    assert_eq!(
        run(Filter::extension("txt")),
        [
            r"C:\Docs\Report.TXT",
            r"C:\Docs\notes.txt",
            r"C:\Docs2\other.txt"
        ]
    );
    assert_eq!(run(Filter::extension(".TXT")).len(), 3);
    // 마지막 점 뒤만 확장자다.
    assert_eq!(
        run(Filter::extension("gz")),
        [r"C:\Docs\Sub\Archive.tar.gz"]
    );
    assert!(run(Filter::extension("tar")).is_empty());
}

#[test]
fn 그리고_또는_아님을_겹쳐_쓴다() {
    let txt = || Filter::extension("txt");
    let docs = || Filter::under(r"C:\Docs");
    assert_eq!(
        run(Filter::and([txt(), docs()])),
        [r"C:\Docs\Report.TXT", r"C:\Docs\notes.txt"]
    );
    assert_eq!(
        run(Filter::or([
            Filter::extension("md"),
            Filter::extension("sys")
        ])),
        [r"C:\Docs\Sub\deep.md", r"C:\pagefile.sys"]
    );
    assert_eq!(
        run(Filter::and([txt(), Filter::not(docs())])),
        [r"C:\Docs2\other.txt"]
    );
    // (txt 또는 md) 그리고 아님(숨김) 그리고 크기 150 이상
    let tree = Filter::and([
        Filter::or([txt(), Filter::extension("md")]),
        Filter::not(Filter::attributes(ATTRIBUTE_HIDDEN)),
        Filter::size(Some(150), None),
    ]);
    assert_eq!(run(tree), [r"C:\Docs\notes.txt", r"C:\Docs2\other.txt"]);
    // 빈 그리고는 모두, 빈 또는은 아무것도 아니다.
    assert_eq!(run(Filter::and([])).len(), 10);
    assert!(run(Filter::or([])).is_empty());
    assert!(run(Filter::not(Filter::and([]))).is_empty());
}

#[test]
fn 경로를_못_만드는_항목은_아님_조건에도_나오지_않는다() {
    let mut i = sample();
    add(&mut i, 90, 999, "고아.txt", 1, 1, 0x20);
    let hits = i.query(&Query::filter(Filter::not(Filter::extension("zzz"))));
    assert_eq!(hits.len(), 10);
}

#[test]
fn 결과_수_제한과_이름_부분_일치_조건() {
    let i = sample();
    let q = Query::filter(Filter::name_contains("DOCS", Case::Insensitive)).limit(2);
    assert_eq!(i.query(&q).len(), 2);
    assert_eq!(
        i.query(&Query::filter(Filter::extension("txt")).limit(0))
            .len(),
        0
    );
}

#[test]
fn 경로_부분_일치는_폴더_이름_조각으로_그_아래_항목을_낸다() {
    // 대소문자를 무시하고, 폴더 자신의 경로에도 든다.
    let f = Filter::path_contains(r"docs\sub", Case::Insensitive);
    assert_eq!(
        run(f),
        [
            r"C:\Docs\Sub",
            r"C:\Docs\Sub\Archive.tar.gz",
            r"C:\Docs\Sub\deep.md"
        ]
    );
    // 이름 부분 일치와 달리 폴더 이름도 본다: `sub\`는 이름에 없고 경로에만 있다.
    let f = Filter::path_contains(r"SUB\", Case::Insensitive);
    assert_eq!(
        run(f),
        [r"C:\Docs\Sub\Archive.tar.gz", r"C:\Docs\Sub\deep.md"]
    );
    // 대소문자를 구별하면 `Docs\`만 맞고 `docs\`는 아니다.
    let f = Filter::path_contains(r"Docs\", Case::Sensitive);
    assert_eq!(run(f).len(), 5);
    let f = Filter::path_contains(r"docs\", Case::Sensitive);
    assert!(run(f).is_empty());
    // 볼륨 루트도 경로의 일부다.
    assert_eq!(
        run(Filter::path_contains("c:", Case::Insensitive)).len(),
        10
    );
    assert!(run(Filter::path_contains("d:", Case::Insensitive)).is_empty());
}

#[test]
fn 경로_부분_일치는_한글_경로도_맞는다() {
    let mut i = Index::new(r"C:\", ROOT);
    add(&mut i, 10, ROOT, "사진", 0, 0, ATTRIBUTE_DIRECTORY);
    add(&mut i, 11, 10, "여름 바다.jpg", 10, 1, 0x20);
    add(&mut i, 12, ROOT, "Ünï", 0, 0, ATTRIBUTE_DIRECTORY);
    add(&mut i, 13, 12, "x.txt", 1, 1, 0x20);
    let paths = |f: Filter| -> Vec<String> {
        let mut v: Vec<String> = i
            .query(&Query::filter(f))
            .into_iter()
            .map(|h| h.path)
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        paths(Filter::path_contains(r"사진\여름", Case::Insensitive)),
        [r"C:\사진\여름 바다.jpg"]
    );
    assert_eq!(
        paths(Filter::path_contains(r"ünï\", Case::Insensitive)),
        [r"C:\Ünï\x.txt"]
    );
}

#[test]
fn 경로_와일드카드는_경로_전체와_맞는다() {
    let f = Filter::path_wildcard(r"C:\Docs\*.txt", Case::Insensitive).unwrap();
    assert_eq!(run(f), [r"C:\Docs\Report.TXT", r"C:\Docs\notes.txt"]);
    // `*`는 `\`도 넘는다.
    let f = Filter::path_wildcard(r"*\Sub\*", Case::Insensitive).unwrap();
    assert_eq!(
        run(f),
        [r"C:\Docs\Sub\Archive.tar.gz", r"C:\Docs\Sub\deep.md"]
    );
    let f = Filter::path_wildcard(r"c:\docs?\*", Case::Insensitive).unwrap();
    assert_eq!(run(f), [r"C:\Docs2\other.txt"]);
    // 경로 전체와 맞아야 하므로 일부만 맞는 것은 아니다.
    let f = Filter::path_wildcard(r"Docs\notes", Case::Insensitive).unwrap();
    assert!(run(f).is_empty());
    // 와일드카드 아닌 글자는 글자 그대로다.
    let f = Filter::path_wildcard(r"C:\Docs\Sub\deepxmd", Case::Insensitive).unwrap();
    assert!(run(f).is_empty());
    // 대소문자 구별.
    let f = Filter::path_wildcard(r"c:\docs\*", Case::Sensitive).unwrap();
    assert!(run(f).is_empty());
    let f = Filter::path_wildcard(r"C:\Docs\*", Case::Sensitive).unwrap();
    assert_eq!(run(f).len(), 5);
}

#[test]
fn 경로_조건은_그리고_또는_아님과_섞인다() {
    let sub = || Filter::path_contains(r"docs\sub\", Case::Insensitive);
    let f = Filter::and([sub(), Filter::extension("md")]);
    assert_eq!(run(f), [r"C:\Docs\Sub\deep.md"]);
    let f = Filter::and([sub(), Filter::not(Filter::extension("md"))]);
    assert_eq!(run(f), [r"C:\Docs\Sub\Archive.tar.gz"]);
    let f = Filter::or([
        Filter::path_wildcard(r"C:\Docs2\*", Case::Insensitive).unwrap(),
        Filter::name_contains("noext", Case::Insensitive),
    ]);
    assert_eq!(run(f), [r"C:\Docs2\other.txt", r"C:\noext"]);
    // 아님: 경로 조건에 맞지 않는 항목(경로를 만들 수 있는 것만).
    let f = Filter::not(Filter::path_contains("docs", Case::Insensitive));
    assert_eq!(run(f), [r"C:\noext", r"C:\pagefile.sys"]);
    // 폴더 아래 조건과도 섞는다.
    let f = Filter::and([
        Filter::under(r"C:\Docs"),
        Filter::path_wildcard(r"*.txt", Case::Insensitive).unwrap(),
    ]);
    assert_eq!(run(f), [r"C:\Docs\Report.TXT", r"C:\Docs\notes.txt"]);
}

#[test]
fn 경로를_못_만드는_항목은_경로_조건에_나오지_않는다() {
    let mut i = sample();
    add(&mut i, 90, 999, "고아.txt", 1, 1, 0x20);
    let all = Filter::path_contains("", Case::Insensitive);
    assert_eq!(i.query(&Query::filter(all)).len(), 10);
    let w = Filter::path_wildcard("*", Case::Insensitive).unwrap();
    assert_eq!(i.query(&Query::filter(w)).len(), 10);
}
