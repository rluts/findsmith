//! AWB `HideText.HideMore(articleText, hideOnlyTargetOfWikilink: true)` with
//! `HideText(hideExternalLinks: true, leaveMetaHeadings: false, hideImages: true)`:
//! replaces markup that typo rules must not touch with `⌊⌊⌊⌊M0⌋⌋⌋⌋` tokens.

use std::{collections::HashSet, ops::Range, sync::LazyLock};

use super::TypoSite;
use crate::dotnet_regex::{self, DotNetOptions, DotNetRegex, MatchError};

pub(super) struct Hider {
    short_description: DotNetRegex,
    category: DotNetRegex,
    images: DotNetRegex,
}

pub(super) struct Hidden {
    pub(super) text: String,
    tokens: Vec<String>,
}

static SIMPLE_REF: LazyLock<DotNetRegex> = LazyLock::new(|| net(r"<ref>[^<>]+</ref>"));
static EXTERNAL_LINKS_HTTP: LazyLock<DotNetRegex> = LazyLock::new(|| {
    net_with(
        r#"(https?)://(?:[\w\._\-~!/\*"'\(\):;@&=+$,\\\?%#\[\]]+?(?=}})|[\w\._\-~!/\*"'\(\):;@&=+$,\\\?%#\[\]]*)|(\[https?)://.*?\]"#,
        DotNetOptions {
            ignore_case: true,
            ..DotNetOptions::default()
        },
    )
});
static EXTERNAL_LINKS: LazyLock<DotNetRegex> = LazyLock::new(|| {
    net_with(
        r#"(?:(https?|ftp|mailto|irc|gopher|telnet|nntp|worldwind|news|svn):)?//(?:[\w\._\-~!/\*"'\(\):;@&=+$,\\\?%#\[\]]+?(?=}})|[\w\._\-~!/\*"'\(\):;@&=+$,\\\?%#\[\]]*)|\[(?:(https?|ftp|mailto|irc|gopher|telnet|nntp|worldwind|news|svn):)?//.*?\]"#,
        DotNetOptions {
            ignore_case: true,
            ..DotNetOptions::default()
        },
    )
});
static HEADINGS: LazyLock<DotNetRegex> = LazyLock::new(|| {
    net_with(
        r"^={1,6} *(.*?) *={1,6}(?: *⌊⌊⌊⌊\d{1,4}⌋⌋⌋⌋| *<!--.*?-->|< *[Bb][Rr] */ *>)?\s*$",
        DotNetOptions {
            multiline: true,
            ..DotNetOptions::default()
        },
    )
});
static COMMENTS: LazyLock<DotNetRegex> = LazyLock::new(|| {
    net_with(
        r"<!--.*?-->",
        DotNetOptions {
            singleline: true,
            ..DotNetOptions::default()
        },
    )
});
static INDENTED_TEXT: LazyLock<DotNetRegex> = LazyLock::new(|| {
    net_with(
        r"^:.*",
        DotNetOptions {
            multiline: true,
            ..DotNetOptions::default()
        },
    )
});
static WIKI_LINKS_PLUS_WORD: LazyLock<DotNetRegex> =
    LazyLock::new(|| net(r"\[\[[^\[\]\n]+\]\](\w+)"));
