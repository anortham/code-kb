//! Which arguments of a call are values that cannot be null. julie writes a C# `int?` value as
//! `int`, a Kotlin `Int?` value as `Int`, and a Swift `Int!` value as `Int`, so the overload
//! rules keep the overloads that take a nullable value for such an argument. The source shows
//! that some arguments cannot be nullable: a literal, a C# `new` object, a Swift construction of a
//! project struct or class that no failable initializer of its own, of a superclass, or of a
//! protocol it conforms to can build, and a local, parameter, field, or property whose declared
//! type has no nullable marker and is no type parameter.

use std::collections::HashSet;
use std::rc::Rc;

use rusqlite::{Connection, params};

use crate::queries::QueryError;

/// The language whose argument source is read.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Dialect {
    CSharp,
    Kotlin,
    Swift,
}

const DECLARATIONS_SQL: &str = "WITH RECURSIVE chain(symbol_id, depth) AS (
         SELECT ?1, 0
         UNION ALL
         SELECT s.parent_symbol_id, chain.depth + 1
         FROM symbols s JOIN chain ON s.symbol_id = chain.symbol_id
         WHERE s.parent_symbol_id IS NOT NULL AND chain.depth < 32
     )
     SELECT chain.depth, COALESCE(v.signature, ''), v.path, v.start_byte, v.end_byte
     FROM chain
     JOIN symbols v ON v.parent_symbol_id = chain.symbol_id
     WHERE v.name = ?2 AND v.kind IN ('variable', 'field', 'property', 'constant', 'parameter')
     ORDER BY chain.depth";
const CHAIN_SIGNATURES_SQL: &str = "WITH RECURSIVE chain(symbol_id, depth) AS (
         SELECT ?1, 0
         UNION ALL
         SELECT s.parent_symbol_id, chain.depth + 1
         FROM symbols s JOIN chain ON s.symbol_id = chain.symbol_id
         WHERE s.parent_symbol_id IS NOT NULL AND chain.depth < 32
     )
     SELECT COALESCE(s.signature, '') FROM chain JOIN symbols s ON s.symbol_id = chain.symbol_id";
const SWIFT_CONSTRUCTED_SQL: &str = "SELECT
       (SELECT COUNT(*) FROM symbols
         WHERE name = ?1 AND language = 'swift' AND kind IN ('struct', 'class')),
       (SELECT COUNT(*) FROM symbols
         WHERE name = ?1 AND language = 'swift'
           AND kind NOT IN ('struct', 'class', 'module', 'constructor', 'import'))";
const SWIFT_FAILABLE_SQL: &str = "SELECT COUNT(*) FROM symbols c
     JOIN symbols p ON p.symbol_id = c.parent_symbol_id
     WHERE p.name = ?1 AND p.language = 'swift' AND c.kind = 'constructor'
       AND ((' ' || c.signature) LIKE '% init?%' OR (' ' || c.signature) LIKE '% init!%')";
/// The declarations and extensions of a Swift type or protocol, whose heads name its bases.
const SWIFT_DECLARATIONS_SQL: &str = "SELECT kind, COALESCE(signature, '') FROM symbols
     WHERE name = ?1 AND language = 'swift'
       AND kind IN ('struct', 'class', 'enum', 'interface', 'protocol', 'module')";
const SWIFT_BASES_SQL: &str = "SELECT base.name FROM symbols child
     JOIN relationships r ON r.from_symbol_id = child.symbol_id
          AND r.kind IN ('extends', 'implements')
     JOIN symbols base ON base.symbol_id = r.to_symbol_id
     WHERE child.name = ?1 AND child.language = 'swift'
     UNION
     SELECT pe.target_terminal_name FROM symbols child
     JOIN pending_relationships pe ON pe.from_symbol_id = child.symbol_id
          AND pe.kind IN ('extends', 'implements')
     WHERE child.name = ?1 AND child.language = 'swift'";
