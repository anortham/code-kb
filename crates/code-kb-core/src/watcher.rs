use ignore::WalkBuilder;
use notify::RecursiveMode;
use notify_debouncer_full::{DebouncedEvent, Debouncer, RecommendedCache, new_debouncer};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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
    failures: Arc<Mutex<WatcherFailureState>>,
}

#[derive(Default)]
struct WatcherFailureState {
    generation: u64,
    failure: Option<String>,
}

impl WatcherHandle {
    pub fn pending_failure(&self) -> Option<(u64, String)> {
        let failures = self
            .failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        failures
            .failure
            .as_ref()
            .map(|failure| (failures.generation, failure.clone()))
    }

    pub fn clear_failure_if_unchanged(&self, generation: u64) -> bool {
        let mut failures = self
            .failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if failures.generation != generation {
            return false;
        }
        failures.failure = None;
        true
    }
}

fn record_failure(failures: &Arc<Mutex<WatcherFailureState>>, failure: String) {
    let mut failures = failures
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    failures.generation = failures.generation.wrapping_add(1);
    if failures.failure.is_none() {
        failures.failure = Some(failure.chars().take(512).collect());
    }
}

/// Starts debounced background file watcher with git storm circuit breaker.
pub fn start_watcher(
    workspace: Workspace,
    db_path: PathBuf,
) -> Result<WatcherHandle, WatcherError> {
    let ws_clone = workspace.clone();
    let db_clone = db_path.clone();
    let failures = Arc::new(Mutex::new(WatcherFailureState::default()));
    let callback_failures = Arc::clone(&failures);

    let mut debouncer = new_debouncer(
        Duration::from_millis(150),
        None,
        move |res: Result<Vec<DebouncedEvent>, _>| {
            let events = match res {
                Ok(evts) => evts,
                Err(err) => {
                    warn!("File watcher error: {:?}", err);
                    record_failure(&callback_failures, format!("File watcher error: {err:?}"));
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
                    record_failure(
                        &callback_failures,
                        "Failed to initialize ignore matcher for workspace root".to_string(),
                    );
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
                    if let Some(rel) =
                        crate::workspace::strip_prefix_lossy(norm_path, &ws_clone.canonical_root)
                    {
                        let rel_str = to_forward_slash(rel);
                        let rel_str = rel_str.trim_start_matches('/').to_string();
                        if rel_str.is_empty() || is_hard_excluded(&rel_str) {
                            continue;
                        }
                        let is_dir = norm_path.is_dir();
                        let (matched, error) =
                            ignore_matcher.matched_with_errors(Path::new(&rel_str), is_dir);
                        if let Some(error) = error {
                            warn!("Failed to load ignore rule: {error}");
                            record_failure(
                                &callback_failures,
                                format!("Failed to load ignore rule: {error}"),
                            );
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

            relevant_files.sort_by(|a, b| a.1.cmp(&b.1));
            relevant_files.dedup_by(|a, b| a.1 == b.1);

            if relevant_files.len() > 50 {
                tracing::debug!(
                    "Git storm detected ({} files changed in window). Running bulk scan...",
                    relevant_files.len()
                );
                if let Err(e) = scan_workspace(&ws_clone, &db_clone, false) {
                    warn!("Bulk scan failed during git storm: {e}");
                    record_failure(
                        &callback_failures,
                        format!("Bulk scan failed during git storm: {e}"),
                    );
                }
                return;
            }

            let conn = match crate::db::open_read_only(&db_clone) {
                Ok(conn) => conn,
                Err(e) => {
                    warn!("Failed to open index during watcher update: {e}");
                    record_failure(
                        &callback_failures,
                        format!("Failed to open index during watcher update: {e}"),
                    );
                    return;
                }
            };
            for (abs, rel) in relevant_files {
                if abs.exists() && abs.is_file() {
                    let result = match ensure_fresh_file(&ws_clone, &db_clone, &conn, &rel) {
                        Ok(_) => Ok(()),
                        Err(_) => update_file(&ws_clone, &db_clone, &rel),
                    };
                    if let Err(e) = result {
                        warn!("Failed to refresh '{}' from watcher: {e}", rel);
                        record_failure(
                            &callback_failures,
                            format!("Failed to refresh '{rel}' from watcher: {e}"),
                        );
                    }
                } else if !abs.exists() {
                    let escaped = crate::queries::escape_like(&rel);
                    let pattern = format!("{escaped}/%");
                    let child_paths_result = conn
                        .prepare("SELECT path FROM files WHERE path LIKE ?1 ESCAPE '\\' LIMIT 51")
                        .and_then(|mut stmt| {
                            let rows = stmt.query_map([&pattern], |r| r.get::<_, String>(0))?;
                            rows.collect::<Result<Vec<_>, _>>()
                        });
                    let child_paths = match child_paths_result {
                        Ok(paths) => paths,
                        Err(e) => {
                            warn!("Failed to read indexed paths for deleted '{}': {e}", rel);
                            record_failure(
                                &callback_failures,
                                format!("Failed to read indexed paths for deleted '{rel}': {e}"),
                            );
                            continue;
                        }
                    };
                    if child_paths.len() > 50 {
                        if let Err(e) = scan_workspace(&ws_clone, &db_clone, false) {
                            warn!("Bulk scan failed after deletion of '{}': {e}", rel);
                            record_failure(
                                &callback_failures,
                                format!("Bulk scan failed after deletion of '{rel}': {e}"),
                            );
                        }
                    } else {
                        for child in child_paths {
                            if let Err(e) = delete_file(&ws_clone, &db_clone, &child) {
                                warn!("Failed to remove deleted file '{}': {e}", child);
                                record_failure(
                                    &callback_failures,
                                    format!("Failed to remove deleted file '{child}': {e}"),
                                );
                            }
                        }
                        if let Err(e) = delete_file(&ws_clone, &db_clone, &rel) {
                            warn!("Failed to remove deleted file '{}': {e}", rel);
                            record_failure(
                                &callback_failures,
                                format!("Failed to remove deleted file '{rel}': {e}"),
                            );
                        }
                    }
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
        failures,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_drive_root_leading_slash_trimmed() {
        let raw_rel = Path::new("/src/components/button.rs");
        let rel_str = to_forward_slash(raw_rel);
        let trimmed = rel_str.trim_start_matches('/').to_string();
        assert_eq!(trimmed, "src/components/button.rs");
        assert!(!trimmed.starts_with('/'));

        let raw_rel_win = Path::new("\\src\\components\\button.rs");
        let rel_str_win = to_forward_slash(raw_rel_win);
        let trimmed_win = rel_str_win.trim_start_matches('/').to_string();
        assert_eq!(trimmed_win, "src/components/button.rs");
        assert!(!trimmed_win.starts_with('/'));
    }

    #[test]
    fn test_child_files_query_pattern_escaped() {
        let dir_rel = "src/components";
        let escaped = dir_rel
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("{escaped}/%");
        assert_eq!(pattern, "src/components/%");

        let dir_with_wildcards = "src/foo_bar%baz";
        let escaped2 = dir_with_wildcards
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern2 = format!("{escaped2}/%");
        assert_eq!(pattern2, "src/foo\\_bar\\%baz/%");
    }
}
