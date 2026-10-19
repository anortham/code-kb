use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params, params_from_iter};

use crate::models::{
    CallingTest, ConstructionSite, LabeledSite, OverrideRow, Registration, SiteLabel, Symbol,
    VariantUse,
};
use crate::queries::{
    QueryError, ROUTE_FAMILIES, get_symbol_by_id, has_column, has_table, is_test_path,
    matches_family, names_a_test, path_proximity,
};
use crate::test_commands::{CommandFinder, TestClass, TestSpec};

use super::{Accounting, is_constructor};

const MAX_DEPTH: usize = 8;
const MAX_TYPES: usize = 50;
const MAX_OVERRIDDEN: usize = 5;
const EDGE_TIME_LIMIT: Duration = Duration::from_secs(1);
const JVM_LANGUAGES: [&str; 4] = ["java", "kotlin", "scala", "groovy"];
const MAX_IMPL_HEADER_LINES: usize = 20;
const ITEM_KEYWORDS: [&str; 14] = [
    "pub",
    "trait",
    "fn",
    "mod",
    "struct",
    "enum",
    "union",
    "type",
    "const",
    "static",
    "extern",
    "use",
    "macro_rules!",
    "async",
];
const CLASS_LIKE_KINDS: [&str; 6] = [
    "class",
    "struct",
    "record",
    "interface",
    "trait",
    "protocol",
];
const TYPE_KINDS: [&str; 8] = [
    "class",
    "struct",
    "record",
    "interface",
    "trait",
    "protocol",
    "enum",
    "union",
];
const CALLABLE_KINDS: [&str; 2] = ["method", "function"];
const BUILDS_THE_CLASS: &str = "builds the class";
const NOT_BUILT_AFTER: [&str; 8] = [
    "impl", "for", "struct", "enum", "union", "trait", "type", "dyn",
];
const BRACE_SCAN_BYTES: usize = 4096;
const HANDLER_KEYS: [&str; 6] = [
    "view_target",
    "handler",
    "handler_name",
    "controller_action",
    "view_name",
    "function_name",
];
const ID_TIED_ROUTING: [&str; 4] = [
    "decorator_routing",
    "attribute_routing",
    "annotation_routing",
    "attribute",
];
const MORE_ROUTE_FAMILIES: [&str; 2] = [".request_mapping.", ".url_pattern."];
const MAX_HELPER_DEPTH: usize = 4;
const MAX_CLASS_DEPTH: usize = 8;
const LANGUAGE_OF_SQL: &str = "SELECT language FROM symbols WHERE symbol_id = ?1";
const IMPORT_ROWS_SQL: &str = "SELECT COALESCE(signature, ''),
                CASE WHEN json_valid(metadata_json)
                     THEN COALESCE(json_extract(metadata_json, '$.source'), '') ELSE '' END
         FROM symbols WHERE kind = 'import' AND path = ?1
           AND (name = ?2 OR substr(name, -length(?2) - 1) = '.' || ?2)";
const JVM_PACKAGE_SQL: &str = "SELECT signature FROM symbols
             WHERE path = ?1 AND kind = 'namespace' AND parent_symbol_id IS NULL
               AND signature LIKE 'package %'
             ORDER BY start_line LIMIT 1";
const VALID_METADATA: &str =
    "CASE WHEN json_valid(sf.metadata_json) THEN sf.metadata_json ELSE '{}' END";

/// SQL that is true when the symbol `alias` can be extended or implemented. An enum cannot,
/// except a Python one, which is a class that has an `Enum` base and may have no members.
fn base_kind_sql(alias: &str) -> String {
    format!(
        "({alias}.kind IN ({}) OR ({alias}.kind = 'enum' AND {alias}.language = 'python'))",
        sql_list(&CLASS_LIKE_KINDS)
    )
}

