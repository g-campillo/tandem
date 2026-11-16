//! Whole-line operations: duplicate, move, delete, join, sort, trim and change case.
use super::{
    Edit, LineBlock, Splice, block_lines, clamp, line_block, line_end, line_start, next_line_start,
    splice, word_range_at,
};
use std::ops::Range;

/// Which way a line operation copies or moves the lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
}

/// Copies the selected lines above (`Up`) or below (`Down`) themselves. The selection
/// stays on the upper copy for `Up` and moves to the lower copy for `Down`.
pub fn duplicate_lines(text: &str, selection: &Range<usize>, direction: Direction) -> Option<Edit> {
    let selection = clamped(text, selection);
    let block = line_block(text, &selection);
    let body = &text[block.start..block.end];
    let new = replaced(text, block.start..block.start, &format!("{body}\n"));
    let selection = match direction {
        // A selection ending after a CRLF break ends after the upper copy's "\n".
        Direction::Up => selection.start..selection.end.min(block.end + 1),
        Direction::Down => {
            let shift = body.len() + 1;
            selection.start + shift..selection.end + shift
        }
    };
    Some(Edit::between(text, &new, selection))
}

/// Swaps the selected lines with the line above (`Up`) or below (`Down`) them, keeping
/// the line break between the two in place. The selection moves with the lines.
pub fn move_lines(text: &str, selection: &Range<usize>, direction: Direction) -> Option<Edit> {
    let selection = clamped(text, selection);
    let block = line_block(text, &selection);
    let body = &text[block.start..block.end];
    let (new, body_start) = match direction {
        Direction::Up => {
            if block.start == 0 {
                return None;
            }
            let prev_start = line_start(text, block.start - 1);
            let prev_end = line_end(text, prev_start);
            let previous = &text[prev_start..prev_end];
            let sep = &text[prev_end..block.start];
            let new = replaced(
                text,
                prev_start..block.end,
                &format!("{body}{sep}{previous}"),
            );
            (new, prev_start)
        }
        Direction::Down => {
            let next = next_line_start(text, block.end)?;
            let next_end = line_end(text, next);
            let following = &text[next..next_end];
            let sep = &text[block.end..next];
            let new = replaced(
                text,
                block.start..next_end,
                &format!("{following}{sep}{body}"),
            );
            (new, block.start + following.len() + sep.len())
        }
    };
    // Each point keeps its offset from the start of the moved lines. A point after them
    // stays within the line break that now follows them, which can be shorter than their
    // old one, or missing when they became the last line.
    let limit = next_line_start(&new, body_start + body.len()).unwrap_or(new.len());
    let to_new = |point: usize| (point - block.start + body_start).min(limit);
    let selection = to_new(selection.start)..to_new(selection.end);
    Some(Edit::between(text, &new, selection))
}

/// Removes the selected lines with their line break and leaves a collapsed cursor at the
/// same column of the line that takes their place.
pub fn delete_lines(text: &str, selection: &Range<usize>) -> Option<Edit> {
    if text.is_empty() {
        return None;
    }
    let selection = clamped(text, selection);
    let block = line_block(text, &selection);
    let next = next_line_start(text, block.end);
    let removed = match next {
        Some(next) => block.start..next,
        // The last line goes together with the line break before it.
        None if block.start > 0 => line_end(text, block.start - 1)..block.end,
        None => 0..text.len(),
    };
    let new = replaced(text, removed.clone(), "");
    // Without a next line the cursor lands on the new last line (at 0 when `new` is empty).
    let target = match next {
        Some(_) => block.start,
        None => line_start(&new, new.len()),
    };
    let column = selection.start - line_start(text, selection.start);
    let cursor = new.floor_char_boundary((target + column).min(line_end(&new, target)));
    Some(Edit {
        range: removed,
        text: String::new(),
        selection: cursor..cursor,
    })
}

/// Joins the selected lines, or the cursor's line with the next one, trimming the
/// whitespace around each break and putting one space between non-empty parts.
pub fn join_lines(text: &str, selection: &Range<usize>) -> Option<Edit> {
    let selection = clamped(text, selection);
    let block = line_block(text, &selection);
    let mut lines = block_lines(text, block);
    if lines.len() == 1 {
        let next = next_line_start(text, block.end)?;
        lines.push(next..line_end(text, next));
    }
    let region = lines[0].start..lines[lines.len() - 1].end;
    let first = text[lines[0].clone()].trim_end();
    let mut joined = first.to_string();
    for line in &lines[1..] {
        let part = text[line.clone()].trim_start();
        joined.truncate(joined.trim_end().len());
        if !joined.is_empty() && !part.is_empty() {
            joined.push(' ');
        }
        joined.push_str(part);
    }
    let selection = if selection.is_empty() {
        let cursor = region.start + first.len();
        cursor..cursor
    } else {
        region.start..region.start + joined.len()
    };
    let new = replaced(text, region, &joined);
    Some(Edit::between(text, &new, selection))
}

