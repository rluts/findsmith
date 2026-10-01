use similar::{ChangeTag, TextDiff};

pub struct DiffLine {
    pub kind: &'static str,
    pub marker: char,
    pub text: String,
}

pub fn unified_lines(original: &str, proposed: &str) -> Vec<DiffLine> {
    TextDiff::from_lines(original, proposed)
        .iter_all_changes()
        .map(|change| {
            let (kind, marker) = match change.tag() {
                ChangeTag::Delete => ("removed", '-'),
                ChangeTag::Insert => ("added", '+'),
                ChangeTag::Equal => ("context", ' '),
            };
            DiffLine {
                kind,
                marker,
                text: change.value().trim_end_matches('\n').to_owned(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unified_diff_marks_removed_and_added_lines() {
        let lines = unified_lines("one\ntwo\n", "one\nthree\n");
        assert!(
            lines
                .iter()
                .any(|line| line.kind == "removed" && line.text == "two")
        );
        assert!(
            lines
                .iter()
                .any(|line| line.kind == "added" && line.text == "three")
        );
    }
}
