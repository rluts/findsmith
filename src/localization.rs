use std::{collections::HashMap, future::Future, sync::LazyLock};

use axum_extra::extract::cookie::{Cookie, PrivateCookieJar, SameSite};
use icu_locale::Locale;
use icu_plurals::PluralRules;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait};
use serde::Serialize;

use crate::entities::user_preference;

pub const DEFAULT_LANGUAGE: &str = "en";
const COOKIE_NAME: &str = "findsmith_language";
const ENGLISH: &str = include_str!("../i18n/en.json");
include!(concat!(env!("OUT_DIR"), "/i18n_catalogs.rs"));

tokio::task_local! { static REQUEST_LANGUAGE: String; }

fn parse_messages(source: &str) -> HashMap<String, String> {
    let value: serde_json::Value = serde_json::from_str(source).expect("valid message catalog");
    value
        .as_object()
        .expect("message map")
        .iter()
        .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.to_owned())))
        .collect()
}

static ENGLISH_MESSAGES: LazyLock<HashMap<String, String>> =
    LazyLock::new(|| parse_messages(ENGLISH));
static TRANSLATED_MESSAGES: LazyLock<HashMap<&'static str, HashMap<String, String>>> =
    LazyLock::new(|| {
        BUNDLED_CATALOGS
            .iter()
            .filter(|(language, _)| *language != DEFAULT_LANGUAGE)
            .map(|(language, source)| (*language, parse_messages(source)))
            .collect()
    });

fn merged_messages(translations: Option<&HashMap<String, String>>) -> HashMap<String, String> {
    let mut messages = ENGLISH_MESSAGES.clone();
    if let Some(translations) = translations {
        messages.extend(translations.clone());
    }
    messages
}

static SOURCE_KEYS: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    ENGLISH_MESSAGES
        .iter()
        .filter(|(key, _)| key.starts_with("findsmith-api-"))
        .map(|(key, value)| (value.clone(), key.clone()))
        .collect()
});
static PLACEHOLDER: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\$([1-9][0-9]*)").expect("valid placeholder pattern"));
static PLURAL: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\{\{PLURAL:\$([1-9][0-9]*)\|([^{}]*)\}\}").expect("valid plural pattern")
});

fn plural_category(language: &str, count: &str) -> String {
    let locale = language
        .parse::<Locale>()
        .or_else(|_| DEFAULT_LANGUAGE.parse::<Locale>());
    let Ok(locale) = locale else {
        return "other".to_owned();
    };
    let Ok(rules) = PluralRules::try_new_cardinal(locale.into()) else {
        return "other".to_owned();
    };
    count
        .parse::<usize>()
        .map(|count| format!("{:?}", rules.category_for(count)).to_ascii_lowercase())
        .unwrap_or_else(|_| "other".to_owned())
}

fn render_template(template: &str, language: &str, args: &[&str]) -> String {
    let with_plurals = PLURAL.replace_all(template, |captures: &regex::Captures<'_>| {
        let index = captures[1].parse::<usize>().unwrap_or(0);
        let count = index
            .checked_sub(1)
            .and_then(|index| args.get(index))
            .copied();
        let Some(count) = count else {
            return captures[0].to_owned();
        };
        let category = plural_category(language, count);
        let forms: HashMap<_, _> = captures[2]
            .split('|')
            .filter_map(|form| form.split_once('='))
            .collect();
        forms
            .get(category.as_str())
            .or_else(|| forms.get("other"))
            .copied()
            .unwrap_or(&captures[0])
            .to_owned()
    });
    PLACEHOLDER
        .replace_all(&with_plurals, |captures: &regex::Captures<'_>| {
            captures[1]
                .parse::<usize>()
                .ok()
                .and_then(|position| args.get(position.saturating_sub(1)))
                .copied()
                .unwrap_or(&captures[0])
                .to_owned()
        })
        .into_owned()
}

pub async fn in_language<F: Future>(language: String, future: F) -> F::Output {
    REQUEST_LANGUAGE.scope(language, future).await
}

pub fn current_language() -> String {
    REQUEST_LANGUAGE
        .try_with(Clone::clone)
        .unwrap_or_else(|_| DEFAULT_LANGUAGE.to_owned())
}