fn sql_list(kinds: &[&str]) -> String {
    kinds
        .iter()
        .map(|kind| format!("'{kind}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What the callers report adds after the sites are labeled. Nothing here changes a label,
/// a basis, or a count.
#[derive(Default)]
pub(super) struct ReachSections {
    pub overrides: Vec<OverrideRow>,
    pub overridden: Vec<OverrideRow>,
    pub constructions: Vec<ConstructionSite>,
    pub variant_uses: Vec<VariantUse>,
    pub registrations: Vec<Registration>,
    pub tests: Vec<CallingTest>,
    pub notes: Vec<String>,
}

/// The sections of the callers report for `target`. The type walk reuses the prepared statements
/// of `accounting`, which labeled the sites.
pub(super) fn reach_sections<'a>(
    accounting: &mut Accounting<'a>,
    target: &Symbol,
    sites: &[LabeledSite],
) -> Result<ReachSections, QueryError> {
    let (conn, root) = (accounting.conn, accounting.root);
    let mut sections = ReachSections::default();
    if let Some(owner) = dispatch_owner(conn, target)? {
        let mut edges = TypeEdges::new(accounting);
        let mut impls = RustImpls::new(root);
        let subtypes = walk(&mut edges, &owner, Step::Subtypes)?;
        sections.overrides =
            methods_named(conn, &subtypes.types, &target.name, |_, row, language| {
                if language != "rust" {
                    return Ok(true);
                }
                match impls.context(&row.path, row.line) {
                    ImplContext::Trait(written) => {
                        written.names(&mut edges.rust, &owner, &row.path, row.line)
                    }
                    _ => Ok(false),
                }
            })?;
        sections.overrides.sort_by(|a, b| {
            a.path
                .cmp(&b.path)
                .then_with(|| a.line.cmp(&b.line))
                .then_with(|| a.symbol_id.cmp(&b.symbol_id))
        });
        if subtypes.type_limit_cut {
            sections.notes.push(format!(
                "overrides are listed for the first {MAX_TYPES} subtypes of `{}` only",
                owner.name
            ));
        }
        let ancestors = walk(&mut edges, &owner, Step::Supertypes)?.types;
        let own_impl =
            (target.language == "rust").then(|| impls.context(&target.path, target.start_line));
        sections.overridden =
            methods_named(
                conn,
                &ancestors,
                &target.name,
                |ancestor, _, _| match &own_impl {
                    Some(ImplContext::Trait(written)) => {
                        written.names(&mut edges.rust, ancestor, &target.path, target.start_line)
                    }
                    Some(ImplContext::Inherent) => Ok(false),
                    _ => Ok(true),
                },
            )?;
        sections.overridden.truncate(MAX_OVERRIDDEN);
        if edges.out_of_time() {
            sections.notes.push(format!(
                "some base and derived types were not checked against their imports within the {} s time limit",
                EDGE_TIME_LIMIT.as_secs()
            ));
        }
    }
    match target.kind.as_str() {
        "class" | "struct" | "record" => {
            sections.constructions = constructions(root, &target.name, sites);
        }
        "enum" => sections.variant_uses = variant_uses(conn, root, target, sites)?,
        _ => {}
    }
    sections.registrations = registrations(conn, target, sites)?;
    sections.tests = calling_tests(conn, root, target, sites)?;
    Ok(sections)
}

struct TypeRef {
    id: String,
    name: String,
    path: String,
}

/// The class, struct, record, interface, or trait that owns a callable target which calls can
/// reach through dynamic dispatch. A constructor is never dispatched.
fn dispatch_owner(conn: &Connection, target: &Symbol) -> Result<Option<TypeRef>, QueryError> {
    let callable = matches!(target.kind.as_str(), "method" | "function");
    let Some(parent) = target.parent_symbol_id.as_deref() else {
        return Ok(None);
    };
    if !callable || is_constructor(target) {
        return Ok(None);
    }
    Ok(get_symbol_by_id(conn, parent)?
        .filter(|owner| TYPE_KINDS.contains(&owner.kind.as_str()))
        .map(|owner| TypeRef {
            id: owner.symbol_id,
            name: owner.name,
            path: owner.path,
        }))
}

/// Types that extend or implement `?1` (named `?2`, in file `?3`): through resolved rows, and
/// through pending rows that name it. After the path, the columns say whether the row is
/// pending, whether a nearer type of that name exists, the module path the row writes before the
/// name, the file and line that write it, and the receiver the row writes right before the name.
fn subtype_sql() -> String {
    let kinds = sql_list(&TYPE_KINDS);
    let rival_kind = base_kind_sql("rival");
    let rival = path_proximity("rival.path", "pe.path");
    let own = path_proximity("?3", "pe.path");
    format!(
        "SELECT s.symbol_id, s.name, s.path, 0 AS pending, 0 AS nearer, '[]' AS written,
                s.path AS written_in, '' AS receiver, 0 AS written_line
         FROM relationships r
         JOIN symbols s ON s.symbol_id = r.from_symbol_id
         WHERE r.to_symbol_id = ?1 AND r.kind IN ('extends', 'implements')
           AND s.kind IN ({kinds})
         UNION
         SELECT s.symbol_id, s.name, s.path, 1,
                EXISTS (SELECT 1 FROM symbols rival
                        WHERE rival.name = ?2 AND {rival_kind}
                          AND {rival} > {own}),
                pe.target_namespace_json, pe.path, COALESCE(pe.target_receiver, ''), pe.start_line
         FROM pending_relationships pe
         JOIN symbols s ON s.symbol_id = pe.from_symbol_id
         WHERE pe.target_terminal_name = ?2 AND pe.kind IN ('extends', 'implements')
           AND s.kind IN ({kinds})
         ORDER BY 3, 1, 4"
    )
}

/// SQL for the name that the import row `import` binds: the name an aliased TypeScript, Python, or
/// Kotlin import takes from its module (`Base` of `import { Base as B }`), or the last segment of
/// the target of a C# alias (`Base1` of `using B = N.Base1`). `NULL` for a namespace or default
/// import and for any other row.
fn imported_name_sql(import: &str) -> String {
    let target = format!(
        "trim(replace(substr({import}.signature, instr({import}.signature, '=') + 1), ';', ''))"
    );
    let last_segment =
        format!("replace({target}, rtrim({target}, replace({target}, '.', '')), '')");
    format!(
        "CASE WHEN json_valid({import}.metadata_json)
                   AND json_extract({import}.metadata_json, '$.importedName') NOT IN ('*', 'default')
              THEN json_extract({import}.metadata_json, '$.importedName')
              WHEN {import}.signature LIKE 'using % = %' THEN {last_segment}
         END"
    )
}

/// Types that `?1` extends or implements, through resolved rows and through pending rows that
/// name them, also by the name that an import alias gives them. After the path, the columns are
/// those of `subtype_sql`.
fn supertype_sql() -> String {
    let base_kind = base_kind_sql("b");
    let rival_kind = base_kind_sql("rival");
    let candidate_kind = base_kind_sql("c");
    let rival = path_proximity("rival.path", "pe.path");
    let candidate = path_proximity("c.path", "pe.path");
    let imported = imported_name_sql("alias");
    format!(
        "SELECT b.symbol_id, b.name, b.path, 0 AS pending, 0 AS nearer, '[]' AS written,
                b.path AS written_in, '' AS receiver, 0 AS written_line
         FROM relationships r
         JOIN symbols b ON b.symbol_id = r.to_symbol_id
         WHERE r.from_symbol_id = ?1 AND r.kind IN ('extends', 'implements')
           AND {base_kind}
         UNION
         SELECT c.symbol_id, c.name, c.path, 1,
                EXISTS (SELECT 1 FROM symbols rival
                        WHERE rival.name = c.name AND {rival_kind}
                          AND {rival} > {candidate}),
                pe.target_namespace_json, pe.path, COALESCE(pe.target_receiver, ''), pe.start_line
         FROM pending_relationships pe
         JOIN symbols c ON c.name = pe.target_terminal_name AND {candidate_kind}
         WHERE pe.from_symbol_id = ?1 AND pe.kind IN ('extends', 'implements')
         UNION
         SELECT c.symbol_id, c.name, c.path, 1,
                EXISTS (SELECT 1 FROM symbols rival
                        WHERE rival.name = c.name AND {rival_kind}
                          AND {rival} > {candidate}),
                pe.target_namespace_json, pe.path, COALESCE(pe.target_receiver, ''), pe.start_line
         FROM pending_relationships pe
         JOIN symbols alias ON alias.kind = 'import' AND alias.path = pe.path
                           AND (alias.name = pe.target_terminal_name
                                OR (json_valid(alias.metadata_json)
                                    AND json_extract(alias.metadata_json, '$.alias')
                                        = pe.target_terminal_name))
         JOIN symbols c ON c.name = {imported} AND {candidate_kind}
         WHERE pe.from_symbol_id = ?1 AND pe.kind IN ('extends', 'implements')
         ORDER BY 3, 1, 4"
    )
}

#[derive(Clone, Copy)]
enum Step {
    Subtypes,
    Supertypes,
}

impl Step {
    fn sql(self) -> String {
        match self {
            Step::Subtypes => subtype_sql(),
            Step::Supertypes => supertype_sql(),
        }
    }

    /// How many of a type's id, name, and path the statement binds.
    fn bound(self) -> usize {
        match self {
            Step::Subtypes => 3,
            Step::Supertypes => 1,
        }
    }
}

/// Decides which pending `extends` and `implements` rows tie a type to the base they name. The
/// callers report's own evidence (imports, language, scope) judges each row once per base.
struct TypeEdges<'a, 'b> {
    conn: &'a Connection,
    root: &'a Path,
    accounting: &'b mut Accounting<'a>,
    labels: HashMap<String, HashMap<String, (SiteLabel, String)>>,
    jvm_owners: HashMap<String, Option<String>>,
    star_imports: HashMap<String, Vec<String>>,
    rust: RustPaths<'a>,
}

impl<'a, 'b> TypeEdges<'a, 'b> {
    fn new(accounting: &'b mut Accounting<'a>) -> Self {
        accounting.restart_clock(EDGE_TIME_LIMIT);
        TypeEdges {
            conn: accounting.conn,
            root: accounting.root,
            rust: RustPaths::new(accounting.conn, accounting.root),
            accounting,
            labels: HashMap::new(),
            jvm_owners: HashMap::new(),
            star_imports: HashMap::new(),
        }
    }

    fn out_of_time(&self) -> bool {
        self.accounting.out_of_time()
    }

    /// Whether the pending row of `subtype` that names `base` stays an edge. A Rust module path
    /// written before the name, or a bare Rust name that its module explains, decides first, by
    /// the trait it names, because the callers report matches a crate name to a package folder
    /// only. Evidence that ties the name to another definition (another language, an unindexed
    /// import, a scope) drops it next; what the file writes about the name (a qualifier, an
    /// import, a star import) decides after that, even against a nearer type of that name; with
    /// no such evidence the nearest type of that name wins.
    fn keeps(
        &mut self,
        base: &TypeRef,
        subtype: &TypeRef,
        nearer_type_exists: bool,
        written: &WrittenPath,
    ) -> Result<bool, QueryError> {
        if let Some(reaches) = written.reaches(&mut self.rust, base)? {
            return Ok(reaches);
        }
        if !self.labels.contains_key(&base.id) {
            let labels = match get_symbol_by_id(self.conn, &base.id)? {
                Some(symbol) => self.accounting.inheritance_labels(&symbol)?,
                None => HashMap::new(),
            };
            self.labels.insert(base.id.clone(), labels);
        }
        if let Some((SiteLabel::Excluded, _)) = self.labels[&base.id].get(&subtype.id) {
            return Ok(false);
        }
        Ok(self
            .file_ties(base, subtype, written)?
            .unwrap_or(!nearer_type_exists))
    }

    /// Whether what the file of `subtype` writes about the name of `base` points at `base`:
    /// yes when it reaches only `base`, no when it reaches other definitions and not `base`, and
    /// `None` when the file says nothing or the evidence reaches several.
    fn file_ties(
        &mut self,
        base: &TypeRef,
        subtype: &TypeRef,
        written: &WrittenPath,
    ) -> Result<Option<bool>, QueryError> {
        let Some(language) = self
            .accounting
            .with_statement(LANGUAGE_OF_SQL, |statement| {
                statement
                    .query_row(params![subtype.id], |row| row.get::<_, String>(0))
                    .optional()
            })?
        else {
            return Ok(None);
        };
        if JVM_LANGUAGES.contains(&language.as_str()) {
            return self.jvm_ties(base, subtype, written);
        }
        let python = language == "python";
        if python && let Some(ties) = self.python_qualifier_ties(base, subtype, written)? {
            return Ok(Some(ties));
        }
        let reached = self
            .accounting
            .imported_definitions(&subtype.path, &language, &base.name)?;
        match reached.as_slice() {
            [] if python => self.python_unreached_ties(base, subtype),
            [] => Ok(None),
            _ if !reached.contains(&base.id) => Ok(Some(false)),
            [_] => Ok(Some(true)),
            _ => Ok(None),
        }
    }

    /// The rule for a Java, Kotlin, Scala, or Groovy file. julie records no source for such an
    /// import, so an import line is read as its written package and name; it records no row for
    /// a Java star import, so the star imports are read from the file. A qualifier written before
    /// the name decides first: it must name the package and enclosing types of `base`, as written,
    /// through an import of its first name, or under the package of the file or a star import. An
    /// import of the name decides next. With star imports and no import of the name, a type of
    /// the package of the file wins; else only the package or outer type that a star import names
    /// counts. `None` when the file of `base` has no package line, or the file has no qualifier,
    /// no import of the name, and no star import.
    fn jvm_ties(
        &mut self,
        base: &TypeRef,
        subtype: &TypeRef,
        written: &WrittenPath,
    ) -> Result<Option<bool>, QueryError> {
        let Some(owner) = self.jvm_owner(base)? else {
            return Ok(None);
        };
        let package = jvm_package(self.accounting, &subtype.path)?;
        if let Some(qualifier) = written.qualifier() {
            let head = qualifier.split('.').next().unwrap_or_default();
            let candidates = match jvm_imported(self.accounting, &subtype.path, head)? {
                Some(imported) => vec![format!("{imported}{}", &qualifier[head.len()..])],
                None => std::iter::once(qualifier.clone())
                    .chain(
                        package
                            .iter()
                            .cloned()
                            .chain(self.star_imports(&subtype.path))
                            .map(|place| format!("{place}.{qualifier}")),
                    )
                    .collect(),
            };
            return Ok(Some(candidates.contains(&owner)));
        }
        if let Some(imported) = jvm_imported(self.accounting, &subtype.path, &base.name)? {
            return Ok(Some(imported == format!("{owner}.{}", base.name)));
        }
        let stars = self.star_imports(&subtype.path);
        if stars.is_empty() {
            return Ok(None);
        }
        if package.as_deref() == Some(owner.as_str()) {
            return Ok(Some(true));
        }
        if let Some(package) = &package
            && jvm_package_defines(self.conn, package, &base.name)?
        {
            return Ok(Some(false));
        }
        Ok(Some(stars.contains(&owner)))
    }

    /// What holds the Java, Kotlin, Scala, or Groovy type `base`, read once per type.
    fn jvm_owner(&mut self, base: &TypeRef) -> Result<Option<String>, QueryError> {
        if let Some(owner) = self.jvm_owners.get(&base.id) {
            return Ok(owner.clone());
        }
        let owner = jvm_container(self.accounting, base)?;
        self.jvm_owners.insert(base.id.clone(), owner.clone());
        Ok(owner)
    }

    fn star_imports(&mut self, path: &str) -> Vec<String> {
        self.star_imports
            .entry(path.to_string())
            .or_insert_with(|| {
                let nested_comments = [".kt", ".kts", ".scala", ".sc"]
                    .iter()
                    .any(|extension| path.ends_with(extension));
                std::fs::read_to_string(self.root.join(path))
                    .map(|text| star_imports(&text, nested_comments))
                    .unwrap_or_default()
            })
            .clone()
    }

    /// A Python qualifier of two or more names before the base (`pkg_a.base.Base`) names the
    /// module of `base`, when its first name is bound by no import or only by a plain
    /// `import a.b`. An alias or a `from` import gives the first name another meaning, and a
    /// single name is an import binding that the callers report judges.
    fn python_qualifier_ties(
        &mut self,
        base: &TypeRef,
        subtype: &TypeRef,
        written: &WrittenPath,
    ) -> Result<Option<bool>, QueryError> {
        let Some(qualifier) = written
            .qualifier()
            .filter(|qualifier| qualifier.contains('.'))
        else {
            return Ok(None);
        };
        let head = qualifier.split('.').next().unwrap_or_default();
        let bound = import_rows(self.accounting, &subtype.path, head)?;
        if bound
            .iter()
            .any(|(signature, _)| !is_plain_import(signature))
        {
            return Ok(None);
        }
        let container = python_container(self.accounting, base)?;
        Ok(Some(
            format!(".{container}").ends_with(&format!(".{qualifier}")),
        ))
    }

    /// A Python name that no import of the file ties to a project definition. An import from a
    /// module outside the project ties the name outside it, so no project type is the base. With
    /// no import of the name, the star imports of the file name the module or package that holds
    /// the base. `None` for a relative import or when the file has neither.
    fn python_unreached_ties(
        &mut self,
        base: &TypeRef,
        subtype: &TypeRef,
    ) -> Result<Option<bool>, QueryError> {
        let named = import_rows(self.accounting, &subtype.path, &base.name)?;
        if !named.is_empty() {
            for (_, source) in &named {
                if !self.accounting.source_is_external(source)? {
                    return Ok(None);
                }
            }
            return Ok(Some(false));
        }
        let stars = import_rows(self.accounting, &subtype.path, "*")?;
        if stars.is_empty()
            || stars
                .iter()
                .any(|(_, source)| source.is_empty() || source.starts_with('.'))
        {
            return Ok(None);
        }
        let container = format!(".{}.", python_container(self.accounting, base)?);
        Ok(Some(stars.iter().any(|(_, source)| {
            container.contains(&format!(".{source}."))
        })))
    }
}

/// The signature and source of each import in `path` that binds `name`. julie names a Kotlin or
/// Scala import by its dotted name (`b.Base`), so `name` also matches the end of such a name.
fn import_rows(
    accounting: &Accounting,
    path: &str,
    name: &str,
) -> Result<Vec<(String, String)>, QueryError> {
    accounting.with_statement(IMPORT_ROWS_SQL, |statement| {
        statement
            .query_map(params![path, name], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()
    })
}

fn is_plain_import(signature: &str) -> bool {
    let signature = signature.trim_start();
    signature.starts_with("import ") && !signature.contains(" as ")
}

/// The dotted name that the import of `name` in `path` writes (`b.Outer` for `import b.Outer;`).
fn jvm_imported(
    accounting: &Accounting,
    path: &str,
    name: &str,
) -> Result<Option<String>, QueryError> {
    Ok(import_rows(accounting, path, name)?
        .iter()
        .find_map(|(signature, _)| qualified_import(signature).map(str::to_string)))
}

/// The package that the file `path` declares.
fn jvm_package(accounting: &Accounting, path: &str) -> Result<Option<String>, QueryError> {
    let signature: Option<String> = accounting
        .with_statement(JVM_PACKAGE_SQL, |statement| {
            statement
                .query_row(params![path], |row| row.get(0))
                .optional()
        })?
        .flatten();
    Ok(signature
        .as_deref()
        .and_then(|signature| signature.strip_prefix("package "))
        .map(|name| name.trim().trim_end_matches(';').trim().to_string()))
}

/// Whether a file of `package` declares a top-level type named `name`.
fn jvm_package_defines(conn: &Connection, package: &str, name: &str) -> Result<bool, QueryError> {
    Ok(conn
        .prepare(&format!(
            "SELECT 1 FROM symbols t
             JOIN symbols ns ON ns.path = t.path AND ns.kind = 'namespace'
                            AND ns.parent_symbol_id IS NULL AND ns.name = ?1
             WHERE t.name = ?2 AND t.parent_symbol_id IS NULL AND {}
             LIMIT 1",
            base_kind_sql("t")
        ))?
        .exists(params![package, name])?)
}

/// The names of the types and other symbols around `id`, outermost first.
fn enclosing_names(conn: &Connection, id: &str) -> Result<Vec<String>, QueryError> {
    let mut statement = conn.prepare(
        "WITH RECURSIVE chain(id, depth) AS (
             SELECT parent_symbol_id, 1 FROM symbols
             WHERE symbol_id = ?1 AND parent_symbol_id IS NOT NULL
             UNION ALL
             SELECT s.parent_symbol_id, chain.depth + 1
             FROM chain JOIN symbols s ON s.symbol_id = chain.id
             WHERE s.parent_symbol_id IS NOT NULL AND chain.depth < 8
         )
         SELECT p.name FROM chain JOIN symbols p ON p.symbol_id = chain.id
         ORDER BY chain.depth DESC",
    )?;
    Ok(statement
        .query_map(params![id], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?)
}

/// What holds a Java, Kotlin, Scala, or Groovy type, as a dotted name: its package, then the
/// types around it (`b.Outer` for `Inner` nested in `Outer` in package `b`). `None` when the
/// file has no package line.
fn jvm_container(accounting: &Accounting, base: &TypeRef) -> Result<Option<String>, QueryError> {
    let Some(package) = jvm_package(accounting, &base.path)? else {
        return Ok(None);
    };
    let mut parts = vec![package];
    parts.extend(enclosing_names(accounting.conn, &base.id)?);
    Ok(Some(parts.join(".")))
}

/// What holds a Python class, as a dotted name: its module, then the classes around it.
fn python_container(accounting: &Accounting, base: &TypeRef) -> Result<String, QueryError> {
    let mut parts = vec![python_module(&base.path)];
    parts.extend(enclosing_names(accounting.conn, &base.id)?);
    Ok(parts.join("."))
}

/// The dotted module of a Python file: `pkg/base.py` is `pkg.base`, `pkg/__init__.py` is `pkg`.
fn python_module(path: &str) -> String {
    let path = path.replace('\\', "/");
    let stem = path
        .rsplit_once('.')
        .map_or(path.as_str(), |(stem, _)| stem);
    stem.strip_suffix("/__init__")
        .unwrap_or(stem)
        .replace('/', ".")
}

/// The package or type that each `import a.b.*;` (Scala `import a.b._`) line of `text` names,
/// outside comments and string and character literals. Kotlin and Scala nest block comments.
fn star_imports(text: &str, nested_comments: bool) -> Vec<String> {
    blank_non_code(text, nested_comments)
        .lines()
        .filter_map(|line| {
            let line = line.trim().strip_prefix("import ")?;
            let line = line.trim_end_matches(';').trim();
            let line = line.strip_prefix("static ").unwrap_or(line);
            line.strip_suffix(".*")
                .or_else(|| line.strip_suffix("._"))
                .map(|place| place.trim().to_string())
        })
        .collect()
}

/// `text` with every comment, string literal, and character literal replaced by spaces, newlines
/// kept. A single-quoted or double-quoted literal ends at its closing quote or at the end of its
/// line, so a stray quote blanks no more than its own line.
fn blank_non_code(text: &str, nested_comments: bool) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let rest = &bytes[at..];
        let end = if rest.starts_with(b"//") {
            at + rest.iter().position(|b| *b == b'\n').unwrap_or(rest.len())
        } else if rest.starts_with(b"/*") {
            block_comment_end(bytes, at, nested_comments)
        } else if rest.starts_with(b"\"\"\"") || rest.starts_with(b"'''") {
            let fence = &rest[..3];
            at + 3
                + rest[3..]
                    .windows(3)
                    .position(|window| window == fence)
                    .map_or(rest.len() - 3, |close| close + 3)
        } else if matches!(rest[0], b'"' | b'\'') {
            quoted_end(bytes, at)
        } else {
            out.push(bytes[at]);
            at += 1;
            continue;
        };
        out.extend(
            bytes[at..end]
                .iter()
                .map(|byte| if *byte == b'\n' { b'\n' } else { b' ' }),
        );
        at = end;
    }
    String::from_utf8(out).unwrap_or_default()
}

/// The byte after the `*/` that closes the block comment opened at `open`, or the end of `bytes`.
fn block_comment_end(bytes: &[u8], open: usize, nested: bool) -> usize {
    let mut depth = 1;
    let mut at = open + 2;
    while at + 1 < bytes.len() {
        match &bytes[at..at + 2] {
            b"*/" => {
                depth -= 1;
                at += 2;
                if depth == 0 {
                    return at;
                }
            }
            b"/*" if nested => {
                depth += 1;
                at += 2;
            }
            _ => at += 1,
        }
    }
    bytes.len()
}

/// The byte after the quote that closes the literal opened at `open`, or the line end.
fn quoted_end(bytes: &[u8], open: usize) -> usize {
    let mut at = open + 1;
    while at < bytes.len() && bytes[at] != b'\n' {
        match bytes[at] {
            b'\\' => {
                at += if bytes.get(at + 1) == Some(&b'\n') {
                    1
                } else {
                    2
                }
            }
            quote if quote == bytes[open] => return at + 1,
            _ => at += 1,
        }
    }
    at.min(bytes.len())
}

/// The dotted name an `import a.b.C;` line writes; an aliased import (`import a.b.C as D`) binds
/// another name and writes none.
fn qualified_import(signature: &str) -> Option<&str> {
    let text = signature.trim().strip_prefix("import")?.trim_start();
    let text = text.strip_prefix("static ").unwrap_or(text);
    let text = text.trim().trim_end_matches(';').trim();
    (text.contains('.') && !text.contains(" as ")).then_some(text)
}

/// The path a pending `extends` or `implements` row writes before the type name (the modules and
/// the receiver), and the file and line that hold the row.
struct WrittenPath {
    modules: Vec<String>,
    file: String,
    line: usize,
    receiver: String,
}

impl WrittenPath {
    /// Whether the path names `base`. A bare Rust name is read by the module that writes it;
    /// `None` for a path that is not Rust or a bare name that its module does not explain.
    fn reaches(&self, paths: &mut RustPaths, base: &TypeRef) -> Result<Option<bool>, QueryError> {
        if !self.file.ends_with(".rs") {
            return Ok(None);
        }
        if self.modules.is_empty() {
            return paths.reaches_bare(&self.file, self.line, base);
        }
        paths
            .reaches(&self.modules, &self.file, self.line, base)
            .map(Some)
    }

    /// The dotted qualifier written before the name (`b.x` of `extends b.x.Base`); `None` when
    /// the name stands alone.
    fn qualifier(&self) -> Option<String> {
        let mut parts = self.modules.clone();
        if !self.receiver.is_empty() {
            parts.push(self.receiver.clone());
        }
        (!parts.is_empty()).then(|| parts.join("."))
    }
}

struct Reached {
    candidate: TypeRef,
    pending: bool,
    nearer_type_exists: bool,
    written: WrittenPath,
}

/// The types a walk reached, and whether a limit cut it before it reached the rest.
struct Walked {
    types: Vec<TypeRef>,
    type_limit_cut: bool,
    depth_limit_cut: bool,
}

/// The types reached from `start` by `step`, nearest first, to depth `MAX_DEPTH` and at most
/// `MAX_TYPES` types.
fn walk(edges: &mut TypeEdges, start: &TypeRef, step: Step) -> Result<Walked, QueryError> {
    let sql = step.sql();
    let mut statement = edges.conn.prepare(&sql)?;
    let mut found: Vec<TypeRef> = Vec::new();
    let mut seen = vec![start.id.clone()];
    let mut frontier = vec![TypeRef {
        id: start.id.clone(),
        name: start.name.clone(),
        path: start.path.clone(),
    }];
    let mut depth_limit_cut = false;
    for level in 1..=MAX_DEPTH {
        let mut next = Vec::new();
        for node in &frontier {
            let args = [&node.id, &node.name, &node.path];
            let reached = statement
                .query_map(params_from_iter(args[..step.bound()].iter()), |row| {
                    Ok(Reached {
                        candidate: TypeRef {
                            id: row.get(0)?,
                            name: row.get(1)?,
                            path: row.get(2)?,
                        },
                        pending: row.get::<_, i64>(3)? != 0,
                        nearer_type_exists: row.get::<_, i64>(4)? != 0,
                        written: WrittenPath {
                            modules: serde_json::from_str(&row.get::<_, String>(5)?)
                                .unwrap_or_default(),
                            file: row.get(6)?,
                            receiver: row.get(7)?,
                            line: row.get::<_, i64>(8)? as usize,
                        },
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            for Reached {
                candidate,
                pending,
                nearer_type_exists,
                written,
            } in reached
            {
                if seen.contains(&candidate.id) {
                    continue;
                }
                if pending {
                    let (base, subtype) = match step {
                        Step::Subtypes => (node, &candidate),
                        Step::Supertypes => (&candidate, node),
                    };
                    if !edges.keeps(base, subtype, nearer_type_exists, &written)? {
                        continue;
                    }
                }
                if found.len() == MAX_TYPES {
                    return Ok(Walked {
                        types: found,
                        type_limit_cut: true,
                        depth_limit_cut,
                    });
                }
                seen.push(candidate.id.clone());
                next.push(TypeRef {
                    id: candidate.id.clone(),
                    name: candidate.name.clone(),
                    path: candidate.path.clone(),
                });
                found.push(candidate);
            }
        }
        if next.is_empty() {
            break;
        }
        depth_limit_cut = level == MAX_DEPTH;
        frontier = next;
    }
    Ok(Walked {
        types: found,
        type_limit_cut: false,
        depth_limit_cut,
    })
}

/// The callable members named like `target` in every type that the owner of `target` extends or
/// implements. `None` when a limit cut the walk or the time ran out, so a base may be missing.
pub(super) fn inherited_methods(
    accounting: &mut Accounting,
    target: &Symbol,
) -> Result<Option<Vec<Symbol>>, QueryError> {
    let conn = accounting.conn;
    let Some(owner) = dispatch_owner(conn, target)? else {
        return Ok(Some(Vec::new()));
    };
    let mut edges = TypeEdges::new(accounting);
    let walked = walk(&mut edges, &owner, Step::Supertypes)?;
    if walked.type_limit_cut || walked.depth_limit_cut || edges.out_of_time() {
        return Ok(None);
    }
    let rows = methods_named(conn, &walked.types, &target.name, |_, _, _| Ok(true))?;
    let mut methods = Vec::with_capacity(rows.len());
    for row in rows {
        match get_symbol_by_id(conn, &row.symbol_id)? {
            Some(method) => methods.push(method),
            None => return Ok(None),
        }
    }
    Ok(Some(methods))
}

/// The callable members of `types` named `name` that `keep` accepts, given the type, the member,
/// and its language, in the order of `types`.
fn methods_named(
    conn: &Connection,
    types: &[TypeRef],
    name: &str,
    mut keep: impl FnMut(&TypeRef, &OverrideRow, &str) -> Result<bool, QueryError>,
) -> Result<Vec<OverrideRow>, QueryError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT symbol_id, path, start_line, language FROM symbols
         WHERE parent_symbol_id = ?1 AND name = ?2 AND kind IN ({})
         ORDER BY start_line, symbol_id",
        sql_list(&CALLABLE_KINDS)
    ))?;
    let mut rows = Vec::new();
    for owner in types {
        let found = stmt
            .query_map(params![owner.id, name], |row| {
                Ok((
                    OverrideRow {
                        symbol_id: row.get(0)?,
                        owner: owner.name.clone(),
                        name: name.to_string(),
                        path: row.get::<_, String>(1)?.replace('\\', "/"),
                        line: row.get::<_, i64>(2)? as usize,
                    },
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for (row, language) in found {
            if keep(owner, &row, &language)? {
                rows.push(row);
            }
        }
    }
    Ok(rows)
}

/// The trait an `impl` header names: the modules written before its name, and its name.
#[derive(Debug, Clone, PartialEq)]
struct TraitPath {
    modules: Vec<String>,
    name: String,
}

impl TraitPath {
    /// Whether the header on `impl_line` of the file `impl_path` names `candidate`. A path with
    /// modules matches only the trait that the modules name. A bare name matches the trait that
    /// its module reads for it, else every trait of that name.
    fn names(
        &self,
        paths: &mut RustPaths,
        candidate: &TypeRef,
        impl_path: &str,
        impl_line: usize,
    ) -> Result<bool, QueryError> {
        if self.name != candidate.name {
            return Ok(false);
        }
        if self.modules.is_empty() {
            return Ok(paths
                .reaches_bare(impl_path, impl_line, candidate)?
                .unwrap_or(true));
        }
        paths.reaches(&self.modules, impl_path, impl_line, candidate)
    }
}

/// What the impl block around a Rust method says about the trait it implements.
#[derive(Debug, Clone, PartialEq)]
enum ImplContext {
    Trait(TraitPath),
    Inherent,
    Outside,
}

const MAX_REEXPORT_HOPS: usize = 1;

/// Where a Rust item or `use` sits: the folder of its package (the path before the last `/src/`)
/// and its module path from the crate root. The path comes from the file below `src/` (`ovr/a.rs`
/// and `ovr/mod.rs` hold `ovr::a` and `ovr`; `lib.rs` and `main.rs` hold the root) and the inline
/// `mod` blocks around the item. The crate name, which other crates write before a path, starts as
/// the folder of the package with `-` read as `_`; `RustPaths::place` takes the `[package]` name
/// of the `Cargo.toml` in that folder instead.
#[derive(Debug, Clone, PartialEq)]
struct Place {
    package: Option<String>,
    crate_name: Option<String>,
    file: String,
    modules: Vec<String>,
}

impl Place {
    fn new(path: &str, inline_modules: Vec<String>) -> Self {
        let file = format!("/{}", path.replace('\\', "/"));
        let src = file.rfind("/src/");
        let mut modules: Vec<String> = src.map_or_else(Vec::new, |at| {
            let below = &file[at + "/src/".len()..];
            below
                .strip_suffix(".rs")
                .unwrap_or(below)
                .split('/')
                .map(str::to_string)
                .collect()
        });
        let is_root = matches!(modules.as_slice(), [only] if only == "lib" || only == "main");
        if is_root || modules.last().is_some_and(|last| last == "mod") {
            modules.pop();
        }
        modules.extend(inline_modules);
        let package = src.map(|at| file[..at].to_string());
        let crate_name = package.as_deref().and_then(|package| {
            let folder = package.rsplit('/').next()?;
            (!folder.is_empty()).then(|| folder.replace('-', "_"))
        });
        Place {
            package,
            crate_name,
            file,
            modules,
        }
    }

    /// Whether `self` is in the crate of `from`. A file outside any `src/` is a crate of its own.
    fn in_crate_of(&self, from: &Place) -> bool {
        self.package == from.package && (self.package.is_some() || self.file == from.file)
    }
}

enum CrateScope<'a> {
    Own,
    Named(&'a str),
}

/// The module paths that `written` can mean when it is written at `from`, in the order Rust
/// tries them. A leading `crate`, `self`, or `super` has one meaning, a leading crate name is a
/// path in that crate, and any other path is read from the module of `from` first and then from
/// the crate root. A path rooted at `std`, `core`, or `alloc` has none.
fn module_ways<'a>(from: &Place, written: &'a [String]) -> Vec<(CrateScope<'a>, Vec<String>)> {
    let Some((head, rest)) = written.split_first() else {
        return Vec::new();
    };
    match head.as_str() {
        "std" | "core" | "alloc" | "Self" => Vec::new(),
        "crate" => vec![(CrateScope::Own, rest.to_vec())],
        "self" | "super" => {
            let mut modules = from.modules.clone();
            let mut rest = if head == "self" { rest } else { written };
            while rest.first().is_some_and(|segment| segment == "super") {
                if modules.pop().is_none() {
                    return Vec::new();
                }
                rest = &rest[1..];
            }
            modules.extend_from_slice(rest);
            vec![(CrateScope::Own, modules)]
        }
        name => vec![
            (CrateScope::Named(name), rest.to_vec()),
            (CrateScope::Own, [from.modules.as_slice(), written].concat()),
            (CrateScope::Own, written.to_vec()),
        ],
    }
}

/// The Rust types of one name and the `use` rows that bind it, each with its place. A `use` row
/// carries the module path it imports from, as written.
#[derive(Default)]
struct RustNamed {
    types: Vec<(String, Place)>,
    uses: Vec<(Place, Vec<String>)>,
}

impl RustNamed {
    /// The ids of the types that the modules `written` name when written at `from`. A path that
    /// spells a module exactly decides. A relative path that spells none ends in the module
    /// path of one type only: that type. A `use` row in the named module that imports the name
    /// stands for the type it imports, for `MAX_REEXPORT_HOPS` hops.
    fn reached<'a>(&'a self, from: &Place, written: &[String]) -> Vec<&'a str> {
        let named = self.exact(from, written, MAX_REEXPORT_HOPS);
        let relative = written.first().is_some_and(|head| {
            !matches!(
                head.as_str(),
                "crate" | "self" | "super" | "std" | "core" | "alloc" | "Self"
            )
        });
        if !named.is_empty() || !relative {
            return named;
        }
        let ending: Vec<&str> = self
            .types
            .iter()
            .filter(|(_, place)| place.modules.ends_with(written))
            .map(|(id, _)| id.as_str())
            .collect();
        if ending.len() == 1 {
            ending
        } else {
            Vec::new()
        }
    }

    /// The ids of the types that a bare name means when written at `from`, read the way Rust
    /// reads it: a type in the module of `from`, else the `use` rows of that module that import
    /// the name, else the glob `use` rows of that module (`globs` holds those of its file). A
    /// `use` row or a glob stands for the types its source names, as `reached` reads that source.
    /// A `use` row that reaches no type decides only when its source is a std path or starts with
    /// a name in `external`, which name no project type; any other source may go through a chain
    /// that the index cannot follow. A glob decides only when it reaches a type. `None` when the
    /// module says nothing about the name.
    fn bare<'a>(
        &'a self,
        from: &Place,
        globs: &[(Place, Vec<String>)],
        external: &HashSet<String>,
    ) -> Option<Vec<&'a str>> {
        let here = |place: &Place| place.file == from.file && place.modules == from.modules;
        let own: Vec<&str> = self
            .types
            .iter()
            .filter(|(_, place)| here(place))
            .map(|(id, _)| id.as_str())
            .collect();
        if !own.is_empty() {
            return Some(own);
        }
        let imports: Vec<&Vec<String>> = self
            .uses
            .iter()
            .filter(|(place, _)| here(place))
            .map(|(_, source)| source)
            .collect();
        if !imports.is_empty() {
            let imported: Vec<&str> = imports
                .iter()
                .flat_map(|source| self.reached(from, source))
                .collect();
            let outside_project = imports.iter().all(|source| {
                source.first().is_some_and(|head| {
                    matches!(head.as_str(), "std" | "core" | "alloc") || external.contains(head)
                })
            });
            return (!imported.is_empty() || outside_project).then_some(imported);
        }
        let starred: Vec<&str> = globs
            .iter()
            .filter(|(place, _)| here(place))
            .flat_map(|(_, source)| self.reached(from, source))
            .collect();
        (!starred.is_empty()).then_some(starred)
    }

    fn exact<'a>(&'a self, from: &Place, written: &[String], hops: usize) -> Vec<&'a str> {
        for (scope, modules) in module_ways(from, written) {
            let holds = |place: &Place| {
                place.modules == modules
                    && match scope {
                        CrateScope::Own => place.in_crate_of(from),
                        CrateScope::Named(name) => place.crate_name.as_deref() == Some(name),
                    }
            };
            let found: Vec<&str> = self
                .types
                .iter()
                .filter(|(_, place)| holds(place))
                .map(|(id, _)| id.as_str())
                .collect();
            if !found.is_empty() {
                return found;
            }
            if hops == 0 {
                continue;
            }
            let followed: Vec<&str> = self
                .uses
                .iter()
                .filter(|(place, _)| holds(place))
                .flat_map(|(place, source)| self.exact(place, source, hops - 1))
                .collect();
            if !followed.is_empty() {
                return followed;
            }
        }
        Vec::new()
    }
}

