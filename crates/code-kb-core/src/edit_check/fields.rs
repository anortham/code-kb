//! Construction sites whose validity depends on a type's fields or synthesized initializer.

use super::{MAX_EXTENT_LINES, word_position};
use crate::models::Symbol;
use crate::queries::QueryError;
use crate::reference_report::arity;
use crate::reference_report::variant_names::{code_only, is_identifier_char};
use rusqlite::{Connection, OptionalExtension, params};

/// Words after which a name stands in a type position and builds nothing.
const TYPE_POSITION_WORDS: [&str; 8] = [
    "impl", "for", "struct", "enum", "union", "trait", "type", "dyn",
];

/// The fields a type gained and lost.
pub(super) struct FieldChange {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// An added field has no default value according to its extracted declaration.
    pub required: bool,
    pub dataclass_constructor: Option<DataclassConstructorChange>,
    /// The number of components a Java record had before the change, when its signature says.
    pub old_components: Option<usize>,
}

pub(super) struct DataclassConstructorChange {
    pub removed: Vec<String>,
    pub required: bool,
}

#[derive(Clone, Copy)]
enum Owner {
    RustStruct,
    GoStruct,
    ZigStruct,
    FSharpRecord,
    Dataclass,
    JavaRecord,
    KotlinDataClass,
    ScalaCaseClass,
    CSharpRecord,
    SwiftStruct,
    CAggregate,
    CppAggregate,
    ElixirStruct,
    RubyStruct,
}

struct IndexedConstructor {
    name: String,
    signature: String,
}

struct OwnerContext<'a> {
    owner: Owner,
    conn: Option<&'a Connection>,
    owner_lines: Option<&'a [String]>,
    constructors: &'a [IndexedConstructor],
    aggregate_fields: Option<usize>,
    required_added_fields: Option<&'a std::collections::HashSet<String>>,
}

struct PrimaryParameter {
    name: String,
    has_default: bool,
}

pub(super) struct WrittenArguments {
    pub(super) positional_count: usize,
    pub(super) named_arguments: Vec<String>,
    pub(super) has_unknown_spread: bool,
}

