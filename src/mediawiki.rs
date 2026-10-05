use std::{error::Error, fmt};

use mwbot::{
    Bot,
    generators::{
        EmbeddedIn, Generator, categories::CategoryMembers, link::LinksHere, search::Search,
    },
};
use serde::Deserialize;
use std::collections::HashMap;

use crate::AppResult;

#[derive(Clone, Copy, Debug)]
pub enum PageListKind {
    CategoryMembers,
    Backlinks,
    Transclusions,
    Search,
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
    const BATCH_SIZE: usize = 50;
    let client = authenticated_client(wiki_url, user_agent, access_token).await?;
    let mut output = Vec::with_capacity(titles.len());

    for batch in titles.chunks(BATCH_SIZE) {
        let response: NormalizeResponse = client
            .get([
                ("action", "query"),
                ("titles", &batch.join("|")),
                ("redirects", "1"),
                ("formatversion", "2"),
            ])
            .await?;
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
                    redirect: redirected,
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
