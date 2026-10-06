use std::{error::Error, fmt};

use mwbot::{
    Bot,
    generators::{
        EmbeddedIn, Generator, categories::CategoryMembers, link::LinksHere, search::Search,
    },
};
use parsoid::prelude::*;
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::{Mutex, OnceLock},
};

use crate::AppResult;

#[derive(Clone, Copy, Debug)]
pub enum PageListKind {
    CategoryMembers,
    Backlinks,
    Transclusions,
    Search,
}

#[derive(Clone, Copy, Debug)]
pub enum ApiPageListKind {
    Watchlist,
    Contributions,
    LinksOnPage,
    CategoriesOnPage,
    RedirectsToPage,
}

pub async fn api_page_list(
    wiki_url: &str,
    user_agent: &str,
    access_token: &str,
    kind: ApiPageListKind,
    value: &str,
    namespaces: &[i64],
    limit: usize,
) -> Result<Vec<NormalizedTitle>, String> {
    let client = authenticated_client(wiki_url, user_agent, access_token)
        .await
        .map_err(|error| error.to_string())?;
    let (module, name, key, argument) = match kind {
        ApiPageListKind::Watchlist => ("list", "watchlistraw", "watchlistraw", ""),
        ApiPageListKind::Contributions => ("list", "usercontribs", "usercontribs", "ucuser"),
        ApiPageListKind::LinksOnPage => ("prop", "links", "links", "titles"),
        ApiPageListKind::CategoriesOnPage => ("prop", "categories", "categories", "titles"),
        ApiPageListKind::RedirectsToPage => ("prop", "redirects", "redirects", "titles"),
    };
    let limit_parameter = match kind {
        ApiPageListKind::Watchlist => "wrlimit",
        ApiPageListKind::Contributions => "uclimit",
        ApiPageListKind::LinksOnPage => "pllimit",
        ApiPageListKind::CategoriesOnPage => "cllimit",
        ApiPageListKind::RedirectsToPage => "rdlimit",
    };
    let namespace_parameter = match kind {
        ApiPageListKind::Watchlist => "wrnamespace",
        ApiPageListKind::Contributions => "ucnamespace",
        ApiPageListKind::LinksOnPage => "plnamespace",
        ApiPageListKind::CategoriesOnPage => "clnamespace",
        ApiPageListKind::RedirectsToPage => "rdnamespace",
    };
    let mut params = vec![
        ("action".to_owned(), "query".to_owned()),
        (module.to_owned(), name.to_owned()),
        (limit_parameter.to_owned(), "max".to_owned()),
        ("formatversion".to_owned(), "2".to_owned()),
    ];
    if !argument.is_empty() {
        params.push((argument.to_owned(), value.to_owned()));
    }
    if !namespaces.is_empty() && !matches!(kind, ApiPageListKind::CategoriesOnPage) {
        params.push((
            namespace_parameter.to_owned(),
            namespaces
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join("|"),
        ));
    }
    let mut titles = Vec::new();
    let mut seen = HashSet::new();
    let mut requests = 0;
    while titles.len() < limit && requests < 20 {
        requests += 1;
        let response: Value = client
            .get(params.clone())
            .await
            .map_err(|error| error.to_string())?;
        for title in api_list_titles(&response, module, key) {
            if seen.insert(title.to_owned()) {
                titles.push(title.to_owned());
            }
            if titles.len() == limit {
                break;
            }
        }
        let Some(next) = response.get("continue").and_then(Value::as_object) else {
            break;
        };
        params.retain(|(key, _)| key != "continue" && !key.ends_with("continue"));
        for (key, value) in next {
            if let Some(value) = value.as_str() {
                params.push((key.clone(), value.to_owned()));
            }
        }
        if !next.keys().any(|key| key != "continue") {
            break;
        }
    }
    let mut pages = normalize_titles_impl(wiki_url, user_agent, access_token, &titles, false)
        .await
        .map_err(|error| error.to_string())?;
    if !namespaces.is_empty() {
        pages.retain(|page| page.namespace.is_some_and(|id| namespaces.contains(&id)));
    }
    Ok(pages)
}

