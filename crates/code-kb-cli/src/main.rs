use clap::{Args, Parser, Subcommand};
use std::path::{Path, PathBuf};

use code_kb_core::{
    Workspace, codebase_outline_op, ensure_fresh_file, ensure_fts_index_path, file_skeleton_op,
    format_context_slice, format_references, format_search_results, fts_search_symbols_scoped,
    get_context_slice_op, get_symbol_body_op, list_structural_fact_categories, load_file_symbols,
    open_read_only, prune_orphaned_stores, queries, replace_symbol_body, scan_workspace,
    search_symbols_scoped,
};

mod logging;
mod mcp;

static DEFAULT_ROUTING_BLOCK: &str = include_str!("../../../hooks/code-kb-routing-block.md");

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
    /// Search for symbols by exact name or prefix.
    Symbol(SymbolArgs),
    /// Natural-language and full-text search over symbol names and docstrings using FTS5 (BM25).
    Search(SearchArgs),
    /// Retrieve exact implementation body of a symbol.
    Body(BodyArgs),
    /// Generate surgical context bundle (target body + callees + types + tests).
    Slice(SliceArgs),
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
    /// Run initial or full workspace scan.
    Scan(ScanArgs),
    /// Prune orphaned artifact databases for workspaces or worktrees that no longer exist on disk.
    Prune(PruneArgs),
    /// Stream server activity logs from background file watcher and reconciliation.
    Logs(LogsArgs),
    /// View tool usage telemetry and token efficiency summary.
    Stats(StatsArgs),
    /// Alias for stats.
    Telemetry(StatsArgs),
    /// Start Model Context Protocol (MCP) server on stdio.
    Serve(ServeArgs),
    /// Output agent lifecycle hook payload (SessionStart, SubagentStart).
    Hook(HookArgs),
}

#[derive(Debug, Args)]
pub struct OutlineArgs {
    /// Optional subpath to scope outline.
    pub path: Option<String>,
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
    #[arg(long)]
    pub path: Option<String>,
    /// Filter by symbol kind (e.g. function, struct, trait, class, interface, enum).
    #[arg(long)]
    pub kind: Option<String>,
    /// Include test functions.
    #[arg(long)]
    pub include_tests: bool,
    /// Maximum number of results.
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
}

#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Natural language keywords or concept to search for.
    pub query: String,
    /// Optional file path or directory prefix to scope search.
    #[arg(long)]
    pub path: Option<String>,
    /// Filter by symbol kind (e.g. function, struct, trait, class, interface, enum).
    #[arg(long)]
    pub kind: Option<String>,
    /// Include test functions and test containers.
    #[arg(long)]
    pub include_tests: bool,
    /// Maximum number of results.
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
}

#[derive(Debug, Args)]
pub struct BodyArgs {
    /// Full or qualified symbol name.
    pub symbol: String,
    /// Optional file path for disambiguation.
    #[arg(long)]
    pub file: Option<String>,
}

#[derive(Debug, Args)]
pub struct SliceArgs {
    /// Target symbol name.
    pub symbol: String,
    /// Optional file path for disambiguation.
    #[arg(long)]
    pub file: Option<String>,
}

#[derive(Debug, Args)]
pub struct RefsArgs {
    /// Target symbol name.
    pub symbol: String,
    /// Direction: "callers" or "callees" (default: "callers").
    #[arg(long, default_value = "callers")]
    pub direction: String,
    /// Maximum number of results.
    #[arg(long, default_value_t = 20)]
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
    #[arg(long, short = 'f')]
    pub file: Option<String>,
    /// Maximum relationship hops (default: 2).
    #[arg(long, short = 'd', default_value_t = 2)]
    pub depth: usize,
    /// Maximum results to return (default: 20).
    #[arg(long, short = 'l', default_value_t = 20)]
    pub limit: usize,
}

#[derive(Debug, Args)]
pub struct FactsArgs {
    /// Category or pattern (e.g. route, query, model, config). If omitted, lists available categories.
    #[arg(default_value = "")]
    pub category: String,
    /// Maximum number of results.
    #[arg(long, default_value_t = 30)]
    pub limit: usize,
}

