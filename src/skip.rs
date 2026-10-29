use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use crate::rules::{MAX_OUTPUT_BYTES, MAX_PATTERN_BYTES};

/// Conditions that skip a page automatically, with the reason recorded in the queue.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct SkipOptions {
    pub no_change: bool,
    pub redirects: bool,
    pub protected: bool,
    pub outside_namespaces: bool,
    pub contains: String,
    pub not_contains: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub min_replacements: usize,
    pub max_replacements: usize,
}

impl Default for SkipOptions {
    fn default() -> Self {
        Self {
            no_change: true,
            redirects: false,
            protected: false,
            outside_namespaces: true,
            contains: String::new(),
            not_contains: String::new(),
            regex: false,
            case_sensitive: true,
            min_replacements: 0,
            max_replacements: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    Missing,
    Redirect,
    Protected,
    Namespace,
    BotExclusion,
    Contains,
    NotContains,
    NoChange,
    TooFewReplacements,
    TooManyReplacements,
}

impl SkipReason {
    pub fn message_key(self) -> &'static str {
        match self {
            Self::Missing => "findsmith-api-skip-missing",
            Self::Redirect => "findsmith-api-skip-redirect",
            Self::Protected => "findsmith-api-skip-protected",
            Self::Namespace => "findsmith-api-skip-namespace",
            Self::BotExclusion => "findsmith-api-skip-bot-exclusion",
            Self::Contains => "findsmith-api-skip-contains",
            Self::NotContains => "findsmith-api-skip-not-contains",
            Self::NoChange => "findsmith-api-skip-no-change",
            Self::TooFewReplacements => "findsmith-api-skip-too-few-replacements",
            Self::TooManyReplacements => "findsmith-api-skip-too-many-replacements",
        }
    }
}

/// What is known about a page before the rules run.
pub struct PageFacts<'a> {
    pub namespace: i64,
    pub redirect: bool,
    pub editable: bool,
    pub content: &'a str,
}

pub struct SkipMatcher {
    options: SkipOptions,
    namespaces: Vec<i64>,
    contains: Option<Regex>,
    not_contains: Option<Regex>,
}

impl SkipMatcher {
    /// `namespaces` is the project filter; an empty list allows every namespace.
    pub fn compile(options: &SkipOptions, namespaces: &[i64]) -> Result<Self, String> {
        Ok(Self {
            contains: content_pattern(&options.contains, options)?,
            not_contains: content_pattern(&options.not_contains, options)?,
            options: options.clone(),
            namespaces: namespaces.to_vec(),
        })
    }

    pub fn before_rules(&self, page: &PageFacts<'_>) -> Option<SkipReason> {
        if self.options.outside_namespaces
            && !self.namespaces.is_empty()
            && !self.namespaces.contains(&page.namespace)
        {
            return Some(SkipReason::Namespace);
        }
        if self.options.redirects && page.redirect {
            return Some(SkipReason::Redirect);
        }
        if self.options.protected && !page.editable {
            return Some(SkipReason::Protected);
        }
        if self
            .contains
            .as_ref()
            .is_some_and(|pattern| pattern.is_match(page.content))
        {
            return Some(SkipReason::Contains);
        }
        if self
            .not_contains
            .as_ref()
            .is_some_and(|pattern| !pattern.is_match(page.content))
        {
            return Some(SkipReason::NotContains);
        }
        None
    }

    pub fn after_rules(
        &self,
        original: &str,
        proposed: &str,
        replacements: usize,
    ) -> Option<SkipReason> {
        if self.options.no_change && original == proposed {
            return Some(SkipReason::NoChange);
        }
        if self.options.min_replacements > 0 && replacements < self.options.min_replacements {
            return Some(SkipReason::TooFewReplacements);
        }
        if self.options.max_replacements > 0 && replacements > self.options.max_replacements {
            return Some(SkipReason::TooManyReplacements);
        }
        None
    }
}

/// Checks options before they are saved, so invalid patterns are reported in the
/// form instead of while reviewing.
pub fn validate(options: &SkipOptions) -> Result<(), String> {
    if options.max_replacements > 0 && options.min_replacements > options.max_replacements {
        return Err("findsmith-api-skip-replacement-bounds".to_owned());
    }
    SkipMatcher::compile(options, &[]).map(|_| ())
}

