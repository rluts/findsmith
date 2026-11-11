//! Loads and caches each wiki's AutoWikiBrowser typo list, and applies it to pages.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

use sea_orm::DatabaseConnection;
use serde::Serialize;

use crate::{
    mediawiki::{PageLoadError, load_page},
    typos::{
        TypoFixResult, TypoList, TypoOptions, TypoProblem, TypoSite, TypoStat, parse_typo_list,
    },
    wikis,
};

/// AWB `Variables.RetfPath`.
pub const TYPO_LIST_TITLE: &str = "Project:AutoWikiBrowser/Typos";
const CACHE_TTL: Duration = Duration::from_secs(60 * 60);
const MAX_REPORTED_PROBLEMS: usize = 200;
const MAX_LIST_BYTES: usize = 4 * 1024 * 1024;

pub struct LoadedTypoList {
    pub list: TypoList,
    pub title: String,
    pub full_url: String,
    pub revision_id: u64,
    loaded_at: Instant,
}

static CACHE: LazyLock<Mutex<HashMap<String, Arc<LoadedTypoList>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Returns the wiki's compiled typo list, loading it at most once an hour.
pub async fn typo_list(
    database: &DatabaseConnection,
    wiki_url: &str,
    user_agent: &str,
    access_token: &str,
) -> Result<Arc<LoadedTypoList>, String> {
    if let Some(cached) = CACHE
        .lock()
        .expect("typo cache lock")
        .get(wiki_url)
        .filter(|cached| cached.loaded_at.elapsed() < CACHE_TTL)
    {
        return Ok(cached.clone());
    }
    let snapshot = load_page(wiki_url, user_agent, access_token, TYPO_LIST_TITLE)
        .await
        .map_err(|error| match error {
            PageLoadError::Missing => crate::localization::required_with_args(
                "findsmith-api-typos-list-missing",
                &[TYPO_LIST_TITLE],
            ),
            other => crate::localization::required_with_args(
                "findsmith-api-typos-list-load-failed",
                &[&other.to_string()],
            ),
        })?;
    if snapshot.content.len() > MAX_LIST_BYTES {
        return Err(crate::localization::required(
            "findsmith-api-typos-list-too-large",
        ));
    }
    let site = typo_site(database, wiki_url).await;
    let content = snapshot.content;
    let list =
        tokio::task::spawn_blocking(move || TypoList::compile(&parse_typo_list(&content), site))
            .await
            .map_err(|error| error.to_string())?;
    let loaded = Arc::new(LoadedTypoList {
        list,
        title: snapshot.title,
        full_url: snapshot.full_url,
        revision_id: snapshot.revision_id,
        loaded_at: Instant::now(),
    });
    CACHE
        .lock()
        .expect("typo cache lock")
        .insert(wiki_url.to_owned(), loaded.clone());
    Ok(loaded)
}

async fn typo_site(database: &DatabaseConnection, wiki_url: &str) -> TypoSite {
    let mut site = TypoSite::default();
    if let Ok(Some(wiki)) = wikis::by_url(database, wiki_url).await {
        if let Ok(namespaces) = wikis::namespaces(&wiki) {
            for namespace in namespaces {
                match namespace.id {
                    6 => site.file_names.push(namespace.name),
                    14 => site.category_names.push(namespace.name),
                    _ => {}
                }
            }
        }
        let language = wiki.domain.split('.').next().unwrap_or("en").to_owned();
        site.right_to_left = matches!(
            language.as_str(),
            "ar" | "arc"
                | "arz"
                | "azb"
                | "ckb"
                | "dv"
                | "fa"
                | "glk"
                | "he"
                | "ks"
                | "lrc"
                | "mzn"
                | "pnb"
                | "ps"
                | "sd"
                | "ug"
                | "ur"
                | "yi"
        );
        site.language = language;
    }
    site
}

#[derive(Clone, Debug, Serialize)]
pub struct TypoProblemView {
    pub word: Option<String>,
    pub find: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct TypoStatView {
    pub word: Option<String>,
    pub find: String,
    pub replace: String,
    pub total: usize,
    pub self_matches: usize,
}

/// What typo fixing did to one page, for the Typos tab.
#[derive(Clone, Debug, Default, Serialize)]
pub struct TypoPageView {
    pub list_title: String,
    pub list_url: String,
    pub list_revision: u64,
    pub rule_count: usize,
    pub problem_count: usize,
    pub problems: Vec<TypoProblemView>,
    /// For example `acheive → achieve, teh → the (2)`; empty when nothing changed.
    pub summary: String,
    pub fixed: bool,
    pub stats: Vec<TypoStatView>,
    /// Rules skipped on this page, normally for exceeding the backtracking budget.
    pub rule_errors: Vec<String>,
    /// Set when the list could not be loaded or applied; the page is then left as the rules made it.
    pub error: Option<String>,
}

impl TypoPageView {
    fn from_list(loaded: &LoadedTypoList) -> Self {
        Self {
            list_title: loaded.title.clone(),
            list_url: loaded.full_url.clone(),
            list_revision: loaded.revision_id,
            rule_count: loaded.list.len(),
            problem_count: loaded.list.problems().len(),
            problems: loaded
                .list
                .problems()
                .iter()
                .take(MAX_REPORTED_PROBLEMS)
                .map(problem_view)
                .collect(),
            ..Self::default()
        }
    }
}

fn problem_view(problem: &TypoProblem) -> TypoProblemView {
    TypoProblemView {
        word: problem.word.clone(),
        find: problem.find.clone(),
        message: problem.message.clone(),
    }
}

fn stat_view(stat: TypoStat) -> TypoStatView {
    TypoStatView {
        word: stat.word,
        find: stat.find,
        replace: stat.replace,
        total: stat.total,
        self_matches: stat.self_matches,
    }
}

/// Runs the wiki's typo list over `text` when the options enable it. Returns the
/// new text and the view; on failure the text is returned unchanged with the
/// error in the view.
#[allow(clippy::too_many_arguments)]
pub async fn apply_typos(
    database: &DatabaseConnection,
    wiki_url: &str,
    user_agent: &str,
    access_token: &str,
    options: &TypoOptions,
    text: String,
    title: &str,
) -> (String, Option<TypoPageView>) {
    if !options.enabled {
        return (text, None);
    }
    let loaded = match typo_list(database, wiki_url, user_agent, access_token).await {
        Ok(loaded) => loaded,
        Err(error) => {
            return (
                text,
                Some(TypoPageView {
                    error: Some(error),
                    ..TypoPageView::default()
                }),
            );
        }
    };
    let mut view = TypoPageView::from_list(&loaded);
    let title = title.to_owned();
    let input = text.clone();
    let worker = loaded.clone();
    let outcome = tokio::task::spawn_blocking(move || worker.list.fix(&input, &title)).await;
    match outcome {
        Ok(Ok(TypoFixResult {
            text: fixed,
            no_change,
            summary,
            stats,
            errors,
        })) => {
            view.fixed = !no_change;
            view.summary = summary;
            view.stats = stats.into_iter().map(stat_view).collect();
            view.rule_errors = errors;
            (fixed, Some(view))
        }
        Ok(Err(error)) => {
            view.error = Some(crate::localization::required_with_args(
                "findsmith-api-typos-apply-failed",
                &[&error.to_string()],
            ));
            (text, Some(view))
        }
        Err(error) => {
            view.error = Some(error.to_string());
            (text, Some(view))
        }
    }
}
