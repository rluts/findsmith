use std::{collections::HashMap, error::Error, fmt};

use regex::{Regex, RegexBuilder};

use crate::{
    masking::{masked_ranges, overlaps},
    projects::{ReplacementRule, RuleMask, RuleOperation},
};

pub const MAX_RULES: usize = 100;
pub const MAX_PATTERN_BYTES: usize = 16 * 1024;
pub const MAX_REPLACEMENT_BYTES: usize = 64 * 1024;
pub const MAX_INPUT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_TOTAL_REPLACEMENTS: usize = 100_000;

#[derive(Debug)]
pub enum RuleError {
    TooManyRules,
    EmptyPattern { index: usize },
    EmptyValue { index: usize },
    InvalidCategory { index: usize },
    InvalidGroup { index: usize },
    ConflictingGroup { index: usize },
    InvalidTemplate { index: usize },
    StructuralRequiresWiki,
    PatternTooLarge { index: usize },
    ReplacementTooLarge { index: usize },
    InvalidRegex { index: usize, message: String },
    InputTooLarge,
    OutputTooLarge,
    TooManyReplacements,
}

impl fmt::Display for RuleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyRules => write!(formatter, "At most {MAX_RULES} rules are allowed."),
            Self::EmptyPattern { index } => {
                write!(formatter, "Rule {} must have a find value.", index + 1)
            }
            Self::EmptyValue { index } => {
                write!(
                    formatter,
                    "Rule {} must have an operation value.",
                    index + 1
                )
            }
            Self::InvalidCategory { index } => {
                write!(
                    formatter,
                    "Rule {} has an invalid category name.",
                    index + 1
                )
            }
            Self::InvalidGroup { index } => write!(
                formatter,
                "Rule {} needs a group name and text condition.",
                index + 1
            ),
            Self::ConflictingGroup { index } => write!(
                formatter,
                "Rule {} has a different condition from other rules in its group.",
                index + 1
            ),
            Self::InvalidTemplate { index } => write!(
                formatter,
                "Rule {} needs a valid template name and parameter name.",
                index + 1
            ),
            Self::StructuralRequiresWiki => {
                formatter.write_str("Template rules require the selected wiki's Parsoid service.")
            }
            Self::PatternTooLarge { index } => {
                write!(
                    formatter,
                    "The find value in rule {} is too large.",
                    index + 1
                )
            }
            Self::ReplacementTooLarge { index } => write!(
                formatter,
                "The replacement value in rule {} is too large.",
                index + 1
            ),
            Self::InvalidRegex { index, message } => {
                write!(
                    formatter,
                    "Rule {} has an invalid regex: {message}",
                    index + 1
                )
            }
            Self::InputTooLarge => formatter.write_str("The input text is too large."),
            Self::OutputTooLarge => formatter.write_str("The transformed text is too large."),
            Self::TooManyReplacements => write!(
                formatter,
                "At most {MAX_TOTAL_REPLACEMENTS} replacements are allowed."
            ),
        }
    }
}

impl Error for RuleError {}

pub struct RuleEngine {
    rules: Vec<CompiledRule>,
    groups: HashMap<String, (String, bool)>,
}

struct CompiledRule {
    enabled: bool,
    operation: RuleOperation,
    group_name: Option<String>,
    mask: RuleMask,
    replace: String,
    replace_all: bool,
    matcher: Option<Matcher>,
}

enum Matcher {
    Plain(Regex),
    Regex(Regex),
}

pub struct TransformationResult {
    pub text: String,
    pub counts: Vec<usize>,
    pub changed: Vec<bool>,
    pub total_replacements: usize,
}

impl RuleEngine {
    pub fn compile(rules: &[ReplacementRule]) -> Result<Self, RuleError> {
        Self::compile_with_category_prefix(rules, "Category")
    }

