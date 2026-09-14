use ignore::WalkBuilder;
use notify::RecursiveMode;
use notify_debouncer_full::{DebouncedEvent, Debouncer, RecommendedCache, new_debouncer};
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;
use tracing::{info, warn};

use crate::sync::{delete_file, ensure_fresh_file, scan_workspace, update_file};
use crate::workspace::{Workspace, is_hard_excluded, to_forward_slash};

#[derive(Debug, Error)]
pub enum WatcherError {
    #[error("Failed to initialize notify watcher: {0}")]
    Notify(#[from] notify::Error),
    #[error("Failed to build gitignore filters: {0}")]
    Ignore(#[from] ignore::Error),
}

/// Active background file watcher handle.
pub struct WatcherHandle {
    // Retaining debouncer keeps the background notify thread running
    _debouncer: Debouncer<notify::RecommendedWatcher, RecommendedCache>,
}

/// Starts debounced background file watcher with git storm circuit breaker.
pub fn start_watcher(
    workspace: Workspace,
    db_path: PathBuf,
) -> Result<WatcherHandle, WatcherError> {
    let ws_clone = workspace.clone();
    let db_clone = db_path.clone();

    // 150ms debounce window
    let mut debouncer = new_debouncer(
        Duration::from_millis(150),
        None,
        move |res: Result<Vec<DebouncedEvent>, _>| {
            let events = match res {
                Ok(evts) => evts,
                Err(err) => {
                    warn!("File watcher error: {:?}", err);
                    return;
                }
            };

            if events.is_empty() {
                return;
            }

            let mut ignore_builder = WalkBuilder::new(&ws_clone.canonical_root);
            ignore_builder
                .standard_filters(true)
                .hidden(false)
                .add_custom_ignore_filename(".julieignore")
                .add_custom_ignore_filename(".code-kb-ignore")
                .add_custom_ignore_filename(".codekbignore");
            let mut ignore_matcher = match ignore_builder.build_matchers().pop() {
                Some(m) => m,
                None => {
                    warn!("Failed to initialize ignore matcher for workspace root; skipping tick");
                    return;
                }
            };

            let mut relevant_files = Vec::new();
            for event in &events {
                // Ignore read/access events (e.g. inotify IN_OPEN / IN_ACCESS)
                if event.kind.is_access() {
                    continue;
                }

                for path in &event.paths {
                    let norm_path = dunce::simplified(path);
                    if let Some(rel) = crate::workspace::strip_prefix_lossy(norm_path, &ws_clone.canonical_root) {
                        let rel_str = to_forward_slash(rel);
                        if is_hard_excluded(&rel_str) {
                            continue;
                        }
                        let is_dir = norm_path.is_dir();
                        let (matched, error) = ignore_matcher.matched_with_errors(rel, is_dir);
                        if let Some(error) = error {
                            warn!("Failed to load ignore rule: {error}");
                        }
                        if matched.is_ignore() {
                            continue;
                        }
                        relevant_files.push((norm_path.to_path_buf(), rel_str));
                    }
                }
            }

            if relevant_files.is_empty() {
                return;
            }

            // Deduplicate paths in this window
            relevant_files.sort_by(|a, b| a.1.cmp(&b.1));
            relevant_files.dedup_by(|a, b| a.1 == b.1);

            // Git checkout storm circuit-breaker:
            // If more than 50 files changed within the debounce window,
            // cancel micro-updates and run a single bulk scan.
            if relevant_files.len() > 50 {
                tracing::debug!(
                    "Git storm detected ({} files changed in window). Running bulk scan...",
                    relevant_files.len()
                );
                if let Err(e) = scan_workspace(&ws_clone, &db_clone, false) {
                    warn!("Bulk scan failed during git storm: {e}");
                }
                return;
            }

            // Otherwise, process incremental updates in lock-free WAL mode
            let conn_opt = crate::db::open_read_only(&db_clone).ok();
            for (abs, rel) in relevant_files {
                if abs.exists() && abs.is_file() {
                    let mut handled = false;
                    if let Some(ref conn) = conn_opt
                        && let Ok(_fresh) = ensure_fresh_file(&ws_clone, &db_clone, conn, &rel)
                    {
                        handled = true;
                    }
                    if !handled {
                        let _ = update_file(&ws_clone, &db_clone, &rel);
                    }
                } else if !abs.exists() {
                    let _ = delete_file(&ws_clone, &db_clone, &rel);
                }
            }
        },
    )?;

    // Watch workspace root recursively
    debouncer.watch(&workspace.canonical_root, RecursiveMode::Recursive)?;

    info!(
        "Tier 3 file watcher active on '{}' (150ms debounce)",
        workspace.canonical_root.display()
    );

    Ok(WatcherHandle {
        _debouncer: debouncer,
    })
}