/// Sorts the selected lines, or every line of the text when the selection touches one
/// line, and selects the sorted lines.
pub fn sort_lines(text: &str, selection: &Range<usize>, descending: bool) -> Option<Edit> {
    let selection = clamped(text, selection);
    let selected = line_block(text, &selection);
    let block = if selected.start == line_start(text, selected.end) {
        // One line: the whole text, without the empty last line after a final line break.
        let end = if text.ends_with('\n') {
            line_end(text, text.len() - 1)
        } else {
            text.len()
        };
        LineBlock { start: 0, end }
    } else {
        selected
    };
    let lines: Vec<&str> = block_lines(text, block)
        .into_iter()
        .map(|line| &text[line])
        .collect();
    let mut ordered = lines.clone();
    if descending {
        ordered.sort_by(|a, b| b.cmp(a));
    } else {
        ordered.sort();
    }
    if ordered == lines {
        return None;
    }
    let sorted = ordered.join("\n");
    let new = replaced(text, block.start..block.end, &sorted);
    Some(Edit::between(
        text,
        &new,
        block.start..block.start + sorted.len(),
    ))
}

/// Removes trailing spaces and tabs from every line of the text. A cursor inside removed
/// whitespace moves to its line's new end.
pub fn trim_trailing_whitespace(text: &str, selection: &Range<usize>) -> Option<Edit> {
    let whole = LineBlock {
        start: 0,
        end: text.len(),
    };
    let splices: Vec<Splice<'_>> = block_lines(text, whole)
        .into_iter()
        .filter_map(|line| {
            let content = &text[line.clone()];
            let kept = content.trim_end_matches([' ', '\t']).len();
            (kept < content.len()).then_some(Splice {
                at: line.start + kept,
                remove: content.len() - kept,
                insert: "",
            })
        })
        .collect();
    if splices.is_empty() {
        return None;
    }
    let (new, selection) = splice(text, &splices, selection);
    Some(Edit::between(text, &new, selection))
}

/// Upper- or lower-cases the selection, or the word at a collapsed cursor.
pub fn transform_case(text: &str, selection: &Range<usize>, upper: bool) -> Option<Edit> {
    let selection = clamped(text, selection);
    let range = if selection.is_empty() {
        word_range_at(text, selection.start)?
    } else {
        selection.clone()
    };
    let original = &text[range.clone()];
    let replacement = if upper {
        original.to_uppercase()
    } else {
        original.to_lowercase()
    };
    if replacement == original {
        return None;
    }
    let new = replaced(text, range.clone(), &replacement);
    let replaced_end = range.start + replacement.len();
    let selection = if selection.is_empty() {
        let cursor = new.floor_char_boundary(selection.start.min(replaced_end));
        cursor..cursor
    } else {
        range.start..replaced_end
    };
    Some(Edit::between(text, &new, selection))
}

/// `selection` clamped into `text`, with its end not before its start.
fn clamped(text: &str, selection: &Range<usize>) -> Range<usize> {
    let start = clamp(text, selection.start);
    start..clamp(text, selection.end).max(start)
}

/// `text` with `range` replaced by `insert`.
fn replaced(text: &str, range: Range<usize>, insert: &str) -> String {
    let mut new = String::with_capacity(text.len() - range.len() + insert.len());
    new.push_str(&text[..range.start]);
    new.push_str(insert);
    new.push_str(&text[range.end..]);
    new
}

#[cfg(test)]
mod tests {
    use super::*;
    use Direction::{Down, Up};

    fn applied(text: &str, edit: Option<Edit>) -> (String, Range<usize>) {
        let edit = edit.expect("an edit");
        (edit.apply(text), edit.selection)
    }

    fn owned(text: &str, selection: Range<usize>) -> (String, Range<usize>) {
        (text.to_string(), selection)
    }

    fn duplicated(
        text: &str,
        selection: Range<usize>,
        direction: Direction,
    ) -> (String, Range<usize>) {
        applied(text, duplicate_lines(text, &selection, direction))
    }

    fn moved(text: &str, selection: Range<usize>, direction: Direction) -> (String, Range<usize>) {
        applied(text, move_lines(text, &selection, direction))
    }

