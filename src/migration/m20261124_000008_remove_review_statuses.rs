//! Rewrites job states saved before the Reviewed page status and the Stopped
//! job status were removed: reviewed pages become pending again, and stopped
//! jobs, which could only be stopped by a failed save, become save_failed.

use sea_orm_migration::{
    prelude::*,
    sea_orm::{ConnectionTrait, Statement},
};
use serde_json::Value;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        rewrite_states(
            manager,
            "JSON_UNQUOTE(JSON_EXTRACT(state_json, '$.status')) = 'stopped' \
             OR JSON_SEARCH(state_json, 'one', 'reviewed', NULL, '$.queue[*].status') IS NOT NULL",
            upgrade_state,
        )
        .await
    }

    /// Reviewed pages cannot be told apart from pending ones afterwards, so
    /// only the job status is restored.
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        rewrite_states(
            manager,
            "JSON_UNQUOTE(JSON_EXTRACT(state_json, '$.status')) = 'save_failed'",
            |state| replace_string(state.get_mut("status"), "save_failed", "stopped"),
        )
        .await
    }
}

async fn rewrite_states(
    manager: &SchemaManager<'_>,
    condition: &str,
    rewrite: impl Fn(&mut Value) -> bool,
) -> Result<(), DbErr> {
    let database = manager.get_connection();
    let backend = manager.get_database_backend();
    let rows = database
        .query_all_raw(Statement::from_string(
            backend,
            format!("SELECT id, CAST(state_json AS CHAR) AS state FROM jobs WHERE {condition}"),
        ))
        .await?;
    for row in rows {
        let id: String = row.try_get("", "id")?;
        let text: String = row.try_get("", "state")?;
        let mut state: Value = serde_json::from_str(&text)
            .map_err(|error| DbErr::Migration(format!("job {id} state: {error}")))?;
        if !rewrite(&mut state) {
            continue;
        }
        database
            .execute_raw(Statement::from_sql_and_values(
                backend,
                "UPDATE jobs SET state_json = ? WHERE id = ?",
                [state.to_string().into(), id.into()],
            ))
            .await?;
    }
    Ok(())
}

/// Returns whether anything changed.
fn upgrade_state(state: &mut Value) -> bool {
    let mut changed = replace_string(state.get_mut("status"), "stopped", "save_failed");
    if let Some(queue) = state.get_mut("queue").and_then(Value::as_array_mut) {
        for entry in queue {
            changed |= replace_string(entry.get_mut("status"), "reviewed", "pending");
        }
    }
    changed
}

fn replace_string(value: Option<&mut Value>, from: &str, to: &str) -> bool {
    match value {
        Some(value) if value.as_str() == Some(from) => {
            *value = Value::from(to);
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{JobState, JobStatus, QueueEntryStatus};

    #[test]
    fn legacy_state_loads_after_upgrade() {
        let mut state = serde_json::json!({
            "status": "stopped",
            "position": 0,
            "queue": [
                {
                    "requested_title": "A", "title": "A", "page_id": 1, "namespace": 0,
                    "redirect": false, "status": "reviewed", "reason": null,
                    "revision_id": null, "revision_timestamp": null, "replacement_count": null
                },
                {
                    "requested_title": "B", "title": "B", "page_id": 2, "namespace": 0,
                    "redirect": false, "status": "saved", "reason": null,
                    "revision_id": null, "revision_timestamp": null, "replacement_count": null
                }
            ]
        });
        assert!(upgrade_state(&mut state));
        assert!(!upgrade_state(&mut state));
        let restored: JobState = serde_json::from_value(state).unwrap();
        assert_eq!(restored.status, JobStatus::SaveFailed);
        assert_eq!(restored.queue[0].status, QueueEntryStatus::Pending);
        assert_eq!(restored.queue[1].status, QueueEntryStatus::Saved);
    }
}
