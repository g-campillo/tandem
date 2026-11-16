//! Selection helpers: occurrences of a text, bracket matching, selection expansion and
//! whole-line selection.
use super::{
    block_lines, clamp, indent_len, is_word_char, line_block, line_end, line_start,
    next_line_start, word_range_at,
};
use std::ops::Range;

/// Every non-overlapping, case-sensitive, literal match of `needle`, ascending. With
/// `whole_word`, only matches with no word char directly before or after them.
pub fn occurrences(text: &str, needle: &str, whole_word: bool) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    if needle.is_empty() {
        return found;
    }
    let mut from = 0;
    while let Some(i) = text[from..].find(needle) {
        let range = from + i..from + i + needle.len();
        from = range.end;
        if !whole_word || is_whole_word(text, &range) {
            found.push(range);
        }
    }
    found
}

/// Whether neither the char before `range` nor the char after it is a word char.
fn is_whole_word(text: &str, range: &Range<usize>) -> bool {
    let before = text[..range.start].chars().next_back();
    let after = text[range.end..].chars().next();
    !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
}

/// The first range of `found` that starts at or after `after` and is not in `taken`,
/// wrapping around to the first free one; `None` when every range is taken.
pub fn next_occurrence(
    found: &[Range<usize>],
    taken: &[Range<usize>],
    after: usize,
) -> Option<Range<usize>> {
    let mut free = found.iter().filter(|range| !taken.contains(range));
    let first_free = free.clone().next();
    free.find(|range| range.start >= after)
        .or(first_free)
        .cloned()
}

/// The offset directly before the bracket that matches the one at or next to `offset`, or
/// before the closer of the innermost pair around `offset`. `None` when there is no such
/// pair or a mismatched bracket comes first.
pub fn matching_bracket(text: &str, offset: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let offset = clamp(text, offset);
    let at = bytes.get(offset).copied();
    let before = offset.checked_sub(1).map(|i| bytes[i]);
    if at.is_some_and(is_opener) {
        match_of(bytes, offset)
    } else if before.is_some_and(is_closer) {
        match_of(bytes, offset - 1)
    } else if at.is_some_and(is_closer) {
        match_of(bytes, offset)
    } else if before.is_some_and(is_opener) {
        match_of(bytes, offset - 1)
    } else {
        enclosing_pair(bytes, offset).map(|(_, closer)| closer)
    }
}

/// The bracket pairs, opener first. They are ASCII, so a byte scan finds exactly the
/// bracket chars of a UTF-8 text.
const BRACKETS: [(u8, u8); 3] = [(b'(', b')'), (b'[', b']'), (b'{', b'}')];

/// The closer of `opener`, `None` when it is not an opening bracket.
fn closer_for(opener: u8) -> Option<u8> {
    BRACKETS
        .iter()
        .find(|pair| pair.0 == opener)
        .map(|pair| pair.1)
}

/// The opener of `closer`, `None` when it is not a closing bracket.
fn opener_for(closer: u8) -> Option<u8> {
    BRACKETS
        .iter()
        .find(|pair| pair.1 == closer)
        .map(|pair| pair.0)
}

fn is_opener(byte: u8) -> bool {
    closer_for(byte).is_some()
}

fn is_closer(byte: u8) -> bool {
    opener_for(byte).is_some()
}

/// The index of the bracket that matches the one at `index`: forwards from an opener,
/// backwards from a closer. `None` when there is none or a mismatched bracket comes first.
fn match_of(bytes: &[u8], index: usize) -> Option<usize> {
    let bracket = bytes[index];
    let (found, partner) = match (closer_for(bracket), opener_for(bracket)) {
        (Some(closer), _) => {
            let after = bytes.iter().copied().enumerate().skip(index + 1);
            (unbalanced(after, true), closer)
        }
        (None, Some(opener)) => {
            let before = bytes[..index].iter().copied().enumerate().rev();
            (unbalanced(before, false), opener)
        }
        (None, None) => return None,
    };
    let (found, byte) = found?;
    (byte == partner).then_some(found)
}

/// The nearest opener before `offset` that is not closed before `offset`, and its closer.
fn enclosing_pair(bytes: &[u8], offset: usize) -> Option<(usize, usize)> {
    let before = bytes[..offset].iter().copied().enumerate().rev();
    let (opener, _) = unbalanced(before, false)?;
    Some((opener, match_of(bytes, opener)?))
}

