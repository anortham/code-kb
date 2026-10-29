//! Which type a written type name names where it is written, so the overload rules compare an
//! argument type with a parameter type by the type they name and not only by their text. A
//! qualified name and a simple name can name one type (`App.Box` and `Box`), and one simple name
//! can name two types (`a.Node` and `b.Node`).
//!
//! A name is resolved only where the index can prove it: a qualified name by the one project
//! type whose namespaces, package, and enclosing types end with it, and a simple name in Java,
//! Kotlin, Scala, and C# by the scope rules of the language (the file, an import, the package or
//! namespaces that hold the code, a `using`). Every other case is unknown.

use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use rusqlite::{Connection, OptionalExtension, params};

use super::{MSBUILD_FILES_SQL, SYMBOL_CHAIN_SQL, csharp_using_alias, csharp_using_namespace};
use crate::queries::QueryError;

const TYPES_NAMED_SQL: &str = "SELECT symbol_id, path FROM symbols
     WHERE name = ?1 AND language = ?2
       AND kind IN ('class', 'struct', 'record', 'interface', 'trait', 'protocol', 'enum',
                    'union', 'type', 'delegate', 'object')
     LIMIT 101";
const VBNET_TYPES_NAMED_SQL: &str = "SELECT symbol_id, path FROM symbols
     WHERE name = ?1 COLLATE NOCASE AND language = ?2
       AND kind IN ('class', 'struct', 'record', 'interface', 'trait', 'protocol', 'enum',
                    'union', 'type', 'delegate', 'object')
     LIMIT 101";
/// More types of one name than this leave every comparison of the name unknown.
const MAX_TYPES: usize = 100;
const PACKAGE_SQL: &str = "SELECT signature FROM symbols
     WHERE path = ?1 AND kind = 'namespace' AND parent_symbol_id IS NULL
       AND signature LIKE 'package %'
     ORDER BY start_line LIMIT 1";
const FILE_IMPORTS_SQL: &str = "SELECT COALESCE(signature, ''), parent_symbol_id IS NULL
     FROM symbols WHERE path = ?1 AND kind = 'import'";
const GLOBAL_USINGS_SQL: &str = "SELECT COALESCE(signature, '') FROM symbols
     WHERE kind = 'import' AND language = 'csharp' AND signature LIKE 'global using %'";
const C_TYPE_WORDS: [&str; 8] = [
    "const", "volatile", "unsigned", "signed", "struct", "class", "enum", "typename",
];

/// Whether two written types name the same type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Same {
    Yes,
    No,
    Unknown,
}

/// Where a type name is written: the file, and the symbol whose namespaces and package hold it.
#[derive(Clone, Copy, Debug)]
pub(super) struct Place<'a> {
    pub path: &'a str,
    pub scope: Option<&'a str>,
}

/// What a type name names where it is written.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Named {
    /// The project type with this dotted name.
    Project(String),
    /// A type outside the project: an explicit import names a type that the index has not.
    Outside,
    Unknown,
}

/// A project type of one name.
#[derive(Debug)]
struct TypeRow {
    /// The dotted name of the type: its namespaces or package, its enclosing types, its name.
    identity: String,
    path: String,
    /// The C# namespace or the Java, Kotlin, or Scala package that holds the type.
    namespace: String,
    /// Defined inside another type or a function, where a simple name reaches it through scopes
    /// the index does not record (an inherited nested type).
    nested: bool,
}

type Types = Option<Rc<Vec<TypeRow>>>;
/// A written type name, the file that writes it, and the symbol that holds it.
type ResolvedKey = (String, Option<String>, String);

/// The type names of one overload set, read once each.
pub(super) struct TypeNames {
    language: &'static str,
    root: PathBuf,
    types: RefCell<HashMap<String, Types>>,
    resolved: RefCell<HashMap<ResolvedKey, Named>>,
    msbuild_usings: OnceCell<bool>,
}

impl TypeNames {
    pub(super) fn new(language: &'static str, root: &Path) -> Self {
        Self {
            language,
            root: root.to_path_buf(),
            types: RefCell::new(HashMap::new()),
            resolved: RefCell::new(HashMap::new()),
            msbuild_usings: OnceCell::new(),
        }
    }

