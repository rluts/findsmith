use crate::{
    localization,
    projects::{ReplacementRule, RuleOperation},
    rules::{ReplacementSample, TransformationResult},
};

pub(super) const EDIT_SUMMARY_ATTRIBUTION_LINK: &str = "[[toolforge:findsmith]]";

const MAX_EDIT_SUMMARY_CHARS: usize = 500;

fn summary_message(key: &str, args: &[&str]) -> String {
    localization::message_with_args(key, args).expect("English edit summary message exists")
}

fn replacement_summary(find: &str, replace: &str) -> String {
    let replacement = if replace.is_empty() {
        localization::message("findsmith-edit-summary-removed")
            .expect("English edit summary message exists")
    } else {
        replace.to_owned()
    };
    summary_message("findsmith-edit-summary-replacement", &[find, &replacement])
}

fn fallback_summary() -> String {
    summary_message("findsmith-edit-summary-autoreplacement", &[])
}

/// A summary from the rules' settings alone, for when no page result is
/// available: the first enabled rule, or the generic fallback.
pub(super) fn suggested_edit_summary(rules: &[ReplacementRule]) -> String {
    rules
        .iter()
        .find(|rule| rule.enabled)
        .map(|rule| rule_summary(rule, None))
        .filter(|prefix| complete_edit_summary(prefix).is_ok())
        .unwrap_or_else(fallback_summary)
}

/// A summary from what the rules actually changed on this page, like AWB's
/// "Add replacements to edit summary": each rule that changed the page, in
/// order, with its first real change and a count when it changed more than
/// once, after a `/* Section */` link when only one section changed. Rules
/// that do not fit in the length limit are left out.
pub(super) fn page_edit_summary(
    rules: &[ReplacementRule],
    result: &TransformationResult,
    original: &str,
    proposed: &str,
) -> String {
    let mut body: Option<String> = None;
    let mut links_section = true;
    for (index, rule) in rules.iter().enumerate() {
        let sample = result.samples.get(index).and_then(Option::as_ref);
        let count = match rule.operation {
            // A replace rule whose matches all kept their text changed nothing.
            RuleOperation::Replace => sample.map_or(0, |sample| sample.edits),
            _ => result.counts.get(index).copied().unwrap_or(0),
        };
        if count == 0 {
            continue;
        }
        let text = rule_summary(rule, sample);
        let text = if count > 1 {
            format!("{text} ({count})")
        } else {
            text
        };
        let candidate = match &body {
            Some(body) => summary_message("findsmith-edit-summary-list", &[body, &text]),
            None => text,
        };
        if complete_edit_summary(&candidate).is_err() {
            continue;
        }
        body = Some(candidate);
        // Categories and appended or prepended text sit at the page's edges, not
        // in the section they happen to touch, so they never link a section.
        links_section &= !matches!(
            rule.operation,
            RuleOperation::Append
                | RuleOperation::Prepend
                | RuleOperation::CategoryAdd
                | RuleOperation::CategoryRemove
                | RuleOperation::CategoryReplace
        );
    }
    let body = body.unwrap_or_else(fallback_summary);
    match links_section
        .then(|| modified_section(original, proposed))
        .flatten()
    {
        Some(section) => {
            let with_section = format!("/* {section} */ {body}");
            if complete_edit_summary(&with_section).is_ok() {
                with_section
            } else {
                body
            }
        }
        None => body,
    }
}

fn rule_summary(rule: &ReplacementRule, sample: Option<&ReplacementSample>) -> String {
    match rule.operation {
        RuleOperation::Replace => match sample {
            Some(sample) => replacement_summary(&sample.before, &sample.after),
            None => replacement_summary(&rule.find, &rule.replace),
        },
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
    }
}

/// The heading text of a line such as `== History ==`, at any level.
fn heading(line: &str) -> Option<&str> {
    let line = line.trim_end();
    let leading = line.len() - line.trim_start_matches('=').len();
    let trailing = line.len() - line.trim_end_matches('=').len();
    let level = leading.min(trailing);
    if !(1..=6).contains(&level) || line.len() <= 2 * level {
        return None;
    }
    let text = line[level..line.len() - level].trim();
    (!text.is_empty()).then_some(text)
}

/// Splits wikitext before each heading line, like AWB's `Tools.SplitToSections`.
fn sections(text: &str) -> Vec<&str> {
    let mut sections = Vec::new();
    let mut start = 0;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if offset > 0 && heading(line).is_some() {
            sections.push(&text[start..offset]);
            start = offset;
        }
        offset += line.len();
    }
    sections.push(&text[start..]);
    sections
}