static GALLERY_TAG: LazyLock<DotNetRegex> = LazyLock::new(|| {
    net_with(
        r"< *gallery\b([^>]*?)>([\s\S]*?)</ *gallery *>",
        DotNetOptions {
            ignore_case: true,
            ..DotNetOptions::default()
        },
    )
});
static IMAGE_TO_BAR: LazyLock<DotNetRegex> = LazyLock::new(|| {
    net_with(
        r"^.+?\.[a-zA-Z]{3,4}\s*(?=\||\r\n)",
        DotNetOptions {
            multiline: true,
            ..DotNetOptions::default()
        },
    )
});
static WIKI_LINK: LazyLock<DotNetRegex> = LazyLock::new(|| net(r"\[\[([^\]\|]+)(?:\]\]|\|)"));
static UNTEMPLATED_QUOTES: LazyLock<DotNetRegex> = LazyLock::new(|| {
    net_with(
        r#"(?<=[^\w]|^)["«»‘’“”‛‟‹›“”„‘’`’“‘”].{1,2000}?["«»‘’“”‛‟‹›“”„‘’`’“‘”](?=[^\w])"#,
        DotNetOptions {
            singleline: true,
            ..DotNetOptions::default()
        },
    )
});
static ITALICS: LazyLock<DotNetRegex> =
    LazyLock::new(|| net(r"(?<!')'{2}((?:[^']+|[^'].*?[^'])?)'{2}(?!')"));
static P_STYLES: LazyLock<DotNetRegex> = LazyLock::new(|| {
    net_with(
        r"<p style\s*=\s*[^<>]+>.*?<\s*/p>",
        DotNetOptions {
            ignore_case: true,
            singleline: true,
            ..DotNetOptions::default()
        },
    )
});
static ANY_TAG: LazyLock<DotNetRegex> = LazyLock::new(|| net(r"<([^<>]+)>"));
static ALL_TAGS_OPEN: LazyLock<DotNetRegex> = LazyLock::new(|| {
    net_with(
        r"<\s*(nowiki|pre|math|chem|timeline|code|source|cite|syntaxhighlight|blockquote|poem|imagemap|includeonly|onlyinclude|noinclude|hiero|score)[^<>]*>",
        DotNetOptions {
            ignore_case: true,
            ..DotNetOptions::default()
        },
    )
});
static REF_SELF_CLOSING: LazyLock<DotNetRegex> = LazyLock::new(|| {
    net_with(
        r"\A<\s*ref\s+(?:name|group|details)\s*=\s*[^<>]*?/\s*>",
        DotNetOptions {
            ignore_case: true,
            singleline: true,
            ..DotNetOptions::default()
        },
    )
});

fn net(pattern: &str) -> DotNetRegex {
    net_with(pattern, DotNetOptions::default())
}

fn net_with(pattern: &str, options: DotNetOptions) -> DotNetRegex {
    DotNetRegex::with_options(pattern, options, dotnet_regex::DEFAULT_BACKTRACK_LIMIT)
        .unwrap_or_else(|error| panic!("AWB regex {pattern}: {error}"))
}

/// AWB `Variables.NamespacesCaseInsensitive`: `(?i:Name|Alias)\s*:`.
fn namespace_pattern(names: &[String]) -> String {
    let mut seen = HashSet::new();
    let names: Vec<String> = names
        .iter()
        .map(|name| name.trim().replace('_', " "))
        .filter(|name| !name.is_empty() && seen.insert(name.to_lowercase()))
        .map(|name| dotnet_regex::escape(&name).replace(r"\ ", "[ _]"))
        .collect();
    format!(r"(?i:{})\s*:", names.join("|"))
}

impl Hider {
    pub(super) fn new(site: &TypoSite) -> Self {
        let category = namespace_pattern(&site.category_names);
        let file = namespace_pattern(&site.file_names);
        Self {
            short_description: net(
                r"^\{\{[\s_]*(?::?[\s_]*(?i:Template)[\s_]*:[\s_]*\s*)?([Ss]hort[_\s]+description)[\s_]*(?:<!--[^>]*?-->\s*|⌊⌊⌊⌊M?\d+⌋⌋⌋⌋\s*)?(?:\||\}\})",
            ),
            category: net(&format!(
                r"\[\[[\s_]*{category}[ _]*([^[\]|\r\n]*?)[ _]*(?:\|([^\|\]]*))?[ _]*\]\]"
            )),
            images: net(&format!(
                r#"(?:\[\[\s*)?{file}[ \%\!"$&'’\(\)\*,\-.\/0-9:;=\?@\w\\\^_`~\x80-\xFF\+–]+\.[a-zA-Z]{{3,4}}\b(?:\s*(?:\]\]|\|))?"#
            )),
        }
    }

