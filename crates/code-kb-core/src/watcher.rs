use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use ignore::gitignore::GitignoreBuilder;
use notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebouncedEvent, Debouncer};
use thiserror::Error;
use tracing::{info, warn};

use crate::sync::{delete_file, scan_workspace, update_file};
use crate::workspace::{to_forward_slash, Workspace};

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
    _debouncer: Debouncer<notify::RecommendedWatcher>,
    pub running: Arc<AtomicBool>,
}

fn is_hard_excluded(rel_path: &str) -> bool {
    let p = rel_path.replace('\\', "/");
    p.starts_with(".git")
        || p.starts_with(".code-kb")
        || p.starts_with("target")
        || p.starts_with("node_modules")
        || p.starts_with(".idea")
        || p.starts_with(".vscode")
        || p.ends_with(".tmp")
        || p.ends_with(".swp")
        || p.ends_with("~")
}

/// Starts debounced background file watcher with git storm circuit breaker.
pub fn start_watcher(
    workspace: Workspace,
    db_path: PathBuf,
) -> Result<WatcherHandle, WatcherError> {
    let mut gitignore_builder = GitignoreBuilder::new(&workspace.canonical_root);
    let gitignore_path = workspace.canonical_root.join(".gitignore");
    if gitignore_path.exists() {
        let _ = gitignore_builder.add(&gitignore_path);
    }
    let julieignore_path = workspace.canonical_root.join(".julieignore");
    if julieignore_path.exists() {
        let _ = gitignore_builder.add(&julieignore_path);
    }
    let gitignore = gitignore_builder.build().unwrap_or_else(|_| {
        GitignoreBuilder::new(&workspace.canonical_root)
            .build()
            .unwrap()
    });

    let running = Arc::new(AtomicBool::new(true));
    let ws_clone = workspace.clone();
    let db_clone = db_path.clone();

    // 150ms debounce window
    let mut debouncer = new_debouncer(
        Duration::from_millis(150),
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

            // Filter relevant events using gitignore & hard exclusions
            let mut relevant_files = Vec::new();
            for event in &events {
                let norm_path = dunce::simplified(&event.path);
                if let Ok(rel) = norm_path.strip_prefix(&ws_clone.canonical_root) {
                    let rel_str = to_forward_slash(rel);
                    if is_hard_excluded(&rel_str) {
                        continue;
                    }
                    let is_dir = norm_path.is_dir();
                    if gitignore.matched(norm_path, is_dir).is_ignore() {
                        continue;
                    }
                    relevant_files.push((norm_path.to_path_buf(), rel_str));
                }
            }

            if relevant_files.is_empty() {
                return;
            }

            // Git checkout storm circuit-breaker:
            // If more than 50 files changed within the debounce window,
            // cancel micro-updates and run a single bulk scan.
            if relevant_files.len() > 50 {
                info!(
                    "Git storm detected ({} files changed in window). Running bulk scan...",
                    relevant_files.len()
                );
                if let Err(e) = scan_workspace(&ws_clone, &db_clone, false) {
                    warn!("Bulk scan failed during git storm: {e}");
                }
                return;
            }

            // Otherwise, process incremental updates in lock-free WAL mode
            for (abs, rel) in relevant_files {
                if abs.exists() && abs.is_file() {
                    let _ = update_file(&ws_clone, &db_clone, &rel);
                } else if !abs.exists() {
                    let _ = delete_file(&ws_clone, &db_clone, &rel);
                }
            }
        },
    )?;

    // Watch workspace root recursively
    debouncer
        .watcher()
        .watch(&workspace.canonical_root, RecursiveMode::Recursive)?;

    info!(
        "Tier 3 file watcher active on '{}' (150ms debounce)",
        workspace.canonical_root.display()
    );

    Ok(WatcherHandle {
        _debouncer: debouncer,
        running,
    })
}
