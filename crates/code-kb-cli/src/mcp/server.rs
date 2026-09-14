use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use code_kb_core::{
    Connection, TelemetryFilter, TimeWindow, WatcherHandle, Workspace, WorkspaceError,
    blast_radius_op, codebase_outline_op, ensure_fts_index_path, file_skeleton_op,
    format_blast_radius, format_context_slice, format_fact_categories, format_find_symbol_results,
    format_references, format_replace_symbol_result, format_search_results,
    format_structural_facts, format_symbol_body, format_telemetry_summary,
    fts_search_symbols_scoped, get_context_slice_op, get_symbol_body_op, get_telemetry_summary,
    list_structural_fact_categories, migrate_legacy_workspace_telemetry, open_global_telemetry_db,
    open_read_only, reconcile_offline_edits, record_tool_call, record_tool_call_conn,
    replace_symbol_body, scan_workspace, search_symbols_scoped, start_watcher,
};

use super::protocol::{CallToolResult, JsonRpcRequest, JsonRpcResponse, Tool};

pub struct McpServer {
    pub workspace: Workspace,
    pub db_path: PathBuf,
    pub explicit_db: Option<PathBuf>,
    pub _watcher: Option<WatcherHandle>,
    pub telemetry_conn: Option<Connection>,
}

impl McpServer {
    pub fn new(workspace: Workspace, explicit_db: Option<&Path>) -> anyhow::Result<Self> {
        let db_path = workspace.locate_db(explicit_db).unwrap_or_else(|_| {
            workspace
                .canonical_root
                .join(".code-kb")
                .join("artifact.db")
        });

        // Trigger cold-start reconciliation and ensure FTS index in background thread if database exists
        if db_path.exists() {
            let ws_clone = workspace.clone();
            let db_clone = db_path.clone();
            std::thread::spawn(move || {
                let _ = ensure_fts_index_path(&db_clone);
                if let Ok(conn) = open_read_only(&db_clone) {
                    let _ = reconcile_offline_edits(&ws_clone, &db_clone, &conn);
                }
            });
        }

        // Tier 3: Start background file watcher with debounce and git storm circuit breaker
        let watcher = if db_path.exists() {
            start_watcher(workspace.clone(), db_path.clone()).ok()
        } else {
            None
        };

        let telemetry_conn = open_global_telemetry_db().ok();
        if let Some(ref conn) = telemetry_conn {
            let _ = migrate_legacy_workspace_telemetry(conn, &workspace.root);
        }

        Ok(Self {
            workspace,
            db_path,
            explicit_db: explicit_db.map(|p| p.to_path_buf()),
            _watcher: watcher,
            telemetry_conn,
        })
    }

    pub fn bind_workspace(&mut self, path: &Path) -> Result<(), WorkspaceError> {
        let ws = Workspace::discover(Some(path))?;
        let db_path = ws
            .locate_db(self.explicit_db.as_deref())
            .unwrap_or_else(|_| ws.canonical_root.join(".code-kb").join("artifact.db"));

        // Guard: only commit binding if target db exists OR target root has a repository marker
        let root = &ws.canonical_root;
        let is_valid = db_path.exists()
            || root.join(".git").exists()
            || root.join("Cargo.toml").exists()
            || root.join("package.json").exists()
            || root.join("go.mod").exists()
            || root.join("pyproject.toml").exists();

        if !is_valid {
            return Err(WorkspaceError::ArtifactNotFound(db_path));
        }

        tracing::info!(
            workspace = %ws.canonical_root.display(),
            db = %db_path.display(),
            "Bound workspace dynamically"
        );

        if !code_kb_core::workspace::paths_equal(&self.workspace.canonical_root, &ws.canonical_root)
            || self._watcher.is_none()
        {
            if db_path.exists() {
                let _ = ensure_fts_index_path(&db_path);
                let ws_clone = ws.clone();
                let db_clone = db_path.clone();
                std::thread::spawn(move || {
                    if let Ok(conn) = open_read_only(&db_clone) {
                        let _ = reconcile_offline_edits(&ws_clone, &db_clone, &conn);
                    }
                });
                self._watcher = start_watcher(ws.clone(), db_path.clone()).ok();
            } else {
                self._watcher = None;
            }
        }

        if self.telemetry_conn.is_none() {
            self.telemetry_conn = open_global_telemetry_db().ok();
        }
        if let Some(ref conn) = self.telemetry_conn {
            let _ = migrate_legacy_workspace_telemetry(conn, &ws.root);
        }
        self.workspace = ws;
        self.db_path = db_path;
        Ok(())
    }

