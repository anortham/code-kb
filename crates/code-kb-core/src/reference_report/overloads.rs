//! Overload binding: which overloads of one name and owner a call can bind, from the argument
//! facts that julie writes on its call identifier: the count, a spread, the type each argument
//! states, Kotlin and Scala argument names, and Swift argument labels.
//!
//! The rules stay on the safe side. A count, a name, or a label that an overload cannot take
//! rules it out. A type rules it out only where the language rule is certain (Java and Kotlin
//! types that no conversion joins). An overload whose parameter types equal the argument types
//! binds the call alone where the language always picks the exact match.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use rusqlite::{Connection, OptionalExtension, params};

use super::arity::{self, Arity, Formal, after_attribute_group, default_start};
use super::{ACCOUNTING_TIME_LIMIT, Accounting, overload_group, sections};
use crate::models::Symbol;
use crate::queries::QueryError;

const PARAMETER_TYPES_SQL: &str = "SELECT s.name, tf.resolved_type FROM symbols s
     JOIN type_facts tf ON tf.symbol_id = s.symbol_id
     WHERE s.parent_symbol_id = ?1";

/// The C# modifiers that change which parameter an argument fills, or how.
const CSHARP_UNMATCHED_MODIFIERS: [&str; 5] = ["params", "ref", "out", "in", "this"];
const CSHARP_ALIASES: [(&str, &str); 13] = [
    ("String", "string"),
    ("Object", "object"),
    ("Boolean", "bool"),
    ("Char", "char"),
    ("Byte", "byte"),
    ("Int16", "short"),
    ("Int32", "int"),
    ("Int64", "long"),
    ("UInt32", "uint"),
    ("UInt64", "ulong"),
    ("Single", "float"),
    ("Double", "double"),
    ("Decimal", "decimal"),
];
const VBNET_ALIASES: [(&str, &str); 10] = [
    ("int16", "short"),
    ("int32", "integer"),
    ("int64", "long"),
    ("uint16", "ushort"),
    ("uint32", "uinteger"),
    ("uint64", "ulong"),
    ("datetime", "date"),
    ("single", "single"),
    ("boolean", "boolean"),
    ("object", "object"),
];
const JAVA_PRIMITIVES: [(&str, &str); 8] = [
    ("byte", "Byte"),
    ("short", "Short"),
    ("char", "Character"),
    ("int", "Integer"),
    ("long", "Long"),
    ("float", "Float"),
    ("double", "Double"),
    ("boolean", "Boolean"),
];
/// The Kotlin types that are final and that no implicit conversion reaches, except the integer
/// literal widening in [`kotlin_converts`].
const KOTLIN_CLOSED: [&str; 13] = [
    "Int", "Long", "Short", "Byte", "Float", "Double", "Char", "Boolean", "String", "UInt",
    "ULong", "UShort", "UByte",
];

/// The argument facts of one call identifier.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ArgumentFacts {
    pub count: Option<u32>,
    pub spread: bool,
    pub types: Option<Vec<Option<String>>>,
    pub names: Option<Vec<Option<String>>>,
    pub labels: Option<Vec<Option<String>>>,
}

impl ArgumentFacts {
    /// The facts of the JSON columns of one call identifier, `None` when it records none.
    pub(crate) fn from_columns(
        count: Option<u32>,
        spread: bool,
        types: Option<&str>,
        names: Option<&str>,
        labels: Option<&str>,
    ) -> Option<Self> {
        let list = |json: Option<&str>| {
            json.and_then(|json| serde_json::from_str::<Vec<Option<String>>>(json).ok())
        };
        let facts = Self {
            count: count.filter(|_| !spread),
            spread,
            types: list(types).filter(|_| !spread),
            names: list(names).filter(|_| !spread),
            labels: list(labels).filter(|_| !spread),
        };
        (facts.count.is_some() || facts.types.is_some()).then_some(facts)
    }

    /// The facts of the `metadata_json` of one call identifier.
    pub(crate) fn from_metadata(json: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(json).ok()?;
        let text = |key: &str| value.get(key).map(serde_json::Value::to_string);
        Self::from_columns(
            value
                .get("arg_count")
                .and_then(serde_json::Value::as_u64)
                .map(|count| count as u32),
            value
                .get("arg_spread")
                .is_some_and(|spread| !spread.is_null()),
            text("arg_types").as_deref(),
            text("arg_names").as_deref(),
            text("arg_labels").as_deref(),
        )
    }

    fn argument_count(&self) -> Option<usize> {
        self.count
            .map(|count| count as usize)
            .or_else(|| self.types.as_ref().map(Vec::len).filter(|_| !self.spread))
    }

    /// `(String, b = Int)`, with `?` for a type julie could not tell.
    fn describe(&self) -> Option<String> {
        let types = self.types.as_ref()?;
        let parts: Vec<String> = types
            .iter()
            .enumerate()
            .map(|(index, written)| {
                let written = written.as_deref().unwrap_or("?");
                let name = self
                    .names
                    .as_ref()
                    .and_then(|names| names.get(index).cloned().flatten());
                let label = self
                    .labels
                    .as_ref()
                    .and_then(|labels| labels.get(index).cloned().flatten());
                match (name, label) {
                    (Some(name), _) => format!("{name} = {written}"),
                    (None, Some(label)) => format!("{label}: {written}"),
                    (None, None) => written.to_string(),
                }
            })
            .collect();
        Some(format!("({})", parts.join(", ")))
    }
}