    fn deleted(text: &str, selection: Range<usize>) -> (String, Range<usize>) {
        applied(text, delete_lines(text, &selection))
    }

    fn joined(text: &str, selection: Range<usize>) -> (String, Range<usize>) {
        applied(text, join_lines(text, &selection))
    }

    fn sorted(text: &str, selection: Range<usize>, descending: bool) -> (String, Range<usize>) {
        applied(text, sort_lines(text, &selection, descending))
    }

    fn trimmed(text: &str, selection: Range<usize>) -> (String, Range<usize>) {
        applied(text, trim_trailing_whitespace(text, &selection))
    }

    fn cased(text: &str, selection: Range<usize>, upper: bool) -> (String, Range<usize>) {
        applied(text, transform_case(text, &selection, upper))
    }

    #[test]
    fn duplicate_one_line_down_puts_the_cursor_on_the_copy() {
        assert_eq!(duplicated("a\nb\n", 1..1, Down), owned("a\na\nb\n", 3..3));
        assert_eq!(duplicated("a\nb\n", 0..0, Down), owned("a\na\nb\n", 2..2));
    }

    #[test]
    fn duplicate_one_line_up_keeps_the_cursor() {
        assert_eq!(duplicated("a\nb\n", 3..3, Up), owned("a\nb\nb\n", 3..3));
        assert_eq!(duplicated("a\nb\n", 1..1, Up), owned("a\na\nb\n", 1..1));
    }

    #[test]
    fn duplicate_a_two_line_selection() {
        let text = "ab\ncd\nef\n";
        assert_eq!(
            duplicated(text, 1..4, Down),
            owned("ab\ncd\nab\ncd\nef\n", 7..10)
        );
        assert_eq!(
            duplicated(text, 1..4, Up),
            owned("ab\ncd\nab\ncd\nef\n", 1..4)
        );
    }

    #[test]
    fn duplicate_the_last_line_without_a_trailing_newline() {
        assert_eq!(duplicated("a\nb", 3..3, Down), owned("a\nb\nb", 5..5));
        assert_eq!(duplicated("a\nb", 3..3, Up), owned("a\nb\nb", 3..3));
    }

    #[test]
    fn duplicate_excludes_a_line_the_selection_ends_at_column_zero_of() {
        assert_eq!(
            duplicated("a\nb\nc\n", 0..4, Down),
            owned("a\nb\na\nb\nc\n", 4..8)
        );
    }

    #[test]
    fn duplicate_up_keeps_a_selection_ending_after_a_crlf_break_on_the_upper_copy() {
        assert_eq!(
            duplicated("\u{e9}\r\nx", 0..4, Up),
            owned("\u{e9}\n\u{e9}\r\nx", 0..3)
        );
    }

    #[test]
    fn move_a_middle_line_up_and_down_with_the_selection() {
        let text = "a\nbc\nd\n";
        assert_eq!(moved(text, 3..4, Up), owned("bc\na\nd\n", 1..2));
        assert_eq!(moved(text, 3..4, Down), owned("a\nd\nbc\n", 5..6));
    }

    #[test]
    fn move_the_first_line_up_or_the_last_line_down_is_none() {
        assert_eq!(move_lines("a\nb", &(0..0), Up), None);
        assert_eq!(move_lines("a\nb", &(1..1), Up), None);
        assert_eq!(move_lines("a\nb", &(3..3), Down), None);
        assert_eq!(move_lines("a\nb\n", &(4..4), Down), None);
        assert_eq!(move_lines("", &(0..0), Down), None);
    }

    #[test]
    fn move_the_last_text_line_below_a_trailing_empty_line() {
        assert_eq!(moved("a\nb\n", 2..2, Down), owned("a\n\nb", 3..3));
    }

    #[test]
    fn move_the_last_line_without_a_newline_up() {
        assert_eq!(moved("a\nb", 3..3, Up), owned("b\na", 1..1));
    }

    #[test]
    fn move_a_multi_line_block_as_one() {
        let text = "a\nb\nc\nd\n";
        assert_eq!(moved(text, 2..5, Down), owned("a\nd\nb\nc\n", 4..7));
        assert_eq!(moved(text, 2..5, Up), owned("b\nc\na\nd\n", 0..3));
    }

    #[test]
    fn move_preserves_a_crlf_break() {
        let text = "a\r\nb\r\n";
        assert_eq!(moved(text, 3..3, Up), owned("b\r\na\r\n", 0..0));
        assert_eq!(moved(text, 0..0, Down), owned("b\r\na\r\n", 3..3));
    }

