use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::Path;
use thiserror::Error;

use crate::models::Symbol;
use crate::queries;
use crate::slicer;
use crate::sync;
use crate::syntax::{self, SyntaxError};
use crate::workspace::Workspace;

#[derive(Debug, Error)]
pub enum EditError {
    #[error("Symbol '{name}' not found in {workspace}. {hint}")]
    SymbolNotFound {
        name: String,
        workspace: String,
        hint: String,
    },
    #[error("File '{path}' not found in {workspace}. {hint}")]
    FileNotFound {
        path: String,
        workspace: String,
        hint: String,
    },
    #[error("Symbol has no body defined (e.g. trait declaration without default implementation)")]
    NoBodyDefined,
    #[error("Optimistic lock failed: expected body hash '{0}', found '{1}'")]
    HashMismatch(String, String),
    #[error(
        "File offsets out of bounds: range [{0}..{1}], but file length is {2} bytes (file may have shrunk or changed)"
    )]
    InvalidOffsetRange(usize, usize, usize),
    #[error("Pre-flight syntax validation failed: {0}")]
    Syntax(#[from] SyntaxError),
    #[error("Replacement content is not valid UTF-8: {0}")]
    InvalidUtf8(String),
    #[error("Failed to read/write file '{0}': {1}")]
    Io(String, #[source] std::io::Error),
    #[error("Synchronization failed and file was rolled back: {0}")]
    SyncWithRollback(String),
    #[error(
        "Synchronization failed after edit ({sync_error}) and rollback also failed: {rollback_error}"
    )]
    SyncRollbackFailed {
        sync_error: String,
        rollback_error: String,
    },
    #[error("File '{0}' was concurrently modified; edit aborted")]
    ConcurrentModification(String),
    #[error("Synchronization failed after edit: {0}")]
    Sync(#[from] sync::SyncError),
    #[error("Workspace path error: {0}")]
    Workspace(#[from] crate::workspace::WorkspaceError),
    #[error("Query error: {0}")]
    Query(#[from] queries::QueryError),
    #[error("The file '{0}' has no match for old_text. {1}")]
    NoMatch(String, String),
    #[error(
        "The file '{path}' has more than one match for old_text, at lines {lines}. Pass occurrence, or add more context lines.",
        path = .0,
        lines = .1.iter().map(usize::to_string).collect::<Vec<_>>().join(", ")
    )]
    AmbiguousMatch(String, Vec<usize>),
    #[error("old_text is empty. Give the text to replace.")]
    EmptyOldText,
    #[error("old_text and new_text are the same. The file needs no edit.")]
    NoChange,
    #[error("The file is {0} bytes. edit_file reads files up to 8388608 bytes.")]
    FileTooLarge(usize),
    #[error("The path '{0}' is not a file.")]
    NotAFile(String),
}

/// Result of an atomic symbol body replacement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditResult {
    pub symbol_name: String,
    pub file_path: String,
    pub old_body_hash: String,
    pub new_body_hash: String,
    pub bytes_written: usize,
    pub syntax_checked: bool,
}

fn symbol_not_found(conn: &Connection, name: &str, path_filter: Option<&str>) -> EditError {
    let (workspace, hint) = queries::symbol_not_found_parts(conn, name, path_filter);
    EditError::SymbolNotFound {
        name: name.to_string(),
        workspace,
        hint,
    }
}

fn file_not_found(conn: &Connection, rel_path: &str) -> EditError {
    let (workspace, hint) = queries::file_not_found_parts(conn, rel_path);
    EditError::FileNotFound {
        path: rel_path.to_string(),
        workspace,
        hint,
    }
}

