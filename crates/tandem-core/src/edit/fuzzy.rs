//! Fuzzy matching for the completion list: a case-insensitive, in-order subsequence match
//! that starts at a word start, scored so that prefixes, word starts and runs of
//! consecutive characters rank first.
use std::ops::Range;

/// How well a query matched a target: a higher `score` ranks first, and `ranges` are the
/// matched byte ranges in the target, with adjacent ones merged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuzzyMatch {
    pub score: i32,
    pub ranges: Vec<Range<usize>>,
}

/// Every matched character.
const MATCHED: i32 = 10;
/// A matched character at the target's start or a word start.
const AT_WORD_START: i32 = 8;
/// A matched character directly after the previous matched one.
const CONSECUTIVE: i32 = 5;
/// A matched character in the same case as the query's.
const SAME_CASE: i32 = 2;
/// Each target character skipped before the first match costs one, up to this many.
const MAX_LEADING_SKIP: usize = 10;
/// The target starts with the query, ignoring case.
const PREFIX: i32 = 20;
/// The target is the query, ignoring case (on top of `PREFIX`).
const EQUAL: i32 = 30;

/// The best-scoring match of `query` in `target`, or `None` when its characters do not
/// all appear in order (ignoring case) with the first one at a word start. An empty
/// query matches with score 0 and no ranges.
pub fn fuzzy_match(query: &str, target: &str) -> Option<FuzzyMatch> {
    if query.is_empty() {
        return Some(FuzzyMatch {
            score: 0,
            ranges: Vec::new(),
        });
    }
    let query: Vec<char> = query.chars().collect();
    let target: Vec<(usize, char)> = target.char_indices().collect();
    let (rows, columns) = (query.len(), target.len());
    if rows > columns {
        return None;
    }
    let word_starts: Vec<bool> = (0..columns).map(|j| is_word_start(&target, j)).collect();
    let hit = |i: usize, j: usize| -> i32 {
        let (q, t) = (query[i], target[j].1);
        MATCHED
            + if word_starts[j] { AT_WORD_START } else { 0 }
            + if q == t { SAME_CASE } else { 0 }
    };

    // `best[i * columns + j]`: the best score of matching `query[..=i]` with `query[i]` on
    // `target[j]`; `previous` holds where `query[i - 1]` matched on that alignment.
    let mut best: Vec<Option<i32>> = vec![None; rows * columns];
    let mut previous = vec![0; rows * columns];
    for j in 0..columns {
        if word_starts[j] && same_ignoring_case(query[0], target[j].1) {
            best[j] = Some(hit(0, j) - j.min(MAX_LEADING_SKIP) as i32);
        }
    }
    for (i, &wanted) in query.iter().enumerate().skip(1) {
        let (row, above) = (i * columns, (i - 1) * columns);
        // The best alignment of `query[..i]` that ends at least two characters before `j`.
        let mut apart: Option<(i32, usize)> = None;
        for j in i..columns {
            if j >= 2
                && let Some(score) = best[above + j - 2]
                && apart.is_none_or(|(kept, _)| score > kept)
            {
                apart = Some((score, j - 2));
            }
            if !same_ignoring_case(wanted, target[j].1) {
                continue;
            }
            let adjacent = best[above + j - 1].map(|score| (score + CONSECUTIVE, j - 1));
            let chosen = match (adjacent, apart) {
                (Some(near), Some(far)) if far.0 > near.0 => Some(far),
                (Some(near), _) => Some(near),
                (None, far) => far,
            };
            if let Some((score, from)) = chosen {
                best[row + j] = Some(score + hit(i, j));
                previous[row + j] = from;
            }
        }
    }

    let last = (rows - 1) * columns;
    let (mut score, mut j) = (0..columns)
        .filter_map(|j| best[last + j].map(|score| (score, j)))
        .reduce(|kept, next| if next.0 > kept.0 { next } else { kept })?;
    let mut matched = vec![j; rows];
    for i in (1..rows).rev() {
        j = previous[i * columns + j];
        matched[i - 1] = j;
    }

    if query
        .iter()
        .zip(&target)
        .all(|(&q, &(_, t))| same_ignoring_case(q, t))
    {
        score += PREFIX;
        if rows == columns {
            score += EQUAL;
        }
    }
    Some(FuzzyMatch {
        score,
        ranges: merged_ranges(&target, &matched),
    })
}

/// Whether two characters are equal ignoring case.
fn same_ignoring_case(a: char, b: char) -> bool {
    a == b || a.to_lowercase().eq(b.to_lowercase())
}

/// Whether `target[j]` starts a word: it is the first character, follows one of
/// `_ - . : / space`, or is an upper-case letter after a lower-case one.
fn is_word_start(target: &[(usize, char)], j: usize) -> bool {
    let Some(&(_, before)) = j.checked_sub(1).and_then(|i| target.get(i)) else {
        return true;
    };
    matches!(before, '_' | '-' | '.' | ':' | '/' | ' ')
        || (before.is_lowercase() && target[j].1.is_uppercase())
}