pub fn message(key: &str) -> Option<String> {
    let language = current_language();
    let localized = TRANSLATED_MESSAGES
        .get(language.as_str())
        .and_then(|messages| messages.get(key));
    localized.or_else(|| ENGLISH_MESSAGES.get(key)).cloned()
}

pub fn message_with_args(key: &str, args: &[&str]) -> Option<String> {
    message(key).map(|template| {
        let language = current_language();
        render_template(&template, &language, args)
    })
}

pub fn required(key: &str) -> String {
    message(key).unwrap_or_else(|| panic!("missing translation key: {key}"))
}

pub fn required_with_args(key: &str, args: &[&str]) -> String {
    message_with_args(key, args).unwrap_or_else(|| panic!("missing translation key: {key}"))
}

pub fn from_source(source: &str) -> String {
    SOURCE_KEYS
        .get(source)
        .and_then(|key| message(key))
        .unwrap_or_else(|| source.to_owned())
}

#[derive(Serialize)]
pub struct TranslationResponse {
    pub language: &'static str,
    pub messages: serde_json::Value,
}

pub fn catalog(language: &str) -> Option<TranslationResponse> {
    if !valid_language(language) {
        return None;
    }
    let selected = if available_language(language) {
        language
    } else {
        DEFAULT_LANGUAGE
    };
    let messages = merged_messages(TRANSLATED_MESSAGES.get(selected));
    Some(TranslationResponse {
        language: BUNDLED_CATALOGS
            .iter()
            .find(|(code, _)| *code == selected)
            .map(|(code, _)| *code)
            .unwrap_or(DEFAULT_LANGUAGE),
        messages: serde_json::to_value(messages).expect("message map"),
    })
}

pub fn available_language(language: &str) -> bool {
    BUNDLED_CATALOGS.iter().any(|(code, _)| *code == language)
}

pub fn available_languages() -> Vec<&'static str> {
    BUNDLED_CATALOGS.iter().map(|(code, _)| *code).collect()
}

