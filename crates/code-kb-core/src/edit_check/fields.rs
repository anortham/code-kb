//! Which constructions of a type break when its fields change. A construction breaks only where
//! the language makes it name every field: a Rust struct literal, an unkeyed Go literal, a call
//! that builds a Python dataclass or a Java record. A constructor of any other class is a
//! parameter change of its own.

use super::{MAX_EXTENT_LINES, word_position};
use crate::models::Symbol;
use crate::reference_report::arity;
use crate::reference_report::variant_names::{code_only, is_identifier_char};

/// Words after which a name stands in a type position and builds nothing.
const TYPE_POSITION_WORDS: [&str; 8] = [
    "impl", "for", "struct", "enum", "union", "trait", "type", "dyn",
];

/// The fields a type gained and lost.
pub(super) struct FieldChange {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// An added field has no default value. Only a Python dataclass reads it.
    pub required: bool,
    /// The number of components a Java record had before the change, when its signature says.
    pub old_components: Option<usize>,
}

enum Owner {
    RustStruct,
    GoStruct,
    Dataclass,
    JavaRecord,
}

fn owner_of(subject: &Symbol) -> Option<Owner> {
    let signature = subject.signature.as_deref().unwrap_or_default();
    match (subject.language.as_str(), subject.kind.as_str()) {
        ("rust", "struct") => Some(Owner::RustStruct),
        ("go", "struct") => Some(Owner::GoStruct),
        ("python", "class") if signature.starts_with("@dataclass") => Some(Owner::Dataclass),
        ("java", "class" | "record")
            if word_position(signature, &subject.name)
                .is_some_and(|at| word_position(&signature[..at], "record").is_some()) =>
        {
            Some(Owner::JavaRecord)
        }
        _ => None,
    }
}

/// Whether a class-body variable with this signature is a field of a Python dataclass: it has an
/// annotation, and the annotation is no `ClassVar`.
pub(super) fn is_dataclass_field(signature: &str) -> bool {
    let head = signature.split('=').next().unwrap_or_default();
    head.split_once(':')
        .is_some_and(|(_, annotation)| !is_class_var(annotation))
}

fn is_class_var(annotation: &str) -> bool {
    let annotation = annotation.trim().trim_matches(['"', '\'']).trim();
    let name = annotation.split('[').next().unwrap_or_default().trim();
    name.rsplit('.').next() == Some("ClassVar")
}

/// Whether a dataclass field with this signature has to be passed to the constructor: it has no
/// default value, or its `field(…)` call passes neither `default` nor `default_factory` and does
/// not keep the field out of `__init__` with `init=False`.
pub(super) fn needs_argument(signature: Option<&str>) -> bool {
    let Some(signature) = signature else {
        return true;
    };
    let code = code_only(signature, "python");
    let Some(equals) = arity::default_start(&code) else {
        return true;
    };
    let Some(arguments) = field_call_arguments(code[equals + 1..].trim()) else {
        return false;
    };
    let named = keyword_arguments(arguments);
    let passes = |name: &str| named.iter().any(|(key, _)| key == name);
    let leaves_init = named
        .iter()
        .any(|(key, value)| key == "init" && value == "False");
    !(passes("default") || passes("default_factory") || leaves_init)
}

/// The text inside the parentheses of a call to `field` (`field(…)`, `dataclasses.field(…)`).
fn field_call_arguments(value: &str) -> Option<&str> {
    let open = value.find('(')?;
    if value[..open].trim().rsplit('.').next() != Some("field") {
        return None;
    }
    let group = &value[open..];
    Some(match after_balanced(group, '(', ')') {
        Some(rest) => &group[1..group.len() - rest.len() - 1],
        None => &group[1..],
    })
}

fn keyword_arguments(arguments: &str) -> Vec<(String, String)> {
    split_body(arguments)
        .0
        .iter()
        .filter_map(|piece| {
            let (key, value) = piece.split_once('=')?;
            let key = key.trim();
            (!value.starts_with('=') && key.chars().all(is_identifier_char))
                .then(|| (key.to_string(), value.trim().to_string()))
        })
        .collect()
}

