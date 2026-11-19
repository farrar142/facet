// target: crates/facet-core/tests/spec_settings_file.rs
// AC: #458: 정의의 기본값이 범위 안·키 겹침 없음·처음 키 목록, 파일 없음이면 기본값·만들지 않음, 바꿔 쓰면 주석·순서·알 수 없는 키 보존, 기본값이면 키를 뺌, 처음 쓰면 폴더·파일 생김, 문법 오류, 값 오류, 받지 않는 값, 쓰기 실패(Windows), 설정 검색
//! 설정 모듈(`facet_core::settings`)의 단일 정의와 설정 파일 읽기·검증·형식 보존 쓰기를 임시 폴더의 실제
//! `settings.toml`로 확인한다. 설정 폴더는 인자로 넘겨 `%APPDATA%`를 건드리지 않는다. 읽기 전용 파일로 쓰기 실패를
//! 만드는 테스트만 Windows 전용이고 나머지는 모든 환경에서 돈다.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use facet_core::settings::{
    self, Category, FILE_NAME, Kind, SaveError, Saved, Settings, Value, ValueError,
};
use tempfile::TempDir;

fn write_file(folder: &Path, text: &str) {
    fs::write(folder.join(FILE_NAME), text).unwrap();
}

fn read_file(folder: &Path) -> String {
    fs::read_to_string(folder.join(FILE_NAME)).unwrap()
}

/// 값이 정의의 형·범위·고를 수 있는 값을 만족하는가.
fn fits(kind: Kind, value: Value) -> bool {
    match (kind, value) {
        (Kind::Bool, Value::Bool(_)) => true,
        (Kind::Integer { min, max }, Value::Integer(n)) => min <= n && n <= max,
        (Kind::Choice(choices), Value::Choice(c)) => choices.contains(&c),
        _ => false,
    }
}

#[test]
#[ignore = "#458"]
fn every_definition_default_fits_its_own_kind_and_range() {
    let definitions = settings::definitions();
    assert!(!definitions.is_empty());
    let mut keys = HashSet::new();
    for definition in definitions {
        assert!(
            fits(definition.kind, definition.default),
            "{}의 기본값 {:?}이 {:?}에 맞지 않는다",
            definition.key,
            definition.default,
            definition.kind
        );
        if let Kind::Integer { min, max } = definition.kind {
            assert!(min <= max, "{}", definition.key);
        }
        assert!(keys.insert(definition.key), "{}가 겹친다", definition.key);
        assert_eq!(
            settings::definition(definition.key),
            Some(definition),
            "{}",
            definition.key
        );
    }
    assert_eq!(settings::definition("no.such_key"), None);
}

#[test]
#[ignore = "#458"]
fn the_first_definitions_have_the_ticket_keys_categories_and_defaults() {
    let expected = [
        ("general.show_hidden", Category::General, Value::Bool(false)),
        ("view.theme", Category::View, Value::Choice("system")),
        ("view.font_size", Category::View, Value::Integer(9)),
        ("view.row_height", Category::View, Value::Integer(0)),
        (
            "shortcuts.preset",
            Category::Shortcuts,
            Value::Choice("explorer"),
        ),
        (
            "search.content_max_file_size_mb",
            Category::Search,
            Value::Integer(50),
        ),
        (
            "thumbnails.cache_limit_mb",
            Category::Thumbnails,
            Value::Integer(1024),
        ),
        (
            "advanced.shell_item_timeout_ms",
            Category::Advanced,
            Value::Integer(2000),
        ),
        (
            "advanced.shell_helper_idle_minutes",
            Category::Advanced,
            Value::Integer(5),
        ),
    ];
    for (key, category, default) in expected {
        let definition = settings::definition(key).unwrap_or_else(|| panic!("{key}가 없다"));
        assert_eq!(definition.category, category, "{key}");
        assert_eq!(definition.default, default, "{key}");
    }
    assert_eq!(
        settings::definition("view.theme").unwrap().kind,
        Kind::Choice(&["light", "dark", "system"])
    );
    assert_eq!(
        settings::definition("shortcuts.preset").unwrap().kind,
        Kind::Choice(&["explorer", "total-commander"])
    );
    assert_eq!(
        settings::definition("search.content_max_file_size_mb")
            .unwrap()
            .kind,
        Kind::Integer { min: 1, max: 2000 }
    );
}