/// Computes SHA256 of a string content.
pub fn hash_content(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

fn is_transient_lock_error(err: &std::io::Error) -> bool {
    #[cfg(windows)]
    {
        if let Some(code) = err.raw_os_error() {
            // ERROR_ACCESS_DENIED = 5, ERROR_SHARING_VIOLATION = 32, ERROR_LOCK_VIOLATION = 33
            if code == 5 || code == 32 || code == 33 {
                return true;
            }
        }
    }
    matches!(err.kind(), std::io::ErrorKind::PermissionDenied)
}

fn persist_with_retry(
    mut temp_file: tempfile::NamedTempFile,
    dest: &Path,
    expected_dest_bytes: Option<&[u8]>,
) -> Result<(), std::io::Error> {
    for attempt in 0..5 {
        if attempt > 0
            && let Some(expected) = expected_dest_bytes
            && let Ok(current) = fs::read(dest)
            && current != expected
        {
            return Err(std::io::Error::other(
                "Destination file was concurrently modified during retry",
            ));
        }
        match temp_file.persist(dest) {
            Ok(_) => return Ok(()),
            Err(e) => {
                let is_transient = is_transient_lock_error(&e.error);
                temp_file = e.file;
                if is_transient && attempt < 4 {
                    std::thread::sleep(std::time::Duration::from_millis(10 * (1 << attempt)));
                    continue;
                }
                return Err(e.error);
            }
        }
    }
    unreachable!()
}

/// Validates, writes, and re-indexes `new_file_bytes` as the whole content of `abs_path`.
/// Returns whether the extractor checked the syntax. Rolls the file back when re-indexing fails.
fn commit_file_edit(
    workspace: &Workspace,
    db_path: &Path,
    abs_path: &Path,
    rel_path: &str,
    existing_bytes: &[u8],
    existing_permissions: &fs::Permissions,
    new_file_bytes: &[u8],
) -> Result<bool, EditError> {
    let new_file_str =
        std::str::from_utf8(new_file_bytes).map_err(|e| EditError::InvalidUtf8(e.to_string()))?;
    let syntax_checked = syntax::validate_syntax(rel_path, new_file_str)?;

    let current_disk =
        fs::read(abs_path).map_err(|e| EditError::Io(abs_path.display().to_string(), e))?;
    if current_disk != existing_bytes {
        return Err(EditError::ConcurrentModification(rel_path.to_string()));
    }

    let target_dir = abs_path.parent().unwrap_or(Path::new("."));
    let mut temp_file = tempfile::Builder::new()
        .prefix(".code-kb-edit-")
        .suffix(".tmp")
        .tempfile_in(target_dir)
        .map_err(|e| EditError::Io(abs_path.display().to_string(), e))?;

    temp_file
        .write_all(new_file_bytes)
        .map_err(|e| EditError::Io(abs_path.display().to_string(), e))?;
    temp_file
        .flush()
        .map_err(|e| EditError::Io(abs_path.display().to_string(), e))?;

    let _ = temp_file
        .as_file()
        .set_permissions(existing_permissions.clone());

    persist_with_retry(temp_file, abs_path, Some(existing_bytes)).map_err(|e| {
        if e.to_string().contains("concurrently modified") {
            EditError::ConcurrentModification(rel_path.to_string())
        } else {
            EditError::Io(abs_path.display().to_string(), e)
        }
    })?;

    if let Err(err) = sync::update_file(workspace, db_path, rel_path) {
        let disk_post_write = fs::read(abs_path);
        if disk_post_write.as_deref().ok() != Some(new_file_bytes) {
            return Err(EditError::ConcurrentModification(format!(
                "File was concurrently modified during re-indexing; rollback aborted: {err}"
            )));
        }

        let rollback_res = (|| -> Result<(), std::io::Error> {
            let mut rollback_tmp = tempfile::Builder::new()
                .prefix(".code-kb-rollback-")
                .suffix(".tmp")
                .tempfile_in(target_dir)?;
            rollback_tmp.write_all(existing_bytes)?;
            rollback_tmp.flush()?;
            let _ = rollback_tmp
                .as_file()
                .set_permissions(existing_permissions.clone());
            persist_with_retry(rollback_tmp, abs_path, Some(new_file_bytes))?;
            Ok(())
        })();

        return match rollback_res {
            Ok(()) => Err(EditError::SyncWithRollback(err.to_string())),
            Err(rollback_err) => Err(EditError::SyncRollbackFailed {
                sync_error: err.to_string(),
                rollback_error: rollback_err.to_string(),
            }),
        };
    }

    Ok(syntax_checked)
}

/// Atomically replaces the implementation body of a symbol by name.
pub fn replace_symbol_body(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
    symbol_name: &str,
    file_path: &str,
    new_body: &str,
    expected_body_hash: Option<&str>,
) -> Result<EditResult, EditError> {
    let (abs_path, rel_path) = workspace.resolve_path(Path::new(file_path))?;
    if !abs_path.exists() {
        return Err(file_not_found(conn, &rel_path));
    }

    // Tier 2: Refresh file in index before querying symbol offsets, propagating any sync errors
    sync::ensure_fresh_file(workspace, db_path, conn, &rel_path)?;

    // Find symbol in database with exact path
    let symbol = queries::get_symbol_by_name_exact(conn, symbol_name, &rel_path)?
        .ok_or_else(|| symbol_not_found(conn, symbol_name, Some(&rel_path)))?;

    let body_start = symbol.body_start_byte.ok_or(EditError::NoBodyDefined)?;
    let body_end = symbol.body_end_byte.ok_or(EditError::NoBodyDefined)?;

    // Read existing file metadata (permissions) and content
    let existing_metadata =
        fs::metadata(&abs_path).map_err(|e| EditError::Io(abs_path.display().to_string(), e))?;
    let existing_permissions = existing_metadata.permissions();
    let existing_bytes =
        fs::read(&abs_path).map_err(|e| EditError::Io(abs_path.display().to_string(), e))?;

    // Bounds check to prevent out-of-bounds panics
    if body_start > body_end || body_end > existing_bytes.len() {
        return Err(EditError::InvalidOffsetRange(
            body_start,
            body_end,
            existing_bytes.len(),
        ));
    }

    let existing_body =
        slicer::slice_bytes_safe(&existing_bytes, body_start, body_end).map_err(|e| {
            EditError::Io(
                abs_path.display().to_string(),
                std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()),
            )
        })?;

    let current_sha256 = hash_content(existing_body);

    // Verify optimistic lock if caller specified expected_body_hash
    if let Some(expected) = expected_body_hash
        && expected != current_sha256
    {
        return Err(EditError::HashMismatch(
            expected.to_string(),
            current_sha256,
        ));
    }

    // Match file line endings (CRLF vs LF)
    let is_crlf = existing_bytes.windows(2).any(|w| w == b"\r\n");
    let normalized_body = if is_crlf {
        // Uniformly normalize all line endings to CRLF, including mixed inputs
        let lf_only = new_body.replace("\r\n", "\n");
        lf_only.replace('\n', "\r\n")
    } else {
        // Uniformly normalize all line endings to LF, including mixed inputs
        new_body.replace("\r\n", "\n")
    };

    // Construct new file content with replaced byte span
    let mut new_file_bytes = Vec::with_capacity(existing_bytes.len() + normalized_body.len());
    new_file_bytes.extend_from_slice(&existing_bytes[..body_start]);
    new_file_bytes.extend_from_slice(normalized_body.as_bytes());
    new_file_bytes.extend_from_slice(&existing_bytes[body_end..]);

    let syntax_checked = commit_file_edit(
        workspace,
        db_path,
        &abs_path,
        &rel_path,
        &existing_bytes,
        &existing_permissions,
        &new_file_bytes,
    )?;

    let new_body_hash = hash_content(&normalized_body);

    Ok(EditResult {
        symbol_name: symbol_name.to_string(),
        file_path: rel_path,
        old_body_hash: current_sha256,
        new_body_hash,
        bytes_written: new_file_bytes.len(),
        syntax_checked,
    })
}

