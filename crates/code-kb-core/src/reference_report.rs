use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use rusqlite::{Connection, OptionalExtension, Statement, params};

use crate::models::{
    LabeledSite, ReferenceCounts, ReferenceReport, ReferenceSite, ReferenceTarget, SiteLabel,
    Symbol, TextOnlyLines, TextScanSummary,
};
use crate::queries::{
    QueryError, built_receiver_sql, class_chain_sql, constructed_class, direct_identifier_rows,
    documentation_language_list, enclosing_scope_sql, get_symbol_by_name,
    grouped_member_access_rows, has_column, implicit_receiver_language_list, import_sites,
    inherited_writes_among, module_import_binding_sql, path_proximity, pending_target_predicate,
    populate_lambda_enclosing_names, receiver_type_sql, symbol_not_found_parts,
};
use crate::text_scan::scan_word;

#[path = "js_modules.rs"]
mod js_modules;

const RESOLVER_ROW_CAP: usize = 5_000;
const OTHER_DEFINITION_CAP: usize = 12;
/// How long the per-site checks of one report may run; later sites keep the label their name gives.
const ACCOUNTING_TIME_LIMIT: Duration = Duration::from_secs(3);
/// Rows per judge query, so the time limit is checked between queries.
const JUDGE_CHUNK: usize = 2_000;
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

fn report_within(
    conn: &Connection,
    root: &Path,
    target: &Symbol,
    time_limit: Duration,
) -> Result<ReferenceReport, QueryError> {
    let class = constructor_class(conn, target)?;
    let mut specs = vec![Spec {
        name: target.name.clone(),
        group: overload_group(conn, target)?,
        calls_only: class.is_some(),
        builds_class: false,
    }];
    if let Some(class) = &class {
        specs.push(Spec {
            name: class.name.clone(),
            group: vec![class.clone()],
            calls_only: true,
            builds_class: true,
        });
    }

    let mut accounting = Accounting::new(conn, root, time_limit);
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

    cannot_see.splice(0..0, cannot_see_for_language(&target.language));
    Ok(ReferenceReport {
        target: report_target(conn, target, definitions_with_name)?,
        counts,
        sites,
        text_only: text.text_only,
        text_scan: text.summary,
        cannot_see,
        imports: import_sites(conn, &target.name, Some(&target.path))?,
    })
}

