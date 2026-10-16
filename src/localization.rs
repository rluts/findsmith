use std::{collections::HashMap, future::Future, sync::LazyLock};

use axum_extra::extract::cookie::{Cookie, PrivateCookieJar, SameSite};
use fixed_decimal::Decimal;
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

/// Layers are ordered from most to least preferred; English fills any gaps.
fn merged_messages(layers: &[&HashMap<String, String>]) -> HashMap<String, String> {
    let mut messages = ENGLISH_MESSAGES.clone();
    for layer in layers.iter().rev() {
        messages.extend((*layer).clone());
    }
    messages
}

/// MediaWiki fallbacks that the primary-subtag rule cannot derive.
const EXPLICIT_FALLBACKS: &[(&str, &[&str])] = &[
    ("zh", &["zh-hans"]),
    ("zh-hant", &["zh-hans"]),
    ("zh-hk", &["zh-hant", "zh-hans"]),
    ("zh-tw", &["zh-hant", "zh-hans"]),
];

/// Languages to consult for `language`, most preferred first, excluding
/// English, which always backs every chain.
fn fallback_candidates(language: &str) -> Vec<&str> {
    let explicit = EXPLICIT_FALLBACKS
        .iter()
        .find(|(code, _)| *code == language)
        .map(|(_, fallbacks)| *fallbacks)
        .unwrap_or_default();
    let primary = language.split('-').next().unwrap_or(language);
    let mut chain = Vec::new();
    for candidate in [language]
        .into_iter()
        .chain(explicit.iter().copied())
        .chain([primary])
    {
        if candidate != DEFAULT_LANGUAGE && !chain.contains(&candidate) {
            chain.push(candidate);
        }
    }
    chain
}

fn fallback_chain(language: &str) -> Vec<&'static str> {
    fallback_candidates(language)
        .into_iter()
        .filter_map(|candidate| {
            BUNDLED_CATALOGS
                .iter()
                .find(|(code, _)| *code == candidate)
                .map(|(code, _)| *code)
        })
        .collect()
}

/// MediaWiki codes whose BCP 47 form is not just a change of letter case.
const NON_STANDARD_CODES: &[(&str, &str)] = &[
    ("als", "gsw"),
    ("bat-smg", "sgs"),
    ("be-x-old", "be-tarask"),
    ("cbk-zam", "cbk"),
    ("de-formal", "de-x-formal"),
    ("eml", "egl"),
    ("es-formal", "es-x-formal"),
    ("fiu-vro", "vro"),
    ("hu-formal", "hu-x-formal"),
    ("map-bms", "jv-x-bms"),
    ("mo", "ro-Cyrl-MD"),
    ("nl-informal", "nl-x-informal"),
    ("nrm", "nrf"),
    ("roa-rup", "rup"),
    ("roa-tara", "nap-x-tara"),
    ("simple", "en-simple"),
    ("sr-ec", "sr-Cyrl"),
    ("sr-el", "sr-Latn"),
    ("zh-classical", "lzh"),
    ("zh-min-nan", "nan"),
    ("zh-yue", "yue"),
];

/// Converts a MediaWiki language code to a BCP 47 tag usable by ICU, `Intl`
/// and the HTML `lang` attribute.
pub fn locale_tag(language: &str) -> String {
    let language = language.to_ascii_lowercase();
    if let Some((_, tag)) = NON_STANDARD_CODES
        .iter()
        .find(|(code, _)| *code == language)
    {
        return (*tag).to_owned();
    }
    let mut private = false;
    language
        .split('-')
        .enumerate()
        .map(|(index, part)| {
            let tag = if index == 0 || private {
                part.to_owned()
            } else if part.len() == 2 {
                part.to_ascii_uppercase()
            } else if part.len() == 4 {
                part[..1].to_ascii_uppercase() + &part[1..]
            } else {
                part.to_owned()
            };
            private |= part.len() == 1;
            tag
        })
        .collect::<Vec<_>>()
        .join("-")
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
    regex::Regex::new(r"(?i)\{\{PLURAL:\s*\$([1-9][0-9]*)\s*\|([^{}]*)\}\}")
        .expect("valid plural pattern")
});