impl FieldChange {
    /// Whether the construction that starts at `line` of `lines` (`None` when the file cannot
    /// be read) of the type `subject` stops compiling or running after this change.
    pub(super) fn breaks(&self, subject: &Symbol, lines: Option<&[String]>, line: usize) -> bool {
        match owner_of(subject) {
            None => false,
            Some(Owner::JavaRecord) => {
                let Some(lines) = lines else {
                    return true;
                };
                let callee = if calls_this(lines, line) {
                    "this"
                } else {
                    subject.name.as_str()
                };
                let counts = call_counts(lines, line, callee);
                self.old_components
                    .is_none_or(|old| counts.is_empty() || counts.contains(&old))
            }
            Some(Owner::Dataclass) => !self.removed.is_empty() || self.required,
            Some(owner) => {
                let go = matches!(owner, Owner::GoStruct);
                let Some(lines) = lines else {
                    return true;
                };
                literals(lines, line, &subject.name, go)
                    .iter()
                    .any(|literal| self.breaks_literal(literal, go))
            }
        }
    }

    /// Whether a `Self { … }` literal or pattern that opens on `line` of `lines`, inside an impl
    /// of a Rust struct, stops compiling after this change.
    pub(super) fn breaks_self(&self, lines: &[String], line: usize) -> bool {
        literals(lines, line, "Self", false)
            .iter()
            .any(|literal| self.breaks_literal(literal, false))
    }

    fn breaks_literal(&self, literal: &Literal, go: bool) -> bool {
        if literal.unclosed {
            return true;
        }
        let elements = &literal.elements;
        let names_removed = elements
            .iter()
            .any(|e| e.key.as_ref().is_some_and(|key| self.removed.contains(key)));
        if go {
            let unkeyed = elements.iter().any(|e| e.key.is_none());
            let all_unkeyed = !elements.is_empty() && elements.iter().all(|e| e.key.is_none());
            (!self.added.is_empty() && all_unkeyed)
                || (!self.removed.is_empty() && (unkeyed || names_removed))
        } else {
            let fills_from_base = elements.iter().any(|e| e.base);
            (!self.added.is_empty() && !fills_from_base)
                || (!self.removed.is_empty() && names_removed)
        }
    }
}

/// Whether the call on `line` is `this(…)`, a record constructor that hands its arguments to
/// another one.
fn calls_this(lines: &[String], line: usize) -> bool {
    line.checked_sub(1)
        .and_then(|index| lines.get(index))
        .and_then(|text| text.trim_start().strip_prefix("this"))
        .is_some_and(|rest| rest.trim_start().starts_with('('))
}

/// The component count in the signature of a Java record.
pub(super) fn record_components(signature: &str, name: &str) -> Option<usize> {
    arity::parameter_segments(signature, name, "java").map(|(_, segments)| segments.len())
}

/// One literal: the field it names and whether it fills the rest from a base value (`..base`).
struct Literal {
    elements: Vec<Element>,
    unclosed: bool,
}

struct Element {
    key: Option<String>,
    base: bool,
}

/// The literals of `name` that open on `line` of `lines`. A Go slice, array, or map literal gives
/// one per element written as `{…}` (the type is left out there).
fn literals(lines: &[String], line: usize, name: &str, go: bool) -> Vec<Literal> {
    let Some(first) = line.checked_sub(1).and_then(|index| lines.get(index)) else {
        return Vec::new();
    };
    let code = code_from(lines, line, if go { "go" } else { "rust" });
    let first_line = &code[..first.len().min(code.len())];
    word_starts(first_line, name)
        .into_iter()
        .flat_map(|at| literals_at(&code, at, name, go))
        .collect()
}

/// Up to `MAX_EXTENT_LINES` lines of `lines` from `line` on.
fn source_from(lines: &[String], line: usize) -> String {
    let end = (line - 1 + MAX_EXTENT_LINES).min(lines.len());
    lines[line - 1..end].join("\n")
}

