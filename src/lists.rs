use std::collections::HashSet;

use crate::{
    jobs::{QueueEntry, QueueEntryStatus},
    mediawiki::NormalizedTitle,
};

pub const MAX_QUEUE_ENTRIES: usize = 500;

pub fn manual_titles(lines: &[String]) -> Result<Vec<String>, String> {
    let titles: Vec<_> = lines
        .iter()
        .map(|title| title.trim())
        .filter(|title| !title.is_empty())
        .map(str::to_owned)
        .collect();
    if titles.is_empty() {
        return Err("Add at least one page title before starting review.".to_owned());
    }
    if titles.len() > MAX_QUEUE_ENTRIES {
        return Err(format!(
            "A review can contain at most {MAX_QUEUE_ENTRIES} page titles."
        ));
    }
    Ok(titles)
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
        assert!(manual_titles(&["  ".to_owned()]).is_err());
    }
}