/// How many levels of Swift superclasses and protocols are read for a failable initializer.
const MAX_SWIFT_BASE_DEPTH: usize = 8;
/// Constraints in a Swift inheritance clause that add no initializer.
const SWIFT_PLAIN_CONSTRAINTS: [&str; 2] = ["AnyObject", "class"];
const CSHARP_MODIFIERS: [&str; 19] = [
    "public",
    "private",
    "protected",
    "internal",
    "static",
    "readonly",
    "const",
    "volatile",
    "required",
    "new",
    "override",
    "virtual",
    "abstract",
    "sealed",
    "unsafe",
    "extern",
    "fixed",
    "scoped",
    "partial",
];

/// The source text of each argument of the call of `name` written at `line` (from 1) and
/// `column` (from 0) of `source`, `None` when the argument list cannot be read.
pub(super) fn argument_texts(
    source: &str,
    line: usize,
    column: Option<usize>,
    name: &str,
) -> Option<Vec<String>> {
    let line_start: usize = source
        .split_inclusive('\n')
        .take(line.checked_sub(1)?)
        .map(str::len)
        .sum();
    let line_text = source[line_start..].lines().next()?;
    let at_column = column.filter(|&column| {
        line_text
            .get(column..)
            .is_some_and(|rest| rest.starts_with(name))
            && word_at(line_text, column, name)
    });
    let start = match at_column {
        Some(column) => column,
        None => {
            let found: Vec<usize> = line_text
                .match_indices(name)
                .map(|(at, _)| at)
                .filter(|&at| {
                    word_at(line_text, at, name) && opens_arguments(&line_text[at + name.len()..])
                })
                .collect();
            let [only] = found.as_slice() else {
                return None;
            };
            *only
        }
    };
    let after = &source[line_start + start + name.len()..];
    let open = skip_type_arguments(after)?;
    split_arguments(&after[open + 1..])
}

fn word_at(text: &str, at: usize, name: &str) -> bool {
    let before = text[..at].chars().next_back();
    let after = text[at + name.len()..].chars().next();
    before.is_none_or(|c| !is_word(c)) && after.is_none_or(|c| !is_word(c))
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn opens_arguments(after: &str) -> bool {
    skip_type_arguments(after).is_some()
}

/// The offset of the `(` that opens an argument list after a called name, past whitespace and
/// type arguments (`<int>`).
fn skip_type_arguments(after: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (at, c) in after.char_indices() {
        match c {
            '<' => depth += 1,
            '>' if depth > 0 => depth -= 1,
            '(' if depth == 0 => return Some(at),
            c if depth == 0 && !c.is_whitespace() => return None,
            _ => {}
        }
    }
    None
}

/// The top-level arguments of an argument list that starts after its `(`, up to the matching `)`.
fn split_arguments(text: &str) -> Option<Vec<String>> {
    let chars: Vec<char> = text.chars().collect();
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut angle = 0usize;
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        let next = chars.get(index + 1).copied();
        match c {
            '/' if next == Some('/') => {
                while index < chars.len() && chars[index] != '\n' {
                    index += 1;
                }
                continue;
            }
            '/' if next == Some('*') => {
                index += 2;
                while index + 1 < chars.len() && !(chars[index] == '*' && chars[index + 1] == '/') {
                    index += 1;
                }
                index += 2;
                continue;
            }
            '"' | '\'' => {
                let end = literal_end(&chars, index)?;
                current.extend(&chars[index..=end]);
                index = end + 1;
                continue;
            }
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth > 0 => depth -= 1,
            ')' if depth == 0 => {
                let last = current.trim().to_string();
                if !(last.is_empty() && arguments.is_empty()) {
                    arguments.push(last);
                }
                return Some(arguments);
            }
            '<' if current.chars().next_back().is_some_and(is_word) => angle += 1,
            '>' if angle > 0 && !matches!(current.chars().next_back(), Some('=' | '-')) => {
                angle -= 1;
            }
            ',' if depth == 0 && angle == 0 => {
                arguments.push(current.trim().to_string());
                current.clear();
                index += 1;
                continue;
            }
            _ => {}
        }
        current.push(c);
        index += 1;
    }
    None
}

