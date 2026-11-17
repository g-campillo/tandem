//! Snippet bodies in the VS Code / TextMate syntax: parsing them and expanding them into
//! the text to insert, with the byte ranges of their tab stops.
use std::collections::{BTreeMap, HashMap};
use std::ops::Range;

/// The variables a snippet may refer to by name. Any other name is an unknown variable.
pub const KNOWN_VARIABLES: &[&str] = &[
    "TM_SELECTED_TEXT",
    "TM_CURRENT_LINE",
    "TM_CURRENT_WORD",
    "TM_LINE_INDEX",
    "TM_LINE_NUMBER",
    "TM_FILENAME",
    "TM_FILENAME_BASE",
    "TM_DIRECTORY",
    "TM_FILEPATH",
    "RELATIVE_FILEPATH",
    "CLIPBOARD",
    "CURRENT_YEAR",
    "CURRENT_YEAR_SHORT",
    "CURRENT_MONTH",
    "CURRENT_MONTH_NAME",
    "CURRENT_MONTH_NAME_SHORT",
    "CURRENT_DATE",
    "CURRENT_DAY_NAME",
    "CURRENT_DAY_NAME_SHORT",
    "CURRENT_HOUR",
    "CURRENT_MINUTE",
    "CURRENT_SECOND",
    "CURRENT_SECONDS_UNIX",
    "UUID",
    "LINE_COMMENT",
    "BLOCK_COMMENT_START",
    "BLOCK_COMMENT_END",
];

/// One element of a parsed snippet body.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Text(String),
    TabStop(u32),
    Placeholder(u32, Vec<Node>),
    Choice(u32, Vec<String>),
    Variable(String, Option<Vec<Node>>),
}

/// How deep placeholders and variable defaults may nest, and how deep rendering (which
/// also follows mirrors into their defaults) may go. Deeper constructs stay literal text
/// and deeper renders stay empty, so a body from a language server cannot exhaust the stack.
const MAX_NESTING: usize = 100;

/// Parses `$1`, `${1}`, `${1:default}`, `${1|a,b|}`, `$name`, `${name}`, `${name:default}`
/// and transforms (`${1/regex/format/flags}`, `${name/…/…/…}`, kept as the plain tab stop
/// or variable). `\$`, `\}` and `\\` are escapes everywhere; inside a choice `\,` and `\|`
/// are too. A construct that is not well formed is literal text; an unclosed default keeps
/// what was parsed inside it after its literal opening.
pub fn parse(body: &str) -> Vec<Node> {
    let mut parser = Parser {
        body,
        pos: 0,
        depth: 0,
    };
    let mut nodes = Vec::new();
    while parser.element(&mut nodes) {}
    nodes
}