    pub fn compile_with_category_prefix(
        rules: &[ReplacementRule],
        category_prefix: &str,
    ) -> Result<Self, RuleError> {
        if rules.len() > MAX_RULES {
            return Err(RuleError::TooManyRules);
        }
        let mut groups = HashMap::new();
        for (index, rule) in rules.iter().enumerate() {
            if rule.group_name.is_empty() && rule.group_contains.is_empty() {
                continue;
            }
            if rule.group_name.trim().is_empty()
                || rule.group_contains.is_empty()
                || rule.group_name.chars().count() > 100
                || rule.group_contains.len() > MAX_PATTERN_BYTES
            {
                return Err(RuleError::InvalidGroup { index });
            }
            let condition = (rule.group_contains.clone(), rule.group_invert);
            if let Some(existing) = groups.insert(rule.group_name.clone(), condition.clone())
                && existing != condition
            {
                return Err(RuleError::ConflictingGroup { index });
            }
        }
        let rules = rules
            .iter()
            .enumerate()
            .map(|(index, rule)| compile_rule(index, rule, category_prefix))
            .collect::<Result<_, _>>()?;
        Ok(Self { rules, groups })
    }

    #[cfg(test)]
    pub fn apply(&self, input: &str) -> Result<TransformationResult, RuleError> {
        self.apply_checked(input, MAX_INPUT_BYTES)
    }

    pub fn apply_intermediate(&self, input: &str) -> Result<TransformationResult, RuleError> {
        self.apply_checked(input, MAX_OUTPUT_BYTES)
    }

    fn apply_checked(
        &self,
        input: &str,
        input_limit: usize,
    ) -> Result<TransformationResult, RuleError> {
        if input.len() > input_limit {
            return Err(RuleError::InputTooLarge);
        }
        let mut text = input.to_owned();
        let mut counts = Vec::with_capacity(self.rules.len());
        let mut changed = Vec::with_capacity(self.rules.len());
        let mut total_replacements = 0;

        for rule in &self.rules {
            let group_matches = rule.group_name.as_ref().is_none_or(|name| {
                let (needle, invert) = self.groups.get(name).expect("compiled group");
                input.contains(needle) != *invert
            });
            if !rule.enabled || !group_matches {
                counts.push(0);
                changed.push(false);
                continue;
            }
            let remaining = MAX_TOTAL_REPLACEMENTS - total_replacements;
            let (next, count) = match rule.operation {
                RuleOperation::Append => (
                    format!("{text}{}", rule.replace),
                    usize::from(!rule.replace.is_empty()),
                ),
                RuleOperation::Prepend => (
                    format!("{}{text}", rule.replace),
                    usize::from(!rule.replace.is_empty()),
                ),
                RuleOperation::CategoryAdd => {
                    let regex = rule.matcher.as_ref().expect("category matcher").regex();
                    if regex.is_match(&text) {
                        (text.clone(), 0)
                    } else {
                        let separator = if text.is_empty() || text.ends_with('\n') {
                            ""
                        } else {
                            "\n"
                        };
                        (
                            format!("{text}{separator}[[Category:{}]]\n", rule.replace),
                            1,
                        )
                    }
                }
                RuleOperation::CategoryRemove => replace_bounded(
                    rule.matcher.as_ref().expect("category matcher").regex(),
                    &text,
                    "",
                    true,
                    remaining,
                    &[],
                )?,
                RuleOperation::CategoryReplace => {
                    let replacement = format!(
                        "[[Category:{}${{sort}}]]${{newline}}",
                        rule.replace.replace('$', "$$")
                    );
                    replace_bounded(
                        rule.matcher.as_ref().expect("category matcher").regex(),
                        &text,
                        &replacement,
                        true,
                        remaining,
                        &[],
                    )?
                }
                RuleOperation::Replace => replace_bounded(
                    rule.matcher.as_ref().expect("replacement matcher").regex(),
                    &text,
                    &rule.replace,
                    rule.replace_all,
                    remaining,
                    &masked_ranges(&text, &rule.mask),
                )?,
                RuleOperation::TemplateRename
                | RuleOperation::TemplateParameterSet
                | RuleOperation::TemplateParameterRemove => {
                    return Err(RuleError::StructuralRequiresWiki);
                }
            };
            if next.len() > MAX_OUTPUT_BYTES {
                return Err(RuleError::OutputTooLarge);
            }
            if count > remaining {
                return Err(RuleError::TooManyReplacements);
            }
            text = next;
            counts.push(count);
            changed.push(count > 0);
            total_replacements += count;
        }

        Ok(TransformationResult {
            text,
            counts,
            changed,
            total_replacements,
        })
    }
}