fn api_list_titles<'a>(response: &'a Value, module: &str, key: &str) -> Vec<&'a str> {
    let items = if module == "list" {
        response.pointer(&format!("/query/{key}"))
    } else {
        response.pointer(&format!("/query/pages/0/{key}"))
    };
    items
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("title").and_then(Value::as_str))
        .collect()
}

pub struct UserIdentity {
    pub central_id: String,
    pub username: String,
}

pub struct PageSnapshot {
    pub page_id: u64,
    pub title: String,
    pub namespace: i64,
    pub content_model: String,
    pub redirect: bool,
    pub protection: Vec<PageProtection>,
    pub editable: bool,
    pub revision_id: u64,
    pub revision_timestamp: String,
    pub server_timestamp: String,
    pub full_url: String,
    pub content: String,
}

pub struct PageProtection {
    pub kind: String,
    pub level: String,
    pub expiry: String,
}

pub struct ParsedPreview {
    pub html: String,
    pub title: String,
    pub module_styles: Vec<String>,
}

#[derive(Deserialize)]
struct ParseResponse {
    parse: ParseResult,
}

#[derive(Deserialize)]
struct ParseResult {
    title: String,
    text: ParseText,
    #[serde(default)]
    modulestyles: Vec<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ParseText {
    Modern(String),
    Legacy {
        #[serde(rename = "*")]
        html: String,
    },
}

pub async fn parse_preview(
    wiki_url: &str,
    user_agent: &str,
    access_token: &str,
    title: &str,
    text: &str,
) -> Result<ParsedPreview, mwapi::Error> {
    let client = authenticated_client(wiki_url, user_agent, access_token).await?;
    let response: ParseResponse = client
        .post([
            ("action", "parse"),
            ("title", title),
            ("text", text),
            ("contentmodel", "wikitext"),
            ("prop", "text|modules|jsconfigvars"),
            ("preview", "1"),
            ("disableeditsection", "1"),
            ("useskin", "vector"),
            ("formatversion", "2"),
        ])
        .await?;
    Ok(ParsedPreview {
        html: match response.parse.text {
            ParseText::Modern(html) | ParseText::Legacy { html } => html,
        },
        title: response.parse.title,
        module_styles: response.parse.modulestyles,
    })
}

#[derive(Clone, Debug)]
pub struct NormalizedTitle {
    pub requested_title: String,
    pub title: String,
    pub page_id: Option<u64>,
    pub namespace: Option<i64>,
    pub redirect: bool,
    pub invalid_reason: Option<String>,
}

#[derive(Debug)]
pub enum PageLoadError {
    EmptyTitle,
    Missing,
    Invalid(String),
    MissingRevision,
    MissingContent,
    UnsupportedContentModel(String),
    Api(mwapi::Error),
}

impl fmt::Display for PageLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTitle => formatter.write_str("Enter a page title."),
            Self::Missing => formatter.write_str("The requested page does not exist."),
            Self::Invalid(reason) if !reason.is_empty() => {
                write!(formatter, "The page title is invalid: {reason}")
            }
            Self::Invalid(_) => formatter.write_str("The page title is invalid."),
            Self::MissingRevision => formatter.write_str("The page has no readable revision."),
            Self::MissingContent => formatter.write_str("The page content could not be loaded."),
            Self::UnsupportedContentModel(model) => {
                write!(
                    formatter,
                    "The page uses unsupported content model “{model}”."
                )
            }
            Self::Api(error) => write!(formatter, "MediaWiki request failed: {error}"),
        }
    }
}

impl Error for PageLoadError {}

impl From<mwapi::Error> for PageLoadError {
    fn from(error: mwapi::Error) -> Self {
        Self::Api(error)
    }
}

#[derive(Deserialize)]
struct GlobalUserInfoResponse {
    query: GlobalUserInfoQuery,
}

#[derive(Deserialize)]
struct GlobalUserInfoQuery {
    globaluserinfo: GlobalUserInfo,
}

#[derive(Deserialize)]
struct GlobalUserInfo {
    id: u64,
    name: String,
}

#[derive(Deserialize)]
struct UserInfoResponse {
    query: UserInfoQuery,
}

#[derive(Deserialize)]
struct UserInfoQuery {
    userinfo: UserInfo,
}

