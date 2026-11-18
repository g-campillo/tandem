//! An inserted snippet while the user fills it in: which tab stop is active, where every
//! stop is as the text changes, and the edits that Tab, ⇧Tab and Esc make.
use super::{Expansion, Stop};
use crate::edit::{Edit, map_offset_across};
use std::ops::Range;

/// The tab stops of a snippet in the document. Offsets are bytes in the document. Every
/// stop has at least one range; `span` is the whole inserted snippet. `final_offset` (where
/// `$0` is) can end up inside a character when multibyte text replaces the text around
/// it; `advance` moves it back to the char boundary before it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnippetSession {
    stops: Vec<Stop>,
    active: usize,
    span: Range<usize>,
    final_offset: usize,
}

/// What Tab or ⇧Tab does: the mirror edit to apply first (if any), the selection to show
/// after it, and the session that follows (None when the snippet is done).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advance {
    pub edit: Option<Edit>,
    pub selection: Range<usize>,
    pub next: Option<SnippetSession>,
}

impl SnippetSession {
    /// The session for `expansion` inserted at byte `at`, and the selection of its first
    /// stop. `None` when the snippet has no stop besides `$0`.
    pub fn start(expansion: &Expansion, at: usize) -> Option<(SnippetSession, Range<usize>)> {
        let stops: Vec<Stop> = expansion
            .stops
            .iter()
            .filter(|stop| !stop.ranges.is_empty())
            .map(|stop| Stop {
                index: stop.index,
                ranges: stop
                    .ranges
                    .iter()
                    .map(|range| at + range.start..at + range.end)
                    .collect(),
                choices: stop.choices.clone(),
            })
            .collect();
        let selection = stops.first()?.ranges[0].clone();
        let session = SnippetSession {
            stops,
            active: 0,
            span: at..at + expansion.text.len(),
            final_offset: at + expansion.final_offset,
        };
        Some((session, selection))
    }

    /// The stop the user is filling in.
    pub fn active_stop(&self) -> &Stop {
        &self.stops[self.active]
    }

    /// Every stop range with whether it belongs to the active stop, for drawing.
    pub fn ranges(&self) -> Vec<(Range<usize>, bool)> {
        self.stops
            .iter()
            .enumerate()
            .flat_map(|(index, stop)| {
                let active = index == self.active;
                stop.ranges.iter().map(move |range| (range.clone(), active))
            })
            .collect()
    }

    /// Whether `offset` is inside the snippet, its edges included.
    pub fn contains(&self, offset: usize) -> bool {
        self.span.start <= offset && offset <= self.span.end
    }

    /// Follows a user edit: `start..old_end` (text before the edit) became `new_len`
    /// bytes. Returns false when the edit is outside the span (the session ends): after it,
    /// or across one of its edges. An edit entirely before the span shifts everything.
    ///
    /// Each range, and the span, is shifted by an edit before it, left by one after it,
    /// grown by one inside it and, when the edit overlaps it partly, kept around what is
    /// left of it and the new text. An insertion at an edge of the active stop's range goes
    /// into it (where two of its ranges touch, into the first), and so does one at an edge
    /// of a range that encloses that range, so a placeholder grows with what is typed into
    /// a placeholder nested in it. An insertion at the edge of any other range shifts it at
    /// its start and leaves it at its end.
    pub fn adjust(&mut self, start: usize, old_end: usize, new_len: usize) -> bool {
        let change = Change {
            start,
            old_end: old_end.max(start),
            new_len,
        };
        let before = change.old_end < self.span.start
            || (!change.is_insertion() && change.old_end == self.span.start);
        let within = self.span.start <= change.start && change.old_end <= self.span.end;
        if !before && !within {
            return false;
        }
        let active = self.active;
        let growing = self.growing_range(&change);
        for (index, stop) in self.stops.iter_mut().enumerate() {
            for (position, range) in stop.ranges.iter_mut().enumerate() {
                let sticky = match &growing {
                    Some((grows, _)) if index == active => position == *grows,
                    Some((_, grows)) => {
                        !grows.is_empty() && range.start <= grows.start && grows.end <= range.end
                    }
                    None => false,
                };
                *range = change.range(range, sticky);
            }
        }
        self.span = change.range(&self.span, true);
        self.final_offset = change.point(self.final_offset);
        true
    }

