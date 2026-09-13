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
    QueryError, compute_blast_radius, find_callee_signatures, find_literals, find_references,
    find_references_ext, find_references_for_symbol, find_related_tests, find_structural_facts,
    find_type_facts, fts_search_symbols, fts_search_symbols_scoped, get_file, get_symbol_by_name,
    get_symbol_by_name_exact, is_test_path, list_structural_fact_categories, load_file_symbols,
    load_scoped_outline_symbols, normalize_kind, sanitize_fts5_query, search_symbols,
    search_symbols_scoped,
};
pub use slicer::{SliceError, slice_symbol, slice_symbol_body};
pub use sync::{
    ReconcileReport, SyncError, delete_file, ensure_fresh_file, find_julie_extract_binary,
    reconcile_offline_edits, scan_workspace, update_file,
};
pub use syntax::{SyntaxError, validate_syntax};
pub use telemetry::{
    TelemetryErrorRecord, TelemetrySummary, ToolInvocation, ToolStat, format_telemetry_summary,
    get_telemetry_summary, open_telemetry_db, record_tool_call, record_tool_call_conn,
};
pub use watcher::{WatcherError, WatcherHandle, start_watcher};
pub use workspace::{
    Workspace, WorkspaceError, is_hard_excluded, normalize_path, parse_file_uri, to_forward_slash,
};

/// Creates a temporary directory in a safe location, prioritizing `CARGO_TARGET_TMPDIR`,
/// `TMPDIR`, and `./target/tmp` over `/tmp` to avoid tmpfs quota limits.
pub fn safe_tempdir() -> tempfile::TempDir {
    if let Ok(target_tmp) = std::env::var("CARGO_TARGET_TMPDIR") {
        let path = std::path::PathBuf::from(&target_tmp);
        if path.exists()
            && let Ok(dir) = tempfile::TempDir::new_in(&path)
        {
            return dir;
        }
    }
    if let Ok(tmp) = std::env::var("TMPDIR") {
        let path = std::path::PathBuf::from(tmp);
        if path.exists()
            && let Ok(dir) = tempfile::TempDir::new_in(&path)
        {
            return dir;
        }
    }
    let local_target_tmp = std::path::PathBuf::from("target/tmp");
    if (local_target_tmp.exists() || std::fs::create_dir_all(&local_target_tmp).is_ok())
        && let Ok(dir) = tempfile::TempDir::new_in(&local_target_tmp)
    {
        return dir;
    }
    tempfile::tempdir().expect("failed to create temporary directory")
}
