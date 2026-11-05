use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use crate::{
    masking,
    rules::{MAX_OUTPUT_BYTES, MAX_PATTERN_BYTES},
};

/// Conditions that skip a page automatically, with the reason recorded in the queue.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct SkipOptions {
    pub no_change: bool,
    pub in_use_tpl: bool,
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
            in_use_tpl: true,
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
    InUse,
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
            Self::InUse => "findsmith-api-skip-in-use",
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
    in_use: Option<Regex>,
}

impl SkipMatcher {
    /// `namespaces` is the project filter; an empty list allows every namespace.
    /// `in_use_templates` holds full titles, such as `Template:In use`.
    pub fn compile(
        options: &SkipOptions,
        namespaces: &[i64],
        in_use_templates: &[String],
    ) -> Result<Self, String> {
        Ok(Self {
            in_use: if options.in_use_tpl {
                template_pattern(in_use_templates)?
            } else {
                None
            },
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
            .in_use
            .as_ref()
            .is_some_and(|pattern| transcludes(pattern, page.content))
        {
            return Some(SkipReason::InUse);
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
    SkipMatcher::compile(options, &[], &[]).map(|_| ())
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
    build_pattern(&pattern, !options.case_sensitive).map(Some)
}

fn build_pattern(pattern: &str, case_insensitive: bool) -> Result<Regex, String> {
    RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        .size_limit(MAX_OUTPUT_BYTES)
        .build()
        .map_err(|error| {
            crate::localization::required_with_args(
                "findsmith-api-skip-pattern-invalid",
                &[&error.to_string()],
            )
        })
}

/// Matches a call to any of the given templates the way MediaWiki resolves the name:
/// the namespace is optional and caseless, the first letter is caseless, and spaces
/// and underscores are interchangeable.
fn template_pattern(titles: &[String]) -> Result<Option<Regex>, String> {
    let mut namespaces = vec!["Template".to_owned()];
    let mut names = Vec::new();
    for title in titles {
        let Some((namespace, name)) = title.split_once(':') else {
            continue;
        };
        let mut letters = name.trim().chars();
        let Some(first) = letters.next() else {
            continue;
        };
        namespaces.push(namespace.trim().to_owned());
        names.push(format!(
            "(?i:{}){}",
            regex::escape(&first.to_string()),
            title_words(letters.as_str())
        ));
    }
    if names.is_empty() {
        return Ok(None);
    }
    namespaces.sort();
    namespaces.dedup();
    let namespaces: Vec<String> = namespaces.iter().map(|name| title_words(name)).collect();
    let pattern = format!(
        r"\{{\{{\s*(?::?\s*(?i:{})\s*:)?[\s_]*(?:{})[\s_]*(?:\||\}}\}})",
        namespaces.join("|"),
        names.join("|")
    );
    build_pattern(&pattern, false).map(Some)
}

fn title_words(value: &str) -> String {
    value
        .split(' ')
        .map(regex::escape)
        .collect::<Vec<_>>()
        .join(r"[\s_]+")
}

/// Whether a match lies in parsed wikitext, not in a comment or `<nowiki>`.
fn transcludes(pattern: &Regex, content: &str) -> bool {
    if !pattern.is_match(content) {
        return false;
    }
    let unparsed = masking::unparsed_ranges(content);
    pattern
        .find_iter(content)
        .any(|found| !masking::overlaps(&unparsed, found.start(), found.end()))
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
        let matcher = SkipMatcher::compile(&SkipOptions::default(), &[0], &[]).unwrap();
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
        let matcher = SkipMatcher::compile(&options, &[], &[]).unwrap();
        assert_eq!(
            matcher.before_rules(&page("{{in use}} body")),
            Some(SkipReason::Contains)
        );
        let options = SkipOptions {
            not_contains: r"\[\[Category:".to_owned(),
            regex: true,
            ..SkipOptions::default()
        };
        let matcher = SkipMatcher::compile(&options, &[], &[]).unwrap();
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
        let matcher = SkipMatcher::compile(&options, &[], &[]).unwrap();
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

    fn in_use_matcher(enabled: bool) -> SkipMatcher {
        let options = SkipOptions {
            in_use_tpl: enabled,
            ..SkipOptions::default()
        };
        let templates = [
            "Шаблон:Редагую".to_owned(),
            "Шаблон:У розробці".to_owned(),
            "Шаблон:Inuse".to_owned(),
        ];
        SkipMatcher::compile(&options, &[], &templates).unwrap()
    }

    #[test]
    fn in_use_templates_match_as_mediawiki_resolves_them() {
        let matcher = in_use_matcher(true);
        for content in [
            "{{Редагую}} text",
            "{{редагую|2026-11-04}} text",
            "{{ Редагую\n| user = A }}",
            "{{У розробці}}",
            "{{у_розробці}}",
            "{{У  розробці}}",
            "{{шаблон:редагую}}",
            "{{Template:Inuse}}",
            "{{:Шаблон:Inuse}}",
        ] {
            assert_eq!(
                matcher.before_rules(&page(content)),
                Some(SkipReason::InUse),
                "{content}"
            );
        }
    }

    #[test]
    fn in_use_templates_ignore_other_text() {
        let matcher = in_use_matcher(true);
        for content in [
            "Редагую text",
            "{{Редагую сторінку}}",
            "{{РЕДАГУЮ}}",
            "{{Користувач:Редагую}}",
            "<!-- {{Редагую}} --> text",
            "<nowiki>{{Редагую}}</nowiki> text",
        ] {
            assert_eq!(matcher.before_rules(&page(content)), None, "{content}");
        }
        assert_eq!(
            matcher.before_rules(&page("<!-- {{Inuse}} --> {{Inuse}}")),
            Some(SkipReason::InUse)
        );
    }

    #[test]
    fn in_use_check_is_optional_and_needs_known_templates() {
        let disabled = in_use_matcher(false);
        assert_eq!(disabled.before_rules(&page("{{Редагую}}")), None);
        let unknown = SkipMatcher::compile(&SkipOptions::default(), &[], &[]).unwrap();
        assert_eq!(unknown.before_rules(&page("{{Редагую}}")), None);
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
