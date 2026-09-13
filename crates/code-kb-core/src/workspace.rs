use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("Failed to canonicalize path {path}: {source}")]
    CanonicalizationFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Path '{0}' is outside workspace root '{1}'")]
    PathOutsideWorkspace(PathBuf, PathBuf),
    #[error("Could not discover workspace root from '{0}'")]
    DiscoveryFailed(PathBuf),
    #[error("Database artifact not found at '{0}'")]
    ArtifactNotFound(PathBuf),
}

/// Lexically clean a path by collapsing `.` and `..` components.
pub fn clean_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut stack = Vec::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if let Some(Component::Normal(_)) = stack.last() {
                    stack.pop();
                } else {
                    stack.push(comp);
                }
            }
            _ => stack.push(comp),
        }
    }
    stack.into_iter().collect()
}

/// Parse an MCP file URI or plain path into a normalized PathBuf.
/// Handles standard file URIs (`file:///path`), URI percent-encoding (e.g. `%20`), and plain paths.
pub fn parse_file_uri(cand: &str) -> Option<PathBuf> {
    if cand.starts_with("file://") {
        if let Ok(url) = url::Url::parse(cand)
            && let Ok(path) = url.to_file_path()
        {
            return Some(normalize_path(&path));
        }
        // Fallback for non-standard file:// patterns
        if let Some(s) = cand.strip_prefix("file:///") {
            if cfg!(windows) {
                Some(normalize_path(Path::new(s)))
            } else {
                Some(normalize_path(&PathBuf::from(format!("/{}", s))))
            }
        } else if let Some(s) = cand.strip_prefix("file://") {
            Some(normalize_path(Path::new(s)))
        } else {
            Some(normalize_path(Path::new(cand)))
        }
    } else {
        Some(normalize_path(Path::new(cand)))
    }
}

/// Strip Windows verbatim prefix (\\?\, \\?\UNC\) using dunce.
pub fn normalize_path(path: &Path) -> PathBuf {
    dunce::simplified(path).to_path_buf()
}

/// Convert a path to forward-slash string representation for stable relative paths.
pub fn to_forward_slash(path: &Path) -> String {
    let s = path.to_string_lossy();
    s.replace('\\', "/")
}

/// Check if a relative path contains directories or file patterns that must never be indexed or watched.
pub fn is_hard_excluded(rel_path: &str) -> bool {
    let p = rel_path.replace('\\', "/");
    let has_excluded_dir = p.split('/').any(|component| {
        matches!(
            component,
            ".git"
                | ".hg"
                | ".svn"
                | ".julie"
                | ".code-kb"
                | ".memories"
                | ".worktrees"
                | "worktrees"
                | ".claude"
                | ".venv"
                | "venv"
                | ".env"
                | ".tox"
                | ".vs"
                | "node_modules"
                | "vendor"
                | "target"
                | "dist"
                | "build"
                | ".cache"
                | "obj"
                | "TestResults"
                | ".idea"
                | ".vscode"
        )
    });

    if has_excluded_dir {
        return true;
    }

    const EXCLUDED_SUFFIXES: &[&str] = &[
        ".min.js",
        ".bundle.js",
        ".generated.js",
        ".generated.jsx",
        ".generated.ts",
        ".generated.tsx",
        ".generated.d.ts",
        ".tmp",
        ".swp",
        "~",
    ];

    EXCLUDED_SUFFIXES.iter().any(|suffix| p.ends_with(suffix))
}

/// Represents a bound workspace session.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub root: PathBuf,
    pub canonical_root: PathBuf,
    pub repo_name: String,
    pub repo_id: String,
}

impl Workspace {
    /// Discover and bind a workspace from an optional path, falling back to CWD and upward traversal.
    pub fn discover(start_path: Option<&Path>) -> Result<Self, WorkspaceError> {
        let current = match start_path {
            Some(p) => p.to_path_buf(),
            None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        };

        let root = Self::find_workspace_root(&current)?;
        let canonical_root =
            normalize_path(&dunce::canonicalize(&root).unwrap_or_else(|_| root.clone()));

        let repo_name = canonical_root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "repo".to_string());

        let mut hasher = Sha256::new();
        hasher.update(to_forward_slash(&canonical_root).as_bytes());
        let hash = hex::encode(hasher.finalize());
        let repo_id = hash[..16].to_string();

