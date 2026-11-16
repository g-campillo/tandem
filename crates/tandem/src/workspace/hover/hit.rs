//! Pure decisions about what the pointer is on, kept free of windows and entities so they
//! are tested directly. Offsets are UTF-8 byte offsets into the editor text.

use gpui_kit::{Bounds, Pixels, Point};
use std::ops::Range;

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The characters on either side of caret `offset`, on the same line: `[after, before]`.
/// The toolkit reports the pointer as the nearest caret offset, so the character under the
/// pointer, if there is one, is one of these two.
pub fn adjacent_chars(text: &str, offset: usize) -> [Option<Range<usize>>; 2] {
    if offset > text.len() || !text.is_char_boundary(offset) {
        return [None, None];
    }
    let on_line = |c: &char| !matches!(c, '\n' | '\r');
    let after = text[offset..]
        .chars()
        .next()
        .filter(on_line)
        .map(|c| offset..offset + c.len_utf8());
    let before = text[..offset]
        .chars()
        .next_back()
        .filter(on_line)
        .map(|c| offset - c.len_utf8()..offset);
    [after, before]
}

/// The character the pointer is on. `None` past the end of a line, left of its first
/// character, on an empty line, and wherever `bounds_of` has no layout.
pub fn char_under_pointer(
    text: &str,
    offset: usize,
    pointer: Point<Pixels>,
    bounds_of: impl Fn(&Range<usize>) -> Option<Bounds<Pixels>>,
) -> Option<Range<usize>> {
    adjacent_chars(text, offset)
        .into_iter()
        .flatten()
        .find(|range| bounds_of(range).is_some_and(|bounds| bounds.contains(&pointer)))
}

/// Whether a tooltip may be requested for the character at `hit`: anything but whitespace.
pub fn is_hoverable(text: &str, hit: &Range<usize>) -> bool {
    text.get(hit.clone())
        .is_some_and(|c| !c.is_empty() && !c.chars().all(char::is_whitespace))
}

/// The offset to send to the language server for `hit`. A position between two tokens is
/// ambiguous and servers prefer the identifier, so punctuation that directly follows a word
/// is asked for at its end.
pub fn request_offset(text: &str, hit: &Range<usize>) -> usize {
    let on_word = text[hit.clone()].chars().all(is_word);
    let after_word = text[..hit.start].chars().next_back().is_some_and(is_word);
    let before_word = text[hit.end..].chars().next().is_some_and(is_word);
    if !on_word && after_word && !before_word {
        hit.end
    } else {
        hit.start
    }
}

/// The identifier-like word (letters, digits and `_`) containing `hit`.
pub fn word_at(text: &str, hit: &Range<usize>) -> Option<Range<usize>> {
    if !text.get(hit.clone())?.chars().all(is_word) {
        return None;
    }
    let start = text[..hit.start]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(hit.start, |(index, _)| index);
    let end = hit.end
        + text[hit.end..]
            .chars()
            .take_while(|c| is_word(*c))
            .map(char::len_utf8)
            .sum::<usize>();
    Some(start..end)
}

/// A language-server range as byte offsets, or `None` when it does not fit the text.
pub fn server_range(text: &str, range: &lsp_types::Range) -> Option<Range<usize>> {
    let start = tandem_lsp::utf16_offset(text, range.start).ok()?;
    let end = tandem_lsp::utf16_offset(text, range.end).ok()?;
    (start <= end).then_some(start..end)
}

/// The symbol a hover reply describes, or `None` when the reply is not about the character
/// under the pointer. `server` is the reply's range; without one the reply is accepted only
/// on a word. The result never leaves the line `hit` is on.
pub fn accepted_range(
    text: &str,
    hit: &Range<usize>,
    server: Option<Range<usize>>,
) -> Option<Range<usize>> {
    let Some(range) = server else {
        return word_at(text, hit);
    };
    if range.start > hit.start || hit.end > range.end {
        return None;
    }
    let line_start = text[..hit.start].rfind('\n').map_or(0, |index| index + 1);
    let line_end = text[hit.end..]
        .find(['\r', '\n'])
        .map_or(text.len(), |index| hit.end + index);
    Some(range.start.max(line_start)..range.end.min(line_end))
}

/// Where the tooltip is anchored: the symbol's box, or the box of the character under the
/// pointer when the symbol is soft-wrapped over two rows.
pub fn anchor_bounds(
    range: &Range<usize>,
    hit: &Range<usize>,
    bounds_of: impl Fn(&Range<usize>) -> Option<Bounds<Pixels>>,
) -> Option<Bounds<Pixels>> {
    let start = bounds_of(&(range.start..range.start))?;
    let end = bounds_of(&(range.end..range.end))?;
    if start.top() == end.top() {
        bounds_of(range)
    } else {
        bounds_of(hit)
    }
}