    /// Tab (`forward`) or ⇧Tab: copies the active stop's first range text into its other
    /// ranges as one `Edit` (offsets in `text`), then selects the next/previous stop.
    /// Moving forward past the last stop selects `final_offset` and ends (`next` None).
    /// Moving back from the first stop stays on it. The edit's selection is the returned
    /// selection, and `next` describes the text after the edit.
    pub fn advance(&self, text: &str, forward: bool) -> Advance {
        let (edit, mut session) = self.mirrored(text);
        let (selection, next) = if forward && self.active + 1 >= self.stops.len() {
            let edited = edit.as_ref().map(|edit| edit.apply(text));
            let end = edited
                .as_deref()
                .unwrap_or(text)
                .floor_char_boundary(session.final_offset);
            (end..end, None)
        } else {
            session.active = if forward {
                self.active + 1
            } else {
                self.active.saturating_sub(1)
            };
            (
                session.stops[session.active].ranges[0].clone(),
                Some(session),
            )
        };
        let edit = edit.map(|edit| Edit {
            selection: selection.clone(),
            ..edit
        });
        Advance {
            edit,
            selection,
            next,
        }
    }

    /// Esc or leaving the snippet: the mirror edit of the active stop, if any. Its selection
    /// is a cursor at the end of the active stop's first range.
    pub fn finish(&self, text: &str) -> Option<Edit> {
        let (edit, session) = self.mirrored(text);
        let end = session.stops[session.active].ranges[0].end;
        edit.map(|edit| Edit {
            selection: end..end,
            ..edit
        })
    }

    /// A choice was picked: replaces the active stop's first range with `value` and
    /// returns the edit, the updated session and the selection after the value.
    pub fn replace_active(&self, value: &str) -> (Edit, SnippetSession, Range<usize>) {
        let range = self.stops[self.active].ranges[0].clone();
        let mut session = self.clone();
        session.replace_range(self.active, 0, value.len());
        let end = range.start + value.len();
        let edit = Edit {
            range,
            text: value.to_string(),
            selection: end..end,
        };
        (edit, session, end..end)
    }

    /// For an insertion, the active stop's range that takes it: the first one that holds
    /// its offset, edges included. Its position in the stop's ranges, and the range.
    fn growing_range(&self, change: &Change) -> Option<(usize, Range<usize>)> {
        if !change.is_insertion() {
            return None;
        }
        let ranges = &self.stops[self.active].ranges;
        let position = ranges
            .iter()
            .position(|range| range.start <= change.start && change.start <= range.end)?;
        Some((position, ranges[position].clone()))
    }

    /// The edit that copies the active stop's first range into its other ranges, and the
    /// session after it. No edit when every mirror already matches; mirrors that overlap
    /// an earlier range of the stop or do not fit `text` are left as they are.
    fn mirrored(&self, text: &str) -> (Option<Edit>, SnippetSession) {
        let mut session = self.clone();
        let Some((source, mirrors)) = self.stops[self.active].ranges.split_first() else {
            return (None, session);
        };
        let Some(value) = text.get(source.clone()) else {
            return (None, session);
        };
        let mut targets = Vec::new();
        let mut free_from = source.end;
        for (position, mirror) in mirrors.iter().enumerate() {
            if mirror.start < free_from {
                continue;
            }
            let Some(current) = text.get(mirror.clone()) else {
                continue;
            };
            free_from = mirror.end;
            if current != value {
                targets.push((position + 1, mirror.clone()));
            }
        }
        let (Some((_, first)), Some((_, last))) = (targets.first(), targets.last()) else {
            return (None, session);
        };
        let range = first.start..last.end;
        let mut new = String::new();
        let mut copied = range.start;
        for (_, mirror) in &targets {
            new.push_str(&text[copied..mirror.start]);
            new.push_str(value);
            copied = mirror.end;
        }
        // Last to first, so each replacement leaves the ranges before it where they are.
        for (position, _) in targets.iter().rev() {
            session.replace_range(self.active, *position, value.len());
        }
        let edit = Edit {
            range,
            text: new,
            selection: 0..0,
        };
        (Some(edit), session)
    }