/// The byte ranges of the `matched` characters (ascending indices into `target`), with
/// adjacent ones merged.
fn merged_ranges(target: &[(usize, char)], matched: &[usize]) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for &j in matched {
        let (start, c) = target[j];
        let end = start + c.len_utf8();
        match ranges.last_mut() {
            Some(range) if range.end == start => range.end = end,
            _ => ranges.push(start..end),
        }
    }
    ranges
}

#[cfg(test)]
// A run of matched characters is one range, so `[0..3]` is the intended value.
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    fn score(query: &str, target: &str) -> i32 {
        fuzzy_match(query, target)
            .unwrap_or_else(|| panic!("{query:?} should match {target:?}"))
            .score
    }

    fn ranges(query: &str, target: &str) -> Vec<Range<usize>> {
        fuzzy_match(query, target)
            .unwrap_or_else(|| panic!("{query:?} should match {target:?}"))
            .ranges
    }

    #[test]
    fn the_exact_word_ranks_before_longer_words_with_the_same_prefix() {
        assert!(score("len", "len") > score("len", "length"));
        // Both start with the query, so they score the same; the completion list breaks
        // such ties by label length.
        assert_eq!(score("len", "length"), score("len", "lenient"));
        let mut ranked = vec!["lenient", "length", "len"];
        ranked.sort_by_key(|target| (std::cmp::Reverse(score("len", target)), target.len()));
        assert_eq!(ranked, ["len", "length", "lenient"]);
    }

    #[test]
    fn initials_match_camel_case_and_snake_case_word_starts() {
        assert_eq!(ranges("fb", "FooBar"), [0..1, 3..4]);
        assert_eq!(ranges("fb", "foo_bar"), [0..1, 4..5]);
    }

    #[test]
    fn the_first_character_must_match_at_a_word_start() {
        assert_eq!(fuzzy_match("ob", "FooBar"), None);
        assert_eq!(fuzzy_match("ar", "foo_bar"), None);
    }

    #[test]
    fn upper_case_initials_match_their_words() {
        assert_eq!(ranges("HM", "HashMap"), [0..1, 4..5]);
        assert_eq!(ranges("hm", "HashMap"), [0..1, 4..5]);
    }

    #[test]
    fn the_same_case_scores_higher() {
        assert!(score("Foo", "Foo") > score("Foo", "foo"));
    }

    #[test]
    fn adjacent_hits_merge_into_one_range() {
        assert_eq!(ranges("len", "length"), [0..3]);
        assert_eq!(ranges("hamap", "HashMap"), [0..2, 4..7]);
    }

    #[test]
    fn ranges_in_a_multibyte_target_lie_on_char_boundaries() {
        let target = "naïve_count";
        assert_eq!(ranges("nc", target), [0..1, 7..8]);
        assert_eq!(ranges("üä", "über_änderung"), [0..2, 6..8]);
        assert_eq!(ranges("ÜÄn", "über_änderung"), [0..2, 6..9]);
        for range in ranges("naïv", target) {
            assert!(target.is_char_boundary(range.start) && target.is_char_boundary(range.end));
        }
    }

    #[test]
    fn an_empty_query_matches_everything_with_no_ranges() {
        let expected = FuzzyMatch {
            score: 0,
            ranges: Vec::new(),
        };
        assert_eq!(fuzzy_match("", "anything"), Some(expected.clone()));
        assert_eq!(fuzzy_match("", ""), Some(expected));
    }

    #[test]
    fn a_query_longer_than_the_target_does_not_match() {
        assert_eq!(fuzzy_match("lengthy", "len"), None);
        assert_eq!(fuzzy_match("a", ""), None);
    }

    #[test]
    fn the_best_alignment_wins_over_the_first_one() {
        // Taking the first `a` and the first `b` leaves them apart; the later word start
        // keeps them together, which scores higher despite the skipped characters.
        assert_eq!(ranges("ab", "axb_ab"), [4..6]);
    }

    #[test]
    fn skipped_leading_characters_cost_at_most_ten() {
        // b: 10 + 8 (word start) + 2 (case), minus one per skipped character.
        assert_eq!(score("b", "a_b"), 18);
        assert_eq!(score("b", "aaaaaaaa_b"), 11);
        assert_eq!(score("b", "aaaaaaaaaaaaaaaaaaaa_b"), 10);
        assert_eq!(score("b", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa_b"), 10);
    }

    #[test]
    fn the_score_adds_up_the_bonuses() {
        // f: 10 + 8 (start) + 2 (case); b: 10 + 8 (word start) + 2 (case).
        assert_eq!(score("fb", "foo_bar"), 40);
        // l, e, n: 30 + 8 (start) + 10 (two consecutive) + 6 (case) + 20 (prefix).
        assert_eq!(score("len", "length"), 74);
        // The same plus 30 for being equal.
        assert_eq!(score("len", "len"), 104);
        // M: 10 + 8 (word start) - 4 (skipped "Hash"), plus 2 when the case matches.
        assert_eq!(score("m", "HashMap"), 14);
        assert_eq!(score("M", "HashMap"), 16);
    }
}
