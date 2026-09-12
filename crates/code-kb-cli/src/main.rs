use std::collections::HashMap;
use std::path::{Path, PathBuf};
use clap::{Args, Parser, Subcommand};

use code_kb_core::{
    ensure_fresh_file, format_codebase_outline, format_context_slice,
    format_file_skeleton, format_references, get_file, get_symbol_by_name,
    load_file_symbols, load_files, open_read_only, replace_symbol_body,
    scan_workspace, search_symbols, slice_symbol_body, ContextSlice,
    Workspace,
};

mod logging;
mod mcp;

#[derive(Debug, Parser)]
#[command(
    name = "code-kb",
    version,
    about = "Lightweight, token-dense code-intelligence engine and MCP server for AI coding agents"
)]
pub struct Cli {
    /// Workspace root directory. Defaults to CWD or upward Git root discovery.
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,

    /// Path to SQLite extraction database artifact.
    #[arg(long, global = true)]
    pub db: Option<PathBuf>,

    /// Format output as JSON.
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
    /// Output compact codebase architectural outline.
    Outline(OutlineArgs),
    /// Progressive disclosure file skeleton with implementation bodies stripped.
    Skeleton(SkeletonArgs),
    /// Search symbols by name, kind, or test flag.
    Symbol(SymbolArgs),
    /// Retrieve exact implementation body of a symbol.
    Body(BodyArgs),
    /// Surgical context bundle: target body + callee signatures + types + tests.
    Slice(SliceArgs),
    /// Discover callers or callees of a symbol.
    Refs(RefsArgs),
    /// Query framework-level structural facts and literals.
    Facts(FactsArgs),
    /// Atomically replace symbol body with pre-flight check and immediate re-index.
    Edit(EditArgs),
    /// Scan and extract workspace AST facts into SQLite catalog.
    Scan(ScanArgs),
    /// Start Model Context Protocol (MCP) server over stdio.
    Serve(ServeArgs),
    /// View active log file location and recent diagnostic entries.
    Logs(LogsArgs),
}

#[derive(Debug, Args)]
pub struct OutlineArgs {
    /// Subdirectory to scope the outline to.
    pub path: Option<String>,
    /// Directory recursion depth.
    #[arg(long, default_value_t = 2)]
    pub depth: usize,
}

#[derive(Debug, Args)]
pub struct SkeletonArgs {
    /// File path relative to workspace root or absolute path.
    pub file: String,
}

#[derive(Debug, Args)]
pub struct SymbolArgs {
    /// Symbol name or search query.
    pub query: String,
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
    /// Direction: "callers" or "callees".
    #[arg(long)]
    pub direction: String,
    /// Maximum number of results.
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
}

#[derive(Debug, Args)]
pub struct FactsArgs {
    /// Category or pattern (e.g. route, query, model, config).
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

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

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