    /// Follows Tandem's own replacement of range `position` of stop `stop` by `new_len`
    /// bytes: that range becomes exactly the new text and the others make room for it.
    fn replace_range(&mut self, stop: usize, position: usize, new_len: usize) {
        let old = self.stops[stop].ranges[position].clone();
        let change = Change {
            start: old.start,
            old_end: old.end,
            new_len,
        };
        for (index, other) in self.stops.iter_mut().enumerate() {
            for (other_position, range) in other.ranges.iter_mut().enumerate() {
                *range = if (index, other_position) == (stop, position) {
                    old.start..old.start + new_len
                } else {
                    change.range(range, false)
                };
            }
        }
        self.span = change.range(&self.span, true);
        self.final_offset = change.point(self.final_offset);
    }
}

/// A replacement of `start..old_end` (offsets in the text before it) by `new_len` bytes.
#[derive(Clone, Copy, Debug)]
struct Change {
    start: usize,
    old_end: usize,
    new_len: usize,
}

impl Change {
    fn is_insertion(&self) -> bool {
        self.start == self.old_end
    }

    /// An offset at or after `old_end`, moved by the length change.
    fn shift(&self, offset: usize) -> usize {
        offset - (self.old_end - self.start) + self.new_len
    }

    /// Where `offset` is after the change, as `Edit::map_offset` says.
    fn point(&self, offset: usize) -> usize {
        map_offset_across(&(self.start..self.old_end), self.new_len, offset)
    }

    /// `range` after the change. A change before it shifts it, one after it leaves it, and
    /// one that overlaps it gives `min(start)..max(mapped end)`: the range then spans what
    /// is left of it and the new text. `sticky` says whether an insertion at its edges goes
    /// into it; otherwise one at its start shifts it and one at its end leaves it.
    fn range(&self, range: &Range<usize>, sticky: bool) -> Range<usize> {
        let Range { start, end } = range.clone();
        if self.is_insertion() {
            let at = self.start;
            if at < start || (at == start && !sticky) {
                return self.shift(start)..self.shift(end);
            }
            if at > end || (at == end && !sticky) {
                return start..end;
            }
            return start..end + self.new_len;
        }
        if self.old_end <= start {
            return self.shift(start)..self.shift(end);
        }
        if self.start >= end {
            return start..end;
        }
        let mapped_end = if end >= self.old_end {
            self.shift(end)
        } else {
            self.start + self.new_len
        };
        start.min(self.start)..mapped_end
    }
}

#[cfg(test)]
mod tests {
    use super::super::{SnippetIndent, expand};
    use super::*;
    use std::collections::HashMap;

    /// A document holding a snippet session, edited the way the editor edits it.
    #[derive(Clone)]
    struct Buffer {
        text: String,
        session: SnippetSession,
    }

    /// `text` with `body` expanded at `at`, and the selection of its first stop.
    fn insert(text: &str, at: usize, body: &str) -> (Buffer, Range<usize>) {
        let indent = SnippetIndent {
            line: String::new(),
            unit: "\t".to_string(),
        };
        let expansion = expand(body, &HashMap::new(), &indent);
        let (session, selection) =
            SnippetSession::start(&expansion, at).expect("the snippet has tab stops");
        let text = format!("{}{}{}", &text[..at], expansion.text, &text[at..]);
        (Buffer { text, session }, selection)
    }

    impl Buffer {
        /// The user replaces `range` by `new`; whether the session goes on.
        fn type_over(&mut self, range: Range<usize>, new: &str) -> bool {
            self.text.replace_range(range.clone(), new);
            self.session.adjust(range.start, range.end, new.len())
        }

