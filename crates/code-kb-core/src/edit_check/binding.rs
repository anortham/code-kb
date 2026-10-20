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

    /// Whether every call of `name` on the line binds another overload: its argument types are
    /// all known and equal the parameter types of another overload, and not of the changed one.
    pub(super) fn binds_another(
        &self,
        conn: &Connection,
        name: &str,
        path: &str,
        line: usize,
    ) -> bool {
        let Ok(mut stmt) = conn.prepare(CALL_ARGUMENT_TYPES_SQL) else {
            return false;
        };
        let Ok(rows) = stmt.query_map(
            params![name, path, path.replace('/', "\\"), line as i64],
            |row| row.get::<_, Option<String>>(0),
        ) else {
            return false;
        };
        let calls: Vec<Option<Vec<String>>> = rows
            .filter_map(Result::ok)
            .map(|types| types.and_then(|json| argument_types(&json)))
            .collect();
        !calls.is_empty()
            && calls.iter().all(|types| {
                types.as_ref().is_some_and(|types| {
                    *types != self.own && self.others.iter().any(|other| other == types)
                })
            })
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

/// A type name without whitespace, `global::`, or a namespace, with a framework name read as its
/// C# keyword (`System.String` is `string`).
fn normalized(written: &str) -> String {
    let compact: String = written.split_whitespace().collect();
    let unqualified = compact.strip_prefix("global::").unwrap_or(&compact);
    let head_end = unqualified.find('<').unwrap_or(unqualified.len());
    let short = &unqualified[unqualified[..head_end].rfind('.').map_or(0, |dot| dot + 1)..];
    ALIASES
        .iter()
        .find(|(framework, _)| *framework == short)
        .map_or_else(|| short.to_string(), |(_, keyword)| keyword.to_string())
}

#[cfg(test)]
mod tests {
    use super::{normalized, parameter_type};

    #[test]
    fn a_type_name_drops_its_namespace_and_reads_a_framework_name_as_its_keyword() {
        assert_eq!(normalized("System.String"), "string");
        assert_eq!(normalized("global::Newtonsoft.Json.Linq.JValue"), "JValue");
        assert_eq!(normalized("List< int >"), "List<int>");
        assert_eq!(
            normalized("System.Collections.Generic.List<int>"),
            "List<int>"
        );
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