struct Parser<'a> {
    body: &'a str,
    pos: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<char> {
        self.body[self.pos..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn eat(&mut self, c: char) -> bool {
        let found = self.peek() == Some(c);
        if found {
            self.pos += c.len_utf8();
        }
        found
    }

    /// The run of chars from here that satisfy `first`, then `rest`.
    fn token(&mut self, first: fn(char) -> bool, rest: fn(char) -> bool) -> Option<&'a str> {
        let start = self.pos;
        if !self.peek().is_some_and(first) {
            return None;
        }
        self.bump();
        while self.peek().is_some_and(rest) {
            self.bump();
        }
        Some(&self.body[start..self.pos])
    }

    /// The digits from here as an index: None without digits, `Some(None)` when they do
    /// not fit a `u32`.
    fn index(&mut self) -> Option<Option<u32>> {
        self.token(|c| c.is_ascii_digit(), |c| c.is_ascii_digit())
            .map(|digits| digits.parse().ok())
    }

    fn name(&mut self) -> Option<&'a str> {
        self.token(
            |c| c == '_' || c.is_ascii_alphabetic(),
            |c| c == '_' || c.is_ascii_alphanumeric(),
        )
    }

    /// Parses one escape, construct or char onto `nodes`. False at the end of the body.
    fn element(&mut self, nodes: &mut Vec<Node>) -> bool {
        let start = self.pos;
        match self.bump() {
            None => return false,
            Some('\\') => match self.peek() {
                Some(escaped @ ('$' | '}' | '\\')) => {
                    self.bump();
                    push_text(nodes, escaped.encode_utf8(&mut [0; 4]));
                }
                _ => push_text(nodes, "\\"),
            },
            Some('$') => {
                if !self.construct(start, nodes) {
                    self.pos = start + 1;
                    push_text(nodes, "$");
                }
            }
            Some(c) => push_text(nodes, c.encode_utf8(&mut [0; 4])),
        }
        true
    }

    /// After the `$` at `start`: a tab stop, placeholder, choice or variable pushed onto
    /// `nodes`, or false when none is there (the position is then undefined).
    fn construct(&mut self, start: usize, nodes: &mut Vec<Node>) -> bool {
        if let Some(index) = self.index() {
            let Some(index) = index else { return false };
            nodes.push(Node::TabStop(index));
            return true;
        }
        if let Some(name) = self.name() {
            nodes.push(Node::Variable(name.to_string(), None));
            return true;
        }
        if self.depth >= MAX_NESTING || !self.eat('{') {
            return false;
        }
        if let Some(index) = self.index() {
            let Some(index) = index else { return false };
            if self.eat(':') {
                self.default(start, nodes, |children| Node::Placeholder(index, children));
                return true;
            }
            if index > 0 && self.eat('|') {
                let Some(options) = self.choice() else {
                    return false;
                };
                nodes.push(Node::Choice(index, options));
                return true;
            }
            if self.eat('}') || (self.eat('/') && self.transform()) {
                nodes.push(Node::TabStop(index));
                return true;
            }
            return false;
        }
        if let Some(name) = self.name() {
            let name = name.to_string();
            if self.eat(':') {
                self.default(start, nodes, |children| {
                    Node::Variable(name, Some(children))
                });
                return true;
            }
            if self.eat('}') || (self.eat('/') && self.transform()) {
                nodes.push(Node::Variable(name, None));
                return true;
            }
        }
        false
    }

    /// The default after `${1:` or `${name:` up to its closing `}`, pushed as `make(children)`.
    /// Without a closing brace the opening from `start` is literal text and the children
    /// parsed so far follow it.
    fn default(
        &mut self,
        start: usize,
        nodes: &mut Vec<Node>,
        make: impl FnOnce(Vec<Node>) -> Node,
    ) {
        let opening = &self.body[start..self.pos];
        let mut children = Vec::new();
        self.depth += 1;
        let closed = loop {
            if self.eat('}') {
                break true;
            }
            if !self.element(&mut children) {
                break false;
            }
        };
        self.depth -= 1;
        if closed {
            nodes.push(make(children));
        } else {
            push_text(nodes, opening);
            for child in children {
                push_node(nodes, child);
            }
        }
    }

    /// The options of `${1|a,b|}` after the first `|`, through the closing `|}`. None when
    /// the choice is not closed or has an empty option.
    fn choice(&mut self) -> Option<Vec<String>> {
        let mut options = Vec::new();
        let mut option = String::new();
        loop {
            match self.bump()? {
                '\\' => match self.peek() {
                    Some(escaped @ ('$' | '}' | '\\' | ',' | '|')) => {
                        self.bump();
                        option.push(escaped);
                    }
                    _ => option.push('\\'),
                },
                separator @ (',' | '|') => {
                    if option.is_empty() {
                        return None;
                    }
                    options.push(std::mem::take(&mut option));
                    if separator == '|' {
                        return self.eat('}').then_some(options);
                    }
                }
                c => option.push(c),
            }
        }
    }

    /// Skips `regex/format/flags}` after a transform's first `/`. False when it is not closed.
    fn transform(&mut self) -> bool {
        // The regex: `\/` is an escaped slash.
        loop {
            match self.bump() {
                None => return false,
                Some('/') => break,
                Some('\\') => {
                    self.eat('/');
                }
                Some(_) => {}
            }
        }
        // The format: `\\` and `\/` are escapes, and a `${1:…}` group may hold slashes.
        loop {
            match self.bump() {
                None => return false,
                Some('/') => break,
                Some('\\') => {
                    let _ = self.eat('\\') || self.eat('/');
                }
                Some('$') => {
                    let resume = self.pos;
                    if !self.format_group() {
                        self.pos = resume;
                    }
                }
                Some(_) => {}
            }
        }
        // The flags.
        loop {
            match self.bump() {
                None => return false,
                Some('}') => return true,
                Some(_) => {}
            }
        }
    }

    /// Skips a `{1}` or `{1:…}` format group after its `$`. False when there is none.
    fn format_group(&mut self) -> bool {
        if !self.eat('{') || self.index().is_none() {
            return false;
        }
        loop {
            match self.bump() {
                None => return false,
                Some('}') => return true,
                Some('\\') => {
                    self.bump();
                }
                Some(_) => {}
            }
        }
    }
}