        fn type_at(&mut self, at: usize, new: &str) -> bool {
            self.type_over(at..at, new)
        }

        /// Each stop as its index and the text of each of its ranges.
        fn stops(&self) -> Vec<(u32, Vec<&str>)> {
            self.session
                .stops
                .iter()
                .map(|stop| {
                    let texts = stop
                        .ranges
                        .iter()
                        .map(|range| &self.text[range.clone()])
                        .collect();
                    (stop.index, texts)
                })
                .collect()
        }

        /// Tab or ⇧Tab: applies the mirror edit and keeps the next session.
        fn advance(&mut self, forward: bool) -> (Range<usize>, Option<Edit>, bool) {
            let advance = self.session.advance(&self.text, forward);
            if let Some(edit) = &advance.edit {
                assert_eq!(edit.selection, advance.selection);
                self.text = edit.apply(&self.text);
            }
            let going_on = advance.next.is_some();
            if let Some(next) = advance.next {
                self.session = next;
            }
            (advance.selection, advance.edit, going_on)
        }
    }

    #[test]
    fn start_shifts_the_stops_by_the_insertion_offset() {
        let (buffer, selection) = insert("// x\n", 3, "fn ${1:name}($2) {$0}");
        assert_eq!(buffer.text, "// fn name() {}x\n");
        assert_eq!(selection, 6..10);
        assert_eq!(&buffer.text[selection], "name");
        assert_eq!(buffer.session.active_stop().index, 1);
        assert_eq!(
            buffer.session.ranges(),
            vec![(6..10, true), (11..11, false)]
        );
        assert_eq!(buffer.session.span, 3..15);
        assert_eq!(buffer.session.final_offset, 14);
        assert!(buffer.session.contains(3) && buffer.session.contains(15));
        assert!(!buffer.session.contains(2) && !buffer.session.contains(16));
    }

    #[test]
    fn a_snippet_without_stops_besides_the_final_one_has_no_session() {
        let indent = SnippetIndent {
            line: String::new(),
            unit: "\t".to_string(),
        };
        for body in ["plain", "f($0)"] {
            let expansion = expand(body, &HashMap::new(), &indent);
            assert_eq!(SnippetSession::start(&expansion, 4), None, "{body:?}");
        }
    }

    #[test]
    fn typing_inside_the_active_placeholder_grows_it_and_shifts_later_stops() {
        let (mut buffer, _) = insert("", 0, "f(${1:ab}, ${2:cd})");
        assert!(buffer.type_at(3, "XY"));
        assert_eq!(buffer.text, "f(aXYb, cd)");
        assert_eq!(buffer.stops(), vec![(1, vec!["aXYb"]), (2, vec!["cd"])]);
        assert_eq!(buffer.session.span, 0..11);
        assert_eq!(buffer.session.final_offset, 11);
    }

    #[test]
    fn typing_at_the_start_of_the_active_placeholder_grows_it() {
        let (mut buffer, _) = insert("", 0, "f(${1:ab}, ${2:cd})");
        assert!(buffer.type_at(2, "X"));
        assert_eq!(buffer.stops(), vec![(1, vec!["Xab"]), (2, vec!["cd"])]);

        // At the start of the snippet the span grows too.
        let (mut buffer, _) = insert("a;", 1, "${1:ab} ${2:cd}");
        assert!(buffer.type_at(1, "X"));
        assert_eq!(buffer.text, "aXab cd;");
        assert_eq!(buffer.stops(), vec![(1, vec!["Xab"]), (2, vec!["cd"])]);
        assert_eq!(buffer.session.span, 1..7);
    }