    pub(super) fn hide_more(&self, text: &str) -> Result<Hidden, MatchError> {
        let mut hidden = Hidden {
            text: text.to_owned(),
            tokens: Vec::new(),
        };
        // Templates, except that only the name of {{Short description}} is hidden
        // so its content gets typo fixing.
        let source = std::mem::take(&mut hidden.text);
        let mut output = String::with_capacity(source.len());
        let mut last = 0;
        for range in nested_templates(&source) {
            output.push_str(&source[last..range.start]);
            let template = &source[range.clone()];
            match self.short_description.first_group(template, 1)? {
                Some(name) => {
                    let ranges: Vec<_> = template
                        .match_indices(name.as_str())
                        .map(|(i, m)| i..i + m.len())
                        .collect();
                    let replaced = hidden.replace(template, ranges);
                    output.push_str(&replaced);
                }
                None => {
                    let token = hidden.token(template);
                    output.push_str(&token);
                }
            }
            last = range.end;
        }
        output.push_str(&source[last..]);
        hidden.text = output;

        hidden.apply_ranges(all_tags)?;
        hidden.apply(&SIMPLE_REF)?;
        hidden.apply(&EXTERNAL_LINKS_HTTP)?;
        if hidden.text.contains("//") {
            hidden.apply(&EXTERNAL_LINKS)?;
        }
        hidden.apply(&HEADINGS)?;
        hidden.apply(&COMMENTS)?;
        hidden.apply(&INDENTED_TEXT)?;
        hidden.apply(&WIKI_LINKS_PLUS_WORD)?;
        hidden.apply(&self.category)?;
        hidden.apply_ranges(refs)?;

        let tags: Vec<String> = ANY_TAG
            .find_ranges(&hidden.text)?
            .into_iter()
            .map(|range| {
                hidden.text[range.start + 1..range.end - 1]
                    .trim()
                    .to_lowercase()
            })
            .collect();
        let has_gallery = tags.iter().any(|tag| tag.contains("gallery"));
        if has_gallery {
            let galleries = GALLERY_TAG.find_ranges(&hidden.text)?;
            let mut output = String::with_capacity(hidden.text.len());
            let mut last = 0;
            let source = std::mem::take(&mut hidden.text);
            for range in galleries {
                output.push_str(&source[last..range.start]);
                let gallery = &source[range.clone()];
                let ranges = IMAGE_TO_BAR.find_ranges(gallery)?;
                output.push_str(&hidden.replace(gallery, ranges));
                last = range.end;
            }
            output.push_str(&source[last..]);
            hidden.text = output;
        }
        hidden.apply(&WIKI_LINK)?;
        if has_gallery || hidden.text.contains("[[") {
            hidden.apply(&self.images)?;
        }
        hidden.apply(&UNTEMPLATED_QUOTES)?;
        hidden.apply(&ITALICS)?;
        if tags.iter().any(|tag| tag.contains("p style")) {
            hidden.apply(&P_STYLES)?;
        }
        Ok(hidden)
    }
}

impl Hidden {
    fn token(&mut self, original: &str) -> String {
        let token = format!("⌊⌊⌊⌊M{}⌋⌋⌋⌋", self.tokens.len());
        self.tokens.push(original.to_owned());
        token
    }

    fn replace(&mut self, text: &str, ranges: Vec<Range<usize>>) -> String {
        let mut output = String::with_capacity(text.len());
        let mut last = 0;
        for range in ranges {
            output.push_str(&text[last..range.start]);
            let token = self.token(&text[range.clone()]);
            output.push_str(&token);
            last = range.end;
        }
        output.push_str(&text[last..]);
        output
    }

    fn apply(&mut self, regex: &DotNetRegex) -> Result<(), MatchError> {
        let ranges = regex.find_ranges(&self.text)?;
        if !ranges.is_empty() {
            let text = std::mem::take(&mut self.text);
            self.text = self.replace(&text, ranges);
        }
        Ok(())
    }