#[test]
#[ignore = "#458"]
fn a_missing_file_gives_defaults_and_creates_nothing() {
    let dir = TempDir::new().unwrap();
    let settings = Settings::load(dir.path());
    for definition in settings::definitions() {
        assert_eq!(
            settings.get(definition.key),
            Some(definition.default),
            "{}",
            definition.key
        );
        assert!(!settings.is_modified(definition.key));
    }
    assert!(settings.syntax_error().is_none());
    assert!(settings.problems().is_empty());
    assert_eq!(settings.file_path(), Some(dir.path().join(FILE_NAME)));
    assert!(!dir.path().join(FILE_NAME).exists(), "파일을 만들었다");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);

    // 설정 폴더가 아예 없어도 만들지 않는다.
    let missing = dir.path().join("facet");
    let settings = Settings::load(&missing);
    assert_eq!(
        settings.get("general.show_hidden"),
        Some(Value::Bool(false))
    );
    assert!(!missing.exists(), "폴더를 만들었다");
}

#[test]
#[ignore = "#458"]
fn defaults_never_write_a_file() {
    let mut settings = Settings::defaults();
    assert_eq!(settings.file_path(), None);
    assert_eq!(settings.get("view.theme"), Some(Value::Choice("system")));
    settings
        .set("view.theme", Value::Choice("dark"))
        .expect("받아야 한다");
    assert_eq!(settings.get("view.theme"), Some(Value::Choice("dark")));
}

#[test]
#[ignore = "#458"]
fn values_in_the_file_are_applied_and_marked_modified() {
    let dir = TempDir::new().unwrap();
    write_file(
        dir.path(),
        "[general]\nshow_hidden = true\n\n[view]\ntheme = \"dark\"\nfont_size = 12\n",
    );
    let settings = Settings::load(dir.path());
    assert!(settings.syntax_error().is_none());
    assert!(settings.problems().is_empty(), "{:?}", settings.problems());
    assert_eq!(settings.get("general.show_hidden"), Some(Value::Bool(true)));
    assert_eq!(settings.get("view.theme"), Some(Value::Choice("dark")));
    assert_eq!(settings.get("view.font_size"), Some(Value::Integer(12)));
    assert!(settings.is_modified("general.show_hidden"));
    assert!(settings.is_modified("view.font_size"));
    assert!(!settings.is_modified("view.row_height"));
    assert_eq!(settings.get("no.such_key"), None);
}

const COMMENTED: &str = "\
# 내 facet 설정
[general]
# 숨김 항목
show_hidden = false # 끝 주석
future_option = \"keep\"

[thumbnails]
cache_limit_mb = 2048

# 새 버전의 표
[future_table]
x = 1
y = [\"가\", \"나\"]
";

#[test]
#[ignore = "#458"]
fn changing_one_value_keeps_comments_order_and_unknown_keys() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), COMMENTED);
    let mut settings = Settings::load(dir.path());
    assert_eq!(
        settings.get("thumbnails.cache_limit_mb"),
        Some(Value::Integer(2048))
    );
    assert_eq!(
        settings.set("general.show_hidden", Value::Bool(true)),
        Ok(Saved::Written)
    );

    let after = read_file(dir.path());
    let before_lines: Vec<&str> = COMMENTED.lines().collect();
    let after_lines: Vec<&str> = after.lines().collect();
    assert_eq!(after_lines.len(), before_lines.len(), "{after}");
    for (before, now) in before_lines.iter().zip(&after_lines) {
        if before.starts_with("show_hidden") {
            assert!(now.starts_with("show_hidden = true"), "{now}");
            assert!(now.contains("# 끝 주석"), "줄 끝 주석이 사라졌다: {now}");
        } else {
            assert_eq!(now, before, "바꾸지 않은 줄이 바뀌었다");
        }
    }

    let reloaded = Settings::load(dir.path());
    assert_eq!(reloaded.get("general.show_hidden"), Some(Value::Bool(true)));
    assert_eq!(
        reloaded.get("thumbnails.cache_limit_mb"),
        Some(Value::Integer(2048))
    );
}

