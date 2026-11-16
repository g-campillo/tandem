//! Where the parts of a step's breakdown occur in its code or command, so the chat panel
//! can show a part's explanation when the user points at it.
use std::ops::Range;

/// The byte ranges of `text` where each part occurs, one list per part, in the order of
/// `parts`. An empty list means the part is not in `text`.
pub fn part_spans(text: &str, parts: &[&str]) -> Vec<Vec<Range<usize>>> {
    let parts: Vec<&str> = parts.iter().map(|part| part.trim()).collect();
    parts
        .iter()
        .enumerate()
        .map(|(index, part)| {
            if part.is_empty() {
                return Vec::new();
            }
            let mut found = exact(text, part);
            if found.is_empty() {
                found = loose(text, part);
            }
            // Parts with the same text share its occurrences in text order.
            let order = parts[..index].iter().filter(|twin| *twin == part).count();
            let last = !parts[index + 1..].contains(part);
            if last {
                found.get(order..).map_or_else(Vec::new, <[_]>::to_vec)
            } else {
                found.get(order).cloned().into_iter().collect()
            }
        })
        .collect()
}

/// The part whose span contains byte `index`: the shortest span wins, then the part
/// listed first. `None` when no span contains it.
pub fn part_at(spans: &[Vec<Range<usize>>], index: usize) -> Option<usize> {
    spans
        .iter()
        .enumerate()
        .flat_map(|(part, spans)| {
            spans
                .iter()
                .filter(|span| span.contains(&index))
                .map(move |span| (span.len(), part))
        })
        .min()
        .map(|(_, part)| part)
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A match must not cut a word in two: when it starts (ends) with a word character, the
/// character before (after) it must not be one.
fn on_boundaries(text: &str, start: usize, end: usize) -> bool {
    let first = text[start..end].chars().next();
    let last = text[start..end].chars().next_back();
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    !(first.is_some_and(is_word) && before.is_some_and(is_word))
        && !(last.is_some_and(is_word) && after.is_some_and(is_word))
}

/// The offset one character after `start`.
fn step(text: &str, start: usize) -> usize {
    start + text[start..].chars().next().map_or(1, char::len_utf8)
}

fn exact(text: &str, needle: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find(needle) {
        let start = from + at;
        let end = start + needle.len();
        if on_boundaries(text, start, end) {
            found.push(start..end);
            from = end;
        } else {
            from = step(text, start);
        }
    }
    found
}

/// Matches a part of several words with any run of whitespace between the words.
fn loose(text: &str, needle: &str) -> Vec<Range<usize>> {
    let words: Vec<&str> = needle.split_whitespace().collect();
    if words.len() < 2 {
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find(words[0]) {
        let start = from + at;
        let mut end = start + words[0].len();
        let mut matched = true;
        for word in &words[1..] {
            let rest = &text[end..];
            let gap = rest.len() - rest.trim_start().len();
            if gap == 0 || !rest[gap..].starts_with(word) {
                matched = false;
                break;
            }
            end += gap + word.len();
        }
        if matched && on_boundaries(text, start, end) {
            found.push(start..end);
            from = end;
        } else {
            from = step(text, start);
        }
    }
    found
}

#[cfg(test)]
// A part found once has a list of one span, so `vec![4..16]` is the intended value.
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    const CODE: &str = "    #[arg(long)]\n    json: bool,";

    #[test]
    fn parts_of_rust_code_and_the_part_under_an_index() {
        let spans = part_spans(CODE, &["#[arg(long)]", "json", "bool", ",", "long", "arg"]);
        assert_eq!(
            spans,
            [
                vec![4..16],
                vec![21..25],
                vec![27..31],
                vec![31..32],
                vec![10..14],
                vec![6..9]
            ]
        );
        assert_eq!(part_at(&spans, 11), Some(4));
        assert_eq!(part_at(&spans, 5), Some(0));
        assert_eq!(part_at(&spans, 0), None);
    }

    #[test]
    fn a_part_does_not_match_inside_a_word() {
        assert_eq!(
            part_spans("let a = data.len(); a", &["a", "data", ".len()"]),
            [vec![4..5, 20..21], vec![8..12], vec![12..18]]
        );
    }

    #[test]
    fn twins_take_one_occurrence_each_and_the_last_takes_the_rest() {
        assert_eq!(
            part_spans("&x + &y + &z", &["&", "&"]),
            [vec![0..1], vec![5..6, 10..11]]
        );
        assert_eq!(
            part_spans("&x", &["&", "&", "&"]),
            [vec![0..1], vec![], vec![]]
        );
    }

    #[test]
    fn a_part_with_whitespace_matches_any_run_of_whitespace() {
        assert_eq!(
            part_spans(CODE, &["#[arg(long)]\njson: bool,", "json:  bool"]),
            [vec![4..32], vec![21..31]]
        );
    }

    #[test]
    fn parts_that_are_not_in_the_text_have_no_spans() {
        assert_eq!(
            part_spans(CODE, &["Vec<T>", "  ", "jso"]),
            vec![Vec::<Range<usize>>::new(); 3]
        );
    }

    #[test]
    fn spans_fall_on_char_boundaries_of_text_that_is_not_ascii() {
        let text = "let naïve = \"héllo\"; // ünï";
        let spans = part_spans(text, &["naïve", "\"héllo\"", "ï"]);
        assert_eq!(spans, [vec![4..10], vec![13..21], vec![]]);
        for span in spans.iter().flatten() {
            assert!(text.is_char_boundary(span.start), "{span:?}");
            assert!(text.is_char_boundary(span.end), "{span:?}");
        }
    }

    #[test]
    fn parts_of_a_command() {
        let spans = part_spans(
            "cargo test --quiet",
            &["cargo", "test", "--quiet", "cargo test"],
        );
        assert_eq!(spans, [vec![0..5], vec![6..10], vec![11..18], vec![0..10]]);
        assert_eq!(part_at(&spans, 2), Some(0));
    }

    #[test]
    fn part_at_breaks_a_tie_by_the_part_listed_first() {
        assert_eq!(part_at(&[vec![0..3], vec![0..3]], 1), Some(0));
        assert_eq!(part_at(&[], 0), None);
    }
}