/// The code of [`source_from`], with comments and strings blanked.
fn code_from(lines: &[String], line: usize, language: &str) -> String {
    code_only(&source_from(lines, line), language)
}

/// The argument counts of the calls of `name` that open on `line` of `lines`.
pub(super) fn call_counts(lines: &[String], line: usize, name: &str) -> Vec<usize> {
    let Some(first) = line.checked_sub(1).and_then(|index| lines.get(index)) else {
        return Vec::new();
    };
    let text = source_from(lines, line);
    let code = code_only(&text, "java");
    let first_line = &code[..first.len().min(code.len())];
    word_starts(first_line, name)
        .into_iter()
        .filter_map(|at| {
            let mut group = code[at + name.len()..].trim_start();
            if group.starts_with('<') {
                group = after_balanced(group, '<', '>')?.trim_start();
            }
            if !group.starts_with('(') {
                return None;
            }
            let inner = match after_balanced(group, '(', ')') {
                Some(after) => &group[1..group.len() - after.len() - 1],
                None => &group[1..],
            };
            let from = code.len() - group.len() + 1;
            Some(argument_count(inner, &text[from..from + inner.len()]))
        })
        .collect()
}

/// The number of arguments in `inner`, the code between the parentheses of a call. `written` is
/// the same text before strings were blanked, so a string argument still counts.
fn argument_count(inner: &str, written: &str) -> usize {
    let mut depth = 0usize;
    let mut start = 0;
    let mut count = 0;
    for (at, c) in inner.char_indices().chain([(inner.len(), ',')]) {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                count += usize::from(!written[start..at].trim().is_empty());
                start = at + 1;
            }
            _ => {}
        }
    }
    count
}

fn word_starts(text: &str, word: &str) -> Vec<usize> {
    text.match_indices(word)
        .map(|(at, _)| at)
        .filter(|&at| {
            !text[..at].ends_with(is_identifier_char)
                && !text[at + word.len()..].starts_with(is_identifier_char)
        })
        .collect()
}

fn literals_at(code: &str, at: usize, name: &str, go: bool) -> Vec<Literal> {
    let before = code[..at].trim_end();
    let last_word = before
        .rsplit(|c: char| !is_identifier_char(c))
        .next()
        .unwrap_or_default();
    if TYPE_POSITION_WORDS.contains(&last_word)
        || before.ends_with("->")
        || (go && before.ends_with(')'))
    {
        return Vec::new();
    }
    let mut rest = code[at + name.len()..].trim_start();
    let (open, close) = if go { ('[', ']') } else { ('<', '>') };
    let arguments = if go {
        rest
    } else {
        rest.strip_prefix("::").unwrap_or(rest)
    };
    if arguments.starts_with(open) {
        let Some(after) = after_balanced(arguments, open, close) else {
            return Vec::new();
        };
        rest = after.trim_start();
    }
    let Some(body) = rest.strip_prefix('{') else {
        return Vec::new();
    };
    let (pieces, unclosed) = split_body(body);
    let element_type = before.strip_suffix('.').map_or(before, |qualified| {
        qualified.trim_end_matches(is_identifier_char)
    });
    let collection = go && element_type.trim_end_matches('*').ends_with(']');
    if !collection {
        return vec![literal_of(&pieces, unclosed, go)];
    }
    pieces
        .iter()
        .filter_map(|piece| {
            let value = top_level_colon(piece).map_or(piece.as_str(), |colon| &piece[colon + 1..]);
            let (inner, unclosed) = split_body(value.trim().strip_prefix('{')?);
            Some(literal_of(&inner, unclosed, go))
        })
        .collect()
}

fn literal_of(pieces: &[String], unclosed: bool, go: bool) -> Literal {
    Literal {
        elements: pieces.iter().map(|piece| element_of(piece, go)).collect(),
        unclosed,
    }
}

fn element_of(piece: &str, go: bool) -> Element {
    if !go && piece.starts_with("..") {
        return Element {
            key: None,
            base: true,
        };
    }
    let key = match top_level_colon(piece) {
        Some(colon) => Some(piece[..colon].trim().to_string()),
        None if !go && piece.chars().all(is_identifier_char) => Some(piece.to_string()),
        None => None,
    };
    Element { key, base: false }
}

