//! RegExTypoFix: AutoWikiBrowser's typo fixing, ported from
//! `WikiFunctions/Parse/RegExTypoFix.cs` and `HideText.HideMore`.
//!
//! Rules come from a wiki page such as `Wikipedia:AutoWikiBrowser/Typos`, one
//! `<Typo word="..." find="..." replace="..." />` per entry. The find and
//! replace values are .NET regular expressions, handled by
//! [`crate::dotnet_regex`].
//!
//! Differences from AWB:
//!
//! - A rule that cannot be compiled is dropped and reported instead of
//!   rejecting the whole list. Duplicate rules keep the first copy.
//! - A rule whose search exceeds the backtracking budget is skipped for that page
//!   and reported, instead of hanging until a timeout.

use std::{
    collections::HashSet,
    sync::{LazyLock, OnceLock},
};

use serde::{Deserialize, Serialize};

use crate::dotnet_regex::{DotNetRegex, MatchError, Replacement};

use hide::Hider;

mod hide;
pub mod lists;

pub const MAX_TYPO_RULES: usize = 20_000;

/// Project and review settings for typo fixing.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct TypoOptions {
    /// Run the wiki's AutoWikiBrowser typo list after the rules.
    pub enabled: bool,
    /// AWB "Skip if no typo fixed".
    pub skip_if_none: bool,
}

/// AWB `TyposDownloader.TypoRegex`.
const TYPO_ENTRY: &str = r#"<(?:Typo)?\s+(?:word="(.*?)"\s+)?find="(.*?)"\s+replace="(.*?)"\s*/?>"#;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypoEntry {
    pub word: Option<String>,
    pub find: String,
    pub replace: String,
}

/// A rule that was not loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypoProblem {
    pub word: Option<String>,
    pub find: String,
    pub message: String,
}

/// Per-rule counts for one page, like AWB `TypoStat`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypoStat {
    pub word: Option<String>,
    pub find: String,
    pub replace: String,
    /// Matches found on the page.
    pub total: usize,
    /// Matches whose replacement equals the matched text.
    pub self_matches: usize,
}

#[derive(Clone, Debug, Default)]
pub struct TypoFixResult {
    pub text: String,
    /// True when no typo changed the page.
    pub no_change: bool,
    /// AWB-style summary without the "typos fixed: " tag, for example
    /// `acheive → achieve, teh → the (2)`.
    pub summary: String,
    pub stats: Vec<TypoStat>,
    /// Rules skipped on this page because they exceeded the backtracking budget.
    pub errors: Vec<String>,
}

/// Site settings that affect typo fixing.
#[derive(Clone, Debug)]
pub struct TypoSite {
    /// Local and canonical names of namespace 14.
    pub category_names: Vec<String>,
    /// Local, canonical and alias names of namespace 6.
    pub file_names: Vec<String>,
    /// Arabic, Egyptian Arabic and Persian use `، ` between summary items.
    pub language: String,
    pub right_to_left: bool,
}

impl Default for TypoSite {
    fn default() -> Self {
        Self {
            category_names: vec!["Category".to_owned()],
            file_names: vec!["File".to_owned(), "Image".to_owned()],
            language: "en".to_owned(),
            right_to_left: false,
        }
    }
}

/// Parses a typo list page. Entries with an empty find value are ignored.
pub fn parse_typo_list(text: &str) -> Vec<TypoEntry> {
    static ENTRY: LazyLock<DotNetRegex> =
        LazyLock::new(|| DotNetRegex::new(TYPO_ENTRY).expect("valid typo entry regex"));
    let mut entries = Vec::new();
    let Ok(ranges) = ENTRY.find_ranges(text) else {
        return entries;
    };
    for range in ranges {
        let entry = &text[range];
        let group = |number| ENTRY.first_group(entry, number).ok().flatten();
        let (Some(find), Some(replace)) = (group(2), group(3)) else {
            continue;
        };
        entries.push(TypoEntry {
            word: group(1),
            find,
            replace,
        });
    }
    entries
}

struct TypoRule {
    word: Option<String>,
    find: String,
    replace_source: String,
    regex: DotNetRegex,
    replacement: Replacement,
    /// AWB skips a grouped rule when it matches inside a wikilink target.
    /// Compiled on first use.
    link_check: Option<OnceLock<Result<DotNetRegex, String>>>,
}

impl TypoRule {
    fn label(&self) -> &str {
        self.word.as_deref().unwrap_or(&self.find)
    }

