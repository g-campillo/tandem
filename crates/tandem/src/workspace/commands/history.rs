//! Where Back and Forward (⌃-, ⌃⇧-) go, and what Go to Line (⌃G) accepts.

/// A cursor position in a project file; `line` and `column` are 0-based.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub path: String,
    pub line: u32,
    pub column: u32,
}

impl Location {
    /// Two locations are the same place when they are on the same line of the same file.
    fn same_place(&self, other: &Location) -> bool {
        self.path == other.path && self.line == other.line
    }
}

/// The most locations Back can return to.
const MAX_BACK: usize = 100;

/// Where the user jumped from (back) and came back from (forward).
#[derive(Default)]
pub struct NavHistory {
    back: Vec<Location>,
    forward: Vec<Location>,
}

impl NavHistory {
    /// Remembers where a jump started; a new jump forgets the locations Back left.
    pub fn record(&mut self, from: Location) {
        if self.back.last().is_some_and(|last| last.same_place(&from)) {
            return;
        }
        self.back.push(from);
        if self.back.len() > MAX_BACK {
            self.back.remove(0);
        }
        self.forward.clear();
    }

    /// The previous location that is not `current`'s line; `current` becomes a Forward target.
    pub fn back(&mut self, current: Option<Location>) -> Option<Location> {
        let target = pop_other_place(&mut self.back, current.as_ref())?;
        self.forward.extend(current);
        Some(target)
    }

    /// The location Back left that is not `current`'s line; `current` becomes a Back target.
    pub fn forward(&mut self, current: Option<Location>) -> Option<Location> {
        let target = pop_other_place(&mut self.forward, current.as_ref())?;
        self.back.extend(current);
        Some(target)
    }
}

/// Pops entries until one is not the same place as `current`.
fn pop_other_place(stack: &mut Vec<Location>, current: Option<&Location>) -> Option<Location> {
    while let Some(entry) = stack.pop() {
        if !current.is_some_and(|current| current.same_place(&entry)) {
            return Some(entry);
        }
    }
    None
}

/// Parses "12", "12:5", "12,5" or ":12" into a 1-based (line, column).
pub fn parse_line_target(input: &str, line_count: usize) -> Option<(u32, u32)> {
    let input = input.trim();
    let input = input.strip_prefix(':').unwrap_or(input);
    let (line, column) = match input.split_once([':', ',']) {
        Some((line, column)) => (line, Some(column)),
        None => (input, None),
    };
    let line: u32 = line.trim().parse().ok()?;
    let last_line = u32::try_from(line_count.max(1)).unwrap_or(u32::MAX);
    let column = column
        .and_then(|column| column.trim().parse::<u32>().ok())
        .unwrap_or(1)
        .max(1);
    Some((line.clamp(1, last_line), column))
}

#[cfg(test)]
mod tests {
    use super::{Location, NavHistory, parse_line_target};

    fn at(path: &str, line: u32, column: u32) -> Location {
        Location {
            path: path.into(),
            line,
            column,
        }
    }

    #[test]
    fn back_returns_the_recorded_location_and_forward_returns_to_where_back_was_called() {
        let mut history = NavHistory::default();
        history.record(at("src/main.rs", 4, 2));
        assert_eq!(
            history.back(Some(at("src/cli.rs", 0, 0))),
            Some(at("src/main.rs", 4, 2))
        );
        assert_eq!(
            history.forward(Some(at("src/main.rs", 4, 2))),
            Some(at("src/cli.rs", 0, 0))
        );
        assert_eq!(
            history.back(Some(at("src/cli.rs", 0, 0))),
            Some(at("src/main.rs", 4, 2))
        );
    }

    #[test]
    fn recording_the_same_place_twice_keeps_one_entry() {
        let mut history = NavHistory::default();
        history.record(at("a.rs", 3, 0));
        history.record(at("a.rs", 3, 7));
        assert_eq!(history.back(None), Some(at("a.rs", 3, 0)));
        assert_eq!(history.back(None), None);
    }

