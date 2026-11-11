//! .NET-compatible regular expressions.
//!
//! AutoWikiBrowser lists such as `Wikipedia:AutoWikiBrowser/Typos` are written
//! for `System.Text.RegularExpressions`. This module parses the .NET dialect,
//! rewrites it into the syntax understood by `fancy-regex` (a backtracking
//! engine that supports lookbehind of any length and backreferences), and
//! expands replacement strings with .NET rules.
//!
//! The rewrite preserves the .NET meaning of constructs whose syntax is shared
//! but whose behaviour differs between the two engines:
//!
//! - `$` without multiline matches at the end *or before a final newline*.
//! - `\Z` matches at the end or before one final newline only.
//! - `\w` is `[\p{L}\p{Mn}\p{Nd}\p{Pc}]`.
//! - `\<`, `\>`, `\{` and other escaped punctuation are literals (in
//!   fancy-regex `\<` and `\>` are word boundaries).
//! - A `{` that does not start a valid quantifier is a literal.
//! - `[` inside a character class is a literal, and `[a-z-[aeiou]]` is class
//!   subtraction.
//! - Unnamed groups are numbered before named groups; `(?n)` makes unnamed
//!   groups non-capturing; `(?x)` ignores pattern whitespace and `#` comments.
//! - Multi-digit `\NN` is a backreference only when that group exists,
//!   otherwise an octal escape.
//! - In replacements, `$1a` is group 1 followed by `a`, and `$N` for a missing
//!   group is a literal.
//!
//! Known differences, deliberately accepted:
//!
//! - `\b` and `\B` use the Unicode word definition of Rust's `regex` crate, which
//!   also treats spacing marks (`Mc`), enclosing marks (`Me`), letter numbers
//!   (`Nl`) and a few alphabetic symbols as word characters.
//! - Case-insensitive matching uses Unicode simple case folding rather than the
//!   culture-sensitive .NET tables.
//! - Captures taken inside a variable-length lookbehind may bind differently in
//!   ambiguous cases, because .NET matches lookbehind right to left.
//! - `\G`, balancing groups `(?<a-b>...)` and `RegexOptions.RightToLeft` are
//!   rejected.

use std::{collections::HashMap, fmt};

use fancy_regex::{Captures, Regex, RegexBuilder};

/// Upper bound on backtracking steps per search, replacing the .NET match timeout.
pub const DEFAULT_BACKTRACK_LIMIT: usize = 1_000_000;

const WORD_CLASS: &str = r"\p{L}\p{Mn}\p{Nd}\p{Pc}";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DotNetOptions {
    pub ignore_case: bool,
    pub multiline: bool,
    pub singleline: bool,
    pub explicit_capture: bool,
    pub ignore_pattern_whitespace: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DotNetRegexError {
    pub position: Option<usize>,
    pub message: String,
}

impl fmt::Display for DotNetRegexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.position {
            Some(position) => write!(formatter, "{} (at offset {position})", self.message),
            None => formatter.write_str(&self.message),
        }
    }
}

impl std::error::Error for DotNetRegexError {}

/// A runtime failure, normally an exhausted backtracking budget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchError(pub String);

impl fmt::Display for MatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for MatchError {}

impl From<fancy_regex::Error> for MatchError {
    fn from(error: fancy_regex::Error) -> Self {
        Self(error.to_string())
    }
}

#[derive(Debug)]
pub struct DotNetRegex {
    regex: Regex,
    /// A linear-time `regex` that matches wherever this regex can match (and
    /// sometimes elsewhere), used to skip the backtracking engine cheaply.
    prefilter: Option<regex::Regex>,
    groups: GroupTable,
}

#[derive(Clone, Debug, Default)]
struct GroupTable {
    /// .NET group number to fancy-regex group index. Always contains 0.
    slots: HashMap<u32, usize>,
    names: HashMap<String, u32>,
    /// Highest .NET group number, used by `$+`.
    last: u32,
}

impl DotNetRegex {
    pub fn new(pattern: &str) -> Result<Self, DotNetRegexError> {
        Self::with_options(pattern, DotNetOptions::default(), DEFAULT_BACKTRACK_LIMIT)
    }

    pub fn with_options(
        pattern: &str,
        options: DotNetOptions,
        backtrack_limit: usize,
    ) -> Result<Self, DotNetRegexError> {
        let translation = translate(pattern, options)?;
        let regex = RegexBuilder::new(&translation.pattern)
            .backtrack_limit(backtrack_limit)
            .build()
            .map_err(|error| DotNetRegexError {
                position: None,
                message: error.to_string(),
            })?;
        let prefilter = regex::RegexBuilder::new(&translation.prefilter)
            .size_limit(1 << 20)
            .build()
            .ok();
        Ok(Self {
            regex,
            prefilter,
            groups: translation.groups,
        })
    }

    pub fn is_match(&self, text: &str) -> Result<bool, MatchError> {
        if !self.may_match(text) {
            return Ok(false);
        }
        Ok(self.regex.is_match(text)?)
    }

    /// False only when the regex certainly does not match `text`.
    pub fn may_match(&self, text: &str) -> bool {
        self.prefilter
            .as_ref()
            .is_none_or(|prefilter| prefilter.is_match(text))
    }

    /// The prefilter's `regex` syntax: a pattern that matches wherever this
    /// regex matches. Lookarounds are dropped and backreferences match anything.
    pub fn prefilter_pattern(&self) -> Option<&str> {
        self.prefilter.as_ref().map(regex::Regex::as_str)
    }

    /// Byte ranges of all non-overlapping matches, like .NET `Regex.Matches`.
    pub fn find_ranges(&self, text: &str) -> Result<Vec<std::ops::Range<usize>>, MatchError> {
        self.regex
            .find_iter(text)
            .map(|found| found.map(|m| m.range()).map_err(MatchError::from))
            .collect()
    }

    /// The text of .NET group `number` in the first match, if any.
    pub fn first_group(&self, text: &str, number: u32) -> Result<Option<String>, MatchError> {
        let Some(captures) = self.regex.captures(text)? else {
            return Ok(None);
        };
        Ok(self
            .groups
            .slots
            .get(&number)
            .and_then(|&slot| captures.get(slot))
            .map(|m| m.as_str().to_owned()))
    }

    /// Parses a .NET replacement pattern against this regex's groups.
    pub fn replacement(&self, replacement: &str) -> Replacement {
        Replacement::parse(replacement, &self.groups)
    }