/// The index of the quote that closes the string or character literal opening at `start`.
fn literal_end(chars: &[char], start: usize) -> Option<usize> {
    let quote = chars[start];
    let verbatim = quote == '"' && start > 0 && chars[start - 1] == '@';
    let triple =
        quote == '"' && chars.get(start + 1) == Some(&'"') && chars.get(start + 2) == Some(&'"');
    let mut index = start + if triple { 3 } else { 1 };
    while index < chars.len() {
        let c = chars[index];
        if triple {
            if chars[index..].starts_with(&['"', '"', '"']) {
                return Some(index + 2);
            }
        } else if c == '\\' && !verbatim {
            index += 1;
        } else if c == quote {
            if verbatim && chars.get(index + 1) == Some(&'"') {
                index += 1;
            } else {
                return Some(index);
            }
        } else if c == '\n' && !verbatim {
            return None;
        }
        index += 1;
    }
    None
}

/// Whether a Swift construction `T(…)` can make an optional value: `T`, a superclass it inherits
/// initializers from, or a protocol it conforms to (with a protocol extension) has a failable
/// `init?` or `init!`, or a base that the inheritance clause or an extension names is not a
/// project type, so its initializers are unknown.
fn swift_may_fail(conn: &Connection, type_name: &str) -> Result<bool, QueryError> {
    let mut seen: HashSet<String> = HashSet::from([type_name.to_string()]);
    let mut frontier = vec![type_name.to_string()];
    for _ in 0..MAX_SWIFT_BASE_DEPTH {
        let mut next = Vec::new();
        for name in &frontier {
            let failable: i64 = conn
                .prepare(SWIFT_FAILABLE_SQL)?
                .query_row(params![name], |row| row.get(0))?;
            if failable > 0 {
                return Ok(true);
            }
            let declarations: Vec<(String, String)> = conn
                .prepare(SWIFT_DECLARATIONS_SQL)?
                .query_map(params![name], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_, _>>()?;
            if declarations.iter().all(|(kind, _)| kind == "module") {
                return Ok(true);
            }
            let mut bases: Vec<String> = declarations
                .iter()
                .flat_map(|(_, signature)| swift_inherited(signature))
                .collect();
            bases.extend(
                conn.prepare(SWIFT_BASES_SQL)?
                    .query_map(params![name], |row| row.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?,
            );
            for base in bases {
                if !SWIFT_PLAIN_CONSTRAINTS.contains(&base.as_str()) && seen.insert(base.clone()) {
                    next.push(base);
                }
            }
        }
        if next.is_empty() {
            return Ok(false);
        }
        frontier = next;
    }
    Ok(true)
}

/// The types and protocols that the inheritance clause of a Swift declaration head names, by
/// their last name (`Base` and `Parsable` of `class Sub<T: P>: Lib.Base, Parsable where …`).
fn swift_inherited(signature: &str) -> Vec<String> {
    let head = signature.split('{').next().unwrap_or_default();
    let head = head.split(" where ").next().unwrap_or_default();
    let mut depth = 0usize;
    let mut clause = None;
    for (at, character) in head.char_indices() {
        match character {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth = depth.saturating_sub(1),
            ':' if depth == 0 => {
                clause = Some(&head[at + 1..]);
                break;
            }
            _ => {}
        }
    }
    let Some(clause) = clause else {
        return Vec::new();
    };
    let mut names = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    for character in clause.chars().chain([',']) {
        match character {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                let name = current.trim().rsplit('.').next().unwrap_or_default().trim();
                if !name.is_empty() {
                    names.push(name.to_string());
                }
                current.clear();
            }
            _ if depth == 0 => current.push(character),
            _ => {}
        }
    }
    names
}

/// Whether the argument text is a value that cannot be null.
pub(super) fn plain_value(
    conn: &Connection,
    dialect: Dialect,
    text: &str,
    caller: Option<&str>,
    source: &dyn Fn(&str) -> Option<Rc<String>>,
) -> Result<bool, QueryError> {
    let value = without_name(dialect, text);
    if is_literal(value) {
        return Ok(true);
    }
    if dialect == Dialect::CSharp
        && let Some(object) = value.strip_prefix("new ")
    {
        let head = object.trim_start();
        let type_name = head
            .split(['(', '{', '['])
            .next()
            .unwrap_or_default()
            .trim();
        return Ok(!type_name.is_empty() && !type_name.contains('?'));
    }
    if dialect == Dialect::Swift
        && let Some((type_name, rest)) = value.split_once('(')
        && is_identifier(type_name)
        && type_name.starts_with(char::is_uppercase)
        && rest.ends_with(')')
        && closes_at_end(rest)
    {
        let (types, others): (i64, i64) = conn
            .prepare(SWIFT_CONSTRUCTED_SQL)?
            .query_row(params![type_name], |row| Ok((row.get(0)?, row.get(1)?)))?;
        return Ok(types > 0 && others == 0 && !swift_may_fail(conn, type_name)?);
    }
    if !is_identifier(value) {
        return Ok(false);
    }
    let Some(caller) = caller else {
        return Ok(false);
    };
    let rows: Vec<(i64, String, String, i64, i64)> = conn
        .prepare(DECLARATIONS_SQL)?
        .query_map(params![caller, value], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })?
        .collect::<Result<_, _>>()?;
    let declarations: Vec<(i64, String)> = rows
        .into_iter()
        .map(|(depth, signature, path, start, end)| {
            let written = (dialect == Dialect::Swift)
                .then(|| {
                    let text = source(&path)?;
                    text.get(start as usize..end as usize).map(str::to_string)
                })
                .flatten();
            (depth, written.unwrap_or(signature))
        })
        .collect();
    let Some(nearest) = declarations.first().map(|(depth, _)| *depth) else {
        return Ok(false);
    };
    let generics: Vec<String> = conn
        .prepare(CHAIN_SIGNATURES_SQL)?
        .query_map(params![caller], |row| row.get::<_, String>(0))?
        .filter_map(Result::ok)
        .flat_map(|signature| super::overloads::type_parameters(&signature))
        .collect();
    Ok(declarations
        .iter()
        .filter(|(depth, _)| *depth == nearest)
        .all(|(_, signature)| {
            declared_type(dialect, signature, value).is_some_and(|declared| {
                !declared.ends_with(['?', '!'])
                    && !declared.starts_with("Nullable<")
                    && !declared.starts_with("System.Nullable<")
                    && !matches!(declared.as_str(), "var" | "dynamic")
                    && !generics.contains(&declared)
            })
        }))
}