fn cannot_see_for_language(language: &str) -> Vec<String> {
    let mut items = Vec::new();
    match language {
        "python" => items.push("calls through getattr() or names built at runtime".to_string()),
        "javascript" | "typescript" | "tsx" | "jsx" | "vue" => items
            .push("calls through computed property names or values built at runtime".to_string()),
        "rust" => items.push("calls inside macro bodies".to_string()),
        "java" | "csharp" | "kotlin" => {
            items.push("calls through reflection or dependency injection".to_string());
        }
        _ => {}
    }
    items.push(
        "calls through dynamic dispatch reach overrides that this list does not show".to_string(),
    );
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

/// The selected symbol and its overloads: the symbols with its name, owner, kind, and file.
fn overload_group(conn: &Connection, target: &Symbol) -> Result<Vec<Symbol>, QueryError> {
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
            s.body_start_line, s.parent_symbol_id, par.name, par.kind
     FROM symbols s LEFT JOIN symbols par ON par.symbol_id = s.parent_symbol_id
     WHERE s.symbol_id = ?1";

const SHADOW_IN_CHAIN_SQL: &str = "WITH RECURSIVE chain(symbol_id, depth) AS (
         SELECT ?1, 0
         UNION ALL
         SELECT s.parent_symbol_id, chain.depth + 1
         FROM symbols s JOIN chain ON s.symbol_id = chain.symbol_id
         WHERE s.parent_symbol_id IS NOT NULL AND chain.depth < 32
     )
     SELECT d.symbol_id
     FROM chain
     JOIN symbols scope ON scope.symbol_id = chain.symbol_id
     JOIN symbols d ON d.parent_symbol_id = chain.symbol_id
     WHERE d.name = ?2 AND d.path = ?3
       AND (d.kind IN ('variable', 'parameter', 'function', 'constant', 'class')
            OR ?4 AND d.kind IN ('type', 'interface', 'enum'))
       AND (chain.depth = 0 OR scope.kind IN ('function', 'method', 'constructor'))
     ORDER BY chain.depth, d.start_line
     LIMIT 1";

const SHADOW_TOP_LEVEL_SQL: &str = "SELECT symbol_id FROM symbols
     WHERE name = ?1 AND path = ?2 AND parent_symbol_id IS NULL
       AND kind NOT IN ('import', 'export', 'module', 'namespace')
     ORDER BY start_line LIMIT 1";

const IMPORT_SOURCES_SQL: &str = "SELECT COALESCE(json_extract(metadata_json, '$.source'), '')
     FROM symbols
     WHERE kind = 'import' AND path = ?1 AND json_valid(metadata_json)
       AND (name = ?2 OR json_extract(metadata_json, '$.alias') = ?2
            OR json_extract(metadata_json, '$.local_name') = ?2)";

const PENDING_IMPORT_MODULES_SQL: &str = "SELECT target_namespace_json FROM pending_relationships
     WHERE kind = 'imports' AND path = ?1 AND target_terminal_name = ?2";

const FILE_EXISTS_SQL: &str = "SELECT 1 FROM files WHERE path = ?1 OR path = ?2 LIMIT 1";

const SOURCE_FILES_SQL: &str = "SELECT 1 FROM files
     WHERE '/' || replace(replace(path, '\\', '/'), '-', '_') LIKE ?1 ESCAPE '\\'
        OR '/' || replace(replace(path, '\\', '/'), '-', '_') LIKE ?2 ESCAPE '\\'
     LIMIT 1";

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

struct Spec {
    name: String,
    group: Vec<Symbol>,
    calls_only: bool,
    builds_class: bool,
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
    /// The names written before the used name (`["z", "core"]` for `z.core.f()`), `None` when
    /// the row does not say.
    chain: Option<Vec<String>>,
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

    fn is_call(&self) -> bool {
        matches!(self.kind.as_str(), "calls" | "call" | "instantiates")
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
}

/// A JavaScript method assigned to a property (`req.accepts = function`): no scope binds its name,
/// so a bare use of that name reads an import or a global.
fn is_property_method(symbol: &Symbol) -> bool {
    symbol.kind == "method"
        && symbol.parent_symbol_id.is_none()
        && language_family(&symbol.language) == "js"
}

/// The type a C# extension method extends: `IEnumerable` for `Foo(this IEnumerable<T> source)`.
fn extended_type(signature: &str) -> Option<String> {
    let (_, rest) = signature.split_once("(this ")?;
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
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
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
    local: bool,
}

impl Definition {
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
}

struct Claim {
    basis: Option<String>,
    candidate: bool,
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

struct Accounting<'a> {
    conn: &'a Connection,
    root: &'a Path,
    /// The judge stops here, so the cheaper checks of the judged rows still fit in the limit.
    judge_deadline: Instant,
    deadline: Instant,
    judge_stmt: RefCell<Option<Statement<'a>>>,
    import_match_stmt: Option<Statement<'a>>,
    statements: RefCell<HashMap<&'static str, Statement<'a>>>,
    external_sources: HashMap<String, bool>,
    /// Definitions an import reaches, by caller file, receiver, and name.
    import_matches: HashMap<(String, String, String), Vec<(String, String)>>,
    js: js_modules::JsCaches,
}

impl<'a> Accounting<'a> {
    fn new(conn: &'a Connection, root: &'a Path, time_limit: Duration) -> Self {
        Accounting {
            conn,
            root,
            judge_deadline: Instant::now() + time_limit / 2,
            deadline: Instant::now() + time_limit,
            judge_stmt: RefCell::new(None),
            import_match_stmt: None,
            statements: RefCell::new(HashMap::new()),
            external_sources: HashMap::new(),
            import_matches: HashMap::new(),
            js: js_modules::JsCaches::default(),
        }
    }

    fn out_of_time(&self) -> bool {
        Instant::now() >= self.deadline
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

    fn definitions(&self, name: &str) -> Result<Vec<Definition>, QueryError> {
        let decorated_start = if has_column(self.conn, "structural_facts", "end_byte")
            && has_column(self.conn, "symbols", "end_byte")
        {
            "(SELECT MIN(f.start_line) FROM structural_facts f
              WHERE f.path = s.path AND f.pattern_id LIKE '%.decorated_definition.%'
                AND f.end_byte = s.end_byte AND f.start_byte < s.start_byte)"
        } else {
            "NULL"
        };
        let mut stmt = self.conn.prepare(&format!(
            "SELECT s.symbol_id, s.name, s.kind, s.language, s.path, s.start_line,
                    s.body_start_line, s.parent_symbol_id, par.name, par.kind, {decorated_start},
                    COALESCE(s.signature, '')
             FROM symbols s LEFT JOIN symbols par ON par.symbol_id = s.parent_symbol_id
             WHERE s.name = ?1 AND s.kind NOT IN ('import', 'export')
               AND s.language NOT IN ({})",
            documentation_language_list()
        ))?;
        let rows = stmt.query_map(params![name], |row| {
            if is_test_label(&row.get::<_, String>(11)?) {
                return Ok(None);
            }
            let kind: String = row.get(2)?;
            let parent_kind: Option<String> = row.get(9)?;
            let parent_name: Option<String> = row.get(8)?;
            let local = kind == "parameter"
                || kind == "variable"
                    && parent_kind
                        .as_deref()
                        .is_some_and(|k| matches!(k, "function" | "method" | "constructor"));
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
                local,
            }))
        })?;
        let mut definitions: Vec<Definition> = rows
            .filter_map(Result::transpose)
            .collect::<Result<_, _>>()?;
        let writes =
            inherited_writes_among(self.conn, definitions.iter().map(|d| d.symbol_id.as_str()))?;
        definitions.retain(|d| !writes.contains(&d.symbol_id));
        Ok(definitions)
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
        let k = definitions.iter().filter(|d| !d.local).count();
        let target = &spec.group[0];

        let mut rows = self.load_rows(&spec.name, &definitions, spec.calls_only)?;
        pair_identifiers(&mut rows);
        rows.retain(|row| !row.consumed);
        mark_hidden_receivers(self.root, &mut rows);
        js_modules::complete_js_chains(self.root, &mut rows);

        let mut order: Vec<usize> = (0..rows.len()).collect();
        order.sort_by(|&a, &b| {
            let (a, b) = (&rows[a], &rows[b]);
            shared_leading_segments(&b.path, &target.path)
                .cmp(&shared_leading_segments(&a.path, &target.path))
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.line.cmp(&b.line))
        });
        let (claims, judged) = self.claims(spec, &rows, &order, cannot_see)?;
        let mut unchecked: Vec<usize> = order[judged..].to_vec();
        let mut verdicts: HashMap<String, Verdict> = HashMap::new();
        let mut undecided: Vec<usize> = Vec::new();
        for (position, &index) in order[..judged].iter().enumerate() {
            if self.out_of_time() {
                unchecked.extend_from_slice(&order[position..judged]);
                break;
            }
            match self.decide_cheap(&rows[index], spec, target, &group_ids, k, &claims)? {
                Some(verdict) => {
                    verdicts.insert(rows[index].key.clone(), verdict);
                }
                None => undecided.push(index),
            }
        }

        let mut others: Vec<&Definition> = definitions
            .iter()
            .filter(|d| !d.local && !group_ids.contains(d.symbol_id.as_str()))
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
                let judged = self.judge(&spec.name, &other.symbol_id, &chunk_rows)?;
                for &index in chunk {
                    let row = &rows[index];
                    if row.hidden_receiver {
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

        for index in undecided {
            let row = &rows[index];
            let verdict = match claims.get(&row.key) {
                Some(claim) => claimed_verdict(row, claim, k, spec.builds_class),
                None if self.out_of_time() => {
                    unchecked.push(index);
                    continue;
                }
                None => self.decide_late(row, &spec.group, &others, k)?,
            };
            verdicts.insert(row.key.clone(), verdict);
        }
        for index in unchecked {
            let verdict = if others.is_empty() {
                Verdict::new(
                    SiteLabel::Likely,
                    format!("name only; {NOT_CHECKED}"),
                    false,
                )
            } else {
                Verdict::new(
                    SiteLabel::Unresolved,
                    format!("{k} definitions; {NOT_CHECKED}"),
                    false,
                )
            };
            verdicts.insert(rows[index].key.clone(), verdict);
        }

        for row in &rows {
            let Some(verdict) = verdicts.remove(&row.key) else {
                continue;
            };
            out.push(site_row(row, verdict, spec));
        }
        if !spec.calls_only {
            for member in &spec.group {
                for grouped in
                    grouped_member_access_rows(self.conn, &member.symbol_id, RESOLVER_ROW_CAP)?
                {
                    out.push(SiteRow {
                        end_line: None,
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

    fn load_rows(
        &self,
        name: &str,
        definitions: &[Definition],
        calls_only: bool,
    ) -> Result<Vec<NameRow>, QueryError> {
        let docs = documentation_language_list();
        let mut rows = Vec::new();

        let mut stmt = self.conn.prepare(&format!(
            "SELECT p.rowid, p.from_symbol_id, COALESCE(s.name, ''), p.kind, p.path,
                    COALESCE(f.language, s.language, ''), p.start_line, p.start_column,
                    p.end_line, p.target_receiver, p.target_namespace_json
             FROM pending_relationships p
             LEFT JOIN symbols s ON s.symbol_id = p.from_symbol_id
             LEFT JOIN files f ON f.file_id = p.file_id
             WHERE p.target_terminal_name = ?1 AND p.kind != 'imports'
               AND COALESCE(f.language, s.language, '') NOT IN ({docs})"
        ))?;
        let mut query = stmt.query(params![name])?;
        while let Some(row) = query.next()? {
            let namespace: Vec<String> = row
                .get::<_, Option<String>>(10)?
                .and_then(|json| serde_json::from_str(&json).ok())
                .unwrap_or_default();
            let receiver: Option<String> = row.get(9)?;
            let chain = namespace
                .iter()
                .chain(receiver.as_ref().filter(|r| !r.is_empty()))
                .cloned()
                .collect();
            rows.push(NameRow {
                key: format!("P:{}", row.get::<_, i64>(0)?),
                source: Source::Pending,
                caller_id: row.get(1)?,
                caller_name: row.get(2)?,
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
                chain: Some(chain),
            });
        }

        let mut stmt = self.conn.prepare(&format!(
            "SELECT r.rowid, r.from_symbol_id, COALESCE(s.name, ''), r.kind, r.path,
                    COALESCE(f.language, s.language, ''), r.start_line, r.start_column,
                    r.end_line, r.to_symbol_id
             FROM relationships r
             JOIN symbols t ON t.symbol_id = r.to_symbol_id
             LEFT JOIN symbols s ON s.symbol_id = r.from_symbol_id
             LEFT JOIN files f ON f.file_id = r.file_id
             WHERE t.name = ?1 AND r.kind != 'imports'
               AND COALESCE(f.language, s.language, '') NOT IN ({docs})"
        ))?;
        let mut query = stmt.query(params![name])?;
        while let Some(row) = query.next()? {
            let line = row.get::<_, Option<i64>>(6)?.unwrap_or(0) as usize;
            rows.push(NameRow {
                key: format!("R:{}", row.get::<_, i64>(0)?),
                source: Source::Relationship,
                caller_id: row.get(1)?,
                caller_name: row.get(2)?,
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
                chain: None,
            });
        }

        if crate::queries::has_table(self.conn, "identifiers") {
            let mut stmt = self.conn.prepare(&format!(
                "SELECT i.identifier_id, i.containing_symbol_id, COALESCE(s.name, ''), i.kind,
                        i.path, i.language, i.start_line, i.start_column,
                        CASE WHEN json_valid(i.metadata_json)
                             THEN json_extract(i.metadata_json, '$.receiver') END,
                        CASE WHEN json_valid(i.metadata_json)
                             THEN json_extract(i.metadata_json, '$.receiver_qualifier') END
                 FROM identifiers i
                 LEFT JOIN symbols s ON s.symbol_id = i.containing_symbol_id
                 WHERE i.name = ?1 AND COALESCE(s.kind, '') != 'import'
                   AND i.language NOT IN ({docs})"
            ))?;
            let mut query = stmt.query(params![name])?;
            while let Some(row) = query.next()? {
                let line = row.get::<_, i64>(6)? as usize;
                let kind: String = row.get(3)?;
                let receiver: Option<String> = row.get(8)?;
                let qualifier: Option<String> = row.get(9)?;
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
                    chain,
                });
            }
        }

        let import_spans = self.import_spans(name)?;
        let definition_spans = definition_spans(definitions);
        rows.retain(|row| {
            if calls_only && !row.is_call() {
                return false;
            }
            if in_spans(&import_spans, &row.path, row.line) {
                return false;
            }
            !(row.source == Source::Identifier && in_spans(&definition_spans, &row.path, row.line))
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
        let mut stmt = self.conn.prepare(&format!(
            "SELECT path, start_line, end_line FROM symbols
             WHERE name = ?1 AND kind = 'import' AND language NOT IN ({docs})"
        ))?;
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
        Ok(spans)
    }

    /// Judges `rows` (pending and identifier rows; relationship rows need no judge) for `target_id`.
    fn judge(
        &self,
        name: &str,
        target_id: &str,
        rows: &[&NameRow],
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
        let mut slot = self.judge_stmt.borrow_mut();
        if slot.is_none() {
            *slot = Some(self.conn.prepare(&judge_sql(self.conn))?);
        }
        let stmt = slot.as_mut().expect("judge statement was just prepared");
        let mut rows = stmt.query(params![name, target_id, pending, identifiers])?;
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
        for member in &spec.group {
            let resolver_ids: HashMap<String, bool> = direct_identifier_rows(
                self.conn,
                &spec.name,
                Some(&member.symbol_id),
                RESOLVER_ROW_CAP,
            )?
            .into_iter()
            .map(|row| {
                (
                    format!("I:{}", row.identifier_id),
                    row.site.kind.ends_with("(candidate)"),
                )
            })
            .collect();
            if resolver_ids.len() >= RESOLVER_ROW_CAP {
                cannot_see.push(format!(
                    "the resolver listed {RESOLVER_ROW_CAP} identifier rows and may hold more; rows past that limit have no resolver evidence"
                ));
            }
            resolvers.push(resolver_ids);
        }
        let mut judged_rows = 0;
        for chunk in order.chunks(JUDGE_CHUNK) {
            if Instant::now() >= self.judge_deadline {
                break;
            }
            let chunk_rows: Vec<&NameRow> = chunk
                .iter()
                .map(|&index| &rows[index])
                .filter(|row| !row.hidden_receiver)
                .collect();
            for (member, resolver_ids) in spec.group.iter().zip(&resolvers) {
                let judged = self.judge(&spec.name, &member.symbol_id, &chunk_rows)?;
                for row in chunk_rows.iter().copied() {
                    let resolver_claim = resolver_ids.get(&row.key).copied();
                    let judged_row = judged.get(&row.key);
                    let claimed = match row.source {
                        Source::Relationship => row
                            .to_symbol_id
                            .as_deref()
                            .is_some_and(|id| id == member.symbol_id),
                        Source::Identifier
                            if matches!(row.kind.as_str(), "type_usage" | "member_access") =>
                        {
                            resolver_claim.is_some()
                        }
                        _ => judged_row.is_some_and(|j| j.claimed),
                    };
                    if !claimed {
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
                    let candidate = resolver_claim.unwrap_or(false);
                    let entry = claims.entry(row.key.clone()).or_insert(Claim {
                        basis: None,
                        candidate,
                    });
                    if entry.basis.is_none() {
                        entry.basis = basis;
                        entry.candidate = candidate;
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
        k: usize,
        claims: &HashMap<String, Claim>,
    ) -> Result<Option<Verdict>, QueryError> {
        if !languages_compatible(&row.language, &target.language) {
            return Ok(Some(Verdict::new(
                SiteLabel::Excluded,
                "other language",
                false,
            )));
        }
        if let Some(shadow) = self.nearest_shadow(row, &spec.name, group_ids)? {
            return Ok(Some(Verdict::new(
                SiteLabel::Excluded,
                shadow.resolves_to(),
                false,
            )));
        }
        if row.is_bare_use() && spec.group.iter().all(is_property_method) {
            return Ok(Some(self.bare_use_of_property_method(row, &spec.name)?));
        }
        if let Some(verdict) = self.js_verdict(row, spec)? {
            return Ok(Some(verdict));
        }
        if let Some(claim) = claims.get(&row.key)
            && let Some(basis) = &claim.basis
        {
            if basis == IMPORT_BASIS {
                let others = self.other_import_matches(row, &spec.name, group_ids)?;
                if others > 0 {
                    return Ok(Some(Verdict::new(
                        SiteLabel::Unresolved,
                        format!("the import matches {} definitions", others + 1),
                        true,
                    )));
                }
            }
            return Ok(Some(claimed_verdict(row, claim, k, spec.builds_class)));
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
            && let Some(other) = self.definition_by_id(to)?
        {
            return Ok(Some(Verdict::new(
                SiteLabel::Excluded,
                other.resolves_to(),
                false,
            )));
        }
        if let Some((binding, source)) = self.external_binding(row)? {
            return Ok(Some(Verdict::new(
                SiteLabel::Excluded,
                format!("receiver `{binding}` is imported from `{source}`"),
                false,
            )));
        }
        Ok(None)
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
        others: &[&Definition],
        k: usize,
    ) -> Result<Verdict, QueryError> {
        if let Some(receiver) = row.qualifier().filter(|_| row.namespace.len() <= 1)
            && let Some(type_name) = self.receiver_type(row, receiver)?
            && !self.owner_related(&type_name, group)?
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
        if others.is_empty() {
            Ok(Verdict::new(SiteLabel::Likely, "name only", false))
        } else {
            Ok(Verdict::new(
                SiteLabel::Unresolved,
                format!("{k} definitions"),
                false,
            ))
        }
    }

    fn definition_by_id(&self, symbol_id: &str) -> Result<Option<Definition>, QueryError> {
        self.with_statement(DEFINITION_BY_ID_SQL, |stmt| {
            stmt.query_row(params![symbol_id], |row| {
                let parent_kind: Option<String> = row.get(9)?;
                let parent_name: Option<String> = row.get(8)?;
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
                    local: false,
                })
            })
            .optional()
        })
    }

    /// The nearest definition of the name in a scope that holds the call, when it is not part of
    /// the target group: a parameter, local variable, or nested function of the caller or an
    /// enclosing function, or a top-level definition of the caller's file.
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
            hit = self.with_statement(SHADOW_IN_CHAIN_SQL, |stmt| {
                stmt.query_row(params![caller, name, row.path, type_use], |r| {
                    r.get::<_, String>(0)
                })
                .optional()
            })?;
        }
        if hit.is_none() {
            hit = self.with_statement(SHADOW_TOP_LEVEL_SQL, |stmt| {
                stmt.query_row(params![name, row.path], |r| r.get::<_, String>(0))
                    .optional()
            })?;
        }
        match hit {
            Some(id) if !group_ids.contains(id.as_str()) => self.definition_by_id(&id),
            _ => Ok(None),
        }
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
        let key = (row.path.clone(), receiver.clone(), name.to_string());
        if !self.import_matches.contains_key(&key) {
            if self.import_match_stmt.is_none() {
                self.import_match_stmt = Some(self.conn.prepare(&import_match_sql())?);
            }
            let stmt = self
                .import_match_stmt
                .as_mut()
                .expect("import match statement was just prepared");
            let matches = stmt
                .query_map(params![row.path, receiver, name], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?
                .filter(|m| !matches!(m, Ok((_, _, signature)) if is_test_label(signature)))
                .map(|m| m.map(|(id, path, _)| (id, path)))
                .collect::<Result<Vec<_>, _>>()?;
            self.import_matches.insert(key.clone(), matches);
        }
        let mut count = 0;
        for (symbol_id, path) in &self.import_matches[&key] {
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

    /// Whether the absolute import that binds the row's qualifier, or the name itself, starts at
    /// the top package of `path` or at a folder above it: `import json` does not reach
    /// `src/flask/json/provider.py`, whose top package is `flask`.
    fn python_import_reaches(
        &self,
        row: &NameRow,
        name: &str,
        path: &str,
    ) -> Result<bool, QueryError> {
        let binding = row.qualifier().unwrap_or(name);
        let sources = self.with_statement(IMPORT_SOURCES_SQL, |stmt| {
            stmt.query_map(params![row.path, binding], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })?;
        if sources.is_empty() || sources.iter().any(|s| s.is_empty() || s.starts_with('.')) {
            return Ok(true);
        }
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
        let reachable: Vec<&str> = if top == folders.len() {
            folders
                .iter()
                .copied()
                .chain(std::iter::once(stem))
                .collect()
        } else {
            folders[..=top].to_vec()
        };
        Ok(sources.iter().any(|source| {
            source
                .split('.')
                .next()
                .is_some_and(|first| reachable.contains(&first))
        }))
    }

    /// The import that binds the qualifier of the call to a module outside the index.
    fn external_binding(&mut self, row: &NameRow) -> Result<Option<(String, String)>, QueryError> {
        let javascript = language_family(&row.language) == "js";
        let imported_head = match row.chain.as_ref().and_then(|chain| chain.first()) {
            Some(head) if javascript && self.js_binds_import(&row.path, head)? => {
                Some(head.as_str())
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
            return if has_import {
                Ok(None)
            } else {
                self.unindexed_path_head(row)
            };
        }
        for source in &sources {
            let external = if javascript {
                self.js_source_is_external(&row.path, source)?
            } else {
                self.source_is_external(source)?
            };
            if !external {
                return Ok(None);
            }
        }
        Ok(Some((qualifier.to_string(), sources[0].clone())))
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
        Ok(match types.as_slice() {
            [only] => Some(only.clone()),
            _ => None,
        })
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
        let definition_spans = definition_spans(&self.definitions(name)?);
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

fn claimed_verdict(row: &NameRow, claim: &Claim, k: usize, builds_class: bool) -> Verdict {
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
    if claim.candidate && k > 1 {
        return Verdict::new(SiteLabel::Unresolved, format!("{k} definitions"), true);
    }
    if row.has_qualifier() {
        return Verdict::new(SiteLabel::Likely, "receiver not typed", true);
    }
    if k > 1 {
        return Verdict::new(
            SiteLabel::Likely,
            format!("nearest of {k} definitions"),
            true,
        );
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
        })
        .collect()
}

/// Marks the calls without a recorded receiver whose name follows a `.` after a call or index
/// result in the source (`single().getAsInt()`, `items[0]?.name()`).
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
            let before = row
                .site_column
                .zip(row.site_line.checked_sub(1).and_then(|i| lines.get(i)))
                .and_then(|(column, line)| line.get(..column))
                .unwrap_or("")
                .trim_end();
            row.hidden_receiver = before
                .strip_suffix('.')
                .is_some_and(|before| before.trim_end_matches(['?', '!']).ends_with([')', ']']));
        }
    }
}

/// Gives each pending or relationship call the first unused call identifier of the same caller
/// and name on or after its line, so a statement that starts above its call reports the line of
/// the call.
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
            let Some(slot) = identifiers
                .iter()
                .enumerate()
                .position(|(slot, &i)| !used[slot] && rows[i].line >= line)
            else {
                continue;
            };
            used[slot] = true;
            let identifier = identifiers[slot];
            let (site_line, site_column) = (rows[identifier].line, rows[identifier].column);
            rows[identifier].consumed = true;
            rows[call].paired = true;
            rows[call].site_line = site_line;
            rows[call].site_column = site_column;
        }
    }
}

fn definition_spans(definitions: &[Definition]) -> HashMap<String, Vec<(usize, usize)>> {
    let mut spans: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
    for definition in definitions {
        let end = definition
            .body_start_line
            .unwrap_or(definition.start_line)
            .max(definition.start_line);
        spans.entry(definition.path.clone()).or_default().push((
            definition
                .decorated_start_line
                .unwrap_or(definition.start_line),
            end,
        ));
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
    match language {
        "javascript" | "typescript" | "tsx" | "jsx" | "vue" => "js",
        "java" | "kotlin" | "scala" | "groovy" => "jvm",
        "c" | "cpp" | "objc" | "objcpp" | "qml" => "c",
        "csharp" | "razor" | "vbnet" | "fsharp" => "dotnet",
        other => other,
    }
}

fn languages_compatible(row: &str, target: &str) -> bool {
    row.is_empty() || language_family(row) == language_family(target)
}

/// The last segment of an import source, `None` for a relative path or a path inside the crate.
fn import_segment(source: &str) -> Option<String> {
    let source = source.trim();
    if source.is_empty()
        || source.starts_with('.')
        || source.starts_with("crate")
        || source.starts_with("self")
        || source.starts_with("super")
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
fn judge_sql(conn: &Connection) -> String {
    let target = "s_target";
    let parent = "s_target_parent";
    let predicate = pending_target_predicate(conn, target, parent);
    let evidence = evidence_sql(conn, target, parent);
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
         JOIN symbols {target} ON {target}.symbol_id = ?2
         LEFT JOIN symbols {parent} ON {parent}.symbol_id = {target}.parent_symbol_id
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
        "SELECT t.symbol_id, t.path, COALESCE(t.signature, '')
         FROM (SELECT ?1 AS path, ?2 AS target_receiver) p
         JOIN symbols t ON t.name = ?3
         WHERE t.parent_symbol_id IS NULL AND t.kind NOT IN ('import', 'export')
           AND t.language NOT IN ({docs})
           AND CASE WHEN p.target_receiver != '' THEN {receiver} ELSE {bare} END",
        docs = documentation_language_list()
    )
}

/// SQL `CASE` that names the positive evidence tying row `p` to `target`, or yields `NULL`.
fn evidence_sql(conn: &Connection, target: &str, parent: &str) -> String {
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
    let receiver_type = receiver_type_sql(conn, parent);
    let qualifier_reaches_owner = module_import_binding_sql(parent, "p.receiver_qualifier");
    let foreign_qualifier = format!(
        "(p.receiver_qualifier IS NOT NULL
          AND EXISTS (SELECT 1 FROM symbols q WHERE q.kind = 'import' AND q.path = p.path
                        AND q.name = p.receiver_qualifier)
          AND NOT {qualifier_reaches_owner})"
    );
    let package_proximity = path_proximity(&format!("{target}.path"), "p.path");
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
            WHEN {has_namespace} AND {parent}.name IS NOT NULL
                 AND EXISTS (SELECT 1 FROM {ns} WHERE value = {parent}.name)
                 THEN 'qualified path'
            WHEN {has_namespace} AND {target}.parent_symbol_id IS NULL THEN 'qualified path'
            WHEN {target}.parent_symbol_id IS NOT NULL AND {names_self}
                 AND s_from.parent_symbol_id = {target}.parent_symbol_id THEN 'class chain'
            WHEN {target}.parent_symbol_id IS NOT NULL
                 AND ({self_receiver} OR {names_self}) AND {receiver_chain} THEN 'class chain'
            WHEN {has_receiver} AND {parent}.name = p.target_receiver AND NOT {foreign_qualifier}
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Workspace, find_julie_extract_binary, open_read_only, safe_tempdir, scan_workspace,
    };

    #[test]
    fn sites_past_the_time_limit_keep_the_label_their_name_gives() {
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");
        let temp = safe_tempdir();
        let root = temp.path();
        std::fs::write(
            root.join("a.py"),
            "def helper():\n    return 1\n\n\ndef use():\n    return helper()\n",
        )
        .unwrap();
        let db = root.join(".code-kb").join("artifact.db");
        scan_workspace(&Workspace::new(root.to_path_buf()), &db, true).unwrap();
        let conn = open_read_only(&db).unwrap();
        let target = select_reference_target(&conn, "helper", None).unwrap();

        let report = report_within(&conn, root, &target, Duration::ZERO).unwrap();

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
}
