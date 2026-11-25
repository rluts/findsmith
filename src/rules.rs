use std::{error::Error, fmt};

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
        let write_message = |formatter: &mut fmt::Formatter<'_>, key: &str, args: &[&str]| {
            formatter.write_str(
                &crate::localization::message_with_args(key, args)
                    .expect("English rule error message exists"),
            )
        };
        match self {
            Self::TooManyRules => write!(formatter, "At most {MAX_RULES} rules are allowed."),
            Self::EmptyPattern { index } => write_message(
                formatter,
                "findsmith-api-rule-find-required",
                &[&(index + 1).to_string()],
            ),
            Self::EmptyValue { index } => write_message(
                formatter,
                "findsmith-api-rule-value-required",
                &[&(index + 1).to_string()],
            ),
            Self::InvalidCategory { index } => write_message(
                formatter,
                "findsmith-api-rule-category-invalid",
                &[&(index + 1).to_string()],
            ),
            Self::InvalidTemplate { index } => write_message(
                formatter,
                "findsmith-api-rule-template-invalid",
                &[&(index + 1).to_string()],
            ),
            Self::StructuralRequiresWiki => {
                formatter.write_str("Template rules require the selected wiki's Parsoid service.")
            }
            Self::PatternTooLarge { index } => write!(
                formatter,
                "The find value in rule {} is too large.",
                index + 1
            ),
            Self::ReplacementTooLarge { index } => write!(
                formatter,
                "The replacement value in rule {} is too large.",
                index + 1
            ),
            Self::InvalidRegex { index, message } => write_message(
                formatter,
                "findsmith-api-rule-regex-invalid",
                &[&(index + 1).to_string(), message],
            ),
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
}

struct CompiledRule {
    enabled: bool,
    operation: RuleOperation,
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
    /// For each replace rule, the first match it actually changed, if any.
    pub samples: Vec<Option<ReplacementSample>>,
    pub total_replacements: usize,
}

/// The first match a replace rule changed, for the edit summary. Matches
/// whose replacement equals the matched text are not counted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplacementSample {
    pub before: String,
    pub after: String,
    pub edits: usize,
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
        let rules = rules
            .iter()
            .enumerate()
            .map(|(index, rule)| compile_rule(index, rule, category_prefix))
            .collect::<Result<_, _>>()?;
        Ok(Self { rules })
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
        let mut samples = Vec::with_capacity(self.rules.len());
        let mut total_replacements = 0;

        for rule in &self.rules {
            if !rule.enabled {
                counts.push(0);
                changed.push(false);
                samples.push(None);
                continue;
            }
            let remaining = MAX_TOTAL_REPLACEMENTS - total_replacements;
            let mut sample = None;
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
                RuleOperation::CategoryRemove => {
                    let (next, count, _) = replace_bounded(
                        rule.matcher.as_ref().expect("category matcher").regex(),
                        &text,
                        "",
                        true,
                        remaining,
                        &[],
                    )?;
                    (next, count)
                }
                RuleOperation::CategoryReplace => {
                    let replacement = format!(
                        "[[Category:{}${{sort}}]]${{newline}}",
                        rule.replace.replace('$', "$$")
                    );
                    let (next, count, _) = replace_bounded(
                        rule.matcher.as_ref().expect("category matcher").regex(),
                        &text,
                        &replacement,
                        true,
                        remaining,
                        &[],
                    )?;
                    (next, count)
                }
                RuleOperation::Replace => {
                    let (next, count, first) = replace_bounded(
                        rule.matcher.as_ref().expect("replacement matcher").regex(),
                        &text,
                        &rule.replace,
                        rule.replace_all,
                        remaining,
                        &masked_ranges(&text, &rule.mask),
                    )?;
                    sample = first;
                    (next, count)
                }
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
            samples.push(sample);
            total_replacements += count;
        }

        Ok(TransformationResult {
            text,
            counts,
            changed,
            samples,
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
) -> Result<(String, usize, Option<ReplacementSample>), RuleError> {
    let mut output = String::with_capacity(text.len().min(MAX_OUTPUT_BYTES));
    let mut last_end = 0;
    let mut count = 0;
    let mut sample: Option<ReplacementSample> = None;

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
        let expanded_start = output.len();
        captures.expand(replacement, &mut output);
        let expanded = &output[expanded_start..];
        if expanded != matched.as_str() {
            match &mut sample {
                Some(sample) => sample.edits += 1,
                None => {
                    sample = Some(ReplacementSample {
                        before: matched.as_str().to_owned(),
                        after: expanded.to_owned(),
                        edits: 1,
                    });
                }
            }
        }
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
        return Ok((text.to_owned(), 0, None));
    }
    output.push_str(&text[last_end..]);
    if output.len() > MAX_OUTPUT_BYTES {
        return Err(RuleError::OutputTooLarge);
    }
    Ok((output, count, sample))
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
        mask: rule.mask.clone(),
        replace: if matches!(
            rule.operation,
            RuleOperation::CategoryAdd | RuleOperation::CategoryReplace
        ) {
            category_name(&rule.replace).to_owned()
        } else if rule.operation == RuleOperation::Replace && !rule.regex {
            // Plain replacements still go through regex expansion.
            rule.replace.replace('$', "$$")
        } else {
            // Regex replacements expand `$` on purpose; other operations insert the text as is.
            rule.replace.clone()
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
    fn samples_record_first_real_change_and_its_count() {
        let rules = [ReplacementRule {
            find: "(a+)".to_owned(),
            replace: "aa".to_owned(),
            regex: true,
            ..Default::default()
        }];
        let result = RuleEngine::compile(&rules)
            .unwrap()
            .apply("aa a aaa")
            .unwrap();
        assert_eq!(result.counts, [3]);
        assert_eq!(
            result.samples,
            [Some(ReplacementSample {
                before: "a".to_owned(),
                after: "aa".to_owned(),
                edits: 2,
            })]
        );
        let unchanged = RuleEngine::compile(&rules).unwrap().apply("aa").unwrap();
        assert_eq!(unchanged.samples, [None]);
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
    fn append_and_prepend_keep_dollar_signs() {
        let mut append = plain("", " costs $5");
        append.operation = RuleOperation::Append;
        let mut prepend = plain("", "$$ ");
        prepend.operation = RuleOperation::Prepend;
        let result = RuleEngine::compile(&[append, prepend])
            .unwrap()
            .apply("it")
            .unwrap();
        assert_eq!(result.text, "$$ it costs $5");
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
