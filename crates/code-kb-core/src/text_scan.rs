use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use ignore::{DirEntry, WalkBuilder, WalkState};
use memchr::{memchr, memchr_iter, memmem};

use crate::workspace::{is_hard_excluded, to_forward_slash};

const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const BINARY_SNIFF_BYTES: u64 = 8 * 1024;
const MAX_THREADS: usize = 8;

/// Lines of a project that hold a word, with the coverage of the scan.
///
/// A binary file counts as scanned: the scan read it and decided to skip it.
/// A file over the size cap never reaches the count: the walker drops it.
#[derive(Debug, Clone)]
pub struct WordHits {
    /// `(relative path with '/', 1-based line)`, sorted, one entry per line.
    pub hits: Vec<(String, usize)>,
    pub files_scanned: usize,
    /// Files the walk found, including the ones the budget left unread.
    pub files_total: usize,
    /// False when the budget cut the scan.
    pub complete: bool,
    pub elapsed: Duration,
}

/// Finds every line of the project under `root` that holds `word` as a whole word.
///
/// Word characters are ASCII letters, digits, `_`, and `$`. The scan is case-sensitive and
/// follows the same ignore rules as the indexer. Once `budget` is spent, the walk goes on
/// to count files but reads none.
pub fn scan_word(root: &Path, word: &str, budget: Duration) -> WordHits {
    let started = Instant::now();
    if word.is_empty() {
        return WordHits {
            hits: Vec::new(),
            files_scanned: 0,
            files_total: 0,
            complete: true,
            elapsed: started.elapsed(),
        };
    }

    let finder = &memmem::Finder::new(word.as_bytes());
    let hits = &Mutex::new(Vec::new());
    let files_total = &AtomicUsize::new(0);
    let files_scanned = &AtomicUsize::new(0);

    WalkBuilder::new(root)
        .standard_filters(true)
        .hidden(false)
        .max_filesize(Some(MAX_FILE_BYTES))
        .threads(
            std::thread::available_parallelism()
                .map_or(1, usize::from)
                .min(MAX_THREADS),
        )
        .add_custom_ignore_filename(".julieignore")
        .add_custom_ignore_filename(".code-kb-ignore")
        .add_custom_ignore_filename(".codekbignore")
        .filter_entry(|entry| {
            !is_hard_excluded(&entry.file_name().to_string_lossy()) && !is_nested_repository(entry)
        })
        .build_parallel()
        .run(|| {
            let mut buf = Vec::new();
            Box::new(move |entry| {
                let Ok(entry) = entry else {
                    return WalkState::Continue;
                };
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    return WalkState::Continue;
                }
                files_total.fetch_add(1, Ordering::Relaxed);
                if started.elapsed() >= budget {
                    return WalkState::Continue;
                }
                files_scanned.fetch_add(1, Ordering::Relaxed);
                let Ok(rel) = entry.path().strip_prefix(root) else {
                    return WalkState::Continue;
                };
                if !read_text(entry.path(), &mut buf).unwrap_or(false) {
                    return WalkState::Continue;
                }
                let lines = word_lines(&buf, finder);
                if !lines.is_empty() {
                    let rel = to_forward_slash(rel);
                    hits.lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .extend(lines.into_iter().map(|line| (rel.clone(), line)));
                }
                WalkState::Continue
            })
        });

    let mut hits = hits
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .split_off(0);
    hits.sort_unstable();
    let files_total = files_total.load(Ordering::Relaxed);
    let files_scanned = files_scanned.load(Ordering::Relaxed);
    WordHits {
        hits,
        files_scanned,
        files_total,
        complete: files_scanned == files_total,
        elapsed: started.elapsed(),
    }
}

