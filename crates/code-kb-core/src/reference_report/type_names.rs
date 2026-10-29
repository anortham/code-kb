//! Which type a written type name names where it is written, so the overload rules compare an
//! argument type with a parameter type by the type they name and not only by their text. A
//! qualified name and a simple name can name one type (`App.Box` and `Box`), one simple name can
//! name two types (`a.Node` and `b.Node`), and one simple name can name a project type in one
//! file and a library type in another (`java.util.Date` and `java.sql.Date`).
//!
//! In Java, Kotlin, Scala, C#, and VB.NET a name is resolved by the scope rules of the language:
//! a type that encloses the code, the file, an import, the package or namespaces that hold the
//! code, a `using` or `Imports` line, a star import. A name that reaches no project type names a
//! library type, which is known by its full name when an import names it or when a package or
//! namespace that the code opens is known to hold it, and is otherwise known only by what the
//! code opens. Two names name one type only when this proves it. C++ and Swift compare a
//! qualified name with the project types and leave every other comparison of a shared simple
//! name unknown.

use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use rusqlite::{Connection, OptionalExtension, params};

use super::library_types::{
    DOTNET_KNOWN, IMPLICIT_USINGS, JAVA_KNOWN, JAVA_LANG, KOTLIN_BUILTINS, SCALA_BUILTINS,
};
use super::{MSBUILD_FILES_SQL, csharp_using_alias, csharp_using_namespace};
use crate::queries::QueryError;

const TYPES_NAMED_SQL: &str = "SELECT symbol_id, path, parent_symbol_id FROM symbols
     WHERE name = ?1 AND language = ?2
       AND kind IN ('class', 'struct', 'record', 'interface', 'trait', 'protocol', 'enum',
                    'union', 'type', 'delegate', 'object')
     LIMIT 101";
const VBNET_TYPES_NAMED_SQL: &str = "SELECT symbol_id, path, parent_symbol_id FROM symbols
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
const PACKAGES_SQL: &str = "SELECT DISTINCT signature FROM symbols
     WHERE kind = 'namespace' AND language = ?1 AND signature LIKE 'package %'";
const NAMESPACES_SQL: &str = "SELECT symbol_id, name, parent_symbol_id FROM symbols
     WHERE kind = 'namespace' AND language = ?1";
const FILE_IMPORTS_SQL: &str = "SELECT COALESCE(signature, ''), parent_symbol_id
     FROM symbols WHERE path = ?1 AND kind = 'import'";
const GLOBAL_USINGS_SQL: &str = "SELECT COALESCE(signature, '') FROM symbols
     WHERE kind = 'import' AND language = 'csharp' AND signature LIKE 'global using %'";
const CHAIN_SQL: &str = "WITH RECURSIVE chain(symbol_id, depth) AS (
         SELECT ?1, 0
         UNION ALL
         SELECT s.parent_symbol_id, chain.depth + 1
         FROM symbols s JOIN chain ON s.symbol_id = chain.symbol_id
         WHERE s.parent_symbol_id IS NOT NULL AND chain.depth < 32
     )
     SELECT s.symbol_id, s.kind, s.name FROM chain JOIN symbols s ON s.symbol_id = chain.symbol_id
     ORDER BY chain.depth";
const C_TYPE_WORDS: [&str; 8] = [
    "const", "volatile", "unsigned", "signed", "struct", "class", "enum", "typename",
];

const JAVA_KEYWORDS: [&str; 9] = [
    "byte", "short", "char", "int", "long", "float", "double", "boolean", "void",
];
const CSHARP_KEYWORDS: [&str; 19] = [
    "bool", "byte", "sbyte", "char", "short", "ushort", "int", "uint", "long", "ulong", "float",
    "double", "decimal", "nint", "nuint", "string", "object", "dynamic", "void",
];
/// VB.NET type keywords in the lower case of [`vbnet_normalized`](super::overloads).
const VBNET_KEYWORDS: [&str; 16] = [
    "boolean", "byte", "sbyte", "char", "short", "ushort", "integer", "uinteger", "long", "ulong",
    "single", "double", "decimal", "string", "object", "date",
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
    /// A library type by its full name: an import names it, or one opened package or namespace
    /// is known to hold it.
    Library(String),
    /// A qualified name that names no project type, known only by its text.
    Written(String),
    /// A library type that a simple name reaches through what the code opens.
    Opened(Rc<Opens>),
    Unknown,
}

