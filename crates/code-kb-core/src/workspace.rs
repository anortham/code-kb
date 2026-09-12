use std::path::{Path, PathBuf};
use sha2::{Digest, Sha256};
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

/// Strip Windows verbatim prefix (\\?\, \\?\UNC\) using dunce.
pub fn normalize_path(path: &Path) -> PathBuf {
    dunce::simplified(path).to_path_buf()
}

/// Convert a path to forward-slash string representation for stable relative paths.
pub fn to_forward_slash(path: &Path) -> String {
    let s = path.to_string_lossy();
    s.replace('\\', "/")
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
        let canonical_root = normalize_path(&dunce::canonicalize(&root).unwrap_or_else(|_| root.clone()));

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
        let canonical_root = normalize_path(&dunce::canonicalize(&root).unwrap_or_else(|_| root.clone()));
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
        let abs_path = if input.is_absolute() {
            normalize_path(input)
        } else {
            normalize_path(&self.canonical_root.join(input))
        };

        // Check if within canonical root
        let rel = match abs_path.strip_prefix(&self.canonical_root) {
            Ok(r) => to_forward_slash(r),
            Err(_) => {
                // Check if dunce-canonicalized root matches
                let norm_abs = dunce::canonicalize(&abs_path).unwrap_or_else(|_| abs_path.clone());
                let norm_root = dunce::canonicalize(&self.canonical_root).unwrap_or_else(|_| self.canonical_root.clone());
                match norm_abs.strip_prefix(&norm_root) {
                    Ok(r) => to_forward_slash(r),
                    Err(_) => return Err(WorkspaceError::PathOutsideWorkspace(abs_path, self.canonical_root.clone())),
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
            candidates.push(cache_dir.join("stores").join(&store_slug).join("artifact.db"));
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
            WorkspaceError::ArtifactNotFound(self.canonical_root.join(".code-kb").join("artifact.db"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
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
}