    fn apply_ranges(
        &mut self,
        find: fn(&str) -> Result<Vec<Range<usize>>, MatchError>,
    ) -> Result<(), MatchError> {
        let ranges = find(&self.text)?;
        if !ranges.is_empty() {
            let text = std::mem::take(&mut self.text);
            self.text = self.replace(&text, ranges);
        }
        Ok(())
    }

    /// AWB `AddBackMore`: restores tokens, repeatedly for nested hiding.
    pub(super) fn add_back(&self, text: &str) -> String {
        let mut text = text.to_owned();
        loop {
            let mut output = String::with_capacity(text.len());
            let mut rest = text.as_str();
            let mut replaced = false;
            while let Some(start) = rest.find("⌊⌊⌊⌊M") {
                let after = &rest[start + "⌊⌊⌊⌊M".len()..];
                let digits =
                    after.len() - after.trim_start_matches(|c: char| c.is_ascii_digit()).len();
                let restored = after[digits..]
                    .strip_prefix("⌋⌋⌋⌋")
                    .and_then(|tail| Some((after[..digits].parse::<usize>().ok()?, tail)))
                    .and_then(|(index, tail)| Some((self.tokens.get(index)?, tail)));
                output.push_str(&rest[..start]);
                match restored {
                    Some((original, tail)) => {
                        output.push_str(original);
                        rest = tail;
                        replaced = true;
                    }
                    None => {
                        output.push_str("⌊⌊⌊⌊M");
                        rest = after;
                    }
                }
            }
            output.push_str(rest);
            text = output;
            if !replaced {
                return text;
            }
        }
    }
}

/// AWB `WikiRegexes.NestedTemplates`:
/// `{{(?>[^\{\}]+|\{(?<DEPTH>)|\}(?<-DEPTH>))*(?(DEPTH)(?!))}}`.
fn nested_templates(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut start = 0;
    while let Some(offset) = text[start..].find("{{") {
        let open = start + offset;
        let mut position = open + 2;
        let mut depth = 0usize;
        while position < bytes.len() {
            match bytes[position] {
                b'{' => depth += 1,
                b'}' if depth > 0 => depth -= 1,
                b'}' => break,
                _ => {}
            }
            position += 1;
        }
        if depth == 0 && text[position..].starts_with("}}") {
            ranges.push(open..position + 2);
            start = position + 2;
        } else {
            start = open + 1;
        }
    }
    ranges
}

/// AWB `WikiRegexes.AllTags`: balanced same-name tags such as `<pre>...</pre>`.
fn all_tags(text: &str) -> Result<Vec<Range<usize>>, MatchError> {
    let mut ranges = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let Some(open) = find_from(&ALL_TAGS_OPEN, text, start)? else {
            break;
        };
        let name = text[open.clone()]
            .trim_start_matches('<')
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect::<String>()
            .to_lowercase();
        let mut position = open.end;
        let mut depth = 0usize;
        let mut end = None;
        while position < text.len() {
            if let Some(length) = tag_at(text, position, &name, true) {
                if depth == 0 {
                    end = Some(position + length);
                    break;
                }
                depth -= 1;
                position += length;
            } else if let Some(length) = tag_at(text, position, &name, false) {
                depth += 1;
                position += length;
            } else {
                position += text[position..].chars().next().map_or(1, char::len_utf8);
            }
        }
        match end {
            Some(end) => {
                ranges.push(open.start..end);
                start = end;
            }
            None => {
                start = open.start + text[open.start..].chars().next().map_or(1, char::len_utf8)
            }
        }
    }
    Ok(ranges)
}

fn find_from(
    regex: &DotNetRegex,
    text: &str,
    start: usize,
) -> Result<Option<Range<usize>>, MatchError> {
    Ok(regex
        .find_ranges(&text[start..])?
        .into_iter()
        .next()
        .map(|range| range.start + start..range.end + start))
}

