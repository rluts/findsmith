use axum::{http::StatusCode, response::Response};
use chrono::{TimeDelta, Utc};
use sha1::{Digest, Sha1};
use unicode_normalization::UnicodeNormalization;

use crate::{
    error::{self, ApiFailureKind},
    jobs::{
        EditAttempt, JobConfigV1, JobError, JobRepository, JobState, JobStatus, QueueEntryStatus,
    },
    localization,
    mediawiki::{
        RevisionHistory, edit_existing_page, edit_permissions, load_page, revisions_since,
    },
    policy,
    wikis::{self},
};

use super::job_actions::JobActionForm;
use super::responses::{action_response, error_page, job_error_response};
use super::summary::{
    EDIT_SUMMARY_ATTRIBUTION_LINK, complete_edit_summary, suggested_edit_summary,
};

#[allow(clippy::too_many_arguments)]
pub(super) async fn save_current_page(
    repository: &JobRepository<'_>,
    owner: &str,
    username: &str,
    access_token: &str,
    user_agent: &str,
    project_id: &str,
    job_id: &str,
    wiki_url: &str,
    state_version: i64,
    state: &mut JobState,
    config: &JobConfigV1,
    form: &JobActionForm,
    back_href: &str,
) -> Response {
    if !state.can_save_current() {
        return error_page(
            StatusCode::CONFLICT,
            "findsmith-api-cannot-save",
            "Select a pending, reviewed, or skipped page in an active or completed review.",
            back_href,
        );
    }
    if state.edit_policy_blocked {
        return error_page(
            StatusCode::CONFLICT,
            "findsmith-api-edit-blocked",
            "findsmith-api-this-job-encountered-a-wiki-policy-warning-start-a-new-review-after-reso",
            back_href,
        );
    }
    let (Some(text), Some(base_revid), Some(start_timestamp)) =
        (&form.proposed_text, form.base_revid, &form.start_timestamp)
    else {
        return error_page(
            StatusCode::BAD_REQUEST,
            "findsmith-api-cannot-save",
            "The reviewed text, base revision, and start timestamp are required.",
            back_href,
        );
    };
    if !form.policy_acknowledged && !state.policy_acknowledged {
        return error_page(
            StatusCode::BAD_REQUEST,
            "Policy acknowledgement required",
            "Acknowledge the selected wiki's editing policies before saving.",
            back_href,
        );
    }
    if text.len() > crate::rules::MAX_OUTPUT_BYTES {
        return error_page(
            StatusCode::BAD_REQUEST,
            "findsmith-api-cannot-save",
            "The proposed wikitext is too large.",
            back_href,
        );
    }
    let summary_prefix = form
        .edit_summary
        .as_deref()
        .map(str::to_owned)
        .unwrap_or_else(|| suggested_edit_summary(&config.rules));
    let summary = match complete_edit_summary(&summary_prefix) {
        Ok(summary) => summary,
        Err(message) => {
            return error_page(
                StatusCode::BAD_REQUEST,
                "findsmith-api-cannot-save",
                message,
                back_href,
            );
        }
    };
    let title = state
        .current()
        .map(|entry| entry.title.clone())
        .unwrap_or_default();
    let access = wikis::access_unregistered(wiki_url, user_agent, username).await;
    if !access.is_some_and(|access| access.allowed == Some(true)) {
        return pause_edit(
            repository,
            owner,
            project_id,
            job_id,
            state_version,
            state,
            "permission",
            "findsmith-api-save-awb-access",
            back_href,
        )
        .await;
    }
    let permissions = match edit_permissions(wiki_url, user_agent, access_token).await {
        Ok(permissions) => permissions,
        Err(error) => {
            return pause_edit(
                repository,
                owner,
                project_id,
                job_id,
                state_version,
                state,
                error::classify(&error).label(),
                "findsmith-api-save-rights-unverified",
                back_href,
            )
            .await;
        }
    };
    if let Some(blocker) = policy::account_blocker(&permissions) {
        let kind = blocker.kind().label();
        let message = match blocker {
            policy::AccountBlocker::Blocked => "findsmith-api-save-account-blocked",
            policy::AccountBlocker::NewMessages => "findsmith-api-save-new-messages",
            policy::AccountBlocker::MissingRights => "findsmith-api-save-rights-missing",
        };
        return pause_edit(
            repository,
            owner,
            project_id,
            job_id,
            state_version,
            state,
            kind,
            message,
            back_href,
        )
        .await;
    }
    if (form.minor && !permissions.can_minor) || (form.bot && !permissions.can_bot) {
        return pause_edit(
            repository,
            owner,
            project_id,
            job_id,
            state_version,
            state,
            "permission",
            "The selected minor or bot flag is not permitted for this account.",
            back_href,
        )
        .await;
    }
    let snapshot = match load_page(wiki_url, user_agent, access_token, &title).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return pause_edit(
                repository,
                owner,
                project_id,
                job_id,
                state_version,
                state,
                "conflict",
                &localization::required_with_args(
                    "findsmith-api-save-reload-page",
                    &[&error.to_string()],
                ),
                back_href,
            )
            .await;
        }
    };
    if snapshot.revision_id != base_revid || snapshot.title != title {
        return pause_edit(
            repository,
            owner,
            project_id,
            job_id,
            state_version,
            state,
            "conflict",
            "findsmith-api-save-revision-changed",
            back_href,
        )
        .await;
    }
    if !snapshot.editable {
        return pause_edit(
            repository,
            owner,
            project_id,
            job_id,
            state_version,
            state,
            "protection",
            "findsmith-api-save-page-protected",
            back_href,
        )
        .await;
    }
    match policy::bot_edit_allowed(wiki_url, user_agent, username, &snapshot.content).await {
        Ok(true) => {}
        Ok(false) => {
            state.edit_policy_blocked = true;
            return pause_edit(
                repository,
                owner,
                project_id,
                job_id,
                state_version,
                state,
                "bot exclusion",
                "findsmith-api-save-bot-excluded",
                back_href,
            )
            .await;
        }
        Err(_) => {
            return pause_edit(
                repository,
                owner,
                project_id,
                job_id,
                state_version,
                state,
                "bot exclusion",
                "findsmith-api-save-bot-check-failed",
                back_href,
            )
            .await;
        }
    }
    if snapshot.content == *text {
        return error_page(
            StatusCode::CONFLICT,
            "No changes",
            "The proposed wikitext matches the current revision.",
            back_href,
        );
    }
    if let Err(seconds) = policy::reserve_edit_slot(owner) {
        return error_page(
            StatusCode::TOO_MANY_REQUESTS,
            "findsmith-api-cannot-save",
            &localization::required_with_args(
                "findsmith-api-save-too-fast",
                &[&seconds.to_string()],
            ),
            back_href,
        );
    }
    state.policy_acknowledged = true;
    state.edit_in_flight = true;
    state.status = JobStatus::Stopped;
    let saved_index = state.position;
    let attempt = EditAttempt {
        content_sha1: stored_content_sha1(text),
        started_at: Utc::now(),
    };
    if let Some(entry) = state.queue.get_mut(state.position) {
        // The base revision and attempt let an interrupted request be checked later.
        entry.revision_id = Some(snapshot.revision_id);
        entry.revision_timestamp = Some(snapshot.revision_timestamp.clone());
        entry.edit_result = Some("in_flight".to_owned());
        entry.reason = Some("findsmith-api-edit-outcome-unknown-reason".to_owned());
        entry.edit_attempt = Some(attempt.clone());
    }
    if let Err(error) = repository
        .update_state(owner, project_id, job_id, state, state_version)
        .await
    {
        return job_error_response(error, back_href);
    }
    let outcome = edit_existing_page(
        wiki_url,
        user_agent,
        access_token,
        username,
        &snapshot.title,
        text,
        snapshot.revision_id,
        &snapshot.revision_timestamp,
        start_timestamp,
        &summary,
        form.minor,
        form.bot,
    )
    .await;
    let saved_revid = outcome.as_ref().ok().copied();
    match outcome {
        Ok(new_revid) => {
            state.edit_in_flight = false;
            state.consecutive_edit_failures = 0;
            state.status = JobStatus::Active;
            if let Some(entry) = state.queue.get_mut(state.position) {
                entry.edit_revision_id = Some(new_revid);
                entry.edit_result = Some("saved".to_owned());
            }
            state.advance(QueueEntryStatus::Saved, None);
        }
        Err(error) => {
            let kind = error::classify(&error);
            // Transport failures and unrecognized responses can happen after MediaWiki committed the edit.
            let ambiguous = ApiFailureKind::is_ambiguous(&error);
            state.edit_in_flight = ambiguous;
            state.consecutive_edit_failures += 1;
            // An ambiguous outcome stays locked as in flight until it is checked instead.
            state.edit_policy_blocked =
                !ambiguous && policy::halts_editing(kind, state.consecutive_edit_failures);
            let repeated = state.edit_policy_blocked && !kind.is_policy_sensitive();
            if let Some(entry) = state.queue.get_mut(state.position) {
                entry.edit_result = Some(kind.label().to_owned());
                entry.reason = Some(if ambiguous {
                    "findsmith-api-save-outcome-ambiguous".to_owned()
                } else if repeated {
                    "findsmith-api-save-repeated-failures".to_owned()
                } else if kind == ApiFailureKind::Conflict {
                    "findsmith-api-save-conflict".to_owned()
                } else if kind == ApiFailureKind::AbuseFilter {
                    "findsmith-api-save-abuse-filter".to_owned()
                } else {
                    localization::required_with_args("findsmith-api-save-rejected", &[kind.label()])
                });
            }
        }
    }
    match (
        repository
            .update_state(owner, project_id, job_id, state, state_version + 1)
            .await,
        saved_revid,
    ) {
        (Ok(()), _) => action_response(back_href.to_owned(), Some(job_id.to_owned())),
        // A verification in another tab changed the state while the edit was
        // in flight. The wiki accepted the edit, so record that on the newer state.
        (Err(JobError::Conflict), Some(new_revid)) => {
            let save = LateSave {
                index: saved_index,
                base_revision: snapshot.revision_id,
                attempt: &attempt,
                new_revision: new_revid,
            };
            record_late_save(repository, owner, project_id, job_id, save, back_href).await
        }
        (Err(error), _) => job_error_response(error, back_href),
    }
}

