use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use code_kb_core::formatters::ZERO_LIMIT_NOTICE;
use code_kb_core::workspace::{NO_PROJECT_MARKER_REASON, paths_equal};
use code_kb_core::{
    Connection, MAX_CONTEXT_OVERLOADS, SymbolSelector, WatcherHandle, Workspace, WorkspaceError,
    create_index, edit_check_op, ensure_fts_index_path, ensure_index_matches_extractor,
    file_sizes_for_paths, file_skeleton_op, find_references_for_symbol_ext, fit_bodies_to_budget,
    fit_context_slices_to_budget, format_context_slices, format_symbol_bodies,
    get_context_slices_selected_op, get_symbol_bodies_selected_op, installed_extractor_version,
    is_project_root, is_unreadable_database, lookup_symbols_op, open_global_telemetry_db,
    open_read_only, reconcile_offline_edits, record_tool_call, record_tool_call_conn,
    reference_report, render_symbol_lookup, resolve_symbol_op, scan_remedy,
    select_reference_target, start_watcher,
};

use super::protocol::{CallToolResult, JsonRpcRequest, JsonRpcResponse, Tool};

const PROJECT_ROOT_DESCRIPTION: &str =
    "Absolute path of the project or git worktree you work in. Change it when you move to another.";

type IndexPrepare = std::thread::JoinHandle<Result<Option<WatcherHandle>, String>>;

fn index_prepare_panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    let message = payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown preparation panic".to_string());
    format!(
        "Index preparation thread panicked: {}",
        message.chars().take(512).collect::<String>()
    )
}

/// The result of a finished prepare; a panic becomes its error.
fn join_index_prepare(prepare: IndexPrepare) -> Result<Option<WatcherHandle>, String> {
    prepare
        .join()
        .unwrap_or_else(|panic| Err(index_prepare_panic_message(panic)))
}

/// The error that each call for `root` returns after the rebuild of its unreadable index failed
/// with `error`. It names the scan that rebuilds the index.
fn failed_rebuild_message(root: &Path, error: &str) -> String {
    let remedy = scan_remedy(root);
    let error = error.trim_end();
    let root = root.display();
    if error.contains(&remedy) {
        format!("Index preparation of '{root}' failed: {error}")
    } else {
        format!("Index preparation of '{root}' failed: {error}. {remedy}")
    }
}

/// A prepare of a root that is no longer active, with that root's index file.
struct ParkedPrepare {
    root: PathBuf,
    db_path: PathBuf,
    prepare: IndexPrepare,
}

pub struct McpServer {
    pub workspace: Workspace,
    pub db_path: PathBuf,
    pub explicit_db: Option<PathBuf>,
    pub _watcher: Option<WatcherHandle>,
    pub telemetry_conn: Option<Connection>,
    /// Index preparation of the active root: a full scan when the index is missing,
    /// otherwise a reconciliation against files that changed while no watcher ran. It also
    /// starts the root's watcher and hands it over when a call joins it. A tool call waits a
    /// bounded time for it so it never answers from a missing or stale index.
    reconcile: Option<IndexPrepare>,
    /// The canonical root the server started with. `--db` pins only this root, and a
    /// refused call records it in telemetry.
    launch_root: PathBuf,
    /// Prepares still running on roots the server left. A scan writes into `artifact.db`
    /// in place, so a root that becomes active again reuses its running prepare and the
    /// watcher that prepare starts.
    parked: Vec<ParkedPrepare>,
    /// False while the active root has had no prepare, which happens when the startup
    /// pre-warm is refused.
    prepared: bool,
    /// `clientInfo.name` of the `initialize` request, for telemetry.
    client: Option<String>,
    /// Roots whose unreadable index failed to rebuild, with the error that call returned. The
    /// server does not rebuild such an index again while it stays unreadable; each call returns
    /// that error, which names the scan that rebuilds it.
    failed_rebuilds: Vec<(PathBuf, String)>,
}

/// Resolves `input` like `Workspace::from_project_root`, except that a launch root with
/// no project marker is accepted when `--db` names an existing file: that file is its index.
/// True when a tool call naming `root` as `project_root` would be accepted from a server
/// launched there with `explicit_db`.
pub(crate) fn accepts_root(root: &Path, explicit_db: Option<&Path>) -> bool {
    resolve_root(&root.to_string_lossy(), root, explicit_db).is_ok()
}

/// True for a listed tool, or its `impact` alias, whose schema requires `project_root`. An
/// unknown name returns false so its answer is "Unknown tool", not a missing-root error.
fn requires_project_root(name: &str) -> bool {
    let name = if name == "impact" {
        "blast_radius"
    } else {
        name
    };
    McpServer::tool_definitions().iter().any(|tool| {
        tool.name == name
            && tool.input_schema["required"]
                .as_array()
                .is_some_and(|required| required.contains(&json!("project_root")))
    })
}

fn resolve_root(
    input: &str,
    launch_root: &Path,
    explicit_db: Option<&Path>,
) -> Result<Workspace, WorkspaceError> {
    match Workspace::from_project_root(input) {
        Err(WorkspaceError::ProjectRootRefused { path, reason })
            if reason == NO_PROJECT_MARKER_REASON
                && paths_equal(&path, launch_root)
                && explicit_db.is_some_and(Path::is_file) =>
        {
            Ok(Workspace::new(path))
        }
        resolved => resolved,
    }
}

/// Prepares the index on a background thread. The thread owns the root's watcher: it starts
/// the watcher once the index exists, before the offline reconcile, and returns it, so an
/// edit made after the prepare reaches the index before any call joins the thread. Drop the
/// previous watcher before the spawn. Its worker stops at its next check, and the index
/// rebuild, `create_index`, and `start_watcher` each wait for it, so one root never has two
/// workers.
fn spawn_index_prepare(workspace: &Workspace, db_path: &Path) -> Option<IndexPrepare> {
    if !db_path.exists() && !is_project_root(&workspace.canonical_root) {
        return None;
    }
    let ws = workspace.clone();
    let db = db_path.to_path_buf();
    Some(std::thread::spawn(move || {
        ensure_index_matches_extractor(&ws, &db, &installed_extractor_version())
            .map_err(|e| e.to_string())?;
        if !db.exists() {
            tracing::info!(ws = %ws.canonical_root.display(), "Database not found; running automatic initial scan");
            create_index(&ws, &db).map_err(|e| e.to_string())?;
        }
        ensure_fts_index_path(&db).map_err(|e| e.to_string())?;
        let watcher = start_watcher(ws.clone(), db.clone()).map_err(|e| e.to_string())?;
        let conn = open_read_only(&db).map_err(|e| e.to_string())?;
        reconcile_offline_edits(&ws, &db, &conn).map_err(|e| e.to_string())?;
        Ok(Some(watcher))
    }))
}

fn result_limit(arguments: &Value, default: usize) -> Result<usize, CallToolResult> {
    match arguments.get("limit") {
        None => Ok(default),
        Some(value) => {
            let parsed = value
                .as_u64()
                .or_else(|| value.as_str().and_then(|s| s.parse::<u64>().ok()));
            match parsed {
                Some(limit) if limit <= code_kb_core::queries::MAX_RESULT_LIMIT as u64 => {
                    Ok(limit as usize)
                }
                _ => Err(CallToolResult::error(format!(
                    "Invalid limit: expected an integer between 0 and {}",
                    code_kb_core::queries::MAX_RESULT_LIMIT
                ))),
            }
        }
    }
}

fn result_offset(arguments: &Value) -> Result<usize, CallToolResult> {
    match arguments.get("offset") {
        None => Ok(0),
        Some(value) => match value
            .as_u64()
            .or_else(|| value.as_str().and_then(|s| s.parse::<u64>().ok()))
            .and_then(|offset| usize::try_from(offset).ok())
        {
            Some(offset) => Ok(offset),
            None => Err(CallToolResult::error(
                "Invalid offset: expected a nonnegative integer representable on this platform",
            )),
        },
    }
}