/// Walks `steps` (indices with their bytes, in walking order), pairing up the brackets
/// met on the way, and returns the first bracket that closes nothing it met: a closer when
/// walking `forward`, an opener when walking backwards. `None` at the end of `steps` or
/// when a bracket does not match the innermost unpaired one.
fn unbalanced(steps: impl Iterator<Item = (usize, u8)>, forward: bool) -> Option<(usize, u8)> {
    let mut expected = Vec::new();
    for (index, byte) in steps {
        let (opens, closes) = if forward {
            (closer_for(byte), opener_for(byte))
        } else {
            (opener_for(byte), closer_for(byte))
        };
        if let Some(partner) = opens {
            expected.push(partner);
        } else if closes.is_some() {
            match expected.pop() {
                None => return Some((index, byte)),
                Some(partner) if partner == byte => {}
                Some(_) => return None,
            }
        }
    }
    None
}

/// The smallest range around `selection` that is larger than it: the word, a string, the
/// inside of a bracket pair, the pair, the lines without their indentation, the whole lines
/// or the whole text. `quotes` are the chars that delimit strings.
pub fn expand_selection(
    text: &str,
    selection: &Range<usize>,
    quotes: &[char],
) -> Option<Range<usize>> {
    let start = clamp(text, selection.start);
    let selection = start..clamp(text, selection.end).max(start);
    // `min_by_key` keeps the first of equally short candidates.
    candidates(text, selection.clone(), quotes)
        .into_iter()
        .filter(|range| {
            range.start <= selection.start && selection.end <= range.end && *range != selection
        })
        .min_by_key(|range| range.len())
}

/// The ranges `expand_selection` chooses from, in order of preference on equal length.
fn candidates(text: &str, selection: Range<usize>, quotes: &[char]) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    if let Some(word) = word_range_at(text, selection.start)
        && word.start <= selection.start
        && selection.end <= word.end
    {
        found.push(word);
    }

    let line = line_start(text, selection.start)..line_end(text, selection.start);
    if line_start(text, selection.end) == line.start {
        for (open, close) in quote_pairs(&text[line.clone()], quotes) {
            found.push(line.start + open.end..line.start + close.start);
            found.push(line.start + open.start..line.start + close.end);
        }
    }

    let bytes = text.as_bytes();
    let mut inside = selection.start;
    while let Some((opener, closer)) = enclosing_pair(bytes, inside) {
        found.push(opener + 1..closer);
        found.push(opener..closer + 1);
        inside = opener;
    }

    let block = line_block(text, &selection);
    let first = block.start..line_end(text, block.start);
    let last = line_start(text, block.end)..block.end;
    let content_start = first.start + indent_len(&text[first]);
    let content_end = last.start + text[last].trim_end_matches([' ', '\t']).len();
    if content_start <= content_end {
        found.push(content_start..content_end);
    }
    found.push(block.start..block.end);
    found.push(0..text.len());
    found
}

/// The strings on `line`, left to right, as the ranges of their opening and closing
/// quote. A quote preceded by a backslash neither opens nor closes a string.
fn quote_pairs(line: &str, quotes: &[char]) -> Vec<(Range<usize>, Range<usize>)> {
    let mut pairs = Vec::new();
    let mut open: Option<(Range<usize>, char)> = None;
    let mut previous = None;
    for (index, c) in line.char_indices() {
        if quotes.contains(&c) && previous != Some('\\') {
            let quote = index..index + c.len_utf8();
            match open.take() {
                None => open = Some((quote, c)),
                Some((opening, q)) if q == c => pairs.push((opening, quote)),
                // Another kind of quote inside a string is part of its content.
                still_open => open = still_open,
            }
        }
        previous = Some(c);
    }
    pairs
}

/// The lines `selection` touches with their line break; when they are already selected
/// exactly, those lines and the line after them.
pub fn select_line(text: &str, selection: &Range<usize>) -> Range<usize> {
    let block = line_block(text, selection);
    let after = |content_end: usize| next_line_start(text, content_end).unwrap_or(text.len());
    let lines = block.start..after(block.end);
    match next_line_start(text, block.end) {
        Some(following) if lines == *selection => block.start..after(following),
        _ => lines,
    }
}