    /// Like .NET `Regex.Replace(text, replacement)`. Returns the new text and the
    /// matches that were replaced, in order.
    pub fn replace_all(
        &self,
        text: &str,
        replacement: &Replacement,
        max_matches: usize,
    ) -> Result<(String, Vec<ReplacedMatch>), MatchError> {
        let mut output = String::with_capacity(text.len());
        let mut matches = Vec::new();
        let mut last_end = 0;
        if !self.may_match(text) {
            return Ok((text.to_owned(), matches));
        }
        for captures in self.regex.captures_iter(text) {
            let captures = captures?;
            let whole = captures.get(0).expect("group 0 always participates");
            if matches.len() == max_matches {
                return Err(MatchError("too many matches".to_owned()));
            }
            output.push_str(&text[last_end..whole.start()]);
            let start = output.len();
            replacement.expand_into(&captures, &self.groups, text, &mut output);
            matches.push(ReplacedMatch {
                start: whole.start(),
                end: whole.end(),
                matched: whole.as_str().to_owned(),
                replacement: output[start..].to_owned(),
            });
            last_end = whole.end();
        }
        if matches.is_empty() {
            return Ok((text.to_owned(), matches));
        }
        output.push_str(&text[last_end..]);
        Ok((output, matches))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplaceError {
    Match(MatchError),
    TooManyReplacements,
}

impl From<MatchError> for ReplaceError {
    fn from(error: MatchError) -> Self {
        Self::Match(error)
    }
}

impl DotNetRegex {
    /// Replaces matches for which `keep(start, end)` is true: all of them, or only
    /// the first kept one. Fails once more than `max_replacements` would be made.
    /// Returns the new text and the number of replacements.
    pub fn replace_where(
        &self,
        text: &str,
        replacement: &Replacement,
        replace_all: bool,
        max_replacements: usize,
        mut keep: impl FnMut(usize, usize) -> bool,
    ) -> Result<(String, usize), ReplaceError> {
        if !self.may_match(text) {
            return Ok((text.to_owned(), 0));
        }
        let mut output = String::with_capacity(text.len());
        let mut last_end = 0;
        let mut count = 0;
        for captures in self.regex.captures_iter(text) {
            let captures = captures.map_err(MatchError::from)?;
            let whole = captures.get(0).expect("group 0 always participates");
            if !keep(whole.start(), whole.end()) {
                continue;
            }
            if count == max_replacements {
                return Err(ReplaceError::TooManyReplacements);
            }
            output.push_str(&text[last_end..whole.start()]);
            replacement.expand_into(&captures, &self.groups, text, &mut output);
            last_end = whole.end();
            count += 1;
            if !replace_all {
                break;
            }
        }
        if count == 0 {
            return Ok((text.to_owned(), 0));
        }
        output.push_str(&text[last_end..]);
        Ok((output, count))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplacedMatch {
    pub start: usize,
    pub end: usize,
    pub matched: String,
    pub replacement: String,
}

/// A parsed .NET replacement pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replacement {
    parts: Vec<ReplacementPart>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ReplacementPart {
    Literal(String),
    Group(u32),
    Prefix,
    Suffix,
    LastGroup,
    Input,
}

impl Replacement {
    fn parse(source: &str, groups: &GroupTable) -> Self {
        let mut parts = Vec::new();
        let mut literal = String::new();
        let chars: Vec<char> = source.chars().collect();
        let mut index = 0;
        let push = |parts: &mut Vec<ReplacementPart>, literal: &mut String, part| {
            if !literal.is_empty() {
                parts.push(ReplacementPart::Literal(std::mem::take(literal)));
            }
            parts.push(part);
        };
        while index < chars.len() {
            let ch = chars[index];
            index += 1;
            if ch != '$' || index == chars.len() {
                literal.push(ch);
                continue;
            }
            // Mirrors RegexParser.ScanDollar: anything unrecognised leaves `$` literal.
            match chars[index] {
                '$' => {
                    literal.push('$');
                    index += 1;
                }
                '&' => {
                    push(&mut parts, &mut literal, ReplacementPart::Group(0));
                    index += 1;
                }
                '`' => {
                    push(&mut parts, &mut literal, ReplacementPart::Prefix);
                    index += 1;
                }
                '\'' => {
                    push(&mut parts, &mut literal, ReplacementPart::Suffix);
                    index += 1;
                }
                '+' => {
                    push(&mut parts, &mut literal, ReplacementPart::LastGroup);
                    index += 1;
                }
                '_' => {
                    push(&mut parts, &mut literal, ReplacementPart::Input);
                    index += 1;
                }
                '{' => {
                    let close = chars[index + 1..].iter().position(|&c| c == '}');
                    let resolved = close.and_then(|close| {
                        let name: String = chars[index + 1..index + 1 + close].iter().collect();
                        resolve_group_name(&name, groups).map(|number| (number, close))
                    });
                    match resolved {
                        Some((number, close)) => {
                            push(&mut parts, &mut literal, ReplacementPart::Group(number));
                            index += close + 2;
                        }
                        None => literal.push('$'),
                    }
                }
                c if c.is_ascii_digit() => {
                    let digits_end = chars[index..]
                        .iter()
                        .position(|c| !c.is_ascii_digit())
                        .map_or(chars.len(), |offset| index + offset);
                    let digits: String = chars[index..digits_end].iter().collect();
                    match digits.parse::<u32>() {
                        Ok(number) if groups.slots.contains_key(&number) => {
                            push(&mut parts, &mut literal, ReplacementPart::Group(number));
                            index = digits_end;
                        }
                        _ => literal.push('$'),
                    }
                }
                _ => literal.push('$'),
            }
        }
        if !literal.is_empty() {
            parts.push(ReplacementPart::Literal(literal));
        }
        Self { parts }
    }

    fn expand_into(
        &self,
        captures: &Captures<'_, str>,
        groups: &GroupTable,
        input: &str,
        output: &mut String,
    ) {
        let group = |number: u32| {
            groups
                .slots
                .get(&number)
                .and_then(|&slot| captures.get(slot))
                .map_or("", |m| m.as_str())
        };
        let whole = captures.get(0).expect("group 0 always participates");
        for part in &self.parts {
            match part {
                ReplacementPart::Literal(text) => output.push_str(text),
                ReplacementPart::Group(number) => output.push_str(group(*number)),
                ReplacementPart::Prefix => output.push_str(&input[..whole.start()]),
                ReplacementPart::Suffix => output.push_str(&input[whole.end()..]),
                ReplacementPart::LastGroup => output.push_str(group(groups.last)),
                ReplacementPart::Input => output.push_str(input),
            }
        }
    }
}

/// Like .NET `Regex.Escape`.
pub fn escape(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' | '*' | '+' | '?' | '|' | '{' | '[' | '(' | ')' | '^' | '$' | '.' | '#' => {
                output.push('\\');
                output.push(ch);
            }
            ' ' => output.push_str("\\ "),
            '\t' => output.push_str("\\t"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\u{c}' => output.push_str("\\f"),
            _ => output.push(ch),
        }
    }
    output
}

fn resolve_group_name(name: &str, groups: &GroupTable) -> Option<u32> {
    if !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()) {
        let number = name.parse().ok()?;
        groups.slots.contains_key(&number).then_some(number)
    } else {
        groups.names.get(name).copied()
    }
}

// ---------------------------------------------------------------------------
// Parsing

struct Translation {
    pattern: String,
    prefilter: String,
    groups: GroupTable,
}

fn translate(pattern: &str, options: DotNetOptions) -> Result<Translation, DotNetRegexError> {
    let mut parser = Parser {
        chars: pattern.char_indices().collect(),
        source_len: pattern.len(),
        index: 0,
        options,
        captures: Vec::new(),
    };
    let node = parser.parse_alternation()?;
    if parser.index < parser.chars.len() {
        return Err(parser.error("too many ')'"));
    }
    let groups = parser.number_groups()?;
    let mut emitter = Emitter {
        output: String::with_capacity(pattern.len() * 2),
        groups: &groups,
        parser: &parser,
        superset: false,
    };
    emitter.emit(&node)?;
    let translated = std::mem::take(&mut emitter.output);
    emitter.superset = true;
    emitter.emit(&node)?;
    Ok(Translation {
        pattern: translated,
        prefilter: emitter.output,
        groups: groups.table,
    })
}

#[derive(Debug, Clone)]
enum Node {
    Empty,
    Literal(char, bool),
    AnyNoNewline,
    AnyChar,
    Class(Box<ClassSet>, bool),
    Raw(&'static str),
    WordBoundary(bool),
    /// Backreference with .NET semantics resolved after numbering.
    Backref(BackrefTarget, usize),
    Group(GroupKind, Box<Node>),
    Concat(Vec<Node>),
    Alternation(Vec<Node>),
    Repeat {
        node: Box<Node>,
        min: u32,
        max: Option<u32>,
        lazy: bool,
    },
    Conditional {
        condition: Condition,
        yes: Box<Node>,
        no: Option<Box<Node>>,
    },
    /// `(?i)` / `(?-i)` in the middle of a group.
    SetCase(bool),
}

#[derive(Debug, Clone)]
enum BackrefTarget {
    Number(u32),
    Name(String),
    /// `\NN` with two or more digits: backreference if the group exists, otherwise octal.
    Ambiguous(String),
}

#[derive(Debug, Clone)]
enum Condition {
    Group(String),
    Expression(Box<Node>),
}

#[derive(Debug, Clone)]
enum GroupKind {
    Capture,
    NonCapture(Option<bool>),
    Atomic,
    LookAhead(bool),
    LookBehind(bool),
}

#[derive(Debug, Clone)]
enum CaptureName {
    Unnamed,
    Named(String),
    Numbered(u32),
}

#[derive(Debug, Clone)]
struct ClassSet {
    negated: bool,
    items: Vec<ClassItem>,
    subtract: Option<Box<ClassSet>>,
}

#[derive(Debug, Clone)]
enum ClassItem {
    Char(char),
    Range(char, char),
    Raw(String),
}

struct Parser {
    chars: Vec<(usize, char)>,
    source_len: usize,
    index: usize,
    options: DotNetOptions,
    captures: Vec<CaptureName>,
}

struct Numbering {
    table: GroupTable,
}

impl Parser {
    fn position(&self) -> usize {
        self.chars
            .get(self.index)
            .map_or(self.source_len, |&(offset, _)| offset)
    }

    fn error(&self, message: &str) -> DotNetRegexError {
        DotNetRegexError {
            position: Some(self.position()),
            message: message.to_owned(),
        }
    }

    fn error_at(&self, index: usize, message: &str) -> DotNetRegexError {
        DotNetRegexError {
            position: Some(
                self.chars
                    .get(index)
                    .map_or(self.source_len, |&(offset, _)| offset),
            ),
            message: message.to_owned(),
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.index).map(|&(_, c)| c)
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.index + offset).map(|&(_, c)| c)
    }

    fn next(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.index += 1;
        Some(ch)
    }

    fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn rest_starts_with(&self, text: &str) -> bool {
        text.chars()
            .enumerate()
            .all(|(offset, ch)| self.peek_at(offset) == Some(ch))
    }

    /// Skips whitespace and `#` comments in `(?x)` mode, and `(?#...)` comments.
    fn skip_trivia(&mut self) -> Result<(), DotNetRegexError> {
        loop {
            if self.options.ignore_pattern_whitespace {
                while self.peek().is_some_and(is_pattern_whitespace) {
                    self.index += 1;
                }
                if self.peek() == Some('#') {
                    while let Some(ch) = self.next() {
                        if ch == '\n' {
                            break;
                        }
                    }
                    continue;
                }
            }
            if self.rest_starts_with("(?#") {
                let start = self.index;
                self.index += 3;
                loop {
                    match self.next() {
                        Some(')') => break,
                        Some(_) => {}
                        None => return Err(self.error_at(start, "unterminated (?#...) comment")),
                    }
                }
                continue;
            }
            return Ok(());
        }
    }

    fn parse_alternation(&mut self) -> Result<Node, DotNetRegexError> {
        let mut branches = vec![self.parse_concat()?];
        while self.eat('|') {
            branches.push(self.parse_concat()?);
        }
        Ok(if branches.len() == 1 {
            branches.pop().expect("one branch")
        } else {
            Node::Alternation(branches)
        })
    }

    fn parse_concat(&mut self) -> Result<Node, DotNetRegexError> {
        let mut nodes = Vec::new();
        loop {
            self.skip_trivia()?;
            match self.peek() {
                None | Some('|') | Some(')') => break,
                _ => {}
            }
            let atom = self.parse_atom()?;
            let atom = self.parse_quantifiers(atom)?;
            nodes.push(atom);
        }
        Ok(match nodes.len() {
            0 => Node::Empty,
            1 => nodes.pop().expect("one node"),
            _ => Node::Concat(nodes),
        })
    }

    fn parse_quantifiers(&mut self, mut atom: Node) -> Result<Node, DotNetRegexError> {
        let mut quantified = false;
        loop {
            self.skip_trivia()?;
            let Some((min, max, consumed)) = self.scan_quantifier() else {
                return Ok(atom);
            };
            if quantified {
                return Err(self.error("nested quantifier"));
            }
            if matches!(atom, Node::SetCase(_) | Node::Empty) {
                return Err(self.error("quantifier following nothing"));
            }
            if let Some(max) = max
                && max < min
            {
                return Err(self.error("illegal {x,y} with x > y"));
            }
            self.index += consumed;
            let lazy = self.eat('?');
            atom = Node::Repeat {
                node: Box::new(atom),
                min,
                max,
                lazy,
            };
            quantified = true;
        }
    }

    /// Returns `(min, max, chars consumed)` when a quantifier starts here.
    fn scan_quantifier(&self) -> Option<(u32, Option<u32>, usize)> {
        match self.peek()? {
            '*' => Some((0, None, 1)),
            '+' => Some((1, None, 1)),
            '?' => Some((0, Some(1), 1)),
            '{' => {
                // .NET IsTrueQuantifier: `{` digits [`,` digits*] `}`.
                let mut offset = 1;
                let mut min = String::new();
                while let Some(c) = self.peek_at(offset).filter(char::is_ascii_digit) {
                    min.push(c);
                    offset += 1;
                }
                if min.is_empty() {
                    return None;
                }
                let mut max = Some(min.clone());
                if self.peek_at(offset) == Some(',') {
                    offset += 1;
                    let mut upper = String::new();
                    while let Some(c) = self.peek_at(offset).filter(char::is_ascii_digit) {
                        upper.push(c);
                        offset += 1;
                    }
                    max = (!upper.is_empty()).then_some(upper);
                }
                if self.peek_at(offset) != Some('}') {
                    return None;
                }
                let min = min.parse().ok()?;
                let max = match max {
                    Some(max) => Some(max.parse().ok()?),
                    None => None,
                };
                Some((min, max, offset + 1))
            }
            _ => None,
        }
    }

    fn parse_atom(&mut self) -> Result<Node, DotNetRegexError> {
        let ignore_case = self.options.ignore_case;
        let ch = self.next().expect("caller checked for input");
        Ok(match ch {
            '(' => self.parse_group()?,
            '[' => Node::Class(Box::new(self.parse_class()?), ignore_case),
            '\\' => self.parse_escape()?,
            '.' if self.options.singleline => Node::AnyChar,
            '.' => Node::AnyNoNewline,
            '^' if self.options.multiline => Node::Raw("(?m:^)"),
            '^' => Node::Raw(r"\A"),
            '$' if self.options.multiline => Node::Raw("(?m:$)"),
            '$' => Node::Raw(r"(?=\n?\z)"),
            '*' | '+' | '?' => {
                return Err(self.error_at(self.index - 1, "quantifier following nothing"));
            }
            '{' if self.index >= 1 && {
                self.index -= 1;
                let is_quantifier = self.scan_quantifier().is_some();
                self.index += 1;
                is_quantifier
            } =>
            {
                return Err(self.error_at(self.index - 1, "quantifier following nothing"));
            }
            other => Node::Literal(other, ignore_case),
        })
    }

    fn parse_group(&mut self) -> Result<Node, DotNetRegexError> {
        let open = self.index - 1;
        let saved = self.options;
        let kind = if self.eat('?') {
            match self.peek() {
                Some(':') => {
                    self.index += 1;
                    GroupKind::NonCapture(None)
                }
                Some('=') => {
                    self.index += 1;
                    GroupKind::LookAhead(true)
                }
                Some('!') => {
                    self.index += 1;
                    GroupKind::LookAhead(false)
                }
                Some('>') => {
                    self.index += 1;
                    GroupKind::Atomic
                }
                Some('<') if matches!(self.peek_at(1), Some('=')) => {
                    self.index += 2;
                    GroupKind::LookBehind(true)
                }
                Some('<') if matches!(self.peek_at(1), Some('!')) => {
                    self.index += 2;
                    GroupKind::LookBehind(false)
                }
                Some(delimiter @ ('<' | '\'')) => {
                    self.index += 1;
                    let close = if delimiter == '<' { '>' } else { '\'' };
                    let name = self.scan_group_name(close)?;
                    let name = if name.bytes().all(|b| b.is_ascii_digit()) {
                        let number = name
                            .parse()
                            .map_err(|_| self.error("capture group number out of range"))?;
                        if number == 0 {
                            return Err(self.error("capture number cannot be zero"));
                        }
                        CaptureName::Numbered(number)
                    } else {
                        CaptureName::Named(name)
                    };
                    self.captures.push(name);
                    GroupKind::Capture
                }
                Some('(') => return self.parse_conditional(open, saved),
                _ => {
                    let (on, off) = self.scan_option_letters()?;
                    let case = self.apply_options(on, off);
                    match self.next() {
                        Some(')') => {
                            // Inline options last until the end of the enclosing group.
                            return Ok(case.map_or(Node::Empty, Node::SetCase));
                        }
                        Some(':') => GroupKind::NonCapture(case),
                        _ => return Err(self.error_at(open, "unrecognized grouping construct")),
                    }
                }
            }
        } else if self.options.explicit_capture {
            GroupKind::NonCapture(None)
        } else {
            self.captures.push(CaptureName::Unnamed);
            GroupKind::Capture
        };
        let body = self.parse_alternation()?;
        if !self.eat(')') {
            return Err(self.error_at(open, "not enough ')'"));
        }
        self.options = saved;
        Ok(Node::Group(kind, Box::new(body)))
    }

    fn scan_group_name(&mut self, close: char) -> Result<String, DotNetRegexError> {
        let mut name = String::new();
        loop {
            match self.next() {
                Some(c) if c == close => break,
                Some('-') => return Err(self.error("balancing groups are not supported")),
                Some(c) if c.is_alphanumeric() || c == '_' => name.push(c),
                _ => return Err(self.error("invalid group name")),
            }
        }
        if name.is_empty() {
            return Err(self.error("invalid group name"));
        }
        Ok(name)
    }

    fn scan_option_letters(&mut self) -> Result<(Vec<char>, Vec<char>), DotNetRegexError> {
        let mut on = Vec::new();
        let mut off = Vec::new();
        let mut negate = false;
        while let Some(c) = self.peek() {
            match c {
                '-' => negate = true,
                'i' | 'm' | 'n' | 's' | 'x' => {
                    if negate {
                        off.push(c)
                    } else {
                        on.push(c)
                    }
                }
                ')' | ':' => break,
                _ => return Err(self.error("unrecognized grouping construct")),
            }
            self.index += 1;
        }
        Ok((on, off))
    }

    /// Applies option letters; returns the new case setting when `i` changed.
    fn apply_options(&mut self, on: Vec<char>, off: Vec<char>) -> Option<bool> {
        let mut case = None;
        for (letters, value) in [(on, true), (off, false)] {
            for letter in letters {
                match letter {
                    'i' => {
                        self.options.ignore_case = value;
                        case = Some(value);
                    }
                    'm' => self.options.multiline = value,
                    'n' => self.options.explicit_capture = value,
                    's' => self.options.singleline = value,
                    'x' => self.options.ignore_pattern_whitespace = value,
                    _ => unreachable!("validated by scan_option_letters"),
                }
            }
        }
        case
    }

    fn parse_conditional(
        &mut self,
        open: usize,
        saved: DotNetOptions,
    ) -> Result<Node, DotNetRegexError> {
        // At `(` of `(?(`.
        let condition_start = self.index;
        self.index += 1;
        let name_end = self.chars[self.index..]
            .iter()
            .position(|&(_, c)| c == ')')
            .map(|offset| self.index + offset);
        let name: Option<String> = name_end.map(|end| {
            self.chars[self.index..end]
                .iter()
                .map(|&(_, c)| c)
                .collect()
        });
        let condition = match name {
            Some(name)
                if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') =>
            {
                self.index = name_end.expect("found") + 1;
                Condition::Group(name)
            }
            _ => {
                // `(?(expression)yes|no)` — the expression is a zero-width lookahead.
                self.index = condition_start;
                let group = self.parse_atom()?;
                let body = match group {
                    Node::Group(GroupKind::LookAhead(_) | GroupKind::LookBehind(_), _) => group,
                    Node::Group(_, body) => Node::Group(GroupKind::LookAhead(true), body),
                    other => Node::Group(GroupKind::LookAhead(true), Box::new(other)),
                };
                Condition::Expression(Box::new(body))
            }
        };
        let yes = self.parse_concat()?;
        let no = if self.eat('|') {
            Some(Box::new(self.parse_concat()?))
        } else {
            None
        };
        if self.peek() == Some('|') {
            return Err(self.error("too many | in (?()|)"));
        }
        if !self.eat(')') {
            return Err(self.error_at(open, "not enough ')'"));
        }
        self.options = saved;
        Ok(Node::Conditional {
            condition,
            yes: Box::new(yes),
            no,
        })
    }

    fn parse_escape(&mut self) -> Result<Node, DotNetRegexError> {
        let escape_start = self.index - 1;
        let Some(ch) = self.next() else {
            return Err(self.error("illegal \\ at end of pattern"));
        };
        Ok(match ch {
            'b' => Node::WordBoundary(true),
            'B' => Node::WordBoundary(false),
            'A' => Node::Raw(r"\A"),
            'z' => Node::Raw(r"\z"),
            'Z' => Node::Raw(r"(?=\n?\z)"),
            'G' => return Err(self.error_at(escape_start, r"\G is not supported")),
            'w' | 'W' | 'd' | 'D' | 's' | 'S' | 'p' | 'P' => {
                self.index -= 1;
                let item = self.parse_class_escape_shorthand()?;
                Node::Class(
                    Box::new(ClassSet {
                        negated: false,
                        items: vec![item],
                        subtract: None,
                    }),
                    false,
                )
            }
            'k' => match self.peek() {
                Some(delimiter @ ('<' | '\'')) => {
                    self.index += 1;
                    let close = if delimiter == '<' { '>' } else { '\'' };
                    let name = self.scan_group_name(close)?;
                    if name.bytes().all(|b| b.is_ascii_digit()) {
                        let number = name
                            .parse()
                            .map_err(|_| self.error("capture group number out of range"))?;
                        Node::Backref(BackrefTarget::Number(number), escape_start)
                    } else {
                        Node::Backref(BackrefTarget::Name(name), escape_start)
                    }
                }
                _ => {
                    return Err(
                        self.error_at(escape_start, "malformed \\k<...> named back reference")
                    );
                }
            },
            '1'..='9' => {
                let mut digits = String::from(ch);
                while let Some(c) = self.peek().filter(char::is_ascii_digit) {
                    digits.push(c);
                    self.index += 1;
                }
                if digits.len() == 1 {
                    Node::Backref(
                        BackrefTarget::Number(digits.parse().expect("digit")),
                        escape_start,
                    )
                } else {
                    Node::Backref(BackrefTarget::Ambiguous(digits), escape_start)
                }
            }
            _ => {
                self.index -= 1;
                Node::Literal(self.parse_char_escape(false)?, self.options.ignore_case)
            }
        })
    }

    /// Parses `\w`-style shorthands and `\p{...}` after the backslash.
    fn parse_class_escape_shorthand(&mut self) -> Result<ClassItem, DotNetRegexError> {
        let ch = self.next().expect("caller peeked");
        Ok(ClassItem::Raw(match ch {
            'w' => WORD_CLASS.to_owned(),
            'W' => format!("[^{WORD_CLASS}]"),
            'd' => r"\p{Nd}".to_owned(),
            'D' => r"\P{Nd}".to_owned(),
            's' => r"\s".to_owned(),
            'S' => r"\S".to_owned(),
            'p' | 'P' => {
                if !self.eat('{') {
                    return Err(self.error("incomplete \\p{X} character escape"));
                }
                let mut name = String::new();
                loop {
                    match self.next() {
                        Some('}') => break,
                        Some(c) => name.push(c),
                        None => return Err(self.error("incomplete \\p{X} character escape")),
                    }
                }
                unicode_property(&name, ch == 'P')
                    .ok_or_else(|| self.error(&format!("unknown property '{name}'")))?
            }
            _ => unreachable!("caller checked"),
        }))
    }

    /// Parses a single-character escape after the backslash (the cursor is on
    /// the character after `\`).
    fn parse_char_escape(&mut self, in_class: bool) -> Result<char, DotNetRegexError> {
        let escape_start = self.index - 1;
        let ch = self
            .next()
            .ok_or_else(|| self.error("illegal \\ at end of pattern"))?;
        Ok(match ch {
            'a' => '\u{7}',
            'b' if in_class => '\u{8}',
            'e' => '\u{1b}',
            'f' => '\u{c}',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'v' => '\u{b}',
            'x' => self.scan_hex(2)?,
            'u' => self.scan_hex(4)?,
            'c' => {
                let control = self
                    .next()
                    .filter(char::is_ascii_alphabetic)
                    .ok_or_else(|| self.error("missing control character"))?;
                char::from(control.to_ascii_uppercase() as u8 - b'@')
            }
            '0'..='7' if ch == '0' || in_class => {
                // Up to three octal digits; .NET masks the value to 8 bits.
                let mut value = ch.to_digit(8).expect("octal");
                for _ in 0..2 {
                    match self.peek().and_then(|c| c.to_digit(8)) {
                        Some(digit) => {
                            value = value * 8 + digit;
                            self.index += 1;
                        }
                        None => break,
                    }
                }
                char::from((value & 0xff) as u8)
            }
            c if c == '_' || c.is_alphanumeric() => {
                return Err(
                    self.error_at(escape_start, &format!("unrecognized escape sequence \\{c}"))
                );
            }
            other => other,
        })
    }

    fn scan_hex(&mut self, digits: usize) -> Result<char, DotNetRegexError> {
        let mut value = 0;
        for _ in 0..digits {
            let digit = self
                .next()
                .and_then(|c| c.to_digit(16))
                .ok_or_else(|| self.error("insufficient hexadecimal digits"))?;
            value = value * 16 + digit;
        }
        char::from_u32(value).ok_or_else(|| self.error("surrogate code units are not supported"))
    }

    /// Parses a character class; the cursor is after `[`.
    fn parse_class(&mut self) -> Result<ClassSet, DotNetRegexError> {
        let open = self.index - 1;
        let negated = self.eat('^');
        let mut items = Vec::new();
        let mut first = true;
        let mut subtract = None;
        loop {
            let Some(ch) = self.next() else {
                return Err(self.error_at(open, "unterminated [] set"));
            };
            let item_start = self.index - 1;
            match ch {
                ']' if !first => break,
                '-' if !first && self.peek() == Some('[') => {
                    // Subtraction must be the last thing in the class.
                    self.index += 1;
                    let inner = self.parse_class()?;
                    if !self.eat(']') {
                        return Err(self
                            .error("a subtraction must be the last element in a character class"));
                    }
                    subtract = Some(Box::new(inner));
                    break;
                }
                _ => {}
            }
            first = false;
            let low = match ch {
                '\\' => match self.peek() {
                    Some('w' | 'W' | 'd' | 'D' | 's' | 'S' | 'p' | 'P') => {
                        let item = self.parse_class_escape_shorthand()?;
                        if self.peek() == Some('-')
                            && self.peek_at(1).is_some_and(|c| c != ']' && c != '[')
                        {
                            return Err(self
                                .error_at(item_start, "cannot include class in character range"));
                        }
                        items.push(item);
                        continue;
                    }
                    _ => self.parse_char_escape(true)?,
                },
                other => other,
            };
            // A range unless `-` is last, or starts a subtraction.
            if self.peek() == Some('-') && !matches!(self.peek_at(1), Some(']') | Some('[') | None)
            {
                self.index += 1;
                let high = match self.next() {
                    Some('\\') => match self.peek() {
                        Some('w' | 'W' | 'd' | 'D' | 's' | 'S' | 'p' | 'P') => {
                            return Err(self
                                .error_at(item_start, "cannot include class in character range"));
                        }
                        _ => self.parse_char_escape(true)?,
                    },
                    Some(other) => other,
                    None => return Err(self.error_at(open, "unterminated [] set")),
                };
                if high < low {
                    return Err(self.error_at(item_start, "range in reverse order"));
                }
                items.push(ClassItem::Range(low, high));
            } else {
                items.push(ClassItem::Char(low));
            }
        }
        Ok(ClassSet {
            negated,
            items,
            subtract,
        })
    }

    fn number_groups(&self) -> Result<Numbering, DotNetRegexError> {
        // .NET: unnamed groups 1..n in order, then explicit numbers stay, then
        // names in order of appearance take the lowest free numbers above n.
        let mut table = GroupTable::default();
        let mut capture_numbers = vec![0; self.captures.len()];
        let mut next_unnamed = 1;
        let mut used = std::collections::BTreeSet::new();
        for (index, name) in self.captures.iter().enumerate() {
            if matches!(name, CaptureName::Unnamed) {
                capture_numbers[index] = next_unnamed;
                used.insert(next_unnamed);
                next_unnamed += 1;
            }
        }
        for (index, name) in self.captures.iter().enumerate() {
            if let CaptureName::Numbered(number) = name {
                capture_numbers[index] = *number;
                used.insert(*number);
            }
        }
        let mut next_named = next_unnamed;
        for (index, name) in self.captures.iter().enumerate() {
            if let CaptureName::Named(name) = name {
                if let Some(&number) = table.names.get(name) {
                    capture_numbers[index] = number;
                    continue;
                }
                while used.contains(&next_named) {
                    next_named += 1;
                }
                used.insert(next_named);
                table.names.insert(name.clone(), next_named);
                capture_numbers[index] = next_named;
            }
        }
        table.slots.insert(0, 0);
        for (index, &number) in capture_numbers.iter().enumerate() {
            if table.slots.insert(number, index + 1).is_some() {
                return Err(DotNetRegexError {
                    position: None,
                    message: format!(
                        "group {number} is defined more than once, which is not supported"
                    ),
                });
            }
        }
        table.last = capture_numbers.iter().copied().max().unwrap_or(0);
        Ok(Numbering { table })
    }
}

/// Assertions: lookarounds, anchors and word boundaries.
fn is_zero_width(node: &Node) -> bool {
    match node {
        Node::Group(GroupKind::LookAhead(_) | GroupKind::LookBehind(_), _)
        | Node::WordBoundary(_)
        | Node::Raw(_) => true,
        Node::Group(GroupKind::NonCapture(_), body) => is_zero_width(body),
        _ => false,
    }
}

fn is_pattern_whitespace(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{b}' | '\u{c}')
}

/// Maps a .NET `\p{...}` name to fancy-regex syntax.
fn unicode_property(name: &str, negated: bool) -> Option<String> {
    const CATEGORIES: &[&str] = &[
        "L", "Lu", "Ll", "Lt", "Lm", "Lo", "M", "Mn", "Mc", "Me", "N", "Nd", "Nl", "No", "P", "Pc",
        "Pd", "Ps", "Pe", "Pi", "Pf", "Po", "S", "Sm", "Sc", "Sk", "So", "Z", "Zs", "Zl", "Zp",
        "C", "Cc", "Cf", "Cs", "Co", "Cn",
    ];
    if CATEGORIES.contains(&name) {
        return Some(format!("\\{}{{{name}}}", if negated { 'P' } else { 'p' }));
    }
    let (start, end) = unicode_block(name)?;
    Some(format!(
        "{}\\x{{{start:04X}}}-\\x{{{end:04X}}}]",
        if negated { "[^" } else { "[" }
    ))
}

/// Unicode block ranges for the .NET `\p{IsBlockName}` syntax.
fn unicode_block(name: &str) -> Option<(u32, u32)> {
    let name = name.strip_prefix("Is")?;
    Some(match name {
        "BasicLatin" => (0x0000, 0x007F),
        "Latin-1Supplement" => (0x0080, 0x00FF),
        "LatinExtended-A" => (0x0100, 0x017F),
        "LatinExtended-B" => (0x0180, 0x024F),
        "IPAExtensions" => (0x0250, 0x02AF),
        "SpacingModifierLetters" => (0x02B0, 0x02FF),
        "CombiningDiacriticalMarks" => (0x0300, 0x036F),
        "Greek" | "GreekandCoptic" => (0x0370, 0x03FF),
        "Cyrillic" => (0x0400, 0x04FF),
        "CyrillicSupplement" => (0x0500, 0x052F),
        "Armenian" => (0x0530, 0x058F),
        "Hebrew" => (0x0590, 0x05FF),
        "Arabic" => (0x0600, 0x06FF),
        "Syriac" => (0x0700, 0x074F),
        "Thaana" => (0x0780, 0x07BF),
        "Devanagari" => (0x0900, 0x097F),
        "Bengali" => (0x0980, 0x09FF),
        "Gurmukhi" => (0x0A00, 0x0A7F),
        "Gujarati" => (0x0A80, 0x0AFF),
        "Oriya" => (0x0B00, 0x0B7F),
        "Tamil" => (0x0B80, 0x0BFF),
        "Telugu" => (0x0C00, 0x0C7F),
        "Kannada" => (0x0C80, 0x0CFF),
        "Malayalam" => (0x0D00, 0x0D7F),
        "Sinhala" => (0x0D80, 0x0DFF),
        "Thai" => (0x0E00, 0x0E7F),
        "Lao" => (0x0E80, 0x0EFF),
        "Tibetan" => (0x0F00, 0x0FFF),
        "Myanmar" => (0x1000, 0x109F),
        "Georgian" => (0x10A0, 0x10FF),
        "HangulJamo" => (0x1100, 0x11FF),
        "Ethiopic" => (0x1200, 0x137F),
        "Cherokee" => (0x13A0, 0x13FF),
        "UnifiedCanadianAboriginalSyllabics" => (0x1400, 0x167F),
        "Ogham" => (0x1680, 0x169F),
        "Runic" => (0x16A0, 0x16FF),
        "Khmer" => (0x1780, 0x17FF),
        "Mongolian" => (0x1800, 0x18AF),
        "LatinExtendedAdditional" => (0x1E00, 0x1EFF),
        "GreekExtended" => (0x1F00, 0x1FFF),
        "GeneralPunctuation" => (0x2000, 0x206F),
        "SuperscriptsandSubscripts" => (0x2070, 0x209F),
        "CurrencySymbols" => (0x20A0, 0x20CF),
        "LetterlikeSymbols" => (0x2100, 0x214F),
        "NumberForms" => (0x2150, 0x218F),
        "Arrows" => (0x2190, 0x21FF),
        "MathematicalOperators" => (0x2200, 0x22FF),
        "MiscellaneousTechnical" => (0x2300, 0x23FF),
        "BoxDrawing" => (0x2500, 0x257F),
        "GeometricShapes" => (0x25A0, 0x25FF),
        "MiscellaneousSymbols" => (0x2600, 0x26FF),
        "Dingbats" => (0x2700, 0x27BF),
        "CJKSymbolsandPunctuation" => (0x3000, 0x303F),
        "Hiragana" => (0x3040, 0x309F),
        "Katakana" => (0x30A0, 0x30FF),
        "CJKUnifiedIdeographs" => (0x4E00, 0x9FFF),
        "HangulSyllables" => (0xAC00, 0xD7AF),
        "PrivateUse" | "PrivateUseArea" => (0xE000, 0xF8FF),
        "AlphabeticPresentationForms" => (0xFB00, 0xFB4F),
        "Arabic PresentationForms-A" | "ArabicPresentationForms-A" => (0xFB50, 0xFDFF),
        "CombiningHalfMarks" => (0xFE20, 0xFE2F),
        "CJKCompatibilityForms" => (0xFE30, 0xFE4F),
        "ArabicPresentationForms-B" => (0xFE70, 0xFEFF),
        "HalfwidthandFullwidthForms" => (0xFF00, 0xFFEF),
        "Specials" => (0xFFF0, 0xFFFF),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Emitting

struct Emitter<'a> {
    output: String,
    groups: &'a Numbering,
    parser: &'a Parser,
    /// Emit a lookaround-free `regex` pattern that matches a superset.
    superset: bool,
}

impl Emitter<'_> {
    fn emit(&mut self, node: &Node) -> Result<(), DotNetRegexError> {
        match node {
            Node::Empty => {}
            Node::Literal(ch, ignore_case) => self.emit_literal(*ch, *ignore_case),
            Node::AnyNoNewline => self.output.push_str(r"[^\n]"),
            Node::AnyChar => self.output.push_str(r"(?s:.)"),
            Node::Class(class, ignore_case) => {
                if *ignore_case {
                    self.output.push_str("(?i:");
                }
                self.emit_class(class);
                if *ignore_case {
                    self.output.push(')');
                }
            }
            // `$` without multiline; dropping an assertion only widens the match.
            Node::Raw(raw) if self.superset && raw.starts_with("(?=") => {}
            Node::Raw(raw) => self.output.push_str(raw),
            Node::WordBoundary(true) => self.output.push_str(r"\b"),
            Node::WordBoundary(false) => self.output.push_str(r"\B"),
            Node::Backref(target, position) => self.emit_backref(target, *position)?,
            Node::Group(GroupKind::LookAhead(_) | GroupKind::LookBehind(_), _) if self.superset => {
            }
            Node::Group(GroupKind::Capture | GroupKind::Atomic, body) if self.superset => {
                self.output.push_str("(?:");
                self.emit(body)?;
                self.output.push(')');
            }
            Node::Conditional { yes, no, .. } if self.superset => {
                self.output.push_str("(?:");
                self.emit(yes)?;
                self.output.push('|');
                if let Some(no) = no {
                    self.emit(no)?;
                }
                self.output.push(')');
            }
            Node::Group(kind, body) => {
                self.output.push_str(match kind {
                    GroupKind::Capture => "(",
                    GroupKind::NonCapture(None) => "(?:",
                    GroupKind::NonCapture(Some(true)) => "(?i:",
                    GroupKind::NonCapture(Some(false)) => "(?-i:",
                    GroupKind::Atomic => "(?>",
                    GroupKind::LookAhead(true) => "(?=",
                    GroupKind::LookAhead(false) => "(?!",
                    GroupKind::LookBehind(true) => "(?<=",
                    GroupKind::LookBehind(false) => "(?<!",
                });
                self.emit(body)?;
                self.output.push(')');
            }
            Node::Concat(nodes) => {
                for node in nodes {
                    self.emit(node)?;
                }
            }
            Node::Alternation(branches) => {
                for (index, branch) in branches.iter().enumerate() {
                    if index > 0 {
                        self.output.push('|');
                    }
                    self.emit(branch)?;
                }
            }
            // .NET accepts a quantified assertion such as `(?!x)?`; fancy-regex
            // rejects it. An assertion matches no text, so repeating it once is
            // the same as repeating it any number of times, and an optional one
            // becomes an alternation with the empty pattern.
            Node::Repeat {
                node, min, lazy, ..
            } if is_zero_width(node) => {
                if *min > 0 {
                    self.emit(node)?;
                } else {
                    self.output.push_str("(?:");
                    if *lazy {
                        self.output.push('|');
                        self.emit(node)?;
                    } else {
                        self.emit(node)?;
                        self.output.push('|');
                    }
                    self.output.push(')');
                }
            }
            Node::Repeat {
                node,
                min,
                max,
                lazy,
            } => {
                let needs_group = !matches!(
                    **node,
                    Node::Literal(_, false)
                        | Node::AnyNoNewline
                        | Node::Group(..)
                        | Node::Class(_, false)
                );
                if needs_group {
                    self.output.push_str("(?:");
                }
                self.emit(node)?;
                if needs_group {
                    self.output.push(')');
                }
                match (min, max) {
                    (0, None) => self.output.push('*'),
                    (1, None) => self.output.push('+'),
                    (0, Some(1)) => self.output.push('?'),
                    (min, None) => self.output.push_str(&format!("{{{min},}}")),
                    (min, Some(max)) if min == max => self.output.push_str(&format!("{{{min}}}")),
                    (min, Some(max)) => self.output.push_str(&format!("{{{min},{max}}}")),
                }
                if *lazy {
                    self.output.push('?');
                }
            }
            Node::Conditional { condition, yes, no } => {
                self.output.push_str("(?(");
                match condition {
                    Condition::Group(name) => {
                        let resolved = if name.bytes().all(|b| b.is_ascii_digit()) {
                            name.parse::<u32>()
                                .ok()
                                .and_then(|n| self.groups.table.slots.get(&n))
                        } else {
                            self.groups
                                .table
                                .names
                                .get(name)
                                .and_then(|n| self.groups.table.slots.get(n))
                        };
                        match resolved {
                            Some(slot) => self.output.push_str(&slot.to_string()),
                            None => {
                                // Not a group: .NET treats the text as a lookahead expression.
                                self.output.push_str("(?=");
                                for ch in name.chars() {
                                    self.emit_literal(ch, false);
                                }
                                self.output.push(')');
                            }
                        }
                    }
                    Condition::Expression(expression) => self.emit(expression)?,
                }
                self.output.push(')');
                self.emit(yes)?;
                if let Some(no) = no {
                    self.output.push('|');
                    self.emit(no)?;
                }
                self.output.push(')');
            }
            Node::SetCase(true) => self.output.push_str("(?i)"),
            Node::SetCase(false) => self.output.push_str("(?-i)"),
        }
        Ok(())
    }

    fn emit_backref(
        &mut self,
        target: &BackrefTarget,
        position: usize,
    ) -> Result<(), DotNetRegexError> {
        let table = &self.groups.table;
        let slot = match target {
            BackrefTarget::Number(number) => table.slots.get(number).copied().ok_or_else(|| {
                self.parser.error_at(
                    position,
                    &format!("reference to undefined group number {number}"),
                )
            })?,
            BackrefTarget::Name(name) => table
                .names
                .get(name)
                .and_then(|number| table.slots.get(number))
                .copied()
                .ok_or_else(|| {
                    self.parser.error_at(
                        position,
                        &format!("reference to undefined group name {name}"),
                    )
                })?,
            BackrefTarget::Ambiguous(digits) => {
                if let Some(slot) = digits.parse::<u32>().ok().and_then(|n| table.slots.get(&n)) {
                    *slot
                } else {
                    // Octal: up to three leading octal digits, the rest literal digits.
                    let octal_len = digits.chars().take(3).take_while(|c| c.is_digit(8)).count();
                    if octal_len == 0 {
                        let number = &digits[..1];
                        return Err(self.parser.error_at(
                            position,
                            &format!("reference to undefined group number {number}"),
                        ));
                    }
                    let value = u32::from_str_radix(&digits[..octal_len], 8).expect("octal digits");
                    self.emit_literal(char::from((value & 0xff) as u8), false);
                    for ch in digits[octal_len..].chars() {
                        self.emit_literal(ch, false);
                    }
                    return Ok(());
                }
            }
        };
        if self.superset {
            self.output.push_str("(?s:.*?)");
        } else {
            // Wrap so a following digit cannot extend the group number.
            self.output.push_str(&format!("(?:\\{slot})"));
        }
        Ok(())
    }

    fn emit_literal(&mut self, ch: char, ignore_case: bool) {
        if ignore_case && ch.is_alphabetic() {
            self.output.push_str("(?i:");
            push_escaped(&mut self.output, ch);
            self.output.push(')');
        } else {
            push_escaped(&mut self.output, ch);
        }
    }

    fn emit_class(&mut self, class: &ClassSet) {
        if let Some(subtract) = &class.subtract {
            self.output.push('[');
            self.emit_class(&ClassSet {
                negated: class.negated,
                items: class.items.clone(),
                subtract: None,
            });
            self.output.push_str("--");
            self.emit_class(subtract);
            self.output.push(']');
            return;
        }
        self.output.push('[');
        if class.negated {
            self.output.push('^');
        }
        for item in &class.items {
            match item {
                ClassItem::Char(ch) => push_escaped(&mut self.output, *ch),
                ClassItem::Range(low, high) => {
                    push_escaped(&mut self.output, *low);
                    self.output.push('-');
                    push_escaped(&mut self.output, *high);
                }
                ClassItem::Raw(raw) => self.output.push_str(raw),
            }
        }
        self.output.push(']');
    }
}

fn push_escaped(output: &mut String, ch: char) {
    match ch {
        '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$'
        | '#' | '&' | '-' | '~' => {
            output.push('\\');
            output.push(ch);
        }
        '\n' => output.push_str(r"\n"),
        '\r' => output.push_str(r"\r"),
        '\t' => output.push_str(r"\t"),
        c if c.is_control() || c.is_whitespace() && c != ' ' => {
            output.push_str(&format!("\\x{{{:X}}}", c as u32));
        }
        c => output.push(c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replace(pattern: &str, replacement: &str, text: &str) -> String {
        let regex = DotNetRegex::new(pattern).unwrap_or_else(|error| panic!("{pattern}: {error}"));
        let replacement = regex.replacement(replacement);
        regex.replace_all(text, &replacement, usize::MAX).unwrap().0
    }

    fn is_match(pattern: &str, text: &str) -> bool {
        DotNetRegex::new(pattern)
            .unwrap_or_else(|error| panic!("{pattern}: {error}"))
            .is_match(text)
            .unwrap()
    }

    #[test]
    fn replacement_group_followed_by_letters() {
        assert_eq!(replace(r"\b(a)cheive", "$1chieve", "acheive"), "achieve");
        assert_eq!(replace(r"(a)(b)", "$1a$2", "ab"), "aab");
    }

    #[test]
    fn replacement_dollar_rules() {
        assert_eq!(replace("(x)", "$$1", "x"), "$1");
        assert_eq!(replace("(x)", "$12", "x"), "$12");
        assert_eq!(replace("(x)", "${1}2", "x"), "x2");
        assert_eq!(replace("(x)", "$&$&", "x"), "xx");
        assert_eq!(replace("b", "[$`|$']", "abc"), "a[a|c]c");
        assert_eq!(replace("(?<n>x)", "<${n}>", "x"), "<x>");
        assert_eq!(replace("x", "$", "x"), "$");
        assert_eq!(replace("x", "$z", "x"), "$z");
        assert_eq!(replace("(a)(b)?", "$+|$_", "a"), "|a");
    }

    #[test]
    fn retf_currency_replacement() {
        assert_eq!(replace(r"(\s)(\d+) US\$", "$1US$$$2", " 5 US$"), " US$5");
    }

    #[test]
    fn unnamed_groups_are_numbered_before_named_groups() {
        assert_eq!(replace("(?<a>x)(y)", "$1-$2-${a}", "xy"), "y-x-x");
        assert!(is_match(r"(?<a>x)(y)\1\2", "xyyx"));
    }

    #[test]
    fn dollar_matches_before_final_newline() {
        assert!(is_match("a$", "a\n"));
        assert!(!is_match("a$", "a\n\n"));
        assert!(is_match(r"a\Z", "a\n"));
        assert!(!is_match(r"a\Z", "a\n\n"));
        assert!(!is_match(r"a\z", "a\n"));
        assert!(is_match("(?m)a$", "a\nb"));
        assert!(!is_match("a$", "a\nb"));
        assert!(!is_match("^b", "a\nb"));
        assert!(is_match("(?m)^b", "a\nb"));
    }

    #[test]
    fn escaped_angle_brackets_are_literals() {
        assert!(is_match(r"\<br", "<br>"));
        assert!(is_match(r"\<\/ref\>", "</ref>"));
        assert!(!is_match(r"\<br", "br"));
    }

    #[test]
    fn literal_braces() {
        assert!(is_match(r"{{sic}}", "{{sic}}"));
        assert!(is_match(r"a{,2}", "a{,2}"));
        assert!(is_match(r"x{2}", "xx"));
        assert!(is_match(r"x{1,}y", "xxy"));
        assert!(is_match(r"\b{", "a{"));
    }

    #[test]
    fn class_subtraction() {
        assert_eq!(replace("[a-z-[aeiou]]+", "_", "hello"), "_e_o");
        assert_eq!(replace("[^a-z-[x]]", "_", "aX1x"), "a__x");
        assert_eq!(replace("[a-z-[d-w-[m-o]]]", "_", "aemnoz"), "_e____");
    }

    #[test]
    fn brackets_and_specials_inside_classes() {
        assert!(is_match(r"[[]", "["));
        assert!(is_match(r"[\[\]]", "]"));
        assert!(is_match(r"[]a]", "]"));
        assert!(is_match(r"[a&&b]", "&"));
        assert!(is_match(r"[a~-]", "-"));
        assert!(is_match(r"[\b]", "\u{8}"));
        assert!(is_match(r"[\w']+", "don't"));
        assert!(!is_match(r"[\W]", "é"));
    }

    #[test]
    fn word_class_matches_dotnet_definition() {
        assert!(is_match(r"^\w+$", "naïveКиїв_٣"));
        // Roman numeral twelve is Nl: a word character in Rust, not in .NET.
        assert!(!is_match(r"\w", "\u{216B}"));
    }

    #[test]
    fn variable_length_lookbehind() {
        assert_eq!(
            replace(
                r"\bDean(?<=[,\.]\s{1,99}\w{1,99}|\bThe\s+\w+)",
                "dean",
                "The Dean, x  Dean"
            ),
            "The dean, x  Dean"
        );
        assert!(is_match(r"(?<=x[\s\w]+)y", "x ab y"));
        assert!(is_match(r"(?<=\p{Lu}[\p{L}\.'-]*)z", "Abc'z"));
    }

    #[test]
    fn lookahead_is_atomic() {
        // .NET does not backtrack into a lookahead that matched.
        assert!(!is_match(r"(?=x|(x))\1x", "xx"));
    }

    #[test]
    fn backreferences() {
        assert_eq!(replace(r"\b(the)\s+\1\b", "$1", "the the cat"), "the cat");
        assert!(is_match(r"(\w)x(?<=\1\1x)", "aax"));
        assert!(is_match(r"(a)\1(?:0)", "aa0"));
        // \10 without ten groups is octal 010 (backspace).
        assert!(is_match(r"(a)\10", "a\u{8}"));
    }

    #[test]
    fn options() {
        assert!(is_match("(?i)ABC", "abc"));
        assert!(is_match("a(?i:B)c", "abc"));
        assert!(!is_match("a(?i:B)C", "abc"));
        // Inline options end with the enclosing group, as in .NET.
        assert!(is_match("(a(?i)b)c", "aBc"));
        assert!(!is_match("(a(?i)b)c", "aBC"));
        assert!(is_match("(?s)a.b", "a\nb"));
        assert!(!is_match("a.b", "a\nb"));
        assert!(is_match("a.b", "a\rb"));
        assert!(is_match("(?x) a b # comment\n c", "abc"));
        assert!(is_match("(?x)a[ ]b", "a b"));
        assert!(is_match("a(?#note)b", "ab"));
        assert_eq!(replace("(?n)(a)(?<x>b)", "$1|${x}", "ab"), "b|b");
    }

    #[test]
    fn escapes() {
        assert!(is_match(r"\x41B\t\cA", "AB\t\u{1}"));
        assert!(is_match(r"\⌊\£\:\-\'", "⌊£:-'"));
        assert!(DotNetRegex::new(r"\q").is_err());
        assert!(DotNetRegex::new(r"\G").is_err());
    }

    #[test]
    fn unicode_properties_and_blocks() {
        assert!(is_match(r"\p{Lu}\p{Ll}", "Ab"));
        assert!(is_match(r"^\p{IsCyrillic}+$", "Київ"));
        assert!(!is_match(r"\P{L}", "abc"));
        assert!(DotNetRegex::new(r"\p{IsKlingon}").is_err());
    }

    #[test]
    fn conditionals_and_atomic_groups() {
        assert!(is_match(r"^(x)?(?(1)(?:a|b))c$", "xbc"));
        assert!(is_match(r"^(<)?a(?(1)>|)$", "<a>"));
        assert!(!is_match(r"^(<)?a(?(1)>|)$", "<a"));
        assert!(!is_match(r"^(?>a+)a$", "aaa"));
    }

    #[test]
    fn rejects_dotnet_errors() {
        for pattern in [
            "*a",
            "a**",
            "(a",
            "a)",
            "[a",
            r"\1(a)x\2",
            "(?<a-b>x)",
            "[z-a]",
        ] {
            assert!(DotNetRegex::new(pattern).is_err(), "{pattern}");
        }
    }

    #[test]
    fn prefilter_is_a_lookaround_free_superset() {
        let regex = DotNetRegex::new(r"\b(a)(?<=x\1)(?!y)b$").unwrap();
        assert_eq!(regex.prefilter_pattern(), Some(r"\b(?:a)b"));
        let backref = DotNetRegex::new(r"(a)\1").unwrap();
        assert_eq!(backref.prefilter_pattern(), Some(r"(?:a)(?s:.*?)"));
        assert!(regex.may_match("ab"));
        assert!(!regex.may_match("ac"));
        assert!(!regex.is_match("ab").unwrap());
    }

    #[test]
    fn backtrack_limit_is_reported() {
        let regex =
            DotNetRegex::with_options(r"^(a|aa)+(?<=a)\1b", DotNetOptions::default(), 10_000)
                .unwrap();
        assert!(regex.is_match(&format!("{}cb", "a".repeat(40))).is_err());
    }
}

#[cfg(test)]
mod retf_samples {
    //! Constructs taken from `Wikipedia:AutoWikiBrowser/Typos`.
    use super::*;

    fn fix(pattern: &str, replacement: &str, text: &str) -> String {
        let regex = DotNetRegex::new(pattern).unwrap_or_else(|error| panic!("{pattern}: {error}"));
        let replacement = regex.replacement(replacement);
        regex.replace_all(text, &replacement, usize::MAX).unwrap().0
    }

    #[test]
    fn backreference_inside_lookbehind() {
        let pattern = r"\b(\d+)(?<=(?:\s|\()\d+)[-—](\d+)(?=(?:[,\.;\)\*]|\s*?(?:\n|\<br)))(?<!\b(?:\1[-—]\1\b|\d{5}[-—]\d{4}))";
        assert_eq!(
            fix(pattern, "$1–$2", "pages 12-15, and 7-7."),
            "pages 12–15, and 7-7."
        );
    }

    #[test]
    fn nested_lookbehind_with_word_boundary_and_backreference() {
        let pattern = r"\b(?<=[\s\(]|\A)([aA])(?<=a|(?:[\.\n]\s\s?\s?|\A)A)\s\s?((?:[aA](?!\b|2\b)|[eE](?!\b))[^\|\[\]\<\⌊\>\{\}\s]{0,29})(?<=\b(?:[\S\s]){1,49}(?<!\b(?:[aA](?:baten|cusa)|[nN]o)\W?\s?\s?[aA]\s?\s?\2))";
        assert_eq!(
            fix(pattern, "$1n $2", "It is a apple and a egg."),
            "It is an apple and an egg."
        );
        assert_eq!(fix(pattern, "$1n $2", "No a apple."), "No a apple.");
    }

    #[test]
    fn quantified_lookahead() {
        // The "Furthermore," rule: an optional negative lookahead.
        let pattern = r"\b(Accordingly|Consequently|Even\s+so|F(?:or\s+example|urthermore)|In(?:deed|\s+other\s+words)|M(?:eanwhile(?!\s+Gardens)|oreover)|N(?:ever|one)theless|On\s+the\s+other\s+hand|Therefore|Subsequently(?!\s+(?:[a-z]+ed|known|rebuilt|told)))(?!\s+\|)?(?=\s+[\p{L}´ˈ׳᾿’′Ꞌꞌ`'\[\|\]]+\b)";
        assert_eq!(
            fix(pattern, "$1,", "Furthermore the plan failed."),
            "Furthermore, the plan failed."
        );
        assert_eq!(
            fix(pattern, "$1,", "Meanwhile Gardens opened."),
            "Meanwhile Gardens opened."
        );
        assert!(
            DotNetRegex::new(r"a(?=b)*b")
                .unwrap()
                .is_match("ab")
                .unwrap()
        );
        assert!(
            DotNetRegex::new(r"a(?=c)+")
                .unwrap()
                .is_match("ac")
                .unwrap()
        );
        assert!(
            !DotNetRegex::new(r"a(?=c)+")
                .unwrap()
                .is_match("ab")
                .unwrap()
        );
        assert!(DotNetRegex::new(r"a\b?b").unwrap().is_match("ab").unwrap());
    }

    #[test]
    fn lookahead_and_word_boundaries_inside_lookbehind() {
        let pattern = r"\b([cC])orr[ei]spondan(c(?:es?|y)|ts?)\b(?!\s+[dD]es?\b)(?<!\b(?:des?|et|[lL](?:a|es))\s+\b[cC]orr[ei]spondan(?:c(?:es?|y)|ts?)\b(?!\s+[dD]es?\b))";
        assert_eq!(
            fix(pattern, "$1orresponden$2", "the correspondance"),
            "the correspondence"
        );
        assert_eq!(
            fix(pattern, "$1orresponden$2", "la correspondance"),
            "la correspondance"
        );
    }

    #[test]
    fn unbounded_lookbehind_with_backreference() {
        let pattern = r"\bL(iterature)(?<=(?:Chinese|English|Spanish)\s+L\1)(?<=\b(?:(?:degrees?|stud(?:ents?|ies))\s+(?:in|of)|read|stud(?:ie[ds]|y(?:ing)?)|t(?:aught|each(?:es|ing)?))[\s\w]+)(?=(?:\s+(?:a(?:nd\s+an?|t)|by|from|in|with)\s|[,;\.\)]|\s+\())";
        assert_eq!(
            fix(pattern, "l$1", "She studied English Literature at Oxford."),
            "She studied English literature at Oxford."
        );
        assert_eq!(
            fix(pattern, "l$1", "English Literature at Oxford."),
            "English Literature at Oxford."
        );
    }

    #[test]
    fn variable_lookbehind_alternatives() {
        let pattern = r"\bDean(?<=[,\.]\s{1,99}\w{1,99}|\s[a-z]{1,99}\s{1,99}\w{1,99}|\bThe\s+\w+|\(\w+)(?<!Office\s[a-z\s]+\w+)\s+[oO]f\s+[sS](ciences?|tud(?:ents|ies))(?=(?:[,\.\)]|\s+(?:a(?:nd|t)|[a-z]{4,20}|for|in)))";
        assert_eq!(
            fix(pattern, "dean of s$1", "He became Dean of Students, then"),
            "He became dean of students, then"
        );
        let pattern = r"\bManagement\b(?<=\b(?:[a-z\d]+|Its|The|\w+['’´ˈ׳᾿‘′Ꞌꞌ`]s)\s+\w+)\s+[bB]oard(?=(?:[,;\.\)]|\s+[a-z\(]))(?!\s+of\s+Cabinet)";
        assert_eq!(
            fix(pattern, "management board", "the Management Board."),
            "the management board."
        );
    }

    #[test]
    fn lookahead_before_lookbehind_and_unicode_property() {
        assert_eq!(
            fix(
                r"(?=at)(?<=\b(?:[A-Z][a-z]*|[a-z]+))ati?oin(al(?:ly)?|ed|ing|s?)\b",
                "ation$1",
                "a natioinal"
            ),
            "a national"
        );
        assert_eq!(
            fix(
                r"\butah(?!'')(ans?)?\b(?<!\b\p{Lu}\.\s+utah\b)",
                "Utah$1",
                "utah and J. utah"
            ),
            "Utah and J. utah"
        );
        assert_eq!(
            fix(
                r"\b([µmkMGT\s]|[-\d\.]+(?:&nbsp;|-)?)(?:h[zZ]|㎐)\b(?<!\b[µmkMGT]hz)",
                "$1&nbsp;Hz",
                "50hz"
            ),
            "50&nbsp;Hz"
        );
    }
}
