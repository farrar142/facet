// target: crates/facet-core/tests/spec_rename_presets_store.rs
// AC: #282: 일곱 규칙 종류의 저장·읽기 왕복, 사람이 읽을 수 있는 TOML, 이름 붙여 저장·바꾸기·지우기, 마지막 규칙, 깨진 묶음 건너뛰기와 보존, Missing·Broken, 임시 파일을 거친 저장, 파일 자리, GLOSSARY의 「규칙 묶음」
//! 일괄 이름 바꾸기의 규칙 묶음 저장소(`facet_core::rename_presets`)를 임시 폴더의 실제 파일로 확인한다: 모든 규칙 종류가
//! 저장한 뒤 읽어도 그대로이고, 파일은 사람이 읽을 수 있는 TOML이며, 이름 붙여 저장·같은 이름 다시 저장·이름 바꾸기·지우기의
//! 규칙과 거부, 마지막에 쓴 규칙, 손으로 쓴 깨진 묶음은 건너뛰고 까닭을 남기되 다시 저장해도 파일에서 사라지지 않음, 파일이
//! 없거나 통째로 깨졌을 때, 임시 파일을 거친 저장, `%APPDATA%` 아래 파일 자리를 본다. 파일 시스템만 쓰므로 모든 환경에서 돈다.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use facet_core::batch_rename::{CaseMode, Edge, Place, Rule, RuleKind, Target};
use facet_core::rename_presets::{
    self, FORMAT_VERSION, Loaded, PresetError, Presets, PresetsError,
};
use tempfile::TempDir;

/// 일곱 종류를 모두 쓰고 켜짐·대상·자리를 고루 섞은 규칙 목록.
fn every_kind() -> Vec<Rule> {
    let mut regex = Rule::new(RuleKind::Replace {
        find: r"^IMG_(\d+)$".to_owned(),
        replace: "사진_${1}".to_owned(),
        regex: true,
        case_sensitive: true,
        all: false,
    });
    regex.target = Target::Whole;
    let mut insert = Rule::new(RuleKind::Insert {
        text: "🎉 앞".to_owned(),
        from: Edge::End,
        at: 2,
    });
    insert.enabled = false;
    let mut case = Rule::new(RuleKind::Case(CaseMode::Title));
    case.target = Target::Extension;
    vec![
        regex,
        insert,
        Rule::new(RuleKind::Delete {
            from: Edge::Start,
            at: 1,
            count: 3,
        }),
        Rule::new(RuleKind::Number {
            start: 7,
            step: 5,
            digits: 4,
            place: Place::At {
                from: Edge::Start,
                at: 2,
            },
            separator: " - ".to_owned(),
        }),
        case,
        Rule::new(RuleKind::Extension {
            new: Some("jpg".to_owned()),
        }),
        Rule::new(RuleKind::Extension { new: None }),
        Rule::new(RuleKind::Date {
            format: "yyyy-MM-dd HH.mm.ss".to_owned(),
            place: Place::End,
            separator: "_".to_owned(),
        }),
        Rule::new(RuleKind::Number {
            start: 0,
            step: 1,
            digits: 0,
            place: Place::Start,
            separator: String::new(),
        }),
        Rule::new(RuleKind::Case(CaseMode::Sentence)),
    ]
}

fn replace(find: &str) -> Vec<Rule> {
    vec![Rule::new(RuleKind::Replace {
        find: find.to_owned(),
        replace: String::new(),
        regex: false,
        case_sensitive: false,
        all: true,
    })]
}

fn read(path: &Path) -> Presets {
    match rename_presets::load(path) {
        Loaded::Read(presets) => presets,
        other => panic!("읽지 못했다: {other:?}"),
    }
}

fn names(presets: &Presets) -> Vec<&str> {
    presets.presets().iter().map(|p| p.name.as_str()).collect()
}

