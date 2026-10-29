//! Overload binding for the edit check: a call binds the overload whose parameter types equal its
//! argument types, so a change of `Equals(object)` does not reach a call that passes a `JValue` to
//! `Equals(JValue)`. The rules are those of [`OverloadSet`].

use std::path::Path;

use rusqlite::{Connection, params};

use crate::models::Symbol;
use crate::queries::QueryError;
use crate::reference_report::overloads::{ArgumentFacts, Call, OverloadSet};

const CALL_ARGUMENTS_SQL: &str = "SELECT metadata_json, path, containing_symbol_id FROM identifiers
     WHERE name = ?1 AND kind = 'call' AND (path = ?2 OR path = ?3) AND start_line = ?4
       AND json_valid(metadata_json)";

/// The changed overload (the first of the set) and the other overloads of its name and owner.
pub(super) struct TypedOverloads {
    set: OverloadSet,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum OverloadBinding {
    Changed,
    Other,
    Unknown,
}

impl TypedOverloads {
    /// `None` unless `subject` has another overload in a language with overload rules.
    pub(super) fn of(
        conn: &Connection,
        root: &Path,
        subject: &Symbol,
    ) -> Result<Option<Self>, QueryError> {
        Ok(OverloadSet::of(conn, root, subject)?.map(|set| Self { set }))
    }

    /// Which overload every call on the line selects by exact argument types.
    pub(super) fn binding(
        &self,
        conn: &Connection,
        name: &str,
        path: &str,
        line: usize,
    ) -> OverloadBinding {
        let Ok(mut stmt) = conn.prepare(CALL_ARGUMENTS_SQL) else {
            return OverloadBinding::Unknown;
        };
        let Ok(rows) = stmt.query_map(
            params![name, path, path.replace('/', "\\"), line as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        ) else {
            return OverloadBinding::Unknown;
        };
        let bindings: Vec<Option<usize>> = rows
            .filter_map(Result::ok)
            .map(|(json, written_path, caller)| {
                ArgumentFacts::from_metadata(&json).and_then(|args| {
                    self.set.exact_binding(&Call {
                        conn,
                        args: &args,
                        path: &written_path.replace('\\', "/"),
                        caller: caller.as_deref(),
                    })
                })
            })
            .collect();
        if bindings.is_empty() {
            OverloadBinding::Unknown
        } else if bindings.iter().all(|binding| *binding == Some(0)) {
            OverloadBinding::Changed
        } else if bindings
            .iter()
            .all(|binding| binding.is_some_and(|index| index != 0))
        {
            OverloadBinding::Other
        } else {
            OverloadBinding::Unknown
        }
    }
}