/// Whether a text that starts after a `(` closes that `(` with its last character.
fn closes_at_end(rest: &str) -> bool {
    split_arguments(rest).is_some()
        && rest
            .char_indices()
            .scan(1i32, |depth, (at, c)| {
                match c {
                    '(' => *depth += 1,
                    ')' => *depth -= 1,
                    _ => {}
                }
                Some((at, *depth))
            })
            .find(|(_, depth)| *depth == 0)
            .is_some_and(|(at, _)| at + 1 == rest.len())
}

/// The argument without a C# or Swift `name:` or a Kotlin `name =` before it.
fn without_name(dialect: Dialect, text: &str) -> &str {
    let text = text.trim();
    let end = text.find(|c: char| !is_word(c)).unwrap_or(text.len());
    if end == 0 {
        return text;
    }
    let rest = text[end..].trim_start();
    let stripped = match dialect {
        Dialect::Kotlin => rest
            .strip_prefix('=')
            .filter(|after| !after.starts_with('=')),
        _ => rest
            .strip_prefix(':')
            .filter(|after| !after.starts_with(':')),
    };
    stripped.map_or(text, str::trim)
}

fn is_identifier(text: &str) -> bool {
    text.chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_')
        && text.chars().all(is_word)
}

/// A number, a character, a string, or a Boolean literal.
fn is_literal(text: &str) -> bool {
    if matches!(text, "true" | "false") {
        return true;
    }
    if (text.starts_with('"') && text.ends_with('"') && text.len() > 1)
        || (text.starts_with('\'') && text.ends_with('\'') && text.len() > 2)
    {
        return true;
    }
    let digits = text.strip_prefix(['-', '+']).unwrap_or(text);
    digits.starts_with(|c: char| c.is_ascii_digit())
        && digits
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.'))
        && digits.matches('.').count() <= 1
}

