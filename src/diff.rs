use serde::Serialize;
use similar::{ChangeTag, DiffOp, InlineChange, TextDiff};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiffFragment {
    pub text: String,
    pub emphasized: bool,
}

#[derive(Serialize)]
pub struct DiffLine {
    pub kind: &'static str,
    pub marker: char,
    pub fragments: Vec<DiffFragment>,
}

#[derive(Serialize)]
pub struct SideBySideLine {
    pub left_number: String,
    pub left_kind: &'static str,
    pub left_marker: char,
    pub left_fragments: Vec<DiffFragment>,
    pub right_number: String,
    pub right_kind: &'static str,
    pub right_marker: char,
    pub right_fragments: Vec<DiffFragment>,
}

struct InlineLine {
    old_index: Option<usize>,
    new_index: Option<usize>,
    fragments: Vec<DiffFragment>,
}

pub fn unified_lines(original: &str, proposed: &str) -> Vec<DiffLine> {
    let diff = TextDiff::from_lines(original, proposed);
    let mut lines = Vec::new();
    for (index, group) in diff.grouped_ops(1).into_iter().enumerate() {
        if index > 0 {
            lines.push(DiffLine {
                kind: "separator",
                marker: '…',
                fragments: Vec::new(),
            });
        }
        lines.extend(group.iter().flat_map(|operation| {
            diff.iter_inline_changes(operation).map(|change| {
                let (kind, marker) = change_style(change.tag());
                DiffLine {
                    kind,
                    marker,
                    fragments: fragments(&change),
                }
            })
        }));
    }
    lines
}

pub fn side_by_side_lines(original: &str, proposed: &str) -> Vec<SideBySideLine> {
    let diff = TextDiff::from_lines(original, proposed);
    let mut rows = Vec::new();

    for (group_index, group) in diff.grouped_ops(1).into_iter().enumerate() {
        if group_index > 0 {
            rows.push(side_row(
                None,
                "separator",
                '…',
                Vec::new(),
                None,
                "separator",
                '…',
                Vec::new(),
            ));
        }
        for operation in group {
            let changes: Vec<_> = diff
                .iter_inline_changes(&operation)
                .map(|change| {
                    let tag = change.tag();
                    (
                        tag,
                        InlineLine {
                            old_index: change.old_index(),
                            new_index: change.new_index(),
                            fragments: fragments(&change),
                        },
                    )
                })
                .collect();

            match operation {
                DiffOp::Equal { .. } => {
                    for (_, line) in changes {
                        rows.push(side_row(
                            line.old_index,
                            "context",
                            ' ',
                            line.fragments.clone(),
                            line.new_index,
                            "context",
                            ' ',
                            line.fragments,
                        ));
                    }
                }
                DiffOp::Delete { .. } => {
                    for (_, line) in changes {
                        rows.push(side_row(
                            line.old_index,
                            "removed",
                            '-',
                            line.fragments,
                            None,
                            "empty",
                            ' ',
                            Vec::new(),
                        ));
                    }
                }
                DiffOp::Insert { .. } => {
                    for (_, line) in changes {
                        rows.push(side_row(
                            None,
                            "empty",
                            ' ',
                            Vec::new(),
                            line.new_index,
                            "added",
                            '+',
                            line.fragments,
                        ));
                    }
                }
                DiffOp::Replace { .. } => {
                    let mut deleted = changes
                        .iter()
                        .filter(|(tag, _)| *tag == ChangeTag::Delete)
                        .map(|(_, line)| line)
                        .peekable();
                    let mut inserted = changes
                        .iter()
                        .filter(|(tag, _)| *tag == ChangeTag::Insert)
                        .map(|(_, line)| line)
                        .peekable();
                    while deleted.peek().is_some() || inserted.peek().is_some() {
                        let left = deleted.next();
                        let right = inserted.next();
                        rows.push(side_row(
                            left.and_then(|line| line.old_index),
                            if left.is_some() { "removed" } else { "empty" },
                            if left.is_some() { '-' } else { ' ' },
                            left.map(|line| line.fragments.clone()).unwrap_or_default(),
                            right.and_then(|line| line.new_index),
                            if right.is_some() { "added" } else { "empty" },
                            if right.is_some() { '+' } else { ' ' },
                            right.map(|line| line.fragments.clone()).unwrap_or_default(),
                        ));
                    }
                }
            }
        }
    }
    rows
}