pub fn valid_language(language: &str) -> bool {
    if language.len() > 64 {
        return false;
    }
    let mut parts = language.split('-');
    let Some(primary) = parts.next() else {
        return false;
    };
    (2..=8).contains(&primary.len())
        && primary.bytes().all(|byte| byte.is_ascii_alphabetic())
        && parts.all(|part| {
            !part.is_empty()
                && part.len() <= 16
                && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
}

pub fn cookie_language(jar: &PrivateCookieJar) -> Option<String> {
    jar.get(COOKIE_NAME)
        .map(|cookie| cookie.value().to_owned())
        .filter(|value| valid_language(value))
}

pub fn set_cookie(jar: PrivateCookieJar, language: &str, secure: bool) -> PrivateCookieJar {
    let cookie = Cookie::build((COOKIE_NAME, language.to_owned()))
        .path("/")
        .secure(secure)
        .http_only(true)
        .same_site(SameSite::Lax)
        .permanent()
        .build();
    jar.add(cookie)
}

pub async fn preferred_language(
    database: &DatabaseConnection,
    owner: Option<&str>,
    jar: &PrivateCookieJar,
) -> Result<String, sea_orm::DbErr> {
    let stored = if let Some(owner) = owner {
        user_preference::Entity::find_by_id(owner)
            .one(database)
            .await?
            .map(|preference| preference.language)
    } else {
        None
    };
    Ok(resolve_language(
        stored.as_deref(),
        cookie_language(jar).as_deref(),
    ))
}

fn resolve_language(stored: Option<&str>, cookie: Option<&str>) -> String {
    stored
        .filter(|value| available_language(value))
        .or_else(|| cookie.filter(|value| available_language(value)))
        .unwrap_or(DEFAULT_LANGUAGE)
        .to_owned()
}

pub async fn save_language(
    database: &DatabaseConnection,
    owner: Option<&str>,
    language: &str,
) -> Result<(), sea_orm::DbErr> {
    if let Some(owner) = owner {
        if let Some(existing) = user_preference::Entity::find_by_id(owner)
            .one(database)
            .await?
        {
            let mut active: user_preference::ActiveModel = existing.into();
            active.language = Set(language.to_owned());
            active.update(database).await?;
        } else {
            user_preference::ActiveModel {
                owner: Set(owner.to_owned()),
                language: Set(language.to_owned()),
            }
            .insert(database)
            .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_contains_english_messages() {
        let english = catalog("en").unwrap();
        assert_eq!(english.messages["findsmith-home-title"], "FindSmith");
        assert!(english.messages.get("@metadata").is_none());
        assert_eq!(catalog("zz-unknown").unwrap().language, "en");
        assert!(catalog("../en").is_none());
        assert!(available_languages().contains(&"en"));
    }

    #[test]
    fn language_codes_are_constrained() {
        assert!(valid_language("en"));
        assert!(valid_language("zh-Hant"));
        assert!(valid_language("zh-classical"));
        assert!(!valid_language(""));
        assert!(!valid_language("a"));
        assert!(!valid_language("../en"));
        assert!(!valid_language("en--US"));
    }

    #[test]
    fn database_language_takes_priority_over_cookie() {
        assert_eq!(resolve_language(Some("en"), Some("zz-unknown")), "en");
        assert_eq!(resolve_language(None, Some("zz-unknown")), "en");
        assert_eq!(resolve_language(None, None), "en");
    }

    #[test]
    fn translated_messages_overlay_english() {
        let translated = HashMap::from([("findsmith-nav-home".to_owned(), "Головна".to_owned())]);
        let messages = merged_messages(Some(&translated));
        assert_eq!(messages["findsmith-nav-home"], "Головна");
        assert_eq!(messages["findsmith-home-title"], "FindSmith");
    }

    #[test]
    fn every_bundled_catalog_is_selectable() {
        for (language, _) in BUNDLED_CATALOGS {
            assert!(available_language(language));
            let response = catalog(language).unwrap();
            assert_eq!(response.language, *language);
            assert_eq!(response.messages["findsmith-home-title"], "FindSmith");
        }
    }

    #[test]
    fn message_documentation_covers_catalog() {
        let english: serde_json::Value = serde_json::from_str(ENGLISH).unwrap();
        let docs: serde_json::Value =
            serde_json::from_str(include_str!("../i18n/qqq.json")).unwrap();
        for key in english
            .as_object()
            .unwrap()
            .keys()
            .filter(|key| *key != "@metadata")
        {
            assert!(
                docs.get(key).and_then(serde_json::Value::as_str).is_some(),
                "missing documentation for {key}"
            );
        }
    }

    #[test]
    fn backend_messages_resolve_from_catalog() {
        assert!(SOURCE_KEYS.contains_key("Project not found"));
        assert_eq!(from_source("Project not found"), "Project not found");
        assert_eq!(
            SOURCE_KEYS.get("Edit outcome unknown").map(String::as_str),
            Some("findsmith-api-edit-outcome-unknown")
        );
        assert_eq!(
            SOURCE_KEYS
                .get("Edit outcome unknown until Wikimedia responds; do not retry this job if the request is interrupted.")
                .map(String::as_str),
            Some("findsmith-api-edit-outcome-unknown-reason")
        );
        assert_eq!(
            message_with_args("findsmith-api-source-more", &["Alpha", "2"]).as_deref(),
            Some("Alpha and 2 more")
        );
        assert_eq!(
            message_with_args("findsmith-api-source-more", &["$2", "3"]).as_deref(),
            Some("$2 and 3 more")
        );
    }

    #[test]
    fn plural_forms_follow_selected_language() {
        let template = "$1 {{PLURAL:$1|one=page|other=pages}}";
        assert_eq!(render_template(template, "en", &["1"]), "1 page");
        assert_eq!(render_template(template, "en", &["2"]), "2 pages");
        let template = "$1 {{PLURAL:$1|one=one|few=few|many=many|other=other}}";
        assert_eq!(render_template(template, "uk", &["1"]), "1 one");
        assert_eq!(render_template(template, "uk", &["2"]), "2 few");
        assert_eq!(render_template(template, "uk", &["5"]), "5 many");
    }
}