fn content_pattern(value: &str, options: &SkipOptions) -> Result<Option<Regex>, String> {
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > MAX_PATTERN_BYTES {
        return Err("findsmith-api-skip-pattern-too-long".to_owned());
    }
    let pattern = if options.regex {
        value.to_owned()
    } else {
        regex::escape(value)
    };
    RegexBuilder::new(&pattern)
        .case_insensitive(!options.case_sensitive)
        .size_limit(MAX_OUTPUT_BYTES)
        .build()
        .map(Some)
        .map_err(|error| {
            crate::localization::required_with_args(
                "findsmith-api-skip-pattern-invalid",
                &[&error.to_string()],
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(content: &str) -> PageFacts<'_> {
        PageFacts {
            namespace: 0,
            redirect: false,
            editable: true,
            content,
        }
    }

    #[test]
    fn defaults_skip_unchanged_pages_and_pages_outside_the_filter() {
        let matcher = SkipMatcher::compile(&SkipOptions::default(), &[0]).unwrap();
        assert_eq!(matcher.before_rules(&page("text")), None);
        let talk = PageFacts {
            namespace: 1,
            ..page("text")
        };
        assert_eq!(matcher.before_rules(&talk), Some(SkipReason::Namespace));
        let redirect = PageFacts {
            redirect: true,
            editable: false,
            ..page("text")
        };
        assert_eq!(matcher.before_rules(&redirect), None);
        assert_eq!(
            matcher.after_rules("same", "same", 0),
            Some(SkipReason::NoChange)
        );
        assert_eq!(matcher.after_rules("old", "new", 1), None);
    }

    #[test]
    fn content_conditions_support_plain_text_and_regex() {
        let options = SkipOptions {
            contains: "{{In use".to_owned(),
            case_sensitive: false,
            ..SkipOptions::default()
        };
        let matcher = SkipMatcher::compile(&options, &[]).unwrap();
        assert_eq!(
            matcher.before_rules(&page("{{in use}} body")),
            Some(SkipReason::Contains)
        );
        let options = SkipOptions {
            not_contains: r"\[\[Category:".to_owned(),
            regex: true,
            ..SkipOptions::default()
        };
        let matcher = SkipMatcher::compile(&options, &[]).unwrap();
        assert_eq!(
            matcher.before_rules(&page("no categories")),
            Some(SkipReason::NotContains)
        );
        assert_eq!(matcher.before_rules(&page("[[Category:A]]")), None);
    }

    #[test]
    fn redirect_protection_and_replacement_bounds_are_optional() {
        let options = SkipOptions {
            redirects: true,
            protected: true,
            min_replacements: 2,
            max_replacements: 3,
            ..SkipOptions::default()
        };
        let matcher = SkipMatcher::compile(&options, &[]).unwrap();
        let redirect = PageFacts {
            redirect: true,
            ..page("#REDIRECT [[A]]")
        };
        assert_eq!(matcher.before_rules(&redirect), Some(SkipReason::Redirect));
        let protected = PageFacts {
            editable: false,
            ..page("text")
        };
        assert_eq!(
            matcher.before_rules(&protected),
            Some(SkipReason::Protected)
        );
        assert_eq!(
            matcher.after_rules("a", "b", 1),
            Some(SkipReason::TooFewReplacements)
        );
        assert_eq!(matcher.after_rules("a", "b", 2), None);
        assert_eq!(
            matcher.after_rules("a", "b", 4),
            Some(SkipReason::TooManyReplacements)
        );
    }

    #[test]
    fn invalid_options_are_rejected() {
        let inverted = SkipOptions {
            min_replacements: 5,
            max_replacements: 2,
            ..SkipOptions::default()
        };
        assert!(validate(&inverted).is_err());
        let bad_regex = SkipOptions {
            contains: "(".to_owned(),
            regex: true,
            ..SkipOptions::default()
        };
        assert!(validate(&bad_regex).is_err());
        let old_state: SkipOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(old_state, SkipOptions::default());
    }
}