#[derive(Deserialize)]
struct UserInfo {
    #[serde(default)]
    groups: Vec<String>,
    #[serde(default)]
    rights: Vec<String>,
    #[serde(default)]
    blockedby: Option<String>,
    #[serde(default)]
    messages: bool,
}

pub struct EditPermissions {
    pub can_read: bool,
    pub can_edit: bool,
    pub can_minor: bool,
    pub can_bot: bool,
    pub blocked: bool,
    pub new_messages: bool,
}

pub async fn edit_permissions(
    wiki_url: &str,
    user_agent: &str,
    access_token: &str,
) -> Result<EditPermissions, mwapi::Error> {
    let client = authenticated_client(wiki_url, user_agent, access_token).await?;
    let response: UserInfoResponse = client
        .get([
            ("action", "query"),
            ("meta", "userinfo"),
            ("uiprop", "rights|groups|blockinfo|hasmsg"),
        ])
        .await?;
    let user = response.query.userinfo;
    Ok(EditPermissions {
        can_read: user.rights.iter().any(|right| right == "read"),
        can_edit: user.rights.iter().any(|right| right == "edit"),
        can_minor: user.rights.iter().any(|right| right == "minoredit"),
        can_bot: can_mark_bot_edit(&user),
        blocked: user.blockedby.is_some(),
        new_messages: user.messages,
    })
}

fn can_mark_bot_edit(user: &UserInfo) -> bool {
    user.groups
        .iter()
        .any(|group| matches!(group.as_str(), "bot" | "flood" | "flooder"))
        && user.rights.iter().any(|right| right == "bot")
}

