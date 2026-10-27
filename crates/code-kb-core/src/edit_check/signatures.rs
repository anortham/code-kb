//! Signatures compared without what cannot break a caller: parameter names, default values, a
//! binding `mut`, and in Python every annotation. A parameter list is read piece by piece and
//! matched to the parameter rows of the symbol; a piece with no row, or a row with no piece, makes
//! the whole signature compare as text.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use rusqlite::Connection;

use super::fields::{WrittenArguments, after_balanced, split_body};
use super::word_position;
use crate::models::Symbol;
use crate::queries::{QueryError, get_symbol_by_id};
use crate::reference_report::arity::{self, ParameterRow};
use crate::reference_report::variant_names::{code_only, is_identifier_char};

/// Languages where a caller can name a parameter, so a rename can break a call.
const NAMED_ARGUMENT_LANGUAGES: [&str; 12] = [
    "python",
    "kotlin",
    "csharp",
    "swift",
    "php",
    "ruby",
    "scala",
    "dart",
    "fsharp",
    "vbnet",
    "r",
    "powershell",
];

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
    names: Vec<Option<String>>,
    context: String,
    parameter_shapes: Vec<String>,
    named_optionality: Vec<Option<bool>>,
    formal_contracts: Vec<FormalContract>,
    explicit_receiver: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FormalContract {
    label: Option<String>,
    position: Option<usize>,
    required: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RequiredParameter {
    pub(super) position: Option<usize>,
    pub(super) label: Option<String>,
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

fn comparison_signature(symbol: &Symbol, root: &Path) -> String {
    let signature = signature_of(symbol);
    if symbol.language == "csharp"
        && symbol.kind == "class"
        && arity::parameter_segments(&signature, &symbol.name, "csharp").is_none()
    {
        csharp_primary_constructor_signature(symbol, root).unwrap_or(signature)
    } else {
        signature
    }
}

fn csharp_primary_constructor_signature(symbol: &Symbol, root: &Path) -> Option<String> {
    let source = source_start(symbol, root)?;
    let end = symbol.body_start_byte?.checked_sub(symbol.start_byte)?;
    let header = source.get(..end)?.trim_end();
    arity::parameter_segments(header, &symbol.name, "csharp")?;
    Some(header.to_string())
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
        let signature = comparison_signature(symbol, root);
        Ok(Shape {
            raw: tail_of(&signature, &symbol.name),
            normal: normalize(conn, symbol, &signature)?,
            contract: contract_words(conn, symbol, root)?,
        })
    }

    pub(super) fn call_names(&self) -> &[Option<String>] {
        self.normal
            .as_ref()
            .map_or(&[], |normal| normal.names.as_slice())
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
        if language == "dart" {
            if dart_optional_named_extension(was, is) {
                return Difference::None;
            }
            if let Some(nonbreaking) = dart_named_optionality_change(was, is) {
                return if nonbreaking {
                    Difference::None
                } else {
                    Difference::Signature
                };
            }
        }
        if optional_formal_extension(was, is) {
            return Difference::None;
        }
        if was.tail != is.tail {
            return Difference::Signature;
        }
        if was.names == is.names {
            return Difference::None;
        }
        if was.names.len() != is.names.len() {
            return Difference::Signature;
        }
        let sorted = |names: &[Option<String>]| {
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
        let mut renames = Vec::new();
        for (old, new) in was.names.iter().zip(&is.names) {
            match (old, new) {
                (Some(old), Some(new)) if old == new => {}
                (Some(old), Some(new)) if old != new => {
                    renames.push((old.clone(), new.clone()));
                }
                (None, None) => {}
                _ => return Difference::Signature,
            }
        }
        if renames.is_empty() {
            Difference::Signature
        } else {
            Difference::Renamed(renames)
        }
    }
}

fn optional_formal_extension(was: &Normal, is: &Normal) -> bool {
    let existing = was.formal_contracts.len();
    is.formal_contracts.len() > existing
        && was.context == is.context
        && was.explicit_receiver == is.explicit_receiver
        && is.names.get(..existing) == Some(was.names.as_slice())
        && is.parameter_shapes.get(..existing) == Some(was.parameter_shapes.as_slice())
        && is.named_optionality.get(..existing) == Some(was.named_optionality.as_slice())
        && is.formal_contracts.get(..existing) == Some(was.formal_contracts.as_slice())
        && is.formal_contracts[existing..]
            .iter()
            .all(|formal| formal.required == Some(false))
}

pub(super) fn is_primary_constructor_class(
    conn: &Connection,
    symbol: &Symbol,
    root: &Path,
) -> Result<bool, QueryError> {
    if symbol.kind != "class"
        || !matches!(
            symbol.language.as_str(),
            "csharp" | "fsharp" | "kotlin" | "scala"
        )
    {
        return Ok(false);
    }
    let signature = comparison_signature(symbol, root);
    if arity::parameter_segments(&signature, &symbol.name, &symbol.language).is_none() {
        return Ok(false);
    }
    Ok(normalize(conn, symbol, &signature)?.is_some())
}

fn dart_optional_named_extension(was: &Normal, is: &Normal) -> bool {
    let old_len = was.names.len();
    is.names.len() > old_len
        && was.context == is.context
        && is.names.get(..old_len) == Some(was.names.as_slice())
        && is.parameter_shapes.get(..old_len) == Some(was.parameter_shapes.as_slice())
        && is.named_optionality.get(..old_len) == Some(was.named_optionality.as_slice())
        && is.names[old_len..].iter().all(Option::is_some)
        && is.named_optionality[old_len..]
            .iter()
            .all(|optionality| *optionality == Some(true))
}

fn dart_named_optionality_change(was: &Normal, is: &Normal) -> Option<bool> {
    if was.context != is.context
        || was.names != is.names
        || was.parameter_shapes != is.parameter_shapes
        || was.named_optionality.len() != is.named_optionality.len()
    {
        return None;
    }
    let mut changed = false;
    for (was, is) in was.named_optionality.iter().zip(&is.named_optionality) {
        match (was, is) {
            (Some(false), Some(true)) => changed = true,
            (Some(true), Some(false)) => return Some(false),
            (was, is) if was == is => {}
            _ => return None,
        }
    }
    changed.then_some(true)
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
    let primary_constructor_type =
        symbol.kind == "class" && matches!(language, "kotlin" | "scala" | "csharp");
    let segments = argument_segments(segments, language);
    let explicit_receiver = symbol.kind == "method"
        && segments
            .first()
            .is_some_and(|(segment, _, _)| match language {
                "python" => ["self", "cls"]
                    .iter()
                    .any(|receiver| word_position(segment.trim_start(), receiver) == Some(0)),
                "rust" => arity::is_rust_self(segment),
                "csharp" => arity::after_csharp_this(segment).is_some(),
                _ => false,
            });
    if language == "powershell" && segments.is_empty() && !rows.is_empty() {
        let pieces = rows
            .iter()
            .map(|row| {
                let signature = row.signature.as_deref().unwrap_or(&row.name);
                piece_of(signature, &[row], language)
            })
            .collect::<Vec<_>>();
        let names = rows
            .iter()
            .map(|row| Some(row.name.clone()))
            .collect::<Vec<_>>();
        let formal_contracts = names
            .iter()
            .cloned()
            .map(|label| FormalContract {
                label,
                position: None,
                required: None,
            })
            .collect();
        let before = &signature[..list.start];
        let after = &signature[list.end..];
        let normalized = format!("{before}{}:{}{after}", pieces.len(), pieces.join(","));
        return Ok(Some(Normal {
            tail: tail_of(&normalized, &symbol.name),
            names,
            context: format!("{before}\0{after}"),
            parameter_shapes: pieces,
            named_optionality: vec![None; rows.len()],
            formal_contracts,
            explicit_receiver: false,
        }));
    }
    let mut pieces = Vec::new();
    let mut names = Vec::new();
    let mut parameter_shapes = Vec::new();
    let mut named_optionality = Vec::new();
    let mut formal_contracts = Vec::new();
    let mut used = vec![false; rows.len()];
    let mut first = 0;
    let mut previous_group = None;
    let mut python_positional_only = python
        && segments
            .iter()
            .any(|(segment, _, _)| parameter_key(segment, language) == "/");
    let mut python_keyword_only = false;
    let mut next_positional = 0usize;
    let mut positional_after_variadic = false;
    let php_last_required = (language == "php")
        .then(|| {
            segments
                .iter()
                .enumerate()
                .filter_map(|(index, (segment, _, _))| {
                    let (optional, variadic) =
                        arity::parameter_properties(segment, language).unwrap_or((false, false));
                    (!optional && !variadic).then_some(index)
                })
                .next_back()
        })
        .flatten();
    while first < segments.len() {
        let (segment, named_group, optional_group) = &segments[first];
        let segment = segment.as_str();
        let named_group = *named_group;
        let optional_group = *optional_group;
        let key = parameter_key(segment, language);
        if let Some(group) = named_group {
            if previous_group != Some(group) {
                pieces.push(group.to_string());
                previous_group = Some(group);
            }
        } else if previous_group.is_some() {
            previous_group = None;
        }
        if python && matches!(key.as_str(), "*" | "/") {
            pieces.push(format!("<{key}>"));
            if key == "/" {
                python_positional_only = false;
            } else {
                python_keyword_only = true;
            }
            first += 1;
            continue;
        }
        if language == "r" && key == "..." {
            pieces.push("<variadic>".to_string());
            names.push(Some("...".to_string()));
            parameter_shapes.push("<variadic>".to_string());
            named_optionality.push(None);
            formal_contracts.push(FormalContract {
                label: Some("...".to_string()),
                position: None,
                required: Some(false),
            });
            first += 1;
            continue;
        }
        if matches!(language, "c" | "cpp" | "rust" | "zig") && key == "..." {
            pieces.push("<variadic>".to_string());
            names.push(None);
            parameter_shapes.push("<variadic>".to_string());
            named_optionality.push(None);
            formal_contracts.push(FormalContract {
                label: None,
                position: None,
                required: Some(false),
            });
            first += 1;
            continue;
        }
        if python && key.starts_with('\\') {
            python_keyword_only = true;
        }
        if language == "rust" && is_ignored_parameter(&key) {
            let piece = piece_of(segment, &[], language);
            pieces.push(piece.clone());
            names.push(None);
            parameter_shapes.push(piece);
            named_optionality.push(None);
            formal_contracts.push(FormalContract {
                label: None,
                position: Some(next_positional),
                required: Some(true),
            });
            next_positional += 1;
            first += 1;
            continue;
        }
        let Some((last, text, mut owned)) = (first..segments.len()).find_map(|last| {
            let text = segments[first..=last]
                .iter()
                .map(|(segment, _, _)| segment.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let key = parameter_key(&text, language);
            let owned: Vec<usize> = (0..rows.len())
                .filter(|&at| {
                    let row = &rows[at];
                    let written = row.signature.as_deref().unwrap_or(&row.name);
                    let row_key = parameter_key(written, language);
                    if primary_constructor_type {
                        word_position(&text, &row.name).is_some()
                    } else {
                        row_key == key
                            || row_key == comparable(&row.name)
                                && word_position(&text, &row.name) == Some(0)
                    }
                })
                .collect();
            (!owned.is_empty()).then_some((last, text, owned))
        }) else {
            return Ok(None);
        };
        owned.sort_by_key(|&at| word_position(&text, &rows[at].name));
        let owned_rows: Vec<&ParameterRow> = owned.iter().map(|&at| &rows[at]).collect();
        let piece_text = if language == "dart" {
            strip_dart_required(&text)
        } else {
            &text
        };
        let base_piece = piece_of(piece_text, &owned_rows, language);
        let mut piece = base_piece.clone();
        if optional_group == Some("dart-optional-positional") {
            piece = format!("<optional-positional>{piece}");
        }
        if named_group.is_some() && language == "dart" {
            piece = format!("<named>{piece}");
        }
        for &at in &owned {
            used[at] = true;
            pieces.push(piece.clone());
            let label = callable_parameter_name(
                symbol,
                segment,
                &rows[at].name,
                language,
                named_group.is_some(),
                python_positional_only,
                python_keyword_only,
            );
            names.push(label.clone());
            parameter_shapes.push(base_piece.clone());
            named_optionality.push(
                (language == "dart" && named_group.is_some())
                    .then(|| word_position(segment, "required").is_none()),
            );
            let keyword_only = (language == "dart" && named_group.is_some())
                || (python && python_keyword_only)
                || (language == "python" && segment.trim_start().starts_with('*'))
                || (language == "ruby" && ruby_keyword_parameter(segment, &rows[at].name))
                || positional_after_variadic;
            let explicit_receiver = first == 0
                && match language {
                    "python" if symbol.kind == "method" => ["self", "cls"]
                        .iter()
                        .any(|receiver| word_position(segment.trim_start(), receiver) == Some(0)),
                    "rust" if symbol.kind == "method" => arity::is_rust_self(segment),
                    "csharp" if symbol.kind == "method" => {
                        arity::after_csharp_this(segment).is_some()
                    }
                    _ => false,
                };
            let typescript_this = language == "typescript" && arity::is_this_parameter(segment);
            let position = (!keyword_only && !explicit_receiver && !typescript_this)
                .then_some(next_positional);
            if position.is_some() {
                next_positional += 1;
            }
            let properties = arity::parameter_properties(segment, language);
            let variadic = properties.is_some_and(|(_, variadic)| variadic);
            let required = properties.map(|(optional, variadic)| {
                let optional =
                    if language == "php" && php_last_required.is_some_and(|last| first < last) {
                        false
                    } else if language == "dart" && named_group.is_some() {
                        word_position(segment, "required").is_none()
                    } else {
                        optional
                    };
                !optional && !variadic
            });
            let required = typescript_this.then_some(false).or(required);
            formal_contracts.push(FormalContract {
                label: if typescript_this { None } else { label },
                position,
                required,
            });
            if variadic {
                next_positional = next_positional.saturating_sub(1);
                positional_after_variadic = true;
            }
        }
        first = last + 1;
        if python && (key.starts_with('*') && !key.starts_with("**")) {
            python_keyword_only = true;
        }
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
        "fsharp" => without_body(&signature[list.end + 1..]).to_string(),
        "go" => go_results(&signature[list.end + 1..]).map_or_else(
            || signature[list.end..].to_string(),
            |types| format!("){types}"),
        ),
        _ => signature[list.end..].to_string(),
    };
    let text = format!("{before}{}:{}{after}", pieces.len(), pieces.join(","));
    let context = format!("{before}\0{after}");
    Ok(Some(Normal {
        tail: tail_of(&text, &symbol.name),
        names,
        context,
        parameter_shapes,
        named_optionality,
        formal_contracts,
        explicit_receiver,
    }))
}

fn argument_segments(
    segments: Vec<&str>,
    language: &str,
) -> Vec<(String, Option<&'static str>, Option<&'static str>)> {
    let mut parts = Vec::new();
    for segment in segments {
        let segment = segment.trim();
        if language == "dart" {
            if let Some(inner) = segment
                .strip_prefix('{')
                .and_then(|text| text.strip_suffix('}'))
            {
                parts.extend(
                    split_body(inner)
                        .0
                        .into_iter()
                        .map(|part| (part.trim().to_string(), Some("<named-group>"), None)),
                );
                continue;
            }
            if let Some(inner) = segment
                .strip_prefix('[')
                .and_then(|text| text.strip_suffix(']'))
            {
                parts.extend(split_body(inner).0.into_iter().map(|part| {
                    (
                        part.trim().to_string(),
                        Some("<optional-positional-group>"),
                        Some("dart-optional-positional"),
                    )
                }));
                continue;
            }
        }
        parts.push((segment.to_string(), None, None));
    }
    parts
}

fn parameter_key(segment: &str, language: &str) -> String {
    let segment = if language == "gdscript" {
        segment.trim().strip_prefix("...").unwrap_or(segment)
    } else {
        segment
    };
    match language {
        "dart" => comparable(strip_dart_required(&without_annotations(segment, language))),
        _ => comparable(&without_annotations(segment, language)),
    }
}

fn strip_dart_required(segment: &str) -> &str {
    segment.strip_prefix("required ").unwrap_or(segment)
}

fn callable_parameter_name(
    symbol: &Symbol,
    segment: &str,
    name: &str,
    language: &str,
    named_group: bool,
    python_positional_only: bool,
    python_keyword_only: bool,
) -> Option<String> {
    let externally_named = match language {
        "python" => !python_positional_only && !segment.trim_start().starts_with('*'),
        "swift" => swift_external_label(segment, name).is_some(),
        "ruby" => ruby_keyword_parameter(segment, name),
        "dart" => named_group,
        "fsharp" => matches!(symbol.kind.as_str(), "method" | "constructor" | "class"),
        "vbnet" => !segment
            .split(|character: char| !is_identifier_char(character))
            .any(|word| word == "ParamArray"),
        "r" => segment.trim() != "...",
        _ => true,
    };
    if !externally_named {
        return None;
    }
    if language == "swift" {
        return swift_external_label(segment, name);
    }
    if language == "python" && python_keyword_only && segment.trim_start().starts_with('*') {
        return None;
    }
    Some(name.to_string())
}

fn swift_external_label(segment: &str, name: &str) -> Option<String> {
    let before_type = segment.split(':').next()?.trim();
    let words: Vec<&str> = before_type.split_whitespace().collect();
    match words.as_slice() {
        [label] if *label == name => Some(name.to_string()),
        [label, local] if *local == name && *label != "_" => Some((*label).to_string()),
        _ => None,
    }
}

fn ruby_keyword_parameter(segment: &str, name: &str) -> bool {
    let text = segment.trim_start();
    text.strip_prefix(name)
        .is_some_and(|rest| rest.trim_start().starts_with(':'))
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
        if matches!(language, "php" | "powershell") {
            let variable = format!("${}", row.name);
            if let Some(at) = text.find(&variable) {
                text.replace_range(at..at + variable.len(), " ");
                continue;
            }
        }
        if language == "swift"
            && let Some(label) = swift_external_label(&text, &row.name)
            && label != row.name
            && let Some(at) = word_position(&text, &label)
        {
            text.replace_range(at..at + label.len(), " ");
        }
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
#[cfg(test)]
pub(super) fn names_parameter(
    lines: &[&str],
    callee: &str,
    parameter: &str,
    language: &str,
) -> bool {
    let all_old_labels = [parameter.to_string()];
    names_parameter_with_formals(lines, callee, parameter, language, &all_old_labels)
}

pub(super) fn names_parameter_with_formals(
    lines: &[&str],
    callee: &str,
    parameter: &str,
    language: &str,
    all_old_labels: &[String],
) -> bool {
    let text = lines.join("\n");
    let code = code_only(&text, language);
    for call_start in word_positions(&code, callee) {
        let tail_start = call_start + callee.len();
        if language == "powershell" {
            if powershell_names_parameter(&code[call_start..], callee, parameter, all_old_labels) {
                return true;
            }
            continue;
        }
        if language == "ruby" && !code[tail_start..].trim_start().starts_with('(') {
            let arguments = code[tail_start..]
                .lines()
                .next()
                .unwrap_or_default()
                .split(';')
                .next()
                .unwrap_or_default();
            if split_body(arguments).0.iter().any(|argument| {
                argument_names_parameter(argument, parameter, language, all_old_labels)
            }) {
                return true;
            }
            continue;
        }
        let suffix = &code[tail_start..];
        if let Some(open) = call_parenthesis(suffix) {
            let suffix = suffix.trim_start();
            if names_in_call(&suffix[open..], callee, parameter, language, all_old_labels) {
                return true;
            }
        }
    }
    false
}

fn word_positions(text: &str, word: &str) -> Vec<usize> {
    text.match_indices(word)
        .filter_map(|(start, _)| {
            let before = text[..start].chars().next_back();
            let after = text[start + word.len()..].chars().next();
            let is_word_char = |character: char| character == '_' || character.is_alphanumeric();
            (!before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)).then_some(start)
        })
        .collect()
}

fn call_parenthesis(suffix: &str) -> Option<usize> {
    let leading = suffix.len() - suffix.trim_start().len();
    let suffix = suffix.trim_start();
    if suffix.starts_with('(') {
        return Some(leading);
    }
    let (open, close) = match suffix.chars().next()? {
        '<' => ('<', '>'),
        '[' => ('[', ']'),
        _ => return None,
    };
    let rest = after_balanced(suffix, open, close)?;
    let consumed = suffix.len() - rest.len();
    let after_types = &suffix[consumed..];
    let spaces = after_types.len() - after_types.trim_start().len();
    after_types
        .trim_start()
        .starts_with('(')
        .then_some(leading + consumed + spaces)
}

pub(super) fn renamed_parameter_call(
    lines: &[&str],
    callee: &str,
    old_parameter: &str,
    new_parameter: &str,
    language: &str,
    all_old_labels: &[String],
    all_new_labels: &[String],
) -> bool {
    if !names_parameter_with_formals(lines, callee, old_parameter, language, all_old_labels) {
        return false;
    }
    match language {
        "r" => {
            let code = code_only(&lines.join("\n"), language);
            r_named_arguments(&code, callee).is_some_and(|arguments| {
                arguments.iter().any(|argument| {
                    r_formal_matches(argument, old_parameter, all_old_labels)
                        && !r_formal_matches(argument, new_parameter, all_new_labels)
                })
            })
        }
        "powershell" => {
            let code = code_only(&lines.join("\n"), language);
            powershell_named_arguments(&code, callee).is_some_and(|arguments| {
                arguments.iter().any(|argument| {
                    powershell_formal_matches(argument, old_parameter, all_old_labels)
                        && !powershell_formal_matches(argument, new_parameter, all_new_labels)
                })
            })
        }
        _ => true,
    }
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

pub(super) fn added_required_dart_named_parameters(was: &Symbol, is: &Symbol) -> Vec<String> {
    if was.language != "dart" || is.language != "dart" {
        return Vec::new();
    }
    let (Some(before), Some(after)) = (dart_named_formals(was), dart_named_formals(is)) else {
        return Vec::new();
    };
    after
        .into_iter()
        .filter_map(|(index, name, required)| {
            if !required {
                return None;
            }
            match before.iter().find(|(_, old, _)| old == &name) {
                Some((_, _, was_required)) if *was_required => None,
                Some(_) => Some(name),
                None if before
                    .iter()
                    .any(|(old_index, _, was_required)| *old_index == index && *was_required) =>
                {
                    None
                }
                None => Some(name),
            }
        })
        .collect()
}

pub(super) fn added_required_ruby_keyword_parameters(was: &Symbol, is: &Symbol) -> Vec<String> {
    if was.language != "ruby" || is.language != "ruby" {
        return Vec::new();
    }
    let (Some(before), Some(after)) = (ruby_keyword_formals(was), ruby_keyword_formals(is)) else {
        return Vec::new();
    };
    after
        .into_iter()
        .filter_map(|(index, name, required)| {
            if !required {
                return None;
            }
            match before.iter().find(|(_, old, _)| old == &name) {
                Some((_, _, was_required)) if *was_required => None,
                Some(_) => Some(name),
                None if before
                    .iter()
                    .any(|(old_index, _, was_required)| *old_index == index && *was_required) =>
                {
                    None
                }
                None => Some(name),
            }
        })
        .collect()
}

fn dart_named_formals(symbol: &Symbol) -> Option<Vec<(usize, String, bool)>> {
    let signature = signature_of(symbol);
    let (_, segments) = arity::parameter_segments(&signature, &symbol.name, "dart")?;
    Some(
        argument_segments(segments, "dart")
            .into_iter()
            .enumerate()
            .filter_map(|(index, (segment, group, _))| {
                group?;
                let label = arity::default_start(&segment)
                    .map_or(segment.as_str(), |at| &segment[..at])
                    .split(|character: char| !is_identifier_char(character))
                    .rfind(|word| {
                        !word.is_empty() && !matches!(*word, "required" | "this" | "super")
                    })?
                    .to_string();
                Some((index, label, word_position(&segment, "required").is_some()))
            })
            .collect(),
    )
}

fn ruby_keyword_formals(symbol: &Symbol) -> Option<Vec<(usize, String, bool)>> {
    let signature = signature_of(symbol);
    let (_, segments) = arity::parameter_segments(&signature, &symbol.name, "ruby")?;
    let mut formals = Vec::new();
    for (index, segment) in segments.into_iter().enumerate() {
        let Some((name, default)) = segment.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty()
            || !name
                .chars()
                .next()
                .is_some_and(|character| character == '_' || character.is_alphabetic())
            || !name.chars().all(is_identifier_char)
        {
            return None;
        }
        formals.push((index, name.to_string(), default.trim().is_empty()));
    }
    Some(formals)
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

pub(super) fn added_required_parameters(
    before: &Shape,
    after: &Shape,
    language: &str,
) -> Option<Vec<RequiredParameter>> {
    let (was, is) = (before.normal.as_ref()?, after.normal.as_ref()?);
    if was
        .formal_contracts
        .iter()
        .chain(&is.formal_contracts)
        .any(|formal| formal.required.is_none())
    {
        return None;
    }
    let pure_rename = was.formal_contracts.len() == is.formal_contracts.len()
        && was.parameter_shapes == is.parameter_shapes
        && was.context == is.context
        && was
            .formal_contracts
            .iter()
            .zip(&is.formal_contracts)
            .all(|(old, new)| old.position == new.position && old.required == new.required);
    let mut added = Vec::new();
    for (index, new) in is.formal_contracts.iter().enumerate() {
        if new.required != Some(true) || (new.position.is_none() && new.label.is_none()) {
            continue;
        }
        if language == "ruby" && new.position.is_some() && new.label.is_none() {
            continue;
        }
        let positional_binding = new.position.is_some()
            && (is.explicit_receiver
                || is.formal_contracts[..index]
                    .iter()
                    .any(|formal| formal.position.is_some() && formal.required == Some(false)));
        if new.position.is_some() && !positional_binding {
            continue;
        }
        let same_binding = was.formal_contracts.iter().any(|old| {
            old.required == Some(true) && old.position == new.position && old.label == new.label
        });
        if same_binding {
            continue;
        }
        let renamed_in_place = pure_rename
            && was
                .formal_contracts
                .get(index)
                .is_some_and(|old| old.required == Some(true));
        if renamed_in_place {
            continue;
        }
        added.push(RequiredParameter {
            position: new.position,
            label: new.label.clone(),
        });
    }
    Some(added)
}

pub(super) fn missing_required_parameters_with_receiver(
    actual: &WrittenArguments,
    required: &[RequiredParameter],
    language: &str,
    receiver_offset: Option<usize>,
) -> RequiredKeywordArguments {
    let mut unknown_receiver_binding = false;
    let missing = required
        .iter()
        .filter(|parameter| {
            let passed_by_position = parameter.position.is_some_and(|position| {
                receiver_offset
                    .is_some_and(|offset| actual.positional_count.saturating_sub(offset) > position)
            });
            unknown_receiver_binding |= parameter.position.is_some() && receiver_offset.is_none();
            let passed_by_name = parameter.label.as_deref().is_some_and(|label| {
                actual.named_arguments.iter().any(|actual| match language {
                    "vbnet" => actual.eq_ignore_ascii_case(label),
                    "powershell" => actual.eq_ignore_ascii_case(label),
                    _ => actual == label,
                })
            });
            !passed_by_position && !passed_by_name
        })
        .map(|parameter| {
            parameter.label.clone().unwrap_or_else(|| {
                parameter.position.map_or_else(
                    || "unknown parameter".to_string(),
                    |position| format!("argument {}", position + 1),
                )
            })
        })
        .collect();
    RequiredKeywordArguments {
        missing,
        has_unknown_spread: actual.has_unknown_spread || unknown_receiver_binding,
    }
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

pub(super) fn required_ruby_keyword_arguments(
    lines: &[&str],
    callee: &str,
    required: &[String],
) -> Option<RequiredKeywordArguments> {
    let code = code_only(&lines.join("\n"), "ruby");
    let start = word_position(&code, callee)? + callee.len();
    let arguments = ruby_call_arguments(&code[start..])?;
    Some(RequiredKeywordArguments {
        missing: required
            .iter()
            .filter(|parameter| {
                !names_parameter_with_formals(lines, callee, parameter, "ruby", required)
            })
            .cloned()
            .collect(),
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
fn names_in_call(
    call: &str,
    callee: &str,
    parameter: &str,
    language: &str,
    all_old_labels: &[String],
) -> bool {
    let inner = match after_balanced(call, '(', ')') {
        Some(rest) => &call[1..call.len() - rest.len() - 1],
        None => &call[1..],
    };
    split_body(inner).0.iter().any(|argument| {
        let named = argument_names_parameter(argument, parameter, language, all_old_labels);
        named
            || argument.match_indices(callee).any(|(at, _)| {
                let nested = argument[at + callee.len()..].trim_start();
                !argument[..at].ends_with(is_identifier_char)
                    && nested.starts_with('(')
                    && names_in_call(nested, callee, parameter, language, all_old_labels)
            })
    })
}

fn argument_names_parameter(
    argument: &str,
    parameter: &str,
    language: &str,
    all_old_labels: &[String],
) -> bool {
    let Some(name) = argument_name(argument, language) else {
        return false;
    };
    match language {
        "r" => r_formal_matches(name, parameter, all_old_labels),
        "vbnet" => name.eq_ignore_ascii_case(parameter),
        _ => name == parameter,
    }
}

fn argument_name<'a>(argument: &'a str, language: &str) -> Option<&'a str> {
    let argument = argument.trim();
    let separator = match language {
        "csharp" | "swift" | "php" | "ruby" | "dart" | "vbnet" => ':',
        _ => '=',
    };
    let name = argument
        .find(separator)
        .map_or(argument, |at| argument[..at].trim());
    let valid_separator = argument[name.len()..].trim_start().starts_with(separator)
        && !argument[name.len()..]
            .trim_start()
            .get(1..)
            .is_some_and(|rest| rest.starts_with(separator));
    if !valid_separator {
        return None;
    }
    Some(name)
}

fn r_formal_matches(argument: &str, parameter: &str, formals: &[String]) -> bool {
    let exact: Vec<&str> = formals
        .iter()
        .map(String::as_str)
        .filter(|formal| *formal == argument)
        .collect();
    if !exact.is_empty() {
        return exact.as_slice() == [parameter];
    }
    let matching: Vec<&str> = formals
        .iter()
        .map(String::as_str)
        .take_while(|formal| *formal != "...")
        .filter(|formal| !formal.is_empty() && formal.starts_with(argument))
        .collect();
    matching.as_slice() == [parameter]
}

fn powershell_formal_matches(argument: &str, parameter: &str, formals: &[String]) -> bool {
    let argument = argument.to_ascii_lowercase();
    let exact: Vec<&str> = formals
        .iter()
        .map(|formal| formal.as_str())
        .filter(|formal| formal.to_ascii_lowercase() == argument)
        .collect();
    if !exact.is_empty() {
        return exact
            .first()
            .is_some_and(|formal| formal.eq_ignore_ascii_case(parameter));
    }
    let matching: Vec<&str> = formals
        .iter()
        .map(|formal| formal.as_str())
        .filter(|formal| formal.to_ascii_lowercase().starts_with(&argument))
        .collect();
    matching.len() == 1 && matching[0].eq_ignore_ascii_case(parameter)
}

fn r_named_arguments(code: &str, callee: &str) -> Option<Vec<String>> {
    let mut arguments = Vec::new();
    let mut found = false;
    for (at, _) in code.match_indices(callee) {
        if code[..at]
            .chars()
            .next_back()
            .is_some_and(is_identifier_char)
        {
            continue;
        }
        let tail = code[at + callee.len()..].trim_start();
        if !tail.starts_with('(') {
            continue;
        }
        let rest = after_balanced(tail, '(', ')')?;
        let closing = tail.len().checked_sub(rest.len() + 1)?;
        for argument in split_body(&tail[1..closing]).0 {
            if let Some(name) = argument_name(&argument, "r") {
                arguments.push(name.to_string());
            }
        }
        found = true;
    }
    found.then_some(arguments)
}

fn ruby_call_arguments(tail: &str) -> Option<Vec<String>> {
    let tail = tail.trim_start();
    let arguments = if tail.starts_with('(') {
        let rest = after_balanced(tail, '(', ')')?;
        let closing = tail.len().checked_sub(rest.len() + 1)?;
        tail[1..closing].to_string()
    } else {
        tail.lines()
            .next()?
            .split(';')
            .next()
            .unwrap_or_default()
            .to_string()
    };
    Some(split_body(&arguments).0)
}

fn powershell_names_parameter(
    code: &str,
    callee: &str,
    parameter: &str,
    all_old_labels: &[String],
) -> bool {
    powershell_named_arguments(code, callee).is_some_and(|arguments| {
        arguments
            .iter()
            .any(|argument| powershell_formal_matches(argument, parameter, all_old_labels))
    })
}

fn powershell_named_arguments(code: &str, callee: &str) -> Option<Vec<String>> {
    let start = word_position(code, callee)? + callee.len();
    let arguments = code[start..]
        .lines()
        .next()?
        .split_whitespace()
        .filter_map(|token| token.strip_prefix('-'))
        .map(|name| name.split(':').next().unwrap_or(name).to_string())
        .collect();
    Some(arguments)
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