/// Reads where the Rust types and `use` rows of a name sit, once per name, the glob `use` rows of
/// a file, once per file, and the crate name of a package, once per package.
struct RustPaths<'a> {
    conn: &'a Connection,
    root: &'a Path,
    named: HashMap<String, RustNamed>,
    globs: HashMap<String, Vec<(Place, Vec<String>)>>,
    crate_names: HashMap<String, Option<String>>,
    dependencies: HashMap<String, HashSet<String>>,
    packages: Option<HashSet<String>>,
    project_named: HashMap<String, bool>,
}

impl<'a> RustPaths<'a> {
    fn new(conn: &'a Connection, root: &'a Path) -> Self {
        RustPaths {
            conn,
            root,
            named: HashMap::new(),
            globs: HashMap::new(),
            crate_names: HashMap::new(),
            dependencies: HashMap::new(),
            packages: None,
            project_named: HashMap::new(),
        }
    }

    /// Whether the modules `written` before `candidate`'s name, on `line` of the file `path`,
    /// name `candidate`.
    fn reaches(
        &mut self,
        written: &[String],
        path: &str,
        line: usize,
        candidate: &TypeRef,
    ) -> Result<bool, QueryError> {
        let inline = self.inline_modules(path, line)?;
        let from = self.place(path, inline);
        self.load_named(&candidate.name)?;
        Ok(self.named[&candidate.name]
            .reached(&from, written)
            .contains(&candidate.id.as_str()))
    }

    /// Whether the bare name of `candidate`, written on `line` of the file `path`, means
    /// `candidate`; `None` when the module that writes it says nothing about the name.
    fn reaches_bare(
        &mut self,
        path: &str,
        line: usize,
        candidate: &TypeRef,
    ) -> Result<Option<bool>, QueryError> {
        let inline = self.inline_modules(path, line)?;
        let from = self.place(path, inline);
        self.load_named(&candidate.name)?;
        self.load_globs(path)?;
        let heads: Vec<String> = self.named[&candidate.name]
            .uses
            .iter()
            .filter(|(place, _)| place.file == from.file)
            .filter_map(|(_, source)| source.first().cloned())
            .collect();
        let mut external = HashSet::new();
        for head in heads {
            if self.is_external(path, &head)? {
                external.insert(head);
            }
        }
        Ok(self.named[&candidate.name]
            .bare(&from, &self.globs[path], &external)
            .map(|meant| meant.contains(&candidate.id.as_str())))
    }

    /// Whether the name `head`, the first segment of a `use` path in the file `path`, is a
    /// dependency of the nearest `Cargo.toml` and no project module, folder, package, or crate
    /// has that name. Such a path names an item outside the project.
    fn is_external(&mut self, path: &str, head: &str) -> Result<bool, QueryError> {
        let name = head.replace('-', "_");
        Ok(self.dependencies_of(path).contains(&name) && !self.project_has(&name)?)
    }

    /// The dependency names of the nearest `Cargo.toml` at or above the file `path`.
    fn dependencies_of(&mut self, path: &str) -> &HashSet<String> {
        let mut dir = path.rsplit_once('/').map_or("", |(parent, _)| parent);
        while !dir.is_empty() && !self.root.join(dir).join("Cargo.toml").is_file() {
            dir = dir.rsplit_once('/').map_or("", |(parent, _)| parent);
        }
        let root = self.root;
        self.dependencies.entry(dir.to_string()).or_insert_with(|| {
            std::fs::read_to_string(root.join(dir).join("Cargo.toml"))
                .map(|toml| dependency_names(&toml))
                .unwrap_or_default()
        })
    }