#[test]
fn 모든_규칙_종류가_저장한_뒤_읽어도_그대로다() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("rename-presets.toml");
    let mut presets = Presets::new();
    presets.save_preset("모두", every_kind()).unwrap();
    presets.save_preset("하나", replace("IMG_")).unwrap();
    presets.save_preset("빈 묶음", Vec::new()).unwrap();
    presets.set_last(every_kind());

    rename_presets::save(&path, &presets).unwrap();
    let back = read(&path);
    assert_eq!(
        names(&back),
        ["모두", "하나", "빈 묶음"],
        "저장 차례 그대로"
    );
    assert_eq!(back.find("모두").unwrap().rules, every_kind());
    assert_eq!(back.find("하나").unwrap().rules, replace("IMG_"));
    assert!(back.find("빈 묶음").unwrap().rules.is_empty());
    assert_eq!(back.last(), Some(every_kind().as_slice()));
    assert!(back.skipped().is_empty());
    assert_eq!(back, presets);

    let text = presets.to_toml();
    assert_eq!(Presets::from_toml(&text).unwrap(), presets);
}

#[test]
fn 파일은_버전과_묶음_이름과_규칙_종류가_보이는_toml이다() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("rename-presets.toml");
    let mut presets = Presets::new();
    presets.save_preset("사진 정리", every_kind()).unwrap();
    rename_presets::save(&path, &presets).unwrap();

    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(FORMAT_VERSION, 1);
    assert!(
        text.lines().any(|line| line.trim() == "version = 1"),
        "{text}"
    );
    assert!(
        text.lines()
            .any(|line| line.trim_start().starts_with("name") && line.contains("사진 정리")),
        "묶음 이름이 글자 그대로 보인다: {text}"
    );
    for kind in [
        "replace",
        "insert",
        "delete",
        "number",
        "case",
        "extension",
        "date",
    ] {
        assert!(
            text.lines()
                .any(|line| line.trim_start().starts_with("kind") && line.contains(kind)),
            "규칙 종류 {kind}가 글자로 보인다: {text}"
        );
    }
}

#[test]
fn 같은_이름으로_다시_저장하면_그_묶음의_규칙이_바뀌고_빈_이름은_거부한다() {
    let mut presets = Presets::new();
    assert!(presets.presets().is_empty());
    presets.save_preset("Photos", replace("a")).unwrap();
    presets.save_preset("문서", replace("b")).unwrap();

    presets.save_preset("  photos ", replace("c")).unwrap();
    assert_eq!(presets.presets().len(), 2, "새 묶음이 생기지 않는다");
    assert_eq!(
        presets.presets()[0].rules,
        replace("c"),
        "자리는 그대로 규칙만 바뀐다"
    );
    assert_eq!(presets.find("PHOTOS").unwrap().rules, replace("c"));
    assert_eq!(presets.find(" 문서 ").unwrap().rules, replace("b"));
    assert!(presets.find("없음").is_none());

    assert_eq!(
        presets.save_preset("", replace("d")),
        Err(PresetError::EmptyName)
    );
    assert_eq!(
        presets.save_preset("   ", replace("d")),
        Err(PresetError::EmptyName)
    );
    assert_eq!(presets.presets().len(), 2);
}

#[test]
fn 이름을_바꾸고_지운다() {
    let mut presets = Presets::new();
    presets.save_preset("가", replace("1")).unwrap();
    presets.save_preset("나", replace("2")).unwrap();
    presets.save_preset("photos", replace("3")).unwrap();

    assert_eq!(
        presets.rename_preset("없음", "다"),
        Err(PresetError::NotFound)
    );
    assert_eq!(
        presets.rename_preset("가", " 나 "),
        Err(PresetError::NameTaken)
    );
    assert_eq!(
        presets.rename_preset("가", "  "),
        Err(PresetError::EmptyName)
    );
    assert_eq!(names(&presets), ["가", "나", "photos"], "거부하면 그대로");

    presets.rename_preset("가", "다").unwrap();
    presets.rename_preset("photos", "Photos").unwrap();
    assert_eq!(
        names(&presets),
        ["다", "나", "Photos"],
        "자리는 그대로, 대소문자만 바꾸기도 됨"
    );
    assert_eq!(presets.find("다").unwrap().rules, replace("1"));
    assert!(presets.find("가").is_none());

    assert_eq!(presets.delete_preset("가"), Err(PresetError::NotFound));
    presets.delete_preset(" 나 ").unwrap();
    assert_eq!(names(&presets), ["다", "Photos"]);
    presets.delete_preset("PHOTOS").unwrap();
    presets.delete_preset("다").unwrap();
    assert!(presets.presets().is_empty());
}