    fn link_check(&self) -> Option<Result<&DotNetRegex, &str>> {
        let cell = self.link_check.as_ref()?;
        let compiled = cell.get_or_init(|| {
            let pattern = format!(
                r"\[\[[^[\]\n\|]*?{}[^\[\]\r\n\|]*?(?<!\[\[[A-Z]?[a-z-]{{2,}}:[^[\]\n]+)(?:\]\]|\|)",
                self.find
            );
            DotNetRegex::new(&pattern).map_err(|error| error.to_string())
        });
        Some(compiled.as_ref().map_err(String::as_str))
    }
}

struct TypoGroup {
    size: usize,
    allow: Option<DotNetRegex>,
    disallow: Option<DotNetRegex>,
    rules: Vec<TypoRule>,
    /// Per batch of `size` rules, a linear-time regex that matches wherever any
    /// rule of the batch can match; `None` when it could not be built.
    batches: Option<Vec<Option<regex::Regex>>>,
}

impl TypoGroup {
    fn new(size: usize, allow: Option<&str>, disallow: Option<&str>) -> Self {
        let compile = |pattern: &str| DotNetRegex::new(pattern).expect("valid AWB group regex");
        Self {
            size,
            allow: allow.map(compile),
            disallow: disallow.map(compile),
            rules: Vec::new(),
            batches: None,
        }
    }

    fn is_suitable(&self, find: &str) -> bool {
        if let Some(allow) = &self.allow
            && !allow.is_match(find).unwrap_or(false)
        {
            return false;
        }
        if let Some(disallow) = &self.disallow
            && disallow.is_match(find).unwrap_or(false)
        {
            return false;
        }
        true
    }

    fn make_batches(&mut self) {
        if self.size <= 1 {
            return;
        }
        // AWB joins a batch's patterns as `prefix(a|b|...)postfix` and tries its
        // rules only when that matches, which amounts to "some rule of the batch
        // matches". Joining the rules' lookaround-free prefilters matches a
        // superset of that, so results are unchanged while the backtracking
        // regexes run less often.
        let mut batches = Vec::new();
        for chunk in self.rules.chunks(self.size) {
            let alternatives: Option<Vec<String>> = chunk
                .iter()
                .map(|rule| {
                    rule.regex
                        .prefilter_pattern()
                        .map(|pattern| format!("(?:{pattern})"))
                })
                .collect();
            batches.push(alternatives.and_then(|alternatives| {
                regex::RegexBuilder::new(&alternatives.join("|"))
                    .size_limit(8 << 20)
                    .build()
                    .ok()
            }));
        }
        self.batches = Some(batches);
    }

    /// AWB `TypoGroup.FixTypos`.
    #[allow(clippy::too_many_arguments)]
    fn fix_typos(
        &self,
        text: &mut String,
        summary: &mut String,
        title: &str,
        original: &str,
        site: &TypoSite,
        stats: &mut Vec<TypoStat>,
        errors: &mut Vec<String>,
    ) {
        stats.clear();
        match &self.batches {
            Some(batches) => {
                for (batch, rules) in batches.iter().zip(self.rules.chunks(self.size)) {
                    // Without a batch prefilter every rule is tried.
                    if batch.as_ref().is_some_and(|batch| !batch.is_match(text)) {
                        continue;
                    }
                    for rule in rules {
                        // A rule that cannot match makes no change: skip its link check.
                        if !rule.regex.may_match(text) {
                            continue;
                        }
                        let in_link_target = match rule.link_check() {
                            Some(Ok(check)) => match check.is_match(original) {
                                Ok(found) => found,
                                Err(error) => {
                                    errors.push(rule_error(rule, &error));
                                    continue;
                                }
                            },
                            Some(Err(error)) => {
                                errors.push(format!("{}: {error}", rule.label()));
                                continue;
                            }
                            None => false,
                        };
                        if !in_link_target {
                            fix_typo(text, summary, rule, title, site, stats, errors);
                        }
                    }
                }
            }
            None => {
                for rule in &self.rules {
                    fix_typo(text, summary, rule, title, site, stats, errors);
                }
            }
        }
    }
}

fn rule_error(rule: &TypoRule, error: &MatchError) -> String {
    format!("{}: {error}", rule.label())
}