    /// Whether the project has a module, a folder or file in a Rust path, a package, or a crate
    /// called `name`.
    fn project_has(&mut self, name: &str) -> Result<bool, QueryError> {
        if let Some(known) = self.project_named.get(name) {
            return Ok(*known);
        }
        let in_paths: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM symbols
                           WHERE language = 'rust' AND kind IN ('namespace', 'module') AND name = ?1)
                 OR EXISTS(SELECT 1 FROM files WHERE language = 'rust'
                           AND (instr('/' || replace(replace(path, '\\', '/'), '-', '_'), '/' || ?1 || '/') > 0
                                OR instr('/' || replace(replace(path, '\\', '/'), '-', '_'), '/' || ?1 || '.rs') > 0))",
            params![name],
            |row| row.get(0),
        )?;
        let has = in_paths || self.package_names()?.contains(name);
        self.project_named.insert(name.to_string(), has);
        Ok(has)
    }

    /// The `[package]` names of the `Cargo.toml` files in the index, with `-` read as `_`.
    fn package_names(&mut self) -> Result<&HashSet<String>, QueryError> {
        if self.packages.is_none() {
            let mut statement = self.conn.prepare(
                "SELECT path FROM files
                 WHERE path = 'Cargo.toml' OR path LIKE '%/Cargo.toml' OR path LIKE '%\\Cargo.toml'",
            )?;
            let manifests = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            let names = manifests
                .iter()
                .filter_map(|manifest| {
                    let folder = manifest.replace('\\', "/");
                    let folder = folder.trim_end_matches("Cargo.toml").trim_end_matches('/');
                    cargo_package_name(self.root, folder)
                })
                .map(|name| name.replace('-', "_"))
                .collect();
            self.packages = Some(names);
        }
        Ok(self.packages.get_or_insert_default())
    }

    /// The place of the Rust item or `use` on a file `path`, with the crate name that the
    /// `Cargo.toml` of its package gives.
    fn place(&mut self, path: &str, inline_modules: Vec<String>) -> Place {
        let mut place = Place::new(path, inline_modules);
        if let Some(package) = &place.package {
            let name = self
                .crate_names
                .entry(package.clone())
                .or_insert_with(|| cargo_package_name(self.root, package));
            if let Some(name) = name {
                place.crate_name = Some(name.replace('-', "_"));
            }
        }
        place
    }

    /// The names of the inline `mod` blocks around `line` of `path`, outermost first.
    fn inline_modules(&self, path: &str, line: usize) -> Result<Vec<String>, QueryError> {
        let mut statement = self.conn.prepare(
            "SELECT name FROM symbols
             WHERE path = ?1 AND kind = 'namespace' AND start_line <= ?2 AND end_line >= ?2
             ORDER BY start_line",
        )?;
        Ok(statement
            .query_map(params![path, line as i64], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?)
    }

    fn load_named(&mut self, name: &str) -> Result<(), QueryError> {
        if !self.named.contains_key(name) {
            let loaded = self.load(name)?;
            self.named.insert(name.to_string(), loaded);
        }
        Ok(())
    }

    fn load(&mut self, name: &str) -> Result<RustNamed, QueryError> {
        let conn = self.conn;
        let mut named = RustNamed::default();
        let mut types = conn.prepare(&format!(
            "SELECT symbol_id, path FROM symbols
             WHERE name = ?1 AND language = 'rust' AND kind IN ({})",
            sql_list(&TYPE_KINDS)
        ))?;
        let types = types
            .query_map(params![name], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<(String, String)>, _>>()?;
        for (id, path) in types {
            let place = self.place(&path, enclosing_names(conn, &id)?);
            named.types.push((id, place));
        }
        let mut uses = conn.prepare(
            "SELECT s.symbol_id, s.path, p.target_namespace_json
             FROM symbols s
             JOIN pending_relationships p ON p.from_symbol_id = s.symbol_id
                  AND p.kind = 'imports' AND p.target_terminal_name = ?1
             WHERE s.kind = 'import' AND s.name = ?1 AND s.language = 'rust'",
        )?;
        let uses = uses
            .query_map(params![name], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<Vec<(String, String, String)>, _>>()?;
        for (id, path, source) in uses {
            let place = self.place(&path, enclosing_names(conn, &id)?);
            let source = serde_json::from_str(&source).unwrap_or_default();
            named.uses.push((place, source));
        }
        Ok(named)
    }

    /// The glob `use` rows of the file `path`, each with the module path it imports from.
    fn load_globs(&mut self, path: &str) -> Result<(), QueryError> {
        if self.globs.contains_key(path) {
            return Ok(());
        }
        let conn = self.conn;
        let mut statement = conn.prepare(
            "SELECT s.symbol_id, s.name, COALESCE(s.signature, ''), p.target_terminal_name,
                    COALESCE(p.target_namespace_json, '[]')
             FROM symbols s
             JOIN pending_relationships p ON p.from_symbol_id = s.symbol_id
                  AND p.kind = 'imports'
             WHERE s.path = ?1 AND s.kind = 'import' AND s.language = 'rust'",
        )?;
        let rows = statement
            .query_map(params![path], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut globs = Vec::new();
        for (id, name, signature, terminal, namespace) in rows {
            let mut source: Vec<String> = serde_json::from_str(&namespace).unwrap_or_default();
            if is_glob(&name, &terminal, &source, &signature) {
                source.push(terminal);
                let place = self.place(path, enclosing_names(conn, &id)?);
                globs.push((place, source));
            }
        }
        self.globs.insert(path.to_string(), globs);
        Ok(())
    }
}

/// Whether a Rust `use` row, as julie writes it, imports every item of a module (`use a::b::*;`).
/// julie names such a row by its whole path (`a::b`) and records the last name (`b`) as the
/// terminal name and the names before it as the namespace. A path of one name has no namespace,
/// so only the signature tells `use b::*;` from `use b;`.
fn is_glob(name: &str, terminal: &str, namespace: &[String], signature: &str) -> bool {
    if namespace.is_empty() {
        name == terminal && signature.contains(&format!("{terminal}::*"))
    } else {
        name == format!("{}::{terminal}", namespace.join("::"))
    }
}

/// The `[package]` name of the `Cargo.toml` in the package folder `package` (`/crates/core`, or
/// the empty string for a package at the project root).
fn cargo_package_name(root: &Path, package: &str) -> Option<String> {
    let manifest = root
        .join(package.trim_start_matches('/'))
        .join("Cargo.toml");
    crate::test_commands::package_name(&std::fs::read_to_string(manifest).ok()?)
}

/// The names a `Cargo.toml` gives its dependencies: the keys of `[dependencies]`,
/// `[dev-dependencies]`, `[build-dependencies]` and their `[target.….…]` forms, and the name in a
/// `[dependencies.name]` header. A rename (`name = { package = "real" }`) is known by `name`, the
/// way the code spells it. `-` is read as `_`.
fn dependency_names(toml: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut in_table = false;
    for line in toml.lines().map(str::trim) {
        if let Some(header) = line.strip_prefix('[') {
            let segments = toml_segments(
                header
                    .trim_start_matches('[')
                    .split(']')
                    .next()
                    .unwrap_or(""),
            );
            let kind = segments.iter().position(|segment| {
                matches!(
                    segment.as_str(),
                    "dependencies" | "dev-dependencies" | "build-dependencies"
                )
            });
            let direct = kind.filter(|at| *at == 0 || segments[0] == "target");
            in_table = direct.is_some_and(|at| segments.len() == at + 1);
            if let Some(name) = direct.and_then(|at| segments.get(at + 1)) {
                names.insert(name.replace('-', "_"));
            }
        } else if in_table
            && !line.starts_with('#')
            && let Some((key, _)) = line.split_once('=')
            && let Some(name) = toml_segments(key.trim()).first()
        {
            names.insert(name.replace('-', "_"));
        }
    }
    names
}

/// The dotted segments of a TOML key or table name, without their quotes. A dot inside a quoted
/// segment (`'cfg(a.b)'`) does not split it.
fn toml_segments(key: &str) -> Vec<String> {
    let mut segments = vec![String::new()];
    let mut quote = None;
    for c in key.chars() {
        match (c, quote) {
            ('\'' | '"', None) => quote = Some(c),
            (c, Some(open)) if c == open => quote = None,
            ('.', None) => segments.push(String::new()),
            (c, _) => segments.last_mut().expect("one segment").push(c),
        }
    }
    segments
        .iter()
        .map(|segment| segment.trim().to_string())
        .collect()
}

/// Reads the impl header of a Rust method from the source, once per file.
struct RustImpls<'a> {
    root: &'a Path,
    files: HashMap<String, Vec<String>>,
}

impl<'a> RustImpls<'a> {
    fn new(root: &'a Path) -> Self {
        RustImpls {
            root,
            files: HashMap::new(),
        }
    }

    fn context(&mut self, path: &str, line: usize) -> ImplContext {
        let lines = self.files.entry(path.to_string()).or_insert_with(|| {
            std::fs::read_to_string(self.root.join(path))
                .map(|text| text.lines().map(str::to_string).collect())
                .unwrap_or_default()
        });
        impl_context(lines, line)
    }
}

fn indent(text: &str) -> usize {
    text.len() - text.trim_start().len()
}

fn starts_word(text: &str, word: &str) -> bool {
    text.strip_prefix(word)
        .is_some_and(|after| after.is_empty() || after.starts_with(char::is_whitespace))
}

fn starts_impl(trimmed: &str) -> bool {
    let item = trimmed
        .strip_prefix("unsafe")
        .map_or(trimmed, str::trim_start);
    item.strip_prefix("impl").is_some_and(|after| {
        after.is_empty() || after.starts_with(|c: char| c == '<' || c.is_whitespace())
    })
}

fn starts_item(trimmed: &str) -> bool {
    let word = trimmed
        .split(|c: char| !(is_identifier_char(c) || c == '!'))
        .next()
        .unwrap_or("");
    ITEM_KEYWORDS.contains(&word)
}

/// The impl block that holds the method on `line` (1-based): the nearest `impl` line above it
/// with less indentation, or the line itself when the whole impl is on one line. An item
/// keyword on a less indented line first (`trait Sub: Base {`) means the method is not in an impl.
fn impl_context(lines: &[String], line: usize) -> ImplContext {
    let Some(own) = line.checked_sub(1).and_then(|index| lines.get(index)) else {
        return ImplContext::Outside;
    };
    let own_indent = indent(own);
    for index in (0..line).rev() {
        let text = &lines[index];
        let trimmed = text.trim_start();
        let own_line = index + 1 == line;
        let encloses = own_line || indent(text) < own_indent;
        if trimmed.is_empty() || trimmed.starts_with("//") || !encloses {
            continue;
        }
        if starts_impl(trimmed) {
            return parse_impl_header(&impl_header(lines, index));
        }
        if !own_line && starts_item(trimmed) {
            return ImplContext::Outside;
        }
    }
    ImplContext::Outside
}

/// The text of the item header that starts on line `at` (0-based), up to its first `{`.
fn impl_header(lines: &[String], at: usize) -> String {
    let mut header = String::new();
    for text in lines[at..].iter().take(MAX_IMPL_HEADER_LINES) {
        if let Some((before, _)) = text.split_once('{') {
            header.push_str(before);
            break;
        }
        header.push_str(text);
        header.push(' ');
    }
    header
}

/// `impl<T> path::Trait<A> for Type where …` is the trait `Trait` in the modules `path`;
/// `impl<T> Type<T>` is inherent.
fn parse_impl_header(header: &str) -> ImplContext {
    let item = header.trim_start();
    let item = item.strip_prefix("unsafe").map_or(item, str::trim_start);
    let Some(rest) = item.strip_prefix("impl").map(str::trim_start) else {
        return ImplContext::Outside;
    };
    let rest = if rest.starts_with('<') {
        after_balanced(rest, '<', '>').unwrap_or("")
    } else {
        rest
    };
    match implemented_trait(rest) {
        Some(path) => {
            let head = path.trim().trim_start_matches('!');
            let head = head.split('<').next().unwrap_or(head);
            let mut segments = head.rsplit("::").map(str::trim);
            let name = segments.next().unwrap_or("");
            if name.is_empty() {
                return ImplContext::Outside;
            }
            let mut modules: Vec<String> = segments
                .filter(|module| !module.is_empty())
                .map(str::to_string)
                .collect();
            modules.reverse();
            ImplContext::Trait(TraitPath {
                modules,
                name: name.to_string(),
            })
        }
        None => ImplContext::Inherent,
    }
}

/// The text before the first `for` outside any `<…>`, `(…)`, or `[…]`; a `where` ends the search.
fn implemented_trait(text: &str) -> Option<&str> {
    let mut depth = 0usize;
    let mut previous = ' ';
    for (at, c) in text.char_indices() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' if previous == '-' => {}
            '>' | ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 && previous.is_whitespace() => {
                if starts_word(&text[at..], "where") {
                    return None;
                }
                if starts_word(&text[at..], "for") {
                    return Some(&text[..at]);
                }
            }
            _ => {}
        }
        previous = c;
    }
    None
}

#[derive(Clone, Copy, PartialEq)]
enum BracedLanguage {
    Rust,
    Go,
}

fn braced_language(path: &str) -> Option<BracedLanguage> {
    match path.rsplit_once('.')?.1 {
        "rs" => Some(BracedLanguage::Rust),
        "go" => Some(BracedLanguage::Go),
        _ => None,
    }
}

/// The lines that build a value of the target type, one row per line, files in the order of
/// their first site. A site counts when it is a call, `new`, or the constructor step's
/// `builds the class`, or, in Rust and Go, when it is a type use that opens a literal (`Foo {`). julie writes some
/// literals (inside `vec![…]`, as a field value) as a variable read, so that counts too.
/// Excluded sites never count.
fn constructions(root: &Path, name: &str, sites: &[LabeledSite]) -> Vec<ConstructionSite> {
    let mut hits: Vec<(usize, &str, usize)> = Vec::new();
    let mut literals: BTreeMap<&str, Vec<(usize, usize, usize)>> = BTreeMap::new();
    for (index, labeled) in sites.iter().enumerate() {
        let site = &labeled.site;
        if labeled.label == SiteLabel::Excluded {
            continue;
        }
        let Some(line) = site.start_line else {
            continue;
        };
        if matches!(site.kind.as_str(), "calls" | "call" | "instantiates")
            || labeled.basis == BUILDS_THE_CLASS
        {
            hits.push((index, &site.path, line));
        } else if matches!(site.kind.as_str(), "type_usage" | "variable_ref")
            && braced_language(&site.path).is_some()
            && let Some(column) = site.start_column
        {
            literals
                .entry(&site.path)
                .or_default()
                .push((index, line, column));
        }
    }
    for (path, candidates) in literals {
        let (Some(language), Ok(text)) = (
            braced_language(path),
            std::fs::read_to_string(root.join(path)),
        ) else {
            continue;
        };
        let starts = line_starts(&text);
        for (index, line, column) in candidates {
            let opens_literal = line
                .checked_sub(1)
                .and_then(|index| starts.get(index))
                .is_some_and(|&start| line_builds(&text, start, column, name, language));
            if opens_literal {
                hits.push((index, path, line));
            }
        }
    }
    let mut first_site: HashMap<&str, usize> = HashMap::new();
    for &(index, path, _) in &hits {
        let first = first_site.entry(path).or_insert(index);
        *first = (*first).min(index);
    }
    hits.sort_by_key(|&(_, path, line)| (first_site[path], line));
    hits.dedup_by(|a, b| a.1 == b.1 && a.2 == b.2);
    hits.into_iter()
        .map(|(_, path, line)| ConstructionSite {
            path: path.to_string(),
            line,
        })
        .collect()
}

/// Whether the site of `name` at byte `column` of the line that starts at `line_start` builds a
/// value: any whole-word `name` in code on that line opens a literal.
fn line_builds(
    text: &str,
    line_start: usize,
    column: usize,
    name: &str,
    language: BracedLanguage,
) -> bool {
    name_columns(text, line_start, column, name, language)
        .into_iter()
        .any(|at| opens_a_literal(text, line_start, at, name, language))
}

fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(at, _)| at + 1))
        .collect()
}

fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The byte columns where `name` can open a literal on the line that starts at `line_start`: the
/// site's own `column`, then each other whole-word `name` in code, outside comments, strings, and
/// character literals. The report keeps one site per caller and line, so the site may be a
/// pattern while a literal of the same type stands beside it.
fn name_columns(
    text: &str,
    line_start: usize,
    column: usize,
    name: &str,
    language: BracedLanguage,
) -> Vec<usize> {
    let line = text
        .get(line_start..)
        .and_then(|rest| rest.split('\n').next())
        .unwrap_or_default();
    let mut columns = vec![column];
    let mut at = 0;
    while let Some(c) = line[at..].chars().next() {
        if let Some(end) = non_code_end(line, at, language) {
            at = end;
            continue;
        }
        if line[at..].starts_with(name)
            && at != column
            && !line[..at].ends_with(is_identifier_char)
            && !line[at + name.len()..].starts_with(is_identifier_char)
        {
            columns.push(at);
        }
        at += c.len_utf8();
    }
    columns
}