#[test]
fn 마지막에_쓴_규칙은_묶음_목록에_나오지_않고_저장한_뒤에도_그대로다() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("rename-presets.toml");
    let mut presets = Presets::new();
    assert_eq!(presets.last(), None);

    presets.set_last(replace("old"));
    presets.set_last(every_kind());
    assert!(
        presets.presets().is_empty(),
        "이름 없는 항목은 묶음이 아니다"
    );
    rename_presets::save(&path, &presets).unwrap();

    let back = read(&path);
    assert!(back.presets().is_empty());
    assert_eq!(back.last(), Some(every_kind().as_slice()));

    let mut empty = Presets::new();
    empty.save_preset("묶음", replace("x")).unwrap();
    rename_presets::save(&path, &empty).unwrap();
    assert_eq!(read(&path).last(), None);
}

/// 좋은 묶음 하나와 깨진 묶음 다섯(이름 없음, 모르는 종류, 빠진 값, 틀린 형, 앞과 같은 이름)을 손으로 쓴 파일.
const HAND_WRITTEN: &str = r#"version = 1

[[preset]]
name = "좋음"
[[preset.rule]]
kind = "case"
enabled = true
target = "stem"
mode = "upper"

[[preset]]
[[preset.rule]]
kind = "case"
enabled = true
target = "stem"
mode = "lower"

[[preset]]
name = "모르는 종류"
[[preset.rule]]
kind = "shuffle"
enabled = true
target = "stem"

[[preset]]
name = "빠진 값"
[[preset.rule]]
kind = "replace"
enabled = true
target = "stem"
replace = "b"
regex = false
case_sensitive = false
all = true

[[preset]]
name = "틀린 형"
[[preset.rule]]
kind = "insert"
enabled = true
target = "stem"
text = "x"
from = "start"
at = "셋"

[[preset]]
name = "좋음"
[[preset.rule]]
kind = "case"
enabled = true
target = "stem"
mode = "title"
"#;

#[test]
fn 깨진_묶음은_건너뛰고_까닭을_남기며_나머지는_읽힌다() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("rename-presets.toml");
    fs::write(&path, HAND_WRITTEN).unwrap();

    let presets = read(&path);
    assert_eq!(names(&presets), ["좋음"]);
    assert_eq!(
        presets.find("좋음").unwrap().rules,
        vec![Rule::new(RuleKind::Case(CaseMode::Upper))],
        "앞 묶음이 남고 같은 이름의 뒤 묶음은 건너뜀"
    );
    let skipped: Vec<Option<&str>> = presets
        .skipped()
        .iter()
        .map(|s| s.name.as_deref())
        .collect();
    assert_eq!(
        skipped,
        [
            None,
            Some("모르는 종류"),
            Some("빠진 값"),
            Some("틀린 형"),
            Some("좋음"),
        ],
        "파일 차례로"
    );
    for s in presets.skipped() {
        assert!(!s.error.is_empty(), "까닭이 있다: {s:?}");
    }
    assert!(path.exists(), "묶음 하나가 깨졌다고 파일을 옮기지 않는다");
}

#[test]
fn 다시_저장해도_깨진_묶음은_파일에서_사라지지_않는다() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("rename-presets.toml");
    fs::write(&path, HAND_WRITTEN).unwrap();

    let mut presets = read(&path);
    presets.save_preset("새 묶음", replace("IMG_")).unwrap();
    presets.set_last(replace("DSC"));
    rename_presets::save(&path, &presets).unwrap();

    let text = fs::read_to_string(&path).unwrap();
    for name in ["모르는 종류", "빠진 값", "틀린 형", "shuffle"] {
        assert!(text.contains(name), "{name}이 남아 있다: {text}");
    }
    let back = read(&path);
    assert_eq!(names(&back), ["좋음", "새 묶음"]);
    assert_eq!(back.find("새 묶음").unwrap().rules, replace("IMG_"));
    assert_eq!(back.last(), Some(replace("DSC").as_slice()));
    assert_eq!(back.skipped().len(), 5, "다시 읽어도 같은 묶음을 건너뜀");

    // 깨진 묶음을 손으로 고치면 다시 읽힌다(확장자 지우기는 종류 말고 더 쓸 값이 없다).
    let fixed = text.replace("\"shuffle\"", "\"extension\"");
    fs::write(&path, fixed).unwrap();
    let fixed = read(&path);
    assert_eq!(
        fixed.find("모르는 종류").unwrap().rules,
        vec![Rule::new(RuleKind::Extension { new: None })]
    );
    assert_eq!(fixed.skipped().len(), 4);
}