    // Handle Logs command (does not require existing database)
    if let Command::Logs(args) = &cli.command {
        let log_dir = logging::get_log_dir(&workspace.canonical_root);
        println!("Log directory: {}", log_dir.display());

        let mut log_files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&log_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Ok(meta) = entry.metadata() {
                        let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                        log_files.push((path, mtime));
                    }
                }
            }
        }

        log_files.sort_by(|a, b| b.1.cmp(&a.1));

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
        println!("Scanning workspace at '{}'...", workspace.canonical_root.display());
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
            let files = load_files(&conn)?;
            let mut symbols_by_file = HashMap::new();
            for f in &files {
                if let Ok(syms) = load_file_symbols(&conn, &f.path) {
                    symbols_by_file.insert(f.path.clone(), syms);
                }
            }

            let text = format_codebase_outline(
                &workspace.repo_name,
                &files,
                &symbols_by_file,
                args.depth,
            );
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&files)?);
            } else {
                println!("{text}");
            }
        }
        Command::Skeleton(args) => {
            let (_, rel_path) = workspace.resolve_path(Path::new(&args.file))?;
            let _ = ensure_fresh_file(&workspace, &db_path, &conn, &rel_path);

            let symbols = load_file_symbols(&conn, &rel_path)?;
            let file_meta = get_file(&conn, &rel_path).ok().flatten();
            let line_count = file_meta.and_then(|m| m.line_count.map(|l| l as usize));

            let skeleton = format_file_skeleton(&rel_path, &symbols, line_count);
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&symbols)?);
            } else {
                println!("{skeleton}");
            }
        }
        Command::Symbol(args) => {
            let matches = search_symbols(
                &conn,
                &args.query,
                args.kind.as_deref(),
                args.include_tests,
                args.limit,
            )?;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&matches)?);
            } else {
                println!("Found {} symbols matching \"{}\":\n", matches.len(), args.query);
                for s in matches {
                    let sig = s.signature.as_deref().unwrap_or(&s.name);
                    println!("- {} `{}` [{}:{}-{}]", s.kind, s.name, s.path, s.start_line, s.end_line);
                    println!("  Signature: {sig}");
                    if let Some(doc) = s.doc_comment {
                        let first = doc.lines().next().unwrap_or("").trim();
                        println!("  Doc: {first}");
                    }
                }
            }
        }
        Command::Body(args) => {
            let symbol = get_symbol_by_name(&conn, &args.symbol, args.file.as_deref())?
                .ok_or_else(|| anyhow::anyhow!("Symbol '{}' not found", args.symbol))?;

            let _ = ensure_fresh_file(&workspace, &db_path, &conn, &symbol.path);
            let abs_file = workspace.canonical_root.join(&symbol.path);
            let body = slice_symbol_body(&abs_file, &symbol)?;

            if cli.json {
                println!("{}", serde_json::json!({ "symbol": symbol, "body": body }));
            } else {
                println!("// {}:{}-{} ({})\n{body}", symbol.path, symbol.start_line, symbol.end_line, symbol.name);
            }
        }
        Command::Slice(args) => {
            let target_symbol = get_symbol_by_name(&conn, &args.symbol, args.file.as_deref())?
                .ok_or_else(|| anyhow::anyhow!("Symbol '{}' not found", args.symbol))?;

            let _ = ensure_fresh_file(&workspace, &db_path, &conn, &target_symbol.path);
            let abs_file = workspace.canonical_root.join(&target_symbol.path);
            let target_body = slice_symbol_body(&abs_file, &target_symbol)?;

            let mut callee_signatures = Vec::new();
            if let Ok(callees) = code_kb_core::find_references(&conn, &args.symbol, "callees", 10) {
                for c in callees {
                    if let Ok(Some(s)) = get_symbol_by_name(&conn, &c.to_symbol_name, None) {
                        let sig = s.signature.unwrap_or(s.name);
                        callee_signatures.push(format!("{sig} ({}:{})", s.path, s.start_line));
                    }
                }
            }

            let mut related_types = Vec::new();
            if let Ok(types) = code_kb_core::find_type_facts(&conn, &target_symbol.symbol_id) {
                for t in types {
                    related_types.push(t.resolved_type);
                }
            }

            let related_tests = match search_symbols(&conn, &args.symbol, None, true, 5) {
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

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&slice)?);
            } else {
                println!("{}", format_context_slice(&slice));
            }
        }
        Command::Refs(args) => {
            let refs = code_kb_core::find_references(&conn, &args.symbol, &args.direction, args.limit)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&refs)?);
            } else {
                println!("{}", format_references(&args.symbol, &refs, &args.direction));
            }
        }
        Command::Facts(args) => {
            let facts = code_kb_core::find_structural_facts(&conn, &args.category, args.limit)?;
            let literals = code_kb_core::find_literals(&conn, &args.category, args.limit)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                    "structural_facts": facts,
                    "literals": literals
                }))?);
            } else {
                println!("Structural facts for '{}' ({} found):\n", args.category, facts.len());
                for f in &facts {
                    let parent = f.containing_symbol_name.as_deref().unwrap_or("top-level");
                    println!("- {} [{}:{}] (pattern: {}, in: {})", f.capture_name, f.path, f.start_line, f.pattern_id, parent);
                }
                if !literals.is_empty() {
                    println!("\nMatching literals ({} found):\n", literals.len());
                    for l in &literals {
                        println!("- \"{}\" [{}:{}] (kind: {})", l.literal_text, l.path, l.start_line, l.kind);
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
                res.symbol_name, res.file_path, res.old_body_hash, res.new_body_hash, res.bytes_written
            );
        }
        Command::Serve(_) | Command::Scan(_) | Command::Logs(_) => unreachable!(),
    }

    Ok(())
}