#[allow(clippy::too_many_arguments)]
fn side_row(
    left_index: Option<usize>,
    left_kind: &'static str,
    left_marker: char,
    left_fragments: Vec<DiffFragment>,
    right_index: Option<usize>,
    right_kind: &'static str,
    right_marker: char,
    right_fragments: Vec<DiffFragment>,
) -> SideBySideLine {
    SideBySideLine {
        left_number: line_number(left_index),
        left_kind,
        left_marker,
        left_fragments,
        right_number: line_number(right_index),
        right_kind,
        right_marker,
        right_fragments,
    }
}

fn line_number(index: Option<usize>) -> String {
    index
        .map(|index| (index + 1).to_string())
        .unwrap_or_default()
}

fn change_style(tag: ChangeTag) -> (&'static str, char) {
    match tag {
        ChangeTag::Delete => ("removed", '-'),
        ChangeTag::Insert => ("added", '+'),
        ChangeTag::Equal => ("context", ' '),
    }
}

fn fragments(change: &InlineChange<'_, str>) -> Vec<DiffFragment> {
    let mut fragments: Vec<_> = change
        .iter_strings_lossy()
        .map(|(emphasized, value)| DiffFragment {
            text: value.into_owned(),
            emphasized,
        })
        .collect();
    if let Some(last) = fragments.last_mut() {
        last.text = last.text.trim_end_matches('\n').to_owned();
    }
    fragments
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(fragments: &[DiffFragment]) -> String {
        fragments
            .iter()
            .map(|fragment| fragment.text.as_str())
            .collect()
    }

    #[test]
    fn unified_diff_marks_removed_and_added_lines() {
        let lines = unified_lines("one\ntwo words\n", "one\nthree words\n");
        let removed = lines.iter().find(|line| line.kind == "removed").unwrap();
        let added = lines.iter().find(|line| line.kind == "added").unwrap();
        assert_eq!(text(&removed.fragments), "two words");
        assert_eq!(text(&added.fragments), "three words");
        assert!(
            removed
                .fragments
                .iter()
                .any(|fragment| fragment.emphasized && fragment.text.contains("two"))
        );
        assert!(
            added
                .fragments
                .iter()
                .any(|fragment| fragment.emphasized && fragment.text.contains("three"))
        );
    }

    #[test]
    fn side_by_side_diff_aligns_replaced_lines() {
        let lines = side_by_side_lines("one\ntwo words\n", "one\nthree words\n");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].left_number, "2");
        assert_eq!(lines[1].left_marker, '-');
        assert_eq!(text(&lines[1].left_fragments), "two words");
        assert_eq!(lines[1].right_number, "2");
        assert_eq!(lines[1].right_marker, '+');
        assert_eq!(text(&lines[1].right_fragments), "three words");
    }

    #[test]
    fn side_by_side_diff_leaves_empty_cell_for_insertions() {
        let lines = side_by_side_lines("one\n", "one\ntwo\n");
        assert_eq!(lines[1].left_kind, "empty");
        assert!(lines[1].left_fragments.is_empty());
        assert_eq!(lines[1].right_kind, "added");
        assert_eq!(text(&lines[1].right_fragments), "two");
    }

    #[test]
    fn unchanged_files_produce_no_diff_lines() {
        assert!(unified_lines("спільний текст\n", "спільний текст\n").is_empty());
    }

    #[test]
    fn diff_keeps_one_context_line_around_each_change() {
        let lines = unified_lines(
            "one\ntwo\nthree\nfour\nfive\nsix\nseven\n",
            "one\nchanged\nthree\nfour\nfive\nchanged too\nseven\n",
        );
        let context: Vec<_> = lines
            .iter()
            .filter(|line| line.kind == "context")
            .map(|line| text(&line.fragments))
            .collect();
        assert_eq!(context, ["one", "three", "five", "seven"]);
        assert_eq!(
            lines.iter().filter(|line| line.kind == "separator").count(),
            1
        );
    }

    #[test]
    fn side_by_side_diff_keeps_three_distant_replacements() {
        let lines = side_by_side_lines(
            "first old\ncontext a\ncontext b\nsecond old\ncontext c\ncontext d\nthird old\n",
            "first new\ncontext a\ncontext b\nsecond new\ncontext c\ncontext d\nthird new\n",
        );
        let removed: Vec<_> = lines
            .iter()
            .filter(|line| line.left_kind == "removed")
            .map(|line| text(&line.left_fragments))
            .collect();
        let added: Vec<_> = lines
            .iter()
            .filter(|line| line.right_kind == "added")
            .map(|line| text(&line.right_fragments))
            .collect();
        assert_eq!(removed, ["first old", "second old", "third old"]);
        assert_eq!(added, ["first new", "second new", "third new"]);
    }
}