    #[test]
    fn record_clears_the_forward_stack() {
        let mut history = NavHistory::default();
        history.record(at("a.rs", 1, 0));
        assert_eq!(history.back(Some(at("b.rs", 2, 0))), Some(at("a.rs", 1, 0)));
        history.record(at("a.rs", 1, 0));
        assert_eq!(history.forward(Some(at("c.rs", 0, 0))), None);
    }

    #[test]
    fn back_skips_entries_on_the_current_line() {
        let mut history = NavHistory::default();
        history.record(at("a.rs", 1, 0));
        history.record(at("b.rs", 5, 0));
        assert_eq!(history.back(Some(at("b.rs", 5, 9))), Some(at("a.rs", 1, 0)));
    }

    #[test]
    fn back_on_an_empty_stack_is_none() {
        let mut history = NavHistory::default();
        assert_eq!(history.back(Some(at("a.rs", 0, 0))), None);
        assert_eq!(history.forward(Some(at("a.rs", 0, 0))), None);
        history.record(at("a.rs", 2, 0));
        assert_eq!(history.back(Some(at("a.rs", 2, 4))), None);
    }

    #[test]
    fn forward_skips_entries_on_the_current_line() {
        let mut history = NavHistory::default();
        history.record(at("a.rs", 1, 0));
        history.record(at("b.rs", 2, 0));
        assert_eq!(history.back(Some(at("c.rs", 3, 0))), Some(at("b.rs", 2, 0)));
        assert_eq!(history.back(Some(at("b.rs", 2, 0))), Some(at("a.rs", 1, 0)));
        // The forward stack holds b.rs:2 on top of c.rs:3; standing on b.rs:2 skips it.
        assert_eq!(
            history.forward(Some(at("b.rs", 2, 5))),
            Some(at("c.rs", 3, 0))
        );
    }

    #[test]
    fn the_101st_record_drops_the_oldest() {
        let mut history = NavHistory::default();
        for line in 0..101 {
            history.record(at("a.rs", line, 0));
        }
        let mut lines = Vec::new();
        while let Some(location) = history.back(None) {
            lines.push(location.line);
        }
        assert_eq!(lines.len(), 100);
        assert_eq!(lines.first(), Some(&100));
        assert_eq!(lines.last(), Some(&1));
    }

    #[test]
    fn back_without_a_current_location_still_returns_the_entry() {
        let mut history = NavHistory::default();
        history.record(at("a.rs", 1, 0));
        assert_eq!(history.back(None), Some(at("a.rs", 1, 0)));
        // Nothing was pushed onto the forward stack.
        assert_eq!(history.forward(None), None);
    }

    #[test]
    fn parse_line_target_reads_a_line_and_an_optional_column() {
        assert_eq!(parse_line_target("12", 100), Some((12, 1)));
        assert_eq!(parse_line_target("12:5", 100), Some((12, 5)));
        assert_eq!(parse_line_target("12,5", 100), Some((12, 5)));
        assert_eq!(parse_line_target(":12", 100), Some((12, 1)));
        assert_eq!(parse_line_target(" 7 ", 100), Some((7, 1)));
    }

    #[test]
    fn parse_line_target_clamps_the_line_and_the_column() {
        assert_eq!(parse_line_target("0", 100), Some((1, 1)));
        assert_eq!(parse_line_target("250", 100), Some((100, 1)));
        assert_eq!(parse_line_target("12:0", 100), Some((12, 1)));
        assert_eq!(parse_line_target("12:x", 100), Some((12, 1)));
        assert_eq!(parse_line_target("5", 0), Some((1, 1)));
    }

    #[test]
    fn parse_line_target_rejects_text_that_is_not_a_line_number() {
        assert_eq!(parse_line_target("", 100), None);
        assert_eq!(parse_line_target("abc", 100), None);
        assert_eq!(parse_line_target("-3", 100), None);
    }
}
