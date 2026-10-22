use code_kb_core::{Workspace, find_julie_extract_binary, safe_tempdir, scan_workspace};
use serde_json::{Value, json};
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime};

const CARGO_TOML: &str = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const STOP_REASON: &str = "Before you finish: 1 sites still use the old shape of `area`.\nsrc/second.rs:4  area: parameters 2 → 3\nEdit them or say why they need no change.";

struct Repo {
    root: PathBuf,
    _temp: tempfile::TempDir,
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "core.autocrlf=false", "-c", "commit.gpgsign=false"])
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn write(root: &Path, path: &str, content: &str) {
    let full = root.join(path);
    fs::create_dir_all(full.parent().unwrap()).unwrap();
    fs::write(full, content).unwrap();
}

fn committed_repo<P: AsRef<str>, C: AsRef<str>>(files: &[(P, C)], with_git: bool) -> Repo {
    let temp = safe_tempdir();
    let root = temp.path().to_path_buf();
    if with_git {
        git(&root, &["init", "-q", "-b", "main"]);
    }
    for (path, content) in files {
        write(&root, path.as_ref(), content.as_ref());
    }
    if with_git {
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "base"]);
    }
    Repo { root, _temp: temp }
}

fn index_path(root: &Path) -> PathBuf {
    root.join(".code-kb").join("artifact.db")
}

fn indexed_repo<P: AsRef<str>, C: AsRef<str>>(files: &[(P, C)], with_git: bool) -> Repo {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let repo = committed_repo(files, with_git);
    scan_workspace(
        &Workspace::new(repo.root.clone()),
        &index_path(&repo.root),
        true,
    )
    .expect("scan failed");
    repo
}

fn rust_function_files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Cargo.toml", CARGO_TOML),
        (
            "src/lib.rs",
            "pub mod geometry;\npub mod first;\npub mod second;\n",
        ),
        (
            "src/geometry.rs",
            "pub fn area(w: i32, h: i32) -> i32 {\n    w * h\n}\n",
        ),
        (
            "src/first.rs",
            "use crate::geometry;\n\npub fn first() -> i32 {\n    crate::geometry::area(2, 3)\n}\n",
        ),
        (
            "src/second.rs",
            "use crate::geometry;\n\npub fn second() -> i32 {\n    crate::geometry::area(4, 5)\n}\n",
        ),
    ]
}

fn rust_function_repo() -> Repo {
    indexed_repo(&rust_function_files(), true)
}

fn add_depth_parameter(repo: &Repo) {
    write(
        &repo.root,
        "src/geometry.rs",
        "pub fn area(w: i32, h: i32, depth: i32) -> i32 {\n    w * h * depth\n}\n",
    );
    write(
        &repo.root,
        "src/first.rs",
        "use crate::geometry;\n\npub fn first() -> i32 {\n    crate::geometry::area(2, 3, 1)\n}\n",
    );
}

fn payload(session_id: &str, cwd: &Path, stop_hook_active: bool) -> String {
    json!({
        "session_id": session_id,
        "cwd": cwd,
        "hook_event_name": "Stop",
        "stop_hook_active": stop_hook_active,
    })
    .to_string()
}

fn run_hook(event: &str, stdin: Option<&str>, cwd: &Path) -> Output {
    run_hook_with_env(event, stdin, cwd, &[])
}

fn run_hook_with_env(
    event: &str,
    stdin: Option<&str>,
    cwd: &Path,
    envs: &[(&str, &OsStr)],
) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .args(["hook", event])
        .env("CODE_KB_TELEMETRY_DIR", telemetry_dir(cwd))
        .envs(envs.iter().copied())
        .current_dir(cwd)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(text) = stdin {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }
    child.wait_with_output().unwrap()
}

fn telemetry_dir(root: &Path) -> PathBuf {
    root.join(".code-kb").join("telemetry")
}

fn hook_rows(root: &Path) -> Vec<(String, i64, i64, Option<i64>)> {
    let conn = code_kb_core::Connection::open(telemetry_dir(root).join("telemetry.db")).unwrap();
    conn.prepare("SELECT event, sites_listed, blocked, followed FROM hook_telemetry ORDER BY id")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn without_parent_repository(root: &Path) -> [(&'static str, &OsStr); 1] {
    [(
        "GIT_CEILING_DIRECTORIES",
        root.parent().unwrap().as_os_str(),
    )]
}

fn start_session(repo: &Repo, session_id: &str) -> Output {
    run_hook(
        "SessionStart",
        Some(&payload(session_id, &repo.root, false)),
        &repo.root,
    )
}

fn session_file(root: &Path, session_id: &str) -> PathBuf {
    root.join(".code-kb")
        .join("sessions")
        .join(format!("{session_id}.json"))
}

fn session_json(root: &Path, session_id: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(session_file(root, session_id)).unwrap()).unwrap()
}

fn assert_silent(output: &Output) {
    assert!(output.status.success(), "{:?}", output.status);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "",
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_blocks_with(output: &Output, reason: &str) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let decision: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stdout)));
    assert_eq!(decision, json!({"decision": "block", "reason": reason}));
}