pub async fn bot_edit_allowed(
    wiki_url: &str,
    user_agent: &str,
    username: &str,
    text: &str,
) -> Result<bool, String> {
    let rest_url = wiki_url
        .strip_suffix("api.php")
        .ok_or("The wiki has no supported Parsoid endpoint.")?;
    let client = ParsoidClient::new(&format!("{rest_url}rest.php"), user_agent)
        .map_err(|error| format!("Could not connect to Parsoid: {error}"))?;
    let code = client
        .transform_to_html(text)
        .await
        .map_err(|error| format!("Could not inspect bot exclusions: {error}"))?
        .into_mutable();
    let templates = code
        .filter_templates()
        .map_err(|error| format!("Could not inspect bot exclusions: {error}"))?;
    let user = username.replace('_', " ").to_lowercase();
    for template in templates {
        let name = template.name_in_wikitext().replace('_', " ").to_lowercase();
        let name = name.strip_prefix("template:").unwrap_or(&name).trim();
        if name == "nobots" {
            return Ok(false);
        }
        if name != "bots" {
            continue;
        }
        if let Some(allow) = template.param("allow") {
            let allowed = allow
                .split(',')
                .map(|name| name.trim().replace('_', " ").to_lowercase())
                .any(|name| name == "all" || name == user);
            if !allowed {
                return Ok(false);
            }
        }
        if let Some(deny) = template.param("deny") {
            let denied = deny
                .split(',')
                .map(|name| name.trim().replace('_', " ").to_lowercase())
                .any(|name| name == "all" || name == user);
            if denied {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditFailureKind {
    Authentication,
    Permission,
    Block,
    Protection,
    Captcha,
    Spam,
    Conflict,
    Maxlag,
    AbuseFilter,
    Unknown,
}

impl EditFailureKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Authentication => "authentication",
            Self::Permission => "permission",
            Self::Block => "block",
            Self::Protection => "protection",
            Self::Captcha => "captcha",
            Self::Spam => "spam",
            Self::Conflict => "conflict",
            Self::Maxlag => "maxlag",
            Self::AbuseFilter => "abuse filter",
            Self::Unknown => "unknown",
        }
    }
}

pub fn classify_edit_error(error: &mwapi::Error) -> EditFailureKind {
    use EditFailureKind as Kind;
    let code = match error {
        mwapi::Error::NotLoggedIn | mwapi::Error::NotLoggedInAsBot | mwapi::Error::BadToken => {
            return Kind::Authentication;
        }
        mwapi::Error::Maxlag { .. }
        | mwapi::Error::TooManyRequests { .. }
        | mwapi::Error::Readonly { .. } => return Kind::Maxlag,
        mwapi::Error::ApiError(error) => error.code.to_ascii_lowercase(),
        _ => return Kind::Unknown,
    };
    if code.contains("abusefilter") {
        Kind::AbuseFilter
    } else if code.contains("captcha") {
        Kind::Captcha
    } else if code.contains("spam") {
        Kind::Spam
    } else if code.contains("editconflict") || code == "articleexists" {
        Kind::Conflict
    } else if code.contains("protected") || code.contains("cascade") {
        Kind::Protection
    } else if code.contains("blocked") || code.contains("block") {
        Kind::Block
    } else if code.contains("assert") || code.contains("token") || code.contains("login") {
        Kind::Authentication
    } else if code.contains("permission")
        || code.contains("denied")
        || code.contains("create")
        || code.contains("right")
    {
        Kind::Permission
    } else {
        Kind::Unknown
    }
}

fn edit_clients() -> &'static Mutex<HashMap<(String, String), mwapi::Client>> {
    static CLIENTS: OnceLock<Mutex<HashMap<(String, String), mwapi::Client>>> = OnceLock::new();
    CLIENTS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[allow(clippy::too_many_arguments)]
pub async fn edit_existing_page(
    wiki_url: &str,
    user_agent: &str,
    access_token: &str,
    username: &str,
    title: &str,
    text: &str,
    revision_id: u64,
    revision_timestamp: &str,
    start_timestamp: &str,
    minor: bool,
    bot: bool,
) -> Result<u64, mwapi::Error> {
    // Cloned clients share mwapi's CSRF token cache. Keep the cache bounded and server-side.
    let key = (wiki_url.to_owned(), access_token.to_owned());
    let cached = edit_clients()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&key)
        .cloned();
    let client = if let Some(client) = cached {
        client
    } else {
        let client = mwapi::Client::builder(wiki_url)
            .set_oauth2_token(access_token)
            .set_user_agent(user_agent)
            .set_assert(mwapi::Assert::User)
            .set_maxlag(5)
            .set_retry_limit(0)
            .build()
            .await?;
        let mut clients = edit_clients()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if clients.len() >= 128 {
            clients.clear();
        }
        clients.insert(key, client.clone());
        client
    };
    let mut params = vec![
        ("action".to_owned(), "edit".to_owned()),
        ("title".to_owned(), title.to_owned()),
        ("text".to_owned(), text.to_owned()),
        ("summary".to_owned(), "Autoreplacement".to_owned()),
        ("assertuser".to_owned(), username.to_owned()),
        ("baserevid".to_owned(), revision_id.to_string()),
        ("basetimestamp".to_owned(), revision_timestamp.to_owned()),
        ("starttimestamp".to_owned(), start_timestamp.to_owned()),
        ("nocreate".to_owned(), "1".to_owned()),
    ];
    if minor {
        params.push(("minor".to_owned(), "1".to_owned()));
    }
    if bot {
        params.push(("bot".to_owned(), "1".to_owned()));
    }
    let response: Value = client.post_with_token("csrf", params).await?;
    if response.pointer("/edit/result").and_then(Value::as_str) != Some("Success") {
        let code = if response.pointer("/edit/captcha").is_some() {
            "captcha"
        } else if response.pointer("/edit/spamblacklist").is_some() {
            "spamdetected"
        } else if response.pointer("/edit/abusefilter").is_some() {
            "abusefilter-warning"
        } else {
            "edit-failed"
        };
        return Err(mwapi::Error::ApiError(mwapi::ApiError {
            code: code.to_owned(),
            text: "MediaWiki did not save the edit.".to_owned(),
            data: None,
        }));
    }
    response
        .pointer("/edit/newrevid")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            mwapi::Error::Unknown("MediaWiki did not return the new revision ID.".to_owned())
        })
}

#[derive(Deserialize)]
struct PageResponse {
    curtimestamp: String,
    query: PageQuery,
}

#[derive(Deserialize)]
struct NormalizeResponse {
    query: NormalizeQuery,
}

#[derive(Deserialize)]
struct NormalizeQuery {
    #[serde(default)]
    normalized: Vec<TitleMapping>,
    #[serde(default)]
    redirects: Vec<TitleMapping>,
    pages: Vec<NormalizePage>,
}