/// Whether the tooltip stays open with the pointer at `pointer`: on the symbol, on the
/// card, or in the gap between them.
pub fn keeps_open(
    pointer: Point<Pixels>,
    symbol: Bounds<Pixels>,
    card: Option<Bounds<Pixels>>,
    gap: Pixels,
) -> bool {
    symbol.contains(&pointer) || card.is_some_and(|card| card.dilate(gap).contains(&pointer))
}

/// Whether a keystroke closes the tooltip: every key except ⌘C, which copies its selection.
pub fn closes_on_key(platform: bool, key: &str) -> bool {
    !(platform && key.eq_ignore_ascii_case("c"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{point, px};

    /// A fixed-pitch layout like the editor's: 10px per character, 20px per line, text
    /// starting at (100, 50). A range's box runs from its start to its end, as the
    /// toolkit's `range_to_bounds` does.
    fn grid(text: &str, wrap: Option<usize>) -> impl Fn(&Range<usize>) -> Option<Bounds<Pixels>> {
        let text = text.to_owned();
        move |range| {
            let cell = |offset: usize| {
                let before = text.get(..offset)?;
                let mut row = 0;
                for line in before.split('\n').rev().skip(1) {
                    row += 1 + wrap.map_or(0, |wrap| line.chars().count().saturating_sub(1) / wrap);
                }
                let mut column = before.rsplit('\n').next().unwrap_or("").chars().count();
                if let Some(wrap) = wrap {
                    row += column / wrap;
                    column %= wrap;
                }
                Some(point(
                    px(100. + 10. * column as f32),
                    px(50. + 20. * row as f32),
                ))
            };
            let (start, end) = (cell(range.start)?, cell(range.end)?);
            Some(Bounds::from_corners(start, point(end.x, end.y + px(20.))))
        }
    }

    /// The pointer over `column` (fractions allowed) of visual row `row`.
    fn at(row: usize, column: f32) -> Point<Pixels> {
        point(px(100. + 10. * column), px(50. + 20. * row as f32 + 10.))
    }

    fn hit(text: &str, offset: usize, pointer: Point<Pixels>) -> Option<Range<usize>> {
        char_under_pointer(text, offset, pointer, grid(text, None))
    }

    #[test]
    fn the_character_under_the_pointer_is_the_one_before_or_after_the_caret() {
        let text = "let total = 1;\n";
        // Left half of `t`: the caret snaps to its start.
        assert_eq!(hit(text, 4, at(0, 4.2)), Some(4..5));
        // Right half of `t`: the caret snaps to its end.
        assert_eq!(hit(text, 5, at(0, 4.8)), Some(4..5));
    }

    #[test]
    fn nothing_is_under_the_pointer_beside_the_text() {
        let text = "    total\n\nnext";
        // Past the end of the line the caret snaps to the line end.
        assert_eq!(hit(text, 9, at(0, 30.)), None);
        assert_eq!(hit(text, 9, at(0, 9.1)), None);
        // Left of the first character, in the gutter.
        assert_eq!(hit(text, 0, at(0, -3.)), None);
        // An empty line.
        assert_eq!(hit(text, 10, at(1, 0.)), None);
        assert_eq!(hit(text, 10, at(1, 12.)), None);
        // Past the end of the last line, which has no line break.
        assert_eq!(hit(text, 15, at(2, 20.)), None);
        assert_eq!(hit(text, 15, at(2, 3.5)), Some(14..15));
    }

    #[test]
    fn whitespace_is_found_but_never_hoverable() {
        let text = "a + b  \n";
        assert_eq!(hit(text, 1, at(0, 1.2)), Some(1..2));
        assert!(!is_hoverable(text, &(1..2)));
        // Trailing whitespace.
        assert_eq!(hit(text, 6, at(0, 5.6)), Some(5..6));
        assert!(!is_hoverable(text, &(5..6)));
        assert!(!is_hoverable("\ta", &(0..1)));
        // Between tokens the operator itself is a candidate; the server decides.
        assert!(is_hoverable(text, &(2..3)));
        assert!(is_hoverable(text, &(0..1)));
        assert!(!is_hoverable(text, &(3..3)));
        assert!(!is_hoverable(text, &(90..91)));
    }

    #[test]
    fn multi_byte_characters_are_whole_characters() {
        let text = "let s = \"héllo 🦀\";";
        let e = text.find('é').unwrap();
        let crab = text.find('🦀').unwrap();
        assert_eq!(adjacent_chars(text, e), [Some(e..e + 2), Some(e - 1..e)]);
        assert_eq!(
            adjacent_chars(text, crab + 4),
            [Some(crab + 4..crab + 5), Some(crab..crab + 4)]
        );
        // Inside a character there is no caret.
        assert_eq!(adjacent_chars(text, e + 1), [None, None]);
        assert_eq!(adjacent_chars(text, crab + 2), [None, None]);
        assert_eq!(adjacent_chars(text, text.len() + 1), [None, None]);
        // Columns count characters, so the crab is column 15.
        assert_eq!(hit(text, crab, at(0, 15.3)), Some(crab..crab + 4));
        assert_eq!(hit(text, crab + 4, at(0, 15.7)), Some(crab..crab + 4));
        assert!(is_hoverable(text, &(crab..crab + 4)));
    }

    #[test]
    fn line_breaks_are_never_under_the_pointer() {
        let text = "ab\r\ncd";
        assert_eq!(adjacent_chars(text, 2), [None, Some(1..2)]);
        assert_eq!(adjacent_chars(text, 3), [None, None]);
        assert_eq!(adjacent_chars(text, 4), [Some(4..5), None]);
        assert_eq!(adjacent_chars("", 0), [None, None]);
    }

    #[test]
    fn the_last_character_of_a_soft_wrapped_row_has_no_box() {
        // Ten columns per row: "abcdefghij" then "klm".
        let text = "abcdefghijklm";
        let layout = grid(text, Some(10));
        assert_eq!(
            char_under_pointer(text, 9, at(0, 9.2), &layout),
            None,
            "its end is reported on the next row"
        );
        assert_eq!(char_under_pointer(text, 8, at(0, 8.2), &layout), Some(8..9));
        assert_eq!(
            char_under_pointer(text, 10, at(1, 0.2), &layout),
            Some(10..11)
        );
        // Right of the wrapped row's last glyph the caret is the wrap boundary.
        assert_eq!(char_under_pointer(text, 10, at(0, 30.), &layout), None);
    }

    #[test]
    fn punctuation_after_a_word_is_requested_at_its_end() {
        let text = "value? foo(bar) a.b x + y";
        let q = text.find('?').unwrap();
        assert_eq!(request_offset(text, &(q..q + 1)), q + 1);
        // Inside and at the start of a word the start is unambiguous.
        assert_eq!(request_offset(text, &(0..1)), 0);
        assert_eq!(request_offset(text, &(2..3)), 2);
        // Between two words either side is a word, so the start is kept.
        let open = text.find('(').unwrap();
        assert_eq!(request_offset(text, &(open..open + 1)), open);
        let dot = text.find('.').unwrap();
        assert_eq!(request_offset(text, &(dot..dot + 1)), dot);
        let close = text.find(')').unwrap();
        assert_eq!(request_offset(text, &(close..close + 1)), close + 1);
        let plus = text.find('+').unwrap();
        assert_eq!(request_offset(text, &(plus..plus + 1)), plus);
    }

    #[test]
    fn words_take_letters_digits_and_underscores_in_any_script() {
        let text = "let größe_2 = 变量.len();";
        let g = text.find('g').unwrap();
        let word = g..text.find(" =").unwrap();
        assert_eq!(&text[word.clone()], "größe_2");
        let o = text.find('ö').unwrap();
        assert_eq!(word_at(text, &(o..o + 2)), Some(word.clone()));
        assert_eq!(word_at(text, &(g..g + 1)), Some(word.clone()));
        assert_eq!(word_at(text, &(word.end - 1..word.end)), Some(word));
        let han = text.find('变').unwrap();
        assert_eq!(word_at(text, &(han + 3..han + 6)), Some(han..han + 6));
        let dot = text.find('.').unwrap();
        assert_eq!(word_at(text, &(dot..dot + 1)), None);
        assert_eq!(word_at(text, &(3..4)), None);
    }

    #[test]
    fn a_reply_must_cover_the_character_under_the_pointer() {
        let text = "let sum = add(value);\n";
        let add = 10..13;
        let open = 13..14;
        // The server answered for `add` while the pointer is on `(`.
        assert_eq!(accepted_range(text, &open, Some(add.clone())), None);
        assert_eq!(
            accepted_range(text, &(11..12), Some(add.clone())),
            Some(add.clone())
        );
        assert_eq!(
            accepted_range(text, &(10..11), Some(add.clone())),
            Some(add.clone())
        );
        assert_eq!(
            accepted_range(text, &(12..13), Some(add.clone())),
            Some(add.clone())
        );
        assert_eq!(accepted_range(text, &(14..15), Some(add.clone())), None);
        assert_eq!(accepted_range(text, &(9..10), Some(add)), None);
    }

    #[test]
    fn a_reply_without_a_range_is_accepted_only_on_a_word() {
        let text = "let sum = add(value);\n";
        assert_eq!(accepted_range(text, &(11..12), None), Some(10..13));
        assert_eq!(accepted_range(text, &(13..14), None), None);
        assert_eq!(accepted_range(text, &(8..9), None), None);
    }

    #[test]
    fn a_reply_range_is_cut_to_the_line_under_the_pointer() {
        let text = "call(\r\n    value,\r\n)\r\n";
        let value = text.find("value").unwrap();
        let whole = 0..text.find(')').unwrap() + 1;
        assert_eq!(
            accepted_range(text, &(value..value + 1), Some(whole.clone())),
            Some(7..value + 6),
            "from the line's first character to before its line break"
        );
        assert_eq!(accepted_range(text, &(0..1), Some(whole)), Some(0..5));
    }

    #[test]
    fn server_ranges_count_utf16_units() {
        let text = "a\nlet s = \"🦀\"; x\n";
        let x = text.rfind('x').unwrap();
        // `🦀` is two UTF-16 units and four bytes, so `x` is column 14.
        let range = lsp_types::Range::new(
            lsp_types::Position::new(1, 14),
            lsp_types::Position::new(1, 15),
        );
        assert_eq!(server_range(text, &range), Some(x..x + 1));
        let past = lsp_types::Range::new(
            lsp_types::Position::new(1, 14),
            lsp_types::Position::new(1, 99),
        );
        assert_eq!(server_range(text, &past), None);
        let missing = lsp_types::Range::new(
            lsp_types::Position::new(9, 0),
            lsp_types::Position::new(9, 1),
        );
        assert_eq!(server_range(text, &missing), None);
        let reversed = lsp_types::Range::new(
            lsp_types::Position::new(1, 4),
            lsp_types::Position::new(1, 1),
        );
        assert_eq!(server_range(text, &reversed), None);
    }

    #[test]
    fn the_tooltip_is_anchored_to_the_symbol_or_to_the_character_when_wrapped() {
        let text = "abcdefghijklm";
        let flat = grid(text, None);
        assert_eq!(anchor_bounds(&(2..6), &(3..4), &flat), flat(&(2..6)));
        // `ijkl` wraps after `j`.
        let wrapped = grid(text, Some(10));
        assert_eq!(anchor_bounds(&(8..12), &(8..9), &wrapped), wrapped(&(8..9)));
        assert_eq!(anchor_bounds(&(2..6), &(3..4), |_| None), None);
    }

    #[test]
    fn the_tooltip_stays_open_on_the_symbol_the_card_and_the_gap_between() {
        let symbol = Bounds::from_corners(point(px(100.), px(200.)), point(px(150.), px(220.)));
        let card = Bounds::from_corners(point(px(100.), px(96.)), point(px(400.), px(196.)));
        let gap = px(4.);
        assert!(keeps_open(
            point(px(120.), px(210.)),
            symbol,
            Some(card),
            gap
        ));
        assert!(keeps_open(
            point(px(300.), px(120.)),
            symbol,
            Some(card),
            gap
        ));
        assert!(keeps_open(
            point(px(120.), px(198.)),
            symbol,
            Some(card),
            gap
        ));
        // Beside the symbol, under the card: inside their bounding box, on neither.
        assert!(!keeps_open(
            point(px(300.), px(210.)),
            symbol,
            Some(card),
            gap
        ));
        assert!(!keeps_open(
            point(px(120.), px(230.)),
            symbol,
            Some(card),
            gap
        ));
        // Before the card is painted only the symbol counts.
        assert!(keeps_open(point(px(120.), px(210.)), symbol, None, gap));
        assert!(!keeps_open(point(px(300.), px(120.)), symbol, None, gap));
    }

    #[test]
    fn every_key_but_copy_closes_the_tooltip() {
        assert!(closes_on_key(false, "escape"));
        assert!(closes_on_key(false, "c"));
        assert!(closes_on_key(true, "p"));
        assert!(!closes_on_key(true, "c"));
        assert!(!closes_on_key(true, "C"));
    }
}
