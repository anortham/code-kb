//! Signatures compared without what cannot break a caller: parameter names, default values, a
//! binding `mut`, and in Python every annotation. A parameter list is read piece by piece and
//! matched to the parameter rows of the symbol; a piece with no row, or a row with no piece, makes
//! the whole signature compare as text.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use rusqlite::Connection;

use super::fields::{after_balanced, split_body};
use super::word_position;
use crate::models::Symbol;
use crate::queries::{QueryError, get_symbol_by_id};
use crate::reference_report::arity::{self, ParameterRow};
use crate::reference_report::variant_names::{code_only, is_identifier_char};

/// Languages where a caller can name a parameter, so a rename can break a call.
const NAMED_ARGUMENT_LANGUAGES: [&str; 4] = ["python", "kotlin", "csharp", "swift"];

/// Words of a declaration that change how a caller must call it, with the languages that give
/// them that meaning.
const CALL_CONTRACT_WORDS: [(&str, &[&str]); 3] = [
    (
        "async",
        &[
            "rust",
            "python",
            "javascript",
            "jsx",
            "typescript",
            "tsx",
            "csharp",
        ],
    ),
    ("suspend", &["kotlin"]),
    (
        "static",
        &[
            "java",
            "csharp",
            "javascript",
            "jsx",
            "typescript",
            "tsx",
            "cpp",
            "php",
            "swift",
            "dart",
        ],
    ),
];

/// How much of a declaration is read to find its modifiers.
const HEAD_BYTES: u64 = 1024;

/// Words that start a Go type and so are no result name.
const GO_TYPE_WORDS: [&str; 5] = ["chan", "func", "map", "interface", "struct"];

/// A signature as `Detector` compares it.
pub(super) struct Shape {
    raw: String,
    normal: Option<Normal>,
    contract: Vec<&'static str>,
}

struct Normal {
    tail: String,
    names: Vec<String>,
}

impl Shape {
    fn toggled(&self, other: &Shape) -> Vec<String> {
        let added = other
            .contract
            .iter()
            .filter(|word| !self.contract.contains(word))
            .map(|word| format!("{word} added"));
        let removed = self
            .contract
            .iter()
            .filter(|word| !other.contract.contains(word))
            .map(|word| format!("{word} removed"));
        added.chain(removed).collect()
    }
}

/// The words of `CALL_CONTRACT_WORDS` that the declaration of `symbol` holds before its name. The
/// signature of a TypeScript function leaves `async` out, so the words are read from the file;
/// the signature serves when the file cannot be read. A C++ `static` counts only in a class or
/// struct: at file or namespace scope it gives internal linkage, and no caller sees it.
fn contract_words(
    conn: &Connection,
    symbol: &Symbol,
    root: &Path,
) -> Result<Vec<&'static str>, QueryError> {
    let language = symbol.language.as_str();
    let declaration = without_attributes(
        &source_start(symbol, root).unwrap_or_else(|| symbol.signature.clone().unwrap_or_default()),
        language,
    );
    let head = declaration_head(&declaration, &symbol.name);
    let code = code_only(head, language);
    let static_counts = language != "cpp" || is_class_member(conn, symbol)?;
    Ok(CALL_CONTRACT_WORDS
        .iter()
        .filter(|(word, languages)| {
            languages.contains(&language)
                && (static_counts || *word != "static")
                && word_position(&code, word).is_some()
        })
        .map(|(word, _)| *word)
        .collect())
}

fn is_class_member(conn: &Connection, symbol: &Symbol) -> Result<bool, QueryError> {
    let Some(parent) = &symbol.parent_symbol_id else {
        return Ok(false);
    };
    Ok(get_symbol_by_id(conn, parent)?
        .is_some_and(|owner| matches!(owner.kind.as_str(), "class" | "struct")))
}

/// The first `HEAD_BYTES` of the declaration of `symbol` in its file.
fn source_start(symbol: &Symbol, root: &Path) -> Option<String> {
    let mut file = File::open(root.join(&symbol.path)).ok()?;
    file.seek(SeekFrom::Start(symbol.start_byte as u64)).ok()?;
    let mut bytes = Vec::new();
    file.take(HEAD_BYTES).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The text of `declaration` before the name, or before the parameter list when the name stands
/// elsewhere (`const f = async (a) => a`).
fn declaration_head<'a>(declaration: &'a str, name: &str) -> &'a str {
    let end = word_position(declaration, name)
        .or_else(|| declaration.find('('))
        .unwrap_or(0);
    &declaration[..end]
}

