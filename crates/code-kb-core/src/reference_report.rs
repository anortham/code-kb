use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use rusqlite::types::ToSql;
use rusqlite::{Connection, OptionalExtension, Row, Statement, params, params_from_iter};

use crate::models::{
    LabeledSite, ReferenceCounts, ReferenceReport, ReferenceSite, ReferenceTarget, SiteLabel,
    Symbol, TextOnlyLines, TextScanSummary,
};
use crate::queries::{
    QueryError, built_receiver_sql, call_language_family, call_languages_compatible,
    class_chain_sql, constructed_class, direct_identifier_rows, documentation_language_list,
    enclosing_scope_sql, get_symbol_by_name, grouped_member_access_rows, has_column,
    implicit_receiver_language_list, import_sites, import_sites_of_absent, inherited_writes_among,
    is_js_family, module_import_binding_sql, owner_binding_in_scope_sql, path_proximity,
    populate_lambda_enclosing_names, receiver_type_sql, symbol_not_found_parts,
};
use crate::text_scan::scan_word;

pub(crate) mod arity;
mod js_bindings;
mod js_modules;
mod receiver_types;
mod sections;
pub(crate) mod variant_names;

pub(crate) use sections::{destructurings, enum_variant_names, uses_of_variants};

const RESOLVER_ROW_CAP: usize = 5_000;
const OTHER_DEFINITION_CAP: usize = 12;
/// How long the per-site checks of one report may run; later sites keep the label their name gives.
const ACCOUNTING_TIME_LIMIT: Duration = Duration::from_secs(3);
/// Rows per judge query, so the time limit is checked between queries.
const JUDGE_CHUNK: usize = 2_000;
const RECEIVER_CHAIN_ANCESTRY_OWNER_CAP: usize = 8_192;
const RECEIVER_CHAIN_ANCESTRY_EDGE_CAP: usize = 8_192;
const RECEIVER_CHAIN_ANCESTRY_DEPTH_CAP: usize = 8;
const NOT_CHECKED: &str = "not checked within the time limit";
const TEXT_SCAN_BUDGET: Duration = Duration::from_secs(3);

const CLASS_LIKE_KINDS: &[&str] = &[
    "class",
    "struct",
    "interface",
    "trait",
    "enum",
    "record",
    "object",
    "protocol",
    "union",
];
const SELF_RECEIVERS: &[&str] = &["self", "this", "cls", "Self", "super"];
const EXTERNAL_RUST_ROOTS: &[&str] = &["std", "core", "alloc"];
/// The basis of a claim that rests on an import; a package import can match several files.
const IMPORT_BASIS: &str = "import";
/// The bases that rest on the written name alone, which a callback parameter of that name hides.
const NAME_RESOLVED_BASES: &[&str] = &[
    IMPORT_BASIS,
    "same file",
    "enclosing scope",
    "qualified path",
];

/// Selects the callers target by name exactly as `find_references_scoped` does: the same
/// `QueryError::SymbolNotFound` (built with `symbol_not_found_parts`) or ambiguity error.
pub fn select_reference_target(
    conn: &Connection,
    name: &str,
    path_filter: Option<&str>,
) -> Result<Symbol, QueryError> {
    match get_symbol_by_name(conn, name, path_filter)? {
        Some(target) => Ok(target),
        None => {
            let (workspace, hint) = symbol_not_found_parts(conn, name, path_filter);
            Err(QueryError::SymbolNotFound {
                name: name.to_string(),
                workspace,
                hint,
            })
        }
    }
}

/// The labeled sites of the selected target and a whole-word text count of its name.
/// `root` is the workspace root the text scan walks. `include_external` only matters for callees.
pub fn reference_report(
    conn: &Connection,
    root: &Path,
    target: &Symbol,
    _include_external: bool,
) -> Result<ReferenceReport, QueryError> {
    report_within(conn, root, target, ACCOUNTING_TIME_LIMIT)
}

/// The report the post-edit check reads: the labeled sites and the reach sections of `target`,
/// without the whole-word text scan and without the exclusion of a call whose argument count the
/// target does not accept, so a call that still passes the old count keeps its label. `target`
/// may be a symbol the index no longer holds; the report then judges the sites of its name as if
/// the symbol were still there, with its owner (`parent_symbol_id`) taken from the index.
pub fn reference_report_for_edit(
    conn: &Connection,
    root: &Path,
    target: &Symbol,
    absent_condition: Option<&str>,
) -> Result<ReferenceReport, QueryError> {
    report_with(
        conn,
        root,
        target,
        ACCOUNTING_TIME_LIMIT,
        ReportMode {
            text_scan: false,
            arity_exclusion: false,
            absent_target: crate::queries::get_symbol_by_id(conn, &target.symbol_id)?.is_none(),
        },
        absent_condition,
    )
}

pub(crate) fn reference_report_for_removed_member(
    conn: &Connection,
    root: &Path,
    owner: &Symbol,
    name: &str,
) -> Result<ReferenceReport, QueryError> {
    let mut target = owner.clone();
    target.symbol_id = format!("removed-member:{}:{name}", owner.symbol_id);
    target.name = name.to_string();
    target.kind = "field".to_string();
    target.signature = None;
    target.parent_symbol_id = Some(owner.symbol_id.clone());
    target.start_line = owner.start_line;
    target.start_column = owner.start_column;
    target.end_line = owner.start_line;
    target.end_column = owner.start_column;
    target.body_start_line = None;
    target.body_start_column = None;
    target.body_end_line = None;
    target.body_end_column = None;
    target.body_start_byte = None;
    target.body_end_byte = None;
    target.body_hash = None;
    report_with(
        conn,
        root,
        &target,
        ACCOUNTING_TIME_LIMIT,
        ReportMode {
            text_scan: false,
            arity_exclusion: false,
            absent_target: true,
        },
        None,
    )
}

/// The report `blast_radius` reads when its walk finds no caller or no test: the labels and the
/// reach sections of `find_references`, without the whole-word text scan.
pub fn reference_report_for_impact(
    conn: &Connection,
    root: &Path,
    target: &Symbol,
) -> Result<ReferenceReport, QueryError> {
    report_with(
        conn,
        root,
        target,
        ACCOUNTING_TIME_LIMIT,
        ReportMode {
            text_scan: false,
            arity_exclusion: true,
            absent_target: false,
        },
        None,
    )
}

/// What a report computes besides the labeled sites.
#[derive(Clone, Copy)]
struct ReportMode {
    text_scan: bool,
    arity_exclusion: bool,
    /// The index does not hold the target, so its row cannot be joined by id.
    absent_target: bool,
}

fn report_within(
    conn: &Connection,
    root: &Path,
    target: &Symbol,
    time_limit: Duration,
) -> Result<ReferenceReport, QueryError> {
    let mode = ReportMode {
        text_scan: true,
        arity_exclusion: true,
        absent_target: false,
    };
    report_with(conn, root, target, time_limit, mode, None)
}

fn report_with(
    conn: &Connection,
    root: &Path,
    target: &Symbol,
    time_limit: Duration,
    mode: ReportMode,
    absent_condition: Option<&str>,
) -> Result<ReferenceReport, QueryError> {
    let class = constructor_class(conn, target)?;
    let group = overload_group(conn, target)?;
    let mut accounting = Accounting::new(conn, root, time_limit);
    accounting.absent = mode.absent_target.then(|| target.clone());
    accounting.target_condition = match absent_condition {
        Some(condition) => Some(condition.to_string()),
        None => preprocessor_condition(conn, &target.symbol_id)?,
    };
    let target_arity = match arity::group_arity(conn, root, &group, false)? {
        Some(own) if mode.arity_exclusion => {
            arity::widened_by_ancestors(&mut accounting, target, own)?
        }
        _ => None,
    };
    accounting.restart_clock(time_limit);
    let mut specs = vec![Spec {
        name: target.name.clone(),
        arity: target_arity,
        local_to: enclosing_function(conn, &group)?,
        group,
        scope: if class.is_some() {
            RowScope::Calls
        } else {
            RowScope::All
        },
        builds_class: false,
    }];
    if let Some(class) = &class {
        specs.push(Spec {
            name: class.name.clone(),
            group: vec![class.clone()],
            scope: RowScope::Calls,
            builds_class: true,
            local_to: None,
            arity: if mode.arity_exclusion {
                arity::group_arity(conn, root, &specs[0].group, true)?
            } else {
                None
            },
        });
    }

    let mut site_rows = Vec::new();
    let mut cannot_see = Vec::new();
    for spec in &specs {
        accounting.account_spec(spec, &mut site_rows, &mut cannot_see)?;
    }
    let definitions_with_name = accounting.definition_count(&target.name)?;

    let mut sites = merge_site_rows(site_rows);
    let unchecked = sites
        .iter()
        .filter(|site| site.basis.ends_with(NOT_CHECKED))
        .count();
    if unchecked > 0 {
        cannot_see.push(format!(
            "{unchecked} of {} sites were not checked within the {} s time limit",
            sites.len(),
            time_limit.as_secs()
        ));
    }
    let mut report_sites: Vec<ReferenceSite> = sites.iter().map(|row| row.site.clone()).collect();
    populate_lambda_enclosing_names(conn, &mut report_sites)?;
    for (row, site) in sites.iter_mut().zip(report_sites) {
        row.site.enclosing_symbol_name = site.enclosing_symbol_name;
    }
    sites.sort_by(|a, b| {
        a.label
            .rank()
            .cmp(&b.label.rank())
            .then_with(|| a.site.path.cmp(&b.site.path))
            .then_with(|| a.site.start_line.cmp(&b.site.start_line))
            .then_with(|| a.site.start_column.cmp(&b.site.start_column))
    });

    let text = match &class {
        _ if !mode.text_scan => TextOutcome::not_run(),
        Some(class) => TextOutcome::skipped(class),
        None => accounting.text_outcome(root, &target.name, &sites)?,
    };

    let mut counts = ReferenceCounts {
        sites: sites.len(),
        ..text.counts
    };
    for site in &sites {
        match site.label {
            SiteLabel::Proven => counts.proven += 1,
            SiteLabel::Likely => counts.likely += 1,
            SiteLabel::Unresolved => counts.unresolved += 1,
            SiteLabel::Excluded => counts.excluded += 1,
        }
    }

    let reach = sections::reach_sections(&mut accounting, target, &sites)?;
    cannot_see.splice(0..0, cannot_see_for_language(&target.language));
    cannot_see.extend(reach.notes);
    let report = ReferenceReport {
        target: report_target(conn, target, definitions_with_name)?,
        counts,
        sites,
        text_only: text.text_only,
        text_scan: text.summary,
        cannot_see,
        imports: match mode.absent_target {
            true => import_sites_of_absent(
                conn,
                &target.name,
                &target.path,
                target.parent_symbol_id.is_some(),
            )?,
            false => import_sites(conn, &target.name, Some(&target.path))?,
        },
        overrides: reach.overrides,
        overridden: reach.overridden,
        constructions: reach.constructions,
        variant_uses: reach.variant_uses,
        registrations: reach.registrations,
        tests: reach.tests,
    };
    Ok(report)
}

fn cannot_see_for_language(language: &str) -> Vec<String> {
    let mut items = Vec::new();
    match language {
        "python" => items.push("calls through getattr() or names built at runtime".to_string()),
        language if is_js_family(language) => items
            .push("calls through computed property names or values built at runtime".to_string()),
        "rust" => items.push("calls inside macro bodies".to_string()),
        "java" | "csharp" | "kotlin" => {
            items.push("calls through reflection or dependency injection".to_string());
        }
        "go" => items.push(
            "calls through a Go interface that the index does not record reach methods this list does not show"
                .to_string(),
        ),
        _ => {}
    }
    items
}

fn report_target(
    conn: &Connection,
    target: &Symbol,
    definitions_with_name: usize,
) -> Result<ReferenceTarget, QueryError> {
    let mut qualified = target.clone();
    crate::queries::qualify_members(conn, std::iter::once(&mut qualified))?;
    Ok(ReferenceTarget {
        symbol_id: target.symbol_id.clone(),
        name: target.name.clone(),
        display_name: qualified.name,
        kind: crate::formatters::display_kind(target).to_string(),
        path: target.path.clone(),
        line: target.start_line,
        definitions_with_name,
    })
}

fn is_constructor(target: &Symbol) -> bool {
    target.kind == "constructor" || (target.language == "python" && target.name == "__init__")
}

fn constructor_class(conn: &Connection, target: &Symbol) -> Result<Option<Symbol>, QueryError> {
    if !is_constructor(target) {
        return Ok(None);
    }
    let class_id = match constructed_class(conn, &target.symbol_id)? {
        Some((id, _)) => id,
        None => match &target.parent_symbol_id {
            Some(parent) => parent.clone(),
            None => return Ok(None),
        },
    };
    crate::queries::get_symbol_by_id(conn, &class_id)
}

/// The range of the overloads of `target` joined with the same-named methods of its base types,
/// in Java, C#, and Kotlin, where a call sees the overloads it inherits (C++ and TypeScript hide
/// them, and a Python override replaces the method). The own range when the walk was cut.
pub(crate) fn inherited_group_arity(
    conn: &Connection,
    root: &Path,
    target: &Symbol,
) -> Result<Option<arity::Arity>, QueryError> {
    let group = overload_group(conn, target)?;
    let Some(own) = arity::group_arity(conn, root, &group, false)? else {
        return Ok(None);
    };
    if !matches!(target.language.as_str(), "java" | "csharp" | "kotlin") {
        return Ok(Some(own));
    }
    let mut accounting = Accounting::new(conn, root, ACCOUNTING_TIME_LIMIT);
    Ok(arity::widened_by_ancestors(&mut accounting, target, own.clone())?.or(Some(own)))
}

/// The selected symbol and its overloads: the symbols with its name, owner, kind, and file.
pub(crate) fn overload_group(
    conn: &Connection,
    target: &Symbol,
) -> Result<Vec<Symbol>, QueryError> {
    let mut stmt = conn.prepare(
        "SELECT symbol_id FROM symbols
         WHERE name = ?1 AND path = ?2 AND kind = ?3 AND parent_symbol_id IS ?4
         ORDER BY start_line, symbol_id",
    )?;
    let ids = stmt
        .query_map(
            params![
                target.name,
                target.path,
                target.kind,
                target.parent_symbol_id
            ],
            |row| row.get::<_, String>(0),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let mut group = vec![target.clone()];
    for id in ids.into_iter().filter(|id| *id != target.symbol_id) {
        if let Some(symbol) = crate::queries::get_symbol_by_id(conn, &id)? {
            group.push(symbol);
        }
    }
    Ok(group)
}

const DEFINITION_BY_ID_SQL: &str =
    "SELECT s.symbol_id, s.name, s.kind, s.language, s.path, s.start_line,
            s.body_start_line, s.parent_symbol_id, par.name, par.kind, COALESCE(s.signature, ''),
            COALESCE(par.signature, '')
     FROM symbols s LEFT JOIN symbols par ON par.symbol_id = s.parent_symbol_id
     WHERE s.symbol_id = ?1";

/// Whether the relationship row `r` links its call to a member of a class that encloses the
/// caller, the caller included; `t` is the linked symbol.
const LINKS_ENCLOSING_CLASS_SQL: &str = "EXISTS (
         WITH RECURSIVE enclosing(symbol_id, depth) AS (
             SELECT r.from_symbol_id, 0
             UNION ALL
             SELECT s.parent_symbol_id, enclosing.depth + 1
             FROM symbols s JOIN enclosing ON s.symbol_id = enclosing.symbol_id
             WHERE s.parent_symbol_id IS NOT NULL AND enclosing.depth < 32
         )
         SELECT 1 FROM enclosing WHERE symbol_id = t.parent_symbol_id
     )";

const SHADOW_IN_CHAIN_SQL: &str = "WITH RECURSIVE chain(symbol_id, depth) AS (
         SELECT ?1, 0
         UNION ALL
         SELECT s.parent_symbol_id, chain.depth + 1
         FROM symbols s JOIN chain ON s.symbol_id = chain.symbol_id
         WHERE s.parent_symbol_id IS NOT NULL AND chain.depth < 32
     )
     SELECT d.symbol_id, COALESCE(d.signature, ''), d.kind
     FROM chain
     JOIN symbols scope ON scope.symbol_id = chain.symbol_id
     JOIN symbols d ON d.parent_symbol_id = chain.symbol_id
     WHERE d.name = ?2 AND d.path = ?3
       AND (d.kind IN ('variable', 'parameter', 'function', 'constant', 'class')
            OR ?4 AND d.kind IN ('type', 'interface', 'enum'))
       AND (chain.depth = 0 OR scope.kind IN ('function', 'method', 'constructor'))
     ORDER BY chain.depth, d.start_line > ?5, abs(?5 - d.start_line)";

/// An ownerless JavaScript method is an object-literal member or a property, which binds no name.
const SHADOW_TOP_LEVEL_SQL: &str = concat!(
    "SELECT symbol_id, COALESCE(signature, ''), kind FROM symbols
     WHERE name = ?1 AND path = ?2 AND parent_symbol_id IS NULL
       AND kind NOT IN ('import', 'export', 'module', 'namespace')
       AND NOT (kind = 'method' AND language IN (",
    js_family_sql!(),
    "))
     ORDER BY start_line > ?3, abs(?3 - start_line)"
);

const IMPORT_SOURCES_SQL: &str = "SELECT COALESCE(json_extract(metadata_json, '$.source'), '')
     FROM symbols
     WHERE kind = 'import' AND path = ?1 AND json_valid(metadata_json)
       AND (name = ?2 OR json_extract(metadata_json, '$.alias') = ?2
            OR json_extract(metadata_json, '$.local_name') = ?2)";

const IMPORT_SIGNATURES_SQL: &str = "SELECT COALESCE(signature, '') FROM symbols
     WHERE kind = 'import' AND path = ?1 AND name = ?2";

const PYTHON_IMPORT_ROWS_SQL: &str = "SELECT COALESCE(signature, ''),
            COALESCE(json_extract(metadata_json, '$.source'), ''),
            COALESCE(json_extract(metadata_json, '$.importedName'), '')
     FROM symbols
     WHERE kind = 'import' AND path = ?1 AND json_valid(metadata_json)
       AND (name = ?2 OR json_extract(metadata_json, '$.alias') = ?2
            OR json_extract(metadata_json, '$.local_name') = ?2)";

const PENDING_IMPORT_MODULES_SQL: &str = "SELECT target_namespace_json FROM pending_relationships
     WHERE kind = 'imports' AND path = ?1 AND target_terminal_name = ?2";

const FILE_EXISTS_SQL: &str = "SELECT 1 FROM files WHERE path = ?1 OR path = ?2 LIMIT 1";

const PYTHON_FILES_NAMED_SQL: &str = "SELECT replace(path, '\\', '/') FROM files
     WHERE language = 'python'
       AND ('/' || replace(path, '\\', '/') LIKE ?1 OR '/' || replace(path, '\\', '/') LIKE ?2)
     LIMIT 200";

/// At this many candidate files an import counts as reaching the project, unchecked.
const PYTHON_FILES_NAMED_CAP: usize = 200;

const SOURCE_FILES_SQL: &str = "SELECT 1 FROM files
     WHERE '/' || replace(replace(path, '\\', '/'), '-', '_') LIKE ?1 ESCAPE '\\'
        OR '/' || replace(replace(path, '\\', '/'), '-', '_') LIKE ?2 ESCAPE '\\'
     LIMIT 1";

const MAX_GO_ALIAS_HOPS: usize = 4;

const GO_ALIAS_TARGETS_SQL: &str = "SELECT path, json_extract(metadata_json, '$.alias_target')
     FROM symbols
     WHERE name = ?1 AND language = 'go' AND kind IN ('type', 'type_alias')
       AND json_valid(metadata_json)
       AND json_extract(metadata_json, '$.alias_target') IS NOT NULL";

const GO_IMPORT_PATHS_SQL: &str = "SELECT COALESCE(signature, '') FROM symbols
     WHERE path = ?1 AND name = ?2 AND language = 'go' AND kind = 'import'";

const GO_PACKAGE_SQL: &str = "SELECT name FROM symbols
     WHERE path = ?1 AND language = 'go' AND kind = 'module' LIMIT 1";

const RECEIVER_TYPE_SQL: &str = "WITH RECURSIVE chain(symbol_id, depth) AS (
         SELECT ?1, 0
         UNION ALL
         SELECT s.parent_symbol_id, chain.depth + 1
         FROM symbols s JOIN chain ON s.symbol_id = chain.symbol_id
         WHERE s.parent_symbol_id IS NOT NULL AND chain.depth < 32
     ), typed(depth, resolved_type) AS (
         SELECT chain.depth, tf.resolved_type
         FROM chain
         JOIN symbols r ON r.parent_symbol_id = chain.symbol_id
         JOIN type_facts tf ON tf.symbol_id = r.symbol_id
         WHERE r.name = ?2 AND r.path = ?3
           AND r.kind IN ('variable', 'parameter', 'field', 'property')
     )
     SELECT DISTINCT resolved_type FROM typed
     WHERE depth = (SELECT MIN(depth) FROM typed)";

const ANCESTORS_BY_RELATIONSHIP_SQL: &str = "SELECT DISTINCT base.name
     FROM symbols child
     JOIN relationships r ON r.from_symbol_id = child.symbol_id
          AND r.kind IN ('extends', 'implements')
     JOIN symbols base ON base.symbol_id = r.to_symbol_id
     WHERE child.name = ?1";

const ANCESTORS_BY_PENDING_SQL: &str = "SELECT DISTINCT pe.target_terminal_name
     FROM symbols child
     JOIN pending_relationships pe ON pe.from_symbol_id = child.symbol_id
          AND pe.kind IN ('extends', 'implements')
     WHERE child.name = ?1";

const OWNER_IN_PACKAGE_SQL: &str = "SELECT COUNT(*) FROM symbols
     WHERE name = ?1
       AND kind IN ('class', 'struct', 'interface', 'trait', 'enum', 'record', 'object', 'protocol', 'union')
       AND replace(path, '\\', '/') LIKE ?2 ESCAPE '\\'";

const TYPE_HEAD_KINDS_SQL: &str =
    "SELECT DISTINCT kind FROM symbols WHERE name = ?1 AND kind != 'parameter'";

const RECEIVER_DECLARATIONS_SQL: &str = "WITH RECURSIVE chain(symbol_id, depth) AS (
         SELECT ?1, 0
         UNION ALL
         SELECT s.parent_symbol_id, chain.depth + 1
         FROM symbols s JOIN chain ON s.symbol_id = chain.symbol_id
         WHERE s.parent_symbol_id IS NOT NULL AND chain.depth < 32
     )
     SELECT chain.depth, r.kind, r.start_line FROM chain
     JOIN symbols r ON r.parent_symbol_id = chain.symbol_id
     WHERE r.name = ?2 AND r.path = ?3 AND r.kind IN ('variable', 'constant', 'parameter')
     UNION ALL
     SELECT 99, r.kind, r.start_line FROM symbols r
     WHERE r.parent_symbol_id IS NULL AND r.name = ?2 AND r.path = ?3
       AND r.kind IN ('variable', 'constant')
     ORDER BY 1, 3 DESC";

const JS_IMPORTER_PATHS_SQL: &str = concat!(
    "SELECT DISTINCT replace(path, '\\', '/') FROM symbols
     WHERE kind = 'import' AND name = ?1
       AND language IN (",
    js_family_sql!(),
    ")"
);

const MEMBER_ACCESS_PATHS_SQL: &str =
    "SELECT DISTINCT path FROM identifiers WHERE name = ?1 AND kind = 'member_access'";

const VARIABLE_REF_SITES_SQL: &str = "SELECT path, start_line, start_column FROM identifiers
     WHERE name = ?1 AND kind = 'variable_ref' AND start_column IS NOT NULL";

const TYPE_DECLARATIONS_SQL: &str = "SELECT COALESCE(signature, '') FROM symbols
     WHERE name = ?1 AND kind = 'type' AND language = ?2";

const SYMBOL_CHAIN_SQL: &str = "WITH RECURSIVE chain(symbol_id, depth) AS (
         SELECT ?1, 0
         UNION ALL
         SELECT s.parent_symbol_id, chain.depth + 1
         FROM symbols s JOIN chain ON s.symbol_id = chain.symbol_id
         WHERE s.parent_symbol_id IS NOT NULL AND chain.depth < 32
     )
     SELECT s.kind, s.name FROM chain JOIN symbols s ON s.symbol_id = chain.symbol_id
     ORDER BY chain.depth";

const CSHARP_USINGS_SQL: &str = "SELECT COALESCE(signature, '') FROM symbols
     WHERE kind = 'import' AND language = 'csharp'
       AND (path = ?1 OR signature LIKE 'global using %')";

const PREPROCESSOR_CONDITION_SQL: &str = "SELECT json_extract(metadata_json, '$.preprocessorIf')
     FROM symbols WHERE symbol_id = ?1 AND json_valid(metadata_json)";

const CSHARP_FILE_USINGS_SQL: &str = "SELECT COALESCE(signature, ''),
            CASE WHEN json_valid(metadata_json)
                 THEN json_extract(metadata_json, '$.preprocessorIf') END
     FROM symbols
     WHERE kind = 'import' AND language = 'csharp' AND path = ?1";

