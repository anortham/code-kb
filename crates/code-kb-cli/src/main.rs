use clap::{Args, Parser, Subcommand};
use std::path::{Path, PathBuf};

use code_kb_core::{
    Workspace, codebase_outline_op, ensure_fresh_file, ensure_fts_index_path, file_skeleton_op,
    format_context_slice, format_edit_file_result, format_fact_categories,
    format_find_symbol_results, format_references, format_replace_symbol_result,
    format_search_results, format_structural_facts, format_symbol_body,
    fts_search_symbols_explained, fts_search_symbols_scoped, get_context_slice_op,
    get_symbol_body_op, list_structural_fact_categories_scoped, load_file_symbols, open_read_only,
    queries, replace_symbol_body, scan_workspace, search_symbols_scoped,
};

mod logging;
mod mcp;

static DEFAULT_ROUTING_BLOCK: &str = include_str!("routing-block.md");

fn parse_result_limit(raw: &str) -> Result<usize, String> {
    let limit = raw
        .parse::<usize>()
        .map_err(|_| "limit must be a non-negative integer".to_string())?;
    code_kb_core::queries::validate_result_limit(limit).map_err(|error| error.to_string())?;
    Ok(limit)
}

#[derive(Debug, Parser)]
#[command(
    name = "code-kb",
    about = "Agent-facing code-intelligence engine & MCP server",
    version
)]
pub struct Cli {
    /// Target repository path (defaults to current directory).
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,

    /// Path to explicit SQLite artifact database.
    #[arg(long, global = true)]
    pub db: Option<PathBuf>,

    /// Output results as JSON.
    #[arg(long, global = true)]
    pub json: bool,

    /// Enable verbose debug logging.
    #[arg(long, short = 'v', global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Print hierarchical outline of codebase directory tree.
    Outline(OutlineArgs),
    /// Render file skeleton containing symbol signatures with implementation bodies stripped.
    Skeleton(SkeletonArgs),
    /// Search for symbols by exact identifier name or prefix.
    #[command(alias = "symbol")]
    Lookup(LookupArgs),
    /// Natural-language and identifier search over symbol names, signatures, and docstrings; substrings inside identifiers are found (`sha256` finds `parseSha256Sidecar`).
    Search(SearchArgs),
    /// Retrieve exact implementation body of a symbol.
    Body(BodyArgs),
    /// Generate surgical symbol context bundle (target body + callees + types + tests).
    #[command(alias = "slice")]
    Context(ContextArgs),
    /// Find callers or callees of a symbol.
    Refs(RefsArgs),
    /// Predict downstream impact and which tests to run before/after edits.
    BlastRadius(BlastRadiusArgs),
    /// Alias for blast-radius.
    Impact(BlastRadiusArgs),
    /// Query framework-level structural facts (routes, tables, models, config keys).
    Facts(FactsArgs),
    /// Atomically replace the body of a symbol.
    Edit(EditArgs),
    /// Replace text in one file without reading it first.
    EditFile(EditFileArgs),
    /// Run initial or full workspace scan.
    Scan(ScanArgs),
    /// Stream server activity logs from background file watcher and reconciliation.
    Logs(LogsArgs),
    /// View tool usage telemetry and token efficiency summary.
    Stats(StatsArgs),
    /// Alias for stats.
    Telemetry(StatsArgs),
    /// Generate diagnostic bundle and pre-filled GitHub bug report URL.
    BugReport(BugReportArgs),
    /// Start Model Context Protocol (MCP) server on stdio.
    Serve(ServeArgs),
    /// Output agent lifecycle hook payload (SessionStart, SubagentStart).
    Hook(HookArgs),
}

#[derive(Debug, Args)]
pub struct OutlineArgs {
    /// Optional subpath to scope outline.
    #[arg(long, short = 'p')]
    pub path: Option<String>,
    /// Positional path to scope outline if --path is not provided.
    pub positional_path: Option<String>,
    /// Maximum directory recursion depth (default: 2).
    #[arg(long, default_value_t = 2)]
    pub depth: usize,
}