pub(super) enum Difference {
    None,
    Signature,
    /// Words of the call contract that came or went, as `async added`.
    Modifiers(Vec<String>),
    Renamed(Vec<(String, String)>),
}

impl Shape {
    /// `root` is the folder that holds the file of `symbol`, where its modifiers are read.
    pub(super) fn of(conn: &Connection, symbol: &Symbol, root: &Path) -> Result<Shape, QueryError> {
        let signature = signature_of(symbol);
        Ok(Shape {
            raw: tail_of(&signature, &symbol.name),
            normal: normalize(conn, symbol, &signature)?,
            contract: contract_words(conn, symbol, root)?,
        })
    }

    /// How `self` (before) differs from `other` (after) in the way a caller can see.
    pub(super) fn compare(&self, other: &Shape, language: &str) -> Difference {
        let toggled = self.toggled(other);
        if !toggled.is_empty() {
            return Difference::Modifiers(toggled);
        }
        let (Some(was), Some(is)) = (&self.normal, &other.normal) else {
            return if self.raw == other.raw {
                Difference::None
            } else {
                Difference::Signature
            };
        };
        if was.tail != is.tail {
            return Difference::Signature;
        }
        if was.names == is.names {
            return Difference::None;
        }
        if was.names.len() != is.names.len() {
            return Difference::Signature;
        }
        let sorted = |names: &[String]| {
            let mut names = names.to_vec();
            names.sort();
            names
        };
        if sorted(&was.names) == sorted(&is.names) {
            return Difference::Signature;
        }
        if !NAMED_ARGUMENT_LANGUAGES.contains(&language) {
            return Difference::None;
        }
        Difference::Renamed(
            was.names
                .iter()
                .zip(&is.names)
                .filter(|(old, new)| old != new)
                .map(|(old, new)| (old.clone(), new.clone()))
                .collect(),
        )
    }
}

/// The signature of `symbol` without its leading attributes.
pub(super) fn signature_of(symbol: &Symbol) -> String {
    without_attributes(
        symbol.signature.as_deref().unwrap_or_default(),
        &symbol.language,
    )
}

/// The signature from the name on, with the words before the name that a caller never sees
/// dropped (modifiers, decorators, `pub(…)`), and whitespace folded.
pub(super) fn tail_of(signature: &str, name: &str) -> String {
    let text = signature.split_whitespace().collect::<Vec<_>>().join(" ");
    let Some(at) = word_position(&text, name) else {
        return text;
    };
    let head: Vec<&str> = text[..at]
        .split_whitespace()
        .filter(|word| {
            !word.starts_with('@')
                && !word.starts_with("pub(")
                && !super::SIGNATURE_MODIFIERS.contains(word)
        })
        .collect();
    format!("{} {}", head.join(" "), &text[at..])
}

fn normalize(
    conn: &Connection,
    symbol: &Symbol,
    signature: &str,
) -> Result<Option<Normal>, QueryError> {
    let language = symbol.language.as_str();
    let Some((list, segments)) = arity::parameter_segments(signature, &symbol.name, language)
    else {
        return Ok(None);
    };
    let rows = arity::parameter_rows(conn, symbol)?;
    let python = language == "python";
    let mut pieces = Vec::new();
    let mut names = Vec::new();
    let mut used = vec![false; rows.len()];
    let mut first = 0;
    while first < segments.len() {
        let key = comparable(&without_annotations(segments[first], language));
        if python && matches!(key.as_str(), "*" | "/") {
            pieces.push(format!("<{key}>"));
            first += 1;
            continue;
        }
        if language == "rust" && is_ignored_parameter(&key) {
            pieces.push(piece_of(segments[first], &[], language));
            names.push("_".to_string());
            first += 1;
            continue;
        }
        let Some((last, text, mut owned)) = (first..segments.len()).find_map(|last| {
            let text = segments[first..=last].join(", ");
            let key = comparable(&without_annotations(&text, language));
            let owned: Vec<usize> = (0..rows.len())
                .filter(|&at| {
                    let row = &rows[at];
                    let written = row.signature.as_deref().unwrap_or(&row.name);
                    comparable(&without_annotations(written, language)) == key
                })
                .collect();
            (!owned.is_empty()).then_some((last, text, owned))
        }) else {
            return Ok(None);
        };
        owned.sort_by_key(|&at| word_position(&text, &rows[at].name));
        let owned_rows: Vec<&ParameterRow> = owned.iter().map(|&at| &rows[at]).collect();
        let piece = piece_of(&text, &owned_rows, language);
        for &at in &owned {
            used[at] = true;
            pieces.push(piece.clone());
            names.push(rows[at].name.clone());
        }
        first = last + 1;
    }
    let mut before = signature[..list.start].to_string();
    let unmatched: Vec<&ParameterRow> = rows
        .iter()
        .zip(&used)
        .filter(|(_, used)| !**used)
        .map(|(row, _)| row)
        .collect();
    match unmatched.as_slice() {
        [] => {}
        [receiver] if language == "go" && symbol.kind == "method" => {
            let Some(at) = word_position(&before, &receiver.name) else {
                return Ok(None);
            };
            before.replace_range(at..at + receiver.name.len(), "");
        }
        _ => return Ok(None),
    }
    let after = match language {
        "python" => ")".to_string(),
        "kotlin" | "csharp" => without_body(&signature[list.end..]).to_string(),
        "go" => go_results(&signature[list.end + 1..]).map_or_else(
            || signature[list.end..].to_string(),
            |types| format!("){types}"),
        ),
        _ => signature[list.end..].to_string(),
    };
    let text = format!("{before}{}:{}{after}", pieces.len(), pieces.join(","));
    Ok(Some(Normal {
        tail: tail_of(&text, &symbol.name),
        names,
    }))
}

