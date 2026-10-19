//! Per-session state of the post-edit check, one file per session at
//! `<root>/.code-kb/sessions/{session_id}.json`: the commit the session started at, the diff the
//! last hook check ran on, and the diff a Stop hook blocked on. A `{session_id}.lock` file next to
//! it serializes the hooks of one session.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

const LATEST_MAX_AGE: Duration = Duration::from_secs(24 * 3600);
const PRUNE_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
const MAX_ID_LEN: usize = 128;
const LOCK_STALE_AGE: Duration = Duration::from_secs(60);
const LOCK_POLL: Duration = Duration::from_millis(25);

/// What the hooks remember about one session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_diff: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_diff: Option<String>,
}

impl SessionState {
    /// True when a hook of this session already checked the diff `hash` or blocked on it.
    pub fn has_seen(&self, hash: &str) -> bool {
        [&self.checked_diff, &self.blocked_diff]
            .into_iter()
            .any(|seen| seen.as_deref() == Some(hash))
    }
}

/// True for a session id that is safe in a file name: letters, digits, `_` and `-` only.
pub fn is_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn sessions_dir(root: &Path) -> PathBuf {
    root.join(".code-kb").join("sessions")
}

/// Creates `<root>/.code-kb/sessions` and, like an index does, the `.gitignore` that hides
/// `.code-kb`, so a project with no index yet stays out of its own commits.
fn create_sessions_dir(root: &Path) -> io::Result<()> {
    crate::sync::ensure_index_dir(&root.join(".code-kb").join("artifact.db"))?;
    fs::create_dir_all(sessions_dir(root))
}

fn session_file(root: &Path, id: &str) -> io::Result<PathBuf> {
    if is_session_id(id) {
        Ok(sessions_dir(root).join(format!("{id}.json")))
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a session id holds only letters, digits, `_` and `-`",
        ))
    }
}

fn parse(path: &Path) -> Option<SessionState> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

pub fn read_session(root: &Path, id: &str) -> Option<SessionState> {
    parse(&session_file(root, id).ok()?)
}

pub fn write_session(root: &Path, id: &str, state: &SessionState) -> io::Result<()> {
    let path = session_file(root, id)?;
    create_sessions_dir(root)?;
    let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    fs::write(&temp, serde_json::to_string(state)?)?;
    fs::rename(&temp, &path).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })
}

/// Records `head` as the base of a session whose file has none, and keeps the other fields of the
/// file. A session that has a base keeps it, so a resume or a compaction does not move it. The
/// caller holds the session lock, because a `Stop` hook rewrites the whole file.
pub fn record_session_base(root: &Path, id: &str, head: &str) -> io::Result<()> {
    let state = read_session(root, id).unwrap_or_default();
    if state.base.is_some() {
        return Ok(());
    }
    write_session(
        root,
        id,
        &SessionState {
            base: Some(head.to_string()),
            ..state
        },
    )
}

/// The lock of one session. Dropping it removes the lock file.
#[derive(Debug)]
pub struct SessionLock {
    path: PathBuf,
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Takes the lock of a session by creating `{session_id}.lock` exclusively, polling for up to
/// `wait`. `None` when the lock stays held, or the id or the folder is unusable. A lock file
/// older than the hook timeout belongs to a hook that died, and the next caller takes it over.
pub fn lock_session(root: &Path, id: &str, wait: Duration) -> Option<SessionLock> {
    let path = session_file(root, id).ok()?.with_extension("lock");
    create_sessions_dir(root).ok()?;
    let deadline = Instant::now() + wait;
    loop {
        match File::options().write(true).create_new(true).open(&path) {
            Ok(_) => return Some(SessionLock { path }),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::AlreadyExists | io::ErrorKind::PermissionDenied
                ) =>
            {
                if is_stale(&path) && fs::remove_file(&path).is_ok() {
                    continue;
                }
                if Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(LOCK_POLL);
            }
            Err(_) => return None,
        }
    }
}

fn is_stale(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .is_ok_and(|modified| age_of(modified) > LOCK_STALE_AGE)
}

fn age_of(modified: SystemTime) -> Duration {
    SystemTime::now()
        .duration_since(modified)
        .unwrap_or(Duration::ZERO)
}

