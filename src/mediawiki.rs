use std::{error::Error, fmt};

use serde::Deserialize;

use crate::AppResult;

pub const UKWIKI_ID: &str = "ukwiki";
pub const UKWIKI_API_URL: &str = "https://uk.wikipedia.org/w/api.php";

pub fn wiki_api_url(wiki_id: &str) -> Option<&'static str> {
    match wiki_id {
        UKWIKI_ID => Some(UKWIKI_API_URL),
        _ => None,
    }
}

pub fn wiki_id(api_url: &str) -> Option<&'static str> {
    match api_url {
        UKWIKI_API_URL => Some(UKWIKI_ID),
        _ => None,
    }
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
struct PageResponse {
    curtimestamp: String,
    query: PageQuery,
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