/// How many parameters of a Rust function are written `_: T`.
pub(super) fn ignored_parameters(symbol: &Symbol) -> usize {
    if symbol.language != "rust" {
        return 0;
    }
    symbol
        .signature
        .as_deref()
        .and_then(|signature| arity::parameter_segments(signature, &symbol.name, "rust"))
        .map_or(0, |(_, segments)| {
            segments
                .iter()
                .filter(|segment| is_ignored_parameter(segment))
                .count()
        })
}

/// Whether a Rust parameter is written `_: T`. The index holds no row for it.
fn is_ignored_parameter(parameter: &str) -> bool {
    parameter
        .split_once(':')
        .is_some_and(|(pattern, _)| pattern.trim() == "_")
}

/// `text` without the leading attribute groups of C# (`[Obsolete]`, `[A, B(1)] [C]`) and PHP
/// (`#[Pure]`); any other language keeps its text.
fn without_attributes(text: &str, language: &str) -> String {
    let opener = match language {
        "csharp" => "[",
        "php" => "#[",
        _ => return text.to_string(),
    };
    let code = code_only(text, language);
    let mut end = 0;
    while let Some(group) = code[end..].trim_start().strip_prefix(opener) {
        let Some(after) = after_balanced(&code[code.len() - group.len() - 1..], '[', ']') else {
            break;
        };
        end = code.len() - after.len();
    }
    text[end..].to_string()
}

/// `text` without the annotations of a Java or Kotlin parameter (`@Deprecated`, `@Size(min = 1)`,
/// `@param:Foo`) and the attributes of a C# or PHP one; any other language keeps its text.
fn without_annotations(text: &str, language: &str) -> String {
    if !matches!(language, "java" | "kotlin") {
        return without_attributes(text, language);
    }
    let mut bare = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('@') {
        bare.push_str(&rest[..at]);
        bare.push(' ');
        let name = &rest[at + 1..];
        let name_end = name
            .find(|c: char| !(is_identifier_char(c) || matches!(c, '.' | ':')))
            .unwrap_or(name.len());
        let after_name = &name[name_end..];
        rest = if after_name.trim_start().starts_with('(') {
            after_balanced(after_name.trim_start(), '(', ')').unwrap_or_default()
        } else {
            after_name
        };
    }
    bare.push_str(rest);
    bare
}

/// The result types of a Go function written as `(int, error)`, `(n int, err error)`, `(x, y
/// int)`, or `int`, as `(int,error)`; `""` for no result. `None` when the text is not a result
/// list. `tail` is the text after the closing parenthesis of the parameter list.
fn go_results(tail: &str) -> Option<String> {
    let tail = tail.trim();
    if tail.is_empty() {
        return Some(String::new());
    }
    let collapse = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let Some(group) = tail.strip_prefix('(') else {
        return Some(format!("({})", collapse(tail)));
    };
    if !after_balanced(tail, '(', ')')?.trim().is_empty() {
        return None;
    }
    let pieces = split_body(group.strip_suffix(')')?).0;
    let named = |piece: &str| {
        piece
            .split_once(char::is_whitespace)
            .is_some_and(|(first, _)| {
                first.chars().all(is_identifier_char) && !GO_TYPE_WORDS.contains(&first)
            })
    };
    if !pieces.iter().any(|piece| named(piece)) {
        let types: Vec<String> = pieces.iter().map(|piece| collapse(piece)).collect();
        return Some(format!("({})", types.join(",")));
    }
    let mut types = Vec::new();
    let mut current = None;
    for piece in pieces.iter().rev() {
        if named(piece) {
            current = Some(collapse(piece.split_once(char::is_whitespace)?.1));
        }
        types.push(current.clone()?);
    }
    types.reverse();
    Some(format!("({})", types.join(",")))
}

