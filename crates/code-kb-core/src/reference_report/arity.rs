//! The number of arguments a target accepts, read from the parameter rows and the signature of its
//! definitions, and the exclusion of a call whose argument count no definition of the target
//! accepts. A target the index cannot describe exactly gets no range, so no call is excluded.

use std::fs;
use std::ops::{Range, RangeInclusive};
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use super::{Accounting, QueryError, sections};
use crate::models::Symbol;

/// Python decorators that keep the parameters of the function they wrap.
const SIGNATURE_KEEPING_DECORATORS: &[&str] = &[
    "staticmethod",
    "classmethod",
    "abstractmethod",
    "abc.abstractmethod",
    "override",
    "typing.override",
    "typing_extensions.override",
    "final",
    "typing.final",
    "overload",
    "typing.overload",
];

const PARAMETER_ROWS_SQL: &str =
    "SELECT symbols.name, COALESCE(symbols.start_byte, 0), symbols.signature FROM symbols
     JOIN symbols owner ON owner.symbol_id = symbols.parent_symbol_id
     WHERE symbols.parent_symbol_id = ?1
       AND (CASE WHEN json_valid(symbols.metadata_json)
                 THEN json_extract(symbols.metadata_json, '$.role') END = 'parameter'
            OR (owner.language IN ('kotlin', 'scala', 'csharp')
                AND owner.kind = 'class'
                AND symbols.kind = 'property'
                AND owner.signature IS NOT NULL
                AND COALESCE(symbols.start_byte, 0) >= COALESCE(owner.start_byte, 0)
                AND COALESCE(symbols.start_byte, 0)
                    < COALESCE(owner.start_byte, 0) + length(CAST(owner.signature AS BLOB))))
     ORDER BY symbols.start_line, symbols.start_column, symbols.symbol_id";

const CLASS_ATTRIBUTES_SQL: &str = "SELECT s.signature FROM symbols s
     JOIN symbols owner ON owner.symbol_id = s.parent_symbol_id
     WHERE s.name = ?1 AND s.language = 'python' AND owner.kind = 'class'
       AND s.kind IN ('variable', 'property', 'constant', 'field')";

/// The modifiers a C# parameter may carry before or after `this`.
const CSHARP_PARAMETER_MODIFIERS: [&str; 4] = ["ref", "in", "scoped", "readonly"];

/// Methods whose callers reach them through a base type with another parameter list: an override
/// may drop a default value, take an array for `params`, or take fewer parameters. Rust and Go
/// match a trait or interface method exactly.
const OVERRIDING_LANGUAGES: [&str; 5] = ["python", "java", "csharp", "kotlin", "typescript"];

/// The language whose arity rule judges a definition or a call: TSX counts as TypeScript.
fn rule_language(language: &str) -> Option<&'static str> {
    Some(match language {
        "python" => "python",
        "java" => "java",
        "csharp" => "csharp",
        "go" => "go",
        "rust" => "rust",
        "kotlin" => "kotlin",
        "c" => "c",
        "cpp" => "cpp",
        "typescript" | "tsx" => "typescript",
        "swift" => "swift",
        "php" => "php",
        "ruby" => "ruby",
        "scala" => "scala",
        "dart" => "dart",
        "fsharp" => "fsharp",
        "vbnet" => "vbnet",
        "r" => "r",
        "zig" => "zig",
        "gdscript" => "gdscript",
        "elixir" => "elixir",
        "erlang" => "erlang",
        _ => return None,
    })
}

/// How many arguments a call to one definition or a group of overloads may pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Arity {
    language: &'static str,
    min: u32,
    /// The most arguments a bound call passes, `None` when a parameter takes any number.
    max: Option<u32>,
    /// The most arguments any call passes: a call that names the owner may pass the receiver as
    /// its first argument (`Base.method(self, x)`, `Type::method(value, x)`), one more than `max`.
    accepted_max: Option<u32>,
}

impl Arity {
    fn union(self, other: Arity) -> Arity {
        let highest = |a: Option<u32>, b: Option<u32>| a.zip(b).map(|(a, b)| a.max(b));
        Arity {
            language: self.language,
            min: self.min.min(other.min),
            max: highest(self.max, other.max),
            accepted_max: highest(self.accepted_max, other.accepted_max),
        }
    }

    /// The text of the range a bound call to the definition satisfies.
    pub(crate) fn range_text(&self) -> String {
        match self.max {
            Some(max) if max == self.min => self.min.to_string(),
            Some(max) => format!("{}-{max}", self.min),
            None => format!("{} or more", self.min),
        }
    }

    /// Whether a call written in `site_language` with `count` arguments passes a number of
    /// arguments the target takes, `None` when the arity rule does not judge that language or
    /// when the call fits the target only under one of the counts it may pass. Unlike the callers
    /// report, a Go call with one argument is judged: `f(g())` may spread, but the edit check
    /// still names a call that passes one.
    pub(crate) fn accepts(&self, site_language: &str, count: u32) -> Option<bool> {
        if rule_language(site_language) != Some(self.language) {
            return None;
        }
        let fits = self
            .passed_counts(count)
            .filter(|&passed| self.fits(passed));
        match (fits.count(), self.passed_counts(count).count()) {
            (0, _) => Some(false),
            (fitting, all) if fitting == all => Some(true),
            _ => None,
        }
    }

    fn fits(&self, count: u32) -> bool {
        count >= self.min && self.accepted_max.is_none_or(|max| count <= max)
    }

