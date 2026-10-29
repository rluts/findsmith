use std::{error::Error, fmt};

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set, SqlErr, sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{entities::project, skip::SkipOptions};

pub const DEFAULT_QUEUE_LIMIT: usize = 100;
pub const MAX_QUEUE_LIMIT: usize = 500;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct ProjectState {
    pub sources: Vec<PageSource>,
    pub pages: Vec<String>,
    pub categories: Vec<String>,
    pub backlinks: Vec<String>,
    pub transclusions: Vec<String>,
    pub searches: Vec<String>,
    pub namespaces: Vec<i64>,
    pub queue_limit: usize,
    pub rules: Vec<ReplacementRule>,
    pub skip: SkipOptions,
}

impl Default for ProjectState {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            pages: Vec::new(),
            categories: Vec::new(),
            backlinks: Vec::new(),
            transclusions: Vec::new(),
            searches: Vec::new(),
            namespaces: Vec::new(),
            queue_limit: DEFAULT_QUEUE_LIMIT,
            rules: Vec::new(),
            skip: SkipOptions::default(),
        }
    }
}

impl ProjectState {
    pub fn ordered_sources(&self) -> Vec<PageSource> {
        if !self.sources.is_empty() {
            return self.sources.clone();
        }

        [
            (PageSourceKind::Manual, &self.pages),
            (PageSourceKind::Category, &self.categories),
            (PageSourceKind::Backlinks, &self.backlinks),
            (PageSourceKind::Transclusions, &self.transclusions),
            (PageSourceKind::Search, &self.searches),
        ]
        .into_iter()
        .filter(|(_, values)| !values.is_empty())
        .map(|(kind, values)| PageSource {
            kind,
            values: values.clone(),
        })
        .collect()
    }