/// The overload rules of a language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rules {
    Java,
    Kotlin,
    Scala,
    CSharp,
    Cpp,
    Swift,
    VbNet,
}

impl Rules {
    fn of(language: &str) -> Option<Self> {
        Some(match language {
            "java" => Self::Java,
            "kotlin" => Self::Kotlin,
            "scala" => Self::Scala,
            "csharp" => Self::CSharp,
            "cpp" => Self::Cpp,
            "swift" => Self::Swift,
            "vbnet" => Self::VbNet,
            _ => return None,
        })
    }

    fn normalized(self, written: &str) -> String {
        match self {
            Self::CSharp => return csharp_normalized(written),
            Self::VbNet => return vbnet_normalized(written),
            _ => {}
        }
        let compact: String = if self == Self::Cpp {
            cpp_canonical(written)
        } else {
            written.split_whitespace().collect()
        };
        let mut plain = String::with_capacity(compact.len());
        let mut depth = 0;
        for character in compact.chars() {
            match character {
                '<' => depth += 1,
                '>' if depth > 0 => depth -= 1,
                '[' if self == Self::Scala => depth += 1,
                ']' if self == Self::Scala && depth > 0 => depth -= 1,
                _ if depth == 0 => plain.push(character),
                _ => {}
            }
        }
        let plain = match self {
            Self::Kotlin => plain.trim_end_matches('?'),
            _ => plain.as_str(),
        };
        let prefixes: &[&str] = match self {
            Self::Java => &["java.lang."],
            Self::Kotlin => &["kotlin.reflect.", "kotlin."],
            Self::Scala => &["java.lang.", "scala."],
            Self::Swift => &["Swift."],
            _ => &[],
        };
        prefixes
            .iter()
            .find_map(|prefix| plain.strip_prefix(prefix))
            .unwrap_or(plain)
            .to_string()
    }

    /// Whether no conversion of the language passes a value of type `argument` to a parameter of
    /// type `parameter`. Only Java and Kotlin types that the language closes judge it.
    fn incompatible(self, argument: &str, parameter: &str) -> bool {
        match self {
            Self::Java => java_converts(argument, parameter) == Some(false),
            Self::Kotlin => kotlin_converts(argument, parameter) == Some(false),
            _ => false,
        }
    }
}

fn java_box(primitive: &str) -> Option<&'static str> {
    JAVA_PRIMITIVES
        .iter()
        .find(|(name, _)| *name == primitive)
        .map(|(_, boxed)| *boxed)
}

fn java_unbox(boxed: &str) -> Option<&'static str> {
    JAVA_PRIMITIVES
        .iter()
        .find(|(_, name)| *name == boxed)
        .map(|(primitive, _)| *primitive)
}

/// JLS 5.1.2: the widening primitive conversions.
fn java_widens(from: &str, to: &str) -> bool {
    let targets: &[&str] = match from {
        "byte" => &["short", "int", "long", "float", "double"],
        "short" | "char" => &["int", "long", "float", "double"],
        "int" => &["long", "float", "double"],
        "long" => &["float", "double"],
        "float" => &["double"],
        _ => &[],
    };
    from == to || targets.contains(&to)
}

/// Whether a method invocation converts `argument` to `parameter` (JLS 5.3: identity, widening
/// primitive, boxing, and unboxing then widening), `None` when either type is not a primitive, a
/// boxed primitive, or `String`, whose subtypes the index cannot list.
fn java_converts(argument: &str, parameter: &str) -> Option<bool> {
    let closed =
        |name: &str| name == "String" || java_box(name).is_some() || java_unbox(name).is_some();
    if !closed(argument) || !closed(parameter) {
        return None;
    }
    Some(if argument == parameter {
        true
    } else if java_box(argument).is_some() {
        java_widens(argument, parameter) || java_box(argument) == Some(parameter)
    } else if let Some(primitive) = java_unbox(argument) {
        java_box(parameter).is_some() && java_widens(primitive, parameter)
    } else {
        false
    })
}

/// Whether Kotlin passes `argument` to `parameter`, `None` when either type is not a final
/// built-in type. julie writes `Int` for an integer literal, which also fills a `Long`, `Short`,
/// or `Byte`, and `UInt` for an unsigned literal, which also fills the other unsigned types.
fn kotlin_converts(argument: &str, parameter: &str) -> Option<bool> {
    if !KOTLIN_CLOSED.contains(&argument) || !KOTLIN_CLOSED.contains(&parameter) {
        return None;
    }
    Some(
        argument == parameter
            || (argument == "Int" && matches!(parameter, "Long" | "Short" | "Byte"))
            || (argument == "UInt" && matches!(parameter, "ULong" | "UShort" | "UByte")),
    )
}

/// A type name without whitespace or `global::`, with an explicit framework name read as its
/// C# keyword (`System.String` is `string`). Other namespaces retain their identity.
fn csharp_normalized(written: &str) -> String {
    let compact: String = written.split_whitespace().collect();
    let unqualified = compact.strip_prefix("global::").unwrap_or(&compact);
    unqualified
        .strip_prefix("System.")
        .and_then(|framework| {
            CSHARP_ALIASES
                .iter()
                .find(|(name, _)| *name == framework)
                .map(|(_, keyword)| *keyword)
        })
        .unwrap_or(unqualified)
        .to_string()
}

