use std::{env, fs, path::PathBuf};

use regex::Regex;
use serde_json::Value;

fn main() {
    let catalog_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("i18n");
    println!("cargo:rerun-if-changed={}", catalog_dir.display());

    let mut catalogs = Vec::new();
    for entry in fs::read_dir(&catalog_dir).expect("read i18n catalogs") {
        let path = entry.expect("read catalog entry").path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let code = path.file_stem().and_then(|value| value.to_str()).unwrap();
        if code == "qqq" {
            continue;
        }
        assert!(
            valid_catalog_code(code),
            "invalid language code in {}",
            path.display()
        );
        catalogs.push((code.to_owned(), path));
    }
    catalogs.sort_by(|left, right| left.0.cmp(&right.0));
    assert!(
        catalogs.iter().any(|(code, _)| code == "en"),
        "missing i18n/en.json"
    );

    validate_catalogs(&catalogs);

    let entries = catalogs
        .iter()
        .map(|(code, path)| {
            format!(
                "({code:?}, include_str!({:?})),",
                path.display().to_string()
            )
        })
        .collect::<String>();
    let output = format!("pub const BUNDLED_CATALOGS: &[(&str, &str)] = &[{entries}];\n");
    let output_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("i18n_catalogs.rs");
    fs::write(output_path, output).expect("write catalog manifest");
}

fn validate_catalogs(catalogs: &[(String, PathBuf)]) {
    let read = |path: &PathBuf| -> serde_json::Map<String, Value> {
        let source = fs::read_to_string(path).expect("read translation catalog");
        serde_json::from_str::<Value>(&source)
            .expect("valid translation JSON")
            .as_object()
            .cloned()
            .expect("translation catalog must be an object")
    };
    let english_path = &catalogs.iter().find(|(code, _)| code == "en").unwrap().1;
    let english = read(english_path);
    let placeholder = Regex::new(r"\$([1-9][0-9]*)").unwrap();
    let plural = Regex::new(r"\{\{PLURAL:\$([1-9][0-9]*)\|([^{}]*)\}\}").unwrap();
    for (code, path) in catalogs {
        let messages = read(path);
        for (key, value) in &messages {
            if key == "@metadata" {
                continue;
            }
            let source = english
                .get(key)
                .unwrap_or_else(|| panic!("unknown translation key {key} in {}", path.display()));
            let text = value
                .as_str()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| panic!("{key} in {} must be non-empty text", path.display()));
            if code != "en" {
                let expected: std::collections::BTreeSet<_> = placeholder
                    .find_iter(source.as_str().unwrap())
                    .map(|m| m.as_str())
                    .collect();
                let actual: std::collections::BTreeSet<_> =
                    placeholder.find_iter(text).map(|m| m.as_str()).collect();
                assert_eq!(
                    actual,
                    expected,
                    "{key} in {} changes placeholders",
                    path.display()
                );
            }
            for captures in plural.captures_iter(text) {
                let mut categories = std::collections::HashSet::new();
                for arm in captures[2].split('|') {
                    let (category, word) = arm.split_once('=').unwrap_or(("", ""));
                    assert!(
                        matches!(category, "zero" | "one" | "two" | "few" | "many" | "other")
                            && !word.is_empty()
                            && categories.insert(category),
                        "invalid plural form in {key} in {}",
                        path.display()
                    );
                }
                assert!(
                    categories.contains("other"),
                    "missing other plural form in {key}"
                );
            }
            assert!(
                !plural.replace_all(text, "").contains("{{PLURAL:"),
                "malformed plural form in {key} in {}",
                path.display()
            );
        }
    }
}

fn valid_catalog_code(code: &str) -> bool {
    let mut parts = code.split('-');
    let Some(primary) = parts.next() else {
        return false;
    };
    (2..=8).contains(&primary.len())
        && primary.bytes().all(|byte| byte.is_ascii_lowercase())
        && parts.all(|part| {
            !part.is_empty()
                && part.len() <= 16
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}
