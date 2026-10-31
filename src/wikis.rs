use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set, SqlErr,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use crate::{
    entities::{user_preference, wiki},
    localization,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Namespace {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Deserialize)]
struct SiteInfoResponse {
    query: SiteInfoQuery,
}
#[derive(Debug, Deserialize)]
struct SiteInfoQuery {
    general: General,
    namespaces: HashMap<String, ApiNamespace>,
}
#[derive(Debug, Deserialize)]
struct General {
    wikiid: String,
    sitename: String,
}
#[derive(Debug, Deserialize)]
struct ApiNamespace {
    id: i64,
    name: String,
}

#[derive(Debug, Deserialize)]
struct CheckResponse {
    query: CheckQuery,
}
#[derive(Debug, Deserialize)]
struct CheckQuery {
    pages: Vec<CheckPage>,
}
#[derive(Debug, Deserialize)]
struct CheckPage {
    #[serde(default)]
    missing: bool,
    #[serde(default)]
    ns: i64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    protection: Vec<Protection>,
    #[serde(default)]
    revisions: Vec<Revision>,
}
#[derive(Debug, Deserialize)]
struct Protection {
    #[serde(rename = "type")]
    kind: String,
    level: String,
    expiry: String,
}
#[derive(Debug, Deserialize)]
struct Revision {
    slots: Slots,
}
#[derive(Debug, Deserialize)]
struct Slots {
    main: Slot,
}
#[derive(Debug, Deserialize)]
struct Slot {
    contentmodel: String,
    #[serde(rename = "content", alias = "*")]
    content: String,
}

