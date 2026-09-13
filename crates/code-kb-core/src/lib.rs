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

pub use db::{ensure_fts_index, ensure_fts_index_path, open_read_only, open_read_write, DbError};
pub use edit::{replace_symbol_body, EditError, EditResult};
pub use syntax::{validate_syntax, SyntaxError};
pub use formatters::{
    format_codebase_outline, format_context_slice, format_file_skeleton, format_references,
    format_search_results,
};
pub use models::{
    ContextSlice, FileFact, LiteralFact, ReferenceSite, StructuralFact, Symbol, SymbolSearchResult,
    TypeFact,
};
pub use ops::{
    codebase_outline_op, file_skeleton_op, get_context_slice_op, get_symbol_body_op, OpError,
};
pub use queries::{
    find_literals, find_references, find_structural_facts, find_type_facts, fts_search_symbols,
    get_file, get_symbol_by_name, get_symbol_by_name_exact, load_file_symbols, load_files,
    load_scoped_outline_symbols, sanitize_fts5_query, search_symbols, QueryError,
};
pub use slicer::{slice_symbol, slice_symbol_body, SliceError};
pub use sync::{
    delete_file, ensure_fresh_file, find_julie_extract_binary, reconcile_offline_edits,
    scan_workspace, update_file, ReconcileReport, SyncError,
};
pub use watcher::{start_watcher, WatcherError, WatcherHandle};
pub use workspace::{normalize_path, parse_file_uri, to_forward_slash, Workspace, WorkspaceError};
