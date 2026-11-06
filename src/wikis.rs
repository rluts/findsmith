use crate::{
    entities::{user_preference, wiki},
    localization,
};
use chrono::Utc;
use mwapi::Client;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set, SqlErr,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
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

#[derive(Debug, Deserialize)]
struct WbEntitiesResponse {
    entities: HashMap<String, WbEntity>,
}

#[derive(Debug, Deserialize)]
struct WbEntity {
    #[serde(default)]
    sitelinks: HashMap<String, WbSitelink>,
}

#[derive(Debug, Deserialize)]
struct WbSitelink {
    title: String,
}

#[derive(Debug, Deserialize)]
struct RedirectsResponse {
    #[serde(default)]
    query: Option<RedirectsQuery>,
    #[serde(default, rename = "continue")]
    continuation: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
struct RedirectsQuery {
    pages: Vec<RedirectsPage>,
}

#[derive(Debug, Deserialize)]
struct RedirectsPage {
    #[serde(default)]
    redirects: Vec<RedirectEntry>,
}

#[derive(Debug, Deserialize)]
struct RedirectEntry {
    title: String,
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

async fn wikidata_client(user_agent: &str) -> Result<mwapi::Client, mwapi::Error> {
    crate::mediawiki::client_builder("https://www.wikidata.org/w/api.php", user_agent)
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
            ("rvprop", "content"),
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
        .ok_or("The CheckPageJSON page was not returned.")?;
    validate_check_page(page)
}

fn validate_check_page(page: CheckPage) -> Result<(String, CheckList), String> {
    if page.missing || page.ns != 4 {
        return Err(
            "findsmith-api-project-autowikibrowser-checkpagejson-does-not-exist-on-this-wiki"
                .into(),
        );
    }
    // Ideally sysop-protected, but some wiki (e.g. testwiki, dewiki) has other levels of protection
    if !page.protection.iter().any(|item| {
        item.kind == "edit"
            && !matches!(item.level.as_str(), "" | "*" | "all")
            && (item.expiry == "infinity"
                || chrono::DateTime::parse_from_rfc3339(&item.expiry)
                    .is_ok_and(|date| date > Utc::now()))
    }) {
        return Err("findsmith-api-checkpagejson-must-be-protected".into());
    }
    let revision = page
        .revisions
        .into_iter()
        .next()
        .ok_or("CheckPageJSON has no readable revision.")?;
    // Only the content matters: it must parse as the AWB lists, whatever its content model.
    let list: CheckList = serde_json::from_str(&revision.slots.main.content)
        .map_err(|_| "findsmith-api-checkpagejson-must-contain-awb-lists".to_owned())?;
    Ok((page.title, list))
}

/// Wikidata items of `Template:In use` and `Template:Under construction`.
const IN_USE_TEMPLATE_ITEMS: [&str; 2] = ["Q5617064", "Q10560270"];

async fn fetch_in_use_template(wiki_id: &str, user_agent: &str) -> Result<Vec<String>, String> {
    let ids = IN_USE_TEMPLATE_ITEMS.join("|");
    let wd_client = wikidata_client(user_agent)
        .await
        .map_err(|e| e.to_string())?;
    let response: WbEntitiesResponse = wd_client
        .get([
            ("action", "wbgetentities"),
            ("ids", ids.as_str()),
            ("props", "sitelinks"),
            ("sitefilter", wiki_id),
            ("formatversion", "2"),
        ])
        .await
        .map_err(|e| e.to_string())?;

    Ok(IN_USE_TEMPLATE_ITEMS
        .iter()
        .filter_map(|item| response.entities.get(*item)?.sitelinks.get(wiki_id))
        .map(|link| link.title.clone())
        .collect())
}

const MAX_REDIRECT_REQUESTS: usize = 10;

async fn load_templates_with_redirects(
    client: &Client,
    wiki_id: &str,
    user_agent: &str,
) -> Result<BTreeSet<String>, String> {
    let templates = fetch_in_use_template(wiki_id, user_agent).await?;
    if templates.is_empty() {
        return Ok(BTreeSet::new());
    }
    let mut params: Vec<(String, String)> = [
        ("action".to_string(), "query".to_string()),
        ("prop".to_string(), "redirects".to_string()),
        ("titles".to_string(), templates.join("|")),
        ("rdprop".to_string(), "title".to_string()),
        ("rdnamespace".to_string(), "10".to_string()),
        ("rdlimit".to_string(), "max".to_string()),
        ("formatversion".to_string(), "2".to_string()),
    ]
    .into_iter()
    .collect();
    let mut titles: BTreeSet<String> = templates.into_iter().collect();
    for _ in 0..MAX_REDIRECT_REQUESTS {
        let response: RedirectsResponse = client
            .get(params.clone())
            .await
            .map_err(|e| e.to_string())?;
        titles.extend(
            response
                .query
                .into_iter()
                .flat_map(|query| query.pages)
                .flat_map(|page| page.redirects)
                .map(|redirect| redirect.title),
        );
        let Some(next) = response.continuation else {
            return Ok(titles);
        };
        params.retain(|(key, _)| !key.ends_with("continue"));
        params.extend(next);
    }
    Ok(titles)
}

/// The "in use" and "under construction" templates of a wiki and their redirects, as
/// last stored. Unregistered wikis have none.
pub async fn in_use_templates(database: &DatabaseConnection, wiki_url: &str) -> Vec<String> {
    match by_url(database, wiki_url).await {
        Ok(Some(wiki)) => stored_in_use_templates(&wiki),
        Ok(None) => Vec::new(),
        Err(error) => {
            eprintln!("FindSmith: could not read in-use templates of {wiki_url}: {error}");
            Vec::new()
        }
    }
}

fn stored_in_use_templates(wiki: &wiki::Model) -> Vec<String> {
    serde_json::from_value(wiki.in_use_templates_json.clone()).unwrap_or_default()
}

async fn load_site_info(client: &Client) -> Result<SiteInfoResponse, mwapi::Error> {
    client
        .get([
            ("action", "query"),
            ("meta", "siteinfo"),
            ("siprop", "general|namespaces"),
            ("formatversion", "2"),
        ])
        .await
}

fn namespace_list(namespaces: HashMap<String, ApiNamespace>) -> Vec<Namespace> {
    let mut namespaces: Vec<Namespace> = namespaces
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
    namespaces
}

/// How often to look for wikis whose stored details need refreshing. A failed refresh
/// changes nothing, so it is tried again on the next check.
const WIKI_REFRESH_CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// How long a wiki's namespaces and templates are used before they are loaded again.
const WIKI_REFRESH_AFTER: chrono::TimeDelta = chrono::TimeDelta::days(7);
/// Sooner for a wiki with no templates: registration may have failed to load them, or
/// the wiki may have been registered before they were stored.
const EMPTY_TEMPLATES_REFRESH_AFTER: chrono::TimeDelta = chrono::TimeDelta::days(1);

fn refresh_due(
    updated_at: chrono::DateTime<Utc>,
    has_templates: bool,
    now: chrono::DateTime<Utc>,
) -> bool {
    let age = now - updated_at;
    age >= WIKI_REFRESH_AFTER || (!has_templates && age >= EMPTY_TEMPLATES_REFRESH_AFTER)
}

async fn refresh(
    database: &DatabaseConnection,
    user_agent: &str,
    wiki: wiki::Model,
) -> Result<(), String> {
    let client = public_client(&wiki.api_url, user_agent)
        .await
        .map_err(|e| e.to_string())?;
    let site_info = load_site_info(&client).await.map_err(|e| e.to_string())?;
    if site_info.query.general.wikiid != wiki.id || site_info.query.namespaces.is_empty() {
        return Err("the wiki did not return its wiki ID or namespace list".to_owned());
    }
    let templates = load_templates_with_redirects(&client, &wiki.id, user_agent).await?;
    let namespaces = namespace_list(site_info.query.namespaces);
    let mut active: wiki::ActiveModel = wiki.into();
    active.namespaces_json = Set(serde_json::to_value(namespaces).map_err(|e| e.to_string())?);
    // An empty result more likely means a broken lookup than that the templates are gone.
    if !templates.is_empty() {
        active.in_use_templates_json =
            Set(serde_json::to_value(templates).map_err(|e| e.to_string())?);
    }
    active.updated_at = Set(Utc::now());
    active.update(database).await.map_err(|e| e.to_string())?;
    Ok(())
}

async fn refresh_due_wikis(database: &DatabaseConnection, user_agent: &str) {
    let wikis = match list(database).await {
        Ok(wikis) => wikis,
        Err(error) => {
            eprintln!("FindSmith: could not list wikis to refresh: {error}");
            return;
        }
    };
    for wiki in wikis {
        let has_templates = !stored_in_use_templates(&wiki).is_empty();
        if !refresh_due(wiki.updated_at, has_templates, Utc::now()) {
            continue;
        }
        let id = wiki.id.clone();
        match refresh(database, user_agent, wiki).await {
            Ok(()) => println!("FindSmith: refreshed wiki {id}"),
            Err(error) => eprintln!("FindSmith: could not refresh wiki {id}: {error}"),
        }
    }
}

/// Keeps stored namespaces and templates current, starting with a check at startup.
pub fn spawn_refresh(database: DatabaseConnection, user_agent: String) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(WIKI_REFRESH_CHECK_INTERVAL);
        loop {
            interval.tick().await;
            refresh_due_wikis(&database, &user_agent).await;
        }
    });
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
        localization::required_with_args("findsmith-api-wiki-connect-error", &[&e.to_string()])
    })?;
    let response = load_site_info(&client).await.map_err(|e| {
        localization::required_with_args("findsmith-api-wiki-info-error", &[&e.to_string()])
    })?;
    let wiki_id = response.query.general.wikiid;
    if wiki_id.is_empty() || wiki_id.len() > 100 || response.query.namespaces.is_empty() {
        return Err("This wiki did not return a valid wiki ID or namespace list.".into());
    }
    if by_id(database, &wiki_id)
        .await
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Err("findsmith-api-this-wiki-is-already-in-the-list".into());
    }
    let (check_page, _) = check_page(&api_url, user_agent).await?;
    let namespaces = namespace_list(response.query.namespaces);
    // A failed template lookup must not block registration; the refresh task retries it.
    let in_use_templates = load_templates_with_redirects(&client, &wiki_id, user_agent)
        .await
        .unwrap_or_else(|error| {
            eprintln!("FindSmith: could not load in-use templates of {api_url}: {error}");
            BTreeSet::new()
        });
    let now = Utc::now();
    let active = wiki::ActiveModel {
        id: Set(wiki_id),
        domain: Set(domain),
        api_url: Set(api_url),
        name: Set(response.query.general.sitename),
        check_page: Set(check_page),
        namespaces_json: Set(serde_json::to_value(namespaces).map_err(|e| e.to_string())?),
        in_use_templates_json: Set(
            serde_json::to_value(in_use_templates).map_err(|e| e.to_string())?
        ),
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
    fn wikis_refresh_weekly_and_sooner_without_templates() {
        let now = Utc::now();
        let age = |days| now - chrono::TimeDelta::days(days);
        assert!(!refresh_due(now, true, now));
        assert!(!refresh_due(age(6), true, now));
        assert!(refresh_due(age(7), true, now));
        assert!(!refresh_due(now, false, now));
        assert!(refresh_due(age(1), false, now));
    }

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
    fn check_page_requires_protection_and_awb_lists() {
        let raw = serde_json::json!({
            "ns": 4, "title": "Wikipedia:AutoWikiBrowser/CheckPageJSON",
            "protection": [{"type": "edit", "level": "sysop", "expiry": "infinity"}],
            "revisions": [{"slots": {"main": {"contentmodel": "json", "content": "{\"enabledusers\":[\"Alice\"],\"enabledbots\":[\"Bot\"]}"}}}]
        });
        let page: CheckPage = serde_json::from_value(raw.clone()).unwrap();
        let (_, list) = validate_check_page(page).unwrap();
        assert_eq!(list.enabledusers, ["Alice"]);
        assert_eq!(list.enabledbots, ["Bot"]);
        let mut wikitext_model = raw.clone();
        wikitext_model["revisions"][0]["slots"]["main"]["contentmodel"] = "wikitext".into();
        let page = serde_json::from_value(wikitext_model).unwrap();
        assert!(validate_check_page(page).is_ok());
        let mut semi_protected = raw.clone();
        semi_protected["protection"][0]["level"] = "autoconfirmed".into();
        let page = serde_json::from_value(semi_protected).unwrap();
        assert!(validate_check_page(page).is_ok());
        let mut move_only = raw.clone();
        move_only["protection"][0]["type"] = "move".into();
        let page = serde_json::from_value(move_only).unwrap();
        assert!(validate_check_page(page).is_err());
        let mut unprotected = raw;
        unprotected["protection"] = serde_json::json!([]);
        let page = serde_json::from_value(unprotected).unwrap();
        assert_eq!(
            validate_check_page(page).unwrap_err(),
            "findsmith-api-checkpagejson-must-be-protected"
        );
    }
}
