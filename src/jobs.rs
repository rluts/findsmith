use std::{error::Error, fmt};

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set, sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{entities::job, projects::ReplacementRule};

pub const JOB_CONFIG_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobConfigV1 {
    pub version: u32,
    pub rules: Vec<ReplacementRule>,
}

impl JobConfigV1 {
    pub fn new(rules: Vec<ReplacementRule>) -> Self {
        Self {
            version: JOB_CONFIG_VERSION,
            rules,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Active,
    Stopped,
    Complete,
}

impl JobStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Stopped => "Stopped",
            Self::Complete => "Complete",
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueueEntryStatus {
    Pending,
    Reviewed,
    Skipped,
    Missing,
    Invalid,
}

impl QueueEntryStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Reviewed => "Reviewed",
            Self::Skipped => "Skipped",
            Self::Missing => "Missing",
            Self::Invalid => "Invalid",
        }
    }

    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Pending)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct QueueEntry {
    pub requested_title: String,
    pub title: String,
    pub page_id: Option<u64>,
    pub namespace: Option<i64>,
    pub redirect: bool,
    pub status: QueueEntryStatus,
    pub reason: Option<String>,
    pub revision_id: Option<u64>,
    pub revision_timestamp: Option<String>,
    pub replacement_count: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobState {
    pub status: JobStatus,
    pub position: usize,
    pub queue: Vec<QueueEntry>,
}

impl JobState {
    pub fn new(queue: Vec<QueueEntry>) -> Self {
        Self {
            status: if queue.is_empty() {
                JobStatus::Complete
            } else {
                JobStatus::Active
            },
            position: 0,
            queue,
        }
    }

    pub fn current(&self) -> Option<&QueueEntry> {
        self.queue.get(self.position)
    }

    pub fn counters(&self) -> JobCounters {
        let mut counters = JobCounters::default();
        for entry in &self.queue {
            match entry.status {
                QueueEntryStatus::Pending => counters.pending += 1,
                QueueEntryStatus::Reviewed => counters.reviewed += 1,
                QueueEntryStatus::Skipped => counters.skipped += 1,
                QueueEntryStatus::Missing | QueueEntryStatus::Invalid => counters.unavailable += 1,
            }
        }
        counters
    }

    pub fn previous(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        if self.status == JobStatus::Active {
            self.position = self.position.saturating_sub(1);
        }
        self.status = JobStatus::Active;
    }

    pub fn advance(&mut self, status: QueueEntryStatus, reason: Option<String>) {
        if let Some(entry) = self.queue.get_mut(self.position)
            && entry.status.is_pending()
        {
            entry.status = status;
            entry.reason = reason;
        }
        if self.position + 1 < self.queue.len() {
            self.position += 1;
        } else {
            self.status = JobStatus::Complete;
        }
    }
}

#[derive(Default)]
pub struct JobCounters {
    pub pending: usize,
    pub reviewed: usize,
    pub skipped: usize,
    pub unavailable: usize,
}

#[derive(Debug)]
pub enum JobError {
    Conflict,
    Database(DbErr),
    Serialization(serde_json::Error),
}

impl fmt::Display for JobError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict => formatter
                .write_str("This review changed in another request. Reload it and try again."),
            Self::Database(error) => error.fmt(formatter),
            Self::Serialization(error) => error.fmt(formatter),
        }
    }
}

impl Error for JobError {}

impl From<DbErr> for JobError {
    fn from(error: DbErr) -> Self {
        Self::Database(error)
    }
}

impl From<serde_json::Error> for JobError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}

pub struct JobRepository<'a> {
    database: &'a DatabaseConnection,
}

impl<'a> JobRepository<'a> {
    pub fn new(database: &'a DatabaseConnection) -> Self {
        Self { database }
    }

    pub async fn create(
        &self,
        owner: &str,
        project_id: &str,
        wiki_url: &str,
        config: JobConfigV1,
        state: JobState,
    ) -> Result<job::Model, JobError> {
        let now = Utc::now();
        Ok(job::ActiveModel {
            id: Set(Uuid::new_v4().to_string()),
            project_id: Set(project_id.to_owned()),
            owner: Set(owner.to_owned()),
            wiki_url: Set(wiki_url.to_owned()),
            config_json: Set(serde_json::to_value(config)?),
            state_json: Set(serde_json::to_value(state)?),
            state_version: Set(1),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(self.database)
        .await?)
    }

    pub async fn get(
        &self,
        owner: &str,
        project_id: &str,
        id: &str,
    ) -> Result<Option<job::Model>, JobError> {
        Ok(job::Entity::find_by_id(id)
            .filter(job::Column::Owner.eq(owner))
            .filter(job::Column::ProjectId.eq(project_id))
            .one(self.database)
            .await?)
    }

    pub async fn latest(
        &self,
        owner: &str,
        project_id: &str,
    ) -> Result<Option<job::Model>, JobError> {
        Ok(job::Entity::find()
            .filter(job::Column::Owner.eq(owner))
            .filter(job::Column::ProjectId.eq(project_id))
            .order_by_desc(job::Column::UpdatedAt)
            .one(self.database)
            .await?)
    }

    pub async fn update_state(
        &self,
        owner: &str,
        project_id: &str,
        id: &str,
        state: &JobState,
        expected_version: i64,
    ) -> Result<(), JobError> {
        let result = job::Entity::update_many()
            .col_expr(
                job::Column::StateJson,
                Expr::value(serde_json::to_value(state)?),
            )
            .col_expr(job::Column::StateVersion, Expr::value(expected_version + 1))
            .col_expr(job::Column::UpdatedAt, Expr::value(Utc::now()))
            .filter(job::Column::Id.eq(id))
            .filter(job::Column::Owner.eq(owner))
            .filter(job::Column::ProjectId.eq(project_id))
            .filter(job::Column::StateVersion.eq(expected_version))
            .exec(self.database)
            .await?;
        if result.rows_affected == 1 {
            Ok(())
        } else {
            Err(JobError::Conflict)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str) -> QueueEntry {
        QueueEntry {
            requested_title: title.to_owned(),
            title: title.to_owned(),
            page_id: Some(1),
            namespace: Some(0),
            redirect: false,
            status: QueueEntryStatus::Pending,
            reason: None,
            revision_id: None,
            revision_timestamp: None,
            replacement_count: None,
        }
    }

    #[test]
    fn advancing_records_result_and_completes_at_end() {
        let mut state = JobState::new(vec![entry("A"), entry("B")]);
        state.advance(QueueEntryStatus::Reviewed, None);
        assert_eq!(state.position, 1);
        assert_eq!(state.queue[0].status, QueueEntryStatus::Reviewed);
        state.advance(QueueEntryStatus::Skipped, Some("User skipped".to_owned()));
        assert_eq!(state.status, JobStatus::Complete);
        assert_eq!(state.counters().skipped, 1);
    }

    #[test]
    fn previous_never_underflows() {
        let mut state = JobState::new(vec![entry("A")]);
        state.previous();
        assert_eq!(state.position, 0);
    }
}