    /// Whether `argument`, written at `at`, and `parameter`, written at `declared_at`, name the
    /// same type. Names that differ without their qualifiers never do; equal text names one type
    /// unless the index has two types of the name and the scopes do not tell them apart.
    pub(super) fn same(
        &self,
        conn: &Connection,
        argument: &str,
        at: Place,
        parameter: &str,
        declared_at: Place,
    ) -> Same {
        let plain = unqualified(argument);
        if plain != unqualified(parameter) {
            return Same::No;
        }
        self.same_named(conn, &plain, argument, at, parameter, declared_at)
            .unwrap_or(Same::Unknown)
    }

    fn same_named(
        &self,
        conn: &Connection,
        plain: &str,
        argument: &str,
        at: Place,
        parameter: &str,
        declared_at: Place,
    ) -> Result<Same, QueryError> {
        let names = type_words(plain);
        let [name] = names.as_slice() else {
            if argument != parameter {
                return Ok(Same::Unknown);
            }
            for name in &names {
                if self.identities(conn, name)?.is_none_or(|count| count > 1) {
                    return Ok(Same::Unknown);
                }
            }
            return Ok(Same::Yes);
        };
        let Some(count) = self.identities(conn, name)? else {
            return Ok(Same::Unknown);
        };
        if argument == parameter && count < 2 {
            return Ok(Same::Yes);
        }
        let named = (
            self.resolve(conn, argument, name, at)?,
            self.resolve(conn, parameter, name, declared_at)?,
        );
        Ok(match named {
            (Named::Project(one), Named::Project(other)) if one == other => Same::Yes,
            (Named::Project(_), Named::Project(_))
            | (Named::Project(_), Named::Outside)
            | (Named::Outside, Named::Project(_)) => Same::No,
            _ => Same::Unknown,
        })
    }

    /// The number of distinct project types named `name`, `None` past [`MAX_TYPES`]. Rows with
    /// one dotted name are one type (a C# or VB.NET partial type, a C++ declaration and its
    /// definition), except in Swift, where the index names no module, so two such rows are the
    /// types of two modules.
    fn identities(&self, conn: &Connection, name: &str) -> Result<Option<usize>, QueryError> {
        Ok(self.types(conn, name)?.map(|types| {
            if self.language == "swift" {
                return types.len();
            }
            types
                .iter()
                .map(|row| row.identity.as_str())
                .collect::<BTreeSet<_>>()
                .len()
        }))
    }