impl Matcher {
    fn regex(&self) -> &Regex {
        match self {
            Self::Plain(regex) | Self::Regex(regex) => regex,
        }
    }
}

fn replace_bounded(
    regex: &Regex,
    text: &str,
    replacement: &str,
    replace_all: bool,
    remaining_replacements: usize,
    masked: &[std::ops::Range<usize>],
) -> Result<(String, usize), RuleError> {
    let mut output = String::with_capacity(text.len().min(MAX_OUTPUT_BYTES));
    let mut last_end = 0;
    let mut count = 0;

    for captures in regex.captures_iter(text) {
        if count == remaining_replacements {
            return Err(RuleError::TooManyReplacements);
        }
        let matched = captures
            .get(0)
            .expect("regex captures always include group 0");
        if overlaps(masked, matched.start(), matched.end()) {
            continue;
        }
        output.push_str(&text[last_end..matched.start()]);
        captures.expand(replacement, &mut output);
        if output.len() > MAX_OUTPUT_BYTES {
            return Err(RuleError::OutputTooLarge);
        }
        last_end = matched.end();
        count += 1;
        if !replace_all {
            break;
        }
    }

    if count == 0 {
        return Ok((text.to_owned(), 0));
    }
    output.push_str(&text[last_end..]);
    if output.len() > MAX_OUTPUT_BYTES {
        return Err(RuleError::OutputTooLarge);
    }
    Ok((output, count))
}

