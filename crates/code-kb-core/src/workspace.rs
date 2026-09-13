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
                | ".agents"
                | ".razorback"
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
}

impl Workspace {
    /// Discover and bind a workspace from an optional path, falling back to CWD and upward traversal.
    pub fn discover(start_path: Option<&Path>) -> Result<Self, WorkspaceError> {
        let current = match start_path {
            Some(p) => p.to_path_buf(),
            None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        };

        let root = Self::find_workspace_root(&current)?;
        Ok(Self::new(root))
    }

    /// Create workspace binding directly for a known root directory.
    pub fn new(root: PathBuf) -> Self {
        let canonical_root =
            normalize_path(&dunce::canonicalize(&root).unwrap_or_else(|_| root.clone()));
        let repo_name = canonical_root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "repo".to_string());

        Self {
            root,
            canonical_root,
            repo_name,
        }
    }

    /// Find root by searching upwards for .git, .code-kb, or workspace markers.
    pub fn find_workspace_root(start: &Path) -> Result<PathBuf, WorkspaceError> {
        let curr = if start.is_file() {
            start.parent().unwrap_or(start).to_path_buf()
        } else {
            start.to_path_buf()
        };

        // Pass 1: Look for .git or .code-kb all the way up
        let mut probe = curr.clone();
        loop {
            if probe.join(".code-kb").exists() || probe.join(".git").exists() {
                let canon = dunce::canonicalize(&probe).unwrap_or(probe);
                return Ok(normalize_path(&canon));
            }
            if let Some(name) = probe.file_name().and_then(|n| n.to_str())
                && is_hard_excluded(name)
            {
                break;
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
        let mut curr_marker = curr.clone();
        loop {
            if curr_marker.join("Cargo.toml").exists()
                || curr_marker.join("package.json").exists()
                || curr_marker.join("go.mod").exists()
                || curr_marker.join("pyproject.toml").exists()
            {
                let canon = dunce::canonicalize(&curr_marker).unwrap_or(curr_marker);
                return Ok(normalize_path(&canon));
            }
            if let Some(name) = curr_marker.file_name().and_then(|n| n.to_str())
                && is_hard_excluded(name)
            {
                break;
            }

            if let Some(parent) = curr_marker.parent() {
                if parent == curr_marker {
                    break;
                }
                curr_marker = parent.to_path_buf();
            } else {
                break;
            }
        }

        // Default to start directory if no markers found
        let start_dir = if start.is_file() {
            start.parent().unwrap_or(start).to_path_buf()
        } else {
            start.to_path_buf()
        };
        let canon = dunce::canonicalize(&start_dir).unwrap_or(start_dir);
        Ok(normalize_path(&canon))
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
                                effective_abs,
                                self.canonical_root.clone(),
                            ));
                        }
                        forward
                    }
                    Err(_) => {
                        return Err(WorkspaceError::PathOutsideWorkspace(
                            effective_abs,
                            self.canonical_root.clone(),
                        ));
                    }
                }
            }
        };

        Ok((effective_abs, rel))
    }

    /// Relativizes a path filter string (which may be absolute, file:// URI, or relative)
    /// against this workspace root into a forward-slash relative path suitable for SQLite queries.
    pub fn relativize_filter(&self, filter: &str) -> String {
        let trimmed = filter.trim();
        if trimmed.is_empty() {
            return String::new();
        }

        // Handle file:// URI
        let path_str = if trimmed.starts_with("file://") {
            parse_file_uri(trimmed)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| trimmed.to_string())
        } else {
            trimmed.to_string()
        };

        let raw_path = Path::new(&path_str);
        let simplified = dunce::simplified(raw_path);

        if simplified.is_absolute() {
            if let Ok((_, rel)) = self.resolve_path(simplified) {
                return rel;
            }
            // If resolve_path failed (e.g. non-existent path), try prefix stripping on normalized strings
            let norm_simplified = normalize_path(simplified);
            let norm_root = normalize_path(&self.canonical_root);
            if let Ok(rel) = norm_simplified.strip_prefix(&norm_root) {
                let forward = to_forward_slash(rel);
                if !forward.starts_with("../") && forward != ".." {
                    return forward.trim_matches('/').to_string();
                }
            }
            let norm_raw_root = normalize_path(&self.root);
            if let Ok(rel) = norm_simplified.strip_prefix(&norm_raw_root) {
                let forward = to_forward_slash(rel);
                if !forward.starts_with("../") && forward != ".." {
                    return forward.trim_matches('/').to_string();
                }
            }
        }

        // Relative path: normalize slashes and trim leading ./ or /
        let forward = to_forward_slash(Path::new(&path_str));
        let trimmed = forward.trim_start_matches("./").trim_matches('/');
        if trimmed == "." {
            String::new()
        } else {
            trimmed.to_string()
        }
    }

    /// Resolve candidate database paths for this workspace:
    /// 1. Explicit override path (if provided)
    /// 2. In-tree `.code-kb/artifact.db` or `.code-kb/store.db`
    pub fn candidate_db_paths(&self, explicit_db: Option<&Path>) -> Vec<PathBuf> {
        let mut candidates = Vec::new();

        if let Some(p) = explicit_db {
            candidates.push(p.to_path_buf());
        }

        // In-tree options
        candidates.push(self.canonical_root.join(".code-kb").join("artifact.db"));
        candidates.push(self.canonical_root.join(".code-kb").join("store.db"));
        candidates.push(self.canonical_root.join("artifact.db"));

        candidates
    }

    /// Finds the first existing database file, or returns the default target location.
    pub fn locate_db(&self, explicit_db: Option<&Path>) -> Result<PathBuf, WorkspaceError> {
        if let Some(p) = explicit_db {
            return Ok(p.to_path_buf());
        }

        let candidates = self.candidate_db_paths(None);
        for candidate in &candidates {
            if candidate.exists() && candidate.is_file() {
                return Ok(candidate.clone());
            }
        }

        Ok(self.canonical_root.join(".code-kb").join("artifact.db"))
    }
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

    #[test]
    fn test_relativize_filter() {
        let temp = tempfile::tempdir().unwrap();
        let ws = Workspace::new(temp.path().to_path_buf());

        // Relative path
        assert_eq!(ws.relativize_filter("."), "");
        assert_eq!(ws.relativize_filter("./"), "");
        assert_eq!(ws.relativize_filter("src/models"), "src/models");
        assert_eq!(ws.relativize_filter("./src/models/"), "src/models");
        assert_eq!(
            ws.relativize_filter(r"src\models\mod.rs"),
            "src/models/mod.rs"
        );

        // Absolute path inside workspace
        let abs_file = temp.path().join("src").join("lib.rs");
        assert_eq!(
            ws.relativize_filter(&abs_file.to_string_lossy()),
            "src/lib.rs"
        );

        // File URI
        let uri = format!("file://{}", abs_file.to_string_lossy().replace('\\', "/"));
        assert_eq!(ws.relativize_filter(&uri), "src/lib.rs");
    }
}