#[derive(Debug, Args)]
pub struct SkeletonArgs {
    /// Relative or absolute path to source file.
    pub file: String,
}

#[derive(Debug, Args)]
pub struct SymbolArgs {
    /// Symbol name or prefix query.
    pub query: String,
    /// Optional file path or directory prefix to scope search.
    #[arg(long, alias = "file", alias = "file-path")]
    pub path: Option<String>,
    /// Filter by symbol kind (e.g. function, struct, trait, class, interface, enum).
    #[arg(long)]
    pub kind: Option<String>,
    /// Include test functions, test containers, and rows from test files.
    #[arg(long, alias = "is-test")]
    pub include_tests: bool,
    /// Maximum number of results (0-200).
    #[arg(long, default_value_t = 20, value_parser = parse_result_limit)]
    pub limit: usize,
}

pub type LookupArgs = SymbolArgs;

#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Concept, keyword, or identifier query. Matches whole names, name words, name substrings (`sha256` finds `parseSha256Sidecar`), signatures, and docstrings; results carry the rerank score.
    pub query: String,
    /// Optional file path or directory prefix to scope search.
    #[arg(long, alias = "file", alias = "file-path")]
    pub path: Option<String>,
    /// Filter by symbol kind (e.g. function, struct, trait, class, interface, enum).
    #[arg(long)]
    pub kind: Option<String>,
    /// Include test functions, test containers, and rows from test files.
    #[arg(long, alias = "is-test")]
    pub include_tests: bool,
    /// Maximum number of results (0-200).
    #[arg(long, default_value_t = 20, value_parser = parse_result_limit)]
    pub limit: usize,
    /// Print the rerank breakdown under each result (JSON: an `explain` object per result).
    #[arg(long)]
    pub explain: bool,
}

#[derive(Debug, Args)]
pub struct BodyArgs {
    /// Full or qualified symbol name.
    pub symbol: String,
    /// Optional file path for disambiguation.
    #[arg(short = 'f', long, alias = "path", alias = "file-path")]
    pub file: Option<String>,
}

#[derive(Debug, Args)]
pub struct SliceArgs {
    /// Target symbol name.
    pub symbol: String,
    /// Optional file path for disambiguation.
    #[arg(short = 'f', long, alias = "path", alias = "file-path")]
    pub file: Option<String>,
    /// Include external stdlib/runtime calls in callee signatures (default: false).
    #[arg(long, default_value_t = false)]
    pub include_external: bool,
}

pub type ContextArgs = SliceArgs;

#[derive(Debug, Args)]
pub struct RefsArgs {
    /// Target symbol name.
    pub symbol: String,
    /// Optional file path for disambiguation.
    #[arg(short = 'f', long, alias = "path", alias = "file-path")]
    pub file: Option<String>,
    /// Direction: "callers" or "callees" (default: "callers").
    #[arg(long, default_value = "callers", value_parser = ["callers", "callees"])]
    pub direction: String,
    /// Maximum number of results (0-200).
    #[arg(long, default_value_t = 20, value_parser = parse_result_limit)]
    pub limit: usize,
    /// Include unresolved external runtime/stdlib primitives in callees.
    #[arg(long)]
    pub include_external: bool,
}

#[derive(Debug, Args)]
pub struct BlastRadiusArgs {
    /// Optional symbol name to seed blast radius walk.
    pub symbol: Option<String>,
    /// Optional file path to seed blast radius walk.
    #[arg(long, short = 'f', alias = "path", alias = "file-path")]
    pub file: Option<String>,
    /// Maximum relationship hops (default: 2).
    #[arg(long, short = 'd', default_value_t = 2)]
    pub depth: usize,
    /// Maximum results to return, 0-200 (default: 20).
    #[arg(long, short = 'l', default_value_t = 20, value_parser = parse_result_limit)]
    pub limit: usize,
}