/// AWB `TypoGroup.FixTypo`.
fn fix_typo(
    text: &mut String,
    summary: &mut String,
    rule: &TypoRule,
    title: &str,
    site: &TypoSite,
    stats: &mut Vec<TypoStat>,
    errors: &mut Vec<String>,
) {
    // Don't apply the typo if it matches the page title.
    match rule.regex.is_match(title) {
        Ok(true) => return,
        Ok(false) => {}
        Err(error) => {
            errors.push(rule_error(rule, &error));
            return;
        }
    }
    let (next, matches) = match rule.regex.replace_all(text, &rule.replacement, usize::MAX) {
        Ok(result) => result,
        Err(error) => {
            errors.push(rule_error(rule, &error));
            return;
        }
    };
    if matches.is_empty() {
        return;
    }
    *text = next;
    let mut changed = 0;
    for found in &matches {
        if found.replacement != found.matched {
            changed += 1;
            if changed == 1 {
                if !summary.is_empty() {
                    summary.push_str(summary_separator(site));
                }
                summary.push_str(&found.matched);
                summary.push_str(if site.right_to_left { " ← " } else { " → " });
                summary.push_str(&found.replacement);
            }
        }
    }
    if changed > 1 {
        summary.push_str(&format!(" ({changed})"));
    }
    stats.push(TypoStat {
        word: rule.word.clone(),
        find: rule.find.clone(),
        replace: rule.replace_source.clone(),
        total: matches.len(),
        self_matches: matches.len() - changed,
    });
}

fn summary_separator(site: &TypoSite) -> &'static str {
    if matches!(site.language.as_str(), "ar" | "arz" | "fa") {
        "، "
    } else {
        ", "
    }
}

/// A compiled typo list.
pub struct TypoList {
    groups: Vec<TypoGroup>,
    problems: Vec<TypoProblem>,
    count: usize,
    hider: Hider,
    site: TypoSite,
}