/// The library packages or namespaces that the code of a simple name opens, and the build project
/// (the folder of its `.csproj` or `.vbproj`) whose project-wide usings and imports it also sees.
#[derive(Debug, PartialEq, Eq)]
struct Opens {
    names: BTreeSet<String>,
    project: String,
}

/// A project type of one name.
#[derive(Debug)]
struct TypeRow {
    /// The dotted name of the type: its namespaces or package, its enclosing types, its name.
    identity: String,
    path: String,
    /// The C# or VB.NET namespace or the Java, Kotlin, or Scala package that holds the type.
    namespace: String,
    /// Defined inside another type or a function, where a simple name reaches it through scopes
    /// the index does not record (an inherited nested type).
    nested: bool,
    parent: Option<String>,
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
    /// The packages or namespaces the project declares.
    declared: OnceCell<HashSet<String>>,
    /// The folder of the build project that holds each folder, with what it opens.
    projects: RefCell<HashMap<PathBuf, Rc<BuildProject>>>,
}

/// A `.csproj` or `.vbproj` and the namespaces it opens in each of its files.
#[derive(Debug, Default)]
struct BuildProject {
    folder: String,
    opens: Vec<String>,
    /// A `using` alias of the project, which can give any simple name another meaning.
    aliases: Vec<String>,
}

impl TypeNames {
    pub(super) fn new(language: &'static str, root: &Path) -> Self {
        Self {
            language,
            root: root.to_path_buf(),
            types: RefCell::new(HashMap::new()),
            resolved: RefCell::new(HashMap::new()),
            msbuild_usings: OnceCell::new(),
            declared: OnceCell::new(),
            projects: RefCell::new(HashMap::new()),
        }
    }

    /// Whether `argument`, written at `at`, and `parameter`, written at `declared_at`, name the
    /// same type. Names that differ without their qualifiers never do.
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

    fn scoped(&self) -> bool {
        matches!(
            self.language,
            "java" | "kotlin" | "scala" | "csharp" | "vbnet"
        )
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
        let words = if self.language == "vbnet" {
            plain.replace("(of", "(")
        } else {
            plain.to_string()
        };
        let names = type_words(&words);
        if !self.scoped() {
            return self.same_unscoped(conn, &names, argument, at, parameter, declared_at);
        }
        if names.len() > 1 && argument != parameter {
            return Ok(Same::Unknown);
        }
        let mut same = Same::Yes;
        for name in &names {
            if self.is_keyword(name) {
                continue;
            }
            let one = self.compare(
                &self.resolve(conn, argument, name, at)?,
                &self.resolve(conn, parameter, name, declared_at)?,
            );
            match (names.len(), one) {
                (_, Same::Yes) => {}
                (1, other) => return Ok(other),
                _ => same = Same::Unknown,
            }
        }
        Ok(same)
    }