/// The byte after the comment, string, or character literal that opens at `open` of `line`, or
/// `None` when code stands there. One that does not close on the line runs to its end. Rust nests
/// block comments, reads raw strings (`r#"…"#`), and tells a character literal from a lifetime;
/// Go reads backtick strings and rune literals.
fn non_code_end(line: &str, open: usize, language: BracedLanguage) -> Option<usize> {
    let bytes = line.as_bytes();
    let rest = &line[open..];
    match bytes[open] {
        b'/' if rest.starts_with("//") => Some(line.len()),
        b'/' if rest.starts_with("/*") => Some(block_comment_end(
            bytes,
            open,
            language == BracedLanguage::Rust,
        )),
        b'"' => Some(quoted_end(bytes, open)),
        b'`' if language == BracedLanguage::Go => Some(
            rest[1..]
                .find('`')
                .map_or(line.len(), |close| open + close + 2),
        ),
        b'\'' => char_literal_end(line, open),
        b'r' | b'b' | b'c'
            if language == BracedLanguage::Rust && !line[..open].ends_with(is_identifier_char) =>
        {
            raw_string_end(line, open)
        }
        _ => None,
    }
}

/// The byte after the character literal (`'x'`, `'"'`, `'\''`, `'\u{22}'`) that opens at `open`;
/// `None` for a Rust lifetime (`'a`, `'static`).
fn char_literal_end(line: &str, open: usize) -> Option<usize> {
    let mut chars = line[open + 1..].chars();
    let first = chars.next()?;
    let escaped = first == '\\';
    let body = first.len_utf8() + if escaped { chars.next()?.len_utf8() } else { 0 };
    let tail = &line[open + 1 + body..];
    let close = if escaped {
        tail.find('\'')?
    } else {
        tail.starts_with('\'').then_some(0)?
    };
    Some(open + 1 + body + close + 1)
}

/// The byte after the Rust raw string (`r"…"`, `br#"…"#`, `cr##"…"##`) that opens at `open`, or
/// `None` when the text there is not one.
fn raw_string_end(line: &str, open: usize) -> Option<usize> {
    let after_prefix = line[open..]
        .strip_prefix(['b', 'c'])
        .unwrap_or(&line[open..]);
    let hashes = after_prefix.strip_prefix('r')?;
    let fence = hashes.len() - hashes.trim_start_matches('#').len();
    let body = hashes[fence..].strip_prefix('"')?;
    let closing = format!("\"{}", "#".repeat(fence));
    Some(body.find(&closing).map_or(line.len(), |close| {
        line.len() - body.len() + close + closing.len()
    }))
}

/// Whether `name` at byte `column` of the line that starts at `line_start` opens a struct
/// literal: a `{` follows it, after the generic arguments (`<…>` or `::<…>` in Rust, `[…]` in Go),
/// it does not stand where a type is declared or named, and its braces do not close into a
/// pattern.
fn opens_a_literal(
    text: &str,
    line_start: usize,
    column: usize,
    name: &str,
    language: BracedLanguage,
) -> bool {
    let at = line_start + column;
    let Some(after) = text
        .get(at..)
        .and_then(|rest| rest.strip_prefix(name))
        .and_then(|rest| rest.split('\n').next())
    else {
        return false;
    };
    if names_a_type(&text[line_start..at], language) {
        return false;
    }
    let (open, close) = match language {
        BracedLanguage::Rust => ('<', '>'),
        BracedLanguage::Go => ('[', ']'),
    };
    let mut rest = after.trim_start();
    let arguments = match language {
        BracedLanguage::Rust => rest.strip_prefix("::").unwrap_or(rest),
        BracedLanguage::Go => rest,
    };
    if arguments.starts_with(open) {
        let Some(remainder) = after_balanced(arguments, open, close) else {
            return false;
        };
        rest = remainder.trim_start();
    }
    if !rest.starts_with('{') {
        return false;
    }
    let brace_at = at + name.len() + (after.len() - rest.len());
    !(language == BracedLanguage::Rust && closes_into_a_pattern(&text[brace_at..], &text[..at]))
}

/// The text after the group that opens `text` with `open` and closes with the matching `close`;
/// a `>` that ends `->` does not close.
fn after_balanced(text: &str, open: char, close: char) -> Option<&str> {
    let mut depth = 0usize;
    let mut previous = ' ';
    for (at, c) in text.char_indices() {
        if c == open {
            depth += 1;
        } else if c == close && !(close == '>' && previous == '-') {
            depth -= 1;
            if depth == 0 {
                return Some(&text[at + c.len_utf8()..]);
            }
        }
        previous = c;
    }
    None
}

/// Whether `before`, the text of the line up to the name, ends where a type is declared or
/// named and not built: after `->`, `impl`, `for`, `struct`, `enum`, `union`, `trait`, `type`, or
/// `dyn`, after the generic parameters of an `impl`, or, in Go, after the parameters of a func.
/// Path qualifiers, `&`, `*`, `mut`, lifetimes, and Go `[]` and `map[…]` prefixes are skipped.
fn names_a_type(before: &str, language: BracedLanguage) -> bool {
    let mut before = before.trim_end();
    loop {
        let shorter = strip_qualifier(before, language)
            .or_else(|| strip_reference(before))
            .or_else(|| strip_go_collection(before, language));
        match shorter {
            Some(shorter) => before = shorter.trim_end(),
            None => break,
        }
    }
    if before.ends_with("->") || (language == BracedLanguage::Go && before.ends_with(')')) {
        return true;
    }
    if language == BracedLanguage::Rust && before.ends_with('>') {
        return before_generics(before).is_some_and(|rest| last_word(rest.trim_end()) == "impl");
    }
    NOT_BUILT_AFTER.contains(&last_word(before))
}

fn last_word(text: &str) -> &str {
    text.rsplit(|c: char| !is_identifier_char(c))
        .next()
        .unwrap_or("")
}

fn strip_qualifier(before: &str, language: BracedLanguage) -> Option<&str> {
    let separator = if language == BracedLanguage::Rust {
        "::"
    } else {
        "."
    };
    before
        .strip_suffix(separator)
        .map(|path| path.trim_end_matches(is_identifier_char))
}

fn strip_reference(before: &str) -> Option<&str> {
    if let Some(rest) = before.strip_suffix(['&', '*']) {
        return Some(rest);
    }
    if let Some(rest) = before.strip_suffix("mut")
        && !rest.ends_with(is_identifier_char)
    {
        return Some(rest);
    }
    let word = last_word(before);
    let rest = before.strip_suffix(word)?;
    (!word.is_empty()).then_some(rest)?.strip_suffix('\'')
}

fn strip_go_collection(before: &str, language: BracedLanguage) -> Option<&str> {
    if language != BracedLanguage::Go {
        return None;
    }
    if let Some(rest) = before.strip_suffix("map")
        && !rest.ends_with(is_identifier_char)
    {
        return Some(rest);
    }
    let group = before.strip_suffix(']')?;
    let mut depth = 1usize;
    for (at, c) in group.char_indices().rev() {
        match c {
            ']' => depth += 1,
            '[' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&group[..at]);
                }
            }
            _ => {}
        }
    }
    None
}

/// The text before the `<…>` group that ends `text`; a `->` inside the group does not close it.
fn before_generics(text: &str) -> Option<&str> {
    let mut depth = 0usize;
    let mut chars = text.char_indices().rev().peekable();
    while let Some((at, c)) = chars.next() {
        match c {
            '>' if chars.peek().is_some_and(|&(_, before)| before == '-') => {
                chars.next();
            }
            '>' => depth += 1,
            '<' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(&text[..at]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether the braces at the start of `text` close into a Rust pattern; `before` is the file
/// text up to the name that opens them. The scan climbs out of the groups around the braces
/// (`(…)`, `[…]`, `{…}`), skipping the rest of each, and reads what follows the outermost one:
/// `=>`, `=`, `|`, `:`, `if`, or `in` makes a pattern. The second argument of `matches!` is a
/// pattern too. A literal closes within `BRACE_SCAN_BYTES` or is taken as a literal.
fn closes_into_a_pattern(text: &str, before: &str) -> bool {
    let window = &text[..text.floor_char_boundary(BRACE_SCAN_BYTES)];
    let Some(mut rest) = after_balanced(window, '{', '}') else {
        return false;
    };
    let mut around = None;
    let mut climbed = 0;
    loop {
        rest = rest.trim_start();
        if ends_a_pattern(rest) {
            return true;
        }
        match rest.chars().next() {
            Some(closer @ (')' | ']' | '}')) => {
                let around = around.get_or_insert_with(|| Around::of(before));
                if closer == ')' && around.matches_arguments.get(climbed) == Some(&true) {
                    return true;
                }
                climbed += 1;
                rest = &rest[1..];
            }
            Some(',') => {
                let around = around.get_or_insert_with(|| Around::of(before));
                match skip_siblings(&rest[1..], around.in_closure_parameters) {
                    Some(next) => rest = next,
                    None => return false,
                }
            }
            _ => return false,
        }
    }
}

fn ends_a_pattern(tail: &str) -> bool {
    let single = |c: char, twice: &str| tail.starts_with(c) && !tail.starts_with(twice);
    tail.starts_with("=>")
        || single('=', "==")
        || single('|', "||")
        || single(':', "::")
        || ["if", "in"].iter().any(|word| {
            tail.strip_prefix(word)
                .is_some_and(|rest| !rest.starts_with(is_identifier_char))
        })
}

fn is_single_pipe(bytes: &[u8], at: usize) -> bool {
    at.checked_sub(1).and_then(|previous| bytes.get(previous)) != Some(&b'|')
        && bytes.get(at + 1) != Some(&b'|')
}

/// The text from the first closer that no opener in `text` matches, so the rest of a group after
/// a `,`; a `;` ends the search. In the parameters of a closure, a single `|` ends it too.
fn skip_siblings(text: &str, in_closure_parameters: bool) -> Option<&str> {
    let mut depth = 0usize;
    for (at, c) in text.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth == 0 => return Some(&text[at..]),
            ')' | ']' | '}' => depth -= 1,
            ';' if depth == 0 => return None,
            '|' if depth == 0 && in_closure_parameters && is_single_pipe(text.as_bytes(), at) => {
                return Some(&text[at..]);
            }
            _ => {}
        }
    }
    None
}

/// What the text before a name says about the groups around it.
struct Around {
    matches_arguments: Vec<bool>,
    in_closure_parameters: bool,
}

/// Whether a closure can start after `before`: at the start of the text, after an opener, a `,`,
/// a `;`, a `:`, an `=` or `=>`, another `|`, or the words `move`, `async`, and `return`. A `|`
/// anywhere else is a bit-or or an alternation in a pattern.
fn can_start_a_closure(before: &str) -> bool {
    let before = before.trim_end();
    before.is_empty()
        || before.ends_with(['(', '[', '{', ',', ';', ':', '=', '|'])
        || before.ends_with("=>")
        || ["move", "async", "return"].contains(&last_word(before))
}

impl Around {
    /// One flag per open group around the end of `before`, innermost first: whether the group is
    /// the argument list of `matches!` and a `,` stands between its opener and the end. Stops at
    /// a `;` or after `BRACE_SCAN_BYTES`. The end stands in the parameters of a closure when the
    /// nearest single `|` at its level comes where a closure can start. A `=>` at that level
    /// comes first for an arm of a `match`, so the `|` of the arm's pattern never counts.
    fn of(before: &str) -> Self {
        let mut matches_arguments = Vec::new();
        let (mut nested, mut comma) = (0usize, false);
        let mut in_closure_parameters = None;
        let scanned = before
            .char_indices()
            .rev()
            .take_while(|&(at, _)| before.len() - at <= BRACE_SCAN_BYTES);
        for (at, c) in scanned {
            let at_the_end_level =
                nested == 0 && matches_arguments.is_empty() && in_closure_parameters.is_none();
            match c {
                ')' | ']' | '}' => nested += 1,
                '(' | '[' | '{' if nested > 0 => nested -= 1,
                '(' | '[' | '{' => {
                    let is_matches =
                        c == '(' && comma && before[..at].trim_end().ends_with("matches!");
                    matches_arguments.push(is_matches);
                    comma = false;
                }
                ',' if nested == 0 => comma = true,
                ';' if nested == 0 => break,
                '|' if at_the_end_level && is_single_pipe(before.as_bytes(), at) => {
                    in_closure_parameters = Some(can_start_a_closure(&before[..at]));
                }
                '>' if at_the_end_level && before[..at].ends_with('=') => {
                    in_closure_parameters = Some(false);
                }
                _ => {}
            }
        }
        Self {
            matches_arguments,
            in_closure_parameters: in_closure_parameters == Some(true),
        }
    }
}

/// Whether the name at byte `column` of `line` follows the enum's name and `::` or `.` on its line,
/// after any path prefix.
fn follows_the_enum_name(
    text: &str,
    starts: &[usize],
    line: usize,
    column: usize,
    enum_name: &str,
) -> bool {
    let Some(&start) = line.checked_sub(1).and_then(|index| starts.get(index)) else {
        return false;
    };
    let Some(before) = text.get(start..start + column) else {
        return false;
    };
    let before = before.trim_end();
    before
        .strip_suffix("::")
        .or_else(|| before.strip_suffix('.'))
        .and_then(|path| path.trim_end().strip_suffix(enum_name))
        .is_some_and(|rest| !rest.ends_with(is_identifier_char))
}

struct CallerTally {
    caller: String,
    first_line: usize,
    used: BTreeSet<String>,
}

/// Whether `target` is a C or C++ enum whose enumerators are visible as bare names: its
/// declaration does not read `enum class` or `enum struct`. False when the source is unreadable.
fn is_unscoped_c_enum(root: &Path, target: &Symbol) -> bool {
    if !matches!(target.language.as_str(), "c" | "cpp") {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(root.join(&target.path)) else {
        return false;
    };
    let Some(declaration) = text.get(target.start_byte..) else {
        return false;
    };
    let header = declaration.split(['{', ';']).next().unwrap_or_default();
    let mut words = header
        .split(|c: char| !is_identifier_char(c))
        .filter(|word| !word.is_empty());
    words.by_ref().any(|word| word == "enum") && !matches!(words.next(), Some("class" | "struct"))
}

/// The functions that name variants of the enum `target`, most variants first, then by path. A
/// name counts when it is a variant and stands right after the enum's name and `::` or `.` on the
/// line of a site of the enum, or is a member access on the enum's name in a file that holds such
/// a site. A bare enumerator of an unscoped C or C++ enum also counts when it is a variable read
/// in a C or C++ file, on the line of a site of the enum or inside a function that holds a site
/// of the enum. A function that declares a symbol of that name (a parameter or a local) never
/// counts the enumerator, also on the line of a site. julie stores the enumerators of a C enum
/// as `constant` rows, so a C enum takes those as its variants.
fn variant_uses(
    conn: &Connection,
    root: &Path,
    target: &Symbol,
    sites: &[LabeledSite],
) -> Result<Vec<VariantUse>, QueryError> {
    let mut members = conn.prepare(
        "SELECT DISTINCT name FROM symbols
         WHERE parent_symbol_id = ?1 AND (kind = 'enum_member' OR (kind = 'constant' AND ?2 = 'c'))",
    )?;
    let variants = members
        .query_map(params![target.symbol_id, target.language], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if variants.is_empty() || !has_table(conn, "identifiers") {
        return Ok(Vec::new());
    }
    let mut lines: BTreeMap<&str, BTreeSet<usize>> = BTreeMap::new();
    for labeled in sites.iter().filter(|s| s.label != SiteLabel::Excluded) {
        let set = lines.entry(&labeled.site.path).or_default();
        set.extend(labeled.site.start_line);
    }
    let mut declares_name =
        conn.prepare("SELECT 1 FROM symbols WHERE parent_symbol_id = ?1 AND name = ?2 LIMIT 1")?;
    let mut site_function_ids = conn.prepare(
        "SELECT DISTINCT i.containing_symbol_id FROM identifiers i
         JOIN files f ON f.file_id = i.file_id
         WHERE f.path IN (?1, ?2) AND i.name = ?3 AND i.containing_symbol_id IS NOT NULL
           AND i.start_line IN (SELECT value FROM json_each(?4))",
    )?;
    let mut stmt = conn.prepare(
        "SELECT i.name, i.kind, i.start_line, i.start_column, COALESCE(i.containing_symbol_id, ''),
                COALESCE(s.name, ''),
                CASE WHEN json_valid(i.metadata_json)
                     THEN json_extract(i.metadata_json, '$.receiver') END,
                f.language
         FROM identifiers i
         JOIN files f ON f.file_id = i.file_id
         LEFT JOIN symbols s ON s.symbol_id = i.containing_symbol_id
         WHERE f.path IN (?1, ?2) AND i.name IN (SELECT value FROM json_each(?3))",
    )?;
    let names = serde_json::to_string(&variants).unwrap_or_default();
    let bare_enumerators = is_unscoped_c_enum(root, target);
    let mut callers: BTreeMap<(String, String), CallerTally> = BTreeMap::new();
    for (path, site_lines) in &lines {
        let site_functions: BTreeSet<String> = if bare_enumerators {
            site_function_ids
                .query_map(
                    params![
                        path,
                        path.replace('/', "\\"),
                        target.name,
                        serde_json::to_string(site_lines).unwrap_or_default()
                    ],
                    |row| row.get(0),
                )?
                .collect::<Result<_, _>>()?
        } else {
            BTreeSet::new()
        };
        let rows = stmt
            .query_map(params![path, path.replace('/', "\\"), names], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? as usize,
                    row.get::<_, i64>(3)? as usize,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, String>(7)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut source: Option<Option<(String, Vec<usize>)>> = None;
        for (name, kind, line, column, caller_id, caller, receiver, language) in rows {
            let qualified = kind == "member_access" && receiver.as_deref() == Some(&target.name);
            let bare = bare_enumerators
                && kind == "variable_ref"
                && matches!(language.as_str(), "c" | "cpp");
            if !qualified {
                if bare && declares_name.exists(params![caller_id, name])? {
                    continue;
                }
                let in_site_function = bare && site_functions.contains(&caller_id);
                if !site_lines.contains(&line) && !in_site_function {
                    continue;
                }
                let named_by_enum = bare
                    || source
                        .get_or_insert_with(|| {
                            let text = std::fs::read_to_string(root.join(path)).ok()?;
                            let starts = line_starts(&text);
                            Some((text, starts))
                        })
                        .as_ref()
                        .is_some_and(|(text, starts)| {
                            follows_the_enum_name(text, starts, line, column, &target.name)
                        });
                if !named_by_enum {
                    continue;
                }
            }
            let tally = callers
                .entry((path.to_string(), caller_id))
                .or_insert_with(|| CallerTally {
                    caller,
                    first_line: line,
                    used: BTreeSet::new(),
                });
            tally.first_line = tally.first_line.min(line);
            tally.used.insert(name);
        }
    }
    let mut rows: Vec<VariantUse> = callers
        .into_iter()
        .map(|((path, caller_id), tally)| VariantUse {
            caller: tally.caller,
            caller_id,
            path,
            line: tally.first_line,
            variants_used: tally.used.len(),
            variants_total: variants.len(),
        })
        .collect();
    rows.sort_by(|a, b| {
        b.variants_used
            .cmp(&a.variants_used)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.caller.cmp(&b.caller))
    });
    Ok(rows)
}

/// The distinct route-family pattern ids of the index, found by skipping through the pattern
/// index instead of scanning the facts.
fn route_pattern_ids(conn: &Connection) -> Result<Vec<String>, QueryError> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE p(pid) AS (
             SELECT MIN(pattern_id) FROM structural_facts
             UNION ALL
             SELECT (SELECT MIN(pattern_id) FROM structural_facts WHERE pattern_id > p.pid)
             FROM p WHERE pid IS NOT NULL)
         SELECT pid FROM p WHERE pid IS NOT NULL",
    )?;
    let ids = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids
        .into_iter()
        .filter(|id| {
            ROUTE_FAMILIES
                .iter()
                .chain(&MORE_ROUTE_FAMILIES)
                .any(|rule| matches_family(id, rule))
        })
        .collect())
}

struct RouteFact {
    id: String,
    end_line: usize,
    row: Registration,
}

/// SQL that is true when `value` is `?1` or ends in `.` and `?1`, so `views.cart_view` names
/// `cart_view`. A value that starts with `self.`, `cls.`, or `this.` names a member of the class
/// around the fact, so it counts only when `?3` is true: the target is a member of a type.
fn names_target(value: &str) -> String {
    format!(
        "({value} = ?1 OR (substr({value}, -(length(?1) + 1)) = '.' || ?1
           AND (?3 OR NOT ({value} GLOB 'self.*' OR {value} GLOB 'cls.*'
                           OR {value} GLOB 'this.*'))))"
    )
}