/// Largest file `edit_file` reads, in bytes.
const MAX_EDIT_FILE_BYTES: usize = 8 * 1024 * 1024;

/// Which match `edit_file` replaces when `old_text` occurs more than once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Occurrence {
    /// Replace the only match, and refuse when there is more than one.
    #[default]
    Only,
    First,
    Last,
    All,
}

/// How `edit_file` found `old_text` in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchTier {
    /// The bytes of `old_text` are in the file.
    Exact,
    /// The lines of `old_text` are in the file, with other indentation.
    Whitespace,
}

/// Result of an atomic text edit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextEditResult {
    pub file_path: String,
    pub replacements: usize,
    pub first_line: usize,
    pub match_tier: MatchTier,
    pub touched_symbols: Vec<String>,
    pub syntax_checked: bool,
    pub bytes_written: usize,
}

struct TextMatch {
    start: usize,
    end: usize,
    replacement: String,
}

fn leading_whitespace(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset].matches('\n').count() + 1
}

fn exact_matches(haystack: &str, old_text: &str, new_text: &str) -> Vec<TextMatch> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(offset) = haystack[from..].find(old_text) {
        let start = from + offset;
        let end = start + old_text.len();
        found.push(TextMatch {
            start,
            end,
            replacement: new_text.to_string(),
        });
        from = end;
    }
    found
}

