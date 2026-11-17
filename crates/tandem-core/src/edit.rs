//! Pure text transformations behind the editor commands. Offsets are UTF-8 byte offsets on char boundaries. A line ends at '\n'; a '\r' right before it belongs to the line break.
pub mod comment;
pub mod fuzzy;
pub mod lines;
pub mod select;
pub mod snippet;

use std::ops::Range;

/// One replacement of `range` (offsets in the text before the edit) by `text`,
/// and the selection to show afterwards (offsets in the text after the edit).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
    pub selection: Range<usize>,
}

impl Edit {
    /// The smallest single replacement that turns `old` into `new`.
    pub fn between(old: &str, new: &str, selection: Range<usize>) -> Edit {
        if old == new {
            return Edit {
                range: 0..0,
                text: String::new(),
                selection,
            };
        }
        let (old_bytes, new_bytes) = (old.as_bytes(), new.as_bytes());
        let mut prefix = old_bytes
            .iter()
            .zip(new_bytes)
            .take_while(|(a, b)| a == b)
            .count();
        while !old.is_char_boundary(prefix) {
            prefix -= 1;
        }
        let mut suffix = old_bytes[prefix..]
            .iter()
            .rev()
            .zip(new_bytes[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        while !old.is_char_boundary(old.len() - suffix) {
            suffix -= 1;
        }
        Edit {
            range: prefix..old.len() - suffix,
            text: new[prefix..new.len() - suffix].to_string(),
            selection,
        }
    }

    /// Whether applying the edit changes any text (`!range.is_empty() || !text.is_empty()`).
    pub fn changes_text(&self) -> bool {
        !self.range.is_empty() || !self.text.is_empty()
    }

    /// `old` with `range` replaced by `text`.
    pub fn apply(&self, old: &str) -> String {
        let end = clamp(old, self.range.end);
        let start = clamp(old, self.range.start).min(end);
        let mut new = String::with_capacity(old.len() - (end - start) + self.text.len());
        new.push_str(&old[..start]);
        new.push_str(&self.text);
        new.push_str(&old[end..]);
        new
    }

    /// Where `offset` (in the text before the edit) is after it. Offsets before the range
    /// stay, offsets after it move by the length change, offsets inside it move to its start
    /// plus at most the new length, back to a char boundary of the new text. The end of the
    /// range counts as after it, so an offset at an insertion moves past the inserted text.
    pub fn map_offset(&self, offset: usize) -> usize {
        let start = self.range.start.min(self.range.end);
        if (start..self.range.end).contains(&offset) {
            start + self.text.floor_char_boundary(offset - start)
        } else {
            map_offset_across(&self.range, self.text.len(), offset)
        }
    }
}

/// `Edit::map_offset` for a replacement of `range` by `new_len` bytes. Without the new
/// text, an offset inside the range can end up inside one of its characters.
pub(crate) fn map_offset_across(range: &Range<usize>, new_len: usize, offset: usize) -> usize {
    let end = range.end;
    let start = range.start.min(end);
    if offset < start {
        offset
    } else if offset >= end {
        offset - (end - start) + new_len
    } else {
        start + (offset - start).min(new_len)
    }
}

fn clamp(text: &str, offset: usize) -> usize {
    offset.min(text.len())
}

/// The start of the line that contains `offset`.
pub fn line_start(text: &str, offset: usize) -> usize {
    let offset = clamp(text, offset);
    text[..offset].rfind('\n').map_or(0, |i| i + 1)
}

/// The end of the content of the line that contains `offset`, before its line break.
pub fn line_end(text: &str, offset: usize) -> usize {
    let offset = clamp(text, offset);
    match text[offset..].find('\n') {
        Some(i) => {
            let newline = offset + i;
            let crlf = newline > line_start(text, offset) && text.as_bytes()[newline - 1] == b'\r';
            newline - usize::from(crlf)
        }
        None => text.len(),
    }
}

/// The start of the line after the one that contains `offset`, `None` on the last line.
pub fn next_line_start(text: &str, offset: usize) -> Option<usize> {
    let offset = clamp(text, offset);
    text[offset..].find('\n').map(|i| offset + i + 1)
}

/// The byte length of the leading run of spaces and tabs.
pub fn indent_len(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

/// The whole lines a selection touches: `start` is the start of the first line,
/// `end` the end of the last line's content (before its line break).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineBlock {
    pub start: usize,
    pub end: usize,
}

/// The lines `selection` touches. A non-empty selection that ends at the start of a
/// line does not include that line.
pub fn line_block(text: &str, selection: &Range<usize>) -> LineBlock {
    let start = clamp(text, selection.start);
    let end = clamp(text, selection.end).max(start);
    let last = if end > start && line_start(text, end) == end {
        end - 1
    } else {
        end
    };
    LineBlock {
        start: line_start(text, start),
        end: line_end(text, last),
    }
}

/// The content range of every line of `block`, in order.
pub fn block_lines(text: &str, block: LineBlock) -> Vec<Range<usize>> {
    let mut lines = Vec::new();
    let mut start = clamp(text, block.start);
    loop {
        let end = line_end(text, start);
        lines.push(start..end);
        match next_line_start(text, end) {
            Some(next) if end < block.end => start = next,
            _ => return lines,
        }
    }
}

/// Whether `c` belongs to a word: a letter, a digit or `_`.
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The maximal run of word chars that contains the char at `offset` or ends exactly at `offset`.
pub fn word_range_at(text: &str, offset: usize) -> Option<Range<usize>> {
    let offset = clamp(text, offset);
    let start = offset - word_len(text[..offset].chars().rev());
    let end = offset + word_len(text[offset..].chars());
    (start < end).then_some(start..end)
}

/// The byte length of the leading run of word chars in `chars`.
fn word_len(chars: impl Iterator<Item = char>) -> usize {
    chars
        .take_while(|c| is_word_char(*c))
        .map(char::len_utf8)
        .sum()
}

/// One change inside an operation: `remove` bytes at `at` are replaced by `insert`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Splice<'a> {
    pub at: usize,
    pub remove: usize,
    pub insert: &'a str,
}

/// Applies `splices` (ascending by `at`, not overlapping) to `text` and maps `selection`
/// into the new text. An insertion exactly at a collapsed cursor or at a selection's end
/// moves that point after it; at a non-empty selection's start it leaves the start in place.
pub fn splice(
    text: &str,
    splices: &[Splice<'_>],
    selection: &Range<usize>,
) -> (String, Range<usize>) {
    // Clamp every splice into the text and behind the previous one so slicing cannot panic.
    let mut previous_end = 0;
    let splices: Vec<Splice<'_>> = splices
        .iter()
        .map(|s| {
            let at = clamp(text, s.at).max(previous_end);
            let end = clamp(text, s.at.saturating_add(s.remove)).max(at);
            previous_end = end;
            Splice {
                at,
                remove: end - at,
                insert: s.insert,
            }
        })
        .collect();

    let mut new = String::with_capacity(text.len());
    let mut copied = 0;
    for s in &splices {
        new.push_str(&text[copied..s.at]);
        new.push_str(s.insert);
        copied = s.at + s.remove;
    }
    new.push_str(&text[copied..]);

    let map = |point: usize, stick_after: bool| -> usize {
        let (mut inserted, mut removed) = (0, 0);
        for s in &splices {
            if point < s.at || (s.remove == 0 && point == s.at && !stick_after) {
                break;
            }
            if point < s.at + s.remove {
                return s.at + inserted - removed;
            }
            inserted += s.insert.len();
            removed += s.remove;
        }
        point + inserted - removed
    };
    let start = clamp(text, selection.start);
    let end = clamp(text, selection.end).max(start);
    let mapped = if start == end {
        let point = map(start, true);
        point..point
    } else {
        map(start, false)..map(end, true)
    };
    (new, mapped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn between(old: &str, new: &str) -> Edit {
        Edit::between(old, new, 0..0)
    }

    #[test]
    fn between_trims_the_common_prefix_and_suffix() {
        let edit = between("abcXdef", "abcYYdef");
        assert_eq!(edit.range, 3..4);
        assert_eq!(edit.text, "YY");
        assert!(edit.changes_text());
    }

    #[test]
    fn between_backs_off_to_char_boundaries() {
        // é and è share their first byte.
        let edit = between("aé", "aè");
        assert_eq!((edit.range, edit.text.as_str()), (1..3, "è"));
        // é and ũ share their last byte.
        let edit = between("aé", "aũ");
        assert_eq!((edit.range, edit.text.as_str()), (1..3, "ũ"));
    }

    #[test]
    fn between_equal_texts_changes_nothing_but_keeps_the_selection() {
        let edit = Edit::between("same", "same", 2..4);
        assert!(!edit.changes_text());
        assert_eq!(edit.selection, 2..4);
        assert_eq!(edit.apply("same"), "same");
    }

    #[test]
    fn apply_of_between_returns_the_new_text() {
        let cases = [
            ("hello world", "hello brave world"),
            ("hello brave world", "hello world"),
            ("hello world", "hello there"),
            ("", "abc"),
            ("abc", ""),
            ("aaa", "aaaa"),
            ("naïve", "naive"),
        ];
        for (old, new) in cases {
            let edit = between(old, new);
            assert_eq!(edit.apply(old), new, "{old:?} -> {new:?}");
        }
    }

    fn replacement(range: Range<usize>, text: &str) -> Edit {
        Edit {
            range,
            text: text.to_string(),
            selection: 0..0,
        }
    }

    #[test]
    fn map_offset_keeps_offsets_before_the_range() {
        let edit = replacement(4..7, "ab");
        assert_eq!(edit.map_offset(0), 0);
        assert_eq!(edit.map_offset(3), 3);
        assert_eq!(edit.map_offset(4), 4);
    }

    #[test]
    fn map_offset_moves_offsets_after_the_range_by_the_length_change() {
        let shorter = replacement(4..7, "ab");
        assert_eq!(shorter.map_offset(7), 6);
        assert_eq!(shorter.map_offset(10), 9);
        let longer = replacement(4..7, "abcde");
        assert_eq!(longer.map_offset(7), 9);
        assert_eq!(longer.map_offset(10), 12);
    }

    #[test]
    fn map_offset_moves_offsets_inside_the_range_to_its_start_plus_at_most_the_new_length() {
        let shorter = replacement(4..8, "ab");
        assert_eq!(shorter.map_offset(5), 5);
        assert_eq!(shorter.map_offset(6), 6);
        assert_eq!(shorter.map_offset(7), 6);
        let deletion = replacement(4..8, "");
        assert_eq!(deletion.map_offset(6), 4);
        let longer = replacement(4..8, "abcdef");
        assert_eq!(longer.map_offset(7), 7);
    }

    #[test]
    fn map_offset_keeps_an_offset_inside_the_range_on_a_char_boundary_of_the_new_text() {
        // "é" and "ñ" are two bytes each: 5 would be inside "é".
        let multibyte = replacement(4..8, "éñ");
        assert_eq!(multibyte.map_offset(5), 4);
        assert_eq!(multibyte.map_offset(6), 6);
        assert_eq!(multibyte.map_offset(7), 6);
    }

    #[test]
    fn map_offset_moves_an_offset_at_an_insertion_after_it() {
        let insertion = replacement(3..3, "xy");
        assert_eq!(insertion.map_offset(2), 2);
        assert_eq!(insertion.map_offset(3), 5);
        assert_eq!(insertion.map_offset(4), 6);
    }

    #[test]
    fn line_start_end_and_next_line_start() {
        let text = "ab\ncd\r\nef";
        let starts = [
            (0, 0),
            (1, 0),
            (2, 0),
            (3, 3),
            (4, 3),
            (5, 3),
            (7, 7),
            (8, 7),
            (9, 7),
        ];
        for (offset, start) in starts {
            assert_eq!(line_start(text, offset), start, "line_start at {offset}");
        }
        let ends = [
            (0, 2),
            (2, 2),
            (3, 5),
            (4, 5),
            (5, 5),
            (7, 9),
            (8, 9),
            (9, 9),
        ];
        for (offset, end) in ends {
            assert_eq!(line_end(text, offset), end, "line_end at {offset}");
        }
        let next = [
            (0, Some(3)),
            (2, Some(3)),
            (3, Some(7)),
            (5, Some(7)),
            (7, None),
            (9, None),
        ];
        for (offset, start) in next {
            assert_eq!(
                next_line_start(text, offset),
                start,
                "next_line_start at {offset}"
            );
        }
    }

    #[test]
    fn line_end_keeps_a_lone_carriage_return() {
        assert_eq!(line_end("ab\r", 0), 3);
        assert_eq!(line_end("ab\rcd\n", 0), 5);
        assert_eq!(line_end("\r\n", 0), 0);
    }

    #[test]
    fn offsets_past_the_end_are_clamped() {
        let text = "ab\ncd";
        assert_eq!(line_start(text, 99), 3);
        assert_eq!(line_end(text, 99), 5);
        assert_eq!(next_line_start(text, 99), None);
        assert_eq!(line_block(text, &(1..99)), LineBlock { start: 0, end: 5 });
        assert_eq!(word_range_at(text, 99), Some(3..5));
    }

    #[test]
    fn indent_len_counts_leading_spaces_and_tabs() {
        assert_eq!(indent_len("    x"), 4);
        assert_eq!(indent_len("\t \tx "), 3);
        assert_eq!(indent_len("x  "), 0);
        assert_eq!(indent_len("   "), 3);
        assert_eq!(indent_len(""), 0);
    }

    #[test]
    fn line_block_of_a_collapsed_cursor_is_its_line() {
        assert_eq!(
            line_block("ab\ncd\nef", &(4..4)),
            LineBlock { start: 3, end: 5 }
        );
        assert_eq!(
            line_block("ab\ncd\nef", &(3..3)),
            LineBlock { start: 3, end: 5 }
        );
    }

    #[test]
    fn line_block_spans_every_touched_line() {
        assert_eq!(
            line_block("ab\ncd\nef\n", &(1..7)),
            LineBlock { start: 0, end: 8 }
        );
    }

    #[test]
    fn line_block_excludes_a_line_the_selection_ends_at_column_zero_of() {
        assert_eq!(
            line_block("ab\ncd\nef", &(1..6)),
            LineBlock { start: 0, end: 5 }
        );
    }

    #[test]
    fn line_block_reaches_the_last_line_without_a_trailing_newline() {
        assert_eq!(
            line_block("ab\ncd", &(4..4)),
            LineBlock { start: 3, end: 5 }
        );
        assert_eq!(
            line_block("ab\ncd", &(1..5)),
            LineBlock { start: 0, end: 5 }
        );
    }

    #[test]
    fn line_block_ends_before_a_crlf_break() {
        assert_eq!(
            line_block("ab\r\ncd\r\n", &(1..1)),
            LineBlock { start: 0, end: 2 }
        );
        assert_eq!(
            line_block("ab\r\ncd\r\n", &(0..4)),
            LineBlock { start: 0, end: 2 }
        );
        assert_eq!(
            line_block("ab\r\ncd\r\n", &(0..5)),
            LineBlock { start: 0, end: 6 }
        );
    }

    #[test]
    fn block_lines_returns_each_line_content_range() {
        let text = "ab\n\ncd\r\nef";
        let block = line_block(text, &(0..text.len()));
        assert_eq!(block_lines(text, block), vec![0..2, 3..3, 4..6, 8..10]);
        assert_eq!(block_lines("", line_block("", &(0..0))), vec![0..0]);
        assert_eq!(
            block_lines(text, LineBlock { start: 4, end: 6 }),
            vec![4..6]
        );
    }

    fn spliced(
        text: &str,
        splices: &[Splice<'_>],
        selection: Range<usize>,
    ) -> (String, Range<usize>) {
        splice(text, splices, &selection)
    }

    #[test]
    fn splice_maps_points_around_a_removal() {
        let removal = [Splice {
            at: 2,
            remove: 3,
            insert: "",
        }];
        assert_eq!(
            spliced("hello world", &removal, 1..1),
            ("he world".to_string(), 1..1)
        );
        assert_eq!(spliced("hello world", &removal, 2..2).1, 2..2);
        assert_eq!(spliced("hello world", &removal, 3..3).1, 2..2);
        assert_eq!(spliced("hello world", &removal, 5..5).1, 2..2);
        assert_eq!(spliced("hello world", &removal, 8..8).1, 5..5);
        assert_eq!(spliced("hello world", &removal, 1..8).1, 1..5);
    }

    #[test]
    fn splice_replaces_and_shifts_later_points() {
        let replacement = [
            Splice {
                at: 0,
                remove: 1,
                insert: "H",
            },
            Splice {
                at: 6,
                remove: 5,
                insert: "there!",
            },
        ];
        let (text, selection) = spliced("hello world", &replacement, 5..5);
        assert_eq!((text.as_str(), selection), ("Hello there!", 5..5));
    }

    #[test]
    fn splice_insertion_at_a_collapsed_cursor_moves_it_after() {
        let insertion = [Splice {
            at: 1,
            remove: 0,
            insert: "XY",
        }];
        assert_eq!(
            spliced("abc", &insertion, 1..1),
            ("aXYbc".to_string(), 3..3)
        );
    }

    #[test]
    fn splice_insertion_at_a_selection_start_keeps_it_and_at_its_end_moves_it() {
        let at_start = [Splice {
            at: 1,
            remove: 0,
            insert: "XY",
        }];
        assert_eq!(spliced("abcd", &at_start, 1..3).1, 1..5);
        let at_end = [Splice {
            at: 3,
            remove: 0,
            insert: "XY",
        }];
        assert_eq!(spliced("abcd", &at_end, 1..3).1, 1..5);
        let both = [
            Splice {
                at: 1,
                remove: 0,
                insert: "<",
            },
            Splice {
                at: 3,
                remove: 0,
                insert: ">",
            },
        ];
        assert_eq!(spliced("abcd", &both, 1..3), ("a<bc>d".to_string(), 1..5));
    }

    #[test]
    fn word_range_at_finds_the_word_at_or_before_the_offset() {
        assert_eq!(word_range_at("foo bar", 1), Some(0..3));
        assert_eq!(word_range_at("foo bar", 0), Some(0..3));
        assert_eq!(word_range_at("foo bar", 3), Some(0..3));
        assert_eq!(word_range_at("foo bar", 4), Some(4..7));
        assert_eq!(word_range_at("foo bar", 7), Some(4..7));
    }

    #[test]
    fn word_range_at_punctuation_without_a_word_is_none() {
        assert_eq!(word_range_at("a + b", 2), None);
        assert_eq!(word_range_at("a  b", 2), None);
        assert_eq!(word_range_at("", 0), None);
    }

    #[test]
    fn word_range_at_handles_non_ascii_letters_and_underscores() {
        assert_eq!(word_range_at("über straße", 2), Some(0..5));
        assert_eq!(word_range_at("über straße", 13), Some(6..13));
        assert_eq!(word_range_at("let my_var = 1", 6), Some(4..10));
        assert!(is_word_char('_') && is_word_char('é') && is_word_char('7'));
        assert!(!is_word_char('-') && !is_word_char(' '));
    }
}