#[test]
#[ignore = "#458"]
fn a_new_key_goes_into_its_table_without_touching_the_rest() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), COMMENTED);
    let mut settings = Settings::load(dir.path());
    assert_eq!(
        settings.set("view.font_size", Value::Integer(14)),
        Ok(Saved::Written)
    );
    let after = read_file(dir.path());
    // 원래 줄은 모두 같은 차례로 남는다.
    let mut rest = after.lines();
    for line in COMMENTED.lines() {
        assert!(
            rest.any(|l| l == line),
            "「{line}」이 사라지거나 차례가 바뀌었다:\n{after}"
        );
    }
    let parsed: toml::Table = after.parse().unwrap();
    assert_eq!(parsed["view"]["font_size"].as_integer(), Some(14));
    assert_eq!(
        Settings::load(dir.path()).get("view.font_size"),
        Some(Value::Integer(14))
    );
}

#[test]
#[ignore = "#458"]
fn resetting_or_setting_the_default_removes_the_key_from_the_file() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), COMMENTED);
    let mut settings = Settings::load(dir.path());

    assert_eq!(
        settings.reset("thumbnails.cache_limit_mb"),
        Ok(Saved::Written)
    );
    assert_eq!(
        settings.get("thumbnails.cache_limit_mb"),
        Some(Value::Integer(1024))
    );
    assert!(!settings.is_modified("thumbnails.cache_limit_mb"));

    settings
        .set("view.font_size", Value::Integer(15))
        .expect("받아야 한다");
    // 기본값과 같은 값으로 바꾸면 쓰지 않고 뺀다.
    assert_eq!(
        settings.set("view.font_size", Value::Integer(9)),
        Ok(Saved::Written)
    );
    // 파일에 기본값이 그대로 적혀 있던 키도 기본값으로 바꾸면 빠진다.
    assert_eq!(
        settings.set("general.show_hidden", Value::Bool(false)),
        Ok(Saved::Written)
    );

    let after = read_file(dir.path());
    let parsed: toml::Table = after.parse().unwrap();
    let thumbnails = parsed.get("thumbnails").and_then(|t| t.as_table());
    assert!(
        thumbnails.is_none_or(|t| !t.contains_key("cache_limit_mb")),
        "{after}"
    );
    let view = parsed.get("view").and_then(|t| t.as_table());
    assert!(view.is_none_or(|t| !t.contains_key("font_size")), "{after}");
    assert!(
        !parsed["general"]
            .as_table()
            .unwrap()
            .contains_key("show_hidden"),
        "{after}"
    );
    // 다른 내용은 그대로다.
    assert!(after.starts_with("# 내 facet 설정\n"), "{after}");
    assert_eq!(parsed["general"]["future_option"].as_str(), Some("keep"));
    assert_eq!(parsed["future_table"]["x"].as_integer(), Some(1));
    assert!(after.contains("# 새 버전의 표"), "{after}");
}

#[test]
#[ignore = "#458"]
fn the_first_change_creates_the_folder_and_file() {
    let dir = TempDir::new().unwrap();
    let folder = dir.path().join("facet");
    let mut settings = Settings::load(&folder);
    assert_eq!(
        settings.set("search.content_max_file_size_mb", Value::Integer(120)),
        Ok(Saved::Written)
    );
    assert!(folder.join(FILE_NAME).is_file());
    let parsed: toml::Table = read_file(&folder).parse().unwrap();
    assert_eq!(
        parsed["search"]["content_max_file_size_mb"].as_integer(),
        Some(120)
    );
    // 기본값과 다른 값만 쓴다.
    assert_eq!(parsed.len(), 1, "{parsed:?}");
    assert_eq!(parsed["search"].as_table().unwrap().len(), 1);
    // 임시 파일이 남지 않는다.
    let names: Vec<_> = fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from(FILE_NAME)]);

    let reloaded = Settings::load(&folder);
    assert_eq!(
        reloaded.get("search.content_max_file_size_mb"),
        Some(Value::Integer(120))
    );
    assert!(reloaded.is_modified("search.content_max_file_size_mb"));
}

const BROKEN: &str = "[general]\nshow_hidden = true\n[view\nfont_size = 12\n";