/// Appends `text` to `nodes`, joining it to a trailing text node.
fn push_text(nodes: &mut Vec<Node>, text: &str) {
    if let Some(Node::Text(last)) = nodes.last_mut() {
        last.push_str(text);
    } else {
        nodes.push(Node::Text(text.to_string()));
    }
}

fn push_node(nodes: &mut Vec<Node>, node: Node) {
    match node {
        Node::Text(text) => push_text(nodes, &text),
        node => nodes.push(node),
    }
}

/// How inserted lines are indented: `line` is the leading whitespace of the line the
/// snippet starts on, `unit` one indentation level ("    " or "\t").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnippetIndent {
    pub line: String,
    pub unit: String,
}

/// One tab stop of an expanded snippet: every range where its index occurs, in text order,
/// and the options of a choice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stop {
    pub index: u32,
    pub ranges: Vec<Range<usize>>,
    pub choices: Vec<String>,
}

/// The text a snippet inserts. Offsets are bytes in `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expansion {
    pub text: String,
    pub stops: Vec<Stop>,
    pub final_offset: usize,
}

/// Expands `body` into the text to insert and its tab stops.
///
/// Every occurrence of a tab stop index gets the text of the first occurrence with a
/// default (a placeholder's content or a choice's first option), else none; its nested
/// stops are stops only inside that first occurrence. A known variable inserts its value
/// from `variables`, or its default when the value is missing or empty. An unknown variable
/// becomes a placeholder showing its default, else its name, numbered after the highest
/// index in the snippet in order of appearance (repeats share the number). `$0` is not a
/// stop: it sets `final_offset`, which is otherwise the end of the text. Each `\n` inserted
/// is followed by `indent.line` and each `\t` becomes `indent.unit`.
pub fn expand(
    body: &str,
    variables: &HashMap<String, String>,
    indent: &SnippetIndent,
) -> Expansion {
    let mut nodes = parse(body);
    let mut last_index = highest_index(&nodes);
    name_unknown_variables(&mut nodes, &mut last_index, &mut HashMap::new());
    let mut defaults = HashMap::new();
    find_defaults(&nodes, &mut defaults);

    let mut renderer = Renderer {
        variables,
        indent,
        defaults: &defaults,
        text: String::new(),
        ranges: BTreeMap::new(),
        final_offset: None,
        expanding: Vec::new(),
        depth: 0,
    };
    renderer.render(&nodes, true);

    let stops = renderer
        .ranges
        .into_iter()
        .map(|(index, mut ranges)| {
            ranges.sort_by_key(|range| (range.start, range.end));
            let choices = match defaults.get(&index) {
                Some(Node::Choice(_, options)) => options
                    .iter()
                    .map(|option| indented(option, indent))
                    .collect(),
                _ => Vec::new(),
            };
            Stop {
                index,
                ranges,
                choices,
            }
        })
        .collect();
    let final_offset = renderer.final_offset.unwrap_or(renderer.text.len());
    Expansion {
        text: renderer.text,
        stops,
        final_offset,
    }
}