/// Length of `<\s*name[^<>]*>` (open) or `<\s*/\s*name\b[^<>]*>` (close) at `position`.
fn tag_at(text: &str, position: usize, name: &str, closing: bool) -> Option<usize> {
    let rest = text[position..].strip_prefix('<')?;
    let mut rest = rest.trim_start();
    if closing {
        rest = rest.strip_prefix('/')?.trim_start();
    }
    if rest.len() < name.len()
        || !rest.is_char_boundary(name.len())
        || !rest[..name.len()].eq_ignore_ascii_case(name)
    {
        return None;
    }
    let after = &rest[name.len()..];
    if closing && after.chars().next().is_some_and(is_word_char) {
        return None;
    }
    let close = after.find(['<', '>'])?;
    if after.as_bytes()[close] != b'>' {
        return None;
    }
    let end = text.len() - after.len() + close + 1;
    Some(end - position)
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// AWB `WikiRegexes.Refs`: a self-closing named reference, or `<ref ...>`
/// up to the first `</ref>` when no other opening `<ref>` comes first.
fn refs(text: &str) -> Result<Vec<Range<usize>>, MatchError> {
    refs_in(text, &text.to_ascii_lowercase())
}

fn refs_in(text: &str, lower: &str) -> Result<Vec<Range<usize>>, MatchError> {
    let mut ranges = Vec::new();
    let mut start = 0;
    while let Some(offset) = lower[start..].find('<') {
        let open = start + offset;
        if let Some(found) = REF_SELF_CLOSING.find_ranges(&text[open..])?.first() {
            ranges.push(open..open + found.end);
            start = open + found.end;
            continue;
        }
        let Some(open_length) = ref_open_tag(lower, open, false) else {
            start = open + 1;
            continue;
        };
        let mut position = open + open_length;
        let mut end = None;
        while let Some(offset) = lower[position..].find('<') {
            let tag = position + offset;
            if let Some(length) = ref_close_tag(lower, tag) {
                end = Some(tag + length);
                break;
            }
            if ref_open_tag(lower, tag, true).is_some() {
                break;
            }
            position = tag + 1;
        }
        match end {
            Some(end) => {
                ranges.push(open..end);
                start = end;
            }
            None => start = open + 1,
        }
    }
    Ok(ranges)
}

/// `<\s*ref\b[^<>]*>`, or with `nested` the inner form `<\s*ref\b[^>/]*>`.
fn ref_open_tag(lower: &str, position: usize, nested: bool) -> Option<usize> {
    let rest = lower[position..].strip_prefix('<')?.trim_start();
    let after = rest.strip_prefix("ref")?;
    if after.chars().next().is_some_and(is_word_char) {
        return None;
    }
    let stop = if nested {
        after.find(['>', '/'])?
    } else {
        after.find(['<', '>'])?
    };
    if after.as_bytes()[stop] != b'>' {
        return None;
    }
    Some(lower.len() - after.len() + stop + 1 - position)
}

/// `<\s*/\s*ref\s*>`.
fn ref_close_tag(lower: &str, position: usize) -> Option<usize> {
    let rest = lower[position..].strip_prefix('<')?.trim_start();
    let rest = rest.strip_prefix('/')?.trim_start();
    let rest = rest.strip_prefix("ref")?.trim_start();
    let rest = rest.strip_prefix('>')?;
    Some(lower.len() - rest.len() - position)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_markup_scanners() {
        assert_eq!(nested_templates("a {{x|{{y}}}} {{z"), vec![2..13]);
        assert_eq!(all_tags("<pre>a<pre>b</pre></pre> c").unwrap(), vec![0..24]);
        assert_eq!(
            refs(r#"x<ref name="a" /> <ref>t</ref>"#).unwrap(),
            vec![1..17, 18..30]
        );
        assert_eq!(refs("<ref>a<ref>b</ref>").unwrap(), vec![6..18]);
    }
}