#[test]
#[ignore = "#458"]
fn a_syntax_error_gives_defaults_and_its_line_and_column() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), BROKEN);
    let settings = Settings::load(dir.path());
    let error = settings.syntax_error().expect("문법 오류여야 한다");
    assert_eq!(error.line, 3, "{error:?}");
    assert!(error.column >= 1, "{error:?}");
    assert!(!error.message.trim().is_empty());
    // 파일 전체를 무시한다: 앞 줄의 맞는 값도 쓰지 않는다.
    for definition in settings::definitions() {
        assert_eq!(
            settings.get(definition.key),
            Some(definition.default),
            "{}",
            definition.key
        );
    }
}

#[test]
#[ignore = "#458"]
fn with_a_syntax_error_changes_apply_for_this_run_but_the_file_is_not_written() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), BROKEN);
    let mut settings = Settings::load(dir.path());
    assert_eq!(
        settings.set("view.theme", Value::Choice("dark")),
        Ok(Saved::KeptInMemory(SaveError::FileBroken))
    );
    assert_eq!(settings.get("view.theme"), Some(Value::Choice("dark")));
    assert!(settings.is_modified("view.theme"));
    assert_eq!(
        settings.reset("view.theme"),
        Ok(Saved::KeptInMemory(SaveError::FileBroken))
    );
    assert_eq!(settings.get("view.theme"), Some(Value::Choice("system")));
    assert_eq!(read_file(dir.path()), BROKEN, "깨진 파일을 덮어썼다");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

const WRONG_VALUES: &str = "\
[general]
show_hidden = \"yes\"

[search]
content_max_file_size_mb = 999999

[view]
theme = \"pink\"
font_size = 12

[thumbnails]
cache_limit_mb = 2048
";

#[test]
#[ignore = "#458"]
fn a_wrong_value_falls_back_for_that_key_only() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), WRONG_VALUES);
    let settings = Settings::load(dir.path());
    assert!(settings.syntax_error().is_none());

    assert_eq!(
        settings.get("general.show_hidden"),
        Some(Value::Bool(false))
    );
    assert_eq!(
        settings.get("search.content_max_file_size_mb"),
        Some(Value::Integer(50))
    );
    assert_eq!(settings.get("view.theme"), Some(Value::Choice("system")));
    // 나머지는 적용된다.
    assert_eq!(settings.get("view.font_size"), Some(Value::Integer(12)));
    assert_eq!(
        settings.get("thumbnails.cache_limit_mb"),
        Some(Value::Integer(2048))
    );

    let problems = settings.problems();
    assert_eq!(problems.len(), 3, "{problems:?}");
    let find = |key: &str| {
        problems
            .iter()
            .find(|p| p.key == key)
            .unwrap_or_else(|| panic!("{key}의 문제가 없다: {problems:?}"))
    };
    let hidden = find("general.show_hidden");
    assert_eq!(hidden.error, ValueError::WrongType);
    assert_eq!(hidden.line, 2);
    assert!(hidden.column >= 1);
    let size = find("search.content_max_file_size_mb");
    assert_eq!(size.error, ValueError::OutOfRange { min: 1, max: 2000 });
    assert_eq!(size.line, 5);
    let theme = find("view.theme");
    assert_eq!(theme.error, ValueError::NotAChoice);
    assert_eq!(theme.line, 8);
}

#[test]
#[ignore = "#458"]
fn writing_another_key_leaves_the_wrong_values_in_the_file() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), WRONG_VALUES);
    let mut settings = Settings::load(dir.path());
    assert_eq!(
        settings.set("view.font_size", Value::Integer(13)),
        Ok(Saved::Written)
    );
    let after = read_file(dir.path());
    assert!(after.contains("show_hidden = \"yes\""), "{after}");
    assert!(
        after.contains("content_max_file_size_mb = 999999"),
        "{after}"
    );
    assert!(after.contains("theme = \"pink\""), "{after}");
    assert!(after.contains("font_size = 13"), "{after}");
    // 다시 읽어도 같은 문제를 알린다.
    assert_eq!(Settings::load(dir.path()).problems().len(), 3);
}

