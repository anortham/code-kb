use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use code_kb_core::workspace::{NO_PROJECT_MARKER_REASON, paths_equal};
use code_kb_core::{
    Connection, SymbolSelector, TelemetryFilter, TimeWindow, WatcherHandle, Workspace,
    WorkspaceError, blast_radius_selected_op, codebase_outline_op, create_index,
    ensure_fts_index_path, ensure_index_matches_extractor, file_sizes_for_paths, file_skeleton_op,
    find_references_for_symbol_ext, format_blast_radius, format_context_slice,
    format_fact_categories, format_find_symbol_results, format_references, format_search_results,
    format_structural_facts, format_symbol_body, format_telemetry_summary,
    fts_search_symbols_scoped, get_context_slice_selected_op, get_symbol_body_selected_op,
    get_telemetry_summary, installed_extractor_version, is_project_root,
    list_structural_fact_categories_scoped, open_global_telemetry_db, open_read_only,
    reconcile_offline_edits, record_tool_call, record_tool_call_conn, resolve_symbol_op,
    search_symbols_scoped, start_watcher,
};

use super::protocol::{CallToolResult, JsonRpcRequest, JsonRpcResponse, Tool};

const ZERO_LIMIT_NOTICE: &str = "Result limit is 0; increase it to check for matches.";
const PROJECT_ROOT_DESCRIPTION: &str = "Absolute path of the project or git worktree you are working in. Send the same value on every call. Change it when you move to a worktree or another project.";

/// Counts the tree lines a `codebase_outline` answer renders, ignoring the root header
/// and the bracketed notices that follow the tree.
fn rendered_outline_entries(outline: &str) -> usize {
    outline
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty() && !line.starts_with('['))
        .count()
}

type IndexPrepare = std::thread::JoinHandle<Result<Option<WatcherHandle>, String>>;

pub struct McpServer {
    pub workspace: Workspace,
    pub db_path: PathBuf,
    pub explicit_db: Option<PathBuf>,
    pub _watcher: Option<WatcherHandle>,
    pub telemetry_conn: Option<Connection>,
    /// Index preparation of the active root: a full scan when the index is missing,
    /// otherwise a reconciliation against files that changed while no watcher ran. A tool
    /// call waits a bounded time for it so it never answers from a missing or stale index.
    reconcile: Option<IndexPrepare>,
    /// The canonical root the server started with. `--db` pins only this root, and a
    /// refused call records it in telemetry.
    launch_root: PathBuf,
    /// Prepares still running on roots the server left. A scan writes into `artifact.db`
    /// in place, so a root that becomes active again reuses its running prepare.
    parked: Vec<(PathBuf, IndexPrepare)>,
    /// False while the active root has had no prepare, which happens when the startup
    /// pre-warm is refused.
    prepared: bool,
}

/// Resolves `input` like `Workspace::from_project_root`, except that a launch root with
/// no project marker is accepted when `--db` names an existing file: that file is its index.
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