    fn types(&self, conn: &Connection, name: &str) -> Result<Types, QueryError> {
        if let Some(types) = self.types.borrow().get(name) {
            return Ok(types.clone());
        }
        let sql = if self.language == "vbnet" {
            VBNET_TYPES_NAMED_SQL
        } else {
            TYPES_NAMED_SQL
        };
        let rows: Vec<(String, String)> = conn
            .prepare(sql)?
            .query_map(params![name, self.language], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<Result<_, _>>()?;
        let types = if rows.len() > MAX_TYPES {
            None
        } else {
            let mut types = Vec::with_capacity(rows.len());
            for (id, path) in rows {
                types.push(self.type_row(conn, &id, path)?);
            }
            Some(Rc::new(types))
        };
        self.types
            .borrow_mut()
            .insert(name.to_string(), types.clone());
        Ok(types)
    }

    fn type_row(&self, conn: &Connection, id: &str, path: String) -> Result<TypeRow, QueryError> {
        let chain = symbol_chain(conn, id)?;
        let nested = chain.iter().skip(1).any(|(kind, _)| kind != "namespace");
        let mut namespaces: Vec<&str> = chain
            .iter()
            .rev()
            .filter(|(kind, _)| kind == "namespace")
            .map(|(_, name)| name.as_str())
            .collect();
        let package = if is_jvm(self.language) && namespaces.is_empty() {
            package_of(conn, &path)?
        } else {
            None
        };
        if let Some(package) = &package {
            namespaces.push(package);
        }
        let namespace = namespaces.join(".");
        let mut segments: Vec<&str> = namespaces;
        segments.extend(
            chain
                .iter()
                .rev()
                .filter(|(kind, _)| kind != "namespace")
                .map(|(_, name)| name.as_str()),
        );
        let identity = segments.join(".");
        Ok(TypeRow {
            identity: if self.language == "vbnet" {
                identity.to_ascii_lowercase()
            } else {
                identity
            },
            path,
            namespace,
            nested,
        })
    }

    fn resolve(
        &self,
        conn: &Connection,
        written: &str,
        name: &str,
        place: Place,
    ) -> Result<Named, QueryError> {
        let key = (
            place.path.to_string(),
            place.scope.map(str::to_string),
            written.to_string(),
        );
        if let Some(named) = self.resolved.borrow().get(&key) {
            return Ok(named.clone());
        }
        let named = self.resolve_uncached(conn, written, name, place)?;
        self.resolved.borrow_mut().insert(key, named.clone());
        Ok(named)
    }

    fn resolve_uncached(
        &self,
        conn: &Connection,
        written: &str,
        name: &str,
        place: Place,
    ) -> Result<Named, QueryError> {
        let Some(types) = self.types(conn, name)? else {
            return Ok(Named::Unknown);
        };
        let path = qualified_path(written, name);
        if path.len() > 1 {
            let suffix = path.join(".");
            let suffix = if self.language == "vbnet" {
                suffix.to_ascii_lowercase()
            } else {
                suffix
            };
            return Ok(one_of(types.iter().filter(|row| {
                row.identity == suffix || row.identity.ends_with(&format!(".{suffix}"))
            })));
        }
        if types.iter().any(|row| row.nested) {
            return Ok(Named::Unknown);
        }
        match self.language {
            "java" | "kotlin" | "scala" => self.jvm_resolve(conn, name, place, &types),
            "csharp" => self.csharp_resolve(conn, name, place, &types),
            _ => Ok(Named::Unknown),
        }
    }

    /// A simple Java, Kotlin, or Scala name: a type of the file, or the type that an import of
    /// the name writes (both, when they differ, leave it unknown); else the one type of the
    /// file's package, else the one type of a package that a star import names.
    fn jvm_resolve(
        &self,
        conn: &Connection,
        name: &str,
        place: Place,
        types: &[TypeRow],
    ) -> Result<Named, QueryError> {
        let in_file = one_of(types.iter().filter(|row| row.path == place.path));
        let imported = match jvm_import(conn, place.path, name)? {
            JvmImport::None => None,
            JvmImport::Unknown => return Ok(Named::Unknown),
            JvmImport::Of(path) => Some(
                types
                    .iter()
                    .find(|row| row.identity == path)
                    .map_or(Named::Outside, |row| Named::Project(row.identity.clone())),
            ),
        };
        match (in_file, imported) {
            (Named::Unknown, None) => {}
            (Named::Unknown, Some(named)) => return Ok(named),
            (named, None) => return Ok(named),
            (named, Some(other)) if named == other => return Ok(named),
            _ => return Ok(Named::Unknown),
        }
        let package = package_of(conn, place.path)?.unwrap_or_default();
        if types.iter().any(|row| row.namespace == package) {
            return Ok(one_of(types.iter().filter(|row| row.namespace == package)));
        }
        let text = std::fs::read_to_string(self.root.join(place.path)).unwrap_or_default();
        let nested_comments = !place.path.ends_with(".java");
        let stars = super::sections::star_imports(&text, nested_comments);
        Ok(one_of(
            types.iter().filter(|row| stars.contains(&row.namespace)),
        ))
    }

    /// A simple C# name: the one type of the innermost namespace that holds the code and has
    /// one, else the one type of a namespace that a `using` of the file or a `global using`
    /// opens. A `using` alias of the name, a `using` inside a namespace block that opens a
    /// namespace with such a type, and an MSBuild `<Using>` item leave it unknown.
    fn csharp_resolve(
        &self,
        conn: &Connection,
        name: &str,
        place: Place,
        types: &[TypeRow],
    ) -> Result<Named, QueryError> {
        let Some(scope) = place.scope else {
            return Ok(Named::Unknown);
        };
        let mut usings: Vec<(String, bool)> = conn
            .prepare(FILE_IMPORTS_SQL)?
            .query_map(params![place.path], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        usings.extend(
            conn.prepare(GLOBAL_USINGS_SQL)?
                .query_map([], |row| row.get::<_, String>(0))?
                .map(|row| row.map(|signature| (signature, true)))
                .collect::<Result<Vec<_>, _>>()?,
        );
        if usings.iter().any(|(signature, _)| {
            csharp_using_alias(signature).is_some_and(|(alias, _)| alias == name)
        }) {
            return Ok(Named::Unknown);
        }
        let opens = |row: &TypeRow, file_level: bool| {
            usings.iter().any(|(signature, top)| {
                *top == file_level
                    && csharp_using_namespace(signature) == Some(row.namespace.as_str())
            })
        };
        if types.iter().any(|row| opens(row, false)) {
            return Ok(Named::Unknown);
        }
        let enclosing: Vec<String> = symbol_chain(conn, scope)?
            .into_iter()
            .rev()
            .filter(|(kind, _)| kind == "namespace")
            .map(|(_, name)| name)
            .collect();
        let mut level = enclosing.join(".");
        loop {
            if types.iter().any(|row| row.namespace == level) {
                return Ok(one_of(types.iter().filter(|row| row.namespace == level)));
            }
            if level.is_empty() {
                break;
            }
            level = level
                .rsplit_once('.')
                .map_or(String::new(), |(outer, _)| outer.to_string());
        }
        if self.msbuild_usings(conn)? {
            return Ok(Named::Unknown);
        }
        Ok(one_of(types.iter().filter(|row| opens(row, true))))
    }

    fn msbuild_usings(&self, conn: &Connection) -> Result<bool, QueryError> {
        if let Some(known) = self.msbuild_usings.get() {
            return Ok(*known);
        }
        let paths: Vec<String> = conn
            .prepare(MSBUILD_FILES_SQL)?
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        let found = paths.iter().any(|path| {
            std::fs::read_to_string(self.root.join(path))
                .is_ok_and(|text| text.contains("<Using ") || text.contains("<Using\n"))
        });
        Ok(*self.msbuild_usings.get_or_init(|| found))
    }
}

/// The one type these rows name, unknown for none or for two.
fn one_of<'r>(rows: impl Iterator<Item = &'r TypeRow>) -> Named {
    let identities: BTreeSet<&str> = rows.map(|row| row.identity.as_str()).collect();
    match identities.len() {
        1 => Named::Project(
            identities
                .into_iter()
                .next()
                .unwrap_or_default()
                .to_string(),
        ),
        _ => Named::Unknown,
    }
}

fn is_jvm(language: &str) -> bool {
    matches!(language, "java" | "kotlin" | "scala")
}

fn symbol_chain(conn: &Connection, id: &str) -> Result<Vec<(String, String)>, QueryError> {
    Ok(conn
        .prepare(SYMBOL_CHAIN_SQL)?
        .query_map(params![id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?)
}

fn package_of(conn: &Connection, path: &str) -> Result<Option<String>, QueryError> {
    let signature: Option<String> = conn
        .prepare(PACKAGE_SQL)?
        .query_row(params![path], |row| row.get(0))
        .optional()?;
    Ok(signature.as_deref().and_then(|signature| {
        let name = signature
            .strip_prefix("package ")?
            .trim()
            .trim_end_matches(';');
        Some(name.trim().to_string())
    }))
}

enum JvmImport {
    None,
    Of(String),
    Unknown,
}

/// The dotted name that the imports of `path` bind to the simple name `name`: `a.Node` for
/// `import a.Node`. A Kotlin alias of the name (`import a.Other as Node`), a Scala import with
/// selectors or a static import that mentions the name, or two imports that disagree, leave it
/// unknown.
fn jvm_import(conn: &Connection, path: &str, name: &str) -> Result<JvmImport, QueryError> {
    let signatures: Vec<String> = conn
        .prepare(FILE_IMPORTS_SQL)?
        .query_map(params![path], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    let mut found: Option<String> = None;
    for signature in &signatures {
        let Some(text) = signature.trim().strip_prefix("import ") else {
            continue;
        };
        let text = text.trim().trim_end_matches(';').trim();
        if text.contains('{') || text.starts_with("static ") {
            if names_word(text, name) {
                return Ok(JvmImport::Unknown);
            }
            continue;
        }
        if let Some((_, alias)) = text.split_once(" as ") {
            if alias.trim() == name {
                return Ok(JvmImport::Unknown);
            }
            continue;
        }
        let target = text;
        let bound = text.rsplit('.').next().unwrap_or(text);
        if bound != name || target.ends_with(".*") || target.ends_with("._") {
            continue;
        }
        match &found {
            Some(other) if other != target => return Ok(JvmImport::Unknown),
            _ => found = Some(target.to_string()),
        }
    }
    Ok(found.map_or(JvmImport::None, JvmImport::Of))
}

fn names_word(text: &str, word: &str) -> bool {
    text.split(|c: char| !is_word(c)).any(|part| part == word)
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// A written type without the namespaces, packages, and outer types before each name
/// (`App.Box[]` is `Box[]`, `const ns::Point*` is `const Point*`).
pub(super) fn unqualified(written: &str) -> String {
    let mut out = String::with_capacity(written.len());
    let mut rest = written;
    while let Some(c) = rest.chars().next() {
        if !is_word(c) {
            out.push(c);
            rest = &rest[c.len_utf8()..];
            continue;
        }
        let end = rest.find(|c: char| !is_word(c)).unwrap_or(rest.len());
        let (word, after) = rest.split_at(end);
        let next = after.strip_prefix("::").or_else(|| after.strip_prefix('.'));
        if let Some(next) = next.filter(|next| next.starts_with(is_word)) {
            rest = next;
            continue;
        }
        out.push_str(word);
        rest = after;
    }
    out
}

/// The type names of an unqualified written type, without C and C++ qualifier words.
fn type_words(plain: &str) -> Vec<&str> {
    plain
        .split(|c: char| !is_word(c))
        .filter(|word| !word.is_empty() && !C_TYPE_WORDS.contains(word))
        .collect()
}

/// The written path that ends with `name` (`["App", "Box"]` for `App.Box[]`).
fn qualified_path<'w>(written: &'w str, name: &'w str) -> Vec<&'w str> {
    let mut path: Vec<&str> = Vec::new();
    let mut rest = written;
    while let Some(c) = rest.chars().next() {
        if !is_word(c) {
            rest = &rest[c.len_utf8()..];
            continue;
        }
        let end = rest.find(|c: char| !is_word(c)).unwrap_or(rest.len());
        let (word, after) = rest.split_at(end);
        path.push(word);
        if let Some(next) = after
            .strip_prefix("::")
            .or_else(|| after.strip_prefix('.'))
            .filter(|next| next.starts_with(is_word))
        {
            rest = next;
            continue;
        }
        if word == name {
            return path;
        }
        path.clear();
        rest = after;
    }
    vec![name]
}

#[cfg(test)]
mod tests {
    use super::{qualified_path, type_words, unqualified};

    #[test]
    fn a_written_type_drops_the_qualifier_before_each_name() {
        assert_eq!(unqualified("App.Box"), "Box");
        assert_eq!(unqualified("App.Box[]"), "Box[]");
        assert_eq!(unqualified("const ns::Point*"), "const Point*");
        assert_eq!(unqualified("List<App.Box>"), "List<Box>");
        assert_eq!(unqualified("Box?"), "Box?");
        assert_eq!(type_words("const Point*"), ["Point"]);
        assert_eq!(type_words("List<Box>"), ["List", "Box"]);
    }

    #[test]
    fn the_qualified_path_of_a_name_is_the_written_path_that_ends_with_it() {
        assert_eq!(qualified_path("App.Box[]", "Box"), ["App", "Box"]);
        assert_eq!(qualified_path("const ns::Point*", "Point"), ["ns", "Point"]);
        assert_eq!(qualified_path("Box", "Box"), ["Box"]);
        assert_eq!(qualified_path("List<a.Box>", "Box"), ["a", "Box"]);
    }
}