    #[test]
    fn move_identical_lines_still_moves_the_selection() {
        let edit = move_lines("x\nx\n", &(1..1), Down).expect("an edit");
        assert!(!edit.changes_text());
        assert_eq!(edit.selection, 3..3);
        let edit = move_lines("x\nx\n", &(3..3), Up).expect("an edit");
        assert_eq!(edit.selection, 1..1);
    }

    #[test]
    fn move_down_onto_the_last_line_keeps_the_selection_inside_the_text() {
        // The selection ends at column 0 of "c", which has no line break after it.
        assert_eq!(moved("a\nb\nc", 2..4, Down), owned("a\nc\nb", 4..5));
    }

    #[test]
    fn move_keeps_the_selection_on_the_moved_lines_with_mixed_line_breaks() {
        // The selection ends after "b"'s CRLF, which is one byte longer than the "\n"
        // that ends up after "b".
        assert_eq!(
            moved("\u{e9}\nb\r\nc", 3..6, Up),
            owned("b\n\u{e9}\r\nc", 0..2)
        );
        // A cursor between '\r' and '\n' of the last line break.
        assert_eq!(moved("a\r\nb\r\n", 5..5, Down), owned("a\r\n\r\nb", 6..6));
    }

    #[test]
    fn delete_a_middle_line_keeps_the_cursor_column() {
        let text = "abc\nhello\nxy\n";
        assert_eq!(deleted(text, 5..5), owned("abc\nxy\n", 5..5));
        let edit = delete_lines(text, &(5..5)).expect("an edit");
        assert_eq!((edit.range, edit.text.as_str()), (4..10, ""));
    }

    #[test]
    fn delete_clamps_the_column_to_the_shorter_next_line() {
        assert_eq!(deleted("abc\nhello\nxy\n", 8..8), owned("abc\nxy\n", 6..6));
    }

    #[test]
    fn delete_the_last_line_without_a_newline() {
        assert_eq!(deleted("a\nb", 2..2), owned("a", 0..0));
        assert_eq!(deleted("a\nb", 3..3), owned("a", 1..1));
        assert_eq!(deleted("a\r\nb", 3..3), owned("a", 0..0));
    }

    #[test]
    fn delete_the_only_line() {
        assert_eq!(deleted("abc", 1..1), owned("", 0..0));
    }

    #[test]
    fn delete_a_multi_line_selection() {
        assert_eq!(deleted("a\nb\nc\nd\n", 2..5), owned("a\nd\n", 2..2));
    }

    #[test]
    fn delete_in_empty_text_is_none() {
        assert_eq!(delete_lines("", &(0..0)), None);
    }

    #[test]
    fn delete_moves_a_column_inside_a_multi_byte_char_back() {
        // Column 1 of the next line falls inside 'é'.
        assert_eq!(deleted("ab\nxy\né\n", 4..4), owned("ab\né\n", 3..3));
    }

    #[test]
    fn join_trims_both_sides_and_puts_one_space_between() {
        assert_eq!(joined("foo  \n    bar", 1..1), owned("foo bar", 3..3));
    }

    #[test]
    fn join_with_an_empty_next_line_adds_no_space() {
        assert_eq!(joined("foo\n\nbar", 0..0), owned("foo\nbar", 3..3));
    }

    #[test]
    fn join_three_selected_lines_keeps_them_selected() {
        assert_eq!(joined("a\n b \nc\nd", 0..7), owned("a b c\nd", 0..5));
    }

    #[test]
    fn join_on_the_last_line_is_none() {
        assert_eq!(join_lines("a\nb", &(3..3)), None);
    }

    #[test]
    fn sort_with_a_cursor_sorts_the_whole_file_and_keeps_the_final_newline() {
        assert_eq!(sorted("c\na\nb\n", 0..0, false), owned("a\nb\nc\n", 0..5));
    }

    #[test]
    fn sort_a_selection_sorts_only_its_lines() {
        assert_eq!(
            sorted("z\nc\na\nb\ny\n", 2..7, false),
            owned("z\na\nb\nc\ny\n", 2..7)
        );
    }

    #[test]
    fn sort_descending() {
        assert_eq!(sorted("a\nc\nb", 0..0, true), owned("c\nb\na", 0..5));
    }

    #[test]
    fn sort_an_already_sorted_region_is_none() {
        assert_eq!(sort_lines("a\nb\nc\n", &(0..0), false), None);
        assert_eq!(sort_lines("c\nb\na", &(0..0), true), None);
    }

    #[test]
    fn sort_keeps_equal_lines_in_their_order() {
        assert_eq!(sort_lines("a\nb\nb", &(0..0), false), None);
        assert_eq!(sort_lines("b\na\na", &(0..0), true), None);
        assert_eq!(sorted("b\na\nb\na", 0..0, false), owned("a\na\nb\nb", 0..7));
    }