#[derive(Deserialize)]
struct TitleMapping {
    from: String,
    to: String,
}

#[derive(Deserialize)]
struct NormalizePage {
    #[serde(default)]
    pageid: u64,
    #[serde(default)]
    ns: i64,
    title: String,
    #[serde(default)]
    missing: bool,
    #[serde(default)]
    redirect: bool,
    #[serde(default)]
    invalid: bool,
    #[serde(default)]
    invalidreason: String,
}

#[derive(Deserialize)]
struct PageQuery {
    pages: Vec<ApiPage>,
}

#[derive(Deserialize)]
struct ApiPage {
    #[serde(default)]
    pageid: u64,
    #[serde(default)]
    ns: i64,
    title: String,
    #[serde(default)]
    contentmodel: String,
    #[serde(default)]
    fullurl: String,
    #[serde(default)]
    redirect: bool,
    #[serde(default)]
    missing: bool,
    #[serde(default)]
    invalid: bool,
    #[serde(default)]
    invalidreason: String,
    #[serde(default)]
    protection: Vec<ApiProtection>,
    #[serde(default)]
    actions: ApiActions,
    #[serde(default)]
    revisions: Vec<ApiRevision>,
}

#[derive(Default, Deserialize)]
struct ApiActions {
    #[serde(default)]
    edit: bool,
}

#[derive(Deserialize)]
struct ApiProtection {
    #[serde(rename = "type")]
    kind: String,
    level: String,
    expiry: String,
}

#[derive(Deserialize)]
struct ApiRevision {
    revid: u64,
    timestamp: String,
    slots: ApiSlots,
}

#[derive(Deserialize)]
struct ApiSlots {
    main: ApiSlot,
}

#[derive(Deserialize)]
struct ApiSlot {
    contentmodel: String,
    #[serde(rename = "content", alias = "*")]
    content: Option<String>,
}

pub async fn owner_identity(
    wiki_url: &str,
    user_agent: &str,
    access_token: &str,
) -> AppResult<UserIdentity> {
    let client = authenticated_client(wiki_url, user_agent, access_token).await?;
    let response: GlobalUserInfoResponse = client
        .get([("action", "query"), ("meta", "globaluserinfo")])
        .await?;
    let user = response.query.globaluserinfo;
    if user.id == 0 || user.name.trim().is_empty() {
        return Err("OWNER_ACCESS_TOKEN returned an invalid global user".into());
    }

    Ok(UserIdentity {
        central_id: user.id.to_string(),
        username: user.name,
    })
}

pub async fn load_page(
    wiki_url: &str,
    user_agent: &str,
    access_token: &str,
    title: &str,
) -> Result<PageSnapshot, PageLoadError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(PageLoadError::EmptyTitle);
    }
    let client = authenticated_client(wiki_url, user_agent, access_token).await?;
    let response: PageResponse = client
        .get([
            ("action", "query"),
            ("prop", "info|revisions"),
            ("inprop", "protection|url"),
            ("intestactions", "edit"),
            ("rvprop", "ids|timestamp|content|contentmodel"),
            ("rvslots", "main"),
            ("titles", title),
            ("curtimestamp", "1"),
            ("formatversion", "2"),
        ])
        .await?;
    let page = response
        .query
        .pages
        .into_iter()
        .next()
        .ok_or(PageLoadError::Missing)?;
    if page.invalid {
        return Err(PageLoadError::Invalid(page.invalidreason));
    }
    if page.missing {
        return Err(PageLoadError::Missing);
    }
    let revision = page
        .revisions
        .into_iter()
        .next()
        .ok_or(PageLoadError::MissingRevision)?;
    let content_model = if revision.slots.main.contentmodel.is_empty() {
        page.contentmodel
    } else {
        revision.slots.main.contentmodel
    };
    if content_model != "wikitext" {
        return Err(PageLoadError::UnsupportedContentModel(content_model));
    }
    let content = revision
        .slots
        .main
        .content
        .ok_or(PageLoadError::MissingContent)?;

    Ok(PageSnapshot {
        page_id: page.pageid,
        title: page.title,
        namespace: page.ns,
        content_model,
        redirect: page.redirect,
        protection: page
            .protection
            .into_iter()
            .map(|protection| PageProtection {
                kind: protection.kind,
                level: protection.level,
                expiry: protection.expiry,
            })
            .collect(),
        editable: page.actions.edit,
        revision_id: revision.revid,
        revision_timestamp: revision.timestamp,
        server_timestamp: response.curtimestamp,
        full_url: page.fullurl,
        content,
    })
}