/// The heading of the only section that changed, as AWB's `Summary.ModifiedSection`
/// finds it. None when the section count changed, several sections changed, or
/// only the lead changed: "top" would need its own localized message.
fn modified_section(original: &str, proposed: &str) -> Option<String> {
    let before = sections(original);
    let after = sections(proposed);
    if before.len() != after.len() {
        return None;
    }
    let mut changed = before
        .iter()
        .zip(&after)
        .filter(|(before, after)| before != after)
        .map(|(_, after)| *after);
    let section = changed.next()?;
    if changed.next().is_some() {
        return None;
    }
    let text = heading(section.lines().next()?)?;
    (!text.contains("*/")).then(|| text.to_owned())
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
            "Autoreplacement: old -> new using [[toolforge:findsmith]]"
        );
        assert_eq!(
            complete_edit_summary("Custom reason").unwrap(),
            "Custom reason using [[toolforge:findsmith]]"
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

    fn page_summary(rules: &[ReplacementRule], original: &str) -> String {
        let result = crate::rules::RuleEngine::compile(rules)
            .unwrap()
            .apply(original)
            .unwrap();
        page_edit_summary(rules, &result, original, &result.text)
    }

    #[test]
    fn page_summary_skips_rules_that_changed_nothing() {
        let rules = vec![
            ReplacementRule {
                find: "absent".to_owned(),
                replace: "x".to_owned(),
                ..Default::default()
            },
            ReplacementRule {
                find: "colou?r".to_owned(),
                replace: "color".to_owned(),
                regex: true,
                ..Default::default()
            },
        ];
        // "color" -> "color" changes nothing, so it is neither shown nor counted.
        assert_eq!(
            page_summary(&rules, "color colour colour"),
            "Autoreplacement: colour -> color (2)"
        );
        assert_eq!(
            page_summary(&rules, "one colour"),
            "Autoreplacement: colour -> color"
        );
    }

    #[test]
    fn page_summary_lists_every_rule_that_changed_the_page() {
        let rules = vec![
            ReplacementRule {
                find: "colour".to_owned(),
                replace: "color".to_owned(),
                ..Default::default()
            },
            ReplacementRule {
                find: "absent".to_owned(),
                replace: "x".to_owned(),
                ..Default::default()
            },
            ReplacementRule {
                find: "junk".to_owned(),
                ..Default::default()
            },
        ];
        let text = "Lead\n== A ==\ncolour colour junk\n";
        assert_eq!(
            page_summary(&rules, text),
            "/* A */ Autoreplacement: colour -> color (2), Autoreplacement: junk -> (removed)"
        );
        // A rule too long to fit is left out; later rules are still listed.
        let long = "a".repeat(300);
        let rules = vec![
            ReplacementRule {
                find: "x".to_owned(),
                replace: long.clone(),
                ..Default::default()
            },
            ReplacementRule {
                find: "y".to_owned(),
                replace: long,
                ..Default::default()
            },
            ReplacementRule {
                find: "z".to_owned(),
                replace: "w".to_owned(),
                ..Default::default()
            },
        ];
        let summary = page_summary(&rules, "x y z");
        assert!(summary.starts_with("Autoreplacement: x -> a"));
        assert!(summary.ends_with(", Autoreplacement: z -> w"));
        assert!(!summary.contains("y ->"));
        // Any edge rule in the list turns the section link off.
        let rules = vec![
            ReplacementRule {
                find: "old".to_owned(),
                replace: "new".to_owned(),
                ..Default::default()
            },
            ReplacementRule {
                operation: RuleOperation::Append,
                replace: " end".to_owned(),
                ..Default::default()
            },
        ];
        assert_eq!(
            page_summary(&rules, "Lead\n== A ==\nold"),
            "Autoreplacement: old -> new, Appended wikitext"
        );
    }

    #[test]
    fn page_summary_reports_removals_and_falls_back() {
        let remove = [ReplacementRule {
            find: "junk".to_owned(),
            ..Default::default()
        }];
        assert_eq!(
            page_summary(&remove, "a junk b"),
            "Autoreplacement: junk -> (removed)"
        );
        assert_eq!(page_summary(&remove, "clean"), "Autoreplacement");
        let multiline = [ReplacementRule {
            find: "a\nb".to_owned(),
            replace: "c".to_owned(),
            regex: true,
            ..Default::default()
        }];
        assert_eq!(page_summary(&multiline, "a\nb"), "Autoreplacement");
        let append = [ReplacementRule {
            operation: RuleOperation::Append,
            replace: "\n{{stub}}".to_owned(),
            ..Default::default()
        }];
        assert_eq!(page_summary(&append, "text"), "Appended wikitext");
        // Appending changes the last section, but gets no section link.
        assert_eq!(
            page_summary(&append, "Lead\n== Links ==\nx"),
            "Appended wikitext"
        );
        let prepend = [ReplacementRule {
            operation: RuleOperation::Prepend,
            replace: "{{Notice}}\n".to_owned(),
            ..Default::default()
        }];
        assert_eq!(
            page_summary(&prepend, "== Heading ==\nx"),
            "Prepended wikitext"
        );
    }

    #[test]
    fn page_summary_links_the_only_changed_section() {
        let rules = [ReplacementRule {
            find: "old".to_owned(),
            replace: "new".to_owned(),
            ..Default::default()
        }];
        let text = "Lead\n== History ==\nold\n=== Early ===\nsame\n";
        assert_eq!(
            page_summary(&rules, text),
            "/* History */ Autoreplacement: old -> new"
        );
        // Several sections, or only the lead: no section link.
        assert_eq!(
            page_summary(&rules, "old\n== A ==\nold\n"),
            "Autoreplacement: old -> new (2)"
        );
        assert_eq!(
            page_summary(&rules, "old\n== A ==\nx\n"),
            "Autoreplacement: old -> new"
        );
        // A heading that the rule itself changes changes the section count only if
        // it stops being a heading; a renamed heading is still one section.
        assert_eq!(
            page_summary(&rules, "Lead\n== old ==\nx\n"),
            "/* new */ Autoreplacement: old -> new"
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
        assert!(complete_edit_summary("custom [[toolforge:findsmith]]").is_err());
        assert!(complete_edit_summary("custom\nreason").is_err());
    }
}