fn compile_rule(
    index: usize,
    rule: &ReplacementRule,
    category_prefix: &str,
) -> Result<CompiledRule, RuleError> {
    if matches!(
        rule.operation,
        RuleOperation::Replace | RuleOperation::CategoryRemove | RuleOperation::CategoryReplace
    ) && rule.find.is_empty()
    {
        return Err(RuleError::EmptyPattern { index });
    }
    if matches!(
        rule.operation,
        RuleOperation::Append
            | RuleOperation::Prepend
            | RuleOperation::CategoryAdd
            | RuleOperation::CategoryReplace
    ) && rule.replace.is_empty()
    {
        return Err(RuleError::EmptyValue { index });
    }
    if rule.find.len() > MAX_PATTERN_BYTES {
        return Err(RuleError::PatternTooLarge { index });
    }
    if rule.replace.len() > MAX_REPLACEMENT_BYTES {
        return Err(RuleError::ReplacementTooLarge { index });
    }
    if matches!(
        rule.operation,
        RuleOperation::CategoryRemove | RuleOperation::CategoryReplace
    ) && !valid_category_name(&rule.find)
        || matches!(
            rule.operation,
            RuleOperation::CategoryAdd | RuleOperation::CategoryReplace
        ) && !valid_category_name(&rule.replace)
    {
        return Err(RuleError::InvalidCategory { index });
    }
    if matches!(
        rule.operation,
        RuleOperation::TemplateRename
            | RuleOperation::TemplateParameterSet
            | RuleOperation::TemplateParameterRemove
    ) && (rule.find.trim().is_empty()
        || rule.find.contains(['{', '}', '|', '\n', '\r'])
        || matches!(
            rule.operation,
            RuleOperation::TemplateParameterSet | RuleOperation::TemplateParameterRemove
        ) && (rule.parameter.trim().is_empty()
            || rule.parameter.len() > 1024
            || rule.parameter.contains(['=', '|', '{', '}', '\n', '\r']))
        || rule.operation == RuleOperation::TemplateRename && rule.replace.trim().is_empty())
    {
        return Err(RuleError::InvalidTemplate { index });
    }

    let pattern = match rule.operation {
        RuleOperation::Replace if rule.regex => rule.find.clone(),
        RuleOperation::Replace => regex::escape(&rule.find),
        RuleOperation::CategoryAdd => category_pattern(&rule.replace, category_prefix),
        RuleOperation::CategoryRemove | RuleOperation::CategoryReplace => {
            category_pattern(&rule.find, category_prefix)
        }
        RuleOperation::Append
        | RuleOperation::Prepend
        | RuleOperation::TemplateRename
        | RuleOperation::TemplateParameterSet
        | RuleOperation::TemplateParameterRemove => String::new(),
    };
    let matcher = if pattern.is_empty() {
        None
    } else {
        Some(
            RegexBuilder::new(&pattern)
                .case_insensitive(rule.operation != RuleOperation::Replace || !rule.case_sensitive)
                .multi_line(rule.multiline)
                .dot_matches_new_line(rule.dot_matches_new_line)
                .size_limit(MAX_OUTPUT_BYTES)
                .build()
                .map_err(|error| RuleError::InvalidRegex {
                    index,
                    message: error.to_string(),
                })?,
        )
    };
    let matcher = matcher.map(|regex| {
        if rule.regex && rule.operation == RuleOperation::Replace {
            Matcher::Regex(regex)
        } else {
            Matcher::Plain(regex)
        }
    });

    Ok(CompiledRule {
        enabled: rule.enabled,
        operation: rule.operation,
        group_name: (!rule.group_name.is_empty()).then(|| rule.group_name.clone()),
        mask: rule.mask.clone(),
        replace: if matches!(
            rule.operation,
            RuleOperation::CategoryAdd | RuleOperation::CategoryReplace
        ) {
            category_name(&rule.replace).to_owned()
        } else if rule.regex && rule.operation == RuleOperation::Replace {
            rule.replace.clone()
        } else {
            rule.replace.replace('$', "$$")
        },
        replace_all: rule.replace_all,
        matcher,
    })
}

fn category_pattern(name: &str, category_prefix: &str) -> String {
    let name = category_name(name).replace('_', " ");
    let title = regex::escape(&name).replace(' ', "[ _]");
    let prefix = regex::escape(category_prefix);
    format!(
        r"(?im)^[ \t]*\[\[[ \t]*(?:Category|{prefix})[ \t]*:[ \t]*{title}[ \t]*(?P<sort>\|[^\]\r\n]*)?\]\][ \t]*(?P<newline>\r?\n|$)"
    )
}

fn category_name(name: &str) -> &str {
    let name = name.trim();
    match name.split_once(':') {
        Some((prefix, rest)) if prefix.eq_ignore_ascii_case("Category") => rest.trim(),
        _ => name,
    }
}

