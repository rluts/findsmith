use std::{error::Error, fmt};

use regex::{Regex, RegexBuilder};

use crate::projects::ReplacementRule;

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
}

struct CompiledRule {
    enabled: bool,
    replace: String,
    replace_all: bool,
    matcher: Matcher,
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
        if rules.len() > MAX_RULES {
            return Err(RuleError::TooManyRules);
        }
        let rules = rules
            .iter()
            .enumerate()
            .map(|(index, rule)| compile_rule(index, rule))
            .collect::<Result<_, _>>()?;
        Ok(Self { rules })
    }

    pub fn apply(&self, input: &str) -> Result<TransformationResult, RuleError> {
        if input.len() > MAX_INPUT_BYTES {
            return Err(RuleError::InputTooLarge);
        }
        let mut text = input.to_owned();
        let mut counts = Vec::with_capacity(self.rules.len());
        let mut changed = Vec::with_capacity(self.rules.len());
        let mut total_replacements = 0;

        for rule in &self.rules {
            if !rule.enabled {
                counts.push(0);
                changed.push(false);
                continue;
            }
            let regex = match &rule.matcher {
                Matcher::Plain(regex) | Matcher::Regex(regex) => regex,
            };
            let remaining = MAX_TOTAL_REPLACEMENTS - total_replacements;
            let (next, count) =
                replace_bounded(regex, &text, &rule.replace, rule.replace_all, remaining)?;
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

fn replace_bounded(
    regex: &Regex,
    text: &str,
    replacement: &str,
    replace_all: bool,
    remaining_replacements: usize,
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

fn compile_rule(index: usize, rule: &ReplacementRule) -> Result<CompiledRule, RuleError> {
    if rule.find.is_empty() {
        return Err(RuleError::EmptyPattern { index });
    }
    if rule.find.len() > MAX_PATTERN_BYTES {
        return Err(RuleError::PatternTooLarge { index });
    }
    if rule.replace.len() > MAX_REPLACEMENT_BYTES {
        return Err(RuleError::ReplacementTooLarge { index });
    }

    let pattern = if rule.regex {
        rule.find.clone()
    } else {
        regex::escape(&rule.find)
    };
    let regex = RegexBuilder::new(&pattern)
        .case_insensitive(!rule.case_sensitive)
        .multi_line(rule.multiline)
        .dot_matches_new_line(rule.dot_matches_new_line)
        .size_limit(MAX_OUTPUT_BYTES)
        .build()
        .map_err(|error| RuleError::InvalidRegex {
            index,
            message: error.to_string(),
        })?;
    let matcher = if rule.regex {
        Matcher::Regex(regex)
    } else {
        Matcher::Plain(regex)
    };

    Ok(CompiledRule {
        enabled: rule.enabled,
        replace: if rule.regex {
            rule.replace.clone()
        } else {
            rule.replace.replace('$', "$$")
        },
        replace_all: rule.replace_all,
        matcher,
    })
}