    pub fn tool_definitions() -> Vec<Tool> {
        vec![
            Tool {
                name: "codebase_outline".to_string(),
                description: "Provides a top-level architectural orientation of the repository or sub-package in ~200 tokens. Start here when exploring unfamiliar code instead of running directory listings or reading files.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Subdirectory to scope the outline to. Defaults to workspace root."
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
                description: "Returns all types, traits, functions, signatures, docstrings, and visibility for a file with implementation bodies stripped. Use this instead of reading the entire file when inspecting interfaces and types.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "file_path": {
                            "type": "string",
                            "description": "File path relative to workspace root or absolute path."
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
                            "description": "Include test functions and containers (default: false)."
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of symbols to return (default: 20)."
                        }
                    },
                    "required": ["query"]
                }),
            },
            Tool {
                name: "search_symbols".to_string(),
                description: "Natural-language and keyword search over symbol names, signatures, and docstrings. Use when the exact identifier is unknown or searching for concepts (e.g. 'auth middleware', 'retry loop'). Do NOT use if you already know the exact symbol name; use lookup_symbol instead.".to_string(),
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
                            "description": "Include test functions and containers (default: false)."
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of symbols to return (default: 20)."
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
                            "description": "Full or qualified symbol name."
                        },
                        "file_path": {
                            "type": "string",
                            "description": "Optional file path to disambiguate identical symbol names."
                        }
                    },
                    "required": ["symbol_name"]
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
                            "description": "Target symbol name."
                        },
                        "file_path": {
                            "type": "string",
                            "description": "Optional file path to disambiguate identical symbol names."
                        },
                        "include_external": {
                            "type": "boolean",
                            "description": "Include external stdlib/runtime calls in callee signatures (default: false)."
                        }
                    },
                    "required": ["symbol_name"]
                }),
            },
            Tool {
                name: "find_references".to_string(),
                description: "Discovers callers or callees of a symbol using AST relationship facts. Use to trace call graphs and assess impact before refactoring.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "symbol_name": {
                            "type": "string",
                            "description": "Target symbol name."
                        },
                        "direction": {
                            "type": "string",
                            "enum": ["callers", "callees"],
                            "description": "Direction of references ('callers' or 'callees', default: 'callers')."
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum references to return (default: 20)."
                        },
                        "include_external": {
                            "type": "boolean",
                            "description": "If true, includes external runtime/stdlib primitives in callees (default: false, only internal workspace symbols)."
                        }
                    },
                    "required": ["symbol_name"]
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
                        "limit": {
                            "type": "integer",
                            "description": "Maximum results to return (default: 30)."
                        }
                    }
                }),
            },
            Tool {
                name: "blast_radius".to_string(),
                description: "Predicts which downstream symbols are affected and which tests to run before or after edits. With NO arguments, it automatically inspects uncommitted git working-tree changes to map edited lines to impacted symbols and likely tests. You can also pass symbol (or symbol_name) or file (or file_path).".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "symbol": {
                            "type": "string",
                            "description": "Symbol name to seed the impact walk (aliases: symbol_name, name, target)."
                        },
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
                            "description": "Maximum visible test and impact rows (default: 20)."
                        }
                    }
                }),
            },
            Tool {
                name: "replace_symbol_body".to_string(),
                description: "Atomically replaces the implementation body of a function or method by symbol name. Performs pre-flight tree-sitter syntax validation and immediate SQLite re-indexing in a single turn.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "symbol_name": {
                            "type": "string",
                            "description": "Name of the symbol to edit."
                        },
                        "file_path": {
                            "type": "string",
                            "description": "Path to the file containing the symbol."
                        },
                        "new_body": {
                            "type": "string",
                            "description": "New body content to insert."
                        },
                        "expected_body_hash": {
                            "type": "string",
                            "description": "Optional optimistic lock hash of current body (obtained from get_symbol_body or get_symbol_context)."
                        }
                    },
                    "required": ["symbol_name", "file_path", "new_body"]
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
                            "description": "If true, scopes metrics to the currently bound workspace instead of all workspaces (default: false)."
                        }
                    }
                }),
            },
        ]
    }

    pub fn handle_call_tool(&mut self, name: &str, arguments: &Value) -> CallToolResult {
        let start = std::time::Instant::now();
        let res = self.handle_call_tool_inner(name, arguments);
        let duration_ms = start.elapsed().as_millis() as u64;

        let (outcome, error_msg, bytes, est_tokens, est_tokens_saved) = if res.is_error {
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
            )
        } else {
            let bytes: usize = res.content.iter().map(|c| c.text.len()).sum();
            let est_tokens = bytes / 4;
            let outcome = if res.content.is_empty()
                || (res.content.len() == 1 && res.content[0].text.is_empty())
            {
                "empty"
            } else {
                "ok"
            };
            let est_tokens_saved = match name {
                "file_skeleton" | "get_symbol_body" | "get_symbol_context" => {
                    let file_size = arguments
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
                        .map(|m| m.len() as usize);

                    if let Some(size) = file_size {
                        (size / 4).saturating_sub(est_tokens)
                    } else {
                        est_tokens.saturating_mul(3)
                    }
                }
                "lookup_symbol" | "search_symbols" => est_tokens.saturating_mul(3),
                _ => 0,
            };
            (outcome, None, bytes, est_tokens, est_tokens_saved)
        };

        let invocation = code_kb_core::ToolInvocation {
            tool: name,
            duration_ms,
            outcome,
            error_message: error_msg,
            result_count: res.content.len(),
            bytes_returned: bytes,
            est_tokens,
            est_tokens_saved,
        };

        if let Some(ref conn) = self.telemetry_conn {
            record_tool_call_conn(conn, &self.workspace.root, &invocation);
        } else {
            record_tool_call(&self.workspace.root, &invocation);
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

        let filter = TelemetryFilter {
            time_window,
            workspace_root: if workspace_only {
                Some(self.workspace.canonical_root.clone())
            } else {
                None
            },
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

    fn handle_call_tool_inner(&mut self, name: &str, arguments: &Value) -> CallToolResult {
        tracing::info!(tool = name, args = %arguments, "MCP tool called");

        // Early routing for telemetry_summary and unadvertised alias code_kb_stats
        // Must execute before auto-scan check and workspace rebinding so telemetry queries:
        // 1. Succeed immediately on unindexed repositories without creating artifact.db
        // 2. Never rebind the session's active workspace if extra path/file parameters are supplied
        if name == "telemetry_summary" || name == "code_kb_stats" {
            let result = self.handle_telemetry_summary(arguments);
            if result.is_error {
                tracing::warn!(tool = name, "MCP tool returned error");
            } else {
                tracing::info!(tool = name, "MCP tool executed successfully");
            }
            return result;
        }

        // Dynamically bind workspace if passed explicitly or if candidate path points to a different workspace
        if let Some(ws_str) = arguments.get("workspace").and_then(|v| v.as_str()) {
            let _ = self.bind_workspace(Path::new(ws_str));
        } else if let Some(candidate) = arguments
            .get("file_path")
            .or_else(|| arguments.get("path"))
            .or_else(|| arguments.get("file"))
            .and_then(|v| v.as_str())
        {
            let p = if candidate.starts_with("file://") {
                code_kb_core::parse_file_uri(candidate)
                    .unwrap_or_else(|| std::path::PathBuf::from(candidate))
            } else {
                std::path::PathBuf::from(candidate)
            };
            let abs_candidate = if p.is_absolute() {
                code_kb_core::normalize_path(&p)
            } else {
                code_kb_core::normalize_path(&self.workspace.canonical_root.join(&p))
            };

            // Detect if this path belongs to another workspace or a nested git worktree
            if let Ok(target_root) = Workspace::find_workspace_root(&abs_candidate) {
                if !code_kb_core::workspace::paths_equal(
                    &target_root,
                    &self.workspace.canonical_root,
                ) {
                    let _ = self.bind_workspace(&target_root);
                }
            } else if !self.db_path.exists() && abs_candidate.exists() {
                let _ = self.bind_workspace(&abs_candidate);
            }
        }

        // Auto-scan if workspace is a known repository but database artifact does not exist yet
        if !self.db_path.exists() {
            let root = &self.workspace.canonical_root;
            if root.join(".git").exists()
                || root.join("Cargo.toml").exists()
                || root.join("package.json").exists()
                || root.join("go.mod").exists()
                || root.join("pyproject.toml").exists()
            {
                tracing::info!(ws = %root.display(), "Database not found; running automatic initial scan");

                // Worktree fast-path: if this is a git worktree and parent repo has artifact.db,
                // copy parent DB and reconcile instead of scanning from scratch.
                let mut fast_path_taken = false;
                let git_marker = root.join(".git");
                if git_marker.is_file()
                    && let Ok(git_content) = std::fs::read_to_string(&git_marker)
                    && let Some(gitdir_line) =
                        git_content.lines().find(|l| l.starts_with("gitdir:"))
                {
                    let gitdir_str = gitdir_line.trim_start_matches("gitdir:").trim();
                    let gitdir_path = Path::new(gitdir_str);
                    let mut parent_probe = if gitdir_path.is_absolute() {
                        gitdir_path.to_path_buf()
                    } else {
                        root.join(gitdir_path)
                    };
                    while let Some(parent) = parent_probe.parent() {
                        if parent == parent_probe {
                            break;
                        }
                        if parent.join(".git").exists() {
                            let parent_db = parent.join(".code-kb").join("artifact.db");
                            if parent_db.exists() {
                                // Flush WAL checkpoint so committed transactions are flushed into parent_db before copying
                                let flushed = if let Ok(parent_conn) =
                                    code_kb_core::db::open_read_write(&parent_db)
                                {
                                    code_kb_core::db::checkpoint_truncate(&parent_conn).is_ok()
                                } else {
                                    false
                                };
                                if flushed {
                                    if let Some(db_dir) = self.db_path.parent() {
                                        let _ = std::fs::create_dir_all(db_dir);
                                    }
                                    if std::fs::copy(&parent_db, &self.db_path).is_ok() {
                                        tracing::info!(
                                            from = %parent_db.display(),
                                            to = %self.db_path.display(),
                                            "Worktree fast-path: copied parent database, reconciling"
                                        );
                                        let _ = ensure_fts_index_path(&self.db_path);
                                        if let Ok(conn) = open_read_only(&self.db_path) {
                                            let _ = reconcile_offline_edits(
                                                &self.workspace,
                                                &self.db_path,
                                                &conn,
                                            );
                                        }
                                        fast_path_taken = true;
                                    }
                                }
                            }
                            break;
                        }
                        parent_probe = parent.to_path_buf();
                    }
                }

                if !fast_path_taken
                    && let Err(e) = scan_workspace(&self.workspace, &self.db_path, false)
                {
                    tracing::error!("Initial scan failed: {e}");
                }

                if self.db_path.exists() && self._watcher.is_none() {
                    self._watcher =
                        start_watcher(self.workspace.clone(), self.db_path.clone()).ok();
                }
            }
        }

        if !self.db_path.exists() {
            let msg = format!(
                "Database artifact not found at '{}'. Please configure code-kb with '--root <repo-path>' in your MCP config or invoke a tool with a path inside a project repository.",
                self.db_path.display()
            );
            tracing::warn!("{}", msg);
            return CallToolResult::error(msg);
        }

        let conn = match open_read_only(&self.db_path) {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("Failed to open database: {e}");
                tracing::error!("{}", msg);
                return CallToolResult::error(msg);
            }
        };

        let result = match name {
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
                    Ok(text) => CallToolResult::text(text),
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

                match file_skeleton_op(&self.workspace, &self.db_path, &conn, file_path) {
                    Ok(skeleton) => CallToolResult::text(skeleton),
                    Err(e) => CallToolResult::error(e.to_string()),
                }
            }
            "lookup_symbol" => {
                let raw_query = match arguments
                    .get("query")
                    .or_else(|| arguments.get("name"))
                    .or_else(|| arguments.get("q"))
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
                let limit = arguments
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(20) as usize;

                let matches = if (query.contains("::") || query.contains('.'))
                    && let Ok(Some(sym)) =
                        code_kb_core::get_symbol_by_name(&conn, query, path_filter)
                {
                    vec![sym]
                } else {
                    match search_symbols_scoped(
                        &conn,
                        query,
                        kind,
                        path_filter,
                        include_tests,
                        limit,
                    ) {
                        Ok(m) => m,
                        Err(e) => return CallToolResult::error(e.to_string()),
                    }
                };

                let (exact_matches, fts_matches) = if matches.is_empty() {
                    let _ = ensure_fts_index_path(&self.db_path);
                    let fts = fts_search_symbols_scoped(
                        &conn,
                        query,
                        kind,
                        path_filter,
                        include_tests,
                        limit,
                    )
                    .unwrap_or_default();
                    (Vec::new(), fts)
                } else {
                    (matches, Vec::new())
                };

                CallToolResult::text(format_find_symbol_results(
                    query,
                    &exact_matches,
                    &fts_matches,
                ))
            }
            "search_symbols" => {
                let query = match arguments
                    .get("query")
                    .or_else(|| arguments.get("name"))
                    .or_else(|| arguments.get("q"))
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
                let limit = arguments
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(20) as usize;

                let _ = ensure_fts_index_path(&self.db_path);

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

                CallToolResult::text(format_search_results(query, &matches))
            }
            "get_symbol_body" => {
                let raw_name = match arguments
                    .get("symbol_name")
                    .or_else(|| arguments.get("symbol"))
                    .or_else(|| arguments.get("name"))
                    .and_then(|v| v.as_str())
                {
                    Some(n) => n,
                    None => {
                        return CallToolResult::error("Missing required parameter: symbol_name");
                    }
                };
                let symbol_name = Self::sanitize_symbol_name(raw_name);
                let file_path = arguments
                    .get("file_path")
                    .or_else(|| arguments.get("file"))
                    .or_else(|| arguments.get("path"))
                    .and_then(|v| v.as_str());

                match get_symbol_body_op(
                    &self.workspace,
                    &self.db_path,
                    &conn,
                    &symbol_name,
                    file_path,
                ) {
                    Ok((symbol, body)) => CallToolResult::text(format_symbol_body(&symbol, &body)),
                    Err(e) => CallToolResult::error(e.to_string()),
                }
            }
            "get_symbol_context" => {
                let raw_name = match arguments
                    .get("symbol_name")
                    .or_else(|| arguments.get("symbol"))
                    .or_else(|| arguments.get("name"))
                    .and_then(|v| v.as_str())
                {
                    Some(n) => n,
                    None => {
                        return CallToolResult::error("Missing required parameter: symbol_name");
                    }
                };
                let symbol_name = Self::sanitize_symbol_name(raw_name);
                let file_path = arguments
                    .get("file_path")
                    .or_else(|| arguments.get("file"))
                    .or_else(|| arguments.get("path"))
                    .and_then(|v| v.as_str());
                let include_external = arguments
                    .get("include_external")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                match get_context_slice_op(
                    &self.workspace,
                    &self.db_path,
                    &conn,
                    &symbol_name,
                    file_path,
                    include_external,
                ) {
                    Ok(slice) => CallToolResult::text(format_context_slice(&slice)),
                    Err(e) => CallToolResult::error(e.to_string()),
                }
            }
            "find_references" => {
                let raw_name = match arguments
                    .get("symbol_name")
                    .or_else(|| arguments.get("symbol"))
                    .or_else(|| arguments.get("name"))
                    .and_then(|v| v.as_str())
                {
                    Some(n) => n,
                    None => {
                        return CallToolResult::error("Missing required parameter: symbol_name");
                    }
                };
                let symbol_name = Self::sanitize_symbol_name(raw_name);
                let direction = arguments
                    .get("direction")
                    .and_then(|v| v.as_str())
                    .unwrap_or("callers");
                let limit = arguments
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(20) as usize;
                let include_external = arguments
                    .get("include_external")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let refs = match code_kb_core::find_references_ext(
                    &conn,
                    &symbol_name,
                    direction,
                    limit,
                    include_external,
                ) {
                    Ok(r) => r,
                    Err(e) => return CallToolResult::error(e.to_string()),
                };

                CallToolResult::text(format_references(&symbol_name, &refs, direction, limit))
            }
            "find_structural_facts" => {
                let category = arguments
                    .get("category")
                    .or_else(|| arguments.get("cat"))
                    .or_else(|| arguments.get("type"))
                    .or_else(|| arguments.get("kind"))
                    .or_else(|| arguments.get("pattern"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim();

                if category.is_empty() {
                    let categories = match list_structural_fact_categories(&conn) {
                        Ok(c) => c,
                        Err(e) => return CallToolResult::error(e.to_string()),
                    };

                    let mut out = format_fact_categories(&categories);
                    if !categories.is_empty() {
                        out.push_str(
                            "\nCall find_structural_facts(category=\"<name>\") to query matches.",
                        );
                    }
                    return CallToolResult::text(out);
                }

                let limit = arguments
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(30) as usize;

                let facts = match code_kb_core::find_structural_facts(&conn, category, limit) {
                    Ok(f) => f,
                    Err(e) => return CallToolResult::error(e.to_string()),
                };

                let literals =
                    code_kb_core::find_literals(&conn, category, limit).unwrap_or_default();

                CallToolResult::text(format_structural_facts(&facts, &literals, category))
            }
            "blast_radius" | "impact" => {
                let raw_symbol = arguments
                    .get("symbol")
                    .or_else(|| arguments.get("symbol_name"))
                    .or_else(|| arguments.get("name"))
                    .or_else(|| arguments.get("target"))
                    .and_then(|v| v.as_str());
                let sanitized_symbol = raw_symbol.map(Self::sanitize_symbol_name);
                let symbol = sanitized_symbol.as_deref();
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
                let limit = arguments
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(20) as usize;

                match blast_radius_op(&self.workspace, &conn, symbol, file, depth, limit) {
                    Ok(res) => CallToolResult::text(format_blast_radius(&res)),
                    Err(e) => CallToolResult::error(e.to_string()),
                }
            }
            "replace_symbol_body" => {
                let raw_name = match arguments
                    .get("symbol_name")
                    .or_else(|| arguments.get("symbol"))
                    .or_else(|| arguments.get("name"))
                    .and_then(|v| v.as_str())
                {
                    Some(n) => n,
                    None => {
                        return CallToolResult::error("Missing required parameter: symbol_name");
                    }
                };
                let symbol_name = Self::sanitize_symbol_name(raw_name);
                let file_path = match arguments
                    .get("file_path")
                    .or_else(|| arguments.get("file"))
                    .or_else(|| arguments.get("path"))
                    .and_then(|v| v.as_str())
                {
                    Some(p) => p,
                    None => return CallToolResult::error("Missing required parameter: file_path"),
                };
                let new_body = match arguments
                    .get("new_body")
                    .or_else(|| arguments.get("body"))
                    .or_else(|| arguments.get("code"))
                    .or_else(|| arguments.get("content"))
                    .and_then(|v| v.as_str())
                {
                    Some(b) => b,
                    None => return CallToolResult::error("Missing required parameter: new_body"),
                };
                let expected_hash = arguments
                    .get("expected_body_hash")
                    .or_else(|| arguments.get("body_hash"))
                    .or_else(|| arguments.get("expected_hash"))
                    .and_then(|v| v.as_str());

                match replace_symbol_body(
                    &self.workspace,
                    &self.db_path,
                    &conn,
                    &symbol_name,
                    file_path,
                    new_body,
                    expected_hash,
                ) {
                    Ok(res) => CallToolResult::text(format_replace_symbol_result(&res)),
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
                if let Some(params) = &request.params {
                    let mut candidate = None;
                    if let Some(roots) = params.get("roots").and_then(|r| r.as_array())
                        && let Some(u) = roots
                            .first()
                            .and_then(|r| r.get("uri"))
                            .and_then(|u| u.as_str())
                    {
                        candidate = Some(u);
                    } else if let Some(u) = params.get("rootUri").and_then(|u| u.as_str()) {
                        candidate = Some(u);
                    } else if let Some(u) = params.get("rootPath").and_then(|u| u.as_str()) {
                        candidate = Some(u);
                    } else if let Some(folders) =
                        params.get("workspaceFolders").and_then(|f| f.as_array())
                        && let Some(u) = folders
                            .first()
                            .and_then(|f| f.get("uri"))
                            .and_then(|u| u.as_str())
                    {
                        candidate = Some(u);
                    }

                    if let Some(cand) = candidate
                        && let Some(path) = code_kb_core::parse_file_uri(cand)
                    {
                        let _ = self.bind_workspace(&path);
                    }
                }

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
                    "instructions": "For progressive code exploration, start with codebase_outline (~200 tokens) for directory structure. Use file_skeleton to inspect interfaces without bodies. Use lookup_symbol for exact name lookups and search_symbols for natural-language concepts. Use get_symbol_context for surgical context before editing; use get_symbol_body only when the isolated implementation is needed. Trace callers/callees with find_references. Use blast_radius to assess downstream impact and predict which tests to run before/after edits. Use replace_symbol_body for atomic, syntax-validated edits with immediate re-indexing."
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
    use super::*;

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