fn result_page(
    arguments: &Value,
    default_limit: usize,
) -> Result<(usize, usize, usize), CallToolResult> {
    let limit = result_limit(arguments, default_limit)?;
    let offset = result_offset(arguments)?;
    let end = code_kb_core::queries::validate_result_page(offset, limit)
        .map_err(|error| CallToolResult::error(error.to_string()))?;
    Ok((limit, offset, end))
}

impl McpServer {
    pub fn new(workspace: Workspace, explicit_db: Option<&Path>) -> anyhow::Result<Self> {
        let db_path = workspace.locate_db(explicit_db).unwrap_or_else(|_| {
            workspace
                .canonical_root
                .join(".code-kb")
                .join("artifact.db")
        });

        let (reconcile, prepared) = match resolve_root(
            &workspace.canonical_root.to_string_lossy(),
            &workspace.canonical_root,
            explicit_db,
        ) {
            Ok(_) => (spawn_index_prepare(&workspace, &db_path), true),
            Err(e) => {
                tracing::info!(
                    root = %workspace.canonical_root.display(),
                    "Startup index skipped: {e}"
                );
                (None, false)
            }
        };

        let telemetry_conn = open_global_telemetry_db().ok();

        Ok(Self {
            launch_root: workspace.canonical_root.clone(),
            workspace,
            db_path,
            explicit_db: explicit_db.map(|p| p.to_path_buf()),
            _watcher: None,
            telemetry_conn,
            reconcile,
            parked: Vec::new(),
            prepared,
            client: None,
            failed_rebuilds: Vec::new(),
        })
    }

    /// Makes an already resolved root the active one and prepares its index, unless that
    /// root is already active and prepared. The prepare thread starts the root's watcher
    /// once its index exists.
    fn switch_root(&mut self, workspace: Workspace) {
        if self.prepared && paths_equal(&self.workspace.canonical_root, &workspace.canonical_root) {
            return;
        }
        // ponytail: one watcher; alternating roots restart the watcher and the offline
        // reconcile on each switch. Keep a small per-root cache only if telemetry shows
        // agents alternate often.
        let explicit_db = if paths_equal(&workspace.canonical_root, &self.launch_root) {
            self.explicit_db.as_deref()
        } else {
            None
        };
        let db_path = workspace.locate_db(explicit_db).unwrap_or_else(|_| {
            workspace
                .canonical_root
                .join(".code-kb")
                .join("artifact.db")
        });
        tracing::info!(
            workspace = %workspace.canonical_root.display(),
            db = %db_path.display(),
            "Activated project root"
        );

        self._watcher = None;
        self.drop_finished_parked_prepares();
        if let Some(prepare) = self.reconcile.take() {
            let left = ParkedPrepare {
                root: self.workspace.canonical_root.clone(),
                db_path: self.db_path.clone(),
                prepare,
            };
            if left.prepare.is_finished() {
                self.settle_inactive_prepare(left);
            } else {
                self.parked.push(left);
            }
        }
        let parked = self
            .parked
            .iter()
            .position(|parked| paths_equal(&parked.root, &workspace.canonical_root))
            .map(|index| self.parked.swap_remove(index).prepare);
        self.prepared = true;
        self.workspace = workspace;
        self.db_path = db_path;
        self.reconcile = match parked {
            Some(prepare) => Some(prepare),
            None if self.failed_rebuild_error().is_some() => None,
            None => spawn_index_prepare(&self.workspace, &self.db_path),
        };
    }

    /// The error of the failed rebuild of the active root's index while that index stays
    /// unreadable. A readable index, such as one that `code-kb scan` rebuilt, clears the record.
    fn failed_rebuild_error(&mut self) -> Option<String> {
        let index = self
            .failed_rebuilds
            .iter()
            .position(|(root, _)| paths_equal(root, &self.workspace.canonical_root))?;
        if is_unreadable_database(&self.db_path) {
            return Some(self.failed_rebuilds[index].1.clone());
        }
        self.failed_rebuilds.swap_remove(index);
        None
    }

    /// Records that the rebuild of the unreadable index of `root` failed, with the error that
    /// each call for that root returns until the index is readable again.
    fn record_failed_rebuild(&mut self, root: &Path, message: String) {
        self.failed_rebuilds
            .retain(|(failed, _)| !paths_equal(failed, root));
        self.failed_rebuilds.push((root.to_path_buf(), message));
    }

    /// Reads the result of a finished prepare of a root that is no longer active and drops the
    /// watcher it started. A failed rebuild of an index that stays unreadable is recorded, so
    /// switching back to that root returns its error instead of rebuilding again.
    fn settle_inactive_prepare(&mut self, parked: ParkedPrepare) {
        if let Err(error) = join_index_prepare(parked.prepare)
            && is_unreadable_database(&parked.db_path)
        {
            let message = failed_rebuild_message(&parked.root, &error);
            tracing::error!("{}", message);
            self.record_failed_rebuild(&parked.root, message);
        }
    }

    /// Settles each parked prepare that has finished, and drops with it the watcher it started
    /// for a root that is no longer active. Switching back to that root runs a new prepare,
    /// whose offline reconcile picks up edits made in between, unless its rebuild failed.
    fn drop_finished_parked_prepares(&mut self) {
        let finished: Vec<ParkedPrepare> = self
            .parked
            .extract_if(.., |parked| parked.prepare.is_finished())
            .collect();
        for parked in finished {
            self.settle_inactive_prepare(parked);
        }
    }

    /// Resolves a call's `project_root` (or its silent aliases `workspace` and `root`) and
    /// checks that every path argument, relative or absolute, lies inside it and outside any
    /// nested git worktree or submodule.
    fn resolve_project_root(&self, arguments: &Value) -> Result<Workspace, String> {
        let input = ["project_root", "workspace", "root"]
            .into_iter()
            .find_map(|key| arguments.get(key).and_then(Value::as_str))
            .filter(|input| !input.trim().is_empty())
            .ok_or_else(|| {
                "Missing required parameter: project_root. Pass the absolute path of the project or git worktree you are working in.".to_string()
            })?;
        let workspace = resolve_root(input, &self.launch_root, self.explicit_db.as_deref())
            .map_err(|e| {
                format!("{e}. Pass the absolute path of the project or git worktree you are working in as project_root.")
            })?;
        let root = workspace.canonical_root.display();
        for key in ["path", "file_path", "file"] {
            let Some(path) = arguments.get(key).and_then(Value::as_str) else {
                continue;
            };
            if let Err(WorkspaceError::PathOutsideWorkspace(..)) =
                workspace.resolve_path(Path::new(path))
            {
                return Err(format!(
                    "Path '{path}' is outside project_root '{root}'. Pass a path inside project_root, or change project_root to the project that holds the path."
                ));
            }
            if let Some(nested) = workspace.nested_project_root(Path::new(path)) {
                let nested = nested.display();
                return Err(format!(
                    "Path '{path}' belongs to the nested project '{nested}', not to project_root '{root}'. Pass '{nested}' as project_root."
                ));
            }
        }
        Ok(workspace)
    }