/// `suffix` up to the body of an expression-bodied function (`= value * factor`, `=> value`).
fn without_body(suffix: &str) -> &str {
    let end = [arity::default_start(suffix), suffix.find("=>")]
        .into_iter()
        .flatten()
        .min();
    end.map_or(suffix, |end| &suffix[..end])
}

/// `text` without its default value, whitespace folded.
fn comparable(text: &str) -> String {
    let without_default = arity::default_start(text).map_or(text, |at| &text[..at]);
    without_default
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(" ,", ",")
        .replace(", ", ",")
}

/// What of a parameter stays once its names, default value, and binding `mut` are gone: its
/// type and modifiers. A Python parameter keeps only its `*` or `**` marker.
fn piece_of(segment: &str, owned: &[&ParameterRow], language: &str) -> String {
    let text = comparable(&without_annotations(segment, language));
    if language == "python" {
        let stars = text.chars().take_while(|&c| c == '*').count();
        return "*".repeat(stars.min(2));
    }
    let mut text = match (language, text.strip_prefix("mut ")) {
        ("rust", Some(rest)) => rest.to_string(),
        _ => text,
    };
    if language == "rust" && is_ignored_parameter(&text) {
        text.replace_range(..1, " ");
    }
    for row in owned {
        if let Some(at) = word_position(&text, &row.name) {
            text.replace_range(at..at + row.name.len(), " ");
        }
    }
    if language == "java"
        && let Some(at) = word_position(&text, "final")
    {
        text.replace_range(at..at + "final".len(), " ");
    }
    let squeezed: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(" :", ":")
        .replace(": ", ":")
        .replace(" ,", ",")
        .replace(", ", ",");
    squeezed
        .trim_matches(|c: char| matches!(c, ':' | ',' | ' '))
        .to_string()
}

/// Whether the call that spans `lines` (the first line from the callee's name on) passes
/// `parameter` by name, outside strings and comments: `p=` in Python and Kotlin, `p:` in C# and
/// Swift, right after `(` or `,`.
pub(super) fn names_parameter(
    lines: &[&str],
    callee: &str,
    parameter: &str,
    language: &str,
) -> bool {
    let Some((first, rest)) = lines.split_first() else {
        return false;
    };
    let start = word_position(first, callee).unwrap_or(0);
    let mut text = first[start..].to_string();
    for line in rest {
        text.push('\n');
        text.push_str(line);
    }
    let code = code_only(&text, language);
    let Some(open) = code.find('(') else {
        return false;
    };
    let separator = if matches!(language, "csharp" | "swift") {
        ':'
    } else {
        '='
    };
    names_in_call(&code[open..], callee, parameter, separator)
}

pub(super) fn added_required_python_keyword_parameters(was: &Symbol, is: &Symbol) -> Vec<String> {
    if was.language != "python" || is.language != "python" {
        return Vec::new();
    }
    let (Some(before), Some(after)) = (
        required_python_keyword_parameters(was),
        required_python_keyword_parameters(is),
    ) else {
        return Vec::new();
    };
    after
        .into_iter()
        .filter(|name| !before.contains(name))
        .collect()
}

fn required_python_keyword_parameters(symbol: &Symbol) -> Option<Vec<String>> {
    let signature = symbol.signature.as_deref()?;
    let (_, segments) = arity::parameter_segments(signature, &symbol.name, "python")?;
    let mut keyword_only = false;
    let mut required = Vec::new();
    for segment in segments {
        let segment = segment.trim();
        match segment {
            "*" => {
                keyword_only = true;
                continue;
            }
            "/" => continue,
            _ => {}
        }
        if segment.starts_with("**") {
            continue;
        }
        if segment.starts_with('*') {
            keyword_only = true;
            continue;
        }
        if !keyword_only || arity::default_start(segment).is_some() {
            continue;
        }
        let name = segment
            .split(|character: char| {
                character == ':' || character == '=' || character.is_whitespace()
            })
            .next()?
            .trim();
        if name.is_empty()
            || !name
                .chars()
                .next()
                .is_some_and(|character| character == '_' || character.is_alphabetic())
            || !name.chars().all(is_identifier_char)
        {
            return None;
        }
        required.push(name.to_string());
    }
    Some(required)
}