pub async fn normalize_titles(
    wiki_url: &str,
    user_agent: &str,
    access_token: &str,
    titles: &[String],
) -> Result<Vec<NormalizedTitle>, mwapi::Error> {
    normalize_titles_impl(wiki_url, user_agent, access_token, titles, true).await
}

async fn normalize_titles_impl(
    wiki_url: &str,
    user_agent: &str,
    access_token: &str,
    titles: &[String],
    follow_redirects: bool,
) -> Result<Vec<NormalizedTitle>, mwapi::Error> {
    const BATCH_SIZE: usize = 50;
    let client = authenticated_client(wiki_url, user_agent, access_token).await?;
    let mut output = Vec::with_capacity(titles.len());

    for batch in titles.chunks(BATCH_SIZE) {
        let mut params = vec![("action", "query"), ("titles", ""), ("formatversion", "2")];
        let joined = batch.join("|");
        params[1].1 = &joined;
        if follow_redirects {
            params.push(("redirects", "1"));
        }
        let response: NormalizeResponse = client.get(params).await?;
        output.extend(resolve_normalized_batch(batch, response.query));
    }
    Ok(output)
}

pub async fn page_list(
    bot: &Bot,
    kind: PageListKind,
    value: &str,
    namespaces: &[i64],
    limit: usize,
) -> Result<Vec<NormalizedTitle>, mwbot::Error> {
    let namespaces: Vec<u32> = namespaces
        .iter()
        .filter_map(|namespace| u32::try_from(*namespace).ok())
        .collect();
    let mut pages = match kind {
        PageListKind::CategoryMembers => {
            let generator = CategoryMembers::new(value.to_owned());
            if namespaces.is_empty() {
                generator.generate(bot)
            } else {
                generator.namespace(namespaces).generate(bot)
            }
        }
        PageListKind::Backlinks => {
            let generator = LinksHere::new(vec![value.to_owned()]);
            if namespaces.is_empty() {
                generator.generate(bot)
            } else {
                generator.namespaces(namespaces).generate(bot)
            }
        }
        PageListKind::Transclusions => {
            let generator = EmbeddedIn::new(value.to_owned());
            if namespaces.is_empty() {
                generator.generate(bot)
            } else {
                generator.namespace(namespaces).generate(bot)
            }
        }
        PageListKind::Search => {
            let generator = Search::new(value.to_owned());
            if namespaces.is_empty() {
                generator.generate(bot)
            } else {
                generator.namespace(namespaces).generate(bot)
            }
        }
    };
    let mut output = Vec::new();
    while output.len() < limit
        && let Some(page) = pages.recv().await
    {
        let page = page?;
        let title = page.title().to_owned();
        output.push(NormalizedTitle {
            requested_title: title.clone(),
            title,
            page_id: page.id().await?.map(u64::from),
            namespace: Some(i64::from(page.namespace())),
            redirect: page.is_redirect().await?,
            invalid_reason: None,
        });
    }
    Ok(output)
}

pub async fn page_list_bot(
    wiki_url: &str,
    username: &str,
    access_token: &str,
    user_agent: &str,
) -> Result<Bot, mwbot::ConfigError> {
    let script_path = wiki_url.strip_suffix("api.php").unwrap_or(wiki_url);
    Bot::builder(script_path.to_owned())
        .set_oauth2_token(username.to_owned(), access_token.to_owned())
        .set_user_agent(user_agent.to_owned())
        .set_mark_as_bot(false)
        .set_respect_nobots(true)
        .build()
        .await
}

pub async fn user_groups(
    wiki_url: &str,
    user_agent: &str,
    access_token: &str,
) -> Result<Vec<String>, mwapi::Error> {
    let client = authenticated_client(wiki_url, user_agent, access_token).await?;
    let response: UserInfoResponse = client
        .get([
            ("action", "query"),
            ("meta", "userinfo"),
            ("uiprop", "groups"),
        ])
        .await?;
    Ok(response.query.userinfo.groups)
}

