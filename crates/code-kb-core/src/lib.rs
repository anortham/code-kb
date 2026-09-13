pub mod db;
pub mod edit;
pub mod formatters;
pub mod models;
pub mod ops;
pub mod queries;
pub mod slicer;
pub mod sync;
pub mod syntax;
pub mod watcher;
pub mod workspace;

pub use db::{DbError, ensure_fts_index, ensure_fts_index_path, open_read_only, open_read_write};
pub use edit::{EditError, EditResult, replace_symbol_body};
pub use formatters::{
    format_codebase_outline, format_context_slice, format_file_skeleton, format_references,
    format_search_results,
};
pub use models::{
    ContextSlice, FileFact, LiteralFact, ReferenceSite, StructuralFact, Symbol, SymbolSearchResult,
    TypeFact,
};
pub use ops::{
    OpError, codebase_outline_op, file_skeleton_op, get_context_slice_op, get_symbol_body_op,
};
pub use queries::{
    QueryError, find_literals, find_references, find_structural_facts, find_type_facts,
    fts_search_symbols, get_file, get_symbol_by_name, get_symbol_by_name_exact, load_file_symbols,
    load_files, load_scoped_outline_symbols, sanitize_fts5_query, search_symbols,
};
pub use slicer::{SliceError, slice_symbol, slice_symbol_body};
pub use sync::{
    ReconcileReport, SyncError, delete_file, ensure_fresh_file, find_julie_extract_binary,
    reconcile_offline_edits, scan_workspace, update_file,
};
pub use syntax::{SyntaxError, validate_syntax};
pub use watcher::{WatcherError, WatcherHandle, start_watcher};
pub use workspace::{Workspace, WorkspaceError, normalize_path, parse_file_uri, to_forward_slash};
