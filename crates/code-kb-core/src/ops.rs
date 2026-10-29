use rusqlite::Connection;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use thiserror::Error;

use crate::db::{DbError, ensure_fts_index_path};
use crate::edit_check::{self, BaseRequest, EditCheck, EditCheckError, EditLimits, PendingCheck};
use crate::formatters::{
    OutlineNode, add_path_to_outline, add_unsupported_to_outline, format_file_skeleton,
    format_find_symbol_results, format_unindexed_file, render_outline_tree,
};
use crate::models::{
    BlastRadiusResult, CallersReportSites, CallingTest, ContextSlice, ImpactedSymbol, LabeledSite,
    OpenEnd, ReferenceReport, ReferenceTarget, SiteLabel, Symbol, SymbolSearchResult, TestTarget,
    is_generated_lambda_name,
};
use crate::queries::{self, QueryError};
use crate::reference_report::{
    overload_group, reference_report_for_impact, reference_report_for_impact_within,
    select_reference_target, unchecked_site_count,
};
use crate::slicer::{self, SliceError};
use crate::sync::{self, SyncError};
use crate::workspace::{Workspace, WorkspaceError, paths_equal};

#[derive(Debug, Clone)]
pub enum SymbolSelector {
    Name(String),
    Id(String),
}

#[derive(Debug, Error)]
pub enum OpError {
    #[error("Symbol '{name}' not found in {workspace}. {hint}")]
    SymbolNotFound {
        name: String,
        workspace: String,
        hint: String,
    },
    #[error("File '{path}' not found in {workspace}. {hint}")]
    FileNotFound {
        path: String,
        workspace: String,
        hint: String,
    },
    #[error(
        "`{name}` has {count} overloads in {path}; get_symbol_context shows at most {max}. Pass one symbol_id below (CLI: `--symbol-id`), or call get_symbol_body for every body:\n{candidates}"
    )]
    TooManyOverloads {
        name: String,
        count: usize,
        path: String,
        max: usize,
        candidates: String,
    },
    #[error(
        "symbol_id '{id}' is no longer indexed in {workspace}; run lookup_symbol and select a current id"
    )]
    StaleSymbolId { id: String, workspace: String },
    #[error("A database path is required to refresh a symbol_id selector")]
    SymbolIdRequiresDatabasePath,
    #[error("Selected symbol is in '{actual}', not requested file '{requested}'")]
    FileGuardMismatch { requested: String, actual: String },
    #[error("Workspace error: {0}")]
    Workspace(#[from] WorkspaceError),
    #[error("Synchronization error: {0}")]
    Sync(#[from] SyncError),
    #[error("Query error: {0}")]
    Query(#[from] QueryError),
    #[error("Search index is not ready: {0}")]
    SearchIndex(#[from] DbError),
    #[error("Slice error: {0}")]
    Slice(#[from] SliceError),
    #[error(transparent)]
    EditCheck(#[from] EditCheckError),
}

/// What `lookup_symbol` found for one query.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SymbolLookup {
    /// Rows named like the query, or the members of the owner it names.
    pub names: Vec<Symbol>,
    /// Ranked matches by name words, signatures, and docs that `names` does not already show.
    pub ranked: Vec<SymbolSearchResult>,
    /// Close names, or the definitions a path filter excludes, when no row has the query as its
    /// name; `names` never repeats one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<Symbol>,
    /// The text of `suggestions` that precedes the ranked rows; `None` when `miss` holds it.
    #[serde(skip)]
    pub suggestion_hint: Option<String>,
    /// The text for a query that finds nothing: the bound workspace and close names.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub miss: Option<String>,
}

/// Looks up `query` by name. A plain query that no row names exactly also gets the ranked search,
/// so one call answers a name and a concept.
pub fn lookup_symbols_op(
    db_path: &Path,
    conn: &Connection,
    query: &str,
    kind: Option<&str>,
    path_filter: Option<&str>,
    include_tests: bool,
    limit: usize,
) -> Result<SymbolLookup, OpError> {
    let names =
        queries::search_symbols_scoped(conn, query, kind, path_filter, include_tests, limit)?;
    let plain = !query.contains('.') && !query.contains("::");
    let has_query_name = |symbol: &Symbol| symbol.name.eq_ignore_ascii_case(query);
    let named_exactly = names.iter().any(has_query_name);
    let mut ranked = Vec::new();
    if limit > 0 && plain && !named_exactly {
        ensure_fts_index_path(db_path)?;
        ranked = queries::fts_search_symbols_scoped(
            conn,
            query,
            kind,
            path_filter,
            include_tests,
            limit + names.len(),
        )?;
        ranked.retain(|hit| names.iter().all(|s| s.symbol_id != hit.symbol.symbol_id));
        ranked.truncate(limit);
    }
    let found_nothing = names.is_empty() && ranked.is_empty();
    let one_word = !query.chars().any(char::is_whitespace);
    let shows_query_name = named_exactly || ranked.iter().any(|hit| has_query_name(&hit.symbol));
    let kind = kind.map(queries::normalize_kind);
    let matches_kind = |symbol: &Symbol| {
        kind.as_deref()
            .is_none_or(|k| symbol.kind == k || crate::formatters::display_kind(symbol) == k)
    };
    let suggestions = if limit == 0 {
        queries::NameSuggestions::default()
    } else if found_nothing {
        queries::name_suggestions(conn, query, path_filter, &matches_kind)
    } else if plain && one_word && !shows_query_name {
        let new_name_of_kind = |s: &Symbol| {
            matches_kind(s)
                && names
                    .iter()
                    .all(|row| !row.name.eq_ignore_ascii_case(&s.name))
        };
        let found = queries::name_suggestions(conn, query, path_filter, &new_name_of_kind);
        if names.is_empty() || found.excluded_by.is_some() {
            found
        } else {
            queries::NameSuggestions::default()
        }
    } else {
        queries::NameSuggestions::default()
    };
    let suggestion_hint = suggestions.hint();
    let miss = (limit > 0 && found_nothing).then(|| {
        let hint = suggestion_hint
            .clone()
            .unwrap_or_else(|| queries::NO_SIMILAR_NAME.to_string());
        let workspace = queries::workspace_name(conn);
        format!("No symbols found matching \"{query}\" in {workspace}. {hint}\n")
    });
    Ok(SymbolLookup {
        names,
        ranked,
        suggestions: suggestions.symbols,
        suggestion_hint: suggestion_hint.filter(|_| miss.is_none()),
        miss,
    })
}

/// The text of a lookup. Members show as `Owner.name`; the rows in `lookup` keep their raw names.
pub fn render_symbol_lookup(
    conn: &Connection,
    query: &str,
    lookup: &SymbolLookup,
    limit: usize,
) -> Result<String, QueryError> {
    if let Some(miss) = &lookup.miss {
        return Ok(miss.clone());
    }
    let mut names = lookup.names.clone();
    let mut ranked = lookup.ranked.clone();
    queries::qualify_members(
        conn,
        names
            .iter_mut()
            .chain(ranked.iter_mut().map(|hit| &mut hit.symbol)),
    )?;
    let Some(hint) = &lookup.suggestion_hint else {
        return Ok(format_find_symbol_results(query, &names, &ranked, limit));
    };
    let mut sections = Vec::new();
    if !names.is_empty() {
        sections.push(format_find_symbol_results(query, &names, &[], limit));
    }
    sections.push(format!("{hint}\n"));
    if !ranked.is_empty() {
        sections.push(format_find_symbol_results(query, &[], &ranked, limit));
    }
    Ok(sections.join("\n"))
}

fn stale_symbol_id(conn: &Connection, id: &str) -> OpError {
    OpError::StaleSymbolId {
        id: id.to_string(),
        workspace: queries::workspace_name(conn),
    }
}

pub fn resolve_symbol_op(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
    selector: &SymbolSelector,
    file_path: Option<&str>,
) -> Result<Symbol, OpError> {
    let id_selector = matches!(selector, SymbolSelector::Id(_));
    let mut folder = None;
    let guard = if let Some(file_path) = file_path {
        let (absolute, rel) = workspace.resolve_path(Path::new(file_path))?;
        if !absolute.exists() {
            return Err(file_not_found(conn, &rel));
        }
        if absolute.is_dir() {
            folder = Some(rel);
            None
        } else {
            if !id_selector {
                sync::ensure_fresh_file(workspace, db_path, conn, &rel)?;
            }
            Some((absolute, rel))
        }
    } else {
        None
    };
    let scope = guard
        .as_ref()
        .map(|(_, rel)| rel.as_str())
        .or(folder.as_deref());
    let lookup = |conn: &Connection| match selector {
        SymbolSelector::Name(name) => queries::get_symbol_by_name(conn, name, scope),
        SymbolSelector::Id(id) => queries::get_symbol_by_id(conn, id),
    };
    let initial = lookup(conn)?.ok_or_else(|| match selector {
        SymbolSelector::Name(name) => symbol_not_found(conn, name, scope),
        SymbolSelector::Id(id) => stale_symbol_id(conn, id),
    })?;
    if !id_selector
        && let Some((requested_abs, requested_rel)) = &guard
        && !paths_equal(requested_abs, &workspace.canonical_root.join(&initial.path))
    {
        return Err(OpError::FileGuardMismatch {
            requested: requested_rel.clone(),
            actual: initial.path,
        });
    }
    let refreshed = sync::ensure_fresh_file(workspace, db_path, conn, &initial.path)?;
    let refreshed_symbol = if refreshed {
        match selector {
            SymbolSelector::Name(name) => {
                queries::get_symbol_by_name_exact(conn, name, &initial.path)?
            }
            SymbolSelector::Id(id) => queries::get_symbol_by_id(conn, id)?,
        }
        .ok_or_else(|| match selector {
            SymbolSelector::Name(name) => symbol_not_found(conn, name, scope),
            SymbolSelector::Id(id) => stale_symbol_id(conn, id),
        })?
    } else {
        initial
    };
    if let Some((requested_abs, requested_rel)) = &guard
        && !paths_equal(
            requested_abs,
            &workspace.canonical_root.join(&refreshed_symbol.path),
        )
    {
        return Err(OpError::FileGuardMismatch {
            requested: requested_rel.clone(),
            actual: refreshed_symbol.path,
        });
    }
    Ok(refreshed_symbol)
}

fn symbol_not_found(conn: &Connection, name: &str, path_filter: Option<&str>) -> OpError {
    let (workspace, hint) = queries::symbol_not_found_parts(conn, name, path_filter);
    OpError::SymbolNotFound {
        name: name.to_string(),
        workspace,
        hint,
    }
}

fn file_not_found(conn: &Connection, rel_path: &str) -> OpError {
    let (workspace, hint) = queries::file_not_found_parts(conn, rel_path);
    OpError::FileNotFound {
        path: rel_path.to_string(),
        workspace,
        hint,
    }
}

/// Retrieve the body of a symbol by name, guaranteeing fresh offsets and file contents.
pub fn get_symbol_body_op(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
    symbol_name: &str,
    file_path: Option<&str>,
) -> Result<(Symbol, String), OpError> {
    get_symbol_body_selected_op(
        workspace,
        db_path,
        conn,
        &SymbolSelector::Name(symbol_name.to_string()),
        file_path,
    )
}

pub fn get_symbol_body_selected_op(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
    selector: &SymbolSelector,
    file_path: Option<&str>,
) -> Result<(Symbol, String), OpError> {
    let mut symbol = resolve_symbol_op(workspace, db_path, conn, selector, file_path)?;
    if let Some((start_byte, start_line)) = decorated_start(conn, &symbol) {
        symbol.start_byte = start_byte;
        symbol.start_line = start_line;
    }

    let abs_file = workspace.canonical_root.join(&symbol.path);
    let source = slicer::slice_symbol_source(&abs_file, &symbol, symbol.start_byte)?;

    Ok((symbol, source))
}

/// A context read builds callees, types, and tests for each overload, so it stops at this many.
pub const MAX_CONTEXT_OVERLOADS: usize = 4;

/// Like [`get_symbol_body_selected_op`], but a name that selects several overloads of one owner
/// in one file returns the body of each overload instead of an ambiguity error.
pub fn get_symbol_bodies_selected_op(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
    selector: &SymbolSelector,
    file_path: Option<&str>,
) -> Result<Vec<(Symbol, String)>, OpError> {
    match get_symbol_body_selected_op(workspace, db_path, conn, selector, file_path) {
        Err(ambiguity @ OpError::Query(QueryError::AmbiguousSymbol(..))) => overload_selectors(
            workspace,
            db_path,
            conn,
            selector,
            file_path,
            ambiguity,
            usize::MAX,
        )?
        .iter()
        .map(|overload| get_symbol_body_selected_op(workspace, db_path, conn, overload, None))
        .collect(),
        result => result.map(|body| vec![body]),
    }
}

/// Like [`get_context_slice_selected_op`], but a name that selects up to
/// [`MAX_CONTEXT_OVERLOADS`] overloads of one owner in one file returns a slice for each overload.
pub fn get_context_slices_selected_op(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
    selector: &SymbolSelector,
    file_path: Option<&str>,
    include_external: bool,
) -> Result<Vec<ContextSlice>, OpError> {
    let slice = |selector: &SymbolSelector, file_path: Option<&str>| {
        get_context_slice_selected_op(
            workspace,
            db_path,
            conn,
            selector,
            file_path,
            include_external,
        )
    };
    match slice(selector, file_path) {
        Err(ambiguity @ OpError::Query(QueryError::AmbiguousSymbol(..))) => overload_selectors(
            workspace,
            db_path,
            conn,
            selector,
            file_path,
            ambiguity,
            MAX_CONTEXT_OVERLOADS,
        )?
        .iter()
        .map(|overload| slice(overload, None))
        .collect(),
        result => result.map(|slice| vec![slice]),
    }
}

/// The current ids of the overloads a name selects, after their file is refreshed. Returns
/// `ambiguity` for an id selector or when the refresh left none, and `TooManyOverloads` for more
/// than `max`.
fn overload_selectors(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
    selector: &SymbolSelector,
    file_path: Option<&str>,
    ambiguity: OpError,
    max: usize,
) -> Result<Vec<SymbolSelector>, OpError> {
    let SymbolSelector::Name(name) = selector else {
        return Err(ambiguity);
    };
    let scope = file_path
        .map(|path| workspace.resolve_path(Path::new(path)))
        .transpose()?
        .map(|(_, rel)| rel);
    let mut overloads = queries::get_symbol_overloads(conn, name, scope.as_deref())?;
    if let Some(path) = overloads.first().map(|symbol| symbol.path.clone())
        && sync::ensure_fresh_file(workspace, db_path, conn, &path)?
    {
        overloads = queries::get_symbol_overloads(conn, name, scope.as_deref())?;
    }
    if overloads.is_empty() {
        return Err(ambiguity);
    }
    if overloads.len() > max {
        return Err(OpError::TooManyOverloads {
            name: overloads[0].name.clone(),
            count: overloads.len(),
            path: overloads[0].path.clone(),
            max,
            candidates: queries::candidate_id_lines(&overloads),
        });
    }
    Ok(overloads
        .into_iter()
        .map(|symbol| SymbolSelector::Id(symbol.symbol_id))
        .collect())
}

/// The byte and line where the decorators above a Python symbol begin: julie starts the symbol at
/// its `def` or `class` line and records the decorated block as a `decorated_definition` fact.
fn decorated_start(conn: &Connection, symbol: &Symbol) -> Option<(usize, usize)> {
    conn.query_row(
        "SELECT start_byte, start_line FROM structural_facts
         WHERE path = ?1 AND pattern_id LIKE '%.decorated_definition.%'
           AND end_byte = ?2 AND start_byte < ?3
         ORDER BY start_byte DESC LIMIT 1",
        rusqlite::params![
            symbol.path,
            symbol.end_byte as i64,
            symbol.start_byte as i64
        ],
        |row| {
            Ok((
                row.get::<_, i64>(0)? as usize,
                row.get::<_, i64>(1)? as usize,
            ))
        },
    )
    .ok()
}

/// Retrieve a complete context slice for a symbol, guaranteeing fresh offsets.
pub fn get_context_slice_op(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
    symbol_name: &str,
    file_path: Option<&str>,
    include_external: bool,
) -> Result<ContextSlice, OpError> {
    get_context_slice_selected_op(
        workspace,
        db_path,
        conn,
        &SymbolSelector::Name(symbol_name.to_string()),
        file_path,
        include_external,
    )
}

pub fn get_context_slice_selected_op(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
    selector: &SymbolSelector,
    file_path: Option<&str>,
    include_external: bool,
) -> Result<ContextSlice, OpError> {
    let (target_symbol, target_body) =
        get_symbol_body_selected_op(workspace, db_path, conn, selector, file_path)?;

    let callee_signatures = queries::find_callee_signatures(
        conn,
        &target_symbol.name,
        &target_symbol.symbol_id,
        10,
        include_external,
    )?;
    let related_types = queries::find_related_types(conn, &target_symbol, 10)?;
    let related_tests = queries::find_related_tests(conn, &target_symbol, 5)?;

    Ok(ContextSlice {
        target_symbol,
        target_body,
        callee_signatures,
        related_types,
        related_tests,
    })
}

/// Generate file skeleton with fresh file synchronization.
pub fn file_skeleton_op(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
    file_path: &str,
) -> Result<String, OpError> {
    let (effective_abs, rel_path) = workspace.resolve_path(Path::new(file_path))?;
    if !effective_abs.exists() {
        return Err(file_not_found(conn, &rel_path));
    }
    if effective_abs.is_dir() {
        return codebase_outline_op(workspace, conn, 2, Some(&rel_path));
    }
    sync::ensure_fresh_file(workspace, db_path, conn, &rel_path)?;

    let mut symbols = queries::load_file_symbols(conn, &rel_path)?;
    let inherited =
        queries::inherited_writes_among(conn, symbols.iter().map(|s| s.symbol_id.as_str()))?;
    symbols.retain(|symbol| !inherited.contains(&symbol.symbol_id));
    let file_meta = queries::get_file(conn, &rel_path)?;
    if symbols.is_empty() {
        match &file_meta {
            None => {
                return Ok(format_unindexed_file(
                    &rel_path,
                    "Not in the index: the scan skips this path.",
                ));
            }
            Some(meta) if meta.language == "unsupported" => {
                return Ok(format_unindexed_file(
                    &rel_path,
                    "Unsupported file: no extractor for the language.",
                ));
            }
            Some(_) => {}
        }
    }
    let line_count = file_meta.and_then(|m| m.line_count.map(|l| l as usize));
    let parse_errors = queries::count_parse_diagnostics(conn, &rel_path);

    Ok(format_file_skeleton(
        &rel_path,
        &symbols,
        line_count,
        parse_errors,
    ))
}

/// Generate a memory-bounded codebase outline pushed down into SQLite.
pub fn codebase_outline_op(
    workspace: &Workspace,
    conn: &Connection,
    depth: usize,
    path_filter: Option<&str>,
) -> Result<String, OpError> {
    let rel_filter = path_filter.map(|p| workspace.relativize_filter(p));
    let path_filter = rel_filter.as_deref().filter(|path| !path.is_empty());
    let norm = path_filter
        .map(|p| p.replace('\\', "/").trim_matches('/').to_string())
        .filter(|p| !p.is_empty());
    let scope = queries::OutlineScope::new(path_filter, depth);
    let symbols_by_file = queries::load_scoped_outline_symbols(conn, path_filter, depth, 5)?;
    let counts = queries::load_outline_counts(conn, path_filter, depth)?;

    let mut root_node = OutlineNode::default();
    let norm_filter = norm.as_deref().unwrap_or_default();
    let mut files_found = 0;
    let mut listed = 0;
    let mut truncated = false;
    queries::for_each_outline_path(conn, &scope, |file_path| {
        files_found += 1;
        if scope.lists(&file_path) {
            listed += 1;
            if listed > queries::OUTLINE_FILE_CAP {
                truncated = true;
                return false;
            }
        }
        add_path_to_outline(
            &mut root_node,
            &file_path,
            &symbols_by_file,
            &counts,
            depth,
            norm_filter,
        );
        true
    })?;

    let mut unsupported = 0;
    queries::for_each_unsupported_path(conn, &scope, |file_path| {
        unsupported += 1;
        add_unsupported_to_outline(&mut root_node, file_path, depth, norm_filter);
    })?;
    if let Some(filter) = path_filter
        && files_found == 0
        && unsupported == 0
    {
        return Err(file_not_found(conn, filter));
    }

    let display_root = if norm_filter.is_empty() {
        format!("{}/", workspace.repo_name)
    } else {
        format!("{}/{}/", workspace.repo_name, norm_filter)
    };

    let mut out = String::new();
    out.push_str(&format!("{display_root}\n"));
    render_outline_tree(&mut out, &root_node, "", 0, depth);

    if truncated {
        let msg = if path_filter.is_some() {
            "\n[Outline truncated: path matches over 1,000 files. Narrow your path filter or specify a deeper path to reduce scope.]\n"
        } else {
            "\n[Outline truncated: workspace contains over 1,000 files. Use a path filter (e.g. `code-kb outline <path>`) to narrow scope.]\n"
        };
        out.push_str(msg);
    }

    if unsupported > 0 {
        let noun = if unsupported == 1 { "file" } else { "files" };
        out.push_str(&format!(
            "\n[{unsupported} unsupported {noun}: no extractor for the language]\n"
        ));
    }

    Ok(out)
}

/// Checks the working tree against `base`, else the session base of the project, else `HEAD`:
/// the symbols whose shape changed and the sites of each that the diff did not reach.
pub fn edit_check_op(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
    base: Option<&str>,
) -> Result<EditCheck, OpError> {
    match base.map(str::trim).filter(|rev| !rev.is_empty()) {
        Some(rev) => edit_check_with(
            workspace,
            db_path,
            conn,
            BaseRequest::Given(rev),
            EditLimits::default(),
        ),
        None => {
            let session_base = edit_check::latest_session_base(&workspace.canonical_root);
            edit_check_session_op(workspace, db_path, conn, session_base.as_deref())
        }
    }
}

/// Checks the working tree against the base a session recorded, else `HEAD`. A recorded base
/// that git no longer has counts as none.
pub fn edit_check_session_op(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
    session_base: Option<&str>,
) -> Result<EditCheck, OpError> {
    Ok(edit_check_session_diff(workspace, conn, session_base)?.run(workspace, db_path, conn)?)
}

/// The first half of [`edit_check_session_op`]: the diff against the session base and its hash,
/// before any scan of the base. [`PendingCheck::run`] finishes the check.
pub fn edit_check_session_diff(
    workspace: &Workspace,
    conn: &Connection,
    session_base: Option<&str>,
) -> Result<PendingCheck, OpError> {
    let limits = EditLimits::default();
    if let Some(base) = session_base {
        match edit_check::prepare(workspace, conn, BaseRequest::Session(base), limits) {
            Err(EditCheckError::UnknownRevision(_)) => {}
            prepared => return Ok(prepared?),
        }
    }
    Ok(edit_check::prepare(
        workspace,
        conn,
        BaseRequest::Head,
        limits,
    )?)
}

/// Like [`edit_check_op`], with the base named by its source and the cuts given.
pub fn edit_check_with(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
    request: BaseRequest<'_>,
    limits: EditLimits,
) -> Result<EditCheck, OpError> {
    Ok(edit_check::run(workspace, db_path, conn, request, limits)?)
}

/// Compute blast radius and likely tests for a symbol, file, or uncommitted git changes.
pub fn blast_radius_op(
    workspace: &Workspace,
    conn: &Connection,
    symbol: Option<&str>,
    file: Option<&str>,
    max_depth: usize,
    limit: usize,
) -> Result<BlastRadiusResult, OpError> {
    blast_radius_selected_op(
        workspace,
        None,
        conn,
        symbol.map(|s| SymbolSelector::Name(s.to_string())),
        file,
        max_depth,
        limit,
    )
}

pub fn blast_radius_selected_op(
    workspace: &Workspace,
    db_path: Option<&Path>,
    conn: &Connection,
    selector: Option<SymbolSelector>,
    file: Option<&str>,
    max_depth: usize,
    limit: usize,
) -> Result<BlastRadiusResult, OpError> {
    blast_radius_selected_op_with_options(
        workspace, db_path, conn, selector, file, max_depth, limit, false,
    )
}

/// Analyze a selected seed, optionally including import, module, and namespace rows.
#[allow(clippy::too_many_arguments)]
pub fn blast_radius_selected_op_with_options(
    workspace: &Workspace,
    db_path: Option<&Path>,
    conn: &Connection,
    selector: Option<SymbolSelector>,
    file: Option<&str>,
    max_depth: usize,
    limit: usize,
    include_low_signal: bool,
) -> Result<BlastRadiusResult, OpError> {
    if let Some(selector @ SymbolSelector::Id(_)) = selector.as_ref() {
        let db_path = db_path.ok_or(OpError::SymbolIdRequiresDatabasePath)?;
        let selected = resolve_symbol_op(workspace, db_path, conn, selector, file)?;
        let res = queries::compute_blast_radius_scoped_with_ids_and_low_signal(
            conn,
            &[],
            &[&selected.symbol_id],
            None,
            &[],
            if max_depth == 0 { 2 } else { max_depth.min(5) },
            limit,
            include_low_signal,
        )?;
        let depth = if max_depth == 0 { 2 } else { max_depth.min(5) };
        return with_calling_tests(
            workspace,
            conn,
            Some(selected),
            res,
            depth,
            limit,
            include_low_signal,
        );
    }
    let symbol = selector.as_ref().and_then(|selector| match selector {
        SymbolSelector::Name(name) => Some(name.as_str()),
        SymbolSelector::Id(_) => None,
    });
    let clean_symbol = symbol.and_then(|s| {
        let t = s.trim();
        if t.is_empty() { None } else { Some(t) }
    });
    let clean_file = file.and_then(|f| {
        let t = f.trim();
        if t.is_empty() {
            None
        } else {
            Some(workspace.relativize_filter(t))
        }
    });

    let depth = if max_depth == 0 { 2 } else { max_depth.min(5) };
    let (symbol, symbol_path_filter) = match (clean_symbol, clean_file) {
        (Some(s), f) => (s, f),
        (None, Some(f)) => {
            return file_blast_radius(workspace, conn, &f, depth, limit, include_low_signal);
        }
        (None, None) => {
            let mut changes = edit_check::uncommitted_changes(&workspace.root)?;
            changes.retain(|path, _| !crate::workspace::is_hard_excluded(path));
            let res = queries::compute_blast_radius_for_changes_with_low_signal(
                conn,
                &changes,
                depth,
                limit,
                include_low_signal,
            )?;
            let targets = queries::diff_targets(conn, &changes)?;
            return with_report_tests(
                workspace,
                conn,
                targets,
                &changes,
                res,
                depth,
                limit,
                include_low_signal,
            );
        }
    };

    let res = queries::compute_blast_radius_scoped_with_ids_and_low_signal(
        conn,
        &[symbol],
        &[],
        symbol_path_filter.as_deref(),
        &[],
        depth,
        limit,
        include_low_signal,
    )?;
    let target = select_reference_target(conn, symbol, symbol_path_filter.as_deref()).ok();
    with_calling_tests(
        workspace,
        conn,
        target,
        res,
        depth,
        limit,
        include_low_signal,
    )
}

/// How long the callers reports of a file target's symbols may run, from the start of the
/// request, before the rest go unchecked.
const FILE_REPORT_TIME_LIMIT: std::time::Duration = std::time::Duration::from_secs(3);

/// What the callers reports of a blast radius could not cover: the reports that failed and the
/// sites that a report did not check within its time limit.
#[derive(Default)]
struct ReportGaps {
    failed: usize,
    unchecked_sites: usize,
}

impl ReportGaps {
    fn record(&mut self, report: &ReferenceReport) {
        self.unchecked_sites += unchecked_site_count(report);
    }

    fn apply(&self, res: &mut BlastRadiusResult) {
        if self.failed > 0 {
            res.report_symbols_failed = Some(res.report_symbols_failed.unwrap_or(0) + self.failed);
        }
        if self.unchecked_sites > 0 {
            res.report_sites_unchecked =
                Some(res.report_sites_unchecked.unwrap_or(0) + self.unchecked_sites);
        }
    }
}

/// The callers of one report in the two forms [`report_callers`] gives: on their own, and joined
/// to callers that the walk found.
struct ReportCallerRows {
    alone: Vec<(Option<String>, ImpactedSymbol)>,
    with_walk: Vec<(Option<String>, ImpactedSymbol)>,
}

/// The blast radius of a file or folder. The walk follows resolved and pending calls; the callers
/// report of each symbol of the file also ties a read of a name, such as
/// `run_command.make_context()` on a decorated command or a module-level object passed as an
/// argument. Its tests join the walk's, except a test that only `name only` sites tie, and a
/// symbol whose report has a proven or likely site is no open end. The callers of its proven and
/// likely sites outside the file or folder join as for one symbol, and the walk goes on from
/// them. A call of an overload stays a caller: every overload of a group sits in one file, so
/// each one is a seed of the walk.
fn file_blast_radius(
    workspace: &Workspace,
    conn: &Connection,
    file: &str,
    depth: usize,
    limit: usize,
    include_low_signal: bool,
) -> Result<BlastRadiusResult, OpError> {
    file_blast_radius_within(
        workspace,
        conn,
        file,
        depth,
        limit,
        include_low_signal,
        FileReportLimits {
            budget: FILE_REPORT_TIME_LIMIT,
            report: FILE_REPORT_TIME_LIMIT,
        },
    )
}

/// The time the callers reports of a file target may take: `budget` for all of them from the
/// start of the call, and `report` for one.
struct FileReportLimits {
    budget: std::time::Duration,
    report: std::time::Duration,
}

/// [`file_blast_radius`] with the callers reports bounded by `limits`. Each report gets the time
/// that is left of the budget, up to the limit of one report.
fn file_blast_radius_within(
    workspace: &Workspace,
    conn: &Connection,
    file: &str,
    depth: usize,
    limit: usize,
    include_low_signal: bool,
    limits: FileReportLimits,
) -> Result<BlastRadiusResult, OpError> {
    let started = std::time::Instant::now();
    let deadline = started.checked_add(limits.budget).unwrap_or(started);
    let mut tests = Vec::new();
    let mut seen = HashSet::new();
    let mut tied = HashSet::new();
    let mut gaps = ReportGaps::default();
    let mut callers = Vec::new();
    let unchecked = queries::visit_file_report_targets(
        conn,
        &[file],
        deadline,
        |target| -> Result<(), OpError> {
            let left = deadline
                .saturating_duration_since(std::time::Instant::now())
                .min(limits.report);
            let Ok(report) =
                reference_report_for_impact_within(conn, &workspace.root, &target, left)
            else {
                gaps.failed += 1;
                return Ok(());
            };
            gaps.record(&report);
            if report.counts.proven + report.counts.likely > 0 {
                tied.insert((target.path.replace('\\', "/"), target.start_line));
            }
            callers.push(ReportCallerRows {
                alone: report_callers(conn, &report, false, false)?,
                with_walk: report_callers(conn, &report, true, false)?,
            });
            if report.tests.iter().all(|test| test.name_only) {
                return Ok(());
            }
            let name = member_display_name(conn, &target)?;
            for test in report.tests.into_iter().filter(|test| !test.name_only) {
                if seen.insert((test.path.clone(), test.line)) {
                    tests.push(TestTarget {
                        reason: format!("calls `{name}` (find_references)"),
                        name: test.name,
                        path: test.path,
                        line: test.line,
                    });
                }
            }
            Ok(())
        },
    )?;
    let mut res = queries::compute_blast_radius_for_files(
        conn,
        &[file],
        depth,
        limit,
        include_low_signal,
        &tests,
    )?;
    res.open_ends
        .retain(|end| !tied.contains(&(end.path.clone(), end.line)));
    if !res.traversal_ceiling_reached
        && !res.impacted_symbols_truncated
        && !res.likely_tests_truncated
    {
        let walked_callers = !res.impacted_symbols.is_empty();
        let rows = callers
            .into_iter()
            .flat_map(|rows| {
                if walked_callers {
                    rows.with_walk
                } else {
                    rows.alone
                }
            })
            .filter(|(_, row)| !queries::in_seed_paths(&row.path, &[file]));
        let joined = join_report_callers(std::mem::take(&mut res.impacted_symbols), rows);
        res.impacted_symbols = joined.rows;
        walk_on_from_joined(
            conn,
            &mut res,
            &joined.added,
            depth,
            limit,
            include_low_signal,
        )?;
        res.impacted_symbols_found = res.impacted_symbols.len();
        res.impacted_symbols_truncated |= res.impacted_symbols.len() > limit;
        res.impacted_symbols.truncate(limit);
        res.likely_tests_found = res.likely_tests.len();
        res.likely_tests_truncated |= res.likely_tests.len() > limit;
        res.likely_tests.truncate(limit);
    }
    res.report_symbols_unchecked = (unchecked > 0).then_some(unchecked);
    gaps.apply(&mut res);
    Ok(res)
}

/// `Owner.name` for a member of a class or another code type, else the name.
fn member_display_name(conn: &Connection, symbol: &Symbol) -> Result<String, OpError> {
    let owner = match symbol.parent_symbol_id.as_deref() {
        Some(parent) => queries::get_symbol_by_id(conn, parent)?,
        None => None,
    };
    Ok(match owner {
        Some(owner)
            if matches!(
                owner.kind.as_str(),
                "class"
                    | "struct"
                    | "interface"
                    | "trait"
                    | "enum"
                    | "record"
                    | "object"
                    | "protocol"
                    | "union"
            ) =>
        {
            format!("{}.{}", owner.name, symbol.name)
        }
        _ => symbol.name.clone(),
    })
}

fn report_test(test: CallingTest) -> TestTarget {
    TestTarget {
        reason: if test.name_only {
            "calls it by name only (find_references)"
        } else {
            "calls it (find_references)"
        }
        .to_string(),
        name: test.name,
        path: test.path,
        line: test.line,
    }
}

/// Adds, after the walk's tests, the tests in the callers report of each function that a diff
/// changed, and drops such a function from the open ends when its report has a proven or likely
/// site. The callers of its proven and likely sites that the walk did not list, and that hold no
/// changed line, join as depth-1 rows, and the walk goes on from them. The walk follows resolved
/// and pending rows by name; the report also ties a call through a receiver's type, a read of a
/// member, or a name alone.
#[allow(clippy::too_many_arguments)]
fn with_report_tests(
    workspace: &Workspace,
    conn: &Connection,
    targets: Vec<Symbol>,
    changes: &BTreeMap<String, Vec<(usize, usize)>>,
    mut res: BlastRadiusResult,
    depth: usize,
    limit: usize,
    include_low_signal: bool,
) -> Result<BlastRadiusResult, OpError> {
    if targets.is_empty() || res.traversal_ceiling_reached || res.likely_tests_truncated {
        return Ok(res);
    }
    let mut seen: HashSet<(String, usize)> = res
        .likely_tests
        .iter()
        .map(|test| (test.path.clone(), test.line))
        .collect();
    let joins_callers = !res.impacted_symbols_truncated;
    let walked_callers = !res.impacted_symbols.is_empty();
    let mut added = Vec::new();
    let mut gaps = ReportGaps::default();
    for target in &targets {
        let Ok(report) = reference_report_for_impact(conn, &workspace.root, target) else {
            gaps.failed += 1;
            continue;
        };
        gaps.record(&report);
        if report.counts.proven + report.counts.likely > 0 {
            let path = target.path.replace('\\', "/");
            res.open_ends
                .retain(|end| end.path != path || end.line != target.start_line);
        }
        if joins_callers {
            let mut callers = Vec::new();
            let overloaded = walked_callers && overload_group(conn, target)?.len() > 1;
            for (id, row) in report_callers(conn, &report, walked_callers, overloaded)? {
                if !holds_changed_line(conn, id.as_deref(), &row, changes)? {
                    callers.push((id, row));
                }
            }
            let joined = join_report_callers(std::mem::take(&mut res.impacted_symbols), callers);
            res.impacted_symbols = joined.rows;
            added.extend(joined.added);
        }
        for test in report.tests {
            if seen.insert((test.path.clone(), test.line)) {
                res.likely_tests.push(report_test(test));
            }
        }
    }
    gaps.apply(&mut res);
    walk_on_from_joined(conn, &mut res, &added, depth, limit, include_low_signal)?;
    seen.extend(
        res.likely_tests
            .iter()
            .map(|test| (test.path.clone(), test.line)),
    );
    if joins_callers {
        res.impacted_symbols_found = res.impacted_symbols.len();
        res.impacted_symbols_truncated |= res.impacted_symbols.len() > limit;
        res.impacted_symbols.truncate(limit);
    }
    if res.tests_past_depth.is_some() {
        res.past_depth_tests.retain(|key| !seen.contains(key));
        res.tests_past_depth = Some(res.past_depth_tests.len());
    }
    res.likely_tests_found = res.likely_tests.len();
    res.likely_tests_truncated = res.likely_tests.len() > limit;
    res.likely_tests.truncate(limit);
    Ok(res)
}

/// Fills what the walk left empty from the callers report of `target`: the callers of its proven
/// and likely sites when the walk found no impacted symbol, and its tests, or `target` itself when
/// it is a test. The report's tests lead when the walk found no test and follow the walk's tests
/// otherwise. The walk follows resolved and pending rows by name, and the report also proves a
/// call through a receiver's type, so the walk goes on from the callers the report found, one
/// level deeper.
fn with_calling_tests(
    workspace: &Workspace,
    conn: &Connection,
    target: Option<Symbol>,
    mut res: BlastRadiusResult,
    depth: usize,
    limit: usize,
    include_low_signal: bool,
) -> Result<BlastRadiusResult, OpError> {
    let Some(target) = target else {
        return Ok(res);
    };
    if res.traversal_ceiling_reached {
        return Ok(res);
    }
    let needs_tests = res.likely_tests.is_empty() && !res.likely_tests_truncated;
    let needs_callers = res.impacted_symbols.is_empty() && !res.impacted_symbols_truncated;
    let joins_callers = !res.impacted_symbols_truncated;
    let reads_unlisted = !needs_tests
        && !needs_callers
        && joins_callers
        && !res.likely_tests_truncated
        && queries::has_member_read_outside(conn, &target, &listed_rows(&res))?;
    if !needs_tests && !needs_callers && !reads_unlisted {
        return Ok(res);
    }
    // The walk's answer stands alone; an index too old for the report keeps it.
    let Ok(report) = reference_report_for_impact(conn, &workspace.root, &target) else {
        return Ok(res);
    };
    let mut gaps = ReportGaps::default();
    gaps.record(&report);
    gaps.apply(&mut res);
    if report.counts.proven + report.counts.likely > 0 {
        let path = target.path.replace('\\', "/");
        res.open_ends
            .retain(|end| end.path != path || end.line != target.start_line);
    }
    if joins_callers {
        let overloaded = !needs_callers && overload_group(conn, &target)?.len() > 1;
        let callers = report_callers(conn, &report, !needs_callers, overloaded)?
            .into_iter()
            .filter(|(id, _)| needs_callers || id.as_deref() != Some(target.symbol_id.as_str()));
        let joined = join_report_callers(std::mem::take(&mut res.impacted_symbols), callers);
        res.impacted_symbols = joined.rows;
        walk_on_from_joined(
            conn,
            &mut res,
            &joined.added,
            depth,
            limit,
            include_low_signal,
        )?;
        res.impacted_symbols_found = res.impacted_symbols.len();
        res.impacted_symbols_truncated |= res.impacted_symbols.len() > limit;
        res.impacted_symbols.truncate(limit);
        if needs_callers {
            res.callers_report = Some(CallersReportSites {
                sites: report.counts.sites,
                proven: report.counts.proven,
                likely: report.counts.likely,
                unresolved: report.counts.unresolved,
                excluded: report.counts.excluded,
            });
        }
    }
    if res.likely_tests_truncated {
        return Ok(res);
    }
    let report_tests: Vec<TestTarget> = report.tests.into_iter().map(report_test).collect();
    let walked = std::mem::take(&mut res.likely_tests);
    let (first, then) = if needs_tests {
        (report_tests, walked)
    } else {
        (walked, report_tests)
    };
    res.likely_tests = first;
    if res.likely_tests.is_empty() && then.is_empty() && (target.is_test || target.test_container) {
        res.likely_tests.push(TestTarget {
            name: target.name,
            path: target.path.replace('\\', "/"),
            line: target.start_line,
            reason: "the target is a test".to_string(),
        });
    }
    let mut seen: HashSet<(String, usize)> = res
        .likely_tests
        .iter()
        .map(|test| (test.path.clone(), test.line))
        .collect();
    res.likely_tests.extend(
        then.into_iter()
            .filter(|test| seen.insert((test.path.clone(), test.line))),
    );
    if res.tests_past_depth.is_some() {
        res.past_depth_tests.retain(|key| !seen.contains(key));
        res.tests_past_depth = Some(res.past_depth_tests.len());
    }
    res.likely_tests_found = res.likely_tests.len();
    res.likely_tests_truncated |= res.likely_tests.len() > limit;
    res.likely_tests.truncate(limit);
    Ok(res)
}

/// Walks on from the callers that a join put at depth 1, with one level less depth, and adds what
/// that walk found one level deeper. A symbol that is listed already keeps the smaller of its
/// two depths, and the rows stay in depth order.
fn walk_on_from_joined(
    conn: &Connection,
    res: &mut BlastRadiusResult,
    added: &[String],
    depth: usize,
    limit: usize,
    include_low_signal: bool,
) -> Result<(), OpError> {
    if depth <= 1 || added.is_empty() {
        return Ok(());
    }
    let ids: Vec<&str> = added.iter().map(String::as_str).collect();
    let deeper = queries::compute_blast_radius_from_callers_with_low_signal(
        conn,
        &ids,
        depth - 1,
        limit,
        include_low_signal,
    )?;
    let mut listed: HashMap<(String, usize), usize> = res
        .impacted_symbols
        .iter()
        .enumerate()
        .map(|(index, row)| ((row.path.clone(), row.line), index))
        .collect();
    for row in &deeper.impacted_symbols {
        let row_depth = row.depth + 1;
        match listed.get(&(row.path.clone(), row.line)) {
            Some(&index) => {
                let listed_row = &mut res.impacted_symbols[index];
                listed_row.depth = listed_row.depth.min(row_depth);
            }
            None => {
                listed.insert((row.path.clone(), row.line), res.impacted_symbols.len());
                res.impacted_symbols.push(ImpactedSymbol {
                    depth: row_depth,
                    ..row.clone()
                });
            }
        }
    }
    res.impacted_symbols.sort_by_key(|row| row.depth);
    res.impacted_symbols_truncated |= deeper.impacted_symbols_truncated;
    follow_deeper(res, &deeper, depth);
    Ok(())
}

/// Adds what the walk from the report's callers found: its tests, its open ends, and whether it
/// stopped at the depth or the ceiling. Its rows sit one level below the target's callers.
fn follow_deeper(res: &mut BlastRadiusResult, deeper: &BlastRadiusResult, depth: usize) {
    let mut tests: HashSet<(String, usize)> = res
        .likely_tests
        .iter()
        .map(|test| (test.path.clone(), test.line))
        .collect();
    for test in &deeper.likely_tests {
        if tests.insert((test.path.clone(), test.line)) {
            res.likely_tests.push(TestTarget {
                reason: one_level_deeper(&test.reason),
                ..test.clone()
            });
        }
    }
    res.likely_tests_found = res.likely_tests.len();
    let mut ends: HashSet<(String, usize)> = res
        .open_ends
        .iter()
        .map(|end| (end.path.clone(), end.line))
        .collect();
    for end in &deeper.open_ends {
        if ends.insert((end.path.clone(), end.line)) {
            res.open_ends.push(OpenEnd {
                depth: end.depth + 1,
                ..end.clone()
            });
        }
    }
    if deeper.stopped_at_depth.is_some() {
        res.stopped_at_depth = Some(depth);
    }
    if deeper.tests_past_depth.is_some() {
        let mut past: HashSet<(String, usize)> = HashSet::new();
        res.past_depth_tests = std::mem::take(&mut res.past_depth_tests)
            .into_iter()
            .chain(deeper.past_depth_tests.iter().cloned())
            .filter(|key| !tests.contains(key) && past.insert(key.clone()))
            .collect();
        res.tests_past_depth = Some(res.past_depth_tests.len());
        res.tests_past_depth_cut |= deeper.tests_past_depth_cut;
    }
    res.traversal_ceiling_reached |= deeper.traversal_ceiling_reached;
    res.likely_tests_truncated |= deeper.likely_tests_truncated;
}

/// Whether a caller of the report holds a line that the diff changed, which makes it a seed of
/// the walk rather than a symbol the change reaches.
fn holds_changed_line(
    conn: &Connection,
    id: Option<&str>,
    row: &ImpactedSymbol,
    changes: &BTreeMap<String, Vec<(usize, usize)>>,
) -> Result<bool, OpError> {
    let Some(hunks) = changes.get(&row.path) else {
        return Ok(false);
    };
    let (first, last) = match id {
        Some(id) => match queries::get_symbol_by_id(conn, id)? {
            Some(symbol) => (symbol.start_line, symbol.end_line),
            None => return Ok(false),
        },
        None => (row.line, row.line),
    };
    Ok(hunks
        .iter()
        .any(|&(start, end)| start <= last && first <= end))
}

/// The path and line of every impacted symbol and test that the walk listed.
fn listed_rows(res: &BlastRadiusResult) -> HashSet<(String, usize)> {
    res.impacted_symbols
        .iter()
        .map(|row| (row.path.clone(), row.line))
        .chain(
            res.likely_tests
                .iter()
                .map(|test| (test.path.clone(), test.line)),
        )
        .collect()
}

/// The walk's impacted rows with the report's callers joined, and the ids of the callers that
/// the join put at depth 1, either new or moved up from deeper, so a walk can go on from them.
struct JoinedCallers {
    rows: Vec<ImpactedSymbol>,
    added: Vec<String>,
}

/// Joins the report's callers to the walk's rows as depth-1 rows: a caller that the walk listed
/// deeper moves to depth 1, and a caller that the walk did not list follows the walk's depth-1
/// rows. Both count as added, since the walk went on from a moved caller with less depth left.
fn join_report_callers(
    mut rows: Vec<ImpactedSymbol>,
    callers: impl IntoIterator<Item = (Option<String>, ImpactedSymbol)>,
) -> JoinedCallers {
    let mut at = rows.iter().take_while(|row| row.depth <= 1).count();
    let mut added = Vec::new();
    for (id, caller) in callers {
        match rows
            .iter()
            .position(|row| row.path == caller.path && row.line == caller.line)
        {
            Some(index) if rows[index].depth > 1 => {
                added.extend(id);
                let row = rows.remove(index);
                rows.insert(
                    at,
                    ImpactedSymbol {
                        depth: 1,
                        reason: caller.reason,
                        ..row
                    },
                );
                at += 1;
            }
            Some(_) => {}
            None => {
                added.extend(id);
                rows.insert(at, caller);
                at += 1;
            }
        }
    }
    JoinedCallers { rows, added }
}

/// The reason of a test row one call further from the target: `direct caller` becomes
/// `indirect caller [depth 2]`, and `indirect caller [depth N]` becomes depth N + 1.
fn one_level_deeper(reason: &str) -> String {
    const INDIRECT: &str = "indirect caller [depth ";
    if let Some(start) = reason.find(INDIRECT) {
        let digits = start + INDIRECT.len();
        if let Some(end) = reason[digits..].find(']')
            && let Ok(depth) = reason[digits..digits + end].parse::<usize>()
        {
            return format!(
                "{}{INDIRECT}{}{}",
                &reason[..start],
                depth + 1,
                &reason[digits + end..]
            );
        }
        return reason.to_string();
    }
    reason.replacen("direct caller", "indirect caller [depth 2]", 1)
}

/// A member access of a callable target that only its name ties to the target: no receiver
/// evidence supports it.
fn is_candidate_member_access(target: &ReferenceTarget, labeled: &LabeledSite) -> bool {
    matches!(target.kind.as_str(), "function" | "method" | "constructor")
        && labeled.site.kind.starts_with("member_access")
        && labeled.label == SiteLabel::Likely
        && (labeled.basis.starts_with("receiver not typed")
            || labeled.basis.starts_with("name only"))
}

/// The callers of the report's proven and likely sites that are not tests, as depth-1 rows in
/// report order. A site outside any symbol is a row of its own at the site's line. When the rows
/// join callers that the walk found, a candidate member access adds no row, and for a target with
/// overloads a call adds no row either: the report counts the calls of every overload, and the
/// walk already tied each call to the overload it reaches.
fn report_callers(
    conn: &Connection,
    report: &ReferenceReport,
    joins_walk: bool,
    walk_owns_calls: bool,
) -> Result<Vec<(Option<String>, ImpactedSymbol)>, OpError> {
    let tests: HashSet<&str> = report
        .tests
        .iter()
        .map(|test| test.symbol_id.as_str())
        .collect();
    let mut callers: Vec<(&str, Vec<&LabeledSite>)> = Vec::new();
    let mut positions: HashMap<&str, usize> = HashMap::new();
    for labeled in report.sites.iter().filter(|labeled| {
        matches!(labeled.label, SiteLabel::Proven | SiteLabel::Likely)
            && !(joins_walk && is_candidate_member_access(&report.target, labeled))
            && !(walk_owns_calls
                && matches!(
                    labeled.site.kind.as_str(),
                    "calls" | "call" | "instantiates"
                ))
    }) {
        let caller = labeled.site.from_symbol_id.as_str();
        match positions.get(caller).filter(|_| !caller.is_empty()) {
            Some(&position) => callers[position].1.push(labeled),
            None => {
                positions.insert(caller, callers.len());
                callers.push((caller, vec![labeled]));
            }
        }
    }
    let mut rows = Vec::new();
    for (caller, sites) in callers {
        let name_only = sites.iter().all(|labeled| {
            labeled.label == SiteLabel::Likely && labeled.basis.starts_with("name only")
        });
        let reason = Some(
            if name_only {
                "direct caller (find_references, name only)"
            } else {
                "direct caller (find_references)"
            }
            .to_string(),
        );
        if caller.is_empty() {
            let site = &sites[0].site;
            rows.push((
                None,
                ImpactedSymbol {
                    name: "(no enclosing symbol)".to_string(),
                    kind: "code".to_string(),
                    path: site.path.replace('\\', "/"),
                    line: site.start_line.unwrap_or(0),
                    depth: 1,
                    enclosing_symbol_name: None,
                    reason,
                },
            ));
            continue;
        }
        if tests.contains(caller) {
            continue;
        }
        let Some(symbol) = queries::get_symbol_by_id(conn, caller)? else {
            continue;
        };
        if symbol.is_test
            || symbol.test_container
            || (queries::is_test_path(&symbol.path) && queries::names_a_test(&symbol.name))
            || queries::enclosing_walk_test(conn, symbol.parent_symbol_id.as_deref())?.is_some()
        {
            continue;
        }
        let enclosing_symbol_name = if is_generated_lambda_name(
            &symbol.name,
            &symbol.language,
            &symbol.kind,
            symbol.signature.as_deref(),
        ) {
            symbol
                .parent_symbol_id
                .as_deref()
                .map(|parent| queries::get_symbol_by_id(conn, parent))
                .transpose()?
                .flatten()
                .map(|parent| parent.name)
        } else {
            None
        };
        rows.push((
            Some(symbol.symbol_id),
            ImpactedSymbol {
                name: symbol.name,
                kind: symbol.kind,
                path: symbol.path.replace('\\', "/"),
                line: symbol.start_line,
                depth: 1,
                enclosing_symbol_name,
                reason,
            },
        ));
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use std::time::Duration;

    use rusqlite::Connection;

    use super::{
        FileReportLimits, codebase_outline_op, file_blast_radius_within, one_level_deeper,
    };
    use crate::workspace::Workspace;
    use crate::{
        find_julie_extract_binary, formatters::format_blast_radius_report, open_read_only,
        safe_tempdir, scan_workspace,
    };

    const PACKAGE_APP: &str = "LIMIT = 3\n\n\nclass App:\n    def handle_error(self, error):\n        return error\n\n    def dispatch(self):\n        return self.handle_error(LIMIT)\n";

    fn scanned_package() -> (tempfile::TempDir, Workspace, Connection) {
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");
        let temp = safe_tempdir();
        let root = temp.path().to_path_buf();
        for (relative, text) in [
            ("pyproject.toml", "[project]\nname = \"pkg\"\n"),
            ("src/pkg/__init__.py", ""),
            ("src/pkg/app.py", PACKAGE_APP),
            (
                "src/pkg/test_app.py",
                "from pkg.app import App\n\n\ndef helper():\n    return App()\n\n\ndef test_dispatch():\n    assert helper().dispatch() == 3\n",
            ),
            (
                "tests/test_app.py",
                "from pkg.app import App\n\n\ndef test_dispatch():\n    app = App()\n    assert app.dispatch() == 3\n",
            ),
        ] {
            let full = root.join(relative);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, text).unwrap();
        }
        let db = root.join(".code-kb").join("artifact.db");
        let workspace = Workspace::new(root);
        scan_workspace(&workspace, &db, true).unwrap();
        let conn = open_read_only(&db).unwrap();
        (temp, workspace, conn)
    }

    fn limits(budget: Duration, report: Duration) -> FileReportLimits {
        FileReportLimits { budget, report }
    }

    #[test]
    fn a_folder_target_counts_the_symbols_past_its_budget_and_leaves_out_test_files() {
        let (_temp, workspace, conn) = scanned_package();

        let result = file_blast_radius_within(
            &workspace,
            &conn,
            "src/pkg",
            2,
            20,
            false,
            limits(Duration::ZERO, Duration::from_secs(3)),
        )
        .unwrap();

        assert_eq!(result.report_symbols_unchecked, Some(4));
        assert_eq!(result.report_symbols_failed, None);
        assert_eq!(result.report_sites_unchecked, None);
        let text = format_blast_radius_report(workspace.root.as_path(), &result, 0, 20, false);
        assert!(
            text.contains("find_references did not check 4 symbols of the target"),
            "{text}"
        );
    }

    #[test]
    fn a_file_target_counts_the_sites_its_reports_left_unchecked() {
        let (_temp, workspace, conn) = scanned_package();

        let result = file_blast_radius_within(
            &workspace,
            &conn,
            "src/pkg/app.py",
            2,
            20,
            false,
            limits(Duration::from_secs(3600), Duration::ZERO),
        )
        .unwrap();

        assert_eq!(result.report_symbols_unchecked, None);
        assert!(
            result.report_sites_unchecked.is_some_and(|sites| sites > 0),
            "{result:?}"
        );
        let text = format_blast_radius_report(workspace.root.as_path(), &result, 0, 20, false);
        assert!(
            text.contains("within the time limit, so callers and tests"),
            "{text}"
        );
        assert!(text.contains("### Broader Test Scope"), "{text}");
    }

    #[test]
    fn a_file_target_whose_reports_check_every_site_needs_no_broader_suite() {
        let (_temp, workspace, conn) = scanned_package();

        let result = file_blast_radius_within(
            &workspace,
            &conn,
            "src/pkg/app.py",
            2,
            20,
            false,
            limits(Duration::from_secs(3600), Duration::from_secs(3)),
        )
        .unwrap();

        assert_eq!(result.report_symbols_unchecked, None);
        assert_eq!(result.report_symbols_failed, None);
        assert_eq!(result.report_sites_unchecked, None);
        let text = format_blast_radius_report(workspace.root.as_path(), &result, 0, 20, false);
        assert!(!text.contains("### Broader Test Scope"), "{text}");
    }

    #[test]
    fn a_file_target_counts_the_symbols_whose_report_failed() {
        let (_temp, workspace, conn) = scanned_package();
        drop(conn);
        let conn = Connection::open(workspace.root.join(".code-kb").join("artifact.db")).unwrap();
        conn.execute_batch("DROP TABLE identifiers").unwrap();

        let result = file_blast_radius_within(
            &workspace,
            &conn,
            "src/pkg/app.py",
            2,
            20,
            false,
            limits(Duration::from_secs(3600), Duration::from_secs(3)),
        )
        .unwrap();

        assert_eq!(result.report_symbols_failed, Some(4));
        assert_eq!(result.likely_tests[0].name, "test_dispatch");
        let text = format_blast_radius_report(workspace.root.as_path(), &result, 0, 20, false);
        assert!(
            text.contains("find_references failed for 4 symbols of the target"),
            "{text}"
        );
        assert!(text.contains("### Broader Test Scope"), "{text}");
    }

    #[test]
    fn a_test_one_call_further_from_the_target_is_one_level_deeper() {
        assert_eq!(
            one_level_deeper("direct caller"),
            "indirect caller [depth 2]"
        );
        assert_eq!(
            one_level_deeper("encloses `index` (indirect caller [depth 2])"),
            "encloses `index` (indirect caller [depth 3])"
        );
        assert_eq!(one_level_deeper("uses fixture `app`"), "uses fixture `app`");
    }

    #[test]
    fn codebase_outline_accepts_absolute_workspace_root_filter() {
        let temp = crate::safe_tempdir();
        fs::write(temp.path().join("root.rs"), "pub fn root() {}\n").unwrap();
        let workspace = Workspace::new(temp.path().to_path_buf());
        let conn = Connection::open(temp.path().join("index.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE files (
                file_id TEXT, path TEXT, language TEXT, content_hash TEXT,
                content_bytes INTEGER, line_count INTEGER, indexed_at TEXT
            );
            CREATE TABLE symbols (
                symbol_id TEXT, file_id TEXT, path TEXT, language TEXT, name TEXT, kind TEXT,
                signature TEXT, doc_comment TEXT, visibility TEXT, parent_symbol_id TEXT,
                start_line INTEGER, start_column INTEGER, end_line INTEGER, end_column INTEGER,
                start_byte INTEGER, end_byte INTEGER, body_start_line INTEGER,
                body_start_column INTEGER, body_end_line INTEGER, body_end_column INTEGER,
                body_start_byte INTEGER, body_end_byte INTEGER, body_hash TEXT,
                semantic_group TEXT, is_test INTEGER, test_container INTEGER
            );
            INSERT INTO files VALUES ('f', 'root.rs', 'rust', 'hash', 17, 1, 'now');
            INSERT INTO symbols VALUES (
                's', 'f', 'root.rs', 'rust', 'root', 'function', 'pub fn root()', NULL,
                'pub', NULL, 1, 0, 1, 16, 0, 16, 1, 0, 1, 16, 0, 16, NULL, NULL, 0, 0
            );",
        )
        .unwrap();

        let outline =
            codebase_outline_op(&workspace, &conn, 1, Some(temp.path().to_str().unwrap())).unwrap();

        assert!(outline.contains("root.rs"));

        fs::create_dir(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("src/lib.rs"), "pub fn nested() {}\n").unwrap();
        conn.execute_batch(
            "INSERT INTO files VALUES ('nested-file', 'src/lib.rs', 'rust', 'hash', 19, 1, 'now');
             INSERT INTO symbols VALUES (
                'nested-symbol', 'nested-file', 'src/lib.rs', 'rust', 'nested', 'function',
                'pub fn nested()', NULL, 'pub', NULL, 1, 0, 1, 18, 0, 18, 1, 0, 1, 18, 0, 18,
                NULL, NULL, 0, 0
             );",
        )
        .unwrap();

        assert!(
            codebase_outline_op(&workspace, &conn, 2, Some("."))
                .unwrap()
                .contains("root.rs")
        );
        assert!(
            codebase_outline_op(&workspace, &conn, 1, Some("src"))
                .unwrap()
                .contains("lib.rs")
        );
        assert!(
            codebase_outline_op(
                &workspace,
                &conn,
                1,
                Some(temp.path().join("src").to_str().unwrap()),
            )
            .unwrap()
            .contains("lib.rs")
        );
        assert!(
            codebase_outline_op(
                &workspace,
                &conn,
                1,
                Some(temp.path().parent().unwrap().to_str().unwrap()),
            )
            .is_err()
        );
    }

    #[test]
    fn codebase_outline_counts_unsupported_files() {
        let temp = crate::safe_tempdir();
        let workspace = Workspace::new(temp.path().to_path_buf());
        let conn = Connection::open(temp.path().join("index.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE files (
                file_id TEXT, path TEXT, language TEXT, content_hash TEXT,
                content_bytes INTEGER, line_count INTEGER, indexed_at TEXT, status TEXT
            );
            CREATE TABLE symbols (
                symbol_id TEXT, file_id TEXT, path TEXT, language TEXT, name TEXT, kind TEXT,
                signature TEXT, doc_comment TEXT, visibility TEXT, parent_symbol_id TEXT,
                start_line INTEGER, start_column INTEGER, end_line INTEGER, end_column INTEGER,
                start_byte INTEGER, end_byte INTEGER, body_start_line INTEGER,
                body_start_column INTEGER, body_end_line INTEGER, body_end_column INTEGER,
                body_start_byte INTEGER, body_end_byte INTEGER, body_hash TEXT,
                semantic_group TEXT, is_test INTEGER, test_container INTEGER
            );
            INSERT INTO files VALUES ('f1', 'src/lib.rs', 'rust', 'h', 0, 1, 'now', 'indexed');
            INSERT INTO files VALUES ('f2', 'src/blob.bin', 'unknown', 'h', 0, 1, 'now', 'unsupported');
            INSERT INTO files VALUES ('f3', 'docs/blob.bin', 'unknown', 'h', 0, 1, 'now', 'unsupported');
            INSERT INTO files VALUES ('f4', 'src/assets/a.png', 'unknown', 'h', 0, 1, 'now', 'unsupported');
            INSERT INTO symbols VALUES (
                's', 'f1', 'src/lib.rs', 'rust', 'root', 'function', 'pub fn root()', NULL,
                'pub', NULL, 1, 0, 1, 16, 0, 16, 1, 0, 1, 16, 0, 16, NULL, NULL, 0, 0
            );",
        )
        .unwrap();

        let all = codebase_outline_op(&workspace, &conn, 2, None).unwrap();
        assert!(all.contains("3 unsupported files"));
        assert!(all.contains("docs/ (1 unsupported file)"), "{all}");
        assert!(all.contains("src/ (2 unsupported files)"), "{all}");
        assert!(!all.contains("assets/"), "{all}");

        let scoped = codebase_outline_op(&workspace, &conn, 2, Some("src")).unwrap();
        assert!(scoped.contains("2 unsupported files:"), "{scoped}");
    }

    #[test]
    fn codebase_outline_names_plain_files_and_shows_an_overload_once() {
        let temp = crate::safe_tempdir();
        let workspace = Workspace::new(temp.path().to_path_buf());
        let conn = Connection::open(temp.path().join("index.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE files (
                file_id TEXT, path TEXT, language TEXT, content_hash TEXT,
                content_bytes INTEGER, line_count INTEGER, indexed_at TEXT, status TEXT
            );
            CREATE TABLE symbols (
                symbol_id TEXT, file_id TEXT, path TEXT, language TEXT, name TEXT, kind TEXT,
                signature TEXT, doc_comment TEXT, visibility TEXT, parent_symbol_id TEXT,
                start_line INTEGER, start_column INTEGER, end_line INTEGER, end_column INTEGER,
                start_byte INTEGER, end_byte INTEGER, body_start_line INTEGER,
                body_start_column INTEGER, body_end_line INTEGER, body_end_column INTEGER,
                body_start_byte INTEGER, body_end_byte INTEGER, body_hash TEXT,
                semantic_group TEXT, is_test INTEGER, test_container INTEGER
            );
            INSERT INTO files VALUES ('f1', 'src/helpers.py', 'python', 'h', 0, 9, 'now', 'indexed');
            INSERT INTO files VALUES ('f2', 'src/signals.py', 'python', 'h', 0, 3, 'now', 'indexed');
            INSERT INTO files VALUES ('f3', 'src/py.typed', 'unknown', 'h', 0, 0, 'now', 'unsupported');
            INSERT INTO symbols VALUES (
                'a', 'f1', 'src/helpers.py', 'python', 'stream', 'function', 'def stream()', NULL,
                NULL, NULL, 1, 0, 2, 0, 0, 10, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0
            );
            INSERT INTO symbols VALUES (
                'b', 'f1', 'src/helpers.py', 'python', 'stream', 'function', 'def stream(x)', NULL,
                NULL, NULL, 4, 0, 5, 0, 11, 20, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0
            );
            INSERT INTO symbols VALUES (
                'c', 'f2', 'src/signals.py', 'python', 'started', 'variable', 'started = Signal()',
                NULL, NULL, NULL, 1, 0, 1, 0, 0, 18, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0
            );",
        )
        .unwrap();

        let out = codebase_outline_op(&workspace, &conn, 2, Some("src")).unwrap();

        assert!(out.contains("helpers.py [function stream]\n"), "{out}");
        assert!(
            out.contains("(1 file without functions or classes: signals.py)"),
            "{out}"
        );
        assert!(!out.contains("py.typed"), "{out}");
    }

    #[test]
    fn codebase_outline_lists_namespaced_classes_past_deep_files_and_caps_each_subfolder() {
        let temp = crate::safe_tempdir();
        let workspace = Workspace::new(temp.path().to_path_buf());
        let conn = Connection::open(temp.path().join("index.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE files (
                file_id TEXT, path TEXT, language TEXT, content_hash TEXT,
                content_bytes INTEGER, line_count INTEGER, indexed_at TEXT, status TEXT
            );
            CREATE TABLE symbols (
                symbol_id TEXT, file_id TEXT, path TEXT, language TEXT, name TEXT, kind TEXT,
                signature TEXT, doc_comment TEXT, visibility TEXT, parent_symbol_id TEXT,
                start_line INTEGER, start_column INTEGER, end_line INTEGER, end_column INTEGER,
                start_byte INTEGER, end_byte INTEGER, body_start_line INTEGER,
                body_start_column INTEGER, body_end_line INTEGER, body_end_column INTEGER,
                body_start_byte INTEGER, body_end_byte INTEGER, body_hash TEXT,
                semantic_group TEXT, is_test INTEGER, test_container INTEGER
            );
            INSERT INTO files VALUES ('z', 'z/Late.cs', 'csharp', 'h', 0, 9, 'now', 'indexed');
            INSERT INTO symbols VALUES (
                'ns', 'z', 'z/Late.cs', 'csharp', 'App', 'namespace', 'namespace App', NULL,
                NULL, NULL, 1, 0, 9, 0, 0, 90, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0
            );
            INSERT INTO symbols VALUES (
                'late', 'z', 'z/Late.cs', 'csharp', 'Late', 'class', 'class Late', NULL,
                NULL, 'ns', 2, 0, 8, 0, 20, 80, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0
            );",
        )
        .unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        for i in 0..1005 {
            tx.execute(
                "INSERT INTO files VALUES (?1, ?2, 'rust', 'h', 0, 1, 'now', 'indexed')",
                rusqlite::params![format!("d{i}"), format!("a/deep/x/file_{i}.rs")],
            )
            .unwrap();
        }
        for i in 10..55 {
            let path = format!("m/mod_{i}.rs");
            tx.execute(
                "INSERT INTO files VALUES (?1, ?2, 'rust', 'h', 0, 1, 'now', 'indexed')",
                rusqlite::params![format!("m{i}"), path],
            )
            .unwrap();
            tx.execute(
                "INSERT INTO symbols VALUES (?1, ?2, ?3, 'rust', ?4, 'function', 'fn f()', NULL,
                 NULL, NULL, 1, 0, 1, 0, 0, 9, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0)",
                rusqlite::params![format!("s{i}"), format!("m{i}"), path, format!("f{i}")],
            )
            .unwrap();
        }
        tx.commit().unwrap();

        let out = codebase_outline_op(&workspace, &conn, 2, None).unwrap();

        assert!(out.contains("deep/ (1005 indexed files)"), "{out}");
        assert!(out.contains("Late.cs [class Late]"), "{out}");
        assert!(out.contains("mod_49.rs [function f49]"), "{out}");
        assert!(!out.contains("mod_50.rs"), "{out}");
        assert!(
            out.contains("(+5 more files with functions or classes; pass this folder as the path"),
            "{out}"
        );
        assert!(!out.contains("truncated"), "{out}");

        let scoped = codebase_outline_op(&workspace, &conn, 2, Some("m")).unwrap();
        assert!(scoped.contains("mod_54.rs [function f54]"), "{scoped}");
    }

    #[test]
    fn codebase_outline_truncates_over_1000_files() {
        let temp = crate::safe_tempdir();
        let workspace = Workspace::new(temp.path().to_path_buf());
        let conn = Connection::open(temp.path().join("index.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE files (
                file_id TEXT, path TEXT, language TEXT, content_hash TEXT,
                content_bytes INTEGER, line_count INTEGER, indexed_at TEXT
            );
            CREATE TABLE symbols (
                symbol_id TEXT, file_id TEXT, path TEXT, language TEXT, name TEXT, kind TEXT,
                signature TEXT, doc_comment TEXT, visibility TEXT, parent_symbol_id TEXT,
                start_line INTEGER, start_column INTEGER, end_line INTEGER, end_column INTEGER,
                start_byte INTEGER, end_byte INTEGER, body_start_line INTEGER,
                body_start_column INTEGER, body_end_line INTEGER, body_end_column INTEGER,
                body_start_byte INTEGER, body_end_byte INTEGER, body_hash TEXT,
                semantic_group TEXT, is_test INTEGER, test_container INTEGER
            );",
        )
        .unwrap();

        let tx = conn.unchecked_transaction().unwrap();
        for i in 1..=1005 {
            tx.execute(
                "INSERT INTO files VALUES (?1, ?2, 'rust', 'hash', 10, 1, 'now')",
                rusqlite::params![format!("f{i}"), format!("src/file_{i}.rs")],
            )
            .unwrap();
        }
        tx.commit().unwrap();

        let outline = codebase_outline_op(&workspace, &conn, 2, None).unwrap();
        assert!(outline.contains("[Outline truncated: workspace contains over 1,000 files."));

        let scoped_outline = codebase_outline_op(&workspace, &conn, 2, Some("src")).unwrap();
        assert!(scoped_outline.contains("[Outline truncated: path matches over 1,000 files."));
    }
}