fn is_nested_repository(entry: &DirEntry) -> bool {
    entry.file_type().is_some_and(|t| t.is_dir()) && entry.path().join(".git").exists()
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

/// Fills `buf` with the file, or returns `false` when a NUL byte in the first 8 KiB marks it binary.
fn read_text(path: &Path, buf: &mut Vec<u8>) -> std::io::Result<bool> {
    buf.clear();
    let mut file = File::open(path)?;
    (&mut file).take(BINARY_SNIFF_BYTES).read_to_end(buf)?;
    if memchr(0, buf).is_some() {
        return Ok(false);
    }
    file.take(MAX_FILE_BYTES).read_to_end(buf)?;
    Ok(true)
}

fn word_lines(buf: &[u8], finder: &memmem::Finder<'_>) -> Vec<usize> {
    let word_len = finder.needle().len();
    let mut lines = Vec::new();
    let mut line = 1;
    let mut counted_to = 0;
    for start in finder.find_iter(buf) {
        let bounded = (start == 0 || !is_word_byte(buf[start - 1]))
            && buf.get(start + word_len).is_none_or(|&b| !is_word_byte(b));
        if !bounded {
            continue;
        }
        line += memchr_iter(b'\n', &buf[counted_to..start]).count();
        counted_to = start;
        if lines.last() != Some(&line) {
            lines.push(line);
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    const AMPLE: Duration = Duration::from_secs(60);
    const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;

    fn tree(files: &[(&str, &[u8])]) -> TempDir {
        let dir = crate::safe_tempdir();
        fs::create_dir(dir.path().join(".git")).unwrap();
        for (rel, content) in files {
            let path = dir.path().join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
        dir
    }

    fn hits(dir: &TempDir, word: &str) -> Vec<(String, usize)> {
        let found = scan_word(dir.path(), word, AMPLE);
        assert!(found.complete);
        found.hits
    }

    fn at(path: &str, lines: &[usize]) -> Vec<(String, usize)> {
        lines.iter().map(|&l| (path.to_string(), l)).collect()
    }

    #[test]
    fn matches_only_whole_words() {
        let dir = tree(&[(
            "a.txt",
            b"get(x)\nobj.get\ngetter\n_get\n$get\nget_it\na get b\nget1\n1get\n",
        )]);
        assert_eq!(hits(&dir, "get"), at("a.txt", &[1, 2, 7]));
    }

    #[test]
    fn counts_lines_in_crlf_files() {
        let dir = tree(&[("a.txt", b"alpha\r\nneedle\r\nbeta\r\nneedle\r\n")]);
        assert_eq!(hits(&dir, "needle"), at("a.txt", &[2, 4]));
    }

    #[test]
    fn finds_a_hit_on_the_last_line_without_a_newline() {
        let dir = tree(&[("a.txt", b"one\ntwo\nneedle")]);
        assert_eq!(hits(&dir, "needle"), at("a.txt", &[3]));
    }

    #[test]
    fn gives_one_entry_for_two_hits_on_a_line() {
        let dir = tree(&[("a.txt", b"needle needle\nother\nneedle(needle)\n")]);
        assert_eq!(hits(&dir, "needle"), at("a.txt", &[1, 3]));
    }

    #[test]
    fn follows_gitignore_and_custom_ignore_files() {
        let dir = tree(&[
            (".gitignore", b"git_ignored.txt\n"),
            (".julieignore", b"julie_ignored.txt\n"),
            (".code-kb-ignore", b"kb_ignored.txt\n"),
            (".codekbignore", b"kb_alias_ignored.txt\n"),
            ("git_ignored.txt", b"needle\n"),
            ("julie_ignored.txt", b"needle\n"),
            ("kb_ignored.txt", b"needle\n"),
            ("kb_alias_ignored.txt", b"needle\n"),
            ("kept.txt", b"needle\n"),
        ]);
        assert_eq!(hits(&dir, "needle"), at("kept.txt", &[1]));
    }

    #[test]
    fn skips_excluded_folders() {
        let dir = tree(&[
            ("node_modules/pkg/index.js", b"needle\n"),
            ("src/index.js", b"needle\n"),
        ]);
        assert_eq!(hits(&dir, "needle"), at("src/index.js", &[1]));
    }

    #[test]
    fn skips_nested_repositories_and_worktrees() {
        let dir = tree(&[
            ("inner/.git/HEAD", b"ref\n"),
            ("inner/lib.txt", b"needle\n"),
            ("linked/.git", b"gitdir: elsewhere\n"),
            ("linked/lib.txt", b"needle\n"),
            ("outer.txt", b"needle\n"),
        ]);
        assert_eq!(hits(&dir, "needle"), at("outer.txt", &[1]));
    }

    #[test]
    fn scans_a_root_whose_own_name_is_excluded() {
        let parent = crate::safe_tempdir();
        let root = parent.path().join("build");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), "needle\n").unwrap();
        let found = scan_word(&root, "needle", AMPLE);
        assert_eq!(found.hits, at("a.txt", &[1]));
    }

    #[test]
    fn skips_binary_files_by_nul_in_the_first_8_kib() {
        let mut late_nul = b"needle\n".to_vec();
        late_nul.extend(vec![b'x'; 9000]);
        late_nul.push(0);
        let dir = tree(&[
            ("early.dat", b"abc\0\nneedle\n"),
            ("late.dat", late_nul.as_slice()),
        ]);
        assert_eq!(hits(&dir, "needle"), at("late.dat", &[1]));
    }

    #[test]
    fn skips_files_over_2_mb() {
        let padding = |extra: usize| {
            let mut content = b"needle\n".to_vec();
            content.resize(MAX_FILE_BYTES + extra, b'x');
            content
        };
        let dir = tree(&[
            ("at_cap.txt", padding(0).as_slice()),
            ("over_cap.txt", padding(1).as_slice()),
        ]);
        assert_eq!(hits(&dir, "needle"), at("at_cap.txt", &[1]));
    }

    #[test]
    fn a_zero_budget_reads_no_file_and_reports_the_cut() {
        let dir = tree(&[
            ("a.txt", b"needle\n"),
            ("b.txt", b"needle\n"),
            ("c/d.txt", b"needle\n"),
        ]);
        let found = scan_word(dir.path(), "needle", Duration::ZERO);
        assert!(!found.complete);
        assert_eq!(found.files_total, 3);
        assert_eq!(found.files_scanned, 0);
        assert!(found.hits.is_empty());
    }

    #[test]
    fn a_full_scan_is_complete_with_sorted_slash_paths() {
        let dir = tree(&[
            ("b.txt", b"x\nneedle\n"),
            ("a/b/deep.txt", b"needle\nneedle\nx\nneedle\n"),
            ("a/top.txt", b"needle\n"),
        ]);
        let found = scan_word(dir.path(), "needle", AMPLE);
        assert!(found.complete);
        assert_eq!(found.files_scanned, 3);
        assert_eq!(found.files_total, 3);
        let mut expected = at("a/b/deep.txt", &[1, 2, 4]);
        expected.extend(at("a/top.txt", &[1]));
        expected.extend(at("b.txt", &[2]));
        assert_eq!(found.hits, expected);
    }

    #[test]
    fn an_empty_word_matches_nothing() {
        let dir = tree(&[("a.txt", b"needle\n")]);
        let found = scan_word(dir.path(), "", AMPLE);
        assert!(found.hits.is_empty());
        assert!(found.complete);
    }
}