fn valid_category_name(name: &str) -> bool {
    let name = category_name(name);
    !name.is_empty() && !name.contains(['[', ']', '|', '\n', '\r', '#'])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(find: &str, replace: &str) -> ReplacementRule {
        ReplacementRule {
            find: find.to_owned(),
            replace: replace.to_owned(),
            ..ReplacementRule::default()
        }
    }

    #[test]
    fn rules_run_in_visible_order() {
        let engine = RuleEngine::compile(&[plain("a", "b"), plain("b", "c")]).unwrap();
        let result = engine.apply("a").unwrap();
        assert_eq!(result.text, "c");
        assert_eq!(result.counts, vec![1, 1]);
    }

    #[test]
    fn plain_replacement_treats_dollar_as_literal() {
        let engine = RuleEngine::compile(&[plain("price", "$5")]).unwrap();
        assert_eq!(engine.apply("price").unwrap().text, "$5");
    }

    #[test]
    fn empty_replacement_deletes_matches() {
        let result = RuleEngine::compile(&[plain(" obsolete", "")])
            .unwrap()
            .apply("keep obsolete text")
            .unwrap();
        assert_eq!(result.text, "keep text");
        assert_eq!(result.counts, vec![1]);
        assert_eq!(result.total_replacements, 1);
    }

    #[test]
    fn disabled_rule_does_not_change_text() {
        let mut rule = plain("a", "b");
        rule.enabled = false;
        let result = RuleEngine::compile(&[rule]).unwrap().apply("a").unwrap();
        assert_eq!(result.text, "a");
        assert_eq!(result.counts, vec![0]);
    }

    #[test]
    fn append_and_prepend_run_in_visible_order() {
        let mut append = plain("", " end");
        append.operation = RuleOperation::Append;
        let mut prepend = plain("", "start ");
        prepend.operation = RuleOperation::Prepend;
        let result = RuleEngine::compile(&[append, prepend])
            .unwrap()
            .apply("middle")
            .unwrap();
        assert_eq!(result.text, "start middle end");
        assert_eq!(result.counts, vec![1, 1]);
    }

    #[test]
    fn category_operations_preserve_sort_key_and_avoid_duplicates() {
        let mut replace = plain("Old", "New");
        replace.operation = RuleOperation::CategoryReplace;
        let mut add = plain("", "New");
        add.operation = RuleOperation::CategoryAdd;
        let result = RuleEngine::compile(&[replace, add])
            .unwrap()
            .apply("Body\n[[Category:Old|Sort]]\n")
            .unwrap();
        assert_eq!(result.text, "Body\n[[Category:New|Sort]]\n");
        assert_eq!(result.counts, vec![1, 0]);
    }

    #[test]
    fn category_remove_only_removes_matching_category_line() {
        let mut remove = plain("Old", "");
        remove.operation = RuleOperation::CategoryRemove;
        let result = RuleEngine::compile(&[remove])
            .unwrap()
            .apply("Body\n[[Category:Old]]\n[[Category:Other]]\n")
            .unwrap();
        assert_eq!(result.text, "Body\n[[Category:Other]]\n");
    }

    #[test]
    fn category_operation_recognizes_localized_namespace() {
        let mut remove = plain("Старе", "");
        remove.operation = RuleOperation::CategoryRemove;
        let result = RuleEngine::compile_with_category_prefix(&[remove], "Категорія")
            .unwrap()
            .apply("Body\n[[Категорія:Старе]]\n[[Файл:Старе]]\n")
            .unwrap();
        assert_eq!(result.text, "Body\n[[Файл:Старе]]\n");
    }

    #[test]
    fn conditional_group_uses_original_page_text() {
        let mut first = plain("trigger", "gone");
        first.group_name = "group".to_owned();
        first.group_contains = "trigger".to_owned();
        let mut second = plain("other", "changed");
        second.group_name = "group".to_owned();
        second.group_contains = "trigger".to_owned();
        let engine = RuleEngine::compile(&[first, second]).unwrap();
        let result = engine.apply("trigger other").unwrap();
        assert_eq!(result.text, "gone changed");
        assert_eq!(result.counts, vec![1, 1]);
        assert_eq!(engine.apply("other").unwrap().counts, vec![0, 0]);
    }

    #[test]
    fn conflicting_group_conditions_are_rejected() {
        let mut first = plain("a", "b");
        first.group_name = "group".to_owned();
        first.group_contains = "a".to_owned();
        let mut second = first.clone();
        second.group_contains = "other".to_owned();
        assert!(matches!(
            RuleEngine::compile(&[first, second]),
            Err(RuleError::ConflictingGroup { .. })
        ));
    }

    #[test]
    fn replacement_skips_masked_comments_and_link_targets() {
        let mut rule = plain("old", "new");
        rule.mask.comments = true;
        rule.mask.link_targets = true;
        let result = RuleEngine::compile(&[rule])
            .unwrap()
            .apply("old <!-- old --> [[old|old]]")
            .unwrap();
        assert_eq!(result.text, "new <!-- old --> [[old|new]]");
        assert_eq!(result.counts, vec![2]);
    }
}