/// The end of the content of every line `selection` touches.
pub fn line_end_cursors(text: &str, selection: &Range<usize>) -> Vec<usize> {
    block_lines(text, line_block(text, selection))
        .into_iter()
        .map(|line| line.end)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUOTES: [char; 3] = ['"', '\'', '`'];

    fn expanded(text: &str, selection: Range<usize>, quotes: &[char]) -> Option<Range<usize>> {
        expand_selection(text, &selection, quotes)
    }

    #[test]
    fn occurrences_match_inside_longer_words_unless_whole_word() {
        let text = "foo foobar barfoo Foo";
        assert_eq!(occurrences(text, "foo", false), vec![0..3, 4..7, 14..17]);
        assert_eq!(occurrences(text, "foo", true), vec![0..3]);
    }

    #[test]
    fn occurrences_do_not_overlap() {
        assert_eq!(occurrences("aaaa", "aa", false), vec![0..2, 2..4]);
        assert_eq!(occurrences("aaaaa", "aa", false), vec![0..2, 2..4]);
    }

    #[test]
    fn occurrences_treat_non_ascii_neighbours_as_word_chars() {
        let text = "\u{e9}foo foo foo\u{f1}";
        assert_eq!(occurrences(text, "foo", false), vec![2..5, 6..9, 10..13]);
        assert_eq!(occurrences(text, "foo", true), vec![6..9]);
    }

    #[test]
    fn occurrences_of_an_empty_needle_are_empty() {
        assert_eq!(occurrences("abc", "", false), Vec::<Range<usize>>::new());
        assert_eq!(occurrences("abc", "", true), Vec::<Range<usize>>::new());
        assert_eq!(occurrences("", "a", false), Vec::<Range<usize>>::new());
    }

    #[test]
    fn next_occurrence_starts_at_or_after_the_offset() {
        let found = [0..3, 4..7, 10..13];
        assert_eq!(next_occurrence(&found, &[], 4), Some(4..7));
        assert_eq!(next_occurrence(&found, &[], 5), Some(10..13));
        assert_eq!(next_occurrence(&found, &[], 0), Some(0..3));
    }

    #[test]
    fn next_occurrence_wraps_to_the_start() {
        let found = [0..3, 4..7, 10..13];
        assert_eq!(next_occurrence(&found, &[], 11), Some(0..3));
        assert_eq!(next_occurrence(&found, &found[..1], 11), Some(4..7));
    }

    #[test]
    fn next_occurrence_skips_taken_ranges() {
        let found = [0..3, 4..7, 10..13];
        assert_eq!(next_occurrence(&found, &found[1..2], 4), Some(10..13));
        assert_eq!(next_occurrence(&found, &[10..13, 0..3], 11), Some(4..7));
    }

    #[test]
    fn next_occurrence_is_none_when_all_are_taken_or_none_found() {
        let found = [0..3, 4..7];
        assert_eq!(next_occurrence(&found, &[4..7, 0..3], 0), None);
        assert_eq!(next_occurrence(&[], &[], 0), None);
    }

    // f o o ( a [ 1 ] ,   { b  }  )
    // 0 1 2 3 4 5 6 7 8 9 10 11 12 13
    const NESTED: &str = "foo(a[1], {b})";

    #[test]
    fn matching_bracket_from_before_an_opener_goes_before_its_closer() {
        assert_eq!(matching_bracket(NESTED, 3), Some(13));
    }

    #[test]
    fn matching_bracket_from_after_a_closer_goes_before_its_opener() {
        assert_eq!(matching_bracket(NESTED, 14), Some(3));
    }

    #[test]
    fn matching_bracket_inside_without_an_adjacent_bracket_goes_before_the_enclosing_closer() {
        assert_eq!(matching_bracket(NESTED, 9), Some(13));
        assert_eq!(matching_bracket(NESTED, 1), None);
        assert_eq!(matching_bracket(NESTED, 11), Some(12));
    }

    #[test]
    fn matching_bracket_matches_nested_pairs_with_their_own_partner() {
        assert_eq!(matching_bracket(NESTED, 5), Some(7));
        assert_eq!(matching_bracket(NESTED, 7), Some(5));
        assert_eq!(matching_bracket(NESTED, 8), Some(5));
        assert_eq!(matching_bracket(NESTED, 10), Some(12));
        assert_eq!(matching_bracket(NESTED, 13), Some(10));
        assert_eq!(matching_bracket(NESTED, 4), Some(13));
    }

    #[test]
    fn matching_bracket_with_a_mismatched_closer_is_none() {
        assert_eq!(matching_bracket("(]", 0), None);
        assert_eq!(matching_bracket("(]", 1), None);
        assert_eq!(matching_bracket("(]", 2), None);
        assert_eq!(matching_bracket("(a]b", 2), None);
    }

    #[test]
    fn matching_bracket_without_brackets_or_an_enclosing_pair_is_none() {
        assert_eq!(matching_bracket("foo bar", 2), None);
        assert_eq!(matching_bracket("", 0), None);
        assert_eq!(matching_bracket("f(a) b", 6), None);
        assert_eq!(matching_bracket("(a b", 2), None);
    }

    #[test]
    fn expand_selection_grows_step_by_step_to_the_whole_text() {
        let text = "fn f() {\n    call(arg_one, \"hello world\");\n}\n";
        let steps = [
            28..33,
            28..39,
            27..40,
            18..40,
            17..41,
            13..42,
            9..42,
            8..43,
            7..44,
            0..44,
            0..45,
        ];
        let mut selection = 30..30;
        for step in steps {
            let next = expanded(text, selection.clone(), &QUOTES);
            assert_eq!(next, Some(step.clone()), "expanding {selection:?}");
            selection = step;
        }
        assert_eq!(expanded(text, selection, &QUOTES), None);
    }

    #[test]
    fn expand_selection_only_uses_the_given_quotes() {
        let text = "x = 'ab cd';\n";
        assert_eq!(expanded(text, 5..7, &QUOTES), Some(5..10));
        assert_eq!(expanded(text, 5..7, &['"', '`']), Some(0..12));
    }

    #[test]
    fn expand_selection_skips_a_quote_preceded_by_a_backslash() {
        let text = "say(\"a\\\"b\")";
        assert_eq!(expanded(text, 8..9, &QUOTES), Some(5..9));
    }

    #[test]
    fn expand_selection_of_a_multi_line_selection_has_no_word_or_quote_candidate() {
        // s a y ( ' a ' ,   ' b  '  )  \n n  e  x  t  \n
        // 0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18
        let text = "say('a', 'b')\nnext\n";
        assert_eq!(
            candidates(text, 5..16, &QUOTES),
            vec![4..12, 3..13, 0..18, 0..18, 0..19]
        );
        assert_eq!(expanded(text, 5..16, &QUOTES), Some(0..18));
    }

    #[test]
    fn expand_selection_of_a_multi_line_selection_trims_the_block() {
        let text = "    foo bar\n    baz qux\n";
        assert_eq!(expanded(text, 5..18, &QUOTES), Some(4..23));
    }

    #[test]
    fn expand_selection_of_the_whole_text_is_none() {
        assert_eq!(expanded("abc", 0..3, &QUOTES), None);
        assert_eq!(expanded("", 0..0, &QUOTES), None);
    }

    #[test]
    fn select_line_selects_a_middle_line_with_its_break() {
        assert_eq!(select_line("ab\ncd\nef", &(4..4)), 3..6);
        assert_eq!(select_line("ab\ncd\nef", &(1..4)), 0..6);
    }

    #[test]
    fn select_line_selects_the_last_line_without_a_newline() {
        assert_eq!(select_line("ab\ncd\nef", &(7..7)), 6..8);
        assert_eq!(select_line("ab\ncd\nef", &(6..8)), 6..8);
    }

    #[test]
    fn select_line_again_adds_the_next_line() {
        assert_eq!(select_line("ab\ncd\nef", &(3..6)), 3..8);
        assert_eq!(select_line("ab\ncd\nef\n", &(0..3)), 0..6);
        assert_eq!(select_line("ab\ncd\nef\n", &(0..6)), 0..9);
    }

    #[test]
    fn line_end_cursors_gives_one_offset_per_line() {
        assert_eq!(line_end_cursors("ab\ncde\nf\n", &(1..8)), vec![2, 6, 8]);
        assert_eq!(line_end_cursors("ab\ncde\nf\n", &(4..4)), vec![6]);
    }

    #[test]
    fn line_end_cursors_stop_before_a_crlf_break() {
        assert_eq!(line_end_cursors("ab\r\ncd\r\n", &(0..5)), vec![2, 6]);
    }
}