#[test]
#[ignore = "#458"]
fn values_outside_the_definition_are_refused_with_the_range() {
    let dir = TempDir::new().unwrap();
    let mut settings = Settings::load(dir.path());
    assert_eq!(
        settings.set("search.content_max_file_size_mb", Value::Integer(0)),
        Err(ValueError::OutOfRange { min: 1, max: 2000 })
    );
    assert_eq!(
        settings.set("search.content_max_file_size_mb", Value::Integer(2001)),
        Err(ValueError::OutOfRange { min: 1, max: 2000 })
    );
    assert_eq!(
        settings.set("view.font_size", Value::Bool(true)),
        Err(ValueError::WrongType)
    );
    assert_eq!(
        settings.set("view.theme", Value::Choice("pink")),
        Err(ValueError::NotAChoice)
    );
    assert_eq!(
        settings.set("no.such_key", Value::Bool(true)),
        Err(ValueError::UnknownKey)
    );
    assert_eq!(settings.reset("no.such_key"), Err(ValueError::UnknownKey));
    // 받지 않은 값은 아무것도 바꾸지 않는다.
    assert_eq!(
        settings.get("search.content_max_file_size_mb"),
        Some(Value::Integer(50))
    );
    assert!(!dir.path().join(FILE_NAME).exists());
    // 범위 끝은 받는다.
    assert_eq!(
        settings.set("search.content_max_file_size_mb", Value::Integer(2000)),
        Ok(Saved::Written)
    );
}

#[cfg(windows)]
#[test]
#[ignore = "#458"]
#[allow(clippy::permissions_set_readonly_false)]
fn a_failed_write_keeps_the_value_in_memory_and_says_why() {
    let dir = TempDir::new().unwrap();
    let original = "[view]\nfont_size = 11\n";
    write_file(dir.path(), original);
    let path = dir.path().join(FILE_NAME);
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions.clone()).unwrap();

    let mut settings = Settings::load(dir.path());
    let saved = settings.set("view.font_size", Value::Integer(16));

    permissions.set_readonly(false);
    fs::set_permissions(&path, permissions).unwrap();

    match saved {
        Ok(Saved::KeptInMemory(SaveError::Write(_))) => {}
        other => panic!("쓰기 실패여야 한다: {other:?}"),
    }
    assert_eq!(settings.get("view.font_size"), Some(Value::Integer(16)));
    assert_eq!(read_file(dir.path()), original);
}

/// 시험용 표시 이름·설명: 실제 화면 글자는 앱이 준다.
fn text(definition: &settings::Definition) -> (String, String) {
    match definition.key {
        "general.show_hidden" => ("숨김 항목 보이기".to_owned(), "Hidden files".to_owned()),
        "thumbnails.cache_limit_mb" => (
            "썸네일 캐시 상한".to_owned(),
            "디스크에 남길 THUMBNAIL 크기".to_owned(),
        ),
        _ => (String::new(), String::new()),
    }
}

fn found(query: &str) -> Vec<&'static str> {
    settings::search(query, text)
        .into_iter()
        .map(|d| d.key)
        .collect()
}

#[test]
#[ignore = "#458"]
fn search_matches_key_name_and_description_ignoring_case() {
    // 빈 검색어(공백뿐 포함)는 모두를 정의 순서로.
    let all: Vec<&str> = settings::definitions().iter().map(|d| d.key).collect();
    assert_eq!(found(""), all);
    assert_eq!(found("  "), all);
    // 표시 이름.
    assert_eq!(found("숨김"), vec!["general.show_hidden"]);
    // 설명, 대소문자 무시.
    assert_eq!(found("hidden FILES"), vec!["general.show_hidden"]);
    assert_eq!(found("thumbnail 크기"), vec!["thumbnails.cache_limit_mb"]);
    // 키.
    assert_eq!(found("CACHE_LIMIT"), vec!["thumbnails.cache_limit_mb"]);
    let shell = found("shell_");
    assert!(
        shell.contains(&"advanced.shell_item_timeout_ms"),
        "{shell:?}"
    );
    assert!(
        shell.contains(&"advanced.shell_helper_idle_minutes"),
        "{shell:?}"
    );
    assert!(found("없는 설정 글자").is_empty());
}