fn indexed_constructors(
    conn: &Connection,
    subject: &Symbol,
) -> Result<Vec<IndexedConstructor>, QueryError> {
    let mut statement = conn.prepare(
        "SELECT name, COALESCE(signature, '') FROM symbols
         WHERE parent_symbol_id = ?1
           AND (kind = 'constructor'
                OR (language = 'python' AND name = '__init__' AND kind IN ('method', 'function'))
                OR (language = 'ruby' AND name = 'initialize' AND kind IN ('method', 'function')))
         ORDER BY start_byte",
    )?;
    let rows = statement.query_map(params![subject.symbol_id], |row| {
        Ok(IndexedConstructor {
            name: row.get(0)?,
            signature: row.get(1)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn indexed_required_added_fields(
    conn: &Connection,
    subject: &Symbol,
    owner_lines: Option<&[String]>,
    added: &[String],
) -> Result<Option<std::collections::HashSet<String>>, QueryError> {
    let mut statement = conn.prepare(
        "SELECT name FROM symbols
         WHERE parent_symbol_id = ?1 AND kind IN ('field', 'property')",
    )?;
    let rows = statement.query_map(params![subject.symbol_id], |row| row.get::<_, String>(0))?;
    let mut found = std::collections::HashSet::new();
    for row in rows {
        let name = row?;
        if added.contains(&name) && !found.insert(name) {
            return Ok(None);
        }
    }
    if found.len() != added.len() {
        return Ok(None);
    }
    let Some(owner_lines) = owner_lines else {
        return Ok(None);
    };
    let code = code_from(owner_lines, subject.start_line, "zig");
    let Some(at) = word_starts(&code, &subject.name).first().copied() else {
        return Ok(None);
    };
    let Some(after_name) = code.get(at + subject.name.len()..) else {
        return Ok(None);
    };
    let Some(struct_at) = after_name.find("struct") else {
        return Ok(None);
    };
    let Some(opening) = after_name[struct_at + "struct".len()..].find('{') else {
        return Ok(None);
    };
    let opening = struct_at + "struct".len() + opening;
    if matching_brace(after_name, opening).is_none() {
        return Ok(None);
    }
    let (fields, unclosed) = split_body_on(&after_name[opening + 1..], &[',', ';']);
    if unclosed {
        return Ok(None);
    }
    let mut source_defaults = std::collections::HashMap::new();
    for field in fields {
        let Some(colon) = top_level_colon(&field) else {
            continue;
        };
        let name = field[..colon].trim();
        if !name.is_empty()
            && name.chars().all(is_identifier_char)
            && source_defaults
                .insert(
                    name.to_string(),
                    arity::default_start(&code_only(&field, "zig")).is_some(),
                )
                .is_some()
        {
            return Ok(None);
        }
    }
    if added.iter().any(|name| !source_defaults.contains_key(name))
        || added.iter().any(|name| !found.contains(name))
    {
        return Ok(None);
    }
    Ok(Some(
        source_defaults
            .into_iter()
            .filter_map(|(name, has_default)| {
                (added.contains(&name) && !has_default).then_some(name)
            })
            .collect(),
    ))
}

fn indexed_aggregate_field_count(
    conn: &Connection,
    subject: &Symbol,
) -> Result<Option<usize>, QueryError> {
    let count = conn.query_row(
        "SELECT COUNT(*) FROM symbols WHERE parent_symbol_id = ?1 AND kind = 'field'",
        params![subject.symbol_id],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(usize::try_from(count).ok())
}

fn cpp_aggregate_is_proven(subject: &Symbol, owner_lines: Option<&[String]>) -> bool {
    if subject.kind != "struct" {
        return false;
    }
    let Some(owner_lines) = owner_lines else {
        return false;
    };
    let code = code_from(owner_lines, subject.start_line, "cpp");
    let Some(at) = word_starts(&code, &subject.name).first().copied() else {
        return false;
    };
    let Some(after_name) = code.get(at + subject.name.len()..) else {
        return false;
    };
    let Some(opening) = after_name.find('{') else {
        return false;
    };
    if after_name[..opening].trim_start().starts_with(':') {
        return false;
    }
    let body_start = opening + 1;
    let Some(body_end) = matching_brace(after_name, opening) else {
        return false;
    };
    let body = &after_name[body_start..body_end];
    !cpp_has_access_section(body, "private")
        && !cpp_has_access_section(body, "protected")
        && word_starts(body, "virtual").is_empty()
        && word_starts(body, &subject.name).is_empty()
        && !anonymous_aggregate(body)
}

fn ruby_keyword_struct_is_proven(subject: &Symbol, owner_lines: Option<&[String]>) -> bool {
    let Some(owner_lines) = owner_lines else {
        return false;
    };
    let code = code_from(owner_lines, subject.start_line, "ruby");
    let Some(first_line) = code.lines().next() else {
        return false;
    };
    word_starts(first_line, &subject.name)
        .into_iter()
        .any(|at| {
            let Some(after_name) = first_line.get(at + subject.name.len()..) else {
                return false;
            };
            let declaration = after_name.trim_start();
            let Some(declaration) = declaration.strip_prefix('=') else {
                return false;
            };
            let declaration = declaration.trim_start();
            let Some(arguments) = declaration.strip_prefix("Struct.new") else {
                return false;
            };
            let arguments = arguments.trim_start();
            if !arguments.starts_with('(') {
                return false;
            }
            let Some(after) = after_balanced(arguments, '(', ')') else {
                return false;
            };
            let close = arguments.len() - after.len() - 1;
            let inner = &arguments[1..close];
            argument_ranges(inner).into_iter().any(|(start, end)| {
                inner[start..end]
                    .split_once(':')
                    .is_some_and(|(name, value)| {
                        name.trim() == "keyword_init" && value.trim() == "true"
                    })
            })
        })
}

fn elixir_enforced_added_fields(
    owner_lines: Option<&[String]>,
    subject: &Symbol,
    added: &[String],
) -> Option<std::collections::HashSet<String>> {
    let owner_lines = owner_lines?;
    let source_start = subject.start_line.saturating_sub(MAX_EXTENT_LINES).max(1);
    let code = code_from(owner_lines, source_start, "elixir");
    let lines: Vec<_> = code.lines().collect();
    let owner_at = subject.start_line.checked_sub(source_start)?;
    let module_at = lines
        .get(..=owner_at)?
        .iter()
        .rposition(|line| line.trim_start().starts_with("defmodule "))?;
    let module_name = lines[module_at]
        .trim()
        .strip_prefix("defmodule ")?
        .strip_suffix(" do")?
        .trim();
    if module_name != subject.name {
        return None;
    }
    let module_end = lines
        .iter()
        .enumerate()
        .skip(module_at + 1)
        .find(|(_, line)| line.trim_start().starts_with("defmodule "))
        .map_or(lines.len(), |(at, _)| at);
    let defstruct_at = lines
        .iter()
        .enumerate()
        .take(module_end)
        .skip(module_at + 1)
        .find(|(_, line)| line.trim_start().starts_with("defstruct "))
        .map(|(at, _)| at)?;
    let mut enforced = None;
    for line in &lines[module_at + 1..defstruct_at] {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(value) = line.strip_prefix("@enforce_keys") {
            let value = value.trim();
            let list = value.strip_prefix('[')?.strip_suffix(']')?;
            let mut keys = std::collections::HashSet::new();
            for item in list
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
            {
                let name = item.strip_prefix(':')?;
                if name.is_empty()
                    || !name.chars().all(is_identifier_char)
                    || !keys.insert(name.to_string())
                {
                    return None;
                }
            }
            enforced = Some(keys);
        } else if !line.starts_with('@') {
            return None;
        }
    }
    let enforced = enforced.unwrap_or_default();
    Some(
        added
            .iter()
            .filter(|name| enforced.contains(*name))
            .cloned()
            .collect(),
    )
}

fn elixir_literal_is_unambiguous_build(lines: &[String], line: usize, name: &str) -> bool {
    let Some(first) = line.checked_sub(1).and_then(|at| lines.get(at)) else {
        return false;
    };
    let code = code_from(lines, line, "elixir");
    let first_line = &code[..first.len().min(code.len())];
    let spellings =
        std::iter::once(name).chain(name.rsplit('.').next().filter(|last| *last != name));
    spellings.into_iter().any(|spelling| {
        word_starts(first_line, spelling).into_iter().any(|at| {
            let Some(before_name) = first_line.get(..at).map(str::trim_end) else {
                return false;
            };
            let Some(prefix) = before_name.strip_suffix('%').map(str::trim_end) else {
                return false;
            };
            if !(prefix.ends_with("do:") || (prefix.ends_with('=') && !prefix.ends_with("=="))) {
                return false;
            }
            let Some(after_name) = first_line.get(at + spelling.len()..) else {
                return false;
            };
            let Some(open_relative) = after_name.find('{') else {
                return false;
            };
            if !after_name[..open_relative].trim().is_empty() {
                return false;
            }
            let open = at + spelling.len() + open_relative;
            let Some(close) = matching_brace(&code, open) else {
                return false;
            };
            let suffix = code[close + 1..].trim_start();
            !(suffix.starts_with("->")
                || suffix.starts_with("<-")
                || (suffix.starts_with('=') && !suffix.starts_with("==")))
        })
    })
}

fn matching_brace(text: &str, opening: usize) -> Option<usize> {
    let mut depth = 1usize;
    for (at, character) in text[opening + 1..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(opening + 1 + at);
                }
            }
            _ => {}
        }
    }
    None
}

fn cpp_has_access_section(body: &str, access: &str) -> bool {
    word_starts(body, access)
        .into_iter()
        .any(|at| body[at + access.len()..].trim_start().starts_with(':'))
}

fn anonymous_aggregate(body: &str) -> bool {
    ["struct", "union"].into_iter().any(|kind| {
        word_starts(body, kind)
            .into_iter()
            .any(|at| body[at + kind.len()..].trim_start().starts_with('{'))
    })
}

fn custom_constructor_may_accept(
    constructors: &[IndexedConstructor],
    subject: &Symbol,
    lines: Option<&[String]>,
    line: usize,
) -> bool {
    if constructors.is_empty() {
        return false;
    }
    let Some(lines) = lines else {
        return true;
    };
    let counts = call_counts(lines, line, &subject.name);
    if counts.is_empty() {
        return true;
    }
    let language = subject.language.as_str();
    for constructor in constructors {
        let name = match language {
            "kotlin" => "constructor",
            "scala" => "this",
            "swift" => "init",
            "python" => "__init__",
            _ => constructor.name.as_str(),
        };
        let Some((minimum, maximum)) = constructor_arity(&constructor.signature, name, language)
        else {
            return true;
        };
        if counts
            .iter()
            .any(|count| *count >= minimum && *count <= maximum)
        {
            return true;
        }
    }
    false
}

fn constructor_arity(signature: &str, name: &str, language: &str) -> Option<(usize, usize)> {
    let (_, mut segments) = arity::parameter_segments(signature, name, language)?;
    if language == "python"
        && segments
            .first()
            .is_some_and(|segment| is_python_receiver(segment))
    {
        segments.remove(0);
    }
    if segments.iter().any(|segment| {
        segment.starts_with('*')
            || segment.contains("...")
            || segment
                .split_whitespace()
                .any(|word| word == "params" || word == "ParamArray")
    }) {
        return None;
    }
    if segments.iter().enumerate().any(|(index, segment)| {
        arity::default_start(segment).is_some()
            && segments[index + 1..]
                .iter()
                .any(|later| arity::default_start(later).is_none())
    }) {
        return None;
    }
    let minimum = segments
        .iter()
        .filter(|segment| arity::default_start(segment).is_none())
        .count();
    Some((minimum, segments.len()))
}

fn is_python_receiver(segment: &str) -> bool {
    segment
        .split(':')
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .last()
        .is_some_and(|name| matches!(name, "self" | "cls"))
}

fn primary_parameter_name<'a>(segment: &'a str, language: &str) -> Option<&'a str> {
    let signature = segment.get(..arity::default_start(segment).unwrap_or(segment.len()))?;
    match language {
        "kotlin" | "scala" => signature.split(':').next()?.split_whitespace().last(),
        "csharp" => signature.split_whitespace().last(),
        _ => None,
    }
    .map(|name| name.trim_matches(|character: char| !is_identifier_char(character)))
    .filter(|name| !name.is_empty())
}

fn call_argument_names(lines: &[String], line: usize, name: &str, language: &str) -> Vec<String> {
    let Some(first) = line.checked_sub(1).and_then(|index| lines.get(index)) else {
        return Vec::new();
    };
    let text = source_from(lines, line);
    let code = code_only(&text, language);
    let first_line = &code[..first.len().min(code.len())];
    let mut names = Vec::new();
    for at in word_starts(first_line, name) {
        if let Some(arguments) = written_arguments_at_offset(&text, &code, at, name, language) {
            names.extend(arguments.named_arguments);
        }
    }
    names
}

fn swift_property_needs_argument(
    conn: &Connection,
    owner_lines: &[String],
    subject: &Symbol,
    field: &str,
) -> Result<Option<bool>, QueryError> {
    let location = conn
        .query_row(
            "SELECT start_line, start_column FROM symbols
             WHERE parent_symbol_id = ?1 AND name = ?2 AND kind = 'property'
             ORDER BY start_byte LIMIT 1",
            params![subject.symbol_id, field],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    let Some((line, column)) = location else {
        return Ok(None);
    };
    let (Ok(line), Ok(column)) = (usize::try_from(line), usize::try_from(column)) else {
        return Ok(None);
    };
    let Some(source) = line.checked_sub(1).and_then(|index| owner_lines.get(index)) else {
        return Ok(None);
    };
    let code = code_only(source, "swift");
    let Some(declaration) = code.get(column..) else {
        return Ok(None);
    };
    let declaration = declaration
        .split([';', '\n', '}'])
        .next()
        .unwrap_or_default()
        .trim();
    if declaration.is_empty()
        || declaration.starts_with('@')
        || declaration
            .split_whitespace()
            .any(|word| matches!(word, "static" | "class"))
        || declaration.contains("get")
        || declaration.contains("set")
    {
        return Ok(None);
    }
    if arity::default_start(declaration).is_some() {
        return Ok(Some(false));
    }
    let Some((_, property_type)) = declaration.split_once(':') else {
        return Ok(None);
    };
    let property_type = property_type.trim();
    if property_type.ends_with('?') || property_type.starts_with("Optional<") {
        return Ok(Some(false));
    }
    Ok(Some(needs_argument(Some(declaration))))
}

fn owner_of(subject: &Symbol) -> Option<Owner> {
    let signature = subject.signature.as_deref().unwrap_or_default();
    match (subject.language.as_str(), subject.kind.as_str()) {
        ("rust", "struct") => Some(Owner::RustStruct),
        ("go", "struct") => Some(Owner::GoStruct),
        ("zig", "struct") => Some(Owner::ZigStruct),
        ("fsharp", "struct") if signature.contains('{') => Some(Owner::FSharpRecord),
        ("elixir", "struct") => Some(Owner::ElixirStruct),
        ("ruby", "class" | "struct") => Some(Owner::RubyStruct),
        ("python", "class") if signature.starts_with("@dataclass") => Some(Owner::Dataclass),
        ("kotlin", "class") if signature.starts_with("data class") => Some(Owner::KotlinDataClass),
        ("scala", "class") if signature.starts_with("case class") => Some(Owner::ScalaCaseClass),
        ("csharp", "class" | "record")
            if word_position(signature, "record")
                .zip(word_position(signature, &subject.name))
                .is_some_and(|(record, name)| record < name) =>
        {
            Some(Owner::CSharpRecord)
        }
        ("swift", "struct") => Some(Owner::SwiftStruct),
        ("c", "struct" | "class") => Some(Owner::CAggregate),
        ("cpp", "struct" | "class") => Some(Owner::CppAggregate),
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
    #[allow(dead_code)]
    pub(super) fn breaks(&self, subject: &Symbol, lines: Option<&[String]>, line: usize) -> bool {
        owner_of(subject).is_some_and(|owner| {
            self.breaks_for_owner(
                OwnerContext {
                    owner,
                    conn: None,
                    owner_lines: None,
                    constructors: &[],
                    aggregate_fields: None,
                    required_added_fields: None,
                },
                subject,
                lines,
                line,
            )
        })
    }

    pub(super) fn breaks_with_context(
        &self,
        conn: &Connection,
        subject: &Symbol,
        owner_lines: Option<&[String]>,
        construction_lines: Option<&[String]>,
        line: usize,
    ) -> Result<bool, QueryError> {
        let Some(owner) = owner_of(subject) else {
            return Ok(false);
        };
        let constructors = match owner {
            Owner::Dataclass
            | Owner::JavaRecord
            | Owner::KotlinDataClass
            | Owner::ScalaCaseClass
            | Owner::CSharpRecord
            | Owner::SwiftStruct
            | Owner::CppAggregate
            | Owner::RubyStruct => indexed_constructors(conn, subject)?,
            _ => Vec::new(),
        };
        let required_added_fields = if matches!(owner, Owner::ZigStruct) {
            indexed_required_added_fields(conn, subject, owner_lines, &self.added)?
        } else {
            None
        };
        let aggregate_fields = if self.removed.is_empty() {
            None
        } else {
            match owner {
                Owner::CAggregate => indexed_aggregate_field_count(conn, subject)?,
                Owner::CppAggregate
                    if constructors.is_empty() && cpp_aggregate_is_proven(subject, owner_lines) =>
                {
                    indexed_aggregate_field_count(conn, subject)?
                }
                _ => None,
            }
        };
        Ok(self.breaks_for_owner(
            OwnerContext {
                owner,
                conn: Some(conn),
                owner_lines,
                constructors: &constructors,
                aggregate_fields,
                required_added_fields: required_added_fields.as_ref(),
            },
            subject,
            construction_lines,
            line,
        ))
    }

    fn breaks_for_owner(
        &self,
        context: OwnerContext<'_>,
        subject: &Symbol,
        construction_lines: Option<&[String]>,
        line: usize,
    ) -> bool {
        let OwnerContext {
            owner,
            conn,
            owner_lines,
            constructors,
            aggregate_fields,
            required_added_fields,
        } = context;
        match owner {
            Owner::JavaRecord => {
                let Some(lines) = construction_lines else {
                    return false;
                };
                let delegates_to_canonical = calls_this(lines, line);
                if !delegates_to_canonical
                    && custom_constructor_may_accept(constructors, subject, Some(lines), line)
                {
                    return false;
                }
                let callee = if delegates_to_canonical {
                    "this"
                } else {
                    subject.name.as_str()
                };
                let counts = call_counts(lines, line, callee);
                self.old_components.is_some_and(|old| counts.contains(&old))
            }
            Owner::Dataclass => {
                if constructors
                    .iter()
                    .any(|constructor| constructor.name == "__init__")
                {
                    return false;
                }
                self.dataclass_constructor
                    .as_ref()
                    .map_or(!self.removed.is_empty() || self.required, |change| {
                        !change.removed.is_empty() || change.required
                    })
            }
            Owner::KotlinDataClass | Owner::ScalaCaseClass | Owner::CSharpRecord => {
                if custom_constructor_may_accept(constructors, subject, construction_lines, line) {
                    return false;
                }
                self.primary_construction_breaks(subject, construction_lines, line)
            }
            Owner::SwiftStruct => {
                if !constructors.is_empty() {
                    return false;
                }
                conn.is_some_and(|conn| {
                    self.swift_construction_breaks(
                        conn,
                        subject,
                        owner_lines,
                        construction_lines,
                        line,
                    )
                })
            }
            Owner::ZigStruct => {
                let Some(lines) = construction_lines else {
                    return false;
                };
                let Some(required_added_fields) = required_added_fields else {
                    return false;
                };
                aggregate_literals(lines, line, &subject.name, "zig")
                    .iter()
                    .any(|literal| self.breaks_zig_literal(literal, required_added_fields))
            }
            Owner::FSharpRecord => {
                let Some(lines) = construction_lines else {
                    return false;
                };
                aggregate_literals(lines, line, &subject.name, "fsharp")
                    .iter()
                    .any(|literal| self.breaks_fsharp_literal(literal))
            }
            Owner::ElixirStruct => {
                let enforced_added_fields =
                    elixir_enforced_added_fields(owner_lines, subject, &self.added);
                let Some(lines) = construction_lines else {
                    return false;
                };
                aggregate_literals(lines, line, &subject.name, "elixir")
                    .iter()
                    .any(|literal| {
                        self.breaks_aggregate_literal(literal, true, None)
                            || (enforced_added_fields
                                .as_ref()
                                .is_some_and(|fields| !fields.is_empty())
                                && !literal.unclosed
                                && literal.elements.iter().all(|element| element.key.is_some())
                                && elixir_literal_is_unambiguous_build(lines, line, &subject.name)
                                && enforced_added_fields.as_ref().is_some_and(|fields| {
                                    fields.iter().any(|required| {
                                        !literal.elements.iter().any(|element| {
                                            element.key.as_deref() == Some(required.as_str())
                                        })
                                    })
                                }))
                    })
            }
            Owner::RubyStruct => {
                let ruby_proven = ruby_keyword_struct_is_proven(subject, owner_lines);
                if !constructors.is_empty() || !ruby_proven || self.removed.is_empty() {
                    return false;
                }
                let Some(lines) = construction_lines else {
                    return false;
                };
                let (calls, unknown) = written_call_shapes(lines, line, &subject.name, "ruby");
                !unknown
                    && calls.iter().any(|call| {
                        call.named_arguments
                            .iter()
                            .any(|name| self.removed.contains(name))
                    })
            }
            Owner::CAggregate | Owner::CppAggregate => {
                let Some(lines) = construction_lines else {
                    return false;
                };
                let aggregate_is_proven =
                    matches!(owner, Owner::CAggregate) || aggregate_fields.is_some();
                aggregate_literals(lines, line, &subject.name, subject.language.as_str())
                    .iter()
                    .any(|literal| {
                        self.breaks_aggregate_literal(
                            literal,
                            aggregate_is_proven,
                            aggregate_fields,
                        )
                    })
            }
            Owner::RustStruct | Owner::GoStruct => {
                let go = matches!(owner, Owner::GoStruct);
                let Some(lines) = construction_lines else {
                    return true;
                };
                literals(lines, line, &subject.name, go)
                    .iter()
                    .any(|literal| self.breaks_literal(literal, go))
            }
        }
    }

    fn breaks_zig_literal(
        &self,
        literal: &Literal,
        required_added_fields: &std::collections::HashSet<String>,
    ) -> bool {
        if literal.unclosed || literal.elements.iter().any(|element| element.key.is_none()) {
            return false;
        }
        let keys: std::collections::HashSet<_> = literal
            .elements
            .iter()
            .filter_map(|element| element.key.as_deref())
            .collect();
        literal.elements.iter().any(|element| {
            element
                .key
                .as_ref()
                .is_some_and(|key| self.removed.contains(key))
        }) || required_added_fields
            .iter()
            .any(|field| !keys.contains(field.as_str()))
    }

    fn breaks_fsharp_literal(&self, literal: &Literal) -> bool {
        if literal.unclosed {
            return false;
        }
        let keys: Vec<_> = literal
            .elements
            .iter()
            .filter_map(|element| element.key.as_deref())
            .collect();
        literal.elements.iter().any(|element| {
            element
                .key
                .as_ref()
                .is_some_and(|key| self.removed.contains(key))
        }) || (!literal.elements.iter().any(|element| element.base)
            && self.required
            && self
                .added
                .iter()
                .any(|field| !keys.contains(&field.as_str())))
    }

    fn primary_construction_breaks(
        &self,
        subject: &Symbol,
        lines: Option<&[String]>,
        line: usize,
    ) -> bool {
        let language = subject.language.as_str();
        let Some(signature) = subject.signature.as_deref() else {
            return false;
        };
        let Some((_, segments)) = arity::parameter_segments(signature, &subject.name, language)
        else {
            return false;
        };
        let parameters: Option<Vec<_>> = segments
            .iter()
            .map(|segment| {
                Some(PrimaryParameter {
                    name: primary_parameter_name(segment, language)?.to_string(),
                    has_default: arity::default_start(segment).is_some(),
                })
            })
            .collect();
        let Some(parameters) = parameters else {
            return false;
        };
        let Some(lines) = lines else {
            return false;
        };
        let (calls, unknown) = written_call_shapes(lines, line, &subject.name, language);
        if unknown || calls.is_empty() {
            return false;
        }
        if !self.removed.is_empty() {
            for call in &calls {
                if call.positional_count + call.named_arguments.len() > parameters.len()
                    || call
                        .named_arguments
                        .iter()
                        .any(|name| self.removed.contains(name))
                {
                    return true;
                }
            }
        }
        calls.iter().any(|call| {
            parameters.iter().enumerate().any(|(index, parameter)| {
                self.added.contains(&parameter.name)
                    && !parameter.has_default
                    && index >= call.positional_count
                    && !call.named_arguments.contains(&parameter.name)
            })
        })
    }

    fn swift_construction_breaks(
        &self,
        conn: &Connection,
        subject: &Symbol,
        owner_lines: Option<&[String]>,
        construction_lines: Option<&[String]>,
        line: usize,
    ) -> bool {
        let language = subject.language.as_str();
        let counts = construction_lines
            .map(|lines| call_counts(lines, line, &subject.name))
            .unwrap_or_default();
        if counts.is_empty() {
            return false;
        }
        if !self.removed.is_empty()
            && construction_lines.is_some_and(|lines| {
                call_argument_names(lines, line, &subject.name, language)
                    .iter()
                    .any(|name| self.removed.contains(name))
            })
        {
            return true;
        }
        let Some(owner_lines) = owner_lines else {
            return false;
        };
        self.added.iter().any(|field| {
            swift_property_needs_argument(conn, owner_lines, subject, field)
                .ok()
                .flatten()
                .unwrap_or(false)
        })
    }

    fn breaks_aggregate_literal(
        &self,
        literal: &Literal,
        aggregate_is_proven: bool,
        aggregate_fields: Option<usize>,
    ) -> bool {
        !literal.unclosed
            && (literal.elements.iter().any(|element| {
                element
                    .key
                    .as_ref()
                    .is_some_and(|key| aggregate_is_proven && self.removed.contains(key))
            }) || (aggregate_fields.is_some_and(|count| literal.elements.len() > count)
                && !literal.elements.is_empty()
                && literal.elements.iter().all(|element| element.key.is_none())))
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

pub(super) fn written_call_counts(
    lines: &[String],
    line: usize,
    name: &str,
    language: &str,
) -> (Vec<usize>, bool) {
    let (calls, unknown) = written_call_shapes(lines, line, name, language);
    let counts = calls
        .iter()
        .map(|call| {
            call.positional_count
                + if language == "dart" {
                    0
                } else {
                    call.named_arguments.len()
                }
        })
        .collect();
    (counts, unknown)
}

fn written_call_shapes(
    lines: &[String],
    line: usize,
    name: &str,
    language: &str,
) -> (Vec<WrittenArguments>, bool) {
    let (calls, unknown) = written_arguments_on_line(lines, line, name, language);
    (
        calls.into_iter().map(|(_, arguments)| arguments).collect(),
        unknown,
    )
}

pub(super) fn written_arguments_on_line(
    lines: &[String],
    line: usize,
    name: &str,
    language: &str,
) -> (Vec<(usize, WrittenArguments)>, bool) {
    let Some(first) = line.checked_sub(1).and_then(|index| lines.get(index)) else {
        return (Vec::new(), true);
    };
    let text = source_from(lines, line);
    let code = code_only(&text, language);
    let first_line = &code[..first.len().min(code.len())];
    let mut calls = Vec::new();
    let mut unknown = false;
    for at in word_starts(first_line, name) {
        if let Some(call) = written_arguments_at_offset(&text, &code, at, name, language) {
            unknown |= call.has_unknown_spread;
            calls.push((at, call));
        } else if written_call_is_unclosed(&code, at, name, language) {
            unknown = true;
        }
    }
    (calls, unknown)
}

fn written_call_is_unclosed(code: &str, at: usize, name: &str, language: &str) -> bool {
    let Some(mut group) = code.get(at + name.len()..) else {
        return false;
    };
    group = group.trim_start();
    if language == "ruby" && group.starts_with(".new") {
        let Some(after_new) = group.strip_prefix(".new") else {
            return false;
        };
        group = after_new.trim_start();
    }
    if group.starts_with('<') {
        let Some(after_generics) = after_balanced(group, '<', '>') else {
            return true;
        };
        group = after_generics.trim_start();
    }
    group.starts_with('(') && after_balanced(group, '(', ')').is_none()
}

fn written_arguments_at_offset(
    text: &str,
    code: &str,
    at: usize,
    name: &str,
    language: &str,
) -> Option<WrittenArguments> {
    let mut offset = at + name.len();
    let mut group = code.get(offset..)?;
    let whitespace = group.len() - group.trim_start().len();
    offset += whitespace;
    group = group.trim_start();
    if language == "ruby" && group.starts_with(".new") {
        let after_new = group.strip_prefix(".new")?;
        if after_new.starts_with('(') || after_new.chars().next().is_some_and(char::is_whitespace) {
            group = after_new;
            offset += 4;
            let whitespace = group.len() - group.trim_start().len();
            offset += whitespace;
            group = group.trim_start();
        }
    }
    if group.starts_with('<') {
        let after_generics = after_balanced(group, '<', '>')?;
        offset += group.len() - after_generics.len();
        group = after_generics;
        let whitespace = group.len() - group.trim_start().len();
        offset += whitespace;
        group = group.trim_start();
    }
    if !group.starts_with('(') {
        return None;
    }
    let after = after_balanced(group, '(', ')')?;
    let close = group.len() - after.len() - 1;
    let inner = &group[1..close];
    let written = text.get(offset + 1..offset + close)?;
    let arguments: Vec<_> = argument_ranges(inner)
        .into_iter()
        .filter_map(|(start, end)| {
            let argument = inner[start..end].trim();
            (!argument.is_empty()).then(|| argument.to_string())
        })
        .collect();
    let named_arguments: Vec<_> = arguments
        .iter()
        .filter_map(|argument| named_argument_label(argument, language))
        .collect();
    let argument_count = written_argument_count(inner, written, language);
    let positional_count = if language == "dart" {
        argument_count
    } else {
        argument_count.saturating_sub(named_arguments.len())
    };
    Some(WrittenArguments {
        positional_count,
        named_arguments,
        has_unknown_spread: !argument_groups_balanced(inner)
            || arguments
                .iter()
                .any(|argument| argument_spread(argument, language))
            || after.trim_start().starts_with('{'),
    })
}

fn named_argument_label(argument: &str, language: &str) -> Option<String> {
    let separator = match language {
        "kotlin" | "scala" | "fsharp" | "python" => "=",
        "vbnet" => ":=",
        "csharp" | "swift" | "php" | "dart" | "ruby" => ":",
        _ => return None,
    };
    let (name, value) = argument.split_once(separator)?;
    if separator == "=" && value.trim_start().starts_with('=') {
        return None;
    }
    let name = name.trim().strip_prefix('@').unwrap_or(name.trim());
    (!name.is_empty() && name.chars().all(is_identifier_char)).then(|| name.to_string())
}

fn argument_spread(argument: &str, language: &str) -> bool {
    let argument = argument.trim();
    argument.starts_with("...")
        || argument.starts_with("**")
        || (matches!(language, "kotlin" | "python" | "ruby") && argument.starts_with('*'))
        || (language == "scala" && argument.ends_with('*'))
        || (language == "swift" && argument.starts_with("repeat each "))
}

fn argument_groups_balanced(inner: &str) -> bool {
    let mut groups = Vec::new();
    for character in inner.chars() {
        match character {
            '(' => groups.push(')'),
            '[' => groups.push(']'),
            '{' => groups.push('}'),
            ')' | ']' | '}' if groups.pop() != Some(character) => return false,
            _ => {}
        }
    }
    groups.is_empty()
}

fn written_argument_count(inner: &str, written: &str, language: &str) -> usize {
    if language != "dart" {
        return argument_count(inner, written);
    }
    argument_ranges(inner)
        .into_iter()
        .filter(|(start, end)| {
            let argument = inner[*start..*end].trim();
            !argument.is_empty() && !dart_named_argument(argument)
        })
        .count()
}

fn argument_ranges(inner: &str) -> Vec<(usize, usize)> {
    let mut depth = 0usize;
    let mut start = 0;
    let mut ranges = Vec::new();
    for (at, character) in inner.char_indices().chain([(inner.len(), ',')]) {
        match character {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                ranges.push((start, at));
                start = at + 1;
            }
            _ => {}
        }
    }
    ranges
}

fn dart_named_argument(argument: &str) -> bool {
    top_level_colon(argument).is_some_and(|colon| {
        let name = argument[..colon].trim();
        !name.is_empty() && name.chars().all(is_identifier_char)
    })
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

fn aggregate_literals(lines: &[String], line: usize, name: &str, language: &str) -> Vec<Literal> {
    let Some(first) = line.checked_sub(1).and_then(|at| lines.get(at)) else {
        return Vec::new();
    };
    let code = code_from(lines, line, language);
    let first_line_len = code.lines().next().map_or(0, str::len).min(first.len());
    let mut literals = Vec::new();
    let names = std::iter::once(name).chain(name.rsplit('.').next().filter(|last| *last != name));
    for candidate in names {
        for at in word_starts(&code[..first_line_len], candidate) {
            let brace = if language == "fsharp" {
                fsharp_record_literal_brace(&code, at, candidate)
            } else {
                aggregate_brace(&code, at, candidate, language)
            };
            let Some(brace) = brace else {
                continue;
            };
            let Some(body) = code.get(brace + 1..) else {
                continue;
            };
            let (pieces, unclosed) = if language == "fsharp" {
                split_body_on(body, &[',', ';'])
            } else {
                split_body(body)
            };
            literals.push(Literal {
                elements: pieces
                    .iter()
                    .map(|piece| aggregate_element(piece, language))
                    .collect(),
                unclosed,
            });
        }
    }
    literals
}

fn fsharp_record_literal_brace(code: &str, at: usize, name: &str) -> Option<usize> {
    let first_line = code.split('\n').next()?;
    let after_name = first_line.get(at + name.len()..)?;
    let after_name = after_name.trim_start();
    if let Some(rest) = after_name.strip_prefix('=') {
        let whitespace = rest.len() - rest.trim_start().len();
        let brace = at
            + name.len()
            + (first_line.get(at + name.len()..)?.len() - after_name.len())
            + 1
            + whitespace;
        if first_line.get(brace..)?.starts_with('{') {
            let before = first_line.get(..at)?.trim_end();
            let declaration = before.split(':').next()?.trim_start();
            if ["let ", "use ", "member ", "val "]
                .iter()
                .any(|prefix| declaration.starts_with(prefix))
            {
                return Some(brace);
            }
        }
    }
    let before = first_line.get(..at)?.trim_end();
    let colon = before.len().checked_sub(1)?;
    if before.as_bytes().get(colon) != Some(&b':') || before[..colon].trim_end().ends_with(':') {
        return None;
    }
    let close = before[..colon].trim_end().len().checked_sub(1)?;
    if before.as_bytes().get(close) != Some(&b'}') {
        return None;
    }
    let mut depth = 0usize;
    let mut opening = None;
    for (index, character) in before[..=close].char_indices().rev() {
        match character {
            '}' => depth += 1,
            '{' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    opening = Some(index);
                    break;
                }
            }
            _ => {}
        }
    }
    let opening = opening?;
    let prefix = first_line.get(..opening)?;
    let equals = prefix.rfind('=')?;
    let declaration = prefix[..equals].trim_start();
    if !["let ", "use ", "member ", "val "]
        .iter()
        .any(|prefix| declaration.starts_with(prefix))
    {
        return None;
    }
    Some(opening)
}

fn aggregate_brace(code: &str, at: usize, name: &str, language: &str) -> Option<usize> {
    let raw_after = code.get(at + name.len()..)?.split('\n').next()?;
    let leading = raw_after.len() - raw_after.trim_start().len();
    let after = &raw_after[leading..];
    if after.starts_with('(') || after.starts_with("::") {
        return None;
    }
    if language == "zig" {
        return after.starts_with('{').then_some(at + name.len() + leading);
    }
    if language == "elixir" {
        let before = code[..at].trim_end();
        let module = before.rfind('%').map(|percent| &before[percent + 1..])?;
        if module.contains([' ', '\t', '\n']) || !after.starts_with('{') {
            return None;
        }
        return Some(at + name.len() + leading);
    }
    let brace = after.find('{')?;
    let prefix = &after[..brace];
    if prefix.contains([';', '}']) || prefix.contains('(') {
        return None;
    }
    let before = code[..at].trim_end();
    let type_position = before
        .rsplit(|c: char| !is_identifier_char(c))
        .next()
        .is_some_and(|word| matches!(word, "struct" | "class" | "union" | "enum"));
    let is_cpp = language == "cpp";
    let declaration_initializer = prefix
        .split_once('=')
        .is_some_and(|(declaration, _)| !declaration.trim().is_empty())
        || prefix
            .split_whitespace()
            .last()
            .is_some_and(|word| word.chars().all(is_identifier_char));
    if type_position && !declaration_initializer {
        return None;
    }
    let direct_cpp_temporary = is_cpp && !type_position && prefix.trim().is_empty();
    (declaration_initializer || direct_cpp_temporary).then_some(at + name.len() + leading + brace)
}

fn aggregate_element(piece: &str, language: &str) -> Element {
    if language == "fsharp" {
        let trimmed = piece.trim();
        let (base, field) = trimmed
            .split_once(" with ")
            .map_or((false, trimmed), |(_, field)| (true, field));
        let key = field.split_once('=').and_then(|(name, value)| {
            let name = name.trim();
            (!value.trim_start().starts_with('=')
                && !name.is_empty()
                && name.chars().all(is_identifier_char))
            .then(|| name.to_string())
        });
        return Element { key, base };
    }
    if language == "elixir" {
        let key = top_level_colon(piece).and_then(|colon| {
            let name = piece[..colon].trim();
            (!name.is_empty() && name.chars().all(is_identifier_char)).then(|| name.to_string())
        });
        return Element { key, base: false };
    }
    let designator = piece.trim().strip_prefix('.').map(str::trim_start);
    let key = designator.and_then(|designator| {
        let name_end = designator
            .char_indices()
            .take_while(|(_, c)| is_identifier_char(*c))
            .map(|(at, c)| at + c.len_utf8())
            .last()?;
        let name = &designator[..name_end];
        let tail = designator[name_end..].trim_start();
        ((tail.starts_with('=') && !tail.starts_with("=="))
            || (tail.starts_with('.') && tail.contains('=')))
        .then(|| name.to_string())
    });
    Element { key, base: false }
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
    split_body_on(body, &[','])
}

fn split_body_on(body: &str, separators: &[char]) -> (Vec<String>, bool) {
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
            separator if depth == 0 && separators.contains(&separator) => {
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
            dataclass_constructor: None,
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
    fn a_java_record_breaks_when_a_call_matches_its_old_component_count() {
        let changed_record = FieldChange {
            added: vec!["z".to_string()],
            removed: Vec::new(),
            required: true,
            dataclass_constructor: None,
            old_components: Some(2),
        };
        let without_old_count = change(&["z"], &[]);
        let call = Some(lines("Foo(1, 2)"));
        let record = symbol(
            "java",
            "class",
            "Foo",
            "public record Foo(int x, int y, int z)",
        );
        assert!(changed_record.breaks(&record, call.as_deref(), 1));
        assert!(!without_old_count.breaks(&record, call.as_deref(), 1));
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
    fn written_call_counts_keep_nested_arguments_and_mark_unknown_forms() {
        let source_lines = lines("val point = Point(mapOf(\"x\" to Pair(1, 2)), y = 3)");
        assert_eq!(
            written_call_counts(&source_lines, 1, "Point", "kotlin"),
            (vec![2], false)
        );

        let source_lines = lines("scale(1, label: 'x', active: true)");
        assert_eq!(
            written_call_counts(&source_lines, 1, "scale", "dart"),
            (vec![1], false)
        );

        let source_lines = lines("scale(1, 2, label: 'x', active: true)");
        assert_eq!(
            written_call_counts(&source_lines, 1, "scale", "dart"),
            (vec![2], false)
        );

        let source_lines = lines("scale(1, label: 'x')");
        assert_eq!(
            written_call_counts(&source_lines, 1, "scale", "ruby"),
            (vec![2], false)
        );

        let source_lines = lines("val point = Point(*coordinates)");
        assert_eq!(
            written_call_counts(&source_lines, 1, "Point", "kotlin"),
            (vec![1], true)
        );

        let source_lines = lines("val point = Point(1) { 2 }");
        assert_eq!(
            written_call_counts(&source_lines, 1, "Point", "kotlin"),
            (vec![1], true)
        );

        let source_lines = lines("val point = Point(1, 2");
        assert_eq!(
            written_call_counts(&source_lines, 1, "Point", "kotlin"),
            (Vec::new(), true)
        );
    }

    #[test]
    fn a_self_literal_breaks_like_a_named_literal() {
        let added = change(&["c"], &[]);
        assert!(added.breaks_self(&lines("        Self { a: 1, b: 2 }"), 1));
        assert!(!added.breaks_self(&lines("        Self { a: 1, ..base }"), 1));
        assert!(!added.breaks_self(&lines("        let x = 1;"), 1));
    }
}