    #[test]
    fn typing_at_the_end_of_the_active_placeholder_grows_it() {
        let (mut buffer, _) = insert("", 0, "f(${1:ab}, ${2:cd})");
        assert!(buffer.type_at(4, "X"));
        assert!(buffer.type_at(5, "Y"));
        assert_eq!(buffer.text, "f(abXY, cd)");
        assert_eq!(buffer.stops(), vec![(1, vec!["abXY"]), (2, vec!["cd"])]);

        // At the end of the snippet the span and the final offset grow too.
        let (mut buffer, _) = insert(";", 0, "let ${1:x}");
        assert!(buffer.type_at(5, "s"));
        assert_eq!(buffer.text, "let xs;");
        assert_eq!(buffer.stops(), vec![(1, vec!["xs"])]);
        assert_eq!(buffer.session.span, 0..6);
        assert_eq!(buffer.session.final_offset, 6);
    }

    #[test]
    fn deleting_the_active_text_and_retyping_keeps_the_stop() {
        let (mut buffer, selection) = insert("", 0, "f(${1:ab}, ${2:cd})");
        assert!(buffer.type_over(selection, ""));
        assert_eq!(buffer.text, "f(, cd)");
        assert_eq!(buffer.stops(), vec![(1, vec![""]), (2, vec!["cd"])]);
        assert!(buffer.type_at(2, "x"));
        assert!(buffer.type_at(3, "y"));
        assert_eq!(buffer.text, "f(xy, cd)");
        assert_eq!(buffer.stops(), vec![(1, vec!["xy"]), (2, vec!["cd"])]);
    }

    #[test]
    fn typing_over_the_selected_placeholder_replaces_its_text() {
        let (mut buffer, selection) = insert("", 0, "f(${1:ab}, ${2:cd})");
        assert!(buffer.type_over(selection, "z"));
        assert_eq!(buffer.stops(), vec![(1, vec!["z"]), (2, vec!["cd"])]);
    }

    #[test]
    fn an_edit_before_the_span_shifts_everything() {
        let (mut buffer, _) = insert("let a;\n", 7, "f(${1:ab}, ${2:cd})$0");
        let before = buffer.session.clone();
        assert!(buffer.type_over(0..3, "const"));
        assert_eq!(buffer.text, "const a;\nf(ab, cd)");
        assert_eq!(buffer.stops(), vec![(1, vec!["ab"]), (2, vec!["cd"])]);
        assert_eq!(
            buffer.session.span,
            before.span.start + 2..before.span.end + 2
        );
        assert_eq!(buffer.session.final_offset, before.final_offset + 2);

        // A deletion that ends where the snippet starts shifts it as well.
        assert!(buffer.type_over(6..9, ""));
        assert_eq!(buffer.text, "const f(ab, cd)");
        assert_eq!(buffer.stops(), vec![(1, vec!["ab"]), (2, vec!["cd"])]);
        assert_eq!(buffer.session.span, 6..15);
    }

    #[test]
    fn an_edit_after_the_span_ends_the_session() {
        let (mut buffer, _) = insert("x;", 1, "f(${1:ab})");
        assert_eq!(buffer.text, "xf(ab);");
        assert!(!buffer.type_at(7, "z"));

        let (mut buffer, _) = insert("x; y", 1, "f(${1:ab})");
        assert!(!buffer.type_over(8..9, "z"));
    }

    #[test]
    fn an_edit_straddling_an_edge_of_the_span_ends_the_session() {
        let (mut buffer, _) = insert("let a;\n", 7, "f(${1:ab})");
        assert!(!buffer.type_over(5..9, ""));

        let (mut buffer, _) = insert("x;", 1, "f(${1:ab})");
        assert!(!buffer.type_over(5..7, ""));
    }

    #[test]
    fn insertions_at_the_edges_of_other_stops_shift_or_leave_them() {
        let (mut buffer, _) = insert("", 0, "${1:a}${2:b}");
        assert!(buffer.type_at(1, "X"));
        assert_eq!(buffer.stops(), vec![(1, vec!["aX"]), (2, vec!["b"])]);

        let (selection, _, _) = buffer.advance(true);
        assert_eq!(selection, 2..3);
        assert!(buffer.type_at(2, "Y"));
        assert_eq!(buffer.text, "aXYb");
        assert_eq!(buffer.stops(), vec![(1, vec!["aX"]), (2, vec!["Yb"])]);
    }