impl TypoList {
    pub fn compile(entries: &[TypoEntry], site: TypoSite) -> Self {
        // Group order and settings are AWB's `RegExTypoFix.MakeRegexes`.
        let mut groups = vec![
            TypoGroup::new(20, Some(r"^\\b(\(\[[A-M].*)\\b$"), Some(r"[^\\]\\\d")),
            TypoGroup::new(19, Some(r"^\\b(\(\[[N-Z].*)\\b$"), Some(r"[^\\]\\\d")),
            TypoGroup::new(18, Some(r"^\\b((\(|\[)?[A-Za-z].*)"), Some(r"[^\\]\\\d")),
            TypoGroup::new(5, None, Some(r"[^\\]\\\d")),
            TypoGroup::new(1, None, None),
        ];
        let mut problems = Vec::new();
        let mut seen = HashSet::new();
        let mut count = 0;
        for entry in entries {
            let problem = |message: String| TypoProblem {
                word: entry.word.clone(),
                find: entry.find.clone(),
                message,
            };
            if count == MAX_TYPO_RULES {
                problems.push(problem(format!("more than {MAX_TYPO_RULES} rules")));
                continue;
            }
            if !seen.insert(entry.find.as_str()) {
                problems.push(problem("duplicate typo rule".to_owned()));
                continue;
            }
            let regex = match DotNetRegex::new(&entry.find) {
                Ok(regex) => regex,
                Err(error) => {
                    problems.push(problem(error.to_string()));
                    continue;
                }
            };
            let Some(group) = groups
                .iter_mut()
                .find(|group| group.is_suitable(&entry.find))
            else {
                continue;
            };
            let link_check = (group.size > 1).then(OnceLock::new);
            let replacement = regex.replacement(&entry.replace);
            group.rules.push(TypoRule {
                word: entry.word.clone(),
                find: entry.find.clone(),
                replace_source: entry.replace.clone(),
                regex,
                replacement,
                link_check,
            });
            count += 1;
        }
        for group in &mut groups {
            group.make_batches();
        }
        Self {
            groups,
            problems,
            count,
            hider: Hider::new(&site),
            site,
        }
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn problems(&self) -> &[TypoProblem] {
        &self.problems
    }

    /// AWB `RegExTypoFix.PerformTypoFixes`.
    pub fn fix(&self, page_text: &str, title: &str) -> Result<TypoFixResult, MatchError> {
        let unchanged = || TypoFixResult {
            text: page_text.to_owned(),
            no_change: true,
            ..TypoFixResult::default()
        };
        if self.count == 0 || has_ignore_marker(page_text) {
            return Ok(unchanged());
        }
        let mut hidden = self.hider.hide_more(page_text)?;
        // Leave out trailing whitespace and hidden tokens so 4,000 regexes don't scan them.
        let tail_start = tail_start(&hidden.text);
        let tail = hidden.text.split_off(tail_start);
        let original_hidden = hidden.text.clone();

        let mut text = hidden.text.clone();
        let mut summary = String::new();
        let mut errors = Vec::new();
        let mut group_stats: Vec<Vec<TypoStat>> = vec![Vec::new(); self.groups.len()];

        // AWB runs each group on the same input in parallel, keeps the first group
        // that changed anything, and re-runs later changing groups on the result.
        let mut parallel = Vec::with_capacity(self.groups.len());
        for (group, stats) in self.groups.iter().zip(group_stats.iter_mut()) {
            let mut group_text = original_hidden.clone();
            let mut group_summary = String::new();
            group.fix_typos(
                &mut group_text,
                &mut group_summary,
                title,
                page_text,
                &self.site,
                stats,
                &mut errors,
            );
            parallel.push((group_text, group_summary));
        }
        for ((group, stats), (group_text, group_summary)) in
            self.groups.iter().zip(group_stats.iter_mut()).zip(parallel)
        {
            if group_summary.is_empty() {
                continue;
            }
            if summary.is_empty() {
                summary = group_summary;
                text = group_text;
            } else {
                group.fix_typos(
                    &mut text,
                    &mut summary,
                    title,
                    page_text,
                    &self.site,
                    stats,
                    &mut errors,
                );
            }
        }

        let no_change = original_hidden == text;
        text.push_str(&tail);
        let text = if no_change {
            page_text.to_owned()
        } else {
            hidden.add_back(&text)
        };
        errors.sort();
        errors.dedup();
        Ok(TypoFixResult {
            text,
            no_change,
            summary: summary.trim().to_owned(),
            stats: group_stats.into_iter().flatten().collect(),
            errors,
        })
    }
}

/// AWB `RegExTypoFix.IgnoreRegex`: pages marked with sic are left alone.
fn has_ignore_marker(text: &str) -> bool {
    ["(sic)", "[sic]", "[''sic'']", "{{sic}}", "spellfixno"]
        .iter()
        .any(|marker| text.contains(marker))
}

/// Start of AWB `RemoveTail`: `(\s|\n|\r|\*|#|:|⌊⌊⌊⌊M?\d*⌋⌋⌋⌋)+$`.
fn tail_start(text: &str) -> usize {
    let mut end = text.len();
    loop {
        let rest = &text[..end];
        if let Some(ch) = rest.chars().next_back()
            && (ch.is_whitespace() || matches!(ch, '*' | '#' | ':'))
        {
            end -= ch.len_utf8();
            continue;
        }
        if let Some(stripped) = rest.strip_suffix("⌋⌋⌋⌋") {
            let digits = stripped.trim_end_matches(|c: char| c.is_ascii_digit());
            let digits = digits.strip_suffix('M').unwrap_or(digits);
            if let Some(before) = digits.strip_suffix("⌊⌊⌊⌊") {
                end = before.len();
                continue;
            }
        }
        return end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(rules: &[(&str, &str)]) -> TypoList {
        let entries: Vec<_> = rules
            .iter()
            .map(|(find, replace)| TypoEntry {
                word: None,
                find: (*find).to_owned(),
                replace: (*replace).to_owned(),
            })
            .collect();
        TypoList::compile(&entries, TypoSite::default())
    }

    #[test]
    fn parses_typo_list_entries() {
        let text = r#"
<Typo word="Achieve" find="\b(a)cheive(s|d)?\b" replace="$1chieve$2" />
<!--<Typo word="Off" find="x" replace="y" />-->
<Typo find="\bteh\b" replace="the"/>
"#;
        let entries = parse_typo_list(text);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].word.as_deref(), Some("Achieve"));
        assert_eq!(entries[0].replace, "$1chieve$2");
        assert_eq!(entries[2].word, None);
    }

    #[test]
    fn fixes_typos_and_builds_awb_summary() {
        let typos = list(&[(r"\b(a)cheive(s|d)?\b", "$1chieve$2"), (r"\bteh\b", "the")]);
        let result = typos
            .fix("I acheived teh goal and teh rest.", "Page")
            .unwrap();
        assert_eq!(result.text, "I achieved the goal and the rest.");
        assert!(!result.no_change);
        assert_eq!(result.summary, "acheived → achieved, teh → the (2)");
        assert_eq!(result.stats.len(), 2);
    }