fn is_known_variable(name: &str) -> bool {
    KNOWN_VARIABLES.contains(&name)
}

/// The highest tab stop index anywhere in `nodes`, 0 when there is none.
fn highest_index(nodes: &[Node]) -> u32 {
    nodes
        .iter()
        .map(|node| match node {
            Node::Text(_) => 0,
            Node::TabStop(index) | Node::Choice(index, _) => *index,
            Node::Placeholder(index, children) => (*index).max(highest_index(children)),
            Node::Variable(_, default) => default.as_deref().map_or(0, highest_index),
        })
        .max()
        .unwrap_or(0)
}

/// Turns each unknown variable into a placeholder numbered after `last_index`, in order of
/// appearance; a repeated name reuses its number.
fn name_unknown_variables(
    nodes: &mut [Node],
    last_index: &mut u32,
    numbers: &mut HashMap<String, u32>,
) {
    for node in nodes.iter_mut() {
        match node {
            Node::Variable(name, default) if !is_known_variable(name) => {
                let index = *numbers.entry(name.clone()).or_insert_with(|| {
                    *last_index = last_index.saturating_add(1);
                    *last_index
                });
                let mut children = match default.take() {
                    Some(children) if !children.is_empty() => children,
                    _ => vec![Node::Text(std::mem::take(name))],
                };
                name_unknown_variables(&mut children, last_index, numbers);
                *node = Node::Placeholder(index, children);
            }
            Node::Placeholder(_, children) | Node::Variable(_, Some(children)) => {
                name_unknown_variables(children, last_index, numbers);
            }
            _ => {}
        }
    }
}

/// For each index, the first node in depth-first order that gives it a default: a
/// placeholder with content or a choice.
fn find_defaults<'a>(nodes: &'a [Node], defaults: &mut HashMap<u32, &'a Node>) {
    for node in nodes {
        match node {
            Node::Placeholder(index, children) => {
                if !children.is_empty() {
                    defaults.entry(*index).or_insert(node);
                }
                find_defaults(children, defaults);
            }
            Node::Choice(index, _) => {
                defaults.entry(*index).or_insert(node);
            }
            Node::Variable(_, Some(children)) => find_defaults(children, defaults),
            _ => {}
        }
    }
}

/// `text` as inserted: each `\n` followed by the line indent, each `\t` as one unit.
fn indented(text: &str, indent: &SnippetIndent) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\n' => {
                out.push('\n');
                out.push_str(&indent.line);
            }
            '\t' => out.push_str(&indent.unit),
            c => out.push(c),
        }
    }
    out
}

struct Renderer<'a> {
    variables: &'a HashMap<String, String>,
    indent: &'a SnippetIndent,
    defaults: &'a HashMap<u32, &'a Node>,
    text: String,
    ranges: BTreeMap<u32, Vec<Range<usize>>>,
    final_offset: Option<usize>,
    /// The indices whose default text is being rendered, innermost last. An index inside
    /// its own default renders empty.
    expanding: Vec<u32>,
    /// How many `render` calls are open.
    depth: usize,
}

impl<'a> Renderer<'a> {
    /// Appends `nodes`; `record` says whether their stops and `$0` are recorded.
    fn render(&mut self, nodes: &'a [Node], record: bool) {
        if self.depth > MAX_NESTING {
            return;
        }
        self.depth += 1;
        for node in nodes {
            match node {
                Node::Text(text) => self.push(text),
                Node::Variable(name, default) => {
                    match self.variables.get(name).filter(|value| !value.is_empty()) {
                        Some(value) => self.push(value),
                        None => {
                            if let Some(children) = default {
                                self.render(children, record);
                            }
                        }
                    }
                }
                Node::TabStop(index) | Node::Placeholder(index, _) | Node::Choice(index, _) => {
                    self.occurrence(*index, node, record);
                }
            }
        }
        self.depth -= 1;
    }