    /// The argument counts that a call with `written` arguments between its parentheses may pass.
    /// An Elixir pipe (`x |> f(a)`) passes its left value as the first argument, and the written
    /// count leaves it out.
    fn passed_counts(&self, written: u32) -> RangeInclusive<u32> {
        written..=written.saturating_add(u32::from(self.language == "elixir"))
    }

    /// The basis of an exclusion when `count` arguments fit no definition of the target, `None`
    /// when a call with that count can reach one of them or the index cannot tell.
    pub(super) fn exclusion_basis(
        &self,
        name: &str,
        site_language: &str,
        count: u32,
    ) -> Option<String> {
        if rule_language(site_language) != Some(self.language) {
            return None;
        }
        // `f(g())` passes every value of a Go call that returns several.
        if self.passed_counts(count).any(|passed| self.fits(passed))
            || (self.language == "go" && count == 1 && count < self.min)
        {
            return None;
        }
        let noun = if count == 1 { "arg" } else { "args" };
        Some(format!(
            "{count} {noun}; `{name}` takes {}",
            self.range_text()
        ))
    }
}

/// The range of arguments the overloads in `group` accept together, `None` when one of them has no
/// exact range. `constructing` is set for the calls that build the class (`Foo(a)`), which never
/// pass the receiver.
pub(crate) fn group_arity(
    conn: &Connection,
    root: &Path,
    group: &[Symbol],
    constructing: bool,
) -> Result<Option<Arity>, QueryError> {
    group_arity_of(conn, root, group, constructing, false)
}

/// Like [`group_arity`] for overloads that a removal leaves behind or takes away, where the
/// constructors of a Java or Kotlin class have a range when every constructor of the class is in
/// the index.
pub(crate) fn declared_group_arity(
    conn: &Connection,
    root: &Path,
    group: &[Symbol],
) -> Result<Option<Arity>, QueryError> {
    group_arity_of(conn, root, group, false, true)
}

/// Whether the Java or Kotlin class of `constructor` has a constructor that the index lists no
/// symbol for: the canonical constructor of a record, a Kotlin primary constructor, or the
/// implicit constructor of a class that declares none.
pub(crate) fn has_unlisted_constructor(
    conn: &Connection,
    constructor: &Symbol,
) -> Result<bool, QueryError> {
    Ok(matches!(constructor.language.as_str(), "java" | "kotlin")
        && !declared_constructors_are_complete(conn, constructor, &constructor.language)?)
}

fn group_arity_of(
    conn: &Connection,
    root: &Path,
    group: &[Symbol],
    constructing: bool,
    declared: bool,
) -> Result<Option<Arity>, QueryError> {
    let mut union: Option<Arity> = None;
    for member in group {
        let Some(arity) = definition_arity(conn, root, member, constructing, declared)? else {
            return Ok(None);
        };
        union = Some(match union {
            Some(so_far) => so_far.union(arity),
            None => arity,
        });
    }
    if group
        .first()
        .is_some_and(|member| member.language == "erlang")
        && let Some(arity) = union.as_ref()
        && let Some(max) = arity.max
        && max - arity.min + 1 != group.len() as u32
    {
        return Ok(None);
    }
    Ok(union)
}

