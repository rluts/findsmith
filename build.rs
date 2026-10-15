use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
};

use regex::Regex;
use serde_json::{Map, Value};

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

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let entries = bundle_catalogs(&catalogs, &out_dir)
        .iter()
        .map(|(code, path)| {
            format!(
                "({code:?}, include_str!({:?})),",
                path.display().to_string()
            )
        })
        .collect::<String>();
    let output = format!("pub const BUNDLED_CATALOGS: &[(&str, &str)] = &[{entries}];\n");
    fs::write(out_dir.join("i18n_catalogs.rs"), output).expect("write catalog manifest");
}

fn read_catalog(path: &Path) -> Map<String, Value> {
    let source = fs::read_to_string(path).expect("read translation catalog");
    serde_json::from_str::<Value>(&source)
        .unwrap_or_else(|error| panic!("invalid JSON in {}: {error}", path.display()))
        .as_object()
        .cloned()
        .unwrap_or_else(|| panic!("{} must contain a JSON object", path.display()))
}

/// English is authored here, so any defect fails the build. Translations
/// arrive from translatewiki.net; a defective message is dropped with a
/// warning so English covers it instead of the export breaking the build.
fn bundle_catalogs(catalogs: &[(String, PathBuf)], out_dir: &Path) -> Vec<(String, PathBuf)> {
    let placeholder = Regex::new(r"\$([1-9][0-9]*)").unwrap();
    let plural = Regex::new(r"(?i)\{\{PLURAL:\s*\$([1-9][0-9]*)\s*\|([^{}]*)\}\}").unwrap();
    let english_path = &catalogs.iter().find(|(code, _)| code == "en").unwrap().1;
    let english = read_catalog(english_path);
    let placeholders = |text: &str| -> BTreeSet<String> {
        placeholder
            .find_iter(text)
            .map(|found| found.as_str().to_owned())
            .collect()
    };
    let mut english_placeholders = BTreeMap::new();
    for (key, value) in &english {
        if key == "@metadata" {
            continue;
        }
        let text = value
            .as_str()
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| {
                panic!("{key} in {} must be non-empty text", english_path.display())
            });
        if let Some(problem) = plural_problem(&plural, text) {
            panic!("{key} in {}: {problem}", english_path.display());
        }
        english_placeholders.insert(key.as_str(), placeholders(text));
    }

    let bundle_dir = out_dir.join("i18n");
    fs::create_dir_all(&bundle_dir).expect("create bundled catalog directory");
    let mut bundled = Vec::new();
    for (code, path) in catalogs {
        if code == "en" {
            bundled.push((code.clone(), path.clone()));
            continue;
        }
        let mut accepted = Map::new();
        for (key, value) in read_catalog(path) {
            if key == "@metadata" {
                continue;
            }
            let problem = match (english_placeholders.get(key.as_str()), value.as_str()) {
                (None, _) => Some("unknown key".to_owned()),
                (_, None | Some("")) => Some("not non-empty text".to_owned()),
                (Some(expected), Some(text)) => plural_problem(&plural, text).or_else(|| {
                    let extra = placeholders(text)
                        .difference(expected)
                        .cloned()
                        .collect::<Vec<_>>();
                    (!extra.is_empty())
                        .then(|| format!("uses placeholders missing from English: {extra:?}"))
                }),
            };
            match problem {
                Some(problem) => println!(
                    "cargo:warning=skipping {key} in {}: {problem}",
                    path.display()
                ),
                None => {
                    accepted.insert(key, value);
                }
            }
        }
        let bundle_path = bundle_dir.join(format!("{code}.json"));
        fs::write(&bundle_path, Value::Object(accepted).to_string())
            .expect("write bundled catalog");
        bundled.push((code.clone(), bundle_path));
    }
    bundled
}

fn plural_problem(plural: &Regex, text: &str) -> Option<String> {
    let explicit = Regex::new(r"^[0-9]+=").unwrap();
    for captures in plural.captures_iter(text) {
        if captures[2].split('|').all(|form| explicit.is_match(form)) {
            return Some("PLURAL needs at least one form without a number".to_owned());
        }
    }
    plural
        .replace_all(text, "")
        .to_ascii_uppercase()
        .contains("{{PLURAL:")
        .then(|| "malformed or nested PLURAL".to_owned())
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