#[derive(Debug, Args)]
pub struct FactsArgs {
    /// Category or pattern (e.g. route, query, model, config). If omitted, lists available categories.
    #[arg(long, short = 'c')]
    pub category: Option<String>,
    /// Positional category if --category is not provided.
    #[arg(default_value = "")]
    pub positional_category: String,
    /// Optional file path or directory to filter structural facts.
    #[arg(short = 'p', long = "path", alias = "file", alias = "file-path")]
    pub path: Option<String>,
    /// Maximum combined facts and literals (0-200).
    #[arg(long, default_value_t = 30, value_parser = parse_result_limit)]
    pub limit: usize,
}

#[derive(Debug, Args)]
pub struct EditArgs {
    /// Name of symbol to edit.
    pub symbol: String,
    /// Path to file containing symbol.
    #[arg(long, alias = "path", alias = "file-path")]
    pub file: String,
    /// New body content.
    #[arg(long, alias = "code", alias = "new-body")]
    pub body: String,
    /// Optional optimistic lock hash of existing body.
    #[arg(long, alias = "expected-body-hash", alias = "body-hash")]
    pub expected_hash: Option<String>,
}

#[derive(Debug, Args)]
pub struct EditFileArgs {
    /// Path to the file to edit.
    pub file: String,
    /// Text to find in the file.
    #[arg(long, alias = "find", alias = "old-text")]
    pub old: String,
    /// Text that replaces the found text.
    #[arg(long, alias = "replace", alias = "new-text")]
    pub new: String,
    /// Which match to replace. "only" refuses two or more matches.
    #[arg(long, default_value = "only", value_parser = ["only", "first", "last", "all"])]
    pub occurrence: String,
}

#[derive(Debug, Args)]
pub struct ScanArgs {
    /// Force full re-extraction.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct ServeArgs {
    /// Workspace root directory.
    #[arg(long)]
    pub root: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct LogsArgs {
    /// Number of recent log lines to display.
    #[arg(long, default_value_t = 50)]
    pub lines: usize,
}

#[derive(Debug, Args)]
pub struct StatsArgs {
    /// Time window filter (today, 7d, 30d, month, year, all).
    #[arg(short = 's', long, default_value = "all")]
    pub since: String,

    /// Scope telemetry to current workspace (default: global aggregate).
    #[arg(short = 'w', long)]
    pub workspace: bool,

    /// Filter metrics by code-kb version (e.g. '1.1.3'), 'current' (default), or 'all'.
    #[arg(long)]
    pub version: Option<String>,

    /// Include telemetry across all historical versions (default: false).
    #[arg(long)]
    pub all_versions: bool,

    /// Format output as raw JSON instead of human-readable table.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct BugReportArgs {
    /// Optional issue title.
    #[arg(short = 't', long)]
    pub title: Option<String>,

    /// Description of the problem, inserted into the report body.
    #[arg(short = 'd', long)]
    pub description: Option<String>,

    /// Number of recent log lines to include (0 disables).
    #[arg(long, default_value_t = 40)]
    pub logs: usize,