    /// Appends an occurrence of `index`: the text of its default, recording the default's
    /// nested stops only when `node` is the occurrence that defines it.
    fn occurrence(&mut self, index: u32, node: &'a Node, record: bool) {
        let start = self.text.len();
        let expandable = !self.expanding.contains(&index);
        if let Some(&source) = self.defaults.get(&index).filter(|_| expandable) {
            self.expanding.push(index);
            match source {
                Node::Placeholder(_, children) => {
                    self.render(children, record && std::ptr::eq(source, node));
                }
                Node::Choice(_, options) => {
                    if let Some(first) = options.first() {
                        self.push(first);
                    }
                }
                _ => {}
            }
            self.expanding.pop();
        }
        if !record {
            return;
        }
        if index == 0 {
            self.final_offset.get_or_insert(start);
        } else {
            self.ranges
                .entry(index)
                .or_default()
                .push(start..self.text.len());
        }
    }

    fn push(&mut self, text: &str) {
        self.text.push_str(&indented(text, self.indent));
    }
}

#[cfg(test)]
// A stop found once has a list of one range, so `vec![4..8]` is the intended value.
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    fn text(value: &str) -> Node {
        Node::Text(value.to_string())
    }

    fn tabs() -> SnippetIndent {
        SnippetIndent {
            line: String::new(),
            unit: "\t".to_string(),
        }
    }

    fn expand_with(body: &str, variables: &[(&str, &str)]) -> Expansion {
        let variables = variables
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        expand(body, &variables, &tabs())
    }

    fn expand_plain(body: &str) -> Expansion {
        expand_with(body, &[])
    }

    /// Each stop as its index and the text of each of its ranges.
    fn stop_texts(expansion: &Expansion) -> Vec<(u32, Vec<&str>)> {
        expansion
            .stops
            .iter()
            .map(|stop| {
                let texts = stop
                    .ranges
                    .iter()
                    .map(|range| &expansion.text[range.clone()])
                    .collect();
                (stop.index, texts)
            })
            .collect()
    }

    #[test]
    fn plain_text_has_no_stops_and_ends_at_the_end() {
        assert_eq!(parse("let x = 1;"), vec![text("let x = 1;")]);
        let expansion = expand_plain("let x = 1;");
        assert_eq!(expansion.text, "let x = 1;");
        assert!(expansion.stops.is_empty());
        assert_eq!(expansion.final_offset, 10);
    }

    #[test]
    fn simple_and_braced_tab_stops() {
        assert_eq!(
            parse("f($1, ${2})"),
            vec![
                text("f("),
                Node::TabStop(1),
                text(", "),
                Node::TabStop(2),
                text(")"),
            ]
        );
        let expansion = expand_plain("f($1, ${2})");
        assert_eq!(expansion.text, "f(, )");
        assert_eq!(expansion.stops[0].ranges, vec![2..2]);
        assert_eq!(expansion.stops[1].ranges, vec![4..4]);
        assert_eq!(
            expansion.stops.iter().map(|s| s.index).collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn a_tab_stop_number_ends_at_the_first_non_digit() {
        assert_eq!(parse("$1abc"), vec![Node::TabStop(1), text("abc")]);
    }

    #[test]
    fn placeholder_covers_its_default_text() {
        let expansion = expand_plain("let ${1:name} = 1;");
        assert_eq!(expansion.text, "let name = 1;");
        assert_eq!(
            expansion.stops,
            vec![Stop {
                index: 1,
                ranges: vec![4..8],
                choices: Vec::new(),
            }]
        );
    }

    #[test]
    fn final_tab_stop_sets_the_final_offset_and_is_not_a_stop() {
        let expansion = expand_plain("{$0}");
        assert_eq!(expansion.text, "{}");
        assert_eq!(expansion.final_offset, 1);
        assert!(expansion.stops.is_empty());

        let expansion = expand_plain("f($1)");
        assert_eq!(expansion.final_offset, 3);
    }

    #[test]
    fn stops_are_sorted_by_index() {
        let expansion = expand_plain("$3 $1 $2");
        let indices: Vec<u32> = expansion.stops.iter().map(|s| s.index).collect();
        assert_eq!(indices, vec![1, 2, 3]);
    }

    #[test]
    fn nested_placeholders_are_their_own_stops_inside_the_outer_range() {
        assert_eq!(
            parse("${1:foo ${2:bar}}"),
            vec![Node::Placeholder(
                1,
                vec![text("foo "), Node::Placeholder(2, vec![text("bar")])]
            )]
        );
        let expansion = expand_plain("${1:foo ${2:bar}}");
        assert_eq!(expansion.text, "foo bar");
        assert_eq!(expansion.stops[0].ranges, vec![0..7]);
        assert_eq!(expansion.stops[1].ranges, vec![4..7]);
    }

    #[test]
    fn a_repeated_index_is_one_stop_with_a_range_per_occurrence() {
        let expansion = expand_plain("for ${1:i} in 0..n { $1 }");
        assert_eq!(expansion.text, "for i in 0..n { i }");
        assert_eq!(stop_texts(&expansion), vec![(1, vec!["i", "i"])]);
        assert_eq!(expansion.stops[0].ranges, vec![4..5, 16..17]);
    }

    #[test]
    fn a_tab_stop_before_its_placeholder_takes_the_placeholder_text() {
        let expansion = expand_plain("$1 ${1:x}");
        assert_eq!(expansion.text, "x x");
        assert_eq!(expansion.stops[0].ranges, vec![0..1, 2..3]);
    }

    #[test]
    fn the_first_default_wins_over_later_ones() {
        let expansion = expand_plain("${1:a} ${1:b}");
        assert_eq!(expansion.text, "a a");
    }

    #[test]
    fn a_mirror_of_a_nested_placeholder_copies_its_text_without_nested_stops() {
        let expansion = expand_plain("${1:foo ${2:bar}} $1");
        assert_eq!(expansion.text, "foo bar foo bar");
        assert_eq!(
            stop_texts(&expansion),
            vec![(1, vec!["foo bar", "foo bar"]), (2, vec!["bar"])]
        );
    }

    #[test]
    fn a_placeholder_containing_its_own_index_terminates() {
        let expansion = expand_plain("${1:a $1} $1");
        assert_eq!(expansion.text, "a  a ");
    }

    #[test]
    fn choice_inserts_its_first_option_and_records_all() {
        assert_eq!(
            parse("${1|a,b|}"),
            vec![Node::Choice(1, vec!["a".to_string(), "b".to_string()])]
        );
        let expansion = expand_plain("type=${1|a,b|};");
        assert_eq!(expansion.text, "type=a;");
        assert_eq!(
            expansion.stops,
            vec![Stop {
                index: 1,
                ranges: vec![5..6],
                choices: vec!["a".to_string(), "b".to_string()],
            }]
        );
    }

    #[test]
    fn a_tab_stop_mirrors_a_choice() {
        let expansion = expand_plain("${1|post,get|} $1");
        assert_eq!(expansion.text, "post post");
        assert_eq!(expansion.stops[0].choices, vec!["post", "get"]);
    }

    #[test]
    fn choice_options_honour_escaped_commas_and_pipes() {
        assert_eq!(
            parse(r"${1|a\,b,c\|d,e\\f|}"),
            vec![Node::Choice(
                1,
                vec!["a,b".to_string(), "c|d".to_string(), r"e\f".to_string()]
            )]
        );
    }

    #[test]
    fn known_variable_inserts_its_value() {
        assert_eq!(
            parse("$TM_FILENAME ${TM_FILENAME}"),
            vec![
                Node::Variable("TM_FILENAME".to_string(), None),
                text(" "),
                Node::Variable("TM_FILENAME".to_string(), None),
            ]
        );
        let expansion = expand_with("$TM_FILENAME ${TM_FILENAME}", &[("TM_FILENAME", "a.rs")]);
        assert_eq!(expansion.text, "a.rs a.rs");
        assert!(expansion.stops.is_empty());
    }

    #[test]
    fn empty_or_missing_known_variable_uses_its_default_or_nothing() {
        let expansion = expand_with(
            "[${TM_SELECTED_TEXT:none}][${CLIPBOARD:clip}][$UUID]",
            &[("TM_SELECTED_TEXT", "")],
        );
        assert_eq!(expansion.text, "[none][clip][]");
    }

    #[test]
    fn a_known_variable_default_may_hold_stops() {
        let expansion = expand_plain("${TM_SELECTED_TEXT:${1:body}}");
        assert_eq!(expansion.text, "body");
        assert_eq!(stop_texts(&expansion), vec![(1, vec!["body"])]);

        let expansion = expand_with(
            "${TM_SELECTED_TEXT:${1:body}}",
            &[("TM_SELECTED_TEXT", "x")],
        );
        assert_eq!(expansion.text, "x");
        assert!(expansion.stops.is_empty());
    }

    #[test]
    fn unknown_variables_become_placeholders_after_the_highest_index() {
        let expansion = expand_with("${1:a} ${foo} $3 $bar $foo", &[("foo", "ignored")]);
        assert_eq!(expansion.text, "a foo  bar foo");
        assert_eq!(
            stop_texts(&expansion),
            vec![
                (1, vec!["a"]),
                (3, vec![""]),
                (4, vec!["foo", "foo"]),
                (5, vec!["bar"]),
            ]
        );
    }

    #[test]
    fn unknown_variable_in_a_snippet_without_stops_gets_index_one() {
        let expansion = expand_plain("let $name = 1;$0");
        assert_eq!(expansion.text, "let name = 1;");
        assert_eq!(stop_texts(&expansion), vec![(1, vec!["name"])]);
        assert_eq!(expansion.final_offset, 13);
    }

    #[test]
    fn unknown_variable_with_a_default_shows_the_default() {
        let expansion = expand_plain("${1:a} ${foo:bar}");
        assert_eq!(expansion.text, "a bar");
        assert_eq!(
            stop_texts(&expansion),
            vec![(1, vec!["a"]), (2, vec!["bar"])]
        );
    }

    #[test]
    fn placeholder_default_may_be_a_variable() {
        assert_eq!(
            parse("${1:${TM_FILENAME_BASE}}"),
            vec![Node::Placeholder(
                1,
                vec![Node::Variable("TM_FILENAME_BASE".to_string(), None)]
            )]
        );
        let expansion = expand_with(
            "class ${1:${TM_FILENAME_BASE}} {}",
            &[("TM_FILENAME_BASE", "Main")],
        );
        assert_eq!(expansion.text, "class Main {}");
        assert_eq!(expansion.stops[0].ranges, vec![6..10]);
    }

    #[test]
    fn escapes_are_literal_text() {
        assert_eq!(parse(r"\$1 \} \\ \x"), vec![text(r"$1 } \ \x")]);
        let expansion = expand_plain(r"\$1 \}");
        assert_eq!(expansion.text, "$1 }");
        assert!(expansion.stops.is_empty());

        let expansion = expand_plain(r"${1:a\}b}");
        assert_eq!(expansion.text, "a}b");
        assert_eq!(expansion.stops[0].ranges, vec![0..3]);
    }

    #[test]
    fn a_closing_brace_outside_a_placeholder_is_text() {
        assert_eq!(expand_plain("{ $1 }").text, "{  }");
    }

    #[test]
    fn unclosed_placeholder_is_literal_text() {
        assert_eq!(parse("${1:foo"), vec![text("${1:foo")]);
        let expansion = expand_plain("${1:foo");
        assert_eq!(expansion.text, "${1:foo");
        assert!(expansion.stops.is_empty());
    }

    #[test]
    fn unclosed_placeholder_keeps_the_stops_parsed_inside_it() {
        assert_eq!(
            parse("${1:foo $2"),
            vec![text("${1:foo "), Node::TabStop(2)]
        );
        assert_eq!(parse("${name:foo"), vec![text("${name:foo")]);
    }

    #[test]
    fn other_malformed_constructs_are_literal_text() {
        for body in [
            "$",
            "${",
            "${}",
            "${1",
            "${1|a,b",
            "${1|a,b|",
            "${1|a,b|x}",
            "${1||}",
            "${0|a,b|}",
            "${1/a/b",
            "${1/a/b/",
            "${name",
            "${-1}",
            "$ 1",
        ] {
            assert_eq!(parse(body), vec![text(body)], "{body:?}");
            let expansion = expand_plain(body);
            assert_eq!(expansion.text, body, "{body:?}");
            assert!(expansion.stops.is_empty(), "{body:?}");
        }
    }

    #[test]
    fn transforms_are_parsed_and_ignored() {
        let body = r"${1/(.*)/${1:/upcase}/g} ${TM_FILENAME/(.*)\.rs/$1\/x/} ${2/a\/b/c/}";
        assert_eq!(
            parse(body),
            vec![
                Node::TabStop(1),
                text(" "),
                Node::Variable("TM_FILENAME".to_string(), None),
                text(" "),
                Node::TabStop(2),
            ]
        );
        let expansion = expand_with(body, &[("TM_FILENAME", "main.rs")]);
        assert_eq!(expansion.text, " main.rs ");
        assert_eq!(stop_texts(&expansion), vec![(1, vec![""]), (2, vec![""])]);
    }

    #[test]
    fn a_transform_format_may_hold_braces_and_slashes_in_a_group() {
        assert_eq!(
            parse("${1/(a)/${1:?yes/no:n\\}o}/}x"),
            vec![Node::TabStop(1), text("x")]
        );
    }

    #[test]
    fn new_lines_take_the_line_indent_and_tabs_the_unit() {
        let indent = SnippetIndent {
            line: "    ".to_string(),
            unit: "  ".to_string(),
        };
        let expansion = expand("fn ${1:name}() {\n\t$0\n}", &HashMap::new(), &indent);
        assert_eq!(expansion.text, "fn name() {\n      \n    }");
        assert_eq!(expansion.stops[0].ranges, vec![3..7]);
        assert_eq!(expansion.final_offset, "fn name() {\n      ".len());
    }

    #[test]
    fn variable_values_are_indented_too() {
        let indent = SnippetIndent {
            line: "  ".to_string(),
            unit: "    ".to_string(),
        };
        let variables = HashMap::from([("TM_SELECTED_TEXT".to_string(), "a\n\tb".to_string())]);
        let expansion = expand("{\n\t$TM_SELECTED_TEXT\n}", &variables, &indent);
        assert_eq!(expansion.text, "{\n      a\n      b\n  }");
    }

    #[test]
    fn multibyte_text_keeps_ranges_on_char_boundaries() {
        let expansion = expand_plain("héllo ${1:wörld} ñ$2 ${3|ä,ö|}$0");
        assert_eq!(expansion.text, "héllo wörld ñ ä");
        assert_eq!(
            stop_texts(&expansion),
            vec![(1, vec!["wörld"]), (2, vec![""]), (3, vec!["ä"])]
        );
        for stop in &expansion.stops {
            for range in &stop.ranges {
                assert!(expansion.text.is_char_boundary(range.start));
                assert!(expansion.text.is_char_boundary(range.end));
            }
        }
        assert_eq!(expansion.final_offset, expansion.text.len());
    }

    #[test]
    fn deep_nesting_does_not_overflow_the_stack() {
        let depth = 50_000;
        let openings: String = (1..=depth).map(|index| format!("${{{index}:")).collect();
        let body = format!("{openings}x{}", "}".repeat(depth));
        let expansion = expand_plain(&body);
        assert!(expansion.text.contains('x'));
        assert!(!expansion.stops.is_empty());
    }

    #[test]
    fn a_long_chain_of_mirrors_does_not_overflow_the_stack() {
        // Each default mirrors the next stop through a known variable's default.
        let count = 20_000;
        let body: String = (1..count)
            .map(|index| format!("${{{index}:${{UUID:${}}}}} ", index + 1))
            .chain([format!("${{{count}:x}}")])
            .collect();
        let expansion = expand_plain(&body);
        assert!(expansion.text.ends_with('x'));
        assert_eq!(expansion.stops.len(), count);
    }
}
