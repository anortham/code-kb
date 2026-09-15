pub mod db;
pub mod edit;
pub mod formatters;
pub mod models;
pub mod ops;
pub mod queries;
pub mod slicer;
pub mod sync;
pub mod syntax;
pub mod telemetry;
pub mod watcher;
pub mod workspace;

pub use db::{
    Connection, DbError, ensure_fts_index, ensure_fts_index_path, open_read_only, open_read_write,
};
pub use edit::{EditError, EditResult, replace_symbol_body};
pub use formatters::{
    format_blast_radius, format_context_slice, format_fact_categories, format_file_skeleton,
    format_find_symbol_results, format_references, format_replace_symbol_result,
    format_search_results, format_structural_facts, format_symbol_body,
};
pub use models::{
    BlastRadiusResult, ContextSlice, FileFact, ImpactedSymbol, LiteralFact, ReferenceSite,
    StructuralFact, Symbol, SymbolSearchResult, TestTarget, TypeFact,
};
pub use ops::{
    OpError, blast_radius_op, codebase_outline_op, file_skeleton_op, get_context_slice_op,
    get_symbol_body_op,
};
pub use queries::{
    QueryError, compute_blast_radius, compute_blast_radius_scoped, find_callee_signatures,
    find_literals, find_literals_scoped, find_references, find_references_ext,
    find_references_for_symbol, find_references_scoped, find_related_tests, find_structural_facts,
    find_structural_facts_scoped, find_type_facts, fts_search_symbols_scoped, get_file,
    get_symbol_by_name, get_symbol_by_name_exact, is_test_path, list_structural_fact_categories,
    list_structural_fact_categories_scoped, load_file_symbols, load_scoped_outline_symbols,
    normalize_kind, sanitize_fts5_query, search_symbols, search_symbols_scoped,
};
pub use slicer::{SliceError, slice_symbol, slice_symbol_body};
pub use sync::{
    PINNED_JULIE_VERSION, ReconcileReport, SyncError, create_index, delete_file, ensure_fresh_file,
    ensure_index_matches_extractor, find_julie_extract_binary, installed_extractor_version,
    reconcile_offline_edits, scan_workspace, update_file,
};
pub use syntax::{SyntaxError, validate_syntax};
pub use telemetry::{
    BugReportBundle, TelemetryErrorRecord, TelemetryFilter, TelemetrySummary, TimeWindow,
    ToolInvocation, ToolStat, format_telemetry_summary, generate_bug_report,
    get_global_telemetry_dir, get_telemetry_summary, open_global_telemetry_db, open_telemetry_db,
    record_tool_call, record_tool_call_conn,
};
pub use watcher::{WatcherError, WatcherHandle, start_watcher};
pub use workspace::{
    Workspace, WorkspaceError, is_hard_excluded, is_project_root, normalize_path, parse_file_uri,
    strip_prefix_lossy, to_forward_slash,
};

/// Creates a temporary directory in a safe location, prioritizing `CARGO_TARGET_TMPDIR`,
/// `TMPDIR`, and workspace `./target/tmp` over `/tmp` to avoid tmpfs quota limits.
pub fn safe_tempdir() -> tempfile::TempDir {
    if let Ok(target_tmp) = std::env::var("CARGO_TARGET_TMPDIR") {
        let path = std::path::PathBuf::from(&target_tmp);
        if (path.exists() || std::fs::create_dir_all(&path).is_ok())
            && let Ok(dir) = tempfile::TempDir::new_in(&path)
        {
            return dir;
        }
    }
    if let Ok(tmp) = std::env::var("TMPDIR") {
        let path = std::path::PathBuf::from(tmp);
        if (path.exists() || std::fs::create_dir_all(&path).is_ok())
            && let Ok(dir) = tempfile::TempDir::new_in(&path)
        {
            return dir;
        }
    }

    // Search ancestors of CARGO_MANIFEST_DIR or current_dir for workspace target/tmp
    let mut search_dirs = Vec::new();
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        search_dirs.push(std::path::PathBuf::from(manifest_dir));
    }
    if let Ok(cwd) = std::env::current_dir() {
        search_dirs.push(cwd);
    }
    for base in search_dirs {
        let mut cur = base;
        loop {
            let candidate = cur.join("target/tmp");
            if (cur.join("Cargo.toml").exists() || cur.join(".git").exists())
                && (candidate.exists() || std::fs::create_dir_all(&candidate).is_ok())
                && let Ok(dir) = tempfile::TempDir::new_in(&candidate)
            {
                return dir;
            }
            if !cur.pop() {
                break;
            }
        }
    }

    tempfile::tempdir().expect("failed to create temporary directory")
}