    #[test]
    fn a_partial_overlap_keeps_the_stop_around_its_rest_and_the_new_text() {
        let (mut buffer, _) = insert("", 0, "${1:ab} ${2:cd}");
        assert!(buffer.type_over(1..4, "X"));
        assert_eq!(buffer.text, "aXd");
        assert_eq!(buffer.stops(), vec![(1, vec!["aX"]), (2, vec!["Xd"])]);
    }

    #[test]
    fn a_nested_placeholder_keeps_its_parent_around_what_is_typed() {
        let (mut buffer, _) = insert("", 0, "${1:foo ${2:bar}}");
        let (selection, _, _) = buffer.advance(true);
        assert_eq!(&buffer.text[selection.clone()], "bar");
        assert!(buffer.type_over(selection, "b"));
        assert!(buffer.type_at(5, "a"));
        assert!(buffer.type_at(6, "z"));
        assert_eq!(buffer.text, "foo baz");
        assert_eq!(buffer.stops(), vec![(1, vec!["foo baz"]), (2, vec!["baz"])]);

        // Typing at the end of the parent leaves the nested stop as it is.
        let (selection, _, _) = buffer.advance(false);
        assert_eq!(selection, 0..7);
        assert!(buffer.type_at(7, "!"));
        assert_eq!(
            buffer.stops(),
            vec![(1, vec!["foo baz!"]), (2, vec!["baz"])]
        );
    }

    #[test]
    fn advance_moves_forward_through_the_stops_to_the_final_offset() {
        let (mut buffer, selection) = insert("", 0, "fn ${1:name}($2) {\n\t$0\n}");
        assert_eq!(buffer.text, "fn name() {\n\t\n}");
        assert_eq!(selection, 3..7);

        let (selection, edit, going_on) = buffer.advance(true);
        assert_eq!((selection, edit, going_on), (8..8, None, true));
        assert_eq!(buffer.session.active_stop().index, 2);
        assert_eq!(buffer.session.ranges(), vec![(3..7, false), (8..8, true)]);

        let advance = buffer.session.advance(&buffer.text, true);
        assert_eq!(advance.edit, None);
        assert_eq!(advance.selection, 13..13);
        assert_eq!(advance.next, None);
    }

    #[test]
    fn advance_back_from_the_second_stop_selects_the_first() {
        let (mut buffer, _) = insert("", 0, "fn ${1:name}($2) {\n\t$0\n}");
        buffer.advance(true);
        assert!(buffer.type_at(8, "x: u8"));
        let (selection, edit, going_on) = buffer.advance(false);
        assert_eq!((selection, edit, going_on), (3..7, None, true));
        assert_eq!(buffer.session.active_stop().index, 1);
        assert_eq!(buffer.stops(), vec![(1, vec!["name"]), (2, vec!["x: u8"])]);
    }

    #[test]
    fn advance_back_from_the_first_stop_stays_on_it() {
        let (mut buffer, _) = insert("", 0, "fn ${1:name}($2) {}");
        assert!(buffer.type_at(7, "s"));
        let (selection, _, going_on) = buffer.advance(false);
        assert_eq!((selection, going_on), (3..8, true));
        assert_eq!(buffer.session.active_stop().index, 1);
    }

    #[test]
    fn advance_copies_the_typed_text_into_the_mirrors_as_one_edit() {
        let (mut buffer, selection) = insert("", 0, "${1:a} ${2:b} $1 ${3:c} $1");
        assert_eq!(buffer.text, "a b a c a");
        assert!(buffer.type_over(selection, "xy"));
        assert_eq!(buffer.text, "xy b a c a");

        let (selection, edit, going_on) = buffer.advance(true);
        let edit = edit.expect("the mirrors change");
        assert_eq!((edit.range, edit.text.as_str()), (5..10, "xy c xy"));
        assert_eq!(buffer.text, "xy b xy c xy");
        assert!(going_on);
        assert_eq!(&buffer.text[selection], "b");
        assert_eq!(
            buffer.stops(),
            vec![(1, vec!["xy", "xy", "xy"]), (2, vec!["b"]), (3, vec!["c"])]
        );
        assert_eq!(buffer.session.span, 0..12);
        assert_eq!(buffer.session.final_offset, 12);

        // Once the mirrors match, moving on edits nothing.
        let (selection, edit, _) = buffer.advance(false);
        assert_eq!((selection, edit), (0..2, None));
        let (selection, edit, _) = buffer.advance(true);
        assert_eq!((selection, edit), (3..4, None));
    }