const CSHARP_CONDITIONAL_USINGS_SQL: &str = "SELECT COALESCE(signature, ''),
            json_extract(metadata_json, '$.preprocessorIf')
     FROM symbols
     WHERE kind = 'import' AND language = 'csharp' AND json_valid(metadata_json)
       AND json_extract(metadata_json, '$.preprocessorIf') IS NOT NULL";

const MSBUILD_FILES_SQL: &str = "SELECT path FROM files
     WHERE path LIKE '%.csproj' OR path LIKE '%.props' OR path LIKE '%.targets'";

const CSHARP_TYPE_KINDS: &[&str] = &["class", "struct", "interface", "enum", "record", "delegate"];

const JAVA_STATIC_IMPORTS_SQL: &str = "SELECT signature FROM symbols
     WHERE kind = 'import' AND path = ?1 AND (name = ?2 OR signature LIKE '%.*')
       AND signature LIKE 'import static %'";

const CALLABLE_IN_FILE_SQL: &str = "SELECT 1 FROM symbols
     WHERE path = ?1 AND name = ?2 AND kind IN ('method', 'function', 'constructor') LIMIT 1";

const TOP_LEVEL_VARIABLE_SQL: &str = "SELECT 1 FROM symbols
     WHERE path = ?1 AND name = ?2 AND parent_symbol_id IS NULL
       AND kind IN ('variable', 'constant') LIMIT 1";

const FN_TYPES: [&str; 5] = ["fn(", "fn (", "Fn(", "FnMut(", "FnOnce("];

/// Which indexed rows of a name a spec accounts for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RowScope {
    All,
    Calls,
    /// Only the pending `extends` and `implements` rows, which tie a type to its base by name.
    Inheritance,
}

struct Spec {
    name: String,
    group: Vec<Symbol>,
    scope: RowScope,
    builds_class: bool,
    /// The arguments a call to the group may pass; `None` when the index cannot tell.
    arity: Option<arity::Arity>,
    /// The function that holds a group of local functions, which no site outside it can name.
    local_to: Option<Symbol>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Source {
    Pending,
    Relationship,
    Identifier,
}

impl Source {
    fn tag(self) -> &'static str {
        match self {
            Source::Pending => "P",
            Source::Relationship => "R",
            Source::Identifier => "I",
        }
    }
}

struct NameRow {
    key: String,
    source: Source,
    caller_id: Option<String>,
    caller_name: String,
    /// The last line of the caller; julie can name a caller that ends above the row.
    caller_end_line: Option<usize>,
    path: String,
    language: String,
    kind: String,
    line: usize,
    column: Option<usize>,
    end_line: Option<usize>,
    receiver: Option<String>,
    namespace: Vec<String>,
    to_symbol_id: Option<String>,
    site_line: usize,
    site_column: Option<usize>,
    consumed: bool,
    paired: bool,
    /// The call is written on an expression (`getAsSingleElement().getAsInt()`), so it has a
    /// receiver that julie did not record.
    hidden_receiver: bool,
    /// That receiver is the result of a call or an index, or a string literal, and loads no
    /// module (`require(…)`, `import(…)`).
    result_receiver: bool,
    /// The names written before the used name (`["z", "core"]` for `z.core.f()`), `None` when
    /// the row does not say.
    chain: Option<Vec<String>>,
    typed_receiver_chain: Option<Vec<String>>,
    /// The arguments the call passes, `None` when julie recorded no count or the call spreads an
    /// argument, so the count is unknown.
    arg_count: Option<u32>,
    /// julie linked the call to a member of a class that encloses the caller, the caller included.
    links_enclosing_class: bool,
}

impl NameRow {
    fn has_qualifier(&self) -> bool {
        self.hidden_receiver
            || self.receiver.as_deref().is_some_and(|r| !r.is_empty())
            || !self.namespace.is_empty()
    }

    fn qualifier(&self) -> Option<&str> {
        self.namespace
            .first()
            .map(String::as_str)
            .or(self.receiver.as_deref())
            .filter(|q| is_plain_identifier(q))
    }

    fn is_super_call(&self) -> bool {
        self.receiver.as_deref().is_some_and(is_super_receiver)
    }

    fn is_call(&self) -> bool {
        matches!(self.kind.as_str(), "calls" | "call" | "instantiates")
    }

    /// Whether a definition of `kind` never hides the name from this row: a Java call names a
    /// method, so a field, variable, or parameter of that name is another thing.
    fn is_not_named_by(&self, kind: &str) -> bool {
        let value_kind = matches!(
            kind,
            "field" | "property" | "variable" | "constant" | "parameter"
        );
        let java_call = self.language == "java" && matches!(self.kind.as_str(), "calls" | "call");
        let type_use_of_value =
            self.is_bare_type_use() && (value_kind || matches!(kind, "function" | "method"));
        (java_call && value_kind) || type_use_of_value
    }

    fn is_bare_use(&self) -> bool {
        !self.has_qualifier()
            && matches!(
                self.kind.as_str(),
                "calls" | "call" | "variable_ref" | "references"
            )
    }

    /// A JavaScript type usage written without a qualifier (`output<T>`), which a type of that
    /// name in an enclosing scope shadows.
    fn is_bare_type_use(&self) -> bool {
        self.kind == "type_usage"
            && language_family(&self.language) == "js"
            && self.chain.as_ref().is_some_and(Vec::is_empty)
    }

    fn display_kind(&self) -> String {
        match self.kind.as_str() {
            "call" => "calls".to_string(),
            other => other.to_string(),
        }
    }

    /// Whether the row can reach `definition`, another definition of the name: a call cannot
    /// reach a field, and a member access cannot reach a function that only its scope names.
    fn can_reach(&self, definition: &Definition) -> bool {
        let call_of_a_field = self.is_call()
            && !definition.callable
            && languages_compatible(&self.language, &definition.language);
        let member_of_a_scoped_function =
            definition.free && language_family(&self.language) == "js" && self.has_qualifier();
        let call_on_a_result = self.result_receiver
            && definition.module_value
            && language_family(&self.language) == "js";
        !call_of_a_field
            && !member_of_a_scoped_function
            && !call_on_a_result
            && !self.reads_a_module(&definition.kind)
            && !(self.is_bare_use() && definition.is_property_method())
            && self.is_in_scope_of(definition)
            && !(self.is_type_position() && definition.is_value())
    }

    /// A place where a statically typed language names a type: a base type, a built type, a
    /// written type, or the type of a C# member (julie's `uses`).
    fn is_type_position(&self) -> bool {
        STATIC_TYPE_LANGUAGES.contains(&self.language.as_str())
            && (matches!(
                self.kind.as_str(),
                "extends" | "implements" | "instantiates" | "type_usage"
            ) || self.kind == "uses" && self.language == "csharp")
    }

    fn is_in_scope_of(&self, definition: &Definition) -> bool {
        definition.scope.is_none_or(|(start, end)| {
            self.path == definition.path && (start..=end).contains(&self.site_line)
        })
    }

    /// A Rust read of a value (`git.diff()`) never names a module (`mod git;`), which holds none.
    fn reads_a_module(&self, kind: &str) -> bool {
        self.language == "rust"
            && self.kind == "variable_ref"
            && matches!(kind, "namespace" | "module")
    }
}

/// Whether `receiver` makes a call reach an ancestor of the caller's class, never the class
/// itself: `super`, `base`, and the Kotlin form that names the ancestor (`super<Greeter>`).
fn is_super_receiver(receiver: &str) -> bool {
    matches!(receiver, "super" | "base") || receiver.starts_with("super<")
}

/// Languages where a function defined inside a function is named only inside it. JavaScript is
/// not one here: such a function can be placed on an object the outer function returns.
/// Languages where a type and a value are apart, so a place that names a type never names a value.
const STATIC_TYPE_LANGUAGES: &[&str] = &[
    "csharp", "java", "kotlin", "scala", "swift", "dart", "rust", "go", "cpp", "c", "vbnet",
];

const LOCAL_FUNCTION_LANGUAGES: &[&str] = &[
    "python", "rust", "kotlin", "csharp", "swift", "scala", "dart",
];

/// The function, method, or constructor that holds every member of `group` when each one is a
/// function defined inside it in a language of [`LOCAL_FUNCTION_LANGUAGES`].
fn enclosing_function(conn: &Connection, group: &[Symbol]) -> Result<Option<Symbol>, QueryError> {
    let Some(parent) = group
        .first()
        .and_then(|first| first.parent_symbol_id.as_deref())
    else {
        return Ok(None);
    };
    let local = group.iter().all(|member| {
        member.kind == "function"
            && LOCAL_FUNCTION_LANGUAGES.contains(&member.language.as_str())
            && member.parent_symbol_id.as_deref() == Some(parent)
    });
    if !local {
        return Ok(None);
    }
    Ok(crate::queries::get_symbol_by_id(conn, parent)?
        .filter(|outer| matches!(outer.kind.as_str(), "function" | "method" | "constructor")))
}

/// A JavaScript method assigned to a property (`req.accepts = function`): no scope binds its name,
/// so a bare use of that name reads an import or a global.
fn is_property_method(symbol: &Symbol) -> bool {
    symbol.kind == "method"
        && symbol.parent_symbol_id.is_none()
        && language_family(&symbol.language) == "js"
}

/// The object a JavaScript method is assigned on (`req` for `req.param = function`), which julie
/// keeps as `className` in metadata that a removed symbol no longer has.
fn assigned_object(symbol: &Symbol) -> Option<String> {
    if language_family(&symbol.language) != "js" {
        return None;
    }
    let signature = symbol.signature.as_deref()?;
    let (object, _) = signature.split_once(&format!(".{} =", symbol.name))?;
    Some(object.trim().to_string()).filter(|object| is_plain_identifier(object))
}

/// The `#if` condition of the branch that julie kept a symbol from (`preprocessorIf`), nested
/// conditions joined with ` && `.
pub(crate) fn preprocessor_condition(
    conn: &Connection,
    symbol_id: &str,
) -> Result<Option<String>, QueryError> {
    Ok(conn
        .query_row(PREPROCESSOR_CONDITION_SQL, params![symbol_id], |r| {
            r.get::<_, Option<String>>(0)
        })
        .optional()?
        .flatten())
}

/// The terms of a `preprocessorIf` condition: the conditions of nested branches, joined with
/// ` && `.
fn condition_terms(condition: &str) -> impl Iterator<Item = &str> {
    condition.split(" && ").map(str::trim)
}

/// The namespace a C# `using` directive opens (`A.B` for `using A.B;` or `global using A.B;`),
/// none for a `using static` or an alias.
pub(crate) fn csharp_using_namespace(signature: &str) -> Option<&str> {
    let signature = signature.trim();
    let directive = signature.strip_prefix("global ").unwrap_or(signature);
    let namespace = directive
        .strip_prefix("using ")?
        .trim()
        .trim_end_matches(';')
        .trim();
    (!namespace.starts_with("static ") && !namespace.contains('=')).then_some(namespace)
}

pub(crate) fn csharp_using_alias(signature: &str) -> Option<(&str, &str)> {
    let signature = signature.trim();
    let directive = signature.strip_prefix("global ").unwrap_or(signature);
    let alias = directive
        .strip_prefix("using ")?
        .trim()
        .trim_end_matches(';');
    let (name, target) = alias.split_once('=')?;
    let name = name.trim();
    let target = target.trim();
    (is_plain_identifier(name) && !target.is_empty()).then_some((name, target))
}

pub(crate) struct CSharpConstructorQualifier {
    pub(crate) path: String,
    pub(crate) global: bool,
}

pub(crate) fn csharp_constructor_qualifier(
    line: &str,
    column: usize,
) -> Option<CSharpConstructorQualifier> {
    let before = line.get(..column)?.trim_end();
    let before = before.strip_suffix('.')?;
    let split = before.rfind(char::is_whitespace)?;
    let before_new = before.get(..split)?.trim_end();
    let path = before.get(split + 1..)?;
    let global = path.starts_with("global::");
    let path = path.strip_prefix("global::").unwrap_or(path);
    if path.is_empty() || !path.split('.').all(is_plain_identifier) {
        return None;
    }
    let new_start = before_new.rfind("new")?;
    if before_new.get(new_start..)? != "new"
        || before_new[..new_start]
            .chars()
            .next_back()
            .is_some_and(is_word_char)
    {
        return None;
    }
    Some(CSharpConstructorQualifier {
        path: path.to_string(),
        global,
    })
}

pub(crate) fn csharp_namespace_for_symbol(
    conn: &Connection,
    symbol_id: &str,
) -> Result<String, QueryError> {
    let mut stmt = conn.prepare(SYMBOL_CHAIN_SQL)?;
    let chain = stmt
        .query_map(params![symbol_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut parts: Vec<String> = chain
        .into_iter()
        .filter(|(kind, _)| kind == "namespace")
        .map(|(_, name)| name)
        .collect();
    parts.reverse();
    Ok(parts.join("."))
}

pub(crate) fn csharp_type_qualification_for_symbol(
    conn: &Connection,
    symbol_id: &str,
) -> Result<Vec<String>, QueryError> {
    let mut stmt = conn.prepare(SYMBOL_CHAIN_SQL)?;
    let chain = stmt
        .query_map(params![symbol_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut path = Vec::new();
    let mut containing_types = Vec::new();
    for (depth, (kind, name)) in chain.into_iter().enumerate() {
        if kind == "namespace" {
            path.push(name);
        } else if depth > 0 && CSHARP_TYPE_KINDS.contains(&kind.as_str()) {
            containing_types.push(name);
        }
    }
    path.reverse();
    containing_types.reverse();
    path.extend(containing_types);
    Ok(path)
}

pub(crate) type CSharpTypeQualifications = (Vec<(String, Vec<String>)>, bool);

pub(crate) fn csharp_type_qualifications_for_name(
    conn: &Connection,
    name: &str,
) -> Result<CSharpTypeQualifications, QueryError> {
    let mut stmt = conn.prepare(
        "SELECT symbol_id FROM symbols
         WHERE language = 'csharp' AND name = ?1
           AND kind IN ('class', 'struct', 'interface', 'enum', 'record', 'delegate')
         LIMIT 13",
    )?;
    let ids = stmt
        .query_map(params![name], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let complete = ids.len() <= 12;
    let mut paths = Vec::with_capacity(ids.len());
    for id in ids {
        paths.push((id.clone(), csharp_type_qualification_for_symbol(conn, &id)?));
    }
    Ok((paths, complete))
}

pub(crate) fn csharp_type_prefix_in_scope(
    conn: &Connection,
    caller_id: &str,
    prefix: &str,
) -> Result<bool, QueryError> {
    conn.query_row(
        "WITH RECURSIVE chain(symbol_id, depth) AS (
             SELECT ?1, 0
             UNION ALL
             SELECT s.parent_symbol_id, chain.depth + 1
             FROM symbols s JOIN chain ON s.symbol_id = chain.symbol_id
             WHERE s.parent_symbol_id IS NOT NULL AND chain.depth < 32
         )
         SELECT EXISTS(
             SELECT 1 FROM symbols child JOIN chain
               ON child.parent_symbol_id = chain.symbol_id
             WHERE chain.depth > 0 AND child.language = 'csharp'
               AND child.name = ?2
               AND child.kind IN ('class', 'struct', 'interface', 'enum', 'record', 'delegate')
         )",
        params![caller_id, prefix],
        |row| row.get(0),
    )
    .map_err(QueryError::from)
}

/// The type a C# extension method extends: `IEnumerable` for `Foo(this IEnumerable<T> source)`.
fn extended_type(signature: &str) -> Option<String> {
    let rest = signature
        .match_indices('(')
        .find_map(|(open, _)| arity::after_csharp_this(&signature[open + 1..]))?;
    let written = rest.split_whitespace().next()?;
    let unqualified = written.split('<').next()?.rsplit('.').next()?;
    Some(unqualified.trim_end_matches('?').to_string()).filter(|t| is_plain_identifier(t))
}

/// A test-framework call that julie names after its label (`test("strictObject")`): code
/// never calls the label by name.
fn is_test_label(signature: &str) -> bool {
    signature.split_once('(').is_some_and(|(callee, rest)| {
        !callee.is_empty()
            && callee.split('.').all(is_plain_identifier)
            && rest.trim_start().starts_with(['"', '\'', '`'])
    })
}

fn is_plain_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        && chars.all(is_word_char)
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Whether a Java type signature declares a record (`@Deprecated(since = "1") public record
/// Point(int x, int y)`): the word `record` followed by the type name.
fn is_record(signature: &str) -> bool {
    let declaration = outside_quotes_and_parentheses(signature);
    declaration.match_indices("record").any(|(at, word)| {
        let after = &declaration[at + word.len()..];
        !declaration[..at].ends_with(is_word_char)
            && after.starts_with(char::is_whitespace)
            && after.trim_start().starts_with(is_word_char)
    })
}

/// `text` without string literals and without the text inside parentheses, each replaced by a
/// space (`@Schema(description = "a record") public class T` is `@Schema  public class T`).
fn outside_quotes_and_parentheses(text: &str) -> String {
    let mut outside = String::with_capacity(text.len());
    let mut depth = 0usize;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' | '\'' => {
                while let Some(inner) = chars.next() {
                    match inner {
                        '\\' => {
                            chars.next();
                        }
                        _ if inner == c => break,
                        _ => {}
                    }
                }
                if depth == 0 {
                    outside.push(' ');
                }
            }
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    outside.push(' ');
                }
            }
            _ if depth == 0 => outside.push(c),
            _ => {}
        }
    }
    outside
}

/// Whether the declaration `type Name func(...)`, `type Name = func(...)`, `type Name[T any]
/// func(T)`, or Rust `type Name = fn(...)` declares a function type.
fn is_function_type(signature: &str, name: &str, language: &str) -> bool {
    let Some(declared) = signature
        .trim_start()
        .strip_prefix("type")
        .and_then(|rest| rest.trim_start().strip_prefix(name))
    else {
        return false;
    };
    let declared = declared.trim_start();
    let declared = match declared.chars().next() {
        Some(open @ ('[' | '<')) => {
            let close = if open == '[' { ']' } else { '>' };
            let mut depth = 0;
            let end = declared.char_indices().find(|&(_, c)| {
                depth += i32::from(c == open) - i32::from(c == close);
                depth == 0
            });
            end.map_or("", |(end, _)| &declared[end + 1..])
        }
        _ => declared,
    };
    let declared = declared.trim_start().trim_start_matches('=').trim_start();
    if language == "go" {
        declared.starts_with("func")
    } else {
        FN_TYPES.iter().any(|fn_type| declared.contains(fn_type))
    }
}

/// `line` (1-based) split at the byte `column`.
fn split_line<'a>(lines: &[&'a str], line: usize, column: usize) -> Option<(&'a str, &'a str)> {
    lines.get(line.checked_sub(1)?)?.split_at_checked(column)
}

/// `text` without the blanks and `/* */` comments before it; a `//` comment ends the line.
fn after_blanks_and_comments(mut text: &str) -> &str {
    loop {
        text = text.trim_start();
        if text.starts_with("//") {
            return "";
        }
        match text.strip_prefix("/*") {
            Some(comment) => text = comment.split_once("*/").map_or("", |(_, rest)| rest),
            None => return text,
        }
    }
}

/// `text` without the blanks and `/* */` comments at its end.
fn before_blanks_and_comments(mut text: &str) -> &str {
    loop {
        text = text.trim_end();
        match text
            .strip_suffix("*/")
            .and_then(|comment| comment.rsplit_once("/*"))
        {
            Some((rest, _)) => text = rest,
            None => return text,
        }
    }
}

/// Whether `name` written at `line` and `column` stands where an object literal holds a value
/// (`{ render }`, `{ key: render, other }`): `{`, `,`, `:`, or nothing before it, and `,`, `}`, or
/// nothing after it, blanks and comments aside. A call argument (`f(a, render)`) and a receiver
/// (`render.use()`) do not.
fn is_object_value(lines: &[&str], line: usize, column: usize, name: &str) -> bool {
    let Some((before, rest)) = split_line(lines, line, column) else {
        return false;
    };
    let after = rest.get(name.len()..).unwrap_or("");
    before_blanks_and_comments(before)
        .chars()
        .next_back()
        .is_none_or(|c| matches!(c, '{' | ',' | ':'))
        && after_blanks_and_comments(after)
            .chars()
            .next()
            .is_none_or(|c| matches!(c, ',' | '}'))
}

fn is_object_shorthand_site(text: &str, line: usize, column: usize, name: &str) -> bool {
    let Some(source_line) = source_line(text, line) else {
        return false;
    };
    let Some((_, after)) = source_line.split_at_checked(column) else {
        return false;
    };
    if !matches!(
        after_blanks_and_comments(after.get(name.len()..).unwrap_or(""))
            .chars()
            .next(),
        Some(',' | '}')
    ) {
        return false;
    }
    let Some(site) = line_start_offset(text, line).and_then(|start| start.checked_add(column))
    else {
        return false;
    };
    if site > 4096 {
        return false;
    }
    let Some(prefix) = text.get(..site) else {
        return false;
    };
    let Some(open) = js_bindings::unmatched_brace_before(prefix) else {
        return false;
    };
    let object_prefix = &prefix[open + 1..];
    if object_prefix.contains('{') || object_prefix.contains('}') {
        return false;
    }
    if before_blanks_and_comments(object_prefix)
        .chars()
        .next_back()
        .is_some_and(|c| !matches!(c, '{' | ','))
    {
        return false;
    }
    let before_open = prefix[..open].trim_end();
    let after_open = before_open.chars().next_back();
    if after_open == Some('=')
        && !before_open
            .chars()
            .rev()
            .nth(1)
            .is_some_and(|c| matches!(c, '=' | '!' | '<' | '>'))
    {
        return true;
    }
    if let Some(before_return) = before_open.strip_suffix("return")
        && (before_return.is_empty()
            || !is_word_char(before_return.chars().next_back().unwrap_or(' ')))
        && !prefix[before_return.len() + "return".len()..open]
            .chars()
            .any(|c| matches!(c, '\n' | '\r'))
    {
        return true;
    }
    let Some(call_head) = before_open.strip_suffix('(') else {
        return false;
    };
    let Some((before_new, constructor)) = call_head.rsplit_once("new ") else {
        return false;
    };
    !constructor.is_empty()
        && constructor.chars().all(|c| is_word_char(c) || c == '.')
        && !before_new.chars().next_back().is_some_and(is_word_char)
}

fn line_start_offset(text: &str, line: usize) -> Option<usize> {
    let breaks = line.checked_sub(1)?;
    if breaks == 0 {
        return Some(0);
    }
    text.match_indices('\n')
        .nth(breaks - 1)
        .map(|(at, _)| at + 1)
}

fn source_line(text: &str, line: usize) -> Option<&str> {
    let start = line_start_offset(text, line)?;
    text.get(start..)?
        .split_inclusive('\n')
        .next()
        .map(|line| line.strip_suffix('\n').unwrap_or(line))
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
}

fn caller_source_through_site(
    root: &Path,
    path: &str,
    scope_start_line: usize,
    site_line: usize,
) -> Option<String> {
    if scope_start_line == 0 || site_line < scope_start_line {
        return None;
    }
    const PREFIX_BYTE_CAP: u64 = 512 * 1024;
    const SNIPPET_BYTE_CAP: usize = 4096;
    const SNIPPET_LINE_CAP: usize = 64;
    if site_line - scope_start_line >= SNIPPET_LINE_CAP {
        return None;
    }
    let file = std::fs::File::open(root.join(path)).ok()?;
    let file = std::io::Read::take(file, PREFIX_BYTE_CAP);
    let mut source = std::io::BufReader::new(file);
    for _ in 1..scope_start_line {
        skip_source_line(&mut source)?;
    }
    let mut snippet = Vec::with_capacity(SNIPPET_BYTE_CAP);
    for line in scope_start_line..=site_line {
        append_source_line(
            &mut source,
            &mut snippet,
            SNIPPET_BYTE_CAP,
            line == site_line,
        )?;
    }
    String::from_utf8(snippet).ok()
}

fn skip_source_line(reader: &mut impl std::io::BufRead) -> Option<()> {
    loop {
        let (consumed, newline) = {
            let bytes = std::io::BufRead::fill_buf(reader).ok()?;
            if bytes.is_empty() {
                return None;
            }
            match bytes.iter().position(|byte| *byte == b'\n') {
                Some(position) => (position + 1, true),
                None => (bytes.len(), false),
            }
        };
        std::io::BufRead::consume(reader, consumed);
        if newline {
            return Some(());
        }
    }
}

fn append_source_line(
    reader: &mut std::io::BufReader<std::io::Take<std::fs::File>>,
    output: &mut Vec<u8>,
    byte_cap: usize,
    allow_unterminated: bool,
) -> Option<()> {
    let mut copied = false;
    loop {
        let bytes = std::io::BufRead::fill_buf(reader).ok()?;
        if bytes.is_empty() {
            return (copied && allow_unterminated && reader.get_ref().limit() > 0).then_some(());
        }
        let count = bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |position| position + 1);
        if output.len().saturating_add(count) > byte_cap {
            return None;
        }
        let newline = bytes[count - 1] == b'\n';
        output.extend_from_slice(&bytes[..count]);
        std::io::BufRead::consume(reader, count);
        copied = true;
        if newline {
            return Some(());
        }
    }
}

