//! The hooks that need the payload a harness sends on stdin: `SessionStart` records the commit a
//! session started at, and `Stop` and `SubagentStop` run the edit check against it and record each
//! check and whether the agent edited a site of a block. A `Stop` hook leaves an index that
//! another `julie-extract` wrote to the server or the CLI to rebuild.

use std::io::{IsTerminal, Read};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use code_kb_core::{
    STOP_SITES_LISTED, SessionState, Workspace, any_site_edited, edit_check_session_diff,
    ensure_fts_index_path, format_stop_reason, head_commit, index_written_by_other_extractor,
    installed_extractor_version, is_session_id, lock_session, mark_sites, open_read_only,
    prune_sessions, read_session, record_hook_check, record_hook_followed, record_session_base,
    stop_sites, write_session,
};
use serde_json::{Value, json};

const STDIN_WAIT: Duration = Duration::from_secs(2);
const LOCK_WAIT: Duration = Duration::from_secs(2);

pub fn is_stop_event(event: &str) -> bool {
    event.eq_ignore_ascii_case("Stop") || event.eq_ignore_ascii_case("SubagentStop")
}

/// Records the session base for a `SessionStart` payload on stdin. It never fails: a hook
/// that cannot record prints nothing more and leaves its exit code alone.
pub fn record_session_start() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        read_payload().and_then(|payload| record(&payload))
    }));
}

/// Prints a block decision for a `Stop` or `SubagentStop` payload on stdin when a changed symbol
/// still has sites outside the diff. It prints nothing otherwise, for any error, and, before it
/// scans or reports, when a hook of the session already checked this diff or blocked on it.
pub fn run_stop(event: &str) {
    let event = if event.eq_ignore_ascii_case("SubagentStop") {
        "SubagentStop"
    } else {
        "Stop"
    };
    let decision = catch_unwind(AssertUnwindSafe(|| {
        read_payload().and_then(|payload| stop_decision(&payload, event))
    }));
    if let Ok(Some(decision)) = decision {
        println!("{decision}");
    }
}

/// The JSON object on stdin. A terminal, an empty or closed stdin, and a stdin that sends
/// nothing within [`STDIN_WAIT`] all give `None`.
fn read_payload() -> Option<Value> {
    if std::io::stdin().is_terminal() {
        return None;
    }
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = std::io::stdin().read_to_string(&mut text);
        let _ = sender.send(text);
    });
    let text = receiver.recv_timeout(STDIN_WAIT).ok()?;
    serde_json::from_str(&text).ok()
}

fn session_id(payload: &Value) -> Option<&str> {
    payload
        .get("session_id")?
        .as_str()
        .filter(|id| is_session_id(id))
}

fn payload_workspace(payload: &Value) -> Option<Workspace> {
    let cwd = match payload.get("cwd") {
        Some(Value::String(cwd)) => cwd.clone(),
        Some(_) => return None,
        None => std::env::current_dir().ok()?.to_str()?.to_string(),
    };
    Workspace::from_project_root(&cwd).ok()
}

fn existing_index(workspace: &Workspace) -> Option<PathBuf> {
    workspace.locate_db(None).ok().filter(|db| db.is_file())
}

fn record(payload: &Value) -> Option<()> {
    let id = session_id(payload)?;
    let workspace = payload_workspace(payload)?;
    let root = &workspace.canonical_root;
    let head = head_commit(root)?;
    prune_sessions(root);
    let _lock = lock_session(root, id, LOCK_WAIT)?;
    record_session_base(root, id, &head).ok()
}

/// At the first stop after a block, records whether an edit since the block changed a site it
/// listed, and forgets the sites.
fn record_followed(root: &Path, id: &str) -> Option<()> {
    if read_session(root, id)?.blocked_sites.is_empty() {
        return None;
    }
    let _lock = lock_session(root, id, LOCK_WAIT)?;
    let state = read_session(root, id)?;
    if state.blocked_sites.is_empty() {
        return None;
    }
    let followed = any_site_edited(root, &state.blocked_sites);
    let blocked_diff = state.blocked_diff.clone();
    write_session(
        root,
        id,
        &SessionState {
            blocked_sites: Vec::new(),
            ..state
        },
    )
    .ok()?;
    record_hook_followed(root, &blocked_diff?, followed);
    Some(())
}

fn stop_decision(payload: &Value, event: &str) -> Option<String> {
    let id = session_id(payload)?;
    let workspace = payload_workspace(payload)?;
    let root = &workspace.canonical_root;
    record_followed(root, id);
    if payload.get("stop_hook_active").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let db_path = existing_index(&workspace)?;
    if index_written_by_other_extractor(&db_path, &installed_extractor_version()) {
        return None;
    }
    ensure_fts_index_path(&db_path).ok()?;
    let conn = open_read_only(&db_path).ok()?;
    let base = read_session(root, id).and_then(|state| state.base);
    let pending = edit_check_session_diff(&workspace, &conn, base.as_deref()).ok()?;
    let hash = pending.diff_hash().to_string();
    let lock = lock_session(root, id, LOCK_WAIT)?;
    let state = read_session(root, id).unwrap_or_default();
    if state.has_seen(&hash) {
        return None;
    }
    let check = pending.run(&workspace, &db_path, &conn).ok()?;
    let reason = format_stop_reason(&check);
    let sites = stop_sites(&check);
    let (blocked_diff, blocked_sites) = match reason {
        Some(_) => (
            Some(hash.clone()),
            mark_sites(
                root,
                sites
                    .iter()
                    .take(STOP_SITES_LISTED)
                    .map(|site| (site.path.as_str(), site.line)),
            ),
        ),
        None => (state.blocked_diff.clone(), state.blocked_sites.clone()),
    };
    let reported = SessionState {
        blocked_diff,
        checked_diff: Some(hash.clone()),
        blocked_sites,
        ..state
    };
    write_session(root, id, &reported).ok()?;
    drop(lock);
    record_hook_check(root, event, &hash, sites.len(), reason.is_some());
    reason.map(|reason| json!({ "decision": "block", "reason": reason }).to_string())
}