/// How long a check waits before treating a missing revision as proof that an
/// edit was not saved. It covers the request timeout, the server's handler
/// timeout and MediaWiki committing an edit after the client disconnected.
const VERIFY_GRACE_SECONDS: i64 = 180;

/// SHA-1 of the text as MediaWiki stores it: Unicode NFC, line endings
/// normalized to `\n` and trailing whitespace removed.
fn stored_content_sha1(text: &str) -> String {
    let normalized: String = text.nfc().collect();
    let normalized = normalized.replace("\r\n", "\n").replace('\r', "\n");
    let normalized = normalized.trim_end_matches([' ', '\t', '\n', '\r', '\0', '\x0B']);
    Sha1::digest(normalized.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

struct LateSave<'a> {
    index: usize,
    base_revision: u64,
    attempt: &'a EditAttempt,
    new_revision: u64,
}

#[derive(Debug, PartialEq, Eq)]
enum LateSaveOutcome {
    AlreadyRecorded,
    Recorded,
    Unrelated,
}

/// Marks the attempt saved on a state that changed while its request was in
/// flight. The entry must still be the one this attempt was made for.
fn apply_late_save(state: &mut JobState, save: &LateSave<'_>) -> LateSaveOutcome {
    let Some(entry) = state.queue.get_mut(save.index) else {
        return LateSaveOutcome::Unrelated;
    };
    if entry.revision_id != Some(save.base_revision)
        || entry.edit_attempt.as_ref() != Some(save.attempt)
    {
        return LateSaveOutcome::Unrelated;
    }
    if entry.edit_revision_id == Some(save.new_revision)
        && entry.edit_result.as_deref() == Some("saved")
    {
        return LateSaveOutcome::AlreadyRecorded;
    }
    entry.edit_revision_id = Some(save.new_revision);
    entry.edit_result = Some("saved".to_owned());
    entry.reason = None;
    if save.index == state.position && state.edit_in_flight {
        state.edit_in_flight = false;
    }
    state.consecutive_edit_failures = 0;
    if save.index == state.position && state.status == JobStatus::Stopped {
        state.status = JobStatus::Active;
        state.advance(QueueEntryStatus::Saved, None);
    } else if let Some(entry) = state.queue.get_mut(save.index) {
        entry.status = QueueEntryStatus::Saved;
    }
    LateSaveOutcome::Recorded
}

async fn record_late_save(
    repository: &JobRepository<'_>,
    owner: &str,
    project_id: &str,
    job_id: &str,
    save: LateSave<'_>,
    back_href: &str,
) -> Response {
    let job = match repository.get(owner, project_id, job_id).await {
        Ok(Some(job)) => job,
        Ok(None) => return job_error_response(JobError::Conflict, back_href),
        Err(error) => return job_error_response(error, back_href),
    };
    let mut state: JobState = match serde_json::from_value(job.state_json) {
        Ok(state) => state,
        Err(error) => return job_error_response(JobError::Serialization(error), back_href),
    };
    let result = match apply_late_save(&mut state, &save) {
        LateSaveOutcome::AlreadyRecorded => Ok(()),
        LateSaveOutcome::Recorded => {
            repository
                .update_state(owner, project_id, job_id, &state, job.state_version)
                .await
        }
        LateSaveOutcome::Unrelated => Err(JobError::Conflict),
    };
    match result {
        Ok(()) => action_response(back_href.to_owned(), Some(job_id.to_owned())),
        Err(error) => job_error_response(error, back_href),
    }
}

#[allow(clippy::too_many_arguments)]
async fn pause_edit(
    repository: &JobRepository<'_>,
    owner: &str,
    project_id: &str,
    job_id: &str,
    state_version: i64,
    state: &mut JobState,
    kind: &str,
    message: &str,
    back_href: &str,
) -> Response {
    state.status = JobStatus::Stopped;
    if let Some(entry) = state.queue.get_mut(state.position) {
        entry.edit_result = Some(kind.to_owned());
        entry.reason = Some(message.to_owned());
    }
    match repository
        .update_state(owner, project_id, job_id, state, state_version)
        .await
    {
        Ok(()) => action_response(back_href.to_owned(), Some(job_id.to_owned())),
        Err(error) => job_error_response(error, back_href),
    }
}

/// Resolves an edit whose outcome is unknown, for example after a timeout or a
/// server restart, by looking for the FindSmith revision in the page history.
#[allow(clippy::too_many_arguments)]
pub(super) async fn verify_edit_response(
    repository: &JobRepository<'_>,
    owner: &str,
    username: &str,
    access_token: &str,
    user_agent: &str,
    ids: (&str, &str),
    wiki_url: &str,
    state_version: i64,
    mut state: JobState,
    back_href: &str,
) -> Response {
    let (project_id, job_id) = ids;
    let Some(entry) = state.current().filter(|_| state.edit_in_flight) else {
        return error_page(
            StatusCode::CONFLICT,
            "No edit to check",
            "This review has no save whose outcome is unknown.",
            back_href,
        );
    };
    let base_revision = entry.revision_id;
    let attempt = entry.edit_attempt.clone();
    let history = match revisions_since(
        wiki_url,
        user_agent,
        access_token,
        &entry.title,
        base_revision,
    )
    .await
    {
        Ok(history) => history,
        Err(error) => {
            return error_page(
                StatusCode::BAD_GATEWAY,
                "findsmith-api-could-not-verify-edit",
                &localization::required_with_args(
                    "findsmith-api-mediawiki-request-error",
                    &[error::classify(&error).label()],
                ),
                back_href,
            );
        }
    };
    let verification = classify_history(
        &history,
        username,
        attempt
            .as_ref()
            .map(|attempt| attempt.content_sha1.as_str()),
    );
    let remaining_seconds = attempt.as_ref().map_or(0, |attempt| {
        (attempt.started_at + TimeDelta::seconds(VERIFY_GRACE_SECONDS) - Utc::now())
            .num_seconds()
            .max(0)
    });
    match verification {
        Verification::Saved(revision_id) => {
            state.edit_in_flight = false;
            state.consecutive_edit_failures = 0;
            state.status = JobStatus::Active;
            if let Some(entry) = state.queue.get_mut(state.position) {
                entry.edit_revision_id = Some(revision_id);
                entry.edit_result = Some("saved".to_owned());
                entry.reason = None;
            }
            state.advance(QueueEntryStatus::Saved, None);
        }
        // The request may still be running, so a missing revision proves nothing yet.
        Verification::NotSaved | Verification::Unknown if remaining_seconds > 0 => {
            return error_page(
                StatusCode::CONFLICT,
                "findsmith-api-edit-still-in-progress",
                &localization::required_with_args(
                    "findsmith-api-edit-still-in-progress-details",
                    &[&remaining_seconds.to_string()],
                ),
                back_href,
            );
        }
        Verification::NotSaved => {
            state.edit_in_flight = false;
            state.status = JobStatus::Stopped;
            if let Some(entry) = state.queue.get_mut(state.position) {
                entry.edit_result = Some("not_saved".to_owned());
                entry.reason = Some("findsmith-api-edit-verified-not-saved".to_owned());
            }
        }
        Verification::Unknown => {
            return error_page(
                StatusCode::CONFLICT,
                "findsmith-api-edit-outcome-unconfirmed",
                "findsmith-api-edit-outcome-unconfirmed-details",
                back_href,
            );
        }
    }
    match repository
        .update_state(owner, project_id, job_id, &state, state_version)
        .await
    {
        Ok(()) => action_response(back_href.to_owned(), Some(job_id.to_owned())),
        Err(error) => job_error_response(error, back_href),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Verification {
    Saved(u64),
    NotSaved,
    Unknown,
}

/// Decides whether the history after the base revision contains this attempt:
/// a revision by this account, with the FindSmith attribution and the
/// attempt's content hash. Anything that could be the attempt but cannot be
/// confirmed, including an incomplete history, leaves the outcome unknown.
/// Attempts recorded before content hashes were stored match on account and
/// attribution alone.
fn classify_history(
    history: &RevisionHistory,
    username: &str,
    content_sha1: Option<&str>,
) -> Verification {
    let normalize = |name: &str| name.replace('_', " ");
    let own_account = |user: &str| normalize(user) == normalize(username);
    let mut uncertain = !history.complete;
    let mut matched = None;
    // Newest first: keep the oldest match, the one made against the base revision.
    for revision in &history.revisions {
        if own_account(&revision.user) && revision.comment.contains(EDIT_SUMMARY_ATTRIBUTION_LINK) {
            match content_sha1 {
                Some(expected) if revision.sha1.eq_ignore_ascii_case(expected) => {
                    matched = Some(revision.revision_id);
                }
                // Another FindSmith edit by this account, or this one with
                // content MediaWiki transformed or hid.
                Some(_) => uncertain = true,
                None => matched = Some(revision.revision_id),
            }
        } else if revision.user.is_empty()
            || (own_account(&revision.user) && revision.comment.is_empty())
        {
            // Hidden user or summary: the revision could be this attempt.
            uncertain = true;
        }
    }
    match matched {
        Some(revision_id) => Verification::Saved(revision_id),
        None if uncertain => Verification::Unknown,
        None => Verification::NotSaved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mediawiki::RecentRevision;

    fn revision(revision_id: u64, user: &str, comment: &str, sha1: &str) -> RecentRevision {
        RecentRevision {
            revision_id,
            user: user.to_owned(),
            comment: comment.to_owned(),
            sha1: sha1.to_owned(),
        }
    }

    fn history(revisions: Vec<RecentRevision>, complete: bool) -> RevisionHistory {
        RevisionHistory {
            revisions,
            complete,
        }
    }

    fn attributed() -> String {
        format!("Fix typo ({EDIT_SUMMARY_ATTRIBUTION_LINK})")
    }

    #[test]
    fn attempt_is_matched_by_account_attribution_and_content() {
        let complete = history(
            vec![
                revision(12, "Other", "unrelated", "ccc"),
                revision(11, "Some_User", &attributed(), "abc"),
            ],
            true,
        );
        assert_eq!(
            classify_history(&complete, "Some User", Some("ABC")),
            Verification::Saved(11)
        );
        let legacy = history(vec![revision(11, "Some User", &attributed(), "")], true);
        assert_eq!(
            classify_history(&legacy, "Some User", None),
            Verification::Saved(11)
        );
    }

    #[test]
    fn unrelated_findsmith_edit_by_the_same_account_is_not_the_attempt() {
        let other_attempt = history(vec![revision(11, "Some User", &attributed(), "zzz")], true);
        assert_eq!(
            classify_history(&other_attempt, "Some User", Some("abc")),
            Verification::Unknown
        );
        let both = history(
            vec![
                revision(12, "Some User", &attributed(), "zzz"),
                revision(11, "Some User", &attributed(), "abc"),
            ],
            true,
        );
        assert_eq!(
            classify_history(&both, "Some User", Some("abc")),
            Verification::Saved(11)
        );
    }

    #[test]
    fn missing_attempt_is_not_saved_only_with_complete_history() {
        let revisions = || {
            vec![
                revision(12, "Other", "unrelated", "ccc"),
                revision(11, "Some User", "manual edit", "ddd"),
            ]
        };
        assert_eq!(
            classify_history(&history(revisions(), true), "Some User", Some("abc")),
            Verification::NotSaved
        );
        assert_eq!(
            classify_history(&history(revisions(), false), "Some User", Some("abc")),
            Verification::Unknown
        );
        assert_eq!(
            classify_history(&history(Vec::new(), true), "Some User", Some("abc")),
            Verification::NotSaved
        );
        let hidden = history(vec![revision(11, "", "", "")], true);
        assert_eq!(
            classify_history(&hidden, "Some User", Some("abc")),
            Verification::Unknown
        );
    }

    #[test]
    fn content_hash_matches_mediawiki_storage() {
        // MediaWiki stores "Hello\r\nworld\n  " as "Hello\nworld".
        let expected = "8b2b2671aca63a28c0f4f92be7030bf9481f113b";
        assert_eq!(stored_content_sha1("Hello\nworld"), expected);
        assert_eq!(stored_content_sha1("Hello\r\nworld\n  "), expected);
        // Decomposed "café" is stored in NFC.
        assert_eq!(
            stored_content_sha1("cafe\u{301}"),
            "f424452a9673918c6f09b0cdd35b20be8e6ae7d7"
        );
    }

    fn queue_entry(title: &str) -> crate::jobs::QueueEntry {
        crate::jobs::QueueEntry {
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
            edit_attempt: None,
        }
    }

    #[test]
    fn late_save_is_recorded_on_a_state_changed_by_verification() {
        let attempt = EditAttempt {
            content_sha1: "abc".to_owned(),
            started_at: Utc::now(),
        };
        let mut state = JobState::new(vec![
            crate::jobs::QueueEntry {
                revision_id: Some(10),
                edit_result: Some("not_saved".to_owned()),
                edit_attempt: Some(attempt.clone()),
                ..queue_entry("A")
            },
            queue_entry("B"),
        ]);
        state.status = JobStatus::Stopped;
        let save = LateSave {
            index: 0,
            base_revision: 10,
            attempt: &attempt,
            new_revision: 11,
        };
        assert_eq!(
            apply_late_save(&mut state, &save),
            LateSaveOutcome::Recorded
        );
        assert_eq!(state.queue[0].status, QueueEntryStatus::Saved);
        assert_eq!(state.queue[0].edit_revision_id, Some(11));
        assert_eq!(state.position, 1);
        assert_eq!(state.status, JobStatus::Active);
        assert_eq!(
            apply_late_save(&mut state, &save),
            LateSaveOutcome::AlreadyRecorded
        );
        let other = EditAttempt {
            content_sha1: "zzz".to_owned(),
            ..attempt.clone()
        };
        let unrelated = LateSave {
            attempt: &other,
            ..save
        };
        assert_eq!(
            apply_late_save(&mut state, &unrelated),
            LateSaveOutcome::Unrelated
        );
    }
}