/// Prepares the index on a background thread. With `with_watcher`, the thread also starts
/// the watcher once the index exists, before the offline reconcile, and returns it, so an
/// edit made after the prepare reaches the index before any call joins the thread.
fn spawn_index_prepare(
    workspace: &Workspace,
    db_path: &Path,
    with_watcher: bool,
) -> Option<IndexPrepare> {
    if !db_path.exists() && !is_project_root(&workspace.canonical_root) {
        return None;
    }
    let ws = workspace.clone();
    let db = db_path.to_path_buf();
    Some(std::thread::spawn(move || {
        if let Err(e) = ensure_index_matches_extractor(&ws, &db, &installed_extractor_version()) {
            tracing::warn!("Index version check failed: {e}");
        }
        if !db.exists() {
            tracing::info!(ws = %ws.canonical_root.display(), "Database not found; running automatic initial scan");
            create_index(&ws, &db).map_err(|e| e.to_string())?;
        }
        ensure_fts_index_path(&db).map_err(|e| e.to_string())?;
        let watcher = with_watcher
            .then(|| start_watcher(ws.clone(), db.clone()).ok())
            .flatten();
        if let Ok(conn) = open_read_only(&db) {
            let _ = reconcile_offline_edits(&ws, &db, &conn);
        }
        Ok(watcher)
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

impl McpServer {
    pub fn new(workspace: Workspace, explicit_db: Option<&Path>) -> anyhow::Result<Self> {
        let db_path = workspace.locate_db(explicit_db).unwrap_or_else(|_| {
            workspace
                .canonical_root
                .join(".code-kb")
                .join("artifact.db")
        });

        let (reconcile, watcher, prepared) = match resolve_root(
            &workspace.canonical_root.to_string_lossy(),
            &workspace.canonical_root,
            explicit_db,
        ) {
            Ok(_) => {
                if let Err(e) = ensure_index_matches_extractor(
                    &workspace,
                    &db_path,
                    &installed_extractor_version(),
                ) {
                    tracing::warn!("Index version check failed: {e}");
                }
                let watcher = if db_path.exists() {
                    start_watcher(workspace.clone(), db_path.clone()).ok()
                } else {
                    None
                };
                let reconcile = spawn_index_prepare(&workspace, &db_path, watcher.is_none());
                (reconcile, watcher, true)
            }
            Err(e) => {
                tracing::info!(
                    root = %workspace.canonical_root.display(),
                    "Startup index skipped: {e}"
                );
                (None, None, false)
            }
        };

        let telemetry_conn = open_global_telemetry_db().ok();

        Ok(Self {
            launch_root: workspace.canonical_root.clone(),
            workspace,
            db_path,
            explicit_db: explicit_db.map(|p| p.to_path_buf()),
            _watcher: watcher,
            telemetry_conn,
            reconcile,
            parked: Vec::new(),
            prepared,
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
        // reconcile on each switch, and a parked prepare that finishes while its root is
        // inactive holds that root's watcher until it is pruned or dropped. Keep a small
        // per-root cache only if telemetry shows agents alternate often.
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
        self.parked.retain(|(_, prepare)| !prepare.is_finished());
        if let Some(prepare) = self.reconcile.take()
            && !prepare.is_finished()
        {
            self.parked
                .push((self.workspace.canonical_root.clone(), prepare));
        }
        let parked = self
            .parked
            .iter()
            .position(|(root, _)| paths_equal(root, &workspace.canonical_root))
            .map(|index| self.parked.swap_remove(index).1);
        self.reconcile = parked.or_else(|| spawn_index_prepare(&workspace, &db_path, true));
        self.prepared = true;
        self.workspace = workspace;
        self.db_path = db_path;
    }

    /// Resolves a call's `project_root` (or its silent aliases `workspace` and `root`) and
    /// checks that every absolute path argument lies inside it and outside any nested git
    /// worktree, submodule, or indexed project.
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
        for key in ["path", "file_path", "file", "subpath", "dir"] {
            let Some(path) = arguments.get(key).and_then(Value::as_str) else {
                continue;
            };
            if !path.starts_with("file://") && !Path::new(path).is_absolute() {
                continue;
            }
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
                name: "codebase_outline".to_string(),
                description: "Provides a top-level architectural orientation of the repository or sub-package in ~200 tokens. Start here when exploring unfamiliar code instead of running directory listings or reading files.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Subdirectory to scope the outline to, relative to project_root. Defaults to project_root."
                        },
                        "depth": {
                            "type": "integer",
                            "description": "Directory recursion depth (default: 2)."
                        }
                    }
                }),
            },
            Tool {
                name: "file_skeleton".to_string(),
                description: "Returns all types, traits, functions, signatures, docstrings, and visibility for a file with implementation bodies stripped. Use this instead of reading the entire file when inspecting interfaces and types. A directory path returns its outline.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "file_path": {
                            "type": "string",
                            "description": "File path relative to project_root, or an absolute path inside it."
                        }
                    },
                    "required": ["file_path"]
                }),
            },
            Tool {
                name: "lookup_symbol".to_string(),
                description: "Look up symbols by identifier. Use for exact names, qualified paths ('Type::method'), or identifier prefixes. Returns kind, path, and signature. Do NOT use for natural-language concepts or keywords; use search_symbols instead.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Exact identifier name, qualified path ('Type::method'), or prefix. Not a sentence or concept."
                        },
                        "path": {
                            "type": "string",
                            "description": "Optional file path or directory prefix to scope search (e.g. 'crates/code-kb-core')."
                        },
                        "kind": {
                            "type": "string",
                            "description": "Optional filter by kind (e.g. function, struct, trait, class, interface, enum)."
                        },
                        "is_test": {
                            "type": "boolean",
                            "description": "Include test functions, test containers, and rows from test files (default: false). A row whose name equals the query is returned either way."
                        },
                        "limit": {
                            "type": "integer",
                            "minimum": 0,
                            "maximum": code_kb_core::queries::MAX_RESULT_LIMIT,
                            "description": "Maximum number of symbols to return, 0-200 (default: 20)."
                        }
                    },
                    "required": ["query"]
                }),
            },
            Tool {
                name: "search_symbols".to_string(),
                description: "Natural-language and keyword search over symbol names, signatures, and docstrings; substrings inside identifiers are found ('sha256' finds 'parseSha256Sidecar'). Use when the exact identifier is unknown or searching for concepts (e.g. 'auth middleware', 'retry loop'). Do NOT use if you already know the exact symbol name; use lookup_symbol instead.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Natural language query or keywords (e.g. 'parse tokens', 'authentication middleware', 'retry backoff')."
                        },
                        "path": {
                            "type": "string",
                            "description": "Optional file path or directory prefix to scope search (e.g. 'crates/code-kb-core')."
                        },
                        "kind": {
                            "type": "string",
                            "description": "Optional filter by kind (e.g. function, struct, trait, class, interface, enum)."
                        },
                        "is_test": {
                            "type": "boolean",
                            "description": "Include test functions, test containers, and rows from test files (default: false)."
                        },
                        "limit": {
                            "type": "integer",
                            "minimum": 0,
                            "maximum": code_kb_core::queries::MAX_RESULT_LIMIT,
                            "description": "Maximum number of symbols to return, 0-200 (default: 20)."
                        }
                    },
                    "required": ["query"]
                }),
            },
            Tool {
                name: "get_symbol_body".to_string(),
                description: "Retrieves only the raw implementation body of a specific symbol. Use when you only need the implementation without dependency context. If preparing to edit a function, use get_symbol_context instead.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "symbol_name": {
                            "type": "string",
                            "description": "Full or qualified symbol name. Provide exactly one non-empty symbol_name or symbol_id."
                        },
                        "symbol_id": { "type": "string", "description": "Exact current-index symbol identifier. Provide exactly one non-empty symbol_name or symbol_id." },
                        "file_path": {
                            "type": "string",
                            "description": "Optional file path to disambiguate identical symbol names."
                        }
                    }
                }),
            },
            Tool {
                name: "get_symbol_context".to_string(),
                description: "Surgical context bundle combining target body, callee signatures, parameter types, and related tests in one turn. Use this before modifying a function to understand its immediate dependencies.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "symbol_name": {
                            "type": "string",
                            "description": "Target symbol name. Provide exactly one non-empty symbol_name or symbol_id."
                        },
                        "symbol_id": { "type": "string", "description": "Exact current-index symbol identifier. Provide exactly one non-empty symbol_name or symbol_id." },
                        "file_path": {
                            "type": "string",
                            "description": "Optional file path to disambiguate identical symbol names."
                        },
                        "include_external": {
                            "type": "boolean",
                            "description": "Include external stdlib/runtime calls in callee signatures (default: false)."
                        }
                    }
                }),
            },
            Tool {
                name: "find_references".to_string(),
                description: "Discovers callers or callees of a symbol from AST call sites. Callers also include type usages (annotations, casts) and member accesses of the name. Matching is by symbol name, ranked by the call site (same file, same directory, receiver type); same-named symbols with no closer candidate can merge, so pass file_path or a qualified name ('Type::method') for overloaded names and verify before refactoring.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "symbol_name": {
                            "type": "string",
                            "description": "Target symbol name. Provide exactly one non-empty symbol_name or symbol_id."
                        },
                        "symbol_id": { "type": "string", "description": "Exact current-index symbol identifier. Provide exactly one non-empty symbol_name or symbol_id." },
                        "file_path": {
                            "type": "string",
                            "description": "Optional file path to disambiguate symbols with identical names across files."
                        },
                        "direction": {
                            "type": "string",
                            "enum": ["callers", "callees"],
                            "description": "Direction of references ('callers' or 'callees', default: 'callers')."
                        },
                        "limit": {
                            "type": "integer",
                            "minimum": 0,
                            "maximum": code_kb_core::queries::MAX_RESULT_LIMIT,
                            "description": "Maximum references to return, 0-200 (default: 20)."
                        },
                        "include_external": {
                            "type": "boolean",
                            "description": "If true, includes external runtime/stdlib primitives in callees (default: false, only internal workspace symbols)."
                        }
                    }
                }),
            },
            Tool {
                name: "find_structural_facts".to_string(),
                description: "Queries framework-level facts (routes, SQL tables, config keys) extracted from AST. If category is omitted, lists all available categories with counts.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "category": {
                            "type": "string",
                            "description": "Optional fact category or pattern to search (e.g. route, query, model, config). If omitted, lists available categories with counts."
                        },
                        "path": {
                            "type": "string",
                            "description": "Optional file path or directory to filter structural facts."
                        },
                        "limit": {
                            "type": "integer",
                            "minimum": 0,
                            "maximum": code_kb_core::queries::MAX_RESULT_LIMIT,
                            "description": "Maximum facts and literals to return per section, 0-200 each (default: 30)."
                        }
                    }
                }),
            },
            Tool {
                name: "blast_radius".to_string(),
                description: "Predicts which downstream symbols are affected and which tests to run before or after edits. With NO arguments, it inspects uncommitted git changes and uses changed files to predict impact and likely tests. You can also pass symbol (or symbol_name) or file (or file_path).".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "symbol": {
                            "type": "string",
                            "description": "Symbol name to seed the impact walk (aliases: symbol_name, name, target)."
                        },
                        "symbol_id": { "type": "string", "description": "Exact current-index symbol identifier. Provide one non-empty symbol or symbol_id." },
                        "file": {
                            "type": "string",
                            "description": "File path to seed the impact walk (aliases: file_path, path)."
                        },
                        "depth": {
                            "type": "integer",
                            "description": "Maximum relationship hops to walk outward from seeds (default: 2)."
                        },
                        "limit": {
                            "type": "integer",
                            "minimum": 0,
                            "maximum": code_kb_core::queries::MAX_RESULT_LIMIT,
                            "description": "Maximum visible tests and impacted symbols per section, 0-200 each (default: 20)."
                        }
                    }
                }),
            },
            Tool {
                name: "telemetry_summary".to_string(),
                description: "Summarizes code-kb tool usage, token consumption, and tokens saved across sessions and workspaces. Useful for diagnosing usage patterns and evaluating agent performance.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "time_window": {
                            "type": "string",
                            "description": "Optional time window for metrics: 'today', '7d', '30d', 'month', 'year', or 'all' (default: 'all')."
                        },
                        "workspace_only": {
                            "type": "boolean",
                            "description": "If true, scopes metrics to the project of the most recent tool call instead of all workspaces (default: false)."
                        },
                        "version": {
                            "type": "string",
                            "description": "Filter metrics by code-kb version (e.g. '1.1.3'), 'current' (default), or 'all'."
                        },
                        "all_versions": {
                            "type": "boolean",
                            "description": "If true, includes telemetry across all historical versions (default: false)."
                        }
                    }
                }),
            },
        ];
        for tool in tools
            .iter_mut()
            .filter(|tool| tool.name != "telemetry_summary")
        {
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
        let resolved = (name != "telemetry_summary").then(|| self.resolve_project_root(arguments));
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
                    Some(err_text.lines().next().unwrap_or("error")),
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
        };

        if let Some(ref conn) = self.telemetry_conn {
            record_tool_call_conn(conn, &telemetry_root, &invocation);
        } else {
            record_tool_call(&telemetry_root, &invocation);
        }

        res
    }

    fn handle_telemetry_summary(&mut self, arguments: &Value) -> CallToolResult {
        let window_arg = arguments
            .get("time_window")
            .or_else(|| arguments.get("since"))
            .or_else(|| arguments.get("window"))
            .and_then(|v| v.as_str());

        let time_window = match window_arg {
            Some(s) => match TimeWindow::parse(s) {
                Some(w) => w,
                None => {
                    return CallToolResult::error(format!(
                        "Invalid time_window '{s}'. Supported values: today, 7d, 30d, month, year, all"
                    ));
                }
            },
            None => TimeWindow::AllTime,
        };

        let workspace_only = arguments
            .get("workspace_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let all_versions = arguments
            .get("all_versions")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let version_arg = arguments.get("version").and_then(|v| v.as_str());

        let version = if all_versions || version_arg == Some("all") {
            None
        } else if let Some(v) = version_arg {
            if v == "current" {
                Some(env!("CARGO_PKG_VERSION").to_string())
            } else {
                Some(v.to_string())
            }
        } else {
            Some(env!("CARGO_PKG_VERSION").to_string())
        };

        let filter = TelemetryFilter {
            time_window,
            workspace_root: if workspace_only {
                Some(self.workspace.canonical_root.clone())
            } else {
                None
            },
            version: version.clone(),
        };

        let as_json = arguments
            .get("json")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if self.telemetry_conn.is_none() {
            self.telemetry_conn = open_global_telemetry_db().ok();
        }

        let conn = match self.telemetry_conn.as_ref() {
            Some(c) => c,
            None => {
                return CallToolResult::error(
                    "Failed to open global telemetry database".to_string(),
                );
            }
        };

        match get_telemetry_summary(conn, &filter) {
            Ok(mut summary) => {
                if !workspace_only {
                    let ws_filter = TelemetryFilter {
                        time_window,
                        workspace_root: Some(self.workspace.canonical_root.clone()),
                        version,
                    };
                    if let Ok(ws_summary) = get_telemetry_summary(conn, &ws_filter) {
                        summary.recent_errors = ws_summary.recent_errors;
                    } else {
                        summary.recent_errors.clear();
                    }
                }

                if as_json {
                    match serde_json::to_string_pretty(&summary) {
                        Ok(json_str) => CallToolResult::text(json_str),
                        Err(e) => CallToolResult::error(format!(
                            "Failed to serialize telemetry summary: {e}"
                        )),
                    }
                } else {
                    let text = format_telemetry_summary(&summary);
                    CallToolResult::text(text)
                }
            }
            Err(e) => CallToolResult::error(format!("Failed to query telemetry summary: {e}")),
        }
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
                if !trimmed.is_empty() {
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

        // telemetry_summary must run before the index wait: it must never create
        // artifact.db on an unindexed repository.
        if name == "telemetry_summary" {
            let result = self.handle_telemetry_summary(arguments);
            if result.is_error {
                tracing::warn!(tool = name, "MCP tool returned error");
            } else {
                tracing::info!(tool = name, "MCP tool executed successfully");
            }
            return result;
        }

        let rec_start = std::time::Instant::now();
        let (reconcile_ms, prepare_error) = match self.reconcile.take() {
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
                let prepared = prepare.join().unwrap_or(Ok(None));
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

        if self.db_path.exists() && self._watcher.is_none() {
            self._watcher = start_watcher(self.workspace.clone(), self.db_path.clone()).ok();
        }

        if !self.db_path.exists() {
            self.reconcile = spawn_index_prepare(&self.workspace, &self.db_path, true);
            let msg = match prepare_error {
                Some(e) => format!(
                    "Initial scan of '{}' failed: {}; it will be retried on the next tool call.",
                    self.workspace.canonical_root.display(),
                    e.trim_end()
                ),
                None => format!(
                    "No index exists for '{}'; it will be retried on the next tool call.",
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
            self.reconcile =
                spawn_index_prepare(&self.workspace, &self.db_path, self._watcher.is_none());
            let msg = format!(
                "Index preparation of '{}' failed: {e}; it will be retried on the next tool call",
                self.workspace.canonical_root.display()
            );
            tracing::error!("{}", msg);
            let mut err_res = CallToolResult::error(msg);
            err_res.reconcile_ms = reconcile_ms;
            err_res.query_ms = Some(0);
            return err_res;
        }

        let conn = match open_read_only(&self.db_path) {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("Failed to open database: {e}");
                tracing::error!("{}", msg);
                let mut err_res = CallToolResult::error(msg);
                err_res.reconcile_ms = reconcile_ms;
                err_res.query_ms = Some(0);
                return err_res;
            }
        };

        let query_start = std::time::Instant::now();
        let mut result = match name {
            "codebase_outline" => {
                let depth = arguments
                    .get("max_depth")
                    .or_else(|| arguments.get("depth"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(2) as usize;
                let path_filter = arguments
                    .get("path")
                    .or_else(|| arguments.get("subpath"))
                    .or_else(|| arguments.get("dir"))
                    .and_then(|v| v.as_str());

                match codebase_outline_op(&self.workspace, &conn, depth, path_filter) {
                    Ok(text) => {
                        let entries = rendered_outline_entries(&text);
                        CallToolResult::text(text).with_logical_result_count(entries)
                    }
                    Err(e) => CallToolResult::error(e.to_string()),
                }
            }
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

                let matches = match search_symbols_scoped(
                    &conn,
                    query,
                    kind,
                    path_filter,
                    include_tests,
                    limit,
                ) {
                    Ok(m) => m,
                    Err(e) => return CallToolResult::error(e.to_string()),
                };

                let (exact_matches, fts_matches) = if matches.is_empty() {
                    if let Err(e) = ensure_fts_index_path(&self.db_path) {
                        return CallToolResult::error(format!("Search index is not ready: {e}"));
                    }
                    let fts = match fts_search_symbols_scoped(
                        &conn,
                        query,
                        kind,
                        path_filter,
                        include_tests,
                        limit,
                    ) {
                        Ok(fts) => fts,
                        Err(e) => return CallToolResult::error(e.to_string()),
                    };
                    (Vec::new(), fts)
                } else {
                    (matches, Vec::new())
                };

                let baseline_paths = exact_matches
                    .iter()
                    .map(|symbol| symbol.path.clone())
                    .chain(fts_matches.iter().map(|hit| hit.symbol.path.clone()))
                    .collect();

                let output = if limit == 0 {
                    ZERO_LIMIT_NOTICE.to_string()
                } else {
                    format_find_symbol_results(query, &exact_matches, &fts_matches, limit)
                };
                CallToolResult::text(output)
                    .with_logical_result_count(exact_matches.len() + fts_matches.len())
                    .with_baseline_paths(baseline_paths)
            }
            "search_symbols" => {
                let query = match arguments
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

                if let Err(e) = ensure_fts_index_path(&self.db_path) {
                    return CallToolResult::error(format!("Search index is not ready: {e}"));
                }

                let matches = match fts_search_symbols_scoped(
                    &conn,
                    query,
                    kind,
                    path_filter,
                    include_tests,
                    limit,
                ) {
                    Ok(m) => m,
                    Err(e) => return CallToolResult::error(e.to_string()),
                };

                let baseline_paths = matches.iter().map(|hit| hit.symbol.path.clone()).collect();

                let output = if limit == 0 {
                    ZERO_LIMIT_NOTICE.to_string()
                } else {
                    format_search_results(query, &matches, limit)
                };
                CallToolResult::text(output)
                    .with_logical_result_count(matches.len())
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

                match get_symbol_body_selected_op(
                    &self.workspace,
                    &self.db_path,
                    &conn,
                    &selector,
                    file_path,
                ) {
                    Ok((symbol, body)) => CallToolResult::text(format_symbol_body(&symbol, &body))
                        .with_logical_result_count(1)
                        .with_baseline_paths(vec![symbol.path.clone()]),
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

                match get_context_slice_selected_op(
                    &self.workspace,
                    &self.db_path,
                    &conn,
                    &selector,
                    file_path,
                    include_external,
                ) {
                    Ok(slice) => {
                        let target_path = slice.target_symbol.path.clone();
                        CallToolResult::text(format_context_slice(&slice))
                            .with_logical_result_count(1)
                            .with_baseline_paths(vec![target_path])
                    }
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
                let limit = match result_limit(arguments, 20) {
                    Ok(limit) => limit,
                    Err(error) => return error,
                };
                let include_external = arguments
                    .get("include_external")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let (target, refs) = match selector {
                    SymbolSelector::Name(name) => {
                        let path = raw_file_path.map(|path| self.workspace.relativize_filter(path));
                        match code_kb_core::find_references_scoped(
                            &conn,
                            &name,
                            direction,
                            limit,
                            include_external,
                            path.as_deref(),
                        ) {
                            Ok(refs) => (name, refs),
                            Err(error) => return CallToolResult::error(error.to_string()),
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
                        match find_references_for_symbol_ext(
                            &conn,
                            &selected.name,
                            direction,
                            limit,
                            &selected.symbol_id,
                            include_external,
                        ) {
                            Ok(refs) => (selected.name, refs),
                            Err(error) => return CallToolResult::error(error.to_string()),
                        }
                    }
                };

                let baseline_paths = refs.iter().map(|site| site.path.clone()).collect();

                let output = if limit == 0 {
                    ZERO_LIMIT_NOTICE.to_string()
                } else {
                    format_references(&target, &refs, direction, limit)
                };
                CallToolResult::text(output)
                    .with_logical_result_count(refs.len())
                    .with_baseline_paths(baseline_paths)
            }
            "find_structural_facts" => {
                let raw_path = arguments
                    .get("path")
                    .or_else(|| arguments.get("file"))
                    .or_else(|| arguments.get("file_path"))
                    .and_then(|v| v.as_str());
                let rel_path = raw_path.map(|p| self.workspace.relativize_filter(p));
                let path_filter = rel_path.as_deref();

                let category = arguments
                    .get("category")
                    .or_else(|| arguments.get("cat"))
                    .or_else(|| arguments.get("type"))
                    .or_else(|| arguments.get("kind"))
                    .or_else(|| arguments.get("pattern"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim();

                let limit = match result_limit(arguments, 30) {
                    Ok(limit) => limit,
                    Err(error) => return error,
                };

                if category.is_empty() {
                    let categories =
                        match list_structural_fact_categories_scoped(&conn, path_filter) {
                            Ok(c) => c,
                            Err(e) => return CallToolResult::error(e.to_string()),
                        };

                    let mut out = format_fact_categories(&categories);
                    if !categories.is_empty() {
                        out.push_str(
                            "\nCall find_structural_facts(category=\"<name>\") to query matches.",
                        );
                    }
                    CallToolResult::text(out).with_logical_result_count(categories.len())
                } else if limit == 0 {
                    CallToolResult::text(ZERO_LIMIT_NOTICE.to_string())
                } else {
                    let facts = match code_kb_core::find_structural_facts_scoped(
                        &conn,
                        category,
                        path_filter,
                        limit,
                    ) {
                        Ok(f) => f,
                        Err(e) => return CallToolResult::error(e.to_string()),
                    };

                    let literals = match code_kb_core::find_literals_scoped(
                        &conn,
                        category,
                        path_filter,
                        limit,
                    ) {
                        Ok(literals) => literals,
                        Err(error) => return CallToolResult::error(error.to_string()),
                    };

                    if facts.is_empty()
                        && literals.is_empty()
                        && code_kb_core::is_category_alias(category)
                    {
                        let categories =
                            match list_structural_fact_categories_scoped(&conn, path_filter) {
                                Ok(c) => c,
                                Err(e) => return CallToolResult::error(e.to_string()),
                            };
                        let out = format!(
                            "No facts match '{category}' in this repository.\n\n{}",
                            format_fact_categories(&categories)
                        );
                        return CallToolResult::text(out)
                            .with_logical_result_count(categories.len());
                    }

                    let baseline_paths = facts
                        .iter()
                        .map(|fact| fact.path.clone())
                        .chain(literals.iter().map(|literal| literal.path.clone()))
                        .collect();

                    CallToolResult::text(format_structural_facts(
                        &facts, &literals, category, limit,
                    ))
                    .with_logical_result_count(facts.len() + literals.len())
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
                let limit = match result_limit(arguments, 20) {
                    Ok(limit) => limit,
                    Err(error) => return error,
                };

                match blast_radius_selected_op(
                    &self.workspace,
                    Some(&self.db_path),
                    &conn,
                    selector,
                    file,
                    depth,
                    limit,
                ) {
                    Ok(res) => {
                        let baseline_paths = res
                            .likely_tests
                            .iter()
                            .map(|test| test.path.clone())
                            .chain(res.impacted_symbols.iter().map(|sym| sym.path.clone()))
                            .collect();

                        CallToolResult::text(format_blast_radius(&res))
                            .with_logical_result_count(
                                res.likely_tests.len() + res.impacted_symbols.len(),
                            )
                            .with_baseline_paths(baseline_paths)
                    }
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
                    "instructions": "Pass project_root, the absolute path of the project or git worktree you work in, on every call except telemetry_summary. For progressive code exploration, start with codebase_outline (~200 tokens) for directory structure. Use file_skeleton to inspect interfaces without bodies. Use lookup_symbol for exact name lookups and search_symbols for natural-language concepts. Use get_symbol_context for surgical context before native file edits; use get_symbol_body only when the isolated implementation is needed. Trace callers/callees with find_references. Use blast_radius to assess downstream impact and predict which tests to run before or after changes."
                });
                Some(JsonRpcResponse::success(id, init_result))
            }
            "notifications/initialized" => None,
            "ping" => Some(JsonRpcResponse::success(id, json!({}))),
            "tools/list" => {
                let tools = Self::tool_definitions();
                Some(JsonRpcResponse::success(id, json!({ "tools": tools })))
            }
            "tools/call" => {
                let params = request.params.unwrap_or(Value::Null);
                let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let arguments = params.get("arguments").unwrap_or(&Value::Null);

                let result = self.handle_call_tool(tool_name, arguments);
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
            );
            INSERT INTO files VALUES ('f', 'src/lib.rs', 'rust', 'hash', 0, 1, '2026-01-01');
            INSERT INTO symbols VALUES
                ('one', 'f', 'src/lib.rs', 'rust', 'needle', 'function', 'fn needle()', NULL, 'pub', NULL, 1, 0, 1, 1, 0, 1, 1, 0, 1, 1, 0, 1, 'hash', NULL, 0, 0),
                ('two', 'f', 'src/lib.rs', 'rust', 'needle', 'function', 'fn needle_alt()', NULL, 'pub', NULL, 2, 0, 2, 1, 2, 3, 2, 0, 2, 1, 2, 3, 'hash', NULL, 0, 0);",
        )
        .unwrap();
        code_kb_core::db::ensure_fts_index(&conn).unwrap();
        drop(conn);

        let workspace = Workspace::new(root.to_path_buf());
        let mut server = McpServer {
            launch_root: workspace.canonical_root.clone(),
            workspace,
            db_path,
            explicit_db: None,
            _watcher: None,
            telemetry_conn: Some(code_kb_core::telemetry::open_telemetry_db_at(root).unwrap()),
            reconcile: None,
            parked: Vec::new(),
            prepared: true,
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
    }
}