#[derive(Debug, Args)]
pub struct EditArgs {
    /// Name of symbol to edit.
    pub symbol: String,
    /// Path to file containing symbol.
    #[arg(long)]
    pub file: String,
    /// New body content.
    #[arg(long)]
    pub body: String,
    /// Optional optimistic lock hash of existing body.
    #[arg(long)]
    pub expected_hash: Option<String>,
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
pub struct PruneArgs {
    /// Preview stores that would be deleted without actually removing them.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct StatsArgs {
    /// Format output as raw JSON instead of human-readable table.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct HookArgs {
    /// Hook event name (default: "SessionStart", or "SubagentStart").
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
        let summary = code_kb_core::get_telemetry_summary(&workspace.root)
            .map_err(|e| anyhow::anyhow!("Failed to query telemetry: {e}"))?;
        if cli.json || args.json {
            println!("{}", serde_json::to_string_pretty(&summary)?);
        } else {
            print!("{}", code_kb_core::format_telemetry_summary(&summary));
        }
        return Ok(());
    }

    // Handle Prune command (does not require existing database in current workspace)
    if let Command::Prune(args) = &cli.command {
        let pruned = prune_orphaned_stores(args.dry_run);
        if cli.json {
            println!("{}", serde_json::to_string_pretty(&pruned)?);
        } else if pruned.is_empty() {
            println!("No orphaned stores found.");
        } else {
            let action = if args.dry_run {
                "Would prune"
            } else {
                "Pruned"
            };
            println!("{action} {} orphaned store(s):", pruned.len());
            for p in pruned {
                println!("- {}", p.display());
            }
        }
        return Ok(());
    }