    pub fn tool_definitions() -> Vec<Tool> {
        let mut tools = vec![
            Tool {
                name: "file_skeleton".to_string(),
                description: "A file's types, functions, signatures, and docstrings with bodies stripped. Call this before you read a source file with Read, cat, or sed, then fetch the symbols you need with get_symbol_body; read the whole file only when you will change most of it. A directory returns its outline.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "file_path": {
                            "type": "string",
                            "description": "File or directory path."
                        }
                    },
                    "required": ["file_path"]
                }),
            },
            Tool {
                name: "lookup_symbol".to_string(),
                description: "Find symbols by name, qualified name ('Type::method'), prefix, or concept; returns kind, path, line, signature, and id. A query that names no symbol exactly also gets the ranked matches by name words, signatures, and docs. Use instead of grep on a name.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Identifier, qualified name, prefix, or a few words that describe the code."
                        },
                        "path": {
                            "type": "string",
                            "description": "File or directory to search in."
                        },
                        "kind": {
                            "type": "string",
                            "description": "Kind, such as function or class."
                        },
                        "is_test": {
                            "type": "boolean",
                            "description": "Include rows from tests (default: false)."
                        },
                        "limit": {
                            "type": "integer",
                            "minimum": 0,
                            "maximum": code_kb_core::queries::MAX_RESULT_LIMIT,
                            "description": "Default: 20."
                        }
                    },
                    "required": ["query"]
                }),
            },
            Tool {
                name: "get_symbol_body".to_string(),
                description: "The source of one symbol, with its declaration and decorators; overloads return each body. Use instead of reading a line range with Read, cat, or sed -n. Before an edit, use get_symbol_context.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "symbol_name": {
                            "type": "string",
                            "description": "Name or qualified name; pass exactly one of this or symbol_id."
                        },
                        "symbol_id": { "type": "string", "description": "The id from lookup_symbol; pass exactly one of this or symbol_name." },
                        "file_path": {
                            "type": "string",
                            "description": "File or folder that holds the symbol."
                        }
                    }
                }),
            },
            Tool {
                name: "get_symbol_context".to_string(),
                description: format!("A symbol's body with the signatures of what it calls, its parameter types, and its tests, in one call. Overloads return one context each, at most {MAX_CONTEXT_OVERLOADS}. Use before you change a function."),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "symbol_name": {
                            "type": "string",
                            "description": "Name or qualified name; pass exactly one of this or symbol_id."
                        },
                        "symbol_id": { "type": "string", "description": "The id from lookup_symbol; pass exactly one of this or symbol_name." },
                        "file_path": {
                            "type": "string",
                            "description": "File or folder that holds the symbol."
                        },
                        "include_external": {
                            "type": "boolean",
                            "description": "Include library calls (default: false)."
                        }
                    }
                }),
            },
            Tool {
                name: "find_references".to_string(),
                description: "Callers of a symbol: every indexed site with its name, each labeled proven, likely, unresolved, or excluded by its evidence, plus a whole-word text count that accounts for every line of the name. Use instead of rg or grep on a name. Callees: the calls a symbol makes. Pass file_path or a qualified name for a common name. Text reports support bounded per-section pages; each call reads the current index, keep the same project_root and filter, and restart at offset 0 after index changes. Page limit is at most 200; offset may extend beyond 200.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "symbol_name": {
                            "type": "string",
                            "description": "Name or qualified name; pass exactly one of this or symbol_id."
                        },
                        "symbol_id": { "type": "string", "description": "The id from lookup_symbol; pass exactly one of this or symbol_name." },
                        "file_path": {
                            "type": "string",
                            "description": "File or folder that holds the symbol."
                        },
                        "direction": {
                            "type": "string",
                            "enum": ["callers", "callees"],
                            "description": "Default: callers."
                        },
                        "limit": {
                            "type": "integer",
                            "minimum": 0,
                            "maximum": code_kb_core::queries::MAX_RESULT_LIMIT,
                            "description": "Default: 20."
                        },
                        "offset": {
                            "type": "integer",
                            "minimum": 0,
                            "description": "Text page offset in each report section (default: 0). restart at offset 0 after an index change. The offset may exceed 200."
                        },
                        "include_external": {
                            "type": "boolean",
                            "description": "Include library callees (default: false)."
                        }
                    }
                }),
            },
            Tool {
                name: "blast_radius".to_string(),
                description: "The symbols a change affects and the tests to run. With no symbol, symbol_id, or file, it reads the uncommitted git changes. Text output supports bounded per-section pages and optional low-signal import, variable, parameter, field, property, module, and namespace rows. Each call reads the current index; keep the same project_root and filter, and restart at offset 0 after index changes. The page limit is at most 200; discovery stops at 200 rows and larger offsets return no rows.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "symbol": {
                            "type": "string",
                            "description": "Symbol name."
                        },
                        "symbol_id": { "type": "string", "description": "The id from lookup_symbol; pass one of this or symbol." },
                        "file": {
                            "type": "string",
                            "description": "File path."
                        },
                        "depth": {
                            "type": "integer",
                            "maximum": 5,
                            "description": "Hops (default: 2, at most 5)."
                        },
                        "limit": {
                            "type": "integer",
                            "minimum": 0,
                            "maximum": code_kb_core::queries::MAX_RESULT_LIMIT,
                            "description": "Per section (default: 20)."
                        },
                        "offset": {
                            "type": "integer",
                            "minimum": 0,
                            "description": "Text page offset in the test and impact sections (default: 0). restart at offset 0 after an index change. Larger offsets return no rows after the 200-row discovery ceiling."
                        },
                        "include_low_signal": {
                            "type": "boolean",
                            "description": "Include low-signal import, variable, parameter, field, property, module, and namespace impact rows (default: false)."
                        }
                    }
                }),
            },
            Tool {
                name: "check_edit".to_string(),
                description: "After you edit: the symbols whose shape changed against the session base (or base, or HEAD) and every site of each that your diff did not reach. Run it before you say a change is done. Text output supports bounded pages for each folded section; each call reads the current index, keep the same project_root and base, and restart at offset 0 after index changes. The default page size is 12; page limit is at most 200 and offset may extend beyond 200.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "base": {
                            "type": "string",
                            "description": "Git revision to compare with (default: the session base, else HEAD)."
                        },
                        "offset": {
                            "type": "integer",
                            "minimum": 0,
                            "description": "Text page offset per folded section (default: 0). restart at offset 0 after an index change. The offset may exceed 200."
                        },
                        "limit": {
                            "type": "integer",
                            "minimum": 0,
                            "maximum": code_kb_core::queries::MAX_RESULT_LIMIT,
                            "description": "Page size per folded section (default: 12 when paging is requested; maximum 200)."
                        }
                    }
                }),
            },
        ];
        for tool in tools.iter_mut() {
            let schema = &mut tool.input_schema;
            schema["properties"]["project_root"] =
                json!({ "type": "string", "description": PROJECT_ROOT_DESCRIPTION });
            match schema["required"].as_array_mut() {
                Some(required) => required.push(json!("project_root")),
                None => schema["required"] = json!(["project_root"]),
            }
        }
        tools
    }

    pub fn handle_call_tool(&mut self, name: &str, arguments: &Value) -> CallToolResult {
        let start = std::time::Instant::now();
        self.drop_finished_parked_prepares();
        let resolved = requires_project_root(name).then(|| self.resolve_project_root(arguments));
        let (res, telemetry_root) = match resolved {
            Some(Err(message)) => {
                tracing::warn!(tool = name, "MCP tool call refused: {message}");
                (CallToolResult::error(message), self.launch_root.clone())
            }
            resolved => {
                if let Some(Ok(workspace)) = resolved {
                    self.switch_root(workspace);
                }
                (
                    self.handle_call_tool_inner(name, arguments),
                    self.workspace.canonical_root.clone(),
                )
            }
        };
        let duration_ms = start.elapsed().as_millis() as u64;
        let answered_before_index_ready = self.reconcile.is_some();

        let (outcome, error_msg, bytes, est_tokens, est_tokens_saved, est_tokens_saved_known) =
            if res.is_error {
                let err_text = res
                    .content
                    .first()
                    .map(|c| c.text.as_str())
                    .unwrap_or("error");
                (
                    "error",
                    Some(err_text),
                    err_text.len(),
                    err_text.len() / 4,
                    0,
                    false,
                )
            } else {
                let bytes: usize = res.content.iter().map(|c| c.text.len()).sum();
                let est_tokens = bytes / 4;
                let outcome = if res.logical_result_count == Some(0) {
                    "empty"
                } else {
                    "ok"
                };
                let argument_file_size = match name {
                    _ if answered_before_index_ready => None,
                    "file_skeleton" | "get_symbol_body" | "get_symbol_context" => arguments
                        .get("file_path")
                        .or_else(|| arguments.get("file"))
                        .or_else(|| arguments.get("path"))
                        .and_then(|v| v.as_str())
                        .and_then(|p| {
                            self.workspace
                                .resolve_path(Path::new(p))
                                .ok()
                                .map(|(abs, _)| abs)
                                .or_else(|| {
                                    let p_buf = if p.starts_with("file://") {
                                        code_kb_core::parse_file_uri(p)
                                            .unwrap_or_else(|| PathBuf::from(p))
                                    } else {
                                        PathBuf::from(p)
                                    };
                                    if p_buf.is_absolute() {
                                        Some(p_buf)
                                    } else {
                                        Some(self.workspace.canonical_root.join(p_buf))
                                    }
                                })
                        })
                        .and_then(|abs| std::fs::metadata(&abs).ok())
                        .filter(|m| m.is_file())
                        .map(|m| m.len() as usize),
                    _ => None,
                };

                let baseline_bytes =
                    argument_file_size.or_else(|| Some(res.baseline_bytes).filter(|b| *b > 0));

                match baseline_bytes {
                    Some(baseline) => (
                        outcome,
                        None,
                        bytes,
                        est_tokens,
                        (baseline / 4).saturating_sub(est_tokens),
                        true,
                    ),
                    None => (outcome, None, bytes, est_tokens, 0, false),
                }
            };

        let invocation = code_kb_core::ToolInvocation {
            tool: name,
            duration_ms,
            outcome,
            error_message: error_msg,
            logical_result_count: res.logical_result_count,
            bytes_returned: bytes,
            est_tokens,
            est_tokens_saved,
            est_tokens_saved_known,
            reconcile_ms: res.reconcile_ms,
            query_ms: res.query_ms,
            client: self.client.as_deref(),
        };

        if let Some(ref conn) = self.telemetry_conn {
            record_tool_call_conn(conn, &telemetry_root, &invocation);
        } else {
            record_tool_call(&telemetry_root, &invocation);
        }

        res
    }

    pub(crate) fn sanitize_symbol_name(raw: &str) -> String {
        let mut s = raw.trim();
        // Do not strip `()` if the string is literally `"()"` or ends with `"operator()"` (e.g., C++ operator())
        if s.ends_with("()") && s != "()" && !s.ends_with("operator()") {
            s = s[..s.len() - 2].trim();
        }
        let prefixes = [
            "pub async fn ",
            "pub fn ",
            "async fn ",
            "fn ",
            "def ",
            "func ",
            "function ",
        ];
        for p in prefixes {
            if let Some(rest) = s.strip_prefix(p) {
                let trimmed = rest.trim();
                if !trimmed.is_empty() && !trimmed.contains(char::is_whitespace) {
                    s = trimmed;
                }
                break;
            }
        }
        s.to_string()
    }

    fn selector(arguments: &Value) -> Result<SymbolSelector, String> {
        let name = ["symbol_name", "symbol", "name", "target"]
            .into_iter()
            .find_map(|key| {
                arguments
                    .get(key)
                    .filter(|value| !value.is_null())
                    .map(|value| (key, value))
            });
        let id = arguments.get("symbol_id").filter(|value| !value.is_null());
        let (key, value) = match (name, id) {
            (Some(_), Some(_)) => {
                return Err("Specify exactly one of symbol_name or symbol_id".to_string());
            }
            (Some(name), None) => name,
            (None, Some(id)) => ("symbol_id", id),
            (None, None) => {
                return Err("Specify exactly one non-empty symbol_name or symbol_id".to_string());
            }
        };
        let value = value
            .as_str()
            .ok_or_else(|| format!("{key} must be a string"))?;
        if value.is_empty() {
            return Err(format!("{key} must not be empty"));
        }
        if key == "symbol_id" {
            Ok(SymbolSelector::Id(value.to_string()))
        } else {
            let name = Self::sanitize_symbol_name(value);
            if name.is_empty() {
                Err(format!("{key} must not be empty"))
            } else {
                Ok(SymbolSelector::Name(name))
            }
        }
    }

    fn handle_call_tool_inner(&mut self, name: &str, arguments: &Value) -> CallToolResult {
        tracing::info!(tool = name, args = %arguments, "MCP tool called");
        if !requires_project_root(name) {
            return CallToolResult::error(format!("Unknown tool: '{name}'"));
        }

        let rec_start = std::time::Instant::now();
        let (mut reconcile_ms, prepare_error) = match self.reconcile.take() {
            Some(prepare) => {
                let wait = Duration::from_millis(
                    std::env::var("CODE_KB_INDEX_WAIT_MS")
                        .ok()
                        .and_then(|ms| ms.parse().ok())
                        .unwrap_or(5000),
                );
                while !prepare.is_finished() && rec_start.elapsed() < wait {
                    std::thread::sleep(Duration::from_millis(20));
                }
                if !prepare.is_finished() {
                    self.reconcile = Some(prepare);
                    let mut started = CallToolResult::text(format!(
                        "Indexing {} started; call again in a few seconds.",
                        self.workspace.canonical_root.display()
                    ));
                    started.reconcile_ms = Some(rec_start.elapsed().as_millis() as u64);
                    started.query_ms = Some(0);
                    return started;
                }
                let prepared = join_index_prepare(prepare);
                let waited = Some(rec_start.elapsed().as_millis() as u64);
                match prepared {
                    Ok(watcher) => {
                        if watcher.is_some() {
                            self._watcher = watcher;
                        }
                        (waited, None)
                    }
                    Err(e) => (waited, Some(e)),
                }
            }
            None => (Some(0), None),
        };

        if !self.db_path.exists() {
            self._watcher = None;
            self.reconcile = spawn_index_prepare(&self.workspace, &self.db_path);
            let next = if self.reconcile.is_some() {
                "it will be retried on the next tool call."
            } else {
                "it will not be retried because the folder has no project marker and no index. Restore the --db file, or pass a project folder as project_root."
            };
            let msg = match prepare_error {
                Some(e) => format!(
                    "Initial scan of '{}' failed: {}; {next}",
                    self.workspace.canonical_root.display(),
                    e.trim_end()
                ),
                None => format!(
                    "No index exists for '{}'; {next}",
                    self.workspace.canonical_root.display()
                ),
            };
            tracing::error!("{}", msg);
            let mut err_res = CallToolResult::error(msg);
            err_res.reconcile_ms = reconcile_ms;
            err_res.query_ms = Some(0);
            return err_res;
        }

        if let Some(e) = prepare_error {
            self._watcher = None;
            let root = self.workspace.canonical_root.display();
            let msg = if is_unreadable_database(&self.db_path) {
                let active = self.workspace.canonical_root.clone();
                let msg = failed_rebuild_message(&active, &e);
                self.record_failed_rebuild(&active, msg.clone());
                msg
            } else {
                self.reconcile = spawn_index_prepare(&self.workspace, &self.db_path);
                format!(
                    "Index preparation of '{root}' failed: {e}; it will be retried on the next tool call"
                )
            };
            tracing::error!("{}", msg);
            let mut err_res = CallToolResult::error(msg);
            err_res.reconcile_ms = reconcile_ms;
            err_res.query_ms = Some(0);
            return err_res;
        }

        if let Some(msg) = self.failed_rebuild_error() {
            tracing::error!("{}", msg);
            let mut err_res = CallToolResult::error(msg);
            err_res.reconcile_ms = reconcile_ms;
            err_res.query_ms = Some(0);
            return err_res;
        }

        if self._watcher.is_none() {
            match start_watcher(self.workspace.clone(), self.db_path.clone()) {
                Ok(watcher) => self._watcher = Some(watcher),
                Err(e) => {
                    self.reconcile = spawn_index_prepare(&self.workspace, &self.db_path);
                    let root = self.workspace.canonical_root.display();
                    let msg = format!(
                        "Could not establish freshness for '{root}': failed to start the file watcher: {e}; it will be retried on the next tool call. {}",
                        scan_remedy(&self.workspace.canonical_root)
                    );
                    tracing::error!("{}", msg);
                    let mut err_res = CallToolResult::error(msg);
                    err_res.reconcile_ms = reconcile_ms;
                    err_res.query_ms = Some(0);
                    return err_res;
                }
            }
        }

        let conn = match open_read_only(&self.db_path) {
            Ok(c) => c,
            Err(e) => {
                let root = self.workspace.canonical_root.display();
                let msg = if is_unreadable_database(&self.db_path) {
                    format!(
                        "Failed to open the index for '{root}': {e}. {}",
                        scan_remedy(&self.workspace.canonical_root)
                    )
                } else {
                    format!(
                        "Failed to open the index for '{root}': {e}; it will be retried on the next tool call. {}",
                        scan_remedy(&self.workspace.canonical_root)
                    )
                };
                tracing::error!("{}", msg);
                let mut err_res = CallToolResult::error(msg);
                err_res.reconcile_ms = reconcile_ms;
                err_res.query_ms = Some(0);
                return err_res;
            }
        };

        if let Some(watcher) = self._watcher.as_ref()
            && let Some((generation, failure)) = watcher.pending_failure()
        {
            let root = self.workspace.canonical_root.display().to_string();
            let recovery_start = std::time::Instant::now();
            let recovery = reconcile_offline_edits(&self.workspace, &self.db_path, &conn);
            reconcile_ms =
                Some(reconcile_ms.unwrap_or(0) + recovery_start.elapsed().as_millis() as u64);
            if let Err(e) = recovery {
                let error = e.to_string();
                let remedy = scan_remedy(&self.workspace.canonical_root);
                let remedy = if error.contains(&remedy) {
                    String::new()
                } else {
                    format!(" {remedy}")
                };
                let msg = format!(
                    "Could not re-establish a fresh index for '{root}' after a watcher refresh failure ({failure}): {error}; it will be retried on the next tool call.{remedy}"
                );
                tracing::error!("{}", msg);
                let mut err_res = CallToolResult::error(msg);
                err_res.reconcile_ms = reconcile_ms;
                err_res.query_ms = Some(0);
                return err_res;
            }
            if !watcher.clear_failure_if_unchanged(generation) {
                let later_failure = watcher
                    .pending_failure()
                    .map(|(_, failure)| failure)
                    .unwrap_or(failure);
                let msg = format!(
                    "Could not establish a fresh index for '{root}': another watcher refresh failed during recovery ({later_failure}); it will be retried on the next tool call. {}",
                    scan_remedy(&self.workspace.canonical_root)
                );
                tracing::error!("{}", msg);
                let mut err_res = CallToolResult::error(msg);
                err_res.reconcile_ms = reconcile_ms;
                err_res.query_ms = Some(0);
                return err_res;
            }
        }

        let query_start = std::time::Instant::now();
        let mut result = match name {
            "file_skeleton" => {
                let file_path = match arguments
                    .get("file_path")
                    .or_else(|| arguments.get("file"))
                    .or_else(|| arguments.get("path"))
                    .and_then(|v| v.as_str())
                {
                    Some(p) => p,
                    None => return CallToolResult::error("Missing required parameter: file_path"),
                };

                let rendered_file = self
                    .workspace
                    .resolve_path(Path::new(file_path))
                    .ok()
                    .filter(|(abs, _)| abs.is_file())
                    .map(|(_, rel)| rel);

                match file_skeleton_op(&self.workspace, &self.db_path, &conn, file_path) {
                    Ok(skeleton) => {
                        let result = CallToolResult::text(skeleton);
                        match rendered_file.as_deref() {
                            Some(rel) => result.with_logical_result_count(
                                code_kb_core::count_file_symbols(&conn, rel),
                            ),
                            None => result,
                        }
                    }
                    Err(e) => CallToolResult::error(e.to_string()),
                }
            }
            "lookup_symbol" => {
                let raw_query = match arguments
                    .get("query")
                    .or_else(|| arguments.get("name"))
                    .or_else(|| arguments.get("q"))
                    .or_else(|| arguments.get("symbol_name"))
                    .or_else(|| arguments.get("symbol"))
                    .and_then(|v| v.as_str())
                {
                    Some(q) => q,
                    None => return CallToolResult::error("Missing required parameter: query"),
                };
                let sanitized_query = Self::sanitize_symbol_name(raw_query);
                let query = sanitized_query.as_str();
                let raw_path_filter = arguments
                    .get("path")
                    .or_else(|| arguments.get("file_path"))
                    .or_else(|| arguments.get("file"))
                    .and_then(|v| v.as_str());
                let rel_path = raw_path_filter.map(|p| self.workspace.relativize_filter(p));
                let path_filter = rel_path.as_deref();

                let kind = arguments.get("kind").and_then(|v| v.as_str());
                let include_tests = arguments
                    .get("is_test")
                    .or_else(|| arguments.get("include_tests"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let limit = match result_limit(arguments, 20) {
                    Ok(limit) => limit,
                    Err(error) => return error,
                };

                let lookup = match lookup_symbols_op(
                    &self.db_path,
                    &conn,
                    query,
                    kind,
                    path_filter,
                    include_tests,
                    limit,
                ) {
                    Ok(lookup) => lookup,
                    Err(e) => return CallToolResult::error(e.to_string()),
                };

                let baseline_paths = lookup
                    .names
                    .iter()
                    .map(|symbol| symbol.path.clone())
                    .chain(lookup.ranked.iter().map(|hit| hit.symbol.path.clone()))
                    .collect();

                let output = if limit == 0 {
                    ZERO_LIMIT_NOTICE.to_string()
                } else {
                    match render_symbol_lookup(&conn, query, &lookup, limit) {
                        Ok(text) => text,
                        Err(e) => return CallToolResult::error(e.to_string()),
                    }
                };
                CallToolResult::text(output)
                    .with_logical_result_count(lookup.names.len() + lookup.ranked.len())
                    .with_baseline_paths(baseline_paths)
            }
            "get_symbol_body" => {
                let selector = match Self::selector(arguments) {
                    Ok(selector) => selector,
                    Err(error) => return CallToolResult::error(error),
                };
                let file_path = arguments
                    .get("file_path")
                    .or_else(|| arguments.get("file"))
                    .or_else(|| arguments.get("path"))
                    .and_then(|v| v.as_str());

                match get_symbol_bodies_selected_op(
                    &self.workspace,
                    &self.db_path,
                    &conn,
                    &selector,
                    file_path,
                )
                .and_then(|bodies| fit_bodies_to_budget(&conn, bodies))
                {
                    Ok(bodies) => CallToolResult::text(format_symbol_bodies(&bodies))
                        .with_logical_result_count(bodies.len())
                        .with_baseline_paths(vec![bodies[0].0.path.clone()]),
                    Err(e) => CallToolResult::error(e.to_string()),
                }
            }
            "get_symbol_context" => {
                let selector = match Self::selector(arguments) {
                    Ok(selector) => selector,
                    Err(error) => return CallToolResult::error(error),
                };
                let file_path = arguments
                    .get("file_path")
                    .or_else(|| arguments.get("file"))
                    .or_else(|| arguments.get("path"))
                    .and_then(|v| v.as_str());
                let include_external = arguments
                    .get("include_external")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                match get_context_slices_selected_op(
                    &self.workspace,
                    &self.db_path,
                    &conn,
                    &selector,
                    file_path,
                    include_external,
                )
                .and_then(|slices| fit_context_slices_to_budget(&conn, slices))
                {
                    Ok(slices) => CallToolResult::text(format_context_slices(&slices))
                        .with_logical_result_count(slices.len())
                        .with_baseline_paths(vec![slices[0].target_symbol.path.clone()]),
                    Err(e) => CallToolResult::error(e.to_string()),
                }
            }
            "find_references" => {
                let selector = match Self::selector(arguments) {
                    Ok(selector) => selector,
                    Err(error) => return CallToolResult::error(error),
                };
                let raw_file_path = arguments
                    .get("file_path")
                    .or_else(|| arguments.get("file"))
                    .or_else(|| arguments.get("path"))
                    .and_then(|v| v.as_str());
                let direction = arguments
                    .get("direction")
                    .and_then(|v| v.as_str())
                    .unwrap_or("callers");
                let (limit, offset, page_end) = match result_page(arguments, 20) {
                    Ok(page) => page,
                    Err(error) => return error,
                };
                let include_external = arguments
                    .get("include_external")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                if direction == "callers" {
                    let selected = match selector {
                        SymbolSelector::Name(name) => {
                            let path =
                                raw_file_path.map(|path| self.workspace.relativize_filter(path));
                            select_reference_target(&conn, &name, path.as_deref())
                                .map_err(|error| error.to_string())
                        }
                        SymbolSelector::Id(id) => resolve_symbol_op(
                            &self.workspace,
                            &self.db_path,
                            &conn,
                            &SymbolSelector::Id(id),
                            raw_file_path,
                        )
                        .map_err(|error| error.to_string()),
                    };
                    let selected = match selected {
                        Ok(symbol) => symbol,
                        Err(error) => return CallToolResult::error(error),
                    };
                    if limit == 0 {
                        CallToolResult::text(ZERO_LIMIT_NOTICE.to_string())
                            .with_logical_result_count(0)
                    } else {
                        let report = match reference_report(
                            &conn,
                            &self.workspace.root,
                            &selected,
                            include_external,
                        ) {
                            Ok(report) => report,
                            Err(error) => return CallToolResult::error(error.to_string()),
                        };
                        let mut baseline_paths: Vec<String> = report
                            .sites
                            .iter()
                            .map(|labeled| labeled.site.path.clone())
                            .collect();
                        baseline_paths.sort();
                        baseline_paths.dedup();
                        CallToolResult::text(
                            code_kb_core::formatters::format_reference_report_page(
                                &report, limit, offset,
                            ),
                        )
                        .with_logical_result_count(report.counts.sites)
                        .with_baseline_paths(baseline_paths)
                    }
                } else {
                    let (target, refs, page_total) = match selector {
                        SymbolSelector::Name(name) => {
                            let path =
                                raw_file_path.map(|path| self.workspace.relativize_filter(path));
                            if direction == "callees" && limit > 0 {
                                match code_kb_core::queries::find_callees_page_scoped(
                                    &conn,
                                    &name,
                                    limit,
                                    offset,
                                    include_external,
                                    path.as_deref(),
                                ) {
                                    Ok((refs, total)) => (name, refs, Some(total)),
                                    Err(error) => {
                                        return CallToolResult::error(error.to_string());
                                    }
                                }
                            } else {
                                match code_kb_core::find_references_scoped(
                                    &conn,
                                    &name,
                                    direction,
                                    page_end.min(code_kb_core::queries::MAX_RESULT_LIMIT),
                                    include_external,
                                    path.as_deref(),
                                ) {
                                    Ok(refs) => (name, refs, None),
                                    Err(error) => return CallToolResult::error(error.to_string()),
                                }
                            }
                        }
                        SymbolSelector::Id(id) => {
                            let selected = match resolve_symbol_op(
                                &self.workspace,
                                &self.db_path,
                                &conn,
                                &SymbolSelector::Id(id),
                                raw_file_path,
                            ) {
                                Ok(symbol) => symbol,
                                Err(error) => return CallToolResult::error(error.to_string()),
                            };
                            if direction == "callees" && limit > 0 {
                                match code_kb_core::queries::find_callees_page_for_symbol(
                                    &conn,
                                    &selected.name,
                                    &selected.symbol_id,
                                    limit,
                                    offset,
                                    include_external,
                                ) {
                                    Ok((refs, total)) => (selected.name, refs, Some(total)),
                                    Err(error) => {
                                        return CallToolResult::error(error.to_string());
                                    }
                                }
                            } else {
                                match find_references_for_symbol_ext(
                                    &conn,
                                    &selected.name,
                                    direction,
                                    page_end.min(code_kb_core::queries::MAX_RESULT_LIMIT),
                                    &selected.symbol_id,
                                    include_external,
                                ) {
                                    Ok(refs) => (selected.name, refs, None),
                                    Err(error) => return CallToolResult::error(error.to_string()),
                                }
                            }
                        }
                    };

                    let baseline_paths = refs.iter().map(|site| site.path.clone()).collect();

                    let output = if limit == 0 {
                        ZERO_LIMIT_NOTICE.to_string()
                    } else if let Some(total) = page_total {
                        code_kb_core::formatters::format_references_window(
                            &target, &refs, direction, limit, offset, total,
                        )
                    } else {
                        code_kb_core::formatters::format_references_page(
                            &target, &refs, direction, limit, offset,
                        )
                    };
                    CallToolResult::text(output)
                        .with_logical_result_count(if limit == 0 { 0 } else { refs.len() })
                        .with_baseline_paths(baseline_paths)
                }
            }
            "blast_radius" | "impact" => {
                let has_selector = ["symbol_name", "symbol", "name", "target", "symbol_id"]
                    .iter()
                    .any(|key| arguments.get(*key).is_some_and(|value| !value.is_null()));
                let selector = if has_selector {
                    match Self::selector(arguments) {
                        Ok(selector) => Some(selector),
                        Err(error) => return CallToolResult::error(error),
                    }
                } else {
                    None
                };
                let file = arguments
                    .get("file")
                    .or_else(|| arguments.get("file_path"))
                    .or_else(|| arguments.get("path"))
                    .and_then(|v| v.as_str());
                let depth = arguments
                    .get("depth")
                    .or_else(|| arguments.get("max_depth"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(2) as usize;
                let (limit, offset, page_end) = match result_page(arguments, 20) {
                    Ok(page) => page,
                    Err(error) => return error,
                };
                let include_low_signal = arguments
                    .get("include_low_signal")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);

                if limit == 0 {
                    CallToolResult::text(ZERO_LIMIT_NOTICE.to_string()).with_logical_result_count(0)
                } else {
                    match code_kb_core::ops::blast_radius_selected_op_with_options(
                        &self.workspace,
                        Some(&self.db_path),
                        &conn,
                        selector,
                        file,
                        depth,
                        page_end.min(code_kb_core::queries::MAX_RESULT_LIMIT),
                        include_low_signal,
                    ) {
                        Ok(res) => {
                            let baseline_paths = res
                                .likely_tests
                                .iter()
                                .map(|test| test.path.clone())
                                .chain(res.impacted_symbols.iter().map(|sym| sym.path.clone()))
                                .collect();

                            let logical_count =
                                res.likely_tests.len().saturating_sub(offset).min(limit)
                                    + res.impacted_symbols.len().saturating_sub(offset).min(limit);
                            CallToolResult::text(
                                code_kb_core::formatters::format_blast_radius_report(
                                    &self.workspace.root,
                                    &res,
                                    offset,
                                    limit,
                                    include_low_signal,
                                ),
                            )
                            .with_logical_result_count(logical_count)
                            .with_baseline_paths(baseline_paths)
                        }
                        Err(e) => CallToolResult::error(e.to_string()),
                    }
                }
            }
            "check_edit" => {
                let base = match arguments.get("base") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(rev)) => Some(rev.as_str()),
                    Some(_) => return CallToolResult::error("base must be a string"),
                };
                let (limit, offset, _) = match result_page(
                    arguments,
                    code_kb_core::formatters::DEFAULT_EDIT_PAGE_SIZE,
                ) {
                    Ok(page) => page,
                    Err(error) => return error,
                };
                if limit == 0 {
                    return CallToolResult::text(ZERO_LIMIT_NOTICE.to_string())
                        .with_logical_result_count(0);
                }
                match edit_check_op(&self.workspace, &self.db_path, &conn, base) {
                    Ok(check) => CallToolResult::text(
                        code_kb_core::formatters::format_edit_check_page(&check, offset, limit),
                    )
                    .with_logical_result_count(check.changes.len()),
                    Err(e) => CallToolResult::error(e.to_string()),
                }
            }
            _ => CallToolResult::error(format!("Unknown tool: '{name}'")),
        };

        if result.is_error {
            tracing::warn!(tool = name, "MCP tool returned error");
        } else {
            tracing::info!(tool = name, "MCP tool executed successfully");
        }

        let content = &result.content;
        result.baseline_paths.retain(|path| {
            content
                .iter()
                .any(|block| block.text.contains(path.as_str()))
        });
        if !result.baseline_paths.is_empty() {
            result.baseline_bytes = file_sizes_for_paths(&conn, &result.baseline_paths);
        }
        result.reconcile_ms = reconcile_ms;
        result.query_ms = Some(query_start.elapsed().as_millis() as u64);

        result
    }

    pub fn run_stdio(&mut self) -> anyhow::Result<()> {
        tracing::info!(
            workspace = %self.workspace.canonical_root.display(),
            db = %self.db_path.display(),
            "code-kb MCP server listening on stdio"
        );
        let stdin = std::io::stdin();
        let mut reader = BufReader::new(stdin.lock());
        let mut stdout = std::io::stdout();

        let mut line = String::new();

        while reader.read_line(&mut line)? > 0 {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                line.clear();
                continue;
            }

            let request: JsonRpcRequest = match serde_json::from_str(trimmed) {
                Ok(r) => r,
                Err(e) => {
                    let err_resp =
                        JsonRpcResponse::error(None, -32700, format!("Parse error: {e}"));
                    let mut serialized = serde_json::to_string(&err_resp)?;
                    serialized.push('\n');
                    stdout.write_all(serialized.as_bytes())?;
                    stdout.flush()?;
                    line.clear();
                    continue;
                }
            };

            let response = self.handle_request(request);
            if let Some(resp) = response {
                let mut serialized = serde_json::to_string(&resp)?;
                serialized.push('\n');
                stdout.write_all(serialized.as_bytes())?;
                stdout.flush()?;
            }

            line.clear();
        }

        Ok(())
    }

    fn handle_request(&mut self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let id = request.id;
        match request.method.as_str() {
            "initialize" => {
                tracing::info!(params = ?request.params, "MCP initialize received");
                self.client = request
                    .params
                    .as_ref()
                    .and_then(|params| params.pointer("/clientInfo/name"))
                    .and_then(Value::as_str)
                    .map(|name| name.chars().take(100).collect());
                let init_result = json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {
                            "listChanged": false
                        }
                    },
                    "serverInfo": {
                        "name": "code-kb",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "instructions": "Pass project_root, the absolute path of the project or git worktree you work in, on every call. Call file_skeleton before you read a source file, then get_symbol_body or get_symbol_context for the symbols you need. Use lookup_symbol for a name or a concept and find_references instead of grep on a name, and blast_radius to pick the tests to run. After you edit, call check_edit before you say the change is done."
                });
                Some(JsonRpcResponse::success(id, init_result))
            }
            "notifications/initialized" => None,
            "ping" => Some(JsonRpcResponse::success(id, json!({}))),
            "tools/list" => {
                let mut tools = json!(Self::tool_definitions());
                // Claude Code defers MCP tools behind ToolSearch, and its agents almost never
                // search for them, so every code tool asks to be loaded at session start.
                for tool in tools.as_array_mut().into_iter().flatten() {
                    tool["_meta"] = json!({ "anthropic/alwaysLoad": true });
                }
                Some(JsonRpcResponse::success(id, json!({ "tools": tools })))
            }
            "tools/call" => {
                let params = request.params.unwrap_or(Value::Null);
                let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let arguments = params.get("arguments").unwrap_or(&Value::Null);

                let result = self.handle_call_tool(tool_name, arguments);
                release_freed_memory();
                Some(JsonRpcResponse::success(
                    id,
                    serde_json::to_value(result).unwrap_or(Value::Null),
                ))
            }
            _ if id.is_none() => None,
            _ => Some(JsonRpcResponse::error(
                id,
                -32601,
                format!("Method not found: {}", request.method),
            )),
        }
    }
}