#[derive(Debug, Deserialize)]
struct CheckList {
    enabledusers: Vec<String>,
    enabledbots: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct WikiView {
    pub id: String,
    pub domain: String,
    pub name: String,
    pub check_page: String,
}

impl From<wiki::Model> for WikiView {
    fn from(value: wiki::Model) -> Self {
        Self {
            id: value.id,
            domain: value.domain,
            name: value.name,
            check_page: value.check_page,
        }
    }
}

pub async fn list(database: &DatabaseConnection) -> Result<Vec<wiki::Model>, DbErr> {
    wiki::Entity::find()
        .order_by_asc(wiki::Column::Name)
        .all(database)
        .await
}

pub async fn by_id(database: &DatabaseConnection, id: &str) -> Result<Option<wiki::Model>, DbErr> {
    wiki::Entity::find_by_id(id).one(database).await
}

pub async fn last_used(
    database: &DatabaseConnection,
    owner: &str,
) -> Result<Option<String>, DbErr> {
    Ok(user_preference::Entity::find_by_id(owner)
        .one(database)
        .await?
        .and_then(|preference| preference.last_wiki_id))
}

pub async fn save_last_used(
    database: &DatabaseConnection,
    owner: &str,
    wiki_id: &str,
) -> Result<(), DbErr> {
    if let Some(existing) = user_preference::Entity::find_by_id(owner)
        .one(database)
        .await?
    {
        let mut active: user_preference::ActiveModel = existing.into();
        active.last_wiki_id = Set(Some(wiki_id.to_owned()));
        active.update(database).await?;
    } else {
        user_preference::ActiveModel {
            owner: Set(owner.to_owned()),
            language: Set(localization::current_language()),
            last_wiki_id: Set(Some(wiki_id.to_owned())),
        }
        .insert(database)
        .await?;
    }
    Ok(())
}

pub async fn by_url(
    database: &DatabaseConnection,
    api_url: &str,
) -> Result<Option<wiki::Model>, DbErr> {
    wiki::Entity::find()
        .filter(wiki::Column::ApiUrl.eq(api_url))
        .one(database)
        .await
}

pub fn namespaces(wiki: &wiki::Model) -> Result<Vec<Namespace>, serde_json::Error> {
    serde_json::from_value(wiki.namespaces_json.clone())
}

fn normalized_domain(input: &str) -> Result<String, String> {
    let input = input.trim().trim_end_matches('/').to_ascii_lowercase();
    let domain = if let Some(domain) = input.strip_prefix("https://") {
        domain
    } else {
        &input
    };
    if domain.is_empty() || domain.contains(['/', ':', '?', '#', '@']) || !domain.is_ascii() {
        return Err("findsmith-api-enter-a-wikimedia-wiki-domain-such-as-en-wikipedia-org".into());
    }
    const FAMILIES: &[&str] = &[
        "wikipedia.org",
        "wiktionary.org",
        "wikibooks.org",
        "wikiquote.org",
        "wikisource.org",
        "wikinews.org",
        "wikiversity.org",
        "wikivoyage.org",
        "wikimedia.org",
        "wikidata.org",
        "mediawiki.org",
        "wikifunctions.org",
    ];
    if !FAMILIES
        .iter()
        .any(|family| domain == *family || domain.ends_with(&format!(".{family}")))
        || !domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
    {
        return Err("findsmith-api-only-wikimedia-project-domains-are-supported".into());
    }
    Ok(domain.to_owned())
}

async fn public_client(api_url: &str, user_agent: &str) -> Result<mwapi::Client, mwapi::Error> {
    crate::mediawiki::client_builder(api_url, user_agent)
        .build()
        .await
}

async fn check_page(api_url: &str, user_agent: &str) -> Result<(String, CheckList), String> {
    let client = public_client(api_url, user_agent)
        .await
        .map_err(|e| e.to_string())?;
    let response: CheckResponse = client
        .get([
            ("action", "query"),
            ("prop", "info|revisions"),
            ("inprop", "protection"),
            ("rvprop", "content|contentmodel"),
            ("rvslots", "main"),
            ("titles", "Project:AutoWikiBrowser/CheckPageJSON"),
            ("formatversion", "2"),
        ])
        .await
        .map_err(|e| e.to_string())?;
    let page = response
        .query
        .pages
        .into_iter()
        .next()
        .ok_or("findsmith-api-the-checkpagejson-page-was-not-returned")?;
    validate_check_page(page)
}

fn validate_check_page(page: CheckPage) -> Result<(String, CheckList), String> {
    if page.missing || page.ns != 4 {
        return Err(
            "findsmith-api-project-autowikibrowser-checkpagejson-does-not-exist-on-this-wiki"
                .into(),
        );
    }
    if !page.protection.iter().any(|item| {
        item.kind == "edit"
            && item.level == "sysop"
            && (item.expiry == "infinity"
                || chrono::DateTime::parse_from_rfc3339(&item.expiry)
                    .is_ok_and(|date| date > Utc::now()))
    }) {
        return Err(
            "findsmith-api-checkpagejson-must-be-protected-against-editing-by-non-admins".into(),
        );
    }
    let revision = page
        .revisions
        .into_iter()
        .next()
        .ok_or("findsmith-api-checkpagejson-has-no-readable-revision")?;
    if revision.slots.main.contentmodel != "json" {
        return Err("findsmith-api-checkpagejson-must-use-the-json-content-model".into());
    }
    let list: CheckList = serde_json::from_str(&revision.slots.main.content).map_err(|_| {
        "findsmith-api-checkpagejson-must-contain-enabledusers-and-enabledbots-arrays".to_owned()
    })?;
    Ok((page.title, list))
}

pub async fn register(
    database: &DatabaseConnection,
    user_agent: &str,
    input: &str,
) -> Result<wiki::Model, String> {
    let domain = normalized_domain(input)?;
    let api_url = format!("https://{domain}/w/api.php");
    if by_url(database, &api_url)
        .await
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Err("findsmith-api-this-wiki-is-already-in-the-list".into());
    }
    let client = public_client(&api_url, user_agent).await.map_err(|e| {
        crate::localization::required_with_args(
            "findsmith-api-wiki-connect-error",
            &[&e.to_string()],
        )
    })?;
    let response: SiteInfoResponse = client
        .get([
            ("action", "query"),
            ("meta", "siteinfo"),
            ("siprop", "general|namespaces"),
            ("formatversion", "2"),
        ])
        .await
        .map_err(|e| {
            crate::localization::required_with_args(
                "findsmith-api-wiki-info-error",
                &[&e.to_string()],
            )
        })?;
    let wiki_id = response.query.general.wikiid;
    if wiki_id.is_empty() || wiki_id.len() > 100 || response.query.namespaces.is_empty() {
        return Err(
            "findsmith-api-this-wiki-did-not-return-a-valid-wiki-id-or-namespace-list".into(),
        );
    }
    if by_id(database, &wiki_id)
        .await
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Err("findsmith-api-this-wiki-is-already-in-the-list".into());
    }
    let (check_page, _) = check_page(&api_url, user_agent).await?;
    let mut namespaces: Vec<Namespace> = response
        .query
        .namespaces
        .into_values()
        .filter(|entry| entry.id >= 0)
        .map(|entry| Namespace {
            id: entry.id,
            name: if entry.id == 0 {
                "Main".into()
            } else {
                entry.name
            },
        })
        .collect();
    namespaces.sort_by_key(|entry| entry.id);
    let now = Utc::now();
    let active = wiki::ActiveModel {
        id: Set(wiki_id),
        domain: Set(domain),
        api_url: Set(api_url),
        name: Set(response.query.general.sitename),
        check_page: Set(check_page),
        namespaces_json: Set(serde_json::to_value(namespaces).map_err(|e| e.to_string())?),
        created_at: Set(now),
        updated_at: Set(now),
    };
    active.insert(database).await.map_err(|error| {
        if matches!(error.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) {
            "findsmith-api-this-wiki-is-already-in-the-list".into()
        } else {
            error.to_string()
        }
    })
}