    // Handle Logs command (does not require existing database)
    if let Command::Logs(args) = &cli.command {
        let log_dir = logging::get_log_dir(&workspace.canonical_root);
        println!("Log directory: {}", log_dir.display());

        let mut log_files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&log_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file()
                    && let Ok(meta) = entry.metadata()
                {
                    let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                    log_files.push((path, mtime));
                }
            }
        }

        log_files.sort_by_key(|a| std::cmp::Reverse(a.1));

        if let Some((latest_file, _)) = log_files.first() {
            println!("Latest log file: {}\n", latest_file.display());
            if let Ok(content) = std::fs::read_to_string(latest_file) {
                let all_lines: Vec<&str> = content.lines().collect();
                let start = all_lines.len().saturating_sub(args.lines);
                for line in &all_lines[start..] {
                    println!("{line}");
                }
            }
        } else {
            println!("No log files found yet in {}", log_dir.display());
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

    if !db_path.exists() {
        eprintln!(
            "Error: Database artifact not found at '{}'. Run `code-kb scan` first.",
            db_path.display()
        );
        std::process::exit(1);
    }

    let conn = open_read_only(&db_path)?;

    match cli.command {
        Command::Outline(args) => {
            if cli.json {
                let files = queries::load_scoped_files(&conn, args.path.as_deref())?;
                println!("{}", serde_json::to_string_pretty(&files)?);
            } else {
                let text =
                    codebase_outline_op(&workspace, &conn, args.depth, args.path.as_deref())?;
                println!("{text}");
            }
        }
        Command::Skeleton(args) => {
            if cli.json {
                let (_, rel_path) = workspace.resolve_path(Path::new(&args.file))?;
                let _ = ensure_fresh_file(&workspace, &db_path, &conn, &rel_path);
                let symbols = load_file_symbols(&conn, &rel_path)?;
                println!("{}", serde_json::to_string_pretty(&symbols)?);
            } else {
                let skeleton = file_skeleton_op(&workspace, &db_path, &conn, &args.file)?;
                println!("{skeleton}");
            }
        }
        Command::Symbol(args) => {
            let matches = search_symbols_scoped(
                &conn,
                &args.query,
                args.kind.as_deref(),
                args.path.as_deref(),
                args.include_tests,
                args.limit,
            )?;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&matches)?);
            } else {
                println!(
                    "Found {} symbols matching \"{}\":\n",
                    matches.len(),
                    args.query
                );
                for s in matches {
                    let sig = s.signature.as_deref().unwrap_or(&s.name);
                    println!(
                        "- {} `{}` [{}:{}-{}]",
                        s.kind, s.name, s.path, s.start_line, s.end_line
                    );
                    println!("  Signature: {sig}");
                    if let Some(doc) = s.doc_comment {
                        let first = doc.lines().next().unwrap_or("").trim();
                        println!("  Doc: {first}");
                    }
                }
            }
        }
        Command::Search(args) => {
            let _ = ensure_fts_index_path(&db_path);
            let matches = fts_search_symbols_scoped(
                &conn,
                &args.query,
                args.kind.as_deref(),
                args.path.as_deref(),
                args.include_tests,
                args.limit,
            )?;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&matches)?);
            } else {
                println!("{}", format_search_results(&args.query, &matches));
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
                println!("{}", serde_json::json!({ "symbol": symbol, "body": body }));
            } else {
                println!(
                    "// {}:{}-{} ({})\n{body}",
                    symbol.path, symbol.start_line, symbol.end_line, symbol.name
                );
            }
        }
        Command::Slice(args) => {
            let slice = get_context_slice_op(
                &workspace,
                &db_path,
                &conn,
                &args.symbol,
                args.file.as_deref(),
            )?;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&slice)?);
            } else {
                println!("{}", format_context_slice(&slice));
            }
        }
        Command::Refs(args) => {
            let refs = code_kb_core::find_references_ext(
                &conn,
                &args.symbol,
                &args.direction,
                args.limit,
                args.include_external,
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&refs)?);
            } else {
                println!(
                    "{}",
                    format_references(&args.symbol, &refs, &args.direction)
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
            let cat = args.category.trim();
            if cat.is_empty() {
                let categories = list_structural_fact_categories(&conn)?;
                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&categories)?);
                } else if categories.is_empty() {
                    println!("No structural facts or literals indexed in this repository.");
                } else {
                    println!(
                        "Available structural fact & literal categories ({} found):\n",
                        categories.len()
                    );
                    for (name, count) in categories {
                        println!("- `{name}` ({count} occurrences)");
                    }
                    println!("\nRun `code-kb facts <category>` to view matching facts.");
                }
            } else {
                let facts = code_kb_core::find_structural_facts(&conn, cat, args.limit)?;
                let literals = code_kb_core::find_literals(&conn, cat, args.limit)?;
                if cli.json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "structural_facts": facts,
                            "literals": literals
                        }))?
                    );
                } else {
                    println!("Structural facts for '{}' ({} found):\n", cat, facts.len());
                    for f in &facts {
                        let parent = f.containing_symbol_name.as_deref().unwrap_or("top-level");
                        println!(
                            "- {} [{}:{}] (pattern: {}, in: {})",
                            f.capture_name, f.path, f.start_line, f.pattern_id, parent
                        );
                    }
                    if !literals.is_empty() {
                        println!("\nMatching literals ({} found):\n", literals.len());
                        for l in &literals {
                            println!(
                                "- \"{}\" [{}:{}] (kind: {})",
                                l.literal_text, l.path, l.start_line, l.kind
                            );
                        }
                    }
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

            println!(
                "Successfully replaced body of `{}` in `{}`.\nOld Hash: {}\nNew Hash: {}\nBytes Written: {}",
                res.symbol_name,
                res.file_path,
                res.old_body_hash,
                res.new_body_hash,
                res.bytes_written
            );
        }
        Command::Serve(_)
        | Command::Scan(_)
        | Command::Logs(_)
        | Command::Prune(_)
        | Command::Stats(_)
        | Command::Telemetry(_)
        | Command::Hook(_) => {
            unreachable!()
        }
    }

    Ok(())
}
