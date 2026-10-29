use std::{error::Error, fmt};

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set, sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{entities::job, projects::ReplacementRule, skip::SkipOptions};

pub const JOB_CONFIG_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobConfigV1 {
    pub version: u32,
    pub rules: Vec<ReplacementRule>,
    #[serde(default)]
    pub skip: SkipOptions,
    #[serde(default)]
    pub namespaces: Vec<i64>,
}

impl JobConfigV1 {
    pub fn new(rules: Vec<ReplacementRule>, skip: SkipOptions, namespaces: Vec<i64>) -> Self {
        Self {
            version: JOB_CONFIG_VERSION,
            rules,
            skip,
            namespaces,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Building,
    Active,
    Stopped,
    Cancelled,
    Failed,
    Complete,
}

impl JobStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Building => "findsmith-api-building-queue",
            Self::Active => "findsmith-api-active",
            Self::Stopped => "findsmith-api-stopped",
            Self::Cancelled => "findsmith-api-cancelled",
            Self::Failed => "findsmith-api-failed",
            Self::Complete => "findsmith-api-complete",
        }
    }

    pub fn is_building(&self) -> bool {
        matches!(self, Self::Building)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueueEntryStatus {
    Pending,
    Reviewed,
    Saved,
    Skipped,
    Missing,
    Invalid,
}

impl QueueEntryStatus {
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
    #[serde(default)]
    pub edit_revision_id: Option<u64>,
    #[serde(default)]
    pub edit_result: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobState {
    pub status: JobStatus,
    pub position: usize,
    pub queue: Vec<QueueEntry>,
    #[serde(default)]
    pub generation_message: String,
    #[serde(default = "default_queue_limit")]
    pub queue_limit: usize,
    #[serde(default)]
    pub generation_found: usize,
    #[serde(default)]
    pub generation_duplicates: usize,
    #[serde(default)]
    pub generation_limit_reached: bool,
    #[serde(default)]
    pub preparse_active: bool,
    #[serde(default)]
    pub preparse_cursor: usize,
    #[serde(default)]
    pub preparse_render_equivalent: bool,
    #[serde(default)]
    pub edit_in_flight: bool,
    #[serde(default)]
    pub policy_acknowledged: bool,
    #[serde(default)]
    pub edit_policy_blocked: bool,
    #[serde(default)]
    pub consecutive_edit_failures: u32,
}

impl JobState {
    pub fn repeat_with_new_rules(previous: &Self) -> Self {
        let mut state = Self::building(previous.queue_limit);
        state.queue = previous
            .queue
            .iter()
            .cloned()
            .map(|mut entry| {
                entry.status = QueueEntryStatus::Pending;
                entry.reason = None;
                entry.revision_id = None;
                entry.revision_timestamp = None;
                entry.replacement_count = None;
                entry.edit_revision_id = None;
                entry.edit_result = None;
                entry
            })
            .collect();
        state.finish_build();
        state.generation_message =
            "findsmith-api-rules-changed-review-every-page-again-against-its-latest-revision"
                .to_owned();
        state
    }

    #[cfg(test)]
    pub fn new(queue: Vec<QueueEntry>) -> Self {
        Self {
            status: if queue.is_empty() {
                JobStatus::Complete
            } else {
                JobStatus::Active
            },
            position: 0,
            queue,
            generation_message: String::new(),
            queue_limit: crate::projects::MAX_QUEUE_LIMIT,
            generation_found: 0,
            generation_duplicates: 0,
            generation_limit_reached: false,
            preparse_active: false,
            preparse_cursor: 0,
            preparse_render_equivalent: false,
            edit_in_flight: false,
            policy_acknowledged: false,
            edit_policy_blocked: false,
            consecutive_edit_failures: 0,
        }
    }

    pub fn building(queue_limit: usize) -> Self {
        Self {
            status: JobStatus::Building,
            position: 0,
            queue: Vec::new(),
            generation_message: "findsmith-api-generation-preparing".to_owned(),
            queue_limit,
            generation_found: 0,
            generation_duplicates: 0,
            generation_limit_reached: false,
            preparse_active: false,
            preparse_cursor: 0,
            preparse_render_equivalent: false,
            edit_in_flight: false,
            policy_acknowledged: false,
            edit_policy_blocked: false,
            consecutive_edit_failures: 0,
        }
    }

    pub fn append_unique(&mut self, entries: Vec<QueueEntry>) {
        let track_generation = self.status == JobStatus::Building;
        if track_generation {
            self.generation_found += entries.len();
        }
        let mut seen: std::collections::HashSet<String> =
            self.queue.iter().map(|entry| entry.title.clone()).collect();
        for entry in entries {
            if !seen.insert(entry.title.clone()) {
                if track_generation {
                    self.generation_duplicates += 1;
                }
                continue;
            }
            if self.queue.len() >= self.queue_limit {
                if track_generation {
                    self.generation_limit_reached = true;
                }
                break;
            }
            self.queue.push(entry);
        }
        if track_generation && self.queue.len() >= self.queue_limit {
            self.generation_limit_reached = true;
        }
    }

    pub fn finish_build(&mut self) {
        self.generation_message = crate::localization::required_with_args(
            "findsmith-api-generation-ready-count",
            &[&self.queue.len().to_string()],
        );
        self.status = if self.queue.is_empty() {
            JobStatus::Complete
        } else {
            JobStatus::Active
        };
    }

    pub fn move_entry(&mut self, index: usize, direction: isize) {
        let target = index as isize + direction;
        if index >= self.queue.len() || target < 0 || target as usize >= self.queue.len() {
            return;
        }
        let target = target as usize;
        self.queue.swap(index, target);
        if self.position == index {
            self.position = target;
        } else if self.position == target {
            self.position = index;
        }
    }

    pub fn remove_entry(&mut self, index: usize) {
        if index >= self.queue.len() {
            return;
        }
        self.queue.remove(index);
        if self.position > index {
            self.position -= 1;
        }
        if self.queue.is_empty() {
            self.position = 0;
            self.status = JobStatus::Complete;
        } else if self.position >= self.queue.len() {
            self.position = self.queue.len() - 1;
        }
    }

    pub fn clear(&mut self) {
        self.queue.clear();
        self.position = 0;
        self.status = JobStatus::Complete;
        self.generation_message = "findsmith-api-queue-cleared".to_owned();
    }

    pub fn sort_by_title(&mut self) {
        let selected = self.position;
        let mut entries: Vec<(usize, QueueEntry)> = std::mem::take(&mut self.queue)
            .into_iter()
            .enumerate()
            .collect();
        entries.sort_by_cached_key(|(_, entry)| entry.title.to_lowercase());
        self.position = entries
            .iter()
            .position(|(original_index, _)| *original_index == selected)
            .unwrap_or(0);
        self.queue = entries.into_iter().map(|(_, entry)| entry).collect();
    }

    pub fn select(&mut self, index: usize) -> bool {
        if index >= self.queue.len() || self.status == JobStatus::Building || self.preparse_active {
            return false;
        }
        self.position = index;
        true
    }

    pub fn finish_preparse_if_done(&mut self) {
        if self.preparse_cursor < self.queue.len() {
            return;
        }
        self.preparse_active = false;
        if let Some(index) = self
            .queue
            .iter()
            .position(|entry| entry.status.is_pending())
        {
            self.position = index;
            self.status = JobStatus::Active;
        } else {
            self.status = JobStatus::Complete;
        }
    }

    pub fn current(&self) -> Option<&QueueEntry> {
        self.queue.get(self.position)
    }

    pub fn can_save_current(&self) -> bool {
        matches!(self.status, JobStatus::Active | JobStatus::Complete)
            && !self.preparse_active
            && self.current().is_some_and(|entry| {
                matches!(
                    entry.status,
                    QueueEntryStatus::Pending | QueueEntryStatus::Reviewed
                )
            })
    }

    pub fn counters(&self) -> JobCounters {
        let mut counters = JobCounters::default();
        for entry in &self.queue {
            match entry.status {
                QueueEntryStatus::Pending => counters.pending += 1,
                QueueEntryStatus::Reviewed => counters.reviewed += 1,
                QueueEntryStatus::Saved => {
                    counters.reviewed += 1;
                    counters.saved += 1;
                }
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

    pub fn resume(&mut self) -> bool {
        if self.status != JobStatus::Stopped || self.queue.is_empty() {
            return false;
        }
        self.status = JobStatus::Active;
        true
    }

    pub fn advance(&mut self, status: QueueEntryStatus, reason: Option<String>) {
        let after_save = status == QueueEntryStatus::Saved;
        if let Some(entry) = self.queue.get_mut(self.position)
            && (entry.status.is_pending()
                || (after_save && entry.status == QueueEntryStatus::Reviewed))
        {
            entry.status = status;
            entry.reason = reason;
        }
        let next = (self.position + 1..self.queue.len())
            .chain(0..self.position)
            .find(|&index| {
                let entry = &self.queue[index];
                entry.status.is_pending()
                    || (after_save && entry.status == QueueEntryStatus::Reviewed)
            });
        if let Some(index) = next {
            self.position = index;
            if after_save && !self.queue.iter().any(|entry| entry.status.is_pending()) {
                self.status = JobStatus::Complete;
            }
        } else {
            self.status = JobStatus::Complete;
        }
    }
}

fn default_queue_limit() -> usize {
    crate::projects::MAX_QUEUE_LIMIT
}

#[derive(Default)]
pub struct JobCounters {
    pub pending: usize,
    pub reviewed: usize,
    pub saved: usize,
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

    pub async fn update_config(
        &self,
        owner: &str,
        project_id: &str,
        id: &str,
        config: &JobConfigV1,
    ) -> Result<(), JobError> {
        let result = job::Entity::update_many()
            .col_expr(
                job::Column::ConfigJson,
                Expr::value(serde_json::to_value(config)?),
            )
            .col_expr(job::Column::UpdatedAt, Expr::value(Utc::now()))
            .filter(job::Column::Id.eq(id))
            .filter(job::Column::Owner.eq(owner))
            .filter(job::Column::ProjectId.eq(project_id))
            .exec(self.database)
            .await?;
        if result.rows_affected == 1 {
            Ok(())
        } else {
            Err(JobError::Conflict)
        }
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
            edit_revision_id: None,
            edit_result: None,
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
    fn resume_keeps_position_and_review_decisions() {
        let mut state = JobState::new(vec![entry("A"), entry("B")]);
        state.advance(QueueEntryStatus::Reviewed, None);
        state.status = JobStatus::Stopped;
        assert!(state.resume());
        assert_eq!(state.status, JobStatus::Active);
        assert_eq!(state.position, 1);
        assert_eq!(state.queue[0].status, QueueEntryStatus::Reviewed);
        assert!(!state.resume());
    }

    #[test]
    fn changed_rules_reopen_saved_pages_in_a_new_job_state() {
        let mut previous = JobState::new(vec![entry("A"), entry("B")]);
        previous.queue[0].status = QueueEntryStatus::Saved;
        previous.queue[0].edit_revision_id = Some(42);
        previous.queue[0].edit_result = Some("saved".to_owned());
        previous.queue[1].status = QueueEntryStatus::Reviewed;
        previous.status = JobStatus::Complete;

        let next = JobState::repeat_with_new_rules(&previous);
        assert_eq!(next.status, JobStatus::Active);
        assert_eq!(next.position, 0);
        assert!(next.queue.iter().all(|entry| entry.status.is_pending()));
        assert!(next.queue[0].edit_revision_id.is_none());
        assert_eq!(previous.queue[0].edit_revision_id, Some(42));
        assert_eq!(previous.queue[0].status, QueueEntryStatus::Saved);
        assert!(!next.policy_acknowledged);
    }

    #[test]
    fn selecting_processed_page_does_not_change_its_outcome() {
        let mut state = JobState::new(vec![entry("A"), entry("B")]);
        state.queue[0].status = QueueEntryStatus::Saved;
        state.status = JobStatus::Complete;
        state.position = 1;
        assert!(state.select(0));
        assert_eq!(state.position, 0);
        assert_eq!(state.status, JobStatus::Complete);
        assert_eq!(state.queue[0].status, QueueEntryStatus::Saved);
        assert!(!state.select(2));
    }

    #[test]
    fn reviewed_page_can_be_saved_after_review_completes() {
        let mut state = JobState::new(vec![entry("A"), entry("B")]);
        state.advance(QueueEntryStatus::Reviewed, None);
        state.advance(QueueEntryStatus::Reviewed, None);
        assert_eq!(state.status, JobStatus::Complete);
        assert!(state.select(0));
        assert!(state.can_save_current());
        state.advance(QueueEntryStatus::Saved, None);
        assert_eq!(state.status, JobStatus::Complete);
        assert_eq!(state.queue[0].status, QueueEntryStatus::Saved);
        assert_eq!(state.counters().saved, 1);
        assert_eq!(state.position, 1);
        assert!(state.can_save_current());
        state.advance(QueueEntryStatus::Saved, None);
        assert_eq!(state.queue[1].status, QueueEntryStatus::Saved);
        assert!(!state.can_save_current());
        state.queue[1].status = QueueEntryStatus::Reviewed;
        state.status = JobStatus::Stopped;
        assert!(!state.can_save_current());
        state.status = JobStatus::Complete;
        state.queue[1].status = QueueEntryStatus::Skipped;
        assert!(!state.can_save_current());
    }

    #[test]
    fn saving_moves_to_next_reviewed_or_pending_page_and_wraps() {
        let mut state = JobState::new(vec![
            entry("Reviewed first"),
            entry("Skipped"),
            entry("Pending"),
            entry("Reviewed last"),
        ]);
        state.queue[0].status = QueueEntryStatus::Reviewed;
        state.queue[1].status = QueueEntryStatus::Skipped;
        state.queue[3].status = QueueEntryStatus::Reviewed;
        state.advance(QueueEntryStatus::Saved, None);
        assert_eq!(state.position, 2);
        assert_eq!(state.status, JobStatus::Active);
        state.advance(QueueEntryStatus::Saved, None);
        assert_eq!(state.position, 3);
        assert_eq!(state.status, JobStatus::Complete);
        assert!(state.can_save_current());

        state.queue[0].status = QueueEntryStatus::Reviewed;
        state.advance(QueueEntryStatus::Saved, None);
        assert_eq!(state.position, 0);
        assert_eq!(state.status, JobStatus::Complete);
        assert_eq!(state.queue[3].status, QueueEntryStatus::Saved);
    }

    #[test]
    fn existing_job_state_without_edit_fields_still_loads() {
        let mut value = serde_json::to_value(JobState::new(vec![entry("A")])).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("edit_in_flight");
        object.remove("edit_policy_blocked");
        object.remove("policy_acknowledged");
        object.get_mut("queue").unwrap().as_array_mut().unwrap()[0]
            .as_object_mut()
            .unwrap()
            .remove("edit_result");
        let restored: JobState = serde_json::from_value(value).unwrap();
        assert!(!restored.edit_in_flight);
        assert!(!restored.edit_policy_blocked);
        assert!(!restored.policy_acknowledged);
        assert!(restored.queue[0].edit_result.is_none());
    }

    #[test]
    fn generated_entries_are_bounded_and_deduplicated() {
        let mut state = JobState::building(2);
        state.append_unique(vec![entry("A"), entry("A"), entry("B"), entry("C")]);
        assert_eq!(state.queue.len(), 2);
        assert_eq!(state.queue[0].title, "A");
        assert_eq!(state.queue[1].title, "B");
        assert_eq!(state.generation_found, 4);
        assert_eq!(state.generation_duplicates, 1);
        assert!(state.generation_limit_reached);
    }

    #[test]
    fn queue_edits_keep_current_position_valid() {
        let mut state = JobState::new(vec![entry("A"), entry("B"), entry("C")]);
        state.position = 1;
        state.move_entry(1, -1);
        assert_eq!(state.position, 0);
        assert_eq!(state.current().unwrap().title, "B");
        state.remove_entry(0);
        assert_eq!(state.position, 0);
        assert_eq!(state.current().unwrap().title, "A");
        state.clear();
        assert!(state.queue.is_empty());
        assert_eq!(state.status, JobStatus::Complete);
    }

    #[test]
    fn sorting_orders_all_pages_and_preserves_selection_and_decisions() {
        let mut state = JobState::new(vec![
            entry("Reviewed"),
            entry("Zulu"),
            entry("Current"),
            entry("Alpha"),
        ]);
        state.queue[0].status = QueueEntryStatus::Reviewed;
        state.position = 2;
        state.sort_by_title();
        assert_eq!(
            state
                .queue
                .iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>(),
            ["Alpha", "Current", "Reviewed", "Zulu"]
        );
        assert_eq!(state.position, 1);
        assert_eq!(state.current().unwrap().title, "Current");
        assert_eq!(state.queue[2].status, QueueEntryStatus::Reviewed);
    }

    #[test]
    fn sorting_completed_review_reorders_saved_pages() {
        let mut state = JobState::new(vec![
            entry("User:RLuts/test4"),
            entry("User:RLuts/test"),
            entry("User:RLuts/test2"),
            entry("User:RLuts/test3"),
        ]);
        for entry in &mut state.queue {
            entry.status = QueueEntryStatus::Saved;
        }
        state.status = JobStatus::Complete;
        state.sort_by_title();
        assert_eq!(
            state
                .queue
                .iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>(),
            [
                "User:RLuts/test",
                "User:RLuts/test2",
                "User:RLuts/test3",
                "User:RLuts/test4"
            ]
        );
        assert_eq!(state.position, 3);
        assert_eq!(state.status, JobStatus::Complete);
        assert!(
            state
                .queue
                .iter()
                .all(|entry| entry.status == QueueEntryStatus::Saved)
        );
    }

    #[test]
    fn advancing_after_sort_visits_pending_pages_before_selected_page() {
        let mut state = JobState::new(vec![entry("Zulu"), entry("Alpha"), entry("Bravo")]);
        state.sort_by_title();
        assert_eq!(state.position, 2);
        state.advance(QueueEntryStatus::Reviewed, None);
        assert_eq!(state.current().unwrap().title, "Alpha");
        state.advance(QueueEntryStatus::Reviewed, None);
        assert_eq!(state.current().unwrap().title, "Bravo");
        state.advance(QueueEntryStatus::Reviewed, None);
        assert_eq!(state.status, JobStatus::Complete);
        assert_eq!(state.counters().pending, 0);
    }

    #[test]
    fn preparse_returns_to_first_page_requiring_review() {
        let mut state = JobState::new(vec![entry("No change"), entry("Needs review")]);
        state.queue[0].status = QueueEntryStatus::Skipped;
        state.preparse_active = true;
        state.preparse_cursor = 2;
        state.finish_preparse_if_done();
        assert!(!state.preparse_active);
        assert_eq!(state.position, 1);
        assert_eq!(state.status, JobStatus::Active);
    }
}