#[test]
fn 파일이_없으면_missing이고_아무_파일도_만들지_않는다() {
    let dir = TempDir::new().unwrap();
    let loaded = rename_presets::load(&dir.path().join("rename-presets.toml"));
    assert!(matches!(loaded, Loaded::Missing), "{loaded:?}");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn 통째로_깨진_파일은_옆에_남기고_broken이다() {
    let cases = [
        ("TOML 아님", "version = 1\n[[preset]\nname = "),
        ("버전 없음", "[[preset]]\nname = \"a\"\n"),
        ("모르는 버전", "version = 99\n"),
        ("버전이 수가 아님", "version = \"1\"\n"),
    ];
    for (case, text) in cases {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("rename-presets.toml");
        fs::write(&path, text).unwrap();
        let loaded = rename_presets::load(&path);
        let Loaded::Broken { error, kept } = loaded else {
            panic!("{case}: 깨진 파일로 보지 않았다: {loaded:?}");
        };
        match case {
            "버전 없음" => assert!(
                matches!(error, PresetsError::MissingVersion),
                "{case}: {error:?}"
            ),
            "모르는 버전" => assert!(
                matches!(&error, PresetsError::UnknownVersion(v) if v == "99"),
                "{case}: {error:?}"
            ),
            "버전이 수가 아님" => assert!(
                matches!(error, PresetsError::UnknownVersion(_)),
                "{case}: {error:?}"
            ),
            _ => assert!(
                matches!(error, PresetsError::Invalid(_)),
                "{case}: {error:?}"
            ),
        }
        let kept: PathBuf = kept.unwrap_or_else(|| panic!("{case}: 남긴 파일이 없다"));
        assert_eq!(kept.parent(), Some(dir.path()), "{case}: 옆에 남긴다");
        let name = kept.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("rename-presets.broken-") && name.ends_with(".toml"),
            "{case}: {name}"
        );
        assert_eq!(fs::read_to_string(&kept).unwrap(), text, "{case}");
        assert!(!path.exists(), "{case}: 다음 저장이 덮어쓰지 않게 옮긴다");
    }
}

#[test]
fn 저장은_옆_임시_파일을_거치고_폴더가_없으면_만든다() {
    let dir = TempDir::new().unwrap();
    let path = dir
        .path()
        .join("없는 폴더")
        .join("facet")
        .join("rename-presets.toml");
    let temp = rename_presets::temp_path(&path);
    assert_eq!(temp.parent(), path.parent(), "임시 파일은 옆에 둔다");
    assert_eq!(
        temp.file_name().unwrap().to_string_lossy(),
        "rename-presets.toml.tmp"
    );

    let mut before = Presets::new();
    before.save_preset("앞", replace("a")).unwrap();
    rename_presets::save(&path, &before).unwrap();
    assert_eq!(read(&path), before);
    assert!(!temp.exists(), "바꿔치기하면 임시 파일이 남지 않는다");

    // 앞선 저장이 끊겨 반쯤 쓴 임시 파일이 남아 있어도 원래 파일은 그대로이고 다음 저장은 된다.
    fs::write(&temp, "version = 1\n[[pre").unwrap();
    assert_eq!(read(&path), before);
    let mut after = before;
    after.save_preset("뒤", replace("b")).unwrap();
    rename_presets::save(&path, &after).unwrap();
    assert_eq!(read(&path), after);
    assert!(!temp.exists());
}

#[test]
fn 파일_자리는_appdata_아래_facet_폴더다() {
    let appdata = PathBuf::from("C:/Users/사용자/AppData/Roaming");
    let given = appdata.clone();
    let path = rename_presets::presets_path_with(move |name| {
        (name == "APPDATA").then(|| OsString::from(given.clone()))
    });
    assert_eq!(
        path,
        Some(appdata.join("facet").join("rename-presets.toml"))
    );
    assert_eq!(rename_presets::presets_path_with(|_| None), None);
    assert_eq!(
        rename_presets::presets_path_with(|_| Some(OsString::new())),
        None,
        "빈 APPDATA는 모름"
    );
}

#[test]
fn glossary에_규칙_묶음이_있다() {
    let glossary = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../GLOSSARY.md");
    let text = fs::read_to_string(glossary).unwrap();
    assert!(text.contains("규칙 묶음"));
}
