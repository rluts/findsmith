use crate::{
    localization,
    projects::{ReplacementRule, RuleOperation},
};

// Move to mediawiki.org? meta?
pub(super) const EDIT_SUMMARY_ATTRIBUTION_LINK: &str = "[[User:RLuts/FS|FindSmith]]";

const MAX_EDIT_SUMMARY_CHARS: usize = 500;

fn summary_message(key: &str, args: &[&str]) -> String {
    localization::message_with_args(key, args).expect("English edit summary message exists")
}

pub(super) fn suggested_edit_summary(rules: &[ReplacementRule]) -> String {
    let candidate = rules
        .iter()
        .find(|rule| rule.enabled)
        .map(|rule| match rule.operation {
            RuleOperation::Replace => {
                let replacement = if rule.replace.is_empty() {
                    localization::message("findsmith-edit-summary-removed")
                        .expect("English edit summary message exists")
                } else {
                    rule.replace.clone()
                };
                summary_message(
                    "findsmith-edit-summary-replacement",
                    &[&rule.find, &replacement],
                )
            }
            RuleOperation::Append => summary_message("findsmith-edit-summary-append", &[]),
            RuleOperation::Prepend => summary_message("findsmith-edit-summary-prepend", &[]),
            RuleOperation::CategoryAdd => {
                summary_message("findsmith-edit-summary-category-add", &[&rule.replace])
            }
            RuleOperation::CategoryRemove => {
                summary_message("findsmith-edit-summary-category-remove", &[&rule.find])
            }
            RuleOperation::CategoryReplace => summary_message(
                "findsmith-edit-summary-category-replace",
                &[&rule.find, &rule.replace],
            ),
            RuleOperation::TemplateRename => summary_message(
                "findsmith-edit-summary-template-rename",
                &[&rule.find, &rule.replace],
            ),
            RuleOperation::TemplateParameterSet => {
                let value = if rule.replace.is_empty() {
                    localization::message("findsmith-edit-summary-empty")
                        .expect("English edit summary message exists")
                } else {
                    rule.replace.clone()
                };
                summary_message(
                    "findsmith-edit-summary-parameter-set",
                    &[&rule.parameter, &rule.find, &value],
                )
            }
            RuleOperation::TemplateParameterRemove => summary_message(
                "findsmith-edit-summary-parameter-remove",
                &[&rule.parameter, &rule.find],
            ),
        });
    candidate
        .filter(|prefix| complete_edit_summary(prefix).is_ok())
        .unwrap_or_else(|| summary_message("findsmith-edit-summary-autoreplacement", &[]))
}

/// The rules' summary followed by AWB's "typos fixed: ..." for the current page.
/// Typo items that do not fit in the summary length are dropped from the end.
pub(super) fn suggested_edit_summary_with_typos(rules: &[ReplacementRule], typos: &str) -> String {
    let rules_summary = rules
        .iter()
        .any(|rule| rule.enabled)
        .then(|| suggested_edit_summary(rules));
    if typos.is_empty() {
        return rules_summary.unwrap_or_else(|| suggested_edit_summary(rules));
    }
    let mut items: Vec<&str> = typos.split(", ").collect();
    while !items.is_empty() {
        let typos = summary_message("findsmith-edit-summary-typos", &[&items.join(", ")]);
        let candidate = match &rules_summary {
            Some(rules_summary) => format!("{rules_summary}; {typos}"),
            None => typos,
        };
        if complete_edit_summary(&candidate).is_ok() {
            return candidate;
        }
        items.pop();
    }
    rules_summary.unwrap_or_else(|| suggested_edit_summary(rules))
}

pub(super) fn complete_edit_summary(prefix: &str) -> Result<String, &'static str> {
    let prefix = prefix.trim();
    if prefix.is_empty() {
        return Err("Enter an edit summary before saving.");
    }
    if prefix.contains(EDIT_SUMMARY_ATTRIBUTION_LINK) {
        return Err(
            "The Findsmith attribution is added automatically; do not include it in the editable summary.",
        );
    }
    if prefix.chars().any(char::is_control) {
        return Err("findsmith-api-summary-one-line");
    }
    let summary = summary_message(
        "findsmith-edit-summary-with-attribution",
        &[prefix, EDIT_SUMMARY_ATTRIBUTION_LINK],
    );
    if summary.chars().count() > MAX_EDIT_SUMMARY_CHARS {
        return Err(
            "The edit summary, including the required Findsmith attribution, must be at most 500 characters.",
        );
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typo_summary_follows_rules_and_fits_the_limit() {
        assert_eq!(
            suggested_edit_summary_with_typos(&[], "teh → the, acheive → achieve"),
            "typos fixed: teh → the, acheive → achieve"
        );
        let rules = vec![ReplacementRule {
            find: "a".to_owned(),
            replace: "b".to_owned(),
            ..ReplacementRule::default()
        }];
        assert!(
            suggested_edit_summary_with_typos(&rules, "teh → the")
                .ends_with("; typos fixed: teh → the")
        );
        let long = vec!["x".repeat(200); 5].join(", ");
        let summary = suggested_edit_summary_with_typos(&[], &long);
        assert!(complete_edit_summary(&summary).is_ok());
        assert_eq!(summary.matches(", ").count(), 1);
        assert_eq!(
            suggested_edit_summary_with_typos(&[], ""),
            "Autoreplacement"
        );
    }

    #[test]
    fn edit_summary_falls_back_when_rule_text_is_too_long() {
        let rules = vec![ReplacementRule {
            find: "a".repeat(500),
            replace: "b".to_owned(),
            ..Default::default()
        }];
        assert_eq!(suggested_edit_summary(&rules), "Autoreplacement");
        let multiline = [ReplacementRule {
            find: "a\nb".to_owned(),
            replace: "c".to_owned(),
            ..Default::default()
        }];
        assert_eq!(suggested_edit_summary(&multiline), "Autoreplacement");
        assert!(complete_edit_summary(&"a".repeat(500)).is_err());
        let attribution_length = complete_edit_summary("a").unwrap().chars().count() - 1;
        let allowed = "ї".repeat(500 - attribution_length);
        assert_eq!(
            complete_edit_summary(&allowed).unwrap().chars().count(),
            500
        );
        assert!(complete_edit_summary(&format!("{allowed}ї")).is_err());
        assert!(complete_edit_summary("custom\nreason").is_err());
    }
}