    /// C++ and Swift: equal text names one type when the index has at most one type of each name
    /// in it; a qualified name names the one project type it ends with.
    fn same_unscoped(
        &self,
        conn: &Connection,
        names: &[&str],
        argument: &str,
        at: Place,
        parameter: &str,
        declared_at: Place,
    ) -> Result<Same, QueryError> {
        let [name] = names else {
            if argument != parameter {
                return Ok(Same::Unknown);
            }
            for name in names {
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
        Ok(self.compare(
            &self.resolve(conn, argument, name, at)?,
            &self.resolve(conn, parameter, name, declared_at)?,
        ))
    }

    fn is_keyword(&self, name: &str) -> bool {
        match self.language {
            "java" => JAVA_KEYWORDS.contains(&name),
            "csharp" => CSHARP_KEYWORDS.contains(&name),
            "vbnet" => VBNET_KEYWORDS.contains(&name.to_ascii_lowercase().as_str()),
            _ => false,
        }
    }

    /// Whether two resolved names name one type. Two sets of opened names reach one library type
    /// in Java, C#, and VB.NET when one set holds the other, since a name that two opened
    /// packages or namespaces hold does not compile; in Kotlin and Scala a star import outranks
    /// the default imports, so the sets must be equal.
    fn compare(&self, one: &Named, other: &Named) -> Same {
        use Named::{Library, Opened, Project, Written};
        match (one, other) {
            (Project(a), Project(b)) | (Library(a), Library(b)) => {
                if a == b {
                    Same::Yes
                } else {
                    Same::No
                }
            }
            (Project(_), Library(_)) | (Library(_), Project(_)) => Same::No,
            (Written(a), Written(b)) | (Written(a), Library(b)) | (Library(b), Written(a))
                if a == b =>
            {
                Same::Yes
            }
            (Library(full), Opened(opens)) | (Opened(opens), Library(full)) => {
                let package = full.rsplit_once('.').map_or("", |(package, _)| package);
                if opens.names.contains(package) {
                    Same::Yes
                } else {
                    Same::Unknown
                }
            }
            (Opened(a), Opened(b)) if a.project == b.project => {
                let nested = a.names.is_subset(&b.names) || b.names.is_subset(&a.names);
                let same = match self.language {
                    "kotlin" | "scala" => a.names == b.names,
                    _ => nested,
                };
                if same { Same::Yes } else { Same::Unknown }
            }
            _ => Same::Unknown,
        }
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
        let rows: Vec<(String, String, Option<String>)> = conn
            .prepare(sql)?
            .query_map(params![name, self.language], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<_, _>>()?;
        let types = if rows.len() > MAX_TYPES {
            None
        } else {
            let mut types = Vec::with_capacity(rows.len());
            for (id, path, parent) in rows {
                types.push(self.type_row(conn, &id, path, parent)?);
            }
            Some(Rc::new(types))
        };
        self.types
            .borrow_mut()
            .insert(name.to_string(), types.clone());
        Ok(types)
    }

    fn type_row(
        &self,
        conn: &Connection,
        id: &str,
        path: String,
        parent: Option<String>,
    ) -> Result<TypeRow, QueryError> {
        let chain = symbol_chain(conn, id)?;
        let nested = chain.iter().skip(1).any(|(_, kind, _)| kind != "namespace");
        let mut namespaces: Vec<&str> = chain
            .iter()
            .rev()
            .filter(|(_, kind, _)| kind == "namespace")
            .map(|(_, _, name)| name.as_str())
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
                .filter(|(_, kind, _)| kind != "namespace")
                .map(|(_, _, name)| name.as_str()),
        );
        let identity = segments.join(".");
        let fold = |text: String| {
            if self.language == "vbnet" {
                text.to_ascii_lowercase()
            } else {
                text
            }
        };
        Ok(TypeRow {
            identity: fold(identity),
            path,
            namespace: fold(namespace),
            nested,
            parent,
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
            format!("{written}\u{0}{name}"),
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
            let suffix = self.fold(&path.join("."));
            let matched: Vec<&TypeRow> = types
                .iter()
                .filter(|row| {
                    row.identity == suffix || row.identity.ends_with(&format!(".{suffix}"))
                })
                .collect();
            if !matched.is_empty() || !self.scoped() {
                return Ok(one_of(matched.into_iter()));
            }
            if self.aliases_in(conn, place, path[0])? {
                return Ok(Named::Unknown);
            }
            return Ok(Named::Written(suffix));
        }
        if let Some(scope) = place.scope {
            let chain = symbol_chain(conn, scope)?;
            for (id, _, _) in &chain {
                let enclosed: Vec<&TypeRow> = types
                    .iter()
                    .filter(|row| row.nested && row.parent.as_deref() == Some(id.as_str()))
                    .collect();
                if !enclosed.is_empty() {
                    return Ok(one_of(enclosed.into_iter()));
                }
            }
        }
        if types.iter().any(|row| row.nested) {
            return Ok(Named::Unknown);
        }
        let generic = written_generic(written, name);
        match self.language {
            "java" | "kotlin" | "scala" => self.jvm_resolve(conn, name, place, &types),
            "csharp" => self.dotnet_resolve(conn, name, generic, place, &types),
            "vbnet" => {
                self.dotnet_resolve(conn, &name.to_ascii_lowercase(), generic, place, &types)
            }
            _ => Ok(Named::Unknown),
        }
    }

    fn fold(&self, text: &str) -> String {
        if self.language == "vbnet" {
            text.to_ascii_lowercase()
        } else {
            text.to_string()
        }
    }

    /// Whether the file of `place` binds `first` as an import alias (C# `using J = …;`, VB.NET
    /// `Imports J = …`, Kotlin `import … as J`), which gives a qualified name another meaning.
    fn aliases_in(&self, conn: &Connection, place: Place, first: &str) -> Result<bool, QueryError> {
        let first = self.fold(first);
        Ok(file_imports(conn, place.path)?
            .iter()
            .any(|(signature, _)| match self.language {
                "csharp" => csharp_using_alias(signature).is_some_and(|(alias, _)| alias == first),
                "vbnet" => matches!(
                    vbnet_import(signature),
                    Some(VbImport::Alias(alias)) if alias == first
                ),
                "kotlin" | "scala" => signature
                    .split_once(" as ")
                    .is_some_and(|(_, alias)| alias.trim() == first),
                _ => false,
            }))
    }

    /// A simple Java, Kotlin, or Scala name: a type of the file, or the type that an import of
    /// the name writes (both, when they differ, leave it unknown); else the one type of the
    /// file's package, else the one type of a package that a star import names. Scala ranks a
    /// star import above the package of another file in some versions and below it in others,
    /// so a Scala name that both reach with different types is unknown. A name that reaches no
    /// project type names a library type.
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
                    .map_or(Named::Library(path), |row| {
                        Named::Project(row.identity.clone())
                    }),
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
        let text = std::fs::read_to_string(self.root.join(place.path)).unwrap_or_default();
        let nested_comments = !place.path.ends_with(".java");
        let stars: BTreeSet<String> = super::sections::star_imports(&text, nested_comments)
            .into_iter()
            .collect();
        let in_package: Vec<&TypeRow> = types
            .iter()
            .filter(|row| row.namespace == package)
            .collect();
        let starred: Vec<&TypeRow> = types
            .iter()
            .filter(|row| stars.contains(&row.namespace))
            .collect();
        match (in_package.is_empty(), starred.is_empty()) {
            (false, false) if self.language == "scala" => {
                let (local, imported) =
                    (one_of(in_package.into_iter()), one_of(starred.into_iter()));
                return Ok(if local == imported {
                    local
                } else {
                    Named::Unknown
                });
            }
            (false, _) => return Ok(one_of(in_package.into_iter())),
            (true, false) => return Ok(one_of(starred.into_iter())),
            (true, true) => {}
        }
        let builtins: &[&str] = match self.language {
            "java" => &JAVA_LANG,
            "kotlin" => &KOTLIN_BUILTINS,
            _ => &SCALA_BUILTINS,
        };
        if builtins.contains(&name) {
            let package = match self.language {
                "java" => "java.lang",
                other => other,
            };
            return Ok(Named::Library(format!("{package}.{name}")));
        }
        if self.language == "java" {
            let known: BTreeSet<&str> = JAVA_KNOWN
                .iter()
                .filter(|(package, names)| stars.contains(*package) && names.contains(&name))
                .map(|(package, _)| *package)
                .collect();
            match known.len() {
                0 => {}
                1 => {
                    let package = known.into_iter().next().unwrap_or_default();
                    return Ok(Named::Library(format!("{package}.{name}")));
                }
                _ => return Ok(Named::Unknown),
            }
        }
        let declared = self.declared(conn)?;
        Ok(Named::Opened(Rc::new(Opens {
            names: stars
                .into_iter()
                .filter(|star| !declared.contains(star))
                .collect(),
            project: String::new(),
        })))
    }

    /// A simple C# or VB.NET name (VB.NET in lower case): the one type of the innermost
    /// namespace that holds the code and has one, else the one type of a namespace that a
    /// `using` or `Imports` line of the file, a `global using`, or the build project opens. A
    /// `using` alias of the name, a `using` inside a namespace block that opens a namespace with
    /// such a type, and an MSBuild `<Using>` item leave it unknown. A name that reaches no
    /// project type names a library type.
    fn dotnet_resolve(
        &self,
        conn: &Connection,
        name: &str,
        generic: bool,
        place: Place,
        types: &[TypeRow],
    ) -> Result<Named, QueryError> {
        let Some(scope) = place.scope else {
            return Ok(Named::Unknown);
        };
        let vbnet = self.language == "vbnet";
        let project = self.build_project(place.path);
        if project.aliases.iter().any(|alias| alias == name) {
            return Ok(Named::Unknown);
        }
        let chain = symbol_chain(conn, scope)?;
        let mut usings: Vec<(String, bool, bool)> = Vec::new();
        for (signature, parent) in file_imports(conn, place.path)? {
            let top = parent.is_none();
            let applies = parent
                .as_deref()
                .is_none_or(|parent| chain.iter().any(|(id, _, _)| id == parent));
            if vbnet {
                match vbnet_import(&signature) {
                    Some(VbImport::Alias(alias)) if alias == name => return Ok(Named::Unknown),
                    Some(VbImport::Namespace(namespace)) => usings.push((namespace, top, applies)),
                    _ => {}
                }
                continue;
            }
            if csharp_using_alias(&signature).is_some_and(|(alias, _)| alias == name) {
                return Ok(Named::Unknown);
            }
            if let Some(namespace) = csharp_using_namespace(&signature) {
                usings.push((namespace.to_string(), top, applies));
            } else if let Some(class) = csharp_using_static(&signature) {
                usings.push((format!("static {class}"), top, applies));
            }
        }
        let globals: Vec<String> = if vbnet {
            Vec::new()
        } else {
            conn.prepare(GLOBAL_USINGS_SQL)?
                .query_map([], |row| row.get::<_, String>(0))?
                .filter_map(|row| row.ok())
                .filter_map(|signature| csharp_using_namespace(&signature).map(str::to_string))
                .collect()
        };
        let opens_namespace = |namespace: &str, file_level: bool| {
            usings
                .iter()
                .any(|(using, top, _)| *top == file_level && using == namespace)
                || (file_level
                    && (globals.iter().any(|global| global == namespace)
                        || project.opens.iter().any(|open| open == namespace)))
        };
        if types
            .iter()
            .any(|row| opens_namespace(&row.namespace, false))
        {
            return Ok(Named::Unknown);
        }
        let enclosing: Vec<String> = chain
            .iter()
            .rev()
            .filter(|(_, kind, _)| kind == "namespace")
            .map(|(_, _, name)| self.fold(name))
            .collect();
        let mut levels = Vec::new();
        let mut level = enclosing.join(".");
        loop {
            levels.push(level.clone());
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
        if !vbnet && !types.is_empty() && self.msbuild_usings(conn)? {
            return Ok(Named::Unknown);
        }
        let opened = one_of(
            types
                .iter()
                .filter(|row| opens_namespace(&row.namespace, true)),
        );
        if opened != Named::Unknown
            || types
                .iter()
                .any(|row| opens_namespace(&row.namespace, true))
        {
            return Ok(opened);
        }
        let mut names: BTreeSet<String> = usings
            .into_iter()
            .filter(|(_, _, applies)| *applies)
            .map(|(using, _, _)| using)
            .collect();
        names.extend(project.opens.iter().cloned());
        names.extend(levels.into_iter().filter(|level| !level.is_empty()));
        let known: BTreeSet<&str> = DOTNET_KNOWN
            .iter()
            .filter(|(namespace, arity, types)| {
                arity.fits(generic)
                    && names.contains(&self.fold(namespace))
                    && types.iter().any(|known| self.fold(known) == name)
            })
            .map(|(namespace, _, _)| *namespace)
            .collect();
        match known.len() {
            0 => {}
            1 => {
                let namespace = known.into_iter().next().unwrap_or_default();
                return Ok(Named::Library(self.fold(&format!("{namespace}.{name}"))));
            }
            _ => return Ok(Named::Unknown),
        }
        let declared = self.declared(conn)?;
        names.retain(|namespace| !declared.contains(namespace));
        Ok(Named::Opened(Rc::new(Opens {
            names,
            project: project.folder.clone(),
        })))
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

    /// The packages (Java, Kotlin, Scala) or namespaces (C#, VB.NET, in lower case) that the
    /// project declares, whose types the index lists.
    fn declared(&self, conn: &Connection) -> Result<&HashSet<String>, QueryError> {
        if let Some(declared) = self.declared.get() {
            return Ok(declared);
        }
        let mut declared = HashSet::new();
        if is_jvm(self.language) {
            let signatures: Vec<String> = conn
                .prepare(PACKAGES_SQL)?
                .query_map(params![self.language], |row| row.get(0))?
                .collect::<Result<_, _>>()?;
            declared.extend(
                signatures
                    .iter()
                    .filter_map(|signature| package_name(signature)),
            );
        } else {
            let rows: Vec<(String, String, Option<String>)> = conn
                .prepare(NAMESPACES_SQL)?
                .query_map(params![self.language], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?
                .collect::<Result<_, _>>()?;
            let by_id: HashMap<&str, (&str, Option<&str>)> = rows
                .iter()
                .map(|(id, name, parent)| (id.as_str(), (name.as_str(), parent.as_deref())))
                .collect();
            for (id, _, _) in &rows {
                let mut parts = Vec::new();
                let mut next = Some(id.as_str());
                while let Some((name, parent)) = next.and_then(|id| by_id.get(id)) {
                    parts.push(*name);
                    next = *parent;
                    if parts.len() > 32 {
                        break;
                    }
                }
                parts.reverse();
                declared.insert(self.fold(&parts.join(".")));
            }
        }
        Ok(self.declared.get_or_init(|| declared))
    }

    /// The nearest `.csproj` (C#) or `.vbproj` (VB.NET) above `path`, with the namespaces it
    /// opens in each of its files: its `<Using Include>` or `<Import Include>` items, and the
    /// SDK's implicit usings when it turns them on and removes none.
    fn build_project(&self, path: &str) -> Rc<BuildProject> {
        let extension = if self.language == "vbnet" {
            "vbproj"
        } else {
            "csproj"
        };
        let mut folder = self.root.join(path);
        folder.pop();
        let start = folder.clone();
        if let Some(project) = self.projects.borrow().get(&start) {
            return project.clone();
        }
        let found = loop {
            let file = std::fs::read_dir(&folder).ok().and_then(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .find(|path| path.extension().is_some_and(|found| found == extension))
            });
            if let Some(file) = file {
                let text = std::fs::read_to_string(&file).unwrap_or_default();
                let relative = folder
                    .strip_prefix(&self.root)
                    .map(|relative| relative.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                break self.read_build_project(relative, &text);
            }
            if folder == self.root || !folder.pop() || !folder.starts_with(&self.root) {
                break BuildProject::default();
            }
        };
        let project = Rc::new(found);
        self.projects.borrow_mut().insert(start, project.clone());
        project
    }

    fn read_build_project(&self, folder: String, text: &str) -> BuildProject {
        let item = if self.language == "vbnet" {
            "<Import"
        } else {
            "<Using"
        };
        let mut opens = Vec::new();
        let mut aliases = Vec::new();
        let mut removes = false;
        for element in text.split(item).skip(1) {
            let element = element.split('>').next().unwrap_or_default();
            if !element.starts_with(char::is_whitespace) {
                continue;
            }
            if attribute(element, "Remove").is_some() {
                removes = true;
                continue;
            }
            let Some(include) = attribute(element, "Include") else {
                continue;
            };
            if let Some(alias) = attribute(element, "Alias") {
                aliases.push(self.fold(&alias));
            } else if attribute(element, "Static").is_some_and(|value| value == "true") {
                opens.push(format!("static {include}"));
            } else {
                opens.push(self.fold(&include));
            }
        }
        let implicit = text.contains("<ImplicitUsings>enable</ImplicitUsings>")
            || text.contains("<ImplicitUsings>true</ImplicitUsings>");
        if self.language == "csharp" && implicit && !removes {
            opens.extend(
                IMPLICIT_USINGS
                    .iter()
                    .map(|namespace| namespace.to_string()),
            );
        }
        BuildProject {
            folder,
            opens,
            aliases,
        }
    }
}

/// The value of the XML attribute `name` in the text of one element.
fn attribute(element: &str, name: &str) -> Option<String> {
    let at = element.find(&format!("{name}=\""))?;
    let value = &element[at + name.len() + 2..];
    Some(value[..value.find('"')?].trim().to_string())
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

/// Whether `name` takes type arguments where `written` writes it (`List<int>`).
fn written_generic(written: &str, name: &str) -> bool {
    let mut rest = written;
    while let Some(at) = rest.find(name) {
        let before = rest[..at].chars().next_back();
        let after = &rest[at + name.len()..];
        if before.is_none_or(|c| !is_word(c)) && !after.starts_with(is_word) {
            return after.trim_start().starts_with(['<', '(']);
        }
        rest = after;
    }
    false
}

/// The chain of `id` and the symbols that hold it, innermost first: id, kind, name.
fn symbol_chain(conn: &Connection, id: &str) -> Result<Vec<(String, String, String)>, QueryError> {
    Ok(conn
        .prepare(CHAIN_SQL)?
        .query_map(params![id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<_, _>>()?)
}

/// The import rows of a file: the signature and the symbol (a namespace block) that holds it.
fn file_imports(
    conn: &Connection,
    path: &str,
) -> Result<Vec<(String, Option<String>)>, QueryError> {
    Ok(conn
        .prepare(FILE_IMPORTS_SQL)?
        .query_map(params![path], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?)
}

fn package_of(conn: &Connection, path: &str) -> Result<Option<String>, QueryError> {
    let signature: Option<String> = conn
        .prepare(PACKAGE_SQL)?
        .query_row(params![path], |row| row.get(0))
        .optional()?;
    Ok(signature.as_deref().and_then(package_name))
}

fn package_name(signature: &str) -> Option<String> {
    let name = signature
        .strip_prefix("package ")?
        .trim()
        .trim_end_matches(';');
    Some(name.trim().to_string())
}

/// The class of a C# `using static C;`.
fn csharp_using_static(signature: &str) -> Option<&str> {
    let signature = signature.trim();
    let directive = signature.strip_prefix("global ").unwrap_or(signature);
    let class = directive
        .strip_prefix("using ")?
        .trim()
        .strip_prefix("static ")?
        .trim()
        .trim_end_matches(';')
        .trim();
    (!class.is_empty()).then_some(class)
}

#[derive(Debug, PartialEq, Eq)]
enum VbImport {
    /// The namespace or type an `Imports` line opens, in lower case.
    Namespace(String),
    /// The alias of an `Imports A = …` line, in lower case.
    Alias(String),
}

/// A VB.NET `Imports` line; `None` for an XML namespace import.
fn vbnet_import(signature: &str) -> Option<VbImport> {
    let text = signature.trim();
    let (head, rest) = text.split_at(text.find(char::is_whitespace)?);
    if !head.eq_ignore_ascii_case("imports") {
        return None;
    }
    let rest = rest.trim();
    if rest.starts_with('<') || rest.is_empty() {
        return None;
    }
    Some(match rest.split_once('=') {
        Some((alias, _)) => VbImport::Alias(alias.trim().to_ascii_lowercase()),
        None => VbImport::Namespace(rest.to_ascii_lowercase()),
    })
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
    let mut found: Option<String> = None;
    for (signature, _) in file_imports(conn, path)? {
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
    use super::{
        VbImport, attribute, csharp_using_static, qualified_path, type_words, unqualified,
        vbnet_import, written_generic,
    };

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

    #[test]
    fn a_name_is_generic_where_type_arguments_follow_it() {
        assert!(written_generic("List<int>", "List"));
        assert!(!written_generic("List<int>", "int"));
        assert!(!written_generic("Exception", "Exception"));
        assert!(written_generic("list(ofinteger)", "list"));
        assert!(!written_generic("IList<Box>", "List"));
    }

    #[test]
    fn a_vbnet_imports_line_opens_a_namespace_or_binds_an_alias_in_lower_case() {
        assert_eq!(
            vbnet_import("Imports System.Text.Json"),
            Some(VbImport::Namespace("system.text.json".into()))
        );
        assert_eq!(
            vbnet_import("Imports J = Newtonsoft.Json"),
            Some(VbImport::Alias("j".into()))
        );
        assert_eq!(vbnet_import("Imports <xmlns:a=\"urn:a\">"), None);
        assert_eq!(
            csharp_using_static("using static System.Math;"),
            Some("System.Math")
        );
        assert_eq!(csharp_using_static("using System;"), None);
    }

    #[test]
    fn an_msbuild_attribute_is_read_from_its_element_text() {
        let element = " Include=\"App.Core\" Alias=\"Core\" /";
        assert_eq!(attribute(element, "Include").as_deref(), Some("App.Core"));
        assert_eq!(attribute(element, "Alias").as_deref(), Some("Core"));
        assert_eq!(attribute(element, "Static"), None);
    }
}