fn plural_rules(language: &str) -> Option<PluralRules> {
    let tag = locale_tag(language);
    let primary = tag.split('-').next().unwrap_or(DEFAULT_LANGUAGE);
    [tag.as_str(), primary, DEFAULT_LANGUAGE]
        .into_iter()
        .filter_map(|candidate| candidate.parse::<Locale>().ok())
        .find_map(|locale| PluralRules::try_new_cardinal(locale.into()).ok())
}

/// Picks a form the way MediaWiki does: an explicit `N=` form matching the
/// count wins, otherwise forms follow the language's CLDR category order and
/// the last form covers any categories left out.
fn plural_form<'a>(language: &str, count: &str, forms: &'a str) -> &'a str {
    let count_number = count.parse::<u64>().ok();
    let mut positional = Vec::new();
    for form in forms.split('|') {
        match form.split_once('=') {
            Some((number, text))
                if !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()) =>
            {
                if count_number.is_some() && number.parse::<u64>().ok() == count_number {
                    return text;
                }
            }
            _ => positional.push(form),
        }
    }
    let Some(last) = positional.len().checked_sub(1) else {
        return "";
    };
    let index = plural_rules(language)
        .zip(count.parse::<Decimal>().ok())
        .and_then(|(rules, count)| {
            let category = rules.category_for(&count);
            rules
                .categories()
                .position(|candidate| candidate == category)
        })
        .unwrap_or(last);
    positional[index.min(last)]
}