#[derive(Clone, Debug, Serialize)]
pub struct AccessView {
    pub wiki_id: String,
    pub wiki_name: String,
    pub check_page: String,
    pub allowed: Option<bool>,
    pub bot: bool,
    pub error: Option<String>,
}

const ACCESS_CHECK_TTL: Duration = Duration::from_secs(5 * 60);

struct CachedAccess {
    checked_at: Instant,
    value: AccessResult,
}

impl CachedAccess {
    fn is_fresh(&self) -> bool {
        self.checked_at.elapsed() < ACCESS_CHECK_TTL
    }
}

#[derive(Clone)]
struct AccessResult {
    allowed: Option<bool>,
    bot: bool,
    error: Option<String>,
    check_page_title: String,
}

fn access_cache() -> &'static Mutex<HashMap<(String, String), CachedAccess>> {
    static CACHE: OnceLock<Mutex<HashMap<(String, String), CachedAccess>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub async fn access(wiki: &wiki::Model, user_agent: &str, username: &str) -> AccessView {
    access_url(
        &wiki.id,
        &wiki.name,
        &wiki.domain,
        &wiki.api_url,
        user_agent,
        username,
    )
    .await
}

pub async fn access_unregistered(
    api_url: &str,
    user_agent: &str,
    username: &str,
) -> Option<AccessView> {
    let domain = url::Url::parse(api_url).ok()?.host_str()?.to_owned();
    Some(access_url(api_url, &domain, &domain, api_url, user_agent, username).await)
}

async fn access_url(
    wiki_id: &str,
    wiki_name: &str,
    domain: &str,
    api_url: &str,
    user_agent: &str,
    username: &str,
) -> AccessView {
    let key = (api_url.to_owned(), username.replace('_', " "));
    let cached = access_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&key)
        .filter(|entry| entry.is_fresh())
        .map(|entry| entry.value.clone());
    let result = if let Some(cached) = cached {
        cached
    } else {
        let checked = check_page(api_url, user_agent).await;
        let result = match checked {
            Ok((title, users)) => {
                let member = |list: &[String]| {
                    list.iter()
                        .any(|name| name.replace('_', " ") == username.replace('_', " "))
                };
                let bot = member(&users.enabledbots);
                AccessResult {
                    allowed: Some(bot || member(&users.enabledusers)),
                    bot,
                    error: None,
                    check_page_title: title,
                }
            }
            Err(error) => AccessResult {
                allowed: None,
                bot: false,
                error: Some(error),
                check_page_title: "Project:AutoWikiBrowser/CheckPageJSON".into(),
            },
        };
        let mut cache = access_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.retain(|_, entry| entry.is_fresh());
        cache.insert(
            key,
            CachedAccess {
                checked_at: Instant::now(),
                value: result.clone(),
            },
        );
        result
    };
    AccessView {
        wiki_id: wiki_id.to_owned(),
        wiki_name: wiki_name.to_owned(),
        check_page: format!(
            "https://{}/wiki/{}",
            domain,
            result.check_page_title.replace(' ', "_")
        ),
        allowed: result.allowed,
        bot: result.bot,
        error: result.error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn domain_rejects_non_wikimedia_hosts_and_paths() {
        assert_eq!(
            normalized_domain("https://en.wikipedia.org/"),
            Ok("en.wikipedia.org".into())
        );
        for value in [
            "evil.org",
            "en.wikipedia.org.evil.org",
            "en.wikipedia.org:8080",
            "en.wikipedia.org/w/api.php",
            "localhost",
        ] {
            assert!(normalized_domain(value).is_err(), "{value}");
        }
    }

    #[test]
    fn check_page_requires_protected_json_with_awb_lists() {
        let raw = serde_json::json!({
            "ns": 4, "title": "Wikipedia:AutoWikiBrowser/CheckPageJSON",
            "protection": [{"type": "edit", "level": "sysop", "expiry": "infinity"}],
            "revisions": [{"slots": {"main": {"contentmodel": "json", "content": "{\"enabledusers\":[\"Alice\"],\"enabledbots\":[\"Bot\"]}"}}}]
        });
        let page: CheckPage = serde_json::from_value(raw.clone()).unwrap();
        let (_, list) = validate_check_page(page).unwrap();
        assert_eq!(list.enabledusers, ["Alice"]);
        assert_eq!(list.enabledbots, ["Bot"]);
        let mut unprotected = raw;
        unprotected["protection"] = serde_json::json!([]);
        let page = serde_json::from_value(unprotected).unwrap();
        assert!(validate_check_page(page).is_err());
    }
}