    #[test]
    fn trim_removes_trailing_spaces_and_tabs_on_every_line() {
        assert_eq!(
            trimmed("a  \nb\t\t\nc \t\r\nd", 0..0),
            owned("a\nb\nc\r\nd", 0..0)
        );
    }

    #[test]
    fn trim_moves_a_cursor_in_removed_whitespace_to_the_line_end() {
        assert_eq!(trimmed("ab   \ncd ", 4..4), owned("ab\ncd", 2..2));
        assert_eq!(trimmed("ab   \ncd ", 9..9), owned("ab\ncd", 5..5));
    }

    #[test]
    fn trim_on_clean_text_is_none() {
        assert_eq!(trim_trailing_whitespace("a\nb \tc\n", &(0..0)), None);
    }

    #[test]
    fn transform_a_selection_to_upper_and_to_lower_case() {
        assert_eq!(cased("let foo = 1", 4..7, true), owned("let FOO = 1", 4..7));
        assert_eq!(cased("LET FOO", 0..3, false), owned("let FOO", 0..3));
    }

    #[test]
    fn transform_the_word_under_a_collapsed_cursor() {
        assert_eq!(cased("let foo = 1", 5..5, true), owned("let FOO = 1", 5..5));
        assert_eq!(cased("let foo = 1", 7..7, true), owned("let FOO = 1", 7..7));
    }

    #[test]
    fn transform_sharp_s_to_upper_case_grows_the_text() {
        assert_eq!(cased("straße", 0..7, true), owned("STRASSE", 0..7));
        assert_eq!(cased("straße x", 6..6, true), owned("STRASSE x", 6..6));
        // 'ŉ' (2 bytes) becomes "ʼN" (3 bytes).
        assert_eq!(cased("ŉ", 0..2, true), owned("ʼN", 0..3));
    }

    #[test]
    fn transform_keeps_a_collapsed_cursor_on_a_char_boundary() {
        // 'İ' becomes 'i' plus a 2-byte combining dot, so offset 2 falls inside the dot.
        assert_eq!(cased("İa", 2..2, false), owned("i\u{307}a", 1..1));
        // The Kelvin sign (3 bytes) becomes 'k' (1 byte).
        assert_eq!(cased("\u{212A}\u{212A}", 6..6, false), owned("kk", 2..2));
    }

    #[test]
    fn transform_without_a_word_is_none() {
        assert_eq!(transform_case("a + b", &(2..2), true), None);
    }

    #[test]
    fn transform_already_in_that_case_is_none() {
        assert_eq!(transform_case("ABC", &(0..3), true), None);
        assert_eq!(transform_case("abc", &(1..1), false), None);
    }

    #[test]
    fn every_operation_leaves_the_selection_on_char_boundaries_of_the_new_text() {
        let texts = [
            "",
            "\n",
            "a\n\n",
            "ab \r\nc\u{e9}\td\r\n",
            "\u{130}\u{df} x\n  y\u{212A}  \nz",
            "\u{e9}\r\n\u{e9}\n\u{e9}\r\n",
        ];
        for text in texts {
            let boundaries: Vec<usize> = (0..=text.len())
                .filter(|i| text.is_char_boundary(*i))
                .collect();
            for &start in &boundaries {
                for &end in boundaries.iter().filter(|end| **end >= start) {
                    let selection = start..end;
                    let edits = [
                        duplicate_lines(text, &selection, Up),
                        duplicate_lines(text, &selection, Down),
                        move_lines(text, &selection, Up),
                        move_lines(text, &selection, Down),
                        delete_lines(text, &selection),
                        join_lines(text, &selection),
                        sort_lines(text, &selection, false),
                        sort_lines(text, &selection, true),
                        trim_trailing_whitespace(text, &selection),
                        transform_case(text, &selection, true),
                        transform_case(text, &selection, false),
                    ];
                    for edit in edits.into_iter().flatten() {
                        assert!(
                            edit.range.start <= edit.range.end
                                && text.is_char_boundary(edit.range.start)
                                && text.is_char_boundary(edit.range.end),
                            "{text:?} {selection:?} -> range {:?}",
                            edit.range
                        );
                        let new = edit.apply(text);
                        let after = &edit.selection;
                        assert!(
                            after.start <= after.end
                                && new.is_char_boundary(after.start)
                                && new.is_char_boundary(after.end),
                            "{text:?} {selection:?} -> {new:?} {after:?}"
                        );
                    }
                }
            }
        }
    }
}