#[test]
fn session_start_prints_the_same_text_with_or_without_a_payload() {
    let repo = rust_function_repo();
    let without = run_hook("SessionStart", None, &repo.root);
    let with = start_session(&repo, "abc-123");
    assert!(without.status.success() && with.status.success());
    assert!(!without.stdout.is_empty());
    assert_eq!(with.stdout, without.stdout);
}

#[test]
fn session_start_writes_the_head_commit_as_the_session_base() {
    let repo = rust_function_repo();
    start_session(&repo, "abc_123-X");
    let head = git(&repo.root, &["rev-parse", "HEAD"]);
    assert_eq!(
        session_json(&repo.root, "abc_123-X"),
        json!({ "base": head })
    );
}

#[test]
fn session_start_from_a_subfolder_writes_the_file_in_the_project_root() {
    let repo = rust_function_repo();
    let payload = payload("sub", &repo.root.join("src"), false);
    run_hook("SessionStart", Some(&payload), &repo.root);
    assert!(session_file(&repo.root, "sub").is_file());
}

#[test]
fn session_start_keeps_the_base_a_session_already_has() {
    let repo = rust_function_repo();
    let first = git(&repo.root, &["rev-parse", "HEAD"]);
    start_session(&repo, "kept");
    write(&repo.root, "src/extra.rs", "pub fn extra() {}\n");
    git(&repo.root, &["add", "."]);
    git(&repo.root, &["commit", "-q", "-m", "more"]);
    start_session(&repo, "kept");
    assert_eq!(session_json(&repo.root, "kept"), json!({ "base": first }));
}

#[test]
fn session_start_refuses_a_session_id_that_is_not_a_plain_name() {
    let repo = rust_function_repo();
    for id in ["../escape", "a/b", "a\\b", "..", ".", "", "a.b", "a b"] {
        let output = start_session(&repo, id);
        assert!(output.status.success(), "{id:?}");
    }
    let sessions = repo.root.join(".code-kb").join("sessions");
    let entries = fs::read_dir(&sessions).map_or(0, Iterator::count);
    assert_eq!(entries, 0);
    assert!(!repo.root.join(".code-kb").join("escape.json").exists());
    assert!(!repo.root.join("escape.json").exists());
}

#[test]
fn session_start_writes_no_file_outside_a_git_repository() {
    let repo = indexed_repo(&[("a.py", "def f():\n    return 1\n")], false);
    let output = run_hook_with_env(
        "SessionStart",
        Some(&payload("plain", &repo.root, false)),
        &repo.root,
        &without_parent_repository(&repo.root),
    );
    assert!(output.status.success());
    assert!(!output.stdout.is_empty());
    assert!(!repo.root.join(".code-kb").join("sessions").exists());
}

#[test]
fn session_start_records_the_base_when_the_project_has_no_index_yet() {
    let repo = committed_repo(&[("a.py", "def f():\n    return 1\n")], true);
    let indexed = rust_function_repo();
    let output = start_session(&repo, "no-index");
    let head = git(&repo.root, &["rev-parse", "HEAD"]);
    assert_eq!(
        session_json(&repo.root, "no-index"),
        json!({ "base": head })
    );
    assert!(!index_path(&repo.root).exists());
    let code_kb = repo.root.join(".code-kb");
    assert_eq!(
        fs::read_to_string(code_kb.join(".gitignore")).unwrap(),
        "*\n"
    );
    let mut entries: Vec<String> = fs::read_dir(&code_kb)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    entries.sort();
    assert_eq!(entries, [".gitignore", "sessions"]);
    assert_eq!(output.stdout, start_session(&indexed, "indexed").stdout);
}

#[test]
fn session_start_writes_no_file_in_a_repository_without_a_commit() {
    let repo = committed_repo(&[("a.py", "def f():\n    return 1\n")], false);
    git(&repo.root, &["init", "-q", "-b", "main"]);
    let output = start_session(&repo, "no-commit");
    assert!(output.status.success());
    assert!(!repo.root.join(".code-kb").exists());
}