    #[test]
    fn skips_templates_links_quotes_and_sic() {
        let typos = list(&[(r"\bteh\b", "the")]);
        let text = "{{cite|teh}} [[x|teh]] ''teh'' \"teh\" <!-- teh --> http://teh.example teh";
        assert_eq!(
            typos.fix(text, "Page").unwrap().text,
            "{{cite|teh}} [[x|the]] ''teh'' \"teh\" <!-- teh --> http://teh.example the"
        );
        assert!(typos.fix("teh (sic)", "Page").unwrap().no_change);
    }

    #[test]
    fn rule_matching_title_is_skipped() {
        let typos = list(&[(r"\bteh\b", "the")]);
        assert!(typos.fix("teh word", "Teh teh").unwrap().no_change);
    }

    #[test]
    fn short_description_content_is_fixed() {
        let typos = list(&[(r"\bteh\b", "the")]);
        assert_eq!(
            typos
                .fix("{{Short description|teh thing}}\nBody", "Page")
                .unwrap()
                .text,
            "{{Short description|the thing}}\nBody"
        );
    }

    #[test]
    fn grouped_rule_is_skipped_when_page_links_it() {
        let typos = list(&[(r"\b(B)eleive\b", "$1elieve")]);
        assert!(
            typos
                .fix("Beleive it. See [[Beleive (song)]].", "Page")
                .unwrap()
                .no_change
        );
        assert_eq!(
            typos.fix("Beleive it.", "Page").unwrap().text,
            "Believe it."
        );
    }

    #[test]
    fn invalid_and_duplicate_rules_are_reported() {
        let typos = list(&[(r"\bok\b", "OK"), (r"(?<a-b>x)", "y"), (r"\bok\b", "Ok")]);
        assert_eq!(typos.len(), 1);
        assert_eq!(typos.problems().len(), 2);
    }

    #[test]
    fn self_matches_are_counted_but_not_summarised() {
        let typos = list(&[(r"\b(colou?r)\b", "$1")]);
        let result = typos.fix("colour", "Page").unwrap();
        assert!(result.no_change);
        assert_eq!(result.stats[0].self_matches, 1);
        assert!(result.summary.is_empty());
    }

    #[test]
    fn tail_is_preserved() {
        let typos = list(&[(r"\bteh\b", "the")]);
        assert_eq!(
            typos.fix("teh\n\n[[Category:X]]\n", "Page").unwrap().text,
            "the\n\n[[Category:X]]\n"
        );
    }

    /// Compiles a full typo list and times it. Run with
    /// `cargo test full_typo_list -- --ignored --nocapture` after saving
    /// `Wikipedia:AutoWikiBrowser/Typos` (action=raw) to
    /// `tests/fixtures/typos-enwiki.txt`, or set `FINDSMITH_TYPOS_FILE`.
    #[test]
    #[ignore = "needs a downloaded typo list"]
    fn full_typo_list() {
        let path = std::env::var("FINDSMITH_TYPOS_FILE")
            .unwrap_or_else(|_| "tests/fixtures/typos-enwiki.txt".to_owned());
        let text = std::fs::read_to_string(&path).expect("typo list file");
        let entries = parse_typo_list(&text);
        let started = std::time::Instant::now();
        let typos = TypoList::compile(&entries, TypoSite::default());
        println!(
            "{} entries, {} rules compiled in {:?}, {} problems",
            entries.len(),
            typos.len(),
            started.elapsed(),
            typos.problems().len()
        );
        for problem in typos.problems() {
            println!(
                "  {}: {} -- {}",
                problem.word.as_deref().unwrap_or("-"),
                problem.find,
                problem.message
            );
        }
        let article = format!(
            "{}\n\nIt is a apple. They acheived teh goal in the the end, recieving praise.\n",
            include_str!("../../help.txt").repeat(4)
        );
        let started = std::time::Instant::now();
        let result = typos.fix(&article, "Sample").expect("typo fixing");
        println!(
            "fixed {} bytes in {:?}: {}",
            article.len(),
            started.elapsed(),
            result.summary
        );
        for error in &result.errors {
            println!("  runtime: {error}");
        }
        assert!(
            typos.problems().len() * 100 < entries.len(),
            "more than 1% of rules failed"
        );
    }
}