/// The base of the session file written last within the past day, for a caller that has no
/// session id.
pub fn latest_session_base(root: &Path) -> Option<String> {
    let mut files: Vec<(SystemTime, PathBuf)> = fs::read_dir(sessions_dir(root))
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .filter(|(modified, _)| age_of(*modified) <= LATEST_MAX_AGE)
        .collect();
    files.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    files.iter().find_map(|(_, path)| parse(path)?.base)
}

/// Removes the session files that no session can still use.
pub fn prune_sessions(root: &Path) {
    let Ok(entries) = fs::read_dir(sessions_dir(root)) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let stale = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .is_ok_and(|modified| age_of(modified) > PRUNE_AGE);
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn age(root: &Path, id: &str, by: Duration) {
        let file = File::options()
            .write(true)
            .open(session_file(root, id).unwrap())
            .unwrap();
        file.set_modified(SystemTime::now() - by).unwrap();
    }

    fn based(base: &str) -> SessionState {
        SessionState {
            base: Some(base.to_string()),
            ..SessionState::default()
        }
    }

    #[test]
    fn only_letters_digits_underscore_and_hyphen_make_a_session_id() {
        for id in ["abc", "A-1_b", "0123456789abcdef-0123"] {
            assert!(is_session_id(id), "{id}");
        }
        for id in [
            "", ".", "..", "a.b", "a/b", "a\\b", "a b", "../x", "é", "a\0b",
        ] {
            assert!(!is_session_id(id), "{id:?}");
        }
        assert!(is_session_id(&"a".repeat(MAX_ID_LEN)));
        assert!(!is_session_id(&"a".repeat(MAX_ID_LEN + 1)));
    }

    #[test]
    fn a_state_round_trips_and_leaves_out_what_it_lacks() {
        let root = root();
        write_session(root.path(), "s", &based("abc")).unwrap();
        let text = fs::read_to_string(session_file(root.path(), "s").unwrap()).unwrap();
        assert_eq!(text, "{\"base\":\"abc\"}");
        assert_eq!(read_session(root.path(), "s"), Some(based("abc")));

        let all = SessionState {
            base: Some("abc".to_string()),
            blocked_diff: Some("h".to_string()),
            checked_diff: Some("k".to_string()),
        };
        write_session(root.path(), "s", &all).unwrap();
        assert_eq!(read_session(root.path(), "s"), Some(all));
    }

    #[test]
    fn a_missing_or_unreadable_file_reads_as_no_session() {
        let root = root();
        assert_eq!(read_session(root.path(), "none"), None);
        fs::create_dir_all(sessions_dir(root.path())).unwrap();
        fs::write(sessions_dir(root.path()).join("bad.json"), "not json").unwrap();
        assert_eq!(read_session(root.path(), "bad"), None);
        assert_eq!(read_session(root.path(), "../bad"), None);
    }

    #[test]
    fn a_write_replaces_the_file_and_leaves_no_temporary_file() {
        let root = root();
        write_session(root.path(), "s", &based("one")).unwrap();
        write_session(root.path(), "s", &based("two")).unwrap();
        assert_eq!(read_session(root.path(), "s"), Some(based("two")));
        let names: Vec<_> = fs::read_dir(sessions_dir(root.path()))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["s.json"]);
    }

    #[test]
    fn a_write_or_record_with_a_bad_id_fails_and_creates_nothing() {
        let root = root();
        assert!(write_session(root.path(), "../x", &based("a")).is_err());
        assert!(record_session_base(root.path(), "a/b", "a").is_err());
        assert!(!root.path().join(".code-kb").exists());
    }

    #[test]
    fn the_first_session_file_of_a_root_comes_with_the_gitignore_of_the_index_folder() {
        let root = root();
        record_session_base(root.path(), "s", "abc").unwrap();
        let gitignore = root.path().join(".code-kb").join(".gitignore");
        assert_eq!(fs::read_to_string(gitignore).unwrap(), "*\n");
    }

    #[test]
    fn a_recorded_base_is_kept_by_a_later_record() {
        let root = root();
        record_session_base(root.path(), "s", "first").unwrap();
        record_session_base(root.path(), "s", "second").unwrap();
        assert_eq!(read_session(root.path(), "s"), Some(based("first")));
    }

    #[test]
    fn a_record_adds_the_base_to_a_file_that_has_none_and_keeps_its_other_fields() {
        let root = root();
        let seen = SessionState {
            blocked_diff: Some("b".to_string()),
            checked_diff: Some("c".to_string()),
            ..SessionState::default()
        };
        write_session(root.path(), "s", &seen).unwrap();
        record_session_base(root.path(), "s", "head").unwrap();
        let expected = SessionState {
            base: Some("head".to_string()),
            ..seen
        };
        assert_eq!(read_session(root.path(), "s"), Some(expected));
    }

    #[test]
    fn the_latest_base_is_that_of_the_newest_file() {
        let root = root();
        assert_eq!(latest_session_base(root.path()), None);
        record_session_base(root.path(), "old", "older").unwrap();
        record_session_base(root.path(), "new", "newer").unwrap();
        age(root.path(), "old", Duration::from_secs(3600));
        assert_eq!(latest_session_base(root.path()).as_deref(), Some("newer"));
    }

    #[test]
    fn the_latest_base_skips_a_file_with_no_base_and_a_file_older_than_a_day() {
        let root = root();
        record_session_base(root.path(), "usable", "kept").unwrap();
        age(root.path(), "usable", Duration::from_secs(3600));
        write_session(
            root.path(),
            "blocked-only",
            &SessionState {
                blocked_diff: Some("h".to_string()),
                ..SessionState::default()
            },
        )
        .unwrap();
        assert_eq!(latest_session_base(root.path()).as_deref(), Some("kept"));

        age(root.path(), "usable", Duration::from_secs(25 * 3600));
        assert_eq!(latest_session_base(root.path()), None);
    }

    #[test]
    fn pruning_removes_only_files_older_than_a_week() {
        let root = root();
        record_session_base(root.path(), "stale", "a").unwrap();
        record_session_base(root.path(), "recent", "b").unwrap();
        age(root.path(), "stale", Duration::from_secs(8 * 24 * 3600));
        age(root.path(), "recent", Duration::from_secs(2 * 24 * 3600));
        prune_sessions(root.path());
        assert_eq!(read_session(root.path(), "stale"), None);
        assert_eq!(read_session(root.path(), "recent"), Some(based("b")));
    }

    #[test]
    fn a_held_lock_keeps_a_second_taker_out_until_it_is_dropped() {
        let root = root();
        let first = lock_session(root.path(), "s", Duration::ZERO).unwrap();
        assert!(lock_session(root.path(), "s", Duration::from_millis(60)).is_none());
        assert!(lock_session(root.path(), "other", Duration::ZERO).is_some());
        drop(first);
        assert!(lock_session(root.path(), "s", Duration::ZERO).is_some());
    }

    #[test]
    fn a_lock_file_older_than_the_stale_age_is_taken_over() {
        let root = root();
        fs::create_dir_all(sessions_dir(root.path())).unwrap();
        let lock = sessions_dir(root.path()).join("s.lock");
        let file = File::create(&lock).unwrap();
        file.set_modified(SystemTime::now() - LOCK_STALE_AGE - Duration::from_secs(1))
            .unwrap();
        drop(file);
        let taken = lock_session(root.path(), "s", Duration::ZERO);
        assert!(taken.is_some());
        drop(taken);
        assert!(!lock.exists());
    }

    #[test]
    fn a_lock_for_a_bad_session_id_is_refused_and_creates_nothing() {
        let root = root();
        assert!(lock_session(root.path(), "../x", Duration::ZERO).is_none());
        assert!(!root.path().join(".code-kb").exists());
    }

    #[test]
    fn a_lock_file_is_no_session_file() {
        let root = root();
        let _lock = lock_session(root.path(), "s", Duration::ZERO).unwrap();
        assert_eq!(latest_session_base(root.path()), None);
        assert_eq!(read_session(root.path(), "s"), None);
    }

    #[test]
    fn a_state_has_seen_the_diffs_it_checked_or_blocked_on() {
        let state = SessionState {
            blocked_diff: Some("b".to_string()),
            checked_diff: Some("c".to_string()),
            ..SessionState::default()
        };
        assert!(state.has_seen("b") && state.has_seen("c"));
        assert!(!state.has_seen("d") && !SessionState::default().has_seen(""));
    }

    #[test]
    fn pruning_a_root_with_no_sessions_does_nothing() {
        let root = root();
        prune_sessions(root.path());
        assert!(!root.path().join(".code-kb").exists());
    }
}
