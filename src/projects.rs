use std::{error::Error, fmt};

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set, SqlErr, sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entities::project;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ProjectState {
    pub pages: Vec<String>,
    pub rules: Vec<ReplacementRule>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct ReplacementRule {
    pub enabled: bool,
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
            enabled: true,
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

fn validate_name(name: String) -> Result<String, ProjectError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 255 {
        return Err(ProjectError::InvalidName);
    }
    Ok(name.to_owned())
}