#[derive(Clone)]
struct Definition {
    symbol_id: String,
    name: String,
    path: String,
    start_line: usize,
    /// The first decorator line above the definition, when it has decorators.
    decorated_start_line: Option<usize>,
    body_start_line: Option<usize>,
    owner_name: Option<String>,
    owner_kind: Option<String>,
    local: bool,
    /// The lines of the function that holds this function, which alone names it in a language of
    /// [`LOCAL_FUNCTION_LANGUAGES`].
    scope: Option<(usize, usize)>,
    kind: String,
    language: String,
    signature: String,
    owner_signature: String,
    /// The object a JavaScript function is assigned on (`res.redirect = function`).
    class_name: Option<String>,
    top_level: bool,
    /// A call can reach it: false for a field, property, variable, or constant in Go, Java, or
    /// Rust, except a Go one that holds a func.
    callable: bool,
    /// A JavaScript function, variable, or constant that only its own scope names: not exported,
    /// not a member of an object, and never written as `object.name` in its file.
    free: bool,
    /// A top-level JavaScript function, variable, or constant that no object holds: never written
    /// as `object.name` in its file and never placed in an object literal there or in a file that
    /// imports the name. Only a module object can hold it, and no call result is one.
    module_value: bool,
}

struct ShorthandSite {
    target_member: bool,
    shadow_id: Option<String>,
}

impl Definition {
    fn is_js_shorthand_member_target(&self) -> bool {
        language_family(&self.language) == "js"
            && self
                .owner_kind
                .as_deref()
                .is_some_and(|kind| CLASS_LIKE_KINDS.contains(&kind))
            && (matches!(self.kind.as_str(), "field" | "property")
                || self.kind == "method" && self.owner_kind.as_deref() == Some("interface"))
    }

    fn is_value(&self) -> bool {
        matches!(
            self.kind.as_str(),
            "field" | "constant" | "variable" | "property" | "parameter" | "enum_member"
        )
    }

    fn is_python_class_annotation_without_value(&self) -> bool {
        if self.language != "python"
            || self.owner_name.is_none()
            || !matches!(self.kind.as_str(), "field" | "variable" | "constant")
            || arity::default_start(&self.signature).is_some()
        {
            return false;
        }
        let owner_signature = self.owner_signature.as_str();
        let dataclass = owner_signature.split_whitespace().any(|part| {
            let Some(decorator) = part.strip_prefix('@') else {
                return false;
            };
            let name = decorator
                .split_once('(')
                .map_or(decorator, |(name, _)| name);
            name.rsplit('.').next() == Some("dataclass")
        });
        let typed_dict = owner_signature
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .any(|word| word == "TypedDict");
        !dataclass && !typed_dict
    }

    /// The module source and imported name when this is a JavaScript variable that a
    /// destructured dynamic import binds: a binding, not a definition.
    fn dynamic_import(&self, name: &str) -> Option<(String, String)> {
        (self.kind == "variable" && language_family(&self.language) == "js")
            .then(|| js_modules::dynamic_import_binding(&self.signature, name))
            .flatten()
    }

    /// See [`is_property_method`].
    fn is_property_method(&self) -> bool {
        self.kind == "method" && self.top_level && language_family(&self.language) == "js"
    }

    fn display(&self) -> String {
        match &self.owner_name {
            Some(owner) => format!("{owner}.{}", self.name),
            None => self.name.clone(),
        }
    }

    fn resolves_to(&self) -> String {
        format!(
            "resolves to `{}` ({}:{})",
            self.display(),
            self.path,
            self.start_line
        )
    }
}

#[derive(Clone)]
struct Verdict {
    label: SiteLabel,
    basis: String,
    claimed: bool,
}

impl Verdict {
    fn new(label: SiteLabel, basis: impl Into<String>, claimed: bool) -> Self {
        Verdict {
            label,
            basis: basis.into(),
            claimed,
        }
    }
}

struct SiteRow {
    site: ReferenceSite,
    label: SiteLabel,
    basis: String,
    claimed: bool,
    end_line: Option<usize>,
    arg_count: Option<u32>,
}

struct Claim {
    basis: Option<String>,
    typed_receiver: Option<TypedReceiverClaim>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TypedReceiverClaim {
    ReachesTarget,
    DifferentOwner,
    Unknown,
}

impl TypedReceiverClaim {
    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::ReachesTarget, _) | (_, Self::ReachesTarget) => Self::ReachesTarget,
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            _ => Self::DifferentOwner,
        }
    }
}

struct ReceiverChainOwnerEvidence {
    owners: HashMap<String, Option<String>>,
    pending_base_owners: HashMap<String, Option<String>>,
    incomplete: Option<ReceiverChainLimit>,
    edge_rows_examined: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReceiverChainLimit {
    Owners,
    Edges,
    Depth,
}

#[derive(Clone, Copy)]
struct ReceiverChainLimits {
    owners: usize,
    edges: usize,
    depth: usize,
}

impl ReceiverChainLimits {
    const DEFAULT: Self = Self {
        owners: RECEIVER_CHAIN_ANCESTRY_OWNER_CAP,
        edges: RECEIVER_CHAIN_ANCESTRY_EDGE_CAP,
        depth: RECEIVER_CHAIN_ANCESTRY_DEPTH_CAP,
    };
}

fn receiver_chain_limit_note(limit: ReceiverChainLimit) -> &'static str {
    match limit {
        ReceiverChainLimit::Owners => {
            "receiver-chain ancestry exceeded the 8,192-owner limit; affected sites stayed unresolved"
        }
        ReceiverChainLimit::Edges => {
            "receiver-chain ancestry exceeded the 8,192-edge limit; affected sites stayed unresolved"
        }
        ReceiverChainLimit::Depth => {
            "receiver-chain ancestry exceeded the 8-level limit; affected sites stayed unresolved"
        }
    }
}

fn record_receiver_chain_limit_note(
    cannot_see: &mut Vec<String>,
    limit: Option<ReceiverChainLimit>,
) {
    if let Some(limit) = limit {
        let note = receiver_chain_limit_note(limit);
        if !cannot_see.iter().any(|existing| existing == note) {
            cannot_see.push(note.to_string());
        }
    }
}

struct Judged {
    claimed: bool,
    basis: Option<String>,
}

struct TextOutcome {
    counts: ReferenceCounts,
    text_only: Vec<TextOnlyLines>,
    summary: TextScanSummary,
}

impl TextOutcome {
    fn not_run() -> Self {
        TextOutcome {
            counts: ReferenceCounts::default(),
            text_only: Vec::new(),
            summary: TextScanSummary {
                files_scanned: 0,
                files_total: 0,
                complete: true,
                elapsed_ms: 0,
                skipped: None,
            },
        }
    }

    fn skipped(class: &Symbol) -> Self {
        TextOutcome {
            counts: ReferenceCounts::default(),
            text_only: Vec::new(),
            summary: TextScanSummary {
                files_scanned: 0,
                files_total: 0,
                complete: true,
                elapsed_ms: 0,
                skipped: Some(class.name.clone()),
            },
        }
    }
}

/// A definition that an import reaches: its symbol id, path, and language.
type ImportMatch = (String, String, String);

struct Accounting<'a> {
    conn: &'a Connection,
    root: &'a Path,
    /// The judge stops here, so the cheaper checks of the judged rows still fit in the limit.
    judge_deadline: Instant,
    deadline: Instant,
    judge_plain_stmt: RefCell<Option<Statement<'a>>>,
    judge_chain_stmt: RefCell<Option<Statement<'a>>>,
    receiver_chain_owner_stmt: RefCell<Option<Statement<'a>>>,
    receiver_chain_pending_base_stmt: RefCell<Option<Statement<'a>>>,
    receiver_chain_target_stmt: RefCell<Option<Statement<'a>>>,
    /// The target when the index no longer holds it, and the judge that reads its columns from
    /// the symbol instead of joining its row.
    absent: Option<Symbol>,
    absent_judge_plain_stmt: RefCell<Option<Statement<'a>>>,
    absent_judge_chain_stmt: RefCell<Option<Statement<'a>>>,
    absent_receiver_chain_target_stmt: RefCell<Option<Statement<'a>>>,
    /// The C# `#if` condition of the branch that julie kept the target from.
    target_condition: Option<String>,
    definitions_stmt: RefCell<Option<Statement<'a>>>,
    import_match_stmt: Option<Statement<'a>>,
    statements: RefCell<HashMap<&'static str, Statement<'a>>>,
    external_sources: HashMap<String, bool>,
    msbuild_usings: std::cell::OnceCell<bool>,
    /// Definitions an import reaches, by caller file, receiver, and name.
    import_matches: HashMap<(String, String, String), Vec<ImportMatch>>,
    js: js_modules::JsCaches,
    receivers: receiver_types::ReceiverCaches,
}

impl<'a> Accounting<'a> {
    fn new(conn: &'a Connection, root: &'a Path, time_limit: Duration) -> Self {
        Accounting {
            conn,
            root,
            judge_deadline: Instant::now() + time_limit / 2,
            deadline: Instant::now() + time_limit,
            judge_plain_stmt: RefCell::new(None),
            judge_chain_stmt: RefCell::new(None),
            receiver_chain_owner_stmt: RefCell::new(None),
            receiver_chain_pending_base_stmt: RefCell::new(None),
            receiver_chain_target_stmt: RefCell::new(None),
            absent: None,
            absent_judge_plain_stmt: RefCell::new(None),
            absent_judge_chain_stmt: RefCell::new(None),
            absent_receiver_chain_target_stmt: RefCell::new(None),
            target_condition: None,
            definitions_stmt: RefCell::new(None),
            import_match_stmt: None,
            statements: RefCell::new(HashMap::new()),
            external_sources: HashMap::new(),
            msbuild_usings: std::cell::OnceCell::new(),
            import_matches: HashMap::new(),
            js: js_modules::JsCaches::default(),
            receivers: receiver_types::ReceiverCaches::default(),
        }
    }

    fn out_of_time(&self) -> bool {
        Instant::now() >= self.deadline
    }

    /// Starts a new time limit and keeps every prepared statement and cache. The judge statement
    /// is large, so a later phase of the report reuses it instead of preparing its own.
    fn restart_clock(&mut self, time_limit: Duration) {
        self.judge_deadline = Instant::now() + time_limit / 2;
        self.deadline = Instant::now() + time_limit;
    }