fn resolve_normalized_batch(batch: &[String], query: NormalizeQuery) -> Vec<NormalizedTitle> {
    let normalized: HashMap<_, _> = query
        .normalized
        .into_iter()
        .map(|mapping| (mapping.from, mapping.to))
        .collect();
    let redirects: HashMap<_, _> = query
        .redirects
        .into_iter()
        .map(|mapping| (mapping.from, mapping.to))
        .collect();
    let pages: HashMap<_, _> = query
        .pages
        .into_iter()
        .map(|page| (page.title.clone(), page))
        .collect();

    batch
        .iter()
        .map(|requested| {
            let normalized_title = normalized
                .get(requested)
                .cloned()
                .unwrap_or_else(|| requested.clone());
            let title = redirects
                .get(&normalized_title)
                .cloned()
                .unwrap_or_else(|| normalized_title.clone());
            let redirected = title != normalized_title;
            let page = pages.get(&title).or_else(|| pages.get(&normalized_title));
            match page {
                Some(page) => NormalizedTitle {
                    requested_title: requested.clone(),
                    title: page.title.clone(),
                    page_id: (!page.missing && !page.invalid).then_some(page.pageid),
                    namespace: (!page.invalid).then_some(page.ns),
                    redirect: redirected || page.redirect,
                    invalid_reason: page.invalid.then(|| {
                        if page.invalidreason.is_empty() {
                            "The page title is invalid.".to_owned()
                        } else {
                            page.invalidreason.clone()
                        }
                    }),
                },
                None => NormalizedTitle {
                    requested_title: requested.clone(),
                    title,
                    page_id: None,
                    namespace: None,
                    redirect: redirected,
                    invalid_reason: Some("MediaWiki did not return this title.".to_owned()),
                },
            }
        })
        .collect()
}

