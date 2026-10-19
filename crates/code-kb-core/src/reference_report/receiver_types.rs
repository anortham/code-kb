//! Receiver types that name no one type: a value that can hold any type, and a type parameter of
//! the caller or of its class. A call through either can reach any definition of the name.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::rc::Rc;

use rusqlite::{OptionalExtension, params};

use super::{Accounting, NameRow, QueryError, is_plain_identifier, is_word_char};

/// The longest type-parameter list read from the header of a TypeScript class or interface, in
/// bytes.
const MAX_HEADER: usize = 600;
/// The longest type alias declaration read, in bytes.
const MAX_ALIAS: usize = 300;
/// How many aliases of aliases are followed.
const MAX_ALIAS_HOPS: usize = 4;
const TYPING_MODULES: &[&str] = &["typing", "typing_extensions"];

const ENCLOSING_DECLARATIONS_SQL: &str = "WITH RECURSIVE chain(symbol_id, depth) AS (
         SELECT ?1, 0
         UNION ALL
         SELECT s.parent_symbol_id, chain.depth + 1
         FROM symbols s JOIN chain ON s.symbol_id = chain.symbol_id
         WHERE s.parent_symbol_id IS NOT NULL AND chain.depth < 32
     )
     SELECT s.symbol_id, s.name, s.kind, s.language, s.path, s.start_byte, s.end_byte,
            COALESCE(s.signature, ''),
            CASE WHEN json_valid(s.metadata_json)
                 THEN COALESCE(json_extract(s.metadata_json, '$.typeParameters'), '')
                 ELSE '' END
     FROM chain JOIN symbols s ON s.symbol_id = chain.symbol_id
     ORDER BY chain.depth";

const TYPE_ALIAS_SQL: &str = "SELECT path, start_byte, end_byte FROM symbols
     WHERE name = ?1 AND kind = 'type' AND language IN ('typescript', 'tsx')";

const TYPE_VARIABLE_SQL: &str = "SELECT 1 FROM symbols
     WHERE name = ?1 AND language = 'python' AND kind IN ('variable', 'constant')
       AND parent_symbol_id IS NULL AND signature LIKE '%TypeVar(%'
     LIMIT 1";

const PYTHON_IMPORT_BINDING_SQL: &str =
    "SELECT COALESCE(json_extract(metadata_json, '$.importedName'), ''),
            COALESCE(json_extract(metadata_json, '$.source'), '')
     FROM symbols
     WHERE kind = 'import' AND path = ?1 AND name = ?2 AND json_valid(metadata_json)";

#[derive(Default)]
pub(super) struct ReceiverCaches {
    declared: RefCell<HashMap<String, Rc<Vec<String>>>>,
    aliases: RefCell<HashMap<String, bool>>,
}

struct Enclosing {
    symbol_id: String,
    name: String,
    kind: String,
    language: String,
    path: String,
    start_byte: usize,
    end_byte: usize,
    signature: String,
    type_parameters_json: String,
}

/// Whether a variable declared with `type_name` in `language` can hold a value of any type at the
/// call: TypeScript `any`, `unknown`, and `object`, where a check narrows the variable before the
/// call; Python `Any` and `object`, which no run enforces; and C# `dynamic`. A C# `object`, a
/// Java `Object`, and a Kotlin `Any` take no member call that their type does not declare.
pub(super) fn names_any_value(language: &str, type_name: &str) -> bool {
    match language {
        "typescript" | "tsx" => matches!(type_name, "any" | "unknown" | "object"),
        "python" => matches!(type_name, "Any" | "object"),
        "csharp" => type_name == "dynamic",
        _ => false,
    }
}