/// A VB.NET type name in lower case (VB.NET ignores case), with a framework name read as its
/// keyword (`System.Int32` is `integer`).
fn vbnet_normalized(written: &str) -> String {
    let compact: String = written
        .split_whitespace()
        .collect::<String>()
        .to_ascii_lowercase();
    let unqualified = compact.strip_prefix("system.").unwrap_or(&compact);
    VBNET_ALIASES
        .iter()
        .find(|(name, _)| *name == unqualified)
        .map_or(unqualified, |(_, keyword)| *keyword)
        .to_string()
}

/// A C++ type the way julie spells an argument type: no reference, `const` and `volatile` of the
/// pointee first, and no top-level qualifier, which a call never sees (`const Point&` and
/// `Point const` are `Point`, `char const* const` is `const char*`).
fn cpp_canonical(written: &str) -> String {
    let spaced = written
        .replace('&', " ")
        .replace('*', " * ")
        .replace("constexpr", "const");
    let tokens: Vec<&str> = spaced.split_whitespace().collect();
    let qualifier = |token: &&str| matches!(*token, "const" | "volatile");
    let Some(last_star) = tokens.iter().rposition(|token| *token == "*") else {
        return tokens
            .iter()
            .filter(|token| !qualifier(token))
            .copied()
            .collect::<Vec<_>>()
            .join(" ");
    };
    let first_star = tokens
        .iter()
        .position(|token| *token == "*")
        .unwrap_or(last_star);
    let pointee = &tokens[..first_star];
    let mut words: Vec<&str> = ["const", "volatile"]
        .into_iter()
        .filter(|word| pointee.contains(word))
        .collect();
    words.extend(pointee.iter().filter(|token| !qualifier(token)));
    let mut text = words.join(" ");
    for token in &tokens[first_star..=last_star] {
        if *token == "*" {
            text.push('*');
        } else {
            text.push(' ');
            text.push_str(token);
        }
    }
    text
}