    /// Format output as raw JSON diagnostic bundle.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct HookArgs {
    /// Hook event name (default: "SessionStart", or "SubagentStart", "PreInvocation").
    #[arg(default_value = "SessionStart")]
    pub event: String,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Handle Hook command immediately without requiring workspace discovery or database
    if let Command::Hook(args) = &cli.command {
        let is_copilot = std::env::var("COPILOT_PLUGIN_DATA").is_ok();
        let event = args.event.as_str();

        let content = if let Ok(custom) = std::fs::read_to_string("hooks/code-kb-routing-block.md")
        {
            custom
        } else if let Ok(custom) = std::fs::read_to_string(".code-kb/routing.md") {
            custom
        } else {
            DEFAULT_ROUTING_BLOCK.to_string()
        };

        let trimmed = content.trim();

        let output = if is_copilot {
            if event == "SessionStart" {
                serde_json::json!({ "additionalContext": trimmed })
            } else {
                serde_json::json!({})
            }
        } else if event.eq_ignore_ascii_case("PreInvocation") {
            serde_json::json!({
                "injectSteps": [
                    {
                        "ephemeralMessage": trimmed
                    }
                ]
            })
        } else {
            serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": event,
                    "additionalContext": trimmed,
                }
            })
        };

        println!("{}", serde_json::to_string(&output)?);
        return Ok(());
    }

    // Discover workspace
    let ws_root = cli.root.as_deref();
    let workspace = Workspace::discover(ws_root)?;

    // Initialize structured logging to .code-kb/logs/code-kb.log
    let is_serve = matches!(&cli.command, Command::Serve(_));
    let _log_guard = logging::init_logging(&workspace.canonical_root, is_serve, cli.verbose);
    tracing::info!(
        root = %workspace.canonical_root.display(),
        command = ?std::env::args().collect::<Vec<_>>(),
        "code-kb started"
    );

    // Handle Stats / Telemetry command (does not require artifact.db)
    if let Command::Stats(args) | Command::Telemetry(args) = &cli.command {
        let conn = code_kb_core::open_global_telemetry_db()
            .map_err(|e| anyhow::anyhow!("Failed to open telemetry database: {e}"))?;
        let time_window = code_kb_core::TimeWindow::parse(&args.since).ok_or_else(|| {
            anyhow::anyhow!(
                "Invalid time window '{}'. Supported values: today, 7d, 30d, month, year, all",
                args.since
            )
        })?;
        let workspace_root = if args.workspace {
            Some(workspace.canonical_root.clone())
        } else {
            None
        };
        let version = if args.all_versions || args.version.as_deref() == Some("all") {
            None
        } else if let Some(ref v) = args.version {
            if v == "current" {
                Some(env!("CARGO_PKG_VERSION").to_string())
            } else {
                Some(v.clone())
            }
        } else {
            Some(env!("CARGO_PKG_VERSION").to_string())
        };

        let filter = code_kb_core::TelemetryFilter {
            time_window,
            workspace_root,
            version: version.clone(),
        };
        let mut summary = code_kb_core::get_telemetry_summary(&conn, &filter)
            .map_err(|e| anyhow::anyhow!("Failed to query telemetry: {e}"))?;

        // When global stats are requested, scope recent_errors to the current workspace
        // to prevent leaking private paths or error text from unrelated repositories
        if !args.workspace {
            let ws_filter = code_kb_core::TelemetryFilter {
                time_window,
                workspace_root: Some(workspace.canonical_root.clone()),
                version,
            };
            if let Ok(ws_summary) = code_kb_core::get_telemetry_summary(&conn, &ws_filter) {
                summary.recent_errors = ws_summary.recent_errors;
            } else {
                summary.recent_errors.clear();
            }
        }

        if cli.json || args.json {
            println!("{}", serde_json::to_string_pretty(&summary)?);
        } else {
            print!("{}", code_kb_core::format_telemetry_summary(&summary));
        }
        return Ok(());
    }

    // Handle BugReport command (does not require artifact.db)
    if let Command::BugReport(args) = &cli.command {
        let conn = code_kb_core::open_global_telemetry_db()
            .map_err(|e| anyhow::anyhow!("Failed to open telemetry database: {e}"))?;
        let bundle = code_kb_core::generate_bug_report(
            &conn,
            Some(&workspace.canonical_root),
            args.title.as_deref(),
            args.description.as_deref(),
            args.logs,
        )
        .map_err(|e| anyhow::anyhow!("Failed to generate bug report: {e}"))?;
        if cli.json || args.json {
            println!("{}", serde_json::to_string_pretty(&bundle)?);
        } else {
            println!("{}", bundle.markdown_body.trim_end());
            println!("\nGitHub Issue URL:\n{}", bundle.github_issue_url);
        }
        return Ok(());
    }

    // Handle Logs command (does not require existing database)
    if let Command::Logs(args) = &cli.command {
        let log_dir = logging::get_log_dir(&workspace.canonical_root);
        println!("Log directory: {}", log_dir.display());

        match code_kb_core::workspace::latest_log_file(&workspace.canonical_root) {
            Some(latest_file) => {
                println!("Latest log file: {}\n", latest_file.display());
                if let Ok(content) = std::fs::read_to_string(&latest_file) {
                    let all_lines: Vec<&str> = content.lines().collect();
                    let start = all_lines.len().saturating_sub(args.lines);
                    for line in &all_lines[start..] {
                        println!("{line}");
                    }
                }
            }
            None => println!("No log files found yet in {}", log_dir.display()),
        }
        return Ok(());
    }

    // Handle Serve command
    if let Command::Serve(args) = &cli.command {
        let root = args.root.as_deref().or(ws_root);
        let ws = Workspace::discover(root)?;
        let mut server = mcp::McpServer::new(ws, cli.db.as_deref())?;
        return server.run_stdio();
    }

    // Locate database
    let db_path = workspace.locate_db(cli.db.as_deref())?;

    // Handle Scan command
    if let Command::Scan(args) = &cli.command {
        println!(
            "Scanning workspace at '{}'...",
            workspace.canonical_root.display()
        );
        scan_workspace(&workspace, &db_path, args.force)?;
        println!("Database updated at '{}'.", db_path.display());
        return Ok(());
    }

    code_kb_core::ensure_index_matches_extractor(
        &workspace,
        &db_path,
        &code_kb_core::installed_extractor_version(),
    )?;

    if !db_path.exists() && code_kb_core::is_project_root(&workspace.canonical_root) {
        eprintln!(
            "Index not found; scanning '{}' first.",
            workspace.canonical_root.display()
        );
        code_kb_core::create_index(&workspace, &db_path)?;
    }
    if !db_path.exists() {
        eprintln!(
            "Error: Database artifact not found at '{}'. Run `code-kb scan` first.",
            db_path.display()
        );
        std::process::exit(1);
    }

    ensure_fts_index_path(&db_path)?;
    let conn = open_read_only(&db_path)?;
    code_kb_core::reconcile_offline_edits(&workspace, &db_path, &conn)?;

    match cli.command {
        Command::Outline(args) => {
            let target_path = args.path.or(args.positional_path);
            let rel_path = target_path
                .as_deref()
                .map(|p| workspace.relativize_filter(p));
            let path_filter = rel_path.as_deref();
            if cli.json {
                let files = queries::load_scoped_files(&conn, path_filter)?;
                println!("{}", serde_json::to_string_pretty(&files)?);
            } else {
                let text = codebase_outline_op(&workspace, &conn, args.depth, path_filter)?;
                println!("{text}");
            }
        }
        Command::Skeleton(args) => {
            if cli.json {
                let (_, rel_path) = workspace.resolve_path(Path::new(&args.file))?;
                ensure_fresh_file(&workspace, &db_path, &conn, &rel_path)?;
                let symbols = load_file_symbols(&conn, &rel_path)?;
                println!("{}", serde_json::to_string_pretty(&symbols)?);
            } else {
                let skeleton = file_skeleton_op(&workspace, &db_path, &conn, &args.file)?;
                println!("{skeleton}");
            }
        }
        Command::Lookup(args) => {
            let rel_path = args.path.as_deref().map(|p| workspace.relativize_filter(p));
            let path_filter = rel_path.as_deref();

            let matches = search_symbols_scoped(
                &conn,
                &args.query,
                args.kind.as_deref(),
                path_filter,
                args.include_tests,
                args.limit,
            )?;

            let (exact_matches, fts_matches) = if matches.is_empty() {
                ensure_fts_index_path(&db_path)?;
                let fts = fts_search_symbols_scoped(
                    &conn,
                    &args.query,
                    args.kind.as_deref(),
                    path_filter,
                    args.include_tests,
                    args.limit,
                )?;
                (Vec::new(), fts)
            } else {
                (matches, Vec::new())
            };

            if cli.json {
                if !exact_matches.is_empty() {
                    println!("{}", serde_json::to_string_pretty(&exact_matches)?);
                } else {
                    println!("{}", serde_json::to_string_pretty(&fts_matches)?);
                }
            } else {
                print!(
                    "{}",
                    format_find_symbol_results(
                        &args.query,
                        &exact_matches,
                        &fts_matches,
                        args.limit
                    )
                );
            }
        }
        Command::Search(args) => {
            let rel_path = args.path.as_deref().map(|p| workspace.relativize_filter(p));
            let path_filter = rel_path.as_deref();
            ensure_fts_index_path(&db_path)?;
            let matches = fts_search_symbols_explained(
                &conn,
                &args.query,
                args.kind.as_deref(),
                path_filter,
                args.include_tests,
                args.limit,
                args.explain,
            )?;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&matches)?);
            } else {
                println!(
                    "{}",
                    format_search_results(&args.query, &matches, args.limit)
                );
            }
        }
        Command::Body(args) => {
            let (symbol, body) = get_symbol_body_op(
                &workspace,
                &db_path,
                &conn,
                &args.symbol,
                args.file.as_deref(),
            )?;

            if cli.json {
                let body_hash = code_kb_core::edit::hash_content(&body);
                println!(
                    "{}",
                    serde_json::json!({ "symbol": symbol, "body": body, "body_hash": body_hash })
                );
            } else {
                print!("{}", format_symbol_body(&symbol, &body));
            }
        }
        Command::Context(args) => {
            let slice = get_context_slice_op(
                &workspace,
                &db_path,
                &conn,
                &args.symbol,
                args.file.as_deref(),
                args.include_external,
            )?;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&slice)?);
            } else {
                println!("{}", format_context_slice(&slice));
            }
        }
        Command::Refs(args) => {
            let rel_file = args.file.as_deref().map(|p| workspace.relativize_filter(p));
            let refs = code_kb_core::find_references_scoped(
                &conn,
                &args.symbol,
                &args.direction,
                args.limit,
                args.include_external,
                rel_file.as_deref(),
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&refs)?);
            } else {
                println!(
                    "{}",
                    format_references(&args.symbol, &refs, &args.direction, args.limit)
                );
            }
        }
        Command::BlastRadius(args) | Command::Impact(args) => {
            let result = code_kb_core::blast_radius_op(
                &workspace,
                &conn,
                args.symbol.as_deref(),
                args.file.as_deref(),
                args.depth,
                args.limit,
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!("{}", code_kb_core::format_blast_radius(&result));
            }
        }
        Command::Facts(args) => {
            let rel_path = args.path.as_deref().map(|p| workspace.relativize_filter(p));
            let raw_cat = args.category.unwrap_or(args.positional_category);
            let cat = raw_cat.trim();
            if cat.is_empty() {
                let categories =
                    list_structural_fact_categories_scoped(&conn, rel_path.as_deref())?;
                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&categories)?);
                } else {
                    println!("{}", format_fact_categories(&categories));
                    if !categories.is_empty() {
                        println!("\nRun `code-kb facts <category>` to view matching facts.");
                    }
                }
            } else {
                let facts = code_kb_core::find_structural_facts_scoped(
                    &conn,
                    cat,
                    rel_path.as_deref(),
                    args.limit,
                )?;
                let literals = code_kb_core::find_literals_scoped(
                    &conn,
                    cat,
                    rel_path.as_deref(),
                    args.limit.saturating_sub(facts.len()),
                )?;
                if cli.json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "structural_facts": facts,
                            "literals": literals
                        }))?
                    );
                } else {
                    print!("{}", format_structural_facts(&facts, &literals, cat));
                }
            }
        }
        Command::Edit(args) => {
            let res = replace_symbol_body(
                &workspace,
                &db_path,
                &conn,
                &args.symbol,
                &args.file,
                &args.body,
                args.expected_hash.as_deref(),
            )?;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&res)?);
            } else {
                println!("{}", format_replace_symbol_result(&res));
            }
        }
        Command::EditFile(args) => {
            let occurrence = serde_json::from_value(serde_json::Value::String(args.occurrence))?;
            let res = code_kb_core::edit_file(
                &workspace, &db_path, &conn, &args.file, &args.old, &args.new, occurrence,
            )?;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&res)?);
            } else {
                println!("{}", format_edit_file_result(&res));
            }
        }
        Command::Serve(_)
        | Command::Scan(_)
        | Command::Logs(_)
        | Command::Stats(_)
        | Command::Telemetry(_)
        | Command::Hook(_)
        | Command::BugReport(_) => {
            unreachable!()
        }
    }

    Ok(())
}