#[cfg(test)]
mod prepare_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn a_panicked_prepare_returns_an_error_and_retries() {
        let temp_dir = code_kb_core::safe_tempdir();
        let root = temp_dir.path().to_path_buf();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn recovered_symbol() {}\n").unwrap();
        let workspace = Workspace::new(root.clone());
        let db_path = root.join(".code-kb/artifact.db");
        code_kb_core::scan_workspace(&workspace, &db_path, true).unwrap();
        let prepare = std::thread::spawn(|| -> Result<Option<WatcherHandle>, String> {
            panic!("controlled preparation panic")
        });
        let mut server = McpServer {
            workspace,
            db_path,
            explicit_db: None,
            _watcher: None,
            telemetry_conn: None,
            reconcile: Some(prepare),
            launch_root: root.clone(),
            parked: Vec::new(),
            prepared: true,
            client: None,
            failed_rebuilds: Vec::new(),
        };
        let arguments = json!({
            "project_root": root,
            "query": "recovered_symbol"
        });

        let failed = server.handle_call_tool("lookup_symbol", &arguments);
        assert!(failed.is_error, "{}", failed.content[0].text);
        assert!(
            failed.content[0]
                .text
                .contains("controlled preparation panic"),
            "{}",
            failed.content[0].text
        );
        assert!(failed.content[0].text.contains("retried"));

        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let recovered = server.handle_call_tool("lookup_symbol", &arguments);
            let text = &recovered.content[0].text;
            if !recovered.is_error && !text.starts_with("Indexing ") {
                assert!(text.contains("function `recovered_symbol` ["), "{text}");
                break;
            }
            assert!(Instant::now() < deadline, "{text}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// glibc keeps the pages it frees after a large report, so the process stays at a high
/// resident size until this call returns them to the OS.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[allow(unsafe_code)]
fn release_freed_memory() {
    unsafe extern "C" {
        fn malloc_trim(pad: usize) -> std::ffi::c_int;
    }
    // SAFETY: malloc_trim takes no pointers and only returns free heap pages to the OS.
    unsafe {
        malloc_trim(0);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn release_freed_memory() {}

#[cfg(test)]
mod tests {
    #[test]
    fn selector_requires_one_nonempty_opaque_value() {
        assert!(
            matches!(McpServer::selector(&json!({"symbol_id": "id'quoted"})), Ok(SymbolSelector::Id(id)) if id == "id'quoted")
        );
        assert!(
            matches!(McpServer::selector(&json!({"symbol_name": "run", "symbol_id": null})), Ok(SymbolSelector::Name(name)) if name == "run")
        );
        assert!(
            matches!(McpServer::selector(&json!({"symbol_name": "run", "symbol": "run"})), Ok(SymbolSelector::Name(name)) if name == "run")
        );
        assert!(McpServer::selector(&json!({"symbol_id": ""})).is_err());
        assert!(McpServer::selector(&json!({"symbol_name": "run", "symbol_id": "id"})).is_err());
    }

    #[test]
    fn selector_schemas_use_compatible_object_shapes() {
        let tools = McpServer::tool_definitions();
        for tool in &tools {
            for combinator in ["oneOf", "anyOf", "allOf"] {
                assert!(
                    tool.input_schema.get(combinator).is_none(),
                    "{} has a top-level {combinator}",
                    tool.name
                );
            }
        }
        for name in ["get_symbol_body", "get_symbol_context", "find_references"] {
            let schema = &tools
                .iter()
                .find(|tool| tool.name == name)
                .unwrap()
                .input_schema;
            for selector in ["symbol_name", "symbol_id"] {
                let property = &schema["properties"][selector];
                assert_eq!(property["type"], "string", "{name}.{selector}");
                assert!(property.get("minLength").is_none(), "{name}.{selector}");
                assert!(
                    property["description"]
                        .as_str()
                        .unwrap()
                        .contains("exactly one"),
                    "{name}.{selector}"
                );
            }
        }
    }

    use super::*;

    #[test]
    fn telemetry_records_logical_lookup_counts_without_inventing_savings() {
        let temp = code_kb_core::safe_tempdir();
        let root = temp.path();
        let db_dir = root.join(".code-kb");
        std::fs::create_dir_all(&db_dir).unwrap();
        let db_path = db_dir.join("artifact.db");
        let conn = code_kb_core::open_read_write(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE files (
                file_id TEXT PRIMARY KEY, path TEXT, language TEXT, content_hash TEXT,
                content_bytes INTEGER, line_count INTEGER, indexed_at TEXT
            );
            CREATE TABLE symbols (
                symbol_id TEXT PRIMARY KEY, file_id TEXT, path TEXT, language TEXT, name TEXT, kind TEXT,
                signature TEXT, doc_comment TEXT, visibility TEXT, parent_symbol_id TEXT,
                start_line INTEGER, start_column INTEGER, end_line INTEGER, end_column INTEGER,
                start_byte INTEGER, end_byte INTEGER, body_start_line INTEGER,
                body_start_column INTEGER, body_end_line INTEGER, body_end_column INTEGER,
                body_start_byte INTEGER, body_end_byte INTEGER, body_hash TEXT,
                semantic_group TEXT, is_test INTEGER, test_container INTEGER
            );
            CREATE TABLE relationships (
                relationship_id TEXT PRIMARY KEY, from_symbol_id TEXT, to_symbol_id TEXT,
                kind TEXT, path TEXT, start_line INTEGER, start_column INTEGER
            );
            CREATE TABLE pending_relationships (
                from_symbol_id TEXT, target_terminal_name TEXT, kind TEXT,
                path TEXT, start_line INTEGER, start_column INTEGER
            );",
        )
        .unwrap();
        code_kb_core::db::ensure_fts_index(&conn).unwrap();
        drop(conn);

        let workspace = Workspace::new(root.to_path_buf());
        let watcher = start_watcher(workspace.clone(), db_path.clone()).unwrap();
        let conn = code_kb_core::open_read_write(&db_path).unwrap();
        conn.execute_batch(
            "INSERT INTO files (file_id, path, language, content_hash, content_bytes, line_count, indexed_at, last_revision_id, status)
            VALUES ('f', 'src/lib.rs', 'rust', 'hash', 0, 1, '2026-01-01', 1, 'indexed');
            INSERT INTO symbols (
                symbol_id, file_id, path, language, name, kind, signature, doc_comment,
                visibility, parent_symbol_id, start_line, start_column, end_line, end_column,
                start_byte, end_byte, body_start_line, body_start_column, body_end_line,
                body_end_column, body_start_byte, body_end_byte, body_hash, semantic_group,
                is_test, test_container
            ) VALUES
                ('one', 'f', 'src/lib.rs', 'rust', 'needle', 'function', 'fn needle()', NULL, 'pub', NULL, 1, 0, 1, 1, 0, 1, 1, 0, 1, 1, 0, 1, 'hash', NULL, 0, 0),
                ('two', 'f', 'src/lib.rs', 'rust', 'needle', 'function', 'fn needle_alt()', NULL, 'pub', NULL, 2, 0, 2, 1, 2, 3, 2, 0, 2, 1, 2, 3, 'hash', NULL, 0, 0);",
        )
        .unwrap();
        drop(conn);

        let mut server = McpServer {
            launch_root: workspace.canonical_root.clone(),
            workspace,
            db_path,
            explicit_db: None,
            _watcher: Some(watcher),
            telemetry_conn: Some(code_kb_core::telemetry::open_telemetry_db_at(root).unwrap()),
            reconcile: None,
            parked: Vec::new(),
            prepared: true,
            client: None,
            failed_rebuilds: Vec::new(),
        };
        assert!(
            !server
                .handle_call_tool(
                    "lookup_symbol",
                    &json!({"query": "needle", "project_root": root})
                )
                .is_error
        );
        assert!(
            !server
                .handle_call_tool(
                    "lookup_symbol",
                    &json!({"query": "absent", "project_root": root})
                )
                .is_error
        );

        let rows = server
            .telemetry_conn
            .as_ref()
            .unwrap()
            .prepare(
                "SELECT outcome, result_count, result_count_known, est_tokens_saved
                 FROM tool_telemetry ORDER BY rowid",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![("ok".to_string(), 2, 1, 0), ("empty".to_string(), 0, 1, 0)]
        );
    }

    #[test]
    fn result_limit_rejects_schema_bypassing_values() {
        assert_eq!(result_limit(&json!({}), 20).unwrap(), 20);
        assert_eq!(result_limit(&json!({"limit": 0}), 20).unwrap(), 0);
        assert!(result_limit(&json!({"limit": u64::MAX}), 20).is_err());
        assert!(result_limit(&json!({"limit": -1}), 20).is_err());
        assert!(result_limit(&json!({"limit": 1.5}), 20).is_err());
    }

    #[test]
    fn result_offset_accepts_the_integer_forms_that_limit_accepts() {
        assert_eq!(result_offset(&json!({})).unwrap(), 0);
        assert_eq!(result_offset(&json!({"offset": 50})).unwrap(), 50);
        assert_eq!(result_offset(&json!({"offset": "50"})).unwrap(), 50);
        assert!(result_offset(&json!({"offset": "-1"})).is_err());
        assert!(result_offset(&json!({"offset": "five"})).is_err());
        assert!(result_offset(&json!({"offset": -1})).is_err());
        assert!(result_offset(&json!({"offset": 1.5})).is_err());
    }

    #[test]
    fn test_sanitize_symbol_name() {
        // Strips trailing parens
        assert_eq!(McpServer::sanitize_symbol_name("foo()"), "foo");
        assert_eq!(
            McpServer::sanitize_symbol_name("Type::method()"),
            "Type::method"
        );
        assert_eq!(McpServer::sanitize_symbol_name("  bar()  "), "bar");

        // Strips common function declaration prefixes
        assert_eq!(McpServer::sanitize_symbol_name("fn foo"), "foo");
        assert_eq!(
            McpServer::sanitize_symbol_name("pub fn calculate"),
            "calculate"
        );
        assert_eq!(
            McpServer::sanitize_symbol_name("pub async fn fetch_data"),
            "fetch_data"
        );
        assert_eq!(McpServer::sanitize_symbol_name("async fn run"), "run");
        assert_eq!(McpServer::sanitize_symbol_name("def process"), "process");
        assert_eq!(McpServer::sanitize_symbol_name("func compute"), "compute");
        assert_eq!(
            McpServer::sanitize_symbol_name("function handleRequest"),
            "handleRequest"
        );

        // Strips both prefix and trailing parens
        assert_eq!(
            McpServer::sanitize_symbol_name("pub async fn execute()"),
            "execute"
        );
        assert_eq!(McpServer::sanitize_symbol_name("def my_func()"), "my_func");

        // Preserves operator() and variations
        assert_eq!(McpServer::sanitize_symbol_name("operator()"), "operator()");
        assert_eq!(
            McpServer::sanitize_symbol_name("Class::operator()"),
            "Class::operator()"
        );
        assert_eq!(
            McpServer::sanitize_symbol_name("  operator()  "),
            "operator()"
        );

        // Preserves standalone () and empty inputs
        assert_eq!(McpServer::sanitize_symbol_name("()"), "()");
        assert_eq!(McpServer::sanitize_symbol_name(""), "");
        assert_eq!(McpServer::sanitize_symbol_name("   "), "");

        // Preserves standalone prefixes when nothing follows
        assert_eq!(McpServer::sanitize_symbol_name("fn"), "fn");
        assert_eq!(McpServer::sanitize_symbol_name("fn "), "fn");

        // Preserves symbols with no prefixes or parens
        assert_eq!(McpServer::sanitize_symbol_name("Workspace"), "Workspace");
        assert_eq!(
            McpServer::sanitize_symbol_name("crate::module::symbol"),
            "crate::module::symbol"
        );
        assert_eq!(
            McpServer::sanitize_symbol_name("function that retries"),
            "function that retries"
        );
        assert_eq!(
            McpServer::sanitize_symbol_name("def load config files"),
            "def load config files"
        );
    }
}