    #[test]
    fn a_mirror_after_a_later_stop_moves_that_stop_correctly() {
        let (mut buffer, selection) = insert("", 0, "for ${1:i} in 0..${2:n} { $1 }$0");
        assert_eq!(buffer.text, "for i in 0..n { i }");
        assert!(buffer.type_over(selection, "idx"));
        let (selection, edit, _) = buffer.advance(true);
        let edit = edit.expect("the mirror changes");
        assert_eq!((edit.range, edit.text.as_str()), (18..19, "idx"));
        assert_eq!(buffer.text, "for idx in 0..n { idx }");
        assert_eq!(&buffer.text[selection], "n");
        assert_eq!(
            buffer.stops(),
            vec![(1, vec!["idx", "idx"]), (2, vec!["n"])]
        );
        let (selection, _, going_on) = buffer.advance(true);
        assert_eq!((selection, going_on), (23..23, false));
    }

    #[test]
    fn typing_where_two_mirrors_touch_grows_only_the_first() {
        let (mut buffer, _) = insert("", 0, "${1:a}$1");
        assert_eq!(buffer.text, "aa");
        assert!(buffer.type_at(1, "b"));
        assert_eq!(buffer.stops(), vec![(1, vec!["ab", "a"])]);
        let edit = buffer
            .session
            .finish(&buffer.text)
            .expect("the mirror changes");
        assert_eq!((edit.range.clone(), edit.text.as_str()), (2..3, "ab"));
        assert_eq!(edit.apply(&buffer.text), "abab");
    }

    #[test]
    fn finish_returns_the_mirror_edit_of_the_active_stop() {
        let (mut buffer, selection) = insert("", 0, "${1:i} = $1 + 1;");
        assert_eq!(buffer.session.finish(&buffer.text), None);
        assert!(buffer.type_over(selection, "n"));
        assert!(buffer.type_at(1, "ext"));
        let edit = buffer
            .session
            .finish(&buffer.text)
            .expect("the mirror changes");
        assert_eq!(edit.apply(&buffer.text), "next = next + 1;");
        assert_eq!(edit.selection, 4..4);

        let (buffer, _) = insert("", 0, "f(${1:a}, $2)");
        assert_eq!(buffer.session.finish(&buffer.text), None);
    }