fn render_template(template: &str, language: &str, args: &[&str]) -> String {
    let with_plurals = PLURAL.replace_all(template, |captures: &regex::Captures<'_>| {
        let index = captures[1].parse::<usize>().unwrap_or(0);
        let count = index
            .checked_sub(1)
            .and_then(|index| args.get(index))
            .copied();
        match count {
            Some(count) => plural_form(language, count.trim(), &captures[2]).to_owned(),
            None => captures[0].to_owned(),
        }
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
    fallback_chain(&current_language())
        .into_iter()
        .filter_map(|language| TRANSLATED_MESSAGES.get(language))
        .find_map(|messages| messages.get(key))
        .or_else(|| ENGLISH_MESSAGES.get(key))
        .cloned()
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
    message(source)
        .or_else(|| SOURCE_KEYS.get(source).and_then(|key| message(key)))
        .unwrap_or_else(|| source.to_owned())
}

#[derive(Serialize)]
pub struct TranslationResponse {
    pub language: &'static str,
    pub locale: String,
    pub messages: serde_json::Value,
}

#[derive(Serialize)]
pub struct LanguageOption {
    pub code: &'static str,
    pub locale: String,
}

pub fn catalog(language: &str) -> Option<TranslationResponse> {
    if !valid_language(language) {
        return None;
    }
    let selected = BUNDLED_CATALOGS
        .iter()
        .find(|(code, _)| *code == language)
        .map(|(code, _)| *code)
        .unwrap_or(DEFAULT_LANGUAGE);
    let layers = fallback_chain(selected)
        .into_iter()
        .filter_map(|language| TRANSLATED_MESSAGES.get(language))
        .collect::<Vec<_>>();
    Some(TranslationResponse {
        language: selected,
        locale: locale_tag(selected),
        messages: serde_json::to_value(merged_messages(&layers)).expect("message map"),
    })
}

pub fn available_language(language: &str) -> bool {
    BUNDLED_CATALOGS.iter().any(|(code, _)| *code == language)
}

pub fn available_languages() -> Vec<LanguageOption> {
    BUNDLED_CATALOGS
        .iter()
        .map(|(code, _)| LanguageOption {
            code,
            locale: locale_tag(code),
        })
        .collect()
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
        assert!(
            available_languages()
                .iter()
                .any(|option| option.code == "en" && option.locale == "en")
        );
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
        let messages = merged_messages(&[&translated]);
        assert_eq!(messages["findsmith-nav-home"], "Головна");
        assert_eq!(messages["findsmith-home-title"], "FindSmith");
    }

    #[test]
    fn preferred_layers_win_over_fallbacks() {
        let variant = HashMap::from([("findsmith-nav-home".to_owned(), "Start".to_owned())]);
        let base = HashMap::from([
            ("findsmith-nav-home".to_owned(), "Startseite".to_owned()),
            ("findsmith-nav-help".to_owned(), "Hilfe".to_owned()),
        ]);
        let messages = merged_messages(&[&variant, &base]);
        assert_eq!(messages["findsmith-nav-home"], "Start");
        assert_eq!(messages["findsmith-nav-help"], "Hilfe");
        assert_eq!(messages["findsmith-home-title"], "FindSmith");
    }

    #[test]
    fn fallback_candidates_follow_mediawiki() {
        assert_eq!(fallback_candidates("de-formal"), ["de-formal", "de"]);
        assert_eq!(fallback_candidates("pt-br"), ["pt-br", "pt"]);
        assert_eq!(
            fallback_candidates("zh-hk"),
            ["zh-hk", "zh-hant", "zh-hans", "zh"]
        );
        assert_eq!(fallback_candidates("uk"), ["uk"]);
        assert_eq!(fallback_candidates("en-gb"), ["en-gb"]);
        assert!(fallback_candidates("en").is_empty());
    }

    #[test]
    fn mediawiki_codes_map_to_bcp47() {
        assert_eq!(locale_tag("uk"), "uk");
        assert_eq!(locale_tag("pt-br"), "pt-BR");
        assert_eq!(locale_tag("zh-hans"), "zh-Hans");
        assert_eq!(locale_tag("sr-ec"), "sr-Cyrl");
        assert_eq!(locale_tag("zh-classical"), "lzh");
        assert_eq!(locale_tag("de-formal"), "de-x-formal");
        assert_eq!(locale_tag("be-tarask"), "be-tarask");
        assert_eq!(locale_tag("en-x-piglatin"), "en-x-piglatin");
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
            from_source("findsmith-api-new-messages"),
            "Read your new messages on this wiki before editing."
        );
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
        let template = "$1 {{PLURAL:$1|page|pages}}";
        assert_eq!(render_template(template, "en", &["1"]), "1 page");
        assert_eq!(render_template(template, "en", &["2"]), "2 pages");
        let template = "$1 {{PLURAL:$1|сторінка|сторінки|сторінок}}";
        assert_eq!(render_template(template, "uk", &["1"]), "1 сторінка");
        assert_eq!(render_template(template, "uk", &["2"]), "2 сторінки");
        assert_eq!(render_template(template, "uk", &["5"]), "5 сторінок");
        assert_eq!(render_template(template, "uk", &["21"]), "21 сторінка");
        assert_eq!(render_template(template, "uk", &["1.5"]), "1.5 сторінок");
    }

    #[test]
    fn plural_forms_follow_mediawiki_conventions() {
        let template = "{{PLURAL:$1|0=no pages|one page|$1 pages}}";
        assert_eq!(render_template(template, "en", &["0"]), "no pages");
        assert_eq!(render_template(template, "en", &["1"]), "one page");
        assert_eq!(render_template(template, "en", &["3"]), "3 pages");
        assert_eq!(
            render_template("{{plural:$1|page|pages}}", "en", &["2"]),
            "pages"
        );
        assert_eq!(
            render_template("page{{PLURAL:$1||s}}", "en", &["2"]),
            "pages"
        );
        assert_eq!(
            render_template("{{PLURAL:$1|pages}}", "en", &["1"]),
            "pages"
        );
        assert_eq!(render_template("{{PLURAL:$1|a=b|c}}", "en", &["1"]), "a=b");
        assert_eq!(
            render_template("{{PLURAL:$2|page|pages}}", "en", &["1"]),
            "{{PLURAL:$2|page|pages}}"
        );
        assert_eq!(
            render_template("$1 {{PLURAL:$1|页}}", "zh-classical", &["3"]),
            "3 页"
        );
        assert_eq!(
            render_template("$1 {{PLURAL:$1|a|b|c}}", "sr-ec", &["3"]),
            "3 b"
        );
    }
}