impl Accounting<'_> {
    /// Whether the receiver's declared type can hold a value of any type: `names_any_value`, a
    /// Python `Any` written through its module (`typing.Any`, `t.Any`) or imported under another
    /// name (`from typing import Any as AnyT`), or a TypeScript alias of such a type.
    pub(super) fn holds_any_value(
        &self,
        row: &NameRow,
        type_name: &str,
    ) -> Result<bool, QueryError> {
        if names_any_value(&row.language, type_name) {
            return Ok(true);
        }
        match row.language.as_str() {
            "python" => self.python_any(&row.path, type_name),
            "typescript" | "tsx" => self.alias_of_any(type_name, 0),
            _ => Ok(false),
        }
    }

    /// Whether `type_name` is a type parameter of the caller, of a function that encloses it, or
    /// of a class that encloses it, or a Python `TypeVar`. Such a type stands for any type that
    /// fits its bound.
    pub(super) fn is_type_parameter(
        &self,
        caller_id: &str,
        language: &str,
        type_name: &str,
    ) -> Result<bool, QueryError> {
        if !is_plain_identifier(type_name) {
            return Ok(false);
        }
        if language == "python"
            && self
                .with_statement(TYPE_VARIABLE_SQL, |stmt| {
                    stmt.query_row(params![type_name], |_| Ok(())).optional()
                })?
                .is_some()
        {
            return Ok(true);
        }
        let scopes = self.with_statement(ENCLOSING_DECLARATIONS_SQL, |stmt| {
            stmt.query_map(params![caller_id], |r| {
                Ok(Enclosing {
                    symbol_id: r.get(0)?,
                    name: r.get(1)?,
                    kind: r.get(2)?,
                    language: r.get(3)?,
                    path: r.get(4)?,
                    start_byte: r.get::<_, i64>(5)?.max(0) as usize,
                    end_byte: r.get::<_, i64>(6)?.max(0) as usize,
                    signature: r.get(7)?,
                    type_parameters_json: r.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
        })?;
        Ok(scopes.iter().any(|scope| {
            self.declared_type_parameters(scope)
                .iter()
                .any(|name| name == type_name)
        }))
    }

    fn declared_type_parameters(&self, scope: &Enclosing) -> Rc<Vec<String>> {
        if let Some(names) = self.receivers.declared.borrow().get(&scope.symbol_id) {
            return names.clone();
        }
        let mut names = type_parameters_in(&scope.signature, &scope.name, true);
        names.extend(metadata_type_parameters(&scope.type_parameters_json));
        let typescript = matches!(scope.language.as_str(), "typescript" | "tsx");
        if typescript && matches!(scope.kind.as_str(), "class" | "interface") && names.is_empty() {
            let length = scope
                .end_byte
                .saturating_sub(scope.start_byte)
                .min(MAX_HEADER);
            if let Some(header) = read_bytes(self.root, &scope.path, scope.start_byte, length) {
                names = type_parameters_in(&header, &scope.name, false);
            }
        }
        let names = Rc::new(names);
        self.receivers
            .declared
            .borrow_mut()
            .insert(scope.symbol_id.clone(), names.clone());
        names
    }

    fn python_any(&self, path: &str, type_name: &str) -> Result<bool, QueryError> {
        let (module, last) = match type_name.rsplit_once('.') {
            Some((module, last)) => (Some(module), last),
            None => (None, type_name),
        };
        let binding = module.unwrap_or(type_name);
        if module.is_some_and(|module| TYPING_MODULES.contains(&module)) {
            return Ok(last == "Any");
        }
        let bound = self.with_statement(PYTHON_IMPORT_BINDING_SQL, |stmt| {
            stmt.query_map(params![path, binding], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()
        })?;
        Ok(bound.iter().any(|(imported, source)| {
            let from_typing = TYPING_MODULES.contains(&source.as_str());
            match module {
                Some(_) => last == "Any" && from_typing,
                None => imported == "Any" && from_typing,
            }
        }))
    }

    /// Whether a project type alias named `type_name` stands for `any`, `unknown`, or `object`.
    fn alias_of_any(&self, type_name: &str, hops: usize) -> Result<bool, QueryError> {
        if hops > MAX_ALIAS_HOPS || !is_plain_identifier(type_name) {
            return Ok(false);
        }
        if let Some(&known) = self.receivers.aliases.borrow().get(type_name) {
            return Ok(known);
        }
        let declarations = self.with_statement(TYPE_ALIAS_SQL, |stmt| {
            stmt.query_map(params![type_name], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?.max(0) as usize,
                    r.get::<_, i64>(2)?.max(0) as usize,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()
        })?;
        let mut holds = false;
        for (path, start, end) in declarations {
            let length = end.saturating_sub(start).min(MAX_ALIAS);
            let Some(target) = read_bytes(self.root, &path, start, length)
                .and_then(|text| alias_target(&text, type_name))
            else {
                continue;
            };
            if names_any_value("typescript", &target) || self.alias_of_any(&target, hops + 1)? {
                holds = true;
                break;
            }
        }
        if hops == 0 {
            self.receivers
                .aliases
                .borrow_mut()
                .insert(type_name.to_string(), holds);
        }
        Ok(holds)
    }
}

fn read_bytes(root: &Path, path: &str, start: usize, length: usize) -> Option<String> {
    let mut file = File::open(root.join(path)).ok()?;
    file.seek(SeekFrom::Start(start as u64)).ok()?;
    let mut buffer = Vec::new();
    file.take(length as u64).read_to_end(&mut buffer).ok()?;
    Some(String::from_utf8_lossy(&buffer).into_owned())
}

/// The type `type Name = target;` stands for, as written.
fn alias_target(declaration: &str, name: &str) -> Option<String> {
    let after_keyword = declaration.split_once("type ")?.1.trim_start();
    let after_name = after_keyword.strip_prefix(name)?.trim_start();
    let target = after_name.strip_prefix('=')?.trim();
    Some(target.trim_end_matches(';').trim_end().to_string())
}

/// The names in the `typeParameters` array that julie writes for a TypeScript function
/// (`["T extends { m(a: number): number }"]`).
fn metadata_type_parameters(json: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(json)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| parameter_name(item))
                .collect()
        })
        .unwrap_or_default()
}

/// The names declared by the type-parameter list of a declaration, `owner` being the name that
/// declaration gives: `<T>` or `[T]` right after the name (`Box<T extends Greeter>`,
/// `ViaT<T>(T x)`), and, when `before_name` is set, `<T>` set off by a blank before the name
/// (`static <T extends Greeter> void viaT(T x)`). Annotation arguments and the parameter list are
/// skipped, and the scan ends at a body or a default, so a generic return type or base type is
/// never read.
fn type_parameters_in(text: &str, owner: &str, before_name: bool) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut names = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'(' => match matching_close(bytes, at) {
                Some(close) => at = close,
                None => break,
            },
            b'{' | b'=' => break,
            b'<' | b'[' if opens_list(text, at, owner, before_name) => {
                let Some(close) = matching_close(bytes, at) else {
                    break;
                };
                names.extend(
                    split_items(&text[at + 1..close])
                        .iter()
                        .filter_map(|item| parameter_name(item)),
                );
                at = close;
            }
            _ => {}
        }
        at += 1;
    }
    names
}