    pub fn has_sources(&self) -> bool {
        !self.ordered_sources().is_empty()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PageSourceKind {
    Manual,
    Category,
    Backlinks,
    Transclusions,
    Search,
    Watchlist,
    Contributions,
    LinksOnPage,
    CategoriesOnPage,
    RedirectsToPage,
}

impl PageSourceKind {
    pub fn form_value(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Category => "category",
            Self::Backlinks => "backlinks",
            Self::Transclusions => "transclusions",
            Self::Search => "search",
            Self::Watchlist => "watchlist",
            Self::Contributions => "contributions",
            Self::LinksOnPage => "links_on_page",
            Self::CategoriesOnPage => "categories_on_page",
            Self::RedirectsToPage => "redirects_to_page",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PageSource {
    pub kind: PageSourceKind,
    pub values: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct ReplacementRule {
    pub name: String,
    pub enabled: bool,
    pub operation: RuleOperation,
    pub parameter: String,
    pub mask: RuleMask,
    pub find: String,
    pub replace: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub replace_all: bool,
    pub multiline: bool,
    pub dot_matches_new_line: bool,
}

impl Default for ReplacementRule {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: true,
            operation: RuleOperation::Replace,
            parameter: String::new(),
            mask: RuleMask::default(),
            find: String::new(),
            replace: String::new(),
            regex: false,
            case_sensitive: true,
            replace_all: true,
            multiline: false,
            dot_matches_new_line: false,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct RuleMask {
    pub comments: bool,
    pub tags: bool,
    pub references: bool,
    pub links: bool,
    pub headings: bool,
    pub templates: bool,
    pub link_targets: bool,
}

impl RuleMask {
    pub fn any(&self) -> bool {
        self.comments
            || self.tags
            || self.references
            || self.links
            || self.headings
            || self.templates
            || self.link_targets
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleOperation {
    #[default]
    Replace,
    Append,
    Prepend,
    CategoryAdd,
    CategoryRemove,
    CategoryReplace,
    TemplateRename,
    TemplateParameterSet,
    TemplateParameterRemove,
}

#[derive(Debug)]
pub enum ProjectError {
    InvalidName,
    DuplicateName,
    Conflict,
    Database(DbErr),
    Serialization(serde_json::Error),
}

impl fmt::Display for ProjectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName => {
                formatter.write_str("Project name must contain between 1 and 255 characters.")
            }
            Self::DuplicateName => formatter.write_str("A project with this name already exists."),
            Self::Conflict => formatter.write_str(
                "This project was changed in another tab or request. Reload it and try again.",
            ),
            Self::Database(error) => error.fmt(formatter),
            Self::Serialization(error) => error.fmt(formatter),
        }
    }
}

impl Error for ProjectError {}

impl From<DbErr> for ProjectError {
    fn from(error: DbErr) -> Self {
        if matches!(error.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) {
            Self::DuplicateName
        } else {
            Self::Database(error)
        }
    }
}

impl From<serde_json::Error> for ProjectError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}

pub struct ProjectRepository<'a> {
    database: &'a DatabaseConnection,
}

impl<'a> ProjectRepository<'a> {
    pub fn new(database: &'a DatabaseConnection) -> Self {
        Self { database }
    }

    pub async fn list(&self, owner: &str) -> Result<Vec<project::Model>, ProjectError> {
        Ok(project::Entity::find()
            .filter(project::Column::Owner.eq(owner))
            .order_by_desc(project::Column::UpdatedAt)
            .all(self.database)
            .await?)
    }

    pub async fn get(&self, owner: &str, id: &str) -> Result<Option<project::Model>, ProjectError> {
        Ok(project::Entity::find_by_id(id)
            .filter(project::Column::Owner.eq(owner))
            .one(self.database)
            .await?)
    }

    pub async fn create(
        &self,
        owner: &str,
        name: String,
        wiki_url: String,
    ) -> Result<project::Model, ProjectError> {
        let now = Utc::now();
        let active = project::ActiveModel {
            id: Set(Uuid::new_v4().to_string()),
            owner: Set(owner.to_owned()),
            name: Set(validate_name(name)?),
            wiki_url: Set(wiki_url),
            state_json: Set(serde_json::to_value(ProjectState::default())?),
            state_version: Set(1),
            created_at: Set(now),
            updated_at: Set(now),
        };
        Ok(active.insert(self.database).await?)
    }

    /// Copies a project's configuration under the first free "(copy)" name. Review
    /// jobs and their history stay with the original project.
    pub async fn duplicate(
        &self,
        owner: &str,
        id: &str,
    ) -> Result<Option<project::Model>, ProjectError> {
        let Some(source) = self.get(owner, id).await? else {
            return Ok(None);
        };
        for attempt in 1..=MAX_COPY_ATTEMPTS {
            let now = Utc::now();
            let active = project::ActiveModel {
                id: Set(Uuid::new_v4().to_string()),
                owner: Set(owner.to_owned()),
                name: Set(copy_name(&source.name, attempt)),
                wiki_url: Set(source.wiki_url.clone()),
                state_json: Set(source.state_json.clone()),
                state_version: Set(1),
                created_at: Set(now),
                updated_at: Set(now),
            };
            match active
                .insert(self.database)
                .await
                .map_err(ProjectError::from)
            {
                Ok(project) => return Ok(Some(project)),
                Err(ProjectError::DuplicateName) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(ProjectError::DuplicateName)
    }

    pub async fn update(
        &self,
        owner: &str,
        id: &str,
        name: String,
        wiki_url: String,
        state: ProjectState,
        expected_version: i64,
    ) -> Result<Option<project::Model>, ProjectError> {
        let result = project::Entity::update_many()
            .col_expr(project::Column::Name, Expr::value(validate_name(name)?))
            .col_expr(project::Column::WikiUrl, Expr::value(wiki_url))
            .col_expr(
                project::Column::StateJson,
                Expr::value(serde_json::to_value(state)?),
            )
            .col_expr(
                project::Column::StateVersion,
                Expr::value(expected_version + 1),
            )
            .col_expr(project::Column::UpdatedAt, Expr::value(Utc::now()))
            .filter(project::Column::Id.eq(id))
            .filter(project::Column::Owner.eq(owner))
            .filter(project::Column::StateVersion.eq(expected_version))
            .exec(self.database)
            .await?;
        if result.rows_affected == 1 {
            return self.get(owner, id).await;
        }
        if self.get(owner, id).await?.is_some() {
            Err(ProjectError::Conflict)
        } else {
            Ok(None)
        }
    }

    pub async fn delete(&self, owner: &str, id: &str) -> Result<bool, ProjectError> {
        let result = project::Entity::delete_many()
            .filter(project::Column::Id.eq(id))
            .filter(project::Column::Owner.eq(owner))
            .exec(self.database)
            .await?;
        Ok(result.rows_affected == 1)
    }
}

const MAX_COPY_ATTEMPTS: usize = 20;
const MAX_NAME_CHARS: usize = 255;

fn copy_name(name: &str, attempt: usize) -> String {
    let suffix = if attempt == 1 {
        crate::localization::required("findsmith-api-project-copy-suffix")
    } else {
        crate::localization::required_with_args(
            "findsmith-api-project-copy-number-suffix",
            &[&attempt.to_string()],
        )
    };
    let room = MAX_NAME_CHARS.saturating_sub(suffix.chars().count() + 1);
    let base: String = name.chars().take(room).collect();
    format!("{} {suffix}", base.trim_end())
}

fn validate_name(name: String) -> Result<String, ProjectError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return Err(ProjectError::InvalidName);
    }
    Ok(name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_names_stay_within_the_name_limit() {
        assert_eq!(copy_name("Fixes", 1), "Fixes (copy)");
        assert_eq!(copy_name("Fixes", 3), "Fixes (copy 3)");
        let long = "x".repeat(MAX_NAME_CHARS);
        let copy = copy_name(&long, 2);
        assert_eq!(copy.chars().count(), MAX_NAME_CHARS);
        assert!(copy.ends_with(" (copy 2)"));
    }

    #[test]
    fn legacy_page_lists_become_ordered_sources() {
        let state: ProjectState = serde_json::from_value(serde_json::json!({
            "pages": ["One", "Two"],
            "categories": ["Category:Examples"],
            "backlinks": [],
            "transclusions": [],
            "searches": []
        }))
        .unwrap();

        let sources = state.ordered_sources();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].kind, PageSourceKind::Manual);
        assert_eq!(sources[0].values, ["One", "Two"]);
        assert_eq!(sources[1].kind, PageSourceKind::Category);
    }

    #[test]
    fn explicit_source_order_takes_precedence() {
        let state = ProjectState {
            sources: vec![
                PageSource {
                    kind: PageSourceKind::Search,
                    values: vec!["first".to_owned()],
                },
                PageSource {
                    kind: PageSourceKind::Manual,
                    values: vec!["second".to_owned()],
                },
            ],
            pages: vec!["legacy".to_owned()],
            ..ProjectState::default()
        };

        let sources = state.ordered_sources();
        assert_eq!(sources[0].kind, PageSourceKind::Search);
        assert_eq!(sources[1].kind, PageSourceKind::Manual);
    }
}