pub(super) struct RequiredKeywordArguments {
    pub(super) missing: Vec<String>,
    pub(super) has_unknown_spread: bool,
}

pub(super) fn required_python_keyword_arguments(
    lines: &[&str],
    callee: &str,
    required: &[String],
) -> Option<RequiredKeywordArguments> {
    let code = code_only(&lines.join("\n"), "python");
    let mut calls = code.match_indices(callee).filter(|(at, _)| {
        let end = at + callee.len();
        code[..*at]
            .chars()
            .next_back()
            .is_none_or(|character| !is_identifier_char(character))
            && code[end..].trim_start().starts_with('(')
    });
    let (at, _) = calls.next()?;
    if calls.next().is_some() {
        return None;
    }
    let arguments_start = code[at + callee.len()..]
        .find('(')
        .map(|offset| at + callee.len() + offset)?;
    let call = &code[arguments_start..];
    let rest = after_balanced(call, '(', ')')?;
    let closing = call.len().checked_sub(rest.len() + 1)?;
    let arguments = split_body(&call[1..closing]).0;
    let missing = required
        .iter()
        .filter(|parameter| {
            !arguments
                .iter()
                .any(|argument| is_keyword_argument(argument, parameter))
        })
        .cloned()
        .collect();
    Some(RequiredKeywordArguments {
        missing,
        has_unknown_spread: arguments
            .iter()
            .any(|argument| argument.trim_start().starts_with("**")),
    })
}

fn is_keyword_argument(argument: &str, parameter: &str) -> bool {
    let argument = argument.trim();
    let Some(rest) = argument.strip_prefix(parameter) else {
        return false;
    };
    let rest = rest.trim_start();
    !argument[parameter.len()..].starts_with(is_identifier_char)
        && rest.starts_with('=')
        && !rest[1..].starts_with('=')
}