/// The labels of the sites of the report on lines `first` to `last` of `path`.
fn labels_on(
    sites: &[LabeledSite],
    path: &str,
    first: usize,
    last: usize,
) -> impl Iterator<Item = SiteLabel> {
    sites
        .iter()
        .filter(move |labeled| {
            labeled.site.path == path
                && labeled
                    .site
                    .start_line
                    .is_some_and(|line| (first..=last).contains(&line))
        })
        .map(|labeled| labeled.label)
}

/// The route facts that register `target` as a handler: a decorator, attribute, or annotation
/// route on the symbol itself, or a route fact that names it as its handler. A handler value
/// names `target` by its last dotted segment; a value that starts with `self.`, `cls.`, or `this.`
/// names only a member of a type. A fact that names it only by name counts whenever
/// a proven or likely site of the target is on its lines. Otherwise it does not count when every
/// site of the report on its lines is excluded, or when its file defines a function, class, or
/// variable of that name that a bare name can reach (at module level, or local to a function
/// that holds the first line of the fact, never a member of a type) and the target is in another
/// file. Rows tied by id come first, then rows tied by name, each by path and line.
fn registrations(
    conn: &Connection,
    target: &Symbol,
    sites: &[LabeledSite],
) -> Result<Vec<Registration>, QueryError> {
    if !has_table(conn, "structural_facts") {
        return Ok(Vec::new());
    }
    let patterns = route_pattern_ids(conn)?;
    if patterns.is_empty() {
        return Ok(Vec::new());
    }
    let patterns = serde_json::to_string(&patterns).unwrap_or_default();
    let columns = format!(
        "sf.structural_fact_id, sf.pattern_id, sf.path, sf.start_line, sf.end_line,
         json_extract({VALID_METADATA}, '$.verb'),
         COALESCE(json_extract({VALID_METADATA}, '$.effective_route_template'),
                  json_extract({VALID_METADATA}, '$.route_template'))"
    );
    let read = |row: &rusqlite::Row| -> rusqlite::Result<RouteFact> {
        Ok(RouteFact {
            id: row.get(0)?,
            end_line: row.get::<_, i64>(4)? as usize,
            row: Registration {
                pattern_id: row.get(1)?,
                path: row.get::<_, String>(2)?.replace('\\', "/"),
                line: row.get::<_, i64>(3)? as usize,
                verb: row.get(5)?,
                template: row.get(6)?,
                by_name: false,
            },
        })
    };
    let by_id = conn
        .prepare(&format!(
            "SELECT {columns} FROM structural_facts sf
             WHERE sf.containing_symbol_id = ?1
               AND sf.pattern_id IN (SELECT value FROM json_each(?2))
               AND json_extract({VALID_METADATA}, '$.api_style') IN ({})",
            sql_list(&ID_TIED_ROUTING)
        ))?
        .query_map(params![target.symbol_id, patterns], read)?
        .collect::<Result<Vec<_>, _>>()?;
    let member_of_type = match target.parent_symbol_id.as_deref() {
        Some(parent) => get_symbol_by_id(conn, parent)?
            .is_some_and(|owner| TYPE_KINDS.contains(&owner.kind.as_str())),
        None => false,
    };
    let names = HANDLER_KEYS
        .iter()
        .map(|key| names_target(&format!("json_extract({VALID_METADATA}, '$.{key}')")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let by_name = conn
        .prepare(&format!(
            "SELECT {columns} FROM structural_facts sf
             WHERE sf.pattern_id IN (SELECT value FROM json_each(?2))
               AND ({names} OR EXISTS (
                   SELECT 1 FROM json_each({VALID_METADATA}, '$.handler_names')
                   WHERE {}))",
            names_target("value")
        ))?
        .query_map(params![target.name, patterns, member_of_type], read)?
        .collect::<Result<Vec<_>, _>>()?;
    let known: BTreeSet<String> = by_id.iter().map(|fact| fact.id.clone()).collect();
    let mut rows: Vec<Registration> = by_id.into_iter().map(|fact| fact.row).collect();
    let target_path = target.path.replace('\\', "/");
    let reachable_kinds: Vec<&str> = CLASS_LIKE_KINDS
        .iter()
        .copied()
        .chain(["function", "variable", "constant"])
        .collect();
    let mut bare_name_defined = conn.prepare(&format!(
        "SELECT 1 FROM symbols s LEFT JOIN symbols owner ON owner.symbol_id = s.parent_symbol_id
         WHERE s.path IN (?1, ?2) AND s.name = ?3 AND s.kind IN ({})
           AND (owner.kind IS NULL OR owner.kind IN ('module', 'namespace')
                OR (owner.kind NOT IN ({}) AND ?4 BETWEEN owner.start_line AND owner.end_line))",
        sql_list(&reachable_kinds),
        sql_list(&TYPE_KINDS)
    ))?;
    for fact in by_name.into_iter().filter(|fact| !known.contains(&fact.id)) {
        let path = &fact.row.path;
        let labels: Vec<SiteLabel> = labels_on(sites, path, fact.row.line, fact.end_line).collect();
        let tied = labels
            .iter()
            .any(|label| matches!(label, SiteLabel::Proven | SiteLabel::Likely));
        let only_excluded = !labels.is_empty() && labels.iter().all(|l| *l == SiteLabel::Excluded);
        let defined_elsewhere = *path != target_path
            && bare_name_defined.exists(params![
                path,
                path.replace('/', "\\"),
                target.name,
                fact.row.line as i64
            ])?;
        if !tied && (only_excluded || defined_elsewhere) {
            continue;
        }
        rows.push(Registration {
            by_name: true,
            ..fact.row
        });
    }
    rows.sort_by(|a, b| {
        a.by_name
            .cmp(&b.by_name)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.pattern_id.cmp(&b.pattern_id))
    });
    Ok(rows)
}

struct TestSymbol {
    id: String,
    name: String,
    kind: String,
    path: String,
    line: usize,
    language: String,
    is_test: bool,
    lifecycle: bool,
    parent_id: Option<String>,
    parent_kind: Option<String>,
}

impl TestSymbol {
    fn is_callable(kind: &str) -> bool {
        CALLABLE_KINDS.contains(&kind)
    }

    fn reads_as_test(&self) -> bool {
        (self.is_test
            || (Self::is_callable(&self.kind)
                && names_a_test(&self.name)
                && is_test_path(&self.path)))
            && !self.is_nested_function()
    }

    /// A Python or Go function inside another function: pytest does not collect it, and julie
    /// names a Go `t.Run` closure by its label, which `go test` cannot select alone.
    fn is_nested_function(&self) -> bool {
        matches!(self.language.as_str(), "python" | "go")
            && self.parent_kind.as_deref().is_some_and(Self::is_callable)
    }
}

fn load_test_symbol(
    load: &mut rusqlite::Statement,
    id: &str,
) -> Result<Option<TestSymbol>, QueryError> {
    let mut rows = load.query_map(params![id], |row| {
        Ok(TestSymbol {
            id: row.get(0)?,
            name: row.get(1)?,
            kind: row.get(2)?,
            path: row.get::<_, String>(3)?.replace('\\', "/"),
            line: row.get::<_, i64>(4)? as usize,
            language: row.get(5)?,
            is_test: row.get(6)?,
            lifecycle: row.get(7)?,
            parent_id: row.get(8)?,
            parent_kind: row.get(9)?,
        })
    })?;
    Ok(rows.next().transpose()?)
}

/// The test that `caller` stands for: the caller when julie flags it as a test or its name reads
/// like one in a test file, else the test function that holds it as a helper. A Python class
/// defined inside a function is part of that function. Setup, teardown, and fixture members are
/// never tests, and neither are helpers inside them.
fn test_of(load: &mut rusqlite::Statement, caller: &str) -> Result<Option<TestSymbol>, QueryError> {
    let mut current = load_test_symbol(load, caller)?;
    for _ in 0..=MAX_HELPER_DEPTH {
        let Some(symbol) = current else {
            return Ok(None);
        };
        if symbol.lifecycle {
            return Ok(None);
        }
        if symbol.reads_as_test() {
            let holder = if symbol.language == "python" {
                class_chain(load, &symbol)?.held_by
            } else {
                None
            };
            match holder {
                Some(function) => {
                    current = load_test_symbol(load, &function)?;
                    continue;
                }
                None => return Ok(Some(symbol)),
            }
        }
        current = match (&symbol.parent_id, symbol.parent_kind.as_deref()) {
            (Some(parent), Some(kind)) if TestSymbol::is_callable(kind) => {
                load_test_symbol(load, parent)?
            }
            _ => return Ok(None),
        };
    }
    Ok(None)
}

/// The classes that enclose a test, outermost first, and the function or method that holds the
/// outermost one when the class is defined inside it.
struct ClassChain {
    classes: Vec<TestSymbol>,
    held_by: Option<String>,
}

fn class_chain(
    load: &mut rusqlite::Statement,
    test: &TestSymbol,
) -> Result<ClassChain, QueryError> {
    let mut chain = ClassChain {
        classes: Vec::new(),
        held_by: None,
    };
    let mut parent = test
        .parent_id
        .clone()
        .filter(|_| test.parent_kind.as_deref() == Some("class"));
    while let Some(id) = parent {
        let Some(class) = load_test_symbol(load, &id)? else {
            break;
        };
        parent = match (&class.parent_id, class.parent_kind.as_deref()) {
            (Some(outer), Some("class")) => Some(outer.clone()),
            (Some(holder), Some(kind)) if TestSymbol::is_callable(kind) => {
                chain.held_by = Some(holder.clone());
                None
            }
            _ => None,
        };
        chain.classes.push(class);
        if chain.classes.len() >= MAX_CLASS_DEPTH {
            chain.held_by = None;
            break;
        }
    }
    chain.classes.reverse();
    Ok(chain)
}

/// Whether a Python class derives from `unittest.TestCase`, which pytest collects under any
/// name. A base counts when its name ends in `TestCase`, or when a project class it names does.
struct TestCaseBases<'c> {
    direct: rusqlite::Statement<'c>,
    named: rusqlite::Statement<'c>,
    known: HashMap<String, bool>,
}

impl<'c> TestCaseBases<'c> {
    fn new(conn: &'c Connection) -> Result<Self, QueryError> {
        Ok(Self {
            direct: conn.prepare(
                "SELECT target_terminal_name, NULL FROM pending_relationships
                 WHERE from_symbol_id = ?1 AND kind = 'extends'
                 UNION ALL
                 SELECT s.name, s.symbol_id FROM relationships r
                 JOIN symbols s ON s.symbol_id = r.to_symbol_id
                 WHERE r.from_symbol_id = ?1 AND r.kind = 'extends'",
            )?,
            named: conn.prepare(
                "SELECT symbol_id FROM symbols
                 WHERE name = ?1 AND kind = 'class' AND language = 'python'",
            )?,
            known: HashMap::new(),
        })
    }