/// `own`, the range of the overloads of `target`, widened by the ranges of the methods of the
/// same name in every type that the owner of `target` extends or implements: a call through the
/// base type reaches the override, and the override may drop a default value, take an array for a
/// `params` parameter, or take fewer parameters. `None` when the walk to the base types was cut
/// short or ran out of time, or when the range of one of those methods is unknown.
pub(super) fn widened_by_ancestors(
    accounting: &mut Accounting,
    target: &Symbol,
    own: Arity,
) -> Result<Option<Arity>, QueryError> {
    if target.kind != "method" || !OVERRIDING_LANGUAGES.contains(&own.language) {
        return Ok(Some(own));
    }
    let Some(inherited) = sections::inherited_methods(accounting, target)? else {
        return Ok(None);
    };
    let (conn, root) = (accounting.conn, accounting.root);
    let mut range = own;
    for method in &inherited {
        let Some(arity) = definition_arity(conn, root, method, false, false)? else {
            return Ok(None);
        };
        range = range.union(arity);
    }
    Ok(Some(range))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Receiver {
    None,
    Bound,
    Passable,
}

struct Parameter {
    optional: bool,
    variadic: bool,
    positional: bool,
}

fn definition_arity(
    conn: &Connection,
    root: &Path,
    symbol: &Symbol,
    constructing: bool,
    declared: bool,
) -> Result<Option<Arity>, QueryError> {
    let Some(language) = rule_language(&symbol.language) else {
        return Ok(None);
    };
    let primary_constructor_class =
        symbol.kind == "class" && matches!(language, "csharp" | "fsharp" | "kotlin" | "scala");
    if !matches!(symbol.kind.as_str(), "function" | "method" | "constructor")
        && !primary_constructor_class
    {
        return Ok(None);
    }
    let Some(indexed_signature) = symbol.signature.as_deref() else {
        return Ok(None);
    };
    if symbol.kind == "constructor" {
        let complete = if declared {
            declared_constructors_are_complete(conn, symbol, language)?
        } else {
            constructor_is_complete(conn, symbol, language)?
        };
        if !complete {
            return Ok(None);
        }
    }
    let kotlin_constructor;
    let (symbol, name) = match (language, symbol.kind.as_str()) {
        // julie starts the body of a Kotlin secondary constructor at its parameter list, so the
        // body cut would drop every parameter row.
        ("kotlin", "constructor") => {
            kotlin_constructor = Symbol {
                body_start_byte: None,
                ..symbol.clone()
            };
            (&kotlin_constructor, "constructor")
        }
        _ => (symbol, symbol.name.as_str()),
    };
    let swift_signature;
    let signature = if language == "swift" {
        if symbol.body_start_byte.is_some() {
            let Some(source) = swift_parameter_source(root, symbol) else {
                return Ok(None);
            };
            swift_signature = source;
            swift_signature.as_str()
        } else {
            indexed_signature
        }
    } else {
        indexed_signature
    };
    let csharp_signature;
    let signature = if language == "csharp"
        && symbol.kind == "class"
        && parameter_list(signature, name, language).is_none()
    {
        let Some(source) = csharp_primary_constructor_source(root, symbol) else {
            return Ok(None);
        };
        csharp_signature = source;
        csharp_signature.as_str()
    } else {
        signature
    };
    let Some(list) = parameter_list(signature, name, language) else {
        return Ok(None);
    };
    if language == "c"
        && symbol.kind == "function"
        && list.trim().is_empty()
        && c_declaration_is_nonprototyped(conn, symbol)?
    {
        return Ok(None);
    }
    if language == "typescript" && !is_written_as_function(root, symbol) {
        return Ok(None);
    }
    let mut segments = formal_segments(list, language);
    let mut variadic_marker = false;
    segments.retain(|(segment, _, _)| match (language, segment.as_str()) {
        ("python", "*" | "/") => false,
        ("rust" | "c" | "cpp" | "r" | "zig", "...") => {
            variadic_marker = true;
            false
        }
        ("c" | "cpp", "void") => false,
        ("typescript", text) => !is_this_parameter(text),
        _ => true,
    });
    let php_last_required = (language == "php")
        .then(|| {
            segments
                .iter()
                .enumerate()
                .filter_map(|(index, (segment, _, _))| {
                    let parameter = classify(segment, language);
                    (!parameter.optional && !parameter.variadic).then_some(index)
                })
                .next_back()
        })
        .flatten();

    let mut names = parameter_names(conn, symbol, list)?;
    if matches!(language, "c" | "cpp") && list.trim() == "void" {
        names.clear();
    }
    if language == "go" && symbol.kind == "method" && names.len() == segments.len() + 1 {
        names.remove(0);
    }
    if names.len() != segments.len()
        || names
            .iter()
            .zip(&segments)
            .any(|(name, (segment, _, _))| !contains_parameter_name(segment, name, language))
    {
        return Ok(None);
    }

    let mut parameters: Vec<Parameter> = segments
        .iter()
        .enumerate()
        .map(|(index, (segment, optional_group, positional))| {
            let mut parameter = classify(segment, language);
            parameter.optional |= *optional_group;
            if php_last_required.is_some_and(|last| index < last) {
                parameter.optional = false;
            }
            parameter.positional = *positional;
            parameter
        })
        .collect();
    if variadic_marker {
        parameters.push(Parameter {
            optional: false,
            variadic: true,
            positional: true,
        });
    }
    let segment_refs: Vec<&str> = segments
        .iter()
        .map(|(segment, _, _)| segment.as_str())
        .collect();
    let Some(receiver) = receiver_of(conn, symbol, language, &segment_refs, constructing)? else {
        return Ok(None);
    };
    if receiver != Receiver::None && !parameters.is_empty() && is_receiver_slot(language, symbol) {
        parameters.remove(0);
    }
    let positional_parameters: Vec<&Parameter> = parameters
        .iter()
        .filter(|parameter| parameter.positional)
        .collect();
    let max = (language != "php"
        && !positional_parameters
            .iter()
            .any(|parameter| parameter.variadic))
    .then_some(positional_parameters.len() as u32);
    Ok(Some(Arity {
        language,
        min: positional_parameters
            .iter()
            .filter(|parameter| !parameter.optional && !parameter.variadic)
            .count() as u32,
        max,
        accepted_max: max.map(|max| max + u32::from(receiver == Receiver::Passable)),
    }))
}

fn c_declaration_is_nonprototyped(conn: &Connection, symbol: &Symbol) -> Result<bool, QueryError> {
    let is_definition: Option<String> = conn.query_row(
        "SELECT CASE WHEN json_valid(metadata_json)
                     THEN CAST(json_extract(metadata_json, '$.isDefinition') AS TEXT) END
         FROM symbols WHERE symbol_id = ?1",
        params![symbol.symbol_id],
        |row| row.get(0),
    )?;
    Ok(matches!(is_definition.as_deref(), Some("false" | "0")))
}

fn swift_parameter_source(root: &Path, symbol: &Symbol) -> Option<String> {
    let start = symbol.start_byte;
    let end = symbol.body_start_byte?;
    if end <= start {
        return None;
    }
    fs::read_to_string(root.join(&symbol.path))
        .ok()?
        .get(start..end)
        .map(str::to_string)
}

fn csharp_primary_constructor_source(root: &Path, symbol: &Symbol) -> Option<String> {
    let start = symbol.start_byte;
    let end = symbol.body_start_byte?;
    if end <= start {
        return None;
    }
    let source = fs::read_to_string(root.join(&symbol.path)).ok()?;
    let header = source.get(start..end)?.trim_end();
    parameter_list(header, &symbol.name, "csharp")?;
    Some(header.to_string())
}

/// Whether the first parameter row stands for a receiver that a bound call does not pass. A Go
/// receiver is no parameter of the signature, so nothing is removed.
fn is_receiver_slot(language: &str, symbol: &Symbol) -> bool {
    !(language == "go" && symbol.kind == "method")
}

/// The parameter names of `symbol` in the order of its parameter list `list`. Go parameters that
/// share one type (`a, b int`) have equal positions, so the order in the list decides.
fn parameter_names(
    conn: &Connection,
    symbol: &Symbol,
    list: &str,
) -> Result<Vec<String>, QueryError> {
    let mut rows = parameter_rows(conn, symbol)?;
    rows.sort_by_key(|row| (row.start, word_position(list, &row.name)));
    Ok(rows.into_iter().map(|row| row.name).collect())
}

/// One parameter row of a callable: its name, its start byte, and its own signature text (`b: i32`,
/// `int a`, `v, k int`, `b=1`).
pub(crate) struct ParameterRow {
    pub name: String,
    pub start: usize,
    pub signature: Option<String>,
}

/// The parameter rows of `symbol`, without the rows after its body starts (the parameters of a
/// lambda in the body).
pub(crate) fn parameter_rows(
    conn: &Connection,
    symbol: &Symbol,
) -> Result<Vec<ParameterRow>, QueryError> {
    let mut stmt = conn.prepare(PARAMETER_ROWS_SQL)?;
    let mut rows = stmt
        .query_map(params![symbol.symbol_id], |row| {
            Ok(ParameterRow {
                name: row.get(0)?,
                start: row.get::<_, i64>(1)? as usize,
                signature: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.retain(|row| {
        symbol.language == "powershell"
            || symbol.body_start_byte.is_none_or(|body| row.start < body)
    });
    rows.retain(|row| {
        symbol.language != "fsharp"
            || !row
                .signature
                .as_deref()
                .is_some_and(|signature| signature.contains("ParamArray"))
    });
    Ok(rows)
}

/// How many parameter rows `symbol` has.
pub(crate) fn parameter_count(conn: &Connection, symbol: &Symbol) -> Result<usize, QueryError> {
    Ok(parameter_rows(conn, symbol)?.len())
}

/// How a call reaches the first parameter of the definition: `None` for a definition whose
/// receiver the index cannot tell.
fn receiver_of(
    conn: &Connection,
    symbol: &Symbol,
    language: &str,
    segments: &[&str],
    constructing: bool,
) -> Result<Option<Receiver>, QueryError> {
    let first = segments.first().copied().unwrap_or("");
    Ok(Some(match language {
        "python" => {
            if symbol.kind == "function" && is_bound_as_class_attribute(conn, symbol)? {
                return Ok(None);
            }
            let decorators = python_decorators(conn, symbol)?;
            if decorators
                .iter()
                .any(|d| !SIGNATURE_KEEPING_DECORATORS.contains(&d.as_str()))
            {
                return Ok(None);
            }
            if decorators.iter().any(|d| d == "staticmethod") {
                return Ok(Some(Receiver::None));
            }
            let named_receiver = ["self", "cls"]
                .iter()
                .any(|receiver| starts_with_word(first, receiver));
            if named_receiver {
                if constructing || decorators.iter().any(|d| d == "classmethod") {
                    Receiver::Bound
                } else {
                    Receiver::Passable
                }
            } else if symbol.kind == "function" || first.starts_with('*') {
                Receiver::None
            } else {
                return Ok(None);
            }
        }
        "rust" if is_rust_self(first) => Receiver::Passable,
        "csharp" if symbol.kind == "method" && after_csharp_this(first).is_some() => {
            Receiver::Passable
        }
        "go" if symbol.kind == "method" => Receiver::Passable,
        _ => Receiver::None,
    }))
}

/// Whether a class body binds the function under its own name (`render = render`,
/// `wrapped = staticmethod(wrapped)`): a call through an instance then passes the instance as the
/// first argument, or runs a wrapper with another parameter list. An assignment through the
/// instance (`self.render = render`) binds nothing.
fn is_bound_as_class_attribute(conn: &Connection, function: &Symbol) -> Result<bool, QueryError> {
    let mut stmt = conn.prepare(CLASS_ATTRIBUTES_SQL)?;
    let signatures = stmt
        .query_map(params![function.name], |row| {
            row.get::<_, Option<String>>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(signatures.iter().flatten().any(|signature| {
        !signature.starts_with("self.")
            && signature
                .split_once('=')
                .is_some_and(|(_, value)| contains_word(value, &function.name))
    }))
}

fn python_decorators(conn: &Connection, symbol: &Symbol) -> Result<Vec<String>, QueryError> {
    let metadata: Option<String> = conn
        .query_row(
            "SELECT CASE WHEN json_valid(metadata_json) THEN metadata_json END
             FROM symbols WHERE symbol_id = ?1",
            params![symbol.symbol_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let decorators = metadata
        .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
        .and_then(|value| value.get("decorators").cloned())
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    Ok(decorators
        .iter()
        .filter_map(|decorator| decorator.as_str().map(str::to_string))
        .collect())
}

/// Whether the constructors in the index are all the constructors the class has. A C# class with
/// an explicit constructor has no implicit one, while a struct has a default constructor and a
/// primary constructor holds parameters outside the rows. A Kotlin primary constructor is no
/// symbol, and a Java record has an implicit canonical one.
fn constructor_is_complete(
    conn: &Connection,
    constructor: &Symbol,
    language: &str,
) -> Result<bool, QueryError> {
    match language {
        "python" | "typescript" => Ok(true),
        "csharp" => {
            let owner = match &constructor.parent_symbol_id {
                Some(owner_id) => crate::queries::get_symbol_by_id(conn, owner_id)?,
                None => None,
            };
            let Some(owner) = owner else {
                return Ok(false);
            };
            Ok(owner.kind == "class"
                && owner.signature.as_deref().is_some_and(|signature| {
                    parameter_list(signature, &owner.name, language).is_none()
                })
                && parameter_names(conn, &owner, "")?.is_empty())
        }
        _ => Ok(false),
    }
}

/// Whether the constructors in the index are all the constructors the class has, for the group of
/// a removal. A Java class that declares a constructor has no implicit one, and a Java record
/// has an implicit canonical one. A Kotlin class has no primary constructor when its header holds
/// no parenthesis: no parameter list and no call of a superclass.
fn declared_constructors_are_complete(
    conn: &Connection,
    constructor: &Symbol,
    language: &str,
) -> Result<bool, QueryError> {
    if !matches!(language, "java" | "kotlin") {
        return constructor_is_complete(conn, constructor, language);
    }
    let owner = match &constructor.parent_symbol_id {
        Some(owner_id) => crate::queries::get_symbol_by_id(conn, owner_id)?,
        None => None,
    };
    Ok(owner.is_some_and(|owner| {
        let header = owner.signature.as_deref().unwrap_or_default();
        owner.kind == "class"
            && match language {
                "java" => !contains_word(header, "record"),
                _ => !header.contains('('),
            }
    }))
}

/// Whether the source declares the symbol with its own parameter list (`name(a: number)`). julie
/// also gives a TypeScript property or constant that holds an arrow function a signature, read
/// from the arrow function, while the declared type (`onDone: (a: number) => void = () => {}`)
/// sets what a call may pass.
fn is_written_as_function(root: &Path, symbol: &Symbol) -> bool {
    const HEADER_LIMIT: usize = 2_000;
    let Ok(source) = std::fs::read(root.join(&symbol.path)) else {
        return false;
    };
    let end = symbol
        .body_start_byte
        .unwrap_or(symbol.end_byte)
        .min(symbol.start_byte + HEADER_LIMIT)
        .min(source.len());
    let Some(header) = source
        .get(symbol.start_byte..end)
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
    else {
        return false;
    };
    let mut scanner = Scanner::new("typescript");
    for (index, c) in header.char_indices() {
        if scanner.at_top_level()
            && is_word_char(c)
            && !header[..index].ends_with(is_word_char)
            && starts_with_word(&header[index..], &symbol.name)
        {
            return header[index + symbol.name.len()..]
                .trim_start()
                .starts_with(['(', '<']);
        }
        scanner.step(c, header[index + c.len_utf8()..].chars().next());
    }
    false
}

pub(crate) fn is_rust_self(segment: &str) -> bool {
    let mut text = segment.trim_start();
    text = text.strip_prefix('&').unwrap_or(text).trim_start();
    if let Some(rest) = text.strip_prefix('\'') {
        text = rest
            .trim_start_matches(|c: char| c.is_alphanumeric() || c == '_')
            .trim_start();
    }
    text = text.strip_prefix("mut ").unwrap_or(text).trim_start();
    starts_with_word(text, "self")
}

/// The text after the `this` keyword of the first parameter of a C# extension method, `None` when
/// the parameter has no `this`. Attribute groups and the modifiers `ref`, `in`, `scoped`, and
/// `readonly` may come before it (`[NotNull] this Lib l`, `ref this Big b`), and the modifiers
/// again after it (`this ref int x`).
pub(crate) fn after_csharp_this(parameter: &str) -> Option<&str> {
    let mut text = parameter.trim_start();
    while text.starts_with('[') {
        text = after_attribute_group(text)?.trim_start();
    }
    let rest = skip_csharp_modifiers(text).strip_prefix("this")?;
    rest.starts_with(char::is_whitespace)
        .then(|| skip_csharp_modifiers(rest))
}

fn skip_csharp_modifiers(mut text: &str) -> &str {
    text = text.trim_start();
    while let Some(modifier) = CSHARP_PARAMETER_MODIFIERS
        .iter()
        .find(|modifier| starts_with_word(text, modifier))
    {
        text = text[modifier.len()..].trim_start();
    }
    text
}

/// The text after the attribute group that `text` starts with (`[A, B(1)] rest`).
pub(crate) fn after_attribute_group(text: &str) -> Option<&str> {
    let mut scanner = Scanner::new("csharp");
    for (index, c) in text.char_indices() {
        scanner.step(c, text[index + c.len_utf8()..].chars().next());
        if c == ']' && scanner.depth == 0 && scanner.quote.is_none() {
            return Some(&text[index + 1..]);
        }
    }
    None
}

pub(crate) fn is_this_parameter(segment: &str) -> bool {
    segment == "this" || segment.starts_with("this:") || segment.starts_with("this ")
}

fn starts_with_word(text: &str, word: &str) -> bool {
    text.strip_prefix(word)
        .is_some_and(|rest| !rest.starts_with(is_word_char))
}

fn contains_word(text: &str, word: &str) -> bool {
    word_position(text, word) != usize::MAX
}

/// The offset of the first whole-word occurrence of `word` in `text`, `usize::MAX` when none.
fn word_position(text: &str, word: &str) -> usize {
    text.match_indices(word)
        .find(|(start, _)| {
            !text[..*start].ends_with(is_word_char)
                && !text[start + word.len()..].starts_with(is_word_char)
        })
        .map_or(usize::MAX, |(start, _)| start)
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

fn classify(segment: &str, language: &str) -> Parameter {
    let variadic = match language {
        "python" => segment.starts_with('*'),
        "typescript" => segment.starts_with("..."),
        "java" | "go" | "c" | "cpp" => segment.contains("..."),
        "csharp" => contains_word(segment, "params"),
        "kotlin" => contains_word(segment, "vararg"),
        "swift" | "gdscript" => segment.contains("..."),
        "php" => {
            let declaration = default_start(segment).map_or(segment, |at| &segment[..at]);
            declaration.contains("...")
        }
        "ruby" => segment.trim_start().starts_with('*') || segment.trim() == "...",
        "scala" => segment.trim_end().ends_with('*'),
        "fsharp" | "vbnet" => contains_word(segment, "ParamArray"),
        _ => false,
    };
    let optional = match language {
        "python" | "kotlin" | "cpp" | "php" | "scala" | "swift" | "gdscript" => {
            has_default(segment)
        }
        "ruby" => {
            has_default(segment)
                || ruby_keyword_default(segment)
                || segment.trim_start().starts_with('&')
        }
        "csharp" => has_default(segment) || contains_word(segment, "Optional"),
        "fsharp" => segment.trim_start().starts_with('?'),
        "vbnet" => has_default(segment) || contains_word(segment, "Optional"),
        "elixir" => segment.contains("\\\\"),
        "r" => true,
        "typescript" => {
            has_default(segment) || is_marked_optional(segment) || accepts_void(segment)
        }
        _ => false,
    };
    Parameter {
        optional,
        variadic,
        positional: true,
    }
}

pub(crate) fn parameter_properties(segment: &str, language: &str) -> Option<(bool, bool)> {
    let language = rule_language(language)?;
    let parameter = classify(segment, language);
    Some((parameter.optional, parameter.variadic))
}

fn ruby_keyword_default(segment: &str) -> bool {
    segment
        .split_once(':')
        .is_some_and(|(_, value)| !value.trim().is_empty())
}

fn contains_parameter_name(segment: &str, name: &str, language: &str) -> bool {
    if language == "php" {
        let variable = format!("${name}");
        return contains_word(segment, &variable);
    }
    contains_word(segment, name)
}

fn formal_segments(list: &str, language: &str) -> Vec<(String, bool, bool)> {
    let mut segments = Vec::new();
    for segment in split_top_level(list, language) {
        let segment = segment.trim();
        if language == "dart" {
            if let Some(inner) = segment
                .strip_prefix('{')
                .and_then(|text| text.strip_suffix('}'))
            {
                segments.extend(
                    split_top_level(inner, language)
                        .into_iter()
                        .map(|parameter| {
                            let parameter = parameter.trim();
                            (
                                parameter.to_string(),
                                !contains_word(parameter, "required"),
                                false,
                            )
                        }),
                );
                continue;
            }
            if let Some(inner) = segment
                .strip_prefix('[')
                .and_then(|text| text.strip_suffix(']'))
            {
                segments.extend(
                    split_top_level(inner, language)
                        .into_iter()
                        .map(|parameter| (parameter.trim().to_string(), true, true)),
                );
                continue;
            }
        }
        segments.push((segment.to_string(), false, true));
    }
    segments
}

/// Whether the type of a TypeScript parameter is `void` or a union with `void`: a call may leave
/// such a parameter out.
fn accepts_void(segment: &str) -> bool {
    let mut scanner = Scanner::new("typescript");
    for (index, c) in segment.char_indices() {
        if scanner.at_top_level() && c == ':' {
            return split_at_top_level(&segment[index + 1..], "typescript", '|').contains(&"void");
        }
        scanner.step(c, segment[index + c.len_utf8()..].chars().next());
    }
    false
}

/// Whether the name before the first `:` of a TypeScript parameter ends with `?`.
fn is_marked_optional(segment: &str) -> bool {
    let mut scanner = Scanner::new("typescript");
    for (index, c) in segment.char_indices() {
        if scanner.at_top_level() && c == ':' {
            return segment[..index].trim_end().ends_with('?');
        }
        scanner.step(c, segment[index + c.len_utf8()..].chars().next());
    }
    segment.trim_end().ends_with('?')
}

/// Whether a parameter has a default value.
fn has_default(segment: &str) -> bool {
    default_start(segment).is_some()
}

/// The byte of the `=` that starts the default value of a parameter or field: an `=` outside
/// brackets and quotes that is no part of `==`, `=>`, `<=`, `>=`, or `!=`.
pub(crate) fn default_start(segment: &str) -> Option<usize> {
    let mut scanner = Scanner::new("");
    let mut previous = None;
    for (index, c) in segment.char_indices() {
        let next = segment[index + c.len_utf8()..].chars().next();
        if scanner.at_top_level()
            && c == '='
            && !matches!(next, Some('=' | '>'))
            && !matches!(previous, Some('=' | '!' | '<' | '>'))
        {
            return Some(index);
        }
        scanner.step(c, next);
        previous = Some(c);
    }
    None
}

/// The byte range of the parameter list inside `signature` (between the parentheses) and its
/// top-level pieces, `None` when the signature has no list after the name.
pub(crate) fn parameter_segments<'a>(
    signature: &'a str,
    name: &str,
    language: &str,
) -> Option<(Range<usize>, Vec<&'a str>)> {
    let language = rule_language(language).unwrap_or(language);
    let list = parameter_list(signature, name, language)?;
    let start = list.as_ptr() as usize - signature.as_ptr() as usize;
    Some((start..start + list.len(), split_top_level(list, language)))
}

/// The text inside the first parenthesis group that follows `name` (and an optional list of
/// generic parameters) at the top level of `signature`: decorators, annotations, and attributes
/// before the name sit in brackets or are skipped as words.
fn parameter_list<'a>(signature: &'a str, name: &str, language: &str) -> Option<&'a str> {
    if language == "r" {
        let start = signature.find("function")? + "function".len();
        return list_after(&signature[start..], language);
    }
    if language == "erlang" {
        let start = signature.find(name)? + name.len();
        let after = signature[start..].trim_start().strip_prefix('/')?;
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        return list_after(&after[digits..], language);
    }
    let mut scanner = Scanner::new(language);
    for (index, c) in signature.char_indices() {
        if scanner.at_top_level()
            && is_word_char(c)
            && !signature[..index].ends_with(is_word_char)
            && starts_with_word(&signature[index..], name)
            && let Some(list) = list_after(&signature[index + name.len()..], language)
        {
            return Some(list);
        }
        scanner.step(c, signature[index + c.len_utf8()..].chars().next());
    }
    None
}

/// The text inside the parentheses at the start of `text`, after blanks and an optional
/// generic parameter list.
fn list_after<'a>(text: &'a str, language: &str) -> Option<&'a str> {
    let mut rest = text.trim_start();
    let (open, close) = if language == "go" {
        ('[', ']')
    } else {
        ('<', '>')
    };
    if rest.starts_with(open) {
        let mut depth = 0;
        let mut end = None;
        let mut previous = ' ';
        for (index, c) in rest.char_indices() {
            if c == open {
                depth += 1;
            } else if c == close && !(close == '>' && matches!(previous, '-' | '=')) {
                depth -= 1;
                if depth == 0 {
                    end = Some(index + c.len_utf8());
                    break;
                }
            }
            previous = c;
        }
        rest = rest[end?..].trim_start();
    }
    let body = rest.strip_prefix('(')?;
    let mut scanner = Scanner::new(language);
    for (index, c) in body.char_indices() {
        if scanner.depth == 0 && scanner.quote.is_none() && c == ')' {
            return Some(&body[..index]);
        }
        scanner.step(c, body[index + c.len_utf8()..].chars().next());
    }
    None
}

/// `list` cut at its top-level commas; empty pieces (a trailing comma) are dropped.
fn split_top_level<'a>(list: &'a str, language: &str) -> Vec<&'a str> {
    split_at_top_level(list, language, ',')
}

fn split_at_top_level<'a>(text: &'a str, language: &str, separator: char) -> Vec<&'a str> {
    let mut scanner = Scanner::new(language);
    let mut pieces = Vec::new();
    let mut start = 0;
    for (index, c) in text.char_indices() {
        if c == separator && scanner.at_top_level() {
            pieces.push(text[start..index].trim());
            start = index + 1;
        }
        scanner.step(c, text[index + c.len_utf8()..].chars().next());
    }
    pieces.push(text[start..].trim());
    pieces.retain(|piece| !piece.is_empty());
    pieces
}

/// Tracks brackets, angle brackets of generics, and quotes while text is read left to right.
struct Scanner {
    depth: i32,
    angle: i32,
    quote: Option<char>,
    escaped: bool,
    previous: Option<char>,
    quotes_apostrophe: bool,
    tracks_angles: bool,
}

impl Scanner {
    fn new(language: &str) -> Self {
        Scanner {
            depth: 0,
            angle: 0,
            quote: None,
            escaped: false,
            previous: None,
            quotes_apostrophe: language != "rust",
            tracks_angles: language != "go",
        }
    }

    /// Whether `c` stands outside every bracket, angle list, and quote.
    fn at_top_level(&self) -> bool {
        self.depth == 0 && self.angle == 0 && self.quote.is_none()
    }

    fn step(&mut self, c: char, next: Option<char>) {
        if let Some(quote) = self.quote {
            if self.escaped {
                self.escaped = false;
            } else if c == '\\' {
                self.escaped = true;
            } else if c == quote {
                self.quote = None;
            }
        } else {
            match c {
                '"' | '`' => self.quote = Some(c),
                '\'' if self.quotes_apostrophe => self.quote = Some(c),
                '(' | '[' | '{' => self.depth += 1,
                ')' | ']' | '}' => self.depth = (self.depth - 1).max(0),
                '<' if self.tracks_angles && !matches!(next, Some('=' | '<')) => self.angle += 1,
                '>' if self.tracks_angles
                    && !matches!(self.previous, Some('-' | '='))
                    && !matches!(next, Some('=')) =>
                {
                    self.angle = (self.angle - 1).max(0);
                }
                _ => {}
            }
        }
        self.previous = Some(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segments(signature: &str, name: &str, language: &str) -> Option<Vec<String>> {
        parameter_list(signature, name, language).map(|list| {
            split_top_level(list, language)
                .into_iter()
                .map(str::to_string)
                .collect()
        })
    }

    #[test]
    fn the_parameter_list_follows_the_name_and_skips_decorators_and_generics() {
        assert_eq!(
            segments("@wraps(f(1)) def f(a, b=1)", "f", "python").unwrap(),
            ["a", "b=1"]
        );
        assert_eq!(
            segments(
                "<T extends Fn> void gen(T t, final int[] arr)",
                "gen",
                "java"
            )
            .unwrap(),
            ["T t", "final int[] arr"]
        );
        assert_eq!(
            segments(
                "func Map[T, U any](xs []T, f func(T, int) U) []U",
                "Map",
                "go"
            )
            .unwrap(),
            ["xs []T", "f func(T, int) U"]
        );
        assert_eq!(
            segments(
                "pub fn f<T: Fn(i32) -> i32>(a: T, b: &str) -> i32",
                "f",
                "rust"
            )
            .unwrap(),
            ["a: T", "b: &str"]
        );
        assert_eq!(
            segments(
                "fun String.ext(a: Int, b: Map<String, Int> = mapOf()): Int",
                "ext",
                "kotlin"
            )
            .unwrap(),
            ["a: Int", "b: Map<String, Int> = mapOf()"]
        );
    }

    #[test]
    fn a_signature_without_a_parameter_list_after_the_name_has_no_list() {
        assert_eq!(segments("const f = (a, b) => a", "f", "typescript"), None);
        assert_eq!(segments("public class A", "A", "java"), None);
        assert!(segments("public class A(int x)", "A", "csharp").is_some());
    }

    #[test]
    fn quotes_and_brackets_do_not_split_a_parameter() {
        assert_eq!(
            segments(
                "def f(a, sep=\",\", b=(1, 2), c={'k': [1, 2]})",
                "f",
                "python"
            )
            .unwrap(),
            ["a", "sep=\",\"", "b=(1, 2)", "c={'k': [1, 2]}"]
        );
    }

    #[test]
    fn defaults_are_equals_signs_outside_types_and_brackets() {
        assert!(has_default("b: str = \"x\""));
        assert!(has_default("z = 3"));
        assert!(!has_default("cb: () => void"));
        assert!(!has_default("a: Dict[str, int]"));
        assert!(!has_default("m: Map<String, Int>"));
        assert!(is_marked_optional("b?: string"));
        assert!(!is_marked_optional("b: string | undefined"));
    }

    #[test]
    fn a_csharp_this_parameter_may_follow_attribute_groups_and_modifiers() {
        assert_eq!(after_csharp_this("this Lib l"), Some("Lib l"));
        assert_eq!(after_csharp_this("[NotNull] this Lib l"), Some("Lib l"));
        assert_eq!(
            after_csharp_this("[A(\"]\")][B] ref readonly this Big b"),
            Some("Big b")
        );
        assert_eq!(after_csharp_this("this ref int x"), Some("int x"));
        assert_eq!(after_csharp_this("int thisValue"), None);
        assert_eq!(after_csharp_this("[A] int a"), None);
    }

    #[test]
    fn a_typescript_type_accepts_void_only_as_a_whole_union_member() {
        assert!(accepts_void("x: void"));
        assert!(accepts_void("x: number | void"));
        assert!(!accepts_void("cb: () => void"));
        assert!(!accepts_void("x: Array<void>"));
        assert!(!accepts_void("x: number"));
    }

    #[test]
    fn a_range_text_names_one_count_a_span_or_an_open_end() {
        let range = |min, max| Arity {
            language: "python",
            min,
            max,
            accepted_max: max,
        };
        assert_eq!(range(1, Some(1)).range_text(), "1");
        assert_eq!(range(1, Some(2)).range_text(), "1-2");
        assert_eq!(range(1, None).range_text(), "1 or more");
    }

    #[test]
    fn an_elixir_count_may_pass_one_more_argument_through_a_pipe() {
        let range = |min, max| Arity {
            language: "elixir",
            min,
            max,
            accepted_max: max,
        };
        assert_eq!(range(2, Some(3)).accepts("elixir", 1), None);
        assert_eq!(range(2, Some(3)).accepts("elixir", 2), Some(true));
        assert_eq!(range(2, Some(3)).accepts("elixir", 3), None);
        assert_eq!(range(2, Some(3)).accepts("elixir", 0), Some(false));
        assert_eq!(range(2, Some(3)).accepts("elixir", 4), Some(false));
        assert_eq!(range(2, None).accepts("elixir", 5), Some(true));
        assert_eq!(range(2, Some(3)).exclusion_basis("f", "elixir", 1), None);
        assert_eq!(
            range(2, Some(3))
                .exclusion_basis("f", "elixir", 4)
                .as_deref(),
            Some("4 args; `f` takes 2-3")
        );
        assert_eq!(
            Arity {
                language: "python",
                min: 2,
                max: Some(3),
                accepted_max: Some(3),
            }
            .accepts("python", 1),
            Some(false)
        );
    }

    #[test]
    fn a_ruby_block_parameter_is_optional() {
        let block = classify("&block", "ruby");
        assert!(block.optional && !block.variadic);
        assert!(!classify("name:", "ruby").optional);
        assert!(classify("opt: 1", "ruby").optional);
        assert!(classify("**opts", "ruby").variadic);
    }

    #[test]
    fn a_count_inside_the_range_or_a_go_multi_value_call_is_not_excluded() {
        let range = |language, min, max, receiver_passable| Arity {
            language,
            min,
            max,
            accepted_max: max.map(|max| max + u32::from(receiver_passable)),
        };
        let python = range("python", 1, Some(2), false);
        assert_eq!(
            python.exclusion_basis("f", "python", 3).as_deref(),
            Some("3 args; `f` takes 1-2")
        );
        assert_eq!(
            python.exclusion_basis("f", "python", 0).as_deref(),
            Some("0 args; `f` takes 1-2")
        );
        assert_eq!(python.exclusion_basis("f", "python", 2), None);
        assert_eq!(python.exclusion_basis("f", "javascript", 9), None);
        assert_eq!(
            range("python", 1, Some(1), false)
                .exclusion_basis("f", "python", 2)
                .as_deref(),
            Some("2 args; `f` takes 1")
        );
        assert_eq!(
            range("python", 1, Some(1), true).exclusion_basis("f", "python", 2),
            None
        );
        assert_eq!(
            range("go", 2, Some(2), false).exclusion_basis("f", "go", 1),
            None
        );
        assert_eq!(
            range("kotlin", 2, Some(2), false)
                .exclusion_basis("f", "kotlin", 1)
                .as_deref(),
            Some("1 arg; `f` takes 2")
        );
    }
}