fn reindent(new_text: &str, old_indent: &str, file_indent: &str) -> String {
    new_text
        .lines()
        .map(|line| match line.strip_prefix(old_indent) {
            Some(rest) if !line.trim().is_empty() => format!("{file_indent}{rest}"),
            _ => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Matches the lines of `old_text` against whole file lines, with both ends of each line trimmed,
/// and gives the replacement the indentation of the first matched file line.
fn whitespace_matches(haystack: &str, old_text: &str, new_text: &str) -> Vec<TextMatch> {
    let old_lines: Vec<&str> = old_text.lines().collect();
    let file_lines: Vec<&str> = haystack.lines().collect();
    if old_lines.is_empty() || file_lines.len() < old_lines.len() {
        return Vec::new();
    }
    let mut line_starts = vec![0usize];
    line_starts.extend(haystack.match_indices('\n').map(|(at, _)| at + 1));

    let old_indent = leading_whitespace(old_lines[0]);
    let mut found = Vec::new();
    let mut first = 0;
    while first + old_lines.len() <= file_lines.len() {
        let same = (0..old_lines.len())
            .all(|step| file_lines[first + step].trim() == old_lines[step].trim());
        if !same {
            first += 1;
            continue;
        }
        let last = first + old_lines.len() - 1;
        found.push(TextMatch {
            start: line_starts[first],
            end: line_starts[last] + file_lines[last].len(),
            replacement: reindent(new_text, old_indent, leading_whitespace(file_lines[first])),
        });
        first = last + 1;
    }
    found
}

fn common_prefix_len(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(l, r)| l == r)
        .count()
}

/// The three file lines that share the longest opening with `probe`, so the caller sees
/// what the file holds where the match failed.
fn nearest_lines(haystack: &str, probe: &str) -> String {
    let probe = probe.trim();
    let mut scored: Vec<(usize, usize, &str)> = haystack
        .lines()
        .enumerate()
        .map(|(index, line)| (common_prefix_len(line.trim(), probe), index + 1, line))
        .filter(|(score, _, _)| *score > 0)
        .collect();
    if scored.is_empty() {
        return "No line in the file is similar.".to_string();
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.truncate(3);
    scored.sort_by_key(|(_, line, _)| *line);
    let mut report = String::from("The nearest lines are:");
    for (_, line, text) in scored {
        report.push_str(&format!("\n{line}: {}", text.trim_end()));
    }
    report
}

fn innermost_symbol_names(symbols: &[Symbol], ranges: &[(usize, usize)]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for (first, last) in ranges {
        let enclosing = symbols
            .iter()
            .filter(|symbol| symbol.start_line <= *first && symbol.end_line >= *last);
        let definitions: Vec<&Symbol> = enclosing
            .clone()
            .filter(|symbol| is_definition(symbol))
            .collect();
        let candidates = if definitions.is_empty() {
            enclosing.collect()
        } else {
            definitions
        };
        let best = candidates
            .into_iter()
            .min_by_key(|symbol| symbol.end_line - symbol.start_line);
        if let Some(symbol) = best
            && !names.contains(&symbol.name)
        {
            names.push(symbol.name.clone());
        }
    }
    names
}

fn is_definition(symbol: &Symbol) -> bool {
    queries::DEFINITION_KINDS.contains(&queries::normalize_kind(&symbol.kind).as_str())
}

/// Atomically replaces `old_text` with `new_text` in one file.
///
/// The file is read from disk, so the caller needs no index of its content. An exact match wins;
/// when there is none, the lines of `old_text` are matched with their indentation ignored.
pub fn edit_file(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
    file_path: &str,
    old_text: &str,
    new_text: &str,
    occurrence: Occurrence,
) -> Result<TextEditResult, EditError> {
    if old_text.is_empty() {
        return Err(EditError::EmptyOldText);
    }
    let (abs_path, rel_path) = workspace.resolve_path(Path::new(file_path))?;
    if !abs_path.exists() {
        return Err(file_not_found(conn, &rel_path));
    }
    if !abs_path.is_file() {
        return Err(EditError::NotAFile(rel_path));
    }
    let metadata =
        fs::metadata(&abs_path).map_err(|e| EditError::Io(abs_path.display().to_string(), e))?;
    if metadata.len() as usize > MAX_EDIT_FILE_BYTES {
        return Err(EditError::FileTooLarge(metadata.len() as usize));
    }
    let existing_permissions = metadata.permissions();

    sync::ensure_fresh_file(workspace, db_path, conn, &rel_path)?;

    let existing_bytes =
        fs::read(&abs_path).map_err(|e| EditError::Io(abs_path.display().to_string(), e))?;
    let existing_text =
        std::str::from_utf8(&existing_bytes).map_err(|e| EditError::InvalidUtf8(e.to_string()))?;

    let is_crlf = existing_bytes.windows(2).any(|w| w == b"\r\n");
    let file_text = existing_text.replace("\r\n", "\n");
    let old_text = old_text.replace("\r\n", "\n");
    let new_text = new_text.replace("\r\n", "\n");
    if old_text == new_text {
        return Err(EditError::NoChange);
    }

    let exact = exact_matches(&file_text, &old_text, &new_text);
    let (match_tier, matches) = if exact.is_empty() {
        (
            MatchTier::Whitespace,
            whitespace_matches(&file_text, &old_text, &new_text),
        )
    } else {
        (MatchTier::Exact, exact)
    };

    let selected: Vec<TextMatch> = match occurrence {
        Occurrence::Only if matches.len() > 1 => {
            return Err(EditError::AmbiguousMatch(
                rel_path,
                matches
                    .iter()
                    .map(|found| line_of(&file_text, found.start))
                    .collect(),
            ));
        }
        Occurrence::Only | Occurrence::All => matches,
        Occurrence::First => matches.into_iter().take(1).collect(),
        Occurrence::Last => matches.into_iter().next_back().into_iter().collect(),
    };
    if selected.is_empty() {
        let probe = old_text.lines().next().unwrap_or(&old_text);
        return Err(EditError::NoMatch(
            rel_path,
            nearest_lines(&file_text, probe),
        ));
    }

    let mut edited = String::with_capacity(file_text.len() + new_text.len());
    let mut cursor = 0;
    let mut newlines = 0;
    let mut edited_ranges: Vec<(usize, usize)> = Vec::new();
    for found in &selected {
        let gap = &file_text[cursor..found.start];
        edited.push_str(gap);
        newlines += gap.matches('\n').count();
        let first_line = newlines + 1;
        edited.push_str(&found.replacement);
        newlines += found.replacement.matches('\n').count();
        let last_line = if found.replacement.ends_with('\n') {
            newlines.max(first_line)
        } else {
            newlines + 1
        };
        edited_ranges.push((first_line, last_line));
        cursor = found.end;
    }
    edited.push_str(&file_text[cursor..]);

    let new_file_bytes = if is_crlf {
        edited.replace('\n', "\r\n").into_bytes()
    } else {
        edited.into_bytes()
    };

    let syntax_checked = commit_file_edit(
        workspace,
        db_path,
        &abs_path,
        &rel_path,
        &existing_bytes,
        &existing_permissions,
        &new_file_bytes,
    )?;

    let symbols = queries::load_file_symbols(conn, &rel_path)?;

    Ok(TextEditResult {
        file_path: rel_path,
        replacements: selected.len(),
        first_line: edited_ranges[0].0,
        match_tier,
        touched_symbols: innermost_symbol_names(&symbols, &edited_ranges),
        syntax_checked,
        bytes_written: new_file_bytes.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mixed_crlf_lf_normalization() {
        let mixed_input = "line1\r\nline2\nline3\r\nline4\n";
        // When file is CRLF:
        let lf_only = mixed_input.replace("\r\n", "\n");
        let crlf_normalized = lf_only.replace('\n', "\r\n");
        assert_eq!(crlf_normalized, "line1\r\nline2\r\nline3\r\nline4\r\n");

        // When file is LF:
        let lf_normalized = mixed_input.replace("\r\n", "\n");
        assert_eq!(lf_normalized, "line1\nline2\nline3\nline4\n");
    }

    #[test]
    fn test_persist_with_retry_succeeds() {
        let dir = crate::safe_tempdir();
        let target_file = dir.path().join("test_persist.txt");
        fs::write(&target_file, "initial").unwrap();

        let mut temp_file = tempfile::Builder::new()
            .prefix(".test-persist-")
            .suffix(".tmp")
            .tempfile_in(dir.path())
            .unwrap();
        temp_file.write_all(b"updated").unwrap();
        temp_file.flush().unwrap();

        persist_with_retry(temp_file, &target_file, Some(b"initial"))
            .expect("persist_with_retry must succeed");
        assert_eq!(fs::read_to_string(&target_file).unwrap(), "updated");
    }
}