/// The written type of a C++ parameter without its name and default value, `None` for an array,
/// a function pointer, or a parameter whose name the text does not end with.
fn cpp_parameter_type(segment: &str, name: &str) -> Option<String> {
    let declaration = default_start(segment)
        .map_or(segment, |at| &segment[..at])
        .trim();
    if declaration.contains(['(', '[']) || name.is_empty() {
        return None;
    }
    let written = declaration.strip_suffix(name)?;
    if written.ends_with(|c: char| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let written = written.trim();
    (!written.is_empty()).then(|| written.to_string())
}

/// The written type of a Swift parameter after its colon, without a default value, attributes,
/// or an ownership modifier (`inout`, `borrowing`, `consuming`, `__owned`).
fn swift_parameter_type(segment: &str) -> Option<String> {
    let declaration = default_start(segment).map_or(segment, |at| &segment[..at]);
    let (_, written) = declaration.split_once(':')?;
    let words: Vec<&str> = written
        .split_whitespace()
        .skip_while(|word| {
            word.starts_with('@')
                || matches!(
                    *word,
                    "inout" | "borrowing" | "consuming" | "__owned" | "__shared"
                )
        })
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// The written type of a C# parameter, `None` when a modifier changes how an argument fills it.
fn csharp_parameter_type(segment: &str) -> Option<String> {
    let mut text = segment.trim();
    while text.starts_with('[') {
        text = after_attribute_group(text)?.trim_start();
    }
    let text = default_start(text).map_or(text, |at| &text[..at]).trim();
    let first = text.split_whitespace().next()?;
    if CSHARP_UNMATCHED_MODIFIERS.contains(&first) {
        return None;
    }
    let (written, _name) = text.rsplit_once(char::is_whitespace)?;
    Some(csharp_normalized(written))
}

/// The external label of a Swift parameter: `None` for `_`, the first word of `with a: Int`, and
/// the name of `a: Int`.
fn swift_label(segment: &str, name: &str) -> Option<String> {
    let head = segment.split(':').next().unwrap_or_default();
    let words: Vec<&str> = head
        .split_whitespace()
        .filter(|word| !word.starts_with('@'))
        .collect();
    match words.as_slice() {
        [label, _, ..] if *label == "_" => None,
        [label, _, ..] => Some((*label).to_string()),
        _ => Some(name.to_string()),
    }
}

/// The written type of a Kotlin parameter (`x: Box?` gives `Box?`).
fn kotlin_written_type(segment: &str) -> Option<&str> {
    let declaration = default_start(segment).map_or(segment, |at| &segment[..at]);
    let (_, written) = declaration.split_once(':')?;
    Some(written.trim())
}

/// The names in the first `<…>` of a declaration head, before its parameter list (`fun <T> f`,
/// `class Box<T : Any>`): Kotlin type parameters.
fn type_parameters(signature: &str) -> Vec<String> {
    let head = signature.split('(').next().unwrap_or_default();
    let Some(open) = head.find('<') else {
        return Vec::new();
    };
    let Some(close) = head[open..].find('>') else {
        return Vec::new();
    };
    head[open + 1..open + close]
        .split(',')
        .filter_map(|parameter| {
            parameter
                .split(':')
                .next()?
                .split_whitespace()
                .last()
                .map(str::to_string)
        })
        .collect()
}

#[derive(Clone, Debug)]
struct Parameter {
    name: String,
    label: Option<String>,
    /// The type a call's argument type is compared with.
    written: Option<String>,
    /// The type as the declaration writes it, for the basis of an exclusion.
    shown: Option<String>,
    optional: bool,
    variadic: bool,
    /// A Kotlin parameter that takes a null: `Box?`, a type parameter, or a type the index does
    /// not know.
    takes_null: bool,
}

#[derive(Clone, Debug)]
struct Member {
    display: String,
    /// The arguments a call may pass, widened by the methods of the name in the base types: a
    /// call through a base reaches the override with the base's defaults.
    arity: Option<Arity>,
    parameters: Option<Vec<Parameter>>,
    /// A base type has a method of the name, whose parameter names and labels a call through the
    /// base uses.
    inherits: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fit {
    No,
    Possible,
    /// Every argument has the type of the parameter it fills; `defaults` when the call leaves a
    /// parameter with a default value out.
    Exact {
        defaults: bool,
    },
}

/// The overloads of one target: the target first, then the other overloads of its name, owner,
/// kind, and file. A constructor adds the primary constructor of its class, or an overload of
/// unknown parameters for a constructor the index lists no symbol for.
pub(crate) struct OverloadSet {
    rules: Rules,
    members: Vec<Member>,
}

impl OverloadSet {
    /// `None` unless `target` is a function, method, or constructor of a language with overloads
    /// and has another overload.
    pub(crate) fn of(
        conn: &Connection,
        root: &Path,
        target: &Symbol,
    ) -> Result<Option<Self>, QueryError> {
        let Some(rules) = Rules::of(&target.language) else {
            return Ok(None);
        };
        if !matches!(target.kind.as_str(), "function" | "method" | "constructor") {
            return Ok(None);
        }
        let constructing = target.kind == "constructor";
        let group = overload_group(conn, target)?;
        let mut primary = None;
        let mut unlisted = false;
        if constructing {
            let class = match &target.parent_symbol_id {
                Some(parent) => crate::queries::get_symbol_by_id(conn, parent)?,
                None => None,
            };
            match class {
                Some(class)
                    if class.kind == "class"
                        && matches!(rules, Rules::Kotlin | Rules::Scala | Rules::CSharp)
                        && has_primary_constructor(&class) =>
                {
                    primary = Some(class);
                }
                _ => unlisted = !arity::constructors_listed(conn, target)?,
            }
        }
        if group.len() + usize::from(primary.is_some() || unlisted) < 2 {
            return Ok(None);
        }
        let mut members = Vec::new();
        for symbol in group.iter().chain(primary.iter()) {
            members.push(member(conn, root, rules, symbol, constructing)?);
        }
        if unlisted {
            members.push(Member {
                display: format!("{}(…)", target.name),
                arity: None,
                parameters: None,
                inherits: false,
            });
        }
        Ok((members.len() > 1).then_some(Self { rules, members }))
    }

    fn fit(&self, member: &Member, args: &ArgumentFacts) -> Fit {
        if let (Some(arity), Some(count)) = (&member.arity, args.count)
            && !arity.fits(count)
        {
            return Fit::No;
        }
        let Some(parameters) = &member.parameters else {
            return Fit::Possible;
        };
        let Some(slots) = self.slots(parameters, args) else {
            return if member.inherits {
                Fit::Possible
            } else {
                Fit::No
            };
        };
        let Some(types) = &args.types else {
            return Fit::Possible;
        };
        if args.argument_count() != Some(types.len()) {
            return Fit::Possible;
        }
        let mut exact = true;
        for (written, slot) in types.iter().zip(&slots) {
            let Some(parameter) = slot.map(|slot| &parameters[slot]) else {
                exact = false;
                continue;
            };
            match (written, &parameter.written) {
                (_, _) if parameter.variadic => exact = false,
                (Some(argument), Some(declared)) => {
                    let argument = self.rules.normalized(argument);
                    if argument != *declared {
                        exact = false;
                        if self.rules.incompatible(&argument, declared) {
                            return Fit::No;
                        }
                    }
                }
                _ => exact = false,
            }
        }
        if !exact {
            return Fit::Possible;
        }
        let filled: Vec<usize> = slots.iter().flatten().copied().collect();
        let left_out = (0..parameters.len()).filter(|index| !filled.contains(index));
        let mut defaults = false;
        for index in left_out {
            if !parameters[index].optional {
                return Fit::Possible;
            }
            defaults = true;
        }
        Fit::Exact { defaults }
    }

    /// The parameter each argument fills, `None` for an argument whose parameter the facts do not
    /// tell; the outer `None` when a name or a label fits no parameter.
    fn slots(&self, parameters: &[Parameter], args: &ArgumentFacts) -> Option<Vec<Option<usize>>> {
        let count = args.argument_count()?;
        if let Some(labels) = args.labels.as_ref().filter(|_| self.rules == Rules::Swift) {
            return swift_slots(parameters, labels, args.types.as_deref());
        }
        let names = args.names.as_ref();
        let mut slots = Vec::with_capacity(count);
        for index in 0..count {
            match names.and_then(|names| names.get(index)).cloned().flatten() {
                Some(name) => {
                    let slot = parameters.iter().position(|p| p.name == name)?;
                    slots.push(Some(slot));
                }
                None => {
                    let slot = index.min(parameters.len().saturating_sub(1));
                    let fills = index < parameters.len()
                        || parameters.last().is_some_and(|last| last.variadic);
                    slots.push(fills.then_some(slot));
                }
            }
        }
        Some(slots)
    }

    /// The overloads the call can bind, by index (the target is 0), `None` when the call records
    /// no argument facts.
    pub(crate) fn bound(&self, args: &ArgumentFacts) -> Option<Vec<usize>> {
        args.argument_count()?;
        let fits: Vec<Fit> = self
            .members
            .iter()
            .map(|member| self.fit(member, args))
            .collect();
        if let Some(exact) = self.exact_of(&fits, args) {
            return Some(vec![exact]);
        }
        Some(
            fits.iter()
                .enumerate()
                .filter(|(_, fit)| **fit != Fit::No)
                .map(|(index, _)| index)
                .collect(),
        )
    }

    /// The one overload whose parameter types equal the argument types and that the language
    /// picks over every other overload the call fits: an identity conversion ranks above every
    /// other, and one that leaves no default out wins over those that do. julie writes `Int` for
    /// a Kotlin `Int?` value, so a Kotlin call also checks the overloads that take a null.
    pub(crate) fn exact_binding(&self, args: &ArgumentFacts) -> Option<usize> {
        let fits: Vec<Fit> = self
            .members
            .iter()
            .map(|member| self.fit(member, args))
            .collect();
        self.exact_of(&fits, args)
    }

    fn exact_of(&self, fits: &[Fit], args: &ArgumentFacts) -> Option<usize> {
        let exact = |defaults: bool| -> Vec<usize> {
            fits.iter()
                .enumerate()
                .filter(|(_, fit)| **fit == Fit::Exact { defaults })
                .map(|(index, _)| index)
                .collect()
        };
        let chosen = match (exact(false).as_slice(), exact(true).as_slice()) {
            ([only], _) => *only,
            ([], [only]) => *only,
            _ => return None,
        };
        if self.rules == Rules::Kotlin && self.null_can_pick_another(chosen, fits, args) {
            return None;
        }
        Some(chosen)
    }

    /// Whether a Kotlin argument could be a nullable value that the chosen overload does not
    /// take and another overload the call fits takes: julie writes `Int` for an `Int?` value.
    fn null_can_pick_another(&self, chosen: usize, fits: &[Fit], args: &ArgumentFacts) -> bool {
        let Some(chosen_parameters) = &self.members[chosen].parameters else {
            return true;
        };
        let Some(chosen_slots) = self.slots(chosen_parameters, args) else {
            return true;
        };
        for (index, member) in self.members.iter().enumerate() {
            if index == chosen || fits[index] == Fit::No {
                continue;
            }
            let Some(parameters) = &member.parameters else {
                return true;
            };
            let Some(slots) = self.slots(parameters, args) else {
                continue;
            };
            let takes_null_instead = chosen_slots.iter().zip(&slots).any(|(own, other)| {
                let own_takes_null = own.is_none_or(|slot| chosen_parameters[slot].takes_null);
                let other_takes_null = other.is_none_or(|slot| parameters[slot].takes_null);
                !own_takes_null && other_takes_null
            });
            if takes_null_instead {
                return true;
            }
        }
        false
    }

    /// The basis of an exclusion when the call binds only other overloads than the target.
    pub(crate) fn exclusion_basis(&self, args: &ArgumentFacts) -> Option<String> {
        let bound = self.bound(args)?;
        if bound.is_empty() || bound.contains(&0) {
            return None;
        }
        let names: Vec<String> = bound
            .iter()
            .take(2)
            .map(|&index| format!("`{}`", self.members[index].display))
            .collect();
        let more = bound.len().saturating_sub(2);
        let overloads = match (names.as_slice(), more) {
            ([one], _) => one.clone(),
            ([first, second], 0) => format!("{first} or {second}"),
            ([first, second], more) => format!("{first}, {second}, or {more} more"),
            _ => return None,
        };
        let what = match args.describe() {
            Some(types) => format!("arguments `{types}` bind"),
            None if args.count == Some(1) => "1 arg fits".to_string(),
            None => format!("{} args fit", args.count?),
        };
        Some(format!("{what} only {overloads}"))
    }
}

/// The parameter each Swift argument fills by its label. An argument with no label and no known
/// type may be a trailing closure, which leaves out its label, so it can also fill a labeled
/// parameter that can take a closure.
fn swift_slots(
    parameters: &[Parameter],
    labels: &[Option<String>],
    types: Option<&[Option<String>]>,
) -> Option<Vec<Option<usize>>> {
    let mut slots = Vec::with_capacity(labels.len());
    let mut next = 0;
    let mut in_variadic: Option<usize> = None;
    for (index, label) in labels.iter().enumerate() {
        let may_trail = label.is_none()
            && types.is_none_or(|types| types.get(index).is_none_or(Option::is_none));
        if let Some(variadic) = in_variadic
            && label.is_none()
        {
            slots.push(Some(variadic));
            continue;
        }
        in_variadic = None;
        loop {
            let parameter = parameters.get(next)?;
            if parameter.label == *label
                || (may_trail && swift_takes_closure(parameter.written.as_deref()))
            {
                slots.push(Some(next));
                if parameter.variadic {
                    in_variadic = Some(next);
                }
                next += 1;
                break;
            }
            if !(parameter.optional || parameter.variadic) {
                return None;
            }
            next += 1;
        }
    }
    parameters[next.min(parameters.len())..]
        .iter()
        .all(|parameter| parameter.optional || parameter.variadic)
        .then_some(slots)
}

/// Whether a Swift parameter of this type can take a closure: a function type, or a name that
/// is not a built-in value type, since a type alias can name a function type.
fn swift_takes_closure(written: Option<&str>) -> bool {
    let Some(written) = written else {
        return true;
    };
    if written.contains("->") {
        return true;
    }
    let plain = written.trim_end_matches(['?', '!']);
    !(plain.starts_with(['[', '(']) || SWIFT_VALUE_TYPES.contains(&plain))
}

const SWIFT_VALUE_TYPES: [&str; 18] = [
    "Int",
    "Int8",
    "Int16",
    "Int32",
    "Int64",
    "UInt",
    "UInt8",
    "UInt16",
    "UInt32",
    "UInt64",
    "Double",
    "Float",
    "Bool",
    "String",
    "Character",
    "Substring",
    "Float16",
    "Float80",
];

fn has_primary_constructor(class: &Symbol) -> bool {
    class.signature.as_deref().is_some_and(|signature| {
        signature
            .find(class.name.as_str())
            .is_some_and(|at| signature[at + class.name.len()..].contains('('))
    })
}

fn member(
    conn: &Connection,
    root: &Path,
    rules: Rules,
    symbol: &Symbol,
    constructing: bool,
) -> Result<Member, QueryError> {
    let shape = arity::definition_shape(conn, root, symbol, constructing, None)?;
    let Some(shape) = shape else {
        return Ok(Member {
            display: format!("{}(…)", symbol.name),
            arity: None,
            parameters: None,
            inherits: false,
        });
    };
    let facts: HashMap<String, String> = if rules == Rules::CSharp {
        HashMap::new()
    } else {
        let mut stmt = conn.prepare(PARAMETER_TYPES_SQL)?;
        stmt.query_map(params![symbol.symbol_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<_, _>>()?
    };
    let generics: Vec<String> = if rules == Rules::Kotlin {
        let owner = match &symbol.parent_symbol_id {
            Some(parent) => conn
                .query_row(
                    "SELECT signature FROM symbols WHERE symbol_id = ?1",
                    params![parent],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten(),
            None => None,
        };
        symbol
            .signature
            .iter()
            .chain(owner.iter())
            .flat_map(|signature| type_parameters(signature))
            .collect()
    } else {
        Vec::new()
    };
    let parameters: Vec<Parameter> = shape
        .formals
        .iter()
        .map(|formal| parameter(rules, formal, &facts, &generics))
        .collect();
    let types: Vec<String> = parameters
        .iter()
        .map(|parameter| {
            let shown = parameter.shown.as_deref().unwrap_or("?");
            if rules == Rules::Swift {
                format!("{}: {shown}", parameter.label.as_deref().unwrap_or("_"))
            } else {
                shown.to_string()
            }
        })
        .collect();
    let display_name = if symbol.kind == "class" || symbol.kind == "constructor" {
        constructed_name(conn, symbol)?
    } else {
        symbol.name.clone()
    };
    let (arity, inherits) = match symbol.kind.as_str() {
        "method" => {
            let mut accounting = Accounting::new(conn, root, ACCOUNTING_TIME_LIMIT);
            match sections::inherited_methods(&mut accounting, symbol)? {
                Some(inherited) if inherited.is_empty() => (Some(shape.arity), false),
                Some(inherited) => {
                    let mut group = vec![symbol.clone()];
                    group.extend(inherited);
                    (arity::group_arity(conn, root, &group, constructing)?, true)
                }
                None => (None, true),
            }
        }
        _ => (Some(shape.arity), false),
    };
    Ok(Member {
        display: format!("{display_name}({})", types.join(", ")),
        arity,
        parameters: Some(parameters),
        inherits,
    })
}

fn constructed_name(conn: &Connection, symbol: &Symbol) -> Result<String, QueryError> {
    if symbol.kind == "class" {
        return Ok(symbol.name.clone());
    }
    let parent = match &symbol.parent_symbol_id {
        Some(parent) => conn
            .query_row(
                "SELECT name FROM symbols WHERE symbol_id = ?1",
                params![parent],
                |row| row.get::<_, String>(0),
            )
            .optional()?,
        None => None,
    };
    Ok(parent.unwrap_or_else(|| symbol.name.clone()))
}

fn parameter(
    rules: Rules,
    formal: &Formal,
    facts: &HashMap<String, String>,
    generics: &[String],
) -> Parameter {
    let shown = match rules {
        Rules::CSharp => csharp_parameter_type(&formal.segment),
        Rules::Cpp => cpp_parameter_type(&formal.segment, &formal.name),
        Rules::Swift => swift_parameter_type(&formal.segment),
        _ => facts.get(&formal.name).cloned(),
    };
    let written = shown.as_deref().map(|shown| rules.normalized(shown));
    let takes_null = rules == Rules::Kotlin
        && match kotlin_written_type(&formal.segment) {
            Some(declared) => {
                declared.ends_with('?')
                    || written.is_none()
                    || generics.iter().any(|generic| generic == declared)
            }
            None => true,
        };
    Parameter {
        name: formal.name.clone(),
        label: (rules == Rules::Swift)
            .then(|| swift_label(&formal.segment, &formal.name))
            .flatten(),
        written,
        shown,
        optional: formal.optional,
        variadic: formal.variadic,
        takes_null,
    }
}

/// The overload sets the caller walk of `blast_radius` checks a step against: a step from a
/// caller to an overload stays unless every call of the name in the caller binds only other
/// overloads.
pub(crate) struct OverloadSteps<'c> {
    conn: &'c Connection,
    root: PathBuf,
    sets: HashMap<String, Option<Rc<(OverloadSet, String)>>>,
    enabled: bool,
}

const CALLER_ARGUMENTS_SQL: &str = "SELECT metadata_json FROM identifiers
     WHERE containing_symbol_id = ?1 AND name = ?2 AND kind = 'call'";
const CALLER_PENDING_CALLS_SQL: &str = "SELECT COUNT(*) FROM pending_relationships
     WHERE from_symbol_id = ?1 AND target_terminal_name = ?2 AND kind = 'calls'";
const CALLER_RELATIONSHIP_CALLS_SQL: &str = "SELECT COUNT(*) FROM relationships r
     JOIN symbols t ON t.symbol_id = r.to_symbol_id
     WHERE r.from_symbol_id = ?1 AND t.name = ?2 AND r.kind = 'calls'";

impl<'c> OverloadSteps<'c> {
    pub(crate) fn new(conn: &'c Connection) -> Self {
        let root = conn
            .query_row(
                "SELECT value FROM artifact_metadata WHERE key = 'root_path'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(PathBuf::from)
            .unwrap_or_default();
        Self {
            conn,
            root,
            sets: HashMap::new(),
            enabled: crate::queries::has_column(conn, "identifiers", "metadata_json"),
        }
    }

    /// Whether the step from `caller` to `origin` stays in the walk.
    pub(crate) fn keeps(&mut self, caller: &str, origin: &str) -> bool {
        if !self.enabled || caller == origin {
            return true;
        }
        self.binds_origin(caller, origin).unwrap_or(true)
    }

    fn binds_origin(&mut self, caller: &str, origin: &str) -> Result<bool, QueryError> {
        let Some(entry) = self.set_of(origin)? else {
            return Ok(true);
        };
        let (set, name) = entry.as_ref();
        let mut stmt = self.conn.prepare(CALLER_ARGUMENTS_SQL)?;
        let calls: Vec<Option<String>> = stmt
            .query_map(params![caller, name], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        if calls.is_empty() {
            return Ok(true);
        }
        let rows: i64 = self
            .conn
            .prepare(CALLER_PENDING_CALLS_SQL)?
            .query_row(params![caller, name], |row| row.get::<_, i64>(0))?
            + self
                .conn
                .prepare(CALLER_RELATIONSHIP_CALLS_SQL)?
                .query_row(params![caller, name], |row| row.get::<_, i64>(0))?;
        if rows > calls.len() as i64 {
            return Ok(true);
        }
        Ok(calls.iter().any(|metadata| {
            metadata
                .as_deref()
                .and_then(ArgumentFacts::from_metadata)
                .is_none_or(|args| set.exclusion_basis(&args).is_none())
        }))
    }

    fn set_of(&mut self, origin: &str) -> Result<Option<Rc<(OverloadSet, String)>>, QueryError> {
        if let Some(entry) = self.sets.get(origin) {
            return Ok(entry.clone());
        }
        let entry = match crate::queries::get_symbol_by_id(self.conn, origin)? {
            Some(symbol) => match OverloadSet::of(self.conn, &self.root, &symbol)? {
                Some(set) => {
                    let name = if symbol.kind == "constructor" {
                        constructed_name(self.conn, &symbol)?
                    } else {
                        symbol.name.clone()
                    };
                    Some(Rc::new((set, name)))
                }
                None => None,
            },
            None => None,
        };
        self.sets.insert(origin.to_string(), entry.clone());
        Ok(entry)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Rules, cpp_parameter_type, csharp_normalized, csharp_parameter_type, java_converts,
        kotlin_converts, swift_label, swift_parameter_type, swift_takes_closure, type_parameters,
    };

    #[test]
    fn a_csharp_type_name_keeps_its_namespace_and_reads_a_framework_name_as_its_keyword() {
        assert_eq!(csharp_normalized("System.String"), "string");
        assert_eq!(
            csharp_normalized("global::Newtonsoft.Json.Linq.JValue"),
            "Newtonsoft.Json.Linq.JValue"
        );
        assert_eq!(csharp_normalized("List< int >"), "List<int>");
        assert_eq!(csharp_normalized("Custom.String"), "Custom.String");
        assert_eq!(csharp_normalized("object?"), "object?");
    }

    #[test]
    fn a_csharp_parameter_with_a_modifier_has_no_exact_type_and_a_default_keeps_its_type() {
        assert_eq!(
            csharp_parameter_type("[NotNull] JValue other").as_deref(),
            Some("JValue")
        );
        assert_eq!(
            csharp_parameter_type("Dictionary<string, int> map").as_deref(),
            Some("Dictionary<string,int>")
        );
        assert_eq!(
            csharp_parameter_type("int count = 0").as_deref(),
            Some("int")
        );
        assert_eq!(csharp_parameter_type("params object[] args"), None);
        assert_eq!(csharp_parameter_type("ref int value"), None);
        assert_eq!(csharp_parameter_type("this JToken token"), None);
    }

    #[test]
    fn java_converts_by_widening_boxing_and_unboxing_and_leaves_open_types_undecided() {
        assert_eq!(java_converts("int", "long"), Some(true));
        assert_eq!(java_converts("int", "Integer"), Some(true));
        assert_eq!(java_converts("Integer", "long"), Some(true));
        assert_eq!(java_converts("char", "int"), Some(true));
        assert_eq!(java_converts("long", "int"), Some(false));
        assert_eq!(java_converts("int", "byte"), Some(false));
        assert_eq!(java_converts("int", "Long"), Some(false));
        assert_eq!(java_converts("Integer", "Long"), Some(false));
        assert_eq!(java_converts("String", "int"), Some(false));
        assert_eq!(java_converts("int", "String"), Some(false));
        assert_eq!(java_converts("int", "Object"), None);
        assert_eq!(java_converts("Box", "String"), None);
        assert_eq!(java_converts("String", "T"), None);
    }

    #[test]
    fn kotlin_converts_only_literals_between_built_in_types() {
        assert_eq!(kotlin_converts("Int", "Long"), Some(true));
        assert_eq!(kotlin_converts("UInt", "ULong"), Some(true));
        assert_eq!(kotlin_converts("Long", "Int"), Some(false));
        assert_eq!(kotlin_converts("Double", "Float"), Some(false));
        assert_eq!(kotlin_converts("Int", "String"), Some(false));
        assert_eq!(kotlin_converts("Int", "Any"), None);
        assert_eq!(kotlin_converts("Box", "String"), None);
    }

    #[test]
    fn a_type_name_drops_type_arguments_nullability_and_the_language_package() {
        assert_eq!(Rules::Java.normalized("java.lang.Class"), "Class");
        assert_eq!(Rules::Java.normalized("List<String>"), "List");
        assert_eq!(Rules::Java.normalized("Foo[]"), "Foo[]");
        assert_eq!(Rules::Kotlin.normalized("kotlin.reflect.KClass"), "KClass");
        assert_eq!(Rules::Kotlin.normalized("Box?"), "Box");
        assert_eq!(Rules::Scala.normalized("List[Int]"), "List");
        assert_eq!(Rules::Swift.normalized("Swift.Int"), "Int");
        assert_eq!(Rules::Swift.normalized("Int?"), "Int?");
    }

    #[test]
    fn a_vbnet_type_name_ignores_case_and_reads_a_framework_name_as_its_keyword() {
        assert_eq!(Rules::VbNet.normalized("Integer"), "integer");
        assert_eq!(Rules::VbNet.normalized("System.Int32"), "integer");
        assert_eq!(Rules::VbNet.normalized("Int64"), "long");
        assert_eq!(Rules::VbNet.normalized("DateTime"), "date");
        assert_eq!(
            Rules::VbNet.normalized("List(Of Integer)"),
            "list(ofinteger)"
        );
    }

    #[test]
    fn a_cpp_type_drops_references_and_top_level_qualifiers_and_puts_pointee_const_first() {
        assert_eq!(Rules::Cpp.normalized("const Point&"), "Point");
        assert_eq!(Rules::Cpp.normalized("Point const &"), "Point");
        assert_eq!(Rules::Cpp.normalized("Point&&"), "Point");
        assert_eq!(Rules::Cpp.normalized("const char*"), "const char*");
        assert_eq!(Rules::Cpp.normalized("char const* const"), "const char*");
        assert_eq!(Rules::Cpp.normalized("unsigned  int"), "unsigned int");
        assert_eq!(
            Rules::Cpp.normalized("const std::vector<int>&"),
            "std::vector"
        );
        assert_eq!(Rules::Cpp.normalized("int**"), "int**");
    }

    #[test]
    fn a_cpp_parameter_type_is_the_text_before_its_name_without_a_default() {
        assert_eq!(
            cpp_parameter_type("const Point& p", "p").as_deref(),
            Some("const Point&")
        );
        assert_eq!(cpp_parameter_type("int b = 0", "b").as_deref(), Some("int"));
        assert_eq!(cpp_parameter_type("char* s", "s").as_deref(), Some("char*"));
        assert_eq!(cpp_parameter_type("int values[]", "values"), None);
        assert_eq!(cpp_parameter_type("void (*f)(int)", "f"), None);
        assert_eq!(cpp_parameter_type("int", "a"), None);
    }

    #[test]
    fn a_swift_parameter_type_follows_the_colon_without_attributes_or_a_default() {
        assert_eq!(swift_parameter_type("_ a: Int").as_deref(), Some("Int"));
        assert_eq!(
            swift_parameter_type("with f: @escaping () -> Int").as_deref(),
            Some("() -> Int")
        );
        assert_eq!(swift_parameter_type("a: inout Int").as_deref(), Some("Int"));
        assert_eq!(
            swift_parameter_type("b: Int? = nil").as_deref(),
            Some("Int?")
        );
        assert_eq!(swift_parameter_type("a"), None);
    }

    #[test]
    fn a_swift_parameter_takes_a_trailing_closure_unless_its_type_is_a_built_in_value() {
        assert!(swift_takes_closure(Some("() -> Int")));
        assert!(swift_takes_closure(Some("((Int) -> Void)?")));
        assert!(swift_takes_closure(Some("Handler")));
        assert!(swift_takes_closure(None));
        assert!(!swift_takes_closure(Some("Int")));
        assert!(!swift_takes_closure(Some("String?")));
        assert!(!swift_takes_closure(Some("[Int]")));
    }

    #[test]
    fn a_swift_label_is_the_first_word_the_name_or_none_for_an_underscore() {
        assert_eq!(swift_label("_ a: Int", "a"), None);
        assert_eq!(swift_label("with a: Int", "a").as_deref(), Some("with"));
        assert_eq!(swift_label("a: Int = 1", "a").as_deref(), Some("a"));
        assert_eq!(
            swift_label("for key: @escaping () -> Void", "key").as_deref(),
            Some("for")
        );
    }

    #[test]
    fn kotlin_type_parameters_come_from_the_head_before_the_parameter_list() {
        assert_eq!(type_parameters("fun <T> run(x: T)"), ["T"]);
        assert_eq!(
            type_parameters("class Box<out T : Any, R>(v: T)"),
            ["T", "R"]
        );
        assert!(type_parameters("fun run(x: List<Int>)").is_empty());
    }
}