        Ok(Self {
            root,
            canonical_root,
            repo_name,
            repo_id,
        })
    }

    /// Create workspace binding directly for a known root directory.
    pub fn new(root: PathBuf) -> Self {
        let canonical_root =
            normalize_path(&dunce::canonicalize(&root).unwrap_or_else(|_| root.clone()));
        let repo_name = canonical_root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "repo".to_string());

        let mut hasher = Sha256::new();
        hasher.update(to_forward_slash(&canonical_root).as_bytes());
        let hash = hex::encode(hasher.finalize());
        let repo_id = hash[..16].to_string();

        Self {
            root,
            canonical_root,
            repo_name,
            repo_id,
        }
    }

    /// Find root by searching upwards for .git, .code-kb, or workspace markers.
    pub fn find_workspace_root(start: &Path) -> Result<PathBuf, WorkspaceError> {
        let mut curr = if start.is_file() {
            start.parent().unwrap_or(start).to_path_buf()
        } else {
            start.to_path_buf()
        };

        // Pass 1: Look for .git or .code-kb all the way up
        let mut probe = curr.clone();
        loop {
            if probe.join(".code-kb").exists() || probe.join(".git").exists() {
                return Ok(probe);
            }
            if let Some(parent) = probe.parent() {
                if parent == probe {
                    break;
                }
                probe = parent.to_path_buf();
            } else {
                break;
            }
        }

        // Pass 2: Look for language project markers
        loop {
            if curr.join("Cargo.toml").exists()
                || curr.join("package.json").exists()
                || curr.join("go.mod").exists()
                || curr.join("pyproject.toml").exists()
            {
                return Ok(curr);
            }

            if let Some(parent) = curr.parent() {
                if parent == curr {
                    break;
                }
                curr = parent.to_path_buf();
            } else {
                break;
            }
        }

        // Default to start directory if no markers found
        Ok(start.to_path_buf())
    }

    /// Resolves an input path (relative or absolute) to a canonical absolute path and relative path.
    pub fn resolve_path(&self, input: &Path) -> Result<(PathBuf, String), WorkspaceError> {
        let joined = if input.is_absolute() {
            input.to_path_buf()
        } else {
            self.canonical_root.join(input)
        };

        // Lexically clean the path to collapse `.` and `..` components
        let cleaned = clean_path(&joined);
        let abs_path = normalize_path(&cleaned);

        // If file exists, canonicalize to resolve any symlinks
        let effective_abs = if abs_path.exists() {
            dunce::canonicalize(&abs_path)
                .map(|p| normalize_path(&p))
                .unwrap_or_else(|_| abs_path.clone())
        } else {
            abs_path.clone()
        };

        let norm_root = dunce::canonicalize(&self.canonical_root)
            .map(|p| normalize_path(&p))
            .unwrap_or_else(|_| self.canonical_root.clone());

        // Check if within canonical root
        let rel = match effective_abs.strip_prefix(&norm_root) {
            Ok(r) => {
                let forward = to_forward_slash(r);
                if forward.starts_with("../") || forward == ".." {
                    return Err(WorkspaceError::PathOutsideWorkspace(
                        abs_path,
                        self.canonical_root.clone(),
                    ));
                }
                forward
            }
            Err(_) => {
                // Fallback check against raw canonical_root
                match effective_abs.strip_prefix(&self.canonical_root) {
                    Ok(r) => {
                        let forward = to_forward_slash(r);
                        if forward.starts_with("../") || forward == ".." {
                            return Err(WorkspaceError::PathOutsideWorkspace(
                                abs_path,
                                self.canonical_root.clone(),
                            ));
                        }
                        forward
                    }
                    Err(_) => {
                        return Err(WorkspaceError::PathOutsideWorkspace(
                            abs_path,
                            self.canonical_root.clone(),
                        ));
                    }
                }
            }
        };

        Ok((abs_path, rel))
    }

    /// Resolve candidate database paths for this workspace:
    /// 1. Explicit override path (if provided)
    /// 2. In-tree `.code-kb/artifact.db` or `.code-kb/store.db`
    /// 3. Centralized user cache `%LOCALAPPDATA%\code-kb\stores\<repo_slug>-<hash>\artifact.db`
    pub fn candidate_db_paths(&self, explicit_db: Option<&Path>) -> Vec<PathBuf> {
        let mut candidates = Vec::new();

        if let Some(p) = explicit_db {
            candidates.push(p.to_path_buf());
        }

        // In-tree options
        candidates.push(self.canonical_root.join(".code-kb").join("artifact.db"));
        candidates.push(self.canonical_root.join(".code-kb").join("store.db"));
        candidates.push(self.canonical_root.join("artifact.db"));

        // Global user cache
        if let Some(proj_dirs) = directories::ProjectDirs::from("com", "code-kb", "code-kb") {
            let cache_dir = proj_dirs.cache_dir();
            let store_slug = format!("{}-{}", self.repo_name, self.repo_id);
            candidates.push(
                cache_dir
                    .join("stores")
                    .join(&store_slug)
                    .join("artifact.db"),
            );
            candidates.push(cache_dir.join("stores").join(&store_slug).join("store.db"));
        }

        candidates
    }

    /// Finds the first existing database file, or returns the default target location.
    pub fn locate_db(&self, explicit_db: Option<&Path>) -> Result<PathBuf, WorkspaceError> {
        let candidates = self.candidate_db_paths(explicit_db);
        for candidate in &candidates {
            if candidate.exists() && candidate.is_file() {
                return Ok(candidate.clone());
            }
        }

        // Return first non-explicit candidate as default if none exist yet
        candidates.into_iter().next().ok_or_else(|| {
            WorkspaceError::ArtifactNotFound(
                self.canonical_root.join(".code-kb").join("artifact.db"),
            )
        })
    }
}