async fn authenticated_client(
    wiki_url: &str,
    user_agent: &str,
    access_token: &str,
) -> Result<mwapi::Client, mwapi::Error> {
    mwapi::Client::builder(wiki_url)
        .set_oauth2_token(access_token)
        .set_user_agent(user_agent)
        .set_assert(mwapi::Assert::User)
        .build()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bot_edit_requires_bot_or_flood_group_and_right() {
        let user: UserInfo = serde_json::from_value(serde_json::json!({
            "groups": ["user", "bot"], "rights": ["edit", "bot"]
        }))
        .unwrap();
        assert!(can_mark_bot_edit(&user));
        for group in ["flood", "flooder"] {
            let flood_user: UserInfo = serde_json::from_value(serde_json::json!({
                "groups": ["user", group], "rights": ["edit", "bot"]
            }))
            .unwrap();
            assert!(can_mark_bot_edit(&flood_user));
        }
        let right_only: UserInfo = serde_json::from_value(serde_json::json!({
            "groups": ["user"], "rights": ["edit", "bot"]
        }))
        .unwrap();
        assert!(!can_mark_bot_edit(&right_only));
        let group_only: UserInfo = serde_json::from_value(serde_json::json!({
            "groups": ["user", "bot"], "rights": ["edit"]
        }))
        .unwrap();
        assert!(!can_mark_bot_edit(&group_only));
        let flood_without_right: UserInfo = serde_json::from_value(serde_json::json!({
            "groups": ["user", "flood"], "rights": ["edit"]
        }))
        .unwrap();
        assert!(!can_mark_bot_edit(&flood_without_right));
    }

    #[test]
    fn edit_errors_are_classified_without_exposing_server_text() {
        let error = |code: &str| {
            mwapi::Error::ApiError(mwapi::ApiError {
                code: code.to_owned(),
                text: "private page text".to_owned(),
                data: None,
            })
        };
        assert_eq!(
            classify_edit_error(&error("abusefilter-warning")),
            EditFailureKind::AbuseFilter
        );
        assert_eq!(
            classify_edit_error(&error("protectedpage")),
            EditFailureKind::Protection
        );
        assert_eq!(
            classify_edit_error(&error("editconflict")),
            EditFailureKind::Conflict
        );
        assert_eq!(
            classify_edit_error(&error("spamdetected")),
            EditFailureKind::Spam
        );
        assert_eq!(
            classify_edit_error(&error("captcha")),
            EditFailureKind::Captcha
        );
        assert_eq!(
            classify_edit_error(&mwapi::Error::Maxlag {
                info: String::new(),
                retry_after: None
            }),
            EditFailureKind::Maxlag
        );
    }

    #[test]
    fn additional_source_shapes_read_titles() {
        let watchlist =
            serde_json::json!({"query": {"watchlistraw": [{"title": "A"}, {"title": "B"}]}});
        assert_eq!(
            api_list_titles(&watchlist, "list", "watchlistraw"),
            ["A", "B"]
        );
        let links = serde_json::json!({"query": {"pages": [{"title": "Source", "links": [{"title": "Target"}]}]}});
        assert_eq!(api_list_titles(&links, "prop", "links"), ["Target"]);
    }

    #[test]
    fn redirect_source_keeps_redirect_page_title() {
        let pages = resolve_normalized_batch(
            &["Alias".to_owned()],
            NormalizeQuery {
                normalized: vec![],
                redirects: vec![],
                pages: vec![NormalizePage {
                    pageid: 7,
                    ns: 0,
                    title: "Alias".to_owned(),
                    missing: false,
                    redirect: true,
                    invalid: false,
                    invalidreason: String::new(),
                }],
            },
        );
        assert_eq!(pages[0].title, "Alias");
        assert_eq!(pages[0].page_id, Some(7));
        assert!(pages[0].redirect);
    }

    #[test]
    fn parse_preview_accepts_modern_and_legacy_text_shapes() {
        for (text, expected) in [
            (serde_json::json!("<p>Preview</p>"), "<p>Preview</p>"),
            (serde_json::json!({"*": "<p>Legacy</p>"}), "<p>Legacy</p>"),
        ] {
            let response: ParseResponse = serde_json::from_value(serde_json::json!({
                "parse": { "title": "Example", "text": text, "modulestyles": ["ext.cite.styles"] }
            }))
            .unwrap();
            assert_eq!(response.parse.modulestyles, ["ext.cite.styles"]);
            assert_eq!(
                match response.parse.text {
                    ParseText::Modern(html) | ParseText::Legacy { html } => html,
                },
                expected
            );
        }
    }

    #[test]
    fn userinfo_deserialization_keeps_all_groups() {
        let response: UserInfoResponse = serde_json::from_value(serde_json::json!({
            "query": {
                "userinfo": {
                    "groups": ["*", "user", "autoconfirmed", "sysop"]
                }
            }
        }))
        .unwrap();
        assert_eq!(
            response.query.userinfo.groups,
            ["*", "user", "autoconfirmed", "sysop"]
        );
    }

    #[test]
    fn normalized_titles_follow_redirects_and_preserve_requested_value() {
        let results = resolve_normalized_batch(
            &["foo_bar".to_owned()],
            NormalizeQuery {
                normalized: vec![TitleMapping {
                    from: "foo_bar".to_owned(),
                    to: "Foo bar".to_owned(),
                }],
                redirects: vec![TitleMapping {
                    from: "Foo bar".to_owned(),
                    to: "Target".to_owned(),
                }],
                pages: vec![NormalizePage {
                    pageid: 42,
                    ns: 0,
                    title: "Target".to_owned(),
                    missing: false,
                    redirect: false,
                    invalid: false,
                    invalidreason: String::new(),
                }],
            },
        );
        assert_eq!(results[0].requested_title, "foo_bar");
        assert_eq!(results[0].title, "Target");
        assert_eq!(results[0].page_id, Some(42));
        assert!(results[0].redirect);
    }

    #[test]
    fn invalid_title_without_api_reason_is_still_invalid() {
        let results = resolve_normalized_batch(
            &["Bad".to_owned()],
            NormalizeQuery {
                normalized: vec![],
                redirects: vec![],
                pages: vec![NormalizePage {
                    pageid: 0,
                    ns: 0,
                    title: "Bad".to_owned(),
                    missing: false,
                    redirect: false,
                    invalid: true,
                    invalidreason: String::new(),
                }],
            },
        );
        assert_eq!(
            results[0].invalid_reason.as_deref(),
            Some("The page title is invalid.")
        );
    }
}
