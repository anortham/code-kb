use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use serde_json::{json, Value};

use code_kb_core::{
    ensure_fresh_file, format_codebase_outline, format_context_slice,
    format_file_skeleton, format_references, get_symbol_by_name, load_file_symbols,
    load_files, open_read_only, reconcile_offline_edits, replace_symbol_body,
    scan_workspace, search_symbols, slice_symbol_body, start_watcher, ContextSlice,
    WatcherHandle, Workspace, WorkspaceError,
};

use super::protocol::{CallToolResult, JsonRpcRequest, JsonRpcResponse, Tool};

pub struct McpServer {
    pub workspace: Workspace,
    pub db_path: PathBuf,
    pub _watcher: Option<WatcherHandle>,
}

impl McpServer {
    pub fn new(workspace: Workspace, explicit_db: Option<&Path>) -> anyhow::Result<Self> {
        let db_path = workspace.locate_db(explicit_db).unwrap_or_else(|_| {
            workspace.canonical_root.join(".code-kb").join("artifact.db")
        });

        // Trigger cold-start reconciliation in background thread if database already exists
        if db_path.exists() {
            let ws_clone = workspace.clone();
            let db_clone = db_path.clone();
            std::thread::spawn(move || {
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

        Ok(Self {
            workspace,
            db_path,
            _watcher: watcher,
        })
    }

    pub fn bind_workspace(&mut self, path: &Path) -> Result<(), WorkspaceError> {
        let ws = Workspace::discover(Some(path))?;
        let db_path = ws.locate_db(None).unwrap_or_else(|_| {
            ws.canonical_root.join(".code-kb").join("artifact.db")
        });

        tracing::info!(
            workspace = %ws.canonical_root.display(),
            db = %db_path.display(),
            "Bound workspace dynamically"
        );

        if self.workspace.canonical_root != ws.canonical_root || self._watcher.is_none() {
            if db_path.exists() {
                self._watcher = start_watcher(ws.clone(), db_path.clone()).ok();
            } else {
                self._watcher = None;
            }
        }

        self.workspace = ws;
        self.db_path = db_path;
        Ok(())
    }

    pub fn tool_definitions() -> Vec<Tool> {
        vec![
            Tool {
                name: "codebase_outline".to_string(),
                description: "Provides a top-level architectural orientation of the repository or sub-package without reading raw files.".to_string(),
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
                description: "Returns all types, traits, functions, signatures, docstrings, and visibility for a file with implementation bodies stripped.".to_string(),
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
                name: "find_symbol".to_string(),
                description: "Fast semantic lookup across symbols in the repository, replacing text grep.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Symbol name or search pattern."
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
                description: "Retrieves the exact implementation body of a specific symbol.".to_string(),
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
                name: "get_context_slice".to_string(),
                description: "Surgical context bundle combining target body, callee signatures, parameter types, and related tests in one call.".to_string(),
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
                        }
                    },
                    "required": ["symbol_name"]
                }),
            },
            Tool {
                name: "find_references".to_string(),
                description: "Discovers callers or callees of a symbol.".to_string(),
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
                            "description": "Direction of references ('callers' or 'callees')."
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum references to return (default: 20)."
                        }
                    },
                    "required": ["symbol_name", "direction"]
                }),
            },
            Tool {
                name: "find_structural_facts".to_string(),
                description: "Queries framework-level facts (routes, SQL tables, config keys) extracted from AST.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "category": {
                            "type": "string",
                            "description": "Fact category or pattern to search (e.g. route, query, model, config)."
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum results to return (default: 30)."
                        }
                    },
                    "required": ["category"]
                }),
            },
            Tool {
                name: "replace_symbol_body".to_string(),
                description: "Atomically replaces the implementation body of a function or method by symbol name.".to_string(),
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
                            "description": "Optional optimistic lock hash of current body."
                        }
                    },
                    "required": ["symbol_name", "file_path", "new_body"]
                }),
            },
        ]
    }

    pub fn handle_call_tool(&mut self, name: &str, arguments: &Value) -> CallToolResult {
        tracing::info!(tool = name, args = %arguments, "MCP tool called");

        // Dynamically bind workspace if passed explicitly or if file_path is provided
        if let Some(ws_str) = arguments.get("workspace").and_then(|v| v.as_str()) {
            let _ = self.bind_workspace(Path::new(ws_str));
        } else if !self.db_path.exists() {
            if let Some(candidate) = arguments.get("file_path").or_else(|| arguments.get("path")).and_then(|v| v.as_str()) {
                let p = Path::new(candidate);
                if p.is_absolute() || p.exists() {
                    let _ = self.bind_workspace(p);
                }
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
                if let Err(e) = scan_workspace(&self.workspace, &self.db_path, false) {
                    tracing::error!("Initial scan failed: {e}");
                } else if self._watcher.is_none() {
                    self._watcher = start_watcher(self.workspace.clone(), self.db_path.clone()).ok();
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
                    .get("depth")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(2) as usize;
                let path_filter = arguments
                    .get("path")
                    .and_then(|v| v.as_str());

                let files = match load_files(&conn) {
                    Ok(f) => f,
                    Err(e) => return CallToolResult::error(e.to_string()),
                };

                let mut symbols_by_file = HashMap::new();
                for f in &files {
                    if let Ok(syms) = load_file_symbols(&conn, &f.path) {
                        symbols_by_file.insert(f.path.clone(), syms);
                    }
                }

                let text = format_codebase_outline(
                    &self.workspace.repo_name,
                    &files,
                    &symbols_by_file,
                    depth,
                    path_filter,
                );
                CallToolResult::text(text)
            }
            "file_skeleton" => {
                let file_path = match arguments.get("file_path").and_then(|v| v.as_str()) {
                    Some(p) => p,
                    None => return CallToolResult::error("Missing required parameter: file_path"),
                };

                let (_, rel_path) = match self.workspace.resolve_path(Path::new(file_path)) {
                    Ok(res) => res,
                    Err(e) => return CallToolResult::error(e.to_string()),
                };

                // Tier 2: JIT Staleness Guard
                let _ = ensure_fresh_file(&self.workspace, &self.db_path, &conn, &rel_path);

                let symbols = match load_file_symbols(&conn, &rel_path) {
                    Ok(s) => s,
                    Err(e) => return CallToolResult::error(e.to_string()),
                };

                let file_meta = code_kb_core::get_file(&conn, &rel_path).ok().flatten();
                let line_count = file_meta.and_then(|m| m.line_count.map(|l| l as usize));

                let skeleton = format_file_skeleton(&rel_path, &symbols, line_count);
                CallToolResult::text(skeleton)
            }
            "find_symbol" => {
                let query = match arguments.get("query").and_then(|v| v.as_str()) {
                    Some(q) => q,
                    None => return CallToolResult::error("Missing required parameter: query"),
                };
                let kind = arguments.get("kind").and_then(|v| v.as_str());
                let include_tests = arguments
                    .get("is_test")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let limit = arguments
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(20) as usize;

                let matches = match search_symbols(&conn, query, kind, include_tests, limit) {
                    Ok(m) => m,
                    Err(e) => return CallToolResult::error(e.to_string()),
                };

                let mut out = format!("Found {} symbols matching \"{query}\":\n\n", matches.len());
                for s in matches {
                    let sig = s.signature.as_deref().unwrap_or(&s.name);
                    out.push_str(&format!(
                        "- {} `{}` [{}:{}-{}]\n",
                        s.kind, s.name, s.path, s.start_line, s.end_line
                    ));
                    out.push_str(&format!("  Signature: {sig}\n"));
                    if let Some(doc) = s.doc_comment {
                        let first_line = doc.lines().next().unwrap_or("").trim();
                        out.push_str(&format!("  Doc: {first_line}\n"));
                    }
                }

                CallToolResult::text(out)
            }
            "get_symbol_body" => {
                let symbol_name = match arguments.get("symbol_name").and_then(|v| v.as_str()) {
                    Some(n) => n,
                    None => return CallToolResult::error("Missing required parameter: symbol_name"),
                };
                let file_path = arguments.get("file_path").and_then(|v| v.as_str());

                let symbol = match get_symbol_by_name(&conn, symbol_name, file_path) {
                    Ok(Some(s)) => s,
                    Ok(None) => {
                        return CallToolResult::error(format!("Symbol '{symbol_name}' not found"));
                    }
                    Err(e) => return CallToolResult::error(e.to_string()),
                };

                // Tier 2: JIT Staleness Guard
                let _ = ensure_fresh_file(&self.workspace, &self.db_path, &conn, &symbol.path);

                let abs_file = self.workspace.canonical_root.join(&symbol.path);
                let body = match slice_symbol_body(&abs_file, &symbol) {
                    Ok(b) => b,
                    Err(e) => return CallToolResult::error(e.to_string()),
                };

                let mut out = format!(
                    "// {}:{}-{} ({})\n",
                    symbol.path, symbol.start_line, symbol.end_line, symbol.name
                );
                out.push_str(&body);
                CallToolResult::text(out)
            }
            "get_context_slice" => {
                let symbol_name = match arguments.get("symbol_name").and_then(|v| v.as_str()) {
                    Some(n) => n,
                    None => return CallToolResult::error("Missing required parameter: symbol_name"),
                };
                let file_path = arguments.get("file_path").and_then(|v| v.as_str());

                let target_symbol = match get_symbol_by_name(&conn, symbol_name, file_path) {
                    Ok(Some(s)) => s,
                    Ok(None) => {
                        return CallToolResult::error(format!("Symbol '{symbol_name}' not found"));
                    }
                    Err(e) => return CallToolResult::error(e.to_string()),
                };

                // Tier 2: JIT Staleness Guard
                let _ = ensure_fresh_file(&self.workspace, &self.db_path, &conn, &target_symbol.path);

                let abs_file = self.workspace.canonical_root.join(&target_symbol.path);
                let target_body = match slice_symbol_body(&abs_file, &target_symbol) {
                    Ok(b) => b,
                    Err(e) => return CallToolResult::error(e.to_string()),
                };

                // Find callees
                let mut callee_signatures = Vec::new();
                if let Ok(callees) = code_kb_core::find_references(&conn, symbol_name, "callees", 10) {
                    for c in callees {
                        if let Ok(Some(s)) = get_symbol_by_name(&conn, &c.to_symbol_name, None) {
                            let sig = s.signature.unwrap_or(s.name);
                            callee_signatures.push(format!("{sig} ({}:{})", s.path, s.start_line));
                        }
                    }
                }

                // Find related types
                let mut related_types = Vec::new();
                if let Ok(types) = code_kb_core::find_type_facts(&conn, &target_symbol.symbol_id) {
                    for t in types {
                        related_types.push(t.resolved_type);
                    }
                }

                // Find tests related to this symbol
                let related_tests = match search_symbols(&conn, symbol_name, None, true, 5) {
                    Ok(tests) => tests.into_iter().filter(|s| s.is_test).collect(),
                    Err(_) => Vec::new(),
                };

                let slice = ContextSlice {
                    target_symbol,
                    target_body,
                    callee_signatures,
                    related_types,
                    related_tests,
                };

                CallToolResult::text(format_context_slice(&slice))
            }
            "find_references" => {
                let symbol_name = match arguments.get("symbol_name").and_then(|v| v.as_str()) {
                    Some(n) => n,
                    None => return CallToolResult::error("Missing required parameter: symbol_name"),
                };
                let direction = match arguments.get("direction").and_then(|v| v.as_str()) {
                    Some(d) => d,
                    None => return CallToolResult::error("Missing required parameter: direction"),
                };
                let limit = arguments
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(20) as usize;

                let refs = match code_kb_core::find_references(&conn, symbol_name, direction, limit) {
                    Ok(r) => r,
                    Err(e) => return CallToolResult::error(e.to_string()),
                };

                CallToolResult::text(format_references(symbol_name, &refs, direction))
            }
            "find_structural_facts" => {
                let category = match arguments.get("category").and_then(|v| v.as_str()) {
                    Some(c) => c,
                    None => return CallToolResult::error("Missing required parameter: category"),
                };
                let limit = arguments
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(30) as usize;

                let facts = match code_kb_core::find_structural_facts(&conn, category, limit) {
                    Ok(f) => f,
                    Err(e) => return CallToolResult::error(e.to_string()),
                };

                let literals = code_kb_core::find_literals(&conn, category, limit).unwrap_or_default();

                let mut out = format!("Structural facts for '{category}' ({} found):\n", facts.len());
                for f in &facts {
                    let parent = f.containing_symbol_name.as_deref().unwrap_or("top-level");
                    out.push_str(&format!(
                        "- {} [{}:{}] (pattern: {}, in: {})\n",
                        f.capture_name, f.path, f.start_line, f.pattern_id, parent
                    ));
                }

                if !literals.is_empty() {
                    out.push_str(&format!("\nMatching literals ({} found):\n", literals.len()));
                    for l in &literals {
                        out.push_str(&format!(
                            "- \"{}\" [{}:{}] (kind: {})\n",
                            l.literal_text, l.path, l.start_line, l.kind
                        ));
                    }
                }

                CallToolResult::text(out)
            }
            "replace_symbol_body" => {
                let symbol_name = match arguments.get("symbol_name").and_then(|v| v.as_str()) {
                    Some(n) => n,
                    None => return CallToolResult::error("Missing required parameter: symbol_name"),
                };
                let file_path = match arguments.get("file_path").and_then(|v| v.as_str()) {
                    Some(p) => p,
                    None => return CallToolResult::error("Missing required parameter: file_path"),
                };
                let new_body = match arguments.get("new_body").and_then(|v| v.as_str()) {
                    Some(b) => b,
                    None => return CallToolResult::error("Missing required parameter: new_body"),
                };
                let expected_hash = arguments.get("expected_body_hash").and_then(|v| v.as_str());

                match replace_symbol_body(
                    &self.workspace,
                    &self.db_path,
                    &conn,
                    symbol_name,
                    file_path,
                    new_body,
                    expected_hash,
                ) {
                    Ok(res) => CallToolResult::text(format!(
                        "Successfully replaced body of `{}` in `{}`.\nOld Hash: {}\nNew Hash: {}\nBytes Written: {}",
                        res.symbol_name, res.file_path, res.old_body_hash, res.new_body_hash, res.bytes_written
                    )),
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
                    let err_resp = JsonRpcResponse::error(None, -32700, format!("Parse error: {e}"));
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
                    if let Some(roots) = params.get("roots").and_then(|r| r.as_array()) {
                        if let Some(u) = roots.first().and_then(|r| r.get("uri")).and_then(|u| u.as_str()) {
                            candidate = Some(u);
                        }
                    } else if let Some(u) = params.get("rootUri").and_then(|u| u.as_str()) {
                        candidate = Some(u);
                    } else if let Some(u) = params.get("rootPath").and_then(|u| u.as_str()) {
                        candidate = Some(u);
                    } else if let Some(folders) = params.get("workspaceFolders").and_then(|f| f.as_array()) {
                        if let Some(u) = folders.first().and_then(|f| f.get("uri")).and_then(|u| u.as_str()) {
                            candidate = Some(u);
                        }
                    }

                    if let Some(cand) = candidate {
                        let path_str = if let Some(stripped) = cand.strip_prefix("file:///") {
                            stripped
                        } else if let Some(stripped) = cand.strip_prefix("file://") {
                            stripped
                        } else {
                            cand
                        };
                        let path = PathBuf::from(path_str);
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
                        "version": "0.1.0"
                    }
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
                Some(JsonRpcResponse::success(id, serde_json::to_value(result).unwrap_or(Value::Null)))
            }
            _ => Some(JsonRpcResponse::error(
                id,
                -32601,
                format!("Method not found: {}", request.method),
            )),
        }
    }
}
