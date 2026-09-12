pub mod db;
pub mod edit;
pub mod formatters;
pub mod models;
pub mod queries;
pub mod slicer;
pub mod sync;
pub mod watcher;
pub mod workspace;

pub use db::{open_read_only, open_read_write, DbError};
pub use edit::{replace_symbol_body, EditError, EditResult};
pub use formatters::{
    format_codebase_outline, format_context_slice, format_file_skeleton, format_references,
};
pub use models::{ContextSlice, FileFact, LiteralFact, ReferenceSite, StructuralFact, Symbol, TypeFact};
pub use queries::{
    find_literals, find_references, find_structural_facts, find_type_facts, get_file,
    get_symbol_by_name, load_file_symbols, load_files, search_symbols, QueryError,
};
pub use slicer::{slice_symbol, slice_symbol_body, SliceError};
pub use sync::{
    delete_file, ensure_fresh_file, find_julie_extract_binary, reconcile_offline_edits,
    scan_workspace, update_file, ReconcileReport, SyncError,
};
pub use watcher::{start_watcher, WatcherError, WatcherHandle};
pub use workspace::{normalize_path, to_forward_slash, Workspace, WorkspaceError};
