//! The git subprocess calls of the edit check. Every call names the repository with `-C`, takes no
//! user git configuration that changes the output format, and never writes to the repository.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use super::EditCheckError;

/// New-side line ranges of each changed file, as inclusive `(first, last)` pairs.
pub(super) type Hunks = HashMap<String, Vec<(usize, usize)>>;

/// A file the diff touches. `base_path` is where the file lived at the base revision, `None` for
/// a file the base does not have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ChangedFile {
    pub path: String,
    pub base_path: Option<String>,
    pub untracked: bool,
}

pub(super) struct Git {
    root: PathBuf,
}

impl Git {
    pub(super) fn open(root: &Path) -> Result<Self, EditCheckError> {
        let git = Git {
            root: root.to_path_buf(),
        };
        let inside = git
            .output(&["rev-parse", "--is-inside-work-tree"])
            .ok()
            .filter(|out| out.status.success())
            .is_some_and(|out| String::from_utf8_lossy(&out.stdout).trim() == "true");
        if inside {
            Ok(git)
        } else {
            Err(EditCheckError::NotARepository(root.display().to_string()))
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new("git");
        command
            .args(["--literal-pathspecs", "-c", "core.quotepath=false", "-C"])
            .arg(&self.root)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_PREFIX")
            .stdin(Stdio::null());
        command
    }

    fn output(&self, args: &[&str]) -> Result<Output, EditCheckError> {
        self.command()
            .args(args)
            .output()
            .map_err(|error| EditCheckError::Git(format!("could not run git: {error}")))
    }

    fn stdout(&self, args: &[&str]) -> Result<Vec<u8>, EditCheckError> {
        let out = self.output(args)?;
        if out.status.success() {
            Ok(out.stdout)
        } else {
            Err(EditCheckError::Git(format!(
                "git {} failed: {}",
                args.first().copied().unwrap_or_default(),
                String::from_utf8_lossy(&out.stderr).trim()
            )))
        }
    }

    /// The commit `rev` names, `None` when the repository has no such commit.
    pub(super) fn resolve(&self, rev: &str) -> Result<Option<String>, EditCheckError> {
        let spec = format!("{rev}^{{commit}}");
        let out = self.output(&[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &spec,
        ])?;
        Ok(out
            .status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .filter(|sha| !sha.is_empty()))
    }

    pub(super) fn empty_tree(&self) -> Result<String, EditCheckError> {
        let out = self.stdout(&["hash-object", "-t", "tree", "--stdin"])?;
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    }

    /// The tracked files that differ from `base` in the working tree, with renames paired, then
    /// the untracked files that git does not ignore. Paths are relative to the root.
    pub(super) fn changed_files(&self, base: &str) -> Result<Vec<ChangedFile>, EditCheckError> {
        let status = self.stdout(&[
            "diff",
            "--name-status",
            "-z",
            "-M",
            "--relative",
            "--no-ext-diff",
            "--no-textconv",
            base,
            "--",
        ])?;
        let mut files = parse_name_status(&String::from_utf8_lossy(&status));
        let untracked = self.stdout(&["ls-files", "--others", "--exclude-standard", "-z"])?;
        for path in String::from_utf8_lossy(&untracked).split('\0') {
            if !path.is_empty() && !path.ends_with('/') {
                files.push(ChangedFile {
                    path: path.to_string(),
                    base_path: None,
                    untracked: true,
                });
            }
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        files.dedup_by(|a, b| a.path == b.path);
        Ok(files)
    }

    /// The new-side line ranges of `files`: from `git diff -U0` for a tracked file, the whole file
    /// for an untracked one.
    pub(super) fn new_side_hunks(
        &self,
        base: &str,
        files: &[ChangedFile],
    ) -> Result<Hunks, EditCheckError> {
        let mut hunks = Hunks::new();
        let mut tracked: Vec<&str> = Vec::new();
        for file in files {
            if file.untracked {
                if let Ok(text) = std::fs::read(self.root.join(&file.path)) {
                    let lines = text.iter().filter(|&&byte| byte == b'\n').count()
                        + usize::from(text.last().is_some_and(|&byte| byte != b'\n'));
                    hunks.insert(file.path.clone(), vec![(1, lines.max(1))]);
                }
            } else {
                tracked.push(&file.path);
                if let Some(old) = file.base_path.as_deref().filter(|old| *old != file.path) {
                    tracked.push(old);
                }
            }
        }
        if tracked.is_empty() {
            return Ok(hunks);
        }
        let mut args = vec![
            "diff",
            "-U0",
            "-M",
            "--relative",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            base,
            "--",
        ];
        args.extend(tracked);
        let patch = self.stdout(&args)?;
        hunks.extend(parse_hunks(&String::from_utf8_lossy(&patch)));
        Ok(hunks)
    }

    /// The content of `path` at the commit `base`, `None` when the commit has no such file.
    pub(super) fn base_blob(&self, base: &str, path: &str) -> Option<Vec<u8>> {
        let object = format!("{base}:./{path}");
        self.stdout(&["cat-file", "blob", &object]).ok()
    }
}

fn parse_name_status(text: &str) -> Vec<ChangedFile> {
    let mut tokens = text.split('\0').filter(|token| !token.is_empty());
    let mut files = Vec::new();
    while let Some(status) = tokens.next() {
        let renamed = status.starts_with('R') || status.starts_with('C');
        let Some(first) = tokens.next() else { break };
        let (path, base_path) = if renamed {
            let Some(second) = tokens.next() else { break };
            (second, status.starts_with('R').then_some(first))
        } else if status.starts_with('A') {
            (first, None)
        } else {
            (first, Some(first))
        };
        files.push(ChangedFile {
            path: path.to_string(),
            base_path: base_path.map(str::to_string),
            untracked: false,
        });
    }
    files
}

fn parse_hunks(patch: &str) -> Hunks {
    let mut hunks = Hunks::new();
    let mut current: Option<String> = None;
    let mut in_header = false;
    for line in patch.lines() {
        if line.starts_with("diff --git ") {
            in_header = true;
            current = None;
        } else if in_header && let Some(header) = line.strip_prefix("+++ ") {
            current = new_path(header);
        } else if line.starts_with("@@ ") {
            in_header = false;
            if let (Some(path), Some(range)) = (&current, new_range(line)) {
                hunks.entry(path.clone()).or_default().push(range);
            }
        }
    }
    hunks
}

fn new_path(header: &str) -> Option<String> {
    let header = header.trim_end_matches('\t');
    if header == "/dev/null" {
        return None;
    }
    let path = if header.starts_with('"') {
        unquote(header)?
    } else {
        header.to_string()
    };
    path.strip_prefix("b/").map(str::to_string)
}

/// The text of a path that git quoted with C escapes (`"a\"b"`, `"caf\303\251"`).
fn unquote(quoted: &str) -> Option<String> {
    let inner = quoted.strip_prefix('"')?.strip_suffix('"')?;
    let bytes = inner.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'\\' {
            out.push(bytes[index]);
            index += 1;
            continue;
        }
        index += 1;
        let escaped = *bytes.get(index)?;
        match escaped {
            b'0'..=b'7' => {
                let digits = &inner[index..(index + 3).min(inner.len())];
                out.push(u8::from_str_radix(digits, 8).ok()?);
                index += 3;
                continue;
            }
            b't' => out.push(b'\t'),
            b'n' => out.push(b'\n'),
            other => out.push(other),
        }
        index += 1;
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// The new-side range of a hunk header `@@ -a,b +c,d @@`, `None` for a hunk that adds no line.
fn new_range(header: &str) -> Option<(usize, usize)> {
    let new_side = header.split_whitespace().nth(2)?.strip_prefix('+')?;
    let (start, count) = match new_side.split_once(',') {
        Some((start, count)) => (start.parse::<usize>().ok()?, count.parse::<usize>().ok()?),
        None => (new_side.parse::<usize>().ok()?, 1),
    };
    (count > 0).then(|| (start, start + count - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hunk_header_gives_the_new_side_range() {
        assert_eq!(new_range("@@ -3,2 +5,4 @@ fn x()"), Some((5, 8)));
        assert_eq!(new_range("@@ -3 +5 @@"), Some((5, 5)));
        assert_eq!(new_range("@@ -3,2 +2,0 @@"), None);
    }

    #[test]
    fn name_status_pairs_renames_and_marks_additions() {
        let files = parse_name_status("M\0a.rs\0A\0b.rs\0R100\0old.rs\0new.rs\0D\0gone.rs\0");
        let summary: Vec<(&str, Option<&str>)> = files
            .iter()
            .map(|f| (f.path.as_str(), f.base_path.as_deref()))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("a.rs", Some("a.rs")),
                ("b.rs", None),
                ("new.rs", Some("old.rs")),
                ("gone.rs", Some("gone.rs")),
            ]
        );
    }

    #[test]
    fn hunks_are_keyed_by_the_new_path_and_content_lines_are_not_headers() {
        let patch = "diff --git a/x.sql b/x.sql\n--- a/x.sql\n+++ b/x.sql\t\n@@ -1,2 +1,2 @@\n--- old comment\n+++ new comment\n@@ -9,0 +10,3 @@\n+a\n+b\n+c\ndiff --git a/gone.rs b/gone.rs\n--- a/gone.rs\n+++ /dev/null\n@@ -1,3 +0,0 @@\n-x\n";
        let hunks = parse_hunks(patch);
        assert_eq!(hunks.get("x.sql"), Some(&vec![(1, 2), (10, 12)]));
        assert!(!hunks.contains_key("gone.rs"));
    }

    #[test]
    fn a_quoted_path_is_unescaped() {
        assert_eq!(unquote(r#""a\"b""#).as_deref(), Some("a\"b"));
        assert_eq!(unquote(r#""caf\303\251""#).as_deref(), Some("café"));
        assert_eq!(
            new_path(r#""b/with \"quote\".rs""#).as_deref(),
            Some("with \"quote\".rs")
        );
    }

    #[test]
    fn a_path_with_spaces_keeps_its_name_without_the_trailing_tab() {
        assert_eq!(
            new_path("b/my pkg/a b.py\t").as_deref(),
            Some("my pkg/a b.py")
        );
    }
}