    fn derives(&mut self, class_id: &str) -> Result<bool, QueryError> {
        if let Some(known) = self.known.get(class_id) {
            return Ok(*known);
        }
        let mut seen = HashSet::new();
        let mut pending = vec![class_id.to_string()];
        let mut found = false;
        while let Some(id) = pending.pop() {
            if !seen.insert(id.clone()) || seen.len() > MAX_TYPES {
                continue;
            }
            let bases = self
                .direct
                .query_map(params![id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            for (name, resolved) in bases {
                if name.ends_with("TestCase") {
                    found = true;
                } else if let Some(base) = resolved {
                    pending.push(base);
                } else {
                    let named = self
                        .named
                        .query_map(params![name], |row| row.get::<_, String>(0))?
                        .collect::<Result<Vec<_>, _>>()?;
                    pending.extend(named);
                }
            }
            if found {
                break;
            }
        }
        self.known.insert(class_id.to_string(), found);
        Ok(found)
    }
}

/// The tests that own a proven or likely site of `target`, by path and line, each with the
/// command that runs it. Each caller is loaded once, and each marker folder is read once.
fn calling_tests(
    conn: &Connection,
    root: &Path,
    target: &Symbol,
    sites: &[LabeledSite],
) -> Result<Vec<CallingTest>, QueryError> {
    let lifecycle = if has_column(conn, "symbols", "test_lifecycle") {
        "COALESCE(s.test_lifecycle, 0)"
    } else {
        "0"
    };
    let mut load = conn.prepare(&format!(
        "SELECT s.symbol_id, s.name, s.kind, s.path, s.start_line, s.language, s.is_test,
                {lifecycle}, s.parent_symbol_id, p.kind
         FROM symbols s
         LEFT JOIN symbols p ON p.symbol_id = s.parent_symbol_id
         WHERE s.symbol_id = ?1"
    ))?;
    let mut callers = HashSet::new();
    let mut tests: BTreeMap<String, TestSymbol> = BTreeMap::new();
    for labeled in sites
        .iter()
        .filter(|site| matches!(site.label, SiteLabel::Proven | SiteLabel::Likely))
    {
        let caller = labeled.site.from_symbol_id.as_str();
        if caller.is_empty() || caller == target.symbol_id || !callers.insert(caller) {
            continue;
        }
        if let Some(test) = test_of(&mut load, caller)?
            && test.id != target.symbol_id
        {
            tests.entry(test.id.clone()).or_insert(test);
        }
    }
    let mut finder = CommandFinder::new(root);
    let mut bases = TestCaseBases::new(conn)?;
    let mut rows = Vec::with_capacity(tests.len());
    for test in tests.into_values() {
        let chain = class_chain(&mut load, &test)?;
        let mut classes = Vec::with_capacity(chain.classes.len());
        for class in &chain.classes {
            classes.push(TestClass {
                name: &class.name,
                test_case: test.language == "python" && bases.derives(&class.id)?,
            });
        }
        let command = finder.command(&TestSpec {
            language: &test.language,
            kind: &test.kind,
            path: &test.path,
            line: test.line,
            name: &test.name,
            classes: &classes,
        });
        rows.push(CallingTest {
            command,
            symbol_id: test.id,
            name: test.name,
            path: test.path,
            line: test.line,
        });
    }
    rows.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builds(line: &str, name: &str, language: BracedLanguage) -> bool {
        let column = line.find(name).unwrap();
        opens_a_literal(line, 0, column, name, language)
    }

    #[test]
    fn a_rust_literal_is_built_and_a_type_before_a_brace_is_not() {
        for line in [
            "    Foo { a: 1 }",
            "    let f = &Foo { a: 1 };",
            "    let f = &mut Foo { a: 1 };",
            "    let f = Foo::<T> { a: 1 };",
            "    let f = Foo::<Vec<u8>> { a: 1 };",
            "    let f = shapes::Foo { a: 1 };",
            "    let f = crate::shapes::Foo { a: 1 };",
            "    Foo { a: 1, ..Default::default() }",
            "    let f = Foo {",
        ] {
            assert!(builds(line, "Foo", BracedLanguage::Rust), "{line}");
        }
        for line in [
            "impl Foo {",
            "impl<T> Foo<T> {",
            "impl<T: Into<u8>> Foo<T> {",
            "impl<'a> Foo<'a> {",
            "unsafe impl Send for Foo {",
            "impl Tr for Foo {",
            "impl Tr for &Foo {",
            "impl Tr for shapes::Foo {",
            "pub fn f() -> Foo {",
            "pub fn f() -> &'a Foo {",
            "pub fn f() -> &'static mut Foo {",
            "pub fn f() -> shapes::Foo {",
            "pub fn f() -> Foo<T> {",
            "struct Foo {",
            "enum Foo {",
            "union Foo {",
            "trait Foo {",
            "type Alias = dyn Foo {",
            "type Foo {",
            "pub fn f(x: Foo) {",
            "let f = Foo::new();",
        ] {
            assert!(!builds(line, "Foo", BracedLanguage::Rust), "{line}");
        }
    }

    #[test]
    fn a_rust_pattern_that_destructures_is_not_built() {
        for text in [
            "let Foo { a } = x;",
            "    Foo { a: 7 } => 1,",
            "    Foo { a } | Bar { a } => 1,",
            "    Foo { a } if a > 0 => 1,",
            "fn f(Foo { a }: Foo) {}",
            "for Foo { a } in items {}",
            "let Foo {\n    a,\n} = x;",
        ] {
            assert!(
                !opens_a_literal(
                    text,
                    0,
                    text.find("Foo").unwrap(),
                    "Foo",
                    BracedLanguage::Rust
                ),
                "{text}"
            );
        }
        for text in [
            "let f = Foo { a: 1 };",
            "x == Foo { a: 1 } && y",
            "Foo { a: 1 }.run()",
            "let f = Foo {\n    a: 1,\n};",
            "let f = Foo { a: format!(\"{}\", 1) };",
        ] {
            assert!(
                opens_a_literal(
                    text,
                    0,
                    text.find("Foo").unwrap(),
                    "Foo",
                    BracedLanguage::Rust
                ),
                "{text}"
            );
        }
    }

    #[test]
    fn a_rust_pattern_nested_in_a_group_is_not_built() {
        for text in [
            "if let Some(Foo { a, .. }) = o {",
            "    Some(Foo { a, .. }) => a,",
            "    Ok(Foo { a, .. }) | Err(Foo { a, .. }) => a,",
            "    Some(Foo { a, .. }) if a > 0 => a,",
            "let (Foo { a, .. }, y) = p;",
            "let (y, Foo { a, .. }) = p;",
            "let [Foo { a, .. }, rest @ ..] = arr;",
            "let Outer { inner: Foo { a, .. }, .. } = o;",
            "let Outer {\n    inner: Foo { a, .. },\n    other,\n} = o;",
            "let &Foo { a, .. } = f;",
            "let (&Foo { a, .. }, y) = p;",
            "let (Some(Foo { a, .. }), y): (Option<Foo>, i32) = p;",
            "let Some(Foo { a, .. }) = o else { return };",
            "while let Some(Foo { a, .. }) = it.next() {",
            "for (i, Foo { a, .. }) in xs.iter().enumerate() {",
            "xs.map(|(Foo { a, .. }, y)| a + y)",
            "xs.map(|Foo { a, .. }, y| a + y)",
            "xs.fold(0, |acc, Foo { a, .. }| acc + a)",
            "xs.map(|y, Foo { a, .. }| a + y)",
            "fn f((Foo { a, .. }, y): (Foo, i32)) {}",
            "fn f(y: i32, Foo { a, .. }: Foo) {}",
            "matches!(f, Foo { a: 1, .. })",
            "assert!(matches!(f, Foo { a: 1, .. }));",
            "matches!(o, Some(Foo { a: 1, .. }))",
            "matches!(o, Some(Foo { a: 1, .. }) if true)",
            "assert!(matches!(\n    o,\n    Some(Foo { a: 1, .. })\n));",
        ] {
            assert!(!builds(text, "Foo", BracedLanguage::Rust), "{text}");
        }
    }

    #[test]
    fn a_rust_literal_nested_in_a_group_is_built() {
        for text in [
            "let t = (Foo { a: 1 }, y);",
            "let x = Some(Foo { a: 1 });",
            "let x = Some((Foo { a: 1 }, y));",
            "let x = f(Foo { a: 1 }, 2) as i32;",
            "if f(Foo { a: 1 }) == y {",
            "x = Foo { a: 1 };",
            "x = Some(Foo { a: 1 });",
            "return Foo { a: 1 };",
            "let v = vec![Foo { a: 1 }, Foo { a: 2 }];",
            "let o = Outer { inner: Foo { a: 1, b: 2 } };",
            "    Outer { inner: Foo { a: 1, b: 2 } }\n}\n\npub fn next() {}\n",
            "for x in [Foo { a: 1 }, Foo { a: 2 }] {",
            "xs.map(|x| Foo { a: x }).collect()",
            "let v = if c { Some(Foo { a: 1 }) } else { None };",
            "assert_eq!(f(), Foo { a: 1 });",
            "assert!(matches!(g(Foo { a: 1 }), Bar));",
            "match o {\n    A => Foo { a: 1 },\n    B => Foo { a: 2 },\n}\n",
        ] {
            assert!(builds(text, "Foo", BracedLanguage::Rust), "{text}");
        }
    }

    fn builds_every(text: &str, name: &str) -> bool {
        text.match_indices(name)
            .all(|(column, _)| opens_a_literal(text, 0, column, name, BracedLanguage::Rust))
    }

    #[test]
    fn a_rust_literal_in_an_arm_with_alternations_is_built_before_more_alternation_arms() {
        for text in [
            "match k {\n    1 | 2 => Foo { a: 1 },\n    3 | 4 => Foo { a: 2 },\n    _ => Foo { a: 3 },\n}\n",
            "match k {\n    | 1 | 2 => Foo { a: 1 },\n    | 3 | 4 => Foo { a: 2 },\n}\n",
            "match k {\n    A(1 | 2) => Foo { a: 1 },\n    B | C => Foo { a: 2 },\n}\n",
            "match k {\n    1 | 2 => { work() }\n    3 | 4 => Foo { a: 2 },\n}\n",
            "match k {\n    X::A | X::B => Foo { a: 1 },\n    X::C | X::D if ok => Foo { a: 2 },\n}\n",
        ] {
            assert!(builds_every(text, "Foo"), "{text}");
        }
    }

    #[test]
    fn a_rust_literal_next_to_a_bit_or_is_built() {
        for text in [
            "g(a | Foo { a: 1 }, b | c);",
            "let t = (a | Foo { a: 1 }, b | c);",
            "h(|x| Foo { a: x }, y | z);",
            "let f = x | y;\nlet g = (Foo { a: 1 }, b | c);",
        ] {
            assert!(builds_every(text, "Foo"), "{text}");
        }
    }

    #[test]
    fn a_rust_pattern_in_closure_parameters_stays_a_pattern_among_alternations() {
        for text in [
            "let f = |Foo { a, .. }, y| a | y;",
            "match k {\n    1 | 2 => g(|Foo { a, .. }, y| a),\n    3 | 4 => 0,\n}\n",
            "match k {\n    1 | 2 => |Foo { a, .. }, y| a,\n    3 | 4 => 0,\n}\n",
            "h(a | b, |Foo { a, .. }, y| a);",
            "let f = x | y;\nlet g = move |Foo { a, .. }, y| a | y;",
            "fn f() { return |Foo { a, .. }, y| a | y; }",
            "xs.map(|Foo { a, .. }, y| a | y)",
        ] {
            assert!(!builds_every(text, "Foo"), "{text}");
        }
    }

    #[test]
    fn a_rust_literal_that_does_not_close_in_the_scan_window_is_built() {
        let long = format!("let t = Foo {{ a: \"{}\" }};", "é".repeat(3000));
        let after = format!("let t = (Foo {{ a: 1 }}, \"{}\");", "é".repeat(3000));

        assert!(builds(&long, "Foo", BracedLanguage::Rust));
        assert!(builds(&after, "Foo", BracedLanguage::Rust));
    }

    #[test]
    fn a_go_literal_is_built_and_a_return_type_before_a_brace_is_not() {
        for line in [
            "\treturn Foo{A: 1}",
            "\treturn &Foo{A: 1}",
            "\treturn []Foo{{A: 1}}",
            "\treturn [3]Foo{{A: 1}}",
            "\treturn map[string]Foo{\"a\": {A: 1}}",
            "\treturn Foo[int]{A: 1}",
            "\tx := m.Foo{A: 1}",
            "\tx := Foo {A: 1}",
        ] {
            assert!(builds(line, "Foo", BracedLanguage::Go), "{line}");
        }
        for line in [
            "func New() Foo {",
            "func New() *Foo {",
            "func New() []Foo {",
            "func New() map[string]Foo {",
            "func New() Foo[int] {",
            "func New() m.Foo {",
            "type Foo struct {",
            "f := func() Foo {",
            "var x Foo",
        ] {
            assert!(!builds(line, "Foo", BracedLanguage::Go), "{line}");
        }
    }

    fn builds_beside_the_first(line: &str, name: &str, language: BracedLanguage) -> bool {
        line_builds(line, 0, line.find(name).unwrap(), name, language)
    }

    #[test]
    fn a_rust_block_comment_holds_no_literal_and_nests() {
        for line in [
            "fn f(x: Foo) -> i32 { let Foo { a } = x; /* Foo { a: 1 } */ a }",
            "fn f(x: Foo) -> i32 { let Foo { a } = x; /* a /* b */ Foo { a: 2 } */ a }",
            "fn f(x: Foo) -> i32 { let Foo { a } = x; /* Foo { a: 1 }",
        ] {
            assert!(
                !builds_beside_the_first(line, "Foo", BracedLanguage::Rust),
                "{line}"
            );
        }
        for line in [
            "fn f(x: Foo) -> Foo { let Foo { a } = x; /* done */ Foo { a } }",
            "fn f(x: Foo) -> Foo { let Foo { a } = x; /* /* a */ */ Foo { a } }",
        ] {
            assert!(
                builds_beside_the_first(line, "Foo", BracedLanguage::Rust),
                "{line}"
            );
        }
    }

    #[test]
    fn a_rust_char_literal_hides_its_quote_and_a_lifetime_is_not_a_char_literal() {
        for line in [
            "fn f(x: Foo, c: char) -> Foo { let Foo { a } = x; if c == '\"' { Foo { a } } else { Foo { a: 0 } } }",
            "fn f(x: Foo, c: char) -> Foo { let Foo { a } = x; if c == '\\'' { Foo { a } } else { Foo { a: 0 } } }",
            "fn f(x: Foo, c: char) -> Foo { let Foo { a } = x; if c == '\\u{22}' { Foo { a } } else { Foo { a: 0 } } }",
            "fn f(x: Foo, c: char) -> Foo { let Foo { a } = x; if c == b'x' { Foo { a } } else { Foo { a: 0 } } }",
            "fn f<'a>(x: &'a Foo, y: &'a str) -> Foo { let Foo { a } = x; Foo { a: 0 } }",
            "fn f(x: &'static Foo) -> Foo { let Foo { a } = x; Foo { a: 0 } }",
        ] {
            assert!(
                builds_beside_the_first(line, "Foo", BracedLanguage::Rust),
                "{line}"
            );
        }
        for line in [
            "fn f(x: Foo) -> i32 { let Foo { a } = x; let c = '{'; let s = \"Foo { a: 1 }\"; a }",
            "fn f(x: Foo) -> i32 { let Foo { a } = x; let c = '\"'; let s = '\\''; a } // Foo { a: 1 }",
        ] {
            assert!(
                !builds_beside_the_first(line, "Foo", BracedLanguage::Rust),
                "{line}"
            );
        }
    }

    #[test]
    fn a_rust_raw_string_holds_no_literal_even_with_quotes_inside() {
        for line in [
            "fn f(x: Foo) -> i32 { let Foo { a } = x; let s = r#\"say \"Foo { a: 1 }\" ok\"#; a }",
            "fn f(x: Foo) -> i32 { let Foo { a } = x; let s = r\"Foo { a: 1 }\"; a }",
            "fn f(x: Foo) -> i32 { let Foo { a } = x; let s = br##\"x \"# Foo { a: 1 }\"##; a }",
        ] {
            assert!(
                !builds_beside_the_first(line, "Foo", BracedLanguage::Rust),
                "{line}"
            );
        }
        let line = "fn f(x: Foo) -> Foo { let Foo { a } = x; let s = r#\"{}\"#; Foo { a: s.len() as i32 + a } }";
        assert!(
            builds_beside_the_first(line, "Foo", BracedLanguage::Rust),
            "{line}"
        );
    }

    #[test]
    fn a_go_raw_string_and_a_rune_literal_hide_their_contents_and_nothing_after() {
        for line in [
            "func a() Foo { s := `http://x`; _ = s; return Foo{A: 1} }",
            "func g(f Foo) Foo { r := '\"'; _ = r; return Foo{A: 3} }",
            "func g(f Foo) Foo { r := '\\''; _ = r; return Foo{A: 3} }",
            "func g(f Foo) Foo { s := \"a\\\"b // c\"; _ = s; return Foo{A: 4} }",
            "func g(f Foo) Foo { /* a /* b */ return Foo{A: 1} }",
        ] {
            assert!(
                builds_beside_the_first(line, "Foo", BracedLanguage::Go),
                "{line}"
            );
        }
        for line in [
            "func g(f Foo) int { s := `Foo{A: 1} \"`; return len(s) }",
            "func g(f Foo) int { /* Foo{A: 1} */ return 1 }",
            "func g(f Foo) int { r := '{'; s := \"Foo{A: 1}\"; return len(s) }",
        ] {
            assert!(
                !builds_beside_the_first(line, "Foo", BracedLanguage::Go),
                "{line}"
            );
        }
    }

    fn context_of(source: &str, needle: &str) -> ImplContext {
        let lines: Vec<String> = source.lines().map(str::to_string).collect();
        let line = lines.iter().position(|text| text.contains(needle)).unwrap() + 1;
        impl_context(&lines, line)
    }

    fn implemented(path: &str) -> ImplContext {
        let mut segments: Vec<String> = path.split("::").map(str::to_string).collect();
        let name = segments.pop().unwrap();
        ImplContext::Trait(TraitPath {
            modules: segments,
            name,
        })
    }

    #[test]
    fn a_method_in_a_trait_impl_names_the_trait_and_the_modules_written_before_it() {
        let source = "impl<T: Clone> std::ops::Add<T> for Wrapper<T> {\n    fn add_it(&self) {}\n}\n\nimpl crate::shapes::Shape for Circle {\n    fn area(&self) {}\n}\n\nunsafe impl Send for Circle {\n    fn marker(&self) {}\n}\n\nimpl<F: Fn() -> i32> Run for Holder<F> {\n    fn go(&self) {}\n}\n\nimpl ::serde::Serialize for Holder {\n    fn emit(&self) {}\n}\n";

        assert_eq!(context_of(source, "add_it"), implemented("std::ops::Add"));
        assert_eq!(
            context_of(source, "area"),
            implemented("crate::shapes::Shape")
        );
        assert_eq!(context_of(source, "marker"), implemented("Send"));
        assert_eq!(context_of(source, "go"), implemented("Run"));
        assert_eq!(context_of(source, "emit"), implemented("serde::Serialize"));
    }

    fn place(path: &str) -> Place {
        Place::new(path, Vec::new())
    }

    fn type_at(id: &str, path: &str, inline_modules: &[&str]) -> (String, Place) {
        let inline = inline_modules.iter().map(|name| name.to_string()).collect();
        (id.to_string(), Place::new(path, inline))
    }

    fn written(path: &str) -> Vec<String> {
        path.split("::").map(str::to_string).collect()
    }

    fn reached(named: &RustNamed, from: &str, path: &str) -> Vec<String> {
        named
            .reached(&place(from), &written(path))
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn a_place_takes_its_module_path_from_the_file_below_src_and_the_inline_mods() {
        assert_eq!(place("src/lib.rs").modules.join("::"), "");
        assert_eq!(place("pkg/src/main.rs").modules.join("::"), "");
        assert_eq!(place("src/ovr.rs").modules.join("::"), "ovr");
        assert_eq!(place("src/ovr/mod.rs").modules.join("::"), "ovr");
        assert_eq!(place("src/ovr/a.rs").modules.join("::"), "ovr::a");
        assert_eq!(place("src\\ovr\\a.rs").modules.join("::"), "ovr::a");
        let inline = Place::new("src/ovr.rs", vec!["shapes".to_string()]);
        assert_eq!(inline.modules.join("::"), "ovr::shapes");
        assert_eq!(place("pkg/src/lib.rs").package.as_deref(), Some("/pkg"));
        assert_eq!(place("tests/a.rs").package, None);
    }

    #[test]
    fn a_leading_crate_self_or_super_spells_one_module_path_of_the_package_of_the_impl() {
        let named = RustNamed {
            types: vec![
                type_at("root", "src/lib.rs", &[]),
                type_at("ovr", "src/ovr.rs", &[]),
                type_at("a", "src/ovr/a.rs", &[]),
                type_at("shapes", "src/lib.rs", &["shapes"]),
                type_at("other", "pkg_b/src/lib.rs", &[]),
            ],
            uses: Vec::new(),
        };

        assert_eq!(reached(&named, "src/ovr.rs", "crate"), ["root"]);
        assert_eq!(reached(&named, "src/x.rs", "crate::ovr"), ["ovr"]);
        assert_eq!(reached(&named, "src/ovr.rs", "self::a"), ["a"]);
        assert_eq!(reached(&named, "src/ovr/b.rs", "super"), ["ovr"]);
        assert_eq!(reached(&named, "src/ovr/b.rs", "super::a"), ["a"]);
        assert_eq!(reached(&named, "src/ovr/b.rs", "super::super"), ["root"]);
        assert_eq!(reached(&named, "src/lib.rs", "self::shapes"), ["shapes"]);
        assert_eq!(reached(&named, "src/impls.rs", "crate::shapes"), ["shapes"]);
        assert!(reached(&named, "src/impls.rs", "self::shapes").is_empty());
        assert!(reached(&named, "src/lib.rs", "super").is_empty());
        assert!(reached(&named, "src/ovr.rs", "crate::a").is_empty());
        assert!(reached(&named, "pkg_a/src/lib.rs", "crate").is_empty());
    }

    #[test]
    fn a_leading_crate_name_names_the_package_folder_with_dashes_read_as_underscores() {
        let named = RustNamed {
            types: vec![
                type_at("a", "crates/my-crate/src/a.rs", &[]),
                type_at("root", "crates/my-crate/src/lib.rs", &[]),
                type_at("own", "pkg_b/src/lib.rs", &[]),
            ],
            uses: Vec::new(),
        };

        assert_eq!(reached(&named, "pkg_b/src/lib.rs", "my_crate::a"), ["a"]);
        assert_eq!(reached(&named, "pkg_b/src/lib.rs", "my_crate"), ["root"]);
        assert!(reached(&named, "pkg_b/src/lib.rs", "pkg_a").is_empty());
        assert!(reached(&named, "pkg_b/src/lib.rs", "std::fmt").is_empty());
        assert!(reached(&named, "pkg_b/src/lib.rs", "core::a").is_empty());
    }

    #[test]
    fn a_path_without_a_prefix_is_read_from_the_module_of_the_impl_then_the_crate_root() {
        let named = RustNamed {
            types: vec![
                type_at("root_a", "src/a.rs", &[]),
                type_at("ovr_a", "src/ovr/a.rs", &[]),
                type_at("deep", "src/m/n/c.rs", &[]),
                type_at("deep_too", "src/p/n/c.rs", &[]),
            ],
            uses: Vec::new(),
        };

        assert_eq!(reached(&named, "src/ovr.rs", "a"), ["ovr_a"]);
        assert_eq!(reached(&named, "src/lib.rs", "a"), ["root_a"]);
        assert_eq!(reached(&named, "src/other.rs", "a"), ["root_a"]);
        assert_eq!(reached(&named, "src/ovr.rs", "ovr::a"), ["ovr_a"]);
        assert!(reached(&named, "src/k.rs", "n::c").is_empty());
        assert_eq!(reached(&named, "src/k.rs", "m::n::c"), ["deep"]);
    }

    #[test]
    fn a_path_without_a_prefix_that_spells_nothing_names_the_one_type_it_ends_like() {
        let named = RustNamed {
            types: vec![
                type_at("ovr_a", "src/ovr/a.rs", &[]),
                type_at("x_b", "src/x/b.rs", &[]),
                type_at("y_b", "src/y/b.rs", &[]),
            ],
            uses: Vec::new(),
        };

        assert_eq!(reached(&named, "src/ovr/c.rs", "a"), ["ovr_a"]);
        assert!(reached(&named, "src/ovr/c.rs", "b").is_empty());
        assert!(reached(&named, "src/ovr/c.rs", "crate::a").is_empty());
        assert!(reached(&named, "src/ovr/c.rs", "std::a").is_empty());
    }

    #[test]
    fn a_use_row_in_the_named_module_stands_for_the_type_it_imports_for_one_hop() {
        let import = |path: &str, source: &str| (place(path), written(source));
        let named = RustNamed {
            types: vec![
                type_at("a", "src/a.rs", &[]),
                type_at("root", "src/lib.rs", &[]),
            ],
            uses: vec![
                import("src/x.rs", "crate::a"),
                import("src/y.rs", "crate::x"),
                import("src/z.rs", "std::fmt"),
            ],
        };

        assert_eq!(reached(&named, "src/lib.rs", "crate::x"), ["a"]);
        assert_eq!(reached(&named, "src/k.rs", "x"), ["a"]);
        assert!(reached(&named, "src/lib.rs", "crate::y").is_empty());
        assert!(reached(&named, "src/lib.rs", "crate::z").is_empty());
        assert_eq!(reached(&named, "src/lib.rs", "crate::a"), ["a"]);
    }

    #[test]
    fn a_bare_name_means_the_type_of_its_module_then_its_use_row_then_its_glob() {
        let import = |path: &str, source: &str| (place(path), written(source));
        let named = RustNamed {
            types: vec![
                type_at("here", "src/a.rs", &[]),
                type_at("used", "src/b.rs", &[]),
                type_at("starred", "src/c.rs", &[]),
            ],
            uses: vec![
                import("src/x.rs", "crate::b"),
                import("src/w.rs", "std::io"),
                import("src/a.rs", "crate::b"),
                import("src/v.rs", "crate::hub::deep"),
            ],
        };
        let star = |path: &str| vec![import(path, "crate::c")];

        assert_eq!(
            named.bare(&place("src/a.rs"), &star("src/a.rs"), &HashSet::new()),
            Some(vec!["here"])
        );
        assert_eq!(
            named.bare(&place("src/x.rs"), &star("src/x.rs"), &HashSet::new()),
            Some(vec!["used"])
        );
        assert_eq!(
            named.bare(&place("src/w.rs"), &star("src/w.rs"), &HashSet::new()),
            Some(Vec::new())
        );
        assert_eq!(
            named.bare(&place("src/y.rs"), &star("src/y.rs"), &HashSet::new()),
            Some(vec!["starred"])
        );
        assert_eq!(
            named.bare(&place("src/v.rs"), &star("src/v.rs"), &HashSet::new()),
            None
        );
        assert_eq!(named.bare(&place("src/y.rs"), &[], &HashSet::new()), None);
        assert_eq!(
            named.bare(&place("src/z.rs"), &star("src/y.rs"), &HashSet::new()),
            None
        );
    }

    #[test]
    fn a_use_row_from_an_external_name_decides_like_a_std_path() {
        let import = |path: &str, source: &str| (place(path), written(source));
        let named = RustNamed {
            types: vec![type_at("project", "src/b.rs", &[])],
            uses: vec![
                import("src/e.rs", "serde"),
                import("src/m.rs", "serde"),
                import("src/m.rs", "crate::b"),
            ],
        };
        let external: HashSet<String> = ["serde".to_string()].into();

        assert_eq!(
            named.bare(&place("src/e.rs"), &[], &external),
            Some(Vec::new())
        );
        assert_eq!(named.bare(&place("src/e.rs"), &[], &HashSet::new()), None);
        assert_eq!(
            named.bare(&place("src/m.rs"), &[], &external),
            Some(vec!["project"])
        );
    }

    #[test]
    fn a_cargo_toml_names_its_dependencies_in_every_table_form() {
        let toml = "[package]\nname = \"not-a-dep\"\n\n[features]\nfeat = [\"a\"]\n\n[dependencies]\nserde = \"1\"\nmy-dep = { version = \"1\" }\n# commented = \"1\"\ninherited.workspace = true\n\"quoted-dep\" = \"1\"\nrenamed = { package = \"real-name\" }\n\n[dev-dependencies]\ndev_only = \"1\"\n\n[build-dependencies.build_dep]\nversion = \"1\"\n\n[target.'cfg(unix)'.dependencies]\nunix_only = \"1\"\n\n[target.'cfg(a.b)'.dev-dependencies.dotted]\nversion = \"1\"\n\n[workspace.dependencies]\nignored = \"1\"\n\n[lib]\nname = \"lib_name\"\n";

        let mut names: Vec<String> = dependency_names(toml).into_iter().collect();
        names.sort();

        assert_eq!(
            names,
            [
                "build_dep",
                "dev_only",
                "dotted",
                "inherited",
                "my_dep",
                "quoted_dep",
                "renamed",
                "serde",
                "unix_only",
            ]
        );
    }

    #[test]
    fn julie_names_a_glob_use_by_its_whole_path() {
        let namespace =
            |names: &[&str]| -> Vec<String> { names.iter().map(|name| name.to_string()).collect() };

        assert!(is_glob(
            "a::b",
            "b",
            &namespace(&["a"]),
            "use a::{b::*, Draw};"
        ));
        assert!(is_glob("super", "super", &[], "use super::*;"));
        assert!(!is_glob(
            "Draw",
            "Draw",
            &namespace(&["a"]),
            "use a::{b::*, Draw};"
        ));
        assert!(!is_glob(
            "P2",
            "Paint",
            &namespace(&["a"]),
            "use a::Paint as P2;"
        ));
        assert!(!is_glob("serde", "serde", &[], "use serde;"));
    }

    #[test]
    fn the_package_name_comes_from_the_cargo_toml_in_the_package_folder() {
        let project = crate::safe_tempdir();
        std::fs::create_dir_all(project.path().join("crates/core")).unwrap();
        std::fs::write(
            project.path().join("crates/core/Cargo.toml"),
            "[package]\nname = \"my-core\"\n",
        )
        .unwrap();
        std::fs::write(
            project.path().join("Cargo.toml"),
            "[workspace]\nmembers = []\n",
        )
        .unwrap();

        assert_eq!(
            cargo_package_name(project.path(), "/crates/core").as_deref(),
            Some("my-core")
        );
        assert_eq!(cargo_package_name(project.path(), ""), None);
        assert_eq!(cargo_package_name(project.path(), "/crates/none"), None);
    }

    #[test]
    fn a_star_import_in_a_comment_or_a_string_is_not_read() {
        let source = "package p\n\n/*\nimport a.*\n*/\n// import b.*\nval s = \"\"\"\nimport c.*\n\"\"\"\nval t = \"import d.*\"\nimport e.*\nimport f._ // note\n";

        assert_eq!(star_imports(source, false), ["e", "f"]);
    }

    #[test]
    fn a_groovy_triple_quoted_string_and_a_char_literal_hide_a_star_import() {
        let source = "def s = '''\nimport a.*;\n'''\nchar q = '\"';\nimport b.*;\n";

        assert_eq!(star_imports(source, false), ["b"]);
    }

    #[test]
    fn a_block_comment_nests_only_where_the_language_nests_it() {
        let source = "/* outer /* inner */\nimport a.*;\n*/\nimport b.*;\n";

        assert_eq!(star_imports(source, true), ["b"]);
        assert_eq!(star_imports(source, false), ["a", "b"]);
    }

    #[test]
    fn a_comment_marker_inside_a_line_comment_does_not_open_a_block() {
        let source = "// see /* here\nimport a.*;\n";

        assert_eq!(star_imports(source, false), ["a"]);
    }

    #[test]
    fn a_method_in_an_inherent_impl_has_no_trait_even_after_a_trait_impl() {
        let source = "impl Shape for Circle {\n    fn area(&self) {}\n}\n\nimpl<T> Circle<T>\nwhere\n    T: for<'a> Fn(&'a str),\n{\n    fn area_of(&self) {}\n}\n";

        assert_eq!(context_of(source, "fn area(&self)"), implemented("Shape"));
        assert_eq!(context_of(source, "area_of"), ImplContext::Inherent);
    }

    #[test]
    fn a_header_that_spans_lines_and_a_one_line_impl_are_read() {
        let source = "impl<T>\n    Shape for Wrapper<T>\nwhere\n    T: Clone,\n{\n    fn area(&self) {}\n}\n\nimpl A for X { fn go(&self) {} }\n";

        assert_eq!(context_of(source, "fn area"), implemented("Shape"));
        assert_eq!(context_of(source, "fn go"), implemented("A"));
    }

    #[test]
    fn an_impl_inside_a_method_body_does_not_own_the_next_method() {
        let source = "impl Shape for Circle {\n    fn area(&self) {\n        impl Local for Inner {\n            fn deep(&self) {}\n        }\n    }\n\n    fn name(&self) {}\n}\n";

        assert_eq!(context_of(source, "fn name"), implemented("Shape"));
    }

    #[test]
    fn a_method_declared_in_a_trait_is_outside_every_impl() {
        let source = "impl Shape for Circle {\n    fn area(&self) {}\n}\n\npub trait Sub: Shape {\n    fn area(&self);\n}\n";

        assert_eq!(context_of(source, "fn area(&self);"), ImplContext::Outside);
    }
}
