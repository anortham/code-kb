//! C# overload binding: a call binds the overload whose parameter types equal its argument types,
//! so a change of `Equals(object)` does not reach a call that passes a `JValue` to
//! `Equals(JValue)`.

use rusqlite::{Connection, params};

use crate::models::Symbol;
use crate::queries::QueryError;
use crate::reference_report::arity::{after_attribute_group, default_start, parameter_segments};
use crate::reference_report::overload_group;

const CALL_ARGUMENT_TYPES_SQL: &str = "SELECT json_extract(metadata_json, '$.arg_types')
     FROM identifiers
     WHERE name = ?1 AND kind = 'call' AND (path = ?2 OR path = ?3) AND start_line = ?4
       AND json_valid(metadata_json)";
/// The modifiers that change which parameter an argument fills, or how: no exact rule holds.
const UNMATCHED_MODIFIERS: [&str; 5] = ["params", "ref", "out", "in", "this"];
const ALIASES: [(&str, &str); 13] = [
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

/// The parameter types of a changed C# overload and of the other overloads of its name and owner.
pub(super) struct TypedOverloads {
    own: Vec<String>,
    others: Vec<Vec<String>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum OverloadBinding {
    Changed,
    Other,
    Unknown,
}

impl TypedOverloads {
    /// `None` unless `subject` is a C# callable with a plain parameter list and another overload
    /// of the same owner has one too.
    pub(super) fn of(conn: &Connection, subject: &Symbol) -> Result<Option<Self>, QueryError> {
        if subject.language != "csharp" {
            return Ok(None);
        }
        let Some(own) = parameter_types(subject) else {
            return Ok(None);
        };
        let others: Vec<Vec<String>> = overload_group(conn, subject)?
            .iter()
            .filter(|symbol| symbol.symbol_id != subject.symbol_id)
            .filter_map(parameter_types)
            .filter(|types| *types != own)
            .collect();
        Ok((!others.is_empty()).then_some(Self { own, others }))
    }

    /// Which overload every call on the line selects by exact argument types.
    pub(super) fn binding(
        &self,
        conn: &Connection,
        name: &str,
        path: &str,
        line: usize,
    ) -> OverloadBinding {
        let Ok(mut stmt) = conn.prepare(CALL_ARGUMENT_TYPES_SQL) else {
            return OverloadBinding::Unknown;
        };
        let Ok(rows) = stmt.query_map(
            params![name, path, path.replace('/', "\\"), line as i64],
            |row| row.get::<_, Option<String>>(0),
        ) else {
            return OverloadBinding::Unknown;
        };
        let calls: Vec<Option<Vec<String>>> = rows
            .filter_map(Result::ok)
            .map(|types| types.and_then(|json| argument_types(&json)))
            .collect();
        if calls.is_empty() {
            OverloadBinding::Unknown
        } else if calls.iter().all(|types| types.as_ref() == Some(&self.own)) {
            OverloadBinding::Changed
        } else if calls.iter().all(|types| {
            types.as_ref().is_some_and(|types| {
                *types != self.own && self.others.iter().any(|other| other == types)
            })
        }) {
            OverloadBinding::Other
        } else {
            OverloadBinding::Unknown
        }
    }
}

/// The argument types of a call, `None` when one of them is unknown.
fn argument_types(json: &str) -> Option<Vec<String>> {
    let types: Vec<Option<String>> = serde_json::from_str(json).ok()?;
    types
        .into_iter()
        .map(|t| t.map(|t| normalized(&t)))
        .collect()
}

/// The parameter types of a C# callable, `None` when a parameter has a default value or a
/// modifier that changes how arguments fill it.
fn parameter_types(symbol: &Symbol) -> Option<Vec<String>> {
    let signature = symbol.signature.as_deref()?;
    let (_, segments) = parameter_segments(signature, &symbol.name, "csharp")?;
    segments
        .iter()
        .map(|segment| parameter_type(segment))
        .collect()
}

fn parameter_type(segment: &str) -> Option<String> {
    let mut text = segment.trim();
    while text.starts_with('[') {
        text = after_attribute_group(text)?.trim_start();
    }
    if default_start(text).is_some() {
        return None;
    }
    let first = text.split_whitespace().next()?;
    if UNMATCHED_MODIFIERS.contains(&first) {
        return None;
    }
    let (written, _name) = text.rsplit_once(char::is_whitespace)?;
    Some(normalized(written))
}

/// A type name without whitespace or `global::`, with an explicit framework name read as its
/// C# keyword (`System.String` is `string`). Other namespaces retain their identity.
fn normalized(written: &str) -> String {
    let compact: String = written.split_whitespace().collect();
    let unqualified = compact.strip_prefix("global::").unwrap_or(&compact);
    unqualified
        .strip_prefix("System.")
        .and_then(|framework| {
            ALIASES
                .iter()
                .find(|(name, _)| *name == framework)
                .map(|(_, keyword)| *keyword)
        })
        .unwrap_or(unqualified)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{normalized, parameter_type};

    #[test]
    fn a_type_name_keeps_its_namespace_and_reads_a_framework_name_as_its_keyword() {
        assert_eq!(normalized("System.String"), "string");
        assert_eq!(
            normalized("global::Newtonsoft.Json.Linq.JValue"),
            "Newtonsoft.Json.Linq.JValue"
        );
        assert_eq!(normalized("List< int >"), "List<int>");
        assert_eq!(
            normalized("System.Collections.Generic.List<int>"),
            "System.Collections.Generic.List<int>"
        );
        assert_eq!(normalized("Custom.String"), "Custom.String");
        assert_eq!(normalized("String"), "String");
        assert_eq!(normalized("object?"), "object?");
    }

    #[test]
    fn a_parameter_with_a_default_or_a_modifier_has_no_exact_type() {
        assert_eq!(
            parameter_type("[NotNull] JValue other").as_deref(),
            Some("JValue")
        );
        assert_eq!(
            parameter_type("Dictionary<string, int> map").as_deref(),
            Some("Dictionary<string,int>")
        );
        assert_eq!(parameter_type("int count = 0"), None);
        assert_eq!(parameter_type("params object[] args"), None);
        assert_eq!(parameter_type("ref int value"), None);
        assert_eq!(parameter_type("this JToken token"), None);
    }
}
