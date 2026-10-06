use std::ops::Range;

use crate::projects::RuleMask;

pub fn masked_ranges(text: &str, mask: &RuleMask) -> Vec<Range<usize>> {
    if !mask.any() {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    if mask.comments {
        let mut position = 0;
        while let Some(relative) = text[position..].find("<!--") {
            let start = position + relative;
            let end = text[start + 4..]
                .find("-->")
                .map_or(text.len(), |index| start + 4 + index + 3);
            ranges.push(start..end);
            position = end;
        }
    }
    if mask.headings {
        let mut start = 0;
        for line in text.split_inclusive('\n') {
            let trimmed = line.trim();
            let opening = trimmed.bytes().take_while(|byte| *byte == b'=').count();
            if (1..=6).contains(&opening)
                && trimmed.len() >= opening * 2
                && trimmed
                    .bytes()
                    .rev()
                    .take_while(|byte| *byte == b'=')
                    .count()
                    >= opening
            {
                ranges.push(start..start + line.len());
            }
            start += line.len();
        }
    }
    if mask.tags || mask.references {
        let mut position = 0;
        let mut open: Vec<(String, usize)> = Vec::new();
        while let Some(relative) = text[position..].find('<') {
            let start = position + relative;
            if text[start..].starts_with("<!--") {
                position = text[start + 4..]
                    .find("-->")
                    .map_or(text.len(), |index| start + 4 + index + 3);
                continue;
            }
            let Some(end) = tag_end(text, start) else {
                break;
            };
            position = end;
            let inside = text[start + 1..end - 1].trim();
            let closing = inside.starts_with('/');
            let inside = inside.strip_prefix('/').unwrap_or(inside);
            if !inside
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_alphabetic())
            {
                continue;
            }
            let name: String = inside
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric())
                .collect::<String>()
                .to_ascii_lowercase();
            if name.is_empty() || !(mask.tags || mask.references && name == "ref") {
                continue;
            }
            if closing {
                if let Some(index) = open.iter().rposition(|(opened, _)| opened == &name) {
                    let (_, opening) = open.remove(index);
                    ranges.push(opening..end);
                } else {
                    ranges.push(start..end);
                }
            } else if inside.ends_with('/')
                || matches!(
                    name.as_str(),
                    "br" | "hr" | "img" | "meta" | "link" | "input" | "wbr"
                )
            {
                ranges.push(start..end);
            } else {
                open.push((name, start));
            }
        }
        for (_, start) in open {
            ranges.push(start..text.len());
        }
    }
    if mask.links || mask.link_targets || mask.templates {
        let bytes = text.as_bytes();
        let mut position = 0;
        let mut links: Vec<(usize, Option<usize>)> = Vec::new();
        let mut templates = Vec::new();
        while position + 1 < bytes.len() {
            match (bytes[position], bytes[position + 1]) {
                (b'[', b'[') => {
                    if mask.links || mask.link_targets {
                        links.push((position, None));
                    }
                    position += 2;
                }
                (b']', b']') => {
                    if let Some((start, separator)) = links.pop() {
                        if mask.links {
                            ranges.push(start..position + 2);
                        } else if mask.link_targets {
                            ranges.push(start + 2..separator.unwrap_or(position));
                        }
                    }
                    position += 2;
                }
                (b'|', _) => {
                    if let Some((_, separator)) = links.last_mut()
                        && separator.is_none()
                    {
                        *separator = Some(position);
                    }
                    position += 1;
                }
                (b'{', b'{') => {
                    if mask.templates {
                        templates.push(position);
                    }
                    position += 2;
                }
                (b'}', b'}') => {
                    if let Some(start) = templates.pop() {
                        ranges.push(start..position + 2);
                    }
                    position += 2;
                }
                _ => position += 1,
            }
        }
        for (start, _) in links {
            ranges.push(start..text.len());
        }
        for start in templates {
            ranges.push(start..text.len());
        }
    }
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        if range.is_empty() {
            continue;
        }
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

fn tag_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut quote = None;
    for (index, byte) in bytes.iter().enumerate().skip(start + 1) {
        match (*byte, quote) {
            (b'\'' | b'"', None) => quote = Some(*byte),
            (value, Some(expected)) if value == expected => quote = None,
            (b'>', None) => return Some(index + 1),
            _ => {}
        }
    }
    None
}

pub fn overlaps(ranges: &[Range<usize>], start: usize, end: usize) -> bool {
    ranges.iter().any(|range| {
        if start == end {
            range.start <= start && start < range.end
        } else {
            range.start < end && start < range.end
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_selected_regions_and_leaves_other_text() {
        let text =
            "word <!-- word --> [[Target|word]]\n== word ==\n{{T|word}} <ref>word</ref> word";
        let mask = RuleMask {
            comments: true,
            link_targets: true,
            headings: true,
            templates: true,
            references: true,
            ..RuleMask::default()
        };
        let ranges = masked_ranges(text, &mask);
        let positions: Vec<_> = text.match_indices("word").map(|(start, _)| start).collect();
        assert_eq!(
            positions
                .iter()
                .map(|start| overlaps(&ranges, *start, *start + 4))
                .collect::<Vec<_>>(),
            [false, true, false, true, true, true, false]
        );
        let target = text.find("Target").unwrap();
        assert!(overlaps(&ranges, target, target + 6));
    }

    #[test]
    fn unclosed_region_masks_to_end() {
        let mask = RuleMask {
            comments: true,
            ..RuleMask::default()
        };
        assert_eq!(masked_ranges("A <!-- hidden", &mask), vec![2..13]);
    }
}