fn opens_list(text: &str, at: usize, owner: &str, before_name: bool) -> bool {
    let before = &text[..at];
    if before
        .strip_suffix(owner)
        .is_some_and(|rest| !owner.is_empty() && !rest.ends_with(is_word_char))
    {
        return true;
    }
    before_name
        && text.as_bytes()[at] == b'<'
        && before.chars().next_back().is_none_or(char::is_whitespace)
}

/// The index of the bracket that closes the one at `open`; the `>` of `=>` and `->` closes
/// nothing.
fn matching_close(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for at in open..bytes.len() {
        match bytes[at] {
            b'<' | b'[' | b'(' | b'{' => depth += 1,
            b'>' if at > 0 && matches!(bytes[at - 1], b'=' | b'-') => {}
            b'>' | b']' | b')' | b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_items(list: &str) -> Vec<&str> {
    let bytes = list.as_bytes();
    let mut items = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (at, byte) in bytes.iter().enumerate() {
        match byte {
            b'<' | b'[' | b'(' | b'{' => depth += 1,
            b'>' if at > 0 && matches!(bytes[at - 1], b'=' | b'-') => {}
            b'>' | b']' | b')' | b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                items.push(&list[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    items.push(&list[start..]);
    items
}

/// The name a type-parameter item declares: `V` of `V extends Greeter`, `out T`, `const T`, or
/// `T : Greeter`. A lifetime (`'a`) declares no type.
fn parameter_name(item: &str) -> Option<String> {
    let mut words = item
        .split(|c: char| !is_word_char(c))
        .filter(|word| !word.is_empty());
    let first = words.next()?;
    let name = if matches!(first, "in" | "out" | "const" | "reified") {
        words.next()?
    } else {
        first
    };
    (!item.trim_start().starts_with('\'')).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(text: &str, owner: &str, before_name: bool) -> Vec<String> {
        type_parameters_in(text, owner, before_name)
    }

    #[test]
    fn a_list_before_the_return_type_declares_its_names() {
        assert_eq!(
            names(
                "static <K, V extends Comparable<? super V>> void viaSecond(K k, V v)",
                "viaSecond",
                true
            ),
            ["K", "V"]
        );
    }

    #[test]
    fn a_list_right_after_the_name_declares_its_names() {
        assert_eq!(
            names("public class Box<T> where T : IGreeter", "Box", true),
            ["T"]
        );
        assert_eq!(names("fn run<'a, T: Tr>(x: &'a T)", "run", true), ["T"]);
        assert_eq!(
            names("fun <out T, reified U> f(x: T)", "f", true),
            ["T", "U"]
        );
    }

    #[test]
    fn annotation_arguments_before_the_list_are_skipped() {
        assert_eq!(
            names(
                "@SuppressWarnings(\"unchecked\") static <T extends Greeter> void viaT(T x)",
                "viaT",
                true
            ),
            ["T"]
        );
    }

    #[test]
    fn a_generic_return_type_or_parameter_type_declares_nothing() {
        assert!(names("static List<Foo> make(Map<Bar, Baz> m)", "make", true).is_empty());
        assert!(names("public Dictionary<K, V> Build()", "Build", true).is_empty());
    }

    #[test]
    fn a_class_header_ends_at_its_body_and_ignores_the_base_type() {
        assert_eq!(
            names(
                "class Box<T extends { m(a: number): number }> extends Base<X> { make() { return new Box<Y>(); } }",
                "Box",
                false
            ),
            ["T"]
        );
        assert!(names("class Plain extends Base<X> { }", "Plain", false).is_empty());
    }

    #[test]
    fn an_arrow_in_a_bound_does_not_end_the_list() {
        assert_eq!(
            names(
                "class Box<T extends (a: number) => void, U> { }",
                "Box",
                false
            ),
            ["T", "U"]
        );
    }

    #[test]
    fn an_alias_target_is_read_as_written() {
        assert_eq!(
            alias_target("export type Anything = any;", "Anything").as_deref(),
            Some("any")
        );
        assert_eq!(alias_target("type Box<T> = any;", "Box"), None);
    }
}
