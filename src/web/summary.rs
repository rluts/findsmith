use crate::{
    localization,
    projects::{ReplacementRule, RuleOperation},
};

pub(super) const EDIT_SUMMARY_ATTRIBUTION_LINK: &str = "[[User:RLuts/FS|Findsmith]]";

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

pub(super) fn complete_edit_summary(prefix: &str) -> Result<String, &'static str> {
    let prefix = prefix.trim();
    if prefix.is_empty() {
        return Err("findsmith-api-summary-required");
    }
    if prefix.contains(EDIT_SUMMARY_ATTRIBUTION_LINK) {
        return Err("findsmith-api-summary-attribution-automatic");
    }
    if prefix.chars().any(char::is_control) {
        return Err("findsmith-api-summary-one-line");
    }
    let summary = summary_message(
        "findsmith-edit-summary-with-attribution",
        &[prefix, EDIT_SUMMARY_ATTRIBUTION_LINK],
    );
    if summary.chars().count() > MAX_EDIT_SUMMARY_CHARS {
        return Err("findsmith-api-summary-max-length");
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_summary_uses_first_enabled_rule_and_fixed_attribution() {
        let rules = vec![
            ReplacementRule {
                enabled: false,
                find: "ignored".to_owned(),
                replace: "ignored".to_owned(),
                ..Default::default()
            },
            ReplacementRule {
                find: "old".to_owned(),
                replace: "new".to_owned(),
                ..Default::default()
            },
        ];
        let suggested = suggested_edit_summary(&rules);
        assert_eq!(suggested, "Autoreplacement: old -> new");
        assert_eq!(
            complete_edit_summary(&suggested).unwrap(),
            "Autoreplacement: old -> new using [[User:RLuts/FS|Findsmith]]"
        );
        assert_eq!(
            complete_edit_summary("Custom reason").unwrap(),
            "Custom reason using [[User:RLuts/FS|Findsmith]]"
        );
    }

    #[test]
    fn edit_summary_describes_each_rule_operation() {
        let cases = [
            (RuleOperation::Append, "Appended wikitext"),
            (RuleOperation::Prepend, "Prepended wikitext"),
            (RuleOperation::CategoryAdd, "Added category New"),
            (RuleOperation::CategoryRemove, "Removed category Old"),
            (
                RuleOperation::CategoryReplace,
                "Replaced category Old with New",
            ),
            (RuleOperation::TemplateRename, "Renamed template Old to New"),
            (
                RuleOperation::TemplateParameterSet,
                "Set parameter key in template Old to New",
            ),
            (
                RuleOperation::TemplateParameterRemove,
                "Removed parameter key from template Old",
            ),
        ];
        for (operation, expected) in cases {
            let rule = ReplacementRule {
                operation,
                find: "Old".to_owned(),
                replace: "New".to_owned(),
                parameter: "key".to_owned(),
                ..Default::default()
            };
            assert_eq!(suggested_edit_summary(&[rule]), expected);
        }
        let rules = vec![
            ReplacementRule {
                operation: RuleOperation::CategoryAdd,
                replace: "New".to_owned(),
                ..Default::default()
            },
            ReplacementRule {
                find: "Old".to_owned(),
                replace: "New".to_owned(),
                ..Default::default()
            },
        ];
        assert_eq!(suggested_edit_summary(&rules), "Added category New");
        let empty_value = [ReplacementRule {
            operation: RuleOperation::TemplateParameterSet,
            find: "Example".to_owned(),
            parameter: "key".to_owned(),
            ..Default::default()
        }];
        assert_eq!(
            suggested_edit_summary(&empty_value),
            "Set parameter key in template Example to (empty)"
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
        assert!(complete_edit_summary("custom [[User:RLuts/FS|Findsmith]]").is_err());
        assert!(complete_edit_summary("custom\nreason").is_err());
    }
}