/// The trimmed pieces of the text that follows an opening `{`, cut at the commas outside every
/// bracket, up to the `}` that closes it; empty pieces are dropped. The flag is set when no `}`
/// closes it within the text.
pub(super) fn split_body(body: &str) -> (Vec<String>, bool) {
    let mut depth = 0usize;
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut closed = false;
    let mut end = body.len();
    for (at, c) in body.char_indices() {
        match c {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' if depth > 0 => depth -= 1,
            '}' => {
                closed = true;
                end = at;
                break;
            }
            ',' if depth == 0 => {
                pieces.push(body[start..at].trim().to_string());
                start = at + 1;
            }
            _ => {}
        }
    }
    pieces.push(body[start..end].trim().to_string());
    pieces.retain(|piece| !piece.is_empty());
    (pieces, !closed)
}

/// The byte of the first `:` outside every bracket that is no part of `::`.
fn top_level_colon(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut previous = ' ';
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        match c {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => depth = depth.saturating_sub(1),
            ':' if depth == 0
                && previous != ':'
                && chars.peek().is_none_or(|&(_, next)| next != ':') =>
            {
                return Some(at);
            }
            _ => {}
        }
        previous = c;
    }
    None
}

/// The text after the group that opens `text` with `open` and closes with the matching `close`.
pub(super) fn after_balanced(text: &str, open: char, close: char) -> Option<&str> {
    let mut depth = 0usize;
    for (at, c) in text.char_indices() {
        if c == open {
            depth += 1;
        } else if c == close {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(&text[at + c.len_utf8()..]);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    fn change(added: &[&str], removed: &[&str]) -> FieldChange {
        let names = |names: &[&str]| names.iter().map(|name| name.to_string()).collect();
        FieldChange {
            added: names(added),
            removed: names(removed),
            required: false,
            old_components: None,
        }
    }

    fn breaks(change: &FieldChange, go: bool, source: &str) -> bool {
        let lines = lines(source);
        literals(&lines, 1, "Foo", go)
            .iter()
            .any(|literal| change.breaks_literal(literal, go))
    }

    #[test]
    fn a_rust_literal_breaks_for_an_added_field_unless_it_fills_from_a_base() {
        let added = change(&["c"], &[]);
        assert!(breaks(&added, false, "let x = Foo { a: 1, b };"));
        assert!(!breaks(&added, false, "let x = Foo { a: 1, ..base };"));
        assert!(!breaks(
            &added,
            false,
            "let x = Foo { a: 1, ..Default::default() };"
        ));
        assert!(breaks(&added, false, "let x = Foo::<u8> { a: 1 };"));
    }

    #[test]
    fn a_rust_literal_breaks_for_a_removed_field_only_when_it_names_it() {
        let removed = change(&[], &["b"]);
        assert!(breaks(&removed, false, "let x = Foo { a: 1, b: 2 };"));
        assert!(breaks(&removed, false, "let x = Foo { a: 1, b };"));
        assert!(breaks(
            &removed,
            false,
            "let x = Foo { a: 1, b: 2, ..base };"
        ));
        assert!(!breaks(&removed, false, "let x = Foo { a: 1, ..base };"));
        assert!(!breaks(&removed, false, "let x = Foo { a: b };"));
    }

    #[test]
    fn a_name_in_a_type_position_or_a_string_opens_no_literal() {
        let added = change(&["c"], &[]);
        assert!(!breaks(&added, false, "pub fn f() -> Foo {"));
        assert!(!breaks(&added, false, "impl Foo {"));
        assert!(!breaks(&added, false, "let s = \"Foo { a: 1 }\";"));
        assert!(!breaks(&added, false, "let x = Foo::new(1);"));
        assert!(!breaks(&added, true, "func (f *Foo) Do() Foo {"));
    }

    #[test]
    fn a_go_literal_breaks_for_an_added_field_only_when_it_is_unkeyed() {
        let added = change(&["Z"], &[]);
        assert!(breaks(&added, true, "return Foo{1, 2}"));
        assert!(breaks(&added, true, "return &Foo{f(a, b), x[1:2]}"));
        assert!(!breaks(&added, true, "return Foo{X: 1, Y: 2}"));
        assert!(!breaks(&added, true, "return Foo{}"));
    }

    #[test]
    fn a_go_literal_breaks_for_a_removed_field_when_it_is_unkeyed_or_keys_it() {
        let removed = change(&[], &["Y"]);
        assert!(breaks(&removed, true, "return Foo{1, 2}"));
        assert!(breaks(&removed, true, "return Foo{X: 1, Y: 2}"));
        assert!(!breaks(&removed, true, "return Foo{X: 1}"));
        assert!(!breaks(&removed, true, "return Foo{}"));
    }

    #[test]
    fn a_go_collection_literal_is_judged_by_its_elements() {
        let added = change(&["Z"], &[]);
        assert!(breaks(&added, true, "return []Foo{{1, 2}, {3, 4}}"));
        assert!(breaks(&added, true, "return []*Foo{{1, 2}}"));
        assert!(breaks(&added, true, "return map[string]Foo{\"a\": {1, 2}}"));
        assert!(!breaks(&added, true, "return []Foo{{X: 1}}"));
        assert!(!breaks(
            &added,
            true,
            "return map[string]Foo{\"a\": {X: 1}}"
        ));
    }

    #[test]
    fn a_go_collection_literal_of_a_package_qualified_type_is_judged_by_its_elements() {
        let added = change(&["Z"], &[]);
        assert!(breaks(&added, true, "return []geom.Foo{{1, 2}}"));
        assert!(breaks(&added, true, "return []*geom.Foo{{1, 2}}"));
        assert!(breaks(&added, true, "return [2]geom.Foo{{1, 2}}"));
        assert!(breaks(
            &added,
            true,
            "return map[string]geom.Foo{\"a\": {1, 2}}"
        ));
        assert!(!breaks(&added, true, "return []geom.Foo{{X: 1}}"));
        assert!(!breaks(&added, true, "return []*geom.Foo{{X: 1}}"));
        assert!(!breaks(
            &added,
            true,
            "return map[string]geom.Foo{\"a\": {X: 1}}"
        ));
    }

    #[test]
    fn a_literal_across_lines_is_read_to_its_closing_brace() {
        let added = change(&["c"], &[]);
        let source = "let x = Foo {\n    a: 1,\n    ..base\n};\nlet y = 1;";
        assert!(!breaks(&added, false, source));
        let source = "let x = Foo {\n    a: 1,\n    b: 2,\n};";
        assert!(breaks(&added, false, source));
    }

    #[test]
    fn a_literal_that_does_not_close_is_taken_as_breaking() {
        assert!(breaks(&change(&["c"], &[]), false, "let x = Foo { a: 1,"));
    }

    use crate::edit_check::test_symbol as symbol;

    #[test]
    fn only_a_rust_or_go_struct_a_dataclass_and_a_java_record_have_breaking_constructions() {
        let added = change(&["z"], &[]);
        let call = Some(lines("Foo(1, 2)"));
        let breaks = |symbol: &Symbol| added.breaks(symbol, call.as_deref(), 1);
        assert!(breaks(&symbol(
            "java",
            "class",
            "Foo",
            "public record Foo(int x)"
        )));
        assert!(!breaks(&symbol("java", "class", "Foo", "public class Foo")));
        assert!(!breaks(&symbol(
            "csharp",
            "class",
            "Foo",
            "public record Foo(int X)"
        )));
        assert!(!breaks(&symbol(
            "typescript",
            "class",
            "Foo",
            "export class Foo"
        )));
        assert!(!breaks(&symbol("python", "class", "Foo", "class Foo")));
        assert!(!breaks(&symbol("rust", "struct", "Foo", "pub struct Foo")));
    }

    #[test]
    fn a_dataclass_breaks_for_a_removed_field_or_an_added_field_without_a_default() {
        let dataclass = symbol("python", "class", "Foo", "@dataclass class Foo");
        let call = Some(lines("Foo(1, 2)"));
        let mut added = change(&["z"], &[]);
        assert!(!added.breaks(&dataclass, call.as_deref(), 1));
        added.required = true;
        assert!(added.breaks(&dataclass, call.as_deref(), 1));
        assert!(change(&[], &["z"]).breaks(&dataclass, call.as_deref(), 1));
    }

    #[test]
    fn a_class_variable_is_no_dataclass_field() {
        assert!(is_dataclass_field("x: int = 1"));
        assert!(is_dataclass_field(
            "x: list[int] = field(default_factory=list)"
        ));
        assert!(!is_dataclass_field("x: ClassVar[int] = 1"));
        assert!(!is_dataclass_field("x: typing.ClassVar = 1"));
        assert!(!is_dataclass_field("x: \"ClassVar[int]\""));
        assert!(!is_dataclass_field("x = 1"));
    }

    #[test]
    fn a_dataclass_field_needs_an_argument_unless_it_has_a_default_or_stays_out_of_init() {
        assert!(needs_argument(None));
        assert!(needs_argument(Some("x: int")));
        assert!(needs_argument(Some("x: int = field(repr=False)")));
        assert!(needs_argument(Some(
            "x: int = dataclasses.field(metadata={})"
        )));
        assert!(!needs_argument(Some("x: int = 1")));
        assert!(!needs_argument(Some("x: int = field(default=1)")));
        assert!(!needs_argument(Some(
            "x: list = field(default_factory=list)"
        )));
        assert!(!needs_argument(Some("x: int = field(init=False)")));
    }

    #[test]
    fn a_this_call_line_is_found_by_its_start() {
        assert!(calls_this(&lines("        this(a, b);"), 1));
        assert!(calls_this(&lines("this (a);"), 1));
        assert!(!calls_this(&lines("var p = new Point(a);"), 1));
        assert!(!calls_this(&lines("thisValue(a);"), 1));
    }

    #[test]
    fn a_this_call_breaks_a_record_only_with_the_old_component_count() {
        let record = symbol(
            "java",
            "record",
            "Point",
            "public record Point(int x, int y)",
        );
        let mut gained = change(&["z"], &[]);
        gained.old_components = Some(2);
        let breaks = |text: &str| gained.breaks(&record, Some(&lines(text)), 1);
        assert!(breaks("        this(x, 0);"));
        assert!(breaks("        this(x, f(1, 2));"));
        assert!(!breaks("        this(x, 0, 0);"));
        assert!(!breaks("        this(x);"));
    }

    #[test]
    fn the_argument_counts_of_a_call_are_read_from_its_own_parentheses() {
        let counts = |text: &str| call_counts(&lines(text), 1, "Point");
        assert_eq!(counts("var p = new Point(1, 2);"), vec![2]);
        assert_eq!(counts("var p = new Point(f(1, 2), 3);"), vec![2]);
        assert_eq!(counts("var p = new Point();"), vec![0]);
        assert_eq!(counts("var p = new Point(\"a, b\", 1);"), vec![2]);
        assert_eq!(counts("var p = new Point(\"a\");"), vec![1]);
        assert_eq!(counts("var p = new Point(1,"), vec![1]);
        assert!(counts("var p = null;").is_empty());
        assert_eq!(counts("var p = new Point<>(\"a\", 1);"), vec![2]);
        assert_eq!(
            counts("var p = new Point<String, Integer>(\"a\", 1);"),
            vec![2]
        );
        assert_eq!(counts("Point<A, B> p = new Point<>(1, 2);"), vec![2]);
    }

    #[test]
    fn a_self_literal_breaks_like_a_named_literal() {
        let added = change(&["c"], &[]);
        assert!(added.breaks_self(&lines("        Self { a: 1, b: 2 }"), 1));
        assert!(!added.breaks_self(&lines("        Self { a: 1, ..base }"), 1));
        assert!(!added.breaks_self(&lines("        let x = 1;"), 1));
    }
}