/// Whether the arguments of the call that opens `call` pass `parameter` by name, or hold a call of
/// `callee` that does.
fn names_in_call(call: &str, callee: &str, parameter: &str, separator: char) -> bool {
    let inner = match after_balanced(call, '(', ')') {
        Some(rest) => &call[1..call.len() - rest.len() - 1],
        None => &call[1..],
    };
    split_body(inner).0.iter().any(|argument| {
        let named = argument.strip_prefix(parameter).is_some_and(|rest| {
            let rest = rest.trim_start();
            !argument[parameter.len()..].starts_with(is_identifier_char)
                && rest.starts_with(separator)
                && !rest[1..].starts_with(separator)
        });
        named
            || argument.match_indices(callee).any(|(at, _)| {
                let nested = argument[at + callee.len()..].trim_start();
                !argument[..at].ends_with(is_identifier_char)
                    && nested.starts_with('(')
                    && names_in_call(nested, callee, parameter, separator)
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_python_call_names_a_parameter_with_an_equals_sign_in_code_only() {
        let call = |text: &str| names_parameter(&[text], "f", "factor", "python");
        assert!(call("return f(1, factor=2)"));
        assert!(call("return f(factor = 2)"));
        assert!(!call("return f(1, 2)"));
        assert!(!call("return f(1, factor == 2)"));
        assert!(!call("return f(1, \"factor=2\")"));
        assert!(!call("return f(1, other_factor=2)"));
        assert!(!call("return f(1)  # factor=2"));
        assert!(!call("return f(factor)"));
    }

    #[test]
    fn a_call_across_lines_is_read_to_its_last_line() {
        let lines = ["x = f(1,", "      factor=2)"];
        assert!(names_parameter(&lines, "f", "factor", "python"));
    }

    #[test]
    fn text_before_the_callee_does_not_count() {
        let lines = ["g(factor=1); f(2)"];
        assert!(!names_parameter(&lines, "f", "factor", "python"));
    }

    #[test]
    fn a_kotlin_call_names_a_parameter_with_an_equals_sign() {
        assert!(names_parameter(
            &["f(1, factor = 2)"],
            "f",
            "factor",
            "kotlin"
        ));
        assert!(!names_parameter(&["f(1, 2)"], "f", "factor", "kotlin"));
    }

    #[test]
    fn a_csharp_or_swift_call_names_a_parameter_with_a_colon() {
        assert!(names_parameter(
            &["F(1, factor: 2);"],
            "F",
            "factor",
            "csharp"
        ));
        assert!(names_parameter(&["f(factor: 2)"], "f", "factor", "swift"));
        assert!(!names_parameter(
            &["F(1, c ? factor : 2);"],
            "F",
            "factor",
            "csharp"
        ));
        assert!(!names_parameter(&["F(1, 2);"], "F", "factor", "csharp"));
    }

    #[test]
    fn a_call_of_the_same_line_does_not_lend_its_named_argument() {
        assert!(!names_parameter(
            &["g(1); f(2, other=3); h(factor=4)"],
            "f",
            "factor",
            "python"
        ));
    }

    #[test]
    fn a_nested_call_lends_its_named_argument_only_when_it_has_the_name_of_the_callee() {
        let call = |text: &str| names_parameter(&[text], "f", "factor", "python");
        assert!(!call("f(g(factor=1))"));
        assert!(!call("f(1, g(factor=2))"));
        assert!(!call("f(1, b=[g(factor=2)])"));
        assert!(call("f(f(factor=1))"));
        assert!(call("f(1, g(f(factor=2)))"));
        assert!(call("f(g(1), factor=2)"));
        let lines = ["f(1,", "  b=g(", "    factor=2,", "  ),", ")"];
        assert!(!names_parameter(&lines, "f", "factor", "python"));
    }

    #[test]
    fn go_result_lists_with_and_without_names_normalize_alike() {
        let same = |a: &str, b: &str| go_results(a) == go_results(b);
        assert!(same(" (int, error)", " (n int, err error)"));
        assert!(same(" (int, int)", " (x, y int)"));
        assert!(same(" int", " (n int)"));
        assert!(!same(" (int, error)", " (string, error)"));
        assert_eq!(go_results("").as_deref(), Some(""));
        assert_eq!(go_results(" (int) extra"), None);
    }

    #[test]
    fn java_and_kotlin_annotations_are_dropped_from_a_parameter() {
        let bare = |text: &str, language: &str| {
            without_annotations(text, language)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert_eq!(bare("@Deprecated int a", "java"), "int a");
        assert_eq!(
            bare("@Size(min = 1, max = (2)) String s", "java"),
            "String s"
        );
        assert_eq!(bare("@param:Foo val a: Int", "kotlin"), "val a: Int");
        assert_eq!(bare("@Deprecated int a", "csharp"), "@Deprecated int a");
    }

    #[test]
    fn csharp_and_php_attribute_groups_are_dropped_from_the_start_of_a_declaration() {
        let bare = |text: &str, language: &str| {
            without_attributes(text, language)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert_eq!(
            bare("[Obsolete] public int H(int a)", "csharp"),
            "public int H(int a)"
        );
        assert_eq!(
            bare("[A, B(1, \"]\")] [C]\n  public int H()", "csharp"),
            "public int H()"
        );
        assert_eq!(bare("[Range(0, 1)] int a", "csharp"), "int a");
        assert_eq!(
            bare("public int[] H(int[,] a)", "csharp"),
            "public int[] H(int[,] a)"
        );
        assert_eq!(
            bare("[Obsolete public int H()", "csharp"),
            "[Obsolete public int H()"
        );
        assert_eq!(
            bare("#[Pure] #[Route('/a')] function f()", "php"),
            "function f()"
        );
        assert_eq!(bare("#[Pure] int $a", "csharp"), "#[Pure] int $a");
        assert_eq!(bare("[Pure] fn f()", "rust"), "[Pure] fn f()");
    }

    #[test]
    fn a_rust_parameter_named_underscore_is_ignored() {
        assert!(is_ignored_parameter("_: u8"));
        assert!(is_ignored_parameter(" _ : u8"));
        assert!(!is_ignored_parameter("_unused: u8"));
        assert!(!is_ignored_parameter("a: u8"));
    }

    #[test]
    fn the_head_of_a_declaration_ends_at_the_name_or_the_parameter_list() {
        assert_eq!(
            declaration_head("pub async fn run(a: u8)", "run"),
            "pub async fn "
        );
        assert_eq!(declaration_head("async (a) => a", "f"), "async ");
    }
}