/// Prunes global cache stores whose original workspace root paths no longer exist on disk.
/// Returns a list of pruned store directory paths.
/// Prunes global cache stores whose original workspace root paths no longer exist on disk.
/// Returns a list of pruned store directory paths.
pub fn prune_orphaned_stores(dry_run: bool) -> Vec<PathBuf> {
    let proj_dirs = match directories::ProjectDirs::from("com", "code-kb", "code-kb") {
        Some(d) => d,
        None => return Vec::new(),
    };

    let stores_dir = proj_dirs.cache_dir().join("stores");
    prune_orphaned_stores_at(&stores_dir, dry_run)
}

/// Prunes stores within a specified directory whose recorded root_path no longer exists on disk.
pub fn prune_orphaned_stores_at(stores_dir: &Path, dry_run: bool) -> Vec<PathBuf> {
    let mut pruned = Vec::new();

    if !stores_dir.exists() || !stores_dir.is_dir() {
        return pruned;
    }

    let entries = match std::fs::read_dir(stores_dir) {
        Ok(e) => e,
        Err(_) => return pruned,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let db_candidate = path.join("artifact.db");
        let store_db_candidate = path.join("store.db");
        let active_db = if db_candidate.exists() {
            Some(db_candidate)
        } else if store_db_candidate.exists() {
            Some(store_db_candidate)
        } else {
            None
        };

        if let Some(db_file) = active_db
            && let Ok(conn) = rusqlite::Connection::open_with_flags(
                &db_file,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
        {
            let root_path: rusqlite::Result<String> = conn.query_row(
                "SELECT value FROM artifact_metadata WHERE key = 'root_path'",
                [],
                |row| row.get(0),
            );

            if let Ok(root_str) = root_path {
                let root_p = Path::new(&root_str);
                if !root_p.exists() {
                    pruned.push(path.clone());
                    if !dry_run {
                        let _ = std::fs::remove_dir_all(&path);
                    }
                }
            }
        }
    }

    pruned
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn test_normalize_path() {
        let p = PathBuf::from(r"\\?\C:\source\code-kb\src\main.rs");
        let norm = normalize_path(&p);
        assert!(!norm.to_string_lossy().starts_with(r"\\?\"));
    }

    #[test]
    fn test_to_forward_slash() {
        let p = PathBuf::from(r"src\models\mod.rs");
        assert_eq!(to_forward_slash(&p), "src/models/mod.rs");
    }

    #[test]
    fn test_workspace_resolve_path() {
        let ws = Workspace::new(PathBuf::from("C:/source/test-project"));
        let (abs, rel) = ws.resolve_path(Path::new("src/lib.rs")).unwrap();
        assert_eq!(rel, "src/lib.rs");
        assert!(abs.to_string_lossy().contains("test-project"));
    }

    #[test]
    fn test_workspace_resolve_path_traversal_escape() {
        let temp = tempfile::tempdir().unwrap();
        let ws = Workspace::new(temp.path().to_path_buf());
        let res = ws.resolve_path(Path::new("sub/../../outside.rs"));
        assert!(
            matches!(res, Err(WorkspaceError::PathOutsideWorkspace(..))),
            "Expected PathOutsideWorkspace error, but got: {:?}",
            res
        );
    }

    #[test]
    fn test_parse_file_uri() {
        #[cfg(windows)]
        let (uri, expected) = ("file:///C:/my%20folder/project", "C:/my folder/project");
        #[cfg(not(windows))]
        let (uri, expected) = ("file:///tmp/my%20folder/project", "/tmp/my folder/project");

        let p1 = parse_file_uri(uri).unwrap();
        assert_eq!(p1, normalize_path(Path::new(expected)));

        // Plain path fallback
        let p2 = parse_file_uri("C:/direct/path").unwrap();
        assert_eq!(p2, normalize_path(Path::new("C:/direct/path")));
    }
}