    /// Runs `run` on the statement for `sql`, preparing it on first use: preparing the larger
    /// queries costs more than running them once per row.
    fn with_statement<T>(
        &self,
        sql: &'static str,
        run: impl FnOnce(&mut Statement<'a>) -> rusqlite::Result<T>,
    ) -> Result<T, QueryError> {
        let mut statements = self.statements.borrow_mut();
        if !statements.contains_key(sql) {
            statements.insert(sql, self.conn.prepare(sql)?);
        }
        let stmt = statements
            .get_mut(sql)
            .expect("statement was just prepared");
        Ok(run(stmt)?)
    }

    fn receiver_chain_owner_evidence(
        &self,
        pending_ids: &[String],
    ) -> Result<ReceiverChainOwnerEvidence, QueryError> {
        let mut evidence = ReceiverChainOwnerEvidence {
            owners: HashMap::new(),
            pending_base_owners: HashMap::new(),
            incomplete: None,
            edge_rows_examined: 0,
        };
        if pending_ids.is_empty() || !crate::queries::has_typed_receiver_chain_support(self.conn) {
            return Ok(evidence);
        }
        let pending_json = serde_json::to_string(pending_ids).unwrap_or_else(|_| "[]".to_string());
        let mut owner_slot = self.receiver_chain_owner_stmt.borrow_mut();
        if owner_slot.is_none() {
            let sql = crate::queries::typed_receiver_chain_owner_rows_sql();
            *owner_slot = Some(self.conn.prepare(&sql)?);
        }
        let owner_rows = owner_slot
            .as_mut()
            .expect("receiver-chain owner query was just prepared")
            .query_map(params![pending_json], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for (row_key, owner_id) in owner_rows {
            evidence.owners.insert(row_key, owner_id);
        }
        self.extend_receiver_chain_ancestry(&mut evidence, ReceiverChainLimits::DEFAULT)?;
        Ok(evidence)
    }

    fn extend_receiver_chain_ancestry(
        &self,
        evidence: &mut ReceiverChainOwnerEvidence,
        limits: ReceiverChainLimits,
    ) -> Result<(), QueryError> {
        let mut frontier: HashSet<String> = evidence.owners.values().flatten().cloned().collect();
        if frontier.len() > limits.owners {
            evidence.incomplete = Some(ReceiverChainLimit::Owners);
            return Ok(());
        }
        if frontier.is_empty() {
            return Ok(());
        }
        let mut visited = HashSet::new();
        for _ in 0..limits.depth {
            if frontier.is_empty() {
                break;
            }
            if visited.len() + frontier.len() > limits.owners {
                evidence.incomplete = Some(ReceiverChainLimit::Owners);
                return Ok(());
            }
            let frontier_ids: Vec<&str> = frontier.iter().map(String::as_str).collect();
            let frontier_json =
                serde_json::to_string(&frontier_ids).unwrap_or_else(|_| "[]".to_string());
            let remaining_edges = limits.edges.saturating_sub(evidence.edge_rows_examined);
            let resolved_limit = remaining_edges.saturating_add(1);
            let resolved_bases = self.with_statement(
                crate::queries::typed_receiver_chain_resolved_base_rows_sql(),
                |stmt| {
                    stmt.query_map(params![&frontier_json, resolved_limit as i64], |row| {
                        row.get::<_, String>(0)
                    })?
                    .collect::<Result<Vec<_>, _>>()
                },
            )?;
            evidence.edge_rows_examined += resolved_bases.len();
            if resolved_bases.len() > remaining_edges {
                evidence.incomplete = Some(ReceiverChainLimit::Edges);
                return Ok(());
            }
            let mut pending_base_slot = self.receiver_chain_pending_base_stmt.borrow_mut();
            if pending_base_slot.is_none() {
                let sql = crate::queries::typed_receiver_chain_pending_base_owner_rows_sql();
                *pending_base_slot = Some(self.conn.prepare(&sql)?);
            }
            let remaining_pending_edges = limits.edges.saturating_sub(evidence.edge_rows_examined);
            let pending_bases = pending_base_slot
                .as_mut()
                .expect("pending-base owner query was just prepared")
                .query_map(
                    params![
                        &frontier_json,
                        remaining_pending_edges.saturating_add(1) as i64
                    ],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
                )?
                .collect::<Result<Vec<_>, _>>()?;
            evidence.edge_rows_examined += pending_bases.len();
            if pending_bases.len() > remaining_pending_edges {
                evidence.incomplete = Some(ReceiverChainLimit::Edges);
                return Ok(());
            }

            visited.extend(frontier.iter().cloned());
            let mut next = HashSet::new();
            for owner_id in resolved_bases {
                if !visited.contains(&owner_id) {
                    next.insert(owner_id);
                    if visited.len() + next.len() > limits.owners {
                        evidence.incomplete = Some(ReceiverChainLimit::Owners);
                        return Ok(());
                    }
                }
            }
            for (pending_row_id, owner_id) in pending_bases {
                if let Some(owner_id) = owner_id.as_ref() {
                    if !visited.contains(owner_id) {
                        next.insert(owner_id.clone());
                        if visited.len() + next.len() > limits.owners {
                            evidence.incomplete = Some(ReceiverChainLimit::Owners);
                            return Ok(());
                        }
                    }
                }
                evidence
                    .pending_base_owners
                    .insert(pending_row_id, owner_id);
            }
            frontier = next;
        }
        if !frontier.is_empty() {
            let frontier_ids: Vec<&str> = frontier.iter().map(String::as_str).collect();
            let frontier_json =
                serde_json::to_string(&frontier_ids).unwrap_or_else(|_| "[]".to_string());
            let has_more = self.with_statement(
                crate::queries::typed_receiver_chain_has_more_ancestors_sql(),
                |stmt| stmt.query_row(params![frontier_json], |row| row.get::<_, bool>(0)),
            )?;
            if has_more {
                evidence.incomplete = Some(ReceiverChainLimit::Depth);
            }
        }
        Ok(())
    }

    fn receiver_chain_target_statuses(
        &self,
        target_id: &str,
        evidence: &ReceiverChainOwnerEvidence,
    ) -> Result<HashMap<String, Option<i64>>, QueryError> {
        if evidence.owners.is_empty() {
            return Ok(HashMap::new());
        }
        if evidence.incomplete.is_some() {
            return Ok(evidence
                .owners
                .keys()
                .map(|row_key| (row_key.clone(), None))
                .collect());
        }
        let owner_json =
            serde_json::to_string(&evidence.owners).unwrap_or_else(|_| "{}".to_string());
        let pending_base_json = serde_json::to_string(&evidence.pending_base_owners)
            .unwrap_or_else(|_| "{}".to_string());
        let absent = self
            .absent
            .as_ref()
            .filter(|symbol| symbol.symbol_id == target_id);
        let mut slot = if absent.is_some() {
            self.absent_receiver_chain_target_stmt.borrow_mut()
        } else {
            self.receiver_chain_target_stmt.borrow_mut()
        };
        if slot.is_none() {
            let sql = receiver_chain_statuses_sql(absent.is_some());
            *slot = Some(self.conn.prepare(&sql)?);
        }
        let stmt = slot
            .as_mut()
            .expect("receiver-chain status query was just prepared");
        let mut statuses = HashMap::new();
        if let Some(symbol) = absent {
            let mut rows = stmt.query(params![
                owner_json,
                pending_base_json,
                symbol.symbol_id,
                symbol.name,
                symbol.kind,
                symbol.language,
                symbol.path,
                symbol.start_line as i64,
                symbol.parent_symbol_id
            ])?;
            while let Some(row) = rows.next()? {
                statuses.insert(row.get(0)?, row.get(1)?);
            }
        } else {
            let mut rows = stmt.query(params![owner_json, pending_base_json, target_id])?;
            while let Some(row) = rows.next()? {
                statuses.insert(row.get(0)?, row.get(1)?);
            }
        }
        Ok(statuses)
    }

    /// Runs `sql` with `path` as `?1` and `others` after it, once for each spelling of the path
    /// the index can hold (`/` and `\`), and reads every row.
    fn rows_at_path<T>(
        &self,
        sql: &'static str,
        path: &str,
        others: &[&dyn ToSql],
        read: fn(&Row) -> rusqlite::Result<T>,
    ) -> Result<Vec<T>, QueryError> {
        let windows = path.replace('/', "\\");
        let mut spellings = vec![path];
        if windows != path {
            spellings.push(&windows);
        }
        let mut rows = Vec::new();
        for spelling in spellings {
            let bound = std::iter::once(&spelling as &dyn ToSql).chain(others.iter().copied());
            rows.extend(self.with_statement(sql, |stmt| {
                stmt.query_map(params_from_iter(bound), read)?
                    .collect::<Result<Vec<_>, _>>()
            })?);
        }
        Ok(rows)
    }

    fn definitions_sql(&self) -> String {
        let decorated_start = if has_column(self.conn, "structural_facts", "end_byte")
            && has_column(self.conn, "symbols", "end_byte")
        {
            "(SELECT MIN(f.start_line) FROM structural_facts f
              WHERE f.path = s.path AND f.pattern_id LIKE '%.decorated_definition.%'
                AND f.end_byte = s.end_byte AND f.start_byte < s.start_byte)"
        } else {
            "NULL"
        };
        format!(
            "SELECT s.symbol_id, s.name, s.kind, s.language, s.path, s.start_line,
                    s.body_start_line, s.parent_symbol_id, par.name, par.kind, {decorated_start},
                    COALESCE(s.signature, ''), s.visibility,
                    CASE WHEN json_valid(s.metadata_json) THEN
                        json_extract(s.metadata_json, '$.className') END,
                    CASE WHEN json_valid(s.metadata_json) THEN
                        json_extract(s.metadata_json, '$.isCommonJSExport') END,
                    COALESCE(par.signature, ''), par.start_line, par.end_line
             FROM symbols s LEFT JOIN symbols par ON par.symbol_id = s.parent_symbol_id
             WHERE s.name = ?1 AND s.kind NOT IN ('import', 'export')
               AND s.language NOT IN ({})",
            documentation_language_list()
        )
    }

    fn definitions(&self, name: &str) -> Result<Vec<Definition>, QueryError> {
        let mut slot = self.definitions_stmt.borrow_mut();
        if slot.is_none() {
            let sql = self.definitions_sql();
            *slot = Some(self.conn.prepare(&sql)?);
        }
        let stmt = slot
            .as_mut()
            .expect("definitions statement was just prepared");
        let rows = stmt.query_map(params![name], |row| {
            let signature: String = row.get(11)?;
            if is_test_label(&signature) {
                return Ok(None);
            }
            let kind: String = row.get(2)?;
            let language: String = row.get(3)?;
            let parent_id: Option<String> = row.get(7)?;
            let parent_kind: Option<String> = row.get(9)?;
            let parent_name: Option<String> = row.get(8)?;
            let owner_signature: String = row.get(15)?;
            let class_name: Option<String> = row.get(13)?;
            let in_function = parent_kind
                .as_deref()
                .is_some_and(|k| matches!(k, "function" | "method" | "constructor"));
            let local = kind == "parameter" || kind == "variable" && in_function;
            let scope = match (
                row.get::<_, Option<i64>>(16)?,
                row.get::<_, Option<i64>>(17)?,
            ) {
                (Some(start), Some(end))
                    if in_function
                        && kind == "function"
                        && LOCAL_FUNCTION_LANGUAGES.contains(&language.as_str()) =>
                {
                    Some((start as usize, end as usize))
                }
                _ => None,
            };
            let exported = row.get::<_, Option<String>>(12)?.as_deref() == Some("public")
                || row.get::<_, Option<i64>>(14)? == Some(1);
            let module_value = language_family(&language) == "js"
                && matches!(kind.as_str(), "function" | "variable" | "constant")
                && class_name.is_none()
                && parent_id.is_none();
            let free = language_family(&language) == "js"
                && matches!(kind.as_str(), "function" | "variable" | "constant")
                && class_name.is_none()
                && !exported
                && !local
                && (parent_id.is_none() || in_function);
            let record_component = language == "java"
                && kind == "property"
                && is_record(&owner_signature)
                && !signature.split_whitespace().any(|word| word == "static");
            let function_typed = language == "rust"
                && matches!(kind.as_str(), "variable" | "constant")
                && FN_TYPES.iter().any(|fn_type| signature.contains(fn_type));
            let callable = record_component
                || function_typed
                || !(matches!(language.as_str(), "go" | "java" | "rust")
                    && matches!(
                        kind.as_str(),
                        "field" | "property" | "variable" | "constant"
                    ));
            Ok(Some(Definition {
                symbol_id: row.get(0)?,
                name: row.get(1)?,
                path: row.get::<_, String>(4)?.replace('\\', "/"),
                start_line: row.get::<_, i64>(5)? as usize,
                decorated_start_line: row.get::<_, Option<i64>>(10)?.map(|v| v as usize),
                body_start_line: row.get::<_, Option<i64>>(6)?.map(|v| v as usize),
                owner_name: parent_name.filter(|_| {
                    parent_kind
                        .as_deref()
                        .is_some_and(|k| CLASS_LIKE_KINDS.contains(&k))
                }),
                owner_kind: parent_kind.filter(|kind| CLASS_LIKE_KINDS.contains(&kind.as_str())),
                local,
                scope,
                top_level: parent_id.is_none(),
                kind,
                language,
                signature,
                owner_signature,
                class_name,
                callable,
                free,
                module_value,
            }))
        })?;
        let mut definitions: Vec<Definition> = rows
            .filter_map(Result::transpose)
            .collect::<Result<_, _>>()?;
        drop(slot);
        let writes =
            inherited_writes_among(self.conn, definitions.iter().map(|d| d.symbol_id.as_str()))?;
        definitions.retain(|d| !writes.contains(&d.symbol_id) && d.dynamic_import(name).is_none());
        let mut probed: HashSet<&str> = definitions
            .iter()
            .filter(|d| d.free || d.module_value)
            .map(|d| d.path.as_str())
            .collect();
        let importers = if definitions.iter().any(|d| d.module_value) {
            self.with_statement(JS_IMPORTER_PATHS_SQL, |stmt| {
                stmt.query_map(params![name], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()
            })?
        } else {
            Vec::new()
        };
        probed.extend(importers.iter().map(String::as_str));
        let member_paths = if probed.is_empty() {
            HashSet::new()
        } else {
            self.paths_using_as_member(name, &probed)?
        };
        let held_by_importer = importers.iter().any(|path| member_paths.contains(path));
        for definition in &mut definitions {
            definition.free &= !member_paths.contains(&definition.path);
            definition.module_value &=
                !held_by_importer && !member_paths.contains(&definition.path);
            if !definition.callable {
                definition.callable = match definition.language.as_str() {
                    "go" => self.holds_func(definition)?,
                    "rust" => self.holds_fn_alias(definition)?,
                    _ => false,
                };
            }
        }
        Ok(definitions)
    }

    /// The files among `paths` that use `name` as a member of an object: written `object.name`,
    /// or placed in an object literal (`{ name }`, `{ key: name }`).
    fn paths_using_as_member(
        &self,
        name: &str,
        paths: &HashSet<&str>,
    ) -> Result<HashSet<String>, QueryError> {
        if !crate::queries::has_table(self.conn, "identifiers") {
            return Ok(HashSet::new());
        }
        let normalized = |path: String| path.replace('\\', "/");
        let mut found: HashSet<String> = self.with_statement(MEMBER_ACCESS_PATHS_SQL, |stmt| {
            stmt.query_map(params![name], |r| r.get::<_, String>(0))?
                .map(|path| path.map(normalized))
                .collect()
        })?;
        let mut sites: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
        let rows = self.with_statement(VARIABLE_REF_SITES_SQL, |stmt| {
            stmt.query_map(params![name], |r| {
                Ok((
                    normalized(r.get::<_, String>(0)?),
                    r.get::<_, i64>(1)? as usize,
                    r.get::<_, i64>(2)? as usize,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()
        })?;
        for (path, line, column) in rows {
            if paths.contains(path.as_str()) && !found.contains(&path) {
                sites.entry(path).or_default().push((line, column));
            }
        }
        for (path, sites) in sites {
            let Ok(text) = std::fs::read_to_string(self.root.join(&path)) else {
                continue;
            };
            let lines: Vec<&str> = text.lines().collect();
            if sites
                .iter()
                .any(|&(line, column)| is_object_value(&lines, line, column, name))
            {
                found.insert(path);
            }
        }
        Ok(found)
    }

    /// Whether a Go field or variable may hold a func: it has no written type, a qualified type
    /// (`http.HandlerFunc`), a func type, or a project type declared as `type X func(...)`.
    fn holds_func(&self, definition: &Definition) -> Result<bool, QueryError> {
        if definition.kind == "constant" {
            return Ok(false);
        }
        let signature = definition.signature.trim_start();
        let signature = ["var ", "const "]
            .iter()
            .find_map(|keyword| signature.strip_prefix(keyword))
            .unwrap_or(signature);
        let declared = signature
            .trim_start()
            .strip_prefix(definition.name.as_str())
            .filter(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
            .map_or("", str::trim_start);
        if declared.is_empty() || declared.starts_with('=') {
            return Ok(true);
        }
        let type_name: String = declared
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if type_name.is_empty() {
            return Ok(false);
        }
        if type_name == "func" || declared[type_name.len()..].starts_with('.') {
            return Ok(true);
        }
        self.declares_function_type(&type_name, "go")
    }

    /// Whether a Rust field or static is declared with a project type alias of a fn type
    /// (`static HOOK: Callback` with `type Callback = fn(i32)`).
    fn holds_fn_alias(&self, definition: &Definition) -> Result<bool, QueryError> {
        let declared = definition
            .signature
            .split_once(definition.name.as_str())
            .and_then(|(_, rest)| rest.trim_start().strip_prefix(':'))
            .and_then(|declared| declared.split(['=', ';']).next())
            .and_then(|declared| declared.rsplit("::").next())
            .map(str::trim);
        match declared {
            Some(type_name) if is_plain_identifier(type_name) => {
                self.declares_function_type(type_name, "rust")
            }
            _ => Ok(false),
        }
    }

    /// Whether a project type named `type_name` is declared as a func (Go) or fn (Rust) type.
    fn declares_function_type(&self, type_name: &str, language: &str) -> Result<bool, QueryError> {
        let signatures = self.with_statement(TYPE_DECLARATIONS_SQL, |stmt| {
            stmt.query_map(params![type_name, language], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })?;
        Ok(signatures
            .iter()
            .any(|signature| is_function_type(signature, type_name, language)))
    }

    fn definition_count(&self, name: &str) -> Result<usize, QueryError> {
        Ok(self.definitions(name)?.iter().filter(|d| !d.local).count())
    }

    fn account_spec(
        &mut self,
        spec: &Spec,
        out: &mut Vec<SiteRow>,
        cannot_see: &mut Vec<String>,
    ) -> Result<(), QueryError> {
        let definitions = self.definitions(&spec.name)?;
        let group_ids: HashSet<&str> = spec.group.iter().map(|s| s.symbol_id.as_str()).collect();
        let target = &spec.group[0];

        let mut rows = self.load_rows(&spec.name, &definitions, spec.scope)?;
        pair_identifiers(&mut rows);
        rows.retain(|row| !row.consumed);
        mark_hidden_receivers(self.root, &mut rows);
        self.mark_result_variables(&mut rows)?;
        js_modules::complete_js_chains(self.root, &mut rows);

        let shorthand_targets: Vec<&Definition> = definitions
            .iter()
            .filter(|definition| group_ids.contains(definition.symbol_id.as_str()))
            .collect();
        let shorthand_member_target = !shorthand_targets.is_empty()
            && shorthand_targets
                .iter()
                .all(|definition| definition.is_js_shorthand_member_target());
        let mut shorthand_shadows = HashMap::new();

        let mut order: Vec<usize> = (0..rows.len()).collect();
        order.sort_by(|&a, &b| {
            let (a, b) = (&rows[a], &rows[b]);
            shared_leading_segments(&b.path, &target.path)
                .cmp(&shared_leading_segments(&a.path, &target.path))
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.line.cmp(&b.line))
        });
        let (claims, judged) = self.claims(spec, &rows, &order, cannot_see)?;
        let mut group = definitions
            .iter()
            .filter(|d| group_ids.contains(d.symbol_id.as_str()))
            .peekable();
        let module_target = group.peek().is_some() && group.all(|d| d.module_value);
        let mut unchecked: Vec<usize> = order[judged..].to_vec();
        let mut verdicts: HashMap<String, Verdict> = HashMap::new();
        let mut undecided: Vec<usize> = Vec::new();
        for (position, &index) in order[..judged].iter().enumerate() {
            if self.out_of_time() {
                unchecked.extend_from_slice(&order[position..judged]);
                break;
            }
            let row = &rows[index];
            let mut shorthand_site = ShorthandSite {
                target_member: shorthand_member_target,
                shadow_id: None,
            };
            match self.decide_cheap(row, spec, target, &group_ids, &claims, &mut shorthand_site)? {
                Some(verdict) => {
                    verdicts.insert(row.key.clone(), verdict);
                }
                None if module_target
                    && row.result_receiver
                    && language_family(&row.language) == "js" =>
                {
                    verdicts.insert(
                        row.key.clone(),
                        Verdict::new(
                            SiteLabel::Excluded,
                            format!(
                                "a call on a call result never reaches the top-level `{}`",
                                spec.name
                            ),
                            false,
                        ),
                    );
                }
                None => undecided.push(index),
            }
            if let Some(shadow) = shorthand_site.shadow_id {
                shorthand_shadows.insert(row.key.clone(), shadow);
            }
        }

        let mut others: Vec<&Definition> = definitions
            .iter()
            .filter(|d| !d.local && !group_ids.contains(d.symbol_id.as_str()))
            .filter(|d| !(spec.scope == RowScope::Inheritance && d.kind == "constructor"))
            .collect();
        others.sort_by_key(|d| {
            (
                std::cmp::Reverse(shared_leading_segments(&d.path, &target.path)),
                d.path.clone(),
                d.start_line,
            )
        });
        let mut checked = 0;
        for other in &others {
            if undecided.is_empty() || checked >= OTHER_DEFINITION_CAP || self.out_of_time() {
                break;
            }
            let mut kept = Vec::with_capacity(undecided.len());
            let mut complete = true;
            for chunk in undecided.chunks(JUDGE_CHUNK) {
                if self.out_of_time() {
                    complete = false;
                    kept.extend_from_slice(chunk);
                    continue;
                }
                let chunk_rows: Vec<&NameRow> = chunk.iter().map(|&index| &rows[index]).collect();
                let pending_ids: Vec<String> = chunk_rows
                    .iter()
                    .filter(|row| row.typed_receiver_chain.is_some())
                    .filter_map(|row| row.key.strip_prefix("P:").map(str::to_string))
                    .collect();
                let receiver_chain_evidence = self.receiver_chain_owner_evidence(&pending_ids)?;
                record_receiver_chain_limit_note(cannot_see, receiver_chain_evidence.incomplete);
                let receiver_chain_statuses = self
                    .receiver_chain_target_statuses(&other.symbol_id, &receiver_chain_evidence)?;
                let judged = self.judge(
                    &spec.name,
                    &other.symbol_id,
                    &chunk_rows,
                    &receiver_chain_statuses,
                )?;
                for &index in chunk {
                    let row = &rows[index];
                    if row.hidden_receiver
                        || !row.is_in_scope_of(other)
                        || row.is_type_position() && other.is_value()
                        || row.language == "java" && !row.can_reach(other)
                        || row.reads_a_module(&other.kind)
                        || row.is_bare_use() && other.is_property_method()
                        || row.language == "python"
                            && other.is_python_class_annotation_without_value()
                    {
                        kept.push(index);
                        continue;
                    }
                    let Some(Judged {
                        claimed: true,
                        basis: Some(basis),
                    }) = judged.get(&row.key)
                    else {
                        kept.push(index);
                        continue;
                    };
                    if basis == IMPORT_BASIS
                        && row.language == "python"
                        && !self.python_import_reaches(row, &spec.name, &other.path)?
                    {
                        kept.push(index);
                        continue;
                    }
                    verdicts.insert(
                        row.key.clone(),
                        Verdict::new(SiteLabel::Excluded, other.resolves_to(), false),
                    );
                }
            }
            undecided = kept;
            if !complete {
                break;
            }
            checked += 1;
        }
        let unchecked_others = others.len() - checked;
        if !undecided.is_empty() && unchecked_others > 0 {
            cannot_see.push(if unchecked_others == 1 {
                "1 other definition with this name was not checked".to_string()
            } else {
                format!("{unchecked_others} other definitions with this name were not checked")
            });
        }

        let absent_definition = match self
            .absent
            .as_ref()
            .filter(|absent| group_ids.contains(absent.symbol_id.as_str()))
        {
            Some(absent) => Some(self.absent_definition(absent)?),
            None => None,
        };
        let group_definitions: Vec<&Definition> = definitions
            .iter()
            .filter(|d| group_ids.contains(d.symbol_id.as_str()))
            .chain(absent_definition.iter())
            .collect();
        for index in undecided {
            let row = &rows[index];
            let rivals = self.rivals(row, &others)?;
            let verdict = match claims.get(&row.key) {
                Some(claim) => claimed_verdict(row, claim, rivals.len(), spec.builds_class),
                None if self.out_of_time() => {
                    unchecked.push(index);
                    continue;
                }
                None => self.decide_late(
                    row,
                    &spec.group,
                    &group_definitions,
                    &rivals,
                    shorthand_shadows.contains_key(&row.key),
                )?,
            };
            verdicts.insert(row.key.clone(), verdict);
        }
        for index in unchecked {
            let rivals = self.rivals(&rows[index], &others)?.len();
            let verdict = if rivals == 0 {
                Verdict::new(
                    SiteLabel::Likely,
                    format!("name only; {NOT_CHECKED}"),
                    false,
                )
            } else {
                Verdict::new(
                    SiteLabel::Unresolved,
                    format!("{} definitions; {NOT_CHECKED}", rivals + 1),
                    false,
                )
            };
            verdicts.insert(rows[index].key.clone(), verdict);
        }

        for row in &rows {
            let Some(mut verdict) = verdicts.remove(&row.key) else {
                continue;
            };
            if matches!(verdict.label, SiteLabel::Likely | SiteLabel::Unresolved)
                && let Some(arity) = &spec.arity
                && let Some(count) = row.arg_count
                && let Some(basis) = arity.exclusion_basis(&spec.name, &row.language, count)
            {
                verdict = Verdict::new(SiteLabel::Excluded, basis, false);
            }
            if verdict.label == SiteLabel::Proven
                && NAME_RESOLVED_BASES.contains(&verdict.basis.as_str())
                && let Some(line) = self.js_binding_line(row, &spec.name)?
            {
                verdict.label = SiteLabel::Likely;
                verdict.basis = format!(
                    "{}; line {line} binds the name as a callback parameter, loop variable, or catch parameter",
                    verdict.basis
                );
            }
            out.push(site_row(row, verdict, spec));
        }
        if spec.scope == RowScope::All {
            for member in &spec.group {
                for grouped in
                    grouped_member_access_rows(self.conn, &member.symbol_id, RESOLVER_ROW_CAP)?
                {
                    out.push(SiteRow {
                        end_line: None,
                        arg_count: None,
                        label: SiteLabel::Proven,
                        basis: "qualified path".to_string(),
                        claimed: true,
                        site: grouped,
                    });
                }
            }
        }
        Ok(())
    }

    /// The label and basis the report gives each pending `extends` or `implements` row that names
    /// `owner`, by the id of the type that holds the row; the best label wins when a type has
    /// several rows.
    fn inheritance_labels(
        &mut self,
        owner: &Symbol,
    ) -> Result<HashMap<String, (SiteLabel, String)>, QueryError> {
        let spec = Spec {
            name: owner.name.clone(),
            group: vec![owner.clone()],
            scope: RowScope::Inheritance,
            builds_class: false,
            arity: None,
            local_to: None,
        };
        let mut rows = Vec::new();
        self.account_spec(&spec, &mut rows, &mut Vec::new())?;
        let mut labels: HashMap<String, (SiteLabel, String)> = HashMap::new();
        for row in rows {
            let better = labels
                .get(&row.site.from_symbol_id)
                .is_none_or(|(label, _)| row.label.rank() < label.rank());
            if better {
                labels.insert(row.site.from_symbol_id, (row.label, row.basis));
            }
        }
        Ok(labels)
    }

    fn load_rows(
        &self,
        name: &str,
        definitions: &[Definition],
        scope: RowScope,
    ) -> Result<Vec<NameRow>, QueryError> {
        let docs = documentation_language_list();
        let inheritance = scope == RowScope::Inheritance;
        let kinds = if inheritance {
            "AND p.kind IN ('extends', 'implements')"
        } else {
            ""
        };
        let mut rows = Vec::new();

        let pending_sql = format!(
            "SELECT p.rowid, p.from_symbol_id, COALESCE(s.name, ''), p.kind, p.path,
                    COALESCE(f.language, s.language, ''), p.start_line, p.start_column,
                    p.end_line, p.target_receiver, p.target_namespace_json, s.end_line
             FROM pending_relationships p
             LEFT JOIN symbols s ON s.symbol_id = p.from_symbol_id
             LEFT JOIN files f ON f.file_id = p.file_id
             WHERE p.target_terminal_name = ?1 AND p.kind != 'imports' {kinds}
               AND COALESCE(f.language, s.language, '') NOT IN ({docs})"
        );
        let mut stmt = self.conn.prepare(&pending_sql)?;
        let mut query = stmt.query(params![name])?;
        while let Some(row) = query.next()? {
            let namespace: Vec<String> = row
                .get::<_, Option<String>>(10)?
                .and_then(|json| serde_json::from_str(&json).ok())
                .unwrap_or_default();
            let receiver: Option<String> = row.get(9)?;
            let chain: Vec<String> = namespace
                .iter()
                .chain(receiver.as_ref().filter(|r| !r.is_empty()))
                .cloned()
                .collect();
            let typed_receiver_chain = (chain.len() > 1
                && receiver
                    .as_deref()
                    .is_some_and(|receiver| !receiver.is_empty()))
            .then(|| chain.clone());
            rows.push(NameRow {
                key: format!("P:{}", row.get::<_, i64>(0)?),
                source: Source::Pending,
                caller_id: row.get(1)?,
                caller_name: row.get(2)?,
                caller_end_line: row.get::<_, Option<i64>>(11)?.map(|v| v as usize),
                path: row.get::<_, String>(4)?.replace('\\', "/"),
                language: row.get(5)?,
                kind: row.get(3)?,
                line: row.get::<_, i64>(6)? as usize,
                column: row.get::<_, Option<i64>>(7)?.map(|v| v as usize),
                end_line: row.get::<_, Option<i64>>(8)?.map(|v| v as usize),
                receiver,
                namespace,
                to_symbol_id: None,
                site_line: row.get::<_, i64>(6)? as usize,
                site_column: row.get::<_, Option<i64>>(7)?.map(|v| v as usize),
                consumed: false,
                paired: false,
                hidden_receiver: false,
                result_receiver: false,
                chain: Some(chain),
                typed_receiver_chain,
                arg_count: None,
                links_enclosing_class: false,
            });
        }

        if !inheritance {
            let relationship_sql = format!(
                "SELECT r.rowid, r.from_symbol_id, COALESCE(s.name, ''), r.kind, r.path,
                    COALESCE(f.language, s.language, ''), r.start_line, r.start_column,
                    r.end_line, r.to_symbol_id, {LINKS_ENCLOSING_CLASS_SQL}, s.end_line
             FROM relationships r
             JOIN symbols t ON t.symbol_id = r.to_symbol_id
             LEFT JOIN symbols s ON s.symbol_id = r.from_symbol_id
             LEFT JOIN files f ON f.file_id = r.file_id
             WHERE t.name = ?1 AND r.kind != 'imports'
               AND (
                    COALESCE(f.language, s.language, '') NOT IN ({docs})
                    OR r.kind IN ('calls', 'call', 'instantiates')
                    OR COALESCE(f.language, s.language, '') = 'css'
                       AND CASE WHEN json_valid(r.metadata_json)
                                THEN json_extract(r.metadata_json, '$.referenceType') END
                           IN ('custom-property', 'composes')
               )"
            );
            let mut stmt = self.conn.prepare(&relationship_sql)?;
            let mut query = stmt.query(params![name])?;
            while let Some(row) = query.next()? {
                let line = row.get::<_, Option<i64>>(6)?.unwrap_or(0) as usize;
                rows.push(NameRow {
                    key: format!("R:{}", row.get::<_, i64>(0)?),
                    source: Source::Relationship,
                    caller_id: row.get(1)?,
                    caller_name: row.get(2)?,
                    caller_end_line: row.get::<_, Option<i64>>(11)?.map(|v| v as usize),
                    path: row.get::<_, String>(4)?.replace('\\', "/"),
                    language: row.get(5)?,
                    kind: row.get(3)?,
                    line,
                    column: row.get::<_, Option<i64>>(7)?.map(|v| v as usize),
                    end_line: row.get::<_, Option<i64>>(8)?.map(|v| v as usize),
                    receiver: None,
                    namespace: Vec::new(),
                    to_symbol_id: row.get(9)?,
                    site_line: line,
                    site_column: row.get::<_, Option<i64>>(7)?.map(|v| v as usize),
                    consumed: false,
                    paired: false,
                    hidden_receiver: false,
                    result_receiver: false,
                    chain: None,
                    typed_receiver_chain: None,
                    arg_count: None,
                    links_enclosing_class: row.get(10)?,
                });
            }
        }

        if !inheritance && crate::queries::has_table(self.conn, "identifiers") {
            let identifier_sql = format!(
                "SELECT i.identifier_id, i.containing_symbol_id, COALESCE(s.name, ''), i.kind,
                        i.path, i.language, i.start_line, i.start_column,
                        CASE WHEN json_valid(i.metadata_json)
                             THEN json_extract(i.metadata_json, '$.receiver') END,
                        CASE WHEN json_valid(i.metadata_json)
                             THEN json_extract(i.metadata_json, '$.receiver_qualifier') END,
                        CASE WHEN json_valid(i.metadata_json)
                             THEN json_extract(i.metadata_json, '$.arg_count') END,
                        CASE WHEN json_valid(i.metadata_json)
                             THEN json_extract(i.metadata_json, '$.arg_spread') END IS NOT NULL
                 FROM identifiers i
                 LEFT JOIN symbols s ON s.symbol_id = i.containing_symbol_id
                 WHERE i.name = ?1 AND COALESCE(s.kind, '') != 'import'
                   AND (i.language NOT IN ({docs})
                        OR i.kind IN ('call', 'calls', 'instantiates'))"
            );
            let mut stmt = self.conn.prepare(&identifier_sql)?;
            let mut query = stmt.query(params![name])?;
            while let Some(row) = query.next()? {
                let line = row.get::<_, i64>(6)? as usize;
                let kind: String = row.get(3)?;
                let receiver: Option<String> = row.get(8)?;
                let qualifier: Option<String> = row.get(9)?;
                let arg_count: Option<u32> = row.get(10)?;
                let arg_spread: bool = row.get(11)?;
                let written = receiver.as_deref().filter(|r| !r.is_empty());
                let chain = match kind.as_str() {
                    "type_usage" => None,
                    "member_access" if written.is_none() => None,
                    _ => Some(
                        qualifier
                            .iter()
                            .flat_map(|q| q.split('.'))
                            .chain(written)
                            .map(str::to_string)
                            .collect(),
                    ),
                };
                rows.push(NameRow {
                    key: format!("I:{}", row.get::<_, String>(0)?),
                    source: Source::Identifier,
                    caller_id: row.get(1)?,
                    caller_name: row.get(2)?,
                    caller_end_line: None,
                    path: row.get::<_, String>(4)?.replace('\\', "/"),
                    language: row.get(5)?,
                    kind,
                    line,
                    column: row.get::<_, Option<i64>>(7)?.map(|v| v as usize),
                    end_line: None,
                    receiver,
                    namespace: Vec::new(),
                    to_symbol_id: None,
                    site_line: line,
                    site_column: row.get::<_, Option<i64>>(7)?.map(|v| v as usize),
                    consumed: false,
                    paired: false,
                    hidden_receiver: false,
                    result_receiver: false,
                    chain,
                    typed_receiver_chain: None,
                    arg_count: arg_count.filter(|_| !arg_spread),
                    links_enclosing_class: false,
                });
            }
        }

        let import_spans = self.import_spans(name)?;
        let definition_spans = definition_spans(definitions, None);
        rows.retain(|row| {
            if scope == RowScope::Calls && !row.is_call() {
                return false;
            }
            if in_spans(&import_spans, &row.path, row.line) {
                return false;
            }
            !(row.source == Source::Identifier
                && !row.is_super_call()
                && in_spans(&definition_spans, &row.path, row.line))
        });
        Ok(rows)
    }

    fn import_spans(&self, name: &str) -> Result<HashMap<String, Vec<(usize, usize)>>, QueryError> {
        let mut spans: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
        let mut add = |path: String, start: i64, end: Option<i64>| {
            let start = start as usize;
            let end = end.map_or(start, |e| (e as usize).max(start));
            spans
                .entry(path.replace('\\', "/"))
                .or_default()
                .push((start, end));
        };
        let docs = documentation_language_list();
        // A full-path import row (`com.squareup.moshi.Json`) ends in `.name`; the trigram
        // index finds those rows without a scan of every import row.
        let dotted = format!("\".{}\"", name.replace('"', "\"\""));
        let sql = if crate::queries::has_table(self.conn, "symbol_names_tri")
            && name.chars().count() >= 2
        {
            format!(
                "SELECT path, start_line, end_line FROM symbols
                 WHERE kind = 'import' AND language NOT IN ({docs}) AND name = ?1
                 UNION ALL
                 SELECT s.path, s.start_line, s.end_line FROM symbol_names_tri
                 CROSS JOIN symbols s ON s.rowid = symbol_names_tri.rowid
                 WHERE symbol_names_tri MATCH ?2 AND s.kind = 'import'
                   AND s.language NOT IN ({docs})
                   AND substr(s.name, -length(?1) - 1) = '.' || ?1"
            )
        } else {
            format!(
                "SELECT path, start_line, end_line FROM symbols
                 WHERE kind = 'import' AND language NOT IN ({docs})
                   AND (name = ?1 OR substr(name, -length(?1) - 1) = '.' || ?1)
                   AND ?2 = ?2"
            )
        };
        let mut stmt = self.conn.prepare(&sql)?;
        for row in stmt.query_map(params![name, dotted], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })? {
            let (path, start, end) = row?;
            add(path, start, end);
        }
        let mut stmt = self.conn.prepare(
            "SELECT path, start_line, end_line FROM pending_relationships
             WHERE target_terminal_name = ?1 AND kind = 'imports'",
        )?;
        for row in stmt.query_map(params![name], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })? {
            let (path, start, end) = row?;
            add(path, start, end);
        }
        let mut stmt = self.conn.prepare(
            "SELECT r.path, r.start_line, r.end_line FROM relationships r
             JOIN symbols t ON t.symbol_id = r.to_symbol_id
             WHERE t.name = ?1 AND r.kind = 'imports' AND r.start_line IS NOT NULL",
        )?;
        for row in stmt.query_map(params![name], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })? {
            let (path, start, end) = row?;
            add(path, start, end);
        }
        let mut stmt = self.conn.prepare(concat!(
            "SELECT path, start_line, end_line, COALESCE(signature, '') FROM symbols
             WHERE name = ?1 AND kind = 'variable'
               AND language IN (",
            js_family_sql!(),
            ")"
        ))?;
        for row in stmt.query_map(params![name], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, String>(3)?,
            ))
        })? {
            let (path, start, end, signature) = row?;
            if js_modules::dynamic_import_binding(&signature, name).is_some() {
                add(path, start, end);
            }
        }
        Ok(spans)
    }

    /// Judges `rows` (pending and identifier rows; relationship rows need no judge) for `target_id`.
    fn judge(
        &self,
        name: &str,
        target_id: &str,
        rows: &[&NameRow],
        receiver_chain_statuses: &HashMap<String, Option<i64>>,
    ) -> Result<HashMap<String, Judged>, QueryError> {
        let ids_of = |source: Source| -> String {
            let ids: Vec<&str> = rows
                .iter()
                .filter(|row| row.source == source)
                .filter_map(|row| row.key.split_once(':').map(|(_, id)| id))
                .collect();
            serde_json::to_string(&ids).unwrap_or_else(|_| "[]".to_string())
        };
        let (pending, identifiers) = (ids_of(Source::Pending), ids_of(Source::Identifier));
        let absent = self
            .absent
            .as_ref()
            .filter(|symbol| symbol.symbol_id == target_id);
        let receiver_chain = rows.iter().any(|row| row.typed_receiver_chain.is_some());
        let typed_receiver_chain =
            receiver_chain && crate::queries::has_typed_receiver_chain_support(self.conn);
        let chain_status_json =
            serde_json::to_string(receiver_chain_statuses).unwrap_or_else(|_| "{}".to_string());
        let mut slot = if absent.is_some() {
            if receiver_chain {
                self.absent_judge_chain_stmt.borrow_mut()
            } else {
                self.absent_judge_plain_stmt.borrow_mut()
            }
        } else if receiver_chain {
            self.judge_chain_stmt.borrow_mut()
        } else {
            self.judge_plain_stmt.borrow_mut()
        };
        if slot.is_none() {
            let sql = judge_sql(self.conn, absent.is_some(), receiver_chain);
            *slot = Some(self.conn.prepare(&sql)?);
        }
        let stmt = slot.as_mut().expect("judge statement was just prepared");
        let mut rows = match (absent, typed_receiver_chain) {
            (Some(symbol), true) => stmt.query(params![
                name,
                target_id,
                pending,
                identifiers,
                symbol.name,
                symbol.kind,
                symbol.language,
                symbol.path,
                symbol.start_line as i64,
                symbol.parent_symbol_id,
                chain_status_json
            ])?,
            (Some(symbol), false) => stmt.query(params![
                name,
                target_id,
                pending,
                identifiers,
                symbol.name,
                symbol.kind,
                symbol.language,
                symbol.path,
                symbol.start_line as i64,
                symbol.parent_symbol_id
            ])?,
            (None, true) => stmt.query(params![
                name,
                target_id,
                pending,
                identifiers,
                chain_status_json
            ])?,
            (None, false) => stmt.query(params![name, target_id, pending, identifiers])?,
        };
        let mut judged = HashMap::new();
        while let Some(row) = rows.next()? {
            let source: String = row.get(0)?;
            let id: String = row.get(1)?;
            judged.insert(
                format!("{source}:{id}"),
                Judged {
                    claimed: row.get::<_, i64>(2)? != 0,
                    basis: row.get(3)?,
                },
            );
        }
        Ok(judged)
    }

    /// The claims of the target group on the rows of `order`, judged in chunks until the time
    /// limit, and how many rows at the start of `order` were judged.
    fn claims(
        &self,
        spec: &Spec,
        rows: &[NameRow],
        order: &[usize],
        cannot_see: &mut Vec<String>,
    ) -> Result<(HashMap<String, Claim>, usize), QueryError> {
        let mut claims: HashMap<String, Claim> = HashMap::new();
        let mut resolvers = Vec::with_capacity(spec.group.len());
        let needs_resolver = order.iter().any(|&index| {
            let row = &rows[index];
            !row.hidden_receiver
                && row.source == Source::Identifier
                && matches!(row.kind.as_str(), "type_usage" | "member_access")
        });
        for member in &spec.group {
            let resolver_ids: HashSet<String> =
                if spec.scope == RowScope::Inheritance || !needs_resolver {
                    HashSet::new()
                } else {
                    let resolver_rows = direct_identifier_rows(
                        self.conn,
                        &spec.name,
                        Some(&member.symbol_id),
                        RESOLVER_ROW_CAP,
                    )?;
                    resolver_rows
                        .into_iter()
                        .map(|row| format!("I:{}", row.identifier_id))
                        .collect::<HashSet<_>>()
                };
            if resolver_ids.len() >= RESOLVER_ROW_CAP {
                cannot_see.push(format!(
                    "the resolver listed {RESOLVER_ROW_CAP} identifier rows and may hold more; rows past that limit have no resolver evidence"
                ));
            }
            resolvers.push(resolver_ids);
        }
        let mut judged_rows = 0;
        let typed_receiver_chain_supported =
            crate::queries::has_typed_receiver_chain_support(self.conn);
        for chunk in order.chunks(JUDGE_CHUNK) {
            if Instant::now() >= self.judge_deadline {
                break;
            }
            let chunk_rows: Vec<&NameRow> = chunk
                .iter()
                .map(|&index| &rows[index])
                .filter(|row| !row.hidden_receiver)
                .collect();
            let pending_ids: Vec<String> = chunk_rows
                .iter()
                .filter(|row| row.typed_receiver_chain.is_some())
                .filter_map(|row| row.key.strip_prefix("P:").map(str::to_string))
                .collect();
            let receiver_chain_evidence = self.receiver_chain_owner_evidence(&pending_ids)?;
            record_receiver_chain_limit_note(cannot_see, receiver_chain_evidence.incomplete);
            for (member, resolver_ids) in spec.group.iter().zip(&resolvers) {
                let receiver_chain_statuses = self
                    .receiver_chain_target_statuses(&member.symbol_id, &receiver_chain_evidence)?;
                let judged = self.judge(
                    &spec.name,
                    &member.symbol_id,
                    &chunk_rows,
                    &receiver_chain_statuses,
                )?;
                for row in chunk_rows.iter().copied() {
                    let judged_row = judged.get(&row.key);
                    let typed_receiver =
                        if row.typed_receiver_chain.is_some() && typed_receiver_chain_supported {
                            Some(
                                match receiver_chain_statuses.get(&row.key).copied().flatten() {
                                    Some(1) => TypedReceiverClaim::ReachesTarget,
                                    Some(0) => TypedReceiverClaim::DifferentOwner,
                                    _ => TypedReceiverClaim::Unknown,
                                },
                            )
                        } else {
                            None
                        };
                    let claimed = match row.source {
                        Source::Relationship => row
                            .to_symbol_id
                            .as_deref()
                            .is_some_and(|id| id == member.symbol_id),
                        Source::Identifier
                            if matches!(row.kind.as_str(), "type_usage" | "member_access") =>
                        {
                            resolver_ids.contains(&row.key)
                        }
                        _ => judged_row.is_some_and(|j| j.claimed),
                    };
                    if !claimed && typed_receiver.is_none() {
                        continue;
                    }
                    let basis = match row.source {
                        // julie links a call inside a method to that method by name alone.
                        Source::Relationship if row.caller_id == row.to_symbol_id => None,
                        Source::Relationship => Some("same file".to_string()),
                        _ => judged_row.and_then(|j| j.basis.clone()),
                    };
                    let basis = match basis {
                        Some(basis)
                            if basis == IMPORT_BASIS
                                && row.language == "python"
                                && !self.python_import_reaches(
                                    row,
                                    &spec.name,
                                    &member.path,
                                )? =>
                        {
                            None
                        }
                        basis => basis,
                    };
                    let entry = claims.entry(row.key.clone()).or_insert(Claim {
                        basis: None,
                        typed_receiver: None,
                    });
                    if let Some(typed_receiver) = typed_receiver {
                        entry.typed_receiver = Some(
                            entry
                                .typed_receiver
                                .map_or(typed_receiver, |previous| previous.merge(typed_receiver)),
                        );
                    }
                    if claimed && entry.basis.is_none() {
                        entry.basis = basis;
                    }
                }
            }
            judged_rows += chunk.len();
        }
        Ok((claims, judged_rows))
    }

    fn decide_cheap(
        &mut self,
        row: &NameRow,
        spec: &Spec,
        target: &Symbol,
        group_ids: &HashSet<&str>,
        claims: &HashMap<String, Claim>,
        shorthand_site: &mut ShorthandSite,
    ) -> Result<Option<Verdict>, QueryError> {
        if !languages_compatible(&row.language, &target.language) {
            return Ok(Some(Verdict::new(
                SiteLabel::Excluded,
                "other language",
                false,
            )));
        }
        if let Some(verdict) = self.csharp_qualified_constructor(row, target)? {
            return Ok(Some(verdict));
        }
        if spec
            .group
            .iter()
            .all(|member| member.signature.as_deref().is_some_and(is_test_label))
        {
            return Ok(Some(Verdict::new(
                SiteLabel::Excluded,
                format!("`{}` is a test label, which code never names", spec.name),
                false,
            )));
        }
        if spec
            .group
            .iter()
            .all(|member| row.reads_a_module(&member.kind))
        {
            return Ok(Some(Verdict::new(
                SiteLabel::Excluded,
                format!(
                    "`{}` is a module, which a read of a value never names",
                    spec.name
                ),
                false,
            )));
        }
        if let Some(outer) = &spec.local_to
            && !(row.path == outer.path.replace('\\', "/")
                && (outer.start_line..=outer.end_line).contains(&row.site_line))
        {
            return Ok(Some(Verdict::new(
                SiteLabel::Excluded,
                format!("`{}` is local to `{}`", spec.name, outer.name),
                false,
            )));
        }
        // julie writes a Java method reference (`parser::next`) as a call, so a member access
        // without parentheses reads a field.
        if row.language == "java"
            && row.kind == "member_access"
            && spec
                .group
                .iter()
                .all(|member| member.language == "java" && member.kind == "method")
        {
            return Ok(Some(Verdict::new(
                SiteLabel::Excluded,
                format!(
                    "a member access reads a field, not the method `{}`",
                    spec.name
                ),
                false,
            )));
        }
        if let Some(typed_receiver) = claims.get(&row.key).and_then(|claim| claim.typed_receiver) {
            return Ok(match typed_receiver {
                TypedReceiverClaim::ReachesTarget => {
                    Some(Verdict::new(SiteLabel::Proven, "receiver type", false))
                }
                TypedReceiverClaim::DifferentOwner => Some(Verdict::new(
                    SiteLabel::Excluded,
                    format!(
                        "receiver chain `{}` does not reach `{}`",
                        row.typed_receiver_chain
                            .as_deref()
                            .unwrap_or_default()
                            .join("."),
                        spec.name
                    ),
                    false,
                )),
                TypedReceiverClaim::Unknown => None,
            });
        }
        if let Some(shadow) = self.nearest_shadow(row, &spec.name, group_ids)? {
            let mut shorthand_candidate = false;
            if shorthand_site.target_member
                && row.kind == "variable_ref"
                && language_family(&row.language) == "js"
                && let Some(column) = row.site_column
                && shadow.local
                && matches!(shadow.kind.as_str(), "parameter" | "variable")
                && shadow.dynamic_import(&spec.name).is_none()
                && let Some(caller_id) = row.caller_id.as_deref()
                && let Some(caller) = self.definition_by_id(caller_id)?
                && caller.path == row.path
                && let Some(source) = caller_source_through_site(
                    self.root,
                    &row.path,
                    caller.start_line,
                    row.site_line,
                )
                && let Some(site_line) = row
                    .site_line
                    .checked_sub(caller.start_line)
                    .and_then(|line| line.checked_add(1))
            {
                shorthand_candidate =
                    is_object_shorthand_site(&source, site_line, column, &spec.name);
            }
            if shorthand_candidate {
                shorthand_site.shadow_id = Some(shadow.symbol_id.clone());
            }
            let dynamic = shadow.dynamic_import(&spec.name);
            if let Some((source, imported)) = &dynamic
                && let Some(verdict) =
                    self.js_dynamic_import_verdict(row, spec, source, imported)?
            {
                return Ok(Some(verdict));
            }
            let follows_project_module = match &dynamic {
                Some((source, _)) => !self.js_source_is_external(&row.path, source)?,
                None => false,
            };
            if !follows_project_module && !shorthand_candidate {
                let basis = match self.js_binding_line(row, &spec.name)? {
                    Some(line) if line > shadow.start_line || shadow.start_line > row.site_line => {
                        format!(
                            "line {line} binds the name as a callback parameter, loop variable, or catch parameter"
                        )
                    }
                    _ => shadow.resolves_to(),
                };
                return Ok(Some(Verdict::new(SiteLabel::Excluded, basis, false)));
            }
        }
        if let Some(line) = self.js_enclosing_binding_line(row, &spec.name)? {
            let is_shorthand_value_binding = match shorthand_site.shadow_id.as_deref() {
                Some(shadow_id) => self
                    .definition_by_id(shadow_id)?
                    .is_some_and(|shadow| shadow.start_line == line),
                None => false,
            };
            if !is_shorthand_value_binding {
                return Ok(Some(Verdict::new(
                    SiteLabel::Excluded,
                    format!(
                        "line {line} binds the name as a callback parameter, loop variable, or catch parameter"
                    ),
                    false,
                )));
            }
        }
        if let Some(verdict) = self.java_static_import(row, spec)? {
            return Ok(Some(verdict));
        }
        if let Some(verdict) = self.java_bare_call_outside_classes(row, spec)? {
            return Ok(Some(verdict));
        }
        if let Some(verdict) = self.csharp_type_outside_namespaces(row, spec)? {
            return Ok(Some(verdict));
        }
        if let Some(verdict) = self.csharp_conditional_fallback(row, &spec.name)? {
            return Ok(Some(verdict));
        }
        if row.is_bare_use() && spec.group.iter().all(is_property_method) {
            return Ok(Some(self.bare_use_of_property_method(row, &spec.name)?));
        }
        if let Some(verdict) = self.js_verdict(row, spec)?
            && (shorthand_site.shadow_id.is_none() || verdict.label != SiteLabel::Proven)
        {
            return Ok(Some(verdict));
        }
        if let Some(claim) = claims.get(&row.key)
            && let Some(basis) = &claim.basis
        {
            let skip_proven_claim = shorthand_site.shadow_id.is_some()
                && claimed_verdict(row, claim, 0, spec.builds_class).label == SiteLabel::Proven;
            if !skip_proven_claim {
                if basis == IMPORT_BASIS {
                    if language_family(&row.language) == "js"
                        && let Some(verdict) = self.external_import(row, &spec.name)?
                    {
                        return Ok(Some(verdict));
                    }
                    let others = self.other_import_matches(row, &spec.name, group_ids)?;
                    if others > 0 {
                        return Ok(Some(Verdict::new(
                            SiteLabel::Unresolved,
                            format!("the import matches {} definitions", others + 1),
                            true,
                        )));
                    }
                }
                return Ok(Some(claimed_verdict(row, claim, 0, spec.builds_class)));
            }
        }
        if self.crate_path_names_owner(row, &spec.group)? {
            return Ok(Some(Verdict::new(
                SiteLabel::Proven,
                "qualified path",
                false,
            )));
        }
        if let Some(to) = row.to_symbol_id.as_deref()
            && !row.hidden_receiver
            && !group_ids.contains(to)
            && shorthand_site.shadow_id.as_deref() != Some(to)
            && let Some(other) = self.definition_by_id(to)?
        {
            return Ok(Some(Verdict::new(
                SiteLabel::Excluded,
                other.resolves_to(),
                false,
            )));
        }
        self.external_import(row, &spec.name)
    }

    /// A bare Java call whose file imports the name statically from a class that owns no
    /// target (`import static java.lang.Math.toIntExact;`). Java takes such a call from the import
    /// unless the caller's file or class chain defines the name. A removed target defines nothing.
    /// A bare Java call names a method of the caller's class, of a class that encloses it, of
    /// their ancestors, or of a class that the file imports it from statically (`import static
    /// C.name;` or `import static C.*;`). An external base class cannot extend a project class,
    /// so an ancestor that is not indexed does not change this.
    fn java_bare_call_outside_classes(
        &self,
        row: &NameRow,
        spec: &Spec,
    ) -> Result<Option<Verdict>, QueryError> {
        if row.language != "java" || !row.is_call() || !row.is_bare_use() {
            return Ok(None);
        }
        let Some(caller) = row.caller_id.as_deref() else {
            return Ok(None);
        };
        let mut owners = Vec::new();
        for member in &spec.group {
            let owner = (member.language == "java" && member.kind == "method")
                .then(|| self.definition_by_id(&member.symbol_id))
                .transpose()?
                .flatten()
                .and_then(|d| d.owner_name);
            match owner {
                Some(owner) => owners.push(owner),
                None => return Ok(None),
            }
        }
        let chain = self.with_statement(SYMBOL_CHAIN_SQL, |stmt| {
            stmt.query_map(params![caller], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()
        })?;
        let is_class = |kind: &str| matches!(kind, "class" | "interface" | "enum" | "record");
        // julie hangs the methods of an anonymous class on the field or method that builds it,
        // so its supertype, whose methods a bare call also reaches, is unknown.
        if chain.windows(2).any(|pair| {
            matches!(pair[0].0.as_str(), "method" | "constructor") && !is_class(&pair[1].0)
        }) {
            return Ok(None);
        }
        let enclosing: Vec<String> = chain
            .into_iter()
            .filter(|(kind, _)| is_class(kind))
            .map(|(_, name)| name)
            .collect();
        let Some(innermost) = enclosing.first().cloned() else {
            return Ok(None);
        };
        let mut reachable = HashSet::new();
        for class in enclosing {
            reachable.extend(self.ancestor_names(&class)?);
            reachable.insert(class);
        }
        let imported_from = self.with_statement(JAVA_STATIC_IMPORTS_SQL, |stmt| {
            stmt.query_map(params![row.path, spec.name], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })?;
        reachable.extend(imported_from.iter().filter_map(|signature| {
            let imported = signature
                .strip_prefix("import static ")?
                .trim_end_matches(';');
            let class_path = imported.trim().rsplit_once('.')?.0;
            Some(class_path.rsplit('.').next()?.to_string())
        }));
        if owners.iter().any(|owner| reachable.contains(owner)) {
            return Ok(None);
        }
        Ok(Some(Verdict::new(
            SiteLabel::Excluded,
            format!("a bare call reaches only the classes of `{innermost}` and its static imports"),
            false,
        )))
    }

    fn java_static_import(
        &self,
        row: &NameRow,
        spec: &Spec,
    ) -> Result<Option<Verdict>, QueryError> {
        if row.language != "java" || !row.is_call() || row.has_qualifier() {
            return Ok(None);
        }
        let signatures = self.with_statement(JAVA_STATIC_IMPORTS_SQL, |stmt| {
            stmt.query_map(params![row.path, spec.name], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })?;
        let class_paths: Vec<String> = signatures
            .iter()
            .filter_map(|signature| {
                let imported = signature
                    .strip_prefix("import static ")?
                    .trim_end_matches(';');
                Some(imported.trim().rsplit_once('.')?.0.to_string())
            })
            .collect();
        let Some(class_path) = class_paths.first() else {
            return Ok(None);
        };
        let distinct_class_paths: HashSet<&str> = class_paths.iter().map(String::as_str).collect();
        let names_target_class = class_paths.iter().any(|class_path| {
            let class_file = format!("/{}.java", class_path.replace('.', "/"));
            spec.group
                .iter()
                .any(|member| format!("/{}", member.path.replace('\\', "/")).ends_with(&class_file))
        });
        if names_target_class && distinct_class_paths.len() > 1 {
            return Ok(None);
        }
        if names_target_class
            && self
                .rows_at_path(CALLABLE_IN_FILE_SQL, &row.path, &[&spec.name], |r| {
                    r.get::<_, i64>(0)
                })?
                .is_empty()
        {
            return Ok(Some(Verdict::new(SiteLabel::Proven, IMPORT_BASIS, true)));
        }
        let class_name = class_path.rsplit('.').next().unwrap_or(class_path.as_str());
        let mut chain = HashSet::new();
        if let Some(caller) = row.caller_id.as_deref()
            && let Some(class) = self.definition_by_id(caller)?.and_then(|d| d.owner_name)
        {
            chain = self.ancestor_names(&class)?;
            chain.insert(class);
        }
        let present = spec.group.iter().filter(|member| {
            self.absent
                .as_ref()
                .is_none_or(|absent| absent.symbol_id != member.symbol_id)
        });
        for member in present {
            if member.path.replace('\\', "/") == row.path {
                return Ok(None);
            }
            let owner = self
                .definition_by_id(&member.symbol_id)?
                .and_then(|d| d.owner_name);
            if owner.is_some_and(|owner| owner == class_name || chain.contains(&owner)) {
                return Ok(None);
            }
        }
        Ok(Some(Verdict::new(
            SiteLabel::Excluded,
            format!("`{}` is imported from `{class_path}`", spec.name),
            false,
        )))
    }

    fn external_import(
        &mut self,
        row: &NameRow,
        name: &str,
    ) -> Result<Option<Verdict>, QueryError> {
        Ok(self.external_binding(row, name)?.map(|(binding, source)| {
            Verdict::new(
                SiteLabel::Excluded,
                format!("receiver `{binding}` is imported from `{source}`"),
                false,
            )
        }))
    }

    /// A Rust path call `krate::module::Owner::name()` whose leading segments name the crate and
    /// the modules that hold the target. The resolver maps crate names to folders only for
    /// functions without an owner, so it leaves such a method call unclaimed.
    fn crate_path_names_owner(&self, row: &NameRow, group: &[Symbol]) -> Result<bool, QueryError> {
        if row.language != "rust"
            || row.source != Source::Pending
            || row.kind != "calls"
            || row.namespace.len() < 2
        {
            return Ok(false);
        }
        let Some((owner_segment, leading)) = row.namespace.split_last() else {
            return Ok(false);
        };
        if matches!(leading[0].as_str(), "crate" | "self" | "super" | "Self") {
            return Ok(false);
        }
        for member in group.iter().filter(|member| member.language == "rust") {
            let Some(owner) = self
                .definition_by_id(&member.symbol_id)?
                .and_then(|definition| definition.owner_name)
            else {
                continue;
            };
            let path = member.path.replace('\\', "/");
            let Some((package, module_path)) = path.rsplit_once("/src/") else {
                continue;
            };
            let package_name = package
                .rsplit('/')
                .next()
                .unwrap_or(package)
                .replace('-', "_");
            let modules_match = leading[1..].iter().all(|segment| {
                module_path
                    .split('/')
                    .any(|part| part == segment || part.strip_suffix(".rs") == Some(segment))
            });
            if owner != *owner_segment || leading[0] != package_name || !modules_match {
                continue;
            }
            let in_package = format!("{}/%", crate::queries::escape_like(package));
            let owners = self.with_statement(OWNER_IN_PACKAGE_SQL, |stmt| {
                stmt.query_row(params![owner, in_package], |r| r.get::<_, i64>(0))
            })?;
            if owners == 1 {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn decide_late(
        &mut self,
        row: &NameRow,
        group: &[Symbol],
        group_definitions: &[&Definition],
        rivals: &[&Definition],
        shorthand_candidate: bool,
    ) -> Result<Verdict, QueryError> {
        if let Some(verdict) = self.outside_ancestors(row, group)? {
            return Ok(verdict);
        }
        if row.typed_receiver_chain.is_some()
            && crate::queries::has_typed_receiver_chain_support(self.conn)
        {
            return Ok(if shorthand_candidate {
                Verdict::new(
                    SiteLabel::Unresolved,
                    format!("{} definitions", rivals.len() + 2),
                    false,
                )
            } else if rivals.is_empty() {
                Verdict::new(SiteLabel::Likely, "name only", false)
            } else {
                Verdict::new(
                    SiteLabel::Unresolved,
                    format!("{} definitions", rivals.len() + 1),
                    false,
                )
            });
        }
        if let Some(receiver) = row.qualifier().filter(|_| row.namespace.len() <= 1)
            && let Some(type_name) = self.receiver_type(row, receiver)?
            && !self.receiver_type_related(row, &type_name, group)?
        {
            return Ok(Verdict::new(
                SiteLabel::Excluded,
                format!("receiver `{receiver}` has type `{type_name}`"),
                false,
            ));
        }
        if let Some(head) = self.foreign_type_head(row, group)? {
            return Ok(Verdict::new(
                SiteLabel::Excluded,
                format!("receiver `{head}` has type `{head}`"),
                false,
            ));
        }
        if row.kind == "member_access"
            && rivals.is_empty()
            && let Some(target) = self.absent.as_ref()
            && matches!(target.kind.as_str(), "function" | "method" | "constructor")
            && let Some(parent) = target.parent_symbol_id.as_deref()
            && let Some(owner) = crate::queries::get_symbol_by_id(self.conn, parent)?
            && CLASS_LIKE_KINDS.contains(&owner.kind.as_str())
            && owner.language == row.language
            && let Some(receiver) = row.qualifier()
            && self.receiver_type(row, receiver)?.as_deref() == Some(owner.name.as_str())
        {
            return Ok(Verdict::new(SiteLabel::Proven, "receiver type", false));
        }
        if let Some(object) = self.object_assigned_on(row, group_definitions, rivals)? {
            return Ok(Verdict::new(
                SiteLabel::Likely,
                format!(
                    "receiver `{object}` names the object `{}` is assigned on",
                    group[0].name
                ),
                false,
            ));
        }
        if shorthand_candidate {
            return Ok(Verdict::new(
                SiteLabel::Unresolved,
                format!("{} definitions", rivals.len() + 2),
                false,
            ));
        }
        if rivals.is_empty() {
            Ok(Verdict::new(SiteLabel::Likely, "name only", false))
        } else {
            Ok(Verdict::new(
                SiteLabel::Unresolved,
                format!("{} definitions", rivals.len() + 1),
                false,
            ))
        }
    }

    /// A `super` or `base` call whose julie link into the caller's own class was dropped reaches
    /// an ancestor of that class. A target none of whose owners is such an ancestor is out of
    /// reach.
    fn outside_ancestors(
        &self,
        row: &NameRow,
        group: &[Symbol],
    ) -> Result<Option<Verdict>, QueryError> {
        let (Some(caller), Some(receiver)) = (row.caller_id.as_deref(), row.receiver.as_deref())
        else {
            return Ok(None);
        };
        if !row.links_enclosing_class {
            return Ok(None);
        }
        let Some(class) = self
            .definition_by_id(caller)?
            .and_then(|caller| caller.owner_name)
        else {
            return Ok(None);
        };
        let ancestors = self.ancestor_names(&class)?;
        for member in group {
            match self
                .definition_by_id(&member.symbol_id)?
                .and_then(|member| member.owner_name)
            {
                Some(owner) if !ancestors.contains(&owner) => {}
                _ => return Ok(None),
            }
        }
        Ok(Some(Verdict::new(
            SiteLabel::Excluded,
            format!("receiver `{receiver}` reaches only the ancestors of `{class}`"),
            false,
        )))
    }

    /// The receiver of a JavaScript call that names the object the target is assigned on
    /// (`res.redirect = function`), when no rival is assigned on an object of that name.
    fn object_assigned_on(
        &self,
        row: &NameRow,
        group: &[&Definition],
        rivals: &[&Definition],
    ) -> Result<Option<String>, QueryError> {
        let Some(receiver) = row
            .receiver
            .as_deref()
            .filter(|r| is_plain_identifier(r) && !SELF_RECEIVERS.contains(r))
            .filter(|_| matches!(row.kind.as_str(), "calls" | "call" | "member_access"))
        else {
            return Ok(None);
        };
        let assigned_on_receiver = |d: &&Definition| {
            language_family(&d.language) == "js"
                && matches!(d.kind.as_str(), "method" | "function")
                && d.top_level
                && d.class_name.as_deref() == Some(receiver)
        };
        if !group.iter().all(assigned_on_receiver)
            || rivals
                .iter()
                .any(|d| d.class_name.as_deref() == Some(receiver))
        {
            return Ok(None);
        }
        let Some(target) = group.first() else {
            return Ok(None);
        };
        let objects =
            self.rows_at_path(TOP_LEVEL_VARIABLE_SQL, &target.path, &[&receiver], |row| {
                row.get::<_, i64>(0)
            })?;
        Ok((!objects.is_empty()).then(|| receiver.to_string()))
    }

    fn definition_by_id(&self, symbol_id: &str) -> Result<Option<Definition>, QueryError> {
        if let Some(absent) = self.absent.as_ref().filter(|s| s.symbol_id == symbol_id) {
            return self.absent_definition(absent).map(Some);
        }
        self.with_statement(DEFINITION_BY_ID_SQL, |stmt| {
            stmt.query_row(params![symbol_id], |row| {
                let parent_kind: Option<String> = row.get(9)?;
                let parent_name: Option<String> = row.get(8)?;
                let kind: String = row.get(2)?;
                let local = kind == "parameter"
                    || kind == "variable"
                        && parent_kind.as_deref().is_some_and(|kind| {
                            matches!(kind, "function" | "method" | "constructor")
                        });
                Ok(Definition {
                    symbol_id: row.get(0)?,
                    name: row.get(1)?,
                    path: row.get::<_, String>(4)?.replace('\\', "/"),
                    start_line: row.get::<_, i64>(5)? as usize,
                    decorated_start_line: None,
                    body_start_line: row.get::<_, Option<i64>>(6)?.map(|v| v as usize),
                    owner_name: parent_name.filter(|_| {
                        parent_kind
                            .as_deref()
                            .is_some_and(|k| CLASS_LIKE_KINDS.contains(&k))
                    }),
                    owner_kind: parent_kind
                        .filter(|kind| CLASS_LIKE_KINDS.contains(&kind.as_str())),
                    local,
                    scope: None,
                    kind,
                    language: row.get(3)?,
                    signature: row.get(10)?,
                    owner_signature: row.get(11)?,
                    class_name: None,
                    top_level: row.get::<_, Option<String>>(7)?.is_none(),
                    callable: true,
                    free: false,
                    module_value: false,
                })
            })
            .optional()
        })
    }

    /// The definition of the target the index no longer holds, with the owner its
    /// `parent_symbol_id` names.
    fn absent_definition(&self, symbol: &Symbol) -> Result<Definition, QueryError> {
        let owner = match &symbol.parent_symbol_id {
            Some(parent) => crate::queries::get_symbol_by_id(self.conn, parent)?,
            None => None,
        };
        let owner_signature = owner
            .as_ref()
            .and_then(|owner| owner.signature.clone())
            .unwrap_or_default();
        let owner_kind = owner
            .as_ref()
            .filter(|owner| CLASS_LIKE_KINDS.contains(&owner.kind.as_str()))
            .map(|owner| owner.kind.clone());
        let owner_name = owner
            .filter(|owner| CLASS_LIKE_KINDS.contains(&owner.kind.as_str()))
            .map(|owner| owner.name);
        Ok(Definition {
            symbol_id: symbol.symbol_id.clone(),
            name: symbol.name.clone(),
            path: symbol.path.replace('\\', "/"),
            start_line: symbol.start_line,
            decorated_start_line: None,
            body_start_line: symbol.body_start_line,
            owner_name,
            owner_kind,
            local: false,
            scope: None,
            kind: symbol.kind.clone(),
            language: symbol.language.clone(),
            signature: String::new(),
            owner_signature,
            class_name: assigned_object(symbol),
            top_level: symbol.parent_symbol_id.is_none(),
            callable: matches!(symbol.kind.as_str(), "function" | "method" | "constructor"),
            free: false,
            module_value: false,
        })
    }

    /// The nearest definition of the name in a scope that holds the call, when it is not part of
    /// the target group: a parameter, local variable, or nested function of the caller or an
    /// enclosing function, or a top-level definition of the caller's file. In one scope the last
    /// definition above the call wins, since julie does not record block scopes.
    fn nearest_shadow(
        &self,
        row: &NameRow,
        name: &str,
        group_ids: &HashSet<&str>,
    ) -> Result<Option<Definition>, QueryError> {
        let type_use = row.is_bare_type_use();
        if !row.is_bare_use() && !type_use {
            return Ok(None);
        }
        let mut hit = None;
        if let Some(caller) = row.caller_id.as_deref() {
            hit = self.first_real_definition(
                SHADOW_IN_CHAIN_SQL,
                params![caller, name, row.path, type_use, row.site_line as i64],
                row,
            )?;
        }
        if hit.is_none() {
            hit = self.first_real_definition(
                SHADOW_TOP_LEVEL_SQL,
                params![name, row.path, row.site_line as i64],
                row,
            )?;
        }
        match hit {
            Some(id) if !group_ids.contains(id.as_str()) => self.definition_by_id(&id),
            _ => Ok(None),
        }
    }

    /// The first row of `sql` that `row` can name and that is not a test label
    /// (`describe("mix")`), which julie indexes as a function but code never reads by name.
    fn first_real_definition(
        &self,
        sql: &'static str,
        query: impl rusqlite::Params,
        row: &NameRow,
    ) -> Result<Option<String>, QueryError> {
        let rows = self.with_statement(sql, |stmt| {
            stmt.query_map(query, |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()
        })?;
        Ok(rows
            .into_iter()
            .find(|(_, signature, kind)| !is_test_label(signature) && !row.is_not_named_by(kind))
            .map(|(id, ..)| id))
    }

    /// The definitions that the import binding of `receiver` (or of `name` itself when the
    /// receiver is empty) in file `importer` reaches, as `(symbol_id, path, language)` rows.
    fn import_matches_of(
        &mut self,
        importer: &str,
        receiver: &str,
        name: &str,
    ) -> Result<&[ImportMatch], QueryError> {
        let key = (importer.to_string(), receiver.to_string(), name.to_string());
        if !self.import_matches.contains_key(&key) {
            if self.import_match_stmt.is_none() {
                self.import_match_stmt = Some(self.conn.prepare(&import_match_sql())?);
            }
            let stmt = self
                .import_match_stmt
                .as_mut()
                .expect("import match statement was just prepared");
            let matches = stmt
                .query_map(params![importer, receiver, name], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                })?
                .filter(|m| !matches!(m, Ok((_, _, signature, _)) if is_test_label(signature)))
                .map(|m| m.map(|(id, path, _, language)| (id, path, language)))
                .collect::<Result<Vec<_>, _>>()?;
            self.import_matches.insert(key.clone(), matches);
        }
        Ok(&self.import_matches[&key])
    }

    /// How many top-level definitions outside the target group the import behind the row also
    /// reaches: a package import (`import * as z from "zod"`) can match every file of the package.
    fn other_import_matches(
        &mut self,
        row: &NameRow,
        name: &str,
        group_ids: &HashSet<&str>,
    ) -> Result<usize, QueryError> {
        let receiver = row.receiver.clone().unwrap_or_default();
        let matches = self.import_matches_of(&row.path, &receiver, name)?.to_vec();
        let mut count = 0;
        for (symbol_id, path, _) in &matches {
            if group_ids.contains(symbol_id.as_str()) {
                continue;
            }
            if row.language == "python"
                && !self.python_import_reaches(row, name, &path.replace('\\', "/"))?
            {
                continue;
            }
            count += 1;
        }
        Ok(count)
    }

    /// The top-level definitions named `name`, in the language of the importer, that the import
    /// binding of `name` in file `importer` reaches. A Python absolute import reaches only the
    /// files its first segment starts at.
    fn imported_definitions(
        &mut self,
        importer: &str,
        language: &str,
        name: &str,
    ) -> Result<Vec<String>, QueryError> {
        let matches = self.import_matches_of(importer, "", name)?.to_vec();
        let mut reached = Vec::new();
        for (symbol_id, path, definition_language) in matches {
            if !languages_compatible(&definition_language, language) {
                continue;
            }
            if language == "python"
                && !self.python_binding_reaches(importer, name, &path.replace('\\', "/"))?
            {
                continue;
            }
            reached.push(symbol_id);
        }
        Ok(reached)
    }

    fn bare_use_of_property_method(
        &self,
        row: &NameRow,
        name: &str,
    ) -> Result<Verdict, QueryError> {
        let sources = self.with_statement(IMPORT_SOURCES_SQL, |stmt| {
            stmt.query_map(params![row.path, name], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })?;
        let basis = match sources.iter().find(|source| !source.is_empty()) {
            Some(source) => format!("resolves to the import of `{name}` from `{source}`"),
            None => "a bare name cannot reach a property method".to_string(),
        };
        Ok(Verdict::new(SiteLabel::Excluded, basis, false))
    }

    /// Marks the JavaScript calls whose receiver is a `const` of the caller's scope or file that
    /// holds a call result or a new object (`const schema = z.object({…})`), the nearest
    /// declaration above the call.
    fn mark_result_variables(&self, rows: &mut [NameRow]) -> Result<(), QueryError> {
        let mut texts: HashMap<String, Option<String>> = HashMap::new();
        for row in rows.iter_mut() {
            if row.result_receiver
                || row.hidden_receiver
                || language_family(&row.language) != "js"
                || !row.is_call()
                || !row.namespace.is_empty()
            {
                continue;
            }
            let (Some(receiver), Some(caller)) = (
                row.receiver
                    .clone()
                    .filter(|r| is_plain_identifier(r) && !SELF_RECEIVERS.contains(&r.as_str())),
                row.caller_id.as_deref(),
            ) else {
                continue;
            };
            let declarations = self.with_statement(RECEIVER_DECLARATIONS_SQL, |stmt| {
                stmt.query_map(params![caller, receiver, row.path], |r| {
                    Ok((r.get::<_, String>(1)?, r.get::<_, i64>(2)? as usize))
                })?
                .collect::<Result<Vec<_>, _>>()
            })?;
            let Some((kind, line)) = declarations
                .into_iter()
                .find(|(_, line)| *line <= row.site_line)
            else {
                continue;
            };
            if kind == "parameter" {
                continue;
            }
            let text = texts
                .entry(row.path.clone())
                .or_insert_with(|| std::fs::read_to_string(self.root.join(&row.path)).ok());
            row.result_receiver = text
                .as_deref()
                .and_then(|text| text.lines().nth(line.saturating_sub(1)))
                .is_some_and(|declaration| const_holds_result(declaration, &receiver));
        }
        Ok(())
    }

    /// The other definitions that can reach `row`; a module receiver reaches no member of a class.
    fn rivals<'d>(
        &self,
        row: &NameRow,
        others: &[&'d Definition],
    ) -> Result<Vec<&'d Definition>, QueryError> {
        let module_receiver = self.receiver_is_python_module(row)?;
        let bare_csharp = row.language == "csharp" && !row.has_qualifier();
        let absent_member = self.absent.as_ref().is_some_and(|target| {
            target.kind == "field"
                || matches!(target.kind.as_str(), "function" | "method" | "constructor")
        });
        let receiver_type = match (row.kind.as_str(), absent_member) {
            ("member_access", true) => row
                .qualifier()
                .map(|receiver| self.receiver_type(row, receiver))
                .transpose()?
                .flatten(),
            _ => None,
        };
        let mut rivals = Vec::new();
        for &other in others {
            if !row.can_reach(other) || module_receiver && other.owner_name.is_some() {
                continue;
            }
            if let Some(type_name) = receiver_type.as_deref()
                && let Some(symbol) = crate::queries::get_symbol_by_id(self.conn, &other.symbol_id)?
                && !self.receiver_type_related(row, type_name, std::slice::from_ref(&symbol))?
            {
                continue;
            }
            let csharp_type = other.language == "csharp"
                && (other.kind == "constructor"
                    || other.owner_name.is_none()
                        && CSHARP_TYPE_KINDS.contains(&other.kind.as_str()));
            if bare_csharp && csharp_type && !self.csharp_type_in_scope(row, other)? {
                continue;
            }
            rivals.push(other);
        }
        Ok(rivals)
    }

    /// A bare C# use of a top-level type whose namespace the use's file neither holds nor opens,
    /// and that no `using Name = …;` alias names.
    fn csharp_type_outside_namespaces(
        &self,
        row: &NameRow,
        spec: &Spec,
    ) -> Result<Option<Verdict>, QueryError> {
        if row.language != "csharp" || row.has_qualifier() {
            return Ok(None);
        }
        let mut namespace = None;
        for member in &spec.group {
            let Some(definition) = self.definition_by_id(&member.symbol_id)? else {
                return Ok(None);
            };
            if definition.language != "csharp"
                || definition.owner_name.is_some()
                || !CSHARP_TYPE_KINDS.contains(&definition.kind.as_str())
                || self.csharp_type_in_scope(row, &definition)?
            {
                return Ok(None);
            }
            namespace = Some(self.csharp_namespace(&definition.symbol_id)?);
        }
        let usings = self.with_statement(CSHARP_USINGS_SQL, |stmt| {
            stmt.query_map(params![row.path], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })?;
        let alias = format!("using {} =", spec.name);
        if usings.iter().any(|using| using.contains(&alias)) {
            return Ok(None);
        }
        Ok(namespace.map(|namespace| {
            Verdict::new(
                SiteLabel::Excluded,
                format!("namespace `{namespace}` is not open here"),
                false,
            )
        }))
    }

    fn csharp_qualified_constructor(
        &self,
        row: &NameRow,
        target: &Symbol,
    ) -> Result<Option<Verdict>, QueryError> {
        if row.language != "csharp"
            || target.language != "csharp"
            || !row.is_call()
            || row.has_qualifier()
            || !CSHARP_TYPE_KINDS.contains(&target.kind.as_str())
        {
            return Ok(None);
        }
        let Some(column) = row.site_column else {
            return Ok(None);
        };
        let Some(source) =
            caller_source_through_site(self.root, &row.path, row.site_line, row.site_line)
        else {
            return Ok(None);
        };
        let Some(line) = source.lines().next() else {
            return Ok(None);
        };
        let Some(qualifier) = csharp_constructor_qualifier(line, column) else {
            return Ok(None);
        };
        if !qualifier.global {
            let alias = qualifier.path.split('.').next().unwrap_or_default();
            let aliases = self.with_statement(CSHARP_USINGS_SQL, |stmt| {
                stmt.query_map(params![row.path], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()
            })?;
            if aliases
                .iter()
                .filter_map(|using| csharp_using_alias(using))
                .any(|(name, _)| name == alias)
            {
                return Ok(Some(Verdict::new(
                    SiteLabel::Unresolved,
                    format!("qualified type alias `{alias}` is not resolved"),
                    false,
                )));
            }
        }
        let caller_namespace = match row.caller_id.as_deref() {
            Some(caller) => csharp_namespace_for_symbol(self.conn, caller)?,
            None => {
                return Ok(Some(Verdict::new(
                    SiteLabel::Unresolved,
                    "qualified C# type path has no indexed caller scope",
                    false,
                )));
            }
        };
        if !qualifier.global && !caller_namespace.is_empty() {
            return Ok(Some(Verdict::new(
                SiteLabel::Unresolved,
                "relative C# type qualification is not resolved",
                false,
            )));
        }
        if !qualifier.global
            && let Some(caller) = row.caller_id.as_deref()
            && csharp_type_prefix_in_scope(
                self.conn,
                caller,
                qualifier.path.split('.').next().unwrap_or_default(),
            )?
        {
            return Ok(Some(Verdict::new(
                SiteLabel::Unresolved,
                "qualified C# path may name a nested type in the caller scope",
                false,
            )));
        }
        let target_path = csharp_type_qualification_for_symbol(self.conn, &target.symbol_id)?;
        if qualifier.path == target_path.join(".") {
            return Ok(Some(Verdict::new(
                SiteLabel::Proven,
                "qualified type",
                true,
            )));
        }
        let (candidates, complete) = csharp_type_qualifications_for_name(self.conn, &target.name)?;
        if candidates
            .iter()
            .any(|(id, path)| id != &target.symbol_id && path.join(".") == qualifier.path)
        {
            return Ok(Some(Verdict::new(
                SiteLabel::Excluded,
                "qualified path matches another indexed C# type",
                false,
            )));
        }
        Ok(Some(Verdict::new(
            SiteLabel::Unresolved,
            if complete {
                "qualified C# type path does not match an indexed definition"
            } else {
                "qualified C# type path is ambiguous across indexed definitions"
            },
            false,
        )))
    }

    /// A bare C# use of a target that julie kept from an `#if !X` branch, in a file that always
    /// opens a namespace that other files open under `#if X`: the file builds against that
    /// namespace, which defines the name where `X` holds. A file that opens it under `#if X`
    /// itself pairs its use with the target, which a build without `X` needs, so it stays.
    fn csharp_conditional_fallback(
        &self,
        row: &NameRow,
        name: &str,
    ) -> Result<Option<Verdict>, QueryError> {
        if row.language != "csharp" || row.has_qualifier() {
            return Ok(None);
        }
        let Some(condition) = self.target_condition.as_deref() else {
            return Ok(None);
        };
        let negated: Vec<&str> = condition_terms(condition)
            .filter_map(|term| term.strip_prefix('!'))
            .map(str::trim)
            .collect();
        if negated.is_empty() {
            return Ok(None);
        }
        let usings = self.with_statement(CSHARP_FILE_USINGS_SQL, |stmt| {
            stmt.query_map(params![row.path], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()
        })?;
        let under_negated = |condition: &str| {
            condition_terms(condition)
                .find_map(|term| negated.iter().copied().find(|symbol| *symbol == term))
        };
        if usings
            .iter()
            .any(|(_, condition)| condition.as_deref().and_then(under_negated).is_some())
        {
            return Ok(None);
        }
        let paired = self.with_statement(CSHARP_CONDITIONAL_USINGS_SQL, |stmt| {
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<Result<Vec<_>, _>>()
        })?;
        Ok(usings
            .iter()
            .filter(|(_, condition)| condition.is_none())
            .filter_map(|(signature, _)| csharp_using_namespace(signature))
            .find_map(|namespace| {
                let symbol = paired.iter().find_map(|(signature, condition)| {
                    (csharp_using_namespace(signature) == Some(namespace))
                        .then(|| under_negated(condition))
                        .flatten()
                })?;
                Some(Verdict::new(
                    SiteLabel::Unresolved,
                    format!(
                        "`{name}` is defined only without `{symbol}`, and this file always opens `{namespace}`, which other files open with it"
                    ),
                    false,
                ))
            }))
    }

    /// Whether a bare C# use can name the type of `other`: its namespace is the global one, one
    /// that holds the use, or one that a `using` of the use's file or a `global using` opens. A
    /// project file with an MSBuild `<Using …/>` item opens namespaces the index does not see, so
    /// every namespace counts as open there.
    fn csharp_type_in_scope(&self, row: &NameRow, other: &Definition) -> Result<bool, QueryError> {
        let namespace = self.csharp_namespace(&other.symbol_id)?;
        let Some(caller) = row.caller_id.as_deref().filter(|_| !namespace.is_empty()) else {
            return Ok(true);
        };
        if self.msbuild_usings()? {
            return Ok(true);
        }
        let enclosing = self.csharp_namespace(caller)?;
        if enclosing == namespace || enclosing.starts_with(&format!("{namespace}.")) {
            return Ok(true);
        }
        let usings = self.with_statement(CSHARP_USINGS_SQL, |stmt| {
            stmt.query_map(params![row.path], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })?;
        Ok(usings
            .iter()
            .any(|using| csharp_using_namespace(using) == Some(namespace.as_str())))
    }

    /// Whether a `.csproj`, `.props`, or `.targets` file of the project holds a `<Using …/>` item.
    fn msbuild_usings(&self) -> Result<bool, QueryError> {
        if let Some(known) = self.msbuild_usings.get() {
            return Ok(*known);
        }
        let paths = self.with_statement(MSBUILD_FILES_SQL, |stmt| {
            stmt.query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })?;
        let found = paths.iter().any(|path| {
            std::fs::read_to_string(self.root.join(path))
                .is_ok_and(|text| text.contains("<Using ") || text.contains("<Using\n"))
        });
        Ok(*self.msbuild_usings.get_or_init(|| found))
    }

    /// The dotted namespace that holds `symbol_id`, read from its `namespace` ancestors; empty
    /// for the global namespace. A removed target starts from its owner in the index.
    fn csharp_namespace(&self, symbol_id: &str) -> Result<String, QueryError> {
        let start = match self.absent.as_ref().filter(|s| s.symbol_id == symbol_id) {
            Some(absent) => match absent.parent_symbol_id.as_deref() {
                Some(parent) => parent,
                None => return Ok(String::new()),
            },
            None => symbol_id,
        };
        csharp_namespace_for_symbol(self.conn, start)
    }

    /// Whether the receiver chain of a Python row names a module of the project: `flask.json`
    /// after `import flask`, or `json` after `from flask import json`.
    fn receiver_is_python_module(&self, row: &NameRow) -> Result<bool, QueryError> {
        if row.language != "python" || row.hidden_receiver {
            return Ok(false);
        }
        let chain: Vec<&str> = row
            .namespace
            .iter()
            .chain(row.receiver.iter())
            .flat_map(|part| part.split('.'))
            .filter(|part| !part.is_empty())
            .collect();
        let Some((head, rest)) = chain.split_first() else {
            return Ok(false);
        };
        let imports = self.with_statement(PYTHON_IMPORT_ROWS_SQL, |stmt| {
            stmt.query_map(params![row.path, head], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()
        })?;
        let [(signature, source, imported)] = imports.as_slice() else {
            return Ok(false);
        };
        if source.is_empty() || source.starts_with('.') {
            return Ok(false);
        }
        let mut module: Vec<&str> = source.split('.').collect();
        if signature.trim_start().starts_with("from ") {
            module.push(imported);
        }
        module.extend(rest);
        let module = module.join("/");
        let package = format!(
            "%/{}",
            crate::queries::escape_like(&format!("{module}/__init__.py"))
        );
        let file = format!("%/{}", crate::queries::escape_like(&format!("{module}.py")));
        self.with_statement(SOURCE_FILES_SQL, |stmt| stmt.exists(params![package, file]))
    }

    /// Whether the absolute import that binds the row's qualifier, or the name itself, starts at
    /// the top package of `path` or at a folder above it: `import json` does not reach
    /// `src/flask/json/provider.py`, whose top package is `flask`.
    fn python_import_reaches(
        &self,
        row: &NameRow,
        name: &str,
        path: &str,
    ) -> Result<bool, QueryError> {
        self.python_binding_reaches(&row.path, row.qualifier().unwrap_or(name), path)
    }

    /// Whether the absolute import of `binding` in file `importer` starts at the top package of
    /// `path` or at a folder above it.
    fn python_binding_reaches(
        &self,
        importer: &str,
        binding: &str,
        path: &str,
    ) -> Result<bool, QueryError> {
        let sources = self.with_statement(IMPORT_SOURCES_SQL, |stmt| {
            stmt.query_map(params![importer, binding], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })?;
        if sources.is_empty() || sources.iter().any(|s| s.is_empty() || s.starts_with('.')) {
            return Ok(true);
        }
        let reachable = self.python_import_heads(path)?;
        Ok(sources.iter().any(|source| {
            source
                .split('.')
                .next()
                .is_some_and(|first| reachable.contains(&first))
        }))
    }

    /// The first segments of an absolute import that reach the Python file `path`: its top package
    /// (the outermost folder with `__init__.py`) and the plain folders above it, or, outside a
    /// package, its folders and its stem.
    fn python_import_heads<'p>(&self, path: &'p str) -> Result<Vec<&'p str>, QueryError> {
        let (folders, file) = match path.rsplit_once('/') {
            Some((dir, file)) => (dir.split('/').collect::<Vec<_>>(), file),
            None => (Vec::new(), path),
        };
        let mut top = folders.len();
        while top > 0 {
            let init = format!("{}/__init__.py", folders[..top].join("/"));
            let is_package = self.with_statement(FILE_EXISTS_SQL, |stmt| {
                stmt.exists(params![init, init.replace('/', "\\")])
            })?;
            if !is_package {
                break;
            }
            top -= 1;
        }
        let stem = file.strip_suffix(".py").unwrap_or(file);
        Ok(if top == folders.len() {
            folders
                .iter()
                .copied()
                .chain(std::iter::once(stem))
                .collect()
        } else {
            folders[..=top].to_vec()
        })
    }

    /// Whether the absolute Python import `source` reaches no file of the project: `import json`
    /// beside `src/flask/json/`, whose top package is `flask`, names the standard library.
    fn python_source_is_external(&mut self, source: &str) -> Result<bool, QueryError> {
        let key = format!("python:{source}");
        if let Some(known) = self.external_sources.get(&key) {
            return Ok(*known);
        }
        let first = source.split('.').next().unwrap_or_default();
        let external = if source.starts_with('.') || !is_plain_identifier(first) {
            false
        } else {
            let paths = self.with_statement(PYTHON_FILES_NAMED_SQL, |stmt| {
                stmt.query_map(
                    params![format!("%/{first}/%"), format!("%/{first}.py")],
                    |r| r.get::<_, String>(0),
                )?
                .collect::<Result<Vec<_>, _>>()
            })?;
            let mut reached = paths.len() >= PYTHON_FILES_NAMED_CAP;
            for path in &paths {
                if reached {
                    break;
                }
                reached = self.python_import_heads(path)?.contains(&first);
            }
            !reached
        };
        self.external_sources.insert(key, external);
        Ok(external)
    }

    /// The import that binds the qualifier of the call to a module outside the index; for a
    /// JavaScript use, the first name written (`z` of `z.core.f()`, or `name` itself).
    fn external_binding(
        &mut self,
        row: &NameRow,
        name: &str,
    ) -> Result<Option<(String, String)>, QueryError> {
        let javascript = language_family(&row.language) == "js";
        let imported_head = match row.chain.as_ref() {
            Some(chain) if javascript => {
                let head = chain.first().map_or(name, String::as_str);
                self.js_binds_import(&row.path, head)?.then_some(head)
            }
            _ => None,
        };
        let Some(qualifier) = imported_head.or_else(|| row.qualifier()) else {
            return Ok(None);
        };
        if SELF_RECEIVERS.contains(&qualifier) || qualifier == "crate" {
            return Ok(None);
        }
        if row.language == "rust" && EXTERNAL_RUST_ROOTS.contains(&qualifier) {
            return Ok(Some((qualifier.to_string(), qualifier.to_string())));
        }
        let mut sources = self.with_statement(IMPORT_SOURCES_SQL, |stmt| {
            stmt.query_map(params![row.path, qualifier], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })?;
        let has_import = !sources.is_empty();
        sources.retain(|source| !source.is_empty());
        let pending_modules = self.with_statement(PENDING_IMPORT_MODULES_SQL, |stmt| {
            stmt.query_map(params![row.path, qualifier], |r| {
                r.get::<_, Option<String>>(0)
            })?
            .collect::<Result<Vec<_>, _>>()
        })?;
        for modules in pending_modules.into_iter().flatten() {
            if let Ok(parts) = serde_json::from_str::<Vec<String>>(&modules)
                && !parts.is_empty()
            {
                sources.push(parts.join("::"));
            }
        }
        if sources.is_empty() {
            return match has_import {
                true if row.language == "java" => self.java_import_outside(row, qualifier),
                true => Ok(None),
                false => self.unindexed_path_head(row),
            };
        }
        for source in &sources {
            let external = if javascript {
                self.js_source_is_external(&row.path, source)?
            } else if row.language == "python" {
                self.python_source_is_external(source)?
            } else {
                self.source_is_external(source)?
            };
            if !external {
                return Ok(None);
            }
        }
        Ok(Some((qualifier.to_string(), sources[0].clone())))
    }

    /// A Java import of the receiver from a package that no project folder holds (`Assert` after
    /// `import org.junit.Assert;`). julie writes no source for a Java import, so the package is
    /// read from the signature: the segments before the first capitalized one.
    fn java_import_outside(
        &self,
        row: &NameRow,
        qualifier: &str,
    ) -> Result<Option<(String, String)>, QueryError> {
        let signatures = self.with_statement(IMPORT_SIGNATURES_SQL, |stmt| {
            stmt.query_map(params![row.path, qualifier], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })?;
        let [signature] = signatures.as_slice() else {
            return Ok(None);
        };
        let imported = signature
            .trim()
            .trim_start_matches("import ")
            .trim()
            .trim_end_matches(';')
            .trim();
        let package: Vec<&str> = imported
            .split('.')
            .take_while(|segment| segment.starts_with(|c: char| c.is_ascii_lowercase()))
            .collect();
        if imported.starts_with("static ") || package.is_empty() {
            return Ok(None);
        }
        let folder = format!("%/{}/%", crate::queries::escape_like(&package.join("/")));
        let indexed = self.with_statement(SOURCE_FILES_SQL, |stmt| {
            stmt.exists(params![folder, folder])
        })?;
        Ok((!indexed).then(|| (qualifier.to_string(), imported.to_string())))
    }

    /// A lowercase head of a path call (`dunce::simplified()`) that no symbol, folder, or file of
    /// the project carries: a crate or package outside the index.
    fn unindexed_path_head(
        &mut self,
        row: &NameRow,
    ) -> Result<Option<(String, String)>, QueryError> {
        let Some(head) = row
            .namespace
            .first()
            .filter(|head| is_plain_identifier(head) && !SELF_RECEIVERS.contains(&head.as_str()))
            .filter(|head| *head != "crate" && head.chars().next().is_some_and(char::is_lowercase))
        else {
            return Ok(None);
        };
        let kinds = self.with_statement(TYPE_HEAD_KINDS_SQL, |stmt| {
            stmt.query_map(params![head], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })?;
        if kinds.is_empty() && self.source_is_external(head)? {
            return Ok(Some((head.clone(), head.clone())));
        }
        Ok(None)
    }

    fn source_is_external(&mut self, source: &str) -> Result<bool, QueryError> {
        if let Some(known) = self.external_sources.get(source) {
            return Ok(*known);
        }
        let runtime_library = !source.starts_with('.')
            && matches!(
                source.split([':', '.', '/']).find(|part| !part.is_empty()),
                Some("std" | "core" | "alloc")
            );
        let external = match import_segment(source).filter(|_| !runtime_library) {
            None => false,
            Some(segment) => {
                let escaped = crate::queries::escape_like(&segment);
                let (as_folder, as_file) = (format!("%/{escaped}/%"), format!("%/{escaped}.%"));
                !self.with_statement(SOURCE_FILES_SQL, |stmt| {
                    stmt.exists(params![as_folder, as_file])
                })?
            }
        };
        let external = external || runtime_library;
        self.external_sources.insert(source.to_string(), external);
        Ok(external)
    }

    /// The type of the receiver variable nearest to the caller, when its type facts agree.
    fn receiver_type(&self, row: &NameRow, receiver: &str) -> Result<Option<String>, QueryError> {
        let Some(caller) = row.caller_id.as_deref() else {
            return Ok(None);
        };
        if SELF_RECEIVERS.contains(&receiver) {
            return Ok(None);
        }
        let types = self.with_statement(RECEIVER_TYPE_SQL, |stmt| {
            stmt.query_map(params![caller, receiver, row.path], |r| {
                r.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()
        })?;
        let [only] = types.as_slice() else {
            return Ok(None);
        };
        if self.holds_any_value(row, only)?
            || self.is_type_parameter(caller, &row.language, only)?
        {
            return Ok(None);
        }
        Ok(Some(only.clone()))
    }

    /// True when a call through a receiver of type `type_name` may reach the target group. A Go
    /// type is read without its pointer and type parameters, through its project aliases, and with
    /// a package name only when that name can name the package of an owner.
    fn receiver_type_related(
        &self,
        row: &NameRow,
        type_name: &str,
        group: &[Symbol],
    ) -> Result<bool, QueryError> {
        if row.language == "go" {
            self.go_type_related(&row.path, type_name, group, 0)
        } else {
            self.owner_related(type_name, group)
        }
    }

    fn go_type_related(
        &self,
        path: &str,
        type_name: &str,
        group: &[Symbol],
        hops: usize,
    ) -> Result<bool, QueryError> {
        let bare = crate::db::go_receiver_type_name(type_name);
        let (package, name) = match bare.rsplit_once('.') {
            Some((package, name)) => (Some(package), name),
            None => (None, bare),
        };
        let named = match package {
            None => true,
            Some(package) => self.go_package_may_own(path, package, group)?,
        };
        if named && self.owner_related(name, group)? {
            return Ok(true);
        }
        if hops >= MAX_GO_ALIAS_HOPS {
            return Ok(false);
        }
        let aliases = self.with_statement(GO_ALIAS_TARGETS_SQL, |stmt| {
            stmt.query_map(params![name], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()
        })?;
        for (alias_path, target) in aliases {
            if self.go_type_related(&alias_path, &target, group, hops + 1)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether the package name `package`, written in the file `path`, can name the package of an
    /// owner of the group: through an import of that file, or as the owner's package name or
    /// folder when the file has no import of that name.
    fn go_package_may_own(
        &self,
        path: &str,
        package: &str,
        group: &[Symbol],
    ) -> Result<bool, QueryError> {
        let imports = self.with_statement(GO_IMPORT_PATHS_SQL, |stmt| {
            stmt.query_map(params![path, package], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })?;
        for member in group {
            let folder = crate::db::parent_folder(&member.path);
            let folder_name = folder
                .rsplit(['/', '\\'])
                .next()
                .filter(|name| !name.is_empty());
            let package_name = self.with_statement(GO_PACKAGE_SQL, |stmt| {
                stmt.query_row(params![member.path], |r| r.get::<_, String>(0))
                    .optional()
            })?;
            let names_owner =
                |name: &str| Some(name) == folder_name || Some(name) == package_name.as_deref();
            if imports.is_empty() {
                if names_owner(package) {
                    return Ok(true);
                }
                continue;
            }
            for signature in &imports {
                let Some(import_path) = go_import_path(signature) else {
                    return Ok(true);
                };
                if import_path.rsplit('/').take(2).any(names_owner) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// True when `type_name` is an owner of the target group or shares an inheritance chain
    /// with one, so a call through it may still reach the target.
    fn owner_related(&self, type_name: &str, group: &[Symbol]) -> Result<bool, QueryError> {
        let mut owners: Vec<String> = group
            .iter()
            .filter_map(|member| member.signature.as_deref().and_then(extended_type))
            .collect();
        for member in group {
            if let Some(definition) = self.definition_by_id(&member.symbol_id)? {
                match definition.owner_name {
                    Some(owner) => owners.push(owner),
                    None => return Ok(true),
                }
            }
        }
        if owners.iter().any(|owner| owner == type_name) {
            return Ok(true);
        }
        let type_ancestors = self.ancestor_names(type_name)?;
        for owner in &owners {
            if type_ancestors.contains(owner) || self.ancestor_names(owner)?.contains(type_name) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn ancestor_names(&self, class_name: &str) -> Result<HashSet<String>, QueryError> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut frontier = vec![class_name.to_string()];
        for _ in 0..8 {
            let mut next = Vec::new();
            for name in frontier {
                for sql in [ANCESTORS_BY_RELATIONSHIP_SQL, ANCESTORS_BY_PENDING_SQL] {
                    let bases = self.with_statement(sql, |stmt| {
                        stmt.query_map(params![name], |r| r.get::<_, String>(0))?
                            .collect::<Result<Vec<_>, _>>()
                    })?;
                    for base in bases {
                        if seen.insert(base.clone()) {
                            next.push(base);
                        }
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        Ok(seen)
    }

    /// The capitalized type a call is written through (`Vec::new()`, `Command::new()`,
    /// `Math.max()`) when that type cannot own the target: no project symbol has its name, or it
    /// is a project class that is not an owner of the target, nor related to one, nor an alias.
    fn foreign_type_head(
        &self,
        row: &NameRow,
        group: &[Symbol],
    ) -> Result<Option<String>, QueryError> {
        let Some(head) = row
            .namespace
            .last()
            .map(String::as_str)
            .or(row.receiver.as_deref())
            .filter(|head| is_plain_identifier(head) && !SELF_RECEIVERS.contains(head))
        else {
            return Ok(None);
        };
        if !head.chars().next().is_some_and(char::is_uppercase) {
            return Ok(None);
        }
        let kinds = self.with_statement(TYPE_HEAD_KINDS_SQL, |stmt| {
            stmt.query_map(params![head], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })?;
        let foreign = if kinds.is_empty() {
            true
        } else {
            let type_like = kinds.iter().any(|k| CLASS_LIKE_KINDS.contains(&k.as_str()));
            let aliased = kinds
                .iter()
                .any(|k| matches!(k.as_str(), "type" | "type_alias" | "typedef"));
            type_like && !aliased && !self.owner_related(head, group)?
        };
        Ok(foreign.then(|| head.to_string()))
    }

    fn text_outcome(
        &self,
        root: &Path,
        name: &str,
        sites: &[SiteRowOut],
    ) -> Result<TextOutcome, QueryError> {
        let hits = scan_word(root, name, TEXT_SCAN_BUDGET);
        let site_spans: HashMap<String, Vec<(usize, usize)>> = {
            let mut spans: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
            for site in sites {
                let start = site.site.start_line.unwrap_or(0);
                spans
                    .entry(site.site.path.clone())
                    .or_default()
                    .push((start, site.end_line.unwrap_or(start).max(start)));
            }
            spans
        };
        let definition_spans = definition_spans(&self.definitions(name)?, Some(&hits.hits));
        let import_spans = self.import_spans(name)?;

        let mut counts = ReferenceCounts::default();
        let mut text_only: Vec<TextOnlyLines> = Vec::new();
        for (path, line) in &hits.hits {
            counts.text_lines += 1;
            if in_spans(&site_spans, path, *line) {
                counts.site_lines += 1;
            } else if in_spans(&definition_spans, path, *line) {
                counts.definition_lines += 1;
            } else if in_spans(&import_spans, path, *line) {
                counts.import_lines += 1;
            } else {
                counts.text_only_lines += 1;
                match text_only.last_mut() {
                    Some(last) if last.path == *path => last.lines.push(*line),
                    _ => text_only.push(TextOnlyLines {
                        path: path.clone(),
                        lines: vec![*line],
                    }),
                }
            }
        }
        Ok(TextOutcome {
            counts,
            text_only,
            summary: TextScanSummary {
                files_scanned: hits.files_scanned,
                files_total: hits.files_total,
                complete: hits.complete,
                elapsed_ms: hits.elapsed.as_millis() as u64,
                skipped: None,
            },
        })
    }
}

type SiteRowOut = LabeledSite;

fn claimed_verdict(row: &NameRow, claim: &Claim, rivals: usize, builds_class: bool) -> Verdict {
    if let Some(basis) = &claim.basis {
        if builds_class {
            return Verdict::new(SiteLabel::Proven, "builds the class", true);
        }
        if row.kind == "variable_ref"
            && language_family(&row.language) == "js"
            && matches!(basis.as_str(), "same file" | "enclosing scope")
        {
            return Verdict::new(
                SiteLabel::Likely,
                format!("{basis}; the index does not record callback parameters"),
                true,
            );
        }
        return Verdict::new(SiteLabel::Proven, basis, true);
    }
    if rivals > 0 {
        return Verdict::new(
            SiteLabel::Unresolved,
            format!("{} definitions", rivals + 1),
            true,
        );
    }
    if row.has_qualifier() {
        return Verdict::new(SiteLabel::Likely, "receiver not typed", true);
    }
    Verdict::new(SiteLabel::Likely, "name only", true)
}

fn site_row(row: &NameRow, verdict: Verdict, spec: &Spec) -> SiteRow {
    let end_line = row.end_line.filter(|end| !row.paired && *end > row.line);
    SiteRow {
        site: ReferenceSite {
            from_symbol_name: row.caller_name.clone(),
            from_symbol_id: row.caller_id.clone().unwrap_or_default(),
            to_symbol_name: spec.name.clone(),
            kind: row.display_kind(),
            path: row.path.clone(),
            start_line: Some(row.site_line),
            start_column: row.site_column,
            occurrences: None,
            target: None,
            enclosing_symbol_name: None,
        },
        label: verdict.label,
        basis: verdict.basis,
        claimed: verdict.claimed,
        end_line,
        arg_count: row.arg_count,
    }
}

fn merge_site_rows(rows: Vec<SiteRow>) -> Vec<LabeledSite> {
    let mut merged: HashMap<(String, usize), SiteRow> = HashMap::new();
    let mut order: Vec<(String, usize)> = Vec::new();
    for row in rows {
        let key = (row.site.path.clone(), row.site.start_line.unwrap_or(0));
        match merged.get_mut(&key) {
            None => {
                order.push(key.clone());
                merged.insert(key, row);
            }
            Some(kept) => {
                let end = kept.end_line.max(row.end_line);
                let better = (row.label.rank(), !row.claimed, row.site.start_column)
                    < (kept.label.rank(), !kept.claimed, kept.site.start_column);
                if better {
                    *kept = row;
                }
                kept.end_line = end;
            }
        }
    }
    order
        .into_iter()
        .filter_map(|key| merged.remove(&key))
        .map(|row| LabeledSite {
            site: row.site,
            label: row.label,
            basis: row.basis,
            end_line: row.end_line,
            arg_count: row.arg_count,
        })
        .collect()
}

/// Marks the calls without a recorded receiver whose name follows a `.` after a call or index
/// result in the source (`single().getAsInt()`, `items[0]?.name()`), starts a line that
/// continues a chain (`    .readonly()`), or starts the line below a `.` (`make().` then `foo()`).
fn mark_hidden_receivers(root: &Path, rows: &mut [NameRow]) {
    let mut by_path: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, row) in rows.iter().enumerate() {
        // PHP writes string concatenation with `.`.
        if row.is_call() && !row.has_qualifier() && row.language != "php" {
            by_path.entry(row.path.clone()).or_default().push(index);
        }
    }
    for (path, indices) in by_path {
        let Ok(text) = std::fs::read_to_string(root.join(&path)) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        for index in indices {
            let row = &mut rows[index];
            let Some((before, _)) = row
                .site_column
                .and_then(|column| split_line(&lines, row.site_line, column))
            else {
                continue;
            };
            let before = before.trim_end();
            let starts_line = matches!(before.trim_start(), "." | "?.")
                || before.is_empty() && continues_chain(&lines, row.site_line);
            row.hidden_receiver = starts_line || awaits_member(before);
            row.result_receiver = if starts_line {
                code_line_above(&lines, row.site_line).is_some_and(|above| {
                    ends_with_result(above.trim_end_matches('.')) && !loads_module(above)
                })
            } else {
                awaits_member(before) && !loads_module(before)
            };
        }
    }
}

/// Whether `text` ends with a call result, an index result, or a string literal and a `.`
/// (`make().`, `items[0]?.`, `make()!.`, `'abc'.`) that waits for a member name.
fn awaits_member(text: &str) -> bool {
    text.strip_suffix('.').is_some_and(ends_with_result)
}

/// Whether `text` ends with a call result, an index result, or a string literal.
fn ends_with_result(text: &str) -> bool {
    text.trim_end_matches(['?', '!'])
        .ends_with([')', ']', '\'', '"', '`'])
}

/// Whether `text` loads a module, whose result can be a module object.
fn loads_module(text: &str) -> bool {
    text.contains("require(") || text.contains("import(")
}

/// Whether `line` declares `name` as a `const` that holds a call result or a new object
/// (`const schema = z.object({`) and loads no module.
fn const_holds_result(line: &str, name: &str) -> bool {
    let Some(at) = line.find(&format!("const {name}")) else {
        return false;
    };
    let rest = line[at + "const ".len() + name.len()..].trim_start();
    let initializer = match rest.strip_prefix(':') {
        Some(typed) => typed.split_once('=').map(|(_, value)| value),
        None => rest.strip_prefix('='),
    };
    let Some(initializer) = initializer.map(str::trim_start) else {
        return false;
    };
    if initializer.starts_with(['=', '>']) || loads_module(initializer) {
        return false;
    }
    let initializer = initializer.strip_prefix("await ").unwrap_or(initializer);
    let initializer = initializer.strip_prefix("new ").unwrap_or(initializer);
    let head = initializer
        .find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '$' | '.' | '?')))
        .unwrap_or(initializer.len());
    head > 0 && initializer[head..].trim_start().starts_with(['(', '<'])
}

/// Whether the last line of code above `line` (1-based) ends with a call or index result and a
/// `.` that waits for the name on `line`.
fn continues_chain(lines: &[&str], line: usize) -> bool {
    code_line_above(lines, line).is_some_and(awaits_member)
}

/// The last non-blank line above `line` (1-based), trimmed, unless it is a comment.
fn code_line_above<'l>(lines: &[&'l str], line: usize) -> Option<&'l str> {
    lines
        .iter()
        .take(line.saturating_sub(1))
        .rev()
        .map(|text| text.trim())
        .find(|text| !text.is_empty())
        .filter(|text| {
            !["//", "/*", "*", "#"]
                .iter()
                .any(|start| text.starts_with(start))
        })
}

/// Gives each pending or relationship call the first unused call identifier of the same caller
/// and name on or after its line and inside the caller, so a statement that starts above its
/// call reports the line of the call.
fn pair_identifiers(rows: &mut [NameRow]) {
    let mut by_caller: HashMap<String, (Vec<usize>, Vec<usize>)> = HashMap::new();
    for (index, row) in rows.iter().enumerate() {
        let Some(caller) = row.caller_id.clone() else {
            continue;
        };
        let entry = by_caller.entry(caller).or_default();
        match row.source {
            Source::Pending | Source::Relationship if row.kind == "calls" => entry.0.push(index),
            Source::Identifier if row.kind == "call" => entry.1.push(index),
            _ => {}
        }
    }
    for (calls, identifiers) in by_caller.values_mut() {
        calls.sort_by_key(|&i| (rows[i].line, rows[i].column, rows[i].source.tag(), i));
        identifiers.sort_by_key(|&i| (rows[i].line, rows[i].column, i));
        let mut used = vec![false; identifiers.len()];
        for &call in calls.iter() {
            let line = rows[call].line;
            let last = rows[call].caller_end_line.unwrap_or(usize::MAX).max(line);
            let Some(slot) = identifiers
                .iter()
                .enumerate()
                .position(|(slot, &i)| !used[slot] && (line..=last).contains(&rows[i].line))
            else {
                continue;
            };
            used[slot] = true;
            let identifier = identifiers[slot];
            let (site_line, site_column) = (rows[identifier].line, rows[identifier].column);
            let alone_on_line = identifiers
                .iter()
                .filter(|&&other| rows[other].line == site_line)
                .count()
                == 1;
            if alone_on_line && rows[call].links_enclosing_class && rows[identifier].is_super_call()
            {
                rows[call].consumed = true;
                rows[identifier].links_enclosing_class = true;
                continue;
            }
            rows[call].arg_count = rows[identifier].arg_count.filter(|_| alone_on_line);
            rows[identifier].consumed = true;
            rows[call].paired = true;
            rows[call].site_line = site_line;
            rows[call].site_column = site_column;
        }
    }
    attach_counts_by_line(rows);
}

/// Gives each pending or relationship call that has no count (a `new Foo(a)` construction) the
/// count of the one call identifier on its line.
fn attach_counts_by_line(rows: &mut [NameRow]) {
    let mut identifiers: HashMap<(String, usize), Vec<Option<u32>>> = HashMap::new();
    for row in rows.iter() {
        if row.source == Source::Identifier && row.kind == "call" {
            identifiers
                .entry((row.path.clone(), row.line))
                .or_default()
                .push(row.arg_count);
        }
    }
    for row in rows.iter_mut() {
        if row.source != Source::Identifier
            && row.is_call()
            && row.arg_count.is_none()
            && let Some(counts) = identifiers.get(&(row.path.clone(), row.line))
            && let [count] = counts.as_slice()
        {
            row.arg_count = *count;
        }
    }
}

/// Languages where julie starts a body at its first statement, which is code and not part of the
/// definition. Elsewhere a body starts at its `{` or `=`, which can share the line of the name.
const BODY_IS_FIRST_STATEMENT: &[&str] = &["python", "ruby", "lua", "fsharp", "vbnet"];

/// The lines of each definition. With `name_lines`, the sorted lines that hold the name, a
/// definition whose head lines hold no such line is a one-line definition (`@d` then
/// `def f(): return 1`), so its span keeps the first body line, which holds the name.
fn definition_spans(
    definitions: &[Definition],
    name_lines: Option<&[(String, usize)]>,
) -> HashMap<String, Vec<(usize, usize)>> {
    let mut spans: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
    for definition in definitions {
        let start = definition
            .decorated_start_line
            .unwrap_or(definition.start_line);
        let names_a_line_in = |first: usize, last: usize| {
            name_lines.is_none_or(|lines| {
                let from = lines.partition_point(|(path, line)| {
                    (path.as_str(), *line) < (definition.path.as_str(), first)
                });
                lines
                    .get(from)
                    .is_some_and(|(path, line)| *path == definition.path && *line <= last)
            })
        };
        let end = definition
            .body_start_line
            .map_or(definition.start_line, |body| {
                if BODY_IS_FIRST_STATEMENT.contains(&definition.language.as_str())
                    && names_a_line_in(start, body.saturating_sub(1))
                {
                    body.saturating_sub(1)
                } else {
                    body
                }
            })
            .max(definition.start_line);
        spans
            .entry(definition.path.clone())
            .or_default()
            .push((start, end));
    }
    spans
}

fn in_spans(spans: &HashMap<String, Vec<(usize, usize)>>, path: &str, line: usize) -> bool {
    spans.get(path).is_some_and(|list| {
        list.iter()
            .any(|&(start, end)| start <= line && line <= end)
    })
}

fn shared_leading_segments(a: &str, b: &str) -> usize {
    a.split('/')
        .zip(b.split('/'))
        .take_while(|(x, y)| x == y)
        .count()
}

fn language_family(language: &str) -> &str {
    call_language_family(language)
}

fn languages_compatible(row: &str, target: &str) -> bool {
    call_languages_compatible(row, target)
}

fn receiver_chain_statuses_sql(absent_target: bool) -> String {
    let target = "s_target";
    let target_join = if absent_target {
        format!(
            "JOIN (SELECT ?3 AS symbol_id, ?4 AS name, ?5 AS kind, ?6 AS language, ?7 AS path,
                          ?8 AS start_line, ?9 AS parent_symbol_id) {target}"
        )
    } else {
        format!("JOIN symbols {target} ON {target}.symbol_id = ?3")
    };
    let status = crate::queries::typed_receiver_chain_target_status_with_base_owner_map_sql(
        "receiver_owner.value",
        target,
        "?2",
    );
    format!(
        "SELECT receiver_owner.key,
                CASE WHEN receiver_owner.value IS NULL THEN NULL ELSE {status} END
         FROM json_each(?1) receiver_owner
         {target_join}"
    )
}

/// The last segment of an import source, `None` for a relative path or a path inside the crate.
fn import_segment(source: &str) -> Option<String> {
    let source = source.trim();
    let first_segment = source.split([':', '.']).next().unwrap_or("");
    if source.is_empty()
        || source.starts_with('.')
        || ["crate", "self", "super"].contains(&first_segment)
    {
        return None;
    }
    let normalized = source
        .replace("::", ".")
        .replace('/', ".")
        .replace('-', "_");
    normalized
        .rsplit('.')
        .find(|segment| !segment.is_empty())
        .map(str::to_string)
}

/// SQL that returns, for every pending or identifier row with the name `?1`, whether the
/// predicate of the resolver ties it to the symbol `?2` and the positive evidence for it.
fn judge_sql(conn: &Connection, absent_target: bool, include_receiver_chain: bool) -> String {
    let target = "s_target";
    let parent = "s_target_parent";
    let target_join = if absent_target {
        format!(
            "JOIN (SELECT ?2 AS symbol_id, ?5 AS name, ?6 AS kind, ?7 AS language, ?8 AS path,
                          ?9 AS start_line, ?10 AS parent_symbol_id) {target}"
        )
    } else {
        format!("JOIN symbols {target} ON {target}.symbol_id = ?2")
    };
    let typed_receiver_chain_supported = crate::queries::has_typed_receiver_chain_support(conn);
    let (receiver_chain_status, receiver_chain_status_join) =
        if typed_receiver_chain_supported && include_receiver_chain {
            let status_parameter = if absent_target { "?11" } else { "?5" };
            (
                Some("receiver_chain_status.value"),
                format!(
                    "LEFT JOIN json_each({status_parameter}) receiver_chain_status
                    ON receiver_chain_status.key = p.src || ':' || p.row_id"
                ),
            )
        } else {
            (Some("0"), String::new())
        };
    let predicate = crate::queries::pending_target_predicate_with_receiver_chain_status(
        conn,
        target,
        parent,
        receiver_chain_status,
    );
    let evidence =
        evidence_sql_with_receiver_chain_status(conn, target, parent, receiver_chain_status);
    format!(
        "SELECT p.src, p.row_id,
                CASE WHEN p.src = 'I' AND p.kind IN ('type_usage', 'member_access')
                     THEN COALESCE(({predicate}), 0) ELSE 1 END AS claimed,
                {evidence} AS basis
         FROM (
             SELECT 'P' AS src, CAST(pr.rowid AS TEXT) AS row_id,
                    pr.from_symbol_id AS from_symbol_id, pr.kind AS kind, pr.path AS path,
                    pr.start_line AS start_line, pr.start_column AS start_column,
                    pr.target_receiver AS target_receiver,
                    pr.target_namespace_json AS target_namespace_json,
                    COALESCE((SELECT language FROM files WHERE file_id = pr.file_id), '') AS language,
                    NULL AS receiver_qualifier
             FROM pending_relationships pr
             WHERE pr.target_terminal_name = ?1 AND pr.kind != 'imports'
               AND CAST(pr.rowid AS TEXT) IN (SELECT value FROM json_each(?3))
             UNION ALL
             SELECT 'I', i.identifier_id, i.containing_symbol_id, i.kind, i.path, i.start_line,
                    i.start_column,
                    CASE WHEN json_valid(i.metadata_json)
                         THEN json_extract(i.metadata_json, '$.receiver') END,
                    '[]', i.language,
                    CASE WHEN json_valid(i.metadata_json)
                         THEN json_extract(i.metadata_json, '$.receiver_qualifier') END
             FROM identifiers i
             WHERE i.name = ?1 AND i.identifier_id IN (SELECT value FROM json_each(?4))
         ) p
         LEFT JOIN symbols s_from ON s_from.symbol_id = p.from_symbol_id
         {target_join}
         LEFT JOIN symbols {parent} ON {parent}.symbol_id = {target}.parent_symbol_id
         {receiver_chain_status_join}
         WHERE {predicate}
            OR (p.src = 'I' AND p.kind IN ('type_usage', 'member_access'))"
    )
}

/// The top-level definitions named `?3` that the import binding of receiver `?2` (or of the name
/// itself when `?2` is empty) in file `?1` reaches, as `(symbol_id, path)` rows.
fn import_match_sql() -> String {
    let receiver = module_import_binding_sql("t", "p.target_receiver");
    let bare = module_import_binding_sql("t", "t.name");
    format!(
        "SELECT t.symbol_id, t.path, COALESCE(t.signature, ''), t.language
         FROM (SELECT ?1 AS path, ?2 AS target_receiver) p
         JOIN symbols t ON t.name = ?3
         WHERE t.parent_symbol_id IS NULL AND t.kind NOT IN ('import', 'export')
           AND t.language NOT IN ({docs})
           AND CASE WHEN p.target_receiver != '' THEN {receiver} ELSE {bare} END",
        docs = documentation_language_list()
    )
}

/// SQL `CASE` that names the positive evidence tying row `p` to `target`, or yields `NULL`.
fn evidence_sql_with_receiver_chain_status(
    conn: &Connection,
    target: &str,
    parent: &str,
    receiver_chain_status: Option<&str>,
) -> String {
    let ns = "json_each(CASE WHEN json_valid(p.target_namespace_json) THEN p.target_namespace_json ELSE '[]' END)";
    let has_namespace = "COALESCE(p.target_namespace_json, '[]') NOT IN ('', '[]')";
    let has_receiver = "COALESCE(p.target_receiver, '') != ''";
    let self_receiver = "p.target_receiver IN ('self', 'this', 'cls', 'Self', 'super')";
    let names_self = format!("EXISTS (SELECT 1 FROM {ns} WHERE value = 'Self')");
    let receiver_chain = class_chain_sql(target, "p.target_receiver IS NOT 'super'");
    let inherited = class_chain_sql(target, &format!("{target}.kind IN ('variable', 'field')"));
    let enclosing = enclosing_scope_sql(target);
    let implicit_languages = implicit_receiver_language_list();
    let module_receiver = module_import_binding_sql(target, "p.target_receiver");
    let module_bare = module_import_binding_sql(target, &format!("{target}.name"));
    let receiver_chain_json = "json_insert(
        CASE WHEN json_valid(p.target_namespace_json)
                   AND json_type(p.target_namespace_json) = 'array'
             THEN p.target_namespace_json ELSE '[]' END,
        '$[#]', p.target_receiver
    )";
    let typed_receiver_chain_supported = crate::queries::has_typed_receiver_chain_support(conn);
    let typed_receiver_chain_evidence = if typed_receiver_chain_supported {
        let matches_target = match receiver_chain_status {
            Some(status) => format!("({status}) = 1"),
            None => {
                let receiver_chain_owner = crate::queries::typed_receiver_chain_owner_id_sql(
                    "p.from_symbol_id",
                    "p.path",
                    "p.start_line",
                    "p.language",
                    receiver_chain_json,
                );
                crate::queries::typed_receiver_chain_matches_target_sql(
                    &receiver_chain_owner,
                    target,
                )
            }
        };
        format!(
            "WHEN (p.src = 'P' AND json_array_length({receiver_chain_json}) > 1
              AND p.target_receiver IS NOT NULL AND p.target_receiver != ''
              ) THEN CASE WHEN {matches_target} THEN 'receiver type' END"
        )
    } else {
        String::new()
    };
    let receiver_type = if crate::queries::has_type_fact_support(conn) {
        receiver_type_sql(conn, parent)
    } else {
        "0".to_string()
    };
    let receiver_type = if typed_receiver_chain_supported {
        format!(
            "CASE WHEN p.src = 'P' AND json_array_length({receiver_chain_json}) > 1
                  THEN 0 ELSE {receiver_type} END"
        )
    } else {
        receiver_type
    };
    let qualifier_reaches_owner = module_import_binding_sql(parent, "p.receiver_qualifier");
    let foreign_qualifier = format!(
        "(p.receiver_qualifier IS NOT NULL
          AND EXISTS (SELECT 1 FROM symbols q WHERE q.kind = 'import' AND q.path = p.path
                        AND q.name = p.receiver_qualifier)
          AND NOT {qualifier_reaches_owner})"
    );
    let package_proximity = path_proximity(&format!("{target}.path"), "p.path");
    let owner_in_scope = owner_binding_in_scope_sql(parent);
    let built = built_receiver_sql(conn, target)
        .map(|built| {
            format!(
                "WHEN {has_receiver} AND {target}.parent_symbol_id IS NOT NULL AND {built}
                      THEN 'builds the class'"
            )
        })
        .unwrap_or_default();
    format!(
        "CASE
            {typed_receiver_chain_evidence}
            WHEN {has_namespace} AND {parent}.name IS NOT NULL
                 AND EXISTS (SELECT 1 FROM {ns} WHERE value = {parent}.name) AND {owner_in_scope}
                 THEN 'qualified path'
            WHEN {has_namespace} AND {target}.parent_symbol_id IS NULL THEN 'qualified path'
            WHEN {target}.parent_symbol_id IS NOT NULL AND {names_self}
                 AND s_from.parent_symbol_id = {target}.parent_symbol_id THEN 'class chain'
            WHEN {target}.parent_symbol_id IS NOT NULL
                 AND ({self_receiver} OR {names_self}) AND {receiver_chain} THEN 'class chain'
            WHEN {has_receiver} AND {parent}.name = p.target_receiver AND NOT {foreign_qualifier}
                 AND {owner_in_scope}
                 THEN 'qualified path'
            WHEN {has_receiver} AND {target}.parent_symbol_id IS NULL AND {module_receiver}
                 THEN 'import'
            WHEN {has_receiver} AND {target}.parent_symbol_id IS NOT NULL AND {receiver_type}
                 THEN 'receiver type'
            {built}
            WHEN NOT ({has_receiver}) AND NOT ({has_namespace}) THEN
                CASE
                    WHEN {target}.parent_symbol_id IS NULL AND {target}.path = p.path
                         THEN 'same file'
                    WHEN {target}.parent_symbol_id = p.from_symbol_id THEN 'enclosing scope'
                    WHEN {target}.parent_symbol_id IS NOT NULL AND {enclosing}
                         THEN 'enclosing scope'
                    WHEN {target}.parent_symbol_id IS NOT NULL AND p.language IN ({implicit_languages})
                         AND (s_from.parent_symbol_id = {target}.parent_symbol_id OR {inherited})
                         THEN 'class chain'
                    WHEN {target}.parent_symbol_id IS NULL AND {module_bare} THEN 'import'
                    WHEN {target}.parent_symbol_id IS NULL AND {target}.language IN ('go', 'java')
                         AND {target}.language = p.language AND ({package_proximity}) >= 1
                         THEN 'same package'
                END
         END"
    )
}

/// The import path of a Go import row (`import g "fx/geom"`).
fn go_import_path(signature: &str) -> Option<&str> {
    let (_, rest) = signature.split_once('"')?;
    rest.split_once('"').map(|(path, _)| path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Workspace, find_julie_extract_binary, open_read_only, safe_tempdir, scan_workspace,
    };

    fn receiver_chain_accounting(conn: &Connection) -> Accounting<'_> {
        conn.execute_batch(
            "CREATE TABLE symbols (
                symbol_id TEXT, path TEXT, language TEXT, name TEXT, kind TEXT,
                parent_symbol_id TEXT, start_line INTEGER, metadata_json TEXT,
                signature TEXT, file_id TEXT
            );
            CREATE TABLE type_facts (symbol_id TEXT, resolved_type TEXT, language TEXT);
            CREATE TABLE relationships (from_symbol_id TEXT, to_symbol_id TEXT, kind TEXT);
            CREATE TABLE pending_relationships (
                from_symbol_id TEXT, target_terminal_name TEXT, kind TEXT, path TEXT,
                start_line INTEGER, file_id TEXT, target_receiver TEXT, target_namespace_json TEXT
            );
            CREATE TABLE files (file_id TEXT, language TEXT);",
        )
        .unwrap();
        Accounting::new(conn, Path::new("."), Duration::from_secs(1))
    }

    fn receiver_chain_root_evidence(owner_id: &str) -> ReceiverChainOwnerEvidence {
        ReceiverChainOwnerEvidence {
            owners: HashMap::from([("P:1".to_string(), Some(owner_id.to_string()))]),
            pending_base_owners: HashMap::new(),
            incomplete: None,
            edge_rows_examined: 0,
        }
    }

    #[test]
    fn receiver_chain_ancestry_stops_at_edge_limit_plus_one() {
        let conn = Connection::open_in_memory().unwrap();
        let accounting = receiver_chain_accounting(&conn);
        for index in 0..20 {
            conn.execute(
                "INSERT INTO relationships VALUES ('root', ?1, 'extends')",
                params![format!("base-{index}")],
            )
            .unwrap();
        }
        let mut evidence = receiver_chain_root_evidence("root");

        accounting
            .extend_receiver_chain_ancestry(
                &mut evidence,
                ReceiverChainLimits {
                    owners: 100,
                    edges: 2,
                    depth: 8,
                },
            )
            .unwrap();

        assert_eq!(evidence.incomplete, Some(ReceiverChainLimit::Edges));
        assert_eq!(evidence.edge_rows_examined, 3);
        let statuses = accounting
            .receiver_chain_target_statuses("target", &evidence)
            .unwrap();
        assert_eq!(statuses.get("P:1"), Some(&None));
    }

    #[test]
    fn receiver_chain_ancestry_marks_unexpanded_depth_incomplete_and_keeps_leaves_complete() {
        let conn = Connection::open_in_memory().unwrap();
        let accounting = receiver_chain_accounting(&conn);
        conn.execute_batch(
            "INSERT INTO relationships VALUES ('root', 'middle', 'extends');
            INSERT INTO relationships VALUES ('middle', 'leaf', 'implements');",
        )
        .unwrap();
        let mut evidence = receiver_chain_root_evidence("root");

        accounting
            .extend_receiver_chain_ancestry(
                &mut evidence,
                ReceiverChainLimits {
                    owners: 10,
                    edges: 10,
                    depth: 1,
                },
            )
            .unwrap();

        assert_eq!(evidence.incomplete, Some(ReceiverChainLimit::Depth));

        let leaf_conn = Connection::open_in_memory().unwrap();
        let leaf_accounting = receiver_chain_accounting(&leaf_conn);
        leaf_conn
            .execute(
                "INSERT INTO relationships VALUES ('root', 'middle', 'extends')",
                [],
            )
            .unwrap();
        let mut leaf_evidence = receiver_chain_root_evidence("root");

        leaf_accounting
            .extend_receiver_chain_ancestry(
                &mut leaf_evidence,
                ReceiverChainLimits {
                    owners: 10,
                    edges: 10,
                    depth: 1,
                },
            )
            .unwrap();

        assert_eq!(leaf_evidence.incomplete, None);
    }

    #[test]
    fn receiver_chain_ancestry_marks_owner_limit_incomplete() {
        let conn = Connection::open_in_memory().unwrap();
        let accounting = receiver_chain_accounting(&conn);
        conn.execute(
            "INSERT INTO relationships VALUES ('root', 'middle', 'extends')",
            [],
        )
        .unwrap();
        let mut evidence = receiver_chain_root_evidence("root");

        accounting
            .extend_receiver_chain_ancestry(
                &mut evidence,
                ReceiverChainLimits {
                    owners: 1,
                    edges: 10,
                    depth: 8,
                },
            )
            .unwrap();

        assert_eq!(evidence.incomplete, Some(ReceiverChainLimit::Owners));
    }

    #[test]
    fn receiver_chain_limit_notes_surface_each_unknown_coverage_once() {
        for (limit, expected) in [
            (
                ReceiverChainLimit::Owners,
                "receiver-chain ancestry exceeded the 8,192-owner limit; affected sites stayed unresolved",
            ),
            (
                ReceiverChainLimit::Edges,
                "receiver-chain ancestry exceeded the 8,192-edge limit; affected sites stayed unresolved",
            ),
            (
                ReceiverChainLimit::Depth,
                "receiver-chain ancestry exceeded the 8-level limit; affected sites stayed unresolved",
            ),
        ] {
            let mut cannot_see = Vec::new();
            record_receiver_chain_limit_note(&mut cannot_see, Some(limit));
            record_receiver_chain_limit_note(&mut cannot_see, Some(limit));

            assert_eq!(cannot_see, [expected]);
        }
    }

    #[test]
    fn judge_sql_keeps_typed_receiver_status_bound_and_compact() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE symbols (
                symbol_id TEXT, path TEXT, language TEXT, name TEXT, kind TEXT,
                parent_symbol_id TEXT, start_line INTEGER, metadata_json TEXT, signature TEXT
            );
            CREATE TABLE type_facts (symbol_id TEXT, resolved_type TEXT, language TEXT);
            CREATE TABLE relationships (from_symbol_id TEXT, to_symbol_id TEXT, kind TEXT);
            CREATE TABLE files (file_id TEXT, language TEXT);
            CREATE TABLE pending_relationships (
                from_symbol_id TEXT, target_terminal_name TEXT, kind TEXT, path TEXT,
                start_line INTEGER, file_id TEXT, target_receiver TEXT, target_namespace_json TEXT
            );",
        )
        .unwrap();

        let plain = judge_sql(&conn, false, false);
        let with_chain = judge_sql(&conn, false, true);

        assert!(!plain.contains("receiver_chain_status"));
        assert!(
            plain.len() < 200_000,
            "plain judge SQL grew to {} bytes",
            plain.len()
        );
        assert!(with_chain.contains("LEFT JOIN json_each(?5) receiver_chain_status"));
        assert!(
            with_chain.len() <= plain.len() + 10_000,
            "typed receiver-chain SQL should bind status without expanding the proof: plain={} chain={}",
            plain.len(),
            with_chain.len()
        );
    }

    fn report_past_the_time_limit(
        files: &[(&str, &str)],
        name: &str,
        path: Option<&str>,
    ) -> ReferenceReport {
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");
        let temp = safe_tempdir();
        let root = temp.path();
        for (relative, text) in files {
            let full = root.join(relative);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, text).unwrap();
        }
        let db = root.join(".code-kb").join("artifact.db");
        scan_workspace(&Workspace::new(root.to_path_buf()), &db, true).unwrap();
        let conn = open_read_only(&db).unwrap();
        let target = select_reference_target(&conn, name, path).unwrap();
        report_within(&conn, root, &target, Duration::ZERO).unwrap()
    }

    #[test]
    fn sites_past_the_time_limit_keep_the_label_their_name_gives() {
        let report = report_past_the_time_limit(
            &[(
                "a.py",
                "def helper():\n    return 1\n\n\ndef use():\n    return helper()\n",
            )],
            "helper",
            None,
        );

        let labels: Vec<(SiteLabel, &str)> = report
            .sites
            .iter()
            .map(|site| (site.label, site.basis.as_str()))
            .collect();
        assert_eq!(
            labels,
            [(
                SiteLabel::Likely,
                "name only; not checked within the time limit"
            )]
        );
        assert!(
            report
                .cannot_see
                .iter()
                .any(|note| note == "1 of 1 sites were not checked within the 0 s time limit"),
            "{:?}",
            report.cannot_see
        );
    }

    #[test]
    fn sites_past_the_time_limit_count_only_the_definitions_that_can_reach_them() {
        let report = report_past_the_time_limit(
            &[
                (
                    "a/name.go",
                    "package a\n\nfunc Name() string {\n\treturn \"a\"\n}\n",
                ),
                (
                    "b/cmd.go",
                    "package b\n\ntype Command struct {\n\tName string\n}\n",
                ),
                (
                    "c/use.go",
                    "package c\n\nfunc call(x any) string {\n\treturn x.Name()\n}\n\nfunc read(x any) string {\n\treturn x.Name\n}\n",
                ),
            ],
            "Name",
            Some("a/name.go"),
        );

        let labels: Vec<(usize, SiteLabel, &str)> = report
            .sites
            .iter()
            .map(|site| {
                (
                    site.site.start_line.unwrap_or(0),
                    site.label,
                    site.basis.as_str(),
                )
            })
            .collect();
        assert_eq!(
            labels,
            [
                (
                    4,
                    SiteLabel::Likely,
                    "name only; not checked within the time limit"
                ),
                (
                    8,
                    SiteLabel::Unresolved,
                    "2 definitions; not checked within the time limit"
                ),
            ]
        );
    }

    #[test]
    fn legacy_judge_sql_prepares_without_type_fact_support() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE symbols (
                symbol_id TEXT, path TEXT, language TEXT, name TEXT, kind TEXT,
                parent_symbol_id TEXT, start_line INTEGER, end_line INTEGER,
                body_start_line INTEGER, metadata_json TEXT, signature TEXT
            );
            CREATE TABLE files (file_id TEXT, language TEXT);
            CREATE TABLE relationships (from_symbol_id TEXT, to_symbol_id TEXT, kind TEXT);
            CREATE TABLE pending_relationships (
                from_symbol_id TEXT, target_terminal_name TEXT, kind TEXT, path TEXT,
                start_line INTEGER, start_column INTEGER, end_line INTEGER, file_id TEXT,
                target_receiver TEXT, target_namespace_json TEXT
            );
            CREATE TABLE identifiers (
                identifier_id TEXT, containing_symbol_id TEXT, name TEXT, kind TEXT,
                path TEXT, language TEXT, start_line INTEGER, start_column INTEGER,
                metadata_json TEXT
            );",
        )
        .unwrap();

        let sql = judge_sql(&conn, false, true);
        assert!(!sql.contains("receiver_chain_status"));
        conn.prepare(&sql).unwrap();
    }
}