    #[test]
    fn replace_active_puts_a_choice_into_the_first_range() {
        let (mut buffer, selection) = insert("", 0, r#"<input type="${1|text,email|}" name="$2">"#);
        assert_eq!(buffer.text, r#"<input type="text" name="">"#);
        assert_eq!(selection, 13..17);
        assert_eq!(buffer.session.active_stop().choices, vec!["text", "email"]);

        let (edit, session, selection) = buffer.session.replace_active("email");
        assert_eq!((edit.range.clone(), edit.text.as_str()), (13..17, "email"));
        assert_eq!(selection, 18..18);
        assert_eq!(edit.selection, selection);
        buffer.text = edit.apply(&buffer.text);
        buffer.session = session;
        assert_eq!(buffer.text, r#"<input type="email" name="">"#);
        assert_eq!(buffer.stops(), vec![(1, vec!["email"]), (2, vec![""])]);
        assert_eq!(buffer.session.ranges()[1], (26..26, false));
        assert_eq!(buffer.session.span, 0..28);
    }

    #[test]
    fn the_final_selection_stays_on_a_char_boundary_after_multibyte_text_replaces_it() {
        let (mut buffer, _) = insert("", 0, "fn ${1:name}($2) {\n\t$0\n}");
        assert!(buffer.type_over(12..15, "é"));
        assert_eq!(buffer.text, "fn name() {\né");
        buffer.advance(true);
        let (selection, _, going_on) = buffer.advance(true);
        assert_eq!((selection, going_on), (12..12, false));

        // The same when a picked choice replaces the text around the final offset.
        let (buffer, _) = insert("", 0, "${1:a$0b}");
        let (edit, session, _) = buffer.session.replace_active("éé");
        let text = edit.apply(&buffer.text);
        let advance = session.advance(&text, true);
        assert_eq!((advance.selection, advance.next), (0..0, None));
    }

    /// Asserts that `range` lies in `text` on char boundaries.
    fn assert_on_boundaries(text: &str, range: &Range<usize>, what: &str) {
        assert!(
            range.start <= range.end
                && text.is_char_boundary(range.start)
                && text.is_char_boundary(range.end),
            "{what}: {range:?} in {text:?}"
        );
    }

    impl Buffer {
        /// Tabs through every stop to the end, checking that every edit, selection and
        /// range stays on char boundaries of the text it belongs to.
        fn tab_to_the_end_on_boundaries(&mut self, forward_first: bool) {
            let mut forward = forward_first;
            for _ in 0..=self.session.stops.len() {
                let before = self.text.clone();
                let (selection, edit, going_on) = self.advance(forward);
                if let Some(edit) = &edit {
                    assert_on_boundaries(&before, &edit.range, "mirror edit");
                }
                assert_on_boundaries(&self.text, &selection, "selection");
                if !going_on {
                    return;
                }
                for (range, _) in self.session.ranges() {
                    assert_on_boundaries(&self.text, &range, "stop range");
                }
                forward = true;
            }
            panic!("Tab never left the snippet in {:?}", self.text);
        }
    }

    #[test]
    fn every_user_edit_keeps_edits_and_selections_on_char_boundaries() {
        let bodies = [
            "fn ${1:name}($2) {\n\t$0\n}",
            "${1:a$0b} $1 ${2:é}",
            "${1:foo ${2:bär}}$0 $2",
        ];
        for body in bodies {
            let (buffer, _) = insert("éé", 2, body);
            let text = &buffer.text;
            let boundaries: Vec<usize> = (0..=text.len())
                .filter(|i| text.is_char_boundary(*i))
                .collect();
            for &start in &boundaries {
                for &end in boundaries.iter().filter(|end| **end >= start) {
                    for new in ["", "x", "é", "ñé"] {
                        let mut edited = buffer.clone();
                        if !edited.type_over(start..end, new) {
                            continue;
                        }
                        for forward_first in [true, false] {
                            edited.clone().tab_to_the_end_on_boundaries(forward_first);
                        }
                        if let Some(edit) = edited.session.finish(&edited.text) {
                            assert_on_boundaries(&edited.text, &edit.range, "finish edit");
                            let after = edit.apply(&edited.text);
                            assert_on_boundaries(&after, &edit.selection, "finish selection");
                        }
                        let (edit, session, selection) = edited.session.replace_active("ñé");
                        assert_on_boundaries(&edited.text, &edit.range, "choice edit");
                        let mut chosen = Buffer {
                            text: edit.apply(&edited.text),
                            session,
                        };
                        assert_on_boundaries(&chosen.text, &selection, "choice selection");
                        chosen.tab_to_the_end_on_boundaries(true);
                    }
                }
            }
        }
    }

    #[test]
    fn mirror_ranges_that_do_not_fit_the_text_are_left_alone() {
        let (buffer, _) = insert("", 0, "${1:a} $1 ${2:b}");
        assert_eq!(buffer.session.finish(""), None);
        let advance = buffer.session.advance("", true);
        assert_eq!(advance.edit, None);
        assert_eq!(advance.selection, 4..5);
    }
}