#[test]
fn session_start_ignores_a_payload_that_is_not_json_or_is_empty() {
    let repo = rust_function_repo();
    let expected = run_hook("SessionStart", None, &repo.root).stdout;
    for text in ["", "not json", "[]", "{\"cwd\": 1}"] {
        let output = run_hook("SessionStart", Some(text), &repo.root);
        assert!(output.status.success(), "{text:?}");
        assert_eq!(output.stdout, expected, "{text:?}");
    }
    assert!(!repo.root.join(".code-kb").join("sessions").exists());
}

#[test]
fn session_start_does_not_wait_for_a_stdin_that_stays_open() {
    let repo = rust_function_repo();
    let expected = run_hook("SessionStart", None, &repo.root).stdout;
    let mut child = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .args(["hook", "SessionStart"])
        .current_dir(&repo.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let held_open = child.stdin.take().unwrap();
    let started = Instant::now();
    let output = child.wait_with_output().unwrap();
    drop(held_open);
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(output.status.success());
    assert_eq!(output.stdout, expected);
}

#[test]
fn stop_blocks_once_for_a_site_the_diff_did_not_reach_then_stays_silent_for_the_same_diff() {
    let repo = rust_function_repo();
    start_session(&repo, "s1");
    add_depth_parameter(&repo);
    let stdin = payload("s1", &repo.root, false);

    assert_blocks_with(&run_hook("Stop", Some(&stdin), &repo.root), STOP_REASON);
    assert_silent(&run_hook("Stop", Some(&stdin), &repo.root));
    assert!(session_json(&repo.root, "s1")["base"].is_string());
}

fn set_recorded_extractor_version(root: &Path, version: &str) {
    rusqlite::Connection::open(index_path(root))
        .unwrap()
        .execute(
            "UPDATE artifact_metadata SET value = ?1 WHERE key = 'binary_version'",
            [version],
        )
        .unwrap();
}

fn recorded_extractor_version(root: &Path) -> String {
    rusqlite::Connection::open_with_flags(
        index_path(root),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
    .query_row(
        "SELECT value FROM artifact_metadata WHERE key = 'binary_version'",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

#[test]
fn stop_hooks_print_nothing_and_leave_an_index_from_another_extractor_alone() {
    for event in ["Stop", "SubagentStop"] {
        let repo = rust_function_repo();
        start_session(&repo, "other-extractor");
        add_depth_parameter(&repo);
        set_recorded_extractor_version(&repo.root, "0.0.1");
        let before = fs::metadata(index_path(&repo.root)).unwrap();

        let output = run_hook(
            event,
            Some(&payload("other-extractor", &repo.root, false)),
            &repo.root,
        );

        let after = fs::metadata(index_path(&repo.root)).unwrap();
        assert_eq!(after.len(), before.len(), "{event}");
        assert_eq!(
            after.modified().unwrap(),
            before.modified().unwrap(),
            "{event}"
        );
        assert_eq!(recorded_extractor_version(&repo.root), "0.0.1", "{event}");
        assert_silent(&output);
    }
}

#[test]
fn the_stop_after_a_block_records_it_followed_when_a_listed_site_was_edited() {
    let repo = rust_function_repo();
    start_session(&repo, "f1");
    add_depth_parameter(&repo);
    assert_blocks_with(
        &run_hook("Stop", Some(&payload("f1", &repo.root, false)), &repo.root),
        STOP_REASON,
    );
    assert_eq!(
        session_json(&repo.root, "f1")["blocked_sites"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    write(
        &repo.root,
        "src/second.rs",
        "use crate::geometry;\n\npub fn second() -> i32 {\n    crate::geometry::area(4, 5, 1)\n}\n",
    );

    assert_silent(&run_hook(
        "Stop",
        Some(&payload("f1", &repo.root, true)),
        &repo.root,
    ));
    assert_silent(&run_hook(
        "Stop",
        Some(&payload("f1", &repo.root, false)),
        &repo.root,
    ));
    assert_eq!(
        hook_rows(&repo.root),
        vec![
            ("Stop".to_string(), 1, 1, Some(1)),
            ("Stop".to_string(), 0, 0, None)
        ]
    );
    assert!(
        session_json(&repo.root, "f1")
            .get("blocked_sites")
            .is_none()
    );
}

#[test]
fn one_sessions_follow_through_does_not_resolve_another_sessions_matching_block() {
    let repo = rust_function_repo();
    start_session(&repo, "first");
    start_session(&repo, "second");
    add_depth_parameter(&repo);

    for session_id in ["first", "second"] {
        assert_blocks_with(
            &run_hook(
                "Stop",
                Some(&payload(session_id, &repo.root, false)),
                &repo.root,
            ),
            STOP_REASON,
        );
    }

    write(
        &repo.root,
        "src/second.rs",
        "use crate::geometry;\n\npub fn second() -> i32 {\n    crate::geometry::area(4, 5, 1)\n}\n",
    );
    assert_silent(&run_hook(
        "Stop",
        Some(&payload("first", &repo.root, true)),
        &repo.root,
    ));

    assert_eq!(
        hook_rows(&repo.root),
        vec![
            ("Stop".to_string(), 1, 1, Some(1)),
            ("Stop".to_string(), 1, 1, None),
        ]
    );
}

#[test]
fn a_legacy_hook_telemetry_table_is_upgraded_before_checks_are_recorded() {
    let repo = rust_function_repo();
    let telemetry = telemetry_dir(&repo.root);
    std::fs::create_dir_all(&telemetry).unwrap();
    code_kb_core::Connection::open(telemetry.join("telemetry.db"))
        .unwrap()
        .execute_batch(
            "CREATE TABLE hook_telemetry (
                id INTEGER PRIMARY KEY,
                timestamp TEXT NOT NULL,
                workspace_root TEXT NOT NULL,
                event TEXT NOT NULL,
                diff_hash TEXT NOT NULL,
                sites_listed INTEGER NOT NULL,
                blocked INTEGER NOT NULL,
                followed INTEGER DEFAULT NULL
            );",
        )
        .unwrap();

    start_session(&repo, "legacy");
    add_depth_parameter(&repo);
    assert_blocks_with(
        &run_hook(
            "Stop",
            Some(&payload("legacy", &repo.root, false)),
            &repo.root,
        ),
        STOP_REASON,
    );

    assert_eq!(
        hook_rows(&repo.root),
        vec![("Stop".to_string(), 1, 1, None)]
    );
}

#[test]
fn the_stop_after_a_block_records_it_not_followed_when_no_listed_site_changed() {
    let repo = rust_function_repo();
    start_session(&repo, "f2");
    add_depth_parameter(&repo);
    let stdin = payload("f2", &repo.root, false);
    assert_blocks_with(
        &run_hook("SubagentStop", Some(&stdin), &repo.root),
        STOP_REASON,
    );

    assert_silent(&run_hook("SubagentStop", Some(&stdin), &repo.root));
    assert_eq!(
        hook_rows(&repo.root),
        vec![("SubagentStop".to_string(), 1, 1, Some(0))]
    );
}

#[test]
fn a_disabled_telemetry_flag_records_no_hook_check() {
    let repo = rust_function_repo();
    start_session(&repo, "f3");
    add_depth_parameter(&repo);
    let stdin = payload("f3", &repo.root, false);
    let disabled = [("CODE_KB_NO_TELEMETRY", OsStr::new("1"))];

    let output = run_hook_with_env("Stop", Some(&stdin), &repo.root, &disabled);
    assert_blocks_with(&output, STOP_REASON);
    assert_silent(&run_hook_with_env(
        "Stop",
        Some(&stdin),
        &repo.root,
        &disabled,
    ));
    assert!(!telemetry_dir(&repo.root).exists());
}

#[test]
fn stop_blocks_again_when_the_diff_changes() {
    let repo = rust_function_repo();
    start_session(&repo, "s2");
    add_depth_parameter(&repo);
    let stdin = payload("s2", &repo.root, false);
    assert_blocks_with(&run_hook("Stop", Some(&stdin), &repo.root), STOP_REASON);

    write(
        &repo.root,
        "src/first.rs",
        "use crate::geometry;\n\npub fn first() -> i32 {\n    crate::geometry::area(2, 3, 2)\n}\n",
    );
    assert_blocks_with(&run_hook("Stop", Some(&stdin), &repo.root), STOP_REASON);
}

#[test]
fn stop_is_silent_when_a_stop_hook_already_blocked_this_stop() {
    let repo = rust_function_repo();
    start_session(&repo, "s3");
    add_depth_parameter(&repo);
    let output = run_hook("Stop", Some(&payload("s3", &repo.root, true)), &repo.root);
    assert_silent(&output);
    assert!(session_json(&repo.root, "s3").get("blocked_diff").is_none());
}

#[test]
fn stop_is_silent_when_no_symbol_changed_shape() {
    let repo = rust_function_repo();
    start_session(&repo, "s4");
    let stdin = payload("s4", &repo.root, false);
    assert_silent(&run_hook("Stop", Some(&stdin), &repo.root));

    write(
        &repo.root,
        "src/geometry.rs",
        "pub fn area(w: i32, h: i32) -> i32 {\n    h * w\n}\n",
    );
    assert_silent(&run_hook("Stop", Some(&stdin), &repo.root));
}

#[test]
fn stop_is_silent_when_every_site_is_in_the_diff() {
    let repo = rust_function_repo();
    start_session(&repo, "s5");
    add_depth_parameter(&repo);
    write(
        &repo.root,
        "src/second.rs",
        "use crate::geometry;\n\npub fn second() -> i32 {\n    crate::geometry::area(4, 5, 1)\n}\n",
    );
    assert_silent(&run_hook(
        "Stop",
        Some(&payload("s5", &repo.root, false)),
        &repo.root,
    ));
}

#[test]
fn stop_is_silent_outside_a_git_repository() {
    let repo = indexed_repo(&[("a.py", "def f(x):\n    return x\n")], false);
    write(&repo.root, "a.py", "def f(x, y):\n    return x\n");
    assert_silent(&run_hook_with_env(
        "Stop",
        Some(&payload("plain", &repo.root, false)),
        &repo.root,
        &without_parent_repository(&repo.root),
    ));

    let nowhere = safe_tempdir();
    assert_silent(&run_hook_with_env(
        "Stop",
        Some(&payload("plain", nowhere.path(), false)),
        nowhere.path(),
        &without_parent_repository(nowhere.path()),
    ));
}

#[test]
fn stop_is_silent_for_a_missing_or_unusable_payload() {
    let repo = rust_function_repo();
    add_depth_parameter(&repo);
    assert_silent(&run_hook("Stop", None, &repo.root));
    for text in [
        "",
        "not json",
        "{}",
        "[]",
        "{\"session_id\": 7}",
        "{\"cwd\": \"/\"}",
    ] {
        assert_silent(&run_hook("Stop", Some(text), &repo.root));
    }
}

#[test]
fn stop_is_silent_for_a_session_id_that_is_not_a_plain_name() {
    let repo = rust_function_repo();
    add_depth_parameter(&repo);
    for id in ["../escape", "a/b", "..", "", "a.b"] {
        assert_silent(&run_hook(
            "Stop",
            Some(&payload(id, &repo.root, false)),
            &repo.root,
        ));
    }
    assert!(!repo.root.join(".code-kb").join("sessions").exists());
}

#[test]
fn stop_compares_with_head_when_the_session_has_no_base() {
    let repo = rust_function_repo();
    add_depth_parameter(&repo);
    let stdin = payload("late", &repo.root, false);
    assert_blocks_with(&run_hook("Stop", Some(&stdin), &repo.root), STOP_REASON);
    let state = session_json(&repo.root, "late");
    assert!(state.get("base").is_none());
    assert!(state["blocked_diff"].is_string());
}

#[test]
fn stop_compares_with_the_session_base_after_the_edit_is_committed() {
    let repo = rust_function_repo();
    start_session(&repo, "s6");
    add_depth_parameter(&repo);
    git(&repo.root, &["commit", "-q", "-am", "add depth"]);
    assert_blocks_with(
        &run_hook("Stop", Some(&payload("s6", &repo.root, false)), &repo.root),
        STOP_REASON,
    );

    let after_head = payload("other", &repo.root, false);
    assert_silent(&run_hook("Stop", Some(&after_head), &repo.root));
}

#[test]
fn stop_falls_back_to_head_when_the_session_base_is_gone() {
    let repo = rust_function_repo();
    write_session_base(&repo, "s7", "0123456789abcdef0123456789abcdef01234567");
    add_depth_parameter(&repo);
    assert_blocks_with(
        &run_hook("Stop", Some(&payload("s7", &repo.root, false)), &repo.root),
        STOP_REASON,
    );
}

fn write_session_base(repo: &Repo, session_id: &str, base: &str) {
    let path = session_file(&repo.root, session_id);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, json!({ "base": base }).to_string()).unwrap();
}

#[test]
fn subagent_stop_blocks_once_like_stop() {
    let repo = rust_function_repo();
    start_session(&repo, "s8");
    add_depth_parameter(&repo);
    let stdin = payload("s8", &repo.root, false);
    assert_blocks_with(
        &run_hook("SubagentStop", Some(&stdin), &repo.root),
        STOP_REASON,
    );
    assert_silent(&run_hook("SubagentStop", Some(&stdin), &repo.root));
    assert_silent(&run_hook("Stop", Some(&stdin), &repo.root));
}

#[test]
fn subagent_stop_is_silent_when_a_stop_hook_already_blocked_this_stop() {
    let repo = rust_function_repo();
    start_session(&repo, "s9");
    add_depth_parameter(&repo);
    assert_silent(&run_hook(
        "SubagentStop",
        Some(&payload("s9", &repo.root, true)),
        &repo.root,
    ));
}

#[cfg(unix)]
struct ExtractorStub {
    script: PathBuf,
    log: PathBuf,
    real: PathBuf,
    _temp: tempfile::TempDir,
}

#[cfg(unix)]
impl ExtractorStub {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let temp = safe_tempdir();
        let script = temp.path().join("julie-extract");
        fs::write(
            &script,
            "#!/bin/sh\ncase \"$1\" in scan|update|delete) echo \"$1\" >> \"$STUB_LOG\" ;; esac\nexec \"$STUB_REAL\" \"$@\"\n",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            log: temp.path().join("calls.log"),
            real: find_julie_extract_binary().unwrap(),
            script,
            _temp: temp,
        }
    }

    fn hook(&self, event: &str, repo: &Repo, session_id: &str) -> Output {
        run_hook_with_env(
            event,
            Some(&payload(session_id, &repo.root, false)),
            &repo.root,
            &[
                ("JULIE_EXTRACT_BIN", self.script.as_os_str()),
                ("STUB_LOG", self.log.as_os_str()),
                ("STUB_REAL", self.real.as_os_str()),
            ],
        )
    }

    fn calls(&self) -> usize {
        fs::read_to_string(&self.log).map_or(0, |text| text.lines().count())
    }
}

fn lock_file(root: &Path, session_id: &str) -> PathBuf {
    root.join(".code-kb")
        .join("sessions")
        .join(format!("{session_id}.lock"))
}

#[cfg(unix)]
fn edit_body_only(repo: &Repo) {
    write(
        &repo.root,
        "src/geometry.rs",
        "pub fn area(w: i32, h: i32) -> i32 {\n    h * w\n}\n",
    );
}

#[cfg(unix)]
#[test]
fn a_second_stop_on_a_diff_the_last_check_found_clean_runs_no_extractor() {
    let repo = rust_function_repo();
    start_session(&repo, "clean");
    edit_body_only(&repo);
    let stub = ExtractorStub::new();

    assert_silent(&stub.hook("Stop", &repo, "clean"));
    let after_first = stub.calls();
    assert!(after_first > 0);
    let state = session_json(&repo.root, "clean");
    assert!(state["checked_diff"].is_string(), "{state}");
    assert!(state.get("blocked_diff").is_none(), "{state}");
    assert!(!lock_file(&repo.root, "clean").exists());

    assert_silent(&stub.hook("Stop", &repo, "clean"));
    assert_silent(&stub.hook("SubagentStop", &repo, "clean"));
    assert_eq!(stub.calls(), after_first);
}

#[cfg(unix)]
#[test]
fn a_second_stop_on_a_diff_that_already_blocked_runs_no_extractor() {
    let repo = rust_function_repo();
    start_session(&repo, "blocked");
    add_depth_parameter(&repo);
    let stub = ExtractorStub::new();

    assert_blocks_with(&stub.hook("Stop", &repo, "blocked"), STOP_REASON);
    let after_first = stub.calls();
    let state = session_json(&repo.root, "blocked");
    assert_eq!(state["checked_diff"], state["blocked_diff"], "{state}");

    assert_silent(&stub.hook("Stop", &repo, "blocked"));
    assert_silent(&stub.hook("SubagentStop", &repo, "blocked"));
    assert_eq!(stub.calls(), after_first);
}

#[cfg(unix)]
#[test]
fn an_edit_after_a_clean_check_runs_the_check_again() {
    let repo = rust_function_repo();
    start_session(&repo, "again");
    edit_body_only(&repo);
    let stub = ExtractorStub::new();
    assert_silent(&stub.hook("Stop", &repo, "again"));
    let after_clean = stub.calls();

    add_depth_parameter(&repo);

    assert_blocks_with(&stub.hook("Stop", &repo, "again"), STOP_REASON);
    assert!(stub.calls() > after_clean);
}

#[test]
fn a_diff_that_blocked_stays_silent_when_it_returns_after_a_clean_diff() {
    let repo = rust_function_repo();
    start_session(&repo, "back");
    let stdin = payload("back", &repo.root, false);
    add_depth_parameter(&repo);
    assert_blocks_with(&run_hook("Stop", Some(&stdin), &repo.root), STOP_REASON);

    for (path, content) in rust_function_files() {
        write(&repo.root, path, content);
    }
    assert_silent(&run_hook("Stop", Some(&stdin), &repo.root));

    add_depth_parameter(&repo);
    assert_silent(&run_hook("Stop", Some(&stdin), &repo.root));
}

#[test]
fn parallel_stops_for_one_session_block_exactly_once() {
    let repo = rust_function_repo();
    start_session(&repo, "race");
    add_depth_parameter(&repo);
    let stdin = payload("race", &repo.root, false);
    let (stdin, root) = (&stdin, &repo.root);

    let outputs: Vec<Output> = std::thread::scope(|scope| {
        let handles: Vec<_> = ["Stop", "SubagentStop", "Stop", "SubagentStop"]
            .into_iter()
            .map(|event| scope.spawn(move || run_hook(event, Some(stdin), root)))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    });

    assert!(outputs.iter().all(|output| output.status.success()));
    let blocks: Vec<&Output> = outputs
        .iter()
        .filter(|output| !output.stdout.is_empty())
        .collect();
    assert_eq!(blocks.len(), 1, "{outputs:?}");
    assert_blocks_with(blocks[0], STOP_REASON);
    assert!(!lock_file(&repo.root, "race").exists());
}

#[test]
fn a_stop_that_cannot_take_the_session_lock_within_two_seconds_prints_nothing() {
    let repo = rust_function_repo();
    start_session(&repo, "held");
    add_depth_parameter(&repo);
    let lock = lock_file(&repo.root, "held");
    fs::write(&lock, "").unwrap();

    let started = Instant::now();
    let output = run_hook(
        "Stop",
        Some(&payload("held", &repo.root, false)),
        &repo.root,
    );

    assert_silent(&output);
    assert!(started.elapsed() >= Duration::from_secs(2));
    assert!(lock.exists());
    assert!(
        session_json(&repo.root, "held")
            .get("checked_diff")
            .is_none()
    );
}

#[test]
fn a_session_lock_older_than_the_hook_timeout_is_taken_over() {
    let repo = rust_function_repo();
    start_session(&repo, "dead");
    add_depth_parameter(&repo);
    let lock = lock_file(&repo.root, "dead");
    fs::write(&lock, "").unwrap();
    fs::File::options()
        .write(true)
        .open(&lock)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(120))
        .unwrap();

    assert_blocks_with(
        &run_hook(
            "Stop",
            Some(&payload("dead", &repo.root, false)),
            &repo.root,
        ),
        STOP_REASON,
    );
    assert!(!lock.exists());
}

#[test]
fn session_start_adds_the_base_to_a_session_file_that_a_stop_created() {
    let repo = rust_function_repo();
    add_depth_parameter(&repo);
    let stdin = payload("late", &repo.root, false);
    assert_blocks_with(&run_hook("Stop", Some(&stdin), &repo.root), STOP_REASON);
    assert!(session_json(&repo.root, "late").get("base").is_none());

    start_session(&repo, "late");

    let state = session_json(&repo.root, "late");
    assert_eq!(state["base"], git(&repo.root, &["rev-parse", "HEAD"]));
    assert!(state["checked_diff"].is_string(), "{state}");
    assert_eq!(state["blocked_diff"], state["checked_diff"], "{state}");
}

#[test]
fn session_start_that_cannot_take_the_session_lock_within_two_seconds_adds_no_base() {
    let repo = rust_function_repo();
    add_depth_parameter(&repo);
    let stdin = payload("busy", &repo.root, false);
    assert_blocks_with(&run_hook("Stop", Some(&stdin), &repo.root), STOP_REASON);
    let lock = lock_file(&repo.root, "busy");
    fs::write(&lock, "").unwrap();

    let started = Instant::now();
    start_session(&repo, "busy");

    assert!(started.elapsed() >= Duration::from_secs(2));
    assert!(lock.exists());
    assert!(session_json(&repo.root, "busy").get("base").is_none());
}

#[test]
fn stop_lists_at_most_fifteen_sites() {
    let mut files: Vec<(String, String)> = vec![
        ("Cargo.toml".to_string(), CARGO_TOML.to_string()),
        (
            "src/geometry.rs".to_string(),
            "pub fn area(w: i32, h: i32) -> i32 {\n    w * h\n}\n".to_string(),
        ),
    ];
    let mut lib = String::from("pub mod geometry;\n");
    for index in 0..20 {
        lib.push_str(&format!("pub mod user{index};\n"));
        files.push((
            format!("src/user{index}.rs"),
            format!(
                "use crate::geometry;\n\npub fn user{index}() -> i32 {{\n    crate::geometry::area(1, 2)\n}}\n"
            ),
        ));
    }
    files.push(("src/lib.rs".to_string(), lib));
    let repo = indexed_repo(&files, true);
    start_session(&repo, "many");
    write(
        &repo.root,
        "src/geometry.rs",
        "pub fn area(w: i32, h: i32, depth: i32) -> i32 {\n    w * h * depth\n}\n",
    );
    let output = run_hook(
        "Stop",
        Some(&payload("many", &repo.root, false)),
        &repo.root,
    );
    let decision: Value = serde_json::from_slice(&output.stdout).unwrap();
    let reason = decision["reason"].as_str().unwrap();
    let lines: Vec<&str> = reason.lines().collect();
    assert_eq!(
        lines[0],
        "Before you finish: 20 sites still use the old shape of `area`."
    );
    assert_eq!(lines.len(), 1 + 15 + 1, "{reason}");
    assert_eq!(
        lines.last().copied(),
        Some("Edit them or say why they need no change.")
    );
}

#[test]
fn check_without_a_base_compares_with_the_session_base_and_says_so() {
    let repo = rust_function_repo();
    start_session(&repo, "s10");
    let base = git(&repo.root, &["rev-parse", "HEAD"]);
    add_depth_parameter(&repo);
    git(&repo.root, &["commit", "-q", "-am", "add depth"]);

    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env("CODE_KB_TELEMETRY_DIR", repo.root.join(".telemetry_test"))
        .arg("--root")
        .arg(&repo.root)
        .arg("check")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.starts_with(&format!(
            "check_edit: working tree against {} (session base), ",
            &base[..10]
        )),
        "{text}"
    );
    assert!(text.contains("src/second.rs:4"), "{text}");
}

#[test]
fn check_outside_a_git_repository_is_an_error() {
    let repo = indexed_repo(&[("a.py", "def f():\n    return 1\n")], false);
    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env("CODE_KB_TELEMETRY_DIR", repo.root.join(".telemetry_test"))
        .envs(without_parent_repository(&repo.root))
        .arg("--root")
        .arg(&repo.root)
        .arg("check")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not a git repository"), "{stderr}");
}

#[test]
fn check_with_an_unknown_base_prints_the_revision_sentence_once() {
    let repo = rust_function_repo();
    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env("CODE_KB_TELEMETRY_DIR", repo.root.join(".telemetry_test"))
        .arg("--root")
        .arg(&repo.root)
        .args(["check", "--base", "nonexistent"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let sentence = "git revision `nonexistent` does not exist in this repository";
    assert_eq!(stderr.matches(sentence).count(), 1, "{stderr}");
}

fn rename_area(repo: &Repo, also_second: bool) {
    write(
        &repo.root,
        "src/geometry.rs",
        "pub fn surface(w: i32, h: i32) -> i32 {\n    w * h\n}\n",
    );
    write(
        &repo.root,
        "src/first.rs",
        "use crate::geometry;\n\npub fn first() -> i32 {\n    crate::geometry::surface(2, 3)\n}\n",
    );
    if also_second {
        write(
            &repo.root,
            "src/second.rs",
            "use crate::geometry;\n\npub fn second() -> i32 {\n    crate::geometry::surface(4, 5)\n}\n",
        );
    }
}

fn run_check(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env("CODE_KB_TELEMETRY_DIR", root.join(".telemetry_test"))
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn check_exits_zero_on_a_proven_site_outside_the_diff_without_fail_on_proven() {
    let repo = rust_function_repo();
    rename_area(&repo, false);

    let output = run_check(&repo.root, &["check"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("src/second.rs:4"), "{text}");
}

#[test]
fn check_with_fail_on_proven_exits_three_after_the_report_on_a_proven_site_outside_the_diff() {
    let repo = rust_function_repo();
    rename_area(&repo, false);

    let output = run_check(&repo.root, &["check", "--fail-on-proven"]);
    assert_eq!(
        output.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("src/second.rs:4"), "{text}");

    let json_output = run_check(&repo.root, &["--json", "check", "--fail-on-proven"]);
    assert_eq!(json_output.status.code(), Some(3));
    let check: Value = serde_json::from_slice(&json_output.stdout).unwrap();
    assert_eq!(check["changes"][0]["proven"][0]["path"], "src/second.rs");
}

#[test]
fn check_with_fail_on_proven_exits_zero_when_the_diff_reaches_every_site() {
    let repo = rust_function_repo();
    rename_area(&repo, true);

    let output = run_check(&repo.root, &["check", "--fail-on-proven"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains("src/second.rs:4"), "{text}");
}

#[test]
fn check_with_fail_on_proven_exits_one_on_an_error() {
    let repo = rust_function_repo();
    let output = run_check(
        &repo.root,
        &["check", "--fail-on-proven", "--base", "nonexistent"],
    );
    assert_eq!(output.status.code(), Some(1));
}
