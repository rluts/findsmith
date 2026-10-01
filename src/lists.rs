use std::collections::HashSet;

use crate::{
    jobs::{QueueEntry, QueueEntryStatus},
    mediawiki::NormalizedTitle,
    projects::MAX_QUEUE_LIMIT,
};

pub fn source_values(lines: &[String], label: &str) -> Result<Vec<String>, String> {
    let titles: Vec<_> = lines
        .iter()
        .map(|title| title.trim())
        .filter(|title| !title.is_empty())
        .map(str::to_owned)
        .collect();
    if titles.len() > MAX_QUEUE_LIMIT {
        return Err(format!(
            "{label} can contain at most {MAX_QUEUE_LIMIT} entries."
        ));
    }
    Ok(titles)
}

pub fn validate_queue_limit(limit: usize) -> Result<usize, String> {
    if (1..=MAX_QUEUE_LIMIT).contains(&limit) {
        Ok(limit)
    } else {
        Err(format!(
            "Queue limit must be between 1 and {MAX_QUEUE_LIMIT}."
        ))
    }
}

pub fn validate_namespaces(namespaces: &[i64]) -> Result<Vec<i64>, String> {
    if namespaces.len() > 50 {
        return Err("Select at most 50 namespaces.".to_owned());
    }
    let mut output = Vec::new();
    for namespace in namespaces {
        if !(-2..=32_767).contains(namespace) {
            return Err(format!("Namespace {namespace} is out of range."));
        }
        if !output.contains(namespace) {
            output.push(*namespace);
        }
    }
    Ok(output)
}

pub fn queue_from_normalized(titles: Vec<NormalizedTitle>) -> Vec<QueueEntry> {
    let mut seen = HashSet::new();
    titles
        .into_iter()
        .filter_map(|title| {
            if !seen.insert(title.title.clone()) {
                return None;
            }
            let (status, reason) = if let Some(reason) = title.invalid_reason {
                (QueueEntryStatus::Invalid, Some(reason))
            } else if title.page_id.is_none() {
                (
                    QueueEntryStatus::Missing,
                    Some("The requested page does not exist.".to_owned()),
                )
            } else {
                (QueueEntryStatus::Pending, None)
            };
            Some(QueueEntry {
                requested_title: title.requested_title,
                title: title.title,
                page_id: title.page_id,
                namespace: title.namespace,
                redirect: title.redirect,
                status,
                reason,
                revision_id: None,
                revision_timestamp: None,
                replacement_count: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_deduplicates_canonical_titles_in_order() {
        let queue = queue_from_normalized(vec![
            NormalizedTitle {
                requested_title: "foo".to_owned(),
                title: "Foo".to_owned(),
                page_id: Some(1),
                namespace: Some(0),
                redirect: false,
                invalid_reason: None,
            },
            NormalizedTitle {
                requested_title: "Foo".to_owned(),
                title: "Foo".to_owned(),
                page_id: Some(1),
                namespace: Some(0),
                redirect: false,
                invalid_reason: None,
            },
        ]);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].requested_title, "foo");
    }

    #[test]
    fn empty_manual_list_is_rejected() {
        assert!(
            source_values(&["  ".to_owned()], "Pages")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn queue_limit_is_bounded() {
        assert_eq!(validate_queue_limit(100), Ok(100));
        assert!(validate_queue_limit(0).is_err());
        assert!(validate_queue_limit(MAX_QUEUE_LIMIT + 1).is_err());
    }
}