/// The declared type in the signature of a variable named `name`, or, in Swift, in its
/// declaration text, since julie's signature of a Swift property drops the `!` of `Int!`.
fn declared_type(dialect: Dialect, signature: &str, name: &str) -> Option<String> {
    match dialect {
        Dialect::Kotlin | Dialect::Swift => {
            let head = signature.split(['=', '{']).next().unwrap_or(signature);
            let (_, written) = head.split_once(':')?;
            let words: Vec<&str> = written
                .split_whitespace()
                .skip_while(|word| word.starts_with('@') || *word == "inout")
                .collect();
            (!words.is_empty()).then(|| words.join(" "))
        }
        Dialect::CSharp => {
            let head = signature
                .split(['=', '{', ';'])
                .next()
                .unwrap_or(signature)
                .trim();
            let (written, last) = head.rsplit_once(char::is_whitespace)?;
            if last != name {
                return None;
            }
            let words: Vec<&str> = written
                .split_whitespace()
                .skip_while(|word| CSHARP_MODIFIERS.contains(word))
                .collect();
            if words.is_empty()
                || words
                    .iter()
                    .any(|word| matches!(*word, "ref" | "out" | "in"))
            {
                return None;
            }
            Some(words.concat())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Dialect, argument_texts, declared_type, is_literal, swift_inherited, without_name,
    };

    #[test]
    fn a_swift_inheritance_clause_names_each_base_by_its_last_name() {
        assert_eq!(
            swift_inherited("class Sub<T: P>: Lib.Base, Parsable where T: Q"),
            ["Base", "Parsable"]
        );
        assert_eq!(
            swift_inherited("extension Parsable where Self: Codable"),
            Vec::<String>::new()
        );
        assert_eq!(swift_inherited("struct Box: Holder<Int>"), ["Holder"]);
        assert!(swift_inherited("struct Pt").is_empty());
    }

    #[test]
    fn the_arguments_of_a_call_are_its_top_level_texts() {
        let source = "a\n    Svc.Nul(5, new Dictionary<string, int>(), f(1, 2), \"x,)\")\n";
        assert_eq!(
            argument_texts(source, 2, Some(8), "Nul"),
            Some(vec![
                "5".to_string(),
                "new Dictionary<string, int>()".to_string(),
                "f(1, 2)".to_string(),
                "\"x,)\"".to_string(),
            ])
        );
        assert_eq!(argument_texts("Run()\n", 1, None, "Run"), Some(Vec::new()));
        assert_eq!(
            argument_texts("Go<int>(a,\n  b)\n", 1, Some(0), "Go"),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(argument_texts("f(a) + f(b)\n", 1, None, "f"), None);
    }

    #[test]
    fn a_literal_is_a_number_a_character_a_string_or_a_boolean() {
        for literal in ["5", "-1.5e3", "0x1F", "5L", "'c'", "\"s\"", "true"] {
            assert!(is_literal(literal), "{literal}");
        }
        for other in ["x", "null", "k + 1", "nil", "1.2.3"] {
            assert!(!is_literal(other), "{other}");
        }
    }

    #[test]
    fn a_named_argument_drops_its_name() {
        assert_eq!(without_name(Dialect::CSharp, "count: 5"), "5");
        assert_eq!(without_name(Dialect::CSharp, "a::b"), "a::b");
        assert_eq!(without_name(Dialect::Kotlin, "count = 5"), "5");
        assert_eq!(without_name(Dialect::Kotlin, "a == b"), "a == b");
        assert_eq!(without_name(Dialect::Swift, "x: v"), "v");
    }

    #[test]
    fn a_declared_type_comes_from_the_signature_of_the_variable() {
        assert_eq!(
            declared_type(Dialect::CSharp, "private readonly int? _x", "_x").as_deref(),
            Some("int?")
        );
        assert_eq!(
            declared_type(Dialect::CSharp, "int k = 3", "k").as_deref(),
            Some("int")
        );
        assert_eq!(
            declared_type(Dialect::CSharp, "public int X { get; set; }", "X").as_deref(),
            Some("int")
        );
        assert_eq!(declared_type(Dialect::CSharp, "out int x", "x"), None);
        assert_eq!(
            declared_type(Dialect::Kotlin, "val w: Int? = v", "w").as_deref(),
            Some("Int?")
        );
        assert_eq!(declared_type(Dialect::Swift, "let u", "u"), None);
        assert_eq!(
            declared_type(Dialect::Swift, "w: Int!", "w").as_deref(),
            Some("Int!")
        );
    }
}
